use std::collections::BTreeSet;

use glam::DVec3;
use overmatch::construction::schema::Bond;

use super::fracture::Pattern;
use super::geom::{Face, Polyhedron};
use super::spec::Piece;
use super::*;

/// The shipped specs, baked once per test against the shipped registry.
fn shipped() -> Vec<(Spec, Baked)> {
    specs(&crate::repo_root())
        .expect("the shipped specs parse")
        .into_iter()
        .map(|spec| {
            let baked = bake(&spec.construction, schema::substance_density)
                .unwrap_or_else(|err| panic!("{}: {err}", spec.path));
            (spec, baked)
        })
        .collect()
}

/// THE BAKE IS CANONICAL, AND THE SHIPPED FILE IS ITS OUTPUT: two bakes of every shipped spec
/// write the same bytes, the bytes parse back to the same construction through the game's own
/// reader, and they are the bytes `assets/` ships. Two peers read those bytes into the map
/// digest, so a bake that drifted (a hash-ordered list, an unrounded float, a platform's own
/// trigonometry) or a file left stale behind its spec splits the handshake.
#[test]
fn two_bakes_write_the_same_bytes_and_the_shipped_file_is_one() {
    let root = crate::repo_root();
    for (spec, baked) in shipped() {
        let id = &spec.construction.id;
        let text = write(&baked.asset, &spec.path);
        let again = bake(&spec.construction, schema::substance_density).unwrap();
        assert_eq!(
            text,
            write(&again.asset, &spec.path),
            "{id}: two bakes differ"
        );
        assert_eq!(
            ConstructionAsset::from_ron(&text).as_ref(),
            Ok(&baked.asset),
            "{id}: the text reads back as written",
        );
        let path = asset_path(&root, id);
        let shipped = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("{}: {err} — run bake-constructions", path.display()));
        assert!(
            shipped == text,
            "{} is stale against its spec or the bake — run bake-constructions",
            path.display()
        );
    }
}

/// THE JOINT GRAPH HOLDS THE CONSTRUCTION TOGETHER: every joint and anchor has positive area and a
/// unit normal, every chunk reaches an anchor through joints (one connected graph), and every
/// anchor is a downward chunk face on the foundation plane, of a chunk that reaches that plane.
/// The support law walks from the anchors over the joints; a chunk the graph cannot reach would
/// fall the moment anything changed, and an anchor above the foundation would hold a wall up in
/// mid-air.
#[test]
fn every_chunk_is_joined_to_the_ground_and_anchors_lie_on_the_foundation() {
    for (spec, baked) in shipped() {
        let asset = &baked.asset;
        let id = &spec.construction.id;
        let n = asset.chunks.len();
        assert!(!asset.anchors.is_empty(), "{id}: no anchor");
        let mut parent: Vec<usize> = (0..n).collect();
        fn root(parent: &mut [usize], mut k: usize) -> usize {
            while parent[k] != k {
                parent[k] = parent[parent[k]];
                k = parent[k];
            }
            k
        }
        for joint in &asset.joints {
            assert!(joint.area_m2 > 0.0, "{id}: joint {joint:?}");
            let normal = glam::Vec3::from_array(joint.normal);
            assert!((normal.length() - 1.0).abs() < 1e-5, "{joint:?}");
            let (a, b) = (
                root(&mut parent, joint.a as usize),
                root(&mut parent, joint.b as usize),
            );
            parent[a] = b;
        }
        let ground: BTreeSet<usize> = asset
            .anchors
            .iter()
            .map(|anchor| root(&mut parent, anchor.chunk as usize))
            .collect();
        for k in 0..n {
            assert!(
                ground.contains(&root(&mut parent, k)),
                "{id}: chunk {k} ({}) reaches no anchor",
                asset.elements[usize::from(asset.chunks[k].element)].name
            );
        }
        assert_eq!(ground.len(), 1, "{id}: one connected construction");
        for anchor in &asset.anchors {
            assert!(anchor.area_m2 > 0.0);
            assert_eq!(anchor.centroid[1], asset.foundation_y_m, "{anchor:?}");
            let lowest = asset.chunks[anchor.chunk as usize]
                .hull
                .iter()
                .map(|p| p[1])
                .fold(f32::INFINITY, f32::min);
            assert_eq!(lowest, asset.foundation_y_m, "{anchor:?}");
        }
    }
}

/// THE CHUNKS TILE THEIR ELEMENTS: each element's chunks sum to its piece's volume, and every
/// written chunk corner lies inside its own piece, on or under each of the piece's face planes.
/// The baker relies on conservation of the authored volume, and a roof's users on a board's cut
/// staying within that board. Mutant: a cut that reaches across a piece's face into the next board
/// fails, though a box test admits it wherever two boards' bounds overlap. Controlled course
/// fixtures separately check that head joints split each row.
#[test]
fn the_chunks_tile_each_element() {
    for (spec, baked) in shipped() {
        let asset = &baked.asset;
        let id = &spec.construction.id;
        for (element, piece) in asset.elements.iter().zip(&spec.construction.pieces) {
            let [first, end] = element.chunks;
            let run = &asset.chunks[first as usize..end as usize];
            let sum: f32 = run.iter().map(|chunk| chunk.volume_m3).sum();
            let whole = piece.solid.volume() as f32;
            assert!(
                (sum - whole).abs() <= 1e-4 * whole,
                "{id}: {} chunks sum to {sum} m³ of {whole}",
                element.name
            );
            for chunk in run {
                for &p in &chunk.hull {
                    let p = glam::Vec3::from_array(p).as_dvec3();
                    // The writer rounds every coordinate to a micrometre, then to f32.
                    assert!(
                        piece
                            .solid
                            .faces
                            .iter()
                            .all(|face| face.normal.dot(p) <= face.offset + 1e-5),
                        "{id}: a chunk of {} leaves it at {p}",
                        element.name
                    );
                }
            }
        }
    }
}

/// EVERY INTERFACE IS ONE CONTACT, AT ITS FULL AREA: in every shipped bake each chunk's cut faces,
/// those on none of its piece's face planes, are exactly its joints to that piece's other chunks,
/// and its joints to other pieces' chunks never exceed its faces on its piece's surface. The
/// support law and the seams' and attachments' holds read each joint's area once, so a contact
/// counted twice holds a board's nailing, or a wall's seam, double. A pair listed twice is refused
/// by the schema's strict joint order. Mutant: a contact search that counts an overlap from both
/// faces fails here.
#[test]
fn every_interface_is_one_contact() {
    for (spec, baked) in shipped() {
        let asset = &baked.asset;
        let id = &spec.construction.id;
        let mut inner = vec![0.0; asset.chunks.len()];
        let mut outer = vec![0.0; asset.chunks.len()];
        for joint in &asset.joints {
            let (a, b) = (joint.a as usize, joint.b as usize);
            let tally = if asset.chunks[a].element == asset.chunks[b].element {
                &mut inner
            } else {
                &mut outer
            };
            tally[a] += f64::from(joint.area_m2);
            tally[b] += f64::from(joint.area_m2);
        }
        let solids: Vec<Polyhedron> = spec
            .construction
            .pieces
            .iter()
            .flat_map(|piece| fracture::chunks(piece, id, &mut CutCount::default()))
            .collect();
        assert_eq!(solids.len(), asset.chunks.len(), "{id}: the bake's chunks");
        for (c, solid) in solids.iter().enumerate() {
            let piece = &spec.construction.pieces[usize::from(asset.chunks[c].element)];
            let on_surface = |face: &Face| {
                piece.solid.faces.iter().any(|side| {
                    side.normal.dot(face.normal) > 1.0 - 1e-9
                        && (side.offset - face.offset).abs() < PLANE_M
                })
            };
            let faces = solid.faces();
            let (surface, cut) = faces.iter().fold((0.0, 0.0), |(surface, cut), face| {
                if on_surface(face) {
                    (surface + face.area(), cut)
                } else {
                    (surface, cut + face.area())
                }
            });
            // The search drops an overlap under its least joint area, at most one per face; the
            // writer rounds each joint's area to a millionth.
            let slack = faces.len() as f64 * (contact::MIN_AREA_M2 + 1e-6);
            assert!(
                (inner[c] - cut).abs() <= slack,
                "{id}: chunk {c}: joints inside its piece {} against its cut faces {cut}",
                inner[c]
            );
            assert!(
                outer[c] <= surface + slack,
                "{id}: chunk {c}: joints to other pieces {} against its surface {surface}",
                outer[c]
            );
        }
    }
}

/// Every shipped house generator's spec, then the fixture house.
fn houses() -> Vec<House> {
    let dir = crate::repo_root().join(SPECS_DIR);
    let mut houses: Vec<House> = std::fs::read_dir(&dir)
        .expect("the specs' folder is shipped")
        .map(|entry| entry.expect("a readable entry").path())
        .filter(|path| path.to_string_lossy().ends_with(HOUSE_SUFFIX))
        .map(|path| House::from_ron(&std::fs::read_to_string(&path).unwrap()).unwrap())
        .collect();
    houses.push(fixture_house());
    houses
}

/// THE ROOF IS LAID IN ITS AUTHORED BOARDS: each roof plane is `boards_per_slope` pieces, every two
/// neighbours sharing a whole side, `board_thickness_m` deep along the board's length, and each
/// board with two neighbours `board_width_m` across between them; the cutter cuts each piece alone
/// (`the_chunks_tile_each_element`). Board layout is construction, so no break crosses from one
/// board into another; the roof's response and its view rely on that. Widths and lengths are read
/// from the laid pieces. Mutant: a plane laid as one slab, or two boards as one piece, then cut by
/// cells, fails here.
#[test]
fn each_roof_board_is_its_own_piece() {
    for house in houses() {
        let built = house::construction(&house).unwrap();
        let mut planes: Vec<&str> = built
            .pieces
            .iter()
            .filter(|piece| piece.part == "roof")
            .map(|piece| piece.component.as_str())
            .collect();
        planes.sort_unstable();
        planes.dedup();
        assert!(!planes.is_empty(), "{}: a roof", house.id);
        for plane in planes {
            let boards: Vec<&Piece> = built
                .pieces
                .iter()
                .filter(|piece| piece.part == "roof" && piece.component == plane)
                .collect();
            let what = format!("{}: {plane}", house.id);
            assert_eq!(boards.len(), house.boards_per_slope as usize, "{what}");
            let solids: Vec<Polyhedron> = boards.iter().map(|board| board.solid.clone()).collect();
            let sides = contact::joints(&solids);
            assert_eq!(sides.len(), boards.len() - 1, "{what}: a row of boards");
            for side in &sides {
                let board = boards[side.a];
                let (near, far) = board.solid.extent(fracture::grain(board.surface_frame));
                let whole = house.board_thickness_m * (far - near);
                assert!((side.area - whole).abs() < 1e-9 * whole, "{what}: {side:?}");
            }
            for (k, board) in boards.iter().enumerate() {
                let mine: Vec<&contact::Joint> = sides
                    .iter()
                    .filter(|side| side.a == k || side.b == k)
                    .collect();
                if let [one, other] = mine[..] {
                    let across = (one.centroid - other.centroid).dot(one.normal).abs();
                    assert!(
                        (across - house.board_width_m).abs() < 1e-9,
                        "{what}: {} is {across} m across",
                        board.name
                    );
                }
            }
        }
    }
}

/// FIXTURE DATA: a constructed house with solid walls and a pitched board roof. No shipped house
/// is required to keep its openings, joins or attachment choices for mutation tests. Every cut
/// choice is a fixture's.
const FIXTURE_HOUSE: &str = r#"House(
    id: "fixture", width_m: 4.0, length_m: 6.0, wall_thickness_m: 0.25,
    course_m: 0.1, module_m: 0.3, foundation_courses: 1, foundation_attachment: "mortared",
    eaves_course: 20, pitch: (3, 4), plate_height_m: 0.1, board_width_m: 0.2,
    board_thickness_m: 0.05, boards_per_slope: 14, gable_overhang_m: 0.3,
    parts: (
        walls: (material: "Brick", surface_frame: Upright, cut: Courses((
            row_courses: (3, Fixture("fixture")), crack_chance: (0.2, Fixture("fixture")),
            crack_shift_m: (0.09, Fixture("fixture")), min_width_m: (0.06, Fixture("fixture")),
        ))),
        plates: (material: "Wood", surface_frame: Grain((0.0, 0.0, 1.0)), cut: Cells((
            size_m: (0.25, Fixture("fixture")), stretch: (4.0, Fixture("fixture")),
        ))),
        roof: (material: "Wood", surface_frame: Grain((0.0, 0.0, 1.0)), cut: Cells((
            size_m: (0.15, Fixture("fixture")), stretch: (3.0, Fixture("fixture")),
        ))),
    ),
    walls: (east: [], west: [], north: [], south: []),
    joins: [
        (parts: ("pier", "pier"), by: "mortared"),
        (parts: ("pier", "wall_plate"), by: "mortared"),
        (parts: ("gable", "wall_plate"), by: "mortared"),
        (parts: ("wall_plate", "roof"), by: "nailed"),
        (parts: ("roof", "roof"), by: "nailed"),
        (parts: ("gable", "roof"), by: "resting"),
    ],
)"#;

fn fixture_house() -> House {
    House::from_ron(FIXTURE_HOUSE).expect("the constructed fixture states a house")
}

/// FIXTURE DATA: a running bond with straight head joints.
const FIXTURE_COURSES: &str = "Courses((origin: (0.0, 0.0, 0.0), along: (1.0, 0.0, 0.0), up: (0.0, 1.0, 0.0), \
         course_m: 0.075, module_m: 0.26, rule: (row_courses: (3, Fixture(\"fixture\")), \
         crack_chance: (0.0, Fixture(\"fixture\")), crack_shift_m: (0.05, Fixture(\"fixture\")), \
         min_width_m: (0.08, Fixture(\"fixture\")))))";

/// FIXTURE DATA: a stretched-cell cut, `size_m` across the grain.
fn cells(size_m: f64, stretch: f64) -> String {
    format!(
        "Cells((size_m: ({size_m:?}, Fixture(\"fixture\")), stretch: ({stretch:?}, Fixture(\"fixture\"))))"
    )
}

/// FIXTURE DATA: an authored box piece `lo..hi`, its own component, as construction text.
fn box_piece(
    name: &str,
    part: &str,
    material: &str,
    frame: &str,
    lo: DVec3,
    hi: DVec3,
    pattern: &str,
) -> String {
    let solid = super::geom::tests::cuboid(lo, hi);
    let corners: Vec<String> = solid
        .corners
        .iter()
        .map(|p| format!("({:?}, {:?}, {:?})", p.x, p.y, p.z))
        .collect();
    let faces: Vec<String> = solid
        .faces
        .iter()
        .map(|f| format!("{:?}", f.corners))
        .collect();
    format!(
        "(name: \"{name}\", part: \"{part}\", component: \"{name}\", material: \"{material}\", \
         surface_frame: {frame}, corners: [{}], faces: [{}], pattern: {pattern})",
        corners.join(", "),
        faces.join(", ")
    )
}

/// FIXTURE DATA: a construction of `pieces` on the plane `y = 0`, mortared to it.
fn construction(pieces: &[String], joins: &str) -> String {
    format!(
        "Construction(id: \"fixture\", foundation_y_m: 0.0, foundation_attachment: \"mortared\", \
         pieces: [{}], joins: [{joins}])",
        pieces.join(", ")
    )
}

/// The one piece of a construction of `piece` alone, resolved.
fn lone(piece: String) -> Piece {
    let mut built = ConstructionSpec::from_ron(&construction(&[piece], "")).unwrap();
    built.pieces.remove(0)
}

/// FIXTURE DATA: a masonry unit the shipped registry does not hold, and its density, kg/m³.
const CLINKER: (&str, f64) = ("FixtureClinker", 1900.0);

/// The shipped registry's densities, and [`CLINKER`]'s.
fn with_clinker(name: &str) -> Result<f64, String> {
    if name == CLINKER.0 {
        Ok(CLINKER.1)
    } else {
        schema::substance_density(name)
    }
}

/// A NEW MATERIAL IS DATA: a construction naming a material key no Rust names bakes, its chunks
/// weighed by that key's density and written under that key, and each piece is cut by the
/// pattern its data names whatever it is made of: swap the two pieces' patterns and the chunk
/// counts swap with them. Content authors rely on adding an object or a material without a
/// generator or a match arm. Mutants: a bake that names materials from a closed list, weighs by
/// a hard-coded material, or picks a piece's pattern from its material fails.
#[test]
fn an_additional_material_bakes_from_data_alone() {
    let courses = FIXTURE_COURSES;
    let grain = "Grain((1.0, 0.0, 0.0))";
    let text = |block: &str, beam: &str| {
        construction(
            &[
                box_piece(
                    "block",
                    "block_part",
                    CLINKER.0,
                    "Upright",
                    DVec3::ZERO,
                    DVec3::new(1.2, 0.9, 0.3),
                    block,
                ),
                box_piece(
                    "beam",
                    "beam_part",
                    "Wood",
                    grain,
                    DVec3::new(0.0, 0.9, 0.0),
                    DVec3::new(1.2, 1.05, 0.3),
                    beam,
                ),
            ],
            "(parts: (\"block_part\", \"beam_part\"), by: \"nailed\")",
        )
    };
    let counts = |block: &str, beam: &str| {
        let built = ConstructionSpec::from_ron(&text(block, beam)).unwrap();
        let asset = bake(&built, with_clinker)
            .expect("a new material bakes")
            .asset;
        assert_eq!(asset.materials, [CLINKER.0, "Wood"]);
        for chunk in &asset.chunks {
            let material = &asset.materials[usize::from(chunk.material)];
            let density = with_clinker(material).unwrap();
            // Volume and mass are each written to a millionth of their unit.
            let expected = f64::from(chunk.volume_m3) * density;
            assert!(
                (f64::from(chunk.mass_kg) - expected).abs() <= 1e-6 * (density + 1.0),
                "{chunk:?} of {material}"
            );
        }
        asset
            .elements
            .iter()
            .map(|element| element.chunks[1] - element.chunks[0])
            .collect::<Vec<_>>()
    };
    let striped = cells(0.1, 3.0);
    let [block, beam] = counts(courses, "Whole")[..] else {
        unreachable!("two pieces")
    };
    assert!(block > 1, "the courses cut the block");
    assert_eq!(beam, 1, "the beam is whole");
    let [block, beam] = counts("Whole", &striped)[..] else {
        unreachable!("two pieces")
    };
    assert_eq!(block, 1, "the block is whole");
    assert!(beam > 1, "the cells cut the beam");
    let err = bake(
        &ConstructionSpec::from_ron(&text(courses, "Whole")).unwrap(),
        schema::substance_density,
    )
    .map(|_| ())
    .unwrap_err();
    assert!(
        err.contains(CLINKER.0),
        "a key no registry holds is refused: {err}"
    );
}

/// The extent of `cells` along `axis`, summed.
fn reach(cells: &[Polyhedron], axis: DVec3) -> f64 {
    cells
        .iter()
        .map(|cell| {
            let (lo, hi) = cell.extent(axis);
            hi - lo
        })
        .sum()
}

/// Cells conserve the piece's volume and every interface: each certified cell's surface equals
/// its boundary faces plus the contacts found once at full area. Bakers rely on that partition
/// for mass and the joint graph. Mutants that lose volume or fail to pair a cut face fail here.
/// Fixture geometry; the reach ratio is an observation, not a bound on clipped cells.
#[test]
fn cells_partition_their_piece() {
    let (lo, hi) = (DVec3::ZERO, DVec3::splat(1.8));
    let whole = super::geom::tests::cuboid(lo, hi);
    for (grain, frame, across) in [
        (DVec3::Z, "Grain((0.0, 0.0, 1.0))", DVec3::X),
        (DVec3::X, "Grain((1.0, 0.0, 0.0))", DVec3::Z),
        (DVec3::Y, "Upright", DVec3::X),
    ] {
        let piece = lone(box_piece(
            "cube",
            "Pier",
            "Wood",
            frame,
            lo,
            hi,
            &cells(0.2, 3.0),
        ));
        let cut = fracture::chunks(&piece, "fixture", &mut CutCount::default());
        assert!(cut.len() > 1, "{frame}: the fixture is split into cells");
        let volume: f64 = cut.iter().map(Polyhedron::volume).sum();
        assert!(
            (volume - whole.volume()).abs() < 1e-9 * whole.volume(),
            "{frame}: {volume}"
        );
        let found = contact::joints(&cut);
        let on_cube = |face: &Face| {
            whole.faces().iter().any(|side| {
                side.normal.dot(face.normal) > 1.0 - 1e-12
                    && (side.offset - face.offset).abs() < 1e-9
            })
        };
        for (k, cell) in cut.iter().enumerate() {
            cell.certify(PLANE_M).unwrap();
            let (clo, chi) = cell.bounds();
            assert!(clo.cmpge(lo - 1e-9).all() && chi.cmple(hi + 1e-9).all());
            let faces = cell.faces();
            let surface: f64 = faces.iter().map(Face::area).sum();
            let boundary: f64 = faces.iter().filter(|f| on_cube(f)).map(Face::area).sum();
            let joined: f64 = found
                .iter()
                .filter(|joint| joint.a == k || joint.b == k)
                .map(|joint| joint.area)
                .sum();
            // The search drops an overlap under its least joint area, at most one per face.
            assert!(
                (boundary + joined - surface).abs() < faces.len() as f64 * contact::MIN_AREA_M2,
                "{frame}: cell {k}: {boundary} + {joined} against {surface}"
            );
        }
        let ratio = reach(&cut, grain) / reach(&cut, across);
        eprintln!("MEASURED fixture {frame}: along/across reach ratio {ratio:.6}");
    }
}

/// THE GRAIN AXIS IS THE SURFACE FRAME'S: an authored grain is its own unit direction, and an
/// upright frame's axis is the construction's up. The cutter and the view read one grain fact.
#[test]
fn the_grain_axis_is_the_surface_frames() {
    use overmatch::construction::schema::SurfaceFrame;
    assert_eq!(fracture::grain(SurfaceFrame::Upright), DVec3::Y);
    let tilted = fracture::grain(SurfaceFrame::Grain([0.6, 0.0, 0.8]));
    assert!(
        (tilted - DVec3::new(0.6, 0.0, 0.8)).length() < 1e-6,
        "{tilted}"
    );
}

/// FASTENING FOLLOWS THE JOINS, NEVER THE PATTERN: whichever pattern cuts the walls and the roof,
/// every joint between components is held by the kind its join names, and inside a component by
/// the seam. The fixture's joins name kinds a pattern would not suggest (a nailed wall, a
/// mortared roof). Mutant: a bake that reads mortar from courses, nails from boards or cells,
/// fails.
#[test]
fn fastening_follows_the_joins_never_the_pattern() {
    use super::spec::PartCut;
    let base = fixture_house();
    let rotate = |by: &str| match by {
        "mortared" => "nailed",
        "nailed" => "resting",
        _ => "mortared",
    };
    let cells_cut = || match ron::from_str::<Pattern>(&cells(0.25, 2.0)).unwrap() {
        Pattern::Cells(cells) => PartCut::Cells(cells),
        other => unreachable!("{other:?}"),
    };
    for (walls, roof) in [
        (base.parts.walls.cut.clone(), base.parts.roof.cut.clone()),
        (cells_cut(), PartCut::Whole),
        (PartCut::Whole, cells_cut()),
    ] {
        let mut house = base.clone();
        house.parts.walls.cut = walls;
        house.parts.roof.cut = roof;
        for join in &mut house.joins {
            join.by = rotate(&join.by).to_owned();
        }
        let built = house::construction(&house).unwrap();
        let asset = bake(&built, schema::substance_density).unwrap().asset;
        assert_bond_kinds(&built, &asset);
    }
}

/// A SHIPPED SPEC CARRIES NO FIXTURE LABEL, IN THE TEST BUILD TOO: the fixture house loads inline,
/// and the same text read as a shipped spec is refused naming the field. The bake binary has no
/// fixture label at all, so a shipped spec the tests accepted with one could not be baked.
/// Mutant: deleting the production reader's shipped guard admits these files.
#[test]
fn a_shipped_spec_refuses_a_fixture_label() {
    fixture_house();
    let root = std::env::temp_dir().join(format!("overmatch-tools-shipped-{}", std::process::id()));
    let dir = root.join(SPECS_DIR);
    for (suffix, text, field) in [
        (HOUSE_SUFFIX, FIXTURE_HOUSE.to_owned(), "parts.walls.cut"),
        (
            PIECES_SUFFIX,
            construction(
                &[box_piece(
                    "cube",
                    "Pier",
                    "Wood",
                    "Upright",
                    DVec3::ZERO,
                    DVec3::ONE,
                    &cells(0.2, 2.0),
                )],
                "",
            ),
            "size_m",
        ),
    ] {
        std::fs::create_dir_all(&dir).unwrap();
        let name = format!("fixture{suffix}");
        std::fs::write(dir.join(&name), text).unwrap();
        let result = specs(&root).map(|_| ());
        std::fs::remove_dir_all(&root).unwrap();
        let err = result.unwrap_err();
        assert!(
            err.contains(&name) && err.contains(field) && err.contains("Fixture"),
            "{err}"
        );
    }
    fixture_house();
}

/// A CUT STATES ITS JOB, NEVER A STRENGTH: a pattern's size labelled as a measured fact does not
/// parse, and a Taste with no reason is refused naming its field. Content authors rely on cut
/// sizes being looks or resolutions, so balance never hides in one.
#[test]
fn a_cut_states_its_job_never_a_strength() {
    let lo_hi = (DVec3::ZERO, DVec3::ONE);
    let claim = "Cells((size_m: (0.2, Measured(\"a strength\")), stretch: (2.0, Taste(\"long\"))))";
    let piece = box_piece("cube", "Pier", "Wood", "Upright", lo_hi.0, lo_hi.1, claim);
    let err = ConstructionSpec::from_ron(&construction(&[piece], "")).unwrap_err();
    assert!(err.contains("Measured"), "{err}");
    let blank = "Cells((size_m: (0.2, Taste(\" \")), stretch: (2.0, Taste(\"long\"))))";
    let piece = box_piece("cube", "Pier", "Wood", "Upright", lo_hi.0, lo_hi.1, blank);
    let err = ConstructionSpec::from_ron(&construction(&[piece], "")).unwrap_err();
    assert!(err.contains("size_m"), "{err}");
}

/// AN AUTHORED PIECE IS A CLOSED CONVEX SOLID OR REFUSED: a face wound inward and a face naming
/// a corner past the list are refused before any cut, naming the piece. The cutter relies on
/// every piece it reads being one.
#[test]
fn an_authored_piece_that_is_no_convex_solid_is_refused() {
    let good = box_piece(
        "cube",
        "Pier",
        "Wood",
        "Upright",
        DVec3::ZERO,
        DVec3::ONE,
        "Whole",
    );
    ConstructionSpec::from_ron(&construction(std::slice::from_ref(&good), ""))
        .expect("a cube resolves");
    let inward = good.replacen("[4, 5, 6, 7]", "[7, 6, 5, 4]", 1);
    let past = good.replacen("[4, 5, 6, 7]", "[4, 5, 6, 8]", 1);
    assert_ne!(inward, good, "the fixture's far cap is [4, 5, 6, 7]");
    for (case, text, breach) in [
        ("inward", inward, "no closed convex solid"),
        ("past", past, "past corners"),
    ] {
        let err = ConstructionSpec::from_ron(&construction(&[text], "")).unwrap_err();
        assert!(
            err.contains("cube") && err.contains(breach),
            "{case}: {err}"
        );
    }
}

/// Construction authors rely on crack_chance selecting cracks: zero never cracks, and a positive
/// chance produces cracks where the inline fixture leaves room for them.
#[test]
fn authored_crack_probability_controls_cuts() {
    use super::spec::PartCut;
    let mut house = fixture_house();
    let mut chance = |value: f64| {
        let PartCut::Courses(rule) = &mut house.parts.walls.cut else {
            unreachable!("the fixture's walls are coursed")
        };
        rule.crack_chance.0 = value;
        bake(
            &house::construction(&house).unwrap(),
            schema::substance_density,
        )
        .unwrap()
        .cuts
    };
    let uncracked = chance(0.0);
    assert_eq!(uncracked.cracks, 0);
    assert!(uncracked.joints > 0);
    assert!(chance(1.0).cracks > 0);
}

/// THE STREAM IS NAMED, NOT SEQUENCED: the same names draw the same numbers, other names
/// others, so one element's cuts never shift when another element is added or re-cut.
#[test]
fn the_cut_stream_is_a_function_of_its_names() {
    let draw = |parts: &[&str]| fracture::Stream::new(parts).unit();
    assert_eq!(
        draw(&["house_1", "east/pier_0"]),
        draw(&["house_1", "east/pier_0"])
    );
    assert_ne!(
        draw(&["house_1", "east/pier_0"]),
        draw(&["house_1", "east/pier_1"])
    );
    assert_ne!(
        draw(&["ab", "c"]),
        draw(&["a", "bc"]),
        "the separator counts"
    );
    let mut stream = fracture::Stream::new(&["x"]);
    assert!(
        (0..1000)
            .map(|_| stream.unit())
            .all(|u| (0.0..1.0).contains(&u))
    );
}

/// Construction authors select foundation attachments independently of component joins.
#[test]
fn anchors_name_the_authored_foundation_attachment() {
    let mut house = fixture_house();
    for name in ["mortared", "resting"] {
        house.foundation_attachment = name.into();
        let asset = bake(
            &house::construction(&house).unwrap(),
            schema::substance_density,
        )
        .unwrap()
        .asset;
        assert!(!asset.anchors.is_empty());
        for anchor in &asset.anchors {
            assert_eq!(asset.attachments[usize::from(anchor.attachment)], name);
        }
    }
    house.foundation_attachment = "unknown-fixture-kind".into();
    assert!(house.check().unwrap_err().contains("foundation_attachment"));
}

/// Every joint holds by the construction's declared join between components, or by its own seam
/// inside one component. The runtime's fracture law relies on this distinction.
fn assert_bond_kinds(construction: &Construction, asset: &ConstructionAsset) {
    let id = &construction.id;
    let element = |c: u32| &asset.elements[usize::from(asset.chunks[c as usize].element)];
    for joint in &asset.joints {
        let (a, b) = (element(joint.a), element(joint.b));
        if a.component == b.component {
            assert_eq!(joint.bond, Bond::Seam, "{id}: {joint:?}");
        } else {
            let declared = construction
                .joins
                .iter()
                .find(|join| join.joins((&a.part, &b.part)))
                .expect("every baked component pair has its declared join");
            let Bond::Attached(k) = joint.bond else {
                panic!("{id}: {joint:?} is no attachment")
            };
            assert_eq!(
                asset.attachments[usize::from(k)],
                declared.by,
                "{id}: {joint:?}"
            );
        }
    }
}

/// The bake takes every component join from data. Iterate every shipped spec and each of its
/// joins; a new construction adds no test branch. Switching a join to a different declared kind
/// changes those bonds alone, preserving the joint graph.
#[test]
fn every_bond_between_components_is_the_kind_its_spec_names() {
    for (spec, baked) in shipped() {
        let asset = &baked.asset;
        let built = &spec.construction;
        assert_bond_kinds(built, asset);
        for (k, join) in built.joins.iter().enumerate() {
            let mut missing = built.clone();
            missing.joins.remove(k);
            let err = bake(&missing, schema::substance_density)
                .map(|_| ())
                .unwrap_err();
            assert!(err.contains("no join holds"), "{}: {err}", built.id);
            // A spec using only one attachment kind offers no alternate kind to contrast; its
            // direct mapping and missing-join refusal above still exercise every join.
            let Some(other) = built.joins.iter().find(|other| other.by != join.by) else {
                continue;
            };
            let mut changed = built.clone();
            changed.joins[k].by.clone_from(&other.by);
            let rebaked = bake(&changed, schema::substance_density)
                .expect("only the declared holding kind changed");
            assert_bond_kinds(&changed, &rebaked.asset);
            assert_eq!(asset.joints.len(), rebaked.asset.joints.len());
            let kind = |asset: &ConstructionAsset, bond| match bond {
                Bond::Seam => None,
                Bond::Attached(k) => Some(asset.attachments[usize::from(k)].clone()),
            };
            let mut differences = 0;
            for (was, now) in asset.joints.iter().zip(&rebaked.asset.joints) {
                assert_eq!((was.a, was.b), (now.a, now.b));
                differences += usize::from(kind(asset, was.bond) != kind(&rebaked.asset, now.bond));
            }
            assert!(
                differences > 0,
                "{}: {join:?} holds an actual joint",
                built.id
            );
        }
    }
}

/// FASTENING IS CONSTRUCTION DATA, NEVER AN ELEMENT'S NAME: with every piece renamed (each takes
/// the next piece's name, so a wall answers to a board's and a board to a wall's), every joint still
/// holds by the join its elements' part names declare, or by the seam inside one component. A roof's
/// boards are nailed only because their joins say so. Mutant: a bake that reads nailing, or any
/// holding kind, from a name fails here.
#[test]
fn fastening_never_reads_an_element_name() {
    let mut built: Vec<Construction> = shipped()
        .into_iter()
        .map(|(spec, _)| spec.construction)
        .collect();
    built.push(house::construction(&fixture_house()).unwrap());
    for construction in built {
        let names: Vec<String> = construction
            .pieces
            .iter()
            .map(|piece| piece.name.clone())
            .collect();
        let mut renamed = construction.clone();
        for (k, piece) in renamed.pieces.iter_mut().enumerate() {
            piece.name.clone_from(&names[(k + 1) % names.len()]);
        }
        let asset = bake(&renamed, schema::substance_density).unwrap().asset;
        assert_bond_kinds(&renamed, &asset);
    }
}

/// Idle joins and unknown attachment kinds are refused before play. FIXTURE DATA supplies the
/// intentionally absent sill-to-roof pair; shipped houses need not have that topology.
#[test]
fn a_join_that_holds_nothing_or_names_no_kind_is_refused() {
    use overmatch::construction::attachments::{ATTACHMENTS_PATH, Attachments};
    let house = fixture_house();
    bake(
        &house::construction(&house).unwrap(),
        schema::substance_density,
    )
    .expect("the unmutated construction bakes");
    let mut blank = house.clone();
    blank.joins[0].parts.0.clear();
    assert!(
        blank
            .check()
            .unwrap_err()
            .contains("parts: author two non-blank part names")
    );
    let mut idle = house.clone();
    idle.joins.push(spec::Join {
        parts: ("sill".into(), "roof".into()),
        by: house.joins[0].by.clone(),
    });
    let err = bake(
        &house::construction(&idle).unwrap(),
        schema::substance_density,
    )
    .map(|_| ())
    .unwrap_err();
    assert!(err.contains("holds no joint"), "{err}");
    let mut unknown = house;
    let mut name = "unregistered-fixture-kind".to_owned();
    while Attachments::shipped_static().holds(&name).is_ok() {
        name.push('_');
    }
    unknown.joins[0].by.clone_from(&name);
    let err = unknown.check().unwrap_err();
    for part in [name.as_str(), ATTACHMENTS_PATH, "name one of those"] {
        assert!(err.contains(part), "{part}: {err}");
    }
}

/// A HOUSE THE GENERATOR CANNOT LAY OUT IS REFUSED BEFORE ANY GEOMETRY: the generator lays a pier
/// before, between and after a wall's openings in `u` order, so an opening past the wall's face or
/// one over the next leaves a negative pier, and a cell with no finite size is a cut that never
/// ends. Mutant: a check that drops the span, the order or a part's cut passes one of these to the
/// generator.
#[test]
fn a_house_the_generator_cannot_lay_out_is_refused() {
    use super::spec::PartCut;
    let mut house = fixture_house();
    house.walls.east = vec![spec::Opening {
        u: (-1.0, 0.0),
        courses: (0, 10),
    }];
    house
        .check()
        .expect("the constructed opening is inside the wall");
    let (_, end) = house.wall_span("east");
    let mut past = house.clone();
    past.walls.east[0].u.1 = end + 1.0;
    let mut over = house.clone();
    over.walls.east.push(spec::Opening {
        u: (-0.1, 0.1),
        courses: (0, 10),
    });
    let mut endless = house;
    let PartCut::Cells(cells) = &mut endless.parts.plates.cut else {
        unreachable!("the fixture's plates are cut in cells")
    };
    cells.size_m.0 = f64::INFINITY;
    for (case, broken, field) in [
        ("past the wall", past, "pier"),
        ("over the next", over, "pier"),
        ("an endless piece", endless, "parts.plates.cut"),
    ] {
        let err = broken.check().unwrap_err();
        assert!(err.contains(field), "{case}: {err}");
    }
}

/// The bake's mass and colliders require disjoint piece interiors. Fixture boxes may share a
/// face, edge or corner, but coincident, contained and partially intersecting boxes must be
/// refused at resolution, naming both pieces. A per-piece certificate alone admits them.
#[test]
fn authored_pieces_share_boundaries_but_never_matter() {
    let first = box_piece(
        "first",
        "Pier",
        "Wood",
        "Upright",
        DVec3::ZERO,
        DVec3::ONE,
        "Whole",
    );
    for (lo, hi, overlaps) in [
        (DVec3::ZERO, DVec3::ONE, true),
        (DVec3::splat(0.2), DVec3::splat(0.8), true),
        (DVec3::splat(0.5), DVec3::splat(1.5), true),
        (DVec3::X, DVec3::new(2.0, 1.0, 1.0), false),
        (DVec3::new(1.0, 1.0, 0.0), DVec3::new(2.0, 2.0, 1.0), false),
        (DVec3::ONE, DVec3::splat(2.0), false),
        (DVec3::splat(2.0), DVec3::splat(3.0), false),
    ] {
        let second = box_piece("second", "Pier", "Wood", "Upright", lo, hi, "Whole");
        let result = ConstructionSpec::from_ron(&construction(&[first.clone(), second], ""));
        if overlaps {
            let err = result.unwrap_err();
            assert!(
                err.contains("first") && err.contains("second") && err.contains("overlap"),
                "{err}"
            );
        } else {
            result.expect("boundary contact and separation contain no duplicated matter");
        }
    }
}

/// Bounding boxes overlap for these fixture wedges, but their slabs x+z <= 0.8 and x+z >= 1.2
/// are disjoint. Authors need the gate to compare solids, not refuse their enclosing boxes.
#[test]
fn overlapping_bounds_do_not_imply_overlapping_matter() {
    let cube = super::geom::tests::cuboid(DVec3::ZERO, DVec3::ONE);
    let axis = DVec3::new(1.0, 0.0, 1.0);
    let solids = [
        cube.clip(axis, 0.8).unwrap(),
        cube.clip(-axis, -1.2).unwrap(),
    ];
    let texts: Vec<_> = ["first", "second"]
        .map(|name| {
            box_piece(
                name,
                "Pier",
                "Wood",
                "Upright",
                DVec3::ZERO,
                DVec3::ONE,
                "Whole",
            )
        })
        .into();
    let mut spec: ConstructionSpec = ron::from_str(&construction(&texts, "")).unwrap();
    for (piece, solid) in spec.pieces.iter_mut().zip(solids) {
        piece.corners = solid.corners;
        piece.faces = solid.faces.into_iter().map(|face| face.corners).collect();
    }
    spec.resolve()
        .expect("the slabs share no matter despite overlapping bounds");
}

/// Courses cut along the authored bond and leave depth intact. Fixture rows of three and ten
/// courses are four modules wide and one metre deep: the three interior head joints must make
/// four full-depth chunks. A module-only size gate rejects valid rows; skipping cuts fails here.
#[test]
fn course_chunk_bounds_follow_the_authored_geometry_and_row() {
    let mut piece = lone(box_piece(
        "row",
        "Pier",
        "Wood",
        "Upright",
        DVec3::ZERO,
        DVec3::ONE,
        FIXTURE_COURSES,
    ));
    for rows in [3, 10] {
        let Pattern::Courses(bond) = &mut piece.pattern else {
            unreachable!("fixture courses")
        };
        bond.rule.row_courses.0 = rows;
        let height = f64::from(rows) * bond.course_m;
        let module = bond.module_m;
        let depth = 1.0;
        piece.solid =
            super::geom::tests::cuboid(DVec3::ZERO, DVec3::new(4.0 * module, height, depth));
        let mut count = CutCount::default();
        let cut = fracture::chunks(&piece, "fixture", &mut count);
        assert_eq!(count.joints, 3);
        assert_eq!(cut.len(), 4);
        for chunk in cut {
            chunk.certify(PLANE_M).unwrap();
            let (lo, hi) = chunk.bounds();
            let expected = DVec3::new(module, height, depth);
            assert!((hi - lo - expected).length() < 1e-9, "{rows}: {lo}..{hi}");
            assert!((chunk.volume() - module * height * depth).abs() < 1e-12);
        }
    }
}

/// Authors rely on min_width_m at both row ends, not merely the middle. Fixture: a one-row
/// trapezoid tapers from 1 m to 0.8 m. With 0.26 m modules and a 0.08 m minimum, only the joints
/// at 0.26 and 0.52 fit; 0.78 would leave 0.02 m at the top. A midpoint-only check accepts it.
#[test]
fn course_cuts_leave_the_authored_minimum_at_both_row_ends() {
    let mut piece = lone(box_piece(
        "taper",
        "Pier",
        "Wood",
        "Upright",
        DVec3::ZERO,
        DVec3::ONE,
        FIXTURE_COURSES,
    ));
    let Pattern::Courses(bond) = &mut piece.pattern else {
        unreachable!("fixture courses")
    };
    bond.course_m = 1.0;
    bond.rule.row_courses.0 = 1;
    let minimum = bond.rule.min_width_m.0;
    piece.solid = geom::Prism {
        frame: geom::Frame {
            origin: DVec3::ZERO,
            u: DVec3::X,
            v: DVec3::Y,
        },
        profile: [(0.0, 0.0), (1.0, 0.0), (0.8, 1.0), (0.0, 1.0)]
            .map(|(x, y)| glam::DVec2::new(x, y))
            .to_vec(),
        near: 0.0,
        far: 0.2,
    }
    .solid();
    for chance in [0.0, 1.0] {
        let Pattern::Courses(bond) = &mut piece.pattern else {
            unreachable!("fixture courses")
        };
        bond.rule.crack_chance.0 = chance;
        let mut count = CutCount::default();
        let cut = fracture::chunks(&piece, "fixture", &mut count);
        assert_eq!(
            count.joints + count.cracks,
            2,
            "only two head joints clear both endpoints"
        );
        assert_eq!(cut.len(), 3);
        for chunk in cut {
            for y in [0.0, 1.0] {
                let (lo, hi) = chunk.section_extent(DVec3::Y, y, DVec3::X).unwrap();
                assert!(
                    hi - lo >= minimum - 1e-9,
                    "{y}: width {} < {minimum}",
                    hi - lo
                );
            }
        }
    }
}

/// The stretched metric is Euclidean distance after undoing grain stretch. With the same seed,
/// stretching the source and its grid along grain must stretch each cell by that factor; rotating
/// both geometry and grain must rotate those cells. Authors and the view rely on these relations,
/// not a clipped cell's aspect ratio. Mutants ignoring metric stretch or using a fixed grain fail.
/// Fixture: a centred cube, then a threefold stretch and proper cardinal-axis rotations.
#[test]
fn cells_follow_stretch_and_grain_rotation() {
    let source = lone(box_piece(
        "affine",
        "Pier",
        "Wood",
        "Grain((0.0, 0.0, 1.0))",
        DVec3::splat(-0.6),
        DVec3::splat(0.6),
        &cells(0.3, 1.0),
    ));
    let reference = fracture::chunks(&source, "fixture", &mut CutCount::default());
    assert!(reference.len() > 1);
    let rotations: [(schema::SurfaceFrame, fn(DVec3) -> DVec3); 3] = [
        (schema::SurfaceFrame::Grain([0.0, 0.0, 1.0]), |p| p),
        (schema::SurfaceFrame::Grain([1.0, 0.0, 0.0]), |p| {
            DVec3::new(p.z, p.x, p.y)
        }),
        (schema::SurfaceFrame::Upright, |p| {
            DVec3::new(p.x, p.z, -p.y)
        }),
    ];
    for (frame, rotate) in rotations {
        let transform = |p: DVec3| rotate(DVec3::new(p.x, p.y, 3.0 * p.z));
        let mut piece = source.clone();
        piece.surface_frame = frame;
        piece.solid = Polyhedron::from_faces(
            source
                .solid
                .corners
                .iter()
                .copied()
                .map(transform)
                .collect(),
            source
                .solid
                .faces
                .iter()
                .map(|face| face.corners.clone())
                .collect(),
        );
        let Pattern::Cells(rule) = &mut piece.pattern else {
            unreachable!("fixture cells")
        };
        rule.stretch.0 = 3.0;
        let actual = fracture::chunks(&piece, "fixture", &mut CutCount::default());
        assert_eq!(
            actual.len(),
            reference.len(),
            "{frame:?}: affine transforms preserve cells"
        );
        for (before, after) in reference.iter().zip(actual) {
            after.certify(PLANE_M).unwrap();
            let expected: Vec<_> = before.corners.iter().copied().map(transform).collect();
            for (points, other) in [(&expected, &after.corners), (&after.corners, &expected)] {
                assert!(
                    points
                        .iter()
                        .all(|p| other.iter().any(|q| p.distance(*q) < 1e-9)),
                    "{frame:?}: transformed vertices disagree"
                );
            }
            assert!((after.volume() - 3.0 * before.volume()).abs() < 1e-9 * after.volume());
        }
    }
}
