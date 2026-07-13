//! Networking adapter shared by the client and dedicated server composition roots.
//!
//! [`protocol`] owns the symmetric wire contract, [`physics`] the Avian integration, [`rig`] the
//! replicated tank lifecycle, and [`diagnostics`] plus [`harness`] the measurement tooling.

use avian3d::schedule::PhysicsSystems;
use bevy::prelude::*;

mod client;
mod contact_probe;
mod death_screen;
mod debug_hud;
mod diagnostics;
mod harness;
mod hit_feel;
mod physics;
mod protocol;
mod render_error;
mod rig;
mod server;
/// The loss-injected end-to-end tripwire: two real apps over a real (conditioned) lightyear link,
/// asserting exactly-once cosmetic-shell spawn and ricochet carry-through under seeded packet loss.
/// Test-only — it exists to close the model-vs-reality gap the redundancy unit tests leave open.
#[cfg(test)]
mod shot_loss;
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

/// The shared networking layer both composition roots mount: the wire contract (`protocol`), the
/// physics re-anchor (`physics`), the networked rig lifecycle (`rig`), and the physics NaN probe.
/// Identical on client and server, as lightyear demands.
fn plugin(app: &mut App) {
    protocol::plugin(app);
    physics::plugin(app);
    rig::plugin(app);
    // Probe ahead of the physics pass, so the first corrupt value is named BEFORE avian's own
    // finite-asserts panic mid-step (the Update-schedule tripwire never sees it — corruption and
    // panic land inside one FixedMain run).
    app.add_systems(
        FixedPostUpdate,
        diagnostics::fixed_nan_probe.before(PhysicsSystems::Prepare),
    );
}

/// `SimPlugin` mounts `state::sim_plugin` (`AppState`, `GameplaySet` gated on `Playing`), and the
/// composition roots have no menu/loading flow to drive that transition themselves ("the roots
/// never enter Playing on their own now"). Both already gate their spawn/rig work on the spec load
/// (`spawn_pending_tanks` / `attach_replicated_rig`); this just opens the `GameplaySet` gate once,
/// the same load dependency, so the sim actually ticks.
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
