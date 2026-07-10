//! Overmatch — a realistic 3D multiplayer tank game (single-player vertical slice).
//!
//! Organized one plugin per feature. `GamePlugin` composes them; `main.rs` only supplies
//! the runtime and runs the app. Each feature module owns its components, systems, and its
//! own wiring (a `pub fn plugin(app: &mut App)`), so this list reads as a table of contents.

// Two clippy lints fight Bevy's ECS paradigm and are allowed crate-wide (as Bevy's own codebase
// does): `type_complexity` fires on ordinary multi-component query tuples, and `too_many_arguments`
// on systems that legitimately need many params. We de-duplicate the genuinely-repeated query shapes
// behind named `QueryData`/`SystemParam` (e.g. `damage::VolumeFacets`, `damage::ControlledTank`);
// what remains is irreducible ECS shape, not a smell.
#![allow(clippy::type_complexity, clippy::too_many_arguments)]

use avian3d::prelude::{PhysicsInterpolationPlugin, PhysicsLayer, PhysicsPlugins};
use bevy::prelude::*;

mod aim;
/// The runtime asset-root resolver (`asset_root`) — where `assets/` lives, resolved once and shared
/// by both `AssetPlugin` (`net::client`) and the tank bake (`bake`) so they never open different
/// `.glb` files. Always compiled: `bake` builds under `--no-default-features`, where `net` is off.
mod assets;
/// The tank-geometry extractor + shadow harness (sim/view split — design
/// `sim-view-split-and-tank-bake.md` §8). `extract(glb) → TankGeometry` IS the sim skeleton's
/// spawn source since step 1 (`tank::spawn_tank_sim`); the shadow harness keeps proving it
/// equivalent to the instantiated scene on every view bind.
mod bake;
mod ballistics;
mod branding;
mod camera;
/// The command layer: device reads → player bindings → per-tank serializable `TankCommand`. The
/// seam authoritative multiplayer hangs off; sim modules consume commands, never devices.
mod command;
/// The controlled tank's crew bar + swap input — a shared piece of the fixed player UI, mounted by
/// both `GamePlugin` and the sandbox (each scoped to the `Controlled` tank).
mod crew_ui;
pub(crate) mod damage;
#[cfg(feature = "dev_tools")]
mod debug;
mod driving;
/// Re-exported for `tests/spherecast_scale.rs`: the sphere probe's witness-geometry distance
/// reconstruction + its TOI-band guard (the parry TOI-tolerance workaround) are pinned against
/// raw parry casts there — the helper's math and parry's behavior, not the `apply_suspension`
/// call site (that wiring's live guard is the idle at-rest harness metric).
pub use driving::{SPHERE_CAST_TOI_SLACK, sphere_cast_ground_contact};
/// Fire control: per-weapon superelevation range tables + the player-dialed range. Sits atop
/// `ballistics`; the aim commit reads it to lob the aim point so the bore elevates for range.
mod firecontrol;
/// The dedicated-server guard: boots `SimPlugin` headless (no GPU/window/winit) and drives the
/// tank via `TankCommand` — fails first if sim code grows a hard render dependency.
#[cfg(test)]
mod headless_test;
/// The shared tank-state HUD (world-anchored capability/crew/damage readouts). Mounted by both
/// `GamePlugin` and the sandbox; each tags its own world camera with `hud::HudCamera`.
mod hud;
/// The game's networking layer (`net` feature only). Public so the `client`/`server` bins can call
/// `net::client::run()`/`net::server::run()`; not part of `GamePlugin`.
#[cfg(feature = "net")]
pub mod net;
/// The armor ballistics sandbox (`bin/armor_sandbox`). Public so the binary can mount it; not part
/// of `GamePlugin`.
pub mod sandbox;
mod shooting;
mod sight;
mod spec;
mod state;
mod tank;
/// The jitter-trace recorder (`SPIKE_TRACE=<path>`): an env-gated JSONL log of rendered vs. simulated
/// pose, rollback events, and correction decay — passive instrumentation for the MP hull-jitter
/// investigation. Off (zero cost) unless the env var is set. Net-specific rows are `#[cfg(net)]`.
mod trace;
/// The track-model sandbox (`bin/track_sandbox`). Public so the binary can mount it; not part of
/// `GamePlugin`. Self-contained: its own code-generated primitive rig + locomotion, for developing
/// the continuous-track model in isolation.
pub mod track_sandbox;
mod world;

/// Marker resource: this app is a NETWORK CLIENT running the shared sim as a REPLICA of the server,
/// not an authority. Damage is server-authoritative — the client still flies shells, raycasts, sparks
/// impacts, and despawns spent shells (all cosmetic), but must NOT deposit HP or apply hit impulse, or
/// it would independently simulate a divergent local kill the server never sanctioned (the bug this
/// slice fixes). `ballistics` gates its four HP writes and `on_hit_impulse` on this being ABSENT.
/// Inserted ONLY by `net::client::run`; single-player (`GamePlugin`), the sandbox, and the dedicated
/// server never insert it, so those authorities keep depositing damage normally. Lives at the crate
/// root (not `net`) so the always-compiled `ballistics` can reference it without the `net` feature.
#[derive(Resource, Default)]
pub(crate) struct ClientReplica;

/// Physics collision layers. Wheel suspension rays filter to `Terrain` only, so they ignore
/// the vehicle's own hull collider (ADR-0005). Shared infra, hence at the crate root.
#[derive(PhysicsLayer, Default, Clone, Copy, Debug)]
pub(crate) enum Layer {
    #[default]
    Default,
    Terrain,
    Vehicle,
    /// Ballistic volumes (armor plates + modules): what the penetration march raycasts against,
    /// distinct from `Vehicle` (the dynamic collision proxy). "Same geometry, two layers" (ADR-0008).
    Armor,
}

/// The simulation — the authority layer, in the client/server sense (see the memory note and
/// bevy_replicon's "abstracting over configurations"): everything the server must run to be the
/// truth, and everything a predicting client re-runs. Consumes `TankCommand`s, never devices;
/// steps on the fixed clock. A dedicated server mounts exactly this (plus netcode) on
/// `MinimalPlugins`; the single-player game mounts it alongside [`ClientPlugin`].
pub struct SimPlugin;

impl Plugin for SimPlugin {
    fn build(&self, app: &mut App) {
        // NOTE: physics (avian `PhysicsPlugins`) is deliberately NOT mounted here — its
        // configuration is the one thing that legitimately differs per composition root:
        // single-player wants `PhysicsInterpolationPlugin::interpolate_all()` (ADR-0004), the
        // networked bins must disable exactly that plugin for `LightyearAvianPlugin` (spike log,
        // increment 5). The composition root (GamePlugin / the net bins) owns the choice.
        app.add_plugins((
            state::sim_plugin,
            world::plugin,
            // `spec` registers the `.tank.ron` data-asset loader before `tank` spawns the tank
            // and requests one (ADR-0010).
            spec::plugin,
            // Sim/view split: extract the tank glb as data at startup — the sim skeleton's spawn
            // source on every composition (SP, net client, net server) — and shadow-verify it
            // against every instantiated scene.
            bake::plugin,
            tank::sim_plugin,
            // Commands are the sim's only input: `core_plugin` puts a `TankCommand` on every tank
            // and consumes latched edges each tick; `driving`/`shooting`/`aim` read it.
            command::core_plugin,
            driving::plugin,
            aim::sim_plugin,
            // `ballistics` owns the shell trajectory + impact seam; `shooting` is the gun control
            // that drives it (the sandbox drives the same `FireShell` from its camera).
            ballistics::plugin,
            // Range tables at bind: the servo bridge lobs each tank's aim by its commanded range.
            firecontrol::sim_plugin,
            damage::plugin,
            shooting::plugin,
        ));
    }
}

/// Gate the device-reading [`state::PlayerInputSet`] on a captured cursor (`state::cursor_locked`),
/// in each schedule its members live in: `Update` (aim commit, view toggle), `PostUpdate` (free-look
/// orbit), and `RunFixedMainLoop` (gunner aim, range dial, drive gather). Shared by both windowed
/// composition roots — SP [`ClientPlugin`] and net [`NetClientPlugin`] — so the license to consume
/// mouse/gameplay input (`grab_mode == Locked`) is configured identically in one place. The headless
/// server and the scripted harness mount neither root, so the gate never touches them.
fn gate_player_input(app: &mut App) {
    use bevy::ecs::schedule::ScheduleLabel;
    use state::{PlayerInputSet, cursor_locked};
    for schedule in [
        Update.intern(),
        PostUpdate.intern(),
        RunFixedMainLoop.intern(),
    ] {
        app.configure_sets(schedule, PlayerInputSet.run_if(cursor_locked));
    }
}

/// The client — command generation (devices → `TankCommand`) and presentation (state → screen).
/// Requires [`SimPlugin`] in the same app (single-player and listen-server mount both; a pure
/// network client will too, for interpolation/prediction).
pub struct ClientPlugin;

impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {
        gate_player_input(app);
        app.add_plugins((
            branding::plugin,
            // Pause/cursor handling (drives the states that `state::sim_plugin` owns).
            state::client_plugin,
            // Device gather: the only device→command translation.
            command::client_plugin,
            tank::client_plugin,
            camera::plugin,
            aim::client_plugin,
            // `sight` owns the gunner-view toggle/mode that `camera` and `aim` branch on.
            sight::plugin,
            // The player's range dial (rides to the sim inside the command).
            firecontrol::client_plugin,
            // The tank-state HUD and the controlled tank's crew bar + `1`–`5` swap input.
            hud::plugin,
            crew_ui::plugin,
        ));

        // Physics visualization (collider/ray wireframes) + debug toggles, behind the `dev_tools`
        // feature (default-on, droppable from an optimized build via `--no-default-features`).
        #[cfg(feature = "dev_tools")]
        app.add_plugins((avian3d::prelude::PhysicsDebugPlugin, debug::plugin));
    }
}

/// The networked client's presentation + device gather (Milestone B step 8): [`ClientPlugin`]
/// minus the single-player-only pieces. No `state::client_plugin` — its Esc pause freezes the
/// local sim and physics clock, which desyncs a predicting client from a server that keeps
/// ticking; there is no online pause, so the netcode bin owns its own cursor-release menu overlay
/// instead. No `tank::client_plugin` — the Tab possession swap is an SP scenario tool; under
/// netcode the server assigns possession (`ControlledBy`).
#[cfg(feature = "net")]
pub struct NetClientPlugin;

#[cfg(feature = "net")]
impl Plugin for NetClientPlugin {
    fn build(&self, app: &mut App) {
        gate_player_input(app);
        app.add_plugins((
            branding::plugin,
            command::client_plugin,
            camera::plugin,
            aim::client_plugin,
            sight::plugin,
            firecontrol::client_plugin,
            hud::plugin,
            crew_ui::plugin,
            // Bottom-right ping/FPS/frame-time debug panel — net-client only (ping is meaningless
            // in SP), for testing against the deployed server.
            net::debug_hud::plugin,
            // The death screen + respawn key — net-client only (SP has no respawn flow): shows
            // "YOU DIED" when the player's own tank is knocked out and latches the respawn edge.
            net::death_screen::plugin,
            // View-layer combat feedback (net-client only): the camera kick + damage flash when the
            // player is hit, and the hit-marker when the player's shell drops an opponent's health.
            net::hit_feel::plugin,
        ));

        // Physics visualization + debug toggles, same pair `ClientPlugin` mounts for SP
        // (`G` = force arrows + collider wireframes, `X` = x-ray, `F` = camera detach). View-only:
        // it reads `Suspension`/`GlobalTransform` and draws gizmos — nothing sim-visible — so it is
        // safe on a predicting client and is never mounted by the headless server (which composes
        // `SimPlugin` only, never this plugin). Behind the `dev_tools` feature (default-on).
        #[cfg(feature = "dev_tools")]
        app.add_plugins((avian3d::prelude::PhysicsDebugPlugin, debug::plugin));
    }
}

/// Every gameplay feature, composed — the single-player configuration: the full sim plus the
/// local client, one app, no netcode. Add to an `App` that already has the runtime plugins.
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            // The single-player physics choice: bodies render at an interpolated pose between
            // fixed steps (ADR-0004). The networked bins mount lightyear's config instead.
            PhysicsPlugins::default().set(PhysicsInterpolationPlugin::interpolate_all()),
            SimPlugin,
            // The single-player scenario: two-tank duel spawn, first tank controlled.
            tank::sp_spawn_plugin,
            ClientPlugin,
            // Passive jitter-trace recorder (frame + tick rows; no net extras in this build). Idle
            // unless `SPIKE_TRACE` is set.
            trace::sp_plugin,
        ));
    }
}
