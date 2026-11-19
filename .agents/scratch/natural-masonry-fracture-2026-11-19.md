# Natural masonry fracture in the cutter

**Decision:** keep simulated chunks convex and cheap, but redraw the cut so its seams follow the bond's own line families with seeded sizes, and draw every chunk as the whole bricks it owns. This is C (restricted to bond-lattice vertices) plus B, with D done in the view only. A is rejected. Nothing was built or run; every figure below is read from source or derived.

## 1. What reads as toy blocks

From the three wall timelines and the pile pictures (1280 px sheets; fine mortar detail is not resolvable):

- **Opening outline.** In `b-opening-timeline.png` and `b-corner-timeline.png` the hole beside the window is a plus or staircase of right angles. Its steps are one chunk tall (0.225 m, three courses) and half a chunk wide; real toothing steps one course (0.075 m).
- **Identical near-cubes.** The pieces in flight and in the heaps (`staged/cutter/pile-*.png`, `look-1/after/pile-*.png`) are the same block repeated.
  - 701 of 994 brick chunks are axis-aligned boxes; 397 are exactly 0.26 x 0.225 x 0.25 m (MEASURED, main's baked asset).
  - 988 of 994 have 8 corners (MEASURED).
- **Regular lattice.** Rows are a fixed three courses, on one datum across every piece of a wall. Vertical seams alternate by half a module, so the chunks themselves form a running bond of giant bricks.
  - Of brick-to-brick joints, 1614 are bed, 822 vertical and 147 slanted (MEASURED).
- **Flat faces with the wall's picture on them.** Every chunk face, break faces included, takes the element's surface and metre UVs (`src/construction/view.rs:319-350`, uv at `:344`; `Faces::chunk` `:378-387`). A lump's bed and end faces show a miniature brick wall.
- **Chunk against brick.** A chunk is one stretcher module by three courses by the full wall thickness, 0.014625 m³ median (MEASURED). That is 5.8 brick-with-joint volumes of 0.26 x 0.13 x 0.075 m (DERIVED).
  - Brick size is from data: 250 x 120 x 65 mm in 10 mm joints (`tools/constructions/house_1.house.ron`, comment above `course_m: 0.075`, `module_m: 0.26`; the GOST 530 single format, recalled, agrees).
- **Missing.** No single bricks or halves anywhere, and no ragged edge finer than a chunk.

## 2. The cutter today (main)

- **Patterns.** `Whole`, `Courses`, `Cells` (`tools/src/construction/fracture.rs:75-80`). The pattern is named by construction data; the cutter reads no material (`:1-5`, and the code agrees).
- **`Courses`** (`fracture.rs:219-334`):
  - Row boundaries every `row_courses x course_m` from the datum (`:226-243`).
  - Candidate cuts on the bottom course's head joints, offset half a module on alternate rows (`:260-265`).
  - With `crack_chance` each end of a cut moves up to `crack_shift_m` off its joint (`:288-298`).
  - `min_width_m` refuses slivers (`:274-283`).
  - Every cut is one plane across the whole row (`:314-331`). The module doc's "cuts its middle course through half a unit" only describes that straight plane crossing a brick.
- **Authored numbers** (`house_1.house.ron`, `parts.walls.cut`):
  - `row_courses` 3, Resolution.
  - `crack_chance` 0.2, Taste.
  - `crack_shift_m` 0.09, Taste.
  - `min_width_m` 0.08, Resolution.
  - `course_m` and `module_m` are construction facts.
- **Result.** 636 head-joint cuts and 149 cracks (MEASURED, `staged/cutter/bake.log:6`).
- **Run-time geometry.** A chunk is a convex hull of baked corners (`src/construction/schema.rs`, `Chunk::hull`).
  - Colliders are `Collider::convex_hull` (`src/construction/breakage.rs:2574`, `src/construction/sweep.rs:171`).
  - The law block is `Block::hull`.
  - The view mesh is built at run time from the same hulls (`view.rs:433-457`). There is no baked view mesh.
- **Break faces.** No interior material exists. The Brick row holds an unused candidate, `feedback.exposed_linear_rgb` (`assets/materials/materials.ron`, Brick, from `:610`). The brick visual is `brick_wall_006` at 3 m per repeat (`assets/materials/visuals.ron`).
- **Brick seam.** Tensile 0.69 MPa, G_I 10 J/m², cohesion 0.165 MPa, G_II 28.4 J/m². The row has no unit tensile strength and no unit fracture energy.

## 3. Constraints on a chunk shape

- **Convex and closed.** `certify.rs:1-13` requires closure, partition by volume, finite positive-definite inertia, and joints over every cut face inside a piece. The runtime refuses a hull that bounds no solid (`breakage.rs:227`).
- **Joints.** A joint is the overlap of opposed coplanar faces of at least 1 mm² (`tools/src/construction/contact.rs:1-21`). It is found across piece boundaries too, so rows need not line up between pieces. Each stores area, centroid, normal and bond.
- **Wire.** The network carries indices only:
  - `Event::{Wear, Dust, Crack, Hole, Separate}(u16)` (`breakage.rs:397-405`).
  - `Piece.chunks: Vec<u16>` (`:670-680`).
  - A hard limit of 65,535 chunks and joints (`:218-225`).
  - Shape costs no wire bytes; chunk count costs 2 bytes per chunk per loose group.
- **Asset size.** Main's house is 762,771 bytes; the 0.95 MB figure is `lane/rubble-staged`, 948,969 bytes (both MEASURED).
  - Brick chunks number 994 on both branches.
  - On main a brick chunk costs 304 bytes and a brick joint 111 bytes (MEASURED).
  - A lattice corner costs 21 bytes of text; a free Voronoi corner costs 26 (MEASURED, brick against wood chunks).
  - Headroom on staged is 51 kB to 1,000,000 bytes (DERIVED).
- **Size floor.** A body is kept when the cube root of its summed volume is at least 0.5 m (`assets/ballistics/physical_laws.ron:9`, `src/debris/crumble.rs:234-237`). That is 0.125 m³, about 9 of today's chunks (DERIVED). Smaller matter is a view-only shed (`src/debris/decor.rs:1-10`, at most 256 drawn) or fill.
- **Server.** One static hull collider per standing chunk of a touched element, and one joint row per joint in the bond solve. I found no measured per-chunk or per-joint cost.

## 4. What real brickwork does (all recalled, no source read)

- Bed joints are continuous weak planes; head joints are staggered. In-plane cracks therefore run in three families: along a bed joint, as a stepped rake rising one course per half brick, and as a vertical zipper alternating head joints and units.
  - With this brick the rake is atan(0.075 / 0.13) = 30 degrees above horizontal (DERIVED).
- A crack goes straight through a unit when that is cheaper than the detour around it.
  - The detour around one course costs G_I x 0.065 + G_II x 0.13 = 4.3 J per metre of wall thickness (DERIVED from the Brick row).
  - Through the unit costs G_unit x 0.065. With a recalled, unsourced clay-brick G of 50-100 J/m², that is 3.3-6.5 J per metre.
  - The two are comparable, so a mix of toothed and straight is the expected look.
- Debris is lumps of several bricks still mortared, single bricks and halves at the broken edges, and dust.

## 5. Options

- **A, toothed simulated chunks: rejected.**
  - A toothed chunk is not convex, so it needs a compound collider and a non-convex law block; every face, hole-table and clip routine assumes one hull.
  - A three-course vertical seam becomes 5 faces instead of 1. Joints go from 2,583 to about 2,583 x (2/3 + 5/3) = 6,030, adding 380 kB at 111 bytes each (DERIVED). That is over the 1 MB limit.
  - The interlock it would add is not something a player can perceive.
- **B alone: not enough.** Teeth on today's lattice still give identical lumps on a regular grid.
- **C as free Voronoi: rejected for bytes and slivers.** About 12 corners at 26 bytes against 8 at 21 adds 144 bytes per chunk, or 143 kB over 994 chunks (DERIVED). Staged would reach 1.09 MB. Cells smaller than a brick would own no unit.
- **C restricted to the bond lattice, plus B: chosen.**
- **D as simulated bricks: rejected.** 13.4 m³ / 0.002535 m³ = 5,290 units (DERIVED), about 5.3 times the chunks. The unit level is kept in the view only.

## 6. The design

**One generic idea in two layers.**
- A pattern has units (the material's visible indivisible pieces, on a lattice) and cells (the convex simulated chunks).
- The cutter draws cells whose boundaries lie on the unit lattice's line families.
- The view draws each cell as the units it owns.
- Other materials take different data through the same mechanism:
  - Ashlar stone: a larger unit and a low through-unit chance.
  - Concrete: no units, so the view is the hull.
  - Timber: stays on `Cells`, where the grain is its unit direction.

**Cutter: `Courses` redrawn.** Same `Cut { bottom, top }` planes and the same convex trapezoid prisms.
- Row height is drawn per piece and row from a weighted list of course counts.
  - The stream is seeded by the piece, so bed seams stop at piece boundaries and no wall-long seam survives.
  - The half-row merge at a gable apex stays.
- Along a row, the next cut's distance is drawn from a weighted list of half-module run lengths.
- Each cut draws a class:
  - *Zipper*: vertical on a head joint.
  - *Rake left or right*: the top moves by row courses x module / 2, the bond's own slope, so the plane passes through brick corners.
- `min_width_m` and the section limits (`fits`) stay. A rake that does not fit falls back to a zipper, then to no cut.
- Every vertex is a lattice point, so corners stay short decimals.
- `crack_chance` and `crack_shift_m` are deleted; through-unit breaks move to the view rule below.

**View: units owned by cells.**
- The asset carries the bond per element (origin, along, up, course, module, through-unit chance). It is view-only; the simulation never reads it.
- At load each unit, a through-thickness prism clipped to its element, is given to the chunk holding its centre.
- A unit that a chunk boundary bisects (the zipper's alternate courses) is decided by a hash of asset, element and unit index:
  - With the through-unit chance it is split on the plane.
  - Otherwise it goes whole to one side.
- Ownership is a partition, so both sides of a seam agree by construction: no gap and no overlap in an intact wall, and no seed to keep in step.
- `Faces::chunk` then emits per owned unit:
  - its two wall faces, with today's UVs;
  - a side quad wherever the neighbouring unit has another owner.
- Largest gap between drawn and simulated shape: 0.13 m on a zipper tooth, 0.065 m on a rake (DERIVED). This is the stand-in: nobody can tell the collider is a trapezoid.

**Break faces.**
- A side quad not on the element's original exterior is interior. The test already exists for skins (`skin_tint`, `view.rs:413`).
- Unit bed and head faces take a mortar colour.
- Split-unit faces take the row's `exposed_linear_rgb`.
- Start with the vertex colour that is already plumbed; add a flat lane only if the wall picture still shows through.

**Hole edge.** The standing wall's outline becomes real brick toothing: bed runs, 30-degree rakes stepping one course at a time, and zippers with seeded handedness and occasional clean breaks.

**Loose bricks (second stage, view only).**
- When a joint between a leaving chunk and a standing one separates, a seeded share of the units along it leave both views and fall as single-unit sheds. This deepens the teeth.
- A chunk that sheds below the floor draws as its units.
- This is the hierarchy: lump body, then toothed sheds, then bricks, then fill. It adds no chunks, joints or wire bytes.

**Data added, with jobs.**

| Field | Job | Starting value |
|---|---|---|
| `row_courses` list | Taste (lump height mix); smallest entry Resolution (no sliver hulls) | 2:2, 3:2, 4:1 |
| `run_half_modules` list | Taste (lump length mix) | 2:3, 3:2, 4:1 |
| `rake_share` | Taste (stepped against vertical seams) | 0.5 |
| `through_unit_chance` | StandIn for the crack-path energy law; names the missing unit fracture energy | 0.2 |
| mortar colour, Brick `feedback` | Taste | by eye |
| `min_width_m` | Resolution, unchanged | 0.08 |

## 7. Cost (DERIVED unless marked)

- **Chunks.** Mean cell is 2.8 courses x 2.67 half-modules = 0.21 x 0.347 m = 0.073 m², against 0.0585 m² today. Brick chunks fall to about 994 x 0.0585 / 0.073 = 800. Edge merges are ignored.
- **Joints.** Brick joints scale to about 2,070. Unaligned rows at piece boundaries add back an assumed 10%, about 200.
- **Asset on staged.** About 949 − 59 (chunks) − 57 (joints) + 22 (boundary joints) + 4 (bond rows, 27 brick elements) = 0.86 MB.
  - At an unchanged 994 chunks it would be about 0.975 MB, so the mean cell must not shrink.
- **Server.** Fewer hulls, each still 8 corners, and fewer joints. No new per-tick work.
- **Wire.** No format change and no new bytes; pieces list fewer indices.
- **Client.** At most 2,749 units x 8 quads, about 44,000 triangles with every element swapped, against about 12,000 today. Ownership is computed once per asset.
- **Format.** `FORMAT` goes from 8 to 9. `house_1` is rebaked; `fir_1` is rewritten for the format but its cut is unchanged.

## 8. Build outline (line counts DERIVED from the code each step replaces)

| Step | Where | Lines |
|---|---|---|
| Redraw `courses()` (116 lines today), rule fields and checks, module doc | `tools/src/construction/fracture.rs` | 120 |
| Cutter tests: lattice vertices, no seam shared by adjacent rows, same bytes on a second bake | `tools/src/construction/tests.rs` | 90 |
| Bond record and its checks; bake writes it | `src/construction/schema.rs`, `tools/src/construction/house.rs` | 70 |
| Unit ownership at load | `src/construction/view.rs` | 90 |
| Unit faces in `Faces::chunk` (`Faces` is 110 lines today) | `src/construction/view.rs` | 110 |
| Interior classification and tint | `src/construction/view.rs`, `materials.ron` | 50 |
| View tests | `src/construction/view.rs` | 100 |
| Loose bricks (second stage) | `src/debris/decor.rs`, `view.rs` | 120 |

Total about 630 lines, plus 120 for the second stage.

## 9. Cheapest experiment

All on `lane/rubble-lab`, same seed and aim. The "before" is the existing `wall/b-opening`, `wall/b-corner` and `wall/arm-b` clips.

1. **Tint only** (about 40 lines, no rebake): interior faces take `exposed_linear_rgb`. Run `scripts/rubble-lab run burst <new dir> --clips`.
2. **Cutter redraw** (about 120 lines plus a rebake, no runtime change): run `burst` again, and `ram` for lumps on the ground.
3. **Unit-owned view** (about 270 lines plus the format bump): same two runs.

What to look at:
- The wall and close cameras at the after-collapse checkpoint.
- Whether the outline still shows right-angle blocks or aligned seams.
- Whether any two lumps in the heap look alike.
- Whether tooth edges land on the painted mortar lines.
- Whether a lump's broken face still shows a miniature wall.

A rebake moves which chunk the round meets, so compare the look, not the opening's area.

## 10. Risks

- **Texture against bond.** `brick_wall_006` is a photograph at 3 m per repeat; its painted bricks may not sit on the 0.26 x 0.075 m lattice. Teeth would then cut painted bricks. The fix is a visual whose brick grid is stated in data and UVs scaled from the bond; it may need a different texture.
- **Element boundaries.** Units are clipped per element, so a hole edge on a pier-to-sill boundary is straight. Ownership per component would fix it but needs whole elements drawn as units beside a swapped one.
- **Hole shader.** It reads the chunk's hull planes; a tooth fragment lies up to 0.13 m outside its chunk's hull.
- **Z-fighting.** Coincident opposite-facing tooth sides rely on back-face culling, as shared chunk faces do today.
- **Teeth through other matter.** A resting lump's teeth sink up to 0.13 m into the ground or a neighbour, and a round can pass through a drawn tooth.
- **Rebake.** Every construction's chunk indices move; any fixture pinned to an index breaks.
- **Bake determinism.** It holds only if every draw stays on the named `Stream`; the second-bake test is the guard.

## 11. Not verified

- Nothing was built, baked or run; all counts and bytes for the new cut are derived, and the 10% boundary-joint allowance is an assumption.
- All masonry behaviour and the unit fracture energy are recalled.
- Whether the 1 MB limit is 1,000,000 or 1,048,576 bytes.
- The texture's brick grid, back-face culling, and how `src/construction_surface.rs` and `construction_holes.wesl` would take an interior colour or outside-hull fragments: neither file was read in full.
- Whether `Choice<T>` (`tools/src/construction/spec.rs`) accepts a weighted list; whether `certify`'s interface check accepts unaligned rows across pieces (only its doc was read).
- Per-chunk and per-joint server cost, and the client's mesh rebuild time at four times the triangles.
- Whether `Separate` and `Dust` events carry enough to place loose bricks without a new wire field.
- `src/construction/fracture/bondsolve.rs` and the vendored Avian fork were not read.

### Critical Files for Implementation
- /Users/Yan/github/vikng-dev/overmatch/tools/src/construction/fracture.rs
- /Users/Yan/github/vikng-dev/overmatch/src/construction/view.rs
- /Users/Yan/github/vikng-dev/overmatch/src/construction/schema.rs
- /Users/Yan/github/vikng-dev/overmatch/tools/constructions/house_1.house.ron
- /Users/Yan/github/vikng-dev/overmatch/assets/materials/materials.ron
