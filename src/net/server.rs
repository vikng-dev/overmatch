//! Authoritative dedicated-server composition root.

use core::time::Duration;
use std::net::{Ipv4Addr, SocketAddr};

use avian3d::prelude::{Position, RigidBody, Rotation};
use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use lightyear::prelude::input::native::{ActionState, NativeStateSequence};
use lightyear::prelude::input::server::{InputValidationAppExt, authorize_controlled_targets};
use lightyear::prelude::server::*;
use lightyear::prelude::*;

use super::disclosure::{CombatDisclosure, NetTankStatus};
use super::grip::GripRestState;
use super::protocol::{
    LaunchedTurretPose, NetCrew, NetTank, NetTrackGripAnchor, PROTOCOL_FINGERPRINT, ServoAngles,
    SetSpawnPoint,
};
use super::{diagnostics, harness, open_gameplay_gate, physics, spawn_map};
use crate::command::{ConsumeCommandEdges, TankCommand};
use crate::damage::TankKnockedOut;
use crate::state::GameplaySet;
use crate::tank::{
    PendingTankAssets, Rig, TankContent, TankSimSource, load_tank_assets, spawn_complete_tank,
};
use crate::{CombatantId, SimPlugin};

const PORT: u16 = 5888;

pub fn run() {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(bevy::render::RenderPlugin {
                render_creation: bevy::render::settings::WgpuSettings {
                    backends: None,
                    ..default()
                }
                .into(),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                ..default()
            })
            .disable::<bevy::winit::WinitPlugin>(),
    )
    // Headless composition needs its own application runner.
    .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_millis(2)));

    app.add_plugins(ServerPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / 64.0),
    });
    app.add_plugins(super::plugin);
    super::grip::install_server(&mut app);
    super::disclosure::install_server(&mut app);
    app.add_plugins(physics::physics_plugins());
    app.add_plugins(SimPlugin);
    app.add_plugins(crate::trace::server_plugin);
    app.add_plugins(crate::cost::server_plugin);
    app.add_plugins(crate::shot_trace::server_plugin);
    super::shot_transport::install_server(&mut app);
    // Authority must reject input targets not controlled by their sending client.
    app.add_input_validator(authorize_controlled_targets::<NativeStateSequence<TankCommand>>);
    // Lightyear visibility hooks require `ReplicationSender` on each remote link.
    app.add_observer(attach_replication_sender);

    let server = app
        .world_mut()
        .spawn((
            Name::new("Server"),
            NetcodeServer::new(NetcodeConfig {
                // Must match the client's `Authentication::Manual.protocol_id`.
                protocol_id: PROTOCOL_FINGERPRINT,
                private_key: [0; 32], // dev only — matches the client's Authentication::Manual
                ..default()
            }),
            LocalAddr(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), PORT)),
            ServerUdpIo::default(),
        ))
        .id();
    app.add_systems(Startup, move |mut commands: Commands| {
        commands.trigger(Start { entity: server });
        info!("server: starting, listening on 0.0.0.0:{PORT}");
    });
    app.add_systems(Startup, load_tank_assets);
    app.init_resource::<PendingClients>();
    app.init_resource::<SpawnLane>();
    app.init_resource::<SpawnOverrides>();
    app.init_resource::<CombatantIds>();
    let config = harness::PerturbConfig {
        perturb: harness::env_flag("SPIKE_PERTURB", true),
    };
    info!("server: SPIKE_PERTURB={}", config.perturb);
    app.insert_resource(config);

    app.add_systems(
        Update,
        (
            handle_new_clients,
            // Drain the spawn-map lane at frame rate: lightyear clears an undrained `MessageReceiver`
            // in `Last` every frame (the same rule `grip::answer_resync_requests` follows in `Update`).
            receive_spawn_points,
            spawn_pending_tanks,
            spawn_bot,
            schedule_bot_respawn,
            respawn_dead_bots,
            open_gameplay_gate,
            diagnostics::log_positions,
            diagnostics::log_sim_evidence,
        ),
    );
    app.add_systems(
        FixedUpdate,
        (
            log_tank_commands,
            harness::perturb_after_delay,
            // Command writers must run before edge consumption.
            drive_bot.in_set(GameplaySet).before(ConsumeCommandEdges),
            respawn_player_tanks
                .in_set(GameplaySet)
                .before(ConsumeCommandEdges),
        ),
    );

    app.run();
}

/// Connected clients waiting for the server's next spawn pass. The simulation blueprint is eager,
/// so this queue batches link setup and spawn work rather than waiting on view assets.
#[derive(Resource, Default)]
struct PendingClients(Vec<(Entity, PeerId)>);

/// Match-local combatant identities. `next` is never reused; player links retain their id across
/// respawn and the optional bot retains its one id across its timed respawns.
#[derive(Resource, Default)]
struct CombatantIds {
    next: u64,
    players: bevy::platform::collections::HashMap<Entity, CombatantId>,
    bot: Option<CombatantId>,
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

    fn bot(&mut self) -> CombatantId {
        if let Some(id) = self.bot {
            return id;
        }
        let id = self.allocate();
        self.bot = Some(id);
        id
    }
}

/// Insert the marker required by Lightyear's per-client visibility hooks before tank spawning.
pub(super) fn attach_replication_sender(add: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(add.entity).insert(ReplicationSender);
}

/// Monotonic spawn-lane allocator; concurrent tanks must not overlap at spawn.
#[derive(Resource, Default)]
struct SpawnLane(u32);

/// Clearance the spawn pose keeps above the ground, metres — the flat-pad spawn's `y = 2.0` over a
/// surface at 0, reproduced over terrain as the footprint's max ground height + this.
const SPAWN_CLEARANCE_M: f32 = 2.0;

/// Half-side (m) of the conservative axis-aligned square footprint a spawn samples the ground
/// over (10 m × 10 m — covers the hull at any yaw). Spawn Y is the MAXIMUM grid height over this
/// square plus [`SPAWN_CLEARANCE_M`]: sampling only the root point buried the uphill axles by a
/// measured 1.65 m on a legal slope click.
const SPAWN_FOOTPRINT_HALF_M: f32 = 5.0;

/// Occupancy radius (m) for spawn placement: a requested spawn point with any live tank root
/// within this XZ distance counts as occupied (conservative cylinder around the tank volume).
/// Two players clicking the same map point would otherwise spawn fully overlapping dynamic
/// colliders and let the solver fling them.
const SPAWN_OCCUPIED_RADIUS_M: f32 = 6.0;

/// Deterministic outward search pattern for an occupied spawn point: 8 fixed compass directions
/// (unit vectors — exact constants, no trig at runtime) tried at each of [`SPAWN_SEARCH_RADII_M`]
/// in order; the first free candidate wins. Fixed order = same result on every peer/replay.
const SPAWN_SEARCH_DIRECTIONS: [Vec2; 8] = [
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

/// Ring radii (m) of the outward spawn search, inside the ~50 m budget; past the last ring the
/// caller falls back to the lane spawn.
const SPAWN_SEARCH_RADII_M: [f32; 6] = [8.0, 16.0, 24.0, 32.0, 40.0, 48.0];

/// Per-client spawn overrides chosen on the client's spawn map, keyed by the CLIENT LINK entity —
/// the same key [`CombatantIds`] and `ControlledBy::owner` use, so a client can only ever move its
/// own spawn (the message carries no id to forge).
///
/// Lifecycle: set/overwritten by [`receive_spawn_points`] on every accepted request, read by
/// [`respawn_player_tanks`] on that client's next respawn, and PERSISTENT — it is NOT cleared on
/// use, so every subsequent respawn keeps landing at the last point the player picked until they
/// pick another. The initial join spawn ignores it entirely (lane logic owns first placement).
#[derive(Resource, Default)]
struct SpawnOverrides(bevy::platform::collections::HashMap<Entity, Vec2>);

/// Validate a requested spawn XZ: reject non-finite components outright (a NaN would NaN the solver
/// the moment the body is inserted), otherwise clamp into the placeable square
/// ([`spawn_map::SPAWN_LIMIT_M`], inside the terrain edge). Pure, so the bound is unit-testable
/// without a world.
fn validate_spawn_request(request: SetSpawnPoint) -> Option<Vec2> {
    if !request.x.is_finite() || !request.z.is_finite() {
        return None;
    }
    let limit = spawn_map::SPAWN_LIMIT_M;
    Some(Vec2::new(
        request.x.clamp(-limit, limit),
        request.z.clamp(-limit, limit),
    ))
}

/// A lane pose, lifted onto the terrain: the lane's XZ, the ground under it, plus the standard
/// clearance. On the flat-slab world this is exactly the old `y = 2.0` pose. The `SPIKE_SPAWN_POSE`
/// harness override is honoured VERBATIM (it names an exact resting contact for the beached-rest
/// repro) and is never lifted.
fn lane_spawn_pos(
    base: Vec3,
    lane: u32,
    harness_override: bool,
    height: Option<&crate::terrain_grid::HeightGrid>,
) -> Vec3 {
    let pos = base + lane_offset(lane);
    if harness_override {
        return pos;
    }
    Vec3::new(
        pos.x,
        spawn_surface_height(height, pos.xz()) + SPAWN_CLEARANCE_M,
        pos.z,
    )
}

/// The authoritative spawn pose for a validated override: the client names X and Z, the authority
/// resolves Y from the terrain it owns — never from the client.
fn override_spawn_pos(xz: Vec2, ground_height: f32) -> Vec3 {
    Vec3::new(xz.x, ground_height + SPAWN_CLEARANCE_M, xz.y)
}

/// The surface height under a spawn FOOTPRINT: the maximum grid height over the conservative
/// [`SPAWN_FOOTPRINT_HALF_M`] square around the XZ (`HeightGrid::max_height_in_square` — a tank
/// dropped at the center-point height on a slope spawns with its uphill running gear buried).
/// Used by ALL spawn paths: lane spawns, override spawns, and the bot. Absent grid = the
/// flat-slab fallback world, whose surface is y = 0, which reproduces exactly the lane spawn's
/// `y = 2.0`.
fn spawn_surface_height(grid: Option<&crate::terrain_grid::HeightGrid>, xz: Vec2) -> f32 {
    grid.map_or(0.0, |grid| {
        grid.max_height_in_square(xz.x, xz.y, SPAWN_FOOTPRINT_HALF_M)
    })
}

/// Resolve a requested spawn XZ against live tank positions (Finding: two players clicking the
/// same point spawned overlapping dynamic colliders). Returns the first UNOCCUPIED point — the
/// request itself, or the first free candidate of a deterministic outward ring search
/// ([`SPAWN_SEARCH_DIRECTIONS`] × [`SPAWN_SEARCH_RADII_M`], candidates clamped into the placeable
/// square) — plus whether it was nudged; `None` when everything within ~50 m is occupied (the
/// caller falls back to the lane spawn). Pure, so the policy is unit-testable without a world.
fn resolve_free_spawn_xz(desired: Vec2, occupied: &[Vec2]) -> Option<(Vec2, bool)> {
    let limit = spawn_map::SPAWN_LIMIT_M;
    let free = |candidate: Vec2| {
        occupied.iter().all(|tank| {
            candidate.distance_squared(*tank) >= SPAWN_OCCUPIED_RADIUS_M * SPAWN_OCCUPIED_RADIUS_M
        })
    };
    if free(desired) {
        return Some((desired, false));
    }
    for radius in SPAWN_SEARCH_RADII_M {
        for dir in SPAWN_SEARCH_DIRECTIONS {
            let candidate = (desired + dir * radius).clamp(Vec2::splat(-limit), Vec2::splat(limit));
            if free(candidate) {
                return Some((candidate, true));
            }
        }
    }
    None
}

/// Record each client's latest spawn-map choice. The key is the SENDING LINK, so authority is
/// structural: nothing in the message names a target. Reliable-and-ordered lane, and only the last
/// accepted request survives — so the surviving override matches the player's latest click, which
/// is exactly the semantics of "where I want to come back".
fn receive_spawn_points(
    mut receivers: Query<(Entity, &mut MessageReceiver<SetSpawnPoint>), With<ClientOf>>,
    mut overrides: ResMut<SpawnOverrides>,
) {
    for (link, mut receiver) in &mut receivers {
        for request in receiver.receive() {
            let Some(xz) = validate_spawn_request(request) else {
                warn!("server: rejected non-finite spawn request from link {link}");
                continue;
            };
            overrides.0.insert(link, xz);
            info!(
                "server: link {link} set next spawn to ({:.1}, {:.1})",
                xz.x, xz.y
            );
        }
    }
}

/// Symmetric X offsets around the base spawn pose.
fn lane_offset(lane: u32) -> Vec3 {
    let step = lane.div_ceil(2) as f32 * 8.0;
    let sign = if lane % 2 == 1 { 1.0 } else { -1.0 };
    Vec3::new(sign * step, 0.0, 0.0)
}

/// Queues each newly connected client for [`spawn_pending_tanks`] (one predicted tank per client,
/// owned by that client; every other client interpolates it).
fn handle_new_clients(
    new: Query<(Entity, &RemoteId), (Added<Connected>, With<ClientOf>)>,
    mut pending: ResMut<PendingClients>,
) {
    for (link, remote) in &new {
        info!("server: client connected: {remote} (link {link})");
        pending.0.push((link, remote.0));
    }
}

/// Spawns a complete authoritative tank for every queued client. View handles may still be
/// loading; the simulation body comes from the eager blueprint.
fn spawn_pending_tanks(
    mut pending: ResMut<PendingClients>,
    assets: Res<PendingTankAssets>,
    source: TankSimSource,
    time: Res<Time<Virtual>>,
    config: Res<harness::PerturbConfig>,
    mut lane: ResMut<SpawnLane>,
    mut combatants: ResMut<CombatantIds>,
    height: Option<Res<crate::terrain_grid::HeightGrid>>,
    mut commands: Commands,
) {
    if pending.0.is_empty() {
        return;
    }
    let Some(content) = source.get() else {
        return;
    };
    // Harness override (`SPIKE_SPAWN_POSE`): place the tank onto a known resting contact for the
    // beached-rest repro; unset in every normal run, so the default flat-pad spawn stands.
    let harness_pose = harness::spawn_pose();
    let (base_pos, spawn_rot) =
        harness_pose.unwrap_or((Vec3::new(0.0, SPAWN_CLEARANCE_M, 0.0), Quat::IDENTITY));
    for (link, client_id) in pending.0.drain(..) {
        // Fan each client out onto its own lane (lane 0 = the base pose, so the single-client and
        // `SPIKE_SPAWN_POSE` cases are unshifted); the counter persists so reconnects don't collide.
        // The lane's XZ is unchanged; only its Y follows the ground now (`lane_spawn_pos`), so a
        // join onto a hillside no longer starts inside the terrain.
        let spawn_pos = lane_spawn_pos(base_pos, lane.0, harness_pose.is_some(), height.as_deref());
        lane.0 += 1;
        let root = spawn_player_tank(
            &mut commands,
            content,
            &assets,
            link,
            client_id,
            spawn_pos,
            spawn_rot,
            combatants.player(link),
        );
        if config.perturb {
            commands.entity(root).insert(harness::PendingPerturbation {
                at: time.elapsed() + Duration::from_secs(2),
            });
        }
    }
}

/// Construct an authoritative player tank. Initial join and respawn share this exact ownership and
/// prediction bundle so reacquisition cannot drift from first spawn.
fn spawn_player_tank(
    commands: &mut Commands,
    content: TankContent<'_>,
    assets: &PendingTankAssets,
    link: Entity,
    client_id: PeerId,
    spawn_pos: Vec3,
    spawn_rot: Quat,
    combatant: CombatantId,
) -> Entity {
    spawn_complete_tank(
        commands,
        content,
        assets.presentation(),
        (
            (
                Name::new("Tank"),
                NetTank,
                combatant,
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
                // Clients build their own local skeleton; replicate only root state.
                DisableReplicateHierarchy,
                // Owner predicts; every other client interpolates.
                PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
                InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
                ControlledBy {
                    owner: link,
                    lifetime: default(),
                },
            ),
            (NetTrackGripAnchor::default(), GripRestState::default()),
        ),
    )
}

/// Marker for the ownerless test-bot tank ([`spawn_bot`]) — scopes [`drive_bot`] to it, and keeps
/// it out of every other tank query the server runs.
#[derive(Component)]
struct Bot;

/// The bot's home XZ (the old flat-pad `(0, 12)`); its Y comes from the same footprint-safe
/// ground query every player spawn uses ([`bot_spawn_pos`] — the fixed `y = 2` pose measured
/// 113.65 m underground on the shipped heightmap).
const BOT_SPAWN_XZ: Vec2 = Vec2::new(0.0, 12.0);

/// The bot's spawn pose: home XZ lifted onto the terrain footprint like every other spawn.
fn bot_spawn_pos(height: Option<&crate::terrain_grid::HeightGrid>) -> Vec3 {
    Vec3::new(
        BOT_SPAWN_XZ.x,
        spawn_surface_height(height, BOT_SPAWN_XZ) + SPAWN_CLEARANCE_M,
        BOT_SPAWN_XZ.y,
    )
}

/// Spawn the optional ownerless interpolation-test bot once.
fn spawn_bot(
    mut spawned: Local<bool>,
    assets: Res<PendingTankAssets>,
    source: TankSimSource,
    mut combatants: ResMut<CombatantIds>,
    height: Option<Res<crate::terrain_grid::HeightGrid>>,
    mut commands: Commands,
) {
    // `is_err()` = the var is unset: present (even empty, e.g. `OVERMATCH_BOT=`) counts as on.
    if *spawned || std::env::var("OVERMATCH_BOT").is_err() {
        return;
    }
    let Some(content) = source.get() else {
        return;
    };
    *spawned = true;
    let pos = bot_spawn_pos(height.as_deref());
    let root = spawn_bot_entity(&mut commands, &assets, content, combatants.bot(), pos);
    info!(
        "server: spawned circling bot tank {root} (OVERMATCH_BOT) at ({:.1}, {:.1}, {:.1})",
        pos.x, pos.y, pos.z
    );
}

/// Construct the ownerless bot used by both initial spawn and respawn. `spawn_pos` comes from
/// [`bot_spawn_pos`] at both call sites — footprint-lifted onto the terrain, never a constant.
fn spawn_bot_entity(
    commands: &mut Commands,
    assets: &PendingTankAssets,
    content: TankContent<'_>,
    combatant: CombatantId,
    spawn_pos: Vec3,
) -> Entity {
    spawn_complete_tank(
        commands,
        content,
        assets.presentation(),
        (
            (
                Name::new("Bot"),
                Bot,
                // Name is not replicated; NetBot identifies it to the client HUD.
                super::protocol::NetBot,
                NetTank,
                combatant,
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
                CombatDisclosure::hidden(),
                Replicate::to_clients(NetworkTarget::All),
            ),
            (
                DisableReplicateHierarchy,
                // No owner or prediction target: every client interpolates this body.
                InterpolationTarget::to_clients(NetworkTarget::All),
            ),
            (NetTrackGripAnchor::default(), GripRestState::default()),
        ),
    )
}

/// Schedules a bot's respawn the tick its root gains [`TankKnockedOut`] — the emergent death label
/// (`damage::mark_dead_tanks` at 0 living crew, `damage::process_cookoffs` on cookoff). Stamps the
/// virtual-clock instant 5 s out onto [`BotRespawnAt`]; the `Without<BotRespawnAt>` filter keeps a
/// second death label (e.g. crew-loss after a cookoff) from rescheduling. Reads the same
/// `Time<Virtual>` clock the respawn consumer and `spawn_pending_tanks` do.
fn schedule_bot_respawn(
    dead: Query<Entity, (With<Bot>, Added<TankKnockedOut>, Without<BotRespawnAt>)>,
    time: Res<Time<Virtual>>,
    mut commands: Commands,
) {
    for bot in &dead {
        commands
            .entity(bot)
            .insert(BotRespawnAt(time.elapsed_secs() + 5.0));
        info!("server: bot {bot} knocked out; respawning in 5s");
    }
}

/// When a scheduled [`BotRespawnAt`] comes due, sweep the dead bot and spawn a fresh one at the same
/// pose through the same blueprint-backed constructor.
fn respawn_dead_bots(
    dead: Query<(Entity, &BotRespawnAt, &Rig, &CombatantId), With<Bot>>,
    assets: Res<PendingTankAssets>,
    source: TankSimSource,
    time: Res<Time<Virtual>>,
    height: Option<Res<crate::terrain_grid::HeightGrid>>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs();
    // (root, its Rig.turret) for every bot now due. Capture the turret handle BEFORE despawning the
    // root: if the bot cooked off, `damage::launch_turrets_on_cookoff` stripped the turret's
    // `ChildOf` and made it a free body, so it is NOT a descendant of the root and the recursive
    // root despawn below would miss it — leaking one launched turret per respawn.
    let due: Vec<(Entity, Entity, CombatantId)> = dead
        .iter()
        .filter(|(_, at, _, _)| now >= at.0)
        .map(|(root, _, rig, combatant)| (root, rig.turret, *combatant))
        .collect();
    if due.is_empty() {
        return;
    }
    let Some(content) = source.get() else {
        return;
    };
    for (root, turret, combatant) in due {
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
            combatant,
            bot_spawn_pos(height.as_deref()),
        );
        info!("server: respawned bot as {fresh} (was {root})");
    }
}

/// Marker scheduling a bot respawn: the `Time<Virtual>` timestamp (secs) at which the dead bot is
/// swept and a fresh one spawned. Inserted by [`schedule_bot_respawn`], consumed by
/// [`respawn_dead_bots`].
#[derive(Component)]
struct BotRespawnAt(f32);

/// Server-authoritative PLAYER respawn (the friend-fight counterpart to the bot loop above): when a
/// client's own tank is knocked out and the client latches a [`TankCommand::respawn`] edge, sweep the
/// dead tank and spawn that client a fresh one through the same ownership path connect uses
/// ([`spawn_player_tank`]).
///
/// **The death is VALIDATED on the authority, never trusted from the client.** The query is gated
/// `With<TankKnockedOut>` — the emergent death label the server itself latched (`damage::mark_dead_tanks`
/// at 0 living crew, `damage::process_cookoffs` on cookoff) off its OWN authoritative sim. A client
/// that forges `respawn: true` while alive names a tank that carries no `TankKnockedOut`, so it never
/// matches and nothing happens. `With<ControlledBy>` (read as `&ControlledBy`) scopes this to owned
/// player tanks and excludes the ownerless bot, whose death→respawn is the separate timed
/// [`respawn_dead_bots`] loop — the two never overlap.
///
/// Runs on the fixed clock in `GameplaySet`, `.before(ConsumeCommandEdges)`, exactly like [`drive_bot`]
/// and every other command reader: `bridge_action_state_to_tank_command` has already written this
/// tick's `respawn` edge (and, under input starvation, already CLEARED it via `TankCommand::clear_edges`,
/// so a stale held-last input can't re-trigger a respawn), and `consume_edges` clears it at the tick's
/// end — so a single latched edge respawns exactly once. The recursive root despawn drops the dead rig;
/// `tank::sweep_launched_turret_on_root_despawn` (`On<Remove, Rig>`, mounted in `SimPlugin` on both
/// ends) sweeps a cooked-off turret that detached from the root, so no launched turret leaks — the same
/// guarantee `respawn_dead_bots` relies on.
fn respawn_player_tanks(
    dead: Query<(Entity, &TankCommand, &ControlledBy, &CombatantId), With<TankKnockedOut>>,
    remotes: Query<&RemoteId>,
    // Every tank root's live position — the occupancy set an override spawn checks against
    // (includes dead husks still on the field and the bot; excludes roots swept this pass).
    tanks: Query<(Entity, &Position), With<NetTank>>,
    assets: Res<PendingTankAssets>,
    source: TankSimSource,
    mut lane: ResMut<SpawnLane>,
    overrides: Res<SpawnOverrides>,
    // The authority's own ground truth for spawn-map placement; absent in the flat-slab world.
    height: Option<Res<crate::terrain_grid::HeightGrid>>,
    mut commands: Commands,
) {
    // (dead root, owner link, owner client id) for every owned tank that both IS dead and asked to
    // respawn this tick. Resolve the owner's `RemoteId` up front (the `PeerId` the fresh tank must be
    // predicted/owned by); an owner link mid-disconnect with no `RemoteId` is skipped rather than
    // respawned to a client that is leaving.
    let requests: Vec<(Entity, Entity, PeerId, CombatantId)> = dead
        .iter()
        .filter(|(_, command, _, _)| command.respawn)
        .filter_map(|(root, _, controlled, combatant)| {
            remotes
                .get(controlled.owner)
                .ok()
                .map(|remote| (root, controlled.owner, remote.0, *combatant))
        })
        .collect();
    if requests.is_empty() {
        return;
    }
    let Some(content) = source.get() else {
        return;
    };
    // A respawn takes the NEXT free lane (never reset — same rule a reconnecting client follows), so a
    // fresh tank never lands on top of another body and NaNs the solver. The base pose honors the
    // `SPIKE_SPAWN_POSE` harness override exactly as the connect path does.
    let harness_pose = harness::spawn_pose();
    let (base_pos, spawn_rot) =
        harness_pose.unwrap_or((Vec3::new(0.0, SPAWN_CLEARANCE_M, 0.0), Quat::IDENTITY));
    // Roots being swept this pass are NOT occupancy: their colliders despawn in the same command
    // flush the fresh tanks spawn in.
    let sweeping: Vec<Entity> = requests.iter().map(|(root, ..)| *root).collect();
    // Occupancy also includes the spots ALREADY HANDED OUT this pass: two clients respawning onto
    // the same click in the same tick would otherwise both read the pre-pass world and overlap.
    let mut placed_this_pass: Vec<Vec2> = Vec::new();
    for (root, link, client_id, combatant) in requests {
        // A spawn-map choice REPLACES the lane pose for this respawn (and stays chosen for the next
        // one — see `SpawnOverrides`); the lane counter is left alone so the fallback placement is
        // unchanged for everyone still using it. The Y is resolved on the authority.
        let spawn_pos = match overrides.0.get(&link) {
            Some(&xz) => {
                let occupied: Vec<Vec2> = tanks
                    .iter()
                    .filter(|(tank, _)| !sweeping.contains(tank))
                    .map(|(_, position)| position.0.xz())
                    .chain(placed_this_pass.iter().copied())
                    .collect();
                match resolve_free_spawn_xz(xz, &occupied) {
                    Some((spot, nudged)) => {
                        if nudged {
                            info!(
                                "server: spawn override ({:.1}, {:.1}) occupied — nudged to ({:.1}, {:.1})",
                                xz.x, xz.y, spot.x, spot.y
                            );
                        }
                        Some(override_spawn_pos(
                            spot,
                            spawn_surface_height(height.as_deref(), spot),
                        ))
                    }
                    None => {
                        info!(
                            "server: spawn override ({:.1}, {:.1}) and every candidate within \
                             {} m occupied — falling back to the lane spawn",
                            xz.x,
                            xz.y,
                            SPAWN_SEARCH_RADII_M[SPAWN_SEARCH_RADII_M.len() - 1]
                        );
                        None
                    }
                }
            }
            None => None,
        }
        .unwrap_or_else(|| {
            let pos = lane_spawn_pos(base_pos, lane.0, harness_pose.is_some(), height.as_deref());
            lane.0 += 1;
            pos
        });
        placed_this_pass.push(spawn_pos.xz());
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
        );
        info!("server: player {client_id} respawn requested — swept {root}, spawned {fresh}");
    }
}

/// Drive the bot in a steady circle AND hold its main gun's trigger: constants written straight into
/// its own `TankCommand` (a required component of `Tank`). The bot carries no `ActionState`, so
/// `bridge_action_state_to_tank_command` (protocol.rs)
/// never touches it — this is the sole writer. Ordered in `GameplaySet` before the edge-consumer,
/// with the other command writers; the fields are levels (never cleared), so it circles and fires for
/// good. Firing makes the bot a self-firing target that exercises remote shot presentation, recoil,
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

/// Log commands received through Lightyear's input buffer.
fn log_tank_commands(states: Query<(Entity, &ActionState<TankCommand>)>) {
    for (entity, state) in &states {
        let cmd = &state.0;
        if cmd.throttle != 0.0 || cmd.fire_primary {
            info!(
                "server: {entity} command: throttle={} steer={} fire_primary={}",
                cmd.throttle, cmd.steer, cmd.fire_primary
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combatant_ids_are_nonzero_unique_and_retained_for_respawn() {
        let mut world = World::new();
        let player_a = world.spawn_empty().id();
        let player_b = world.spawn_empty().id();
        let mut ids = CombatantIds::default();

        let first_life = ids.player(player_a);
        let respawn = ids.player(player_a);
        let other_player = ids.player(player_b);
        let bot_first_life = ids.bot();
        let bot_respawn = ids.bot();

        assert_ne!(first_life, CombatantId(0));
        assert_eq!(
            first_life, respawn,
            "a player keeps its match identity on respawn"
        );
        assert_eq!(
            bot_first_life, bot_respawn,
            "the bot keeps its match identity on respawn"
        );
        assert_ne!(first_life, other_player);
        assert_ne!(first_life, bot_first_life);
        assert_ne!(other_player, bot_first_life);
    }

    /// An in-bounds request passes through untouched; an out-of-bounds one is CLAMPED into the
    /// playable square rather than refused, so a click near the map edge still places you.
    #[test]
    fn spawn_requests_are_clamped_into_the_world() {
        let limit = spawn_map::SPAWN_LIMIT_M;
        assert_eq!(
            validate_spawn_request(SetSpawnPoint { x: 40.0, z: -90.0 }),
            Some(Vec2::new(40.0, -90.0)),
        );
        assert_eq!(
            validate_spawn_request(SetSpawnPoint {
                x: 99_000.0,
                z: -99_000.0,
            }),
            Some(Vec2::new(limit, -limit)),
        );
    }

    /// A non-finite request is REFUSED outright: a NaN spawn would NaN the solver the moment the
    /// body enters the world, and no clamp can repair it.
    #[test]
    fn non_finite_spawn_requests_are_refused() {
        assert_eq!(
            validate_spawn_request(SetSpawnPoint {
                x: f32::NAN,
                z: 0.0
            }),
            None,
        );
        assert_eq!(
            validate_spawn_request(SetSpawnPoint {
                x: 0.0,
                z: f32::INFINITY,
            }),
            None,
        );
    }

    /// The override reproduces the flat-pad spawn's clearance: XZ from the client, Y from the
    /// ground the AUTHORITY sampled, plus the same 2 m the lane spawn uses.
    #[test]
    fn override_pose_keeps_the_lane_spawn_clearance() {
        let flat = override_spawn_pos(Vec2::new(10.0, -20.0), 0.0);
        assert_eq!(
            flat,
            Vec3::new(10.0, 2.0, -20.0),
            "over flat ground this is exactly the default lane spawn's y",
        );
        let hill = override_spawn_pos(Vec2::new(10.0, -20.0), 37.5);
        assert_eq!(hill, Vec3::new(10.0, 39.5, -20.0));
    }

    /// The lane spawn keeps its XZ and its 2 m clearance; with no height grid (the flat-slab world)
    /// it is byte-for-byte the pose the server used before terrain. A `SPIKE_SPAWN_POSE` override is
    /// passed through untouched — it names an exact resting contact, so lifting it would break the
    /// beached-rest repro.
    #[test]
    fn lane_spawn_keeps_its_xz_and_clearance() {
        let base = Vec3::new(0.0, SPAWN_CLEARANCE_M, 0.0);
        assert_eq!(
            lane_spawn_pos(base, 0, false, None),
            Vec3::new(0.0, 2.0, 0.0)
        );
        assert_eq!(
            lane_spawn_pos(base, 1, false, None),
            base + lane_offset(1),
            "lane fan-out is unchanged over flat ground",
        );
        let harness = Vec3::new(3.0, 0.42, -7.0);
        assert_eq!(
            lane_spawn_pos(harness, 0, true, None),
            harness,
            "a harness spawn pose is never lifted",
        );
    }

    #[test]
    #[should_panic(expected = "combatant-id allocator exhausted u64")]
    fn combatant_id_exhaustion_is_not_silently_reused() {
        let mut ids = CombatantIds {
            next: u64::MAX,
            ..default()
        };
        let _ = ids.allocate();
    }

    /// The occupancy policy: a free request passes through untouched; a request on top of a live
    /// tank is nudged to the FIRST free candidate of the deterministic ring search; a fully
    /// crowded neighbourhood returns `None` (lane-spawn fallback).
    #[test]
    fn occupied_spawn_requests_are_nudged_deterministically() {
        let desired = Vec2::new(100.0, -50.0);
        // Free field: untouched, not nudged.
        assert_eq!(resolve_free_spawn_xz(desired, &[]), Some((desired, false)));
        // A tank exactly at the radius is NOT blocking (>= is free)…
        let at_radius = desired + Vec2::new(SPAWN_OCCUPIED_RADIUS_M, 0.0);
        assert_eq!(
            resolve_free_spawn_xz(desired, &[at_radius]),
            Some((desired, false)),
        );
        // …but one inside it is, and the search takes candidates in fixed ring order: the
        // occupant at +5.9 m X blocks the request, blocks +X@8 (2.1 m away) and NE@8 (5.66 m
        // away), so the first FREE candidate is +Z@8 — deterministically.
        let blocking = desired + Vec2::new(SPAWN_OCCUPIED_RADIUS_M - 0.1, 0.0);
        let (spot, nudged) = resolve_free_spawn_xz(desired, &[blocking]).expect("a ring is free");
        assert!(nudged);
        assert_eq!(
            spot,
            desired + SPAWN_SEARCH_DIRECTIONS[2] * SPAWN_SEARCH_RADII_M[0],
            "first free candidate in fixed ring order"
        );
        // A tank ON the point (opponent parked there): first candidate +X@8 is 8 m away — free.
        let (spot, nudged) = resolve_free_spawn_xz(desired, &[desired]).expect("a ring is free");
        assert!(nudged);
        assert_eq!(spot, desired + Vec2::new(SPAWN_SEARCH_RADII_M[0], 0.0));
        // Every candidate occupied (a tank parked on each): lane fallback.
        let mut crowd = vec![desired];
        for radius in SPAWN_SEARCH_RADII_M {
            for dir in SPAWN_SEARCH_DIRECTIONS {
                crowd.push(desired + dir * radius);
            }
        }
        assert_eq!(resolve_free_spawn_xz(desired, &crowd), None);
        // Candidates near the map edge clamp into the placeable square.
        let limit = spawn_map::SPAWN_LIMIT_M;
        let corner = Vec2::new(limit, limit);
        let (spot, nudged) = resolve_free_spawn_xz(corner, &[corner]).expect("a ring is free");
        assert!(nudged);
        assert!(spot.x.abs() <= limit && spot.y.abs() <= limit);
    }

    /// The footprint ground query (Finding: sampling only the tank-root point buried the uphill
    /// axles 1.65 m on a legal slope click): spawn Y reads the MAXIMUM grid height over the
    /// 10 m × 10 m footprint, so a bump 2.5 m off-center lifts the spawn even though the
    /// center-point height is 0.
    #[test]
    fn spawn_height_samples_the_tank_footprint_not_the_point() {
        use crate::terrain_grid::{GRID_RESOLUTION, HeightGrid};
        // The production lattice (2.5 m spacing), flat except one 10 m node at (x=2.5, z=0) —
        // inside the footprint square of a spawn at the origin.
        let n = GRID_RESOLUTION as usize;
        let mut samples = vec![0.0f32; n * n];
        let center = n / 2; // node at world 0
        samples[center * n + (center + 1)] = 10.0;
        let grid = HeightGrid::new(samples.into(), GRID_RESOLUTION);
        assert_eq!(grid.height_at(0.0, 0.0), 0.0, "center-point height is flat");
        assert_eq!(
            spawn_surface_height(Some(&grid), Vec2::ZERO),
            10.0,
            "the footprint max sees the 2.5 m-offset bump"
        );
        // Far away the footprint reads the flat ground.
        assert_eq!(
            spawn_surface_height(Some(&grid), Vec2::new(500.0, 500.0)),
            0.0
        );
        // And the override pose composes it with the standard clearance.
        assert_eq!(
            override_spawn_pos(Vec2::ZERO, spawn_surface_height(Some(&grid), Vec2::ZERO)),
            Vec3::new(0.0, 12.0, 0.0),
        );
        // No grid = the flat-slab world: y = 2 exactly, for players and the bot alike.
        assert_eq!(spawn_surface_height(None, Vec2::ZERO), 0.0);
        assert_eq!(bot_spawn_pos(None), Vec3::new(0.0, 2.0, 12.0));
    }
}
