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
use crate::command::TankCommand;
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
            open_gameplay_gate,
            diagnostics::log_positions,
            diagnostics::log_sim_evidence,
        ),
    );
    app.add_systems(
        FixedUpdate,
        (log_tank_commands, harness::perturb_after_delay),
    );

    app.run();
}

/// Connected clients waiting on the Tiger spec load (§2 of the map: the spec is a load
/// dependency of spawning, same as `tank.rs` — `spawn_tank_sim` requires the spec + extracted
/// geometry already loaded). A client can connect before the tiny `.tank.ron` parse finishes;
/// queueing avoids spawning a tank root the spawner would immediately panic on.
#[derive(Resource, Default)]
struct PendingClients(Vec<(Entity, PeerId)>);

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
    for (link, client_id) in pending.0.drain(..) {
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
            Position(Vec3::new(0.0, 2.0, 0.0)),
            // Explicit identity, NOT left to `RigidBody`'s required-component default — that
            // default is `Rotation::PLACEHOLDER` (f32::MAX sentinel, avian rigid_body/mod.rs:271),
            // and if replication captures the spawn frame before the transform sync overwrites it,
            // the client's confirmed history for the earliest ticks holds the sentinel — which
            // rollbacks would then faithfully restore into the sim (the placeholder-NaN class,
            // spike log 2026-07-04).
            Rotation::default(),
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
