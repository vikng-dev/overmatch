# Track-view contact cache, issue #155

The view caches exact directional wheel targets and lower-route depths. Unchanged presented pose,
terrain revision, print response, ground pressure and rig inputs reuse those contacts. Articulated
circles also key the wrap's contact cache. Moving targets refresh when the fixed clock advances,
after physics writeback, so the query uses the presented hull pose. Multiple fixed steps before a
render coalesce into its latest sample. Wheel, belly and slack filters, material phase, shoe placement
and the phase-sampled shadow ribbon continue at render cadence.

Depth profiles interpolate by normalized distance along the lower route, including its end arcs;
articulation can change the route's station count. The final clearance clamp uses the current circles.
Terrain swaps and pose snaps reseed filters and contacts. Print-response and rig-input changes reseed
contacts immediately, even between fixed steps. Reinstance constructs fresh cosmetic state.

This is view-only state: no simulation components, forces or spawn construction changed. Directional
`cast_ray` semantics remain intact. Prints retain their existing drawn-shoe ground-contact and sweep
checks; their contact logic is not replaced by the interpolated fit targets.

## Gates

MEASURED results:

- `cargo fmt --all --check`: pass.
- `scripts/serial-cargo clippy --locked --all-targets --features dev_ui,bitprobe -- -D warnings`: pass.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe track::view::tests`: 9 passed.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe track::wrap::tests`: 3 passed.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe track_prints::stamper::tests`: 7 passed.
- Dev capture binary built with `scripts/serial-cargo build --locked --bin overmatch --features dev_ui,bitprobe`.

The settled-fit test counts calls through the exact terrain oracle: MEASURED zero fit queries across
256 settled render frames, with scrolling phase and continued filters, plus agreement with the uncached
reference. ASSUMED synthetic fixture: 512 warmup frames, a 128 Hz render clock and alternating fixed
samples. The moving-target tests check interpolation, no between-tick probes, changing articulation,
and immediate exact reseeding. Existing wrap tests retain the swept-crest and shoe/wheel clearance gates.

No frame-time measurements or A/B/B/A runs were taken. The owner runs performance pricing.

## Capture provenance and reproduction

Baseline: `a6dbf90`, with only the capture clock and camera-follow options added. Candidate: this lane's
contact cache. Both use the same assets and capture options. All binaries run hidden, offline, under
one `/usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock`; each exits after its screenshots save.

ASSUMED capture configuration: virtual frame step `0.0078125 s`, image `1280x720`, no UI or gizmos.
The washboard fixture uses the existing fallback course: the absent map id selects the supported
flat-ground/course fallback. The controlled tank is placed at `(0, -84)`, heading `0`, and starts the
scripted drive at app second `13`. Captures at app seconds `12.5` and `14` show the standing and moving
belts over alternating raised obstacles. The side camera follows at world offset `(8, 1.6, 2)` metres.
These are fixture choices, not tank-specific logic or pass conditions.

```sh
export OVERMATCH_CARGO_LOCK_WAIT_S=10800
OVERMATCH_OFFLINE=1 SPIKE_SIM_WINDOWED=1 \
OVERMATCH_MAP=track-fit-fixture OVERMATCH_TRACK_PRINTS_DEMO=0,-84,0 \
OVERMATCH_CAPTURE=target/track-fit-155/rough-after/belts.png \
OVERMATCH_CAPTURE_AT=12.5,14 OVERMATCH_CAPTURE_SIZE=1280x720 \
OVERMATCH_CAPTURE_UI=0 OVERMATCH_CAPTURE_GIZMOS=0 \
OVERMATCH_CAPTURE_STEP_S=0.0078125 OVERMATCH_CAPTURE_FOLLOW=8,1.6,2 \
/usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock target/debug/overmatch
```

`before/` and `after/` retain the unmodified PNGs. `compare.py` compares every RGB pixel, without
alignment, blur, resizing, edge removal or masking. ASSUMED comparison configuration: channel tolerance
`8/255`; the fixed belt rectangle is `(235,225)-(1065,460)`, excluding the right/bottom limits.
The JSON also reports the whole frame, maximum difference and all pixels over tolerance.

```sh
python3 .agents/scratch/perf-2026-10-29/track-fit-155/compare.py \
  .agents/scratch/perf-2026-10-29/track-fit-155/before \
  .agents/scratch/perf-2026-10-29/track-fit-155/after \
  --crop 235,225,1065,460 > comparison.json
```

## Pixel results and visual decision

MEASURED belt-rectangle statistics, in RGB levels out of `255`:

| Capture | Within tolerance | Pixels over tolerance | Mean channel difference | P99 pixel difference | Maximum channel difference |
|---|---:|---:|---:|---:|---:|
| Washboard, standing | 99.973340% | 52 | 0.017223 | 1 | 56 |
| Washboard, moving | 97.929249% | 4039 | 0.597628 | 13 | 100 |
| Printed map, standing | 99.484235% | 1006 | 0.179253 | 5 | 85 |
| Printed map, moving | 99.804153% | 382 | 0.069035 | 1 | 76 |

The JSON retains unrounded values and whole-frame statistics. The moving washboard differences over
tolerance are confined to MEASURED pixel bounds `(268,244)-(1054,417)`, within the running gear. The
hull and terrain align. Visual inspection shows edge/shading differences from the delayed targets;
the average belt-channel change remains below MEASURED `1/255`, but strict same-coordinate `8/255`
equality is **not fully met**. No pixels are discarded from the comparison.

Decision: preserve the issue's requested tick-interpolated contact policy and report its residual
visual cost. Owner review must decide whether these edge outliers meet the intended visual tolerance;
this report does not declare strict pixel equality. Performance pricing remains the owner's run.

- Standing: [before](before/belts-00012.5000s.png), [after](after/belts-00012.5000s.png).
- Moving: [before](before/belts-00014.0000s.png), [after](after/belts-00014.0000s.png).
- [Moving absolute RGB difference, amplified by ASSUMED 8x for inspection](moving-difference-8x.png).

The supplementary printed-map stills use ASSUMED fixture configuration
`OVERMATCH_MAP=kalinovo`, `OVERMATCH_TRACK_PRINTS_DEMO=70,-20,90`,
`OVERMATCH_CAPTURE_FOLLOW=2,1.6,8` and capture times `12.5,16`, with the same remaining options.
They cover the existing print sink and contact consumers in the shipped world:
[standing before](printed-before/belts-00012.5000s.png),
[standing after](printed-after/belts-00012.5000s.png),
[moving before](printed-before/belts-00016.0000s.png),
[moving after](printed-after/belts-00016.0000s.png).
`printed-comparison.json` is reproduced with `compare.py printed-before printed-after` and the same
crop/tolerance options.

