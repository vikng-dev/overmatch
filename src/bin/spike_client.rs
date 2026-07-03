//! Networking-spike client (steps 5–6 of the lightyear spike map's recommended order): connects
//! to a local `spike_server` over UDP+netcode, predicts its own tank body, and forces a rollback
//! via both a `LinkConditioner` (genuine prediction lead) and the server's one-shot perturbation.
//! Spike-local device gather (WASD + LMB read directly) — deliberately NOT the game's
//! `command::client_plugin`. Mounts `spec::plugin` so the client can load the same `.tank.ron` +
//! glb the server does and build an identical local rig on its predicted root (increment 6).
//!
//! Run with `cargo run --bin spike_client --features net`. Pass `--simulate-input` (or set
//! `SPIKE_SIMULATE_INPUT`) to run headless and programmatically drive throttle for a few seconds,
//! proving prediction + rollback without a human at the keyboard.

// Same rationale as lib.rs's crate-level allow (bins don't inherit it).
#![allow(clippy::type_complexity)]

use core::time::Duration;
use std::net::{Ipv4Addr, SocketAddr};

use avian3d::prelude::Position;
use bevy::app::ScheduleRunnerPlugin;
use bevy::asset::LoadState;
use bevy::prelude::*;
use lightyear::prelude::client::*;
use lightyear::prelude::input::client::InputSystems;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::{Controlled as NetControlled, *};
use overmatch::net::{PendingTankSpec, SpikeBeacon, SpikeTank, load_tank_spec, spike_tank_rig};
use overmatch::{Rig, TankCommand, Turret, on_tank_ready, spec_plugin};

const SERVER_PORT: u16 = 5888;

/// Latches edge inputs (LMB click) across render frames until a fixed tick consumes them — the
/// same latch contract as the game's `gather_commands`, kept spike-local.
#[derive(Resource, Default)]
struct EdgeLatch {
    fire_primary: bool,
}

/// `--simulate-input` state: a fixed-tick counter driving a scripted throttle window, then a
/// clean exit once enough time has passed to observe the forced rollback + convergence.
#[derive(Resource, Default)]
struct SimulateInput {
    ticks: u32,
}

/// Tracks the predicted tank's previous-tick `Position` so a big jump can be logged as a
/// `ROLLBACK-SNAP` (the map's suggested fallback detector alongside `PredictionMetrics`).
#[derive(Component, Default)]
struct LastPosition(Option<Vec3>);

/// Verdict 2 (increment 6): the turret node's previous-tick pose relative to the hull, so a jump
/// in that *relative* pose (as opposed to the hull's own world-space rollback snap) would mean
/// `update_child_collider_position` failed to keep the child rig tracking the root through a
/// replay. Logged around the perturbation window only (see `watch_turret_pose`).
#[derive(Resource, Default)]
struct TurretWatch {
    last_relative: Option<Vec3>,
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
    // entity. `net::plugin` also mounts `LightyearAvianPlugin` + Position/Rotation/velocity
    // registration (map §5); `physics_plugins()` gives the matching disables.
    app.add_plugins(ClientPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / 64.0),
    });
    app.add_plugins(overmatch::net::plugin);
    app.add_plugins(overmatch::net::physics_plugins());
    // The real rig (increment 6): same minimal mounting as the server — the `.tank.ron` loader
    // `on_tank_ready` depends on, not the full `tank::sim_plugin` (no servo/suspension here).
    app.add_plugins(spec_plugin);

    let server_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), SERVER_PORT);
    // Pid-based id so back-to-back runs don't collide inside the server's disconnect timeout.
    let client_id = u64::from(std::process::id());
    // ~100 ms delay + jitter on the inbound link, so the client's prediction genuinely runs ahead
    // of the last-confirmed server state (increment 5 rollback-forcing mechanism #1).
    // Env-tunable for bisecting rollback causes: SPIKE_LATENCY_MS=0 disables the conditioner
    // entirely (pure loopback), isolating latency-window effects from genuine sim divergence.
    let latency_ms: u64 = std::env::var("SPIKE_LATENCY_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    let jitter_ms: u64 = std::env::var("SPIKE_JITTER_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let conditioner = (latency_ms > 0).then(|| {
        RecvLinkConditioner::new(LinkConditionerConfig::new(
            Duration::from_millis(latency_ms),
            Duration::from_millis(jitter_ms),
            0.0,
        ))
    });
    let client = app
        .world_mut()
        .spawn((
            Name::new("SpikeClient"),
            Client::default(),
            Link::new(conditioner),
            LocalAddr(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0)),
            PeerAddr(server_addr),
            PredictionManager::default(),
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
        // The client builds its own ground — rollback replays need terrain to collide with.
        commands.spawn(overmatch::net::spike_ground());
        info!("spike_client: connecting to {server_addr} as client_id={client_id}");
    });
    app.add_systems(Startup, load_tank_spec);

    app.add_observer(log_connected)
        .add_observer(claim_input_slot)
        .add_observer(log_predicted_tank)
        .init_resource::<EdgeLatch>()
        .init_resource::<RollbackWatch>()
        .init_resource::<TurretWatch>()
        .add_systems(
            Update,
            (
                log_beacon,
                attach_predicted_rig,
                count_rig_binds,
                watch_rollback_metrics,
                watch_turret_pose,
                log_snap,
                log_position,
            ),
        )
        .add_systems(
            RunFixedMainLoop,
            latch_edges.in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop),
        )
        .add_systems(
            FixedPreUpdate,
            // Rollback replays re-run FixedPreUpdate too (map §8) — lightyear itself restores
            // `ActionState` from the `InputBuffer` per replayed tick (and `buffer_action_state`
            // is `Without<Rollback>`, so the buffer can't be corrupted), but without this gate
            // the scripted tick counter would count every replayed tick (verified live: 640
            // "ticks" burned in <5 s wall).
            buffer_input
                .in_set(InputSystems::WriteClientInputs)
                .run_if(not(is_in_rollback)),
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

/// Give the predicted tank its LOCAL rig (map §6's `handle_new_character` pattern, increment 6's
/// swap for the primitive cuboid): avian components are not replicated, and a predicted entity
/// without a body cannot be re-simulated during rollback replay — the symptom is continuous
/// rollback from spawn, every confirmed packet disagreeing with a frozen prediction. A plain
/// system (not an observer on `Predicted`) because `SpikeTank` arrives by replication and may
/// land after the marker; also waits on the spec load (§8's spawn-before-bind race, mirrored from
/// `tank.rs`/`sandbox.rs` — `on_tank_ready` would panic on an unloaded spec). This is the exact
/// moment the §8 UNCERTAIN gets exercised: `Predicted`/`PredictionTarget` is already on the entity
/// (attached server-side at spawn) several ticks *before* the glb scene finishes loading and
/// `on_tank_ready` binds the rig — see the spike log for what was observed in that window.
fn attach_predicted_rig(
    tanks: Query<
        Entity,
        (
            With<Predicted>,
            With<SpikeTank>,
            Without<avian3d::prelude::RigidBody>,
        ),
    >,
    spec: Option<Res<PendingTankSpec>>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    if tanks.is_empty() {
        return;
    }
    let Some(spec) = spec else { return };
    if !matches!(asset_server.load_state(&spec.0), LoadState::Loaded) {
        return;
    }
    for entity in &tanks {
        info!("spike_client: {entity} predicted tank gets local rig (spec loaded)");
        commands
            .entity(entity)
            .insert(spike_tank_rig(&asset_server, &spec.0))
            .observe(on_tank_ready);
    }
}

/// Verdict 1 (increment 6), client side: same `Added<Rig>` count as the server — the predicted
/// root is exactly where a rollback replay could plausibly re-fire an async-load observer if the
/// map §8 "rollback re-runs FixedMain only" assumption were wrong, so this is the side that
/// actually matters for the verdict.
fn count_rig_binds(binds: Query<Entity, Added<Rig>>) {
    for entity in &binds {
        info!("spike_client: {entity} Rig bound (on_tank_ready fired)");
    }
}

/// Verdict 2 (increment 6): the turret's pose *relative to the hull* — logged only when it moves
/// more than the map's 0.1 m bar in one tick, which should never happen (the turret doesn't slew
/// in this spike; nothing drives `ServoCommand`) unless `update_child_collider_position` failed to
/// keep the child rig glued to the root through a rollback replay. Absolute world deltas are
/// expected (the perturbation moves the whole tank); only the hull-relative offset is diagnostic.
fn watch_turret_pose(
    hulls: Query<&GlobalTransform, With<overmatch::Hull>>,
    turrets: Query<&GlobalTransform, With<Turret>>,
    mut watch: ResMut<TurretWatch>,
) {
    let (Ok(hull), Ok(turret)) = (hulls.single(), turrets.single()) else {
        return;
    };
    let relative = hull.translation().distance(turret.translation());
    let relative_vec = turret.translation() - hull.translation();
    if let Some(previous) = watch.last_relative {
        let delta = (relative_vec - previous).length();
        if delta > 0.1 {
            warn!(
                "spike_client: TURRET-DRIFT relative offset moved {delta:.3} m in one tick \
                 (hull-relative distance now {relative:.3} m) — child rig desynced from root"
            );
        }
    }
    watch.last_relative = Some(relative_vec);
}

/// Increment-5 success signal: the predicted tank arrives carrying `Predicted`.
fn log_predicted_tank(add: On<Add, Predicted>, tanks: Query<(), With<SpikeTank>>) {
    if tanks.contains(add.entity) {
        info!(
            "spike_client: {} predicted (carries Predicted) — moves immediately under input",
            add.entity
        );
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
        LastPosition::default(),
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
        // Scripted drive: full throttle for the first ~4s, spanning the ~2s server perturbation,
        // then release so friction stops the tank and rest positions can be compared.
        state.0.throttle = if sim.ticks <= 256 { 1.0 } else { 0.0 };
        state.0.fire_primary = false;
    } else {
        state.0.throttle =
            keys.pressed(KeyCode::KeyW) as i8 as f32 - keys.pressed(KeyCode::KeyS) as i8 as f32;
        state.0.steer =
            keys.pressed(KeyCode::KeyD) as i8 as f32 - keys.pressed(KeyCode::KeyA) as i8 as f32;
        state.0.fire_primary = latch.fire_primary;
        latch.fire_primary = false;
    }
}

/// Simulate mode: exit cleanly once the script has played out (long enough to cover the ~2s
/// server perturbation and settle afterward), or bail on a wall-clock timeout if the connection
/// never came up.
fn simulate_watchdog(
    simulate: Res<SimulateInput>,
    time: Res<Time<Real>>,
    mut exit: MessageWriter<AppExit>,
) {
    if simulate.ticks >= 640 {
        info!("spike_client: simulation script complete, exiting");
        exit.write(AppExit::Success);
    } else if time.elapsed_secs() > 40.0 {
        error!("spike_client: watchdog timeout — never got an input slot");
        exit.write(AppExit::error());
    }
}

/// Polls `PredictionMetrics` each frame and logs on change — the primary "a rollback fired"
/// signal (map's suggested mechanism; `lightyear_prediction`'s own diagnostics counter).
#[derive(Resource, Default)]
struct RollbackWatch {
    last_count: u32,
}

fn watch_rollback_metrics(metrics: Res<PredictionMetrics>, mut watch: ResMut<RollbackWatch>) {
    if metrics.rollbacks != watch.last_count {
        info!(
            "spike_client: ROLLBACK fired (PredictionMetrics.rollbacks={}, rollback_ticks={})",
            metrics.rollbacks, metrics.rollback_ticks
        );
        watch.last_count = metrics.rollbacks;
    }
}

/// Periodic predicted-position log (every ~2 s) — diffed against the server's own periodic log
/// for the increment-5/6 convergence success criterion.
fn log_position(
    tanks: Query<(Entity, &Position), (With<Predicted>, With<SpikeTank>)>,
    mut timer: Local<f32>,
    time: Res<Time>,
) {
    *timer += time.delta_secs();
    if *timer < 2.0 {
        return;
    }
    *timer = 0.0;
    for (entity, position) in &tanks {
        info!("spike_client: {entity} position={:?}", position.0);
    }
}

/// Backup rollback detector (map's fallback): a same-tick `Position` discontinuity > 0.5 m on the
/// predicted entity. Also logs final positions for the convergence check.
fn log_snap(
    mut tanks: Query<(Entity, &Position, &mut LastPosition), (With<Predicted>, With<SpikeTank>)>,
) {
    for (entity, position, mut last) in &mut tanks {
        if let Some(previous) = last.0 {
            let delta = (position.0 - previous).length();
            if delta > 0.5 {
                info!(
                    "spike_client: ROLLBACK-SNAP {entity} moved {delta:.2} m in one tick (from {previous:?} to {:?})",
                    position.0
                );
            }
        }
        last.0 = Some(position.0);
    }
}
