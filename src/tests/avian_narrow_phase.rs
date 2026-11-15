//! GUARD for the avian3d fork's narrow-phase rows (`vendor/avian3d-0.8.0-overmatch`,
//! `OVERMATCH_PATCH.md`): which pairs `update_contacts` may skip or read as bodiless ("Unreadable
//! pair state"). Game state only: the server's contacts between bodies, not damage.

use avian3d::prelude::*;
use bevy::prelude::*;

use super::avian_child_pose_churn::physics_app;

/// Fixture: separated spheres with overlapping AABBs, at a scale whose gap exceeds the configured
/// contact tolerance. Sensors keep contact tests independent of solver response. Returns the app,
/// the static sphere, the dynamic sphere and their radius (m).
fn stationary_pair() -> (App, Entity, Entity, f32) {
    let mut app = physics_app();
    app.insert_resource(Gravity::ZERO);
    let tolerance = app
        .world()
        .resource::<NarrowPhaseConfig>()
        .contact_tolerance
        * app.world().resource::<PhysicsLengthUnit>().0;
    // DERIVED: sqrt(2) * 1.5r - 2r > tolerance for r = 1 + 10t.
    let radius = 1.0 + 10.0 * tolerance;
    let fixed = app
        .world_mut()
        .spawn((
            RigidBody::Static,
            Transform::IDENTITY,
            Collider::sphere(radius),
        ))
        .id();
    let moving = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Sensor,
            Transform::from_xyz(1.5 * radius, 1.5 * radius, 0.0),
            Collider::sphere(radius),
        ))
        .id();
    for _ in 0..4 {
        app.update();
    }
    (app, fixed, moving, radius)
}

/// Contact users need a pair whose collider lost state the narrow phase reads refused, not left
/// with its last update. A mutant that skips every pair it cannot read keeps it silently.
#[test]
#[should_panic(expected = "a pair's posed colliders keep the state the narrow phase reads")]
fn a_pair_whose_collider_lost_its_layers_is_refused() {
    let (mut app, fixed, _, _) = stationary_pair();
    app.world_mut()
        .entity_mut(fixed)
        .remove::<CollisionLayers>();
    app.update();
}

/// The client's replication may lift a body's pose for a frame (`crate::tests::avian_body_query_gap`),
/// and the narrow phase skips its pairs for that frame. A mutant that refuses every pair it cannot
/// read stops the client there.
#[test]
fn a_pair_whose_collider_lost_its_pose_is_skipped() {
    let (mut app, fixed, moving, _) = stationary_pair();
    let pose = *app.world().get::<Position>(fixed).unwrap();
    app.world_mut().entity_mut(fixed).remove::<Position>();
    app.update();
    app.world_mut().entity_mut(fixed).insert(pose);
    app.update();
    assert!(
        app.world()
            .resource::<ContactGraph>()
            .get(fixed, moving)
            .is_some()
    );
}

/// Contact users need a pair whose body lost state the narrow phase reads refused, not read as
/// bodiless. A mutant that reads every body it cannot fetch as absent keeps it silently.
#[test]
#[should_panic(expected = "a collider's body keeps the state the narrow phase reads")]
fn a_pair_whose_body_lost_its_velocity_is_refused() {
    let (mut app, _, moving, _) = stationary_pair();
    app.world_mut()
        .entity_mut(moving)
        .remove::<LinearVelocity>();
    app.update();
}

/// A disabled body's pairs generate no constraints, and the narrow phase reads the body as absent
/// (as it does a client body whose pose replication lifted: `crate::tests::avian_body_query_gap`).
/// A mutant that refuses every body it cannot fetch stops there.
#[test]
fn a_pair_whose_body_is_disabled_reads_it_as_absent() {
    let (mut app, fixed, _, radius) = stationary_pair();
    let disabled = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            RigidBodyDisabled,
            Transform::from_xyz(-1.5 * radius, 0.0, 0.0),
            Collider::sphere(radius),
        ))
        .id();
    app.update();
    let (_, pair) = app
        .world()
        .resource::<ContactGraph>()
        .get(fixed, disabled)
        .expect("the broad phase pairs a disabled body's collider");
    assert!(pair.is_touching() && !pair.generates_constraints());
}
