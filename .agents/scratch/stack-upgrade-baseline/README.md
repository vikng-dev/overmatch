# Stack upgrade baseline: the "before" set (#356)

The before half of epic #355's before/after comparison: the shipped stack (Bevy 0.19, avian3d 0.7,
lightyear 0.30) measured so the upgrade can be compared against it. Damage-model content here is
game state only.

- **Commit.** The initial outputs carry `264515e5`: the bay's HEAD,
  264515e50b0da455b9ba05e26f477b0fbfd0018a, which is v0.7.0-alpha.17 (b8baa8e4) plus one docs-only
  commit (its parent is b8baa8e4; it adds five files under `.agents/scratch/`). The plan said
  alpha.16; the coordinator's brief moved the baseline to alpha.17. The bitprobe pair ran
  later, at the fixture fix the coordinator sent: its files carry `9c766de6`, a local merge of
  main whose code is eaac2a66's, since rewritten off this branch (`bitprobe.9c766de6.cmd.txt`).
  The completed belt probe and slice capture carry `f86cba0d`: the probe lifetime fix and an offline capture-only replay cue, with no
  production physics or recorder change. Their diagnoses are in `gap-diagnosis.bee99262.md`.
- **Machine.** Apple M4, 10 cores, 16 GiB, macOS 26.5.2 (MEASURED, Bevy's SystemInfo line). Other
  agents' builds shared the machine and the build lock throughout.
- **Timing runs.** Every timing run held the machine build lock (`/tmp/overmatch-cargo.lock`) for
  its whole run, so no compile could start during it, and each first passed a gate: no `rustc`
  running and a 1-minute load of at most 4. Each record says how it held the lock and the load it
  started at. Every timing row but the one-client collapse was recorded before the coordinator
  asked for the lock; all of them held it anyway (the runners take it themselves), so none was
  rerun.
- **Command records.** Every output has a `*.cmd.txt` beside it: the commit, the working directory
  (the bay root), the exact command line, its start time, the load just before it, its exit status
  and its wall time (which includes any wait for the lock).
- **Numbers.** Every number in these files is MEASURED by the run named beside it unless labelled
  DERIVED. Results are observations: the after set is compared with them, not held to them.
- **Settings and screens.** Every game client ran with `OVERMATCH_CONFIG_DIR` pointing at an empty
  directory, so the owner's own settings never entered a baseline (the render-scale capture's
  directory holds only its `video.ron`). Every run was hidden or headless and silent
  (`OVERMATCH_UNATTENDED=1`, `OVERMATCH_WINDOW=hidden|headless`, `OVERMATCH_MUTE=1`).
- **Binaries.** No image, archive or file over 1 MB is in git. Every frame, dump and large log
  stays on the recording machine under `/Users/Yan/.cache/overmatch/stack-upgrade-baseline/264515e5/`
  (the "cache" below), at the path the records name. `MANIFEST.sha256` lists every file there with
  its size and sha256: `awk '!/^#/ {print $1"  "$3}' MANIFEST.sha256 | shasum -a 256 -c`.

## Gaps

One row remains substituted: **the collapse ran with one real client, not thirty**. The plan's
30-client command never got past launch on this machine, so the row is `--tanks 30 --clients 1`
(29 bots). The after set must run the same flags, or the two are not comparable.

The two missing rows are closed at `f86cba0d`: `belt_friction_probe` completed all six cases, and
`pen_cam_slice.wgsl` is shown in a hidden capture of a recorded non-penetration. The slice is
replay evidence; the live target-ahead recorder's missing interior hull frame remains a production
defect, diagnosed without changing it. See `gap-diagnosis.bee99262.md` and the command records below.

## Rows (plan A2)

| Row | Result | Files |
|---|---|---|
| Cross-target bitprobe pair | IDENTICAL, at the fixture fix: the macOS leg at 9c766de6 (a local merge of main, whose code is eaac2a66's), the Linux leg at eaac2a66 (CI run 43747219902); startup dump and all 3072 tick payloads identical. At alpha.17 both legs panic (`src/bitprobe.rs:909`, "the differential fixture captures exactly one tank"): the fix (6d868057) keeps the fixture's tank awake through settling. It changes only the fixture, so the verdict stands for alpha.17's simulation code | `bitprobe.9c766de6.cmd.txt`, `bitprobe-diff.9c766de6.txt`; the alpha.17 attempt: `bitprobe.264515e5.cmd.txt` |
| Track scorecard | OK: 1 passed in 86.80 s | `track-scorecard.264515e5.{md,json}` |
| Track probes | `belt_friction_probe` OK at f86cba0d: all six cases, full table in `track-probes/belt_friction_probe.f86cba0d.txt` (command/load/lock record beside it). Retired contact bodies are named and their motion series end; no fallback sample. The original incomplete five-case 264515e5 output is retained. `creep_stall_probe` does not exist at this commit: 2665f22b deleted it (and `soft_run_probe`) before alpha.17. `rubble_climb_probe` OK, 51 lines, 501.27 s. `turn_probe` OK: the filter runs `turn_probe` and `pivot_probe`, both passed, 188.29 s | `track-probes/` |
| Parked on a pile | OK: all six runs valid | `pile-park/` |
| Scheduled collapse | The plan's command (30 real clients) is INVALID on this machine: run on the debug and on the release binaries, every client missed the harness's 20 s launch watchdog (`src/net/harness.rs:358`). Recorded with `--tanks 30 --clients 1` (29 bots, one real client), the hosted runs' workload: COMPLETE, 59 windows. The after set uses the same flags | `collapse/` |
| Tick attribution | Per system: `scripts/perf/tick-trace.sh` and `trace_profile.py` landed (the prep file's proposal) and run: 1920 empty-window and 7680 combat-window ticks profiled. Recorded a second time after the profiler learned to refuse malformed records and bad begin/end pairs; the pre-fix profiler wrote identical tables from that trace (the record says how much the second run moved from the first). Per stage: the collapse run's `tick-attribution.json` | `tick-trace/`; cache `collapse/tick-attribution.264515e5.json.gz` |
| Damage-model cost harness | OK: `idle1`, `fire1`; tables per role | `cost/` |
| Shader pipelines compile | OK: every pipeline the first 30 s created compiled, 0 ERROR lines | `shader-compile-gate.264515e5.*` |
| Shader captures | All 15 plans produced their frames. `lod-showcase.ron` requests no frames, so its scene ran under `lod-showcase-frames.264515e5.plan.ron` (frames added; the 14 s frame was byte-identical to the 10 s one and is not kept). The tab-board wrapper exited 1 from its own shell bug after both frames were written | records `captures/`; frames cache `captures/` |
| Capture gaps | Eight added plans in `scripts/capture/`, each with its frames: `impact-vfx`, `render-scale`, `pen-cam-xray`, `pen-cam-killcam`, `pen-cam-glass`, `optic-mask`, `muzzle-vfx`, `pen-cam-slice`. The last replays a recorded NonPen through the shooter corner at f86cba0d; all seventeen shaders now have coverage (below) | records `captures/`; frames cache `captures/` |

## Shader coverage

Which frame shows each of the 17 files in `assets/shaders/` (by eye, unless marked DERIVED).

| Shader | Shown in |
|---|---|
| `terrain_blend.wgsl` | every outdoor frame, e.g. `p1-normal`, `p3-hill`, `p7-farplane` |
| `construction_surface.wgsl` | `example`, `impact-vfx`, `wood-preview` |
| `construction_holes.wgsl` | imported by both construction shaders; the hole in `example-00012` and `impact-vfx-00011` |
| `construction_surface_prepass.wgsl` | DERIVED: the construction material's prepass and shadow shader, drawn with every construction frame above |
| `tank_surface.wgsl` | `hull-closeup`, `muzzle-vfx`, `pen-cam-killcam-00029.5` |
| `track_belt_mesh.wgsl`, `track_belt.wgsl` (its import) | `hull-closeup` |
| `render_scale_upscale.wgsl` | `render-scale` (its log: "3D main pass 640x360 inside a 1280x720 target") |
| `vfx_trail.wgsl` | `impact-vfx-00010.05` to `-00010.3` (the round's flight smoke) |
| `vfx_billboard.wgsl` | `muzzle-vfx-00063` (the 88's muzzle sprites and ground blast); the impact's sprites in `impact-vfx-00011` are faint |
| `optic_mask.wgsl` | `optic-mask-00030` (the glass centred; at 12 s the surround fills the frame). A UI material: the surroundings plans run `ui: false` and never draw it |
| `pen_cam_look.wgsl` | `pen-cam-xray`, `pen-cam-killcam`, `pen-cam-glass` |
| `pen_cam_look_belt.wgsl` | `pen-cam-xray` (the belts in the live x-ray) |
| `pen_cam_glow.wgsl` | `pen-cam-killcam`, `pen-cam-glass` (the round's path) |
| `pen_cam_outline.wgsl` | `pen-cam-killcam`, `pen-cam-glass` (the struck components' silhouettes) |
| `pen_cam_picture.wgsl` | `pen-cam-glass` (the record's fields drawn as one volume read from its depth pictures). `pen-cam-killcam`'s round stopped in the ammunition and drew no field |
| `pen_cam_slice.wgsl` | `pen-cam-slice.f86cba0d.png` (cache `captures/`): the top-right inset's hatched plate cut and stopped path, inspected by eye. Offline replay of the existing simulated NonPen report; UI enabled. `captures/pen-cam-slice.f86cba0d.cmd.txt` records the hidden run and its `Slice on Corner` mark |

The capture frames are hidden-window renders: never read a frame time out of them. A shot's aim is
picked through the window's centre, so the plans that fire keep the hidden window's logical size
(1280x720 at scale 1).

## In the cache

`MANIFEST.sha256` lists every file; by directory:

| Path | What |
|---|---|
| `captures/` | every capture frame the records in `captures/` name, and `*.half.jpg`, half-size viewing copies of the wood-preview frames, never for pixel comparison |
| `wood-preview-*.264515e5.png` | the five full-size wood-preview frames (15 MB each) |
| `bitprobe-macos-aarch64.264515e5.obp.gz` | the alpha.17 macOS leg's partial dump, written before the panic |
| `collapse/tick-attribution.264515e5.json.gz` | the recorded collapse's per-stage tick attribution |
| `rubble-climb-csv/` | the rubble climb probe's 51 per-tick CSVs (13 MB) |
| `turn-probe/turn_probe.264515e5.txt.gz` | the turn probe's printed lines with every per-tick series (64 MB raw); `track-probes/turn_probe.264515e5.txt` drops the series |
| `pile-park-run/` | the pile run directory (218 MB) |
| `collapse-run/` | the recorded collapse run directory (`rows.json`, `server.log`, the full `tick-attribution.json`) |
| `collapse-debug-attempt/`, `collapse-release-attempt/` | the two invalid 30-client runs' logs |
| `tick-trace-run/` | the recorded tick trace run's server and client logs (its 13.3 GB chrome trace was deleted after profiling) |
| `tick-trace-first-run/` | the first tick trace run, profiled before the profiler's fix and replaced: its profiles, tables, record and logs |
| `bitprobe/` | both full bitprobe dumps (26 MB each), named and hashed in `bitprobe.9c766de6.cmd.txt` |
