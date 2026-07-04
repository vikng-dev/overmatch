# Sim divergence & the determinism landscape (bevy / avian / lightyear)

2026-07-04. Written after the step-8 rollback investigations, before the latency feel test
(slices 2/3). Two halves: what we now *know empirically* about our own client/server divergence,
and what the stack currently offers for cross-platform determinism. Sources: this repo's spike
log (measured), vendored crate sources (verified, cited by file:line), and web material (flagged).

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

Lesson: **most of our "divergence" so far was bugs, not physics.** The genuinely irreducible
part is contact-transient noise, and it is exactly what rollback + per-component thresholds are
designed to absorb. Second lesson: every one of these was found by measuring, not by reasoning —
the same discipline applies to anything below.

Why "same machine" still diverges at all: avian ships with the `parallel` feature **on by
default** (avian3d-0.7.0/Cargo.toml:57-63) and we run defaults — solver work is threaded, so
constraint application order varies run-to-run and process-to-process. Plus ECS iteration order
differs between client and server worlds (different entity histories), feeding order-dependent
float sums. This is why bit-exactness was never on the table for the current architecture.

## 3. What the stack offers today

- **avian `enhanced-determinism`** (off by default, off for us): libm math everywhere for
  cross-*platform* consistency of transcendentals; docs claim "improving determinism across
  architectures at a small performance cost" (avian3d-0.7.0/src/lib.rs:66; third-party material
  quotes 10–30% — unverified). It does NOT fix parallel/iteration order: for strict determinism
  you must also disable `parallel` (single-threaded solver) and take care that gameplay code
  iterates in a stable order. So "deterministic avian" = libm + single-thread + disciplined
  queries — a meaningful perf and ergonomics bill.
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

## 4. What this means for Overmatch

1. **Our architecture choice already sidesteps the hard problem.** State replication +
   prediction means the macOS-client-vs-Linux-server float gap (libm transcendentals, NEON vs
   AVX) shows up as a *higher background rollback rate*, not as desyncs or wrongness — the server
   is always right, clients converge by construction. The cloud feel test (Edgegap, first
   outing) is therefore also our cross-platform divergence measurement: compare
   `PredictionDiagnostics` rates against the same-machine baselines recorded in the spike log
   (~10/run smooth ground at 100 ms, ~135/20 s washboard).
2. **`enhanced-determinism` is a cheap dial we can turn if cross-platform rates disappoint.**
   It narrows the client/server transcendental gap even without full lockstep discipline —
   worth an A/B on the cloud test if rates are much worse than local. Perf cost lands on both
   sim ends; measure before adopting.
3. **Disabling `parallel` is NOT recommended** for us: it buys same-machine order stability we
   don't need (thresholds already absorb it) and costs solver throughput the 10v10 aspiration
   will want.
4. **Lockstep/deterministic replication stays rejected** for this game: 16-wheel contact-rich
   physics per tank × cross-platform × Rust ecosystem's current maturity = the expensive path,
   and its bandwidth win doesn't matter at 1v1–3v3. Revisit only if state bandwidth ever becomes
   the binding constraint (10v10 on thin pipes).
5. **Keep the divergence budget honest with the tools built this session:** `SPIKE_SIM_LONG` +
   `SPIKE_SIM_WINDOWED` + `SPIKE_SIM_AIM_SWEEP` reproduce the workload classes; `nan_tripwire`
   names corruption; per-component thresholds are the tuning surface (velocity loose, position/
   rotation tight — discrete/gameplay-binary state must never be threshold-banded).

Open thread tracked in the spike log: the residual ~25%→2/8 bind-window crash window
(`update_ray_caster_positions` NaN under the bind-burst rollbacks; suspect DisableRollback-grace
inconsistency during replay) — pre-feel-test fix candidate. Also: the set-configuration hole
(`PhysicsTransformSystems::Propagate` orphaned when `PhysicsTransformPlugin` is disabled) looks
like an upstream lightyear_avian gap worth reporting.

Web sources (background, treat as secondary): [DeepWiki avian determinism](https://deepwiki.com/avianphysics/avian/10.3-determinism) (machine-generated), [avian repo](https://github.com/avianphysics/avian), [bevy determinism discussion #2480](https://github.com/bevyengine/bevy/discussions/2480).
