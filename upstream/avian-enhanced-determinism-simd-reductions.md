## Summary

`enhanced-determinism` routes transcendental math through portable `libm` (via
`simba/libm_force`, `glam/libm`) and makes Parry's hash collections deterministic — but it does
not make simulation results reproducible across CPU architectures, because glam's
architecture-specific SIMD paths reduce vectors in different orders on SSE2 vs NEON. Since float
addition is not associative, mathematically-equivalent groupings produce different last bits.

We traced a concrete client/server divergence (macOS aarch64 vs Linux x86_64, identical `--locked`
builds) to exactly this class, with `Quat::length_squared()` inside `fast_renormalize()` in the
solver-body pose writeback as the first observed seed.

## Environment

- avian3d 0.7.0 (default 3D f32, no `simd`, `parallel` on), bevy 0.19, glam 0.32.1 with
  `libm` enabled
- Left: macOS aarch64 (Apple M4), release. Right: Linux x86_64 (GitHub Actions runner,
  baseline target-cpu), release. Same commit, `--locked`.

## What we measured

We built a differential harness that runs an identical deterministic scenario (a tracked vehicle:
settle → straight-line acceleration → cruise → steering) on both targets and dumps raw f32 bit
patterns per tick at several pipeline seams.

- All startup constants MEASURED identical (1,345 values, world construction included).
- Both targets MEASURED bit-identical for 2,314 consecutive ticks at the DECLARED 64 Hz — the
  entire straight-line phase.
- First divergence appears a few ticks after asymmetric steering load begins, in the post-solve
  pose: `rotation.z` and `rotation.w` differ by MEASURED 1 ULP each, `position.y` by MEASURED
  16 ULPs (5.96e-8 m), velocities still identical on that tick. Everything downstream (contacts,
  forces) diverges on the following tick and grows chaotically thereafter.

The signature (rotation ±1 ULP + amplified position, velocities clean) matches the writeback in
`dynamics/solver/solver_body/plugin.rs`:

```rust
let old_world_com = *rot * com.0;
*rot = (solver_body.delta_rotation * *rot).fast_renormalize();
let new_world_com = *rot * com.0;
**pos += solver_body.delta_position + old_world_com - new_world_com;
```

`fast_renormalize` uses `length_squared()`; the position line then amplifies the MEASURED 1-ULP
quaternion change through the two rotated-COM evaluations (our COM is DERIVED ~0.88 m from
origin).

## Root cause

glam 0.32.1 reduces the 4-lane dot differently per arch:

- SSE2 (`src/sse2.rs` `dot4_in_x`): effectively `(x²+z²) + (y²+w²)`
- NEON (`src/neon.rs` via `vaddvq_f32`/pairwise add): effectively `(x²+y²) + (z²+w²)`

Equivalent in reals, not in f32. Once the quaternion has four "interesting" components
(steering-induced yaw+roll in our case), one grouping crosses a rounding boundary the other
doesn't. This is by design on glam's side — `scalar-math` is glam's supported opt-out — so the gap
is at the feature-contract level here: `enhanced-determinism` (whose docs describe cross-platform
determinism) doesn't currently imply it.

## Suggested fix

Either:

1. Make `enhanced-determinism` also enable `glam/scalar-math` (feature unification reaches
   bevy_math's glam), or
2. Document prominently that cross-architecture determinism additionally requires the user to
   enable `glam/scalar-math` themselves.

We MEASURED same-architecture runs byte-identical for the full DECLARED 3,072-tick scenario, so the
divergence is purely the cross-arch SIMD path. Happy to provide the full bit dumps or more details
if useful.

---

## Status (local record)

- 2026-07-21: **FIX VALIDATED** on branch `codex-scalarmath`, commit `020f9fd`. The MEASURED
  macOS-aarch64 vs Linux-x86_64 bitprobe pair reports startup `IDENTICAL` for all 1,345 named raw
  values and tick payloads `IDENTICAL` for all 3,072 ticks across all seven seams.
- Upstream report remains DRAFTED, NOT FILED. It was briefly filed as avianphysics/avian#1030 by
  process error and closed minutes later; re-file only on explicit direction.
- Original evidence: MEASURED bitprobe cross-target run pair at `d0fd64a`: startup identical
  (1,345 values), bit-identical for 2,314 ticks, then first divergence at tick 2314 in
  `pose_velocity`: `position.y` at 16 ULP and `rotation.z`/`.w` at 1 ULP each; all track seams
  follow at tick 2315.
- Root cause: glam 0.32.1 SSE2 dot4 groups `(x²+z²)+(y²+w²)`, while NEON `vaddvq` groups
  `(x²+y²)+(z²+w²)`; the seed lands in avian `fast_renormalize` and rotated-COM position
  compensation (`solver_body/plugin.rs:280`, `physics_transform/transform.rs:806`).
- Local fix RE-LANDED after one renderer-driven revert: workspace-wide `glam/scalar-math`, the
  direct glam dependency unified on 0.32.1, a vendored bevy_reflect 0.19.0 compatibility patch for
  scalar glam's unsupported `BVec3A`/`BVec4A` serde types, and a vendored bevy_pbr 0.19.0
  `MeshUniform` alignment workaround with a static Rust-size-versus-shader-size gate. The renderer
  defect is tracked separately in `bevy-uninitbuffervec-rust-size-vs-shader-stride.md`.
- Performance: MEASURED wall time was 18.2 s with scalar math vs 20.3 s with SIMD for the full
  probe including dump; the difference is within measurement noise.
- Proposed upstream change: `enhanced-determinism` implies `glam/scalar-math`, or documents that
  cross-architecture determinism requires it.
