//! Child collider placement: one law for every boundary that needs a body's child colliders at
//! the body's pose (Avian before its narrow phase, the game's walk after physics, and the frame
//! boundary that replaces lightyear_avian's child pass).
//!
//! A child collider takes `body pose ∘ ColliderTransform`. Avian's own system assigned that pose to
//! every child of every body every step, so every standing static collider was marked changed and
//! its bounds recomputed each step; [`PhysicsPlacementPlugin`] replaces it after plugin setup.
//! [`ChildColliderPoses`] visits only children whose inputs changed (own offset,
//! own pose, attachment, or the body's pose, kind or collider list) and writes a component only
//! where its bits change, so an unchanged pose keeps its change tick.
//!
//! Re-enabling an entity changes no tick, and Bevy clears removal messages every `App::update`, so
//! a fixed-step boundary would miss a re-enable made between its runs. [`ReenabledPlugin`] turns
//! each disabling component's removal into a buffered [`Reenabled`] message that every boundary
//! reads through its own cursor.
//!
//! Who relies on it: the dedicated server, whose map holds every intact construction element as a
//! static body's child collider, and ballistics, which reads a moving tank's child colliders at the
//! pose this law leaves between steps.

use avian3d::prelude::*;
use bevy::ecs::component::ComponentId;
use bevy::ecs::entity_disabling::DefaultQueryFilters;
use bevy::ecs::schedule::ScheduleCleanupPolicy;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

type ChildPose = (
    &'static ColliderTransform,
    &'static mut Position,
    &'static mut Rotation,
    &'static ColliderOf,
);

type BodyColliders = (
    &'static Position,
    &'static Rotation,
    &'static RigidBodyColliders,
);

/// The child placement inputs shared by every boundary. An unchanged child is already at its
/// composed pose; a moved body's attached colliders are reached through `RigidBodyColliders`, so a
/// collider attached by `ColliderOf` alone, with no Transform hierarchy, follows its body too.
#[derive(SystemParam)]
pub struct ChildColliderPoses<'w, 's> {
    colliders: ParamSet<
        'w,
        's,
        (
            Query<
                'w,
                's,
                ChildPose,
                (
                    Without<RigidBody>,
                    Or<(
                        Changed<ColliderTransform>,
                        Changed<Position>,
                        Changed<Rotation>,
                        Changed<ColliderOf>,
                    )>,
                ),
            >,
            Query<'w, 's, ChildPose, Without<RigidBody>>,
        ),
    >,
    bodies: Query<'w, 's, (&'static Position, &'static Rotation), With<RigidBody>>,
    moved_bodies: Query<
        'w,
        's,
        BodyColliders,
        (
            With<RigidBody>,
            Or<(
                Changed<Position>,
                Changed<Rotation>,
                Changed<RigidBody>,
                Changed<RigidBodyColliders>,
            )>,
        ),
    >,
    body_colliders: Query<'w, 's, BodyColliders, With<RigidBody>>,
    reenabled: MessageReader<'w, 's, Reenabled>,
    query_filters: Res<'w, DefaultQueryFilters>,
    watched: Res<'w, WatchedDisabling>,
}

impl ChildColliderPoses<'_, '_> {
    /// Places every child whose inputs changed since this parameter's last run, and each child or
    /// body a [`Reenabled`] names, writing only changed bits.
    pub fn update(&mut self) {
        self.watched.assert_covers(&self.query_filters);
        for (local, mut position, mut rotation, owner) in &mut self.colliders.p0() {
            // A disabled body has no active pose to carry its child to; its re-enable announces it.
            if let Ok((at, turn)) = self.bodies.get(owner.body) {
                write(local, at, turn, &mut position, &mut rotation);
            }
        }
        let mut children = self.colliders.p1();
        for (at, turn, colliders) in &self.moved_bodies {
            for collider in colliders {
                // The body's own collider and disabled colliders are no active child colliders.
                if let Ok((local, mut position, mut rotation, _)) = children.get_mut(collider) {
                    write(local, at, turn, &mut position, &mut rotation);
                }
            }
        }
        for &Reenabled { entity } in self.reenabled.read() {
            // The entity is a child collider, a body, or neither. One still disabled by another
            // component, or despawning, matches no query; its last removal announces it again.
            if let Ok((local, mut position, mut rotation, owner)) = children.get_mut(entity)
                && let Ok((at, turn)) = self.bodies.get(owner.body)
            {
                write(local, at, turn, &mut position, &mut rotation);
            }
            if let Ok((at, turn, colliders)) = self.body_colliders.get(entity) {
                for collider in colliders {
                    if let Ok((local, mut position, mut rotation, _)) = children.get_mut(collider) {
                        write(local, at, turn, &mut position, &mut rotation);
                    }
                }
            }
        }
    }
}

/// The placement system. The game runs it in `PhysicsStepSystems::First`, where Avian placed
/// children before its narrow phase, and after the fixed loop, where lightyear_avian did.
pub fn place_child_colliders(mut poses: ChildColliderPoses) {
    poses.update();
}

/// Writes the pose the child takes from its body, each component only where its bits change: the
/// stored bits are those an unconditional write leaves, and only an unchanged pose's change ticks
/// differ. Bitwise, so a `-0.0` never stands in for the `0.0` (or one NaN payload for another) the
/// unconditional write would store.
fn write(
    local: &ColliderTransform,
    body_position: &Position,
    body_rotation: &Rotation,
    position: &mut Mut<Position>,
    rotation: &mut Mut<Rotation>,
) {
    let new_position = body_position.0 + body_rotation * local.translation;
    let new_rotation = (body_rotation.0 * local.rotation).normalize();
    if position.0.to_array().map(f32::to_bits) != new_position.to_array().map(f32::to_bits) {
        position.0 = new_position;
    }
    if rotation.0.to_array().map(f32::to_bits) != new_rotation.to_array().map(f32::to_bits) {
        rotation.0 = new_rotation;
    }
}

/// An entity that lost a disabling component of [`DefaultQueryFilters`]. A [`Message`] lives until
/// a fixed step has run, so every reader in or outside the fixed schedule sees each re-enable once,
/// through its own cursor. The entity may still carry another disabling component, or be
/// despawning: a reader admits it only where its default-filtered queries now match it.
#[derive(Message, Clone, Copy, Debug)]
pub struct Reenabled {
    pub entity: Entity,
}

/// Writes [`Reenabled`] from an observer on the removal of every disabling component. Components
/// registered by plugins built after this one are watched from [`Plugin::finish`]; one registered
/// later still is refused by every placement boundary ([`WatchedDisabling::assert_covers`]), since
/// its removals would go unannounced.
pub struct ReenabledPlugin;

impl Plugin for ReenabledPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Reenabled>()
            .init_resource::<WatchedDisabling>();
        watch_new_disabling(app.world_mut());
    }

    fn finish(&self, app: &mut App) {
        watch_new_disabling(app.world_mut());
    }
}

/// The disabling components whose removal writes [`Reenabled`].
#[derive(Resource, Default)]
pub struct WatchedDisabling(Vec<ComponentId>);

impl WatchedDisabling {
    /// Panics if a disabling component's removal would not be announced.
    pub fn assert_covers(&self, filters: &DefaultQueryFilters) {
        for id in filters.disabling_ids() {
            assert!(
                self.0.contains(&id),
                "disabling component {id:?} was registered after ReenabledPlugin::finish: its \
                 removals write no Reenabled, and a re-enabled child would keep a stale pose"
            );
        }
    }
}

fn watch_new_disabling(world: &mut World) {
    let watched = world.resource::<WatchedDisabling>();
    let new: Vec<ComponentId> = world
        .resource::<DefaultQueryFilters>()
        .disabling_ids()
        .filter(|id| !watched.0.contains(id))
        .collect();
    if new.is_empty() {
        return;
    }
    world
        .resource_mut::<WatchedDisabling>()
        .0
        .extend(new.iter().copied());
    world.spawn(Observer::new(announce_reenabled).with_components(new));
}

fn announce_reenabled(removed: On<Remove<()>>, mut reenabled: MessageWriter<Reenabled>) {
    reenabled.write(Reenabled {
        entity: removed.entity,
    });
}

/// Proof that cleanup replaced Avian's scheduled pose writer.
#[derive(Resource)]
pub(super) struct PhysicsPlacementReady;

/// Inserted by FramePlacementPlugin::build in the shared Lightyear installer; removed only after
/// its cleanup replaces the frame-boundary writer.
#[derive(Resource)]
pub(crate) struct FramePlacementPending;

/// Replaces Avian's child placement after all plugins have built and finished. Removal initializes
/// the schedule, so the replacement is added afterwards to avoid an ambiguous pair of writers.
pub struct PhysicsPlacementPlugin;

impl Plugin for PhysicsPlacementPlugin {
    fn build(&self, _app: &mut App) {}

    fn cleanup(&self, app: &mut App) {
        use avian3d::collision::collider::collider_transform::update_child_collider_position;
        let removed = app
            .remove_systems_in_set(
                PhysicsSchedule,
                update_child_collider_position,
                ScheduleCleanupPolicy::RemoveSystemsOnly,
            )
            .expect("Avian pose-writer removal requires an initializable PhysicsSchedule containing its system set");
        assert_eq!(
            removed, 1,
            "the game replaces exactly one Avian child placement system"
        );
        app.add_systems(
            PhysicsSchedule,
            place_child_colliders.in_set(PhysicsStepSystems::First),
        );
        app.insert_resource(PhysicsPlacementReady);
    }
}
