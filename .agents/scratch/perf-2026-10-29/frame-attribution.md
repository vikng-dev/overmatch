# Frame attribution: where the rendered frame goes (2026-10-29)

Per stage (main world and compute pool, render thread, GPU) and, within each, per bevy system,
on the offline duel at 2 and 30 tanks, M4, 2560x1440. This note is about frame cost; it touches no
damage-model code.

**Every timing here is "daytime: owner working".** The owner was using the machine (browser open),
so the runs were hidden, short and sampled. Use them as **relative shares for ranking**; compare
no total with another session. The full GPU battery runs at night (last section).

## Verdict

- **2 tanks: the GPU paces the frame.** The render thread waits **5.20 ms per frame** for a Metal
  command buffer inside `submit_pending_command_buffers` (DERIVED from MEASURED samples), which happens only when the
  GPU has not retired earlier frames. So the GPU is saturated and its work per frame is about the
  whole frame, **~14.2 ms** (DERIVED). The render world's CPU path is ~9 ms of it (DERIVED: frame
  minus that wait), and the main world's ~7.7 ms (main thread busy 1.57 + waiting on its pool
  systems 4.39 + fixed tick 1.71, DERIVED). Which passes fill the GPU's 14 ms is the night battery.
- **30 tanks: the main world paces the frame.** The render thread's GPU wait drops to **0.001 ms**
  (DERIVED: the GPU keeps up). The main thread spends **17.0 of 18.1 ms** in its own schedules:
  busy 2.24, waiting on Update's pool systems 9.02, on FixedMain's 5.75 (DERIVED). The render
  thread sits idle ~5.8 ms per frame waiting for the next extract (DERIVED: its 11.96 ms of
  waiting minus the 6.18 ms spent inside its own schedule and the camera driver).
- **The 5.9 ms no-draw loop at 2 tanks is engine bookkeeping, not game logic.** With nothing drawn
  the main world (~6.2 ms) and render world (~6.8 ms) chains are about equal (sampled run, 7.52 ms
  frames). The CPU time is bevy's per-mesh render bookkeeping (`collect_meshes_for_gpu_building`,
  5.61 ms of pool time per frame) and transform propagation (4.11 ms); every game system together
  is 0.84 ms (DERIVED, pool time summed over threads).

## The 10 largest consumers

CPU figures are **ms of CPU time per frame, summed over all threads** (pool-ms). A parallel
consumer's pool-ms can exceed its wall time on the critical path; a serial one's cannot. All CPU
rows are DERIVED from MEASURED `/usr/bin/sample` counts (method below), daytime: owner working.

| # | Consumer | Stage, thread | 2 tanks | 30 tanks | Label |
|---|---|---|---|---|---|
| 1 | GPU work, all passes | GPU | ≈14.2 ms wall (saturated) | below 18.1 ms wall (not saturated) | DERIVED from the command-buffer wait |
| 2 | Transform propagation: `propagate_parent_transforms` and its workers, `mark_dirty_trees`, previous-transform copies | main world, pool | 5.59 | 8.89 | DERIVED |
| 3 | `collect_meshes_for_gpu_building` (system 1.77 / 1.46, its worker tasks 3.65 / 1.75) | render world prepare, pool | 5.42 | 3.21 | DERIVED |
| 4 | Track view: `track::view::drive_track_views` | main world Update, **serial** | 0.29 | 3.89 | DERIVED |
| 5 | Track sim: `track::sim::apply_track_forces` | main world FixedUpdate, **serial** | 0.20 | 3.46 | DERIVED |
| 6 | Physics, avian3d (collider AABBs, narrow phase, solver, transform sync) | main world, pool and main thread | 2.65 | 4.43 | DERIVED |
| 7 | Visibility: frustum and range culling, shadow-cascade culling (`check_dir_light_mesh_visibility`) | main world, pool | 2.97 | 4.13 | DERIVED |
| 8 | Render-graph encode and submit: `camera_driver` 1.40, `submit_pending_command_buffers` 1.10 (busy), `run_render_schedule` 0.42 | render thread | 3.27 | 3.32 | DERIVED |
| 9 | Other render prepare and queue (mesh allocator, instance and indirect buffers, bind groups, clusters, specialization) | render world, pool | 2.39 | 3.39 | DERIVED |
| 10 | Extract (`extract_meshes_for_gpu_building` 0.32 / 0.26 and the component extracts) | extract, main thread's critical path | 0.76 | 1.03 | DERIVED |

Below the ten: the generated sky environment light's bind groups, `prepare_generated_environment_map_bind_groups`,
**0.29 / 0.28** pool-ms per frame, CPU only; its GPU half runs every frame and is priced tonight
(`render-audit.md` item 1). UI layout and text 0.28 / 0.29. Track-print stamping 0.02 / 0.24.

### The track `cast_ray` split (render-audit item 2)

`terrain_grid::HeightGrid::cast_ray`, by the system that called it (DERIVED, busy samples under the
frame):

| Caller | 2 tanks | 30 tanks |
|---|---|---|
| authority sim, `track::sim::apply_track_forces` | 0.069 ms | **1.11 ms** |
| view, `track::view::drive_track_views` | 0.069 ms | **1.02 ms** |

**About half each.** The rays are a quarter to a third of each system's time: the rest of
`drive_track_views` (2.87 ms at 30 tanks) is wrap fitting, shoe placement and the shadow ribbons,
and the rest of `apply_track_forces` (2.35 ms) is the contact law around them. Both run **serially**, with no parallel tasks of
their own (no `par_iter` or scope task appears under either), so at 30 tanks they put
~3.9 ms (view, every frame) and ~3.0 ms per fixed tick (sim; DERIVED: 3.46 ms per frame at
64 ticks/s over 55.2 frames/s, 1.16 ticks per frame) on the main world's critical path.

### What the no-draw loop does

Hidden window, nothing drawn, 2 tanks (sampled, 7.52 ms frames; unsampled the loop is 5.86 ms,
`focus-gap-diagnosis.md`). Pool-ms per frame, DERIVED:

| Group | pool-ms |
|---|---|
| Render prepare and queue | 7.43, of which `collect_meshes_for_gpu_building` 5.61 |
| Transforms | 4.11 |
| Visibility | 2.17 |
| Render-graph encode and submit (render thread; `camera_driver` 0.68 with no view target) | 1.83 |
| Physics | 1.28 |
| Game systems (`overmatch::*`) | 0.84 |
| Extract | 0.54 |
| UI | 0.25 |

**The suspect inside `collect_meshes_for_gpu_building`.** Its worker tasks spend most of their
time in `std::sync::mpmc::Sender<MainEntity>::send`: **4.6–4.9 ms of pool time per frame at 2
tanks, 0.7–0.8 ms at 30** (DERIVED from the samples under that symbol, all four runs). In the vendored source those senders carry only
two lists: the entities to re-extract next frame, because `prepare` found their mesh or material
not ready, and the entities removed this frame (`bevy_pbr` `render/mesh.rs:2455–2610`). Each send
can wake the consumer, which is consistent with the `__psynch_cvsignal` self time beside it. Which
entities are re-sent every frame, and why, is UNMEASURED; the identical-code-folding of
`Sender<Entity>`-shaped monomorphs means a few other channels may share the symbol. It is the
largest single CPU item that is neither game logic nor scaling with tanks.

**Is the sky environment light's filtering in the no-draw loop?** On the CPU side, yes:
`prepare_generated_environment_map_bind_groups` costs 0.21 pool-ms with nothing drawn. Its GPU
passes are not priced here; the night battery's `no-env-light` pair does it.

### Unexplained remainder

- **GPU, all of it.** The 2-tank frame is GPU-bound and no pass split exists yet: Metal GPU
  timestamps are disabled on macOS and Apple's GPU tools were out of scope. The night battery
  prices passes by ablation.
- **CPU not inside any system frame:** 2.11 pool-ms per frame at 2 tanks, 2.51 at 30
  (DERIVED: busy samples on the main, pool and render threads minus everything attributed to a
  system or to a task a system spawned). This is the executor's own scheduling and spinning.
  Metal's driver threads add 1.57 and 0.87.
- **The critical path at 30 tanks.** Sampling gives each system's CPU time, not when it ran, so
  how the main world's ~27 pool-ms fold into its 17.0 ms chain is not measured. The serial track
  systems (rows 4 and 5) are on it by construction; how much of transforms, physics and visibility
  overlaps them needs a timeline (tracy).

## Method

**Binary.** `scripts/serial-cargo build --locked --profile profiling --bin overmatch --features
track_prints_demo`: release codegen plus symbols, the shipped feature set plus the track-print demo
for the night battery's print scene. No `dev_ui`. The sampled runs used a build of the lane's tree
just before `94f20cf` (sha256 prefix `0c82e0e899f34fb0`); it differs only in the draw census, which
then counted batch sets but not draw commands. The smoke used `94f20cf` itself.

**Runs.** Hidden window only (`SPIKE_SIM_WINDOWED=1`). Offscreen: `SPIKE_FRAME_COST_OFFSCREEN=1`,
the 3D camera drawing into a 2560x1440 image at scale 2 (the drawn count advanced 1405 times over
1406 frame rows, and 1102 times over 1104, in the two sample windows). No render: the hidden window alone,
drawn count 0. `OVERMATCH_MUTE=1`, `OVERMATCH_NO_FOCUS_PAUSE=1`, a `video.ron` holding only
`vsync_mode: Off`, `SPIKE_PHASE_COUNTS=1`. 30 tanks: `OVERMATCH_PROBE_TANKS=30`.

**Sampling.** `/usr/bin/sample <pid> 20 1` from t = 25 s on the stream's clock. Symbols are
Rust v0-mangled, demangled with Xcode's `llvm-cxxfilt`, so a system's name is the last generic
argument of its `FunctionSystem<…>::run_unsafe` frame. A system's time is the busy samples whose
innermost system frame is its own; work a system hands to the pool (`par_iter`, scope tasks,
propagation workers) is attributed back to it by the task's closure path. Busy excludes samples
whose leaf is a wait primitive. Per-frame ms = samples x (20 s ÷ samples per thread) ÷ frames in
the window, the frames counted from the run's own stream. Stage groups are DERIVED from system
names (crate and verb: `extract_*`, `prepare_*`, `queue_*`, the camera driver).

| Run | Sample window frames | p50 frame in window | ms per sample |
|---|---|---|---|
| 2 tanks, offscreen | 1406 | 14.20 ms | 1.521 |
| 30 tanks, offscreen | 1104 | 17.28 ms | 1.474 |
| 2 tanks, no render | 2660 | 7.32 ms | 1.430 |
| 30 tanks, no render | 1256 | 15.29 ms | 1.370 |

Frame times are MEASURED under the sampler and the owner's load; the sampler slows the loop
(6.66 against 5.86 ms in `focus-gap-diagnosis.md`). The parser and per-run reports are scratch
(`/usr/bin/sample` output is 9–11 MB per run) and are not committed.

**Draw counts.** `SPIKE_PHASE_COUNTS=1` logs, every 5 s, the multidraw batch sets per view and
phase, and the indirect draw commands per phase over all its views. Metal has no native
multi-draw-indirect: wgpu-hal encodes one indirect draw per command on the CPU (vendored
wgpu-hal-29.0.4 `src/metal/command.rs`, `draw_indexed_indirect`), so the per-phase total is the CPU
draw count. Per view, the sampled runs show 6 opaque and 6 prepass batch sets and 7/7/8 shadow sets
for the three cascades, the same at 2 and 30 tanks. The draw commands per frame (MEASURED, smoke
runs, all views of a phase together):

| Phase | 2 tanks | 30 tanks |
|---|---|---|
| Opaque | 71 | 140 |
| Depth prepass | 71 | 140 |
| Shadow, 3 cascades | 288 | 621 |
| Alpha mask, both | 0 | 0 |
| Transparent | 0 | 0 |
| **Total** | **430** | **901** |

A bin is one mesh and material, and the 28 probe tanks share the duel tanks' meshes, so 15 times
the tanks costs 2.1 times the draws. The shadow cascades are two thirds of them. The camera
driver's encode time did not move with them: 1.40 ms at both counts. Triangles are not counted.

## Instrument (Step 1)

- **Positive proof of drawing.** A render-world system counts the frames in which a `Camera3d`
  view held a `ViewTarget`; every frame row carries the count as `drawn`; `analyze.py` fails a
  window whose count advances on fewer than 99% of its frames. MEASURED on this session's runs:
  offscreen 100.0% and 99.9%; the hidden no-render runs 0.0%. The no-render stream also reported
  the primary 120 Hz monitor and a 2560x1440 surface, so it passed every negative gate and was
  rejected by this one alone.
- **Offscreen mode.** `OFFSCREEN=1` for every frame-run runner. Hidden window, cameras retargeted
  on their first frame to one 2560x1440 image at scale factor 2, no present. The analyzer reads the
  mode from the stream (`offscreen: true` surface rows) and then skips only the display and
  occlusion gates. The visible mode keeps every gate, plus the `render scale:` and
  `occluded=false` log checks.
- **Ablation levers.** `SPIKE_ABLATE=bloom,env_light,depth_prepass,ui` and `SPIKE_PHASE_COUNTS=1`
  (`src/render_cost.rs`). `scripts/perf/run-ablation.sh` runs any list of `name|video fields|env`
  conditions.

## Smoke (daytime: owner working)

Offscreen, hidden window, `scripts/perf/run-ablation.sh` with the lane's binary (`94f20cf`,
sha256 prefix `55f2a9bf92251544`), 15 s warmup and 30 s measured, every gate passed, the drawn count
advancing on every measured frame. One condition each, in this order, so the MSAA delta is a single
pair, not an A/B/B/A block:

| Condition | p50 | p95 | p99 | Δ p50 against `cal-2` |
|---|---|---|---|---|
| `cal-2`: 2 tanks | 14.34 | 17.72 | 19.41 | — |
| `msaa-off-2`: 2 tanks, `msaa: Off` | 11.59 | 15.44 | 16.63 | **−2.75** |
| `cal-30`: 30 tanks | 19.31 | 24.36 | 26.43 | +4.97 |

MEASURED, daytime: owner working. The calibration pair lands on the frames this lane set out to
explain (14.6 and 19.2 ms p50). It is the night battery's `cal-start` and `cal-end` condition for
condition. MSAA off saving 2.75 ms on a GPU-bound frame says MSAA is a real share of the GPU's
14 ms; the night battery's A/B/B/A block prices it properly.

## Night battery

Owner: built-in display on, unlocked, dimmed but lit; no external monitor; hands off. The battery
opens visible windows in its first block, so the orchestrator runs it; this lane never does. From
the main checkout, once this lane is merged:

```
cd /Users/Yan/github/vikng-dev/overmatch
BIN=<dir holding the profiling build of the merged commit> \
  caffeinate -dims scripts/perf/run-night-battery.sh .agents/scratch/perf-2026-10-29/runs/night
```

Build the binary first, in a bay: `scripts/serial-cargo build --locked --profile profiling --bin
overmatch --features track_prints_demo`, then point `BIN` at its `target/profiling`. The daytime
runs used exactly that build of this lane's commit. Then run one warm-up condition visibly to fill
the shader cache: `env DURATION_S=5 WARMUP_S=5 BIN=… scripts/perf/run-ablation.sh /tmp/warm 'w|'`.

**Expected wall clock: about 2 h 05 min** (DERIVED in the script's runbook).

Blocks, in order, each in its own subdirectory; a failed block does not stop the next. Every
condition after the first block is offscreen and carries `SPIKE_PHASE_COUNTS=1`.

1. **Visible against offscreen** (`vis-off/`), 60 s measured, every gate on. Order:
   `vis-2-a, off-2-a, off-2-b, vis-2-b, vis-30-a, off-30-a, off-30-b, vis-30-b`. The block ends with
   one analyzer table over all eight. This settles whether offscreen measures what the window
   does; `focus-gap-diagnosis.md` found 0.1 ms at 2 tanks and 0.7 ms at 30 with the older capture
   retarget.
2. **Calibration** (`cal-start/`): `cal-2`, `cal-30`, 30 s measured, exactly the smoke's pair
   above, to compare night with day.
3. **Duel, 2 tanks** (`duel-2/`): A/B/B/A blocks sharing their A, 60 s each: `base-0, rs75-a,
   rs75-b, base-1, rs50-a, rs50-b, base-2, msaa-off-a, msaa-off-b, base-3, shadows-off-a,
   shadows-off-b, base-4, cascades-2-a, cascades-2-b, base-5, res-2048-a, res-2048-b, base-6,
   no-bloom-a, no-bloom-b, base-7, no-env-light-a, no-env-light-b, base-8, no-prepass-a,
   no-prepass-b, base-9, no-ui-a, no-ui-b, base-10`. A lever's cost is the mean of its pair minus
   the mean of the bases on either side.
4. **Battle, 30 tanks** (`battle-30/`): the same shape over `shadows-off, cascades-2, res-2048,
   msaa-off, rs50, no-env-light`.
5. **Track-print relief** (`prints-fwd/`, `prints-rev/`): `mud-lighting, mud-parallax, mud-sun`,
   then reversed. Re-measures `track-prints-relief-cost.md`, which drew nothing.
6. **Forest** (`forest-fwd/`, `forest-rev/`): `forest-cones, forest-trees, duel-cones, duel-trees`,
   then reversed. The rendering half of issue 123.
7. **Calibration again** (`cal-end/`), for drift across the night.

`battery-status.txt` lists each block's OK or FAILED. Read deltas within a block only.

A locked screen was the earlier plan. With the display left on it does not arise; whether
offscreen rendering survives a locked screen or a sleeping display is untested, and the drawn gate
would fail any condition where it did not.

## Not measured, and why

- **GPU time per pass:** Metal GPU timestamps are disabled on macOS (`client_render_plugin`) and
  Apple's tools were out of bounds. The night battery prices passes by ablation.
- **The step 3 ablations:** deferred to the night battery by the owner's daytime rule.
- **Tank LOD:** no lever exists, and adding one is more than a few lines; unmeasured.
- **Normal prepass:** the game camera has none (`camera.rs` mounts `DepthPrepass` only); nothing
  to price.
- **Triangles per pass:** not cheap to collect; not counted.
- **Which entities `collect_meshes_for_gpu_building` re-sends every frame:** UNMEASURED; the
  next step is a count of `MeshesToReextractNextFrame` and of removals per frame.
- **The main world's critical path at 30 tanks:** needs a timeline profiler (tracy), not a sampler.
