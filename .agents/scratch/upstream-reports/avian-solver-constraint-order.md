# avian3d 0.7: solver constraint order derives from entity index, not a spatial key — cross-World non-determinism

**Target:** github.com/Jondolf/avian · avian3d 0.7 · **Severity for us:** LOW *current impact* (bounded, self-healing, absorbed by server authority) but **HIGHEST strategic** — this is the single remaining same-machine obstacle to fully deterministic forward sim, i.e. the enabler for **predict-both-with-rollback under our existing server-authoritative, state-re-anchored architecture** (NOT a move to an input-only / lockstep wire — that architecture stays rejected, see ADR-0015 and the design doc §4.1/§4.4). · **Status:** unfiled
**Lineage:** the unresolved SOLVER-side tail of issues #406 / PR #480 ("make contacts deterministic across Worlds" — the broad phase was changed to sort by spatial key instead of `Entity` id; the constraint/solver graph was not).

## Suggested title

Contact solver accumulation order depends on Entity index (graph-color assignment), breaking cross-World determinism for multi-manifold bodies

## Mechanism (verified in avian3d 0.7 source)

A single dynamic body carrying **≥2 simultaneous contact manifolds** has its manifolds distributed
across solver graph **colors** by a greedy search keyed on the body's **entity index**:
`color.body_set.get(body1.index_u32())` / `body2.index_u32()`
(`dynamics/solver/constraint_graph.rs:184-191`). Graph coloring forbids a body appearing twice in
one color, so that body's multiple manifolds are forced into **different colors**.

Colors are then applied in a **serial, color-index-ordered** outer loop:
`for color in constraint_graph.colors.iter_mut().take(COLOR_OVERFLOW_INDEX)`
(`dynamics/solver/plugin.rs:557-561`). The `par_for_each` at line 563 only parallelizes *disjoint
bodies within a single color* — it never splits one body's accumulation across threads, so the
`parallel` feature has no bearing here (confirmed empirically below).

The per-color application accumulates onto the body:
`angular_velocity += inverse_inertia * cross(r, impulse)`
(`dynamics/solver/contact/mod.rs:314/317` normal, `349/352` friction, `259/262` warm-start,
`402/405` restitution).

Manifold→edge insertion order is ascending `contact_id` = graph edge-insertion order
(`collision/narrow_phase/system_param.rs:140-147`). Both the color assignment (entity index) and
the insertion order therefore derive from **entity identity**, not geometry. Two ECS Worlds with
different spawn histories have different Entity indices for the same logical body (measured:
server tank `4294966669` vs client tank `4294966650`), so the same set of manifolds accumulates
onto `angular_velocity` in a **different color order**.

**Effect-size framing (corrected 2026-07-11):** the driver is stronger than float-sum
non-associativity. Colors are solved as sequential-impulse (Gauss-Seidel) passes — reordering
colors changes **which impulses are computed against already-updated velocities**, so the
divergence is order-1 immediately, not ULP-scale: the candidate fix's RED test measures the
same wedge diverging to angular velocities `(−0.52, −1.33, 0.82)` vs `(1.61, −0.18, −3.54)` at
tick 1 of settled multi-manifold contact. Float non-associativity is the additional,
smaller term.

## Measured (this game, 64 Hz, commit a79d50f)

- **Same-World is bit-exact**: two separate *server* processes, identical scripted input, over a
  140-tick multi-contact wedge-settling window (angular velocity evolving through 134 distinct
  values) → **140/140 = 100% bit-identical** (position, rotation, lin/ang velocity). Rules out
  thread nondeterminism and confirms no sim-affecting HashMap iteration (Rust per-process
  `RandomState` would otherwise break run-to-run identity).
- **Cross-World diverges, angular-velocity first**: client vs server (different Worlds), same run,
  identical inputs (Δthrottle/Δsteer = 0), diverges from the first settled contact tick, ordered
  `|Δav| 0.155 ≫ |Δlv| 0.100 ≫ |Δp| 0.0013 ≫ |Δq| 5e-4 rad`. Bounded (≤ ~9 cm) and self-healing.
- **`parallel` off does not change it**: rebuilt `default-features = false` minus `parallel`,
  same divergence (|Δav| 0.154 vs 0.155) — structural, not a threading artifact.
- **`enhanced-determinism` does not touch it**: that feature only routes transcendentals through
  libm (`physics_transform/transform.rs:242-245`, `291-293`); it changes nothing about color
  assignment or accumulation order.

Angular velocity moves first because each contact contributes `I⁻¹·(r × impulse)` with a distinct
lever arm `r` per point, so reordering the sum shifts the angular result most; the linear channel
scales one impulse by scalar `inv_mass` and point contributions largely cancel. Only a body with a
persistent multi-manifold contact set triggers it — in this game the wedged-on-a-slab-edge hull
(`hc`=2). Normal driving never does: wheels are external raycast forces (not solver constraints)
and a rolling hull carries at most a single, transient manifold (whose own points reduce in
parry's geometry-derived, world-independent order).

## Suggested upstream fix

Make the constraint solve order canonical and geometry-derived rather than entity-derived: sort
each color's `contact_constraints` (or the manifold→color assignment) by a stable **spatial key**
(contact world position, then normal), the same remedy PR #480 applied to the broad phase. That
makes the accumulation order identical across Worlds with identical geometry, closing cross-World
(and, with it, a prerequisite for cross-platform) determinism for multi-manifold bodies. Relates
to the cache-free-solver discussion (#734) but is independent and much smaller.

## Candidate fix status (adversarially reviewed 2026-07-11, branch `fix/solver-constraint-order`)

Patch exists and is sound: canonical coloring by lexicographic-min world contact point then
normal (`total_cmp`), rebuilt on a topology-dirty flag, parallel body-disjointness invariant
re-established via a separate canonical body set; warm-start keyed by `contact_id` (immune to
reorder); new test = two Worlds with different spawn histories, ≥2 manifolds on both bodies for
≥100/180 ticks, full-state equality every tick (passes at HEAD; RED fails with the order-1
divergence quoted above). Suite 68 passed + 1 pre-existing sub-ULP ARM rasterizer failure
(confirmed at the v0.7.0 tag).

**MUST-FIX before filing:** the 2D cross-platform determinism hash constant was recalibrated
against the default f32 build (`0x3126af7d`) but avian CI runs that test under
`f64 + enhanced-determinism + xpbd_joints + …` — measured under the exact CI feature set the
patch produces `0x4fa858dc` and the test FAILS as submitted. Re-derive under CI features and
confirm from the PR's own CI run. The hash change itself must be called out in the PR: the
patch changes canonical results for all users, not just cross-World cases.

**Disclose in PR text:** (a) bit-equal spatial-key ties fall back to entity order (stable sort)
— coincident-geometry manifolds could still diverge, degenerate; (b) the dirty flag is global,
so busy scenes re-sort near-every step — perf claims (+0.37% whole-step) are the author's, not
independently verified; (c) `ContactManifoldSortKey::new` panics on a zero-point manifold where
the old path tolerated it (believed unreachable); (d) semver: `GraphColor`/`ConstraintGraph`
gain private fields (breaks struct-literal construction downstream).

**Game-level evidence (this repo's divergence instrument, 2026-07-11):** flat-cruise long
course with the patch: physics bit-exact on all 1262 shared ticks (no regression, matches
unpatched). The live-network instrument cannot re-measure the wedge signature (connect
transient seeds pre-window state deltas; rollback starvation prevents re-anchoring), and the
short-course "class-3" persistent windows are NOT this defect (identical 0.230 mm fire-edge
seed on patched and unpatched builds, 2/12 vs 1/12) — the cross-World proof is the crate test.

## Our workaround + removal condition

**None, deliberately.** The divergence is bounded, self-healing, and fully absorbed by the
server-authoritative + prediction architecture (it manifests only as occasional tiny corrections
in a pathological wedged state; normal play is bit-exact). We do not work around it.

Its importance is strategic: it is the **last same-machine non-determinism source**, and the exact
obstacle between us and deterministic forward sim. **What it enables is predict-both-with-rollback
under the architecture we already run** — server-authoritative, state replicated and kept as the
re-anchor and divergence detector. It does **not** require moving to an input-only / lockstep wire
(that stays rejected — the slowest peer would gate everyone and one divergence would desync
permanently with no authority to re-anchor; see ADR-0015 and the design doc §4.4). If this ships
upstream (plus `enhanced-determinism` for the independent transcendental axis), the divergence
error class collapses and predicting non-owned tanks becomes viable without a wire-model change.
Track it as that enabler, not a bug to patch locally.

**A reader must not come away thinking parallelism is the risk — it is the safe part.** The
parallel dynamics step is order-invariant **by construction**: greedy edge-colouring gives each
body at most one edge per colour, so the `par_for_each` within a colour writes to disjoint bodies
(`plugin.rs:557-561`). Measured: rebuilding without the `parallel` feature does not change the
divergence (|Δav| 0.154 vs 0.155, `:22`/`:47-48` above). The live nondeterminism is entirely the
**serial, entity-index-keyed colouring order** (`constraint_graph.rs:184-191`, applied serially by
colour index in `plugin.rs:557-561`) — a per-body property of *which* colour a manifold lands in,
not of how colours are executed. The upstream fix is to make that assignment geometry-derived
(spatial key), not to touch threading.

## What fixing this unlocks for us

Nothing gets **deleted** — we shipped no workaround. Everything here is capability.

**Explore — predict-everyone, and with it tank-tank collision that a player can actually feel.**
Non-owned tanks are `Interpolated` today, so a client's own hull lives at the predicted tick `P` and an
opponent's collider at the interpolation tick `I`: a local ram is sheared by ~RTT/2 + interpolation delay
(`design/timelines-and-shear.md` §3), and contact is therefore resolved server-side
([[adr/0017-mutual-contact-resolves-on-the-authority]]). ADR-0017 does not reject predicting opponents —
it **defers** it, and names *this defect* as the revisit condition, verbatim: *"Revisit this ADR when — and
only when — avian's entity-index-keyed constraint ordering is fixed."* The reason is stated there too:
with a non-deterministic contact solver, two predicted bodies both mispredict and each feeds the other's
error through the one part of the sim that **expands** perturbations rather than damping them. Fixing the
ordering collapses the *divergence* error class (same state + same inputs → same result across Worlds) and
leaves only irreducible *misprediction* — which is what makes predicting a second body a bounded problem
instead of a mutual-amplification one.

**Optimize — the threshold ratchet.** `ROLLBACK_POSITION_M` / `ROLLBACK_ROTATION_RAD` are pinned at 0.05
(`net/protocol.rs:1038-1040`) — **five times coarser** than the reference 1 cm / 0.01 rad, because solver
noise on a 16-contact 57 t rig trips the reference bar far more often than genuine misprediction (measured:
~430 rollbacks/15 s at 100 ms latency). ADR-0015 marks these explicitly as *"a ratchet, not a setting: as
the divergence they absorb collapses (contact-restore fix, upstream constraint ordering), tighten them
toward the 1 cm / 0.01 rad reference values."* This report is the "upstream constraint ordering" half of
that sentence: it is the remaining source of the contact-adjacent divergence (ADR-0015's ranked cause #1;
flat-ground cruise is already bit-exact). A tighter bar means genuine desync is caught centimetres earlier.

**Explore — the divergence instrument's remaining open class.** The class-3 persistent divergence windows
in the client/server hash comparison are still unattributed; the candidate patch's game-level run shows the
short-course windows are **NOT** this defect (identical 0.230 mm fire-edge seed patched and unpatched), so
determinism does not close that tab by itself — but it removes the one *known* same-machine source, which
is what makes the residual attributable at all.

**Blocked-by / honest limits — determinism is necessary, not sufficient.**

- **Predict-everyone needs more than this fix.** ADR-0017 names two further blockers, neither upstream:
  the bot has no `ControlledBy` and no client authoring its input, so nothing rebroadcasts for it and a
  predicted bot would coast on a default command (so this is really *predict-every-**player***, a mixed
  mode); and reliable remote fire needs input-side rollback, which targets ticks not gated by state
  confirmation and would break the tick-agnosticism of the crew/belt appliers (`net/protocol.rs`).
- **No wire savings.** An earlier note claimed `ServoAngles`/`FireEvent` become deletable under
  predict-everyone. Both retractions are on the record and both stand: ADR-0016 — *"`ServoAngles` … becomes
  derivable exactly when remote inputs are — and the bot, having no client to author its input, would still
  need it"*; ADR-0021 — `FireEvent` must **survive** predict-everyone as the starvation fallback (a predicted
  remote tank that fails its fire level closed under input starvation fires nothing locally, while the
  server, which had the input, does). The payoff here is gameplay and divergence collapse, not bandwidth.
- **Forward determinism ≠ replay determinism.** This fix buys *forward* determinism (same state + same
  inputs → same result, across Worlds). Prediction + rollback also wants *replay* determinism (restore +
  resimulate lands bit-identically on the forward path), which no engine sells today — avian #734 is the
  open thread (ADR-0015). Cross-platform is a further axis again (transcendentals; `enhanced-determinism`
  routes those through libm and does nothing for the ordering, see above).
- Deeper prediction windows (the 0-tick / adaptive input-delay experiments in
  [lightyear-check-starvation.md](lightyear-check-starvation.md)) get their *chaos* bill from replay
  amplification through friction/contact — measured 5.6× median, 43× p90. Determinism is what makes a
  deep replay cheap and boring. So this report is a soft prerequisite for those experiments too, though it
  does not gate them.
