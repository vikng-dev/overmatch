//! Authoritative dedicated-server composition root.

use core::time::Duration;
use std::net::{Ipv4Addr, SocketAddr};

use avian3d::prelude::{Position, RigidBody, Rotation};
use bevy::ecs::schedule::ScheduleConfigs;
use bevy::ecs::system::{ScheduleSystem, SystemParam};
use bevy::prelude::*;
use lightyear::prelude::input::native::{ActionState, NativeStateSequence};
use lightyear::prelude::input::server::{InputValidationAppExt, authorize_controlled_targets};
use lightyear::prelude::server::*;
use lightyear::prelude::*;

use super::admission::JoinAdmission;
use super::battle::{BattleState, Boundary, CycleRules, HoldCommands, in_combat, places_tanks};
use super::battle_mode::BattleMode;
use super::disclosure::{CombatDisclosure, NetTankStatus, RespawnState, SpawnPickRefusal};
use super::hosting::{Hosted, HostedConfig};
use super::protocol::{
    LaunchedTurretPose, NetCrew, NetTank, ServoAngles, SetSpawnPoint, protocol_id,
};
use super::score::{Opening, RespawnGate, RespawnWait};
use super::spawn_placement::SpawnPlacement;
use super::{harness, open_gameplay_gate, physics, spawn_map};
use crate::command::{ConsumeCommandEdges, TankCommand};
use crate::damage::TankKnockedOut;
use crate::hull_marks::HullMarks;
use crate::state::{GameplaySet, SimPhase};
// The spawn-height rule and its constants live in `terrain_grid`: ONE rule for every spawn path in
// every composition (see `terrain_grid::spawn_surface_height`), so the offline duel, the lanes, the
// bot and the spawn-map overrides cannot drift apart. Spawn points here are HORIZONTAL (XZ) — Y is
// sampled at the moment of spawn.
use crate::tank::{
    PendingTankAssets, Rig, TankContent, TankSimSource, load_tank_sim_assets, spawn_complete_tank,
};
use crate::terrain_grid::spawn_pos;
// The spawn selector reads the world's standing geometry through the SAME encoding the analytic
// track field is built from (`world::TerrainMap` blocks → `TerrainBlock`), so the two cannot drift.
use crate::track::oracle::TerrainBlock;
use crate::world::{SpawnObstacles, TerrainMap};
use crate::{CombatantId, CombatantLife, SimPlugin};

const PORT: u16 = 5888;

// Resolution: the authority evaluates gameplay at 64 Hz, shared with Lightyear's timeline.
pub(super) const TICK: Duration = Duration::from_nanos(15_625_000);

pub fn run() {
    let hosted = match HostedConfig::from_env() {
        Ok(hosted) => hosted,
        Err(err) => {
            eprintln!("overmatch-server: fatal: {err}");
            std::process::exit(2);
        }
    };
    if let Some(config) = &hosted {
        super::hosting::install_panic_hook(config).unwrap_or_else(|err| {
            eprintln!("overmatch-server: fatal: could not start panic reporter: {err}");
            std::process::exit(2);
        });
    }
    let bind = harness::env_text("OVERMATCH_SERVER_BIND")
        .and_then(|value| bind_ip(value.as_deref()))
        .unwrap_or_else(harness::refuse_start);
    let bot_count = harness::env_text("OVERMATCH_BOT")
        .and_then(|value| BotCount::from_value(value.as_deref()))
        .unwrap_or_else(harness::refuse_start);
    let mut app = App::new();
    app.add_plugins(crate::server_plugin_floor().set(bevy::log::LogPlugin {
        // This target only reports missing RenderApp extraction; the server has no GPU backend.
        filter: format!(
            "{},bevy_render::extract_resource=off",
            bevy::log::DEFAULT_FILTER
        ),
        ..default()
    }));
    super::server_host::log_startup();
    // No GPU supplies compression support. Declare the loaders' existing NONE fallback explicitly;
    // the server allowlist carries only PNG textures and `load_tank_sim_assets` opens no scene.
    app.insert_resource(bevy::image::CompressedImageFormatSupport(
        bevy::image::CompressedImageFormats::NONE,
    ));
    app.add_plugins(super::server_runner::plugin);
    app.add_plugins(super::plugin);
    super::disclosure::install_server(&mut app);
    super::construction::install_server(&mut app);
    // A capture plan's wall cue (`OVERMATCH_CAPTURE`, its `wall`): inert without one.
    crate::dev_capture::server_wall_plugin(&mut app);
    if let Err(err) = super::debris_stress::install_server(&mut app) {
        eprintln!("overmatch-server: fatal: {err}");
        std::process::exit(2);
    }
    super::he_stress::install_server(&mut app).unwrap_or_else(harness::refuse_start);
    super::loadtest::install(&mut app).unwrap_or_else(harness::refuse_start);
    app.add_plugins(physics::physics_plugins());
    app.add_plugins(SimPlugin);
    app.add_plugins(crate::trace::server_plugin);
    app.add_plugins(crate::cost::server_plugin);
    super::server_stats::install(&mut app).unwrap_or_else(harness::refuse_start);
    app.add_plugins(crate::shot_trace::server_plugin);
    super::shot_transport::install_server(&mut app);
    super::shot_explanation::install_server(&mut app);
    // Every strike's hull mark, recorded on the struck root and replicated by diff.
    super::hull_marks::install_server(&mut app);
    // Authority must reject input targets not controlled by their sending client.
    app.add_input_validator(authorize_controlled_targets::<NativeStateSequence<TankCommand>>);
    // Lightyear visibility hooks require `ReplicationSender` on each remote link.
    app.add_observer(attach_replication_sender);

    let admission = JoinAdmission::default();
    app.insert_resource(admission.clone());
    let server = app
        .world_mut()
        .spawn((
            Name::new("Server"),
            netcode_server(hosted.as_ref(), admission),
            LocalAddr(SocketAddr::new(bind.into(), PORT)),
            ServerUdpIo::default(),
        ))
        .id();
    app.add_systems(
        PostStartup,
        (move |mut commands: Commands| {
            commands.trigger(Start { entity: server });
            info!("server: starting, listening on {bind}:{PORT}");
        })
        .after(super::ram::prepare),
    );
    // The SIM load: the spec sheet only. The dedicated server opens no view artifact — no scene,
    // no textures, no rungs (ADR-0035); the walk's geometry arrives through `bake`'s extraction of
    // `<id>.sim.glb`.
    app.add_systems(Startup, load_tank_sim_assets);
    install_battle(&mut app, BattleMode::shipped(), TICK);
    app.insert_resource(bot_count);
    install_spawn_picks(&mut app);
    install_client_spawns(&mut app);

    app.add_systems(
        Update,
        (
            spawn_bots.run_if(places_tanks),
            respawn_dead_bots.run_if(in_combat),
            open_gameplay_gate,
        ),
    );
    app.add_systems(
        FixedUpdate,
        (
            // A bot authors its command as a device does, before the parse and the Battle's hold.
            drive_bot.before(crate::command::ParseCommands),
            player_respawn_system(),
        ),
    );

    let hosting = hosted
        .as_ref()
        .map(Hosted::start)
        .transpose()
        .unwrap_or_else(|err| {
            eprintln!("overmatch-server: fatal: could not start hosting reporter: {err}");
            std::process::exit(2);
        });
    if let Some(hosting) = &hosting {
        hosting.install(&mut app);
    }

    app.run();
    if let Some(hosting) = hosting {
        hosting.finish();
    }
}

/// The production join-to-spawn boundary: queue each new link once before placing pending tanks.
fn install_client_spawns(app: &mut App) {
    app.add_systems(
        Update,
        (handle_new_clients, spawn_pending_tanks.run_if(places_tanks)).chain(),
    );
}

/// Root replacement publishes before the drive snapshot; no source can disappear between
/// snapshot publication and the same tick's ordered contact commit.
fn player_respawn_system() -> ScheduleConfigs<ScheduleSystem> {
    respawn_player_tanks
        .run_if(in_combat)
        .in_set(GameplaySet)
        .before(SimPhase::DrivingForces)
        .before(ConsumeCommandEdges)
        .into_configs()
}

/// Install the production Battle clock and score. The short-mode headless probe mounts this same
/// composition, with the same netcode gate, rather than reproducing the phase machine in a test.
pub(super) fn install_battle(app: &mut App, mode: BattleMode, tick: Duration) {
    app.init_resource::<PendingClients>()
        .init_resource::<SpawnLane>()
        .init_resource::<SpawnOverrides>()
        .init_resource::<CombatantIds>()
        .init_resource::<JoinAdmission>()
        .insert_resource(CycleRules::new(&mode, tick));
    super::score::install_server(
        app,
        RespawnWait::new(mode.respawn_wait, tick),
        BattleState::default(),
    );
    app.insert_resource(SpawnPlacement::new(mode.spawn));
    app.add_systems(
        FixedUpdate,
        cycle_battles
            .after(super::protocol::publish_shot_clock)
            .before(HoldCommands)
            .before(GameplaySet),
    );
}

/// Advance before the Battle's hold and gameplay read this tick. No combatant or flying round
/// survives the results boundary; connected links persist and are queued together at the next
/// Battle, whose spawn pass places them into its rebuilt world.
#[expect(
    clippy::too_many_arguments,
    reason = "one atomic phase boundary spans clock, score, admission, connected links and combat cleanup"
)]
fn cycle_battles(
    clock: Res<crate::ShotClock>,
    rules: Res<CycleRules>,
    mut battles: Query<(&mut BattleState, &mut super::score::Scoreboard)>,
    admission: Res<JoinAdmission>,
    clients: Query<(Entity, &RemoteId), (With<ClientOf>, With<Connected>)>,
    mut pending: ResMut<PendingClients>,
    mut overrides: ResMut<SpawnOverrides>,
    mut lane: ResMut<SpawnLane>,
    combat: Query<Entity, Or<(With<NetTank>, With<crate::ballistics::Projectile>)>>,
    combatants: Query<&CombatantId, With<NetTank>>,
    mut commands: Commands,
) {
    let (mut battle, mut score) = battles
        .single_mut()
        .expect("server startup spawns exactly one Battle with its scoreboard");
    let previous = *battle;
    let mut next = *battle;
    match next.advance(clock.0, !clients.is_empty(), &rules, &score) {
        // Bots can already be placed while the empty server waits for a player.
        Boundary::Combat => {
            for &combatant in &combatants {
                score.enter_combat(combatant);
            }
        }
        Boundary::Results => {
            pending.0.clear();
            for root in &combat {
                commands.entity(root).despawn();
            }
        }
        Boundary::NextBattle => {
            score.0.clear();
            overrides.0.clear();
            lane.0 = 0;
            let number = next.number;
            commands.queue(move |world: &mut World| super::world_reset::start_next(world, number));
            pending.0 = clients
                .iter()
                .map(|(link, remote)| (link, remote.0))
                .collect();
        }
        Boundary::None => {}
    }
    admission.set_open(next.accepts_joins());
    if previous != next {
        *battle = next;
        info!(
            "server: Battle {} {:?} at tick {}",
            battle.number,
            battle.phase,
            clock.0.raw()
        );
    }
}

/// The address the UDP socket binds, from `OVERMATCH_SERVER_BIND`: every interface unless an IPv4
/// address is named. `127.0.0.1` keeps a local run off the network, and so out of the host
/// firewall's way. A value that is not an address is refused at launch rather than widened to
/// every interface.
fn bind_ip(value: Option<&str>) -> Result<Ipv4Addr, harness::LeverError> {
    match value.map(str::trim) {
        None | Some("") => Ok(Ipv4Addr::UNSPECIFIED),
        Some(value) => value.parse().map_err(|_| {
            harness::LeverError::invalid("OVERMATCH_SERVER_BIND", value, "an IPv4 address")
        }),
    }
}

/// The netcode endpoint. Hosted: the Worker's per-match key and its player cap, past which the
/// handshake answers `ServerFull`. Otherwise: the all-zero dev key every client's
/// `Authentication::Manual` holds, and lightyear's default cap.
pub(super) fn netcode_server(
    hosted: Option<&HostedConfig>,
    admission: JoinAdmission,
) -> NetcodeServer {
    let defaults = NetcodeConfig::default();
    NetcodeServer::new(NetcodeConfig {
        // Must match the client's `Authentication::Manual.protocol_id`.
        protocol_id: protocol_id(),
        private_key: hosted.map_or([0; 32], HostedConfig::netcode_key),
        max_clients: hosted.map_or(defaults.max_clients, HostedConfig::max_clients),
        // OFF: a manual token names the address the CLIENT dialled — the host's public IP and
        // mapped port — while this socket binds `0.0.0.0`, and netcode's check matches an
        // unspecified bind only against LOOPBACK token addresses. Left on, every non-loopback
        // client is dropped at the connection request (an `info!`, no error) and retries forever.
        // The server cannot know its own public address to list it in
        // `additional_expected_addresses`.
        server_addr_check: false,
        connection_request_handler: Some(std::sync::Arc::new(admission)),
        ..defaults
    })
}

/// Connected clients waiting for the server's next spawn pass. The simulation blueprint is eager,
/// so this queue batches link setup and spawn work rather than waiting on view assets.
#[derive(Resource, Default)]
struct PendingClients(Vec<(Entity, PeerId)>);

/// Match-local combatant identities. `next` is never reused; player links retain their id across
/// respawn, and each bot, keyed by its index, retains its id across its timed respawns. `lives`
/// counts each combatant's spawns, the next [`CombatantLife`] to hand out.
#[derive(Resource, Default)]
struct CombatantIds {
    next: u64,
    players: bevy::platform::collections::HashMap<Entity, CombatantId>,
    bots: bevy::platform::collections::HashMap<u32, CombatantId>,
    lives: bevy::platform::collections::HashMap<CombatantId, u32>,
}

impl CombatantIds {
    fn allocate(&mut self) -> CombatantId {
        self.next = self.next.checked_add(1).expect(
            "combatant-id allocator exhausted u64; match cannot allocate another combatant",
        );
        CombatantId(self.next)
    }

    fn player(&mut self, link: Entity) -> CombatantId {
        if let Some(id) = self.players.get(&link) {
            return *id;
        }
        let id = self.allocate();
        self.players.insert(link, id);
        id
    }

    fn bot(&mut self, index: u32) -> CombatantId {
        if let Some(id) = self.bots.get(&index) {
            return *id;
        }
        let id = self.allocate();
        self.bots.insert(index, id);
        id
    }

    /// The life `combatant`'s next spawn is: 0 for its first, one more each time.
    fn life(&mut self, combatant: CombatantId) -> CombatantLife {
        let next = self.lives.entry(combatant).or_default();
        let life = CombatantLife(*next);
        *next = next
            .checked_add(1)
            .expect("combatant-life counter exhausted u32");
        life
    }
}

/// Insert the marker required by Lightyear's per-client visibility hooks before tank spawning.
pub(super) fn attach_replication_sender(add: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(add.entity).insert(ReplicationSender);
}

/// Monotonic spawn-lane allocator; concurrent tanks must not overlap at spawn.
#[derive(Resource, Default)]
struct SpawnLane(u32);

/// Deterministic outward search pattern for an occupied spawn point: per-radius direction
/// tables of fixed unit vectors (exact constants — no trig at runtime, so every peer/replay
/// resolves the identical candidate), tried ring by ring through [`SPAWN_SEARCH_RINGS`]; the
/// first free candidate wins.
///
/// WHY per-radius counts: 8 fixed compass spokes leave 2π·48/8 ≈ 37.7 m arc gaps at the outer
/// ring — free ground the search never samples. Each ring's count is
/// `max(8, ceil(2πr / 8 m))` — DERIVED candidates at most ~8 m of arc apart, the fixed
/// search resolution — pinned by `spawn_search_rings_are_unit_uniform_and_dense_enough`.
const SPAWN_DIRS_8: [Vec2; 8] = [
    Vec2::new(1.0, 0.0),
    Vec2::new(
        core::f32::consts::FRAC_1_SQRT_2,
        core::f32::consts::FRAC_1_SQRT_2,
    ),
    Vec2::new(0.0, 1.0),
    Vec2::new(
        -core::f32::consts::FRAC_1_SQRT_2,
        core::f32::consts::FRAC_1_SQRT_2,
    ),
    Vec2::new(-1.0, 0.0),
    Vec2::new(
        -core::f32::consts::FRAC_1_SQRT_2,
        -core::f32::consts::FRAC_1_SQRT_2,
    ),
    Vec2::new(0.0, -1.0),
    Vec2::new(
        core::f32::consts::FRAC_1_SQRT_2,
        -core::f32::consts::FRAC_1_SQRT_2,
    ),
];

/// 13 unit directions for the 16 m ring: cos/sin(2πk/13), CCW from +X — spacing
/// 2π·16/13 ≈ 7.73 m of arc (≤ the 8 m coverage target). Exact constants,
/// no runtime trig; near-zero components snapped to 0. Pinned by
/// `spawn_search_rings_are_unit_uniform_and_dense_enough`.
const SPAWN_DIRS_13: [Vec2; 13] = [
    Vec2::new(1.0, 0.0),
    Vec2::new(0.885456, 0.46472317),
    Vec2::new(0.56806475, 0.82298386),
    Vec2::new(0.12053668, 0.99270886),
    Vec2::new(-0.3546049, 0.9350162),
    Vec2::new(-0.7485108, 0.66312265),
    Vec2::new(-0.97094184, 0.23931566),
    Vec2::new(-0.97094184, -0.23931566),
    Vec2::new(-0.7485108, -0.66312265),
    Vec2::new(-0.3546049, -0.9350162),
    Vec2::new(0.12053668, -0.99270886),
    Vec2::new(0.56806475, -0.82298386),
    Vec2::new(0.885456, -0.46472317),
];
/// 19 unit directions for the 24 m ring: cos/sin(2πk/19), CCW from +X — spacing
/// 2π·24/19 ≈ 7.94 m of arc (≤ the 8 m coverage target). Exact constants,
/// no runtime trig; near-zero components snapped to 0. Pinned by
/// `spawn_search_rings_are_unit_uniform_and_dense_enough`.
const SPAWN_DIRS_19: [Vec2; 19] = [
    Vec2::new(1.0, 0.0),
    Vec2::new(0.94581723, 0.32469946),
    Vec2::new(0.7891405, 0.6142127),
    Vec2::new(0.54694813, 0.8371665),
    Vec2::new(0.24548548, 0.9694003),
    Vec2::new(-0.082579345, 0.9965845),
    Vec2::new(-0.40169543, 0.91577333),
    Vec2::new(-0.67728156, 0.7357239),
    Vec2::new(-0.87947375, 0.47594738),
    Vec2::new(-0.9863613, 0.16459459),
    Vec2::new(-0.9863613, -0.16459459),
    Vec2::new(-0.87947375, -0.47594738),
    Vec2::new(-0.67728156, -0.7357239),
    Vec2::new(-0.40169543, -0.91577333),
    Vec2::new(-0.082579345, -0.9965845),
    Vec2::new(0.24548548, -0.9694003),
    Vec2::new(0.54694813, -0.8371665),
    Vec2::new(0.7891405, -0.6142127),
    Vec2::new(0.94581723, -0.32469946),
];
/// 26 unit directions for the 32 m ring: cos/sin(2πk/26), CCW from +X — spacing
/// 2π·32/26 ≈ 7.73 m of arc (≤ the 8 m coverage target). Exact constants,
/// no runtime trig; near-zero components snapped to 0. Pinned by
/// `spawn_search_rings_are_unit_uniform_and_dense_enough`.
const SPAWN_DIRS_26: [Vec2; 26] = [
    Vec2::new(1.0, 0.0),
    Vec2::new(0.97094184, 0.23931566),
    Vec2::new(0.885456, 0.46472317),
    Vec2::new(0.7485108, 0.66312265),
    Vec2::new(0.56806475, 0.82298386),
    Vec2::new(0.3546049, 0.9350162),
    Vec2::new(0.12053668, 0.99270886),
    Vec2::new(-0.12053668, 0.99270886),
    Vec2::new(-0.3546049, 0.9350162),
    Vec2::new(-0.56806475, 0.82298386),
    Vec2::new(-0.7485108, 0.66312265),
    Vec2::new(-0.885456, 0.46472317),
    Vec2::new(-0.97094184, 0.23931566),
    Vec2::new(-1.0, 0.0),
    Vec2::new(-0.97094184, -0.23931566),
    Vec2::new(-0.885456, -0.46472317),
    Vec2::new(-0.7485108, -0.66312265),
    Vec2::new(-0.56806475, -0.82298386),
    Vec2::new(-0.3546049, -0.9350162),
    Vec2::new(-0.12053668, -0.99270886),
    Vec2::new(0.12053668, -0.99270886),
    Vec2::new(0.3546049, -0.9350162),
    Vec2::new(0.56806475, -0.82298386),
    Vec2::new(0.7485108, -0.66312265),
    Vec2::new(0.885456, -0.46472317),
    Vec2::new(0.97094184, -0.23931566),
];
/// 32 unit directions for the 40 m ring: cos/sin(2πk/32), CCW from +X — spacing
/// 2π·40/32 ≈ 7.85 m of arc (≤ the 8 m coverage target). Exact constants,
/// no runtime trig; near-zero components snapped to 0. Pinned by
/// `spawn_search_rings_are_unit_uniform_and_dense_enough`.
const SPAWN_DIRS_32: [Vec2; 32] = [
    Vec2::new(1.0, 0.0),
    Vec2::new(0.98078525, 0.19509032),
    Vec2::new(0.9238795, 0.38268343),
    Vec2::new(0.8314696, 0.55557024),
    Vec2::new(0.70710677, 0.70710677),
    Vec2::new(0.55557024, 0.8314696),
    Vec2::new(0.38268343, 0.9238795),
    Vec2::new(0.19509032, 0.98078525),
    Vec2::new(0.0, 1.0),
    Vec2::new(-0.19509032, 0.98078525),
    Vec2::new(-0.38268343, 0.9238795),
    Vec2::new(-0.55557024, 0.8314696),
    Vec2::new(-0.70710677, 0.70710677),
    Vec2::new(-0.8314696, 0.55557024),
    Vec2::new(-0.9238795, 0.38268343),
    Vec2::new(-0.98078525, 0.19509032),
    Vec2::new(-1.0, 0.0),
    Vec2::new(-0.98078525, -0.19509032),
    Vec2::new(-0.9238795, -0.38268343),
    Vec2::new(-0.8314696, -0.55557024),
    Vec2::new(-0.70710677, -0.70710677),
    Vec2::new(-0.55557024, -0.8314696),
    Vec2::new(-0.38268343, -0.9238795),
    Vec2::new(-0.19509032, -0.98078525),
    Vec2::new(0.0, -1.0),
    Vec2::new(0.19509032, -0.98078525),
    Vec2::new(0.38268343, -0.9238795),
    Vec2::new(0.55557024, -0.8314696),
    Vec2::new(0.70710677, -0.70710677),
    Vec2::new(0.8314696, -0.55557024),
    Vec2::new(0.9238795, -0.38268343),
    Vec2::new(0.98078525, -0.19509032),
];
/// 38 unit directions for the 48 m ring: cos/sin(2πk/38), CCW from +X — spacing
/// 2π·48/38 ≈ 7.94 m of arc (≤ the 8 m coverage target). Exact constants,
/// no runtime trig; near-zero components snapped to 0. Pinned by
/// `spawn_search_rings_are_unit_uniform_and_dense_enough`.
const SPAWN_DIRS_38: [Vec2; 38] = [
    Vec2::new(1.0, 0.0),
    Vec2::new(0.9863613, 0.16459459),
    Vec2::new(0.94581723, 0.32469946),
    Vec2::new(0.87947375, 0.47594738),
    Vec2::new(0.7891405, 0.6142127),
    Vec2::new(0.67728156, 0.7357239),
    Vec2::new(0.54694813, 0.8371665),
    Vec2::new(0.40169543, 0.91577333),
    Vec2::new(0.24548548, 0.9694003),
    Vec2::new(0.082579345, 0.9965845),
    Vec2::new(-0.082579345, 0.9965845),
    Vec2::new(-0.24548548, 0.9694003),
    Vec2::new(-0.40169543, 0.91577333),
    Vec2::new(-0.54694813, 0.8371665),
    Vec2::new(-0.67728156, 0.7357239),
    Vec2::new(-0.7891405, 0.6142127),
    Vec2::new(-0.87947375, 0.47594738),
    Vec2::new(-0.94581723, 0.32469946),
    Vec2::new(-0.9863613, 0.16459459),
    Vec2::new(-1.0, 0.0),
    Vec2::new(-0.9863613, -0.16459459),
    Vec2::new(-0.94581723, -0.32469946),
    Vec2::new(-0.87947375, -0.47594738),
    Vec2::new(-0.7891405, -0.6142127),
    Vec2::new(-0.67728156, -0.7357239),
    Vec2::new(-0.54694813, -0.8371665),
    Vec2::new(-0.40169543, -0.91577333),
    Vec2::new(-0.24548548, -0.9694003),
    Vec2::new(-0.082579345, -0.9965845),
    Vec2::new(0.082579345, -0.9965845),
    Vec2::new(0.24548548, -0.9694003),
    Vec2::new(0.40169543, -0.91577333),
    Vec2::new(0.54694813, -0.8371665),
    Vec2::new(0.67728156, -0.7357239),
    Vec2::new(0.7891405, -0.6142127),
    Vec2::new(0.87947375, -0.47594738),
    Vec2::new(0.94581723, -0.32469946),
    Vec2::new(0.9863613, -0.16459459),
];

/// The outward spawn search, inside the ~50 m budget: ring radii (m) ascending, each paired
/// with its direction table (see [`SPAWN_DIRS_8`] for the density rationale). Past the last
/// ring the request waits until a candidate is free.
const SPAWN_SEARCH_RINGS: [(f32, &[Vec2]); 6] = [
    (8.0, &SPAWN_DIRS_8),
    (16.0, &SPAWN_DIRS_13),
    (24.0, &SPAWN_DIRS_19),
    (32.0, &SPAWN_DIRS_26),
    (40.0, &SPAWN_DIRS_32),
    (48.0, &SPAWN_DIRS_38),
];

/// Each link's last accepted map pick, retained through respawns until disconnect or the next
/// Battle. Sender-derived link keys prevent choosing for another player. Initial joins use
/// automatic placement.
#[derive(Resource, Default)]
struct SpawnOverrides(bevy::platform::collections::HashMap<Entity, Vec2>);

/// Validate a requested spawn XZ: reject non-finite components outright (a NaN would NaN the solver
/// the moment the body is inserted), otherwise clamp into the placeable square
/// ([`spawn_map::spawn_limit`], inside the terrain edge). Pure, so the bound is unit-testable
/// without a world.
fn validate_spawn_request(half_extent: f32, request: SetSpawnPoint) -> Option<Vec2> {
    if !request.x.is_finite() || !request.z.is_finite() {
        return None;
    }
    Some(spawn_map::clamp_to_spawn_limit(
        half_extent,
        Vec2::new(request.x, request.z),
    ))
}

/// A lane's spawn POINT — horizontal, like every spawn definition. Lane 0 is the base point, so
/// the single-client case is unshifted.
pub(crate) fn lane_spawn_xz(base_xz: Vec2, lane: u32, radius: f32) -> Vec2 {
    base_xz + lane_offset(lane, radius * 2.0)
}

/// The harness supplies an exact contact pose: lane offsets are horizontal and never resample Y.
fn lane_spawn_pos(pose: Vec3, lane: u32, radius: f32) -> Vec3 {
    let xz = lane_spawn_xz(pose.xz(), lane, radius);
    Vec3::new(xz.x, pose.y, xz.y)
}

/// The XZ ground a terrain block DENIES a spawn — its world footprint expanded by
/// the data-derived collision `radius` — or `None` for a block
/// that IS ground rather than something standing on it.
///
/// That distinction is the vertical test, taken against the block's OWN ground — the surface under
/// its centre, where `scatter` projected it — so the margin is the construction's own height. A block
/// whose top does not clear that surface is drivable ground (the flat-slab world's 1500 m ground
/// block, a buried course slab) and denies nothing. Against the CANDIDATE's ground the margin
/// would instead be the roof's clearance over `spawn_pos`'s max-over-footprint sample, which the
/// shipped map cuts to 0.08 m on one house.
///
/// The footprint is the [`TerrainBlock`] encoding the analytic track field is built from
/// (`track::terrain::build_track_field`) — the unit cube posed by the Transform — taken as its
/// world AABB: exact for an axis-aligned block, conservative for a yawed one.
fn block_footprint(
    block: &Transform,
    height: Option<&crate::terrain_grid::HeightGrid>,
    radius: f32,
) -> Option<Rect> {
    let (min, max) = TerrainBlock::new(block.translation, block.rotation, block.scale).world_aabb();
    let ground = height.map_or(0.0, |grid| {
        grid.height_at(block.translation.x, block.translation.z)
    });
    (max.y > ground).then(|| {
        Rect::from_corners(Vec2::new(min.x, min.z), Vec2::new(max.x, max.z)).inflate(radius)
    })
}

/// Whether a [`block_footprint`] denies `candidate`. The boundary is OPEN, exactly like the tank
/// test (touching at the required clearance is free). Block clearance uses one body radius;
/// body-to-body clearance uses both radii.
fn footprint_denies(footprint: Rect, candidate: Vec2) -> bool {
    candidate.cmpgt(footprint.min).all() && candidate.cmplt(footprint.max).all()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpawnBlockage {
    Occupied,
    StandingGeometry,
}

/// Search the finite nearby candidate set. Bodies can move or be replaced; standing map
/// footprints cannot clear through waiting for body occupancy, so they require a new pick.
fn resolve_free_spawn_xz(
    half_extent: f32,
    desired: Vec2,
    occupied: &[Vec2],
    blocks: &[Transform],
    height: Option<&crate::terrain_grid::HeightGrid>,
    radius: f32,
) -> Result<(Vec2, bool), SpawnBlockage> {
    let standing: Vec<Rect> = blocks
        .iter()
        .filter_map(|block| block_footprint(block, height, radius))
        .collect();
    let candidates = std::iter::once(desired).chain(SPAWN_SEARCH_RINGS.into_iter().flat_map(
        |(distance, dirs)| {
            dirs.iter().map(move |dir| {
                spawn_map::clamp_to_spawn_limit(half_extent, desired + *dir * distance)
            })
        },
    ));
    let mut clear_ground = false;
    for (index, candidate) in candidates.enumerate() {
        if standing
            .iter()
            .any(|footprint| footprint_denies(*footprint, candidate))
        {
            continue;
        }
        clear_ground = true;
        if !occupied
            .iter()
            .any(|tank| candidate.distance_squared(*tank) < (2.0 * radius).powi(2))
        {
            return Ok((candidate, index != 0));
        }
    }
    Err(if clear_ground {
        SpawnBlockage::Occupied
    } else {
        SpawnBlockage::StandingGeometry
    })
}

/// Parse outside coordinates once at receipt, then admit only picks with a candidate on clear
/// ground. Body occupancy is deliberately absent: it is a retryable placement state.
fn check_spawn_pick(
    half_extent: f32,
    request: SetSpawnPoint,
    blocks: &[Transform],
    height: Option<&crate::terrain_grid::HeightGrid>,
    radius: f32,
) -> Result<Vec2, SpawnPickRefusal> {
    #[cfg(test)]
    SPAWN_PICK_EVALUATIONS.set(SPAWN_PICK_EVALUATIONS.get() + 1);
    let xz = validate_spawn_request(half_extent, request).ok_or(SpawnPickRefusal::NonFinite)?;
    match resolve_free_spawn_xz(half_extent, xz, &[], blocks, height, radius) {
        Ok(_) => Ok(xz),
        Err(SpawnBlockage::StandingGeometry) => Err(SpawnPickRefusal::NoClearGround),
        Err(SpawnBlockage::Occupied) => {
            unreachable!("an empty body occupancy list cannot block clear ground")
        }
    }
}

#[cfg(test)]
thread_local! {
    static SPAWN_PICK_EVALUATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Drain every frame before Lightyear clears unread messages in `Last`.
fn install_spawn_picks(app: &mut App) {
    spawn_map::validate_spawn_map_data();
    app.add_systems(Update, receive_spawn_points);
}

#[derive(SystemParam)]
struct SpawnPickGround<'w> {
    height: Option<Res<'w, crate::terrain_grid::HeightGrid>>,
    map: Res<'w, TerrainMap>,
    obstacles: Res<'w, SpawnObstacles>,
    source: TankSimSource<'w>,
}

/// Evaluate each sending link's newest pick per frame. Superseded clicks do not run placement
/// or change its accepted destination. Nothing in a request names another player.
fn receive_spawn_points(
    mut receivers: Query<(Entity, &mut MessageReceiver<SetSpawnPoint>), With<ClientOf>>,
    mut overrides: ResMut<SpawnOverrides>,
    ground: SpawnPickGround,
    mut own: Query<(&ControlledBy, &mut RespawnState)>,
    mut refused: Local<bevy::platform::collections::HashMap<Entity, SpawnPickRefusal>>,
) {
    let half_extent = spawn_map::world_half_extent(ground.height.as_deref());
    refused.retain(|link, _| receivers.contains(*link));
    overrides.0.retain(|link, _| receivers.contains(*link));
    let mut geometry = None;
    for (link, mut receiver) in &mut receivers {
        if let Some(request) = receiver.receive().last() {
            let (standing, radius) = geometry.get_or_insert_with(|| {
                (
                    ground
                        .map
                        .blocks
                        .iter()
                        .chain(&ground.obstacles.0)
                        .copied()
                        .collect::<Vec<_>>(),
                    ground.source.get().contact_radius(),
                )
            });
            let checked = check_spawn_pick(
                half_extent,
                request,
                standing.as_slice(),
                ground.height.as_deref(),
                *radius,
            );
            // A phase boundary can remove the old root before this frame drains its last messages.
            for (controlled, mut state) in &mut own {
                if controlled.owner == link {
                    let next = RespawnState {
                        pick_refusal: checked.as_ref().err().copied(),
                        ..*state
                    };
                    state.set_if_neq(next);
                }
            }
            match checked {
                Ok(xz) => {
                    refused.remove(&link);
                    overrides.0.insert(link, xz);
                    debug!(
                        "server: link {link} set next spawn to ({:.1}, {:.1})",
                        xz.x, xz.y
                    );
                }
                Err(reason) => {
                    if refused.insert(link, reason) != Some(reason) {
                        warn!(
                            "server: refused spawn pick from link {link}: {reason:?}; previous spawn choice kept"
                        );
                    }
                }
            }
        }
    }
}

/// Symmetric X offsets around the base spawn point. HORIZONTAL, like the spawn points it shifts:
/// a lane fans a joining client sideways, it never decides how high the tank sits.
fn lane_offset(lane: u32, spacing: f32) -> Vec2 {
    let step = lane.div_ceil(2) as f32 * spacing;
    let sign = if lane % 2 == 1 { 1.0 } else { -1.0 };
    Vec2::new(sign * step, 0.0)
}

/// Queues each newly connected client for one owned tank via [`spawn_pending_tanks`].
/// Every client interpolates its hull from the authoritative stream (ADR-0037).
fn handle_new_clients(
    new: Query<(Entity, &RemoteId), (Added<Connected>, With<ClientOf>)>,
    mut pending: ResMut<PendingClients>,
) {
    for (link, remote) in &new {
        info!("server: client connected: {remote} (link {link})");
        if !pending.0.iter().any(|(queued, _)| *queued == link) {
            // The next-Battle boundary may already have queued this newly connected link.
            pending.0.push((link, remote.0));
        }
    }
}

/// Spawns a complete authoritative tank for every queued client. View handles may still be
/// loading; the simulation body comes from the eager blueprint.
#[expect(
    clippy::too_many_arguments,
    reason = "first spawn reads the checked map, mode, live tanks, assets and client identities"
)]
fn spawn_pending_tanks(
    mut pending: ResMut<PendingClients>,
    assets: Res<PendingTankAssets>,
    source: TankSimSource,
    mut lane: ResMut<SpawnLane>,
    mut placement: ResMut<SpawnPlacement>,
    mut ram: Option<ResMut<super::ram::Ram>>,
    manifest: Res<crate::map::MapManifest>,
    tanks: Query<&Position, (With<NetTank>, Without<TankKnockedOut>)>,
    connected: Query<(), With<Connected>>,
    mut combatants: ResMut<CombatantIds>,
    height: Option<Res<crate::terrain_grid::HeightGrid>>,
    mut commands: Commands,
) {
    if pending.0.is_empty() {
        return;
    }
    let content = source.get();
    // The harness's exact resting-contact instrument keeps its authored pose and lanes. Ordinary
    // joins use map points; Y is resolved from the authority's ground at the moment of spawn.
    let harness_pose = if let Some(ram) = ram.as_deref_mut() {
        Some(ram.pose())
    } else {
        harness::spawn_pose()
    };
    let spawn_rot = harness_pose.map_or(Quat::IDENTITY, |(_, rot)| rot);
    let mut live: Vec<Vec2> = tanks.iter().map(|position| position.0.xz()).collect();
    for (link, client_id) in std::mem::take(&mut pending.0) {
        if !connected.contains(link) {
            // A queued join may disconnect while every authored point is too close.
            continue;
        }
        let pos = if let Some((pose, _)) = harness_pose {
            let pos = lane_spawn_pos(pose, lane.0, content.contact_radius());
            lane.0 += 1;
            pos
        } else {
            let Some(xz) = placement.choose(
                &manifest.spawn_points,
                &live,
                2.0 * content.contact_radius(),
            ) else {
                // All points are inside the minimum: keep this join queued until one is free.
                pending.0.push((link, client_id));
                continue;
            };
            spawn_pos(height.as_deref(), xz)
        };
        // Commands are deferred: later clients in this pass must see the point already handed out.
        live.push(pos.xz());
        let combatant = combatants.player(link);
        spawn_player_tank(
            &mut commands,
            content,
            &assets,
            link,
            client_id,
            pos,
            spawn_rot,
            combatant,
            combatants.life(combatant),
        );
    }
}

/// Construct an authoritative player tank. Initial join and respawn share this exact ownership
/// bundle so reacquisition cannot drift from first spawn.
#[expect(
    clippy::too_many_arguments,
    reason = "the authoritative tank's construction inputs: content, assets, link, peer, pose and combatant identity"
)]
fn spawn_player_tank(
    commands: &mut Commands,
    content: TankContent<'_>,
    assets: &PendingTankAssets,
    link: Entity,
    client_id: PeerId,
    spawn_pos: Vec3,
    spawn_rot: Quat,
    combatant: CombatantId,
    life: CombatantLife,
) -> Entity {
    let root = spawn_complete_tank(
        commands,
        content,
        assets.presentation(),
        (
            (
                Name::new("Tank"),
                NetTank,
                combatant,
                life,
                Transform::default(),
                // Authority body role and colliders enter the same command flush.
                RigidBody::Dynamic,
                ActionState::<TankCommand>::default(),
                Position(spawn_pos),
                // Explicit wire pose prevents Avian's required-component placeholder entering history.
                Rotation(spawn_rot),
                ServoAngles::default(),
                NetCrew::default(),
                NetTankStatus::Active,
                LaunchedTurretPose::default(),
                CombatDisclosure::owner(link),
                Replicate::to_clients(NetworkTarget::All),
            ),
            (
                RespawnState::default(),
                // Born empty in the spawn bundle, public, appended by diff.
                HullMarks::default(),
                // Clients build their own local skeleton; replicate only root state.
                DisableReplicateHierarchy,
                // ONE TIMELINE: every client interpolates every hull, the owner's included — the
                // own hull renders and drives from the server stream exactly like an opponent's.
                InterpolationTarget::to_clients(NetworkTarget::All),
                // The owner marker, and the only one the client's game layer keys on: it rides
                // `ControlledBy`'s owner-scoped visibility, not the prediction target.
                ControlledBy {
                    owner: link,
                    lifetime: default(),
                },
            ),
        ),
    );
    info!("server: spawned tank {root} for client {client_id} — owner interpolates");
    root
}

/// An ownerless test-bot tank ([`spawn_bots`]) — scopes [`drive_bot`] to it, and keeps it out of
/// every other tank query the server runs. `home` is the XZ it spawns and respawns at.
#[derive(Component, Clone, Copy)]
struct Bot {
    home: Vec2,
}

/// Bot 0's home spawn POINT — horizontal, like every spawn definition (the old flat-pad
/// `(0, 12)`). Its Y is sampled at spawn time by the shared rule; the fixed `y = 2` pose it used to
/// carry measured 113.65 m underground once the heightmap world landed.
pub(crate) const BOT_SPAWN_XZ: Vec2 = Vec2::new(0.0, 12.0);

/// Bot `index`'s home: the player lanes' fan-out ([`lane_spawn_xz`]) around [`BOT_SPAWN_XZ`], so
/// bot 0 keeps the one bot's home and no two bots share a point.
fn bot_home_xz(index: u32, radius: f32) -> Vec2 {
    lane_spawn_xz(BOT_SPAWN_XZ, index, radius)
}

/// Operator bot count: unset means none; a u32 requests that many. The tracked load runners
/// `scripts/perf/run-online-cost.sh` and `scripts/perf/run-debris-stress.sh` set numeric counts.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
struct BotCount(u32);

impl BotCount {
    fn from_value(value: Option<&str>) -> Result<Self, harness::LeverError> {
        match value.map(str::trim) {
            None => Ok(Self(0)),
            Some(value) => value
                .parse()
                .map(Self)
                .map_err(|_| harness::LeverError::invalid("OVERMATCH_BOT", value, "a u32 count")),
        }
    }
}

/// Spawn the [`BotCount`] ownerless bots once, each at its own [`bot_home_xz`].
fn spawn_bots(
    bots: Query<(), With<Bot>>,
    count: Res<BotCount>,
    assets: Res<PendingTankAssets>,
    source: TankSimSource,
    mut combatants: ResMut<CombatantIds>,
    height: Option<Res<crate::terrain_grid::HeightGrid>>,
    mut commands: Commands,
) {
    if !bots.is_empty() || count.0 == 0 {
        return;
    }
    let content = source.get();
    for index in 0..count.0 {
        let bot = Bot {
            home: bot_home_xz(index, content.contact_radius()),
        };
        let pos = spawn_pos(height.as_deref(), bot.home);
        let combatant = combatants.bot(index);
        let life = combatants.life(combatant);
        let root = spawn_bot_entity(&mut commands, &assets, content, bot, combatant, life, pos);
        info!(
            "server: spawned circling bot {index} tank {root} (OVERMATCH_BOT) at ({:.1}, {:.1}, {:.1})",
            pos.x, pos.y, pos.z
        );
    }
}

/// Construct an ownerless bot, for both initial spawn and respawn. `spawn_pos` is `bot.home`
/// resolved by `terrain_grid::spawn_pos` at both call sites — footprint-lifted onto the terrain,
/// never a constant.
fn spawn_bot_entity(
    commands: &mut Commands,
    assets: &PendingTankAssets,
    content: TankContent<'_>,
    bot: Bot,
    combatant: CombatantId,
    life: CombatantLife,
    spawn_pos: Vec3,
) -> Entity {
    spawn_complete_tank(
        commands,
        content,
        assets.presentation(),
        (
            (
                Name::new("Bot"),
                bot,
                // Name is not replicated; NetBot identifies it to the client HUD.
                super::protocol::NetBot,
                NetTank,
                combatant,
                life,
                Transform::default(),
                RigidBody::Dynamic,
                Position(spawn_pos),
                Rotation(Quat::IDENTITY),
                ServoAngles::default(),
                NetCrew::default(),
            ),
            (
                NetTankStatus::Active,
                LaunchedTurretPose::default(),
                HullMarks::default(),
                CombatDisclosure::hidden(),
                Replicate::to_clients(NetworkTarget::All),
            ),
            (
                DisableReplicateHierarchy,
                // No owner or prediction target: every client interpolates this body.
                InterpolationTarget::to_clients(NetworkTarget::All),
            ),
        ),
    )
}

/// The game's damage model: replace a knocked-out bot once the same scored opening used by
/// players is due. Its combatant and life select the mode's wait on the authority clock.
fn respawn_dead_bots(
    dead: Query<(Entity, &CombatantLife, &Rig, &CombatantId, &Bot), With<TankKnockedOut>>,
    assets: Res<PendingTankAssets>,
    source: TankSimSource,
    gate: RespawnGate,
    height: Option<Res<crate::terrain_grid::HeightGrid>>,
    mut combatants: ResMut<CombatantIds>,
    mut commands: Commands,
) {
    // (root, its Rig.turret) for every bot now due. Capture the turret handle BEFORE despawning the
    // root: if the bot cooked off, `damage::launch_turrets_on_cookoff` stripped the turret's
    // `ChildOf` and made it a free body, so it is NOT a descendant of the root and the recursive
    // root despawn below would miss it — leaking one launched turret per respawn.
    let due: Vec<(Entity, Entity, CombatantId, Bot)> = dead
        .iter()
        .filter(|(_, life, _, combatant, _)| gate.opening(**combatant, **life) == Opening::Open)
        .map(|(root, _, rig, combatant, bot)| (root, rig.turret, *combatant, *bot))
        .collect();
    if due.is_empty() {
        return;
    }
    let content = source.get();
    for (root, turret, combatant, bot) in due {
        // Recursive despawn sweeps the root and its attached rig (children + relationship targets).
        commands.entity(root).despawn();
        // The launched turret, if it detached on cookoff. `try_despawn` is a silent no-op when the
        // turret is still an attached child (already swept above) or otherwise gone — no panic, no
        // double-free, so the one branch covers both the cookoff and crew-loss deaths.
        commands.entity(turret).try_despawn();
        let fresh = spawn_bot_entity(
            &mut commands,
            &assets,
            content,
            bot,
            combatant,
            combatants.life(combatant),
            spawn_pos(height.as_deref(), bot.home),
        );
        info!("server: respawned bot as {fresh} (was {root})");
    }
}

/// The game's damage model: server-authoritative player respawn replaces an owned knocked-out
/// tank only after a client request. Body occupancy keeps a request pending; standing geometry cancels it and reports refusal.
/// The authority's `TankKnockedOut` marker validates eligibility; `ControlledBy` excludes bots. A
/// request before the life's respawn opens (`score::RespawnGate`, the mode's wait after the
/// knockout) is refused: it is dropped, and the client, which shows the time left, sends another.
/// Runs after the command bridge and before the driving snapshot. Both initial
/// placement and automatic respawn use `SpawnPlacement`; the player's persistent spawn-map pick
/// still wins. Recursive root despawn and `tank::sweep_launched_turret_on_root_despawn` sweep the
/// old rig, including any detached turret.
#[expect(
    clippy::too_many_arguments,
    reason = "a respawn sweep spanning dead tanks, remotes, occupancy, assets, sim source, spawn lane, overrides, terrain, obstacles and ids"
)]
fn respawn_player_tanks(
    dead: Query<
        (
            Entity,
            &TankCommand,
            &ControlledBy,
            &CombatantId,
            &CombatantLife,
            &RespawnState,
        ),
        With<TankKnockedOut>,
    >,
    gate: RespawnGate,
    remotes: Query<&RemoteId>,
    // Every tank root's live position — the occupancy set an override spawn checks against
    // (includes dead husks still on the field and the bot; excludes roots swept this pass).
    tanks: Query<(Entity, &Position, Has<TankKnockedOut>), With<NetTank>>,
    assets: Res<PendingTankAssets>,
    source: TankSimSource,
    mut lane: ResMut<SpawnLane>,
    mut placement: ResMut<SpawnPlacement>,
    manifest: Res<crate::map::MapManifest>,
    overrides: Res<SpawnOverrides>,
    // The authority's own ground truth for spawn-map placement; absent in the flat-slab world.
    height: Option<Res<crate::terrain_grid::HeightGrid>>,
    // The world's standing geometry (the scatter's graybox constructions, as blocks, and its
    // constructions of the destruction model, as obstacles): occupancy an override spawn checks
    // against exactly like a live tank. `world` inserts both at startup in every world.
    map: Res<TerrainMap>,
    obstacles: Res<SpawnObstacles>,
    mut combatants: ResMut<CombatantIds>,
    mut commands: Commands,
) {
    // (dead root, owner link, owner client id) for every owned tank that both IS dead and asked to
    // respawn this tick. Resolve the owner's `RemoteId` up front (the `PeerId` the fresh tank must be
    // owned by); an owner link mid-disconnect with no `RemoteId` is skipped rather than
    // respawned to a client that is leaving.
    let requests: Vec<(Entity, Entity, PeerId, CombatantId, RespawnState)> = dead
        .iter()
        .filter(|(_, command, _, _, _, state)| state.waiting || command.respawn)
        .filter(
            |(root, _, _, combatant, life, _)| match gate.opening(**combatant, **life) {
                Opening::Open => true,
                Opening::In { ticks } => {
                    info!("server: respawn of {root} refused — it opens in {ticks} ticks");
                    false
                }
            },
        )
        .filter_map(|(root, _, controlled, combatant, _, waiting)| {
            remotes
                .get(controlled.owner)
                .ok()
                .map(|remote| (root, controlled.owner, remote.0, *combatant, *waiting))
        })
        .collect();
    if requests.is_empty() {
        return;
    }
    let content = source.get();
    // The harness instrument retains its exact contact pose, as on initial join.
    let harness_pose = harness::spawn_pose();
    let spawn_rot = harness_pose.map_or(Quat::IDENTITY, |(_, rot)| rot);
    let harness_pos = harness_pose.map(|(pos, _)| pos);
    // Only successfully replaced roots leave occupancy: a crowded automatic spawn may wait.
    let mut sweeping: Vec<Entity> = Vec::new();
    let mut live: Vec<Vec2> = tanks
        .iter()
        .filter(|(_, _, knocked_out)| !knocked_out)
        .map(|(_, position, _)| position.0.xz())
        .collect();
    // What stands on the map, in the block encoding: the terrain blocks, then the constructions' bounds.
    let standing: Vec<Transform> = map.blocks.iter().chain(&obstacles.0).copied().collect();
    // Occupancy also includes the spots ALREADY HANDED OUT this pass: two clients respawning onto
    // the same click in the same tick would otherwise both read the pre-pass world and overlap.
    let mut placed_this_pass: Vec<Vec2> = Vec::new();
    for (root, link, client_id, combatant, previous) in requests {
        // Keep the player's spawn-map override and its occupancy nudge ahead of automatic choice.
        let picked = overrides.0.get(&link);
        let override_pos = match picked {
            Some(&xz) => {
                let occupied: Vec<Vec2> = tanks
                    .iter()
                    .filter(|(tank, _, _)| *tank != root && !sweeping.contains(tank))
                    .map(|(_, position, _)| position.0.xz())
                    .chain(placed_this_pass.iter().copied())
                    .collect();
                match resolve_free_spawn_xz(
                    spawn_map::world_half_extent(height.as_deref()),
                    xz,
                    &occupied,
                    &standing,
                    height.as_deref(),
                    content.contact_radius(),
                ) {
                    Ok((spot, nudged)) => {
                        if nudged {
                            info!(
                                "server: spawn override ({:.1}, {:.1}) occupied — nudged to ({:.1}, {:.1})",
                                xz.x, xz.y, spot.x, spot.y
                            );
                        }
                        Some(spawn_pos(height.as_deref(), spot))
                    }
                    Err(reason) => {
                        let state = RespawnState {
                            waiting: reason == SpawnBlockage::Occupied,
                            pick_refusal: if reason == SpawnBlockage::StandingGeometry {
                                Some(SpawnPickRefusal::NoClearGround)
                            } else {
                                previous.pick_refusal
                            },
                        };
                        if state != previous {
                            info!(
                                "server: spawn pick ({:.1}, {:.1}): {reason:?}; player may re-pick on the map",
                                xz.x, xz.y
                            );
                            commands.entity(root).insert(state);
                        }
                        continue;
                    }
                }
            }
            None => None,
        };
        let spawn_pos = if let Some(pos) = override_pos {
            pos
        } else if let Some(pose) = harness_pos {
            let pos = lane_spawn_pos(pose, lane.0, content.contact_radius());
            lane.0 += 1;
            pos
        } else {
            let Some(xz) = placement.choose(
                &manifest.spawn_points,
                &live,
                2.0 * content.contact_radius(),
            ) else {
                if !previous.waiting {
                    info!(
                        "server: player {client_id} respawn waiting — every map spawn point is inside the minimum distance"
                    );
                    commands.entity(root).insert(RespawnState {
                        waiting: true,
                        ..previous
                    });
                }
                continue;
            };
            spawn_pos(height.as_deref(), xz)
        };
        placed_this_pass.push(spawn_pos.xz());
        live.push(spawn_pos.xz());
        sweeping.push(root);
        // Recursive despawn sweeps the dead root and its attached rig; the `On<Remove, Rig>` observer
        // handles any cooked-off turret that had detached (see the system doc).
        commands.entity(root).despawn();
        let fresh = spawn_player_tank(
            &mut commands,
            content,
            &assets,
            link,
            client_id,
            spawn_pos,
            spawn_rot,
            combatant,
            combatants.life(combatant),
        );
        commands.entity(fresh).insert(RespawnState {
            waiting: false,
            ..previous
        });
        info!("server: player {client_id} respawn requested — swept {root}, spawned {fresh}");
    }
}

/// Drive the bot in a steady circle AND hold its main gun's trigger: constants written straight into
/// its own `TankCommand` (a required component of `Tank`). The bot carries no `ActionState`, so
/// `bridge_action_state_to_tank_command` (protocol.rs) never touches it. It writes every tick before
/// the parse, as a device does, so the Battle's hold (`battle::hold_commands`) holds it until combat
/// like every tank; from then on it circles and fires for good. Firing makes the bot a self-firing target that exercises remote shot presentation, recoil,
/// and hit-reaction paths without a second client.
fn drive_bot(mut bots: Query<&mut TankCommand, With<Bot>>) {
    for mut command in &mut bots {
        // Gentle constants: enough drive + differential yaw to circle on the flat pad without
        // leaving it or flipping. Everything else stays at `TankCommand::default()`.
        command.throttle = 0.5;
        command.steer = 0.5;
        // Hold primary fire: the gun fires each time its reload completes (so ~one main-gun shot per
        // reload), forward/unaimed as it circles. Aiming at a player is later bot-AI work; for now
        // this is purely the solo test-fire source described above.
        command.fire_primary = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    // ASSUMED fixture body radius for pure placement-policy tests, unrelated to any vehicle.
    const FIXTURE_BODY_RADIUS_M: f32 = 3.0;
    // The shared spawn rule these tests assert the server composes correctly.
    use crate::terrain_grid::spawn_surface_height;

    /// The game's server instrument must refuse invalid HE input through the production
    /// entry point. Operators rely on refusal before a benchmark starts; omitting its installer
    /// leaves the child running instead. The child has its own environment and no screen.
    #[test]
    fn dedicated_server_refuses_bad_he_before_running() {
        use std::process::{Command, Stdio};
        use std::time::Instant;

        const CHILD: &str = "OVERMATCH_TEST_HE_STARTUP_CHILD";
        if std::env::var_os(CHILD).is_some() {
            run();
            panic!("the dedicated-server entry point returned without refusing invalid HE input");
        }
        let mut command =
            Command::new(std::env::current_exe().expect("the test binary has a path"));
        command.args([
            "--exact",
            "net::server::tests::dedicated_server_refuses_bad_he_before_running",
            "--nocapture",
        ]);
        // Hosting is outside this local startup probe; the contract owns its environment keys.
        let contract: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(include_str!("../../backend/contract/server-env.json"))
                .expect("the shipped hosting contract is a JSON object");
        for key in contract.keys() {
            command.env_remove(key);
        }
        let mut child = command
            .env(CHILD, "1")
            .env("OVERMATCH_HE_STRESS", "invalid")
            .env("OVERMATCH_DEBRIS_STRESS", "off")
            .env("OVERMATCH_BOT", "0")
            .env("OVERMATCH_SERVER_BIND", "127.0.0.1")
            .env("OVERMATCH_UNATTENDED", "1")
            .env("OVERMATCH_WINDOW", "headless")
            .env("OVERMATCH_MUTE", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the current test binary can run the dedicated-server entry point");
        // Resolution: bound a missing startup refusal; no gameplay timing is under test.
        let deadline = Instant::now() + Duration::from_secs(5);
        while child
            .try_wait()
            .expect("the startup child can be polled")
            .is_none()
        {
            if Instant::now() >= deadline {
                child
                    .kill()
                    .expect("the running startup child can be killed");
                let output = child
                    .wait_with_output()
                    .expect("the killed startup child is reaped");
                panic!(
                    "dedicated server did not refuse OVERMATCH_HE_STRESS before startup deadline: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            // Resolution: avoid busy-waiting alongside the startup child.
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child
            .wait_with_output()
            .expect("the startup child is reaped");
        let error = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "startup refusal: {error}");
        assert!(
            error.contains("OVERMATCH_HE_STRESS") && error.contains("expected"),
            "the typed refusal must name the malformed operator input: {error}"
        );
    }

    /// Each spawn of a combatant is its next life, counted per combatant: a respawn is never the
    /// life it replaced, and one combatant's spawns do not advance another's.
    #[test]
    fn each_spawn_of_a_combatant_is_its_next_life() {
        let mut world = World::new();
        let mut ids = CombatantIds::default();
        let player = ids.player(world.spawn_empty().id());
        let bot = ids.bot(0);
        assert_eq!(ids.life(player), CombatantLife(0));
        assert_eq!(ids.life(player), CombatantLife(1));
        assert_eq!(ids.life(bot), CombatantLife(0));
        assert_eq!(ids.life(player), CombatantLife(2));
    }

    #[test]
    fn combatant_ids_are_nonzero_unique_and_retained_for_respawn() {
        let mut world = World::new();
        let player_a = world.spawn_empty().id();
        let player_b = world.spawn_empty().id();
        let mut ids = CombatantIds::default();

        let first_life = ids.player(player_a);
        let respawn = ids.player(player_a);
        let other_player = ids.player(player_b);
        let bot_first_life = ids.bot(0);
        let bot_respawn = ids.bot(0);
        let other_bot = ids.bot(1);

        assert_ne!(first_life, CombatantId(0));
        assert_eq!(
            first_life, respawn,
            "a player keeps its match identity on respawn"
        );
        assert_eq!(
            bot_first_life, bot_respawn,
            "a bot keeps its match identity on respawn"
        );
        assert_ne!(first_life, other_player);
        assert_ne!(first_life, bot_first_life);
        assert_ne!(other_player, bot_first_life);
        assert_ne!(bot_first_life, other_bot, "each bot is its own combatant");
    }

    /// Unset binds every interface, as every host has; a named address binds only it; a typo is
    /// refused, since widening it to every interface would expose the run it was meant to keep
    /// local.
    #[test]
    fn the_bind_lever_names_one_address_or_refuses() {
        assert_eq!(bind_ip(None), Ok(Ipv4Addr::UNSPECIFIED));
        assert_eq!(bind_ip(Some("")), Ok(Ipv4Addr::UNSPECIFIED));
        assert_eq!(bind_ip(Some("127.0.0.1")), Ok(Ipv4Addr::LOCALHOST));
        assert!(bind_ip(Some("localhost")).is_err());
    }

    /// Load runners ask for an explicit count. A malformed value must refuse startup instead
    /// of silently choosing one bot or turning the requested load off.
    #[test]
    fn the_bot_lever_accepts_only_an_explicit_count() {
        assert_eq!(BotCount::from_value(None).unwrap(), BotCount(0));
        assert_eq!(BotCount::from_value(Some("1")).unwrap(), BotCount(1));
        assert_eq!(BotCount::from_value(Some(" 4 ")).unwrap(), BotCount(4));
        assert_eq!(BotCount::from_value(Some("0")).unwrap(), BotCount(0));
        for bad in [
            "",
            "on",
            "ON",
            "true",
            "off",
            "ten",
            "-1",
            "1O",
            "4294967296",
        ] {
            assert!(BotCount::from_value(Some(bad)).is_err(), "{bad}");
        }
    }

    /// N bots are N complete tanks: one spawn pass builds every one from the blueprint, each its own
    /// combatant at its own home, bot 0 at the one bot's home; a later pass adds none. The server's
    /// load test reads its tank count off this lever, so a mutant that spawns one bot, or stacks
    /// them on one point, measures a different server than the one asked for.
    ///
    /// Fixture data: the shipped Tiger, the one tank in the tree.
    #[test]
    fn n_bots_spawn_n_tanks_each_its_own_combatant_at_its_own_home() {
        let root = crate::assets::asset_root();
        let spec_ron = std::fs::read_to_string(root.join(crate::tank::TANK_SPEC_PATH))
            .expect("the shipped spec sheet");
        let asset = crate::bake::certify_asset(
            &root.join(crate::tank::TANK_SIM_GLB_PATH),
            &spec_ron,
            &crate::substances::SubstanceRegistry::shipped(),
        )
        .unwrap_or_else(|_| panic!("the shipped sim artifact certifies"));
        let blueprint = crate::bake::TankBlueprint::new(
            crate::tank::TANK_SPEC_PATH.to_owned(),
            asset.geometry,
            asset.spec,
        );
        let track_colliders = crate::track::bump_stop::TrackColliders::of_blueprint(
            &blueprint,
            &root.join(crate::tank::TANK_SIM_GLB_PATH),
        );
        let mut app = App::new();
        app.insert_resource(blueprint)
            .insert_resource(track_colliders)
            .insert_resource(PendingTankAssets {
                spec: Handle::default(),
                scene: None,
                effects: Vec::new(),
            })
            .insert_resource(BotCount(4))
            .init_resource::<CombatantIds>()
            .add_systems(Update, spawn_bots);
        app.update();
        app.update();

        let radius = app
            .world_mut()
            .run_system_once(|source: TankSimSource| source.get().contact_radius())
            .unwrap();
        let world = app.world_mut();
        let bots: Vec<(Vec2, CombatantId)> = world
            .query_filtered::<(&Bot, &CombatantId), With<crate::tank::Tank>>()
            .iter(world)
            .map(|(bot, combatant)| (bot.home, *combatant))
            .collect();
        assert_eq!(bots.len(), 4, "OVERMATCH_BOT=4 is four bot tanks");
        for (i, (home, combatant)) in bots.iter().enumerate() {
            for (other_home, other_combatant) in &bots[i + 1..] {
                assert!(
                    home.distance(*other_home) >= 2.0 * radius,
                    "two bots spawn within one tank's occupancy: {home} and {other_home}",
                );
                assert_ne!(combatant, other_combatant, "two bots share a combatant");
            }
        }
        // Spawn placement relies on the derived radius enclosing every actual child collider,
        // not just a subset of the authored proxies. Check the complete spawned body tree.
        let colliders: Vec<_> = world
            .query::<(
                Entity,
                &avian3d::prelude::Collider,
                &avian3d::prelude::CollisionLayers,
            )>()
            .iter(world)
            // A filter with no possible contacts is a ray-query surface, not a physical body.
            .filter(|(_, _, layers)| layers.filters != avian3d::prelude::LayerMask::NONE)
            .map(|(entity, collider, _)| (entity, collider.clone()))
            .collect();
        assert!(!colliders.is_empty());
        for (entity, collider) in colliders {
            let hull = collider
                .shape()
                .as_convex_polyhedron()
                .expect("tank construction creates convex proxy and running-gear hulls");
            for mut point in hull.to_trimesh().0 {
                let mut ancestor = entity;
                let mut depth = 0_u32;
                while world.get::<crate::tank::Tank>(ancestor).is_none() {
                    point = world
                        .get::<Transform>(ancestor)
                        .expect("spawned collider ancestors carry their authored local transform")
                        .transform_point(point);
                    ancestor = world
                        .get::<ChildOf>(ancestor)
                        .expect("every tank collider is attached to its tank root")
                        .parent();
                    depth += 1;
                }
                // DERIVED roundoff allowance: a fixed number of f32 operations per ancestor.
                let roundoff = radius * f32::EPSILON * (depth + 1) as f32 * 32.0;
                assert!(
                    point.length() <= radius + roundoff,
                    "the construction radius {radius} must enclose spawned collider {entity} at {point:?}"
                );
            }
        }
        assert!(
            bots.iter().any(|(home, _)| *home == BOT_SPAWN_XZ),
            "bot 0 keeps the one bot's home",
        );
    }

    /// A real construction fixture keeps placement tests on the production spawn systems. The
    /// shipped vehicle supplies fixture data only; every assertion is a generic spawn invariant.
    fn placement_app() -> App {
        placement_app_with(None)
    }

    fn placement_app_with(mode: Option<BattleMode>) -> App {
        let root = crate::assets::asset_root();
        let spec_ron = std::fs::read_to_string(root.join(crate::tank::TANK_SPEC_PATH))
            .expect("the shipped spec sheet");
        let asset = crate::bake::certify_asset(
            &root.join(crate::tank::TANK_SIM_GLB_PATH),
            &spec_ron,
            &crate::substances::SubstanceRegistry::shipped(),
        )
        .unwrap_or_else(|_| panic!("the shipped sim artifact certifies"));
        let blueprint = crate::bake::TankBlueprint::new(
            crate::tank::TANK_SPEC_PATH.to_owned(),
            asset.geometry,
            asset.spec,
        );
        let colliders = crate::track::bump_stop::TrackColliders::of_blueprint(
            &blueprint,
            &root.join(crate::tank::TANK_SIM_GLB_PATH),
        );
        let mut app = App::new();
        // Lightyear's replicated-component removal hooks read this registry on despawn.
        app.init_resource::<bevy_replicon::server::visibility::registry::FilterRegistry>()
            .insert_resource(blueprint)
            .insert_resource(colliders)
            .insert_resource(PendingTankAssets {
                spec: Handle::default(),
                scene: None,
                effects: Vec::new(),
            })
            .insert_resource(crate::map::tests::fixture_manifest(750.0, 0.0, 50.0))
            .insert_resource(SpawnPlacement::new(BattleMode::shipped().spawn))
            // Fixture world: no standing blocks or constructions.
            .insert_resource(crate::ground_field::GroundField::unpainted(
                &crate::ground_types::GroundTypes::load_shipped(),
            ))
            .insert_resource(TerrainMap {
                revision: 0,
                blocks: Vec::new(),
            })
            .init_resource::<SpawnObstacles>()
            .init_resource::<PendingClients>()
            .init_resource::<SpawnOverrides>()
            .init_resource::<SpawnLane>()
            .init_resource::<CombatantIds>()
            .init_resource::<crate::ShotClock>()
            .add_systems(Update, player_respawn_system().after(spawn_pending_tanks));
        install_client_spawns(&mut app);
        if let Some(mode) = mode {
            install_battle(&mut app, mode, TICK);
        } else {
            // Narrow placement/respawn probes hold combat open without advancing the Battle clock.
            super::super::score::mount(&mut app, shipped_wait());
            app.world_mut().spawn((
                super::super::score::Scoreboard::default(),
                BattleState::combat_fixture(),
            ));
        }
        app
    }

    /// Exercise receipt through Lightyear's local typed-message delivery and the shipped installer.
    fn spawn_pick_app() -> App {
        let mut app = placement_app();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin));
        app.add_plugins(lightyear::prelude::client::ClientPlugins {
            tick_duration: TICK,
        });
        app.register_message::<SetSpawnPoint>();
        app.add_channel::<super::super::protocol::SpawnChannel>(ChannelSettings::default());
        install_spawn_picks(&mut app);
        app.finish();
        app.cleanup();
        app
    }

    fn pick_link(app: &mut App, peer: u64) -> Entity {
        app.world_mut()
            .spawn((
                ClientOf,
                Connected,
                RemoteId(PeerId::Netcode(peer)),
                lightyear::connection::host::HostClient { buffer: Vec::new() },
                MessageSender::<SetSpawnPoint>::default(),
                MessageReceiver::<SetSpawnPoint>::default(),
            ))
            .id()
    }

    fn queue_pick(app: &mut App, link: Entity, point: Vec2) {
        app.world_mut()
            .get_mut::<MessageSender<SetSpawnPoint>>(link)
            .unwrap()
            .send::<super::super::protocol::SpawnChannel>(SetSpawnPoint {
                x: point.x,
                z: point.y,
            });
    }

    fn send_pick(app: &mut App, link: Entity, point: Vec2) {
        queue_pick(app, link, point);
        app.update(); // Local delivery runs in Last, after the receiver drain.
        app.update();
    }

    /// A refusal reaches only the sender's root and cannot replace its accepted destination.
    /// The owner relies on this state for the map and respawn UI; another owner's state stays put.
    #[test]
    fn spawn_pick_receipt_updates_only_its_owner_and_retains_the_accepted_point() {
        let mut app = spawn_pick_app();
        let link = pick_link(&mut app, 1);
        let other_link = pick_link(&mut app, 2);
        let root = app
            .world_mut()
            .spawn((
                ControlledBy {
                    owner: link,
                    lifetime: Default::default(),
                },
                RespawnState::default(),
            ))
            .id();
        let other = app
            .world_mut()
            .spawn((
                ControlledBy {
                    owner: other_link,
                    lifetime: Default::default(),
                },
                RespawnState {
                    waiting: true,
                    pick_refusal: None,
                },
            ))
            .id();
        let accepted = Vec2::new(100.0, 0.0); // Fixture points, one outside the later footprint.
        let refused = Vec2::new(-100.0, 0.0);
        send_pick(&mut app, link, accepted);
        assert_eq!(
            app.world().resource::<SpawnOverrides>().0.get(&link),
            Some(&accepted)
        );
        app.world_mut()
            .resource_mut::<SpawnObstacles>()
            .0
            .push(Transform::from_xyz(refused.x, 1.0, refused.y).with_scale(Vec3::splat(120.0))); // Fixture footprint covers every nearby candidate, not the accepted point.
        send_pick(&mut app, link, refused);
        assert_eq!(
            app.world().get::<RespawnState>(root),
            Some(&RespawnState {
                waiting: false,
                pick_refusal: Some(SpawnPickRefusal::NoClearGround)
            })
        );
        assert_eq!(
            app.world().get::<RespawnState>(other),
            Some(&RespawnState {
                waiting: true,
                pick_refusal: None
            })
        );
        assert_eq!(
            app.world().resource::<SpawnOverrides>().0.get(&link),
            Some(&accepted)
        );
        assert!(
            !app.world()
                .resource::<SpawnOverrides>()
                .0
                .contains_key(&other_link)
        );
        send_pick(&mut app, link, Vec2::new(f32::NAN, 0.0));
        assert_eq!(
            app.world().get::<RespawnState>(root),
            Some(&RespawnState {
                waiting: false,
                pick_refusal: Some(SpawnPickRefusal::NonFinite)
            })
        );
        send_pick(&mut app, link, accepted);
        assert_eq!(
            app.world().get::<RespawnState>(root),
            Some(&RespawnState::default())
        );
    }

    /// Accepted destinations belong to their sending link. Disconnect churn must remove each
    /// departed player's choice while a connected player's retained destination stays available.
    #[test]
    fn spawn_choices_end_with_the_sending_link() {
        let mut app = spawn_pick_app();
        let staying = pick_link(&mut app, 1);
        let point = Vec2::new(100.0, 0.0); // Fixture clear destination.
        send_pick(&mut app, staying, point);
        for peer in 2..5 {
            let leaving = pick_link(&mut app, peer);
            send_pick(&mut app, leaving, point);
            assert!(
                app.world()
                    .resource::<SpawnOverrides>()
                    .0
                    .contains_key(&leaving)
            );
            app.world_mut().despawn(leaving);
            app.update();
            let choices = &app.world().resource::<SpawnOverrides>().0;
            assert_eq!(choices.len(), 1, "gone links retain no spawn choice");
            assert_eq!(choices.get(&staying), Some(&point));
        }
    }

    /// A peer can supersede many choices in one frame; only the newest may spend solver work
    /// or alter the retained destination. Evaluating every earlier valid pick breaks this contract.
    #[test]
    fn a_spawn_pick_flood_supersedes_earlier_choices_before_admission() {
        let mut app = spawn_pick_app();
        // Keep the admission counter on this test's thread.
        app.edit_schedule(Update, |schedule| {
            schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        let link = pick_link(&mut app, 1);
        let root = app
            .world_mut()
            .spawn((
                ControlledBy {
                    owner: link,
                    lifetime: Default::default(),
                },
                RespawnState::default(),
            ))
            .id();
        // Fixture in-map points and a footprint covering the nearby search at the refused point.
        let retained = Vec2::new(200.0, 0.0);
        let superseded = Vec2::new(100.0, 0.0);
        let refused = Vec2::new(-100.0, 0.0);
        send_pick(&mut app, link, retained);
        app.world_mut()
            .resource_mut::<SpawnObstacles>()
            .0
            .push(Transform::from_xyz(refused.x, 1.0, refused.y).with_scale(Vec3::splat(120.0)));
        SPAWN_PICK_EVALUATIONS.set(0);
        for _ in 0..1024 {
            // Fixture flood delivered in one frame's drain.
            queue_pick(&mut app, link, superseded);
            queue_pick(&mut app, link, refused);
        }
        app.update();
        app.update();
        assert_eq!(
            SPAWN_PICK_EVALUATIONS.get(),
            1,
            "only the newest pick spends admission work"
        );
        assert_eq!(
            app.world().resource::<SpawnOverrides>().0.get(&link),
            Some(&retained)
        );
        assert_eq!(
            app.world().get::<RespawnState>(root).unwrap().pick_refusal,
            Some(SpawnPickRefusal::NoClearGround)
        );
        send_pick(&mut app, link, superseded);
        assert_eq!(
            app.world().resource::<SpawnOverrides>().0.get(&link),
            Some(&superseded)
        );
        assert_eq!(
            app.world().get::<RespawnState>(root).unwrap().pick_refusal,
            None
        );
    }

    /// The game's damage model: a refused map pick costs neither a retained destination nor an
    /// outstanding automatic respawn. The owner can press R after a living misclick or keep waiting.
    #[test]
    fn refused_picks_preserve_the_previous_destination_and_pending_respawn() {
        for pending in [false, true] {
            let mut app = spawn_pick_app();
            let link = pick_link(&mut app, 1);
            app.world_mut()
                .resource_mut::<PendingClients>()
                .0
                .push((link, PeerId::Netcode(1)));
            app.update();
            let root = app
                .world_mut()
                .query_filtered::<Entity, With<ControlledBy>>()
                .single(app.world())
                .unwrap();
            let accepted = Vec2::new(100.0, 0.0); // Fixture points separated beyond the search radius.
            let refused = Vec2::new(-100.0, 0.0);
            send_pick(&mut app, link, accepted);
            app.world_mut().resource_mut::<SpawnObstacles>().0.push(
                Transform::from_xyz(refused.x, 1.0, refused.y).with_scale(Vec3::splat(120.0)),
            ); // Fixture standing footprint covers every candidate near the refused pick.
            let mut bodies = Vec::new();
            if pending {
                for point in
                    std::iter::once(accepted).chain(SPAWN_SEARCH_RINGS.into_iter().flat_map(
                        |(radius, dirs)| dirs.iter().map(move |dir| accepted + *dir * radius),
                    ))
                {
                    bodies.push(
                        app.world_mut()
                            .spawn((NetTank, Position(Vec3::new(point.x, 0.0, point.y))))
                            .id(),
                    );
                }
                knock_out(&mut app, root);
                advance(&mut app, shipped_wait().ticks());
                app.world_mut()
                    .get_mut::<TankCommand>(root)
                    .unwrap()
                    .respawn = true;
                app.update();
                app.world_mut()
                    .get_mut::<TankCommand>(root)
                    .unwrap()
                    .respawn = false;
            }
            send_pick(&mut app, link, refused);
            if pending {
                for body in bodies {
                    app.world_mut().despawn(body);
                }
            } else {
                knock_out(&mut app, root);
                advance(&mut app, shipped_wait().ticks());
                app.world_mut()
                    .get_mut::<TankCommand>(root)
                    .unwrap()
                    .respawn = true;
            }
            app.update();
            assert!(
                app.world().get_entity(root).is_err(),
                "a refused pick must not disable R or cancel an automatic request"
            );
            let position = app
                .world_mut()
                .query_filtered::<&Position, With<ControlledBy>>()
                .single(app.world())
                .unwrap();
            assert_eq!(
                position.0.xz(),
                accepted,
                "the accepted destination remains reachable"
            );
            let state = app
                .world_mut()
                .query_filtered::<&RespawnState, With<ControlledBy>>()
                .single(app.world())
                .unwrap();
            assert_eq!(
                state.pick_refusal,
                Some(SpawnPickRefusal::NoClearGround),
                "the replacement root preserves the map's refused-marker feedback"
            );
        }
    }

    /// The game's match flow: the first arrival starts combat with one life per link. Results
    /// sweep those lives; the next Battle resets scores and places every connected player at once.
    /// Unchanged ticks must leave replicated score and phase snapshots clean.
    #[test]
    fn immediate_combat_places_every_player_once_and_keeps_unchanged_snapshots_clean() {
        use super::super::battle::BattlePhase;
        use super::super::score::Scoreboard;
        let mode = BattleMode::parse(include_str!("fixtures/battle-short.ron"));
        let mut app = placement_app_with(Some(mode));
        app.insert_resource(crate::map::tests::shipped_manifest());
        app.update(); // Production Startup owns the scoreboard.
        let board = app
            .world_mut()
            .query_filtered::<Entity, With<Scoreboard>>()
            .single(app.world())
            .expect("one fixture board");
        let link = app
            .world_mut()
            .spawn((RemoteId(PeerId::Netcode(1)), Connected, ClientOf))
            .id();
        app.world_mut().run_schedule(FixedUpdate);
        app.update();
        assert!(
            app.world().get::<BattleState>(board).unwrap().is_combat(),
            "the first arrival starts combat on this tick"
        );
        let owned = |app: &mut App| -> Vec<(Entity, Entity)> {
            app.world_mut()
                .query_filtered::<(Entity, &ControlledBy), With<NetTank>>()
                .iter(app.world())
                .map(|(root, owner)| (root, owner.owner))
                .collect()
        };
        let first = owned(&mut app);
        assert_eq!(first.len(), 1, "one life for the first arrival");
        assert_eq!(first[0].1, link);
        // Fixture peer joins open combat.
        app.world_mut()
            .spawn((RemoteId(PeerId::Netcode(2)), Connected, ClientOf));
        advance(&mut app, 1);
        app.world_mut().run_schedule(FixedUpdate);
        app.update();
        let roots: Vec<_> = app
            .world_mut()
            .query_filtered::<(Entity, &ControlledBy, &CombatantId), With<NetTank>>()
            .iter(app.world())
            .map(|(root, owner, id)| (root, owner.owner, *id))
            .collect();
        assert_eq!(roots.len(), 2, "exactly one tank per connected link");
        assert_eq!(
            app.world().get::<Scoreboard>(board).unwrap().0.len(),
            roots.len(),
            "each combat spawn opens its standing once"
        );
        let &(old_root, _, combatant) = roots
            .iter()
            .find(|(_, owner, _)| *owner == link)
            .expect("first player's tank");
        assert_eq!(
            old_root, first[0].0,
            "a late join cannot replace the first player's life"
        );
        app.world_mut().clear_trackers();
        advance(&mut app, 1);
        app.world_mut().run_schedule(FixedUpdate);
        app.update();
        assert_eq!(
            app.world_mut()
                .query_filtered::<(), Changed<BattleState>>()
                .iter(app.world())
                .count(),
            0
        );
        assert_eq!(
            app.world_mut()
                .query_filtered::<(), Changed<Scoreboard>>()
                .iter(app.world())
                .count(),
            0
        );
        let BattlePhase::Combat {
            ends_at: combat_end,
            ..
        } = app.world().get::<BattleState>(board).expect("Battle").phase
        else {
            panic!("combat started");
        };
        // FIXTURE: a combat score the next Battle must clear.
        for standing in &mut app.world_mut().get_mut::<Scoreboard>(board).unwrap().0 {
            standing.knockouts = 3;
            standing.losses = 1;
        }
        let now = app.world().resource::<crate::ShotClock>().0;
        advance(&mut app, now.remaining_until(combat_end));
        app.world_mut().run_schedule(FixedUpdate);
        app.update();
        assert!(
            app.world().get_entity(old_root).is_err(),
            "results sweeps the combat life"
        );
        assert_eq!(
            app.world_mut()
                .query_filtered::<(), With<NetTank>>()
                .iter(app.world())
                .count(),
            0
        );
        let BattlePhase::Results {
            ends_at: results_end,
            ..
        } = app.world().get::<BattleState>(board).expect("Battle").phase
        else {
            panic!("results started");
        };
        advance(&mut app, combat_end.remaining_until(results_end));
        app.world_mut().run_schedule(FixedUpdate);
        app.update();
        assert!(
            app.world()
                .get::<Scoreboard>(board)
                .unwrap()
                .0
                .iter()
                .all(|standing| standing.knockouts == 0 && standing.losses == 0),
            "the new combat clears the frozen scores"
        );
        assert!(
            app.world().get::<BattleState>(board).unwrap().is_combat(),
            "connected players start the next combat on the results boundary"
        );
        assert!(app.world().get::<Connected>(link).is_some());
        let fresh: Vec<_> = app
            .world_mut()
            .query_filtered::<(Entity, &CombatantId, &CombatantLife), With<NetTank>>()
            .iter(app.world())
            .map(|(root, id, life)| (root, *id, *life))
            .collect();
        assert_eq!(fresh.len(), 2);
        let &(root, _, life) = fresh
            .iter()
            .find(|(_, id, _)| *id == combatant)
            .expect("first player returns");
        assert_ne!(root, old_root);
        assert_eq!(life, CombatantLife(1));
    }

    /// The game's match flow: the production phase clock must release every command on the
    /// first arrival's tick. A hold ordered before the clock would delay combat input by one tick.
    #[test]
    fn first_arrival_releases_every_command_on_that_tick() {
        use crate::command::{AimIntent, ParsedTankCommand};
        // FIXTURE: a live player command, written where the input bridge writes it.
        fn author(mut tanks: Query<(&mut TankCommand, &mut ParsedTankCommand)>) {
            for (mut raw, mut parsed) in &mut tanks {
                *raw = TankCommand {
                    throttle: 1.0,
                    steer: -0.5,
                    fire_primary: true,
                    fire_secondary: true,
                    aim: Some(AimIntent::World(Vec3::new(10.0, 0.0, -40.0))),
                    range: 400.0,
                    repair: true,
                    ..default()
                };
                *parsed = ParsedTankCommand::try_from(*raw).expect("a finite fixture command");
            }
        }
        let mut app = App::new();
        let mode = BattleMode::parse(include_str!("fixtures/battle-short.ron"));
        install_battle(&mut app, mode, TICK);
        super::super::battle::install_hold(&mut app);
        app.init_resource::<crate::ShotClock>().add_systems(
            FixedUpdate,
            author.in_set(super::super::protocol::InputBridge),
        );
        let board = app
            .world_mut()
            .spawn((
                super::super::score::Scoreboard::default(),
                BattleState::default(),
            ))
            .id();
        let tank = app.world_mut().spawn(TankCommand::default()).id();
        app.world_mut().run_schedule(FixedUpdate);
        assert!(
            app.world().get::<ParsedTankCommand>(tank).unwrap().held(),
            "the empty server holds input"
        );
        app.world_mut()
            .spawn((RemoteId(PeerId::Netcode(1)), Connected, ClientOf));
        app.world_mut().run_schedule(FixedUpdate);
        assert!(app.world().get::<BattleState>(board).unwrap().is_combat());
        let parsed = *app.world().get::<ParsedTankCommand>(tank).unwrap();
        assert!(!parsed.held());
        assert!(parsed.aim().is_some());
        assert_eq!(parsed.throttle, 1.0);
        assert!(parsed.fire_primary && parsed.fire_secondary && parsed.repair);
    }

    /// The shipped mode's respawn wait on the server's tick.
    fn shipped_wait() -> RespawnWait {
        RespawnWait::new(BattleMode::shipped().respawn_wait, TICK)
    }

    /// Knock `root` out as the authority's transition does: the label, then its scored fact, at
    /// the current [`crate::ShotClock`] tick.
    fn knock_out(app: &mut App, root: Entity) {
        app.world_mut().entity_mut(root).insert(TankKnockedOut {
            reason: crate::damage::KnockoutReason::CrewLoss,
        });
        app.world_mut().trigger(crate::damage::TankKnockout {
            tank: root,
            cause: crate::damage::KnockoutCause::NoKeyedRound,
        });
    }

    /// Advance the authority tick by `ticks`.
    fn advance(app: &mut App, ticks: u32) {
        let mut clock = app.world_mut().resource_mut::<crate::ShotClock>();
        clock.0 = clock.0.after(crate::sim_tick::TickSpan::of(ticks));
    }

    /// The game's damage model supplies respawn eligibility. The mode's wait opens on the server:
    /// an earlier request is refused; a request at the opening replaces the root before driving
    /// snapshots it. The client's countdown and server gate share that opening. Removing the
    /// lifecycle edge leaves the driving reader looking at the old root's deferred replacement.
    #[test]
    fn a_respawn_request_is_refused_before_the_wait_and_granted_after() {
        #[derive(Resource, Default)]
        struct DrivingSnapshot(Vec<Entity>);
        let mut app = placement_app();
        app.init_resource::<DrivingSnapshot>().add_systems(
            Update,
            (|tanks: Query<Entity, With<ControlledBy>>, mut snapshot: ResMut<DrivingSnapshot>| {
                snapshot.0.clear();
                snapshot.0.extend(tanks.iter());
            })
            .in_set(SimPhase::DrivingForces),
        );
        let peer = PeerId::Netcode(1);
        let link = app.world_mut().spawn((RemoteId(peer), Connected)).id();
        app.world_mut()
            .resource_mut::<PendingClients>()
            .0
            .push((link, peer));
        app.update();
        let root = app
            .world_mut()
            .query_filtered::<Entity, With<ControlledBy>>()
            .single(app.world())
            .expect("the joined player has a tank");
        knock_out(&mut app, root);
        let request = |app: &mut App, root: Entity| {
            app.world_mut()
                .get_mut::<TankCommand>(root)
                .expect("tank command")
                .respawn = true;
            app.update();
        };
        let wait = shipped_wait().ticks();
        assert!(wait > 0, "the shipped mode waits before a respawn");
        advance(&mut app, wait - 1);
        request(&mut app, root);
        assert!(
            app.world().get_entity(root).is_ok(),
            "a request one tick before the opening is refused"
        );
        app.world_mut()
            .get_mut::<TankCommand>(root)
            .expect("tank command")
            .respawn = false;
        advance(&mut app, 1);
        request(&mut app, root);
        assert!(
            app.world().get_entity(root).is_err(),
            "a request at the opening replaces the knocked-out tank"
        );
        let replacement = app
            .world_mut()
            .query_filtered::<Entity, With<ControlledBy>>()
            .single(app.world())
            .expect("one replacement player tank");
        assert_eq!(
            app.world().resource::<DrivingSnapshot>().0,
            vec![replacement],
            "the first driving reader sees the complete replacement in this same update"
        );
    }

    /// The game's damage model: bots and players are combatants under the same mode wait.
    /// Each old life survives until the opening, then exactly one replacement keeps its id. Repeated
    /// lives catch a hidden limit; a second mode wait catches a wait compiled into the bot path.
    #[test]
    fn a_bot_respawns_at_its_scored_opening_and_never_before() {
        for (label, mode) in [
            ("shipped", BattleMode::shipped()),
            (
                "fixture",
                BattleMode::parse(include_str!("fixtures/battle-short.ron")),
            ),
        ] {
            let wait = RespawnWait::new(mode.respawn_wait, TICK);
            let mut app = placement_app();
            app.insert_resource(wait)
                .insert_resource(BotCount(1))
                .add_systems(Update, (spawn_bots, respawn_dead_bots).chain());
            app.update();
            let mut root = app
                .world_mut()
                .query_filtered::<Entity, With<Bot>>()
                .single(app.world())
                .expect("one fixture bot");
            let combatant = *app
                .world()
                .get::<CombatantId>(root)
                .expect("spawn identity");
            for replacement in 1..=3 {
                // Fixture repetitions, beyond a two-respawn limit.
                knock_out(&mut app, root);
                advance(&mut app, wait.ticks() - 1);
                app.update();
                assert!(
                    app.world().get_entity(root).is_ok(),
                    "no bot respawn before the opening"
                );
                advance(&mut app, 1);
                app.update();
                assert!(
                    app.world().get_entity(root).is_err(),
                    "the old life is swept at the opening"
                );
                let (fresh, id, life) = app
                    .world_mut()
                    .query_filtered::<(Entity, &CombatantId, &CombatantLife), With<Bot>>()
                    .single(app.world())
                    .expect("exactly one replacement bot");
                assert_ne!(fresh, root);
                assert_eq!(*id, combatant);
                assert_eq!(*life, CombatantLife(replacement));
                root = fresh;
            }
            let respawns = app.world().get::<CombatantLife>(root).unwrap().0;
            println!(
                "MEASURED headless bot probe: mode={label}, respawns={respawns}, early_respawns=0; mode_wait_s={} (mode data)",
                mode.respawn_wait.as_secs_f32()
            );
        }
    }

    /// The game's damage model: automatic joins and respawns share the minimum, ignore knocked-out
    /// tanks, and retry a blocked request after its input edge is gone. A player's map pick still
    /// wins. Mutants: lane placement bypasses the minimum; counting the old root blocks an alone
    /// respawn; losing the latch requires a second request; applying the rule to a pick rejects it.
    #[test]
    fn automatic_spawns_wait_and_retry_but_player_picks_still_win() {
        let mut app = placement_app();
        let client_id = PeerId::Netcode(1);
        let link = app.world_mut().spawn((RemoteId(client_id), Connected)).id();
        // Fixture identity: every server tank that can be knocked out is born with one.
        let blocker = app
            .world_mut()
            .spawn((
                NetTank,
                Position(Vec3::ZERO),
                CombatantId(99),
                CombatantLife(0),
            ))
            .id();
        app.world_mut()
            .resource_mut::<PendingClients>()
            .0
            .push((link, client_id));
        app.update();
        assert_eq!(
            app.world().resource::<PendingClients>().0.len(),
            1,
            "the only authored point is too close to a live tank"
        );
        app.world_mut().entity_mut(blocker).insert(TankKnockedOut {
            reason: crate::damage::KnockoutReason::CrewLoss,
        });
        app.update();
        assert!(app.world().resource::<PendingClients>().0.is_empty());
        let root = app
            .world_mut()
            .query_filtered::<Entity, With<ControlledBy>>()
            .single(app.world())
            .expect("the queued player now has a tank");
        assert_eq!(
            app.world()
                .get::<Position>(root)
                .expect("spawn pose")
                .0
                .xz(),
            Vec2::ZERO
        );
        app.world_mut()
            .entity_mut(blocker)
            .remove::<TankKnockedOut>();
        knock_out(&mut app, root);
        advance(&mut app, shipped_wait().ticks());
        app.world_mut().entity_mut(root).insert(TankCommand {
            respawn: true,
            ..default()
        });
        app.update();
        assert_eq!(
            app.world().get::<RespawnState>(root),
            Some(&RespawnState {
                waiting: true,
                pick_refusal: None
            })
        );
        app.world_mut()
            .get_mut::<TankCommand>(root)
            .expect("tank command")
            .respawn = false;
        app.world_mut().entity_mut(blocker).insert(TankKnockedOut {
            reason: crate::damage::KnockoutReason::CrewLoss,
        });
        app.update();
        assert!(
            app.world().get_entity(root).is_err(),
            "the pending request replaced its old root"
        );
        let fresh = app
            .world_mut()
            .query_filtered::<Entity, With<ControlledBy>>()
            .single(app.world())
            .expect("one replacement player tank");
        let picked = Vec2::new(50.0, 0.0); // ASSUMED fixture point, inside the mode minimum.
        app.world_mut()
            .resource_mut::<SpawnOverrides>()
            .0
            .insert(link, picked);
        app.world_mut()
            .entity_mut(blocker)
            .remove::<TankKnockedOut>();
        knock_out(&mut app, fresh);
        advance(&mut app, shipped_wait().ticks());
        app.world_mut().entity_mut(fresh).insert(TankCommand {
            respawn: true,
            ..default()
        });
        app.update();
        let pos = app
            .world_mut()
            .query_filtered::<&Position, With<ControlledBy>>()
            .single(app.world())
            .expect("the override still spawns a replacement")
            .0
            .xz();
        assert_eq!(
            pos, picked,
            "the player's free pick wins over the automatic minimum"
        );
        assert_eq!(
            app.world().resource::<SpawnOverrides>().0[&link],
            picked,
            "the player's choice persists after use"
        );
    }

    /// The game's damage model: body occupancy keeps the old life pending without another input
    /// edge; standing footprints refuse the pick and never auto-spawn when that refusal clears.
    #[test]
    fn blocked_map_picks_wait_only_for_body_occupancy() {
        for permanent in [false, true] {
            let mut app = spawn_pick_app();
            let client_id = PeerId::Netcode(1);
            let link = pick_link(&mut app, 1);
            app.world_mut()
                .resource_mut::<PendingClients>()
                .0
                .push((link, client_id));
            app.update();
            let root = app
                .world_mut()
                .query_filtered::<Entity, With<ControlledBy>>()
                .single(app.world())
                .unwrap();
            let picked = Vec2::new(50.0, 0.0); // Fixture in-map point.
            let mut bodies = Vec::new();
            if permanent {
                app.world_mut()
                    .resource_mut::<SpawnObstacles>()
                    .0
                    .push(Transform::from_translation(Vec3::Y).with_scale(Vec3::splat(1000.0))); // Fixture footprint covers the finite search.
            } else {
                let points =
                    std::iter::once(picked).chain(SPAWN_SEARCH_RINGS.into_iter().flat_map(
                        |(radius, dirs)| dirs.iter().map(move |dir| picked + *dir * radius),
                    ));
                for point in points {
                    bodies.push(
                        app.world_mut()
                            .spawn((NetTank, Position(Vec3::new(point.x, 0.0, point.y))))
                            .id(),
                    );
                }
            }
            app.world_mut()
                .resource_mut::<SpawnOverrides>()
                .0
                .insert(link, picked);
            knock_out(&mut app, root);
            advance(&mut app, shipped_wait().ticks());
            app.world_mut()
                .get_mut::<TankCommand>(root)
                .unwrap()
                .respawn = true;
            app.update();
            let expected = if permanent {
                RespawnState {
                    waiting: false,
                    pick_refusal: Some(SpawnPickRefusal::NoClearGround),
                }
            } else {
                RespawnState {
                    waiting: true,
                    pick_refusal: None,
                }
            };
            assert_eq!(app.world().get::<RespawnState>(root), Some(&expected));
            app.world_mut()
                .get_mut::<TankCommand>(root)
                .unwrap()
                .respawn = false;
            app.update();
            assert!(app.world().get_entity(root).is_ok());
            app.world_mut().resource_mut::<SpawnObstacles>().0.clear();
            for body in bodies {
                app.world_mut().entity_mut(body).despawn();
            }
            app.update();
            if permanent {
                assert!(
                    app.world().get_entity(root).is_ok(),
                    "a refused request cannot become an automatic respawn"
                );
                send_pick(&mut app, link, picked);
                app.world_mut()
                    .get_mut::<TankCommand>(root)
                    .unwrap()
                    .respawn = true;
                app.update();
            }
            assert!(app.world().get_entity(root).is_err());
            let position = app
                .world_mut()
                .query_filtered::<&Position, With<ControlledBy>>()
                .single(app.world())
                .unwrap();
            assert_eq!(position.0.xz(), picked);
        }
    }

    /// Receipt refuses nonfinite coordinates and a search wholly covered by standing footprints.
    #[test]
    fn spawn_pick_admission_refuses_nonfinite_and_fully_blocked_ground() {
        let half = crate::terrain_grid::FIXTURE_EXTENT.half_extent();
        let blocked = [Transform::from_translation(Vec3::Y).with_scale(Vec3::splat(1000.0))]; // Fixture footprint.
        let pick = SetSpawnPoint { x: 0.0, z: 0.0 };
        let refused = check_spawn_pick(half, pick, &blocked, None, FIXTURE_BODY_RADIUS_M);
        assert_eq!(refused, Err(SpawnPickRefusal::NoClearGround));
        let valid = check_spawn_pick(half, pick, &[], None, FIXTURE_BODY_RADIUS_M);
        assert_eq!(valid, Ok(Vec2::ZERO));
        let malformed = check_spawn_pick(
            half,
            SetSpawnPoint {
                x: f32::NAN,
                z: 0.0,
            },
            &[],
            None,
            FIXTURE_BODY_RADIUS_M,
        );
        assert_eq!(malformed, Err(SpawnPickRefusal::NonFinite));
    }

    /// Commands are deferred, so two queued joins must include earlier placements in the pass's
    /// live set. A disconnected waiting player must not receive an orphan tank later.
    #[test]
    fn joining_players_see_same_pass_spawns_and_disconnected_waiters_are_removed() {
        let mut app = placement_app();
        for id in 1..=2 {
            let peer = PeerId::Netcode(id);
            let link = app.world_mut().spawn((RemoteId(peer), Connected)).id();
            app.world_mut()
                .resource_mut::<PendingClients>()
                .0
                .push((link, peer));
        }
        app.update();
        assert_eq!(app.world().resource::<PendingClients>().0.len(), 1);
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<ControlledBy>>()
                .iter(app.world())
                .count(),
            1,
            "a second player cannot reuse the sole point"
        );
        let link = app.world().resource::<PendingClients>().0[0].0;
        app.world_mut().entity_mut(link).remove::<Connected>();
        app.update();
        assert!(app.world().resource::<PendingClients>().0.is_empty());
    }

    /// Every map's authored automatic spawn points must clear its construction footprints expanded
    /// by the same occupancy radius as the player's spawn-map path. This catches placement edits
    /// that would put an automatically spawned tank in a construction, regardless of prototype id.
    #[test]
    fn authored_spawn_points_clear_construction_footprints() {
        let mut fixture = placement_app();
        let radius = fixture
            .world_mut()
            .run_system_once(|source: TankSimSource| source.get().contact_radius())
            .unwrap();
        let manifest = crate::map::tests::shipped_manifest();
        for instance in &manifest.instances {
            let pose = Transform {
                translation: Vec3::new(instance.translation[0], 0.0, instance.translation[2]),
                rotation: Quat::from_array(instance.rotation).normalize(),
                ..Transform::IDENTITY
            };
            let block = match &manifest.prototypes[&instance.prototype] {
                crate::map::Prototype::Block { min, max, .. } => {
                    let (min, max) = (Vec3::from_array(*min), Vec3::from_array(*max));
                    Transform {
                        translation: pose.translation + pose.rotation * ((min + max) * 0.5),
                        rotation: pose.rotation,
                        scale: max - min,
                    }
                }
                crate::map::Prototype::Construction { asset } => crate::construction::footprint(
                    manifest
                        .constructions
                        .get(asset)
                        .expect("map load resolved every structure"),
                    pose.to_isometry(),
                ),
            };
            if let Some(footprint) = block_footprint(&block, None, radius) {
                for point in manifest.spawn_points.as_slice() {
                    assert!(
                        !footprint_denies(footprint, *point),
                        "spawn {point:?} overlaps construction {}",
                        instance.id
                    );
                }
            }
        }
    }

    /// An in-bounds request passes through untouched; an out-of-bounds one is CLAMPED into the
    /// playable square rather than refused, so a click near the map edge still places you. A
    /// non-finite request is REFUSED outright: a NaN spawn would NaN the solver the moment the
    /// body enters the world, and no clamp can repair it (NaN survives `clamp`).
    #[test]
    fn spawn_requests_are_clamped_into_the_world() {
        let half = crate::terrain_grid::FIXTURE_EXTENT.half_extent();
        let limit = spawn_map::spawn_limit(half);
        assert_eq!(
            validate_spawn_request(half, SetSpawnPoint { x: 40.0, z: -90.0 }),
            Some(Vec2::new(40.0, -90.0)),
        );
        assert_eq!(
            validate_spawn_request(
                half,
                SetSpawnPoint {
                    x: 99_000.0,
                    z: -99_000.0,
                }
            ),
            Some(Vec2::new(limit, -limit)),
        );
        assert_eq!(
            validate_spawn_request(
                half,
                SetSpawnPoint {
                    x: f32::NAN,
                    z: 0.0
                }
            ),
            None,
        );
        assert_eq!(
            validate_spawn_request(
                half,
                SetSpawnPoint {
                    x: 0.0,
                    z: f32::INFINITY,
                }
            ),
            None,
        );
    }

    /// THE regression guard for "tanks spawn underground", netcode half: every spawn point this
    /// layer owns — the lanes a joining client fans out onto, the interpolation bot, and the
    /// extreme corners a spawn-map click can be clamped to — resolved through the shared rule
    /// against the REAL heightmap.
    ///
    /// The sim half lives in `terrain_grid` and covers the offline duel; it cannot cover these,
    /// because `src/net/tests/net_boundary.rs` forbids sim code from naming `crate::net`. Same
    /// rule, same assertion helper, asserted from the layer that owns the points.
    #[test]
    fn every_netcode_spawn_point_lands_above_the_shipped_terrain() {
        use crate::terrain_grid::tests::{assert_spawn_clears_terrain, shipped_grid};
        let grid = shipped_grid();
        let manifest = crate::map::tests::shipped_manifest();
        for (index, &point) in manifest.spawn_points.as_slice().iter().enumerate() {
            assert_spawn_clears_terrain(&grid, &format!("authored spawn {index}"), point);
        }
        // Lanes fan out from the origin and the bots' homes from theirs; 8 covers far more clients
        // than a match holds.
        for lane in 0..8 {
            assert_spawn_clears_terrain(
                &grid,
                &format!("server lane {lane}"),
                lane_spawn_xz(Vec2::ZERO, lane, FIXTURE_BODY_RADIUS_M),
            );
            assert_spawn_clears_terrain(
                &grid,
                &format!("bot {lane}"),
                bot_home_xz(lane, FIXTURE_BODY_RADIUS_M),
            );
        }
        // The extremes a spawn-map click can reach, which the authority clamps to before resolving.
        let limit = spawn_map::spawn_limit(grid.half_extent());
        for (name, xz) in [
            ("spawn-map corner -,-", Vec2::new(-limit, -limit)),
            ("spawn-map corner +,+", Vec2::new(limit, limit)),
            ("spawn-map corner -,+", Vec2::new(-limit, limit)),
            ("spawn-map corner +,-", Vec2::new(limit, -limit)),
        ] {
            assert_spawn_clears_terrain(&grid, name, xz);
        }
    }

    /// The occupancy policy: a free request passes through untouched; a request on top of a live
    /// tank is nudged to the FIRST free candidate of the deterministic ring search; a fully
    /// crowded neighbourhood returns `None` (the request waits).
    #[test]
    fn occupied_spawn_requests_are_nudged_deterministically() {
        let half = crate::terrain_grid::FIXTURE_EXTENT.half_extent();
        let desired = Vec2::new(100.0, -50.0);
        // Free field: untouched, not nudged.
        assert_eq!(
            resolve_free_spawn_xz(half, desired, &[], &[], None, FIXTURE_BODY_RADIUS_M),
            Ok((desired, false))
        );
        // A tank exactly at the radius is NOT blocking (>= is free)…
        let at_radius = desired + Vec2::new(2.0 * FIXTURE_BODY_RADIUS_M, 0.0);
        assert_eq!(
            resolve_free_spawn_xz(
                half,
                desired,
                &[at_radius],
                &[],
                None,
                FIXTURE_BODY_RADIUS_M
            ),
            Ok((desired, false)),
        );
        // …but one inside it is, and the search takes candidates in fixed ring order: the
        // occupant at +5.9 m X blocks the request, blocks +X@8 (2.1 m away) and NE@8 (5.66 m
        // away), so the first FREE candidate is +Z@8 — deterministically.
        let blocking = desired + Vec2::new((2.0 * FIXTURE_BODY_RADIUS_M) - 0.1, 0.0);
        let (spot, nudged) =
            resolve_free_spawn_xz(half, desired, &[blocking], &[], None, FIXTURE_BODY_RADIUS_M)
                .expect("a ring is free");
        assert!(nudged);
        assert_eq!(
            spot,
            desired + SPAWN_DIRS_8[2] * SPAWN_SEARCH_RINGS[0].0,
            "first free candidate in fixed ring order"
        );
        // A tank ON the point (opponent parked there): first candidate +X@8 is 8 m away — free.
        let (spot, nudged) =
            resolve_free_spawn_xz(half, desired, &[desired], &[], None, FIXTURE_BODY_RADIUS_M)
                .expect("a ring is free");
        assert!(nudged);
        assert_eq!(spot, desired + Vec2::new(SPAWN_SEARCH_RINGS[0].0, 0.0));
        // Every candidate occupied (a tank parked on each): the request waits.
        let mut crowd = vec![desired];
        for (radius, dirs) in SPAWN_SEARCH_RINGS {
            for &dir in dirs {
                crowd.push(desired + dir * radius);
            }
        }
        assert_eq!(
            resolve_free_spawn_xz(half, desired, &crowd, &[], None, FIXTURE_BODY_RADIUS_M),
            Err(SpawnBlockage::Occupied)
        );
        // Candidates near the map edge clamp into the placeable square.
        let limit = spawn_map::spawn_limit(half);
        let corner = Vec2::new(limit, limit);
        let (spot, nudged) =
            resolve_free_spawn_xz(half, corner, &[corner], &[], None, FIXTURE_BODY_RADIUS_M)
                .expect("a ring is free");
        assert!(nudged);
        assert!(spot.x.abs() <= limit && spot.y.abs() <= limit);
    }

    /// A construction block in the [`TerrainMap`] encoding (`world::spawn_block` / `scatter`): a
    /// `size`-metre box standing on a surface at y = 0, centred at `center` in XZ, its bottom
    /// `sink` metres under that surface (the house proxy's skirt).
    fn construction(center: Vec2, size: Vec3, sink: f32) -> Transform {
        Transform::from_xyz(center.x, size.y / 2.0 - sink, center.y).with_scale(size)
    }

    /// The construction rule: a spawn inside a house is refused and the ring search lands the tank
    /// outside its walls, while the flat-slab world's GROUND block — a block whose top IS the
    /// surface it defines — denies nothing (it would otherwise swallow the whole placeable square).
    #[test]
    fn spawns_inside_a_construction_are_refused_and_the_ground_block_is_not() {
        let half = crate::terrain_grid::FIXTURE_EXTENT.half_extent();
        let desired = Vec2::new(100.0, -50.0);
        // DERIVED fixture: a hall 16 m × 20 m plus a 3 m body radius has the same footprint
        // covers every ring-8 candidate (max |dx| = 8 < 11, |dz| = 8 < 13), so the first free
        // candidate is ring 16's +X — deterministically, in fixed ring order.
        let hall = construction(desired, Vec3::new(16.0, 6.0, 20.0), 0.5);
        let (spot, nudged) =
            resolve_free_spawn_xz(half, desired, &[], &[hall], None, FIXTURE_BODY_RADIUS_M)
                .expect("a ring is free");
        assert!(nudged);
        assert_eq!(spot, desired + SPAWN_DIRS_13[0] * SPAWN_SEARCH_RINGS[1].0);
        let walls = block_footprint(&hall, None, FIXTURE_BODY_RADIUS_M)
            .expect("a construction stands on the map");
        assert!(
            !footprint_denies(walls, spot),
            "the resolved spot must clear the construction's walls"
        );
        // The flat-slab world's ground: a 1500 m block whose top face IS the surface every spawn
        // stands on. Under a footprint-only rule it would occupy the entire world.
        let ground = Transform::from_xyz(0.0, -0.5, 0.0).with_scale(Vec3::new(1500.0, 1.0, 1500.0));
        assert_eq!(
            block_footprint(&ground, None, FIXTURE_BODY_RADIUS_M),
            None,
            "the ground is not a wall"
        );
        assert_eq!(
            resolve_free_spawn_xz(half, desired, &[], &[ground], None, FIXTURE_BODY_RADIUS_M),
            Ok((desired, false)),
        );
        // A house standing ON that ground is still occupancy — the ground term never masks it.
        let house = construction(desired, Vec3::new(4.0, 3.0, 6.0), 0.5);
        let (spot, nudged) = resolve_free_spawn_xz(
            half,
            desired,
            &[],
            &[ground, house],
            None,
            FIXTURE_BODY_RADIUS_M,
        )
        .expect("a ring is free");
        assert!(nudged);
        assert!(!footprint_denies(
            block_footprint(&house, None, FIXTURE_BODY_RADIUS_M)
                .expect("a house stands on the map"),
            spot,
        ));
    }

    /// A HOUSE OF THE DESTRUCTION MODEL STILL REFUSES A SPAWN INSIDE IT: its walls are no terrain
    /// block (playtest fork F9), so its bounds stand in the spawn obstacles, and a request at the
    /// shipped house's centre (yawed, fixture data) is nudged clear of them. A
    /// world that dropped the houses from spawn avoidance with the blocks lands a tank inside one.
    #[test]
    fn a_spawn_inside_a_house_of_the_destruction_model_is_refused() {
        let half = crate::terrain_grid::FIXTURE_EXTENT.half_extent();
        let desired = Vec2::new(-120.0, 64.0);
        let pose = Transform::from_xyz(desired.x, 0.0, desired.y)
            .with_rotation(Quat::from_rotation_y(0.4));
        let house = crate::construction::footprint(
            &crate::construction::tests::shipped_house(),
            pose.to_isometry(),
        );
        let walls = block_footprint(&house, None, FIXTURE_BODY_RADIUS_M)
            .expect("a house stands on the map");
        assert!(
            footprint_denies(walls, desired),
            "the house covers its own centre"
        );
        let (spot, nudged) =
            resolve_free_spawn_xz(half, desired, &[], &[house], None, FIXTURE_BODY_RADIUS_M)
                .expect("a ring is free");
        assert!(nudged);
        assert!(!footprint_denies(walls, spot));
    }

    /// The ring tables are what their doc claims: per-ring count `max(8, ceil(2πr / 8 m))` (so
    /// candidate spacing never exceeds ~8 m of arc — the 8-spoke pattern left 37.7 m gaps at
    /// 48 m), radii ascending, and every entry the exact unit vector `(cos, sin)(2πk/n)` CCW
    /// from +X. Test-side trig only — the tables themselves are constants.
    #[test]
    fn spawn_search_rings_are_unit_uniform_and_dense_enough() {
        let mut previous = 0.0_f32;
        for (radius, dirs) in SPAWN_SEARCH_RINGS {
            assert!(radius > previous, "ring radii must ascend");
            previous = radius;
            let n = dirs.len();
            let want = ((core::f32::consts::TAU * radius / 8.0).ceil() as usize).max(8);
            assert_eq!(n, want, "ring {radius} m needs {want} directions, has {n}");
            for (k, dir) in dirs.iter().enumerate() {
                let angle = core::f32::consts::TAU * k as f32 / n as f32;
                let exact = Vec2::new(angle.cos(), angle.sin());
                assert!(
                    (*dir - exact).length() < 1e-6,
                    "ring {radius} m dir {k} is {dir:?}, wants {exact:?}"
                );
            }
        }
        assert_eq!(
            SPAWN_SEARCH_RINGS[SPAWN_SEARCH_RINGS.len() - 1].0,
            48.0,
            "the ~50 m search budget is unchanged"
        );
    }

    /// Occupied compass points do not imply occupied intervening arcs. Dense outer rings must
    /// find clear ground between them; players rely on the nearby candidate search covering it.
    #[test]
    fn outer_ring_between_spokes_gap_is_searched() {
        let half = crate::terrain_grid::FIXTURE_EXTENT.half_extent();
        let desired = Vec2::new(100.0, -50.0);
        let mut crowd = vec![desired];
        // Every candidate of every ring below 48 m…
        for (radius, dirs) in &SPAWN_SEARCH_RINGS[..SPAWN_SEARCH_RINGS.len() - 1] {
            for &dir in *dirs {
                crowd.push(desired + dir * *radius);
            }
        }
        // …plus exactly the old 8 compass spokes at 48 m.
        for dir in SPAWN_DIRS_8 {
            crowd.push(desired + dir * 48.0);
        }
        let (spot, nudged) =
            resolve_free_spawn_xz(half, desired, &crowd, &[], None, FIXTURE_BODY_RADIUS_M)
                .expect("a between-spokes candidate is free");
        assert!(nudged);
        // The first free candidate in fixed order: ring 48's direction 1 (9.47° — 7.93 m of arc
        // from the occupied +X spoke, outside the 6 m occupancy radius).
        assert_eq!(spot, desired + SPAWN_DIRS_38[1] * 48.0);
        assert!(
            (spot - desired).length() > 47.9,
            "the free spot sits on the outer ring"
        );
        for tank in &crowd {
            assert!(
                spot.distance(*tank) >= (2.0 * FIXTURE_BODY_RADIUS_M),
                "the found spot must clear every occupant"
            );
        }
    }

    /// The footprint ground query (Finding: sampling only the tank-root point buried the uphill
    /// axles 1.65 m on a legal slope click): spawn Y reads the MAXIMUM grid height over the
    /// 10 m × 10 m footprint, so a bump 2.5 m off-center lifts the spawn even though the
    /// center-point height is 0. A harness spawn-pose override is passed through untouched — it
    /// names an exact resting contact, so re-sampling it would break the beached-rest repro.
    #[test]
    fn spawn_height_samples_the_tank_footprint_not_the_point() {
        use crate::terrain_grid::{GRID_RESOLUTION, HeightGrid};
        // The production lattice (~0.977 m spacing), flat except one 10 m node one step off the
        // origin — inside the footprint square of a spawn at the origin, outside its centre point.
        let n = GRID_RESOLUTION as usize;
        let mut samples = vec![0.0f32; n * n];
        let center = n / 2; // node at world 0
        samples[center * n + (center + 1)] = 10.0;
        let grid = HeightGrid::new(
            samples.into(),
            GRID_RESOLUTION,
            crate::terrain_grid::FIXTURE_EXTENT,
        );
        assert_eq!(grid.height_at(0.0, 0.0), 0.0, "center-point height is flat");
        assert_eq!(
            spawn_surface_height(Some(&grid), Vec2::ZERO),
            10.0,
            "the footprint max sees the off-centre bump"
        );
        // Far away the footprint reads the flat ground.
        assert_eq!(
            spawn_surface_height(Some(&grid), Vec2::new(500.0, 500.0)),
            0.0
        );
        // And the pose composes it with the standard clearance.
        assert_eq!(
            spawn_pos(Some(&grid), Vec2::ZERO),
            Vec3::new(0.0, 12.0, 0.0)
        );
        // No grid = the flat-slab world: y = 2 exactly, for players and the bot alike.
        assert_eq!(spawn_surface_height(None, Vec2::ZERO), 0.0);
        assert_eq!(
            spawn_pos(None, bot_home_xz(0, FIXTURE_BODY_RADIUS_M)),
            Vec3::new(0.0, 2.0, 12.0)
        );
        let harness = Vec3::new(3.0, 0.42, -7.0);
        assert_eq!(
            lane_spawn_pos(harness, 0, FIXTURE_BODY_RADIUS_M),
            harness,
            "a harness spawn pose is never re-sampled",
        );
    }
}
