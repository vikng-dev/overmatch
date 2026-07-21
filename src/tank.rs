//! Tank simulation and presentation facade.
//!
//! Simulation construction is synchronous and data-driven; the glb remains a presentation-only
//! tree. The public surface deliberately exposes completed-root construction and the one
//! replicated-root exception, never the skeleton assembler itself.

use crate::state::{AppState, GameplaySet};
use bevy::prelude::*;

mod integrity;
mod model;
mod scenario;
mod servo;
mod spawn;
mod view;

#[cfg(test)]
pub use model::WeaponState;
pub(crate) use model::rig_world_pose;
#[allow(unused_imports)]
pub use model::{
    Controlled, Hull, Muzzle, Rig, Roadwheel, Tank, TankRoot, TankSim, TankViews, TrackSide,
    Turret, Weapon, WeaponGate, WeaponGateState, WeaponIndex,
};
pub use scenario::{client_plugin, sp_spawn_plugin};
pub use servo::{ServoCommand, ServoIndex, ServoRole, ServoSpec, ServoState, shortest_angle};
#[cfg(feature = "bitprobe")]
pub(crate) use spawn::spawn_bitprobe_tank;
pub(crate) use spawn::{
    PendingTankAssets, TIGER_GLB_PATH, TankContent, TankPresentation, TankSimSource,
    attach_replicated_tank_body, load_tank_assets, spawn_complete_tank,
};
pub use view::{SimParts, ViewNode, ViewOf, view_attach_plugin};

/// Shared simulation composition and schedule. This stays at the facade so module ownership
/// cannot accidentally move an ordering edge: restoration before gameplay, servo stepping after.
pub fn sim_plugin(app: &mut App) {
    app.add_observer(integrity::shield_authored_collider_transform);
    app.add_observer(integrity::shield_late_authored_marker);
    app.add_observer(integrity::sweep_launched_turret_on_root_despawn);
    app.add_systems(
        FixedUpdate,
        servo::restore_servo_truth
            .run_if(in_state(AppState::Playing))
            .before(GameplaySet),
    )
    .add_systems(
        FixedUpdate,
        servo::drive_servos
            .run_if(in_state(AppState::Playing))
            .after(GameplaySet),
    )
    .add_systems(
        Update,
        servo::interpolate_servos.run_if(in_state(AppState::Playing)),
    )
    .add_plugins(view_attach_plugin);
}
