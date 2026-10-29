# The focus gap: the fast runs never drew a frame

Diagnosis of the 5.7 ms vs 14.5 ms gap in `sweep-30-2026-10-29.md`. MEASURED on 2026-10-29, Apple M4,
with hidden windows only.

**Verdict.** No system gated on focus explains the gap. The fast runs' window was occluded from
birth, so bevy never acquired a swapchain texture and the 3D camera never rendered. The 5.7 ms is
the frame loop with no render work, the kind of fiction the occlusion gate exists to catch, and it
slipped through. The lost focus is a second symptom of the same window state, not the cause. The
evening's 14.5 ms is the duel's real cost.

- **Frame loop, MEASURED** with the same binary in a hidden window, focus lost in every run:
  5.86 ms p50 with nothing rendered, and 14.65 ms p50 with the camera rendering 2560x1440 into an
  offscreen image. The evening visible runs read 14.51 ms and 14.59 ms.
- **The aim ray is eliminated, MEASURED:** `commit_aim` costs 4–10 µs per frame, parry's heightfield
  ray 1–6 µs, and the whole focus-gated set at most about 20 µs.
- **There is no frame-time fix.** The fix belongs in the gate: require positive proof that the 3D
  camera drew. Three records built on the fast runs need a correction (section 7).

## 1. Focus-gated systems in the offline client

Focus reaches the offline client (`run_offline`, `GamePlugin`, `ClientPlugin`) by three routes:

| Route | Systems | Under `OVERMATCH_NO_FOCUS_PAUSE=1` |
|---|---|---|
| `state::release_on_focus_lost` | Frees the cursor and enters `AppState::Paused`. | Logs `focus lost — pause suppressed` and returns. The cursor stays `Locked` and the state stays `Playing`. |
| `PlayerInputSet.run_if(state::cursor_locked)` (`lib.rs`, `gate_player_input`) | `aim::commit_aim` (the third-person aim ray), `camera::orbit_look`, `sight::toggle_sight`, `sight::drive_gunner_aim` and `firecontrol::adjust_range` (both gunner view only), `command::gather_commands`, `command::select_round_input`, `sfx::play_fire_click`, and `command::auto_fire` (with `SPIKE_AUTO_FIRE` only) | Gated on `grab_mode == Locked`, not on focus. A failed OS grab would have logged `Could not set cursor grab mode` (vendored bevy_winit-0.19.1, `system.rs`), and no log contains that line. So the set ran in the fast runs too. |
| `Window::focused` | `frame_cost::raise_when_occluded`, which writes it | Acts only after an `occluded=true` message, and the fast runs had none. |

`overlay::input_blocked`, the one reader of `Window::focused` that blocks input, is mounted only by
`NetClientPlugin`. `WinitSettings::continuous()` runs the same update mode with or without focus.

**Consequence:** with the pause suppressed, a focus loss changes no run condition in the offline
client. Only the value of `Window::focused` changes, and no offline system acts on it.

## 2. What the fast runs share

Every condition under `runs/`, read from its log and stream. The p50 is the post-warmup median of
`frame_ms`:

| Runs (UTC start) | Focus lost | `occluded=` message | `render scale:` line | p50 ms |
|---|---|---|---|---|
| `track-prints`, `warm-prints`, `warm-forest`, `forest` (20:31–20:51) | in every condition | **none** | **none** | 4.51–5.72 |
| `sweep-30`, `-30b`, `-30c`, `ab-*`, `warm-sweep*` (13:03–14:18) | in no condition | `false`, at startup | present | 11.74–19.20 |

The split is exact: all 13 fast conditions lack both lines, and all 40 slow conditions have both.

Both missing lines point at the renderer:

- **No `occluded=` message.** A window that becomes visible gets `occluded=false` at startup, and
  every evening run logged one. winit reports occlusion *changes*, so a window that is occluded from
  creation and never uncovered reports nothing. The `frame_cost` module doc already names this
  blind spot.
- **No `render scale:` line.** `render_scale::prepare_render_scale` logs the first frame in which the
  3D camera holds a `ViewTarget`. On an `Occluded` surface, bevy stores no swapchain texture
  (vendored bevy_render-0.19.1 `view/window/mod.rs`, the `CurrentSurfaceTexture::Occluded => {}` arm).
  `prepare_view_targets` (`view/mod.rs`) then finds no output attachment, removes the `ViewTarget`,
  and skips the camera ("we can skip rendering entirely"). A run without the line never rendered the
  3D camera at all.

The cause of the occlusion is not recorded. The fast runs ran at 03:31–03:51 local time, and the
runbook notes a smoke run that died to display sleep at 03:25. A locked screen, a screensaver or a
dark backlight would each fit. The diagnosis does not depend on which it was.

## 3. Method

Every run was hidden (`SPIKE_SIM_WINDOWED=1`, no `SPIKE_SIM_VISIBLE`), and none took the screen.
Every run logged `focus lost — pause suppressed`, so focus was constant: lost throughout. The two
arms differ only in whether the 3D camera renders:

- **no render:** the hidden window alone. Its surface is `Occluded` from birth, the fast runs' state.
- **offscreen render:** the same, plus the existing capture lever,
  `OVERMATCH_CAPTURE=<png> OVERMATCH_CAPTURE_AT=3,500`. At t = 2 s it points every window-targeting
  camera at a 2560x1440 image with scale factor 2 (`dev_capture::retarget`), which is the surface
  size the gates require. The still at 3 s, inside the warmup, shows the duel drawing. The 500 s
  still is never reached, so the run continues until it is killed.

No new lever was needed. Focus gates nothing here (section 1), and the variable that differed is
rendering, which the capture lever already controls.

**Frame loop.** The binary is the shipped measurement build, `perf-2026-10-29/bin/overmatch` (sha256
prefix `540335ba5744c284`), the same file as both the fast and the slow runs. It ran with its asset
root, cloned into the session scratchpad, and with the forest runner's environment:
`OVERMATCH_MUTE=1`, `OVERMATCH_NO_FOCUS_PAUSE=1`, a `video.ron` holding only `vsync_mode: Off`, and
`SPIKE_FRAME_COST`. Each run lasted 85 s, and the p50 is taken over t = 15–75 s. There is one run per
cell.

**Per-system CPU.** This needs a symbolicated build: `scripts/serial-cargo build --locked --profile
profiling --bin overmatch --features track_prints_demo`. It uses release codegen plus symbols, at
HEAD `609534b`. That HEAD differs from the measurement binary by three source commits (the trees'
segment HP pools, a seam test, the track-print demo's hold), none of them in input or rendering. The
feature set is the measurement binary's, not `dev_ui,bitprobe`, because `dev_ui` mounts egui into
the offline root.

Each arm ran hidden. `/usr/bin/sample <pid> 20 1` was taken from 25 s to 45 s after launch. For each
system, the samples of the outermost stack frames whose v0-mangled name holds the function's path
segment (`10commit_aim`, `11HeightField…RayCast`, …) were summed over all threads. That sum is turned
into µs per frame as samples × (20 s ÷ samples per thread) ÷ frames in the window. The frame count is
DERIVED from the run's own stream.

The sampled loop is slower: 6.66 ms p50 at 2 tanks with no render, against 5.86 ms for the shipped
binary unsampled. So the profile is used to attribute time, and the frame-loop table gives the frame
times. One sample is
about 1.4 ms, which makes the resolution 0.5–1.5 µs per frame.

## 4. Results

### Frame loop: shipped binary, hidden window, focus lost in every row

| Tanks | Arm | p50 ms | p95 ms | `render scale:` | Reference visible run, p50 / p95 |
|---|---|---|---|---|---|
| 2 | no render | **5.86** | 8.87 | none | morning `forest/duel-trees`: 5.72 / 8.78, no line |
| 2 | offscreen render | **14.65** | 17.80 | `[2560, 1440]` | evening `sweep-30c/m350-2`: 14.59 / 17.72 (`ab-A1`: 14.51) |
| 30 | no render | **11.66** | 14.21 | none | none recorded |
| 30 | offscreen render | **19.80** | 25.23 | `[2560, 1440]` | evening `sweep-30c/m350-30`: 19.16 / 25.04 |

All MEASURED. The 30-tank rows use `OVERMATCH_PROBE_TANKS=30`, and their log reads
`spawned 28 probe tanks … near placement`. The hidden offscreen arm sends nothing to the compositor,
so it does not replace a visible capture. It does land within 0.1 ms of the evening runs at 2 tanks
and within 0.7 ms at 30.

- **Rendering the 3D camera costs 8.79 ms at p50 with 2 tanks and 8.14 ms with 30** (DERIVED:
  offscreen render minus no render).
- **The fast runs measured the no-render arm.** 5.72 ms against 5.86 ms, with the same two lines
  missing from the log.

### Per system: profiling build under `sample`

All figures are µs or ms per frame, DERIVED from MEASURED sample counts.

| | 2, no render | 2, offscreen | 30, no render | 30, offscreen |
|---|---|---|---|---|
| frames in the 20 s window | 2911 | 1397 | 1387 | 1049 |
| `aim::commit_aim`, aim ray included | 7.4 µs | 8.8 µs | 3.9 µs | 9.9 µs |
| of which parry `HeightField` ray cast | 3.0 µs | 2.2 µs | 1.0 µs | 5.7 µs |
| whole `PlayerInputSet` (all members in section 1) | 16 µs | 12 µs | 11 µs | 20 µs |
| render thread blocked in `-[MTLCommandQueue commandBuffer]` | 0.08 ms | **5.27 ms** | 0.09 ms | 0.23 ms |
| render thread busy (not in a blocking call) | 2.04 ms | 3.63 ms | 4.24 ms | 6.50 ms |
| each of the 4 compute-pool threads busy | 4.1 ms | 5.6 ms | 6.3 ms | 8.3 ms |

- **The focus-gated set costs about 20 µs per frame at most, in every arm.** That is three orders of
  magnitude below the gap. The aim ray is already capped at `world::VIEW_CAST_MAX_M` (2,500 m, the
  world diagonal), and its heightfield traversal costs microseconds even without a vertical reject.
  The terrain memory's suspect is eliminated.
- **At 2 tanks the rendered frame is GPU-bound.** The render thread spends 5.27 ms per frame blocked
  allocating a Metal command buffer. That allocation waits until the GPU retires an earlier frame,
  which is backpressure from the GPU. This is consistent with the sweep's shadows-off saving of
  2.26 ms.
- **At 30 tanks the GPU wait disappears (0.23 ms), and rendering adds CPU work instead.** The render
  thread's busy time rises from 4.24 to 6.50 ms per frame, and each compute-pool thread's from 6.3
  to 8.3 ms. Which stage paces that frame was not resolved.

### Seen in passing, not part of the gap

The largest named game function in every profile is `terrain_grid::HeightGrid::cast_ray`, called by
`track::oracle::BlockField::depth_along_lowered_ground`. This is the track contact, which runs in
both arms. It costs 0.10–0.16 ms per frame at 2 tanks and 1.74–2.14 ms at 30, summed over threads:
about 60–70 µs per tank per frame (DERIVED). It belongs to the 30-tank CPU budget, not to this gap.

## 5. The culprit

No system is the culprit. The gap is the whole render workload of the 3D camera: GPU time at 2
tanks, and render-side CPU at 30. The fast runs skipped all of it because their window never
presented.

The validity gates passed those runs because every presentation gate tests for a *negative* signal:
an occlusion message, a wrong monitor or a wrong surface. A window occluded from birth emits none of
them, and its monitor and surface rows still read correctly.

## 6. Proposed fix (not applied)

**Expected saving: 0 ms of frame time.** 14.5 ms (2 tanks) and 19.2 ms (30 tanks) are the duel's
real cost on this M4 at 2560x1440. The fix makes the instrument refuse the fiction:

1. **Gate on positive proof of drawing.** A render-world system counts the frames in which a
   `Camera3d` view holds a `ViewTarget`. `prepare_render_scale` already reads that fact, and the
   count goes into an `Arc<AtomicU64>` shared with the main world. `frame_cost::sample` writes the
   count into every frame row, and `scripts/perf/analyze.py` fails any condition whose measurement
   window does not advance it on (nearly) every frame. This catches a window that never drew, for
   any OS reason, and also one that stops drawing mid-run without a message.
2. **Stopgap, in the runner only.** `frame-run.zsh` fails a real-mode condition whose log lacks a
   `render scale:` line, or lacks an `occluded=false` message before the warmup ends. Both separated
   all 53 conditions above with no exception. Both are once-per-run facts, though, so they miss a
   window that drew at first and was covered later without a message.

## 7. Records built on the fast runs

These quote numbers from runs that rendered nothing. Only their CPU-side deltas survive.

- `track-prints-relief-cost.md`: "the relief is free" is unmeasured, because no relief was drawn.
  The table needs a visible rerun.
- `.agents/scratch/destruction-2026-10-26/forest-frame-cost-2026-10-29.md`: "the cost is not
  rendering" was read from runs that rendered nothing. The trees-minus-cones deltas (+0.73 ms and
  +1.09 ms p50) stand as **CPU-only** deltas, which still fits the collider-count reading. The trees'
  rendered cost is unmeasured.
- `performance-target-research.md`, line 234: the duel row (5.72 / 8.78 / 9.88 ms) is a no-render
  number.

None of these files was edited here. Each needs either a correction or a visible rerun behind the
gate in section 6.
