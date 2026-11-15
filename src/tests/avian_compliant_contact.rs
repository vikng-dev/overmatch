//! GUARD for the compliant contact hook (`vendor/avian3d-0.8.0-overmatch`, `OVERMATCH_PATCH.md`,
//! "Compliant contact"; record: `upstream/avian-compliant-contact.md`) and the game's law behind
//! it, `crate::avian_ext::compliance`, in the game's tank driving physics (game state only). The
//! soft run (`track::soft_run`) hands its law to every point the narrow phase found and places each
//! spring's free length with `rest`, so a point whose spring is free must bear nothing in the
//! solver, and a loaded body must rest where its springs carry it.

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::avian_ext::compliance::{ContactCompliance, Laws};

/// Fixture: a law for a 1 kg box (N/m, N·s/m), its spring free through the first 5 mm of overlap.
const LAW: ContactCompliance = ContactCompliance {
    stiffness: 1.0e4,
    damping: 1.0e2,
    rest: -5.0e-3,
};

/// Fixture: the spring compressions (overlap + rest, m) the manifold's points after its first are
/// set to: at zero, and free by less than the box closes in one step. Each is in overlap.
const FREE: [f32; 3] = [0.0, -1.0e-3, -2.0e-3];

/// Whether the hand-over caches warm starts on the free points.
#[derive(Resource)]
struct Cached(bool);

/// Whether a manifold has been handed the law.
#[derive(Resource, Default)]
struct Handed(bool);

fn hand_over(
    mut graph: ResMut<ContactGraph>,
    mut laws: ResMut<Laws>,
    cached: Res<Cached>,
    mut handed: ResMut<Handed>,
) {
    for pair in graph.iter_active_mut() {
        for (index, manifold) in pair.manifolds.iter_mut().enumerate() {
            laws.set(pair.contact_id, index, LAW);
            for (point, compression) in manifold.points.iter_mut().skip(1).zip(FREE) {
                point.penetration = compression - LAW.rest;
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

/// A compliant manifold's point whose spring is free at the narrow phase bears nothing for the whole
/// step, warm start included, and the solver keeps it. Fixture: a box closing on a slab at 1 m/s,
/// its manifold handed a law with its first point compressed and the rest in overlap, at zero
/// compression or free. Fails if the inert rule reads overlap in place of the spring (a point at
/// zero compression takes damping), or a free point takes the law once it closes mid-step (an
/// impulse is stored on it), or if a point's cached warm start reaches the solve: the point
/// unwinds it within the substep, but the compressed point and the box move with it.
#[test]
fn a_compliant_point_whose_spring_is_free_bears_nothing() {
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
        "the compressed point bore nothing"
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
        "a warm start cached on a free point moved the step"
    );
}

/// The law a manifold is handed each step.
#[derive(Resource)]
struct Law(ContactCompliance);

fn hand_law(graph: Res<ContactGraph>, mut laws: ResMut<Laws>, law: Res<Law>) {
    for pair in graph.iter_active() {
        for index in 0..pair.manifolds.len() {
            laws.set(pair.contact_id, index, law.0);
        }
    }
}

/// A body rests where its compliant contact's springs carry its weight: its bearing points'
/// springs, `k·(overlap + rest)` each, sum to its weight `W`, whichever side of the surface the
/// spring is free at. Fixture: a 1 kg box face down on a slab, its points handed one law about
/// critically damped. Fails if `rest` misses the separation the solver reads (the springs then
/// carry `W` plus `k·rest` per bearing point).
#[test]
fn a_compliant_contact_rests_where_its_spring_carries_the_load() {
    for rest in [-1.0e-3, 1.0e-3] {
        let law = ContactCompliance {
            stiffness: 1.0e3,
            damping: 4.0e1,
            rest,
        };
        let mut app = super::avian_child_pose_churn::physics_app();
        app.insert_resource(Law(law)).add_systems(
            PhysicsSchedule,
            hand_law
                .after(PhysicsStepSystems::NarrowPhase)
                .before(PhysicsStepSystems::Solver),
        );
        app.world_mut()
            .spawn((RigidBody::Static, Collider::cuboid(4.0, 1.0, 4.0)));
        // Fixture: the box's lower face on the slab's upper face, at y = 0.5.
        let cube = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Collider::cuboid(1.0, 1.0, 1.0),
                Position(Vec3::Y),
                SleepingDisabled,
            ))
            .id();
        for _ in 0..256 {
            app.update();
        }
        let world = app.world();
        let bearing: Vec<f32> = (world.resource::<ContactGraph>().iter_active())
            .flat_map(|pair| &pair.manifolds)
            .flat_map(|manifold| &manifold.points)
            .map(|point| point.penetration)
            .filter(|&overlap| law.bears(overlap))
            .collect();
        assert!(
            !bearing.is_empty(),
            "rest {rest}: the box rests on no spring"
        );
        let carried: f32 = (bearing.iter())
            .map(|overlap| law.stiffness * (overlap + rest))
            .sum();
        let weight = world
            .get::<ComputedMass>(cube)
            .expect("a dynamic body")
            .value()
            * world.resource::<Gravity>().0.length();
        // Resolution: 10 µm of common overlap per bearing point, the settled box's residual.
        let tolerance = bearing.len() as f32 * law.stiffness * 1.0e-5;
        assert!(
            (carried - weight).abs() < tolerance,
            "rest {rest}: the box's springs carry {carried} N of its {weight} N"
        );
    }
}
