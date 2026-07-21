## Summary

`enhanced-determinism` routes transcendental math through portable `libm` (via `simba/libm_force`, `glam/libm`) and makes Parry's hash collections deterministic — but it does not make simulation results reproducible across CPU architectures, because glam's architecture-specific SIMD paths reduce vectors in different orders on SSE2 vs NEON. Since float addition is not associative, mathematically-equivalent groupings produce different last bits.

We traced a concrete client/server divergence (macOS aarch64 vs Linux x86_64, identical `--locked` builds) to exactly this class, with `Quat::length_squared()` inside `fast_renormalize()` in the solver-body pose writeback as the first observed seed.

## Environment

- avian3d 0.7.0 (default 3D f32, no `simd`, `parallel` on), bevy 0.19, glam 0.32.1 with `libm` enabled
- Left: macOS aarch64 (Apple M4), release. Right: Linux x86_64 (GitHub Actions runner, baseline target-cpu), release. Same commit, `--locked`.

## What we measured

We built a differential harness that runs an identical deterministic scenario (a tracked vehicle: settle → straight-line acceleration → cruise → steering) on both targets and dumps raw f32 bit patterns per tick at several pipeline seams.

- All startup constants identical (1,345 values, world construction included).
- Both targets bit-identical for 2,314 consecutive ticks (64 Hz) — the entire straight-line phase.
- First divergence appears a few ticks after asymmetric steering load begins, in the post-solve pose: `rotation.z` and `rotation.w` differ by 1 ULP each, `position.y` by 16 ULPs (5.96e-8 m), velocities still identical on that tick. Everything downstream (contacts, forces) diverges on the following tick and grows chaotically thereafter.

The signature (rotation ±1 ULP + amplified position, velocities clean) matches the writeback in `dynamics/solver/solver_body/plugin.rs`:

```rust
let old_world_com = *rot * com.0;
*rot = (solver_body.delta_rotation * *rot).fast_renormalize();
let new_world_com = *rot * com.0;
**pos += solver_body.delta_position + old_world_com - new_world_com;
```

`fast_renormalize` uses `length_squared()`; the position line then amplifies a 1-ULP quaternion change through the two rotated-COM evaluations (our COM sits ~0.88 m from origin).

## Root cause

glam 0.32.1 reduces the 4-lane dot differently per arch:

- SSE2 (`src/sse2.rs` `dot4_in_x`): effectively `(x²+z²) + (y²+w²)`
- NEON (`src/neon.rs` via `vaddvq_f32`/pairwise add): effectively `(x²+y²) + (z²+w²)`

Equivalent in reals, not in f32. Once the quaternion has four "interesting" components (steering-induced yaw+roll in our case), one grouping crosses a rounding boundary the other doesn't. This is by design on glam's side — `scalar-math` is glam's supported opt-out — so the gap is at the feature-contract level here: `enhanced-determinism` (whose docs describe cross-platform determinism) doesn't currently imply it.

## Suggested fix

Either:

1. Make `enhanced-determinism` also enable `glam/scalar-math` (feature unification reaches bevy_math's glam), or
2. Document prominently that cross-architecture determinism additionally requires the user to enable `glam/scalar-math` themselves.

We verified locally that same-architecture runs are byte-identical for the full 3,072-tick scenario, so the divergence is purely the cross-arch SIMD path. Happy to provide the full bit dumps or more details if useful.

---

## Status (local record)

- 2026-07-21: DRAFTED, NOT FILED. (Was briefly filed as avianphysics/avian#1030
  by process error and closed minutes later; re-file only on explicit direction,
  after the scalar-math fix is validated on our side.)
- Evidence: bitprobe cross-target run pair @ d0fd64a (macOS aarch64 vs Linux
  x86_64 GitHub runner): startup identical (1,345 values), bit-identical for
  2,314 ticks, first divergence tick 2314 pose_velocity position.y (16 ULP,
  rotation.z/.w 1 ULP each) at WIDE-steer onset; all track seams follow at 2315.
- Root cause: glam 0.32.1 SSE2 dot4 groups (x²+z²)+(y²+w²), NEON vaddvq groups
  (x²+y²)+(z²+w²); seed lands in avian fast_renormalize + rotated-COM position
  compensation (solver_body/plugin.rs:280, physics_transform/transform.rs:806).
- Proposed upstream change: enhanced-determinism implies glam/scalar-math, or
  documents that cross-arch determinism requires it.
- Our local fix: glam/scalar-math enabled workspace-wide (validation in flight:
  bitprobe pair must be 3,072/3,072 identical + perf A/B).
