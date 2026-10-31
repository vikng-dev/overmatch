//! GUARD for the vendored moved-collider bounds prune (`vendor/avian3d-0.7.0-child-pose`,
//! `OVERMATCH_PATCH.md`; record: `upstream/avian-moved-aabb-prune.md`).
//!
//! The invariant: Avian's `update_moved_collider_aabbs` examines only the colliders whose pose or
//! shape changed since its previous run, and a static collider that is moved, rescaled, added or
//! removed still reaches the collider trees. Who relies on it: the dedicated server's physics step,
//! whose map holds thousands of static colliders that never move (every building element and tree
//! segment), and everything that reads the trees, the broad phase and ballistics' ray casts among
//! them. Stock Avian 0.7.0 examines every collider every step and fails the first test; its
//! examined count is `ColliderTreeDiagnostics::moved_aabb_candidates`, a field the patch adds.
//!
//! The poses, box dimensions and counts below are fixture data.

use avian3d::collider_tree::ColliderTreeDiagnostics;
use avian3d::prelude::*;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;

use super::avian_child_pose_churn::{physics_app, standing_compound};

fn static_box(app: &mut App, at: Vec3) -> Entity {
    app.world_mut()
        .spawn((
            RigidBody::Static,
            Transform::from_translation(at),
            Collider::cuboid(1.0, 1.0, 1.0),
        ))
        .id()
}

/// The collider a straight-down ray over `at` meets first, found through the collider trees.
fn hit_below(app: &mut App, at: Vec3) -> Option<Entity> {
    app.world_mut()
        .run_system_once(move |query: SpatialQuery| {
            query
                .cast_ray(
                    at + Vec3::Y * 10.0,
                    Dir3::NEG_Y,
                    20.0,
                    true,
                    &SpatialQueryFilter::default(),
                )
                .map(|hit| hit.entity)
        })
        .expect("the ray cast runs")
}

/// A step in which no collider moved examines none: standing static colliders, on a body or a
/// body's child, cost the pass their change ticks alone. Fails on stock Avian 0.7.0, which examines
/// every collider every step.
#[test]
fn the_moved_aabb_pass_examines_no_standing_static_collider() {
    let mut app = physics_app();
    for x in [0.0, 5.0, 10.0] {
        static_box(&mut app, Vec3::new(x, 0.0, 0.0));
    }
    standing_compound(&mut app);
    app.update();
    let examined = app
        .world()
        .resource::<ColliderTreeDiagnostics>()
        .moved_aabb_candidates;
    assert_eq!(
        examined, 0,
        "the moved-AABB pass examined {examined} colliders that stood: is \
         vendor/avian3d-0.7.0-child-pose still the avian3d in the graph?"
    );
}

/// The other half: a static collider moved, rescaled, added or removed is found, or no longer
/// found, where a ray cast through the trees looks for it after one step.
#[test]
fn a_static_collider_moved_rescaled_added_or_removed_reaches_the_trees() {
    let mut app = physics_app();
    let (stood, went) = (Vec3::ZERO, Vec3::new(20.0, 0.0, 0.0));
    let moved = static_box(&mut app, stood);
    for _ in 0..4 {
        app.update();
    }

    app.world_mut().get_mut::<Position>(moved).expect("pose").0 = went;
    app.update();
    assert_eq!(
        hit_below(&mut app, went),
        Some(moved),
        "a moved static collider is not found where it went"
    );
    assert_eq!(
        hit_below(&mut app, stood),
        None,
        "a moved static collider is still found where it stood"
    );

    // Half a unit box's extent is 0.5; scaled by 4 it reaches past 1.5.
    let edge = went + Vec3::X * 1.5;
    assert_eq!(
        hit_below(&mut app, edge),
        None,
        "the box already reaches its edge"
    );
    app.world_mut()
        .get_mut::<Transform>(moved)
        .expect("transform")
        .scale = Vec3::splat(4.0);
    app.update();
    assert_eq!(
        hit_below(&mut app, edge),
        Some(moved),
        "a rescaled static collider is not found out to its new extent"
    );

    let placed = Vec3::new(-20.0, 0.0, 0.0);
    let added = static_box(&mut app, placed);
    app.update();
    assert_eq!(
        hit_below(&mut app, placed),
        Some(added),
        "an added static collider is not found"
    );

    app.world_mut().entity_mut(added).remove::<Collider>();
    app.update();
    assert_eq!(
        hit_below(&mut app, placed),
        None,
        "a removed static collider is still found"
    );
}
