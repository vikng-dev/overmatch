//! Physics configuration shared by the network client and server.

use avian3d::physics_transform::PhysicsTransformSystems;
use avian3d::prelude::{PhysicsInterpolationPlugin, PhysicsTransformPlugin};
use avian3d::schedule::PhysicsSystems;
use bevy::prelude::*;

/// Own the schedule edges removed with `PhysicsTransformPlugin`: collider propagation runs in
/// Avian's `PhysicsSystems::Prepare` set, BEFORE `TransformToPosition`, as that plugin chains them.
///
/// Everything Avian orders after `TransformToPosition` reads the `ColliderTransform`s propagation
/// writes: `update_collider_scale` (`collision/collider/backend.rs`) and the mass-property sets.
/// Unordered, the collider scale can be taken from a `ColliderTransform` that propagation then
/// overwrites, and stay stale for a tick. A body re-rooted mid-hierarchy (a launched turret) is
/// handed one by Avian's `ColliderOf` hook, decomposed from world matrices an ULP off a unit
/// scale, and the collector refuses armour at any scale but a bit-exact 1 (`ballistics::collect`).
pub(crate) fn plugin(app: &mut App) {
    app.add_plugins(FramePlacementPlugin);
    app.configure_sets(
        FixedPostUpdate,
        (
            PhysicsTransformSystems::Propagate,
            PhysicsTransformSystems::TransformToPosition,
        )
            .chain()
            .in_set(PhysicsSystems::Prepare),
    );
}

/// Shared offline/network policy and the game-owned Avian seams.
/// Recycling must stay off: reuse returns before the belt's band filter and wake hook.
pub(crate) fn extensions(app: &mut App) {
    app.insert_resource(avian3d::prelude::NarrowPhaseConfig {
        recycle_distance: 0.0,
        ..default()
    })
    .add_plugins(crate::avian_ext::plugin);
}

/// Plugin set required by `LightyearAvianPlugin`: transform sync and interpolation belong to
/// Lightyear, so Avian's own transform and interpolation plugins stay disabled.
pub fn physics_plugins() -> bevy::app::PluginGroupBuilder {
    crate::offline_physics()
        .disable::<PhysicsTransformPlugin>()
        .disable::<PhysicsInterpolationPlugin>()
}

/// Mounts the network physics integration with the game's spawn-transform policy.
pub(crate) fn lightyear_plugin(app: &mut App) {
    use lightyear::avian3d::plugin::{AvianReplicationMode, LightyearAvianPlugin};
    app.add_plugins(LightyearAvianPlugin {
        replication_mode: AvianReplicationMode::Position {
            sync_to_transform: false,
        },
        register_physics_components: false,
        ..default()
    });
    // Position mode installs no per-frame Transform import. This flag lets Avian's spawn hook
    // initialize Transform-only statics at their authored poses instead of at the origin.
    app.world_mut()
        .resource_mut::<avian3d::physics_transform::PhysicsTransformConfig>()
        .transform_to_position = true;
}

/// Replaces Lightyear's Position-mode placement after both networking and physics plugin setup.
pub struct FramePlacementPlugin;

impl Plugin for FramePlacementPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(crate::avian_ext::child_poses::FramePlacementPending);
    }

    fn cleanup(&self, app: &mut App) {
        use bevy::ecs::schedule::ScheduleCleanupPolicy;
        use lightyear::avian3d::plugin::LightyearAvianPlugin;
        let removed = app.remove_systems_in_set(
            RunFixedMainLoop,
            LightyearAvianPlugin::update_child_collider_position,
            ScheduleCleanupPolicy::RemoveSystemsOnly,
        ).expect("Lightyear pose-writer removal requires an initializable RunFixedMainLoop containing its system set");
        assert_eq!(
            removed, 1,
            "the game replaces exactly one Lightyear child placement system"
        );
        app.add_systems(
            RunFixedMainLoop,
            crate::avian_ext::child_poses::place_child_colliders
                .in_set(RunFixedMainLoopSystems::AfterFixedMainLoop),
        );
        app.world_mut()
            .remove_resource::<crate::avian_ext::child_poses::FramePlacementPending>();
    }
}

#[cfg(test)]
mod tests {
    use avian3d::physics_transform::PhysicsTransformSystems;
    use bevy::ecs::schedule::{NodeId, SystemSet};
    use bevy::prelude::*;

    /// A recycled pair bypasses the belt filter and its wake. Every shared physics root must
    /// regenerate candidates so the driving pass's current band reaches the solver.
    #[test]
    fn shared_physics_keeps_contact_recycling_off() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            super::physics_plugins(),
        ));
        assert_eq!(
            app.world()
                .resource::<avian3d::prelude::NarrowPhaseConfig>()
                .recycle_distance,
            0.0
        );
    }

    /// In the networked composition, collider propagation is ordered before `TransformToPosition`,
    /// which `update_collider_scale` and the mass-property sets follow. Without the edge their
    /// order is the executor's choice, MEASURED on the dedicated server as the scale taken first:
    /// a launched turret's armour then held the scale Avian's `ColliderOf` hook decomposed out of
    /// world matrices, an ULP off 1, for the tick after the launch, and the collector failed closed
    /// every round that met it (`ballistics::collect`).
    #[test]
    fn collider_propagation_precedes_transform_to_position() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            super::physics_plugins(),
            super::plugin,
        ));
        let schedules = app.world().resource::<Schedules>();
        let graph = schedules
            .get(FixedPostUpdate)
            .expect("physics runs in FixedPostUpdate")
            .graph();
        let key = |set: PhysicsTransformSystems| {
            graph
                .system_sets
                .get_key(set.intern())
                .unwrap_or_else(|| panic!("{set:?} is configured"))
        };
        assert!(
            graph.dependency().graph().contains_edge(
                NodeId::Set(key(PhysicsTransformSystems::Propagate)),
                NodeId::Set(key(PhysicsTransformSystems::TransformToPosition)),
            ),
            "collider propagation is not ordered before TransformToPosition"
        );
    }
}
