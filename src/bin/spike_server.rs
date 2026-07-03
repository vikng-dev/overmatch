//! Networking-spike dedicated server (steps 1–4 of the lightyear spike map's recommended order).
//! Headless — the proven `headless_test.rs` recipe (full `DefaultPlugins`, no GPU/window/winit),
//! NOT `MinimalPlugins`, because the server loads the same `.glb`/`.tank.ron` assets the client
//! does. Mounts `SimPlugin` whole. Run with `cargo run --bin spike_server --features net`.

// Same rationale as lib.rs's crate-level allow (bins don't inherit it): ordinary multi-filter
// query tuples trip this lint.
#![allow(clippy::type_complexity)]

use core::time::Duration;
use std::net::{Ipv4Addr, SocketAddr};

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use overmatch::net::SpikeBeacon;
use overmatch::{Roadwheel, SimPlugin, TankCommand};

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
    // entity.
    app.add_plugins(ServerPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / 64.0),
    });
    app.add_plugins(overmatch::net::plugin);
    app.add_plugins(SimPlugin);

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
        info!("spike_server: starting, listening on 0.0.0.0:{PORT}");
    });

    app.add_systems(Update, (log_roadwheels, handle_new_clients));
    app.add_systems(FixedUpdate, log_tank_commands);

    app.run();
}

/// Step-2 success signal: the sim's rigs bound while the netcode connection stays up — the same
/// roadwheel count `headless_test.rs` polls for.
fn log_roadwheels(wheels: Query<&Roadwheel>, mut last: Local<usize>) {
    let count = wheels.iter().count();
    if count != *last {
        info!("spike_server: roadwheels bound: {count}");
        *last = count;
    }
}

/// Step-4 spawn pattern (spike map §6): one server-side entity per connected client carrying the
/// input slot. A bare entity for now — real tank wiring is a later increment.
fn handle_new_clients(
    new: Query<(Entity, &RemoteId), (Added<Connected>, With<ClientOf>)>,
    mut commands: Commands,
) {
    for (link, remote) in &new {
        info!("spike_server: client connected: {remote} (link {link})");
        commands.spawn((
            Name::new("SpikeAvatar"),
            ActionState::<TankCommand>::default(),
            Replicate::to_clients(NetworkTarget::All),
            ControlledBy {
                owner: link,
                lifetime: default(),
            },
        ));
    }
}

/// Step-4 success signal: the client's `TankCommand` arriving through lightyear's input buffer.
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
