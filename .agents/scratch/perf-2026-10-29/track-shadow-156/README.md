# Track shadow ribbon reuse, issue #156

Baseline: `86d6d2d` (the landed contact cache). Candidate: this lane's ribbon reuse.
The existing same-value transform guards from #168 already cover shoes, road wheels, sprockets
and idlers, so their production write paths are unchanged.

Each bound side retains its hull-local joints as an exact equality key. The first valid fit builds
the mesh, topology, winding and attribute buffers; later changed shapes overwrite only positions
in the existing buffer. Joint scratch storage stays allocated. Identical joints skip mutable asset
access altogether. Root motion is absent from this key; new local contacts still update the shape.
No epsilon, coarser geometry, contact policy or phase policy was introduced.

The station count, winding and cross-section are fixed for a bound ribbon. Rebinding creates a fresh
cache. Invalid or differently sized station lists retain the last valid ribbon. The caster readiness
swap is preserved: shoes keep casting until a nonempty proxy exists. Static mode still bakes once;
live modes follow changed joints. The opaque shadow path reads positions only; the visible diagnostic
uses an unlit material. Normals and UVs keep the existing vertex layout without subsequent writes.

## Gates

MEASURED results:

- `cargo fmt --all --check`: pass.
- `scripts/serial-cargo clippy --locked --all-targets --features dev_ui,bitprobe -- -D warnings`: pass.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe track::view::tests`: 10 passed.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe track::shadow_proxy::tests`: 8 passed.
- Both capture binaries built through `scripts/serial-cargo build --locked --bin overmatch --features dev_ui,bitprobe`.

`settled_standing_tank_writes_no_shoe_wheel_or_ribbon_data` drives the actual view system with
blueprint-derived fixture geometry. ASSUMED fixture cadence: a fixed virtual step of `1/128 s`,
`768` warmup frames and `256` observation frames. MEASURED: zero changed view transforms and zero
mutable mesh-asset resource access during observation. Cold placement writes, and a subsequent
phase change resumes writes. Transform propagation and render extraction rely on that invariant.
The fixture runs construction and presentation schedules without physics, windows or asynchronous
asset loading; its geometry is data, not a vehicle-specific pass condition.

`a_deforming_ribbon_builds_indices_once_and_retains_its_buffers` exercises both loop windings.
MEASURED: one topology construction per ribbon across `63` subsequent deformations, retained
index/position/joint buffer addresses, unchanged indices/normals/UVs, and exact agreement of positions
and indices with a fresh build on each deformation. Shadow extraction relies on this connectivity
and shape invariant.

## Capture reproduction

Both runs use the same assets and hidden capture options from #155. Each game process runs under
one `/usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock` and exits after saving its screenshots.
No frame-time measurements or A/B/B/A runs were taken. The orchestrator owns performance pricing.

ASSUMED fixture configuration: fallback washboard course, placement `(0, -84)`, heading `0`,
scripted drive beginning at app second `13`, capture times `12.5` (standing) and `14` (moving),
virtual step `0.0078125 s`, image size `1280x720`, camera offset `(8, 1.6, 2)` metres, no UI/gizmos.
The absent map identifier selects the existing course fallback. These are capture inputs, not tank
logic or asset-specific system gates.

```sh
export OVERMATCH_CARGO_LOCK_WAIT_S=10800
OVERMATCH_OFFLINE=1 SPIKE_SIM_WINDOWED=1 OVERMATCH_MUTE=1 \
OVERMATCH_MAP=track-fit-fixture OVERMATCH_TRACK_PRINTS_DEMO=0,-84,0 \
OVERMATCH_CAPTURE=.agents/scratch/perf-2026-10-29/track-shadow-156/after/belts.png \
OVERMATCH_CAPTURE_AT=12.5,14 OVERMATCH_CAPTURE_SIZE=1280x720 \
OVERMATCH_CAPTURE_UI=0 OVERMATCH_CAPTURE_GIZMOS=0 \
OVERMATCH_CAPTURE_STEP_S=0.0078125 OVERMATCH_CAPTURE_FOLLOW=8,1.6,2 \
/usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock target/debug/overmatch
```

`before/` and `after/` retain the raw screenshots. The comparison reuses #155's `compare.py`,
including every RGB pixel, without alignment, blur, resizing, masking or edge removal.
ASSUMED comparison configuration: zero channel tolerance and rectangle `(235,225)-(1065,650)`,
excluding right/bottom limits. This extends the earlier belt rectangle downward to include its
cast shadow on the rough ground. Whole-frame statistics are retained as well.

```sh
python3 .agents/scratch/perf-2026-10-29/track-fit-155/compare.py \
  .agents/scratch/perf-2026-10-29/track-shadow-156/before \
  .agents/scratch/perf-2026-10-29/track-shadow-156/after \
  --crop 235,225,1065,650 --tolerance 0 \
  > .agents/scratch/perf-2026-10-29/track-shadow-156/comparison.json
```

## Pixel parity

MEASURED results at zero tolerance:

| Frame | Whole-frame identical pixels | Belt-and-shadow identical pixels | Maximum RGB channel difference |
|---|---:|---:|---:|
| Standing | 100% | 100% | 0 |
| Moving over raised obstacles | 100% | 100% | 0 |

Each whole frame contains MEASURED `921600` pixels; the rectangle contains MEASURED `352750`.
The unrounded statistics are in [comparison.json](comparison.json). Visual inspection of the rough
moving frame confirms the belt follows the raised course and its cast shadow is preserved.
There is no residual visual acceptance decision from these stills. Performance pricing remains
with the orchestrator.

- Standing: [before](before/belts-00012.5000s.png), [after](after/belts-00012.5000s.png).
- Moving: [before](before/belts-00014.0000s.png), [after](after/belts-00014.0000s.png).
