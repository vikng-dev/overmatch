# Review of the first track-print slice (#125)

Subject: the video game's cosmetic terrain marks, view only. Review target `c055d4d`;
starting checkout `ef946df`. All user-fixed decisions were preserved. No windowed binary,
Blender, network mutation, or push was run.

## Confirmed findings and fixes

- **P1, history:** every pivot update appended an immortal operation and cell index, even when
  it changed no pixels. `cf70740` introduced exact spatial checkpoints; the performance fix
  retains hot checkpoints raw and compresses only on cache turnover. History remains exact
  across negative coordinates, cell boundaries, recentres, and cold-cell reactivation.
- **P2, grounded edges:** tilt quantization clipped slopes at 127/256, despite accepting shoes
  tilted through 60 degrees. `e3f766e` expands the encoded range so accepted edge contacts survive.
- **P2, world transition:** a synthetic 0.16 m sole centered at Z=0 fits between the world layer's
  old sample rows and contributes no coverage. `7895bcb` replaces four samples with 64 samples,
  at near-scale spacing, stored sparsely in CPU mask tiles. GPU dimensions and formats stay fixed.
- **P2, sliding:** stamping only a frame's final pose leaves gaps when the shoe moves farther
  than its length. `3df0605` resamples continuous contact motion, with separate episodes after
  airborne motion and resets.
- **P2, frame work:** full 64-square mip rebuilds amplify small world updates. World update
  tiles are now 8-square; sample-mask allocation remains 64-square. Hot history avoids periodic
  compaction bursts. Near recentres upload only genuinely changed storage tiles, including
  zero clears of previous marks; empty startup sends no tiles.

## Measurements

MEASURED on Apple M4, Rust 1.98, the repository's `edit` profile, features `dev_ui,bitprobe`.
Command (headless, through the compile queue):

```sh
OVERMATCH_PRINT_BENCH_FRAMES=3600 scripts/serial-cargo test --lib --features dev_ui,bitprobe \
  thirty_tank_pivot_cpu_and_upload_measurement -- --ignored --nocapture
```

Synthetic fixture: 30 tanks, each with 54 continuously grounded shoes; flat 1025-square height
oracle over 1500 m; pivot 1.08 rad/s at 60 Hz (ASSUMED workload). Layout spacing is 12 m clustered
and 200 m dispersed (ASSUMED). This measures contact, resampling, history, both CPU rasters,
mip rebuilds and upload packing. It excludes ECS query overhead, other shoes, render-queue/driver
writes and GPU work. These are not complete game frame times or GPU timings.

| MEASURED final, 3600 frames per layout | Clustered | Dispersed |
|---|---:|---:|
| Mean CPU per frame | 6.217 ms | 4.250 ms |
| p95 CPU per frame | 8.289 ms | 5.123 ms |
| Maximum CPU per frame | 15.657 ms | 11.313 ms |
| Mean upload payload per frame | 31,859 B | 1,109 B |
| Maximum upload payload per frame | 1,004,373 B | 32,085 B |
| Empty startup payload | 0 B | 0 B |
| Retained history payload | 3,932,160 B | 3,407,872 B |
| History cells | 120 | 104 |
| Logical stamps processed | 11,662,380 | 11,662,380 |

The payloads equal those after the shorter 180-frame run: repeat visits do not grow history.
Mean uploads fall as repeated pivots saturate the same footprints; the maximum is the more useful
busy-frame transfer figure. Payload counts exclude allocation, extraction metadata and driver overhead.

For comparison, the functional fixes with batched checkpoints and 64-square world updates measured
9.870/12.150 ms mean and 29.138/19.240 ms p95 over 180 frames (clustered/dispersed). Those numbers are
**not** a measurement of unmodified `c055d4d`.

Memory remains proportional to visited area: the raw active-history cache is at most 8 MiB
(DERIVED, 256 cells times 32 KiB), while cold checkpoints persist as RLE or raw, whichever is
smaller. Full-map incompressible near detail can still require approximately 1.91 GiB
(DERIVED, 32000-square texels times two bytes), plus indexing. No exact persistent fine raster can
promise the research note's rolling-only 5–20 MB estimate for arbitrary full-map coverage.
The sparse world masks have a DERIVED full-occupancy ceiling of 128 MiB, plus their index and mips.
Sampled coverage still cannot guarantee arbitrarily thin subtexel geometry; its spacing matches
what the near store can resolve.

## Verification

Before each code commit: `cargo fmt --all --check`; CI clippy through `scripts/serial-cargo`
with `--locked --all-targets --features dev_ui,bitprobe -- -D warnings`; targeted `track_prints`,
`terrain_blend`, and `track` tests. Final filters: 16 passed / one manual measurement ignored;
2 passed; 174 passed / one manual measurement ignored. The ignored measurement was explicitly
run and passed separately. Test linking reports the existing macOS compact-unwind size warning;
clippy passes with warnings denied.

Regression checks cover edge-contact encoding, world sample phases and union, continuous sliding,
airborne/reset episodes, exact toroidal recentres, bounded cold/hot history including reactivation,
raw fallback, changed-tile uploads and stale-mark clears, and every world mip's average.
Existing bake and mirror tests state their lower-envelope/opening invariants. The no-vehicle-name
and dedicated-server composition tests pass. No hard standards violation was found in the
introduced runtime code; each regression names its consumer and the mutant it excludes.

API audit used the actually locked Bevy 0.19.1 / Avian 0.7.0 / wgpu 29.0.4 sources in the Cargo
registry and the vendored `bevy_pbr`. Checked image construction and COPY_DST usage, zero-init,
mesh attributes/indices and coordinate frames, entity/hierarchy/query APIs, system ordering and
extraction, GPU image lookup, queue-copy layout, demo APIs, material bindings and shader sampling.
`PrepareAssets` precedes `PrepareResources`; `Queue::write_texture` permits unpadded rows;
R8/Rg8 UNorm are filterable; bindings 100–110 match Rust; the borrowed repeat/clamp samplers match
the near/world addressing. No additional sampler, server print resource, wire change or sim state
was introduced. Shader runtime was not rerun: this review remains headless.
