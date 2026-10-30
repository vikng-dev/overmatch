# Shadow instruments for #169

The instrument half is implemented. The orchestrator owns pricing and product choices.
Run `BIN=target/debug scripts/perf/run-shadow-pricing.sh <out-dir>` with the binary built
from the landed change; the script enforces hidden offscreen runs and the machine lock.
Its probe split/overlap values are ASSUMED inputs, preserving the DERIVED shipped
350 m envelope. The split probe with two cascades holds that count fixed in its baseline.
Controls, syntax and application gates are documented in `scripts/perf/run-ablation.sh`.

Caster exclusions resolve through render policy. Mesh descendants inherit the nearest
caster class; tank roots and owned parts supply tank classification even after detachment.
Terrain LODs and fallback course blocks, tree views and flat graybox canopy/trunk meshes,
and building views declare their respective classes. Receiving and camera channels retain
their existing roles. These are boot instruments, without persisted product settings.

Actual per-cascade indirect command counts are skipped. In the installed
`bevy_render-0.19.1/src/render_phase/mod.rs`, `BinnedRenderPhase.batch_sets` is
`pub(crate)` and `RenderMultidrawableBatchSet.bins` is private. The public
`IndirectParametersBuffers` aggregates a phase across views. Extracting per-cascade
command counts would require renderer changes or additional GPU readback. Existing
per-view set counts and aggregate indirect commands remain; the census explicitly logs
this limitation. The final part of a directional shadow view label identifies its cascade.

## Verification

`cargo fmt --all --check`, CI clippy with `--locked --all-targets --features dev_ui,bitprobe
-- -D warnings`, and targeted `render_cost`, `settings::tests`, and `render_policy` tests
passed. Shell syntax checks passed for both runners. The tests name caster isolation,
late descendants, detached owned parts, cast-only proxies, independent stock controls,
invalid conditions, and overlap-only reconciliation as invariants. The normal test/binary
link emitted the platform's compact-unwind size warning; clippy passed without warnings.

Evidence is in `/tmp/overmatch-shadow-169/`: `before.png` is the pre-edit bay binary's
hidden still; `after.png` is the changed binary at stock controls; `levers.png` is one
combined lever smoke. Each capture was requested at ASSUMED app time 5 s with UI and
gizmos disabled. All game processes ran under the machine lock, exited successfully and
were reaped. Frame/pass timing recorders were disabled; the pricing battery was not run.

`levers.log` confirms all caster exclusions enabled, Hardware2x2 applied to the camera,
bounds `[60, 180, 350]` and overlap `0.05` reconciled (ASSUMED probe inputs).
The settled aggregate shadow census is MEASURED 288 indirect commands for stock controls
and MEASURED 3 for the combined smoke. This is an instrumentation check, not attribution
or a frame-time result. The independent caster laws are covered by the targeted tests.

The pixel comparison is recorded in `pixel-diff.json` (RGB channel scale DERIVED 0–255):

| Comparison | Pixels differing in any channel | Mean absolute channel delta | Pixels with a channel delta above ASSUMED threshold 8 |
|---|---:|---:|---:|
| Pre-edit vs changed binary, stock controls | MEASURED 431,148 | MEASURED 0.069534 | MEASURED 3,224 |
| Changed binary, stock vs combined levers | MEASURED 1,030,150 | MEASURED 5.484680 | MEASURED 323,292 |

The stock captures have residual differences; they are separate startup captures rather
than an exact replay. Visual inspection shows the same stock composition and shadow
silhouette, and the combined smoke removes the tank's cast/self shadows. Exact pixel
parity is not established. The orchestrator must run the isolated pricing battery and
judge filter/split/overlap quality before selecting product changes.
