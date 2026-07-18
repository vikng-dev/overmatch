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
/// construction source; the shadow harness keeps proving it
/// equivalent to the instantiated scene on every view bind.
mod bake;
mod ballistics;
mod branding;
mod camera;
/// The command layer: device reads → player bindings → per-tank serializable `TankCommand`. The
/// seam authoritative multiplayer hangs off; sim modules consume commands, never devices.
mod command;
/// The per-fixed-tick sim-COST recorder (`SPIKE_COST_TRACE=<path>`): an env-gated JSONL log of
/// FixedUpdate tick time, the `ballistics::integrate_projectiles` share of it, and entity/projectile
/// counts — the reusable measurement rig for the machine-gun-march cost spike. Off (zero cost) unless
/// the env var is set; registered on the net server and client composition roots.
mod cost;
/// The controlled tank's crew bar + swap input — a shared piece of the fixed player UI, mounted by
/// both `GamePlugin` and the sandbox (each scoped to the `Controlled` tank).
mod crew_ui;
pub(crate) mod damage;
#[cfg(feature = "dev_tools")]
mod debug;
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
/// each [`ShotId`] on BOTH ends — the authority's fire/keyframe/terminal/damage emissions, and the
/// client's arrivals (with the dedup verdict) plus its marker and cosmetic shell/trail boundaries.
/// Net-neutral (plain `u32` ticks), so `ballistics` writes to it without naming the netcode. Off (zero
/// cost) unless the env var is set. Analyzed by `scripts/shot/analyze.py`.
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
/// The track model's pure core (route/oracle/chain math) — consumed by the sandbox lab and, in
/// phase A, the game's track view. See `.agents/docs/design/track-model/architecture.md`.
pub mod track;
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

#[cfg(test)]
mod offline_feel_tests {
    use super::*;
    use track::sim::{TankTransmission, TransmissionFeelTest};
    use track::transmission::{TransmissionMode, TransmissionState};

    /// The offline `T` dial: each press advances governor → hybrid → L600 → governor and
    /// resets every tank's transmission state to a freshly-constructed one (the mode flip
    /// must never inherit another adapter's gear/shift leftovers). This is the scripted
    /// stand-in for the interactive cycle proof — macOS blocks synthetic keystrokes into the
    /// windowed launch, so the cycle path is pinned here instead.
    #[test]
    fn t_key_cycles_transmission_mode_and_resets_state() {
        let mut app = App::new();
        app.insert_resource(TransmissionFeelTest(TransmissionMode::FixedRadii));
        app.init_resource::<ButtonInput<KeyCode>>();
        app.add_systems(Update, cycle_transmission_feel);
        let tank = app
            .world_mut()
            .spawn(TankTransmission(TransmissionState {
                gear: 5,
                shift_ticks: 3,
                steer_step: 2,
                reverse: true,
                park: true,
            }))
            .id();

        let press = |app: &mut App| {
            let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            input.clear();
            input.release(KeyCode::KeyT);
            input.clear();
            input.press(KeyCode::KeyT);
            app.update();
            app.world().resource::<TransmissionFeelTest>().0
        };
        assert_eq!(press(&mut app), TransmissionMode::Governor);
        assert_eq!(
            app.world().get::<TankTransmission>(tank).unwrap().0,
            TransmissionState::default(),
            "a mode flip must reset the transmission state"
        );
        assert_eq!(press(&mut app), TransmissionMode::Hybrid);
        assert_eq!(press(&mut app), TransmissionMode::FixedRadii);
    }
}

/// Marks a network-client replica. Ballistics uses it to suppress authority-only damage and impulse
/// writes while retaining cosmetic flight and impacts.
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

/// Net client's predicted-present tick, republished as net-neutral sim vocabulary. Replica ballistics
/// uses it to age sanctioned outcomes; authority and sandbox compositions do not install it.
#[derive(Resource, Default)]
pub(crate) struct PredictedPresent(pub u32);

/// Net-neutral current tick published before gameplay. Local network fire uses it to construct a
/// [`ShotId`] before shell spawn; authority/sandbox shells may be unkeyed.
#[derive(Resource, Default)]
pub(crate) struct ShotClock(pub u32);

/// Physics collision layers. View/aim queries that want the ground (camera terrain ray, sight
/// probes) filter to `Terrain` only, so they ignore vehicle colliders. Shared infra, hence at
/// the crate root.
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

/// A non-zero, match-local identity assigned synchronously when a combatant spawns.
///
/// Entity ids are an ECS implementation detail: a respawn receives a new entity and every client
/// maps that entity independently. This value stays with the player or bot across respawn, making
/// delayed outcomes addressable without depending on either lifetime or mapping.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub(crate) struct CombatantId(pub(crate) u64);

/// Canonical, net-neutral identity for one shot: `(combatant, weapon, fire_tick)`.
///
/// Invariant: `fire_tick` distinguishes successive rounds from one weapon, while `combatant` is
/// stable plain data rather than an entity mapping. Every shot-scoped wire and cosmetic outcome keys
/// on this triple, so it remains usable across client mappings and a firing tank's despawn.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub(crate) struct ShotId {
    pub(crate) combatant: CombatantId,
    pub(crate) weapon: u8,
    pub(crate) fire_tick: u32,
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
            // The shared analytic terrain field (track architecture §5): built from
            // `TerrainMap` for the sim force systems (phase B) and the client track view —
            // one oracle on server, SP, and net client alike.
            track::terrain_plugin,
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
            // Phase-B locomotion: the track model's belt forces ARE the driving sim
            // (ADR-0025; replaces the raycast-roadwheel model of ADR-0005).
            track::sim_plugin,
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
            // Live tracks: the simulated chain + wheel/sprocket animation on the presented pose
            // (view-only, ADR-0014 — the server never mounts this).
            track::view_plugin,
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
            // Live tracks on the presented pose — predicted AND remote tanks (one code path;
            // `net::render_error` orders the set after its correction smoothing).
            track::view_plugin,
        ));

        // Physics visualization + debug toggles, same pair `ClientPlugin` mounts for SP
        // (`G` = force arrows + collider wireframes, `X` = x-ray, `F` = camera detach). View-only:
        // it reads `TrackContacts`/`GlobalTransform` and draws gizmos — nothing sim-visible — so it is
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

/// The offline feel-test route (`overmatch --offline` / `OVERMATCH_OFFLINE=1`): the windowed
/// runtime plugin set [`run_client`](net::run_client) mounts — exe-relative asset root, continuous
/// winit updates — plus [`GamePlugin`], the true single-player composition. NO netcode: no
/// lightyear plugins, no connection entity, nothing that can attempt a connect.
///
/// This is the ONLY composition that inserts [`track::sim::ElementGripFeelTest`], the
/// startup-latched gate under which `apply_track_forces` runs the per-element grip regime
/// (element-promotion-checklist.md Q1, phase 2 of the element promotion). Every other composition
/// leaves the resource absent and keeps today's aggregate law bit-for-bit.
pub fn run_offline() {
    let mut app = App::new();
    // Exe-relative asset root, exactly as `run_client` resolves it: a double-clicked binary
    // finds `assets/` beside it no matter the launch cwd.
    app.add_plugins(
        DefaultPlugins
            .set(bevy::asset::AssetPlugin {
                // The same `String` conversion `net::client`'s wrapper applies.
                file_path: assets::asset_root().to_string_lossy().into_owned(),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: Some(Window {
                    // ASCII hyphen: `lib.rs` is scanned by the ui_ascii guard now that the
                    // offline feel label spawns `Text` here (default-font surface).
                    title: "Overmatch - offline".into(),
                    ..default()
                }),
                ..default()
            }),
    );
    // Same policy as the net client: never drop below the 64 Hz tick when unfocused.
    app.insert_resource(bevy::winit::WinitSettings::continuous());
    app.add_plugins(GamePlugin);
    // The offline element-grip gate — latched at process start, ONLY here (see the resource doc).
    app.init_resource::<track::sim::ElementGripFeelTest>();
    // The offline transmission feel test (phase 2.5): default the Tiger's authored
    // architecture (L600 fixed-radius regenerative — per-vehicle SPEC eventually; the
    // resource is the interim dial). `T` cycles governor → hybrid → L600 live; the mode is
    // logged AND shown on a fixed screen line so a capture always states what it ran.
    app.insert_resource(track::sim::TransmissionFeelTest(
        track::transmission::TransmissionMode::FixedRadii,
    ));
    app.add_systems(Startup, spawn_transmission_feel_label);
    app.add_systems(Update, cycle_transmission_feel);
    app.run();
}

/// Marker for the offline transmission-mode screen line.
#[derive(Component)]
struct TransmissionFeelLabel;

/// The cheapest honest on-screen statement of the active drivetrain (offline only): one
/// absolute-positioned text line, top-right — the game's debug overlay is gizmo-only, so a
/// bare `Text` node (Bevy's built-in font) is the minimal surface that keeps the mode visible
/// in every screenshot/feel note.
fn spawn_transmission_feel_label(
    mut commands: Commands,
    feel: Res<track::sim::TransmissionFeelTest>,
) {
    info!("offline transmission mode → {} (T cycles)", feel.0.label());
    commands.spawn((
        TransmissionFeelLabel,
        Text::new(String::new()),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::srgb(0.8, 0.75, 0.5)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            right: Val::Px(12.0),
            ..default()
        },
    ));
}

/// `T` cycles the offline transmission mode (governor → hybrid → L600). Every tank's
/// [`track::sim::TankTransmission`] resets so the incoming adapter starts from a constructed
/// state (gear 1, no shift in flight) instead of another mode's leftovers.
fn cycle_transmission_feel(
    keys: Option<Res<ButtonInput<KeyCode>>>,
    feel: Option<ResMut<track::sim::TransmissionFeelTest>>,
    mut states: Query<&mut track::sim::TankTransmission>,
    label: Query<&mut Text, With<TransmissionFeelLabel>>,
) {
    use track::transmission::TransmissionMode;
    let Some(mut feel) = feel else {
        return;
    };
    let cycled = keys.is_some_and(|keys| keys.just_pressed(KeyCode::KeyT));
    if cycled {
        feel.0 = match feel.0 {
            TransmissionMode::Governor => TransmissionMode::Hybrid,
            TransmissionMode::Hybrid => TransmissionMode::FixedRadii,
            TransmissionMode::FixedRadii => TransmissionMode::Governor,
        };
        for mut state in &mut states {
            *state = track::sim::TankTransmission::default();
        }
        info!("offline transmission mode → {}", feel.0.label());
    }
    if cycled || feel.is_added() {
        for mut text in label {
            text.0 = format!("trans [T]: {}", feel.0.label());
        }
    }
}
