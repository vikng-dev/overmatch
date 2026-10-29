# Bookkeeping diagnosis — issue #168

Overmatch's video-game transform/render bookkeeping; the recoil change is presentation in the
**game's damage model**. No gameplay-state law or asset-specific rule changes.

Diagnosis runs and after-fix counters: **daytime: owner working** (the first lane session,
2026-10-29). Release A/B/B/A: **night: owner working** (2026-10-30, 00:28–00:42 local; input seen
during the battery). Hidden windows only, no Instruments/xctrace. Build and run
commands held `/tmp/overmatch-cargo.lock`; builds used `scripts/serial-cargo` with
`OVERMATCH_CARGO_LOCK_WAIT_S=10800`, runners/binaries used `/usr/bin/lockf -k -t 10800`.
The baseline is main `22507da9ec5a1eb165c201ffe9b56ce124ad3a49`; this lane is `lane/bookkeeping`.
Only comparisons within one session are used.

## Diagnosis

### The sends are removals

MEASURED with temporary counters after `MeshExtractionSystems` and after
`collect_meshes_for_gpu_building`. The main-world probe in `Last` also compared changed flags
with prior-frame transform values, grouped by name and tree/tank ancestry. Counters were enabled
only for diagnosis, never for the release frame comparison. Steady snapshots:

| Tanks | Frame | Material-readiness retries | Removal messages | Removed with changed global | Removed with changed previous | Removed with changed visibility |
|---|---|---|---|---|---|---|
| 2 | 300 | 0 | 35,955 | 34,375 | 35,584 | 0 |
| 2 | 600 | 0 | 34,032 | 32,859 | 29,739 | 0 |
| 2 | 900 | 0 | 30,691 | 18,825 | 30,276 | 0 |
| 2 | 1200 | 0 | 21,999 | 21,591 | 16,914 | 0 |
| 30 | 300 | 0 | 42,822 | 39,142 | 40,125 | 0 |
| 30 | 600 | 0 | 43,197 | 39,737 | 40,713 | 0 |

Columns overlap; their sum is not an entity count. These are snapshots, not frame averages.
Examples keyed by actual runtime names: `cap_bottom_00.CutWood`, `cap_top_00.CutWood`,
`cap_bottom_01.CutWood`, and `trunk_00.Bark`, plus unnamed tank meshes. At the first snapshot,
MEASURED removal counts for `cap_bottom_00.CutWood` / `cap_top_00.CutWood` were 631 / 630;
unnamed tank meshes accounted for 672 at two tanks and 6,277 at thirty tanks.

There is no never-ready mesh/material population in these observations. The vendored
`RenderMeshInstanceGpuBuilder::prepare` retries on a missing material binding; missing mesh
allocation instead supplies zero vertex/index counts. All steady material retries above were zero.

Source-checked mechanism in vendored `bevy_pbr 0.19.1`, `src/render/mesh.rs`:

1. `extract_meshes_for_gpu_building` selects changes in `GlobalTransform` and
   `PreviousGlobalTransform`, among other inputs, even for invisible meshes.
2. `extract_mesh_for_gpu_building` unconditionally queues a removal when `ViewVisibility` is false.
   Hidden cut caps and out-of-range trunks therefore get removal messages again when their
   ancestor transform changes, even if they already have no render instance.
3. The collector sends those removals one entity at a time through `mpsc`, and the consumer tries
   to remove their instance and appends them to `RenderGpuCulledEntities::removed` again.
   The readiness-retry and removal channels share the entity payload type, so the sampled sender
   symbol alone did not distinguish them.

The local upstream draft is `upstream/bevy-invisible-mesh-removal-churn.md`. It proposes either
skipping absent instances or batching messages, with lifecycle tests still needed. No engine
code was changed.

### The writers

MEASURED map population in the startup log: 635 trees and 74 buildings. The offline composition's
`PhysicsInterpolationPlugin::interpolate_all()` registers translation/rotation interpolation as
requirements of **every** `RigidBody`, including those static map roots.

Source-checked writers (`avian3d 0.7.0` / `bevy_transform_interpolation 0.5.0`):
`complete_translation_easing` and `complete_rotation_easing` assign in `FixedFirst`;
`ease_translation_lerp` and `ease_rotation_slerp` assign in `RunFixedMainLoop`, before `Update`.
They assign without equality guards. Identical writes trigger change detection; interpolation
arithmetic can also perturb a fixed pose in floating point, so an equality guard alone cannot
eliminate all the resulting propagation.

MEASURED main-world snapshots, `[local dirty, local equal to prior frame, global dirty,
global equal to prior frame]`:

| Tanks | Frame | Local dirty | Of those equal | Global dirty | Of those equal |
|---|---|---|---|---|---|
| 2 | 300 | 1,150 | 201 | 68,457 | 8,840 |
| 2 | 600 | 1,135 | 224 | 56,920 | 223 |
| 2 | 900 | 1,149 | 946 | 33,928 | 14,543 |
| 2 | 1200 | 1,148 | 687 | 53,847 | 14,425 |
| 30 | 300 | 7,254 | 405 | 96,754 | 5,603 |
| 30 | 600 | 7,255 | 2,363 | 98,978 | 5,439 |

`tree/Scene`, `tree/fir_1`, the cap/trunk anchor and primitive names, and unnamed collider nodes
appear among the changed globals. For example, at frame 300 the two-tank snapshot has 516 changed
`tree/Scene` entities and 8,074 changed unnamed tree descendants, with zero local writes to either
group. The tank's unnamed shoe entities have 388 local writes at two tanks and 5,820 at thirty;
those counts match the actual spawned shoe entities, not an assumed asset formula.

The tree view's `present_fall` root visits are read-only. Standing trees with unchanged state and
bound view are skipped, and its trunk writes already compare pose fields. It is not the standing
forest's writer. The isolated regression, `trees::tests::a_static_map_body_is_not_eased`, uses the
real tree spawn under the offline compositions' full physics, with no `present_fall`. MEASURED: with
`interpolate_all()` it fails with `(local dirty, global dirty) = (1, 6)` after warmup, the root
and every node beneath it; with the interpolation plugin at its default it passes with `(0, 0)`.

Other unconditional assignments were source-checked in `drive_track_views` (shoe pose and roadwheel
translation/rotation), `restore_servo_truth`, `drive_servos`, and `apply_recoil`. The probe finds
same-value tank writes (e.g. MEASURED 371 of 388 unnamed tank local writes at two tanks, frame 900).
These pose assignments now compare before writing. Camera following and moving-body interpolation
still write actual frame poses; the camera is a single view, not the forest-wide invalidation.

The local interpolation integration draft is
`upstream/avian-interpolate-all-static-transform-churn.md`.

### Why the hidden no-draw loop costs time

The hidden window has no drawing target (`drawn` stays zero), but the simulation, interpolation,
transform propagation, visibility, extract and render prepare schedules still execute. The forest
is still instantiated. Static-root easing wakes its descendants, previous-transform copies select
those meshes again, and invisible meshes travel through the removal channels. Drawing no pixels
does not bypass that work.

MEASURED initial release reproduction in this session: two tanks, hidden no-draw, 15-second
warmup and 20-second window, 3,313 frames, p50 **5.914458 ms**, aggregate process CPU
**19.103532 ms/frame**, drawn count zero. CPU uses the change in `ps -o time=` (user + system CPU
summed over the process's threads), divided by the frame count over the same interval. This is
broader than the earlier system-only pool estimates: executor, driver and other process threads
are included. It is an independent CPU-time measurement, not wall time multiplied by core count.

A profiling build (`--profile profiling --features track_prints_demo`) sampled the same no-draw
path and again put collector `Sender<MainEntity>::send` and transform propagation at the top of
its busy stacks. The sampler was intrusive, so its frame numbers and extrapolated CPU figures are
not used for the release comparison.

## Fix

- **Easing is opt-in.** The offline compositions (the game, the armour sandbox, the track sandbox)
  mount `offline_physics()` (`src/lib.rs`): avian's `PhysicsPlugins` with the interpolation plugin
  at its default, which eases no body that has not opted in. A body that moves opts in at the
  insertion that makes it dynamic, through `eased_dynamic_body()` (`RigidBody::Dynamic`,
  `TranslationInterpolation`, `RotationInterpolation`): the scenario's tanks (duel, probe and
  LOD-showcase), a launched turret, and the track sandbox's hull. Static map bodies (trees,
  buildings, graybox trunks, terrain) are spawned unchanged and take no easing write. A tree's
  state still poses its children, and the fall animation remains the tree view's own.
- Rejected: opting the static bodies out with `NoTransformEasing` under `interpolate_all()`. It
  measured the same (the counters below were taken on it), but the next static body type that
  forgets the marker pays the whole cost again.
- Use `set_if_neq` for shoe and wheel poses; compare servo rotations and settled recoil translation
  before assignment. All pose calculations and simulation integration remain identical.
- The networked compositions are unchanged: they disable avian's interpolation plugin, so the
  easing components are inert there. The headless sim fixture (`headless_test.rs`) mounts
  `offline_physics()` with the interpolation plugin disabled, as it effectively ran before: every
  reader there sees the fixed-step pose its gates are stated in.

No tree mesh merging or hierarchy redesign (#157), no track algorithm redesign (#156), no per-tank
exceptions, and no engine patch.

## After-fix counters

MEASURED in the first lane session (daytime: owner working) with the temporary counters on a
profiling build of the rejected opt-out variant, offscreen, 20-second warmup. The final opt-in code
eases the same bodies in the offline game (the tanks, and a turret once launched), so these counts
are DERIVED to carry over; they were not re-taken on it. Snapshots, same columns as above:

| Tanks | Frame | Removal messages | Local dirty | Global dirty | Tree global dirty | Material retries |
|---|---|---|---|---|---|---|
| 2 | 300 | 742 | 427 | 1,617 | 0 | 0 |
| 2 | 600 | 742 | 335 | 1,617 | 0 | 0 |
| 2 | 900 | 742 | 191 | 1,617 | 0 | 0 |
| 2 | 1200 | 742 | 105 | 1,617 | 0 | 0 |
| 30 | 300 | 8,414 | 6,210 | 24,241 | 0 | 0 |
| 30 | 600 | 8,288 | 3,797 | 24,188 | 0 | 0 |
| 30 | 900 | 8,414 | 3,081 | 24,241 | 0 | 0 |

Against the diagnosis snapshots: removals fall from 22,000–36,000 to 742 at two tanks and from
~43,000 to ~8,400 at thirty; changed globals fall from 34,000–68,000 to 1,617 and from 97,000–99,000
to ~24,000. Every remaining removal is a tank mesh (672 of the 742 at two tanks are unnamed tank
meshes; the rest include `Wheel_L.MildSteel`/`Wheel_R.MildSteel`, 9 each). The remaining global
changes are the tanks' own hierarchies, 808 entities per tank, every frame.

## Release A/B/B/A

**Night: owner working.** 2026-10-30, 00:28–00:42 local. `HIDIdleTime` at each condition's start
ranged 1–207 s, so the owner was using the machine; `mobileassetd` was at ~79 % of a core eleven
minutes before the battery (not sampled during it). One lock hold covered the whole battery, so no
build overlapped it.

- A = main `22507da`, the profiling build at `.agents/scratch/perf-2026-10-29/bin/night-22507da`.
- B = this lane at `9b71330` (source identical to the final commit), built the same way:
  `scripts/serial-cargo build --locked --profile profiling --bin overmatch --features
  track_prints_demo`, sha256 prefix `9aec1e74acede193`.
- Assets: the bay's checkout for both; the lane changes no asset.
- Order A1 B1 B2 A2, in one session. Means and deltas are DERIVED from the MEASURED runs.

**Offscreen frame time** (`scripts/perf/run-ablation.sh`, `OFFSCREEN=1`, 15 s warmup + 30 s
measured, every gate passed), ms:

| Tanks | Stat | A1 | B1 | B2 | A2 | Mean A | Mean B | B − A |
|---|---|---|---|---|---|---|---|---|
| 2 | p50 | 14.13 | 14.12 | 14.23 | 14.87 | 14.50 | 14.18 | −0.33 |
| 2 | p95 | 17.70 | 16.85 | 15.79 | 18.54 | 18.12 | 16.32 | −1.80 |
| 2 | p99 | 19.12 | 18.68 | 16.94 | 20.16 | 19.64 | 17.81 | −1.83 |
| 30 | p50 | 18.15 | 17.99 | 19.12 | 19.23 | 18.69 | 18.56 | −0.14 |
| 30 | p95 | 23.25 | 23.10 | 24.34 | 24.61 | 23.93 | 23.72 | −0.21 |
| 30 | p99 | 24.80 | 24.94 | 25.94 | 26.20 | 25.50 | 25.44 | −0.06 |

The A1 → A2 drift is +0.73 ms p50 at two tanks and +1.08 ms at thirty. B1's worst frames (73.7 ms
at two tanks, 57.1 ms at thirty) are single spikes; p99 does not move with them.

**Two tanks, process CPU.** The hidden-window helper of the first session: it samples
`ps -o time=` at the start and end of a 30-second window after a 15-second warmup and divides by
the frames in that window. No-draw is the hidden window alone (`drawn` advanced 0 times); offscreen
adds `SPIKE_FRAME_COST_OFFSCREEN=1` (`drawn` advanced on every frame of the window).

| Mode | Measure | A1 | B1 | B2 | A2 | Mean A | Mean B | B − A |
|---|---|---|---|---|---|---|---|---|
| no-draw | frames in 30 s | 4,895 | 6,513 | 6,487 | 4,888 | | | |
| no-draw | p50 ms | 6.003 | 4.124 | 4.122 | 6.001 | 6.002 | 4.123 | **−1.879 (−31 %)** |
| no-draw | CPU ms/frame | 19.40 | 15.34 | 15.30 | 19.72 | 19.56 | 15.32 | **−4.24 (−22 %)** |
| offscreen | p50 ms | 14.525 | 14.278 | 14.242 | 14.688 | 14.61 | 14.26 | −0.35 |
| offscreen | CPU ms/frame | 30.83 | 21.38 | 21.34 | 30.69 | 30.76 | 21.36 | **−9.40 (−31 %)** |

Reading it:

- At two tanks the drawn frame is GPU-paced (`frame-attribution.md`), so the CPU saved, 9.4 ms per
  frame across the process's threads, barely reaches the wall clock: p50 −0.33 ms, inside the
  +0.73 ms drift. The tail tightens (p95 −1.8 ms). The issue's estimate for the two consumers was
  ~11 ms (5.59 + 5.42, DERIVED from samples).
- The no-draw loop, which the GPU does not pace, drops 1.88 ms p50 and runs 33 % more frames in the
  same 30 seconds.
- At thirty tanks the wall-time delta (−0.14 ms p50) is far inside the drift: no change is
  resolved. Process CPU at thirty tanks was not measured.

## Validation and remaining work

Gates on the final code, this session:

- `cargo fmt --all --check`: pass.
- CI clippy (`scripts/serial-cargo clippy --locked --all-targets --features dev_ui,bitprobe -- -D
  warnings`): pass.
- Targeted tests (`trees:: scatter track::view tank::servo shooting world:: easing_tests damage::
  headless_test::`): 73 passed, 0 failed, 1 ignored (the on-demand forest bed).
- Mutants, MEASURED: `offline_physics()` with `interpolate_all()` fails
  `a_static_map_body_is_not_eased` with `(1, 6)`; `eased_dynamic_body()` without
  `RotationInterpolation` fails `a_moving_eased_body_is_drawn_behind_its_latest_fixed_pose` on its
  yaw assertion. The headless tank-add observer also asserts every scenario tank is born with both
  easing components. Both mutants were reverted.
- Nothing visible changes: DERIVED, not checked by eye (hidden windows only). Static bodies were
  eased between identical poses, moving bodies are eased as before, and a skipped pose write is
  one whose value is already in place.

Remaining:

- Moving hierarchies still cost the same bookkeeping every frame. Each tank's 808 entities
  re-propagate every frame, and its hidden meshes re-queue a render removal every frame (742 at
  two tanks, ~8,400 at thirty): bevy's invisible-mesh path,
  `upstream/bevy-invisible-mesh-removal-churn.md`. A resting hull is still eased every frame;
  skipping equal start and end poses is the upstream change drafted in
  `upstream/avian-interpolate-all-static-transform-churn.md`, not applied.
- The thirty-tank wall-time effect, and its process CPU, need a run with the owner away, or a
  timeline (`frame-attribution.md`'s open critical-path question).
- UNMEASURED: `apply_recoil` (`src/shooting.rs`) takes `TankSim` through `DerefMut` for every barrel
  every fixed tick, so `TankSim` reads as changed every tick whether or not a barrel moved. Whether
  anything pays for that is not known; it is outside this fix.
