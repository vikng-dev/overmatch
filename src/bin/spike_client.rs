//! Networking-spike client (step 7): connects to a local `spike_server` over UDP+netcode, predicts
//! its own tank body, and forces a rollback via both a `LinkConditioner` (genuine prediction lead)
//! and the server's one-shot perturbation. `SimPlugin` is mounted for real now — driving/aim/
//! shooting all run under prediction, fed by a `TankCommand` bridge (net.rs) from lightyear's own
//! `ActionState`. Spike-local device gather (WASD + LMB read directly) — deliberately NOT the
//! game's `command::client_plugin`.
//!
//! Run with `cargo run --bin spike_client --features net`. Pass `--simulate-input` (or set
//! `SPIKE_SIMULATE_INPUT`) to run headless and programmatically drive throttle/aim/fire for a few
//! seconds, proving prediction + rollback under a real sim workload without a human at the keyboard.

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
use overmatch::{AppState, Rig, SimPlugin, TankCommand, Turret, on_tank_ready};

const SERVER_PORT: u16 = 5888;

/// Latches edge inputs (LMB click) across render frames until a fixed tick consumes them — the
/// same latch contract as the game's `gather_commands`, kept spike-local.
#[derive(Resource, Default)]
struct EdgeLatch {
    fire_primary: bool,
}

/// `--simulate-input` state: a fixed-tick counter driving a scripted throttle window, then a
/// clean exit once enough time has passed to observe the forced rollback + convergence.
/// `fire_tick` defaults to 300 (mid-drive, well clear of the perturbation); `SPIKE_FIRE_TICK`
/// overrides it for the forced-rollback-with-fire pass (~110 lands beside the ~2 s perturbation).
#[derive(Resource)]
struct SimulateInput {
    ticks: u32,
    fire_tick: u32,
}

impl Default for SimulateInput {
    fn default() -> Self {
        Self {
            ticks: 0,
            fire_tick: std::env::var("SPIKE_FIRE_TICK")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
        }
    }
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
    // Step 7: the real sim — same `SimPlugin` the server mounts, so client-side rollback replay
    // re-runs the actual driving/aim/shooting systems, not a stub. `tank::client_plugin` (Tab
    // swap) and `command::client_plugin` (device gather) are deliberately NOT mounted: this bin's
    // own `buffer_input`/simulate-input path writes `ActionState` directly, bridged into
    // `TankCommand` by `net.rs`'s `bridge_action_state_to_tank_command`.
    app.add_plugins(SimPlugin);

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
    // Env-tunable input delay (step 7 A/B lever, map §5): `fixed_input_delay(n)` pins the input
    // delay to n ticks, shrinking the prediction window — off by default (0 ≈ no_input_delay,
    // except max_predicted_ticks stays 100 which matches our ~7-tick practical case anyway).
    let delay_ticks = overmatch::net::input_delay_ticks();
    if delay_ticks > 0 {
        info!("spike_client: SPIKE_INPUT_DELAY_TICKS={delay_ticks}");
        app.world_mut().entity_mut(client).insert(
            InputTimelineConfig::default()
                .with_input_delay(InputDelayConfig::fixed_input_delay(delay_ticks)),
        );
    }
    app.add_systems(Startup, move |mut commands: Commands| {
        commands.trigger(Connect { entity: client });
        // No local ground spawn: `SimPlugin` → `world::plugin` builds the real game terrain
        // (Terrain-layer static slab + test course) on both sides — rollback replays collide with
        // it, and the wheels' suspension rays (filtered to `Layer::Terrain`) actually hit it,
        // which the old untagged `spike_ground` never was (step-7 terrain decision, see log).
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
                open_gameplay_gate,
                count_rig_binds,
                watch_rollback_metrics,
                watch_turret_pose,
                log_snap,
                log_position,
                count_shell_spawns,
                overmatch::net::log_prediction_diagnostics,
                overmatch::net::log_sim_evidence,
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

/// `SimPlugin` mounts `state::sim_plugin` (`AppState`, `GameplaySet` gated on `Playing`), and the
/// bins have no menu/loading flow to drive the transition (step 7: "the bins never enter Playing
/// on their own now"). Same load dependency `attach_predicted_rig` already waits on — once the
/// spec is in, open the `GameplaySet` gate so the sim actually ticks.
fn open_gameplay_gate(
    spec: Option<Res<PendingTankSpec>>,
    asset_server: Res<AssetServer>,
    state: Res<State<AppState>>,
    mut next: ResMut<NextState<AppState>>,
) {
    if *state.get() != AppState::Loading {
        return;
    }
    let Some(spec) = spec else { return };
    if matches!(asset_server.load_state(&spec.0), LoadState::Loaded) {
        info!("spike_client: spec loaded — entering AppState::Playing");
        next.set(AppState::Playing);
    }
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

/// Counts local shell/tracer spawns (`Added<ShellPath>` — inserted by `on_fire_shell` on every
/// shell). The script fires exactly once, so a count above one during the forced-rollback pass is
/// the "replayed fire duplicates the local tracer" wart the coordinator accepted for this step
/// (fixed later by `PreSpawned`, map §2 — deliberately not added yet).
fn count_shell_spawns(shells: Query<Entity, Added<overmatch::ShellPath>>, mut total: Local<u32>) {
    for entity in &shells {
        *total += 1;
        info!("spike_client: SHELL-SPAWN {entity} (total={})", *total);
    }
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
        let t = sim.ticks;
        // Step-7 script, exercising the real sim under prediction: 2 s idle (rig binds, suspension
        // settles) → 4 s throttle 1.0 + steer 0.3 (ramp_drive + suspension + skid-steer, spanning
        // the ~2 s server perturbation) → coast to rest. The aim intention + range are held from
        // tick 0 so the turret/gun servos slew (drive_aim_servos → drive_servos) while driving;
        // one fire click at tick 300 (Reload starts ready) exercises fire + recoil + reload.
        state.0.throttle = if (128..384).contains(&t) { 1.0 } else { 0.0 };
        state.0.steer = if (128..384).contains(&t) { 0.3 } else { 0.0 };
        // Hull-local, far off-axis so the yaw servo visibly slews; range 800 m dials in real
        // superelevation from the weapon's range table.
        state.0.aim = Some(Vec3::new(200.0, 0.0, -800.0));
        state.0.range = 800.0;
        state.0.fire_primary = t == sim.fire_tick;
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
    if simulate.ticks >= 600 {
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
