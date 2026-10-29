> **INVALID (found 2026-10-29, `focus-gap-diagnosis.md`).** These runs' window was never shown (the screen was locked overnight), so the 3D camera never drew. Every frame time below is the game loop without rendering, and "the relief is free" is NOT established. The gate that missed it is fixed in `scripts/perf/frame-run.zsh`. Re-measure.

# Track-print relief levels: frame cost (issue 125)

MEASURED on 2026-10-29, Apple M4.
- **Binary:** release with `track_prints_demo`.
- **Display:** a visible frontmost window on the built-in 120 Hz display, `vsync_mode: Off`.
- **Machine:** quiet, on AC power.
- **Runner:** `scripts/perf/run-track-prints-cost.sh`, with 50 s of warmup (the demo lays the prints and then holds the camera about 10 m off the pivot) and 60 s measured per condition.
- **Gates:** all six conditions passed every validity gate, including the hold, which began before the measurement window, with the injected relief level in force.

| Pack | TRACK PRINTS | p50 ms | p95 ms | p99 ms |
|---|---|---|---|---|
| mud | LIGHTING | 5.33 | 8.81 | 9.85 |
| mud | PARALLAX | 5.33 | 8.16 | 9.30 |
| mud | SUN SHADOW | 5.35 | 8.32 | 9.42 |
| base | LIGHTING | 5.59 | 8.62 | 9.62 |
| base | PARALLAX | 5.64 | 8.72 | 9.80 |
| base | SUN SHADOW | 5.63 | 8.70 | 9.76 |

**Reading.** Parallax and sun shadow cost at most +0.05 ms at p50 over lighting only. That is within this rig's 1–2 % run-to-run noise, so on the M4 the relief is free at the shipped 30 m range. A weaker GPU is unmeasured.

Raw runs: `runs/track-prints/`.
