//! GUARD for the vendored compliant contact patch (`vendor/avian3d-0.7.0-child-pose`,
//! `OVERMATCH_PATCH.md`; record: `upstream/avian-compliant-contact.md`), in the game's tank driving
//! physics (game state only). The soft run (`track::soft_run`) hands its law to every point the
//! narrow phase found and shares it over the points in overlap only, so a point out of overlap must
//! bear nothing in the solver.

use avian3d::prelude::*;
use bevy::prelude::*;

/// Fixture: a law for a 1 kg box (N/m, N·s/m).
const LAW: ContactCompliance = ContactCompliance {
    stiffness: 1.0e4,
    damping: 1.0e2,
};

/// Fixture: the depths the manifold's points after its first are set to (m): at zero overlap, and
/// apart by less than the box closes in one step.
const OUT_OF_OVERLAP: [f32; 3] = [0.0, -1.0e-3, -2.0e-3];

/// Whether the hand-over caches warm starts on the points out of overlap.
#[derive(Resource)]
struct Cached(bool);

/// Whether a manifold has been handed the law.
#[derive(Resource, Default)]
struct Handed(bool);

fn hand_over(mut graph: ResMut<ContactGraph>, cached: Res<Cached>, mut handed: ResMut<Handed>) {
    for pair in graph.iter_active_mut() {
        for manifold in &mut pair.manifolds {
            manifold.compliance = Some(LAW);
            for (point, depth) in manifold.points.iter_mut().skip(1).zip(OUT_OF_OVERLAP) {
                point.penetration = depth;
                if cached.0 {
                    // Fixture: impulses large against the step's (N·s).
                    point.warm_start_normal_impulse = 5.0;
                    point.warm_start_tangent_impulse = Vec2::new(2.0, -2.0);
                }
            }
            handed.0 = true;
        }
    }
}

/// A compliant manifold's point out of overlap at the narrow phase bears nothing for the whole
/// step, warm start included, and the solver keeps it. Fixture: a box closing on a slab at 1 m/s,
/// its manifold handed a law with its first point in overlap and the rest at zero overlap or
/// apart. Fails if a point at zero overlap takes damping, or one apart takes the law once it closes
/// mid-step (an impulse is stored on it), or if a point's cached warm start reaches the solve: the
/// point unwinds it within the substep, but the point in overlap and the box move with it.
#[test]
fn a_compliant_point_out_of_overlap_bears_nothing() {
    let step = |cached: bool| {
        let mut app = super::avian_child_pose_churn::physics_app();
        app.insert_resource(Gravity(Vec3::ZERO))
            .insert_resource(Cached(cached))
            .init_resource::<Handed>()
            .add_systems(
                PhysicsSchedule,
                hand_over
                    .after(PhysicsStepSystems::NarrowPhase)
                    .before(PhysicsStepSystems::Solver),
            );
        app.world_mut()
            .spawn((RigidBody::Static, Collider::cuboid(4.0, 1.0, 4.0)));
        // Fixture: the box's lower face 0.01 m into the slab's upper face.
        let cube = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Collider::cuboid(1.0, 1.0, 1.0),
                Position(Vec3::Y * 0.99),
                LinearVelocity(Vec3::NEG_Y),
            ))
            .id();
        for _ in 0..4 {
            app.update();
            if app.world().resource::<Handed>().0 {
                break;
            }
        }
        let world = app.world();
        assert!(world.resource::<Handed>().0, "the box never met the slab");
        let points: Vec<ContactPoint> = world
            .resource::<ContactGraph>()
            .iter_active()
            .flat_map(|pair| &pair.manifolds)
            .flat_map(|manifold| manifold.points.iter().copied())
            .collect();
        let motion = (
            world.get::<LinearVelocity>(cube).expect("a dynamic body").0,
            world
                .get::<AngularVelocity>(cube)
                .expect("a dynamic body")
                .0,
        );
        (points, motion)
    };
    let (points, motion) = step(false);
    assert_eq!(
        points.len(),
        4,
        "fixture: a box face down on a slab meets it at four points"
    );
    assert!(
        points[0].normal_impulse > 0.0,
        "the point in overlap bore nothing"
    );
    for point in &points[1..] {
        assert_eq!(
            (
                point.normal_impulse,
                point.warm_start_normal_impulse,
                point.warm_start_tangent_impulse
            ),
            (0.0, 0.0, Vec2::ZERO),
            "a point {} m deep bore load",
            point.penetration
        );
    }
    assert_eq!(
        step(true),
        (points, motion),
        "a warm start cached on a point out of overlap moved the step"
    );
}
