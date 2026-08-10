# 0034 — Tanks are data: generic systems, no per-tank overrides

Status: ACCEPTED 2026-08-08

## Decision

Every system, law, gate and test in this project is written for the *class* "tank", never
for an instance. The Tiger is the seed vehicle and the standing test fixture — it is not
the spec, and nothing may be tested, asserted, tweaked or modeled around it specifically.

1. **Code contains no tank identity.** No tank name appears in game or pipeline logic, no
   name is parsed to select behavior (names never drive behavior — the anti-name-encoding
   rule), no constant is "the Tiger's number", no branch exists because one vehicle needed
   it.
2. **Per-tank truth is declarative asset data, and only that.** `<tank>.tank.ron`,
   `assets/materials/materials.ron`, the LOD manifest. Data files
   *describe* a vehicle: they supply measured parameters to generic laws and select among
   generic models (e.g. a transmission model). They never patch, scale, or exempt system
   behavior for one vehicle. A field whose only honest name would be "make tank X do Y" is
   refused at review.
3. **Gates and pipeline assert invariants, not instances.** The export door and pre-flight
   check properties every valid tank must satisfy (manifold shells, unit-scale nodes, no
   negative determinants, no animation data, substance resolution, degenerate limits).
   Asset-specific facts (triangle counts, substance censuses) may be *printed* as diffs for
   a human to read; they are never hard-coded in a gate or test as a pass condition.
4. **Tests assert class invariants; a shipped tank may serve as witness.** A fixture needs
   *some* asset, and goldens legitimately pin measured behavior of the shipped bytes as
   regression tripwires — but a fixture number is fixture data, labeled as such, expected
   to change when the asset changes. The moment a test would fail for a *different valid
   tank*, the test is asserting the instance and is wrong.
5. **Behavior is emergent from data through law.** There is no requirement that a Tiger
   holds on a 30° slope; its holding power is whatever the grip law yields from its data.
   When a vehicle "needs" an exception to feel or function right, exactly one of two
   things is broken — the law or the asset's data — and that is what gets fixed. A
   per-tank override is never the third option.

## Rationale

The union-field ballistics walk is classless by construction (cost = ∫max(factor) over
substances, geometry from the mesh); the LOD ladder generates every asset from one global
octave grid with no per-asset tuning ([[0033]]); the track laws in `derive.rs` replaced
hard-coded rig constants with `RigGeom` measured per vehicle. Each of those arrived by
deleting an instance-shaped special case, and each deletion made the next vehicle free.
With one tank shipped, instance-shaped shortcuts are cheap and invisible; with the second
tank they become the migration. The precedent is set now, while the count is one.

## Consequences

- Adding a vehicle = a .blend, a `.tank.ron`, substance assignments, a pinned census-free
  export run. Zero code edits, zero new gate logic, zero new test literals.
- The existing Tiger-shaped literals (pinned substance counts, "Link is 1520 tris" door
  checks) are debt under this ADR; the generalization slice removes them and moves the
  census to a printed diff.
- Reviews get a one-line test for any diff touching sim, pipeline or tests: *would this
  line survive tank #2 unchanged?* If not, it needs this ADR's justification or a rewrite.
