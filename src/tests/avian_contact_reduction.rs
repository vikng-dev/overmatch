//! Guards for the vendored body-pair contact reduction (`OVERMATCH_PATCH.md`,
//! `collision/contact_types/reduction.rs`). The game's environment destruction relies on it to
//! keep two pieces' many hull pairs from each costing the solver a manifold; game state only.
//! The grids, boxes and poses are fixture data: any spread of hull pairs exercises the same laws.

use avian3d::collision::contact_types::{
    MAX_NORMALS, MAX_POINTS_PER_NORMAL, PairManifolds, SELECTION_TIE, reduce_body_pair,
};
use avian3d::prelude::*;
use bevy::prelude::*;

const REST: f32 = 0.15;

fn point(at: Vec3, depth: f32) -> ContactPoint {
    ContactPoint::new(at, at, at, depth)
}

/// A square patch of four points about `centre` with one normal, as a hull pair produces.
fn patch(centre: Vec3, normal: Vec3, half: f32, depth: f32) -> ContactManifold {
    let (a, b) = normal.any_orthonormal_pair();
    let corners = [a + b, a - b, -a - b, -a + b].map(|corner| point(centre + corner * half, depth));
    ContactManifold::new(corners, normal)
}

fn pair(manifolds: Vec<ContactManifold>) -> PairManifolds {
    PairManifolds {
        reversed: false,
        manifolds,
    }
}

fn kept(pairs: &[PairManifolds]) -> impl Iterator<Item = &ContactPoint> {
    pairs
        .iter()
        .flat_map(|pair| &pair.manifolds)
        .flat_map(|manifold| &manifold.points)
}

/// A flat seam of many hull pairs costs the solver one bounded set of points, and those points
/// must span the seam's whole footprint: a body resting on them must not tip about a kept edge
/// that lies short of the real footprint's corner. A mutant keeping any four points breaks it.
#[test]
fn a_flat_many_hull_seam_reduces_to_its_footprint_corners() {
    let mut pairs: Vec<PairManifolds> = (-2..=2)
        .flat_map(|x| {
            (-2..=2).map(move |z| {
                pair(vec![patch(
                    Vec3::new(x as f32, 0.0, z as f32),
                    Vec3::Y,
                    0.4,
                    0.0,
                )])
            })
        })
        .collect();
    reduce_body_pair(&mut pairs, REST, SELECTION_TIE);
    assert_eq!(kept(&pairs).count(), MAX_POINTS_PER_NORMAL);
    let manifolds = pairs.iter().map(|pair| pair.manifolds.len()).sum::<usize>();
    assert!(manifolds <= MAX_POINTS_PER_NORMAL, "got {manifolds}");
    for corner in [(1.0, 1.0), (1.0, -1.0), (-1.0, -1.0), (-1.0, 1.0)] {
        let reached = kept(&pairs)
            .map(|p| p.point.x * corner.0 + p.point.z * corner.1)
            .fold(f32::MIN, f32::max);
        assert!(reached > 4.5, "corner {corner:?} reached only {reached}");
    }
}

/// Distinct support normals each keep their own contact: a body braced on a floor and two walls
/// must not lose one face to another's budget. A mutant clustering everything under one normal,
/// or a cap of one normal, breaks it.
#[test]
fn distinct_support_normals_each_survive() {
    let normals = [Vec3::Y, Vec3::X, Vec3::Z];
    let mut pairs: Vec<PairManifolds> = normals
        .iter()
        .flat_map(|&normal| {
            (0..4).map(move |step| {
                pair(vec![patch(
                    normal * 3.0 + normal.any_orthonormal_vector() * step as f32,
                    normal,
                    0.4,
                    0.0,
                )])
            })
        })
        .collect();
    reduce_body_pair(&mut pairs, REST, SELECTION_TIE);
    for normal in normals {
        let count = pairs
            .iter()
            .flat_map(|pair| &pair.manifolds)
            .filter(|manifold| manifold.normal == normal)
            .map(|manifold| manifold.points.len())
            .sum::<usize>();
        assert!(
            (1..=MAX_POINTS_PER_NORMAL).contains(&count),
            "{normal:?} kept {count}"
        );
    }
}

/// Past the normal cap, the normals dropped are the shallowest: a separated seam may lose its
/// budget to a pressed face, never the reverse. A mutant ordering normals otherwise breaks it.
#[test]
fn the_normal_cap_drops_the_shallowest_face() {
    let spokes: Vec<Vec3> = (0..=MAX_NORMALS)
        .map(|i| {
            let turn = i as f32 * std::f32::consts::TAU / (MAX_NORMALS + 1) as f32;
            Vec3::new(turn.cos(), 0.3, turn.sin()).normalize()
        })
        .collect();
    let shallowest = spokes[MAX_NORMALS];
    let mut pairs: Vec<PairManifolds> = spokes
        .iter()
        .enumerate()
        .map(|(i, &normal)| {
            let depth = if normal == shallowest {
                -0.004
            } else {
                0.001 * i as f32
            };
            pair(vec![patch(normal * 2.0, normal, 0.3, depth)])
        })
        .collect();
    reduce_body_pair(&mut pairs, REST, SELECTION_TIE);
    assert!(
        !pairs
            .iter()
            .flat_map(|pair| &pair.manifolds)
            .any(|manifold| manifold.normal == shallowest),
        "the separated face outlived a pressed one"
    );
    assert_eq!(
        pairs.iter().map(|pair| pair.manifolds.len()).sum::<usize>(),
        MAX_NORMALS
    );
}

/// A collider pair ordered the other way round reports its normal flipped; the same face must
/// still cluster as one support and an opposite face must not. A mutant that ignores `reversed`
/// breaks one side of this.
#[test]
fn a_reversed_pair_clusters_on_its_oriented_normal() {
    let same = |reversed| PairManifolds {
        reversed,
        manifolds: vec![patch(Vec3::X * 2.0, Vec3::Y, 0.3, 0.0)],
    };
    let mut flipped = same(true);
    flipped.manifolds[0].normal = -Vec3::Y;
    // Same face, two orderings: one normal, so at most one budget of points.
    let mut pairs = vec![same(false), flipped.clone(), same(false), flipped.clone()];
    pairs[2].manifolds[0]
        .points
        .iter_mut()
        .for_each(|p| p.point.x += 1.0);
    pairs[3].manifolds[0]
        .points
        .iter_mut()
        .for_each(|p| p.point.x -= 1.0);
    reduce_body_pair(&mut pairs, REST, SELECTION_TIE);
    assert_eq!(kept(&pairs).count(), MAX_POINTS_PER_NORMAL);
    // A face reported the opposite way is a second support: both keep points.
    let mut opposed = vec![same(false), same(false), same(true), same(true)];
    opposed[1].manifolds[0]
        .points
        .iter_mut()
        .for_each(|p| p.point.x += 1.0);
    opposed[3].manifolds[0]
        .points
        .iter_mut()
        .for_each(|p| p.point.x += 1.0);
    reduce_body_pair(&mut opposed, REST, SELECTION_TIE);
    assert!(
        kept(&opposed).count() > MAX_POINTS_PER_NORMAL,
        "an opposite support was merged into the first"
    );
}

/// A point closing past the rest speed is a landing, read per chunk by the game's crumble law:
/// every one survives reduction. A mutant that budgets landings breaks it.
#[test]
fn closing_points_always_survive() {
    let mut pairs: Vec<PairManifolds> = (0..6)
        .map(|i| {
            let mut manifold = patch(Vec3::X * i as f32, Vec3::Y, 0.4, 0.0);
            for point in &mut manifold.points {
                point.normal_speed = -(REST * 2.0);
            }
            pair(vec![manifold])
        })
        .collect();
    reduce_body_pair(&mut pairs, REST, SELECTION_TIE);
    assert_eq!(kept(&pairs).count(), 6 * 4, "a landing point was reduced");
}

/// A point already carrying load keeps its place against a marginally farther one, otherwise a
/// resting pair trades its warm-started point for a cold one on solver noise and the pile rocks.
/// Removing the carried bonus breaks it.
#[test]
fn a_carried_load_holds_its_place_within_one_tie() {
    let build = |carried_farther: bool| {
        let lone = |x: f32| {
            let mut manifold = ContactManifold::new([point(Vec3::new(x, 0.0, 0.0), 0.0)], Vec3::Y);
            manifold.points[0].warm_start_normal_impulse = 0.0;
            manifold
        };
        let mut far = lone(10.0);
        let mut nearly_as_far = lone(10.0 - 0.5 * SELECTION_TIE);
        let loaded = if carried_farther {
            &mut far
        } else {
            &mut nearly_as_far
        };
        loaded.points[0].warm_start_normal_impulse = 1.0;
        let mut pairs = vec![
            pair(vec![far]),
            pair(vec![nearly_as_far]),
            pair(vec![patch(Vec3::new(0.0, 0.0, 3.0), Vec3::Y, 0.1, 0.0)]),
            pair(vec![patch(Vec3::new(0.0, 0.0, -3.0), Vec3::Y, 0.1, 0.0)]),
        ];
        reduce_body_pair(&mut pairs, REST, SELECTION_TIE);
        pairs
    };
    for carried_farther in [false, true] {
        let pairs = build(carried_farther);
        let survivor = kept(&pairs)
            .find(|p| p.point.x > 5.0)
            .expect("one far point survives");
        assert_eq!(
            survivor.warm_start_normal_impulse, 1.0,
            "the uncarried near-tie displaced the carried point (carried_farther {carried_farther})"
        );
    }
}

fn app() -> App {
    let mut app = super::avian_child_pose_churn::physics_app();
    app.insert_resource(Gravity(Vec3::NEG_Y * 9.81));
    app
}

/// A static floor of `n`×`n` 1 m box hulls about the origin; returns the root.
fn floor_of(app: &mut App, n: i32) -> Entity {
    let root = app
        .world_mut()
        .spawn((RigidBody::Static, Transform::default()))
        .id();
    for x in 0..n {
        for z in 0..n {
            app.world_mut().spawn((
                ChildOf(root),
                Collider::cuboid(1.0, 1.0, 1.0),
                Transform::from_xyz(
                    x as f32 - (n - 1) as f32 / 2.0,
                    -0.5,
                    z as f32 - (n - 1) as f32 / 2.0,
                ),
            ));
        }
    }
    root
}

/// A body of a 4×4 grid of 1 m box hulls standing on the floor, its hulls' seams offset from the
/// floor's so every box meets several. `extra` shifts its centre of mass; `hooked` spawns its
/// hulls with the contact hook, as the game's belts and standing walls do.
fn slab_at(app: &mut App, at: Vec3, reducible: bool, extra: Vec3, hooked: bool) -> Entity {
    let mut body = app.world_mut().spawn((
        RigidBody::Dynamic,
        Transform::from_translation(at),
        CenterOfMass(extra),
        Mass(1000.0),
        AngularInertia::new(Vec3::splat(2000.0)),
        NoAutoMass,
        NoAutoAngularInertia,
        NoAutoCenterOfMass,
    ));
    if reducible {
        body.insert(ReducibleContacts);
    }
    let root = body.id();
    for x in 0..4 {
        for z in 0..4 {
            let hooks = if hooked {
                ActiveCollisionHooks::MODIFY_CONTACTS
            } else {
                ActiveCollisionHooks::empty()
            };
            app.world_mut().spawn((
                ChildOf(root),
                Collider::cuboid(1.0, 1.0, 1.0),
                Transform::from_xyz(x as f32 - 1.5, 0.0, z as f32 - 1.5),
                hooks,
            ));
        }
    }
    root
}

fn slab(app: &mut App, reducible: bool, extra: Vec3, hooked: bool) -> Entity {
    slab_at(app, Vec3::new(0.3, 0.5, 0.3), reducible, extra, hooked)
}

/// A marked dynamic base resting on the static floor and a body of box hulls on top of it, the
/// base's and body's hull seams offset so each box meets several; a rest touching only the upper
/// interface is what reduction bounds. Returns `(base, body)`.
fn stack(app: &mut App, reducible: bool, extra: Vec3, hooked: bool) -> (Entity, Entity) {
    let base = wide_base(app);
    let body = slab_at(app, Vec3::new(0.3, 1.5, 0.3), reducible, extra, hooked);
    (base, body)
}

/// A wide, heavy, marked dynamic slab on the floor: a stable table the test body stands on.
fn wide_base(app: &mut App) -> Entity {
    let root = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Transform::from_xyz(0.0, 0.5, 0.0),
            ReducibleContacts,
            Mass(50_000.0),
            AngularInertia::new(Vec3::splat(500_000.0)),
            CenterOfMass(Vec3::ZERO),
            NoAutoMass,
            NoAutoAngularInertia,
            NoAutoCenterOfMass,
        ))
        .id();
    for x in 0..8 {
        for z in 0..8 {
            app.world_mut().spawn((
                ChildOf(root),
                Collider::cuboid(1.0, 1.0, 1.0),
                Transform::from_xyz(x as f32 - 3.5, 0.0, z as f32 - 3.5),
            ));
        }
    }
    root
}

/// Pairs involving `body`.
fn body_pairs(app: &App, body: Entity) -> Vec<&ContactPair> {
    app.world()
        .resource::<ContactGraph>()
        .active_pairs()
        .iter()
        .filter(move |pair| pair.is_touching() && [pair.body1, pair.body2].contains(&Some(body)))
        .collect()
}

fn steps(app: &mut App, count: usize) {
    for _ in 0..count {
        app.update();
    }
}

fn contact_counts(pairs: &[&ContactPair]) -> (usize, usize, usize) {
    pairs
        .iter()
        .fold((0, 0, 0), |(pairs, manifolds, points), pair| {
            (
                pairs + 1,
                manifolds + pair.manifolds.len(),
                points
                    + pair
                        .manifolds
                        .iter()
                        .map(|manifold| manifold.points.len())
                        .sum::<usize>(),
            )
        })
}

/// A body on many hull pairs of another body, reduced, stands where it was set and does not tip
/// even with its centre of mass well off-centre over a corner of its footprint, and its flat
/// interface reduces to one bounded set of points. A selection keeping interior or one-sided
/// points tips it; a missing reduction fails the count. The unreduced twin shows the fixture has
/// many manifolds to reduce.
#[test]
fn a_body_on_reduced_contacts_stays_supported_with_a_bounded_count() {
    let mut reduced = app();
    floor_of(&mut reduced, 14);
    let (_, body) = stack(&mut reduced, true, Vec3::new(1.2, 0.0, 1.2), false);
    steps(&mut reduced, 20);
    let (pairs, manifolds, points) = contact_counts(&body_pairs(&reduced, body));
    assert!(pairs > 4, "fixture: many hull pairs touch, got {pairs}");
    assert_eq!(
        points, MAX_POINTS_PER_NORMAL,
        "{points} points over {pairs} pairs"
    );
    assert!(manifolds < pairs, "reduction removed whole manifolds");

    let mut full = app();
    floor_of(&mut full, 14);
    let (_, unmarked) = stack(&mut full, false, Vec3::new(1.2, 0.0, 1.2), false);
    steps(&mut full, 20);
    let (_, full_manifolds, full_points) = contact_counts(&body_pairs(&full, unmarked));
    assert!(
        full_manifolds > manifolds && full_points > points,
        "fixture: the unreduced twin carries more ({full_manifolds} manifolds, {full_points} points against {manifolds}, {points})"
    );
    steps(&mut reduced, 120);

    let rotation = reduced.world().get::<Rotation>(body).unwrap().0;
    assert!(
        rotation.angle_between(Quat::IDENTITY) < 0.02,
        "the body tipped: {rotation:?}"
    );
    let position = reduced.world().get::<Position>(body).unwrap().0;
    assert!(
        (position - Vec3::new(0.3, 1.5, 0.3)).length() < 0.05,
        "the body moved: {position:?}"
    );
}

/// Two bodies resting reduced touch through pairs left with no manifold; those pairs stay in the
/// island and leave it cleanly when the contact ends, or a re-touch finds a stale link and the
/// island panics. A mutant skipping the unlink breaks this at the second touch.
#[test]
fn a_pair_reduced_to_nothing_leaves_and_rejoins_its_island() {
    let mut app = app();
    floor_of(&mut app, 14);
    let (_, body) = stack(&mut app, true, Vec3::ZERO, false);
    steps(&mut app, 20);
    assert!(
        body_pairs(&app, body)
            .iter()
            .any(|pair| pair.manifolds.is_empty()),
        "fixture: some touching pair lost all its manifolds"
    );
    for _ in 0..2 {
        app.world_mut().get_mut::<Position>(body).unwrap().0.y += 3.0;
        steps(&mut app, 20);
        app.world_mut().get_mut::<Position>(body).unwrap().0.y = 1.5;
        steps(&mut app, 20);
    }
}

/// A pair reduced to nothing belongs to its island like any other: when a removed neighbour
/// splits the island, the rebuild must carry such a pair along, or its stale links panic when it
/// later ends. A mutant that rebuilds only contacts with constraint handles breaks this.
#[test]
fn an_island_split_keeps_pairs_reduced_to_nothing() {
    let mut app = app();
    floor_of(&mut app, 16);
    let row = |app: &mut App, x: f32| slab_at(app, Vec3::new(x, 0.5, 0.3), true, Vec3::ZERO, false);
    let left = row(&mut app, -4.0);
    row(&mut app, 0.0);
    let right = row(&mut app, 4.0);
    steps(&mut app, 20);
    assert!(
        app.world()
            .resource::<ContactGraph>()
            .active_pairs()
            .iter()
            .any(|pair| pair.is_touching() && pair.manifolds.is_empty()),
        "fixture: a pair between neighbouring slabs lost all its manifolds"
    );
    // The rest timer then lets the island split around the removed contact.
    app.world_mut().get_mut::<Position>(right).unwrap().0.y += 20.0;
    steps(&mut app, 40);
    // Ending a contact the split had to carry walks its island links.
    app.world_mut().get_mut::<Position>(left).unwrap().0.y += 20.0;
    steps(&mut app, 40);
}

/// Reduction applies only between two marked bodies: a static side (the terrain), an unmarked
/// dynamic body (a tank) and any pair carrying a contact hook (a belt, a standing wall) keep every
/// manifold, so their contacts match an unreduced world. A mutant reducing by default or
/// ignoring a side's marker breaks it.
#[test]
fn only_marked_body_pairs_reduce() {
    let per_pair = |pairs: Vec<&ContactPair>| {
        assert!(pairs.len() > 4, "fixture: {} hull pairs", pairs.len());
        pairs
            .iter()
            .all(|pair| pair.manifolds.len() == 1 && pair.manifolds[0].points.len() == 4)
    };
    // A marked body on the static floor: the floor is not a piece.
    let mut on_floor = app();
    floor_of(&mut on_floor, 14);
    let body = slab(&mut on_floor, true, Vec3::ZERO, false);
    steps(&mut on_floor, 20);
    assert!(
        per_pair(body_pairs(&on_floor, body)),
        "a static side was reduced"
    );

    // An unmarked body on a marked one.
    let mut unmarked = app();
    floor_of(&mut unmarked, 14);
    let (_, body) = stack(&mut unmarked, false, Vec3::ZERO, false);
    steps(&mut unmarked, 20);
    assert!(
        per_pair(body_pairs(&unmarked, body)),
        "an unmarked side was reduced"
    );

    // A marked body whose hulls carry the contact hook.
    let mut hooked = app();
    floor_of(&mut hooked, 14);
    let (_, body) = stack(&mut hooked, true, Vec3::ZERO, true);
    steps(&mut hooked, 20);
    assert!(
        per_pair(body_pairs(&hooked, body)),
        "a hooked pair was reduced"
    );
}
