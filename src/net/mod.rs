//! Networking adapter shared by client and server composition roots.

use avian3d::schedule::PhysicsSystems;
use bevy::prelude::*;

mod client;
mod contact_probe;
/// Tick-stamped server announcements held until the interpolation cursor crosses their tick, so an
/// event presents in sync with the interpolated motion it belongs to.
mod cursor_queue;
mod death_screen;
mod debug_hud;
mod diagnostics;
mod disclosure;
/// The interpolation buffer's edge: starvation instruments (always on) and the bounded
/// extrapolation gap-filler (`OVERMATCH_EXTRAPOLATE=1`).
mod extrapolate;
/// The owner's fire presentation: intent edges on the local tick, the arriving gate reconciled as a
/// legality report instead of read as permission to draw.
mod fire_presentation;
mod harness;
pub(crate) use harness::{env_flag, env_parse, env_value};
mod hit_feel;
/// The interpolation buffer's size, derived from the measured link instead of pinned.
mod interp_delay;
mod physics;
mod protocol;
mod rig;
// `pub(crate)` for its spawn POINTS — see the note on `tank::scenario`.
pub(crate) mod server;
/// Real-UDP, loss-injected shot-transport integration tests.
#[cfg(test)]
mod shot_loss;
mod shot_transport;
// `pub(crate)` for `spawn_limit` — the clamp the spawn regression test resolves its corners at.
pub(crate) mod spawn_map;
/// The sync margins on both wire timelines, derived from the measured link instead of pinned.
mod sync_margin;
#[cfg(test)]
mod test_harness;

/// The hidden-capture focus revocation — shared with `run_offline`'s hidden-capture mode.
#[cfg(target_os = "macos")]
pub(crate) use client::revoke_macos_activation;
/// Run the predicted network client.
pub use client::run as run_client;
/// Run the authoritative dedicated server.
pub use server::run as run_server;

pub(super) use death_screen::plugin as death_screen_plugin;
pub(super) use debug_hud::plugin as debug_hud_plugin;
pub(super) use hit_feel::plugin as hit_feel_plugin;
pub(crate) use protocol::NetBot;
pub(super) use spawn_map::plugin as spawn_map_plugin;

use crate::state::AppState;
use crate::tank::PendingTankAssets;

/// Shared protocol, physics, and safety wiring. Both endpoints must mount it identically.
fn plugin(app: &mut App) {
    protocol::plugin(app);
    physics::plugin(app);
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
