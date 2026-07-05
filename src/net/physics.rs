//! The physics-plugin configuration the networked layer runs under: the avian disables
//! `LightyearAvianPlugin` requires, plus the transform-propagation re-anchor that keeps a
//! freshly-spawned body's child colliders finite through their first physics ticks.

use avian3d::physics_transform::PhysicsTransformSystems;
use avian3d::prelude::{
    IslandPlugin, IslandSleepingPlugin, PhysicsInterpolationPlugin, PhysicsTransformPlugin,
};
use avian3d::schedule::PhysicsSystems;
use bevy::prelude::*;

/// THE COLLIDER-PROPAGATION NaN FIX. Disabling `PhysicsTransformPlugin` (required by
/// `LightyearAvianPlugin`, see `physics_plugins()`) also removes the ONLY `configure_sets`
/// that anchors `PhysicsTransformSystems::Propagate` inside `PhysicsSystems::Prepare`
/// (avian `physics_transform/mod.rs:86`) — but `ColliderTransformPlugin` (mounted by the
/// collider backend, NOT disabled) still adds `propagate_collider_transforms` to that set in
/// `FixedPostUpdate`. Unanchored, it ran at an arbitrary point relative to the physics step:
/// when a fresh body's child colliders caught the wrong interleaving, their
/// `ColliderTransform`s went NaN and took every child collider `Position` with them (~70% of
/// 100 ms runs, within a frame of Dynamic activation, measured in the old async-bind era;
/// activation-order fixes empirically falsified — see the spike log). The hazard is the set
/// anchoring itself, not any spawn timing — re-anchoring restores avian's own ordering.
pub(crate) fn plugin(app: &mut App) {
    app.configure_sets(
        FixedPostUpdate,
        PhysicsTransformSystems::Propagate.in_set(PhysicsSystems::Prepare),
    );
}

/// The disables `LightyearAvianPlugin` requires, plus `IslandPlugin`/`IslandSleepingPlugin` (map
/// §8: sleeping bodies can corrupt rollback replay). Both bins build `PhysicsPlugins` with this,
/// instead of the game's `PhysicsInterpolationPlugin::interpolate_all()` — lightyear's own
/// `FrameInterpolationSystems` takes over that job (map §8's "REAL, already-identified conflict").
pub fn physics_plugins() -> bevy::app::PluginGroupBuilder {
    avian3d::prelude::PhysicsPlugins::default()
        .build()
        .disable::<PhysicsTransformPlugin>()
        .disable::<PhysicsInterpolationPlugin>()
        .disable::<IslandPlugin>()
        .disable::<IslandSleepingPlugin>()
}
