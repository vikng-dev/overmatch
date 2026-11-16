//! GUARD for the vendored body size metrics pass (`vendor/avian3d-0.8.0-overmatch`,
//! `OVERMATCH_PATCH.md`, "Body size metrics once per body").
//!
//! The invariant: a physics step recomputes a body's `BodySizeMetrics` once when any of its
//! colliders or its centre of mass changed, and not at all otherwise; an enabled body missing a
//! component the pass reads is refused, never left with stale metrics. Who relies on it: the
//! dedicated server's step in the tick a construction's piece takes its chunks (hundreds of
//! colliders changed on one body), and Avian's continuous collision, which reads the metrics. The
//! pass's count is `BodySizeMetricUpdates`, a resource the patch adds.
//!
//! The poses, sizes and counts below are fixture data.

use avian3d::dynamics::rigid_body::body_size_metrics::{BodySizeMetricUpdates, BodySizeMetrics};
use avian3d::prelude::*;
use bevy::ecs::entity_disabling::Disabled;
use bevy::prelude::*;

use super::avian_child_pose_churn::physics_app;

fn metrics(app: &App, body: Entity) -> BodySizeMetrics {
    *app.world()
        .get::<BodySizeMetrics>(body)
        .expect("a rigid body carries its size metrics")
}

fn updates(app: &App) -> u32 {
    app.world().resource::<BodySizeMetricUpdates>().0
}

/// Every collider of one body moved in one step: the body is recomputed once and its metrics see
/// the move; a step in which nothing changed recomputes none. Fails if the pass recomputes no
/// stale body (the reach stays) or recomputes on a quiet step. It does not separate the pass's two
/// sources: Avian's mass pass marks the centre of mass changed whenever a collider moves, so a
/// pass reading only that still sees these colliders (MEASURED: that mutant passes).
#[test]
fn a_body_whose_colliders_all_changed_is_recomputed_once() {
    let mut app = physics_app();
    let body = app
        .world_mut()
        .spawn((RigidBody::Static, Transform::default()))
        .id();
    let children: Vec<Entity> = (0..6)
        .map(|i| {
            app.world_mut()
                .spawn((
                    ChildOf(body),
                    Collider::sphere(0.5),
                    Transform::from_xyz(i as f32 - 2.5, 0.0, 0.0),
                ))
                .id()
        })
        .collect();
    let bystander = app
        .world_mut()
        .spawn((
            RigidBody::Static,
            Collider::sphere(0.5),
            Transform::default(),
        ))
        .id();
    for _ in 0..3 {
        app.update();
    }
    assert_eq!(updates(&app), 0, "a step with no change recomputed a body");
    let (before, stood) = (metrics(&app, body), metrics(&app, bystander));

    // Six spheres 1 m apart, centred on the body, reach 2.5 m + 0.5 m from its centre of mass.
    assert!((before.sweep_radius - 3.0).abs() < 1e-4, "{before:?}");

    for (i, &child) in children.iter().enumerate() {
        app.world_mut()
            .get_mut::<Transform>(child)
            .expect("the child keeps its transform")
            .translation
            .x = 10.0 * (i as f32 - 2.5);
    }
    app.update();
    assert_eq!(updates(&app), 1, "one body's colliders changed");
    // 10 m apart they reach 25 m + 0.5 m.
    let after = metrics(&app, body);
    assert!(
        (after.sweep_radius - 25.5).abs() < 1e-3,
        "the body's reach did not follow its colliders: {before:?} -> {after:?}"
    );
    assert_eq!(metrics(&app, bystander), stood, "an untouched body changed");
}

/// A centre of mass moved with no collider changed: the body is recomputed, its reach measured
/// from the new centre. Fails if the pass reads only the colliders' changes.
#[test]
fn a_moved_centre_of_mass_refreshes_the_bodys_reach() {
    let mut app = physics_app();
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            GravityScale(0.0),
            Collider::sphere(0.5),
            Transform::default(),
        ))
        .id();
    for _ in 0..3 {
        app.update();
    }
    // A 0.5 m sphere about its own centre reaches 0.5 m.
    assert!((metrics(&app, body).sweep_radius - 0.5).abs() < 1e-4);

    app.world_mut()
        .entity_mut(body)
        .insert((CenterOfMass(Vec3::X * 2.0), NoAutoCenterOfMass));
    for _ in 0..2 {
        app.update();
    }
    // From a centre 2 m off the sphere's, its far side is 2.5 m away.
    let reach = metrics(&app, body).sweep_radius;
    assert!(
        (reach - 2.5).abs() < 1e-4,
        "the reach was not measured from the moved centre of mass: {reach}"
    );
}

/// An enabled body that lost its centre of mass is refused when its collider changes, not left
/// with the metrics continuous collision reads gone stale. Fails if the pass passes by every body
/// it cannot read.
#[test]
#[should_panic(expected = "a posed enabled body retains LinearVelocity")]
fn a_body_that_lost_its_centre_of_mass_is_refused() {
    let mut app = physics_app();
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Static,
            Collider::sphere(0.5),
            Transform::default(),
        ))
        .id();
    for _ in 0..3 {
        app.update();
    }
    app.world_mut()
        .entity_mut(body)
        .remove::<ComputedCenterOfMass>()
        .insert(Collider::sphere(1.0));
    app.update();
}

/// A disabled body whose collider changed and a body with no collider whose centre of mass moved
/// have no metrics to refresh, and the pass passes both by. Fails if the pass refuses every body
/// it cannot read, or reads a moved centre on a body with no collider.
#[test]
fn a_disabled_or_colliderless_body_is_passed_by() {
    let mut app = physics_app();
    let disabled = app
        .world_mut()
        .spawn((RigidBody::Static, Transform::default()))
        .id();
    let collider = app
        .world_mut()
        .spawn((
            ChildOf(disabled),
            Collider::sphere(0.5),
            Transform::default(),
        ))
        .id();
    let bare = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            GravityScale(0.0),
            Mass(1.0),
            AngularInertia::new(Vec3::ONE),
            Transform::default(),
        ))
        .id();
    for _ in 0..3 {
        app.update();
    }
    app.world_mut().entity_mut(disabled).insert(Disabled);
    app.world_mut()
        .entity_mut(collider)
        .insert(Collider::sphere(1.0));
    app.world_mut()
        .entity_mut(bare)
        .insert((CenterOfMass(Vec3::X), NoAutoCenterOfMass));
    for _ in 0..2 {
        app.update();
        assert_eq!(
            updates(&app),
            0,
            "a body with no metrics to refresh was recomputed"
        );
    }
    assert_eq!(
        app.world()
            .get::<ComputedCenterOfMass>(bare)
            .expect("a rigid body carries its centre of mass")
            .0,
        Vec3::X,
        "the bare body's centre of mass must move for the pass to see it"
    );
}
