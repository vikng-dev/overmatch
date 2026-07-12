//! The game's networking layer (`net` feature). Both the client and the server mount [`plugin`]:
//! lightyear requires IDENTICAL protocol registration on both sides of the wire, added after
//! `ServerPlugins`/`ClientPlugins` and before the `Server`/`Client` connection entity is spawned
//! (see the spike map §3 ordering note). The submodules split that layer by concern:
//! [`protocol`] is the wire contract, [`physics`] the avian configuration, [`rig`] the networked
//! tank-rig lifecycle, [`client`]/[`server`] the two composition roots, and [`diagnostics`] +
//! [`harness`] the measurement/test tooling.

use avian3d::schedule::PhysicsSystems;
use bevy::prelude::*;

pub mod client;
pub mod contact_probe;
pub mod death_screen;
pub mod debug_hud;
pub mod diagnostics;
pub mod harness;
pub mod hit_feel;
pub mod physics;
pub mod protocol;
pub mod render_error;
pub mod rig;
pub mod server;
/// The loss-injected end-to-end tripwire: two real apps over a real (conditioned) lightyear link,
/// asserting exactly-once cosmetic-shell spawn and ricochet carry-through under seeded packet loss.
/// Test-only — it exists to close the model-vs-reality gap the redundancy unit tests leave open.
#[cfg(test)]
mod shot_loss;
pub mod watchdog;

pub use physics::physics_plugins;
pub use rig::client_smoothing_plugin;

use crate::state::AppState;
use crate::tank::PendingTankAssets;

/// The shared networking layer both composition roots mount: the wire contract (`protocol`), the
/// physics re-anchor (`physics`), the networked rig lifecycle (`rig`), and the physics NaN probe.
/// Identical on client and server, as lightyear demands.
pub fn plugin(app: &mut App) {
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
pub(crate) fn open_gameplay_gate(
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
