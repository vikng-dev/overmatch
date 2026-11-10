# Belt cells: split where a triangle wins covered area

Subject: belt support in a video game's authoritative driving simulation. Game state only.

Decision: use the collider's boundary triangles, with each triangle supplying its own normal
and depth plane. Retain a cell whole when a face wins no positive covered area. No cell cap,
force-law change or optional alternate path.

## Verified geometry contract

Parry 0.27.0 `src/shape/convex_polyhedron.rs::from_convex_mesh` sets
`eps = DEFAULT_EPSILON.sqrt()`, removes edges when adjacent triangle normals have dot product
above `1 - eps`, and walks across removed edges transitively. A merged face inherits its seed
triangle's normal. `src/math/mod.rs` defines `DEFAULT_EPSILON = f32::EPSILON`.
DERIVED: the angular threshold is about 1.506 degrees; one degree over one metre gives about
17.46 mm plane displacement. A chained merged outline need not be planar.

`src/transformation/to_trimesh/convex_polyhedron_to_trimesh.rs` fans each merged outline using
its actual vertices. Thus topology still comes from Parry; the restored path computes a plane
from each emitted triangle's cross product and never uses the merged face normal. This is the
same triangle source as `2c3e4d41`. There is no polygon extraction in this lane's final code.
`ConvexPolyhedron::scaled` also multiplies normals by scale; recomputing from triangle vertices
avoids that normal error for nonuniform scaling.

## Partition fixture

MEASURED with the overlay loop verified against `2c3e4d41`, then with the coverage guard restored:
the rotated box has 6 entry triangles and the ring-sampled hull has 112. Counts include both half-columns and uncovered field cells.

| Fixture | Original infinite-line loop | Coverage check with triangle planes |
|---|---:|---:|
| Rotated box | 55 cells | 23 cells |
| Ring-sampled convex hull | 11,698 cells | 805 cells |

The earlier 25/813 coverage counts used normalized normals in an intermediate comparison.
Restoring the original cross-product evaluation changes float sliver subdivisions to 23/805.
These are observations, not linear complexity guarantees or asset acceptance conditions. The
fixture-count assertion was removed; `a_disjoint_face_keeps_the_cell_whole` is the regression
invariant. Coverage, pressure moments, depth crossings and triangle vertex order have independent
geometric checks. `each_triangle_owns_its_depth_plane` reconstructs a known interior point on
each of two adjacent, rotated facets one degree apart; assigning a shared normal moves that point.

## Controlled two-tank contact: counts only

MEASURED with the shipped headless composition and both tanks spawned through the production
constructor. The fixture finds the lowest declared collision proxy on one tank, selects an
upward triangle on it, and places the other tank's left belt over that surface using the loaded
running gear and half the authored engage depth. Both tanks are elevated clear of terrain.
Before each tick their root poses, velocities and drive states are prescribed again; there is
no trajectory or timing gate. Tick zero produced no admitted faces. Contacts specifically
naming the selected hull proxy witness that the intended contact really happened.

For the before run, only the coverage guard was temporarily removed. The resulting overlay loop
was compared with `2c3e4d41` and matched exactly after whitespace normalization. Triangle
extraction and depth evaluation also match that commit. The counter additions remain in both
runs so faces, cells and contacts can be read. The temporary old path is removed.

Every pair below is **before / after**. Face and cell counters cover the entire fixed tick;
`hull contacts` counts the upper tank's belt records naming the lower tank's selected proxy.

| Tick | Faces | Cells | Dynamic belt contacts | Hull contacts |
|---|---:|---:|---:|---:|
| 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |
| 1 | 57 / 57 | 120 / 115 | 103 / 98 | 85 / 80 |
| 2 | 57 / 57 | 129 / 124 | 110 / 104 | 93 / 87 |
| 3 | 52 / 52 | 107 / 103 | 97 / 92 | 78 / 74 |
| 4 | 59 / 59 | 133 / 130 | 111 / 108 | 94 / 89 |
| 5 | 52 / 52 | 110 / 105 | 97 / 92 | 79 / 74 |
| 6 | 59 / 59 | 132 / 125 | 110 / 102 | 93 / 87 |
| 7 | 55 / 55 | 113 / 106 | 105 / 100 | 82 / 79 |
| Total | 391 / 391 | 844 / 808 | 733 / 696 | 604 / 570 |

DERIVED from those rows: cells decrease 4.27%, dynamic contacts 5.05%. This placement exercises
belt-on-hull support; it does not reproduce the historical stall spike. The permanent integration
test gates positive contact with the other tank's actual proxy, never these observed counts.

## Cell growth and next root fix

For a fixed finite input the finite set of face boundary and pairwise depth-crossing lines bounds
the arrangement. Winning-area rejection prevents irrelevant faces from cutting a cell; it does
not bound cells linearly in faces, limit overlapping bodies, or remove every retained interior seam.
Every admitted face is still tested against every existing cell. There is no runtime cap.

MEASURED partition-level adversarial pile, using unscaled chunk hulls from every shipped
construction file discovered under `assets/constructions`, in path order, then stable-sorted by
volume. The pool contains 1,159 chunks. Each prefix below is centred on its authored centroid,
rotated and translated into three staggered, intersecting layers under one link. These are real
asset shapes in an intentionally intersecting input, not a physically settled packing.

MEASURED loaded track dimensions and envelope: link pitch 0.13042593 m, width 0.7247143 m,
probe reach 0.5 m, maximum droop 0.17256261 m. Coordinates are along/lateral/inward, with the
resting face at zero. Base field penetration is zero and free depth is the loaded droop.

Fixture placement for zero-based index `i`: XYZ Euler angles `(0.71i, 1.13i, 2.39i)` radians;
centre `(length*(0.1+0.8*fract(0.618i)), width*(fract(0.414i)-0.5), 0.05+0.04*(i%3))` metres.
No chunk is resized. The solid case clips the entry triangles directly; the loose case passes
those same faces through the production `across_shoe` projection first. Counts include both
halves of one column. Solid cells below equal positive-pressure contacts except the 16-piece
case, whose 758 cells include two uncovered cells; loose cells all carry positive pressure.

| Chunks | Source triangles | Solid entry faces | Solid cells | Loose shadow faces | Loose cells |
|---|---:|---:|---:|---:|---:|
| 16 | 188 | 70 | 758 | 90 | 10 |
| 64 | 764 | 311 | 3466 | 411 | 19 |
| 128 | 1552 | 625 | 6841 | 832 | 37 |
| 256 | 3144 | 1250 | 10649 | 1660 | 57 |
| 512 | 6244 | 2492 | 16227 | 3259 | 65 |

MEASURED maximum in this constructed family: **16,227 cells per column** with solid coverage;
the corresponding loose-piece path gives **65**. This does not show a settled loose pile making
16,227 contacts: its cross-width projection is a different, much smaller partition. Face processing
still grows with all admitted faces. The former fixture inequality is not a bound: at 64 chunks,
3,466 solid cells exceed `2*(1+5*311) = 3,112` (DERIVED).

The next root fix, if the pile remains expensive, is a finite-segment upper-envelope representation
that discards buried boundaries and interior seams before choosing integration cells. Its
quadrature needs an explicit feel decision because the existing engagement law reads each cell's
maximum depth. A numerical cap would silently change coverage and is not the fix.

## Player-visible limits

The triangle depth field matches the original path. Covered area and the linear pressure integral
are preserved to clipping roundoff. Partition grouping changes centroids and per-cell maxima;
shallow engagement can therefore feel firmer. Deep, flat support keeps its fully engaged pressure.
No measured shipped-force percentage or hosted performance claim follows from these count runs.

## Earlier rehearsal: scenario not demonstrated

MEASURED in the first lane: the local hidden rehearsal used 30 tanks, one client and an 80 s
schedule. It recorded 5,109 ticks; dynamic contacts peaked at 61, admitted faces at 33 and cells at
112. In elapsed 66-75 s, dynamic contacts had minimum 0, median 0 and maximum 9 across 582 reported
ticks. The supplied historical trace at `30142c0a` had minimum 122, median 3,756 and maximum
577,027 across 414 reported ticks in that window.

The stall scenario was not demonstrated in the rehearsal. Those trajectories are unpaired, so
577,027 and 9 are not a before/after reduction. The controlled fixtures supply causal count
comparisons. The earlier polygon-based measurements are superseded by the triangle measurements.

## Instrumentation and validation

`belt_partition_faces` counts raised faces per affected column; `belt_partition_cells` counts the
two half-column partitions. Empty columns avoid the face-count atomic update. Surface triangles
prepare once per collider per tick under `belt_boundary_builds`.

The hand-written `Stages::default` is required because `WORK_COUNT` exceeds the standard library's
array `Default` implementations (lengths 0 through 32); it zeroes the counters with `from_fn`.
Verified against [Rust 1.98.0's array implementations](https://github.com/rust-lang/rust/blob/1.98.0/library/core/src/array/mod.rs), whose `array_impl_default` invocation starts at 32.

The disjoint-face mutant emitted two cells and failed its required one-cell assertion. The
restored guard, triangle-depth test and all six other tests in the measurement run passed.
Temporary comparison and count-printing code is removed; no old path or new ignored test remains.

Gates passed:

- `cargo fmt --all --check`.
- CI clippy: `scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`.
- Full gate run: `scripts/serial-cargo test --lib --features dev_ui,bitprobe -- track::world_contact track::rubble cost::stages net::server_stats doc_citations ui_ascii net_boundary net::protocol headless_test`: **132 passed, six existing measurement/scorecard tests ignored**.
- Final targeted recheck: the same filters with `belt_bears_on_another_spawned_hull_proxy` in place of the entire `headless_test` module: **92 passed**. This follows restoration of the existing boundary-test `bitprobe` guard; the requested feature configuration's headless wiring is unchanged.
- The headless runs used `OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1`.
- Local diff reviewed against all six lenses; no backlog row names an edited file.

No timing run was performed. The full suite runs on CI after the orchestrator's push.
Hosted pricing and player judgment of shallow support remain with the orchestrator. Exact hosted
client command, with the target and deployment supplied by the operator:

```sh
OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1 scripts/perf/collapse.sh --target "$TIMING_TARGET" --deployment "$TIMING_DEPLOYMENT" --schedule scripts/perf/rest-3.json --tanks 30 --clients 1 --duration 80 --out target/belt-cells-priced
```
