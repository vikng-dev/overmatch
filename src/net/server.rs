//! The networked dedicated server (step 7 of the lightyear spike map's recommended order):
//! `SimPlugin` mounted for real — driving/aim/shooting/damage all run under prediction now, not
//! the increment-5 stub. Headless — the proven `headless_test.rs` recipe (full `DefaultPlugins`,
//! no GPU/window/winit), NOT `MinimalPlugins`, because the rig loads the same `.glb`/`.tank.ron`
//! assets the client does.
//! Run with `cargo run --bin server --features net`.

use core::time::Duration;
use std::net::{Ipv4Addr, SocketAddr};

use avian3d::prelude::{Position, RigidBody, Rotation};
use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::server::*;
use lightyear::prelude::*;

use super::protocol::ServoAngles;
use super::{diagnostics, harness, open_gameplay_gate, physics, rig};
use crate::SimPlugin;
use crate::command::{ConsumeCommandEdges, TankCommand};
use crate::state::GameplaySet;
use crate::tank::{
    PendingTankAssets, TankSimSource, bind_tank_view, load_tank_assets, spawn_tank_sim,
};

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
    // No winit means no runner: drive the main schedule ourselves, unthrottled enough for a
    // 64 Hz fixed clock.
    .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_millis(2)));

    // Ordering per the spike map §3: ServerPlugins, then protocol registration, then the Server
    // entity. `net::plugin` also mounts `LightyearAvianPlugin` + Position/Rotation/velocity
    // registration (map §5).
    app.add_plugins(ServerPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / 64.0),
    });
    app.add_plugins(super::plugin);
    app.add_plugins(physics::physics_plugins());
    // Step 7: the real sim, for real — driving/aim/shooting/damage all run under prediction now.
    // `SimPlugin` no longer bundles physics (that split already happened in Milestone A), so it
    // composes cleanly alongside `physics_plugins()` above. Not `tank::sp_spawn_plugin` (the
    // single-player two-tank duel scenario) — the server keeps its own per-client spawn below.
    app.add_plugins(SimPlugin);
    // Passive jitter-trace recorder: tick rows only (the server has no predicted view to render).
    // Idle unless `SPIKE_TRACE` is set.
    app.add_plugins(crate::trace::server_plugin);

    let server = app
        .world_mut()
        .spawn((
            Name::new("Server"),
            NetcodeServer::new(NetcodeConfig {
                protocol_id: 0,
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
    let config = harness::PerturbConfig {
        perturb: harness::env_flag("SPIKE_PERTURB", true),
    };
    info!("server: SPIKE_PERTURB={}", config.perturb);
    app.insert_resource(config);

    app.add_systems(
        Update,
        (
            handle_new_clients,
            spawn_pending_tanks,
            // Ownerless test bot (`OVERMATCH_BOT`, default OFF): a self-driving remote every client
            // interpolates, so the remote-tank path can be exercised without a second client.
            spawn_bot,
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
            // Steer the bot each tick — in `GameplaySet` before the edge-consumer, like every other
            // command writer; a direct `TankCommand` write (the bot carries no `ActionState`, so
            // `bridge_action_state_to_tank_command` skips it).
            drive_bot.in_set(GameplaySet).before(ConsumeCommandEdges),
        ),
    );

    app.run();
}

/// Connected clients waiting on the Tiger spec load (§2 of the map: the spec is a load
/// dependency of spawning, same as `tank.rs` — `spawn_tank_sim` requires the spec + extracted
/// geometry already loaded). A client can connect before the tiny `.tank.ron` parse finishes;
/// queueing avoids spawning a tank root the spawner would immediately panic on.
#[derive(Resource, Default)]
struct PendingClients(Vec<(Entity, PeerId)>);

/// Persistent spawn-lane counter so successive (and reconnecting) clients fan out along X instead
/// of stacking on the shared base pose — two tanks spawned at the same spot interpenetrate and NaN
/// the solver. Never reset: a reconnecting client just takes the next free lane. Lanes step 0, +8,
/// −8, +16, −16 … metres ([`lane_offset`]), comfortably inside the 1000 m ground slab and clear of
/// both the −Z test course and the +38 m side-slope.
#[derive(Resource, Default)]
struct SpawnLane(u32);

/// The per-lane X offset laid on top of the base spawn pose: lane 0 is exactly on the base (so a
/// single client — and the deterministic `SPIKE_SPAWN_POSE` harness repro — lands unshifted), then
/// odd lanes step +8 m, even lanes −8 m, the magnitude growing 8 m each pair.
fn lane_offset(lane: u32) -> Vec3 {
    let step = lane.div_ceil(2) as f32 * 8.0;
    let sign = if lane % 2 == 1 { 1.0 } else { -1.0 };
    Vec3::new(sign * step, 0.0, 0.0)
}

/// Queues each newly connected client — spawning is deferred to [`spawn_pending_tanks`] once the
/// Tiger spec has loaded (spike map §6/§7 spawn pattern: one predicted tank per client, owned by
/// that client, everyone else would interpolate it — no second client yet).
fn handle_new_clients(
    new: Query<(Entity, &RemoteId), (Added<Connected>, With<ClientOf>)>,
    mut pending: ResMut<PendingClients>,
) {
    for (link, remote) in &new {
        info!("server: client connected: {remote} (link {link})");
        pending.0.push((link, remote.0));
    }
}

/// Spawns the real Tiger for every queued client once the assets have loaded — sim body built
/// synchronously from the extracted geometry (`spawn_tank_sim`) in the same command batch as the
/// root, so the tank is collider-complete before replication ever sees it; the glb scene attaches
/// later as pure view (`bind_tank_view`, observed below).
fn spawn_pending_tanks(
    mut pending: ResMut<PendingClients>,
    assets: Option<Res<PendingTankAssets>>,
    asset_server: Res<AssetServer>,
    source: TankSimSource,
    time: Res<Time<Virtual>>,
    config: Res<harness::PerturbConfig>,
    mut lane: ResMut<SpawnLane>,
    mut commands: Commands,
) {
    if pending.0.is_empty() {
        return;
    }
    let Some(assets) = assets else { return };
    if !assets.loaded(&asset_server) {
        return;
    }
    let Some((geometry, spec)) = source.get(&assets.spec) else {
        return;
    };
    // Harness override (`SPIKE_SPAWN_POSE`): place the tank onto a known resting contact for the
    // beached-rest repro; unset in every normal run, so the default flat-pad spawn stands.
    let (base_pos, spawn_rot) =
        harness::spawn_pose().unwrap_or((Vec3::new(0.0, 2.0, 0.0), Quat::IDENTITY));
    for (link, client_id) in pending.0.drain(..) {
        // Fan each client out onto its own lane (lane 0 = the base pose, so the single-client and
        // `SPIKE_SPAWN_POSE` cases are unshifted); the counter persists so reconnects don't collide.
        let spawn_pos = base_pos + lane_offset(lane.0);
        lane.0 += 1;
        let mut tank = commands.spawn((
            Name::new("Tank"),
            rig::net_tank_rig(&assets),
            // The server is always the authority: its body simulates from tick 0. Set alongside
            // `spawn_tank_sim`'s collider inserts (below, same command batch), so `Dynamic` is
            // present in the same flush as the colliders — they never sit unattached or
            // placeholder-posed waiting for a body (the step-8 NaN class; `net::rig`'s module
            // invariants state its exact boundary).
            RigidBody::Dynamic,
            ActionState::<TankCommand>::default(),
            Position(spawn_pos),
            // Explicit identity (or the `SPIKE_SPAWN_POSE` override), NOT left to `RigidBody`'s
            // required-component default — that default is `Rotation::PLACEHOLDER` (f32::MAX
            // sentinel, avian rigid_body/mod.rs:271), and if replication captures the spawn frame
            // before the transform sync overwrites it, the client's confirmed history for the
            // earliest ticks holds the sentinel — which rollbacks would then faithfully restore
            // into the sim (the placeholder-NaN class, spike log 2026-07-04).
            Rotation(spawn_rot),
            // The authority's turret/gun lay, for every non-predicted view of this tank
            // (`net::publish_servo_angles` keeps it fresh).
            ServoAngles::default(),
            Replicate::to_clients(NetworkTarget::All),
            // Replicate the ROOT alone. Without this, `Replicate` propagates to every rig child
            // via `ReplicateLike` — the whole sim skeleton (~19 child entities per tank, spawned
            // synchronously under the root by `spawn_tank_sim`) would replicate to each client,
            // where nothing simulates them (the client builds its own skeleton locally), each
            // predicted+history-tracked: a standing spurious rollback-check source while the tank
            // moves, plus per-tick child Position/Rotation bandwidth and B0004 orphan-transform
            // warnings. The authority's turret/gun lay rides the root's `ServoAngles` instead.
            DisableReplicateHierarchy,
            // The committed model: the owner predicts its own tank (input feels instant, rollback
            // reconciles), everyone else interpolates it — the standard pairing every lightyear
            // multiplayer example ships (map §7).
            PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            ControlledBy {
                owner: link,
                lifetime: default(),
            },
        ));
        if config.perturb {
            tank.insert(harness::PendingPerturbation {
                at: time.elapsed() + Duration::from_secs(2),
            });
        }
        tank.observe(bind_tank_view);
        let root = tank.id();
        spawn_tank_sim(&mut commands, root, geometry, spec);
    }
}

/// Marker for the ownerless test-bot tank ([`spawn_bot`]) — scopes [`drive_bot`] to it, and keeps
/// it out of every other tank query the server runs.
#[derive(Component)]
struct Bot;

/// Spawn ONE ownerless "bot" tank that drives in a circle, replicated to every client as a pure
/// interpolated remote — a solo test rig for the remote-tank interpolation/timing path, with no
/// second client needed. Gated behind `OVERMATCH_BOT` (present = on, unset = off), default OFF so
/// every existing run and harness recipe is byte-for-byte unchanged. Same asset gate as
/// [`spawn_pending_tanks`]; spawns exactly once (the `spawned` guard). Mirrors the player spawn's
/// component set minus the ownership half.
fn spawn_bot(
    mut spawned: Local<bool>,
    assets: Option<Res<PendingTankAssets>>,
    asset_server: Res<AssetServer>,
    source: TankSimSource,
    mut commands: Commands,
) {
    // `is_err()` = the var is unset: present (even empty, e.g. `OVERMATCH_BOT=`) counts as on.
    if *spawned || std::env::var("OVERMATCH_BOT").is_err() {
        return;
    }
    let Some(assets) = assets else { return };
    if !assets.loaded(&asset_server) {
        return;
    }
    let Some((geometry, spec)) = source.get(&assets.spec) else {
        return;
    };
    *spawned = true;
    let mut tank = commands.spawn((
        Name::new("Bot"),
        Bot,
        // Replicated bot marker: `Name` doesn't ride the wire, so this is how the client's HUD
        // recognizes the bot to prefix its nameplate with `[BOT]`.
        super::protocol::NetBot,
        rig::net_tank_rig(&assets),
        // Simulated as a normal Dynamic body on the SERVER (it drives); clients receive it via
        // replication and, having no local body role for it, build a Static interpolated body
        // (`net::rig::attach_replicated_rig` picks `Static` for any non-`Predicted` tank).
        RigidBody::Dynamic,
        // A distinct spot on the flat pad, a few metres up +Z — away from the per-client X lanes
        // (`lane_offset`) and the −Z test course, so the circle stays on flat ground.
        Position(Vec3::new(0.0, 2.0, 12.0)),
        // Explicit, for the same replicated-placeholder reason as the player spawn (see there).
        Rotation(Quat::IDENTITY),
        ServoAngles::default(),
        Replicate::to_clients(NetworkTarget::All),
        DisableReplicateHierarchy,
        // NO `PredictionTarget`, NO `ControlledBy`: no client owns or predicts the bot, so on every
        // client the replicated root lands `Interpolated` only — a Static local body — which is
        // exactly the remote-tank path this rig exists to exercise solo.
        InterpolationTarget::to_clients(NetworkTarget::All),
    ));
    tank.observe(bind_tank_view);
    let root = tank.id();
    spawn_tank_sim(&mut commands, root, geometry, spec);
    info!("server: spawned circling bot tank {root} (OVERMATCH_BOT)");
}

/// Drive the bot in a steady circle: constant throttle + steer written straight into its own
/// `TankCommand` (`command.rs`'s read contract, attached by `command`'s `On<Add, Tank>` observer).
/// The bot carries no `ActionState`, so `bridge_action_state_to_tank_command` (protocol.rs) never
/// touches it — this is the sole writer. Ordered in `GameplaySet` before the edge-consumer, with
/// the other command writers; throttle/steer are levels (never cleared), so it circles for good.
fn drive_bot(mut bots: Query<&mut TankCommand, With<Bot>>) {
    for mut command in &mut bots {
        // Gentle constants: enough drive + differential yaw to circle on the flat pad without
        // leaving it or flipping. Everything else stays at `TankCommand::default()`.
        command.throttle = 0.5;
        command.steer = 0.5;
    }
}

/// Step-4 success signal (carried over): the client's `TankCommand` arriving through lightyear's
/// input buffer.
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
