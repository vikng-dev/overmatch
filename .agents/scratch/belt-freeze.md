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
A vertical sweep integrates bands between those segments. A band's owner is selected at emission from its interior centroid. Lateral edges close every band
they span, even when the side-edge keys remain unchanged.
Geometry discovery uses double precision; the existing pressure integration consumes its polygons.

The support at each point remains the highest covering triangle or field plane. Its positive
covered area, pressure integral and first moment retain their geometric meaning. No cap,
alternate path, physical parameter or per-vehicle branch is added. Bands belonging to one visible owner triangle are summed per half, including disconnected parts.
Engagement uses the maximum depth of that visible face region. Per-collider aggregation was not
chosen: it would combine distinct normals and point-motion, damping and grip samples.

The current patch bound lives in `src/track/world_contact/convex.rs`. Internal decomposition
bands do not each create a contact. Discovery cost still depends on the input geometry;
counts alone establish no hosted speedup.

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

Engagement reads each visible face region's maximum depth. Changing regions can change
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

## Visible-face regions and discovery: second pass

Subject: support contacts in the game's authoritative driving simulation. Game state only.
Baseline for every causal comparison below: `89e8970b`. Ownership and aggregation landed as
`59a8113d` and `de83a15e`; the discovery change follows them on `lane/belt-freeze`.

Verified in source: owners were cached when bands opened, lateral edges were discarded, crossing
visibility scanned every region, and every interval allocated active-edge and band-map storage.
The old tests let hiding and burial mask each other; the analytic crossing lay on the half boundary.
The review's float-frequency estimates were not measured and are not used here.

A band now chooses its owner at emission, at its f64 area centroid. Lateral events close the bands
they span even when those bands retain the same side keys. Each footprint point still belongs to
the highest covering triangle, using that triangle's depth. Bands of one owner face sum their area,
pressure integral and first moment before any support sampling. Engagement uses the maximum depth
of the entire visible face region in that half, including disconnected parts. Field regions retain
all their polygons so curved-terrain slicing preserves holes. The module owns the contact bound.

Discovery uses a balanced index of projected along/lateral bounds for burial, boundary cover and
pair-crossing queries. Crossings are clipped as finite segments against the other region; empty
intersections avoid visibility work. Rectangle boundaries and exact duplicate edges enter once.
Sweep and visibility buffers retain capacity across intervals and both halves. Emitted polygons
still own their storage; this is no claim of allocation-free integration or a universal quadratic
worst-case bound. The unconditional pair-by-all-regions scan is gone; dense overlapping input can
still require substantial discovery work. No cap or new authored physical number was added.

`belt_discovery_tests` counts exact burial and edge-cover tests after bounds pruning. It excludes
bounds-index node visits and pair-crossing/segment-event arithmetic. `belt_partition_patches`
counts the positive-pressure owner regions emitted by convex integration, before terrain refinement.
The existing cell counter continues to expose internal bands; bands are not contacts.

MEASURED on identical inputs, both halves of one column. The pile reproduces the recipe in
`belt-cells.md`: 512 unscaled chunks from the 1,159-chunk shipped pool, with 2,492 solid faces or
3,259 shoe-shadow faces. The rehearsal peak is the largest captured face count across the two new
runs, not a reconstruction of the historical 69-face column whose geometry was never retained.
Each paired entry is baseline / new.

| Input | Faces | Bands | Patches | Discovery tests | Per-collider alternative |
|---|---:|---:|---:|---:|---:|
| Captured column | 65 | 576 / 589 | 559 / 79 | 19,328 / 3,795 | 4 |
| New rehearsal column peak | 94 | 1,269 / 1,053 | 1,212 / 107 | 39,397 / 15,683 | 4 |
| 512-chunk solid pile | 2,492 | 1,286 / 1,311 | 1,286 / 196 | 11,007,411 / 2,240,907 | 77 |
| Same pile, shoe shadows | 3,259 | 24 / 22 | 24 / 19 | 391,482 / 241,685 | 6 |

The last column counts distinct support colliders (and the field) per half, over the same visible
regions. Decision: implement per-face aggregation as requested. The coarser alternative would
combine different normals and point-motion, damping and grip samples; only its potential count
is reported. It is not implemented or retained behind a switch.

MEASURED two independent local rehearsals, each running:

```sh
OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1 scripts/perf/collapse.sh --rehearsal --schedule scripts/perf/rest-3.json --tanks 30 --clients 1 --duration 150
```

Both runners exited zero. The following are independent per-tick maxima, not one simultaneous row.
All retained ticks, including collection tails, are in each run's `belt-ticks.csv` under
`target/belt-freeze2/rest3-1/` and `target/belt-freeze2/rest3-2/`. Each row includes faces, bands,
patches, dynamic contacts, discovery tests and the names of belt counters exceeding 2,000.
`target/belt-freeze2/counts.json` records maximum ticks and threshold counts for every belt counter.

| Run | Ticks | Faces | Bands | Patches | Dynamic contacts | Discovery tests |
|---|---:|---:|---:|---:|---:|---:|
| rest-3 / 1 | 9,407 | 729 | 5,116 | 869 | 821 | 57,891 |
| rest-3 / 2 | 8,994 | 1,045 | 4,147 | 1,777 | 1,709 | 53,256 |

MEASURED threshold result: no tick exceeded 2,000 faces, patches or dynamic contacts. Bands exceeded
2,000 on 1,309 / 476 ticks; discovery tests did so on 2,269 / 2,088 ticks. Of the other belt counters,
slab checks crossed it on 678 / 0 ticks and terrain samples on every tick. These are observations,
not caps or acceptance conditions. The trajectories are unpaired; they establish neither a speedup
nor hosted budget compliance. Temporary input capture only logged increasing column face counts;
it and all comparison/mutant modules were removed after measurement.

The analytic oblique crossing lies inside one half and independently derives area, pressure and
both first moments. Crossing shoe shadows cover both input orders, non-dyadic crossing positions
and non-dyadic lateral edges. Partly hidden faces and shared triangle edges have derived patch
counts; every partition fixture checks the module's generic contact bound. Disconnected-region
slicing checks the independently derived pressure and first moment. The captured placement no
longer has a data-specific positive-contact requirement and compares owner counts under reversal.

MEASURED mutation evidence: a `visible` implementation retaining the whole edge fails the visibility
trimming test; returning all regions from burial fails its prefilter test. Returning `None` from
`crossing` fails the interior analytic pressure assertion. Replacing the strict winner comparison
with `>=` fails the field-tie recipient assertion. Evidence is in `target/belt-freeze2/mutant-*.log`
and `discovery-tests.log`; none of these alternate implementations is compiled by the final tree.

Validation uses the required format check, workspace/all-target CI clippy and targeted tests for
world contact, forces, rubble, the shipped belt-on-hull witness, stage counters, server stats and
all four whole-tree guards. Final output is in `target/belt-freeze2/final-gates.log`. Local review
covered all six lenses; no backlog row names an edited file. There are no collider/body constructor
changes, external writes, rebases, merges or pushes in this pass.

Player-visible limits: grouping can change shallow engagement, damping, grip and the resulting
trajectory. Those outputs are observations for play judgment, not pinned obligations. Hosted
pricing and that judgment remain with the orchestrator. Exact timing command:

```sh
OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1 scripts/perf/collapse.sh --target "$TIMING_TARGET" --deployment "$TIMING_DEPLOYMENT" --schedule scripts/perf/rest-3.json --tanks 30 --clients 1 --duration 150 --out target/belt-freeze-priced
```
