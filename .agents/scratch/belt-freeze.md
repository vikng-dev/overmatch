# Belt freeze: visible upper envelope

Subject: support contacts in a video game's authoritative tank driving simulation. Game state only.
Base: `f49ca871`, lane `belt-freeze`.

## Capture and deterministic fixture

MEASURED on the base with temporary count capture: rest-3 and rest-1 both reproduced the
contact avalanche. Each requested 30 tanks, one hidden client and 150 seconds. Counts below
cover every tick retained in the server logs, including the collector's trailing ticks.
They are unpriced local rehearsals; live trajectories are unpaired.

The first rest-3 capture identifies two colliders on one other tank, both at the same body pose:
a compression stop (136 source triangles, 37 admitted faces) and a side plate (254 source
triangles, 28 admitted faces). The log records collider/owner entities, triangle contributions,
world poses, column basis, strip, penetration and travel. The column basis and support pose
supply the complete relative geometry used by partitioning; no chassis trajectory is required.
The fixture rebuilds all running-gear shapes through the production constructor from the
selected tank's shipped data and replays that captured geometry.

MEASURED causal comparison on that identical column:

| Quantity | Base | Final |
|---|---:|---:|
| Admitted faces | 65 | 65 |
| Cells, both halves | 14,533 | 576 |
| Positive-pressure patches | 14,501 | 559 |
| Positive covered area / shoe width, m | 0.12772520 | 0.12772526 |
| Pressure integral / shoe width, m² | 0.03159132 | 0.03159130 |
| Along pressure first moment / width, m³ | 0.0041576126 | 0.0041576130 |
| Lateral pressure first moment / width, m³ | 0.0035078484 | 0.0035078520 |

DERIVED: 96.0% fewer cells and 96.1% fewer patches on the captured input. These counts are
observations, never asset gates. Reversing the final fixture's faces produces 589 cells and
572 patches, with pressure integral 0.031591296 m². Coverage, winning depth and pressure moments
are checked under face reversal; exact cell counts are not pinned.

The original loop carries old cells' seams into a replacing face. A flat rectangular upper
surface over 32 lower triangles therefore emitted 200 cells. DERIVED: its upper envelope needs
one rectangle per column half, hence two cells. The regression failed with **200 != 2** before
the replacement and passes with two afterward. Many tanks or many intersecting solids are not
required to trigger this defect.

## Decision and cost

Build finite visible boundaries of the upper envelope before choosing integration cells.
Clip faces to the column half and field plane. Discard a face wholly covered by one higher
face. Keep face-boundary and pairwise depth-crossing segments only where no surface is higher.
A vertical sweep integrates bands between those segments. A band's owner stays fixed until its
boundaries change; unrelated edge events neither split it nor repeat its ownership search.
Geometry discovery uses double precision; the existing pressure integration consumes its polygons.

The support at each point remains the highest covering triangle or field plane. Its positive
covered area, pressure integral and first moment retain their geometric meaning. No cap,
alternate path, physical parameter or per-vehicle branch is added. Per-collider aggregation was
not chosen: normal, point motion, damping and grip are still sampled on each integration region.

DERIVED: with E visible finite edges and X proper segment crossings, endpoints and crossings
close at most `1 + 4E + 4X` bands per half. Hidden boundaries do not enter this sweep. Visibility
discovery still compares input faces and their occluders; this is not a linear-in-face-count cost
claim. Hosted pricing must assess that remaining work.

Reusing an unchanged band's owner was checked once against the captured fixture: forward counts,
pressure and moments above were unchanged. There is no retained alternate implementation or test
pinning the earlier output.

## Rehearsal counts

MEASURED independent per-tick maxima over each stored trace:

| Run | Ticks | Faces | Cells | Dynamic belt contacts |
|---|---:|---:|---:|---:|
| Base rest-3 | 9,486 | 435 | 27,061 | 26,498 |
| Base rest-1 | 9,498 | 1,396 | 127,634 | 127,405 |
| Instrumented envelope rest-3 | 8,499 | 1,002 | 4,208 | 3,776 |
| Instrumented envelope rest-1 | 9,736 | 1,022 | 4,422 | 3,974 |
| Final rest-3, capture removed | 9,202 | 327 | 1,729 | 1,429 |

The instrumented envelope runs precede owner reuse; the final run uses the committed algorithm.
The final trace extends through elapsed 157.01 s while the runner collects its requested window.
No percentage speedup follows from these unpaired trajectories or from this local allocation.

MEASURED independent logged column maxima in the instrumented envelope runs:

| Run | Faces/column | Cells/column | Positive patches/column |
|---|---:|---:|---:|
| rest-3 | 69 | 1,069 | 1,046 |
| rest-1 | 64 | 731 | 698 |

Neither instrumented envelope run crossed the 2,000-cell/patch column threshold. The base's
threshold capture reports qualifying columns at most once per second, rather than recording a
column maximum; its captured 14,533-cell column is the deterministic fixture above. DERIVED:
every affected column in the final trace has at most 1,729 cells, because the nonnegative
per-column counts sum to that trace's per-tick counter. No runtime cap imposes this observation.

MEASURED belt counts at each largest overall tick, and at the largest belt-phase tick:

| Run / selected tick | Tick | Faces | Cells | Dynamic contacts | Overlaps | Boundary builds |
|---|---:|---:|---:|---:|---:|---:|
| Base rest-3 / overall | 153 | 0 | 0 | 0 | 0 | 0 |
| Base rest-3 / belt phase | 6492 | 434 | 27,061 | 26,498 | 32 | 33 |
| Base rest-1 / overall and belt phase | 9498 | 336 | 127,634 | 127,405 | 9 | 11 |
| Instrumented rest-3 / overall | 1333 | 0 | 0 | 0 | 0 | 0 |
| Instrumented rest-3 / belt phase | 1503 | 136 | 631 | 458 | 26 | 3 |
| Instrumented rest-1 / overall | 9630 | 323 | 1,510 | 1,137 | 46 | 40 |
| Instrumented rest-1 / belt phase | 1671 | 1,022 | 4,422 | 3,974 | 80 | 4 |
| Final rest-3 / overall | 6379 | 13 | 55 | 38 | 4 | 15 |
| Final rest-3 / belt phase | 9171 | 27 | 184 | 101 | 6 | 24 |

The final rehearsal still reports over-budget ticks; removing the contact avalanche does not
establish server budget compliance. No hosted timing run was performed.

Artifacts in this bay are under `target/belt-freeze/`: each table's named run directory contains
`server.log`; `counts.json` summarizes the traces. `before-rest3-1` and `before-rest1-1` contain the
original threshold poses. `after-rest3-1` and `after-rest1-1` contain the temporary column peaks.
`final-rest3-1` contains the clean production run. The temporary server capture is deleted;
existing `scripts/perf` per-tick counters remain its production instrument.

## Player-visible limits and validation

Engagement still reads each integration region's maximum depth. Changing regions can change
shallow support, damping samples and grip quadrature. Those are emergent observations to judge
in play, not stored outcomes to preserve. No new tuned number changes that law.

Passed on the final Rust source:

- `cargo fmt --all --check`.
- `scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe -- track::world_contact track::rubble track::forces headless_test::belt_cells doc_citations ui_ascii net_boundary net::protocol --nocapture`: **109 passed, zero failed or ignored**.
- A separate run of the final artifact's world-contact tests: **17 passed**, including the captured
  fixture, analytic pressure moments, face-order coverage and the buried-face count invariant.
- `git diff --check`; local review against all six lenses. No backlog row names an edited source file.

The initial broader `headless_test` batch was stopped by its verified process PID during long
field-only ground/brake calibrations, with no full-batch verdict. Collider/body construction was
not changed. The completed gate above includes the shipped belt-on-hull integration witness;
the full suite belongs to CI after the orchestrator's push.

All rehearsals used `OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1`.
The operator's remaining work is hosted pricing and shallow-contact feel. Exact pricing command,
with target and deployment supplied by the operator:

```sh
OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1 scripts/perf/collapse.sh --target "$TIMING_TARGET" --deployment "$TIMING_DEPLOYMENT" --schedule scripts/perf/rest-3.json --tanks 30 --clients 1 --duration 150 --out target/belt-freeze-priced
```
