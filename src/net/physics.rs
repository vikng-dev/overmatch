//! Physics configuration shared by the network client and server.

use avian3d::physics_transform::PhysicsTransformSystems;
use avian3d::prelude::{
    IslandPlugin, IslandSleepingPlugin, PhysicsInterpolationPlugin, PhysicsTransformPlugin,
};
use avian3d::schedule::PhysicsSystems;
use bevy::prelude::*;

/// Own the schedule edge removed with `PhysicsTransformPlugin`: collider propagation must run in
/// Avian's `PhysicsSystems::Prepare` set before physics consumes child collider transforms.
pub(crate) fn plugin(app: &mut App) {
    app.configure_sets(
        FixedPostUpdate,
        PhysicsTransformSystems::Propagate.in_set(PhysicsSystems::Prepare),
    );
}

/// Plugin set required by `LightyearAvianPlugin`; Lightyear owns frame interpolation and rollback
/// must not sleep islands.
pub fn physics_plugins() -> bevy::app::PluginGroupBuilder {
    avian3d::prelude::PhysicsPlugins::default()
        .build()
        .disable::<PhysicsTransformPlugin>()
        .disable::<PhysicsInterpolationPlugin>()
        .disable::<IslandPlugin>()
        .disable::<IslandSleepingPlugin>()
}
