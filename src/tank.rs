//! Tank simulation and presentation facade.
//!
//! Simulation construction is synchronous and data-driven; the glb remains a presentation-only
//! tree. The public surface deliberately exposes completed-root construction and the one
//! replicated-root exception, never the skeleton assembler itself.

use crate::state::{AppState, GameplaySet};
use bevy::prelude::*;

// The battle scene (`OVERMATCH_BATTLE`): its spawn is the scenario's, its runtime `run_offline`'s.
pub(crate) mod battle;
mod integrity;
mod model;
// `pub(crate)` for its spawn POINTS: `terrain_grid`'s spawn regression test asserts every
// shipped spawn point in one place (see `every_shipped_spawn_point_lands_above_the_shipped_terrain`).
pub(crate) mod scenario;
mod servo;
mod shadow_proxy;
mod spawn;
mod view;

#[cfg(test)]
pub use model::WeaponState;
pub(crate) use model::rig_world_pose;
#[allow(
    unused_imports,
    reason = "the model's facade list; each target and feature set reaches a different subset"
)]
pub use model::{
    Controlled, EngineSound, Hull, Muzzle, Rig, Roadwheel, Tank, TankRoot, TankServos, TankSim,
    TankViews, TrackSide, Turret, TurretBody, ViewConfig, Weapon, WeaponGate, WeaponGateState,
    WeaponIndex, WeaponLoad, WeaponLoads, WeaponRound,
};
pub use scenario::{client_plugin, sp_spawn_plugin};
pub(crate) use servo::RemoteServos;
// The servo mechanism itself, for the aim regression net: `aim`'s transport tests converge real
// servos at authored Tiger rates rather than asserting on a target angle.
pub use servo::{
    ServoCommand, ServoIndex, ServoRole, ServoSpec, ServoState, servo_pose, shortest_angle,
};
#[cfg(test)]
pub(crate) use servo::{ServoRest, drive_servos, restore_servo_truth};
pub(crate) use spawn::TANK_SPEC_PATH;
#[cfg(any(feature = "bitprobe", feature = "dev_tools", test))]
pub(crate) use spawn::spawn_headless_tank;
pub(crate) use spawn::{
    PendingTankAssets, TANK_GLB_PATH, TANK_SIM_GLB_PATH, TankContent, TankPresentation,
    TankSimSource, attach_replicated_tank_body, load_tank_assets, load_tank_sim_assets,
    spawn_complete_tank,
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
