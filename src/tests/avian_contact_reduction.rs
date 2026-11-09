//! Guards for the vendored body-pair contact reduction (`OVERMATCH_PATCH.md`,
//! `collision/contact_types/reduction.rs`). The game's environment destruction relies on it to
//! keep two pieces' many hull pairs from each costing the solver a manifold; game state only.
//! The grids, boxes and poses are fixture data: any spread of hull pairs exercises the same laws.

use avian3d::collision::contact_types::{
    BEARING_GAP, HULL_TOLERANCE, MAX_NORMALS, PackedFeatureId, PairManifolds, ReductionStep,
    reduce_body_pair,
};
use avian3d::prelude::*;
use bevy::prelude::*;

/// Fixture: Avian's default rest speed, m/s, and the game's 64 Hz step.
const REST: f32 = 0.15;
const STEP_SECS: f32 = 1.0 / 64.0;

/// The reducer's step: the fixture step, unit lengths and the engine's own overlap slop.
fn step() -> ReductionStep {
    ReductionStep {
        delta_secs: STEP_SECS,
        length_unit: 1.0,
        overlap_slop: NarrowPhaseConfig::default().contact_tolerance,
    }
}

fn point(at: Vec3, depth: f32) -> ContactPoint {
    ContactPoint::new(at, at, at, depth)
}

/// A square patch of four points about `centre` with one normal, as a hull pair produces.
fn patch(centre: Vec3, normal: Vec3, half: f32, depth: f32) -> ContactManifold {
    let (a, b) = normal.any_orthonormal_pair();
    let corners = [a + b, a - b, -a - b, -a + b].map(|corner| point(centre + corner * half, depth));
    ContactManifold::new(corners, normal)
}

/// A rectangle of four coplanar points in the XZ plane, facing up.
fn flat_patch(centre: Vec3, half: f32, depth: f32) -> ContactManifold {
    let corners = [(1.0, 1.0), (1.0, -1.0), (-1.0, -1.0), (-1.0, 1.0)]
        .map(|(x, z)| point(centre + Vec3::new(x, 0.0, z) * half, depth));
    ContactManifold::new(corners, Vec3::Y)
}

fn lone(at: Vec3, depth: f32) -> ContactManifold {
    ContactManifold::new([point(at, depth)], Vec3::Y)
}

fn entity(raw: u32) -> Entity {
    Entity::from_raw_u32(raw).expect("a fixture id")
}

/// One collider pair per manifold, the n-th holding collider ids `n` and `n + 1000`.
fn pairs_of(manifolds: impl IntoIterator<Item = ContactManifold>) -> Vec<PairManifolds> {
    manifolds
        .into_iter()
        .enumerate()
        .map(|(n, manifold)| PairManifolds {
            colliders: (entity(n as u32 + 1), entity(n as u32 + 1001)),
            reversed: false,
            manifolds: vec![manifold],
        })
        .collect()
}

fn reduce(pairs: &mut [PairManifolds]) {
    reduce_body_pair(pairs, REST, step());
}

/// Four lone resting points at the corners of a square `half` across each way.
fn corners(half: f32) -> impl Iterator<Item = ContactManifold> {
    [(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)]
        .into_iter()
        .map(move |(x, z)| lone(Vec3::new(x, 0.0, z) * half, 0.0))
}

fn kept(pairs: &[PairManifolds]) -> impl Iterator<Item = &ContactPoint> {
    pairs
        .iter()
        .flat_map(|pair| &pair.manifolds)
        .flat_map(|manifold| &manifold.points)
}

fn count(pairs: &[PairManifolds]) -> usize {
    kept(pairs).count()
}

fn kept_at(pairs: &[PairManifolds], at: Vec3) -> bool {
    kept(pairs).any(|p| p.point == at)
}

/// Sets every point of `manifold` as having carried load last step.
fn carried(mut manifold: ContactManifold) -> ContactManifold {
    for point in &mut manifold.points {
        point.warm_start_normal_impulse = 1.0;
    }
    manifold
}

/// A flat seam of many hull pairs costs the solver only the corners of its footprint: the
/// interior, a point deeper by less than the overlap slop included, and the points along an edge
/// add no support. A mutant keeping any interior or edge point, or losing a corner a body could
/// tip about, breaks it.
#[test]
fn a_flat_many_hull_seam_keeps_only_its_footprint_corners() {
    let mut manifolds: Vec<ContactManifold> = (-2..=2)
        .flat_map(|x| {
            (-2..=2).map(move |z| flat_patch(Vec3::new(x as f32, 0.0, z as f32), 0.4, 0.0))
        })
        .collect();
    manifolds[11].points[1].penetration = 0.5 * step().overlap_slop;
    let mut pairs = pairs_of(manifolds);
    reduce(&mut pairs);
    assert_eq!(count(&pairs), 4);
    for corner in [(2.4, 2.4), (2.4, -2.4), (-2.4, -2.4), (-2.4, 2.4)] {
        assert!(
            kept_at(&pairs, Vec3::new(corner.0, 0.0, corner.1)),
            "{corner:?} was lost"
        );
    }
}

/// The footprint of a compound body is not four-sided in general. Five small coplanar patches
/// place a fifth corner beyond the diamond of the other four, supporting a centre of mass at
/// (1.4, 1.4) that the diamond does not reach (its x+z boundary is about 2.02). A mutant
/// capping points per normal below the hull's vertex count loses that corner and a supported
/// body tips.
#[test]
fn a_footprint_keeps_every_vertex_of_its_hull() {
    let at = |x: f32, z: f32| Vec3::new(x, 0.0, z);
    let mut pairs = pairs_of(
        [
            at(-2.0, 0.0),
            at(2.0, 0.0),
            at(0.0, 2.0),
            at(0.0, -2.0),
            at(1.5, 1.5),
        ]
        .map(|centre| flat_patch(centre, 0.01, 0.0)),
    );
    reduce(&mut pairs);
    let reach = kept(&pairs)
        .map(|p| p.point.x + p.point.z)
        .fold(f32::MIN, f32::max);
    assert!(
        reach > 2.8,
        "the far corner went, footprint reaches x+z {reach}"
    );
    for side in [(-1.0, 0.0), (1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
        let reach = kept(&pairs)
            .map(|p| p.point.x * side.0 + p.point.z * side.1)
            .fold(f32::MIN, f32::max);
        assert!(
            reach >= 2.0,
            "a cardinal patch went: {side:?} reaches {reach}"
        );
    }
}

/// Distinct support normals each keep their own contact: a body braced on a floor and two walls
/// must not lose one face to another. A mutant clustering everything under one normal breaks it.
#[test]
fn distinct_support_normals_each_survive() {
    let normals = [Vec3::Y, Vec3::X, Vec3::Z];
    let mut pairs = pairs_of(normals.iter().flat_map(|&normal| {
        (0..4).map(move |step| {
            patch(
                normal * 3.0 + normal.any_orthonormal_vector() * step as f32,
                normal,
                0.4,
                0.0,
            )
        })
    }));
    reduce(&mut pairs);
    for normal in normals {
        let count = pairs
            .iter()
            .flat_map(|pair| &pair.manifolds)
            .filter(|manifold| manifold.normal == normal)
            .map(|manifold| manifold.points.len())
            .sum::<usize>();
        assert!(count >= 3, "{normal:?} kept {count}");
    }
}

/// A group past the normal ceiling is left whole: the ceiling bounds work and may never cost a
/// face, because a shallow face can be the brace that holds a body up. A mutant dropping the
/// thirteenth normal's points, or reducing the first twelve, breaks it.
#[test]
fn past_the_normal_ceiling_a_group_stays_whole() {
    let spokes = (0..=MAX_NORMALS).map(|i| {
        let turn = i as f32 * std::f32::consts::TAU / (MAX_NORMALS + 1) as f32;
        Vec3::new(turn.cos(), 0.3, turn.sin()).normalize()
    });
    let mut pairs = pairs_of(spokes.flat_map(|normal| {
        (0..3).map(move |step| patch(normal * (2.0 + step as f32), normal, 0.3, 0.0))
    }));
    reduce(&mut pairs);
    assert_eq!(
        count(&pairs),
        (MAX_NORMALS + 1) * 3 * 4,
        "a point past the ceiling was removed"
    );
}

/// A collider pair ordered the other way round reports its normal flipped; the same face must
/// still cluster as one support and an opposite face must not. A mutant that ignores `reversed`
/// breaks one side of this.
#[test]
fn a_reversed_pair_clusters_on_its_oriented_normal() {
    let side = |id: u32, x: f32, reversed: bool, normal: Vec3| PairManifolds {
        colliders: (entity(id), entity(id + 1000)),
        reversed,
        manifolds: vec![{
            let mut manifold = flat_patch(Vec3::X * x, 0.3, 0.0);
            manifold.normal = normal;
            manifold
        }],
    };
    // One face, reported both ways round: its four outer corners and no more.
    let mut pairs = vec![
        side(1, 1.0, false, Vec3::Y),
        side(2, 2.0, true, -Vec3::Y),
        side(3, 3.0, false, Vec3::Y),
        side(4, 4.0, true, -Vec3::Y),
    ];
    reduce(&mut pairs);
    assert_eq!(count(&pairs), 4, "one face became two");
    // A face reported the opposite way is a second support: both keep their corners.
    let mut opposed = vec![
        side(1, 1.0, false, Vec3::Y),
        side(2, 3.0, false, Vec3::Y),
        side(3, 1.0, true, Vec3::Y),
        side(4, 3.0, true, Vec3::Y),
    ];
    reduce(&mut opposed);
    assert_eq!(
        count(&opposed),
        2 * 4,
        "an opposite support was merged into the first"
    );
}

/// One point closing past the rest speed makes the whole body pair a landing for the step, read
/// per chunk by the game's crumble law: every other point survives with it, as the presses of
/// that landing. A mutant keeping only the closing point, or the hull, breaks it.
#[test]
fn a_landing_point_leaves_the_whole_pair_whole() {
    let mut manifolds: Vec<ContactManifold> = (0..6)
        .map(|i| flat_patch(Vec3::X * i as f32, 0.4, 0.0))
        .collect();
    manifolds[3].points[0].normal_speed = -(REST * 2.0);
    let mut pairs = pairs_of(manifolds);
    reduce(&mut pairs);
    assert_eq!(count(&pairs), 6 * 4, "a landing pair lost a point");
}

/// A point that neither touches nor carried load bears nothing, so it must not widen the
/// footprint; a point that carried load last step bears however far it has opened. A mutant
/// treating every point as bearing, or every carried gap as idle, breaks one side.
#[test]
fn only_points_that_bear_shape_the_footprint() {
    let open = -2.0 * BEARING_GAP;
    let build = |gap: ContactManifold| {
        let mut pairs = pairs_of(
            [(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)]
                .map(|(x, z)| lone(Vec3::new(x, 0.0, z), 0.0))
                .into_iter()
                .chain([gap]),
        );
        reduce(&mut pairs);
        pairs
    };
    let outside = Vec3::new(2.0, 0.0, 0.0);
    assert!(
        !kept_at(&build(lone(outside, open)), outside),
        "a gap point widened the footprint"
    );
    assert!(
        kept_at(&build(carried(lone(outside, open))), outside),
        "a loaded point was dropped"
    );
    let touching = -0.5 * BEARING_GAP;
    assert!(
        kept_at(&build(lone(outside, touching)), outside),
        "a touching point did not bear"
    );
}

/// The tolerance is applied after the loaded preference, so a loaded vertex leaves only at half
/// the distance a cold one enters at. Without that a vertex near the limit flips each step and
/// loses its warm start. A mutant comparing the raw reach first drops the loaded one here.
#[test]
fn a_loaded_vertex_holds_inside_the_tolerance_a_cold_one_cannot_pass() {
    let build = |reach: f32, load: bool| {
        let vertex = lone(Vec3::new(0.0, 0.0, 5.0 + reach), 0.0);
        let mut pairs = pairs_of(
            [(-5.0, -5.0), (-5.0, 5.0), (5.0, -5.0), (5.0, 5.0)]
                .map(|(x, z)| lone(Vec3::new(x, 0.0, z), 0.0))
                .into_iter()
                .chain([if load { carried(vertex) } else { vertex }]),
        );
        reduce(&mut pairs);
        pairs
    };
    let vertex = |reach: f32| Vec3::new(0.0, 0.0, 5.0 + reach);
    let inside = 0.75 * HULL_TOLERANCE;
    assert!(
        !kept_at(&build(inside, false), vertex(inside)),
        "a cold near-collinear vertex stayed"
    );
    assert!(
        kept_at(&build(inside, true), vertex(inside)),
        "a loaded vertex left inside its band"
    );
    let beyond = 1.5 * HULL_TOLERANCE;
    assert!(
        kept_at(&build(beyond, false), vertex(beyond)),
        "a real corner was dropped"
    );
}

/// A selection is a function of the contacts, never of the order the solver happened to hand
/// them over: two collider pairs reporting one corner resolve by collider identity, so an active
/// pair order that changes does not trade the point and lose its warm start. A mutant taking
/// the slice order breaks it.
#[test]
fn a_selection_ignores_the_order_of_its_pairs() {
    let build = || {
        pairs_of(
            (0..5)
                .map(|x| flat_patch(Vec3::X * x as f32, 0.5, 0.0))
                .chain([flat_patch(Vec3::X * 4.0, 0.5, 0.0)]),
        )
    };
    let ids = |pairs: &[PairManifolds]| {
        let mut held: Vec<(Entity, Vec3)> = pairs
            .iter()
            .flat_map(|pair| {
                pair.manifolds
                    .iter()
                    .flat_map(move |m| m.points.iter().map(move |p| (pair.colliders.0, p.point)))
            })
            .collect();
        held.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(a.1.x.total_cmp(&b.1.x))
                .then(a.1.z.total_cmp(&b.1.z))
        });
        held
    };
    let mut forward = build();
    reduce(&mut forward);
    let mut backward = build();
    backward.reverse();
    reduce(&mut backward);
    assert_eq!(ids(&forward), ids(&backward));
}

/// Which manifold seeds a support normal comes from collider identity, not depth: normals at 0,
/// 19 and 38 degrees make two clusters when the 0-degree manifold seeds first and one when the
/// 19-degree does, so a seed that follows depth reselects whenever depths trade places. In two
/// clusters one end patch is a hull of its own and keeps its four corners; merged, each end lies
/// at the far end of one hull and keeps two. A mutant seeding by depth breaks it.
#[test]
fn cluster_seeds_do_not_follow_depth() {
    let tilted = |degrees: f32, x: f32, depth: f32| {
        let turn = degrees.to_radians();
        let mut manifold = flat_patch(Vec3::X * x, 0.5, depth);
        manifold.normal = Vec3::new(turn.sin(), turn.cos(), 0.0);
        manifold
    };
    for deep in 0..3 {
        let depth = |n: usize| if n == deep { 0.5 * BEARING_GAP } else { 0.0 };
        let mut pairs = pairs_of([
            tilted(0.0, -3.0, depth(0)),
            tilted(19.0, 0.0, depth(1)),
            tilted(38.0, 3.0, depth(2)),
        ]);
        reduce(&mut pairs);
        let end = |side: f32| kept(&pairs).filter(|p| p.point.x * side >= 2.5).count();
        assert!(
            end(-1.0) == 4 || end(1.0) == 4,
            "the clusters followed depth (deepest {deep}): ends kept {} and {}",
            end(-1.0),
            end(1.0)
        );
    }
}

/// The tolerance is measured to the kept segment, not its line: a vertex past a kept end lies off
/// the outline by more than its distance to the line. Loaded A seeds beside B, and P beyond A's
/// end lies 0.99 of the tolerance from line AB but 1.1 from the segment. A mutant measuring to the
/// line drops P.
#[test]
fn a_vertex_past_a_kept_end_is_measured_to_the_segment() {
    let off = Vec3::new(-0.49, 0.0, 0.99) * HULL_TOLERANCE;
    let mut pairs = pairs_of([
        carried(lone(Vec3::ZERO, 0.0)),
        lone(Vec3::X, 0.0),
        lone(off, 0.0),
    ]);
    reduce(&mut pairs);
    assert!(kept_at(&pairs, off), "a vertex off the outline was dropped");
}

/// A cold point whose opening its closing speed covers within the step bears, or the solver loses
/// a support it would meet this step; the same point holding still does not. Fixture: 2.2 mm open
/// and closing at 0.145 m/s, under the landing speed. A mutant ignoring the closing speed drops it.
#[test]
fn a_point_closing_within_the_step_bears() {
    let outside = Vec3::new(2.0, 0.0, 0.0);
    let build = |closing: f32| {
        let mut opening = lone(outside, -0.0022);
        opening.points[0].normal_speed = -closing;
        let mut pairs = pairs_of(corners(1.0).chain([opening]));
        reduce(&mut pairs);
        pairs
    };
    assert!(
        kept_at(&build(0.145), outside),
        "a closing point did not bear"
    );
    assert!(!kept_at(&build(0.0), outside), "a still, open point bore");
}

/// A point inside the outline that overlaps deeper than the kept ones by more than the engine's
/// slop leaves the whole group whole, since nothing kept would correct that overlap; one within
/// the slop is removed. A mutant without the bypass, or bypassing at any depth, breaks one side.
#[test]
fn an_interior_overlap_past_the_slop_keeps_the_group_whole() {
    let slop = step().overlap_slop;
    let build = |depth: f32| {
        let mut pairs = pairs_of(corners(1.0).chain([lone(Vec3::new(0.2, 0.0, 0.1), depth)]));
        reduce(&mut pairs);
        count(&pairs)
    };
    assert_eq!(
        build(0.1),
        5,
        "a 0.1 m interior overlap was left uncorrected"
    );
    assert_eq!(build(2.0 * slop), 5, "an overlap past the slop was removed");
    assert_eq!(
        build(0.5 * slop),
        4,
        "an overlap within the slop kept the group whole"
    );
}

/// Within one collider pair, manifolds seed in the order of their features, not the order the
/// narrow phase listed them: listing a 19-degree manifold before its pair's 0-degree one must not
/// merge the clusters (see [`cluster_seeds_do_not_follow_depth`]). A mutant seeding manifolds as
/// listed breaks it.
#[test]
fn manifolds_seed_in_feature_order() {
    let tilted = |degrees: f32, x: f32, feature: u32| {
        let turn = degrees.to_radians();
        let mut manifold = flat_patch(Vec3::X * x, 0.5, 0.0);
        manifold.normal = Vec3::new(turn.sin(), turn.cos(), 0.0);
        for point in &mut manifold.points {
            point.feature_id1 = PackedFeatureId::from(feature);
        }
        manifold
    };
    let mut pairs = vec![PairManifolds {
        colliders: (entity(1), entity(2)),
        reversed: false,
        manifolds: vec![
            tilted(19.0, 0.0, 2),
            tilted(38.0, 3.0, 3),
            tilted(0.0, -3.0, 1),
        ],
    }];
    reduce(&mut pairs);
    let end = |side: f32| kept(&pairs).filter(|p| p.point.x * side >= 2.5).count();
    assert!(
        end(-1.0) == 4 || end(1.0) == 4,
        "the listed order seeded the clusters: ends kept {} and {}",
        end(-1.0),
        end(1.0)
    );
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

/// A dynamic body of fixed mass whose centre of mass sits `extra` from its origin.
fn body_at(app: &mut App, at: Vec3, reducible: bool, extra: Vec3) -> Entity {
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
    body.id()
}

/// A body of a 4×4 grid of 1 m box hulls standing on the floor, its hulls' seams offset from the
/// floor's so every box meets several. `hooked` spawns its hulls with the contact hook, as the
/// game's belts and standing walls do.
fn slab_at(app: &mut App, at: Vec3, reducible: bool, extra: Vec3, hooked: bool) -> Entity {
    let root = body_at(app, at, reducible, extra);
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

/// Whether `at`, seen from above, lies over the convex outline of `points`: on the inner side of
/// every line through two points that has all the points on one side.
fn over_outline(points: &[Vec3], at: Vec3) -> bool {
    let flat = |p: Vec3| Vec2::new(p.x, p.z);
    let at = flat(at);
    points.len() >= 3
        && points.iter().all(|&a| {
            points.iter().all(|&b| {
                let side = |p: Vec2| (flat(b) - flat(a)).perp_dot(p - flat(a));
                a == b || !points.iter().all(|&p| side(flat(p)) >= 0.0) || side(at) >= 0.0
            })
        })
}

/// Steps `app` and checks, on every step `body` is awake, that its centre of mass (`extra` from
/// its origin) lies over the outline of the contact points the solver keeps for it. Statics owns
/// the criterion: a body whose centre of mass is over its support outline does not tip, so a
/// reduction that left it outside would tip a supported body. Sleeping pairs leave the active
/// set, so asleep steps are skipped.
fn stays_over_its_outline(app: &mut App, body: Entity, extra: Vec3, count: usize) {
    let mut checked = 0;
    for _ in 0..count {
        app.update();
        let points: Vec<Vec3> = body_pairs(app, body)
            .iter()
            .flat_map(|pair| &pair.manifolds)
            .flat_map(|manifold| &manifold.points)
            .map(|point| point.point)
            .collect();
        if points.is_empty() {
            continue;
        }
        let world = app.world();
        let com =
            world.get::<Position>(body).unwrap().0 + world.get::<Rotation>(body).unwrap().0 * extra;
        assert!(
            over_outline(&points, com),
            "the centre of mass {com} left the kept outline {points:?}"
        );
        checked += 1;
    }
    assert!(
        checked > 10,
        "fixture: the body was awake on {checked} steps"
    );
}

/// A body on many hull pairs of another body, reduced, stays supported with its centre of mass
/// well off-centre over a corner of its footprint, and its flat interface reduces to its four
/// corners. A selection keeping one-sided points leaves the centre of mass outside; a missing
/// reduction fails the count. The unreduced twin shows the fixture has many manifolds to reduce.
#[test]
fn a_body_on_reduced_contacts_stays_supported_with_a_bounded_count() {
    let extra = Vec3::new(1.2, 0.0, 1.2);
    let mut reduced = app();
    floor_of(&mut reduced, 14);
    let (_, body) = stack(&mut reduced, true, extra, false);
    stays_over_its_outline(&mut reduced, body, extra, 20);
    let (pairs, manifolds, points) = contact_counts(&body_pairs(&reduced, body));
    assert!(pairs > 4, "fixture: many hull pairs touch, got {pairs}");
    assert_eq!(points, 4, "{points} points over {pairs} pairs");
    assert!(manifolds < pairs, "reduction removed whole manifolds");

    let mut full = app();
    floor_of(&mut full, 14);
    let (_, unmarked) = stack(&mut full, false, extra, false);
    steps(&mut full, 20);
    let (_, full_manifolds, full_points) = contact_counts(&body_pairs(&full, unmarked));
    assert!(
        full_manifolds > manifolds && full_points > points,
        "fixture: the unreduced twin carries more ({full_manifolds} manifolds, {full_points} points against {manifolds}, {points})"
    );
}

/// A compound footprint need not be four-sided. Five thin legs stand on a marked dynamic base,
/// four at the cardinal points and one out at (1.5, 1.5), with the centre of mass at (1.4, 1.4):
/// only the fifth leg's corner of the footprint holds it. A selection that keeps the four cardinal
/// extremes leaves the centre of mass outside the kept outline.
#[test]
fn a_body_held_by_its_fifth_corner_stays_up() {
    let mut world = app();
    floor_of(&mut world, 14);
    wide_base(&mut world);
    let extra = Vec3::new(1.4, 0.0, 1.4);
    let body = body_at(&mut world, Vec3::new(0.3, 1.5, 0.3), true, extra);
    for (x, z) in [(-2.0, 0.0), (2.0, 0.0), (0.0, 2.0), (0.0, -2.0), (1.5, 1.5)] {
        world.world_mut().spawn((
            ChildOf(body),
            Collider::cuboid(0.1, 1.0, 0.1),
            Transform::from_xyz(x, 0.0, z),
        ));
    }
    stays_over_its_outline(&mut world, body, extra, 40);
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

/// Reduction applies only between two dynamic marked bodies: a static side (marked here, so a
/// mutant ignoring the body kind is caught), an unmarked dynamic body (a tank) and any pair
/// carrying a contact hook (a belt, a standing wall) keep every manifold, so their contacts
/// match an unreduced world. A mutant reducing by default or ignoring a side's marker breaks it.
#[test]
fn only_marked_dynamic_body_pairs_reduce() {
    let per_pair = |pairs: Vec<&ContactPair>| {
        assert!(pairs.len() > 4, "fixture: {} hull pairs", pairs.len());
        pairs
            .iter()
            .all(|pair| pair.manifolds.len() == 1 && pair.manifolds[0].points.len() == 4)
    };
    // A marked body on a marked static floor: static bodies do not take reduction.
    let mut on_floor = app();
    let floor = floor_of(&mut on_floor, 14);
    on_floor
        .world_mut()
        .entity_mut(floor)
        .insert(ReducibleContacts);
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
