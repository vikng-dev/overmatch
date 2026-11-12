//! Networking adapter shared by client and server composition roots.

use avian3d::schedule::PhysicsSystems;
use bevy::prelude::*;

mod admission;
pub(crate) mod battle;
/// The Battle's match UI: the time left on screen, the scoreboard on Tab and the results.
mod battle_hud;
/// The Battle mode's data: one file, parsed and checked once.
mod battle_mode;
mod client;
/// What a build states at the handshake, and the cause a refused peer of another build is shown.
mod compatibility;
/// Tick-stamped server announcements held until the interpolation cursor crosses their tick, so an
/// event presents in sync with the interpolated motion it belongs to.
mod cursor_queue;
/// Real-UDP probes of the controlled tank's input path: the device's command filed on every tick of
/// a frame, the death flow's neutral one while dead, and a click the bridge restores never filed
/// again.
#[cfg(test)]
mod death_flow_input;
mod death_screen;
// The kill cam's composed schedule under test mounts the death screen, the reader of its gate.
#[cfg(test)]
pub(crate) use death_screen::{mount_bare as mount_death_screen, shows_died};
/// The debris stress lever (`OVERMATCH_DEBRIS_STRESS`): server-simulated wall fragments, asleep at
/// rest, their poses replicated, drawn by clients.
mod debris_stress;
mod debug_hud;
mod diagnostics;
mod disclosure;
/// The owner's fire presentation: intent edges on the local tick, the arriving gate reconciled as a
/// legality report instead of read as permission to draw.
mod fire_presentation;
/// Read-only starvation and resync instruments at the interpolation buffer's edge.
mod frontier;
mod harness;
mod he_stress;
pub(crate) use harness::{env_flag, env_parse, env_text, env_value, refuse_start};
mod contact_census;
mod hit_feel;
/// The dedicated server's hosted mode: settings from the Worker, heartbeats, capacity, empty exit.
mod hosting;
/// Real-UDP probe of the client's one-input-message-per-tick send cadence.
#[cfg(test)]
mod input_cadence;
/// The interpolation buffer's size, derived from the measured link instead of pinned.
mod interp_delay;
/// Replicon round trips of the construction ledger: late, lossy and far-behind clients end with
/// the authority's list or refuse the match, and a caught-up one is sent only what it lacks.
#[cfg(test)]
mod ledger_wire;
#[cfg(test)]
mod link_drop;
/// `pub(crate)` for the frozen-rubble bed (`headless_test`), which fells a house the same way.
pub(crate) mod loadtest;
/// The pre-connect Join menu against the matchmaking Worker (matchmaking mode only).
mod matchmaking;
mod physics;
mod protocol;
mod ram;
mod resync;
mod rig;
mod rig_admission;
/// The match score: knockouts and losses per combatant, and each knocked-out life's respawn
/// opening, held by the server and replicated. `pub(crate)` for the ballistics fixture that fires
/// a real round at it.
pub(crate) mod score;
// `pub(crate)` for its spawn POINTS — see the note on `tank::scenario`.
pub(crate) mod server;
/// The dedicated server's host: its task-pool sizing, its startup line and its levers.
pub(crate) mod server_host;
mod server_runner;
/// The dedicated server's periodic stats line on stdout (`OVERMATCH_SERVER_STATS_S`).
mod server_stats;
/// The pen cam's explanations: the authority's per-recipient send, the client's store.
mod shot_explanation;
/// Real-UDP, loss-injected shot-transport integration tests.
#[cfg(test)]
mod shot_loss;
mod shot_transport;
// `pub(crate)` for `spawn_limit` — the clamp the spawn regression test resolves its corners at.
/// The construction ledger: every construction change on one globally visible entity, replicated by diff.
mod construction;
pub(crate) mod spawn_map;
mod spawn_placement;
/// The sync margins on both wire timelines, derived from the measured link instead of pinned.
mod sync_margin;
#[cfg(test)]
mod test_harness;
/// The netcode boundary scan and the upstream lightyear tripwires.
#[cfg(test)]
mod tests;
/// The net client's UI, screen by screen: every root owned, each screen drawing only its own.
#[cfg(test)]
mod ui_ownership;
/// The quantised wire forms of the per-tick hull and belt state.
mod wire_quant;
mod world_reset;

/// Run the network client.
pub use client::run as run_client;
/// Run the authoritative dedicated server.
pub use server::run as run_server;

pub(super) use battle_hud::plugin as battle_hud_plugin;
pub(super) use death_screen::plugin as death_screen_plugin;
pub(super) use debug_hud::plugin as debug_hud_plugin;
pub(super) use hit_feel::plugin as hit_feel_plugin;
pub(crate) use protocol::NetBot;
pub(super) use spawn_map::plugin as spawn_map_plugin;

use crate::state::AppState;
use crate::tank::PendingTankAssets;

/// This build's version: the string the release tag names (`v` + it), the protocol fingerprint
/// folds, the matchmaking Worker matches servers on, and the Edgegap app version is named after.
pub(crate) const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Shared protocol, physics, and safety wiring. Both endpoints must mount it identically.
fn plugin(app: &mut App) {
    protocol::plugin(app);
    rig_admission::plugin(app);
    battle::install_hold(app);
    physics::plugin(app);
    // Record corrupt values before Avian's physics preparation consumes them.
    app.add_systems(
        FixedPostUpdate,
        diagnostics::fixed_nan_probe.before(PhysicsSystems::Prepare),
    );
}

/// Enter gameplay when the current tank view assets are ready.
fn open_gameplay_gate(
    assets: Res<PendingTankAssets>,
    asset_server: Res<AssetServer>,
    state: Res<State<AppState>>,
    mut next: ResMut<NextState<AppState>>,
) {
    if *state.get() != AppState::Loading {
        return;
    }
    if assets.loaded(&asset_server) {
        info!("net: tank assets loaded — entering AppState::Playing");
        next.set(AppState::Playing);
    }
}
