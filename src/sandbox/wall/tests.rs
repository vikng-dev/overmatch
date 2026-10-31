//! The wall prototype's invariants, about the game's environment destruction (game state only),
//! on the game's laws over the baked wall. Fixture numbers are fixture data; the brick is the
//! shipped row's ([`super::brick`]).

use bevy::math::Vec3;

use super::model::*;
use super::{blast_field, brick, layout};

/// A blast of 1 J and no charge (fixture data): its rays' shares of the energy read straight off
/// their hits.
const UNIT_BLAST: Blast = Blast {
    energy: 1.0,
    charge: 0.0,
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
    let sources = [Source {
        energy: 1.0e6,
        at: centre,
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
    let outcome = |structure: &Structure| {
        let mut state = WallState::new(structure);
        crack(structure, &mut state, &sources);
        structural(
            structure,
            &mut state,
            &pushes(structure),
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

/// THE STRUCTURAL CHECK COMPARES THE ANGULAR IMPULSE WITH `√(2 I G_f A)`: on a two-chunk column a
/// push at the top whose moment about the bed joint is 1 % over that capacity cracks the bed joint
/// (the joint alone holds the top chunk), and 1 % under it cracks nothing. Fixture geometry.
#[test]
fn a_push_fails_the_section_at_its_fracture_impulse() {
    let material = brick();
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
            },
            Joint {
                a: 0,
                b: Side::Chunk(1),
                area: 0.05,
                centroid: Vec3::new(0.1, 0.2, 0.125),
                normal: Vec3::Y,
            },
        ],
    };
    let bed = 1;
    let joint = structure.joints[bed];
    let top = structure.chunks[1];
    let r = top.centroid - joint.centroid;
    let own = (3.0 * top.volume / (4.0 * std::f32::consts::PI)).cbrt();
    let inertia = top.mass * (r.length_squared() - r.x * r.x + 0.4 * own * own);
    let capacity = (2.0 * inertia * material.fracture_energy * joint.area).sqrt();
    let at = Vec3::new(0.1, 0.4, 0.25);
    let lever = at.y - joint.centroid.y;
    for (scale, fails) in [(1.01, true), (0.99, false)] {
        let mut state = WallState::new(&structure);
        let push = Push {
            chunk: 1,
            at,
            impulse: Vec3::NEG_Z * (scale * capacity / lever),
        };
        let failures = structural(
            &structure,
            &mut state,
            &[push],
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
        blast_rays(&slab, |_| true, Vec3::Z * standoff, UNIT_BLAST, &field)
            .iter()
            .map(|hit| hit.energy)
            .sum()
    };
    let contact = coupled(1.0e-4);
    assert!((contact - 0.5).abs() < 0.01, "contact couples {contact}");
    let wall = [Block::cuboid(
        Vec3::new(-2.0, -1.0, -0.25),
        Vec3::new(2.0, 1.0, 0.0),
    )];
    let on_wall = |standoff: f32| -> f32 {
        blast_rays(&wall, |_| true, Vec3::Z * standoff, UNIT_BLAST, &field)
            .iter()
            .map(|hit| hit.energy)
            .sum()
    };
    let (near, mid, far) = (on_wall(0.1), on_wall(1.0), on_wall(3.0));
    assert!(near > mid && mid > far && far > 0.0, "{near} {mid} {far}");
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
