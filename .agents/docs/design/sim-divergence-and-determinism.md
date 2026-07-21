# Sim divergence & the determinism landscape (bevy / avian / lightyear)

2026-07-04. Written after the step-8 rollback investigations, before the latency feel test
(slices 2/3). Two halves: what we now *know empirically* about our own client/server divergence,
and what the stack currently offers for cross-platform determinism. Sources: this repo's spike
log (measured), vendored crate sources (verified, cited by file:line), and web material (flagged).
**Updated 2026-07-06** after the architecture-review session: stale claims corrected in place
(marked "(corrected 2026-07-06: …)"), new measured rows in §2, and §5 added with the
solo-divergence model and the two-layer doctrine — whose canonical home is
[ADR-0015](../adr/0015-divergence-doctrine.md). **Updated 2026-07-09:** §5's "dominant term"
(the hc=0 contact-restore claim) re-measured after the shield and retired — see §6 (the
post-shield measurement) and §7 (a lat0 connect-hang open finding). **Updated 2026-07-10:** §8
added — the divergence instrument (per-tick world-independent state hash + offline join) and its
measured baseline: physics state bit-exact on every shared tick of both harness runs, residual
divergence entirely in carried mechanism state (`hsim`). **Updated 2026-07-21:** §11 closes the
cross-architecture SIMD-reduction class with the validated scalar-math bitprobe pair and records
the current divergence inventory.

## 1. Why two runs of "the same sim" diverge at all

IEEE 754 is often blamed too broadly. The precise picture:

- **Basic float ops are exactly specified.** `+ - * / sqrt` on f32 give bit-identical results on
  every IEEE-conforming CPU, same rounding mode. A single-threaded sim using only these, in the
  same order, is bit-deterministic across x86/ARM.
- **What actually breaks it:**
  1. **Transcendentals** (`sin`, `cos`, `atan2`, `powf`…) are NOT specified by IEEE — each
     platform's libm returns different ulps. This is the gap avian's `enhanced-determinism`
     closes (swap system libm for the Rust `libm` crate everywhere: avian, bevy_math,
     bevy_heavy, parry — avian3d-0.7.0/Cargo.toml:73-78).
  2. **FMA and SIMD codegen.** Rust does *not* auto-contract `a*b+c` into FMA (unlike C with
     `-ffast-math`), which removes the classic C++ pitfall — but explicit SIMD paths (glam's)
     can differ by target feature level (SSE2 vs NEON vs scalar fallback), and `mul_add` calls
     are FMA-or-not depending on target.
  3. **Reduction/iteration order.** Float addition isn't associative. Any parallel sum, any
     hash-ordered ECS query iteration, any archetype-order-dependent loop makes results depend
     on entity spawn history and thread scheduling — *even on one machine*.
  4. **Engine-internal ordering.** Solver constraint order, broad-phase pair order, island
     splitting, sleeping heuristics.

## 2. What we measured in Overmatch (same machine, same binary!)

The step-8 investigations give us an unusually concrete divergence taxonomy — all of it at
zero latency, macOS-vs-macOS, identical binaries:

| Source | Magnitude | Status |
|---|---|---|
| Unanchored `propagate_collider_transforms` (schedule race after disabling `PhysicsTransformPlugin`) | ~90% of smooth-ground rollbacks (~150 → ~10/run at 100 ms), plus the bind-window NaN crashes | **FIXED** (ccbe7fc) — was a bug, not float divergence |
| Render-space (`GlobalTransform`) reads in sim systems | small but systematic, worst under high yaw/pitch rates | **FIXED** (ca5e380) — sim reads physics `Position`/`Rotation` |
| Contact transients on rough terrain (washboard/bump) | 20–60 velocity-threshold trips/s at 0.05 m/s; ~135/20 s run even post-fixes | **Irreducible class** — managed by thresholds (`ROLLBACK_VELOCITY` 0.20) |
| Everything else (flat ground, driving, slewing, firing) | ~10 rollbacks/run at 100 ms, ~0 at rest | Healthy noise floor |
| Check starvation: lightyear's receive-time mismatch check skipped at zero prediction margin and never retried (see §5) | 35–50 m divergence with fresh authority arriving every tick and **zero rollbacks**; 3,296 skip-trace lines in one run | **FIXED** (8ae795c, `net/watchdog.rs` backstop) — added 2026-07-06 |
| Rollback contact-restore defect: hull contact fails to re-form on the first replayed tick, seeding mm-scale error that re-trips the 0.05 m bar | hc=0 on 55% of replayed ticks at 80/10 (98.4% at lat0); Δlv exactly −g·dt = 0.1533 m/s at k=1 while pose restore is near-exact (\|Δp\| p50 1.5 mm); contact re-forms at k=1 in 62/69 cases where the client's abandoned timeline still had it vs fails in 80/85 where it didn't | **FIXED 2026-07-06** — `AuthoredLocalTransform` + `shield_authored_collider_transform` (src/tank.rs), ADR-0015 Layer-2, upstream report candidate #3. (superseded 2026-07-06, same-day probe verdict: the abandoned-timeline restore mechanism confirmed that morning — collision state restored to the mispredicted timeline, child colliders not restored at all — is real but BENIGN: with `SPIKE_CONTACT_PROBE` the BVH leaves, moved-proxy set and contact pairs all self-heal within a tick once poses are honest. The actual killer is ATTACHMENT POISONING: lightyear_avian's `AvianReplicationMode::Position` registers `ApplyPosToTransform` as a required component of `Position`/`Rotation` (lightyear_avian3d-0.28.0 plugin.rs:620-623), dragging the tank's child colliders — which carry `Position`/`Rotation` as collider required components — into avian's `position_to_transform` write set (avian3d-0.7.0 physics_transform/mod.rs:254-257, mounted PreUpdate + PostUpdate by lightyear_avian). That system rewrites each proxy's LOCAL `Transform` as its sim-world `Position` `reparented_to` the parent bone's `GlobalTransform`, which is render-blended (FrameInterpolation/VisualCorrection/render-error offset) and one `Propagate` stale — render state leaking into sim, the ADR-0014 leak class, introduced upstream. Each frame deposits the sim-vs-render difference into the authored attachment; `propagate_collider_transforms` folds it into `ColliderTransform` and the pose ratchets (measured: proxies constant to 0.1 mm in healthy runs; 2–13 cm/tick during rollback storms, hull proxy reaching 2.8 m above the root — the sustained hc=0-while-"penetrating" windows were the proxy genuinely elsewhere, not a broad-phase defect, retiring the zombie-pair suspicion). Fix: strip `ApplyPosToTransform` from authored child colliders via an `On<Add>` observer, identical on both ends — the deposit is render-sized on the client but exists server-side too via the stale-`Propagate` term.) Post-shield re-measured 2026-07-09 (§6): retired — the discriminating metric (client hc=0 while server hc>0) is 0/88 replayed ticks at 80/10, and the hc=0 percentages in this row are a poison indicator, not a contact-restore failure rate. |
| In-contact replay load chaos (friction/load through the replay, a separate machine from the row above — anti-correlated with the hc-loss signature) | per-wheel load deltas to 5.8e6 N; the multi-meter replay errors | **OPEN** — absorbed by the Layer-2 thresholds (ADR-0015) — added 2026-07-06 |
| Sphere-cast TOI noise vs large colliders: parry's GJK shape-cast converges on a *relative* tolerance (`eps_rel ≈ 1.09e-3`, parry3d-0.27.0 gjk.rs:661-780), so the sphere probe's `hit.distance` against the 1000 m ground slab came up one-sided up to ~172 mm SHORT — deterministic but pose-discontinuous. A standing divergence AMPLIFIER (mm-scale pose differences between the two ends → 10–40 kN per-wheel force differences through the 551 kN/m spring) and the at-rest limit-cycle pump (~2.2 kW measured, sustaining the ~12 mm / 0.29° hull wobble = the gunner-sight shake on flat ground) | one-sided distance error scales with collider extent: 0.25 mm at 5 m half-extent vs 139–172 mm at 500 m; 10–40 kN per-wheel force noise per tick at rest | **FIXED 2026-07-06** — witness-geometry reconstruction (`sphere_cast_ground_contact`, src/driving.rs): the same hit's `point1`/`normal1` are exact even when the TOI is wrong (measured `point1.y = 0.000000` throughout), so the travel is recomputed from them — GUARDED (same-day hardening): the reconstruction is trusted only for non-penetrating casts with a finite witness, clamped to `[toi_based, toi_based + 0.20]` (0.20 m = the worst live-measured one-sided short error, 199.8 mm; parry's TOI is a lower bound, so the band caps any witness pathology at the old error scale); penetrating starts (parry swaps the witness for a penetration contact whose normal is unrelated to the cast axis) and non-finite witnesses fall back to the old conservative TOI path. `tests/spherecast_scale.rs` pins the helper's math (reconstruction, band, fallbacks) and parry's TOI defect (the workaround-retirement tripwire); it does not bind the `apply_suspension` call site (a thin adapter over the helper) — that wiring's live guard is the idle at-rest harness metric (p.y spread ≲ 0.02 mm). Upstream report candidate #4: parry GJK shape-cast relative tolerance vs large shapes |

Lesson: **most of our "divergence" so far was bugs, not physics.** The genuinely irreducible
part is contact-transient noise, and it is exactly what rollback + per-component thresholds are
designed to absorb. Second lesson: every one of these was found by measuring, not by reasoning —
the same discipline applies to anything below.

Why "same machine" still diverges at all: avian ships with the `parallel` feature **on by
default** (avian3d-0.7.0/Cargo.toml:57-63) and we run defaults — solver work is threaded, so
constraint application order varies run-to-run and process-to-process. Plus ECS iteration order
differs between client and server worlds (different entity histories), feeding order-dependent
float sums. This is why bit-exactness was never on the table for the current architecture.

*(corrected 2026-07-06: the "parallel = nondeterministic solver order" half of that paragraph is
stale for avian 0.7. The **parallelism** is the safe part: the parallel dynamics step is
order-invariant same-machine **by construction** — greedy edge-colouring gives each body one edge
per colour, so `par_for_each` writes are disjoint (`plugin.rs:557-561`), Vec-backed contact
storage; upstream PRs #712/#807 made parallel constraint generation deterministic, and avian CI
enforces cross-platform bit-identity WITH parallel enabled (2D-only test; 3D plausible but
unverified). **Do not read "parallel is safe" as "ordering is solved":** the actual residual
nondeterminism is the **serial, entity-index-keyed colouring order** — which colour a
multi-manifold body's manifolds land in is keyed on `body.index_u32()`
(`constraint_graph.rs:184-191`) and applied in a serial colour-index loop (`plugin.rs:557-561`), so
two ECS Worlds with different entity indices accumulate the same manifolds in a different colour
order (measured, parallel-off changes nothing: |Δav| 0.154 vs 0.155). Full mechanism and
measurement: `.agents/scratch/upstream-reports/avian-solver-constraint-order.md`. And bit-exactness IS on the
table where it matters: flat-ground cruise measured bit-exact client-vs-server over ~880-tick
windows, all fields. The two REAL divergence mechanisms are (1) same-machine replay divergence
from BVH refit topology across rollback restore — lightyear_avian restores the tree from
EnlargedAabbs but refit keeps rollback-time topology → pair-discovery order → coloring
differences at contact transients — and (2) cross-machine entity-index-keyed constraint coloring
between the two ECS worlds, irreducible by config, needs upstream canonical ordering. The ECS
iteration-order sentence stands for gameplay code.)*

## 3. What the stack offers today

- **avian `enhanced-determinism`** (off by default, off for us): libm math everywhere for
  cross-*platform* consistency of transcendentals; docs claim "improving determinism across
  architectures at a small performance cost" (avian3d-0.7.0/src/lib.rs:66; third-party material
  quotes 10–30% — unverified). It does NOT fix parallel/iteration order: for strict determinism
  you must also disable `parallel` (single-threaded solver) and take care that gameplay code
  iterates in a stable order. So "deterministic avian" = libm + single-thread + disciplined
  queries — a meaningful perf and ergonomics bill. *(corrected 2026-07-06: this is a
  cross-ARCHITECTURE dial only — libm transcendentals — irrelevant same-arch; and the "must also
  disable `parallel`" clause is stale, since avian 0.7's parallel dynamics step is order-invariant
  by construction — see the §2 correction.)*
- **lightyear `deterministic_replication`** (we already vendor it): lockstep-style, inputs-only
  replication for entities marked `Deterministic` — "not updated via state, but only via inputs"
  (lightyear_deterministic_replication-0.28.0/src/lib.rs:1-58). This is the architecture that
  *requires* the full determinism bill above. Its `rollback_resources: true` avian mode
  (contact-graph snapshotting) exists for this world, not ours.
- **lightyear state replication + prediction** (what we run): requires only *bounded* divergence
  — the authority continuously re-anchors clients; thresholds define the tolerance band. No
  determinism requirement at all, at the cost of bandwidth (state on the wire) and rollback CPU.
- **bevy core**: no determinism guarantees; query iteration order is explicitly unstable
  (long-standing discussion, bevyengine/bevy#2480). Anything order-sensitive must sort.

**The distinction this section originally lacked (added 2026-07-06): FORWARD vs REPLAY
determinism.** *Forward* determinism — same state + same inputs → same result — is what lockstep
needs, and what Box2D v3 / Box3D actually ship. *Replay* determinism — restore a snapshot,
resimulate, and land bit-identically on the original forward path — is what prediction + rollback
needs, and **no engine sells it today**: restore paths don't reproduce internal solver state
(contact graphs, warm-start impulses, broad-phase topology), so the replay walks a different
constraint order even when the forward sim is perfectly deterministic. avian issue #734 is the
open upstream thread; lightyear's `rollback_resources: true` avian mode (contact-graph
snapshotting, mentioned above) is the ecosystem's closest existing thing. Our own dominant
divergence term (§2's contact-restore row) is precisely a replay-determinism failure, not a
forward one. *(superseded 2026-07-06: the probe reclassified that row — the dominant term was
attachment poisoning, a render→sim leak (a bug, upstream), not a replay-determinism failure;
the replay-determinism framing stands for avian #734 itself, no longer for our dominant term.
Further superseded 2026-07-09: post-shield re-measurement (§6) retires that "dominant term"
outright — the hc=0 metric never discriminated a restore failure from wheel-borne/airborne
contact-free ticks, so it was never evidence of a replay-determinism failure in the first place.)*

## 4. What this means for Overmatch

1. **Our architecture choice bounds the damage — it does not sidestep the problem.**
   *(reframed 2026-07-06; the original heading was "already sidesteps the hard problem", which is
   wrong in spirit.)* State replication + prediction means the macOS-client-vs-Linux-server float
   gap (libm transcendentals, NEON vs AVX) shows up as a *higher background rollback rate*, not
   as desyncs or wrongness — the server is always right, clients converge by construction. That
   part stands. But within state replication, **determinism is the ROLLBACK-KILLER**: every bit
   of forward- and replay-determinism we gain makes corrections rarer, and rollbacks in a solo
   game are a defect indicator, target ~zero (ADR-0015). The Rocket League precedent is exactly
   this shape — server-authoritative + prediction, determinism pursued as the optimization that
   makes corrections rare. Divergence work is the *active agenda*, not a rejected alternative's
   leftover concern. The cloud feel test (Edgegap, first outing) is still also our cross-platform
   divergence measurement: compare `PredictionDiagnostics` rates against the same-machine
   baselines recorded in the spike log (~10/run smooth ground at 100 ms, ~135/20 s washboard).
2. **`enhanced-determinism` is a cheap dial we can turn if cross-platform rates disappoint.**
   It narrows the client/server transcendental gap even without full lockstep discipline —
   worth an A/B on the cloud test if rates are much worse than local. Perf cost lands on both
   sim ends; measure before adopting.
3. **Disabling `parallel` is NOT recommended** for us: it buys same-machine order stability we
   don't need (thresholds already absorb it) and costs solver throughput the 10v10 aspiration
   will want. *(corrected 2026-07-06: moot on both counts — parallel isn't the divergence source
   (§2 correction), and the throughput argument was miscast: the tank is ONE rigid body — the
   wheels are external forces, not solver constraints — so even 10v10 is ~20 bodies, trivial
   solver load.)*
4. **An inputs-only wire stays rejected — determinism-the-property does not.** *(rewritten
   2026-07-09; the original bullet conflated the two, rejecting determinism-the-property with a
   lockstep-specific bandwidth argument — it contradicted §4.1 above, which correctly calls
   determinism "the ROLLBACK-KILLER".)* These are orthogonal choices:
   - **Rejected: lightyear's `deterministic_replication` / an inputs-only wire** (§3). The slowest
     peer gates everyone, and one divergence desyncs permanently with no authority to re-anchor.
     16-wheel contact-rich physics per tank × cross-platform × Rust ecosystem's current maturity
     makes it the expensive path, and its bandwidth win doesn't matter at 1v1–3v3. Revisit only if
     state bandwidth ever becomes the binding constraint (10v10 on thin pipes).
   - **Pursued: determinism-the-property, under server authority, as the rollback-killer** (§4.1,
     ADR-0015:56-60). Target quadrant is deterministic + server-authoritative + **state kept as
     the re-anchor and divergence detector** (the Rocket League / Photon Quantum shape), NOT
     input-only lockstep. Every bit of forward- and replay-determinism gained makes corrections
     rarer without touching the wire model or dropping the re-anchor.
   - **What determinism buys, precisely.** It does *not* make *prediction* more accurate —
     prediction is bounded by information (you cannot know a remote player's next input).
     Determinism eliminates the **divergence** error class (same inputs, different results); it
     cannot touch the **misprediction** class (unknown remote inputs). Those are separate budgets.
5. **Keep the divergence budget honest with the tools built this session:** `SPIKE_SIM_LONG` +
   `SPIKE_SIM_WINDOWED` + `SPIKE_SIM_AIM_SWEEP` reproduce the workload classes; `nan_tripwire`
   names corruption; per-component thresholds are the tuning surface (velocity loose, position/
   rotation tight — discrete/gameplay-binary state must never be threshold-banded).

Open thread tracked in the spike log: the residual ~25%→2/8 bind-window crash window
(`update_ray_caster_positions` NaN under the bind-burst rollbacks; suspect DisableRollback-grace
inconsistency during replay) — pre-feel-test fix candidate. Also: the set-configuration hole
(`PhysicsTransformSystems::Propagate` orphaned when `PhysicsTransformPlugin` is disabled) looks
like an upstream lightyear_avian gap worth reporting.

## 5. 2026-07-06: the solo-divergence model, check starvation, and the two-layer doctrine

Architecture-review session, all measured, none conjectured. The doctrine's canonical home is
[ADR-0015](../adr/0015-divergence-doctrine.md); this section is the measured record.

**The solo-divergence model.** With one player (client vs a static world) there is nothing to
mispredict: client and server *should agree completely*, and **rollbacks in a solo game are a
defect indicator, target ~zero**. Measured divergence causes, ranked:

1. **Contact-adjacent solver noise across two ECS worlds.** Flat-ground cruise IS bit-exact
   client-vs-server (measured over ~880-tick windows, all fields). Divergence exists only at
   contact transients, where entity-index-keyed constraint ordering differs between the worlds
   and contact chaos amplifies last-bit float differences. Irreducible by config; needs upstream
   canonical ordering.
2. **THE DOMINANT TERM — RETIRED (2026-07-09). The correction machinery *appeared* to manufacture
   its own divergence.** As recorded 2026-07-06: "hull contact fails to re-form on the first
   replayed tick: hc=0 on 55% of replayed ticks at 80/10, 98.4% at lat0, with Δlv exactly −g·dt =
   0.1533 m/s vertical at k=1 while pose restore is near-exact (|Δp| p50 1.5 mm)", read as a
   self-feeding engine in hull-contact states, felt as "the hull-stuck tank never settles".
   *(superseded 2026-07-06 by the `SPIKE_CONTACT_PROBE` reclassification (8a08d60), retired by the
   `AuthoredLocalTransform` shield (33cc4e4), re-measured post-shield 2026-07-09 — see §6.)* Two
   things were wrong with the original reading. (a) The mechanism was attachment poisoning, not a
   restore defect — child-collider proxies levitating up to 2.8 m above the root, so hc=0 was
   avian being honest about a collider that had left (§2's contact-restore row). (b) The metric
   itself never measured contact re-formation: hc=0-among-replayed-ticks conflates "no hull
   contact because the tank rides on its wheels or is airborne" (physically correct, and the
   common case) with "contact failed to re-form after restore" — so the 98.4%/55% are a POISON
   INDICATOR, not a contact-restore failure rate, and are evidence for neither direction. The
   multi-meter replay errors are a SEPARATE machine: in-contact friction/load chaos through the
   replay (per-wheel load deltas to 5.8e6 N), anti-correlated with the hc-loss signature, still
   absorbed by the Layer-2 thresholds.
3. **Input-timing slips under jitter** — rare. Trigger attribution ~93% Position; the cause is
   state, not input.

**The check-starvation bug (FIXED this branch, 8ae795c).** lightyear 0.28: for an entity
confirmed every tick, the only rollback-mismatch detector runs at receive time, gated strictly on
`confirmed_tick < current_tick` (lightyear_prediction-0.28.0/src/registry.rs:426-428); skipped
checks are never retried, and the `check_rollback` unchanged-entity scan excludes always-confirmed
entities (rollback.rs:583). `InputDelayConfig::balanced()` at LAN/loopback RTT absorbs all latency
into input delay → zero prediction margin → the client sits level with the server → every update
skips → state rollback permanently, silently dead. Measured: 35–50 m divergence with fresh
authority arriving and **zero rollbacks**; 3,296 skip-trace lines in one run; the falsifier
`SPIKE_INPUT_DELAY_TICKS=0` capped divergence at 0.015–0.57 m. Fixed by `src/net/watchdog.rs`
(forced-rollback backstop). **Methodology consequence:** lat0 rollback counts from pre-watchdog
builds measured check starvation, not convergence — invalid as an A/B metric. lat0 |Δp| tick-row
divergence remains valid.

**The two-layer doctrine** (summary — ADR-0015 is canonical). **Layer 1, permanent, ours:**
contact and force laws must be continuous functions of pose/velocity (divergence continuity) —
shipped applications: sphere-cast suspension (washboard rollbacks −73%), friction static↔kinetic
blend + LuGre anchor relax (wedge storm 44+ with deterministic runaway → 1 in the good regime).
Binding for all future force laws, the track model included (sharp oriented box casts are the
known bug class — rounded shapes or ray/sphere stations). **Layer 2, deliberately removable, each
piece mapped to an upstream defect:** the watchdog ↔ lightyear's skipped-check-never-retried; the
contact-restore fix (landed 2026-07-06, `AuthoredLocalTransform` shield) ↔ lightyear_avian's
blanket `ApplyPosToTransform` on child colliders (superseded 2026-07-06: was "restore path /
avian #734" — the probe repinned the mechanism, see §2); the
coarse thresholds + desync-only velocity bars ↔ the divergence those defects manufacture (tighten
toward the 1 cm reference values as divergence collapses). **Permanent-but-looks-like-
scaffolding:** the render-space error layer (`net/render_error.rs`) — multiplayer reintroduces
legitimate mispredictions forever, and the layer is how ANY correction is presented; it stays.
Strategy: ship the scaffold now (upstream timelines are not ours; #734 open since May 2025), file
upstream reports, keep workarounds small and documented-removable, let the continuity work
compound.

Web sources (background, treat as secondary): [DeepWiki avian determinism](https://deepwiki.com/avianphysics/avian/10.3-determinism) (machine-generated), [avian repo](https://github.com/avianphysics/avian), [bevy determinism discussion #2480](https://github.com/bevyengine/bevy/discussions/2480).

## 6. 2026-07-09: post-shield contact-restore re-measurement

The §2 contact-restore row and §5's ranked term #2 both carried the pre-shield hc=0 numbers
(98.4% at lat0, 55% at 80/10), captured 2026-07-06 **~1 h before** the `AuthoredLocalTransform`
shield landed (33cc4e4). Nobody had re-measured after the shield. This section is that
measurement. Binary built from `src/` as of 0fa6cd8 — the last commit touching it; the commits
after it are docs and editor config (shield 33cc4e4 and probe reclassification 8a08d60
both in). Same harness as the original: server `SPIKE_PERTURB=0`, client `SPIKE_SIM_LONG=1
SPIKE_SIMULATE_INPUT=1` (the ~20 s dead-straight course crossing the bump z≈−70 and washboard
z≈−82…−90), each of `SPIKE_LATENCY_MS=0` and `SPIKE_LATENCY_MS=80 SPIKE_JITTER_MS=10`. Metric
identical to the original: fraction of client `tick` rows with `rp=true` where `hc==0` — the
`SPIKE_TRACE` schema's own fields, no instrumentation added. `NAN-TRIPWIRE|FIXED-NAN|panicked|
B0004` all 0 across every client and server log.

**The raw rate did not fall. Read this as retirement of the metric, not of a number.**

| Condition | pre-shield (poison) | post-shield raw hc=0 | replayed-tick n |
|---|---|---|---|
| lat0 | 98.4% | 50% and 75% (2 runs); pooled 5/8 = **62.5%** | 4 per run, **8 pooled** |
| 80/10 | 55% | 100% every run (6/6); pooled 88/88 = **100%** | 14–21 per run, **88 pooled** |

The 80/10 rate went *up* (55% → 100%). That is not a regression, because the metric conflates
wheel-borne/airborne hc=0 (physically correct) with a restore failure: a higher number here means
only that more of the few replay ticks fell in wheel-borne cruise. The load-bearing evidence is
the discriminating metric the original methodology lacked:

- **Client hc=0 while the server holds hc>0 = 0, across all 88 server-joined replay ticks at
  80/10.** On every replayed tick the server *also* held hc=0 — there was no hull contact to
  re-form and the client agreed. Contact re-forms wherever it should: the one lat0 replay tick
  with the hull genuinely grounded (gnd=16, p.y=−0.126) read hc=1.
- **Airborne/wheel-borne decomposition (lat0).** Every hc=0 replay tick is either airborne
  (gnd=0, p.y≈1.5, falling) or wheel-borne (gnd=16, hull just clear of the ground) — states where
  hc=0 is the correct reading (cf. e12a07b, "hc=0 on every wheel-borne row"). hc=1 appears exactly
  when the hull is actually on the ground.
- **Attachment healed (`SPIKE_CONTACT_PROBE`, 3505 lines, lat0).** The proxy's root-relative
  offset `cto` is constant; `leaf_dvg`/`tleaf_dvg` = 0.000000 throughout; no proxy levitation —
  the 2.8 m ratchet the poison produced is gone. The BVH-stomp and zombie-pair suspects stay
  exonerated.
- **Structural: the denominator collapsed.** Solo rollbacks are now 2–4 per 20 s run (noise
  floor), versus the pre-shield poison storm's position-rollback-every-~7-ticks that produced the
  hundreds of replayed ticks the old percentages were taken over. This is a result in itself, not
  a ratio: the machine that manufactured the replay-tick population is gone.

**Verdict: first-replayed-tick contact re-formation is retired as a defect.** No remaining
contact-restore barrier to predicting non-owned tanks surfaced in this investigation.

Limits, stated in the open:

- **The lat0 sample is thin — n=4 per run, 8 pooled (two runs).** Do not read 62.5% as a solid
  figure; it is 5 of 8 ticks. It could not be grown because of the connect hang in §7.
- **The server-join and the airborne/wheel-borne decomposition are NEW metrics, not the original
  methodology.** The original's "contact re-forms in 62/69 cases where the abandoned timeline
  still had it vs fails in 80/85 where it didn't" split is not reconstructable from the current
  trace schema — it needs the abandoned-timeline contact state, which no trace field carries. So
  these numbers are a fresh, more discriminating read that *retires* the old metric, not a
  like-for-like time series continuing the 98.4%/55%.

## 7. RESOLVED (2026-07-11): client hang at connect — decoded and guarded

**Update (2026-07-11, same session as the load-gating verification below): ROOT CAUSE DECODED,
FIX LANDED.** Two wedges caught live under CPU saturation (thread samples + RSS timelines in the
session scratchpad `connect-hang-catch/`): the main thread parks in the outer schedule's
`block_on` while one Compute-pool worker spins 20+ s inside
`lightyear_inputs::client::prepare_input_message` →
`NativeStateSequence::build_from_input_buffer`. Mechanism, all steps source-verified in the
vendored 0.28 crates:
1. A connect-window `SyncEvent` fires TWO observers (source-verified after the fix landed —
   this refines the commit message's "backward resync" framing): `receive_tick_events`
   (lightyear_inputs client.rs:1157) remaps `InputBuffer.start_tick` by `tick_delta` — so the
   timeline jump itself IS compensated — while `recompute_input_delay_on_sync`
   (lightyear_sync input.rs:52) recomputes `input_delay` from current RTT. **Nothing compensates
   the delay change**: entries sit at `old_now + old_delay`, prepare's `end_tick` becomes
   `new_now + new_delay`, and the remap cancels `tick_delta` exactly — net strand
   `= old_delay − new_delay`. Under load the early RTT samples are inflated by scheduler delay
   (delay computed high on the first sync), then ping settles and a later sync snap recomputes it
   lower → strands of 1-17 ticks observed, wedges landing "a few rollbacks in" = the SECOND snap.
2. `InputBuffer::set_raw` refuses writes below `start_tick` (input_buffer.rs:194-197), so the
   strand is **persistent** — new inputs at the lower delayed tick are silently dropped and the
   buffer never re-anchors.
3. `build_from_input_buffer` (lightyear_inputs_native input_message.rs:94) computes
   `(end_tick + 1 - buffer_start_tick) as usize` where `Tick - Tick → i32`: a strand of ≥ 2 ticks
   goes negative, sign-extends to ~2^64, and the loop `push`es one `Compressed` per iteration —
   simultaneously the silent spin AND the RSS balloon (MEASURED: 1.5 GB at wedge onset) → paging
   collapse (`UN` state) → SIGKILL (MEASURED: 40-96 s), no crash report. `num_ticks`
   (DERIVED: redundancy·1 = 5
   for us) can never make the loop large; the wrap is the only route.
Fix: `drop_stranded_input_buffer` in `src/net/client.rs` — PostUpdate,
`.before(InputSystems::PrepareInputMessage)`, drops the own-tank `NativeBuffer<TankCommand>`
whenever `start_tick` leads `current_tick + input_delay` (never true in steady state; the buffer
is rebuilt at the correct tick on the next FixedPreUpdate write; cost when firing = a few ticks
of unsent input the server hold-last extrapolates anyway). The safe tripwires in
`tests/net_input_buffer_wrap.rs` pin both enablers but deliberately do not execute the unbounded
encoder call. If either changes, inspect the upstream encoder and use a hard-capped reproduction
before retiring the guard. Proof: MEASURED loaded 24-run batch with the guard = **24/24 clean,
guard fired in 20/24 runs**
(MEASURED: strands of 1-17 ticks logged — delay-shrink sync events are ROUTINE under load; the old
3/10 hang rate was the ≥2-tick subset), vs MEASURED 4/12 wedges unguarded the same day.

**Upstream filing package (checked 2026-07-11: bug LIVE at lightyear HEAD — main's
`build_from_input_buffer` is byte-identical; no duplicate issue; #1534 "only buffer inputs after
timeline sync" is adjacent but does NOT close the delay-shrink route; #560 is the historic panic
cousin; #1559 is our other open input-buffer filing):**
(A) PRIMARY: `build_from_input_buffer` (inputs_native input_message.rs) computes
    `(end_tick + 1 - buffer_start_tick) as usize` with `Tick−Tick → i32`; a buffer leading
    `end_tick` by ≥ 2 sign-extends to ~2^64 and the per-iteration `states.push` allocates until
    the process is SIGKILLed — a silent unbounded loop where an empty/absent message (or a
    warn + bail) is correct. One-line shape: clamp both bounds at 0 / early-return on
    `buffer_start_tick > end_tick`.
(B) TRIGGER: on `SyncEvent<InputTimelineConfig>`, `receive_tick_events` compensates the buffer
    for `tick_delta` but `recompute_input_delay_on_sync` changes `input_delay` with no
    corresponding buffer/message compensation — any delay SHRINK ≥ 2 ticks strands the buffer
    ahead of `end_tick = now + delay`, and `set_raw`'s refuse-lower rule makes the strand
    permanent. Repro: link conditioner + CPU-loaded client batch connects, or a separately
    hard-capped encoder process. The constant-offset runaway (§10) stays open — it did NOT
    reproduce (MEASURED: 0/57) and is not
explained by this mechanism, though the backward-resync window it lives in is now partially
defused by the guard; re-sweep on future loaded batches.

**Update (2026-07-11, MEASURED verification batches @ e3ee1ab): the hang is CPU-LOAD-GATED.** 48 headless
scripted 80/10 connects on a quiet box — 24 on main @ e3ee1ab, 24 on the pre-`min_delay` baseline
(d7d103e `src/net/client.rs`, A/B for the interp fix) — produced **0 hangs**; the interp
`min_delay` pin is not the variable. Re-running 12 connects with all 10 cores saturated
(10× `yes` busy-loops, mimicking the wave-A session's parallel-build load) reproduced **4/12**,
same signature: silence immediately after `rollback enabled` → connect `ROLLBACK-SNAP` → first
`ROLLBACK fired`, trace recording stops at the same instant (FixedLast dead, wedged main loop),
server keeps running, then the process dies **SIGKILL/exit 137 at 40–96 s** — three of the four
deaths preceded the harness's external 90 s timeout, with no jetsam/crash report found
(kill source unconfirmed; RSS-balloon-then-jetsam per the check-starvation report's failure mode
is plausible but unproven). Practical reads: (a) the old "3/10" was measured on a loaded box and
matches the loaded rate, not the quiet-box rate; casual playtests on quiet machines won't feel
it, which is why it "disappeared"; (b) any future repro/bisect MUST run under CPU saturation;
(c) the trigger is a scheduling/starvation race, consistent with the receive-time-check-starvation
neighborhood but now load-, not margin-, keyed. Raw runs + classifier script: session scratchpad
`connect-verify{,-ab,-load}/`.

**Update (2026-07-10, §9 session):** the hang is NOT lat0-specific after all — 3 of 10 headless
scripted runs at **80/10** hung the same way (client silent immediately after the connect
ROLLBACK-SNAP log line, process alive, server keeps running). The lat0-only framing below is
the original observation, kept for the record; the zero-margin hypothesis needs re-examination
since 80/10 has healthy margin. Budget retries into any scripted-pair harness.

While gathering §6 the zero-latency client (`SPIKE_LATENCY_MS=0`, headless simulate) reproducibly
**froze at connect**: the main loop stalls immediately after `net::rig`'s "first physics tick
complete — rollback enabled", records ~10 ticks, then lives ~14 s frozen (the only tell is a
`gilrs::ff::server` "iteration took >50 ms" warning that never appears in a clean run) before the
process is SIGKILLed (exit 137, signal 9). It is **lat0-specific** — the 80/10 condition completed
6/6 clean — and **intermittent**, not deterministic: the same lat0 binary also completed on 2 runs
(the §6 lat0 data). It reproduces on a quiet box, with no OOM/jetsam kill, no crash report, and no
thermal throttling (all three verified). Peak RSS on the runs that survive is ~0.7 GB, so it is
not memory. It is not the contact phenomenon.

**Hypothesis worth testing (untested):** the zero-prediction-margin condition already documented
in `src/net/watchdog.rs` — loopback RTT plus `InputDelayConfig::balanced()` drives the prediction
margin to zero, the regime where lightyear's receive-time mismatch check starves. A startup stall
in exactly that regime would fit the lat0-only, at-first-rollback-enable signature. **Left
unresolved and uninvestigated** — not chased, no `src/` change. Methodological cost: it caps the
lat0 sample size for any replayed-tick metric (§6's lat0 n=8), so lat0 measurements here lean on
the two clean runs plus the 80/10 server-join.

## 8. 2026-07-10: the divergence instrument (per-tick state hash) and its baseline

A determinism effort needs a number to drive to zero. This section documents the instrument that
measures it — a per-tick, world-independent state hash on both ends plus an offline join/diff —
and the baseline it reads today: the reference the upstream-patch A/Bs (wave A,
`HANDOFF-upstream-determinism-wave-a.md`) get compared against. Every number below is MEASURED
(2026-07-10, dev-profile binaries from the `divergence-instrument` branch, same machine both
ends, macOS).

### What it is

Two layers, both env-gated (`SPIKE_TRACE`; zero cost unset — the recorder systems are registered
only in a traced run):

1. **Per-tick state hash** (`src/trace.rs`, `hash_tank_state`, riding the existing `record_tick`
   in `FixedLast`). Each fixed tick, for each tank root, on BOTH client and server, new fields on
   the existing `tick` trace row:
   - `h` — the combined hash: the exhaustive boolean "did anything differ this tick?".
   - `hpos` / `hrot` / `hlv` / `hav` / `hsim` — per-component sub-hashes, so a mismatch localizes
     to `Position` / `Rotation` / `LinearVelocity` / `AngularVelocity` / carried sim.
   - `own` — the cross-world tank identity the offline join pairs on (never the entity id).

   What is hashed: the physics state (`Position`, `Rotation`, `LinearVelocity`,
   `AngularVelocity`) by raw f32 BITS (`to_bits` — bit-exactness is the bar), plus the carried
   sim state where hidden divergence lives: `DriveState` (throttle, steer) and all of `TankSim` —
   servo current/previous/velocity, weapon reload/recoil, and the per-wheel brush anchors
   (`Some(point)` vs `None`, discriminated). `hsim` is the ONLY cross-world witness for the
   carried state; no pose/velocity field exposes it — and the baseline below shows that is
   exactly where today's residual divergence lives.

   **World-independence (the load-bearing design constraint).** Client and server entity ids
   differ for the same logical tank (measured: 4294966669 vs 4294966650), so the hash consumes NO
   entity id, no pointer, no `HashMap` iteration, no archetype order — only f32 bits, in a fixed
   field order, with every `TankSim` `Vec` walked in spawn-sorted slot order (`WheelIndex` /
   `ServoIndex` / `WeaponIndex`, identical across worlds by construction's sorted-by-name
   assignment). A fixed FNV-1a 64 (not std's version-seeded SipHash) keeps hashes reproducible
   across builds and re-derivable offline. The row's `own` field — the game `Controlled` marker
   on the client/SP, lightyear's `ControlledBy` on the server, `false` for the ownerless bot on
   both — is the pairing key, so the join never touches `e`. Unit tests (`src/trace.rs`,
   `mod tests`) pin: same state → same hash; one flipped `av` bit → different `h` and `hav` and
   NOTHING else; `+0.0`/`−0.0` hash apart; anchor `None` ≠ `Some(origin)`; entity-id independence.
   Replay semantics are preserved: rollback-replay rows keep their `rp` stamp and the join keeps
   the LAST row per (tick, entity) — the corrected value.

2. **Offline join/diff** (`scripts/divergence/analyze.py`). Pairs each tank across worlds by
   `own` (busiest-entity fallback for pre-`own` traces) and reports per shared tick: hash match
   rate (overall + flat-cruise vs contact-transient windows, classified from the rows' own
   `gnd`/`hc` on both ends), the first-divergence tick with its diverged sub-component(s), a
   sub-component tally over all mismatched ticks, and per-component error magnitudes (|Δp|,
   rotation angle, |Δlv|, |Δav|; p50/p99/max) from the pose/velocity fields the rows already
   carry. `--json` emits the rates/tally as a machine payload for A/B scripting.

### How to run it

Same harness as §6 (server `SPIKE_PERTURB=0` + headless scripted client, `SPIKE_TRACE` both
ends — role-suffixed files; avoid `SPIKE_LATENCY_MS=0`, the §7 connect hang). Direct binary runs
need `BEVY_ASSET_ROOT=<repo>`:

```
# server (background)
BEVY_ASSET_ROOT=$PWD SPIKE_PERTURB=0 SPIKE_TRACE=/tmp/base.jsonl ./target/debug/overmatch-server &
# client (headless scripted; 80/10 = the standard jittered condition; add SPIKE_SIM_LONG=1
# for the ~20 s course crossing the bump z~-70 and washboard z~-82..-90)
BEVY_ASSET_ROOT=$PWD SPIKE_SIMULATE_INPUT=1 SPIKE_SIM_LONG=1 SPIKE_LATENCY_MS=80 \
    SPIKE_JITTER_MS=10 SPIKE_TRACE=/tmp/base.jsonl ./target/debug/overmatch
# analyze (warmup-ticks 0 reports the full run; default drops the first 64 shared ticks)
uv run scripts/divergence/analyze.py --client /tmp/base.client.jsonl \
    --server /tmp/base.server.jsonl --warmup-ticks 0
```

### The baseline (MEASURED 2026-07-10, 80 ms / 10 ms jitter, SPIKE_PERTURB=0)

`NAN-TRIPWIRE|FIXED-NAN|panicked|B0004` all 0 in every log, both runs. Full-run numbers
(`--warmup-ticks 0`).

| Metric | long course (`SPIKE_SIM_LONG`, bump+washboard) | default short course (steer arc + fire) |
|---|---|---|
| shared ticks | 1278 (~20 s) | 584 (~9 s) |
| rollbacks (whole run) | 1 (connect-time, depth 13, empty `trg`) | 2 (connect window, Position ~0.93 m) |
| hash match, overall | 91.71% (1172/1278) | 87.84% (513/584) |
| hash match, flat-cruise window | 90.00% (900 ticks) | 89.69% (572 ticks) |
| hash match, contact-transient window | **100%** (362 ticks) | 0% (12 ticks — all inside the connect transient) |
| mismatched ticks, by sub-component | 106 — **all `hsim`-only** | 71 — **all `hsim`-only** |
| \|Δp\| / rot / \|Δlv\| / \|Δav\| p50/p99/max | **all exactly 0** (bit-exact, every shared tick) | **all exactly 0** (bit-exact, every shared tick) |
| first divergence | tick 1463 (`sim`) | tick 139 (`sim`) |
| mismatch windows (contiguous) | 1463–1535 (73 ticks), 1767–1799 (33 ticks) | 139–209 (71 ticks) |

Reading, in order of importance:

1. **Physics state is bit-exact on EVERY shared tick of both runs — the whole course, bump and
   washboard included.** §5's flat-cruise bit-exactness (~880-tick windows) now extends to the
   full 1278-tick jittered course. The expected contact-transient `|Δav|`-first signature (§2's
   constraint-order row) did NOT appear here: post-shield/witness-fix, this course never enters
   the multi-manifold wedge state that term was measured in. Consequence for the wave-A avian
   A/B: **this course is not a discriminating workload for the constraint-order patch** — use the
   wedge repro (`SPIKE_SPAWN_POSE` on the slab edge, per
   `.agents/scratch/upstream-reports/avian-solver-constraint-order.md`), with this instrument as
   the metric.
2. **The only residual divergence class on this harness is carried-mechanism state (`hsim`),
   invisible to every pose/velocity field** — precisely what the hash exists to catch. It is
   transient and reconverges: both runs' first window starts exactly at the connect-time rollback
   replay (long: window 1463–1535 vs rollback start 1462; short: window 139–209 vs rollback
   starts 138/140), and the long run's second window (1767–1799, 33 ticks) opens ~4 ticks after
   the scripted fire (~tick 1763) and closes on the recoil-settle timescale. Attribution beyond
   the window timing was a HYPOTHESIS at this baseline; §9 decoded both windows — the connect
   window is aim-stream cold start (servo-only, NOT rollback-seeded: it opens at the first
   shared tick and its width is independent of rollback depth), and the fire window was the
   fire/apply_recoil order ambiguity (fixed). The fire term's run-to-run stochasticity (the
   short run's fire at ~tick 439 produced no window) is the executor resolving the ambiguous
   order per process, exactly as §9 measured.
3. The match-rate denominators matter: "flat cruise 90%" does not mean cruise diverges — the
   `hsim` windows happen to sit in ticks classified flat (gnd=16, hc=0). The window split
   classifies the mismatch's LOCATION, not its cause; the sub-component tally (`sim=106/106`,
   `71/71`) is the causal read.

### What the instrument CANNOT see yet (honest limits)

- **Non-tank state.** Only tank roots are hashed. Anything else that carries sim state —
  shells in flight, launched turrets mid-toss, future dynamic map objects — is invisible to `h`.
- **Solo pairing only.** `own` distinguishes two classes (player tank / bot). Multi-client or
  multi-bot worlds need a richer identity than one boolean, or the join pairs wrong tanks.
  Scoped to the solo case by design.
- ~~**`hsim` is a boolean.**~~ CLOSED 2026-07-10: `hsim` decodes into `hdrv`/`hsrv`/`hrld`/
  `hrec`/`hanc`, `SPIKE_TRACE_SIM_FIELDS=1` puts the raw carried values on the row, and the
  analyzer attributes each mismatch window per field family with magnitudes — see §9, which
  used exactly this to decode the baseline's windows.
- **Solver internals are hashed only through their effect.** Warm-start impulses, contact
  manifolds, broad-phase topology are not in the hash; a constraint-order divergence registers as
  a pose/velocity mismatch without naming its mechanism — that still needs the dedicated probes
  (§2, §6).

## 9. 2026-07-10: the hsim windows decoded — three classes, one repo fix (task #24)

The §8 baseline left one open divergence term: `hsim`-only windows at connect and post-fire,
attributed by window timing alone because `hsim` was one boolean. This section decodes them.
Instrument change (branch `hsim-divergence-decode`): `hsim` now splits into `hdrv`/`hsrv`/
`hrld`/`hrec`/`hanc` (drive, servo, reload, recoil, anchor — `hsim` is their fixed-order
combination), `SPIKE_TRACE_SIM_FIELDS=1` puts the raw carried values on the row (`simf`), and
the analyzer gained a MISMATCH WINDOWS section (per-window field attribution, magnitudes,
opens@first-shared-tick vs mid-run, replay-row counts). Every number below is MEASURED
(2026-07-10, dev binaries, same machine, 80/10 jitter, `SPIKE_PERTURB=0`; runs d2–d8, f1, g1–g3 on
`hsim-divergence-decode` @ 5187643 (pre-rebase 0d99a38), rebaseline traces main @ 2a482c6).

### Step zero — the metric survives

The §8 windows are final-timeline divergence, not a join artifact. Verified: lightyear's
rollback replay runs the full `FixedMain` schedule (vendored `lightyear_prediction`
`rollback.rs:1137`), so `record_tick` re-records replayed ticks at the same `FixedLast` phase
(`rp`-stamped) and the keep-last join compares corrected-timeline values; server traces carry
zero duplicate (tick, entity) rows; the tick join has zero holes; and across every run no
replay row ever flipped client/server agreement in either direction (0 "replay fixed it" /
0 "replay broke it" ticks). Window ticks are predominantly forward-simulation rows (§8 long
connect window: 56/70; every permanent window: 100%). One §8 reading does not survive: windows
do not "open at the connect rollback" — they open at the FIRST SHARED TICK in every run
(the two ends had never agreed yet), and window width is flat (~67–73 ticks) across rollback
depths 2–17, so rollback-replay seeding is out as the mechanism.

### Class 1 — the connect window is aim-stream cold start (servo-only, benign)

Every instrumented run (7/7 with aim input): a window opening at the first shared tick,
closing 67–73 ticks later, attributed servo=ALL, reload=recoil=anchor=drive=0, max |Δservo|
0.6148 in every run. The raw values (d3): at the first shared tick the client's servos are
already slewing while the server's sit at bit-exact 0.0; the server then runs the IDENTICAL
trajectory 17–28 ticks late (server t300 ≡ client t272, bit-equal). Mechanism: the scripted
aim is held from script tick 0, the client's prediction applies it immediately, and the
server's view of that input stream starts ~2 dozen ticks later at join; both ends then chase
the same constant hull-local target, so the servo state contracts to agreement (ADR-0016).
Controls: throttle starts at script tick 128 — after the window closes — so aim is the only
live input during the transient (drive=0 in every window is consistent, not exculpatory);
the reverse course (aim None, throttle-only from tick 128) scores **100.00% hash match on all
589 shared ticks through its own connect rollback** — restore and replay of
`TankSim`/`DriveState` are bit-clean when no input is in flight at join. No fix needed: this
is inherent predict-ahead join behavior, confined to servo fields, contractive by
construction. The §8 hypothesis list: restore fidelity (1) and edge semantics (2) are dead —
the reverse control rules them out; anchors (3) belong to class 3 below; writer asymmetry (4)
was the right shape but the writer is the input stream itself.

### Class 2 — the post-fire window was a system-order ambiguity (FIXED, f516fb6)

The fire-adjacent 33-tick window (§8 long 1767–1799; here d5 556..588, d8 561..593; 3 of 6
pre-fix runs) decodes as recoil=33, everything else 0, max |Δrecoil| 3.062. Raw values (d5):
reload timers in perfect lockstep (both ends 3.0000 on the fire tick — the shot fired the
SAME tick on both ends), while the server's recoil (offset, velocity) at tick T+1 bit-equals
the client's at tick T; on the fire tick the server holds the raw kick (off=0, vel=14.0), the
client one integration step (off=0.1709, vel=10.9375). Mechanism: `fire` (applies the kick)
and `apply_recoil` (integrates the spring) both take `&mut TankSim` with NO ordering edge —
Bevy execution-order ambiguity; each process resolves its own order, and when they resolve
opposite the spring integrates on opposite sides of the kick: a one-tick recoil phase that
damps on the spring's ~33-tick settle. The window length is the spring settle time, the 3.062
is (kick − one integration step) — both now predictable from spec. Fix: explicit
`apply_recoil.after(fire)` (kick-then-integrate, the order the remote-fire path
`apply_pending_recoil_kicks` already promises). Post-fix evidence: 0 of 4 runs (f1, g1–g3)
show any `hrec` window, including two whose class-3 window SPANS the fire tick with recoil=0
throughout (f1, g3) — the pre-fix equivalent (d7) carried recoil=33 inside its class-3 window. The remaining unordered `&mut TankSim` neighbors
(driving's suspension/drive chain vs shooting) write disjoint field families; the invariant
and its tripwire comment live at the `shooting::plugin` registration.

### Class 3 — perturbation-seeded physics divergence (OPEN — wave-A territory)

The rebaseline's new term, now characterized: a mid-run window carrying pos+rot+lv+av (+servo/
anchor as symptoms), opening at a perturbation event and persisting to trace end. Seeds
observed: the fire tick (rebaseline short @493, short3 @497 — initial |Δp| 0.230 mm in BOTH,
the same seed twice) and the connect-replay tail (d7 @295: ~1.7 mm; f1 @358 after a
one-tick blip @355). The connect-seeded specimens carry an ALL-16-wheel anchor
discriminant flip on the seed tick (one end's anchors all release/re-grip, the other's don't;
d6/d7/f1 — d6's healed when a rollback snapped it back, d7/f1's persisted); the fire-seeded
specimen g3 seeds with no flip (its anchor term is pure derived world-point offset, 5.2 mm),
with reload=recoil=0 proving the seed is the shell-spawn/hull-impulse perturbation itself,
not the (now-fixed) recoil ambiguity. Below the rollback thresholds
position is non-contractive: the offset can contract dynamically (d7: 1.7 mm → 78 µm by
t450) then re-amplify at contact events (d7 late course: |Δlv| 6.27 m/s, |Δp| 64 mm, still no
rollback). Servo and anchor divergence inside these windows is DERIVED — aim targets are
pose-dependent and anchors are world points — so `hsim` stays red for the window's whole
life; reload/recoil/drive stay 0 throughout. Incidence: 3/12 valid decoded runs (d7, f1, g3), 2/3 rebaseline shorts. Mechanism hypothesis (UNVERIFIED, deliberately left to the wave-A A/B): world-order-
sensitive contact/solver behavior at island-change events — the avian constraint-order class
(upstream report #2) and/or the BVH contact-restore class (#5); the wave-A avian fork A/B on
this instrument is the discriminating experiment. Handed off via
`.agents/scratch/hsim-to-wave-a.md`.

### Standing updates

- **Misfire-feel risk: no measured support.** `hrld` diverged on ZERO ticks across every run
  and every window class. The handoff's concrete fear (client accepts a fire click the server
  rejects on reload skew) has no observed mechanism today; the one carried-state fire term is
  fixed. What predict-both would re-open is class-1-style input-stream windows, not reload skew.
- **§7 hang is not lat0-specific.** 3 of 10 headless 80/10 client runs hung at connect, last
  log line the connect ROLLBACK-SNAP, process alive but silent. Same open item, wider trigger.
- **§8 limits partially closed.** The `hsim`-boolean limit is gone (decode + `simf`
  magnitudes). Non-tank state, solo pairing, and solver internals remain as stated.

### The §9 bar (predict before tracing)

- A window opening at the first shared tick, servo-only, ≤ ~73 ticks, not scaling with
  rollback depth: class 1 — expected at every join while any aim input is live; benign.
- A 33-tick recoil-only window after a shot: class 2 — must NOT appear post-f516fb6; its
  reappearance means a new `TankSim` order ambiguity (check any new `&mut TankSim` system
  against the `shooting::plugin` invariant comment).
- A mid-run pos+rot+lv+av window seeded with an all-wheel anchor flip: class 3 — expect
  reload/recoil/drive = 0, sub-threshold persistence, possible contact re-amplification;
  goes to the avian/wave-A track, not this repo.

## 10. 2026-07-11: wave-A A/B — the three upstream patches vs the instrument (class 3 returned to sender)

The wave-A review session (HANDOFF-wave-a-review.md, now consumed) reviewed Codex's three fork
patches and ran the game-level A/Bs on main @ d7d103e. Verdicts + records:
`.agents/scratch/wave-a-ab-records/`; adoption memo: `.agents/scratch/wave-a-adoption-memo.md`.
Everything below is MEASURED (80/10 unless noted; N given per claim).

- **Class 3 is NOT the avian constraint-order term.** §9 handed class 3 to the wave-A avian
  track; the track hands it back with evidence: short course N=12/side, class-3 incidence 2/12
  unpatched vs 1/12 avian-patched (n.s.), and the fire-edge seed is bit-identical either way
  (|Δp| 0.230 mm — the same constant as both §9 specimens). The all-wheel anchor-flip seed at
  island-change events remains; next suspects are the contact-restore/BVH class (report #5) or
  the shell-spawn/impulse path itself. Class 3 returns to this repo's divergence track.
- **The avian patch is still right upstream** (crate-level cross-World proof re-verified; its
  RED shows order-1 Gauss-Seidel divergence at tick 1 of settled multi-manifold contact) and
  costs nothing here: flat-cruise long course with the patch is physics-bit-exact on every
  shared tick (94.69% vs 94.45% baseline, residue hsim-only). The live-network instrument
  cannot re-measure the §2 wedge signature — the connect transient seeds state deltas before
  the shared window and starved rollback never re-anchors, so the marginally-stable wedge
  diverges fully on both sides (documented in ab-avian.md). Cross-World determinism claims are
  crate-test territory; the instrument's game-level role is the regression gate.
- **Parry workaround retirement is armed.** Against the fork, `tests/spherecast_scale.rs`
  fails exactly as designed; at-rest idle with raw TOI + patched parry matches workaround-on
  (per-tick |dy| p99 0.027 vs 0.029 mm, no limit cycle); long course at baseline (94.85%,
  bit-exact physics). Retire `sphere_cast_ground_contact`'s reconstruction when a parry release
  ships the fix.
- **The lightyear deferred-check fix works — and has a disqualifying edge.** Watchdog off at
  lat10: unpatched wedge sat at |Δp| max 55.3 mm (above the 50 mm bar) with 2 connect-only
  rollbacks (starvation, raw); patched wedge held p50 3.4 / p99 21.9 mm with mid-run rollbacks.
  BUT on the flat course 3/4 patched runs died: deferred markers recorded at zero margin +
  a backward connect SyncEvent → rollback consumption jumps the local timeline ~280 ticks
  FORWARD (base cannot roll back to a future tick), catch-up resim balloons the client to
  7.5 GB, jetsam kills it; the survivor ran ~6,400 ticks ahead of the server. The watchdog
  STAYS until the fork grows a future-tick guard and re-passes ab-lightyear.md's A/B.
- **New (rare) anomaly class for the connect track: constant-offset connect runaway.** 2/24
  standard short runs held a CONSTANT ~880 mm |Δp| from the connect window (one closed at
  tick ~471, one persisted to trace end) with no rollback despite being far above threshold —
  a world-frame offset (spawn/teleport ordering?), not physics divergence, and a watchdog blind
  spot (it compares component histories, which agree tick-to-tick once offset). Belongs to the
  §7 connect investigation. **Verification (2026-07-11 @ e3ee1ab): NOT reproduced — 0/57
  analyzable 80/10 runs** (25 quiet-box on main, 24 quiet-box on the pre-`min_delay` d7d103e
  client baseline, 8 loaded-box survivors; the 4 loaded-box §7 hangs truncate their traces too
  early to judge). Max |Δp| seen anywhere: 50 mm transient (2 runs, closed by tick ~292); the
  five 100%-hash-mismatch runs are mm-scale sub-threshold drift (class-3 family, |Δp| ≤ 9 mm),
  not this. Likely gated on the same session conditions as §7's hang (the 2/24 came from the
  loaded wave-A box); treat as dormant — re-sweep whenever §7's loaded-repro harness runs.
  **Final tally (2026-07-11, post-§7-fix): 0/117.** The last uncovered condition — 24 loaded
  connects into a REUSED long-lived server (the wave-A retry harness reconnected into live
  servers; all earlier probe batches used fresh servers) — also came back clean: 0/24, worst
  |Δp| 5.3 cm rollback-corrected transient, the §7 guard fired ZERO times (reconnect into a
  tick-advanced server does not strand the input buffer), and the server survived 24
  connect/disconnect cycles without leak or wedge. No recreated condition reproduces the
  original 2/24 and no mechanism was ever confirmed. Status: dormant-verging-on-retired — keep
  the auto-sweep convention on loaded batches, chase no further. The watchdog's constant-offset
  blind spot remains real and documented regardless.
- Rebaseline note: the pre-recoil-fix bimodal short course (§9's fire-seeded 2/3 incidence) is
  gone as such post-f516fb6; what remains is the rarer class-3 above. The long course is the
  stable regression gate (94.45–94.85% across all builds, physics bit-exact, hsim-only).

## 11. 2026-07-21: cross-architecture bit determinism achieved

The live macOS-aarch64-client/Linux-x86_64-server storm survived the
[[0027-element-grip-netcode]] protocol repairs, while a subsequent MEASURED same-platform release
control produced zero mismatches. That exonerated the repair/rollback path for this symptom and
isolated a cross-target arithmetic or code-generation class.

The cross-target bitprobe localized it after MEASURED 2,314 identical completed ticks. Under
asymmetric steering load, glam's SSE2 and NEON paths grouped `Quat::length_squared()` differently
inside Avian's `fast_renormalize`: a MEASURED 1-ULP change in two quaternion lanes became a
MEASURED 16-ULP `position.y` change through rotated-center-of-mass compensation. Contact switches
in the stiff law then amplified that seed chaotically into force, grip, and transmission state.
Avian `enhanced-determinism` did not cover this class because it did not select glam's scalar
reductions.

The workspace-wide `glam/scalar-math` decision in
[[0028-cross-architecture-bit-determinism-via-glam-scalar-math]] closes the class for the current
pinned simulation graph. Its validating pair at `codex-scalarmath` commit `020f9fd` was MEASURED
startup `IDENTICAL` for all 1,345 named raw values and tick payloads `IDENTICAL` for all 3,072
ticks across all seven seams. The full probe including dump took MEASURED 18.2 s with scalar math
vs 20.3 s with SIMD, a difference within measurement noise.

"Achieved" is deliberately scoped: identical initial data and inputs now produce bit-identical
results across the two supported architecture legs for the complete probe fixture. It does not
turn genuinely different inputs into the same simulation.

| Divergence class | Current status | Verification / handling |
|---|---|---|
| Architecture-specific glam SIMD reduction order | **CLOSED** by workspace-wide `scalar-math` | Cross-target bitprobe pair: raw startup and every tick seam must be identical |
| Deliberate gameplay impulses applied differently or on different ticks | **REMAINS** as a different-input/ordering class | Live state hashes and rollback metrics detect it; the bitprobe separates it from background architecture drift |
| Other-client inputs not yet known to the predictor | **REMAINS**, irreducible misprediction rather than numerical divergence | Server authority re-anchors it; the render-error layer presents the correction |
| Future dependency, compiler, profile, or target-feature upgrades | **CAN REOPEN** numerical divergence | Rerun the macOS-aarch64/Linux-x86_64 bitprobe pair before merge for every math/physics-affecting upgrade |

The bitprobe is now the cross-architecture verification instrument, not a one-off diagnosis. Keep
the raw dumps when a pair fails: `scripts/bitprobe/diff.py` names the first field and seam, then
reports downstream seam growth without rounding payload values.
