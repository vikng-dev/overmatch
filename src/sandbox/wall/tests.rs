//! The wall prototype's invariants, about the game's environment destruction (game state only).
//! Fixture numbers are fixture data; the brick is the prototype's own ([`super::brick`]).

use bevy::math::Vec3;

use super::model::*;
use super::{CHUNK, JITTER_SEED, WALL_MIN, WALL_SIZE, blast_field, brick};

/// Both cuts the sandbox offers: a grid and one that is no grid.
fn cuts() -> [Cut; 2] {
    [
        running_bond(WALL_MIN, WALL_SIZE, CHUNK, brick()),
        jittered(WALL_MIN, WALL_SIZE, CHUNK, JITTER_SEED, brick()),
    ]
}

/// SUPPORT IS CONNECTIVITY TO AN ANCHOR THROUGH INTACT JOINTS, on any cut. An intact cut has no
/// loose group (a joint the generator missed would drop chunks the moment the wall stands);
/// cracking every joint across a region's boundary frees exactly that region, as one group; and
/// cracking every anchor frees the whole wall as one. The sandbox's falls, and the game's later,
/// rely on it.
#[test]
fn a_group_with_no_intact_path_to_the_ground_falls_as_one() {
    for cut in cuts() {
        let structure = &cut.structure;
        let state = WallState::new(structure);
        assert!(
            loose_groups(structure, &state).is_empty(),
            "an intact cut stands"
        );

        let region: Vec<usize> = (0..structure.chunks.len())
            .filter(|&i| structure.chunks[i].centroid.y > 1.4)
            .collect();
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
}

/// THE LAWS READ THE TABLE, NOT THE PATTERN: renumbering a cut's chunks (and its joints with them)
/// changes no law's cracks, no structural failure and no fall. A law that indexed a grid would
/// crack other joints once the numbering moved.
#[test]
fn renumbering_the_chunks_changes_no_outcome() {
    let cut = jittered(WALL_MIN, WALL_SIZE, CHUNK, JITTER_SEED, brick());
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
    let centre = WALL_MIN + WALL_SIZE * Vec3::new(0.5, 0.55, 1.0);
    let sources = [Source {
        energy: 1.0e6,
        at: centre,
        push: Vec3::NEG_Z,
        back: Some(WALL_SIZE.z),
    }];
    let pushes = |structure: &Structure| -> Vec<Push> {
        (0..structure.chunks.len())
            .filter(|&i| structure.chunks[i].centroid.distance(centre) < 0.5)
            .map(|i| Push {
                chunk: i,
                at: structure.chunks[i].centroid,
                impulse: Vec3::NEG_Z * 400.0,
            })
            .collect()
    };
    let outcome = |structure: &Structure, law: CrackLaw| {
        let mut state = WallState::new(structure);
        crack(law, structure, &mut state, &sources);
        structural(structure, &mut state, &pushes(structure));
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
    for law in CrackLaw::ALL {
        let original = outcome(&cut.structure, law);
        assert!(
            !original.0.is_empty(),
            "{law:?}: the fixture cracks something"
        );
        assert_eq!(outcome(&renumbered, law), original, "{law:?}");
    }
}

/// A POINT DEPOSIT CRUSHES EXACTLY ITS ENERGY while the wall can hold it, and no more than the
/// wall's pools when it cannot: removal neither creates nor loses energy.
#[test]
fn a_point_deposit_crushes_its_energy() {
    let cut = running_bond(WALL_MIN, WALL_SIZE, CHUNK, brick());
    let state = WallState::new(&cut.structure);
    let at = WALL_MIN + WALL_SIZE * Vec3::new(0.5, 0.5, 1.0);
    for energy in [5.0e4, 1.0e6, 5.0e6] {
        let crush = crush_point(&cut.structure, &state, at, energy);
        let total: f32 = crush.doses.iter().map(|(_, dose)| dose).sum();
        assert!(
            (total - energy).abs() <= 1.0e-3 * energy,
            "{energy} J: {total}"
        );
        assert!(crush.doses.iter().all(|(_, dose)| *dose >= 0.0));
    }
    let wall: f32 = state.pool.iter().sum();
    let crush = crush_point(&cut.structure, &state, at, 10.0 * wall);
    assert!(
        (crush.spent - wall).abs() <= 1.0e-3 * wall,
        "{} of {wall}",
        crush.spent
    );
}

/// THE STRUCTURAL CHECK COMPARES THE ANGULAR IMPULSE WITH `√(2 I G_f A)`: on a two-chunk column a
/// push at the top whose moment about the bed joint is 1 % over that capacity cracks the bed joint
/// (the joint alone holds the top chunk), and 1 % under it cracks nothing. Fixture geometry.
#[test]
fn a_push_fails_the_section_at_its_fracture_impulse() {
    let material = brick();
    let blocks = vec![
        Block {
            min: Vec3::ZERO,
            max: Vec3::new(0.2, 0.2, 0.25),
        },
        Block {
            min: Vec3::new(0.0, 0.2, 0.0),
            max: Vec3::new(0.2, 0.4, 0.25),
        },
    ];
    let cut = of_blocks(blocks, 0.0, material);
    let bed = cut
        .structure
        .joints
        .iter()
        .position(|joint| joint.b == Side::Chunk(1))
        .expect("the column's bed joint");
    let joint = cut.structure.joints[bed];
    let top = cut.structure.chunks[1];
    let r = top.centroid - joint.centroid;
    let own = (3.0 * top.volume / (4.0 * std::f32::consts::PI)).cbrt();
    let inertia = top.mass * (r.length_squared() - r.x * r.x + 0.4 * own * own);
    let capacity = (2.0 * inertia * material.joint_toughness * joint.area).sqrt();
    let at = Vec3::new(0.1, 0.4, 0.25);
    let lever = at.y - joint.centroid.y;
    for (scale, fails) in [(1.01, true), (0.99, false)] {
        let mut state = WallState::new(&cut.structure);
        let push = Push {
            chunk: 1,
            at,
            impulse: Vec3::NEG_Z * (scale * capacity / lever),
        };
        let failures = structural(&cut.structure, &mut state, &[push]);
        assert_eq!(state.cracked[bed], fails, "{scale} x capacity");
        assert_eq!(failures.is_empty(), !fails);
    }
}

/// A BLAST'S COUPLING IS ITS GEOMETRY: a charge at the face of a wide wall sends about half its
/// rays into it, and fewer the farther it stands off. No coupling constant enters.
#[test]
fn blast_coupling_falls_with_standoff() {
    let field = blast_field();
    let slab = [Block {
        min: Vec3::new(-50.0, -50.0, -0.25),
        max: Vec3::new(50.0, 50.0, 0.0),
    }];
    let coupled = |standoff: f32| -> f32 {
        blast_rays(&slab, |_| true, Vec3::Z * standoff, 1.0, 1.0, &field)
            .iter()
            .map(|hit| hit.energy)
            .sum()
    };
    let contact = coupled(1.0e-4);
    assert!((contact - 0.5).abs() < 0.01, "contact couples {contact}");
    let wall = [Block {
        min: Vec3::new(-2.0, -1.0, -0.25),
        max: Vec3::new(2.0, 1.0, 0.0),
    }];
    let on_wall = |standoff: f32| -> f32 {
        blast_rays(&wall, |_| true, Vec3::Z * standoff, 1.0, 1.0, &field)
            .iter()
            .map(|hit| hit.energy)
            .sum()
    };
    let (near, mid, far) = (on_wall(0.1), on_wall(1.0), on_wall(3.0));
    assert!(near > mid && mid > far && far > 0.0, "{near} {mid} {far}");
}
