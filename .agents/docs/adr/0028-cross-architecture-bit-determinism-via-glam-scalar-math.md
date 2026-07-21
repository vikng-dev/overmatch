# Cross-architecture bit determinism via glam scalar-math

> **Status: accepted; validated 2026-07-21.**

The simulation enables `glam/scalar-math` workspace-wide and keeps the Bevy/Avian math graph on
one pinned glam 0.32.1 instance. This removes an architecture-specific SIMD reduction order from
simulation math while preserving server authority, state replication, and the divergence doctrine
in [[0015-divergence-doctrine]].

## Context

A live macOS-aarch64 client against the Linux-x86_64 authority produced a rollback storm after the
DECLARED `PROTOCOL_REV = 16` grip repair work. The MEASURED capture contained 2,553 rollback events;
2,688 component triggers named `TankTransmission` and 54 named `Position`. The continuous mismatch
was behaviorally material, not a harmless last-bit comparator artifact: MEASURED transmission-demand
deltas reached 4.854 kN at p99.9 and 5.923 kN maximum, and MEASURED 41 of 2,552 pairable corrections
changed at least one discrete transmission field.

We first repaired the protocol bugs exposed by that storm rather than blaming arithmetic. The
[[0027-element-grip-netcode]] revisions moved checkpoint identity to stable `CombatantId`, admitted
the producer's complete rounding envelope, made raw-bit-identical checkpoints no-op repairs, and
deferred future effect anchors until their completed tick existed locally. A subsequent MEASURED
same-platform release control produced zero mismatches, exonerating the repair/rollback protocol for
this symptom and isolating a cross-target floating-point or code-generation path.

The bitprobe then localized that path. Startup was MEASURED identical across 1,345 named raw values,
and both targets remained bit-identical through MEASURED 2,314 completed ticks. At asymmetric
steering load, glam's SSE2 and NEON implementations grouped `Quat::length_squared()` differently
inside Avian's `fast_renormalize`. The first seed was a MEASURED 1-ULP change in two rotation lanes;
Avian's rotated-center-of-mass pose compensation turned it into a MEASURED 16-ULP position change
while velocity was still identical. On the next tick, contact switches in the stiff contact law
amplified the seed chaotically into the later force, grip, and transmission divergence.

## Decision

Enable `glam/scalar-math` through the workspace's direct glam dependency and unify it with
Bevy/Avian on glam 0.32.1. Cargo feature unification then selects glam's scalar implementation for
the complete simulation math graph on every target. Keep the narrow vendored bevy_reflect 0.19.0
compatibility patch required because scalar glam does not provide serde implementations for
`BVec3A` and `BVec4A`; the patch omits only those unsupported serde registrations.

Rejected alternatives:

- **Patch only the observed Avian/glam reduction.** `fast_renormalize` was merely the first seed
  this fixture exposed. Chasing individual SIMD reductions would repeat for every other vector or
  matrix reduction in the dependency graph and again after upgrades.
- **Enable only Avian `enhanced-determinism`.** It standardizes transcendental math and selected
  collection behavior, but does not cover glam's SSE2-vs-NEON reduction order; the measured seed
  survives that feature contract.
- **Widen continuous-state comparators.** The storm had MEASURED kilonewton-scale tails and crossed
  discrete transmission decisions. A tolerance wide enough to silence it would hide real behavior
  rather than remove the numerical cause.

## Consequences

- Cross-architecture bit identity is proven for the full deterministic fixture at commit
  `020f9fd` on branch `codex-scalarmath`: MEASURED startup `IDENTICAL` for all 1,345 values and tick
  payloads `IDENTICAL` for all 3,072 ticks across all seven seams, macOS aarch64 vs Linux x86_64.
- The cost is negligible in the validating measurement. The full probe including dump took
  MEASURED 18.2 s with scalar math vs 20.3 s with SIMD; the difference is within measurement noise.
- The vendored bevy_reflect patch is an explicit upgrade surface. Any Bevy or glam upgrade must
  re-evaluate whether the missing scalar `BVec3A`/`BVec4A` serde implementations or registrations
  still require it.
- `scripts/bitprobe` is the verification instrument for cross-target raw-bit behavior. Dependency,
  compiler, or profile changes touching math or physics must rerun the macOS-aarch64/Linux-x86_64
  pair before merge.
- `upstream/avian-enhanced-determinism-simd-reductions.md` records the feature-contract gap and its
  evidence. The upstream report is drafted but remains unfiled pending explicit direction.

## Related

[[0015-divergence-doctrine]] · [[0027-element-grip-netcode]] ·
`design/sim-divergence-and-determinism.md` §11
