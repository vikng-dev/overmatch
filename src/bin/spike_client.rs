//! Networking-spike client (steps 1–4 of the lightyear spike map's recommended order): connects
//! to a local `spike_server` over UDP+netcode, logs connection and `SpikeBeacon` replication, and
//! sends `TankCommand` over lightyear's native input path. Spike-local device gather (WASD + LMB
//! read directly) — deliberately NOT the game's `command::client_plugin`.
//!
//! Run with `cargo run --bin spike_client --features net`. Pass `--simulate-input` (or set
//! `SPIKE_SIMULATE_INPUT`) to run headless and programmatically drive throttle + two one-tick
//! fire_primary clicks, proving the wire path without a human at the keyboard.

use core::time::Duration;
use std::net::{Ipv4Addr, SocketAddr};

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use lightyear::prelude::client::*;
use lightyear::prelude::input::client::InputSystems;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::{Controlled as NetControlled, *};
use overmatch::TankCommand;
use overmatch::net::SpikeBeacon;

const SERVER_PORT: u16 = 5888;

/// Latches edge inputs (LMB click) across render frames until a fixed tick consumes them — the
/// same latch contract as the game's `gather_commands`, kept spike-local.
#[derive(Resource, Default)]
struct EdgeLatch {
    fire_primary: bool,
}

/// `--simulate-input` state: a fixed-tick counter driving a scripted throttle window and two
/// single-tick fire_primary "clicks", then a clean exit.
#[derive(Resource, Default)]
struct SimulateInput {
    ticks: u32,
}

fn main() {
    let simulate = std::env::args().any(|a| a == "--simulate-input")
        || std::env::var("SPIKE_SIMULATE_INPUT").is_ok();

    let mut app = App::new();
    if simulate {
        // Headless: same no-GPU/no-window recipe as the server, so automation never opens a window.
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
        .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_millis(2)))
        .init_resource::<SimulateInput>();
    } else {
        app.add_plugins(DefaultPlugins);
    }

    // Ordering per the spike map §3: ClientPlugins, then protocol registration, then the Client
    // entity.
    app.add_plugins(ClientPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / 64.0),
    });
    app.add_plugins(overmatch::net::plugin);

    let server_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), SERVER_PORT);
    // Pid-based id so back-to-back runs don't collide inside the server's disconnect timeout.
    let client_id = u64::from(std::process::id());
    let client = app
        .world_mut()
        .spawn((
            Name::new("SpikeClient"),
            Client::default(),
            Link::new(None),
            LocalAddr(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0)),
            PeerAddr(server_addr),
            NetcodeClient::new(
                Authentication::Manual {
                    server_addr,
                    client_id,
                    private_key: [0; 32],
                    protocol_id: 0,
                },
                NetcodeConfig {
                    client_timeout_secs: 3,
                    token_expire_secs: -1,
                    ..default()
                },
            )
            .expect("manual dev token should always build"),
            UdpIo::default(),
        ))
        .id();
    app.add_systems(Startup, move |mut commands: Commands| {
        commands.trigger(Connect { entity: client });
        info!("spike_client: connecting to {server_addr} as client_id={client_id}");
    });

    app.add_observer(log_connected)
        .add_observer(claim_input_slot)
        .init_resource::<EdgeLatch>()
        .add_systems(Update, log_beacon)
        .add_systems(
            RunFixedMainLoop,
            latch_edges.in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop),
        )
        .add_systems(
            FixedPreUpdate,
            buffer_input.in_set(InputSystems::WriteClientInputs),
        );
    if simulate {
        app.add_systems(Update, simulate_watchdog);
    }

    app.run();
}

/// Step-1 success signal.
fn log_connected(add: On<Add, Connected>) {
    info!("spike_client: connected (entity {})", add.entity);
}

/// Step-3 success signal: the server's beacon arrived with replicon's `Remote` marker.
fn log_beacon(beacons: Query<Entity, (Added<SpikeBeacon>, With<Remote>)>) {
    for entity in &beacons {
        info!("spike_client: SpikeBeacon replicated from server (entity {entity}, Remote)");
    }
}

/// Possession (spike map §6): the server's `ControlledBy` arrives as lightyear's `Controlled`
/// marker on our avatar — claim it as the local input slot.
fn claim_input_slot(add: On<Add, NetControlled>, mut commands: Commands) {
    info!(
        "spike_client: controlled entity {} — input slot",
        add.entity
    );
    commands.entity(add.entity).insert((
        InputMarker::<TankCommand>::default(),
        ActionState::<TankCommand>::default(),
    ));
}

/// Once per render frame, before the fixed loop: latch the click edge so a tick between frames
/// neither loses nor doubles it.
fn latch_edges(mouse: Res<ButtonInput<MouseButton>>, mut latch: ResMut<EdgeLatch>) {
    latch.fire_primary |= mouse.just_pressed(MouseButton::Left);
}

/// Write this tick's `TankCommand` into the lightyear `ActionState` slot. Whole-state snapshot per
/// tick: edges are true for exactly the one tick that consumes the latch.
fn buffer_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut latch: ResMut<EdgeLatch>,
    simulate: Option<ResMut<SimulateInput>>,
    mut slots: Query<&mut ActionState<TankCommand>, With<InputMarker<TankCommand>>>,
) {
    let Ok(mut state) = slots.single_mut() else {
        return;
    };
    if let Some(mut sim) = simulate {
        sim.ticks += 1;
        // Scripted drive: ~4 s of full throttle with two one-tick clicks at ticks 64 and 192.
        state.0.throttle = if sim.ticks <= 256 { 1.0 } else { 0.0 };
        state.0.fire_primary = sim.ticks == 64 || sim.ticks == 192;
        if sim.ticks == 64 || sim.ticks == 192 {
            info!(
                "spike_client: simulated fire_primary click at tick {}",
                sim.ticks
            );
        }
    } else {
        state.0.throttle =
            keys.pressed(KeyCode::KeyW) as i8 as f32 - keys.pressed(KeyCode::KeyS) as i8 as f32;
        state.0.steer =
            keys.pressed(KeyCode::KeyD) as i8 as f32 - keys.pressed(KeyCode::KeyA) as i8 as f32;
        state.0.fire_primary = latch.fire_primary;
        latch.fire_primary = false;
    }
}

/// Simulate mode: exit cleanly once the script has played out (or bail on a wall-clock timeout if
/// the connection never came up, so automation doesn't hang).
fn simulate_watchdog(
    simulate: Res<SimulateInput>,
    time: Res<Time<Real>>,
    mut exit: MessageWriter<AppExit>,
) {
    if simulate.ticks >= 320 {
        info!("spike_client: simulation script complete, exiting");
        exit.write(AppExit::Success);
    } else if time.elapsed_secs() > 30.0 {
        error!("spike_client: watchdog timeout — never got an input slot");
        exit.write(AppExit::error());
    }
}
