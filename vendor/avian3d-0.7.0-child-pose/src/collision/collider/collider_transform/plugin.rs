use crate::{
    ancestor_marker::{AncestorMarker, AncestorMarkerPlugin},
    physics_transform::PhysicsTransformSystems,
    prelude::*,
};
use bevy::{
    ecs::{intern::Interned, schedule::ScheduleLabel},
    prelude::*,
};

/// A plugin for propagating and updating transforms for colliders.
///
/// - Propagates [`Transform`] to [`GlobalTransform`] and [`ColliderTransform`].
/// - Updates [`Position`] and [`Rotation`] for colliders.
///
/// This plugin requires that colliders have the [`ColliderMarker`] component,
/// which is added automatically for colliders if the [`ColliderBackendPlugin`] is enabled.
pub struct ColliderTransformPlugin {
    schedule: Interned<dyn ScheduleLabel>,
}

impl ColliderTransformPlugin {
    /// Creates a [`ColliderTransformPlugin`] with the schedule that is used for running the [`PhysicsSchedule`].
    ///
    /// The default schedule is `FixedPostUpdate`.
    pub fn new(schedule: impl ScheduleLabel) -> Self {
        Self {
            schedule: schedule.intern(),
        }
    }
}

impl Default for ColliderTransformPlugin {
    fn default() -> Self {
        Self {
            schedule: FixedPostUpdate.intern(),
        }
    }
}

impl Plugin for ColliderTransformPlugin {
    fn build(&self, app: &mut App) {
        // Mark ancestors of colliders with `AncestorMarker<ColliderMarker>`.
        // This is used to speed up `ColliderTransform` propagation by skipping
        // trees that have no colliders.
        app.add_plugins(AncestorMarkerPlugin::<ColliderMarker>::default());

        // OVERMATCH PATCH: every child placement boundary reads re-enabled entities.
        if !app.is_plugin_added::<ReenabledPlugin>() {
            app.add_plugins(ReenabledPlugin);
        }

        // Propagate `ColliderTransform`s if there are new colliders.
        // Only traverses trees with `AncestorMarker<ColliderMarker>`.
        app.add_systems(
            self.schedule,
            propagate_collider_transforms.in_set(PhysicsTransformSystems::Propagate),
        );

        let physics_schedule = app
            .get_schedule_mut(PhysicsSchedule)
            .expect("add PhysicsSchedule first");

        // Update child collider positions before narrow phase collision detection.
        // Only traverses trees with `AncestorMarker<ColliderMarker>`.
        physics_schedule
            .add_systems(update_child_collider_position.in_set(PhysicsStepSystems::First));
    }
}

// OVERMATCH PATCH: one placement law serves the physics, post-physics and replication
// boundaries. Change filters still scan ticks, but only changed inputs compose child poses.
type ChildPose = (
    &'static ColliderTransform,
    &'static mut Position,
    &'static mut Rotation,
    &'static ColliderOf,
);

/// The child placement inputs shared by physics and external post-step consumers. An unchanged
/// child is already at its composed pose; a body move visits its attached collider list directly.
#[derive(bevy::ecs::system::SystemParam)]
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
        (
            &'static Position,
            &'static Rotation,
            &'static RigidBodyColliders,
        ),
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
    body_colliders: Query<
        'w,
        's,
        (
            &'static Position,
            &'static Rotation,
            &'static RigidBodyColliders,
        ),
        With<RigidBody>,
    >,
    reenabled: MessageReader<'w, 's, Reenabled>,
    query_filters: Res<'w, bevy::ecs::entity_disabling::DefaultQueryFilters>,
    watched: Res<'w, WatchedDisabling>,
}

impl ChildColliderPoses<'_, '_> {
    /// Place changed child poses, writing only changed bits. A re-enabled entity joins the
    /// default-filtered queries without a pose write, so each [`Reenabled`] places that child, or
    /// that body's attached colliders, once.
    pub fn update(&mut self) {
        self.watched.assert_covers(&self.query_filters);
        for (local, mut position, mut rotation, owner) in &mut self.colliders.p0() {
            // A disabled parent has no active physics pose to propagate.
            let Ok((at, turn)) = self.bodies.get(owner.body) else {
                continue;
            };
            child_pose::write(local, at, turn, &mut position, &mut rotation);
        }
        let mut children = self.colliders.p1();
        for (at, turn, colliders) in &self.moved_bodies {
            for collider in colliders {
                // The body's own collider and disabled colliders are not active child colliders.
                if let Ok((local, mut position, mut rotation, _)) = children.get_mut(collider) {
                    child_pose::write(local, at, turn, &mut position, &mut rotation);
                }
            }
        }
        for &Reenabled { entity } in self.reenabled.read() {
            // The entity is a child collider, a body, or neither. One still disabled by another
            // component, or despawning, matches no query; its last removal announces it again.
            if let Ok((local, mut position, mut rotation, owner)) = children.get_mut(entity)
                && let Ok((at, turn)) = self.bodies.get(owner.body)
            {
                child_pose::write(local, at, turn, &mut position, &mut rotation);
            }
            if let Ok((at, turn, colliders)) = self.body_colliders.get(entity) {
                for collider in colliders {
                    if let Ok((local, mut position, mut rotation, _)) = children.get_mut(collider) {
                        child_pose::write(local, at, turn, &mut position, &mut rotation);
                    }
                }
            }
        }
    }
}

/// OVERMATCH PATCH: an entity that lost a disabling component of [`DefaultQueryFilters`].
///
/// Re-enabling moves no change tick, and Bevy clears its removal messages every `App::update`
/// (`World::clear_trackers`), so a boundary in the fixed schedule, which runs every few frames,
/// misses a removal made in the frames between its runs. A [`Message`] lives until a fixed step has
/// run (`signal_message_update_system` in `FixedPostUpdate`), so every reader in or outside the
/// fixed schedule sees each re-enable once, through its own cursor. The entity may still carry
/// another disabling component, or be despawning: a reader admits it only where its
/// default-filtered queries now match it.
///
/// [`DefaultQueryFilters`]: bevy::ecs::entity_disabling::DefaultQueryFilters
#[derive(Message, Clone, Copy, Debug)]
pub struct Reenabled {
    /// The entity a disabling component left.
    pub entity: Entity,
}

/// OVERMATCH PATCH: writes [`Reenabled`] from an observer on the removal of every disabling
/// component. Components registered by plugins built after this one are watched from
/// [`Plugin::finish`]; one registered later still is refused by every placement boundary
/// ([`WatchedDisabling::assert_covers`]), since its removals would go unannounced.
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
pub struct WatchedDisabling(Vec<bevy::ecs::component::ComponentId>);

impl WatchedDisabling {
    /// Panics if a disabling component's removal would not be announced.
    pub fn assert_covers(&self, filters: &bevy::ecs::entity_disabling::DefaultQueryFilters) {
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
    let new: Vec<_> = world
        .resource::<bevy::ecs::entity_disabling::DefaultQueryFilters>()
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

fn announce_reenabled(removed: On<Remove>, mut reenabled: MessageWriter<Reenabled>) {
    reenabled.write(Reenabled {
        entity: removed.entity,
    });
}

/// Update child collider poses before narrow phase. Post-step consumers use the same parameter
/// to refresh the completed physics pose at their own boundary.
pub fn update_child_collider_position(mut poses: ChildColliderPoses) {
    poses.update();
}

// OVERMATCH PATCH: the child pose and its bitwise write, shared by every visiting path of
// `update_child_collider_position`.
mod child_pose {
    use super::*;

    /// Writes the pose the child takes from its body, each component only where its bits change.
    /// The stored bits are those an unconditional write leaves; only the change ticks of an
    /// unchanged pose differ, and Avian's `update_moved_collider_aabbs` skips a collider whose
    /// ticks did not move.
    pub(super) fn write(
        collider_transform: &ColliderTransform,
        rb_pos: &Position,
        rb_rot: &Rotation,
        position: &mut Mut<Position>,
        rotation: &mut Mut<Rotation>,
    ) {
        let new_position = rb_pos.0 + rb_rot * collider_transform.translation;
        #[cfg(feature = "2d")]
        let new_rotation = *rb_rot * collider_transform.rotation;
        #[cfg(feature = "3d")]
        let new_rotation: Rotation = (rb_rot.0 * collider_transform.rotation.0)
            .normalize()
            .into();
        if !same_bits_position(position.0, new_position) {
            position.0 = new_position;
        }
        if !same_bits_rotation(**rotation, new_rotation) {
            **rotation = new_rotation;
        }
    }

    // Bitwise equality, so a `-0.0` never stands in for a `0.0` (or one NaN payload for another)
    // that the unconditional write would have stored.
    pub(super) fn same_bits_position(a: Vector, b: Vector) -> bool {
        a.to_array().map(Scalar::to_bits) == b.to_array().map(Scalar::to_bits)
    }

    #[cfg(feature = "2d")]
    pub(super) fn same_bits_rotation(a: Rotation, b: Rotation) -> bool {
        [a.cos, a.sin].map(Scalar::to_bits) == [b.cos, b.sin].map(Scalar::to_bits)
    }

    #[cfg(feature = "3d")]
    pub(super) fn same_bits_rotation(a: Rotation, b: Rotation) -> bool {
        a.0.to_array().map(Scalar::to_bits) == b.0.to_array().map(Scalar::to_bits)
    }
}

// `ColliderTransform` propagation should only be continued if the child
// is a collider or is a `AncestorMarker<ColliderMarker>`.
type ShouldPropagate = Or<(With<AncestorMarker<ColliderMarker>>, With<ColliderMarker>)>;

/// Updates [`ColliderTransform`]s based on entity hierarchies. Each transform is computed by recursively
/// traversing the children of each rigid body and adding their transforms together to form
/// the total transform relative to the body.
///
/// This is largely a clone of `propagate_transforms` in `bevy_transform`.
#[allow(clippy::type_complexity)]
pub(crate) fn propagate_collider_transforms(
    mut root_query: Query<
        (Entity, Ref<Transform>, &Children),
        (Without<ChildOf>, With<AncestorMarker<ColliderMarker>>),
    >,
    collider_query: Query<
        (
            Ref<Transform>,
            Option<&mut ColliderTransform>,
            Option<&Children>,
        ),
        (With<ChildOf>, ShouldPropagate),
    >,
    parent_query: Query<(Entity, Ref<Transform>, Has<RigidBody>, Ref<ChildOf>), ShouldPropagate>,
) {
    root_query.par_iter_mut().for_each(
        |(entity, transform, children)| {
            for (child, child_transform, is_child_rb, child_of) in parent_query.iter_many(children) {
                assert_eq!(
                    child_of.parent(), entity,
                    "Malformed hierarchy. This probably means that your hierarchy has been improperly maintained, or contains a cycle"
                );
                let changed = transform.is_changed() || child_of.is_changed();
                let parent_transform = ColliderTransform::from(*transform);
                let child_transform = ColliderTransform::from(*child_transform);
                let scale = parent_transform.scale * child_transform.scale;

                // SAFETY:
                // - `child` must have consistent parentage, or the above assertion would panic.
                // Since `child` is parented to a root entity, the entire hierarchy leading to it is consistent.
                // - We may operate as if all descendants are consistent, since `propagate_collider_transform_recursive` will panic before
                //   continuing to propagate if it encounters an entity with inconsistent parentage.
                // - Since each root entity is unique and the hierarchy is consistent and forest-like,
                //   other root entities' `propagate_collider_transform_recursive` calls will not conflict with this one.
                // - Since this is the only place where `collider_query` gets used, there will be no conflicting fetches elsewhere.
                unsafe {
                    propagate_collider_transforms_recursive(
                        if is_child_rb {
                            ColliderTransform {
                                scale,
                                ..default()
                            }
                        } else {
                            ColliderTransform {
                                translation: parent_transform.scale * child_transform.translation,
                                rotation: child_transform.rotation,
                                scale,
                            }
                        },
                        &collider_query,
                        &parent_query,
                        child,
                        changed,
                    );
                }
            }
        },
    );
}

/// Recursively computes the [`ColliderTransform`] for `entity` and all of its descendants
/// by propagating transforms.
///
/// This is largely a clone of `propagate_recursive` in `bevy_transform`.
///
/// # Panics
///
/// If `entity`'s descendants have a malformed hierarchy, this function will panic occur before propagating
/// the transforms of any malformed entities and their descendants.
///
/// # Safety
///
/// - While this function is running, `collider_query` must not have any fetches for `entity`,
///   nor any of its descendants.
/// - The caller must ensure that the hierarchy leading to `entity`
///   is well-formed and must remain as a tree or a forest. Each entity must have at most one parent.
#[allow(clippy::type_complexity)]
unsafe fn propagate_collider_transforms_recursive(
    transform: ColliderTransform,
    collider_query: &Query<
        (
            Ref<Transform>,
            Option<&mut ColliderTransform>,
            Option<&Children>,
        ),
        (With<ChildOf>, ShouldPropagate),
    >,
    parent_query: &Query<(Entity, Ref<Transform>, Has<RigidBody>, Ref<ChildOf>), ShouldPropagate>,
    entity: Entity,
    mut changed: bool,
) {
    let children = {
        // SAFETY: This call cannot create aliased mutable references.
        //   - The top level iteration parallelizes on the roots of the hierarchy.
        //   - The caller ensures that each child has one and only one unique parent throughout the entire
        //     hierarchy.
        //
        // For example, consider the following malformed hierarchy:
        //
        //     A
        //   /   \
        //  B     C
        //   \   /
        //     D
        //
        // D has two parents, B and C. If the propagation passes through C, but the ChildOf component on D points to B,
        // the above check will panic as the origin parent does match the recorded parent.
        //
        // Also consider the following case, where A and B are roots:
        //
        //  A       B
        //   \     /
        //    C   D
        //     \ /
        //      E
        //
        // Even if these A and B start two separate tasks running in parallel, one of them will panic before attempting
        // to mutably access E.
        let Ok((transform_ref, collider_transform, children)) =
            (unsafe { collider_query.get_unchecked(entity) })
        else {
            return;
        };

        changed |=
            transform_ref.is_changed() || collider_transform.as_ref().is_some_and(|t| t.is_added());
        if changed
            && let Some(mut collider_transform) = collider_transform
            && *collider_transform != transform
        {
            *collider_transform = transform;
        }

        children
    };

    let Some(children) = children else { return };
    for (child, child_transform, is_rb, child_of) in parent_query.iter_many(children) {
        assert_eq!(
            child_of.parent(),
            entity,
            "Malformed hierarchy. This probably means that your hierarchy has been improperly maintained, or contains a cycle"
        );

        let child_transform = ColliderTransform::from(*child_transform);
        let scale = transform.scale * child_transform.scale;

        // SAFETY: The caller guarantees that `collider_query` will not be fetched
        // for any descendants of `entity`, so it is safe to call `propagate_collider_transforms_recursive` for each child.
        //
        // The above assertion ensures that each child has one and only one unique parent throughout the
        // entire hierarchy.
        unsafe {
            propagate_collider_transforms_recursive(
                if is_rb {
                    ColliderTransform { scale, ..default() }
                } else {
                    ColliderTransform {
                        translation: transform.transform_point(child_transform.translation),
                        #[cfg(feature = "2d")]
                        rotation: transform.rotation * child_transform.rotation,
                        #[cfg(feature = "3d")]
                        rotation: Rotation(transform.rotation.0 * child_transform.rotation.0),
                        scale,
                    }
                },
                collider_query,
                parent_query,
                child,
                changed || child_of.is_changed(),
            );
        }
    }
}

// TODO: Add thorough tests for propagation. It's pretty error-prone and changes are risky.
