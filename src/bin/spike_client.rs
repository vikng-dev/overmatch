//! Networking-spike client (step 8): connects to a local `spike_server` over UDP+netcode and is
//! now *playable* — windowed mode mounts the game's real presentation + device gather
//! (`NetClientPlugin`: camera, HUD, mouse aim, gunner optic, range dial, crew bar), marks the
//! replicated own tank with the game's `Controlled` on possession, and feeds the gathered
//! `TankCommand` into lightyear's `ActionState` each tick (`feed_action_state`). Whether that tank
//! is predicted or interpolated is the server's spawn-time choice (`SPIKE_PREDICT`) — the step-8
//! feel-test A/B. Esc is a cursor-release menu overlay, NOT a pause: the sim never stops (there is
//! no online pause; a frozen predicting client desyncs from a server that keeps ticking).
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
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use lightyear::prelude::client::*;
use lightyear::prelude::input::client::InputSystems;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::{Controlled as NetControlled, *};
use overmatch::net::{PendingTankSpec, SpikeBeacon, SpikeTank, load_tank_spec, spike_tank_rig};
use overmatch::{
    AppState, Controlled as GameControlled, NetClientPlugin, Rig, SimPlugin, TankCommand, Turret,
    on_tank_ready,
};

const SERVER_PORT: u16 = 5888;

/// Whether the Esc menu overlay is up. The networked stand-in for the SP pause: cursor released,
/// overlay shown (settings/meta actions later), and `feed_action_state` sends a default command so
/// the tank coasts to a stop instead of holding the last input — but `AppState` never leaves
/// `Playing` and the sim keeps ticking.
#[derive(Resource, Default)]
struct MenuOverlay {
    open: bool,
}

#[derive(Component)]
struct MenuOverlayNode;

/// `--simulate-input` state: a fixed-tick counter driving a scripted throttle window, then a
/// clean exit once enough time has passed to observe the forced rollback + convergence.
/// `fire_tick` defaults to 300 (mid-drive, well clear of the perturbation); `SPIKE_FIRE_TICK`
/// overrides it for the forced-rollback-with-fire pass (~110 lands beside the ~2 s perturbation).
/// `SPIKE_SIM_LONG=1` (rollback-storm diagnostic): drive straight at full throttle for ~15 s —
/// from spawn that crosses the speed bump (z≈−70) and the washboard (z≈−82…−90), the terrain the
/// user's rollback-stream report singled out; the default 4 s arc never leaves the flat pad.
#[derive(Resource)]
struct SimulateInput {
    ticks: u32,
    fire_tick: u32,
    /// Last tick of the throttle window (steer is zeroed when extended, so the course features
    /// dead ahead are actually reached).
    drive_until: u32,
    /// Script length — exit after this many ticks.
    total: u32,
}

impl Default for SimulateInput {
    fn default() -> Self {
        let long = std::env::var("SPIKE_SIM_LONG").is_ok();
        Self {
            ticks: 0,
            fire_tick: std::env::var("SPIKE_FIRE_TICK")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
            drive_until: if long { 1088 } else { 384 },
            total: if long { 1280 } else { 600 },
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
    // Diagnostic lever (rollback-storm investigation): scripted input in a REAL window — the
    // deterministic headless baseline workload, but under vsync frame pacing, real rendering, and
    // the full presentation stack. Separates "windowed render loop causes rollbacks" from "human
    // device input causes rollbacks": same script, only the runtime differs.
    let sim_windowed = simulate && std::env::var("SPIKE_SIM_WINDOWED").is_ok();

    let mut app = App::new();
    if simulate && !sim_windowed {
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
        if sim_windowed {
            app.init_resource::<SimulateInput>();
        }
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
    // re-runs the actual driving/aim/shooting systems, not a stub.
    app.add_plugins(SimPlugin);
    // Step 8, windowed: the game's real presentation + device gather. Its writers fill the
    // `Controlled` tank's `TankCommand` at render rate; `feed_action_state` (below) hands that to
    // lightyear each tick. Headless simulate mode keeps writing `ActionState` directly instead
    // (`buffer_input`) — no devices, no window, no presentation.
    // Mounted for sim-windowed too: the device writers it brings fill `TankCommand` at render
    // rate, but in simulate mode the reverse bridge overwrites that whole struct from the scripted
    // `ActionState` every tick before any sim system reads it — the script rules, the presentation
    // stack (camera, HUD, real rendering) still runs, which is exactly the diagnostic point.
    if !simulate || sim_windowed {
        app.add_plugins(NetClientPlugin);
    }

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
        .init_resource::<RollbackWatch>()
        .init_resource::<TurretWatch>()
        .add_systems(
            Update,
            (
                log_beacon,
                attach_replicated_rig,
                nan_tripwire,
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
        );
    if simulate {
        app.add_systems(Update, simulate_watchdog).add_systems(
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
    } else {
        app.init_resource::<MenuOverlay>()
            .add_systems(Update, toggle_menu)
            .add_systems(OnEnter(AppState::Playing), grab_cursor)
            .add_systems(
                FixedPreUpdate,
                // Same rollback gate as `buffer_input`: during replay lightyear restores the
                // historical `ActionState` per tick — overwriting it with the *current* gathered
                // command would corrupt the replay's input.
                feed_action_state
                    .in_set(InputSystems::WriteClientInputs)
                    .run_if(not(is_in_rollback)),
            );
    }

    app.run();
}

/// Step-1 success signal.
fn log_connected(add: On<Add, Connected>) {
    info!("spike_client: connected (entity {})", add.entity);
}

/// NaN tripwire (bind-window crash diagnostic): names the first entity whose physics `Position`
/// or local `Transform` goes non-finite, with its ancestry — runs before avian's own finite
/// assert kills the app, so the culprit node is in the log.
fn nan_tripwire(
    positions: Query<(Entity, &Position)>,
    transforms: Query<(Entity, &Transform)>,
    names: Query<&Name>,
    parents: Query<&ChildOf>,
    mut tripped: Local<bool>,
) {
    if *tripped {
        return;
    }
    let describe = |entity: Entity| {
        let mut chain = String::new();
        let mut e = entity;
        loop {
            let name = names
                .get(e)
                .map(|n| n.as_str().to_owned())
                .unwrap_or_else(|_| "?".into());
            chain.push_str(&format!("{e}({name}) <- "));
            match parents.get(e) {
                Ok(p) => e = p.parent(),
                Err(_) => break,
            }
        }
        chain
    };
    for (entity, position) in &positions {
        if !position.0.is_finite() {
            error!(
                "spike_client: NAN-TRIPWIRE Position on {} = {:?}",
                describe(entity),
                position.0
            );
            *tripped = true;
        }
    }
    for (entity, transform) in &transforms {
        if !(transform.translation.is_finite() && transform.rotation.is_finite()) {
            error!(
                "spike_client: NAN-TRIPWIRE Transform on {} = {:?}",
                describe(entity),
                transform
            );
            *tripped = true;
        }
    }
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

/// Give the replicated tank its LOCAL rig (map §6's `handle_new_character` pattern, increment 6's
/// swap for the primitive cuboid): avian components are not replicated, and a predicted entity
/// without a body cannot be re-simulated during rollback replay — the symptom is continuous
/// rollback from spawn, every confirmed packet disagreeing with a frozen prediction. A plain
/// system (not an observer on `Predicted`) because `SpikeTank` arrives by replication and may
/// land after the marker; also waits on the spec load (§8's spawn-before-bind race, mirrored from
/// `tank.rs`/`sandbox.rs` — `on_tank_ready` would panic on an unloaded spec). This is the exact
/// moment the §8 UNCERTAIN gets exercised: `Predicted`/`PredictionTarget` is already on the entity
/// (attached server-side at spawn) several ticks *before* the glb scene finishes loading and
/// `on_tank_ready` binds the rig — see the spike log for what was observed in that window.
///
/// Step 8: a non-predicted tank (the own tank under SPIKE_PREDICT=0; remote tanks at step 9)
/// gets the same full rig — the binder's node mapping, servos, and view anchors are what the
/// camera/HUD and `apply_servo_angles` lay the model with — but its body stays `Static`
/// (`activate_bound_rigs` skips it): replication owns its pose, nothing local simulates it.
/// `With<Remote>` = every replicated tank, whichever markers rode along.
fn attach_replicated_rig(
    tanks: Query<
        Entity,
        (
            With<Remote>,
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
        info!("spike_client: {entity} replicated tank gets local rig (spec loaded)");
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
/// marker on our avatar — claim it as the local input slot, and as the game's `Controlled` tank
/// (step 8): the camera, HUD, aim commit, and crew bar all scope off that marker unchanged.
fn claim_input_slot(add: On<Add, NetControlled>, mut commands: Commands) {
    info!(
        "spike_client: controlled entity {} — input slot",
        add.entity
    );
    commands.entity(add.entity).insert((
        InputMarker::<TankCommand>::default(),
        ActionState::<TankCommand>::default(),
        GameControlled,
        LastPosition::default(),
    ));
}

/// Headless simulate mode: write the scripted `TankCommand` into the lightyear `ActionState` slot
/// each tick. Whole-state snapshot per tick, no devices.
fn buffer_input(
    mut sim: ResMut<SimulateInput>,
    mut slots: Query<&mut ActionState<TankCommand>, With<InputMarker<TankCommand>>>,
) {
    let Ok(mut state) = slots.single_mut() else {
        return;
    };
    sim.ticks += 1;
    let t = sim.ticks;
    // Step-7 script, exercising the real sim under prediction: 2 s idle (rig binds, suspension
    // settles) → 4 s throttle 1.0 + steer 0.3 (ramp_drive + suspension + skid-steer, spanning
    // the ~2 s server perturbation) → coast to rest. The aim intention + range are held from
    // tick 0 so the turret/gun servos slew (drive_aim_servos → drive_servos) while driving;
    // one fire click at tick 300 (Reload starts ready) exercises fire + recoil + reload.
    let driving = (128..sim.drive_until).contains(&t);
    state.0.throttle = if driving { 1.0 } else { 0.0 };
    // The long course run drives dead straight (the bump/washboard are on the spawn axis); the
    // default short script arcs to exercise skid-steer.
    state.0.steer = if driving && sim.drive_until == 384 {
        0.3
    } else {
        0.0
    };
    // Hull-local, far off-axis so the yaw servo visibly slews; range 800 m dials in real
    // superelevation from the weapon's range table.
    // SPIKE_SIM_AIM_SWEEP (rollback-storm diagnostic): instead of the constant point, sweep the
    // aim around the tank at ~1.3 rad/s — a player scanning with the mouse. A human recommits the
    // aim EVERY frame from the camera ray; the constant-aim script never exercised that churn.
    state.0.aim = if std::env::var("SPIKE_SIM_AIM_SWEEP").is_ok() {
        let theta = 0.02 * t as f32;
        Some(Vec3::new(800.0 * theta.sin(), 0.0, -800.0 * theta.cos()))
    } else {
        Some(Vec3::new(200.0, 0.0, -800.0))
    };
    state.0.range = 800.0;
    state.0.fire_primary = t == sim.fire_tick;
}

/// Windowed input path: the game's own client writers (`gather_commands`, `commit_aim`,
/// `drive_gunner_aim`, the range dial, the crew bar) have already filled the `Controlled` tank's
/// `TankCommand` at render rate — copy it into lightyear's `ActionState` slot each tick, where the
/// input plugin buffers it for the wire and for rollback replay. The reverse bridge (net.rs) hands
/// it straight back to the sim, so locally the round trip is an identity copy — the buffer is the
/// point. Menu open = a default command: the tank coasts to a stop instead of holding the last
/// input, and clicks in the menu don't fire.
///
/// On a non-predicted own tank (SPIKE_PREDICT=0) the local sim must not act on what the writers
/// wrote — the reverse bridge already skips it, but these fields leak past that gate, so they are
/// cleared *after* the copy: `aim` (drive_aim_servos would slew the turret with zero latency,
/// fighting `apply_servo_angles`), the fire edges (a local tracer would spawn), `crew_swap` (local
/// crew would change). Throttle/steer/range stay — the Static body ignores forces, and clearing
/// per-tick values the writers only refresh per-frame would starve ticks 2..N of a multi-tick
/// frame on the wire. Keyed on `Predicted`, not `Interpolated` — see net.rs's
/// `activate_bound_rigs` on why the latter can't discriminate.
fn feed_action_state(
    menu: Res<MenuOverlay>,
    mut slots: Query<
        (
            &mut TankCommand,
            &mut ActionState<TankCommand>,
            Has<Predicted>,
        ),
        With<InputMarker<TankCommand>>,
    >,
) {
    for (mut command, mut state, predicted) in &mut slots {
        state.0 = if menu.open {
            TankCommand::default()
        } else {
            *command
        };
        if !predicted {
            command.aim = None;
            command.fire_primary = false;
            command.fire_secondary = false;
            command.crew_swap = None;
        }
    }
}

/// Esc toggles the menu overlay: cursor released over the overlay, re-grabbed on close. The
/// networked replacement for `state::client_plugin`'s pause — `AppState` stays `Playing`
/// throughout (see [`MenuOverlay`]).
fn toggle_menu(
    keys: Res<ButtonInput<KeyCode>>,
    mut menu: ResMut<MenuOverlay>,
    window: Single<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>,
    nodes: Query<Entity, With<MenuOverlayNode>>,
    mut commands: Commands,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    menu.open = !menu.open;
    let (mut window, mut cursor) = window.into_inner();
    if menu.open {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
        commands
            .spawn((
                MenuOverlayNode,
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
            ))
            .with_children(|parent| {
                parent.spawn((
                    Text::new("MENU\nEsc to close"),
                    TextFont {
                        font_size: FontSize::Px(48.0),
                        ..default()
                    },
                    TextColor(Color::WHITE),
                ));
            });
    } else {
        // Re-center before locking, so mouse-look resumes owned by this window (same move as the
        // game's `grab_cursor`).
        let center = window.size() / 2.0;
        window.set_cursor_position(Some(center));
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
        for node in &nodes {
            commands.entity(node).despawn();
        }
    }
}

/// Initial cursor grab on entering `Playing` — the one piece of `state::client_plugin` this bin
/// does want (mouse aim needs a locked cursor from the first frame).
fn grab_cursor(window: Single<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>) {
    let (mut window, mut cursor) = window.into_inner();
    let center = window.size() / 2.0;
    window.set_cursor_position(Some(center));
    cursor.grab_mode = CursorGrabMode::Locked;
    cursor.visible = false;
}

/// Simulate mode: exit cleanly once the script has played out (long enough to cover the ~2s
/// server perturbation and settle afterward), or bail on a wall-clock timeout if the connection
/// never came up.
fn simulate_watchdog(
    simulate: Res<SimulateInput>,
    time: Res<Time<Real>>,
    mut exit: MessageWriter<AppExit>,
) {
    if simulate.ticks >= simulate.total {
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

/// Periodic own-tank position log (every ~2 s) — diffed against the server's own periodic log
/// for the convergence criterion (predicted) or the interpolated-tracking evidence (step 8's
/// SPIKE_PREDICT=0 runs).
fn log_position(
    tanks: Query<(Entity, &Position), (With<Remote>, With<SpikeTank>)>,
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
