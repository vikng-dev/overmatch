# avian3d 0.7: solver constraint order derives from entity index, not a spatial key — cross-World non-determinism

**Target:** github.com/Jondolf/avian · avian3d 0.7 · **Severity for us:** LOW *current impact* (bounded, self-healing, absorbed by server authority) but **HIGHEST strategic** — this is the single remaining obstacle to fully deterministic forward sim, i.e. the linchpin for any input-only / lockstep / GGPO architecture. · **Status:** unfiled
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
onto `angular_velocity` in a **different color order**. Float addition is non-associative ⇒ the
result is deterministic per-process but differs between Worlds.

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

## Our workaround + removal condition

**None, deliberately.** The divergence is bounded, self-healing, and fully absorbed by the
server-authoritative + prediction architecture (it manifests only as occasional tiny corrections
in a pathological wedged state; normal play is bit-exact). We do not work around it.

Its importance is strategic: it is the **last same-machine non-determinism source**, and the exact
obstacle between us and deterministic forward sim. If this ships upstream (plus `enhanced-determinism`
for the independent transcendental axis), input-only lockstep / GGPO-style rollback architectures
become viable for this project (see ADR-0015 and the determinism-unlock analysis). Track it as the
enabler, not a bug to patch locally.
