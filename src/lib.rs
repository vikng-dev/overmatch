//! Shared simulation and runtime composition for Overmatch.
//!
//! Product binaries compose this crate as an authoritative server or predicted network client.
//! Direct-simulation sandboxes are analytical tools, not alternate player runtimes. See
//! `.agents/PRODUCT.md` and ADR-0024 for the current product topology.

// Two clippy lints fight Bevy's ECS paradigm and are allowed crate-wide (as Bevy's own codebase
// does): `type_complexity` fires on ordinary multi-component query tuples, and `too_many_arguments`
// on systems that legitimately need many params. We de-duplicate the genuinely-repeated query shapes
// behind named `QueryData`/`SystemParam` (e.g. `damage::VolumeFacets`, `damage::ControlledTank`);
// what remains is irreducible ECS shape, not a smell.
#![allow(clippy::type_complexity, clippy::too_many_arguments)]

use avian3d::prelude::{PhysicsInterpolationPlugin, PhysicsLayer, PhysicsPlugins};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

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
/// The per-fixed-tick sim-COST recorder (`SPIKE_COST_TRACE=<path>`): an env-gated JSONL log of
/// FixedUpdate tick time, the `ballistics::integrate_projectiles` share of it, and entity/projectile
/// counts — the reusable measurement rig for the machine-gun-march cost spike. Off (zero cost) unless
/// the env var is set; registered on the net server and client composition roots.
mod cost;
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
/// The networking implementation. Executables enter through [`run_client`] and [`run_server`];
/// the adapter tree is private to the library.
mod net;
pub use net::{run_client, run_server};
/// The net client's single overlay authority (active-set resource + pure input/cursor/scrim rules for
/// the connect / death / menu / view-death overlays). Lives at the crate root, NOT under `net`,
/// because it is pure view-state that the always-sim `sight` module also declares into — putting it
/// here keeps `sight` from naming `crate::net` (the `tests/net_boundary.rs` guard). Mounted only by
/// [`NetClientPlugin`]; single-player has `state::client_plugin`'s real pause instead.
mod overlay;
/// The armor ballistics sandbox (`bin/armor_sandbox`). Public so the binary can mount it; not part
/// of `GamePlugin`.
pub mod sandbox;
mod shooting;
/// The SHOT-LIFECYCLE recorder (`SPIKE_SHOT_TRACE=<path>`): an env-gated JSONL log of what happens to
/// each [`ShotId`] on BOTH ends — the authority's fire/keyframe/confirm emissions, and the client's
/// arrivals (with the dedup verdict) plus its cosmetic shell's spawn → contact → hold → re-seed /
/// terminal / dissolve. Net-neutral (plain `u32` ticks), so `ballistics` writes to it without naming
/// the netcode. Off (zero cost) unless the env var is set. Analyzed by `scripts/shot/analyze.py`.
mod shot_trace;
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
/// The bundled UI typeface (Barlow Condensed): loads the two weights once and exposes them as a
/// `ui_font::UiFonts` resource that every `Text`-spawning client plugin reads. Mounted by each
/// windowed composition root; retires Bevy's ASCII-only default font.
mod ui_font;
/// Ship-facing view-layer combat VFX: render-only subscribers to the sim's `Impact` and
/// `FireShell` seams (impact puffs, the 88's muzzle flash/light/smoke + shell smoke trail) plus
/// the shared billboard/erosion/gradient-LUT machinery they are built from. Mounted by both
/// windowed clients (ADR-0014 — never the server).
mod vfx;
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

/// Marker resource: lightyear is REPLAYING a rollback right now — re-running `FixedMain` from a
/// restored past tick up to the predicted present, N times in one frame. The sim layer reads it (as
/// `Option<Res<Replaying>>`, `.0` true only mid-replay) to keep VIEW-ONLY, tick-timed cosmetic work
/// OFF replayed ticks: the cosmetic shell march + `Held` aging advance the shell's picture one step
/// per FORWARD tick, and the shooter's own-shell `FireShell` trigger fires once per forward fire
/// tick. Replaying them would double-march every in-flight shell by the rollback depth, over-count
/// the `Held` grace window (burning it in one frame and corrupting the `present − bounce_tick`
/// re-seed arithmetic), and re-spawn a DUPLICATE own shell sharing one `ShotId` every time a replay
/// re-crosses a fire tick. The DETERMINISTIC sim mutations (`TankSim` belt/reload/recoil, hull
/// impulse) still replay — only the cosmetic reconstruction is skipped.
///
/// Net-neutral like [`ClientReplica`]: this crate-root marker is the sim's vocabulary, but only
/// `net::client` (which alone may name lightyear's `Rollback`) WRITES it — a `bool` re-derived at the
/// head of every `FixedUpdate` from whether this is a replayed tick. Absent on the authority
/// (server / SP / sandbox), which never rolls back, so its absence reads as "forward tick" everywhere
/// the writer is not mounted. Lives at the crate root (not `net`) so the always-compiled `ballistics`
/// / `shooting` can reference it without the netcode in scope (`tests/net_boundary`).
#[derive(Resource, Default)]
pub(crate) struct Replaying(pub bool);

/// The net client's PREDICTED PRESENT tick `P` (raw `u32`), republished to the sim layer every
/// FORWARD `FixedUpdate`. Every cosmetic shell a net client flies lives at `P` — the observer's via
/// its `fire_tick` catch-up, the shooter's own natively — so this single value IS each in-flight
/// shell's present tick. The ballistics march reads it (as `Option<Res<PredictedPresent>>`) for F3's
/// tick-triggered consumption: a shell whose interpolated-pose flight MISSED the plate the server
/// resolved on never contacts, so once `P` passes the sanctioned outcome's server tick
/// (`bounce_tick` / `impact_tick`, both net-neutral on the buffer) by a margin, the march consumes it
/// anyway — re-seeding at the server bounce or finalizing at the server impact — rather than letting
/// the round sail on through where the authoritative shell bounced or terminated.
///
/// Net-neutral like [`Replaying`]: a crate-root sim type whose ONLY writer is `net::client` (which
/// alone may read lightyear's `LocalTimeline`). Absent on the authority (server / SP / sandbox),
/// which resolves shots for real and never consults it.
#[derive(Resource, Default)]
pub(crate) struct PredictedPresent(pub u32);

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

/// The correlation spine for one shot: which weapon on which tank fired on which tick. Every
/// shot-scoped wire message keys on it — the cosmetic tracer ([`net::protocol::FireEvent`]) exposes it
/// by accessor (`FireEvent::shot_id`, no extra bytes on the wire), the server-sanctioned bounce
/// ([`net::protocol::RicochetKeyframe`]) carries it, and both ends stamp it on the local cosmetic shell
/// they spawn ([`ballistics::Shot`]) — so an arriving keyframe re-seeds EXACTLY the shell it belongs to,
/// and a redundantly-retransmitted duplicate is deduped instead of spawning a second tracer.
///
/// **`fire_tick` is what makes successive rounds distinct.** An automatic weapon fires the same
/// `(shooter, weapon)` every few ticks; without the tick every round of a burst would share one id and
/// the redundancy dedup would collapse the whole burst to a single shell. It is strictly increasing per
/// `(shooter, weapon)` (one shot per weapon per tick, ticks advance), which the receiver's dedup relies on.
///
/// **NET-NEUTRAL BY DESIGN.** `fire_tick` is a plain `u32` (the raw tick value), not lightyear's `Tick`,
/// so the always-runnable sim layer (`ballistics`) can key shells on it without naming the netcode
/// (`tests/net_boundary`); [`net::protocol`] converts to/from `Tick` at the wire boundary. Lives at the
/// crate root beside [`ClientReplica`]/[`Layer`] for the same reason those do: shared sim/net vocabulary.
/// The `shooter` [`Entity`] is wire-mapped by the carrying message to the receiver's local replica, so a
/// `ShotId` is stable within one receiver — which is exactly the dedup/correlation scope.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct ShotId {
    pub shooter: Entity,
    pub weapon: u8,
    pub fire_tick: u32,
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
            // Load the bundled UI font first: it inserts `UiFonts` at build time, so the HUD/crew
            // spawn systems below always find it (see `ui_font`).
            ui_font::plugin,
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
            // Impact dust puffs — every landed round reads at the target (view-only, ADR-0014).
            vfx::plugin,
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
pub struct NetClientPlugin;

impl Plugin for NetClientPlugin {
    fn build(&self, app: &mut App) {
        gate_player_input(app);
        app.add_plugins((
            // Load the bundled UI font first (inserts `UiFonts` at build time; see `ui_font`).
            ui_font::plugin,
            branding::plugin,
            command::client_plugin,
            camera::plugin,
            aim::client_plugin,
            sight::plugin,
            firecontrol::client_plugin,
            hud::plugin,
            crew_ui::plugin,
            // The single overlay authority (net-client only): one active-set resource + derived
            // input/cursor/scrim rules behind which connect status, the death screen, the Esc menu,
            // and the view-death black all compose with explicit priority and z-order. Owns the one
            // cursor system; the connect/death/sight owners declare their presence into it.
            overlay::plugin,
            // Bottom-right ping/FPS/frame-time debug panel — net-client only (ping is meaningless
            // in SP), for testing against the deployed server.
            net::debug_hud_plugin,
            // The death screen + respawn key — net-client only (SP has no respawn flow): shows
            // "YOU DIED" when the player's own tank is knocked out and latches the respawn edge.
            net::death_screen_plugin,
            // View-layer combat feedback (net-client only): the camera kick + damage flash when the
            // player is hit, and the hit-marker when the player's shell drops an opponent's health.
            net::hit_feel_plugin,
            // Impact dust puffs — every landed round reads at the target (view-only, ADR-0014; the
            // replica's cosmetic shells spark the same `Impact` seam, so remote fire puffs too).
            vfx::plugin,
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
