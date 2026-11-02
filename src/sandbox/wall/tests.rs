//! The wall prototype's invariants, about the game's environment destruction (game state only),
//! on the game's laws over the baked wall. Fixture numbers are fixture data; the brick is the
//! shipped row's ([`super::brick`]).

use crate::buildings::fracture::GroupLoads;
use bevy::math::Vec3;

use super::model::*;
use super::{
    Applied, Charge, Deposit, View, Wall, WallControls, apply_deposit, blast_field, brick, layout,
};

/// The 88 mm HE fixture burst (fixture data): 1.47 MJ, its charge the TNT that carries it, so its
/// reach (23.2 m, DERIVED) holds every standoff here; a coupling is its hits' energy over its own.
const FIXTURE_BLAST: Blast = Blast {
    energy: 1.47e6,
    charge: 1.47e6 / 4.184e6,
    radius: 1e-3,
    products: FIT_ONLY_PRODUCTS,
};

/// SUPPORT IS CONNECTIVITY TO AN ANCHOR THROUGH INTACT JOINTS, on the baked wall. An intact wall
/// has no loose group (a joint the bake missed would drop chunks the moment the wall stands);
/// cracking every joint across a region's boundary frees exactly that region, as one group; and
/// cracking every anchor frees the whole wall as one. The sandbox's falls, and the game's later,
/// rely on it.
#[test]
fn a_group_with_no_intact_path_to_the_ground_falls_as_one() {
    let layout = layout();
    let structure = &layout.cut.structure;
    let state = WallState::new(structure);
    assert!(
        loose_groups(structure, &state).is_empty(),
        "an intact wall stands"
    );

    let top = layout.min.y + 0.8 * layout.size.y;
    let region: Vec<usize> = (0..structure.chunks.len())
        .filter(|&i| structure.chunks[i].centroid.y > top)
        .collect();
    assert!(!region.is_empty());
    let mut state = WallState::new(structure);
    for (j, joint) in structure.joints.iter().enumerate() {
        let inside = |side: Side| match side {
            Side::Chunk(i) => region.contains(&i),
            Side::Ground => false,
        };
        if inside(Side::Chunk(joint.a)) != inside(joint.b) {
            state.cracked[j] = true;
        }
    }
    assert_eq!(loose_groups(structure, &state), vec![region]);

    let mut state = WallState::new(structure);
    for (j, joint) in structure.joints.iter().enumerate() {
        state.cracked[j] = joint.b == Side::Ground;
    }
    let all: Vec<usize> = (0..structure.chunks.len()).collect();
    assert_eq!(loose_groups(structure, &state), vec![all]);
}

/// THE LAW READS THE TABLE, NOT THE PATTERN: renumbering the wall's chunks (and its joints with
/// them) changes no crack, no structural failure and no fall. A law that indexed a pattern would
/// crack other joints once the numbering moved.
#[test]
fn renumbering_the_chunks_changes_no_outcome() {
    let layout = layout();
    let cut = &layout.cut;
    let count = cut.structure.chunks.len();
    let map = |i: usize| count - 1 - i;
    let renumbered = Structure {
        chunks: (0..count).map(|k| cut.structure.chunks[map(k)]).collect(),
        joints: cut
            .structure
            .joints
            .iter()
            .rev()
            .map(|joint| Joint {
                a: map(joint.a),
                b: match joint.b {
                    Side::Chunk(b) => Side::Chunk(map(b)),
                    Side::Ground => Side::Ground,
                },
                ..*joint
            })
            .collect(),
    };
    let centre = cut
        .structure
        .chunks
        .iter()
        .map(|chunk| chunk.centroid)
        .min_by(|a, b| {
            a.distance(layout.centre())
                .total_cmp(&b.distance(layout.centre()))
        })
        .expect("the wall has chunks");
    let source = |structure: &Structure| -> [Load; 1] {
        [Load {
            chunk: (0..structure.chunks.len())
                .find(|&i| structure.chunks[i].centroid == centre)
                .expect("the centre is a chunk's centroid"),
            at: centre,
            energy: 1.0e6,
            crushed: 1.0e6 / brick().crush,
            blow: Vec3::ZERO,
            press: Vec3::ZERO,
            peak: None,
            origin: Origin::Field,
        }]
    };
    let pushes = |structure: &Structure| -> Vec<Load> {
        (0..structure.chunks.len())
            .filter(|&i| structure.chunks[i].centroid.distance(centre) < 0.5)
            .map(|i| Load {
                chunk: i,
                at: structure.chunks[i].centroid,
                energy: 0.0,
                crushed: 0.0,
                blow: Vec3::NEG_Z * 400.0,
                press: Vec3::ZERO,
                peak: None,
                origin: Origin::Field,
            })
            .collect()
    };
    let outcome = |structure: &Structure| {
        let mut state = WallState::new(structure);
        crack(structure, &mut state, &source(structure));
        structural(
            structure,
            &mut state,
            GroupLoads::new(&pushes(structure), Vec3::ZERO),
            &mut SectionCache::default(),
        );
        let mut cracked: Vec<[i64; 3]> = structure
            .joints
            .iter()
            .enumerate()
            .filter(|(j, _)| state.cracked[*j])
            .map(|(_, joint)| (joint.centroid * 1.0e4).round().as_i64vec3().to_array())
            .collect();
        cracked.sort_unstable();
        let mut fallen: Vec<[i64; 3]> = loose_groups(structure, &state)
            .into_iter()
            .flatten()
            .map(|i| {
                (structure.chunks[i].centroid * 1.0e4)
                    .round()
                    .as_i64vec3()
                    .to_array()
            })
            .collect();
        fallen.sort_unstable();
        (cracked, fallen)
    };
    let original = outcome(&cut.structure);
    assert!(!original.0.is_empty(), "the fixture cracks something");
    assert_eq!(outcome(&renumbered), original);
}

/// THE STRUCTURAL CHECK COMPARES THE ANGULAR IMPULSE ABOUT THE HINGE WITH `√(2 I (G_f A + E_g))`:
/// on a two-chunk column a push at the top whose moment about the bed joint's hinge (the rim of its
/// disc on the side the push turns the top toward) is 1 % over that capacity cracks the bed joint
/// (the joint alone holds the top chunk), and 1 % under it cracks nothing; `I` is the top's inertia
/// about the hinge and `E_g` the work of raising its centre of mass over it, for the bed bears the
/// top. A capacity read about the centroid, or without the weight the section carries (one far
/// push then opening a plane a whole building stands on), fails here. Fixture geometry; the
/// fixture's seams state no shear ([`Shear::OpeningOnly`]), so no punch ring (the top sliding off
/// its bed) competes with the hinge this test pins.
#[test]
fn a_push_fails_the_section_at_its_fracture_impulse() {
    let material = brick();
    let seam = Seam {
        shear: Shear::OpeningOnly,
        ..material.seam
    };
    let chunk = |y: f32| Chunk {
        volume: 0.2 * 0.2 * 0.25,
        centroid: Vec3::new(0.1, y, 0.125),
        mass: material.density * 0.2 * 0.2 * 0.25,
        material,
        buried: false,
    };
    let structure = Structure {
        chunks: vec![chunk(0.1), chunk(0.3)],
        joints: vec![
            Joint {
                a: 0,
                b: Side::Ground,
                area: 0.05,
                centroid: Vec3::new(0.1, 0.0, 0.125),
                normal: Vec3::Y,
                bond: Bond::Bonded(seam),
            },
            Joint {
                a: 0,
                b: Side::Chunk(1),
                area: 0.05,
                centroid: Vec3::new(0.1, 0.2, 0.125),
                normal: Vec3::Y,
                bond: Bond::Bonded(seam),
            },
        ],
    };
    let bed = 1;
    let joint = structure.joints[bed];
    let top = structure.chunks[1];
    // The push along -z turns the top about -x, so the hinge is the bed disc's rim toward -z.
    let hinge = joint.centroid - Vec3::Z * (joint.area / std::f32::consts::PI).sqrt();
    let r = top.centroid - hinge;
    let own = (3.0 * top.volume / (4.0 * std::f32::consts::PI)).cbrt();
    let inertia = top.mass * (r.length_squared() - r.x * r.x + 0.4 * own * own);
    let lift = top.mass * GRAVITY * (r.y.hypot(r.z) - r.y);
    let capacity = (2.0 * inertia * (material.seam.fracture_energy * joint.area + lift)).sqrt();
    let at = Vec3::new(0.1, 0.4, 0.25);
    let lever = at.y - joint.centroid.y;
    for (scale, fails) in [(1.01, true), (0.99, false)] {
        let mut state = WallState::new(&structure);
        let push = Load {
            chunk: 1,
            at,
            energy: 0.0,
            crushed: 0.0,
            blow: Vec3::NEG_Z * (scale * capacity / lever),
            press: Vec3::ZERO,
            peak: None,
            origin: Origin::Field,
        };
        let failures = structural(
            &structure,
            &mut state,
            GroupLoads::new(&[push], Vec3::ZERO),
            &mut SectionCache::default(),
        );
        assert_eq!(state.cracked[bed], fails, "{scale} x capacity");
        assert_eq!(failures.is_empty(), !fails);
    }
}

/// A BLAST'S COUPLING IS ITS GEOMETRY: a charge at the face of a wide wall sends about half its
/// rays into it, and fewer the farther it stands off. No coupling constant enters.
#[test]
fn blast_coupling_falls_with_standoff() {
    let field = blast_field();
    let slab = [Block::cuboid(
        Vec3::new(-50.0, -50.0, -0.25),
        Vec3::new(50.0, 50.0, 0.0),
    )];
    let coupled = |standoff: f32| -> f32 {
        sample_blocks(
            &slab,
            |_| true,
            |_| brick(),
            Vec3::Z * standoff,
            FIXTURE_BLAST,
            &field,
        )
        .iter()
        .map(|(_, hit)| hit.energy)
        .sum::<f32>()
            / FIXTURE_BLAST.energy
    };
    let contact = coupled(1.0e-4);
    assert!((contact - 0.5).abs() < 0.01, "contact couples {contact}");
    let wall = [Block::cuboid(
        Vec3::new(-2.0, -1.0, -0.25),
        Vec3::new(2.0, 1.0, 0.0),
    )];
    let on_wall = |standoff: f32| -> f32 {
        sample_blocks(
            &wall,
            |_| true,
            |_| brick(),
            Vec3::Z * standoff,
            FIXTURE_BLAST,
            &field,
        )
        .iter()
        .map(|(_, hit)| hit.energy)
        .sum::<f32>()
            / FIXTURE_BLAST.energy
    };
    let (near, mid, far) = (on_wall(0.1), on_wall(1.0), on_wall(3.0));
    assert!(near > mid && mid > far && far > 0.0, "{near} {mid} {far}");
}

/// Centroid quadrature of the same field on the shipped cut's coplanar front faces: one fixed
/// point and its actual face area per chunk, independent of which angular sample hits it.
/// This is a Law 3 fixture input, not a second production surface finder. ASSUMED fixture
/// energy (J), source height and distance (m); the cut supplies the geometry and material data.
fn wall_face_pushes(energy: f32, distance: f32) -> Vec<Load> {
    let layout = layout();
    let origin = Vec3::new(
        layout.centre().x,
        layout.min.y + 1.45,
        layout.min.z + layout.size.z + distance,
    );
    let blast = Blast::of(energy, 1e-3, FIT_ONLY_PRODUCTS);
    layout
        .cut
        .structure
        .chunks
        .iter()
        .zip(&layout.cut.blocks)
        .enumerate()
        .filter_map(|(chunk, (part, block))| {
            // ASSUMED f32 angular allowance when identifying the coplanar front cap.
            let area: f32 = block
                .face_areas()
                .iter()
                .filter(|(n, _)| n.dot(Vec3::Z) > 1.0 - 1e-5)
                .map(|(_, area)| area)
                .sum();
            assert!(
                area > 0.0,
                "the planar-wall fixture has a visible front cap"
            );
            let at = Vec3::new(part.centroid.x, part.centroid.y, block.max.z);
            let path = at - origin;
            let d = path.length();
            // Outside the sourced field's reach there is no load.
            if d > blast.reach() {
                return None;
            }
            let cos = -(path / d).dot(Vec3::Z);
            Some(Load {
                chunk,
                at,
                energy: 0.0,
                crushed: 0.0,
                blow: Vec3::NEG_Z * (area * blast.specific_impulse(d, cos)),
                press: Vec3::ZERO,
                peak: Some(Vec3::NEG_Z * (area * blast.peak_pressure(d, cos))),
                origin: Origin::Field,
            })
        })
        .collect()
}

/// The shipped free-standing wall under the sandbox's actual field-to-law path. ASSUMED fixture
/// burst energy (J), centre-to-face distance (m), filler density and height. No colliders, bodies
/// or view resources are spawned; `apply_deposit` returns the law outcome before entity work.
fn wall_deposit_at(energy: f32, distance: f32) -> Applied {
    let layout = layout();
    let mut wall = Wall {
        cut: layout.cut.clone(),
        live: crate::buildings::breakage::Live::from_structure(
            layout.cut.structure.clone(),
            layout.cut.elements,
        ),
        structural: vec![false; layout.cut.structure.joints.len()],
        entities: Vec::new(),
        meshes: Vec::new(),
        material: brick(),
    };
    let controls = WallControls {
        mass: 0,
        standoff: 0,
        cased: false,
        round: 0,
        view: View::Front,
        help: false,
    };
    let charge = Charge {
        density: 1650.0,
        chemical: 4.184e6,
        // ASSUMED fixture: a speed constant whose products floor never reaches the fit, so the
        // sweep reads the free-air field alone.
        speed_constant: 1e-6,
        casing_ratio: 0.0,
        casing_speed: 0.0,
    };
    let mass = energy / charge.chemical;
    let radius = charge.blast(mass, false).radius;
    assert!(
        distance > radius,
        "the fixture measures standoff, not contact"
    );
    let deposit = Deposit::Blast {
        mass,
        standoff: distance - radius,
        at: Vec3::new(
            layout.centre().x,
            layout.min.y + 1.45,
            layout.min.z + layout.size.z,
        ),
        into: Vec3::NEG_Z,
    };
    apply_deposit(&mut wall, &controls, &charge, &blast_field(), &deposit)
}

/// THE SAMPLED WALL HAS A FINITE FIELD REACH (the game's environment destruction model).
/// The real sandbox adapter must leave the structure untouched outside the sourced field.
/// Inside it, report its measured toppling curve without asserting an asset-specific distance
/// or assuming its changing angular hit footprint is monotone. ASSUMED fixture energies,
/// source height and sample spacing; the study's comparison distance is DERIVED there.
#[test]
fn the_sampled_wall_reports_its_far_toppling_limit() {
    let step = 0.25_f32;
    for (name, energy) in [
        ("study burst", FIXTURE_BLAST.energy),
        ("bare control", 5.0 * 4.184e6),
    ] {
        let reach = Blast::of(energy, 1e-3, FIT_ONLY_PRODUCTS).reach();
        let samples = (reach / step).ceil() as usize + 1;
        let (mut last_topple, mut first_hold, mut previous) = (None, None, None);
        let mut revivals = 0;
        for k in 2..=samples {
            let distance = k as f32 * step;
            let applied = wall_deposit_at(energy, distance);
            let fell: usize = applied.groups.iter().map(|(group, ..)| group.len()).sum();
            let topples = fell > 0 && applied.failures.iter().any(|f| f.cause == Cause::Push);
            if topples {
                last_topple = Some(distance);
                revivals += usize::from(previous == Some(false));
            } else if first_hold.is_none() {
                first_hold = Some(distance);
            }
            if previous != Some(topples) {
                println!(
                    "MEASURED sampled {name}: at {distance} m topples {topples}, released {fell}"
                );
            }
            previous = Some(topples);
        }
        let outside = wall_deposit_at(energy, samples as f32 * step);
        assert_eq!(
            outside.deposited, 0.0,
            "no energy arrives outside field reach"
        );
        assert!(
            outside.failures.is_empty() && outside.groups.is_empty(),
            "no structural change occurs outside field reach"
        );
        println!(
            "MEASURED sampled {name}: last topple {last_topple:?}, first hold {first_hold:?}, \
            revivals {revivals}, interval {step} m, sampled through {} m",
            samples as f32 * step
        );
        let at_study = wall_deposit_at(energy, 13.8);
        let fell: usize = at_study.groups.iter().map(|(group, ..)| group.len()).sum();
        println!("MEASURED sampled {name} at study's DERIVED 13.8 m: released {fell}");
    }
}

/// A FIXED FAR FACE LOADING TOPPLES MONOTONICALLY AND STOPS AT FINITE DISTANCE (the game's
/// environment destruction model). Law 3 must preserve this ordering when the source moves
/// beyond the wall's bounding diagonal and samples remain attached to its coplanar front caps.
/// This checks the material/section law independently of changing angular-ray coverage. The
/// authority relies on that separation when changing the surface finder. ASSUMED fixture
/// energies and resolution; the geometry supplies the beginning of the far regime. Measured
/// crossings are output only, never a required distance or required outcome for this asset.
#[test]
fn a_fixed_far_face_loading_topples_monotonically_and_stops() {
    let layout = layout();
    let step = 0.25_f32;
    let from = layout.size.length();
    for (name, energy) in [
        ("study burst", FIXTURE_BLAST.energy),
        ("bare control", 5.0 * 4.184e6),
        ("large bare control", 50.0 * 4.184e6),
    ] {
        let reach = Blast::of(energy, 1e-3, FIT_ONLY_PRODUCTS).reach();
        let samples = ((reach - from).max(0.0) / step).ceil() as usize + 1;
        let (mut last_topple, mut first_hold) = (None, None);
        for k in 0..=samples {
            let distance = from + k as f32 * step;
            let loads = wall_face_pushes(energy, distance);
            let mut state = WallState::new(&layout.cut.structure);
            let failures = structural(
                &layout.cut.structure,
                &mut state,
                GroupLoads::new(&loads, Vec3::ZERO),
                &mut SectionCache::default(),
            );
            let topples = failures.iter().any(|f| f.cause == Cause::Push)
                && !fall(&layout.cut.structure, &mut state).is_empty();
            assert!(
                !(first_hold.is_some() && topples),
                "{name}: fixed-face toppling reappeared at {distance} m after {first_hold:?}"
            );
            if topples {
                last_topple = Some(distance);
            } else if first_hold.is_none() {
                first_hold = Some(distance);
            }
        }
        assert!(
            first_hold.is_some(),
            "toppling stops within finite field reach"
        );
        assert!(
            wall_face_pushes(energy, from + samples as f32 * step).is_empty(),
            "the finite field stops before the final source pose"
        );
        println!(
            "MEASURED fixed-face {name}: last topple {last_topple:?}, first hold {first_hold:?}, \
            interval {step} m, far regime from {from} m"
        );
    }
}

/// A HULL'S RAY ENTRY IS THE SHAPE'S, NOT ITS BOX'S: a ray that passes the cut corner of a
/// wedge-shaped chunk misses it, though the wedge's bounding box would be hit; one through the
/// body enters at the face. The sandbox's blast rays and the wall's picks rely on it.
#[test]
fn a_ray_enters_the_hull_not_its_bounding_box() {
    // A unit cube with the corner at (1, 1, *) sliced off.
    let wedge = Block::hull(vec![
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.5, 1.0, 0.0),
        Vec3::new(1.0, 0.5, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(1.0, 0.0, 1.0),
        Vec3::new(0.0, 1.0, 1.0),
        Vec3::new(0.5, 1.0, 1.0),
        Vec3::new(1.0, 0.5, 1.0),
    ]);
    assert!(
        wedge
            .entry(Vec3::new(0.95, 0.95, 5.0), Vec3::NEG_Z)
            .is_none()
    );
    let hit = wedge.entry(Vec3::new(0.25, 0.25, 5.0), Vec3::NEG_Z);
    assert!((hit.expect("through the body") - 4.0).abs() < 1e-4);
}

/// The game's environment blast tool needs casing construction for its cased setting. The
/// operator must see the missing burst block at initialization, before a readout or load can
/// invent zero products momentum. A silent bare-only fallback fails this refusal.
#[test]
#[should_panic(expected = "rounds need a burst block")]
fn the_blast_tool_refuses_missing_casing_construction_at_initialization() {
    Charge::of(&[]);
}

/// The game's environment blast tool shares the production ray driver and deposit pass.
/// ASSUMED fixture placement; the charge uses the tool's cased round's construction, not a fitted
/// chemical-energy-to-wear conversion. Pressure can remove volume while cavity energy is zero.
#[test]
fn the_shared_blast_path_reports_pressure_work_and_rear_scabs() {
    let layout = layout();
    let rounds = super::wall_rounds();
    let he = rounds
        .iter()
        .find(|round| round.key == super::wall_tool().cased_round)
        .expect("the tool's cased round is shipped");
    let charge = Charge::of(std::slice::from_ref(he));
    let mass = he
        .round
        .burst
        .expect("the fixture states a burst")
        .core_mass;
    for (mass, cased, standoff) in [
        (mass, true, 0.1),
        (mass, true, 0.3),
        (mass, true, 2.0),
        (0.9, false, 0.1),
    ] {
        let mut wall = Wall {
            cut: layout.cut.clone(),
            live: crate::buildings::breakage::Live::from_structure(
                layout.cut.structure.clone(),
                layout.cut.elements,
            ),
            structural: vec![false; layout.cut.structure.joints.len()],
            entities: Vec::new(),
            meshes: Vec::new(),
            material: brick(),
        };
        let controls = WallControls {
            mass: 0,
            standoff: 0,
            cased,
            round: 0,
            view: View::Front,
            help: false,
        };
        let deposit = Deposit::Blast {
            mass,
            standoff,
            at: super::aim_point(&wall),
            into: Vec3::NEG_Z,
        };
        let result = apply_deposit(&mut wall, &controls, &charge, &blast_field(), &deposit);
        let (_, _, intercepted, impulse, _) = result.blast.unwrap();
        if standoff == 0.1 {
            assert!(
                result.crushed > 0.0 || result.scabs > 0,
                "a near-contact field on the aimed solid face must exercise pressure removal"
            );
        }
        assert_eq!(
            result.deposited, 0.0,
            "a blast supplies no kinetic cavity energy"
        );
        assert!(
            result.chunks > 0 && impulse > 0.0,
            "the charge reaches the wall"
        );
        println!(
            "MEASURED shared sandbox: {mass:.3} kg, cased {cased}, standoff {standoff:.2} m: kinetic {} J, audit {intercepted:.1} J, impulse {impulse:.1} N s, crushed {:.6e} m3, scabs {}, cracks {}, freed {} groups {:?}",
            result.deposited,
            result.crushed,
            result.scabs,
            result.local
                + result
                    .failures
                    .iter()
                    .map(|f| f.joints.len())
                    .sum::<usize>(),
            result.groups.iter().map(|g| g.0.len()).sum::<usize>(),
            result.groups.iter().map(|g| g.0.len()).collect::<Vec<_>>()
        );
    }
}

/// THE SANDBOX REACHES THE HOLE CAP AS THE GAME DOES, AND THE CAP DUSTS NOTHING: each subcritical
/// channel through one chunk keeps its hole up to [`HOLES_PER_CHUNK`]; one past it wears the pool
/// and leaves the chunk standing. A sandbox that cut no holes, or a count that dusts, fails this.
/// ASSUMED fixture: the largest chunk, the crush energy of a quarter of its pool (m³) spread over
/// the channels.
#[test]
fn the_sandbox_chunk_outlives_the_hole_cap() {
    use crate::buildings::breakage::HOLES_PER_CHUNK;
    let layout = layout();
    let charge = Charge::of(&super::wall_rounds());
    let mut wall = Wall {
        cut: layout.cut.clone(),
        live: crate::buildings::breakage::Live::from_structure(
            layout.cut.structure.clone(),
            layout.cut.elements,
        ),
        structural: vec![false; layout.cut.structure.joints.len()],
        entities: Vec::new(),
        meshes: Vec::new(),
        material: brick(),
    };
    let controls = WallControls {
        mass: 0,
        standoff: 0,
        cased: true,
        round: 0,
        view: View::Front,
        help: false,
    };
    let chunks = &wall.cut.structure.chunks;
    let c = (0..chunks.len())
        .max_by(|&a, &b| chunks[a].volume.total_cmp(&chunks[b].volume))
        .expect("the wall has chunks");
    let at = chunks[c].centroid;
    let energy =
        wall.live.state.pool[c] * chunks[c].material.crush / (4.0 * (HOLES_PER_CHUNK + 1) as f32);
    let channel = Deposit::Line {
        what: "fixture channel".to_owned(),
        lines: vec![(
            c,
            crate::ballistics::DeliveredLine {
                crossing: crate::ballistics::VolumeCrossing {
                    volume: bevy::prelude::Entity::PLACEHOLDER,
                    entry: at - Vec3::Z * 0.05,
                    end: at + Vec3::Z * 0.05,
                    chord: 0.1,
                    cost: 0.0,
                    energy,
                    perforated: true,
                },
                momentum: Vec3::ZERO,
                entrance_momentum: Vec3::ZERO,
                entrance_energy: 0.0,
            },
        )],
        spall: 0.0,
    };
    for kept in 1..=HOLES_PER_CHUNK {
        let result = apply_deposit(&mut wall, &controls, &charge, &blast_field(), &channel);
        assert!(!result.dusted.contains(&c), "channel {kept} is subcritical");
        assert_eq!(
            wall.live.holes[c].len(),
            kept,
            "each channel keeps its hole"
        );
    }
    let pool = wall.live.state.pool[c];
    let result = apply_deposit(&mut wall, &controls, &charge, &blast_field(), &channel);
    assert!(
        !result.dusted.contains(&c) && wall.live.state.pool[c] < pool,
        "the channel past the cap wears the pool and dusts nothing"
    );
    assert_eq!(wall.live.holes[c].len(), HOLES_PER_CHUNK);
}

/// The game's damage-model inspector preserves resolved momentum and entrance work even when
/// no muzzle speed identifies the round. The observer, shared load owner and hole reader must
/// agree. All geometry, work and momentum below are fixture data.
#[test]
fn the_inspector_reads_delivered_work_and_momentum_without_guessing_a_round() {
    use crate::ballistics::{DeliveredLine, ShellMarched, SpallMarks, VolumeCrossing};
    use bevy::prelude::*;
    let mut app = App::new();
    app.init_resource::<super::WallQueue>()
        .init_resource::<super::WallLog>()
        .add_observer(super::read_hits);
    let volume = app.world_mut().spawn(super::WallChunk(0)).id();
    let shell = app.world_mut().spawn(SpallMarks::default()).id();
    let line = DeliveredLine {
        crossing: VolumeCrossing {
            volume,
            entry: Vec3::ZERO,
            end: Vec3::Z,
            chord: 1.0,
            cost: 0.0,
            energy: 7.0,
            perforated: true,
        },
        entrance_energy: 5.0,
        entrance_momentum: Vec3::X * 12.0,
        momentum: Vec3::X * 12.0 + Vec3::Z * 30.0,
    };
    app.world_mut().trigger(ShellMarched {
        shell,
        marks_from: 0,
        cones_from: 0,
        lines: vec![line],
        burst: None,
    });
    let queue = app.world().resource::<super::WallQueue>();
    let Deposit::Line { lines, .. } = &queue.0[0] else {
        panic!("one resolved line")
    };
    assert_eq!(lines.len(), 1);
    let loads = crate::buildings::breakage::resolved_line_loads(0, &brick(), &lines[0].1);
    assert!((loads.iter().map(|load| load.energy).sum::<f32>() - 7.0).abs() < 1e-5);
    assert!((loads.iter().map(|load| load.blow).sum::<Vec3>() - line.momentum).length() < 1e-4);
    let entrance = loads.iter().find(|load| load.blow.x > 0.0).unwrap();
    assert_eq!(entrance.at, Vec3::ZERO);
    assert_eq!(entrance.energy, 5.0);
}

/// Authored inspector spread cannot poison a fired direction; the data gate names its repair.
#[test]
fn the_inspection_tool_refuses_invalid_spread_at_admission() {
    for value in ["-1.0", "NaN", "inf"] {
        let text =
            format!(r#"(cased_round: "fixture", spread_rad: ({value}, Fixture("fixture")))"#);
        let error = super::tool_from_ron(&text)
            .err()
            .expect("invalid spread is refused");
        assert!(error.contains("spread_rad"));
    }
}
