//! The game's damage model: collider poses at successive walk boundaries.
//!
//! Avian's `PhysicsInterpolationPlugin` uses Bevy's transform easing states, which hold render
//! transforms only for opted-in entities and are disabled by `net::physics::physics_plugins`.
//! Avian's solver has no public previous collider pose. This history therefore retains the
//! composed physics pose seen by the walk, independently of rendering or frame count.

use avian3d::prelude::{
    Collider, ColliderOf, ColliderTransform, Position, Rotation, SimpleCollider,
};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Pose {
    pub position: Vec3,
    pub rotation: Quat,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Poses {
    pub from: Pose,
    pub to: Pose,
}

/// The game's damage model: membership is frozen at a walk boundary. A new collider has no
/// earlier trajectory and is born with identical endpoints; a removed collider leaves the table.
#[derive(Resource, Default)]
pub(super) struct ColliderMotion {
    pub colliders: HashMap<Entity, Poses>,
    /// DERIVED conservative displacement of any collider point, used to grow a tree query.
    pub largest_motion: f32,
}

pub(super) fn snapshot(
    mut history: ResMut<ColliderMotion>,
    colliders: Query<(
        Entity,
        &Position,
        &Rotation,
        &Collider,
        Option<&ColliderOf>,
        Option<&ColliderTransform>,
    )>,
    bodies: Query<(&Position, &Rotation)>,
) {
    let mut next = HashMap::with_capacity(colliders.iter().len());
    let mut largest_motion = 0.0f32;
    for (entity, position, rotation, collider, owner, local) in &colliders {
        // Avian updates child Position before its solver, rather than after it. Compose the
        // post-solver body with its current local transform, including servo and recoil motion.
        let to = if let Some(owner) = owner.filter(|owner| owner.body != entity) {
            let (body_position, body_rotation) = bodies
                .get(owner.body)
                .expect("game's damage model: a collider's body has a physics pose");
            let local = local.expect("ColliderOf requires ColliderTransform");
            Pose {
                position: body_position.0 + body_rotation.0 * local.translation,
                rotation: (body_rotation.0 * local.rotation.0).normalize(),
            }
        } else {
            Pose {
                position: position.0,
                rotation: rotation.0,
            }
        };
        // Absence means birth at this boundary, not an invented earlier pose.
        let from = history.colliders.get(&entity).map_or(to, |poses| poses.to);
        let poses = Poses { from, to };
        let bounds = collider.aabb(Vec3::ZERO, Quat::IDENTITY);
        let radius = bounds.min.abs().max(bounds.max.abs()).length();
        // DERIVED: translation plus the longest radius's rotational chord bounds every point
        // throughout shortest-arc slerp. Static endpoints contribute bit-exact zero.
        let turn = poses.from.rotation.angle_between(poses.to.rotation);
        let displacement =
            poses.from.position.distance(poses.to.position) + 2.0 * radius * (0.5 * turn).sin();
        largest_motion = largest_motion.max(displacement);
        next.insert(entity, poses);
    }
    history.colliders = next;
    history.largest_motion = largest_motion;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<ColliderMotion>()
            .add_systems(Update, snapshot);
        app
    }

    /// The game's damage model: the walk needs successive physics poses even when rendering
    /// never runs. Mutant: reading the child's stale Position loses body, servo and recoil motion.
    #[test]
    fn history_composes_body_and_child_motion_and_expires_removed_colliders() {
        let mut app = app();
        let body = app
            .world_mut()
            .spawn((Position(Vec3::ZERO), Rotation::IDENTITY))
            .id();
        let child = app
            .world_mut()
            .spawn((
                Position(Vec3::ZERO),
                Rotation::IDENTITY,
                Collider::cuboid(1.0, 1.0, 1.0),
                ColliderOf { body },
                ColliderTransform {
                    translation: Vec3::X,
                    ..default()
                },
            ))
            .id();
        app.update();
        let birth = app.world().resource::<ColliderMotion>().colliders[&child];
        assert_eq!(birth.from, birth.to);
        assert_eq!(birth.to.position, Vec3::X);
        let body_turn = Quat::from_rotation_y(0.5);
        app.world_mut()
            .entity_mut(body)
            .insert((Position(Vec3::Z), Rotation(body_turn)));
        let local_turn = Quat::from_rotation_x(0.25);
        app.world_mut().entity_mut(child).insert(ColliderTransform {
            translation: Vec3::X * 2.0,
            rotation: Rotation(local_turn),
            ..default()
        });
        app.update();
        let history = app.world().resource::<ColliderMotion>();
        let moved = history.colliders[&child];
        assert_eq!(moved.from, birth.to);
        assert_eq!(moved.to.position, Vec3::Z + body_turn * (Vec3::X * 2.0));
        assert_eq!(moved.to.rotation, (body_turn * local_turn).normalize());
        assert!(history.largest_motion >= moved.from.position.distance(moved.to.position));
        app.world_mut().despawn(child);
        app.update();
        assert!(
            app.world()
                .resource::<ColliderMotion>()
                .colliders
                .is_empty()
        );
    }

    /// The game's damage model: static geometry must add no broad-phase padding. The walk
    /// relies on exact identity motion to keep the static collector's arithmetic unchanged.
    #[test]
    fn static_endpoints_and_motion_are_bit_exact() {
        let mut app = app();
        let collider = app
            .world_mut()
            .spawn((
                Position(Vec3::new(100.0, 20.0, -40.0)),
                Rotation::IDENTITY,
                Collider::cuboid(2.0, 3.0, 4.0),
            ))
            .id();
        app.update();
        app.update();
        let history = app.world().resource::<ColliderMotion>();
        let poses = history.colliders[&collider];
        assert_eq!(poses.from, poses.to);
        assert_eq!(history.largest_motion.to_bits(), 0.0f32.to_bits());
    }
}
