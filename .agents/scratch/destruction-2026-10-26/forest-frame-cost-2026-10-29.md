# Forest frame cost: segmented trees against graybox cones (issue 123)

MEASURED on 2026-10-29, Apple M4.
- **Binary:** release with `track_prints_demo` (see `.agents/scratch/perf-2026-10-29/bin/BUILD.txt`).
- **Display:** a visible frontmost window on the built-in 120 Hz display, `vsync_mode: Off`.
- **Machine:** quiet (no builds), on AC power.
- **Runner:** `scripts/perf/run-forest-cost.sh`, with 15 s of warmup and 60 s measured per condition.
- **Gates:** every validity gate passed on all four conditions: occlusion, stream span, monitor and surface, settings loaded, and the scatter count of 635 firs as trees or as cones.

The baseline is the cones condition of the same scene, with the same binary; only `prototypes.ron` maps the firs to graybox cones.

| Scene | Condition | p50 ms | p95 ms | p99 ms |
|---|---|---|---|---|
| forest (`OVERMATCH_START=540,570,0`, ~48 trunks within 150 m in view) | cones | 4.51 | 7.08 | 8.29 |
| | trees | 5.24 | 8.39 | 9.56 |
| duel (default, no fir in frame) | cones | 4.64 | 7.29 | 8.56 |
| | trees | 5.72 | 8.78 | 9.88 |

**Deltas, trees minus cones:**
- forest: +0.73 ms at p50 and +1.31 ms at p95;
- duel: +1.09 ms at p50 and +1.49 ms at p95.

**Reading.** The trees cost more in the scene where no tree is drawn. So the cost is not rendering: it matches the forest's simulation cost measured earlier (+1.08 ms of tick at idle with 9,768 colliders, a convex collider per segment). The rendered trees, within ~150 m in the forest scene, cost no more than the cones and may cost less (+0.73 against +1.09). That last reading is within two runs' noise and is INFERRED, not measured separately.

**The lever** is the collider count, not the meshes. Possible routes: broadphase or sleeping for static trunks, one collider per tree until it is hit, or fewer segments.

Raw runs: `.agents/scratch/perf-2026-10-29/runs/forest/`, with each condition's log, stream and manifest.
