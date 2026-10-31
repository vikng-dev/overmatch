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

/// Plugin set required by `LightyearAvianPlugin`: transform sync and interpolation belong to
/// Lightyear, so Avian's own transform and interpolation plugins stay disabled.
pub fn physics_plugins() -> bevy::app::PluginGroupBuilder {
    avian3d::prelude::PhysicsPlugins::default()
        .build()
        .disable::<PhysicsTransformPlugin>()
        .disable::<PhysicsInterpolationPlugin>()
}

#[cfg(test)]
mod tests {
    use avian3d::physics_transform::PhysicsTransformSystems;
    use bevy::ecs::schedule::{NodeId, SystemSet};
    use bevy::prelude::*;

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
