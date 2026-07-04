//! The networked client: connects to a local server over UDP+netcode and is *playable* — windowed
//! mode mounts the game's real presentation + device gather (`NetClientPlugin`: camera, HUD, mouse
//! aim, gunner optic, range dial, crew bar), marks the replicated own tank with the game's
//! `Controlled` on possession, and feeds the gathered `TankCommand` into lightyear's `ActionState`
//! each tick (`feed_action_state`). The own tank is always PREDICTED — the committed model: input
//! answers instantly, rollback reconciles against the authority. Esc is a cursor-release menu
//! overlay, NOT a pause: the sim never stops (there is no online pause; a frozen predicting client
//! desyncs from a server that keeps ticking).
//!
//! Run with `cargo run --bin client --features net`. Pass `--simulate-input` (or set
//! `SPIKE_SIMULATE_INPUT`) to run headless and programmatically drive throttle/aim/fire for a few
//! seconds, proving prediction + rollback under a real sim workload without a human at the keyboard.

use core::time::Duration;
use std::net::{Ipv4Addr, SocketAddr};

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use lightyear::prelude::client::*;
use lightyear::prelude::input::client::InputSystems;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::{Controlled as NetControlled, *};

use super::{client_smoothing_plugin, diagnostics, harness, open_gameplay_gate, physics, rig};
use crate::command::TankCommand;
use crate::state::AppState;
use crate::tank::{Controlled as GameControlled, load_tank_spec};
use crate::{NetClientPlugin, SimPlugin};

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

pub fn run() {
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
        .init_resource::<harness::SimulateInput>();
    } else {
        app.add_plugins(DefaultPlugins);
        // Never drop below the 64 Hz tick: the default `WinitSettings::game()` throttles an
        // UNFOCUSED window to 60 Hz reactive updates — under tick rate, so an alt-tabbed client
        // drifts behind the server and resyncs on refocus (lightyear #1113's jitter class).
        app.insert_resource(bevy::winit::WinitSettings::continuous());
        if sim_windowed {
            app.init_resource::<harness::SimulateInput>();
        }
    }

    // Ordering per the spike map §3: ClientPlugins, then protocol registration, then the Client
    // entity. `net::plugin` also mounts `LightyearAvianPlugin` + Position/Rotation/velocity
    // registration (map §5); `physics_plugins()` gives the matching disables.
    app.add_plugins(ClientPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / 64.0),
    });
    app.add_plugins(super::plugin);
    app.add_plugins(physics::physics_plugins());
    // The render half of prediction (frame interpolation + armed rollback correction) — client
    // only; the server has no `Predicted` view to smooth. Mounted in simulate mode too: headless
    // it idles harmlessly, and `SPIKE_SIM_WINDOWED` diagnoses the real presentation stack.
    app.add_plugins(client_smoothing_plugin);
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
            Name::new("Client"),
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
    let delay_ticks = harness::input_delay_ticks();
    if delay_ticks > 0 {
        info!("client: SPIKE_INPUT_DELAY_TICKS={delay_ticks}");
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
        info!("client: connecting to {server_addr} as client_id={client_id}");
    });
    app.add_systems(Startup, load_tank_spec);

    app.add_observer(diagnostics::log_connected)
        .add_observer(claim_input_slot)
        .add_observer(diagnostics::log_predicted_tank)
        .init_resource::<diagnostics::RollbackWatch>()
        .init_resource::<diagnostics::TurretWatch>()
        .add_systems(
            Update,
            (
                rig::attach_replicated_rig,
                diagnostics::nan_tripwire,
                diagnostics::report_orphan_transforms,
                open_gameplay_gate,
                diagnostics::count_rig_binds,
                diagnostics::watch_rollback_metrics,
                diagnostics::watch_turret_pose,
                diagnostics::log_snap,
                diagnostics::log_positions,
                diagnostics::count_shell_spawns,
                diagnostics::log_prediction_diagnostics,
                diagnostics::log_sim_evidence,
            ),
        );
    if simulate {
        app.add_systems(Update, harness::simulate_watchdog).add_systems(
            FixedPreUpdate,
            // Rollback replays re-run FixedPreUpdate too (map §8) — lightyear itself restores
            // `ActionState` from the `InputBuffer` per replayed tick (and `buffer_action_state`
            // is `Without<Rollback>`, so the buffer can't be corrupted), but without this gate
            // the scripted tick counter would count every replayed tick (verified live: 640
            // "ticks" burned in <5 s wall).
            harness::buffer_input
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

/// Possession (spike map §6): the server's `ControlledBy` arrives as lightyear's `Controlled`
/// marker on our avatar — claim it as the local input slot, and as the game's `Controlled` tank
/// (step 8): the camera, HUD, aim commit, and crew bar all scope off that marker unchanged.
fn claim_input_slot(add: On<Add, NetControlled>, mut commands: Commands) {
    info!("client: controlled entity {} — input slot", add.entity);
    commands.entity(add.entity).insert((
        InputMarker::<TankCommand>::default(),
        ActionState::<TankCommand>::default(),
        GameControlled,
        diagnostics::LastPosition::default(),
    ));
}

/// Windowed input path: the game's own client writers (`gather_commands`, `commit_aim`,
/// `drive_gunner_aim`, the range dial, the crew bar) have already filled the `Controlled` tank's
/// `TankCommand` at render rate — copy it into lightyear's `ActionState` slot each tick, where the
/// input plugin buffers it for the wire and for rollback replay. The reverse bridge (net::protocol)
/// hands it straight back to the sim, so locally the round trip is an identity copy — the buffer is
/// the point. Menu open = a default command: the tank coasts to a stop instead of holding the last
/// input, and clicks in the menu don't fire.
fn feed_action_state(
    menu: Res<MenuOverlay>,
    mut slots: Query<(&TankCommand, &mut ActionState<TankCommand>), With<InputMarker<TankCommand>>>,
) {
    for (command, mut state) in &mut slots {
        state.0 = if menu.open {
            TankCommand::default()
        } else {
            *command
        };
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

/// Initial cursor grab on entering `Playing` — the one piece of `state::client_plugin` this module
/// does want (mouse aim needs a locked cursor from the first frame).
fn grab_cursor(window: Single<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>) {
    let (mut window, mut cursor) = window.into_inner();
    let center = window.size() / 2.0;
    window.set_cursor_position(Some(center));
    cursor.grab_mode = CursorGrabMode::Locked;
    cursor.visible = false;
}
