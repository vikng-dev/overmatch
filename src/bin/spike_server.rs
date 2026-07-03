//! Networking-spike dedicated server (steps 5–6 of the lightyear spike map's recommended order).
//! Headless — the proven `headless_test.rs` recipe (full `DefaultPlugins`, no GPU/window/winit),
//! NOT `MinimalPlugins`, because increment 6 loads the same `.glb`/`.tank.ron` assets the client
//! does. `SimPlugin` is deliberately NOT mounted here (task spec for steps 5–6 — it returns in
//! step 7): physics is composed directly, driven by the shared stub movement system in `net.rs`.
//! Run with `cargo run --bin spike_server --features net`.

// Same rationale as lib.rs's crate-level allow (bins don't inherit it): ordinary multi-filter
// query tuples trip this lint.
#![allow(clippy::type_complexity)]

use core::time::Duration;
use std::net::{Ipv4Addr, SocketAddr};

use avian3d::prelude::{Forces, Position, WriteRigidBodyForces};
use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use overmatch::TankCommand;
use overmatch::net::{SpikeBeacon, SpikeTank};

const PORT: u16 = 5888;

fn main() {
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
    app.add_plugins(overmatch::net::plugin);
    app.add_plugins(overmatch::net::physics_plugins());

    let server = app
        .world_mut()
        .spawn((
            Name::new("SpikeServer"),
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
        commands.spawn((
            Name::new("SpikeBeacon"),
            SpikeBeacon,
            Replicate::to_clients(NetworkTarget::All),
        ));
        // Static ground plane — not replicated (both sides build their own; it never moves).
        commands.spawn(overmatch::net::spike_ground());
        info!("spike_server: starting, listening on 0.0.0.0:{PORT}");
    });

    app.add_systems(Update, (handle_new_clients, log_positions));
    app.add_systems(FixedUpdate, (log_tank_commands, perturb_after_delay));

    app.run();
}

/// Per-client one-shot: fires ~2 s after connect, applying a large lateral impulse the client
/// cannot have predicted (server-only side effect) — guarantees a misprediction and thus a
/// rollback (increment 5 success criterion).
#[derive(Component)]
struct PendingPerturbation {
    at: Duration,
}

/// Increment-5 spawn pattern (spike map §6/§7): one predicted primitive body per connected
/// client, owned by that client, everyone else would interpolate it (no second client yet).
fn handle_new_clients(
    new: Query<(Entity, &RemoteId), (Added<Connected>, With<ClientOf>)>,
    time: Res<Time<Virtual>>,
    mut commands: Commands,
) {
    for (link, remote) in &new {
        let client_id = remote.0;
        info!("spike_server: client connected: {remote} (link {link})");
        commands.spawn((
            Name::new("SpikeTank"),
            SpikeTank,
            ActionState::<TankCommand>::default(),
            Position(Vec3::new(0.0, 2.0, 0.0)),
            overmatch::net::spike_tank_physics(),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            ControlledBy {
                owner: link,
                lifetime: default(),
            },
            PendingPerturbation {
                at: time.elapsed() + Duration::from_secs(2),
            },
        ));
    }
}

/// Applies the forced-rollback perturbation once, ~2 s after spawn — a lateral impulse only the
/// server applies, so the client's prediction (which never saw it coming) mispredicts and must
/// roll back when the replicated `Position` disagrees.
fn perturb_after_delay(
    mut tanks: Query<(Entity, &PendingPerturbation, Forces)>,
    time: Res<Time<Virtual>>,
    mut commands: Commands,
) {
    for (entity, pending, mut forces) in &mut tanks {
        if time.elapsed() < pending.at {
            continue;
        }
        // Sized for ~3 m/s of lateral delta-v on the 57 t tank (net.rs::TANK_MASS) — comfortably
        // above the 0.01 m/s-equivalent rollback threshold (forces exactly one misprediction) but
        // small next to the ~4-15 m/s cruise speed, so the resulting one-tick displacement stays
        // under the ROLLBACK-SNAP detector's 0.5 m bar. The previous 4,000,000 N*s value injected
        // ~70 m/s instantly — legitimate per-tick motion at that speed (~1.1 m/tick) was tripping
        // the snap detector on its own, misread as rollback oscillation (see spike log).
        const IMPULSE: f32 = 171_000.0;
        forces.apply_linear_impulse(Vec3::X * IMPULSE);
        info!("spike_server: {entity} perturbation impulse applied (forced rollback trigger)");
        commands.entity(entity).remove::<PendingPerturbation>();
    }
}

/// Periodic authoritative position log (every ~2 s), so the client's converged position can be
/// diffed against this end for the increment-5/6 convergence success criterion.
fn log_positions(
    tanks: Query<(Entity, &Position), With<SpikeTank>>,
    mut timer: Local<f32>,
    time: Res<Time>,
) {
    *timer += time.delta_secs();
    if *timer < 2.0 {
        return;
    }
    *timer = 0.0;
    for (entity, position) in &tanks {
        info!("spike_server: {entity} position={:?}", position.0);
    }
}

/// Step-4 success signal (carried over): the client's `TankCommand` arriving through lightyear's
/// input buffer.
fn log_tank_commands(states: Query<(Entity, &ActionState<TankCommand>)>) {
    for (entity, state) in &states {
        let cmd = &state.0;
        if cmd.throttle != 0.0 || cmd.fire_primary {
            info!(
                "spike_server: {entity} command: throttle={} steer={} fire_primary={}",
                cmd.throttle, cmd.steer, cmd.fire_primary
            );
        }
    }
}
