//! Required local state for the narrow phase, size metrics and island waking. Unposed replicas
//! are a named state; disabled entities do not participate. Complete posed entities must retain
//! the components those consumers require, even when they currently have no contact pair.
//!
//! A body without attached colliders legitimately has no `RigidBodyColliders`. At a settled
//! schedule boundary Bevy's relationship hooks remove `ColliderOf` sources with that target;
//! the attachment observer queues links whose owner does not yet have RigidBody. They must be
//! complete when First admits their posed, enabled participants.

use avian3d::collision::collider::EnlargedAabb;
use avian3d::dynamics::solver::islands::BodyIslandNode;
use avian3d::prelude::*;
use bevy::ecs::query::QueryEntityError;
use bevy::prelude::*;

// Replication may temporarily remove either pose component; those entities are not admitted yet.
type Posed = (With<Position>, With<Rotation>);

/// Links awaiting their owner's construction. Complete spawns never enter this queue.
#[derive(Resource, Default)]
pub(super) struct PendingOwners(pub Vec<(Entity, Entity)>);

/// Refuses incomplete state in the first physics phase, before child placement.
pub fn plugin(app: &mut App) {
    app.init_resource::<PendingOwners>().add_systems(
        PhysicsSchedule,
        (refuse_unfinished_placement, refuse_incomplete_state)
            .chain()
            .in_set(PhysicsStepSystems::First)
            .before(
                avian3d::collision::collider::collider_transform::update_child_collider_position,
            )
            .before(super::child_poses::place_child_colliders),
    );
}

fn refuse_unfinished_placement(
    physics_placement: Option<Res<super::child_poses::PhysicsPlacementReady>>,
    frame_pending: Option<Res<super::child_poses::FramePlacementPending>>,
) {
    assert!(
        physics_placement.is_some() && frame_pending.is_none(),
        "physics placement cleanup must replace the engine pose writers before updates; call App::finish and App::cleanup before running physics"
    );
}

fn refuse_incomplete_state(
    colliders: Query<
        Entity,
        (
            With<ColliderMarker>,
            Posed,
            Without<ColliderDisabled>,
            Or<(Without<CollisionLayers>, Without<EnlargedAabb>)>,
        ),
    >,
    bodies: Query<
        Entity,
        (
            With<RigidBody>,
            Posed,
            Without<RigidBodyDisabled>,
            Or<(
                Without<LinearVelocity>,
                Without<AngularVelocity>,
                Without<ComputedCenterOfMass>,
                Without<BodySizeMetrics>,
                Without<ComputedMass>,
                Without<ComputedAngularInertia>,
            )>,
        ),
    >,
    islands: Query<
        Entity,
        (
            With<BodyIslandNode>,
            Posed,
            Without<RigidBodyDisabled>,
            Without<SleepTimer>,
        ),
    >,
    links: Query<Option<&ColliderOf>, (Posed, Without<ColliderDisabled>)>,
    owners: Query<(
        Has<RigidBody>,
        Has<Position>,
        Has<Rotation>,
        Has<RigidBodyDisabled>,
        Option<&RigidBodyColliders>,
    )>,
    mut pending: ResMut<PendingOwners>,
) {
    // With/Without/Or are archetypal in Bevy 0.20.1. Healthy archetypes never match, so is_empty
    // checks invalid archetype populations rather than walking every healthy entity each tick.
    assert!(
        colliders.is_empty(),
        "a posed enabled collider retains CollisionLayers and EnlargedAabb for the narrow phase; invalid entity: {:?}",
        colliders.iter().next()
    );
    assert!(
        bodies.is_empty(),
        "a posed enabled body retains LinearVelocity, AngularVelocity, ComputedCenterOfMass, BodySizeMetrics, ComputedMass and ComputedAngularInertia; invalid entity: {:?}",
        bodies.iter().next()
    );
    assert!(
        islands.is_empty(),
        "a posed enabled island body retains SleepTimer for island waking; invalid entity: {:?}",
        islands.iter().next()
    );
    // A relationship's target is a value, not an archetype property. Only links inserted before
    // their owner's RigidBody need this lookup; complete construction batches retire on admission.
    pending.0.retain(|&(collider, body)| {
        match links.get(collider) {
            Ok(Some(link)) if link.body == body => {}
            Ok(_) | Err(QueryEntityError::NotSpawned(_)) => return false,
            // Disabled or unposed links await admission, independently of the owner's state.
            Err(QueryEntityError::QueryDoesNotMatch(_, _)) => return true,
            Err(QueryEntityError::AliasedMutability(_)) => unreachable!("one read-only link fetch cannot alias"),
        }
        match owners.get(body) {
            Ok((true, true, true, false, members)) => {
                assert!(
                    members.is_some_and(|members| members.contains(&collider)),
                    "a posed enabled body with attached colliders retains RigidBodyColliders listing each ColliderOf source: body {body}, collider {collider}; complete the attachment after constructing its owner"
                );
                false
            }
            Ok((false, true, true, false, _)) => panic!(
                "ColliderOf targets a rigid body at physics admission: collider {collider} names posed enabled {body} without RigidBody"
            ),
            // A missing pose or RigidBodyDisabled is a named unadmitted state.
            Ok(_) => true,
            Err(QueryEntityError::NotSpawned(_)) => panic!("ColliderOf retains a live owner at physics admission: collider {collider} names despawned {body}"),
            // Default-disabling components keep the owner out of play until enabled.
            Err(QueryEntityError::QueryDoesNotMatch(_, _)) => true,
            Err(QueryEntityError::AliasedMutability(_)) => unreachable!("one read-only owner fetch cannot alias"),
        }
    });
}
