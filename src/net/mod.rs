//! Networking adapter shared by client and server composition roots.

use avian3d::schedule::PhysicsSystems;
use bevy::prelude::*;

mod client;
mod contact_probe;
mod death_screen;
mod debug_hud;
mod diagnostics;
mod disclosure;
mod grip;
mod harness;
pub(crate) use harness::{env_flag, env_parse, env_value};
mod hit_feel;
mod physics;
mod protocol;
mod render_error;
mod rig;
mod server;
/// Real-UDP, loss-injected shot-transport integration tests.
#[cfg(test)]
mod shot_loss;
mod shot_transport;
#[cfg(test)]
mod test_harness;
mod watchdog;

/// Run the predicted network client.
pub use client::run as run_client;
/// Run the authoritative dedicated server.
pub use server::run as run_server;

pub(super) use death_screen::plugin as death_screen_plugin;
pub(super) use debug_hud::plugin as debug_hud_plugin;
pub(super) use hit_feel::plugin as hit_feel_plugin;
pub(crate) use protocol::NetBot;
pub(crate) use render_error::RenderErrorOffset;

use rig::client_smoothing_plugin;

use crate::state::AppState;
use crate::tank::PendingTankAssets;

/// Shared protocol, physics, rig, and safety wiring. Both endpoints must mount it identically.
fn plugin(app: &mut App) {
    // Network compositions promote the element law once at startup. The adapter itself still gates
    // on body role, so interpolated static remotes never simulate or receive the private field.
    app.insert_resource(crate::track::sim::ElementGripNetcode);
    protocol::plugin(app);
    physics::plugin(app);
    rig::plugin(app);
    // Record corrupt values before Avian's physics preparation consumes them.
    app.add_systems(
        FixedPostUpdate,
        diagnostics::fixed_nan_probe.before(PhysicsSystems::Prepare),
    );
}

/// Enter gameplay when the current tank view assets are ready.
fn open_gameplay_gate(
    assets: Option<Res<PendingTankAssets>>,
    asset_server: Res<AssetServer>,
    state: Res<State<AppState>>,
    mut next: ResMut<NextState<AppState>>,
) {
    if *state.get() != AppState::Loading {
        return;
    }
    let Some(assets) = assets else { return };
    if assets.loaded(&asset_server) {
        info!("net: tank assets loaded — entering AppState::Playing");
        next.set(AppState::Playing);
    }
}
