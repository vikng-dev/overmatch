# Hidden meshes' removal churn, issue #173

Labels: MEASURED, DERIVED, ASSUMED, CLAIMED. No frame time was measured: the runner prints frame
statistics, and they are not part of this evidence.

The patches live in the vendored `bevy_pbr` (`vendor/bevy_pbr-0.19.1-scalar-math/OVERMATCH_PATCH.md`,
"Mesh instance removals"). The upstream draft is `upstream/bevy-invisible-mesh-removal-churn.md`.

## Removals per frame at 30 tanks

Hidden offscreen runs (`arm.sh`: `OFFSCREEN=1 scripts/perf/run-ablation.sh`, 15 s warmup and 20 s,
`OVERMATCH_PROBE_TANKS=30 SPIKE_MESH_REMOVALS=1`, muted, one lock hold), two dev builds with
`--features dev_ui,bitprobe`:

- **before**: `08dafad`, the counter on stock `bevy_pbr` 0.19.1;
- **after**: `3659ff9`, the counter, the removal batching and the removal skip.

The raw `mesh_removals:` lines are in `counts-before.txt` and `counts-after.txt`. Each line covers
about 5 s. The first line of each run is startup and is left out below; the after run's second line
(101 frames) is still loading.

| | Frames | Removals queued | Of them with an instance | Channel sends |
|---|---|---|---|---|
| before | 1,701 | 13,676,214 (8,040 per frame) | 33 | one per removal: 8,040 per frame (DERIVED) |
| after | 1,234 | 43 (0.03 per frame) | 43 | none (DERIVED from the source: no removal channel) |

All counts MEASURED except where marked; per-frame figures are DERIVED from them. Before, 99.9998 %
of the queued removals had no instance to remove (DERIVED). The issue's ~8,400 per frame at 30 tanks
was measured on an earlier commit (`bookkeeping-diagnosis.md`); this run's 8,040 agrees with it.

## The lifecycle tests

`tests.txt`: `src/tests/bevy_mesh_removal_lifecycle.rs` on the M4 under Metal (MEASURED). The five
lifecycle tests pass on stock, with the batching, and with both patches. The contract test, one
removal per hidden stretch, fails on stock and with the batching alone (on the frame the meshes go
invisible, and on every frame they move), and passes with the skip.

The tests need a GPU adapter, so they are `#[ignore]`d and CI does not run them. Command:
`scripts/serial-cargo test --lib --features dev_ui,bitprobe bevy_mesh_removal -- --ignored`.

Found on the way, outside this path: inserting or removing `NoIndirectDrawing` on a live camera
fails wgpu validation in stock 0.19.1 (`preprocess_gpu_indexed_frustum_culling_bind_group` meets
the `mesh preprocessing (direct)` pipeline; MEASURED). The tests switch the culling mode by
replacing the camera instead. Not drafted upstream.

## Stills

`capture.sh <binary> <png-dir> <arm>`: hidden, muted, a fresh config directory, 1920x1080, no UI
or gizmos, a fixed step of `0.0078125 s`, a frame at 10 s, one lock hold per scene.

| Scene | What it shows |
|---|---|
| `duel` | the default spawn and camera |
| `close` | the controlled Tiger from 6 m, 2.5 m up (`OVERMATCH_CAPTURE_FOLLOW`) |
| `probes30` | 30 tanks: the duel and the probe grid 43..91 m ahead |

`before/`, `after/` (JPEG, quality 92) and `side-by-side/` (before left); `sbs.py` made them from
the PNGs, and `comparison.json` is its pixel comparison: all three pairs are byte-identical
(MEASURED, 0 differing pixels).

One capture differed. The after binary's first `probes30` run outlined every tank's edges
(`difference/probes30-after-first-run.jpg`: 172,786 differing pixels, largest channel change 176;
MEASURED), which reads as a slightly different settled pose. A second run of the same after binary
is byte-identical to both runs of the before binary (MEASURED), so the after stills use that run.
The cause of the one-off was not isolated. `shadow-bake-170/README.md` reports the same kind of
hairline pose difference between runs and attributes it to asynchronous asset loading (CLAIMED
there, not checked here).
