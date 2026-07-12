//! The networked dedicated server (step 7 of the lightyear spike map's recommended order):
//! `SimPlugin` mounted for real — driving/aim/shooting/damage all run under prediction now, not
//! the increment-5 stub. Headless — the proven `headless_test.rs` recipe (full `DefaultPlugins`,
//! no GPU/window/winit), NOT `MinimalPlugins`, because the rig loads the same `.glb`/`.tank.ron`
//! assets the client does.
//! Run with `cargo run --bin overmatch-server` (the `net` feature is on by default).

use core::time::Duration;
use std::net::{Ipv4Addr, SocketAddr};

use avian3d::prelude::{Position, RigidBody, Rotation};
use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use lightyear::prelude::input::native::{ActionState, NativeStateSequence};
use lightyear::prelude::input::server::{InputValidationAppExt, authorize_controlled_targets};
use lightyear::prelude::server::*;
use lightyear::prelude::*;
// Row fields for the shot-lifecycle recorder (`crate::shot_trace`); evaluated only when it is armed.
use serde_json::json;

use super::protocol::{
    FireBurst, FireChannel, FireEvent, ImpactConfirm, LaunchedTurretPose, NetBelts, NetCrew,
    PROTOCOL_FINGERPRINT, RicochetKeyframe, ServoAngles,
};
use super::{diagnostics, harness, open_gameplay_gate, physics, rig};
use crate::SimPlugin;
use crate::bake::TankGeometry;
use crate::ballistics::{FireShell, ShellRicochet, ShellTerminal};
use crate::command::{ConsumeCommandEdges, TankCommand};
use crate::damage::TankKnockedOut;
use crate::spec::TankSpec;
use crate::state::GameplaySet;
use crate::tank::{
    PendingTankAssets, Rig, TankSimSource, bind_tank_view, load_tank_assets, spawn_tank_sim,
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
    // Per-fixed-tick sim-cost recorder: idle unless `SPIKE_COST_TRACE` is set (the MG-march cost spike).
    app.add_plugins(crate::cost::server_plugin);
    // Shot-lifecycle recorder: the authority's half (fire broadcast / ricochet keyframe / impact
    // confirm, keyed by `ShotId`). Idle unless `SPIKE_SHOT_TRACE` is set — see `crate::shot_trace`.
    app.add_plugins(crate::shot_trace::server_plugin);
    // The cosmetic opponent-fire broadcast: SERVER ONLY. Every authoritative `fire` raises a
    // `FireShell`; `broadcast_fire` turns each one that names a tank into a `FireEvent`; every
    // authoritative ricochet raises a `ShellRicochet` that `on_shell_ricochet` turns into a
    // `RicochetKeyframe`; and every authoritative armor TERMINAL (embed/perforation) raises a
    // `ShellTerminal` that `on_shell_terminal` turns into an `ImpactConfirm` — all broadcast to every
    // client inside a sliding-window `FireBurst` (`FireRings`). Registered here and NOWHERE shared,
    // so the client's own local `FireShell` never tries to send. (`ShotId` stamping is NOT here:
    // `protocol::stamp_shot_ids` runs on BOTH ends, so the server shell that ricochets/terminates and
    // the client's own predicted shell carry the same id.)
    app.init_resource::<FireRings>();
    app.add_observer(broadcast_fire);
    app.add_observer(on_shell_ricochet);
    app.add_observer(on_shell_terminal);
    // THE SENDER. The three observers above only PUSH into the redundancy window; this one system
    // broadcasts it — every tick the window is non-empty, which is what actually makes the window
    // redundant (see `broadcast_fire_window`). `.after(GameplaySet)`: `shooting::fire` and
    // `integrate_projectiles` both raise their events inside `GameplaySet`, and the ordering edge is a
    // sync point, so the observers have already flushed their pushes when this runs — this tick's
    // events go out on this tick, with zero added latency.
    app.add_systems(FixedUpdate, broadcast_fire_window.after(GameplaySet));
    // Server-authoritative input authorization: strip any `InputTarget::Entity` a client is NOT the
    // `ControlledBy` owner of, before the input buffer ever applies it. lightyear ships this as an
    // opt-in `InputSystems::ValidateInputs` system (`add_input_validator` = sugar for
    // `add_systems(PreUpdate, .in_set(ValidateInputs))`, lightyear_inputs server.rs:153-160) and does
    // NOT enable it by default — `ControlledBy` is an optional ownership model. We DO use it (every
    // player tank is spawned `ControlledBy` its link), so register the built-in
    // `authorize_controlled_targets` for our native `TankCommand` sequence. Placed on the SERVER only:
    // it queries the server-side `MessageReceiver<InputMessage<S>>` + `ControlledByRemote` (which only
    // exist on the authority) and is the enforcement point for "a client cannot drive a tank it does
    // not own" — the second line of defense behind unique client ids, in case a client is modified to
    // forge `InputTarget::Entity(opponent)`. Inert on the client (no such receiver), but scoped here so
    // authorization is unambiguously an authority concern.
    app.add_input_validator(authorize_controlled_targets::<NativeStateSequence<TankCommand>>);
    // Give every remote client link a `ReplicationSender`. lightyear's per-client visibility hooks
    // (`ReplicationTarget::on_insert` / `ControlledBy::on_insert`) only set the hide/show bits for
    // `Predicted`/`Interpolated`/`Controlled` on links that carry `ReplicationSender` (or `HostClient`);
    // without it those `on_insert` hooks no-op, and an UNSET visibility bit defaults to VISIBLE — so all
    // three ownership markers broadcast to EVERY client and each one predicts + claims input on EVERY
    // tank (the "one player controls both tanks" leak). The one visibility path that isn't
    // `ReplicationSender`-gated (`handle_new_client_visibility`) fires only at connect, before the tanks
    // asset-load-gate-spawn in `spawn_pending_tanks`, so it never covers them — the spawn-time path is
    // always `on_insert`. Replication itself still works (replicon's `ConnectedClient` drives the send),
    // which is why only the ownership markers leaked. Canonical lightyear examples add this in their
    // `On<Add, LinkOf>` handler; `ReplicationSender` is a unit marker used solely by the visibility
    // hooks (it does not start a duplicate send loop), so inserting it is safe.
    app.add_observer(attach_replication_sender);

    let server = app
        .world_mut()
        .spawn((
            Name::new("Server"),
            NetcodeServer::new(NetcodeConfig {
                // The protocol-compatibility guard's server end: only a client whose connect token was
                // built with the SAME `PROTOCOL_FINGERPRINT` decrypts here (netcode folds `protocol_id`
                // into the token AEAD), so a version/wire-skewed client is refused at the handshake
                // rather than admitted into a replication stream it cannot apply. Must match the
                // client's `Authentication::Manual.protocol_id` (`net::client`). A skewed client is
                // transport-indistinguishable from a down server (the request is silently dropped) —
                // the client-side overlay surfaces that as a combined hint after N failed attempts.
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
            // The bot's death→respawn loop: schedule 5 s out when its root gains `TankKnockedOut`,
            // then sweep the dead bot (and any detached launched turret) and spawn a fresh one.
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
            // Steer the bot each tick — in `GameplaySet` before the edge-consumer, like every other
            // command writer; a direct `TankCommand` write (the bot carries no `ActionState`, so
            // `bridge_action_state_to_tank_command` skips it).
            drive_bot.in_set(GameplaySet).before(ConsumeCommandEdges),
            // Honor a dead client's respawn edge — a command READER, so in `GameplaySet` before the
            // edge-consumer, alongside `drive_bot`, so it sees this tick's bridged `respawn` edge
            // before `consume_edges` clears it.
            respawn_player_tanks
                .in_set(GameplaySet)
                .before(ConsumeCommandEdges),
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

/// Insert [`ReplicationSender`] onto every remote client link the moment it spawns. This is the gate
/// lightyear's per-client `Predicted`/`Interpolated`/`Controlled` visibility hooks require — see the
/// registration in [`run`] for the full rationale. `On<Add, LinkOf>` fires structurally, before both
/// `spawn_pending_tanks` and `spawn_bot`, so the sender is always present when a tank's `on_insert`
/// visibility path runs.
pub(super) fn attach_replication_sender(add: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(add.entity).insert(ReplicationSender);
}

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
        let root = spawn_player_tank(
            &mut commands,
            geometry,
            spec,
            &assets,
            link,
            client_id,
            spawn_pos,
            spawn_rot,
        );
        if config.perturb {
            commands.entity(root).insert(harness::PendingPerturbation {
                at: time.elapsed() + Duration::from_secs(2),
            });
        }
    }
}

/// Build one owned player tank (root + observed view binding + synchronous sim skeleton) at
/// `spawn_pos`/`spawn_rot`, owned + predicted by `client_id`'s `link`, and return its root. Shared by
/// the connect-time [`spawn_pending_tanks`] and the death-time [`respawn_player_tanks`] loop so a
/// respawn re-acquires through the EXACT same ownership/prediction path a fresh join does — the
/// known lightyear gotcha ([`attach_replication_sender`]) means the ownership markers only land
/// correctly through this one component set, so both entry points must share it verbatim. The
/// harness perturbation stays with the connect-time caller (a startup test concern, not a respawn's).
#[expect(
    clippy::too_many_arguments,
    reason = "the full owned-tank spawn contract — pose, assets, and ownership identity — is \
              exactly what both call sites must agree on; bundling it into a struct would only \
              move the same fields"
)]
fn spawn_player_tank(
    commands: &mut Commands,
    geometry: &TankGeometry,
    spec: &TankSpec,
    assets: &PendingTankAssets,
    link: Entity,
    client_id: PeerId,
    spawn_pos: Vec3,
    spawn_rot: Quat,
) -> Entity {
    let mut tank = commands.spawn((
        Name::new("Tank"),
        rig::net_tank_rig(assets),
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
        // The authority's atomic combat snapshot (`net::publish_net_crew` fills it once the rig's
        // volumes exist): per-volume HP + per-seat occupancy/aliveness + in-flight swap, replicated
        // so the client's damage/death/crew bar are server-driven.
        NetCrew::default(),
        // `None` until the turret cooks off (`net::publish_launched_turret_pose` fills it), so
        // the client can show the authoritative toss it does not simulate locally.
        LaunchedTurretPose::default(),
        // Per-weapon belt supply (`net::publish_net_belts` fills it once the rig's weapons exist),
        // replicated so the client's belt-fed fire-gating snaps to server truth.
        NetBelts::default(),
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
    tank.observe(bind_tank_view);
    let root = tank.id();
    spawn_tank_sim(commands, root, geometry, spec);
    root
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
    let root = spawn_bot_entity(&mut commands, &assets, geometry, spec);
    info!("server: spawned circling bot tank {root} (OVERMATCH_BOT)");
}

/// Build one ownerless bot tank (root + observed view binding + synchronous sim skeleton) and
/// return its root. Shared by the initial [`spawn_bot`] and the [`respawn_dead_bots`] loop so both
/// produce a byte-identical bot from the same spawn pose; the env gate and one-shot guard stay in
/// `spawn_bot`, the death timing stays in the respawn systems.
fn spawn_bot_entity(
    commands: &mut Commands,
    assets: &PendingTankAssets,
    geometry: &TankGeometry,
    spec: &TankSpec,
) -> Entity {
    let mut tank = commands.spawn((
        Name::new("Bot"),
        Bot,
        // Replicated bot marker: `Name` doesn't ride the wire, so this is how the client's HUD
        // recognizes the bot to prefix its nameplate with `[BOT]`.
        super::protocol::NetBot,
        rig::net_tank_rig(assets),
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
        // Atomic combat snapshot for the bot too, so a client kills it from replicated state.
        NetCrew::default(),
        // Launched-turret pose datum for the bot too (`None` until its turret cooks off).
        LaunchedTurretPose::default(),
        // Per-weapon belt supply for the bot too (server truth for its belt-fed weapons).
        NetBelts::default(),
        Replicate::to_clients(NetworkTarget::All),
        DisableReplicateHierarchy,
        // NO `PredictionTarget`, NO `ControlledBy`: no client owns or predicts the bot, so on every
        // client the replicated root lands `Interpolated` only — a Static local body — which is
        // exactly the remote-tank path this rig exists to exercise solo.
        InterpolationTarget::to_clients(NetworkTarget::All),
    ));
    tank.observe(bind_tank_view);
    let root = tank.id();
    spawn_tank_sim(commands, root, geometry, spec);
    root
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
/// pose. Same asset gate as [`spawn_bot`]/[`spawn_pending_tanks`] (long-loaded by the time any bot
/// dies, but resolved identically so the spawn path never diverges).
fn respawn_dead_bots(
    dead: Query<(Entity, &BotRespawnAt, &Rig), With<Bot>>,
    assets: Option<Res<PendingTankAssets>>,
    asset_server: Res<AssetServer>,
    source: TankSimSource,
    time: Res<Time<Virtual>>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs();
    // (root, its Rig.turret) for every bot now due. Capture the turret handle BEFORE despawning the
    // root: if the bot cooked off, `damage::launch_turrets_on_cookoff` stripped the turret's
    // `ChildOf` and made it a free body, so it is NOT a descendant of the root and the recursive
    // root despawn below would miss it — leaking one launched turret per respawn.
    let due: Vec<(Entity, Entity)> = dead
        .iter()
        .filter(|(_, at, _)| now >= at.0)
        .map(|(root, _, rig)| (root, rig.turret))
        .collect();
    if due.is_empty() {
        return;
    }
    let Some(assets) = assets else { return };
    if !assets.loaded(&asset_server) {
        return;
    }
    let Some((geometry, spec)) = source.get(&assets.spec) else {
        return;
    };
    for (root, turret) in due {
        // Recursive despawn sweeps the root and its attached rig (children + relationship targets).
        commands.entity(root).despawn();
        // The launched turret, if it detached on cookoff. `try_despawn` is a silent no-op when the
        // turret is still an attached child (already swept above) or otherwise gone — no panic, no
        // double-free, so the one branch covers both the cookoff and crew-loss deaths.
        commands.entity(turret).try_despawn();
        let fresh = spawn_bot_entity(&mut commands, &assets, geometry, spec);
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
    dead: Query<(Entity, &TankCommand, &ControlledBy), With<TankKnockedOut>>,
    remotes: Query<&RemoteId>,
    assets: Option<Res<PendingTankAssets>>,
    asset_server: Res<AssetServer>,
    source: TankSimSource,
    mut lane: ResMut<SpawnLane>,
    mut commands: Commands,
) {
    // (dead root, owner link, owner client id) for every owned tank that both IS dead and asked to
    // respawn this tick. Resolve the owner's `RemoteId` up front (the `PeerId` the fresh tank must be
    // predicted/owned by); an owner link mid-disconnect with no `RemoteId` is skipped rather than
    // respawned to a client that is leaving.
    let requests: Vec<(Entity, Entity, PeerId)> = dead
        .iter()
        .filter(|(_, command, _)| command.respawn)
        .filter_map(|(root, _, controlled)| {
            remotes
                .get(controlled.owner)
                .ok()
                .map(|remote| (root, controlled.owner, remote.0))
        })
        .collect();
    if requests.is_empty() {
        return;
    }
    // Same asset gate as the spawn/respawn loops (long-loaded by the time anyone can die, but resolved
    // identically so the spawn path never diverges).
    let Some(assets) = assets else { return };
    if !assets.loaded(&asset_server) {
        return;
    }
    let Some((geometry, spec)) = source.get(&assets.spec) else {
        return;
    };
    // A respawn takes the NEXT free lane (never reset — same rule a reconnecting client follows), so a
    // fresh tank never lands on top of another body and NaNs the solver. The base pose honors the
    // `SPIKE_SPAWN_POSE` harness override exactly as the connect path does.
    let (base_pos, spawn_rot) =
        harness::spawn_pose().unwrap_or((Vec3::new(0.0, 2.0, 0.0), Quat::IDENTITY));
    for (root, link, client_id) in requests {
        let spawn_pos = base_pos + lane_offset(lane.0);
        lane.0 += 1;
        // Recursive despawn sweeps the dead root and its attached rig; the `On<Remove, Rig>` observer
        // handles any cooked-off turret that had detached (see the system doc).
        commands.entity(root).despawn();
        let fresh = spawn_player_tank(
            &mut commands,
            geometry,
            spec,
            &assets,
            link,
            client_id,
            spawn_pos,
            spawn_rot,
        );
        info!("server: player {client_id} respawn requested — swept {root}, spawned {fresh}");
    }
}

/// Drive the bot in a steady circle AND hold its main gun's trigger: constants written straight into
/// its own `TankCommand` (`command.rs`'s read contract, attached by `command`'s `On<Add, Tank>`
/// observer). The bot carries no `ActionState`, so `bridge_action_state_to_tank_command` (protocol.rs)
/// never touches it — this is the sole writer. Ordered in `GameplaySet` before the edge-consumer,
/// with the other command writers; the fields are levels (never cleared), so it circles and fires for
/// good. Firing makes the bot a self-firing target that exercises the opponent-fire path (the `net`
/// `FireEvent` tracer, and the remote-recoil / hit-reaction slices to come) solo — no second client.
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

/// The server's sliding redundancy windows: recent [`FireEvent`]s, [`RicochetKeyframe`]s, and
/// [`ImpactConfirm`]s, resent in EVERY [`FireBurst`] so one delivered burst repairs a multi-packet
/// loss of any stream (piece 3 — the input-redundancy pattern applied to cosmetics).
///
/// **Retention is TIME-based, not per-stream count-based (F4).** The window keeps every entry younger
/// than [`FIRE_RETAIN_TICKS`] regardless of how many tanks are firing. A fixed depth-N ring is a
/// *single global* ring across all shooters, so with k simultaneous shooters the per-event survival
/// window divides by k — at two MGs a depth-4 ring covers only ~120 ms, inside a routine WAN burst
/// loss ("tracers vanish in big fights"). Sizing by age instead makes each event's redundancy
/// independent of shooter count: a bounce/fire/confirm rides every burst for ~[`FIRE_RETAIN_TICKS`]
/// worth of ticks, which is tuned to cover the consumer's grace window (`RICOCHET_HOLD_TICKS` ≈ 16
/// ticks / ~250 ms — a keyframe older than that has either been consumed or already dissolved) plus
/// send jitter. [`FIRE_WINDOW_MAX`] is a defensive per-stream cap so a pathological fire rate can't
/// unbound a burst; a duel never approaches it.
///
/// **The window is broadcast by the CLOCK, not by new traffic ([`broadcast_fire_window`]).** This is
/// what makes the redundancy real, and it is the fix for the flake the owner saw ("the second client
/// does not always see the post-bounce shell"). Retention alone is not redundancy: an entry is only
/// *re-sent* if a burst actually goes out while it is still in the window. When the send was driven by
/// the events themselves (one burst per fire/ricochet/terminal), the resends an entry got were exactly
/// the events that happened to FOLLOW it within [`FIRE_RETAIN_TICKS`] — so the redundancy scaled with
/// the shooter's fire rate and vanished at the bottom:
///
/// - **88 main gun** (`Single(reload_secs: 3.0)` — one shot per ~192 ticks, ~10× the window): a
///   ricochet keyframe is followed by NOTHING (the bounced round ends in terrain, which never
///   confirms). It rode **exactly one datagram**. One dropped packet = no carry-through, for good —
///   the observer's shell holds at the plate and quietly dissolves. Identically for the `ImpactConfirm`
///   of an isolated 88 armor hit: one datagram, and a drop costs the honest armor read (post-F3 that
///   degrades to a SILENT dissolve — no spark at all) on the observer AND on the shooter's own shell.
/// - **MGs** (`Automatic(rpm: 750)` — a round every ~5 ticks): a keyframe is re-carried by the ~4
///   fires that follow it inside the window, so it rode ~5 datagrams and effectively always landed.
///
/// Same code, same window, 5× the loss resilience purely because the gun fired faster — the exact
/// shape of "works sometimes." Sending the window on the clock instead decouples redundancy from fire
/// rate: every entry rides ~[`FIRE_RETAIN_TICKS`] consecutive bursts (~312 ms) whatever fired it.
#[derive(Resource, Default)]
pub(super) struct FireRings {
    fires: std::collections::VecDeque<FireEvent>,
    keyframes: std::collections::VecDeque<RicochetKeyframe>,
    confirms: std::collections::VecDeque<ImpactConfirm>,
}

/// How long (in ticks) a redundancy entry rides every burst before it ages out — see [`FireRings`].
/// ~20 ticks ≈ 312 ms at 64 Hz: comfortably past the consumer's ~250 ms grace window
/// (`ballistics::RICOCHET_HOLD_TICKS`) plus send jitter, so a valid keyframe/confirm survives long
/// enough for the shell that consumes it, and no longer.
const FIRE_RETAIN_TICKS: i32 = 20;

/// Defensive hard cap on entries retained per stream — bounds a burst's size under a pathological fire
/// rate (a stream firing faster than one round per few ms would otherwise let the time window grow the
/// vec unboundedly). A 1v1 duel at MG cyclic rate keeps ~3–4 fires in flight, far under this.
const FIRE_WINDOW_MAX: usize = 32;

/// A redundancy-window entry that carries the server tick it was stamped on, so [`FireRings::push`] can
/// evict by age. All three wire streams already carry their tick as a field.
trait BurstEntry {
    fn tick(&self) -> Tick;
}
impl BurstEntry for FireEvent {
    fn tick(&self) -> Tick {
        self.fire_tick
    }
}
impl BurstEntry for RicochetKeyframe {
    fn tick(&self) -> Tick {
        self.bounce_tick
    }
}
impl BurstEntry for ImpactConfirm {
    fn tick(&self) -> Tick {
        self.impact_tick
    }
}

impl FireRings {
    /// Prune one ring: drop everything older than [`FIRE_RETAIN_TICKS`] relative to `now` (the current
    /// server tick), then enforce the defensive [`FIRE_WINDOW_MAX`] cap. Entries are appended in tick
    /// order, so eviction is always from the front. `Tick - Tick` is lightyear's wrapping i32
    /// difference, correct across the u32 tick boundary.
    fn prune_ring<T: BurstEntry>(ring: &mut std::collections::VecDeque<T>, now: Tick) {
        while let Some(front) = ring.front() {
            if now - front.tick() > FIRE_RETAIN_TICKS {
                ring.pop_front();
            } else {
                break;
            }
        }
        while ring.len() > FIRE_WINDOW_MAX {
            ring.pop_front();
        }
    }
    /// Push a fresh entry, pruning the ring around it.
    fn push<T: BurstEntry>(ring: &mut std::collections::VecDeque<T>, item: T, now: Tick) {
        ring.push_back(item);
        Self::prune_ring(ring, now);
    }
    /// Age out every stream. Called once per tick by [`broadcast_fire_window`] — the pushes prune their
    /// OWN ring, but a stream that has stopped receiving pushes (the isolated 88 shot: one keyframe and
    /// then silence) would otherwise keep re-riding every burst forever, since nothing else would ever
    /// evict it. Pruning on the clock is what lets `is_empty` below actually go quiet again.
    fn prune(&mut self, now: Tick) {
        Self::prune_ring(&mut self.fires, now);
        Self::prune_ring(&mut self.keyframes, now);
        Self::prune_ring(&mut self.confirms, now);
    }
    /// Nothing recent to re-send — the state the window sits in for all but the ~312 ms after each
    /// event, and what keeps the per-tick broadcast free between shots.
    fn is_empty(&self) -> bool {
        self.fires.is_empty() && self.keyframes.is_empty() && self.confirms.is_empty()
    }
    /// THE SEND DECISION, in one place: age the window to `now`, then send iff anything is still in it.
    /// [`broadcast_fire_window`] is this plus the send; the tests drive it directly to count the
    /// datagrams a given event stream actually produces (which is the whole bug — see [`FireRings`]).
    fn should_send(&mut self, now: Tick) -> bool {
        self.prune(now);
        !self.is_empty()
    }
    /// A burst carrying the whole current window of all three streams (what a sequenced-unreliable
    /// delivery needs so dropping a stale burst loses nothing the next re-carries).
    fn burst(&self) -> FireBurst {
        FireBurst {
            fires: self.fires.iter().cloned().collect(),
            keyframes: self.keyframes.iter().cloned().collect(),
            confirms: self.confirms.iter().cloned().collect(),
        }
    }
}

/// Broadcast the redundancy window every tick it is non-empty — the ONE send site for [`FireBurst`],
/// and the thing that makes [`FireRings`]' retention actually redundant (the full decode of the flake
/// this fixes is on `FireRings`; ADR-0021's "redundancy, not reliability" is what it implements).
///
/// The three observers (`broadcast_fire` / `on_shell_ricochet` / `on_shell_terminal`) now only PUSH.
/// Driving the send from the events themselves tied an entry's resend count to whatever traffic
/// happened to follow it, which is zero for a single-shot gun: an isolated 88 keyframe/confirm rode
/// exactly one datagram and a single packet drop lost the bounce (or the armor read) outright.
///
/// **Latency is unchanged.** `.after(GameplaySet)` puts this behind the sync point that flushes the
/// observers' pushes, so an event raised this tick is broadcast on this tick — the same tick the old
/// event-driven send used. **Bandwidth is bounded, and in the busy case this is a REDUCTION:** the old
/// path sent one FULL burst per event (an MG round + a ricochet + a confirm landing on the same tick
/// sent the whole window three times over); this sends the window at most once per tick no matter how
/// many events it carries. The extra cost is entirely in the quiet case — an isolated shot now costs
/// ~[`FIRE_RETAIN_TICKS`] small datagrams (~312 ms of resends) instead of one — and the window is EMPTY
/// (zero sends) whenever nothing has been fired in the last ~312 ms, which is most of a match.
///
/// Cheap and unconditional: no `Server` yet (no client linked) is the only early-out besides an empty
/// window. The channel is unreliable/sequenced by design, so a failed send is a `debug!`, not an error.
///
/// `pub(super)` so the loss-injected E2E tripwire (`net::shot_loss`) mounts the REAL send site: with
/// the send no longer riding the observers, a harness that registers only the three push observers
/// would broadcast nothing at all — which is precisely why the tripwire is worth more against this
/// architecture than against the old one (it now exercises the window sender itself, under loss).
pub(super) fn broadcast_fire_window(
    servers: Query<&Server>,
    timeline: Res<LocalTimeline>,
    mut rings: ResMut<FireRings>,
    // Shot-lifecycle recorder (`SPIKE_SHOT_TRACE`), absent unless armed: the `send` rows — one per
    // event this datagram CARRIES. This is the one row the recorder could not meaningfully write
    // before: under the old event-driven send, "emitted" and "sent" were the same instant and a `send`
    // row would have been a restatement of `fire`/`kf`/`cf`. Now they are different moments, and HOW
    // MANY copies an event actually got is the exact property this system exists to guarantee — so the
    // instrument measures it instead of assuming it (`scripts/shot/analyze.py`: copies-per-event).
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
    mut sender: ServerMultiMessageSender,
) {
    // Age the window on the clock first, then send iff anything is still in it: a stream nobody is
    // pushing to any more (the isolated shot) must still fall out of the window, or it would re-ride
    // every burst for the rest of the match.
    if !rings.should_send(timeline.tick()) {
        return;
    }
    let Ok(server) = servers.single() else {
        return;
    };
    if let Err(err) =
        sender.send::<FireBurst, FireChannel>(&rings.burst(), server, &NetworkTarget::All)
    {
        debug!("server: FireBurst broadcast dropped: {err}");
        // No datagram left the box, so no `send` row is written for this tick's window: the recorder
        // must count copies that were actually SENT, not copies that were merely eligible.
        return;
    }
    record_sent_copies(&mut shot_trace, &rings, timeline.tick());
}

/// One `send` row per event carried by the burst just broadcast — the copies-per-event ledger
/// (`crate::shot_trace`, `SPIKE_SHOT_TRACE`; absent unless armed, which is the only cost an unrecorded
/// run pays: one `Option` check per tick the window is non-empty).
///
/// **What a `send` row means, and what it is NOT.** It is a TRANSMISSION: this event rode this tick's
/// datagram. The emission rows (`fire` / `kf` / `cf`, written by the three push observers) are a
/// different fact — the tick the shot/bounce/impact HAPPENED — and the analyzer's arrival-lead
/// arithmetic keys off those. Counting the `send` rows for one `ShotId` + stream gives the number of
/// datagram copies that event actually got: exactly the quantity [`broadcast_fire_window`] was written
/// to raise off the floor (the isolated 88 bounce rode ONE copy under the old event-driven send), and
/// the number the client's `fire_rx`/`kf_rx` `dup` verdicts are the receiving end of.
///
/// `c` is the event's AGE in ticks at this send (0 on the tick it was emitted). With a linked client
/// the window goes out every tick it is non-empty, so `c` also reads as the copy's 0-based ordinal —
/// but the analyzer counts rows rather than trusting that, because a burst the sender failed to emit
/// (or a window that filled while no client was linked) breaks the identity and the instrument must
/// report the copies that happened, not the copies that should have.
fn record_sent_copies(
    trace: &mut Option<ResMut<crate::shot_trace::ShotTrace>>,
    rings: &FireRings,
    now: Tick,
) {
    if trace.is_none() {
        return;
    }
    for event in &rings.fires {
        let age = (now - event.fire_tick).max(0);
        crate::shot_trace::record(
            trace,
            "send",
            now.0,
            event.shot_id(),
            || json!({ "s": "fire", "c": age }),
        );
    }
    for keyframe in &rings.keyframes {
        let age = (now - keyframe.bounce_tick).max(0);
        crate::shot_trace::record(
            trace,
            "send",
            now.0,
            keyframe.shot,
            || json!({ "s": "kf", "c": age, "seq": keyframe.sequence }),
        );
    }
    for confirm in &rings.confirms {
        let age = (now - confirm.impact_tick).max(0);
        crate::shot_trace::record(
            trace,
            "send",
            now.0,
            confirm.shot,
            || json!({ "s": "cf", "c": age }),
        );
    }
}

/// Turn each authoritative `FireShell` into a cosmetic `FireEvent` and PUSH it onto the sliding
/// redundancy window — the SERVER half of the opponent-fire seam (`net::protocol::FireEvent`/
/// `FireBurst`). Observes every shot the sim fires; a shot that names a tank (`shooter: Some`) enters
/// the fire ring, while sandbox/`None` shots (no tank) never broadcast. [`broadcast_fire_window`] does
/// the sending — on this same tick, and on every tick after it until the entry ages out (which is what
/// makes the window redundant; see [`FireRings`]).
///
/// **Targeting: `All`, deduped at the receiver.** Every burst goes to every client; a client drops any
/// FIRE naming a tank IT simulates locally (`receive_fire_events`' `locally_fired` guard), so the
/// shooter discarding its own echo is a receiver concern. This is what lets ONE shared burst carry
/// events from MULTIPLE shooters correctly — an `AllExceptSingle(owner)` target could not, since a
/// burst re-carrying another shooter's fires must still reach this owner. The one-frame self-echo the
/// owner discards is negligible, and the redundancy window (which re-carries older shooters' events) is
/// only correct under `All`. KEYFRAMES are not dropped for own shots — the shooter's own shell consumes
/// its bounce (the fall-of-shot read), which also REQUIRES the `All` target here.
pub(super) fn broadcast_fire(
    fire: On<FireShell>,
    // The server's simulation tick. `shooting::fire` raises `FireShell` inside `FixedUpdate`, in
    // `GameplaySet`, AFTER `LocalTimeline` is incremented for this tick (`increment_local_tick` runs
    // in `FixedFirst`); nothing advances it again until the next frame's `FixedFirst`. So this
    // observer — even deferred to the command flush — reads the SAME tick the firing sim step ran on,
    // which is exactly the tick the muzzle pose in `fire.origin` was computed for (and the tick
    // `stamp_shot_ids` stamps into this shell's `Shot`, so a later ricochet keyframe keys the same id).
    // It is also the tick `broadcast_fire_window` (`.after(GameplaySet)`, same tick) stamps its send.
    timeline: Res<LocalTimeline>,
    mut rings: ResMut<FireRings>,
    // Shot-lifecycle recorder (`SPIKE_SHOT_TRACE`), absent unless armed: the authority's `fire` row —
    // the head of every shot's cross-process lifecycle (see `crate::shot_trace`). It records the shot
    // being FIRED, which is what this observer now does; the DATAGRAMS it rides are a separate fact,
    // recorded (as `send` rows) by `broadcast_fire_window`, which is where sending now lives.
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    // Only tank-attributed shots broadcast; a `None` shooter (the sandbox) has no tank to name.
    let Some(source) = fire.shooter else {
        return;
    };
    // The weapon slot rides the wire as a `u8` (ample — a tank carries a handful of weapons). A
    // silent `as u8` would wrap a slot >= 256 mod 256 and recoil a VALID-BUT-WRONG barrel on every
    // remote client; unreachable today, but skip-with-warn on overflow rather than truncate, the
    // same fail-loudly-or-skip discipline as the `Dir3` bore guard (and matches `stamp_shot_ids`).
    let Ok(weapon) = u8::try_from(source.weapon) else {
        warn!(
            "server: weapon slot {} exceeds u8 — skipping FireEvent broadcast for this shot",
            source.weapon
        );
        return;
    };
    let event = FireEvent {
        origin: fire.origin,
        // Carry the bore as a plain `Vec3`; the receiver reconstructs `Dir3` behind a guard.
        direction: fire.direction.as_vec3(),
        speed: fire.speed,
        caliber: fire.caliber,
        mass: fire.mass,
        // The belt decided this round's tracer-ness on the authoritative fire; broadcast it so every
        // remote client dresses the shell identically.
        tracer: fire.tracer,
        shooter: source.tank,
        // Which weapon fired — the receiver derives THIS shot's barrel recoil from its own local
        // spec keyed by this slot.
        weapon,
        // The tick this shot fired on, so the receiver can fast-forward the tracer to where the
        // shell already is in its confirmed timeline (see `FireEvent::fire_tick`).
        fire_tick: timeline.tick(),
    };
    crate::shot_trace::record(
        &mut shot_trace,
        "fire",
        timeline.tick().0,
        event.shot_id(),
        || {
            json!({
                "o": [event.origin.x, event.origin.y, event.origin.z],
                "tr": event.tracer,
                "cal": event.caliber,
            })
        },
    );
    FireRings::push(&mut rings.fires, event, timeline.tick());
}

/// Turn each authoritative `ballistics::ShellRicochet` into a `RicochetKeyframe` and PUSH it onto the
/// redundancy window — the SERVER half of the bounce carry-through (ADR-0016: replicate the CAUSE;
/// every client — observers AND the shooter — derives the re-seed). The authority march raises
/// `ShellRicochet` at the moment it resolves a bounce for a net-attributed shell (`Shot` present,
/// stamped by the shared `protocol::stamp_shot_ids`); this stamps the bounce tick from the server
/// timeline. [`broadcast_fire_window`] sends it this tick and re-sends it every tick until it ages out
/// — the redundancy an isolated 88 bounce never used to get (see [`FireRings`]). Same `FireRings`/`All`
/// window + targeting as `broadcast_fire`: one burst carries every stream, so they share the redundancy
/// for free. `ShellRicochet` is a sim-layer event, so the sandbox raises it too — but this observer is
/// SERVER-ONLY (registered only in the server plugin), so it never fires off the net.
///
/// `pub(super)` so the loss-injected E2E tripwire (`net::shot_loss`) can mount the production wiring.
pub(super) fn on_shell_ricochet(
    ricochet: On<ShellRicochet>,
    timeline: Res<LocalTimeline>,
    mut rings: ResMut<FireRings>,
    // Shot-lifecycle recorder: the `kf` row — the tick the authority RESOLVED the bounce, which is the
    // tick the client's hold is racing. Recorded here, at the push, not at `broadcast_fire_window`: the
    // bounce HAPPENED once, on this tick, and the analyzer's arrival-lead (`recv_tick − bounce_tick`)
    // is only the quantity that sizes `RICOCHET_HOLD_TICKS` if this row means the resolution, not a
    // transmission. The window's re-sends of it are `send` rows.
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    crate::shot_trace::record(
        &mut shot_trace,
        "kf",
        timeline.tick().0,
        ricochet.shot,
        || json!({ "seq": ricochet.sequence }),
    );
    let keyframe = RicochetKeyframe {
        shot: ricochet.shot,
        origin: ricochet.origin,
        direction: ricochet.direction,
        speed: ricochet.speed,
        // Stamped here (the sim can't read the timeline); the observer re-ages by hold duration, so
        // this is carried for audit — see `RicochetKeyframe::bounce_tick`.
        bounce_tick: timeline.tick(),
        sequence: ricochet.sequence,
    };
    FireRings::push(&mut rings.keyframes, keyframe, timeline.tick());
}

/// Turn each authoritative `ballistics::ShellTerminal` (an embed/perforation — the shot's END on
/// armor) into an `ImpactConfirm` and PUSH it onto the redundancy window — the SERVER half of the
/// terminal confirm that completes the shot state machine (ADR-0016: replicate the CAUSE; every client
/// — observers AND the shooter — renders the honest armor read from it). The authority march raises
/// `ShellTerminal` at most once per shot (see its doc); this stamps the impact tick from the server
/// timeline. [`broadcast_fire_window`] sends and re-sends it — and an isolated 88 hit's confirm is the
/// OTHER casualty of the old event-driven send (one datagram; a drop cost the honest armor read
/// outright, degrading it post-F3 to a silent dissolve on the observer AND on the shooter's own shell —
/// see [`FireRings`]). Same `FireRings`/`All` window + targeting as `broadcast_fire`; SERVER-ONLY for
/// the same reason as `on_shell_ricochet`.
pub(super) fn on_shell_terminal(
    terminal: On<ShellTerminal>,
    timeline: Res<LocalTimeline>,
    mut rings: ResMut<FireRings>,
    // Shot-lifecycle recorder: the `cf` row — the shot's authoritative END, stamped at the tick the
    // authority resolved it (same emission-not-transmission rule as `on_shell_ricochet`'s `kf`). A shot
    // with a `cf` here and no `cf_rx`/`end` on the client is the never-consumed class the analyzer
    // counts — and the `send` rows now say whether that shot's confirm ever left the box at all.
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    crate::shot_trace::record(
        &mut shot_trace,
        "cf",
        timeline.tick().0,
        terminal.shot,
        || json!({ "pen": terminal.penetrated, "ab": terminal.after_bounces }),
    );
    let confirm = ImpactConfirm {
        shot: terminal.shot,
        position: terminal.position,
        normal: terminal.normal,
        penetrated: terminal.penetrated,
        // Stamped here (the sim can't read the timeline); the client resolves on receipt, so this is
        // carried for audit — see `ImpactConfirm::impact_tick`.
        impact_tick: timeline.tick(),
        after_bounces: terminal.after_bounces,
    };
    FireRings::push(&mut rings.confirms, confirm, timeline.tick());
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A fire event stamped at `tick` — the only field the F4 windowing reads is `fire_tick`.
    fn fire_at(tick: u32) -> FireEvent {
        FireEvent {
            origin: Vec3::ZERO,
            direction: Vec3::X,
            speed: 800.0,
            caliber: 0.0079,
            mass: 0.0118,
            tracer: true,
            shooter: Entity::PLACEHOLDER,
            weapon: 0,
            fire_tick: Tick(tick),
        }
    }

    /// F4: retention is by AGE, not by per-stream count. A depth-4 ring would have evicted the first
    /// fire after four later ones (the multi-shooter divide-by-k bug); the time window keeps every
    /// entry younger than `FIRE_RETAIN_TICKS` regardless of how many arrive in between.
    #[test]
    fn window_retains_by_age_not_by_count() {
        let mut ring = std::collections::VecDeque::new();
        FireRings::push(&mut ring, fire_at(100), Tick(100));
        // Ten more fires (other shooters) over the next ten ticks — all inside the retain window.
        for t in 101..=110u32 {
            FireRings::push(&mut ring, fire_at(t), Tick(t));
        }
        assert!(
            ring.iter().any(|f| f.fire_tick == Tick(100)),
            "the tick-100 fire survives ten later fires — age-based retention, not count",
        );

        // Advance one tick past the retain window: the tick-100 entry ages out.
        let now = 100 + FIRE_RETAIN_TICKS as u32 + 1;
        FireRings::push(&mut ring, fire_at(now), Tick(now));
        assert!(
            !ring.iter().any(|f| f.fire_tick == Tick(100)),
            "past FIRE_RETAIN_TICKS the entry ages out of the window",
        );
    }

    /// F4: the defensive cap bounds the burst even when entries arrive faster than the time window
    /// would evict (here all on one tick, so the age prune never fires).
    #[test]
    fn window_cap_bounds_the_burst() {
        let mut ring = std::collections::VecDeque::new();
        for _ in 0..(FIRE_WINDOW_MAX + 10) {
            FireRings::push(&mut ring, fire_at(200), Tick(200));
        }
        assert_eq!(
            ring.len(),
            FIRE_WINDOW_MAX,
            "the cap bounds the ring under same-tick churn",
        );
    }

    /// A ricochet keyframe stamped at `tick` — the windowing only reads `bounce_tick`.
    fn keyframe_at(tick: u32) -> RicochetKeyframe {
        RicochetKeyframe {
            shot: crate::ShotId {
                shooter: Entity::PLACEHOLDER,
                weapon: 0,
                fire_tick: tick - 20,
            },
            origin: Vec3::ZERO,
            direction: Vec3::X,
            speed: 400.0,
            bounce_tick: Tick(tick),
            sequence: 0,
        }
    }

    /// Count the datagrams `should_send` emits over `ticks` ticks from `from`, given an event stream
    /// that pushes nothing further — i.e. how many copies of an already-pushed entry actually go out.
    fn sends_over(rings: &mut FireRings, from: u32, ticks: u32) -> Vec<u32> {
        (0..ticks)
            .filter(|t| rings.should_send(Tick(from + t)))
            .collect()
    }

    /// THE FLAKE, PINNED (owner, 2-client playtest: "the second client does not always see the
    /// post-bounce shell", on the 88). An isolated main-gun bounce produces exactly ONE event and then
    /// silence — the 88 reloads for 3.0 s (192 ticks, ~10× the retain window) and the deflected round
    /// ends in TERRAIN, which never confirms. When the send was driven by the events themselves, that
    /// was one datagram for the whole shot: a single dropped packet lost the carry-through outright,
    /// and the observer's shell quietly dissolved at the plate. Sending the window on the CLOCK gives
    /// the keyframe the resends the window always claimed to provide.
    ///
    /// The bar is the CONSUMER's grace window, not the retain window: the observer's shell holds at the
    /// plate for `ballistics::RICOCHET_HOLD_TICKS` (16) ticks, so only the copies sent inside that span
    /// can still be consumed. Every one of them is a fresh chance for the bounce to land.
    #[test]
    fn an_isolated_keyframe_rides_the_whole_window_not_one_datagram() {
        const BOUNCE: u32 = 1000;
        let mut rings = FireRings::default();
        FireRings::push(&mut rings.keyframes, keyframe_at(BOUNCE), Tick(BOUNCE));

        // Nothing else is ever pushed — this IS the 88's whole event stream for the shot.
        let sends = sends_over(&mut rings, BOUNCE, 64);

        assert_eq!(
            sends.len(),
            FIRE_RETAIN_TICKS as usize + 1,
            "the keyframe rides a burst on every tick of its retain window (was: one datagram, \
             total, and a single packet drop killed the bounce)",
        );
        // The consumer can only use what lands before its hold expires. 16 chances, not 1.
        let usable = sends.iter().filter(|t| **t < 16).count();
        assert!(
            usable >= 16,
            "at least RICOCHET_HOLD_TICKS copies land inside the observer's grace window, got \
             {usable}",
        );
    }

    /// The other half of the same bug: an isolated 88 ARMOR HIT's `ImpactConfirm` had exactly one
    /// datagram too (the shell ends — no further events at all), so a single drop cost the honest
    /// armor read on the observer AND on the shooter's own shell, degrading it to the post-F3 SILENT
    /// dissolve (no spark). Same window, same fix.
    #[test]
    fn an_isolated_confirm_rides_the_whole_window_too() {
        const IMPACT: u32 = 2000;
        let mut rings = FireRings::default();
        FireRings::push(
            &mut rings.confirms,
            ImpactConfirm {
                shot: crate::ShotId {
                    shooter: Entity::PLACEHOLDER,
                    weapon: 0,
                    fire_tick: IMPACT - 30,
                },
                position: Vec3::ZERO,
                normal: Vec3::Y,
                penetrated: true,
                impact_tick: Tick(IMPACT),
                after_bounces: 0,
            },
            Tick(IMPACT),
        );
        assert_eq!(
            sends_over(&mut rings, IMPACT, 64).len(),
            FIRE_RETAIN_TICKS as usize + 1,
            "an isolated armor confirm gets the same redundancy as everything else",
        );
    }

    /// And the window goes QUIET again: past the retain span, with nothing new pushed, the broadcast
    /// costs nothing. This is what bounds the per-tick send — the rings are empty for all but the
    /// ~312 ms after each event, which is most of a match (the 88 reloads for 3 s between shots).
    #[test]
    fn the_window_stops_sending_once_it_ages_out() {
        const FIRE: u32 = 500;
        let mut rings = FireRings::default();
        FireRings::push(&mut rings.fires, fire_at(FIRE), Tick(FIRE));

        let last = FIRE + FIRE_RETAIN_TICKS as u32;
        assert!(
            rings.should_send(Tick(last)),
            "still inside the retain window",
        );
        assert!(
            !rings.should_send(Tick(last + 1)),
            "past the window the ring is empty and the server sends nothing",
        );
        assert!(
            !rings.should_send(Tick(last + 1000)),
            "and it stays quiet — the clock-driven prune is what lets an unpushed stream drain",
        );
    }

    /// THE ORDERING THE FIX RESTS ON. Moving the send off the events and onto the clock is only free
    /// if this tick's events still ride THIS tick's burst — otherwise every tracer, bounce and armor
    /// read would be one tick (~16 ms) later than before. `broadcast_fire_window` runs
    /// `.after(GameplaySet)`, and `shooting::fire` / `integrate_projectiles` raise their events INSIDE
    /// `GameplaySet`, so the ordering edge is a sync point: the deferred `trigger` is flushed — running
    /// the push observer — before the sender looks at the window. Pinned here because a future schedule
    /// shuffle that broke it would cost a silent tick of latency on every cosmetic event.
    #[test]
    fn a_shot_rides_the_burst_of_the_tick_it_fired_on() {
        #[derive(Resource, Default)]
        struct Sends(Vec<u32>);

        fn fire_once(mut commands: Commands, mut fired: Local<bool>) {
            if *fired {
                return;
            }
            *fired = true;
            commands.trigger(FireShell {
                origin: Vec3::ZERO,
                direction: Dir3::X,
                speed: 773.0,
                caliber: 0.088,
                mass: 10.2,
                tracer: true,
                shooter: Some(crate::ballistics::ShotSource {
                    tank: Entity::PLACEHOLDER,
                    weapon: 0,
                }),
                catch_up_ticks: 0,
                shot: None,
            });
        }
        // Stands in for `broadcast_fire_window` minus lightyear's sender: same decision, same slot.
        fn record(
            mut rings: ResMut<FireRings>,
            timeline: Res<LocalTimeline>,
            mut sends: ResMut<Sends>,
        ) {
            if rings.should_send(timeline.tick()) {
                sends.0.push(timeline.tick().0);
            }
        }

        let mut app = App::new();
        app.init_resource::<FireRings>();
        app.init_resource::<LocalTimeline>();
        app.init_resource::<Sends>();
        app.add_observer(broadcast_fire);
        app.configure_sets(FixedUpdate, GameplaySet);
        app.add_systems(FixedUpdate, fire_once.in_set(GameplaySet));
        app.add_systems(FixedUpdate, record.after(GameplaySet));

        app.world_mut().run_schedule(FixedUpdate);

        assert_eq!(
            app.world().resource::<Sends>().0,
            vec![0],
            "the shot fired in GameplaySet is already in the window when the sender runs — the burst \
             goes out on the SAME tick, so clock-driven sending costs zero added latency",
        );
    }

    /// The MG never had the bug, and this is why — the asymmetry that made the owner's report read as
    /// "the 88 is flaky": at 750 rpm a round leaves every ~5 ticks, so a keyframe was re-carried by the
    /// ~4 fires that followed it inside the window (~5 datagrams, effectively always delivered) while
    /// the single-shot 88's keyframe rode one. Pinned so a future fire-rate/window change can't quietly
    /// re-introduce a rate-dependent redundancy floor: BOTH now get the full window.
    #[test]
    fn redundancy_no_longer_depends_on_the_gun_that_fired() {
        const BOUNCE: u32 = 3000;
        // The MG stream: a fire every ~5 ticks (750 rpm at 64 Hz), bracketing the bounce.
        let mut mg = FireRings::default();
        FireRings::push(&mut mg.keyframes, keyframe_at(BOUNCE), Tick(BOUNCE));
        let mut mg_sends = 0;
        for t in 0..64u32 {
            let now = Tick(BOUNCE + t);
            if t % 5 == 0 {
                FireRings::push(&mut mg.fires, fire_at(BOUNCE + t), now);
            }
            if mg.should_send(now) {
                mg_sends += 1;
            }
        }
        // The 88 stream: the bounce, then nothing.
        let mut main_gun = FireRings::default();
        FireRings::push(&mut main_gun.keyframes, keyframe_at(BOUNCE), Tick(BOUNCE));
        let gun_sends = sends_over(&mut main_gun, BOUNCE, 64).len();

        // The MG keeps sending (it keeps firing); what matters is that the 88's keyframe now gets its
        // full window of copies rather than a count set by how fast the gun happens to cycle.
        assert!(
            mg_sends >= gun_sends,
            "sanity: the MG stream never sends less"
        );
        assert_eq!(
            gun_sends,
            FIRE_RETAIN_TICKS as usize + 1,
            "the single-shot gun's bounce gets the same redundancy the MG's always had by accident",
        );
    }
}
