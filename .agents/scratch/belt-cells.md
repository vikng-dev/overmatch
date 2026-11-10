# Belt cells: covered faces bound the partition

Subject: belt support in a video game's authoritative driving simulation. Game state only.

Decision: retain a cell whole unless a face wins positive covered area; prepare complete polygon
faces from Parry's topology. No contact cap, switch, force-law edit, or patch coalescing.
The resulting quadrature can make shallow contact firmer because engagement reads each patch's
maximum depth. This is a feel consideration even without optional coalescing.

## Diagnosis and counts

MEASURED baseline: the deterministic partition fixture failed before the fix. A rotated box's
12 boundary triangles gave 6 entry triangles and 55 cells; a ring-sampled convex hull's 224
triangles gave 112 entry triangles and 11,698 cells. The count bound failed on the latter.
Source: `/private/tmp/belt-cells-baseline.log`.

MEASURED after the coverage check: the same entry triangles gave 25 and 813 cells, respectively.
MEASURED after polygon preparation: the box gave 3 entry quads and 28 cells; the hull gave 64
entry polygons and 529 cells. Counts include both column halves and uncovered ground cells.
Source: `/private/tmp/belt-cells-fixed.log` (the temporary comparison, removed before commit).

| Fixture | Before cells / entry face | After cells / entry face | After cells / (entry face x half) |
|---|---:|---:|---:|
| Box | 55 / 6 = 9.17 | 28 / 3 = 9.33 | 4.67 |
| Many-faced hull | 11,698 / 112 = 104.45 | 529 / 64 = 8.27 | 4.13 |

Ratios are DERIVED from the measured counts. The complexity test reads its boundary budget from
faces covering the column, with two half-column seeds. These fixtures remain within one cell per
boundary per half plus the seed; there is no need to combine force patches. Polygon preparation
need not lower every particular count: it removes diagonals, but also changes subdivision order.

The old loop cut every cell at a face's infinite lines, including cells the face never touched or
could not beat in depth. The new intersection check includes both the finite face boundary and
the depth comparison before emitting any split. Surfaces still prepare once per collider per tick.

Pinned API verification: `Cargo.lock` selects Parry 0.27.0. Its local source exposes cuboid quads
through `Cuboid::support_face`, and hull polygons through `ConvexPolyhedron::faces`, `points`, and
`vertices_adj_to_face`. The manifold's `local_support_feature` truncates hull polygons to four
vertices; that helper is unsuitable. The preparation test includes an eight-sided prism cap.

## Geometric and force comparison

MEASURED one-time differential diagnostic, old infinite-line partition versus the new polygon
partition (the comparison code is removed):

| Fixture | Covered length, old -> new | Pressure integral, old -> new | Pressure first moment, old -> new |
|---|---|---|---|
| Box | 0.39635694 -> 0.39635694 | 0.52135694 -> 0.52135694 | (1.0494076, -0.005547969, 0.52135694) -> (1.0494075, -0.005547985, 0.52135694) |
| Many-faced hull | 0.7572418 -> 0.7572433 | 1.2480667 -> 1.2480674 | (2.4976199, -0.000899266, 1.2480668) -> (2.4976256, -0.000899139, 1.2480674) |

These normalized integrals divide projected area/volume by the fixture's shoe width, as the
production law does. The differences are float slivers. The field is invariant; its quadrature
cells, individual centroids, and maximum depths are not identical.

MEASURED engagement diagnostic: with a fixture threshold of 2 and stiffness factored out,
`sum(pressure * clamp(max_depth / threshold, 0, 1))` changed 0.41819227 -> 0.45022333 for the box
and 1.0742303 -> 1.115275 for the hull (DERIVED +7.66% and +3.82%). This tests normalized shallow
engagement sensitivity, not a measured change in shipped vehicle force. Uniformly scaling all
depths and the threshold preserves those percentages. The force formula remains unchanged;
claiming identical downstream force inputs would be incorrect because patch grouping changed.
Player judgment remains appropriate for shallow support on solids. No historical output pin is
left in the tests.

## Instrumentation and verification

`belt_partition_faces` counts raised faces admitted per column; `belt_partition_cells` counts
cells after overlay across its two halves. They flow through the existing work rows and scorecard.
`belt_boundary_builds` replaces the obsolete `belt_triangulations` name for lazy surface preparation.

Tests cover remote-face rejection, complete polygon topology, positive-depth crossings, disjoint
coverage, depth samples, pressure moments, reversed face order, alternate fan triangulations, and
a boundary-sized work budget. The original infinite-line algorithm is exercised as a temporary
mutant diagnostic and is removed before commit.

Runtime counts and final gates follow. Hosted timing, full CI after the orchestrator pushes, and player judgment of shallow support remain.

MEASURED supplied historical stall window: in `price-contact/r4/rest3-3-contact/server.log`
(commit `30142c0a`), elapsed 66-75 s contains 414 reported ticks. Dynamic belt contacts per tick
were min 122, median 3,756, max 577,027. Tick 4171 had 577,027 contacts with 77 candidates,
440 slab checks, 103 slab AABB passes, and 11 surface preparations. The historical trace has no
face/cell counters. A new rehearsal can compare workload counts, not assert paired trajectories.

MEASURED mutant witness: the original split loop emitted 2 cells for a face wholly outside the
partition half; the fixed loop emitted 1. The permanent test requires 1, so restoring the original
loop fails. Source: `/private/tmp/belt-cells-invariants.log`. The old implementation is deleted.

For the orchestrator's hosted timing run, set `TIMING_TARGET` to the running server's `IP:PORT`
and `TIMING_DEPLOYMENT` to its deployment ID; start that server with the recipe environment emitted
by `--print-server-env`. Compare against alpha.13 on the hosted allocation. No timing was priced
in this lane. Exact client-side invocation from the bay:

```sh
scripts/perf/collapse.sh --target "$TIMING_TARGET" --deployment "$TIMING_DEPLOYMENT" --schedule scripts/perf/rest-3.json --tanks 30 --clients 1 --duration 80 --out target/belt-cells-priced
```


## Completed rehearsal

MEASURED: the hidden, unpriced local rehearsal completed with 30 tanks, one real client, one
scheduled event, 79 complete stats windows, and 5,109 recorded ticks. Artifacts:
`target/belt-cells-rest3/`; summary: `/private/tmp/belt-cells-counts.json`.

| Elapsed 66-75 s | Historical trace | Fixed rehearsal |
|---|---:|---:|
| Reported ticks | 414 | 582 |
| Dynamic belt contacts / tick, minimum | 122 | 0 |
| Dynamic belt contacts / tick, median | 3,756 | 0 |
| Dynamic belt contacts / tick, maximum | 577,027 | 9 |

These are MEASURED counts from different runs and commits, with different resulting trajectories.
They are not paired-frame force equivalence or a hosted timing claim. The deterministic partition
fixture supplies the causal comparison.

MEASURED over the entire fixed rehearsal: dynamic contacts peaked at 61; partition faces at 33;
partition cells at 112. Aggregate cells / admitted face was 24,055 / 6,185 = 3.89 (DERIVED).
In the 66-75 s window it was 1,564 / 98 = 15.96 (DERIVED), with at most 59 cells in a tick.
The highest per-tick ratio over the run was 62 cells / 1 face at tick 5102, which emitted only
2 dynamic contacts. Per-face work also depends on polygon edge count; the counters include
uncovered ground cells that do not necessarily emit a positive-pressure contact.

Decision after the rehearsal: omit optional coalescing. The maximum absolute work is small
(112 cells / tick); a face-count-only ratio hides polygon boundary complexity, and coalescing
would further alter quadrature/force grouping. The remaining high-arity case is explicit here,
not hidden behind an aggregate ratio. This lane does not promise one patch per face.

Reproduction command used:

```sh
OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1 scripts/perf/collapse.sh --rehearsal --schedule scripts/perf/rest-3.json --tanks 30 --clients 1 --duration 80 --out target/belt-cells-rest3
```

## Gates

- `cargo fmt --all --check`: passed.
- CI clippy, `scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`: passed (`/private/tmp/belt-cells-clippy-final.log`).
- Final targeted tests, `scripts/serial-cargo test --lib --features dev_ui,bitprobe -- track::world_contact track::rubble cost::stages net::server_stats doc_citations ui_ascii net_boundary net::protocol`: 92 passed (`/private/tmp/belt-cells-targeted-final.log`).
- `headless_test`: 40 passed, six measurement/scorecard tests ignored, in the integration run (`/private/tmp/belt-cells-gates.log`). That run exposed only the new sampling helper's containment test; the helper was corrected to finite-polygon winding and the final eight convex tests and targeted set passed. Production code did not change after the headless run.
- Counts rehearsal: complete; no timing priced. The harness's capacity output is not a capacity claim for this lane; hosted pricing remains the orchestrator's.
- All six lenses reviewed locally; no backlog row names a touched file. Contact reduction, vendored physics, and `src/net/sync_margin.rs` were not edited.

The old partition and differential reporting code are removed. Only invariant tests remain.
