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
fn attach_replication_sender(add: On<Add, LinkOf>, mut commands: Commands) {
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

/// The server's sliding redundancy windows: the last N [`FireEvent`]s, [`RicochetKeyframe`]s, and
/// [`ImpactConfirm`]s, resent in EVERY [`FireBurst`] so one delivered burst repairs a multi-packet
/// loss of any stream (piece 3 — the input-redundancy pattern applied to cosmetics).
///
/// [`FIRE_WINDOW`] = 4 is sized against the worst case: a 750 rpm MG cycles at 12.5 rounds/s, so the
/// server sends one fire burst every ~80 ms, and a 4-deep fire window keeps each event alive across the
/// next ~3 bursts (~240 ms) — past a typical WAN burst loss, while staying a handful of small structs
/// per packet. Keyframes and confirms are rare (one per ricochet / per armor-terminated shot), so
/// their 4-deep windows hold each far longer in wall-clock — an entry only rotates out after N MORE
/// events of its kind — giving bounces and terminals generous redundancy. (Caveat, accepted: a lost
/// entry is only RE-carried by the next burst, which some fire/ricochet/terminal must trigger — an
/// isolated shot's lost confirm may never resend inside the grace window and then degrades to the
/// fail-closed truncation, invariant 3 of ADR-0021.)
#[derive(Resource, Default)]
struct FireRings {
    fires: std::collections::VecDeque<FireEvent>,
    keyframes: std::collections::VecDeque<RicochetKeyframe>,
    confirms: std::collections::VecDeque<ImpactConfirm>,
}

/// Sliding-window depth — see [`FireRings`] for the sizing against MG cyclic rate.
const FIRE_WINDOW: usize = 4;

impl FireRings {
    fn push<T>(ring: &mut std::collections::VecDeque<T>, item: T) {
        if ring.len() == FIRE_WINDOW {
            ring.pop_front();
        }
        ring.push_back(item);
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

/// Turn each authoritative `FireShell` into a cosmetic `FireEvent` and broadcast the current sliding
/// window — the SERVER half of the opponent-fire seam (`net::protocol::FireEvent`/`FireBurst`).
/// Observes every shot the sim fires; a shot that names a tank (`shooter: Some`) is pushed to the fire
/// ring and a `FireBurst` sent so the OTHER clients spawn a local tracer for a tank they only
/// interpolate, while sandbox/`None` shots (no tank) never broadcast.
///
/// **Targeting: `All`, deduped at the receiver.** Every burst goes to every client; a client drops any
/// FIRE naming a tank IT simulates locally (`receive_fire_events`' `locally_fired` guard), so the
/// shooter discarding its own echo is a receiver concern. This is what lets ONE shared burst carry
/// events from MULTIPLE shooters correctly — an `AllExceptSingle(owner)` target could not, since a
/// burst re-carrying another shooter's fires must still reach this owner. The one-frame self-echo the
/// owner discards is negligible, and the redundancy window (which re-carries older shooters' events) is
/// only correct under `All`. KEYFRAMES are not dropped for own shots — the shooter's own shell consumes
/// its bounce (the fall-of-shot read), which also REQUIRES the `All` target here.
fn broadcast_fire(
    fire: On<FireShell>,
    servers: Query<&Server>,
    // The server's simulation tick. `shooting::fire` raises `FireShell` inside `FixedUpdate`, in
    // `GameplaySet`, AFTER `LocalTimeline` is incremented for this tick (`increment_local_tick` runs
    // in `FixedFirst`); nothing advances it again until the next frame's `FixedFirst`. So this
    // observer — even deferred to the command flush — reads the SAME tick the firing sim step ran on,
    // which is exactly the tick the muzzle pose in `fire.origin` was computed for (and the tick
    // `stamp_shot_ids` stamps into this shell's `Shot`, so a later ricochet keyframe keys the same id).
    timeline: Res<LocalTimeline>,
    mut rings: ResMut<FireRings>,
    mut sender: ServerMultiMessageSender,
) {
    // Only tank-attributed shots broadcast; a `None` shooter (the sandbox) has no tank to name.
    let Some(source) = fire.shooter else {
        return;
    };
    // No `Server` collection yet (no client has linked) — nothing to send to; drop the tracer.
    let Ok(server) = servers.single() else {
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
    FireRings::push(&mut rings.fires, event);
    if let Err(err) =
        sender.send::<FireBurst, FireChannel>(&rings.burst(), server, &NetworkTarget::All)
    {
        // A dropped cosmetic burst is not an error (the channel is unreliable by design, and the
        // window covers the loss); keep this at debug so a transient send failure can't spam the log.
        debug!("server: FireBurst broadcast dropped: {err}");
    }
}

/// Turn each authoritative `ballistics::ShellRicochet` into a `RicochetKeyframe` and broadcast the
/// current window — the SERVER half of the bounce carry-through (ADR-0016: replicate the CAUSE; every
/// client — observers AND the shooter — derives the re-seed). The authority march raises
/// `ShellRicochet` at the moment it resolves a bounce for a net-attributed shell (`Shot` present,
/// stamped by the shared `protocol::stamp_shot_ids`); this stamps the bounce tick from the server
/// timeline and sends. Same `FireRings`/`All` window + targeting as `broadcast_fire`: a fire burst
/// re-carries recent keyframes and vice versa, so both streams share the redundancy for free.
/// `ShellRicochet` is a sim-layer event, so the sandbox raises it too — but no `Server` is present
/// there and this observer is SERVER-ONLY, so it never fires off the net.
fn on_shell_ricochet(
    ricochet: On<ShellRicochet>,
    servers: Query<&Server>,
    timeline: Res<LocalTimeline>,
    mut rings: ResMut<FireRings>,
    mut sender: ServerMultiMessageSender,
) {
    let Ok(server) = servers.single() else {
        return;
    };
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
    FireRings::push(&mut rings.keyframes, keyframe);
    if let Err(err) =
        sender.send::<FireBurst, FireChannel>(&rings.burst(), server, &NetworkTarget::All)
    {
        debug!("server: RicochetKeyframe burst dropped: {err}");
    }
}

/// Turn each authoritative `ballistics::ShellTerminal` (an embed/perforation — the shot's END on
/// armor) into an `ImpactConfirm` and broadcast the current window — the SERVER half of the terminal
/// confirm that completes the shot state machine (ADR-0016: replicate the CAUSE; every client —
/// observers AND the shooter — renders the honest armor read from it). The authority march raises
/// `ShellTerminal` at most once per shot (see its doc); this stamps the impact tick from the server
/// timeline and sends. Same `FireRings`/`All` window + targeting as `broadcast_fire`; SERVER-ONLY for
/// the same reason as `on_shell_ricochet` (the sandbox raises the sim event too, but has no `Server`).
fn on_shell_terminal(
    terminal: On<ShellTerminal>,
    servers: Query<&Server>,
    timeline: Res<LocalTimeline>,
    mut rings: ResMut<FireRings>,
    mut sender: ServerMultiMessageSender,
) {
    let Ok(server) = servers.single() else {
        return;
    };
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
    FireRings::push(&mut rings.confirms, confirm);
    if let Err(err) =
        sender.send::<FireBurst, FireChannel>(&rings.burst(), server, &NetworkTarget::All)
    {
        debug!("server: ImpactConfirm burst dropped: {err}");
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
