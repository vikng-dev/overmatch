//! Networking adapter shared by client and server composition roots.

use avian3d::schedule::PhysicsSystems;
use bevy::prelude::*;

/// Unconditional adoption of authoritative facts the client cannot predict — the forced-rollback
/// primitive `HullShock` is merely the first consumer of.
mod adoption;
/// Does the arrival of an unpredicted authoritative component force the rollback that delivers
/// the server's shove? The confirmation run behind the receiving half of combat.
#[cfg(test)]
mod arrival_rollback;
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
mod grip;
mod harness;
pub(crate) use harness::{env_flag, env_parse, env_value};
mod hit_feel;
/// Does an authority hull-shock bump reach the owner's live hull velocity? The receiving half of
/// combat, on the production registration.
#[cfg(test)]
mod hull_shock_rollback;
/// The interpolation buffer's size, derived from the measured link instead of pinned.
mod interp_delay;
/// Does an authoritative hull fact reach the client at the lead the SHIPPING sync config actually
/// produces (0, and −1 under deadband drift)? RED by design — slice 2's acceptance test.
#[cfg(test)]
mod lead_zero_rollback;
mod physics;
mod protocol;
/// The own hull's firing recoil, overlaid on the rendered pose while the replicated kick is still
/// in flight down the interpolation buffer.
mod recoil_overlay;
mod render_error;
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
mod watchdog;

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
pub(crate) use recoil_overlay::RecoilOverlay;
pub(crate) use render_error::RenderErrorOffset;
pub(super) use spawn_map::plugin as spawn_map_plugin;

use rig::client_smoothing_plugin;

use crate::state::AppState;
use crate::tank::PendingTankAssets;

/// Shared protocol, physics, rig, and safety wiring. Both endpoints must mount it identically.
fn plugin(app: &mut App) {
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
