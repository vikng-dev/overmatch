//! The networked dedicated server (step 7 of the lightyear spike map's recommended order):
//! `SimPlugin` mounted for real — driving/aim/shooting/damage all run under prediction now, not
//! the increment-5 stub. Headless — the proven `headless_test.rs` recipe (full `DefaultPlugins`,
//! no GPU/window/winit), NOT `MinimalPlugins`, because the rig loads the same `.glb`/`.tank.ron`
//! assets the client does.
//! Run with `cargo run --bin server --features net`.

use core::time::Duration;
use std::net::{Ipv4Addr, SocketAddr};

use avian3d::prelude::{Position, Rotation};
use bevy::app::ScheduleRunnerPlugin;
use bevy::asset::LoadState;
use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::server::*;
use lightyear::prelude::*;

use super::protocol::ServoAngles;
use super::{diagnostics, harness, open_gameplay_gate, physics, rig};
use crate::SimPlugin;
use crate::command::TankCommand;
use crate::tank::{PendingTankSpec, load_tank_spec, on_tank_ready};

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
    app.add_systems(Startup, load_tank_spec);
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
            diagnostics::count_rig_binds,
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
/// dependency of spawning, same as `tank.rs`/`sandbox.rs` — `on_tank_ready` asserts it's already
/// loaded). A client can connect before the tiny `.tank.ron` parse finishes; queueing avoids
/// spawning a tank root the binder would immediately panic on.
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

/// Spawns the real Tiger rig for every queued client once the spec has loaded — the increment-6
/// swap for increment 5's primitive cuboid spawn. `on_tank_ready` (observed below) builds the
/// colliders/armor volumes from the same spec once the glb scene arrives (async, per tank).
fn spawn_pending_tanks(
    mut pending: ResMut<PendingClients>,
    spec: Option<Res<PendingTankSpec>>,
    asset_server: Res<AssetServer>,
    time: Res<Time<Virtual>>,
    config: Res<harness::PerturbConfig>,
    mut commands: Commands,
) {
    if pending.0.is_empty() {
        return;
    }
    let Some(spec) = spec else { return };
    if !matches!(asset_server.load_state(&spec.0), LoadState::Loaded) {
        return;
    }
    for (link, client_id) in pending.0.drain(..) {
        let mut tank = commands.spawn((
            Name::new("Tank"),
            rig::net_tank_rig(&asset_server, &spec.0),
            ActionState::<TankCommand>::default(),
            Position(Vec3::new(0.0, 2.0, 0.0)),
            // Explicit identity, NOT left to `RigidBody`'s required-component default — that
            // default is `Rotation::PLACEHOLDER` (f32::MAX sentinel, avian rigid_body/mod.rs:271),
            // and if replication captures the spawn frame before the transform sync overwrites it,
            // the client's confirmed history for the earliest ticks holds the sentinel — which the
            // post-bind settle rollbacks then faithfully restore into the sim (the bind-window NaN
            // crash family, spike log 2026-07-04).
            Rotation::default(),
            // The authority's turret/gun lay, for every non-predicted view of this tank
            // (`net::publish_servo_angles` keeps it fresh).
            ServoAngles::default(),
            Replicate::to_clients(NetworkTarget::All),
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
        tank.observe(on_tank_ready);
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
