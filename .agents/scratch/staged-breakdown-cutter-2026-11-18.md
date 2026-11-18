# Staged breakdown through the cutter

Subject: the game's destruction model (pieces, joints, loads and fill), game state only. Everything was read at `142a1519` with `git show`; nothing was run, built or edited. Paths are under `/Users/Yan/github/vikng-dev/overmatch/`. "Recalled" marks a number that is in no file I read.

## 1. What the source says

**The cutter**
- It is Rust: `tools/src/construction/fracture.rs`, driven by `tools/src/construction.rs:63-75`. No Blender script takes part; `scripts/prop/build.py` only writes the tree's `.pieces.ron`.
- Input per piece is a convex solid, a surface frame (`Upright` or `Grain(axis)`) and a `Pattern` named by construction data (`fracture.rs:75-80`). It reads no material: "Nothing here reads what a piece is made of" (`fracture.rs:1-5`).
- There are three patterns: `Whole`, `Courses` (rows on bed joints, cuts on head joints, `:219-334`) and `Cells` (Voronoi cells stretched along the grain, `:336-408`).
- Masonry in `house_1` uses `Courses`: 994 chunks, median extents 0.225 x 0.25 x 0.26 m, mean volume 0.013-0.015 m³ (MEASURED from the baked asset).
- Timber uses `Cells` with `size_m 0.15, stretch 5.0` (`tools/constructions/house_1.house.ron`, `parts.plates` and `parts.roof`). The tree uses stretch 8.

**How a board is cut today**
- One board has 1 to 8 chunks, mean 3.77. The distribution over the 26 boards is 1:3, 2:4, 3:7, 4:3, 5:3, 6:4, 7:1, 8:1 (MEASURED).
- Chunk length along the grain runs from 0.42 to 6.6 m, mean 2.08 m. Three boards are a single 6.6 m chunk and can never snap (MEASURED).
- The cause is in the site rule. Sites are drawn on a lattice of 0.15 x 0.15 x 0.75 m and "a site outside the piece is dropped" (`fracture.rs:353-368`). A board's section is 0.01 m² against a 0.0225 m² cell, so most draws miss.
- Expected sites per board are 0.0665 / 0.016875 = 3.9 (DERIVED), which matches the measured 3.77. The stated 0.75 m resolution is not what the cutter delivers on a thin member.
- The tree shows the same defect: `fir_1` has 37 chunks, 1.05 to 11.3 m long (MEASURED).

**What the bake records per joint**
- `area_m2`, `centroid`, `normal` and `bond: Seam | Attached(kind)` (`src/construction/schema.rs`, `Joint` and `Bond`). No strength and no material are stored; the runtime reads those from the chunks' material row and `attachments.ron`.
- Inside one board there are 90 seam joints in total, with area 0.0012-0.169 m², mean 0.021 m² (MEASURED).
- Their planes are slanted: |normal · grain| has mean 0.56 and minimum 0.02 (MEASURED). The stretch metric tilts cuts into long slants.

**How a joint breaks, and whether bending is in the law**
- Bending is already there. An edge carries a full wrench through the face's area moments (`src/construction/fracture/bondsolve.rs`, `Patch::face`, `Edge::couple`).
- `Edge::stress` returns the peak fibre tension at the polygon's corners, `axial/area + gradient · r` (`bondsolve.rs:296-325`).
- The kernel judges each joint inside a part: `tension > tensile` gives `Cause::Tension`, otherwise shear against cohesion plus friction (`src/construction/fracture/parts/kernel.rs:270-292`).
- A free group has its rigid acceleration removed first (`kernel.rs`, `hold`). So a multi-chunk member landing on one point takes a bending moment at each joint, and the most overloaded joint cracks first (`src/debris/crumble.rs`, `crumble`).
- Cracks are also gated by energy: fracture energy times area per crack (`crumble.rs`, `crack_work`), never more than the landing dissipated.
- So a long member of several chunks does snap at mid-span today. No span term is missing.

**What is wrong is the strength, not the law form**
- The seam's `tensile` is one scalar for every joint, whatever its normal.
- The `Wood` row states 2.4 MPa, tension perpendicular to grain, "the weaker way" (`assets/materials/materials.ron`, `Wood.bulk.tensile`).
- A board 0.2 x 0.05 m bent flat has section modulus 8.33e-5 m³, so it holds 200 N·m (DERIVED).
- Its own weight over a 6.6 m span gives 240 N·m (DERIVED: 29.6 kg, wL²/8). At that strength an end-supported board cracks under its own weight.
- One cross-cut costs 327 J/m² x 0.01 m² = 3.3 J. A 3 m fall carries 870 J, which pays for about 266 cuts (DERIVED).
- The `GreenNorwaySpruce` row makes the opposite choice: its `tensile` is the 36 MPa modulus of rupture along the grain, and its energy is 22,340 J/m². The two wood rows give the same field opposite meanings.

**Every judge of size**

| Where | Measure |
|---|---|
| `crumble.rs:234-237` `server_body`, used by `route` (`:669-679`) from release (`src/construction/breakage.rs:1693`), sweep intake (`breakage/sweep_intake.rs:1450`) and landing (`crumble.rs:632`) | cube root of summed chunk volume against `server_rubble_min_size_m` 0.5 |
| `src/rubble_bed/deposit.rs:156-175` | each chunk's world AABB footprint, with volume / packing |
| `src/debris/decor.rs:184` | farthest corner from the fragment's centre |
| `src/contact_law.rs:41` | radius of the sphere of the chunk's volume |
| `bondsolve.rs:392` | half the cube root of chunk volume, as the compliance length |
| `src/construction/fracture.rs:302` | sphere-of-volume inertia |
| `src/debris/freeze.rs:108` | a count of pieces, not a size |

A whole board, a single element, is routed at release at 0.404 m and becomes a `Shed`.

**What materials hold for a fracture pattern**
- `Wood`: density 448; tensile 2.4 MPa across grain; G_I 327 J/m²; shear cohesion 8.9 MPa parallel to grain; G_II 740 J/m²; compression 38.2 MPa along and 5.6 MPa across; Young's modulus 118 MPa tangential.
- It has no along-grain tension or rupture, no along-grain fracture energy and no along-grain modulus (12.5 GPa appears only inside a comment).
- `Brick` (`materials.ron:610-688`): 0.69 MPa, 10 J/m², cohesion 0.165 MPa, 28.4 J/m², no grain.
- The grain direction is the element's `surface_frame`. The runtime lays it into the material (`fracture.rs`, `Material::laid`), but only compression reads it, through Hankinson's formula (`Material::bearing`, `fracture.rs:269-280`).
- No row holds a fracture spacing.

## 2. Design

Three truths are missing. None of them names a board or a roof.

**A. The cutter delivers the resolution its data states (cutter fact).**
- `Cells` lays its lattice in the piece's own section, not in the construction frame.
- Across the grain, the column count per section axis is `max(1, round(extent / size_m))`, so a member thinner than a cell is one cell across.
- Along the grain, every station of `size_m x stretch` that the piece occupies gets its site, drawn inside the piece's part of that cell instead of being dropped.
- The stretched metric and the jitter stay, so ends remain slanted and lengths uneven.
- Spacing stays `Resolution` and `Taste` in construction data, as the module doc says. I reject a "characteristic fracture spacing" in `materials.ron`: where a beam snaps is decided by its load, so that number would be a resolution dressed as a fact.
- `Courses` is unchanged; brick already breaks on courses.
- A board becomes 6.6 / 0.75 = 8.8, so 9 chunks of about 0.73 m, each joint a full-section cut (DERIVED). Plates become about 16 (15 today). Trunks get a cut every 1.2 m.

**B. The seam is resolved by the grain (material property).**
- A grained row states its seam both ways: across the grain (today's `tensile` and `fracture_energy`) and along it (rupture strength and along-grain opening energy).
- A joint reads the value for its own normal against the element's grain, as compression already does.
- For the form I choose fibre crossing: `T(n) = T_across + (T_along - T_across)(n · g)²`, and likewise for energy (DERIVED: both the fibres crossing a unit of plane and their share of the traction go as cos).
- I reject Hankinson here. It describes off-axis loading where the failure plane is free to follow the grain; a baked plane is not free. At |n · g| = 0.56 it gives 3.4 MPa against 24.8 MPa (DERIVED), which would leave slanted cuts nearly as weak as today.
- Brick has `Grain::None` and is unchanged.
- With a recalled dry red spruce rupture of 74 MPa, the board holds 6.2 kN·m, 26 times its own-weight moment. A snap needs about 3.7 kN at mid-span of a 6.6 m bridge (DERIVED).
- At the green row's 22,340 J/m², one snap costs 223 J, so a 3 m fall pays for at most 3 (DERIVED).
- Timber then lands whole on a flat bed and snaps where it bridges and is struck. Each snap halves the span, which quadruples the load the next one needs.

**C. Size is the silhouette (the floor's measure).**
- Size is the square root of the product of the fragment's two longest extents, in its own oriented box. In player terms: how big the piece looks lying on the ground, which is also what a track meets.
- A thin splinter still counts as small (2 x 0.05 m gives 0.32 m). A board counts (6.6 x 0.2 m gives 1.15 m).
- I reject longest extent: every 0.73 m stick would stay a body, up to 234 for this house (DERIVED).
- I reject the intermediate extent: a board would read 0.2 m.
- I recommend starting the floor at 0.7 m, as the owner's `Taste`. For a one-brick wall, 0.7² x 0.25 = 0.1225 m³, against today's 0.125 m³, so masonry behaves as now and only timber changes.
- At 0.5 m a wall cluster would need only 0.0625 m³, doubling the masonry bound from 8 to 16 bodies per m³ (DERIVED).

**Live bodies after a collapse (DERIVED)**
- Masonry is unchanged at 0.7 m; the measured 7 stands for it. I assume, unverified, that the two nailed roof slopes are among those 7.
- If the slopes stay nailed, the count is 7.
- If every nailed joint parts and nothing snaps, it is 5 + 26 boards + 2 plates = 33.
- The ceiling is 2 bodies per board (a body needs 0.49 / 0.2 = 2.45 m, so 4 chunks, 2.9 m), 4 per plate pair, plus masonry: 5 + 52 + 4 = 61.
- At 0.5 m the timber ceiling is 4 per board: 112.
- For comparison, 39 with the old weld and 7 with the prototype (MEASURED, fair test).

**The chain end**
- Fill is any fragment under the floor by silhouette. At 0.7 m that is timber lengths under 2.45 m, masonry clusters under about 9 chunks, and every single chunk (largest: a board chunk at 0.38 m, a brick chunk at 0.25 m).
- Timber's chain is 6.6 m, then two lengths of about 3 m, then fill drawn as its own 0.73 m sticks seated in the bed. The prototype already keeps real shed meshes on fill (`decor.rs:1-8`).
- Masonry's chain is panel, then clusters of 9 or more chunks, then fill.
- Still wrong and out of this scope: timber volume joins one brick-weight bulk (`src/rubble_bed/bearing.rs:345`).

**Server cost of long thin bodies**
- The count is bounded by the one floor: 2 per board at 0.7 m.
- Each body carries at most 9 hull colliders, each with a small AABB of about 0.75 m.
- Snaps per landing are bounded by energy (3 per board per 3 m fall).
- A settled island turns static and costs nothing (`src/debris/freeze.rs:5-18`).
- Before freeze, expect cost near the control's 2.07-2.31 ms at 39 bodies (MEASURED there), not the prototype's 1.28-1.49 ms.
- The 0.05 m thickness is below the 0.12 m a board travels per tick at 7.7 m/s (DERIVED, 3 m fall at 64 Hz). Thin boards may pass through each other; ground and bed are thick.

## 3. Build outline (line counts DERIVED from the sizes of the code each step replaces)

**Cutter and bake, about 200 lines**
- `cells()` rewrite in `tools/src/construction/fracture.rs` (73 lines today): +60.
- Section axes and extents helper in `tools/src/construction/geom.rs`: 40.
- Module doc: 15.
- Tests in `tools/src/construction/tests.rs`: 85.
- Rebake `house_1` and `fir_1`. No schema change; `FORMAT` stays 8.
- `house_1` chunks go from 1,122 to about 1,258. Board seam joints go from 90 to 208; nailed joints rise by a few hundred.
- The file grows from 762,771 bytes by about 100 kB, 13% (DERIVED from about 370 bytes per chunk and 105 per joint, a split I estimated from the total).

**Runtime, about 330 lines**
- Two along-grain seam fields in the substance registry, stated together or refused: 50.
- Grain-resolved seam in `src/construction/fracture.rs`, read by `JointStrength::of`, `crack_work` and the part interfaces: 90.
- Silhouette size replacing `server_body`, with chunk hull points passed to `route` at its three callers: 50.
- Tests: 120.
- Data: two wood rows and the floor's text: 20.

**Cheapest first experiment**
- Free, today's build: fell a fir and drive onto the trunk. It is long timber under these same laws at 36 MPa, snapping only at its uneven baked cuts.
- First build: the runtime half alone, with no rebake. Roof boards come down as boards, lie across the heap and snap where bridged, at today's uneven cuts, with 3 boards that cannot snap.
- I do not recommend shipping the size measure alone, about 90 lines. At 2.4 MPa and 3.3 J per cut, boards would shatter to sticks on first landing.
- The cutter half follows once the owner has seen boards persist.

## 4. Decisions

- Spacing stays construction data; the cutter is fixed to deliver it. Anisotropy goes into the material seam.
- The size measure is the silhouette, with the floor starting at 0.7 m.
- No bending or span term is added.
- The fibre-crossing form is chosen over Hankinson for baked planes.

## 5. Not verified

- Nothing was run. Every body count is derived, and whether nailed slopes part on landing is assumed.
- The 74 MPa rupture is recalled. No dry along-grain fracture energy was read; 22,340 J/m² is the green row's analogue, itself `Unknown`.
- The fibre-crossing form has no source.
- The baked hulls sum to 15.71 m³ (MEASURED), against the fair test's 8.82 m³ of "occupied material". I did not reconcile the two.
- Thin-body tunnelling under the Avian fork was not checked; the vendored solver was not read.
- Whether `certify` or the load gate refuses any new cut shape was not checked. Callers of `seam()` were not enumerated, so the 90-line figure is the least firm.
- Whether a stick landing on bare ground fades or stays on its own deposit was not traced.
- `bondsolve.rs:392` uses a cube-root compliance length. For a 0.73 m chunk it is about 3.8 times too stiff along the grain (DERIVED). This is harmless for a single-column member and was not examined for plates.

### Critical Files for Implementation
- /Users/Yan/github/vikng-dev/overmatch/tools/src/construction/fracture.rs
- /Users/Yan/github/vikng-dev/overmatch/src/construction/fracture.rs
- /Users/Yan/github/vikng-dev/overmatch/src/construction/fracture/bondsolve.rs
- /Users/Yan/github/vikng-dev/overmatch/src/debris/crumble.rs
- /Users/Yan/github/vikng-dev/overmatch/assets/materials/materials.ron
