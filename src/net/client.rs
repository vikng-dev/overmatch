//! The networked client: connects to a local server over UDP+netcode and is *playable* — windowed
//! mode mounts the game's real presentation + device gather (`NetClientPlugin`: camera, HUD, mouse
//! aim, gunner optic, range dial, crew bar), marks the replicated own tank with the game's
//! `Controlled` on possession, and feeds the gathered `TankCommand` into lightyear's `ActionState`
//! each tick (`feed_action_state`). The own tank is always PREDICTED — the committed model: input
//! answers instantly, rollback reconciles against the authority. Esc is a cursor-release menu
//! overlay, NOT a pause: the sim never stops (there is no online pause; a frozen predicting client
//! desyncs from a server that keeps ticking).
//!
//! Run with `cargo run` (the `overmatch` bin, `net` feature on by default). Pass `--simulate-input`
//! (or set `SPIKE_SIMULATE_INPUT`) to run headless and programmatically drive throttle/aim/fire for a
//! few seconds, proving prediction + rollback under a real sim workload without a human at the keyboard.

use core::time::Duration;
use std::hash::{BuildHasher, Hasher};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use bevy::app::ScheduleRunnerPlugin;
use bevy::asset::AssetPlugin;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use lightyear::prediction::correction::CorrectionPolicy;
use lightyear::prelude::client::*;
use lightyear::prelude::input::client::InputSystems;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::{Controlled as NetControlled, *};

use super::protocol::{FireEvent, NetTank};
use super::{client_smoothing_plugin, diagnostics, harness, open_gameplay_gate, physics, rig};
use crate::ballistics::FireShell;
use crate::command::TankCommand;
use crate::state::{AppState, GameplaySet};
use crate::tank::{
    Controlled as GameControlled, Muzzle, PendingTankAssets, TankRoot, TankSim, Weapon,
    WeaponIndex, load_tank_assets,
};
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
                // Exe-relative asset root (see `asset_root`), so a bundled/double-clicked client
                // finds `assets/` regardless of cwd — headless automation loads the same rig.
                .set(AssetPlugin {
                    file_path: asset_root(),
                    ..default()
                })
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
        // Exe-relative asset root (see `asset_root`): a double-clicked `overmatch`/`overmatch.exe`
        // (or a macOS `.app`) finds `assets/` beside it no matter the launch cwd.
        app.add_plugins(DefaultPlugins.set(AssetPlugin {
            file_path: asset_root(),
            ..default()
        }));
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
    // The render-space error layer (client only): with `instant_correction` on the PredictionManager
    // below, lightyear snaps the sim pose to the corrected present in one frame; this layer
    // accumulates that snap as a decaying offset on the predicted root's render `Transform` so the
    // VIEW never lurches.
    app.add_plugins(super::render_error::plugin);
    // The rollback watchdog (client only): the backstop for lightyear's receive-time mismatch
    // check, which starves permanently at zero prediction margin — exactly where `balanced()`
    // input delay puts a LAN/loopback client (see the module doc for the vendored mechanism).
    app.add_plugins(super::watchdog::plugin);
    // Step 7: the real sim — same `SimPlugin` the server mounts, so client-side rollback replay
    // re-runs the actual driving/aim/shooting systems, not a stub.
    app.add_plugins(SimPlugin);
    // Server-authoritative combat: mark this app a REPLICA so `ballistics` flies/sparks shells
    // cosmetically but never deposits HP or applies hit impulse — damage/death emerge from the
    // server's replicated per-volume health (`net::protocol::NetHealth`) instead of a divergent
    // local kill. Only the net client sets this; SP / sandbox / server stay authorities.
    app.insert_resource(crate::ClientReplica);
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
    // Passive jitter-trace recorder: frame + tick + rollback rows with prediction/correction extras.
    // Idle unless `SPIKE_TRACE` is set.
    app.add_plugins(crate::trace::client_plugin);
    // Diagnostic contact probe: per-tick broad/narrow-phase state for the predicted tank's
    // hull-vs-terrain pairs. Idle (nothing registered) unless `SPIKE_CONTACT_PROBE` is set.
    app.add_plugins(super::contact_probe::plugin);
    // FPS + frame-time diagnostics for the bottom-right debug panel (`net::debug_hud`, mounted in
    // `NetClientPlugin`) — NOT part of `DefaultPlugins`, so it must be added explicitly here.
    app.add_plugins(bevy::diagnostic::FrameTimeDiagnosticsPlugin::default());

    // Server address resolution, in priority order:
    //   1. runtime `OVERMATCH_SERVER` — points a dev/playtest client at any server.
    //   2. compile-time `OVERMATCH_DEFAULT_SERVER` (baked via `option_env!`): CI sets it on the
    //      release client to the deployed droplet, so a double-clicked build connects there with no
    //      env. A dev build leaves it unset and falls through.
    //   3. loopback — the single-machine dev/harness default.
    // Both the runtime and the baked form accept `host:port` (a full `SocketAddr`) or a bare IP
    // (default port appended); see [`parse_server_addr`].
    let loopback = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), SERVER_PORT);
    let server_addr = match std::env::var("OVERMATCH_SERVER") {
        // A malformed RUNTIME override falls back to loopback, not to the baked default: an explicit
        // bad `OVERMATCH_SERVER` shouldn't silently redirect the player to the compiled-in server.
        Ok(raw) => parse_server_addr(&raw).unwrap_or_else(|| {
            error!("client: OVERMATCH_SERVER=\"{raw}\" is not an ip or ip:port — using loopback");
            loopback
        }),
        // No runtime override: use the compile-time baked default if this build has a (valid) one.
        Err(_) => option_env!("OVERMATCH_DEFAULT_SERVER")
            .and_then(parse_server_addr)
            .unwrap_or(loopback),
    };
    // A per-process RANDOM client id, generated once at startup. NOT the PID (the old
    // `u64::from(std::process::id())`): netcode does NOT enforce client-id uniqueness, so a duplicate
    // id silently OVERWRITES the server's `PeerId → Entity` mapping, and ownership routing resolves by
    // RAW id value — `PredictionTarget::to_clients(NetworkTarget::Single(id))` and
    // `PeerMetadata.mapping` both key on the value, not on which machine sent it. Two machines that
    // happened to share a PID would therefore collide: the server misroutes prediction / `ControlledBy`,
    // and an opponent's tank arrives on the wrong client carrying `Predicted`/`Controlled` (turret
    // desync, one client driving both tanks, input contention). A well-distributed random u64 makes a
    // cross-machine collision vanishingly unlikely. `RandomState::new()` is seeded from OS randomness on
    // every call, so a fresh hasher's `finish()` yields a random u64 with NO new dependency. Generated
    // once here and stable for the session; a fresh identity across restarts is fine (back-to-back runs
    // still can't collide inside the server's disconnect timeout — the whole reason the PID existed).
    let client_id = std::hash::RandomState::new().build_hasher().finish();
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
    // Reconciliation DEPTH controls (jitter investigation: felt MP jitter = frequency × depth ×
    // chaos-gain; the threshold coarsening in `protocol` attacked frequency, this attacks depth).
    // Every rollback restores a ~12-tick-old confirmed state and RE-SIMULATES to the present; that
    // replay is chaotic through friction/contact, landing the corrected present 6–35× farther from
    // the old present than the cm-scale client/server error that triggered it. Depth = how many
    // ticks each replay re-runs, and two lightyear defaults inflate it, so we ran the library's
    // maximum-violence configuration by default. Both are set on the input timeline below.
    //
    // (1) Input delay. Every tick of input delay is a tick prediction does NOT run ahead, so it
    //     shrinks the prediction window (hence max rollback depth) directly. `balanced()` spends up
    //     to ~50 ms of latency on input delay before any prediction — lightyear's own recommended
    //     setting "to reduce the amount of rollback ticks needed (to reduce the rollback visual
    //     artifacts and CPU costs)" (lightyear_sync input.rs). The old `PredictionManager::default()`
    //     path selected `no_input_delay()`: 100% of latency absorbed by prediction, maximum depth.
    //     `SPIKE_INPUT_DELAY_TICKS` overrides for A/B — `=0` restores `no_input_delay()` (the old
    //     behavior), `=n` pins `fixed_input_delay(n)`; unset = the shipping `balanced()`.
    let (input_delay, delay_label) = match harness::input_delay_ticks() {
        None => (
            InputDelayConfig::balanced(),
            "balanced (≤3-tick input delay absorbs ~50ms before prediction)".to_string(),
        ),
        Some(0) => (
            InputDelayConfig::no_input_delay(),
            "no_input_delay (SPIKE_INPUT_DELAY_TICKS=0 — old max-prediction behavior)".to_string(),
        ),
        Some(n) => (
            InputDelayConfig::fixed_input_delay(n),
            format!("fixed_input_delay({n}) (SPIKE_INPUT_DELAY_TICKS={n})"),
        ),
    };
    // (2) Sync jitter margin. `jitter_multiple` scales measured jitter into how far ahead prediction
    //     runs purely as jitter safety — pure depth. lightyear defaults to 4 (99.7% packet
    //     coverage); with the 20 ms test conditioner that's ~5 ticks of margin baked into the
    //     prediction window. We ship 2 (95%). `SPIKE_JITTER_MULTIPLE` overrides for A/B; other
    //     `SyncConfig` fields keep their defaults (`jitter_margin: 1.0` etc.).
    let jitter_multiple = harness::jitter_multiple();
    let sync_config = SyncConfig {
        jitter_multiple,
        ..default()
    };
    info!("client: input delay = {delay_label}; sync jitter_multiple = {jitter_multiple}");
    let client = app
        .world_mut()
        .spawn((
            Name::new("Client"),
            Client::default(),
            Link::new(conditioner),
            LocalAddr(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0)),
            PeerAddr(server_addr),
            // (3) The input-rollback branch is a permanent no-op for us — we never rebroadcast
            //     inputs and our own inputs can't mismatch (we author them), so `RollbackMode`'s
            //     input arm only costs a per-frame input-buffer scan. Disable it; STATE rollback
            //     (the real one, against replicated Position/Rotation/velocity) stays `Check`, and
            //     everything else keeps its `RollbackPolicy` default (`max_rollback_ticks: 100`).
            PredictionManager {
                rollback_policy: RollbackPolicy {
                    input: RollbackMode::Disabled,
                    ..default()
                },
                // Let the sim SNAP: collapse lightyear's built-in visual correction to a single frame
                // (`decay_period` 1 ms / `decay_ratio` 1e-7 — the error underflows to ~0 the frame the
                // rollback lands), so the lightyear-visible pose reaches the corrected present at once.
                // ALL visible smoothing then lives in `net::render_error`, which offsets the render
                // `Transform` and decays it with a capped correction velocity — the "view never snaps"
                // layer. Leaving the default 200 ms half-life here would double-smooth (and lightyear's
                // has no velocity cap), reintroducing the lurch this layer exists to kill.
                correction_policy: CorrectionPolicy::instant_correction(),
                ..default()
            },
            // The depth knobs (1)+(2), inserted ALWAYS (no longer only under the env lever): the
            // shipping default is `balanced()` + `jitter_multiple: 2`, not lightyear's max-violence
            // `no_input_delay()` + `jitter_multiple: 4`. `PredictionManager` `#[require]`s an
            // `InputTimelineConfig`; giving it explicitly here wins over that default insert.
            InputTimelineConfig::new(sync_config, input_delay),
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
    // Connect only once the tank assets (spec + glb scene) are loaded. The client still preloads
    // before connecting — but no longer to guard a bind window (that window is gone: the sim body
    // now spawns whole from extracted data the moment the replicated root lands). The spec feeds
    // the spawner (`attach_replicated_rig` → `spawn_tank_sim`), and the glb feeds the geometry
    // extractor + the shadow check + the view attach (`bind_tank_view`); preloading both keeps the
    // view pop-in to ~a frame of scene instantiation instead of a multi-second glb load.
    // (No local ground spawn here: `SimPlugin` → `world::plugin` builds the real game terrain
    // (Terrain-layer static slab + test course) on both sides — rollback replays collide with it,
    // and the wheels' suspension rays (filtered to `Layer::Terrain`) actually hit it.)
    app.add_systems(
        Update,
        move |assets: Option<Res<PendingTankAssets>>,
              asset_server: Res<AssetServer>,
              mut connected: Local<bool>,
              mut commands: Commands| {
            if *connected {
                return;
            }
            let Some(assets) = assets else { return };
            if !assets.loaded(&asset_server) {
                return;
            }
            *connected = true;
            commands.trigger(Connect { entity: client });
            info!(
                "client: tank assets loaded — connecting to {server_addr} as client_id={client_id}"
            );
        },
    );
    app.add_systems(Startup, load_tank_assets);

    app.add_observer(diagnostics::log_connected)
        .add_observer(claim_input_slot)
        .add_observer(diagnostics::log_predicted_tank)
        .init_resource::<diagnostics::RollbackWatch>()
        .init_resource::<diagnostics::TurretWatch>()
        .add_systems(
            Update,
            (
                rig::attach_replicated_rig,
                receive_fire_events,
                diagnostics::nan_tripwire,
                open_gameplay_gate,
                diagnostics::watch_rollback_metrics,
                diagnostics::watch_turret_pose,
                diagnostics::log_snap,
                diagnostics::log_positions,
                diagnostics::count_shell_spawns,
                diagnostics::log_prediction_diagnostics,
                diagnostics::log_sim_evidence,
            ),
        );
    // Remote-barrel recoil (the "derive the consequence" half of the opponent-fire seam). Split
    // across two clocks ON PURPOSE — see the verification finding in `receive_fire_events`:
    //   - `receive_fire_events` (Update, above) DRAINS the receiver at render rate and captures each
    //     shot's CAUSE (shooter + weapon slot) into `PendingRecoilKicks`. It must stay render-rate:
    //     lightyear clears an undrained `MessageReceiver` in `Last` EVERY frame, so a drain on the
    //     fixed clock would silently lose every `FireEvent` that arrives on a 0-fixed-tick frame
    //     (the majority of frames whenever render rate exceeds the 64 Hz tick, and ~all of them in
    //     the headless harness) — a systematic client-side drop, not the network loss the channel is
    //     built to tolerate.
    //   - `apply_pending_recoil_kicks` (here, FixedUpdate) DERIVES the spring kick from this client's
    //     OWN local spec and writes it into `TankSim` on the sim clock, `.before(GameplaySet)` so
    //     `shooting::apply_recoil` (in `GameplaySet`) springs it the same tick. `TankSim` is
    //     fixed-clock sim truth; writing it from Update would be a render→sim leak (non-deterministic
    //     across 0/1/2-tick frames). Gated `not(is_in_rollback)` like `feed_action_state`: a rollback
    //     replays `FixedMain` N times, and re-applying a queued one-shot kick per replayed tick would
    //     multiply it — the queue is drained exactly once, on a real tick.
    app.init_resource::<PendingRecoilKicks>();
    app.add_systems(
        FixedUpdate,
        apply_pending_recoil_kicks
            .before(GameplaySet)
            // Gate on `Playing` to match its consumer: `shooting::apply_recoil` lives in
            // `GameplaySet` (Playing-only), so the applier that WRITES the kick and the system that
            // SPRINGS it must agree on when they may run — otherwise a `FireEvent` draining outside
            // `Playing` writes a kick into `TankSim` that `apply_recoil` never releases.
            .run_if(in_state(AppState::Playing))
            .run_if(not(is_in_rollback)),
    );
    // Ownership trace (opt-in via `OVERMATCH_OWNERSHIP_TRACE`; KEPT — useful): once per second, log
    // every `NetTank`'s ownership markers, so a two-client loopback run can confirm that each client's
    // OWN tank is the sole carrier of `Controlled`/`InputMarker`/`Predicted` and every opponent is
    // `Interpolated`+`Remote` only. This is exactly the axis the shared-PID bug corrupted (an opponent
    // arriving with `Controlled`/`Predicted`). Registered only when the env var is present, so normal
    // runs stay quiet.
    if std::env::var("OVERMATCH_OWNERSHIP_TRACE").is_ok() {
        app.add_systems(Update, log_tank_ownership);
    }
    if simulate {
        app.add_systems(Update, harness::simulate_watchdog)
            .add_systems(
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

/// Parse a server address from a string in either accepted form: a full `host:port` `SocketAddr`,
/// or a bare IP (the default [`SERVER_PORT`] is appended). Shared by the runtime `OVERMATCH_SERVER`
/// override and the compile-time `OVERMATCH_DEFAULT_SERVER` baked default. `None` on a malformed
/// value; the caller decides the fallback.
fn parse_server_addr(raw: &str) -> Option<SocketAddr> {
    raw.parse::<SocketAddr>().ok().or_else(|| {
        raw.parse::<IpAddr>()
            .ok()
            .map(|ip| SocketAddr::new(ip, SERVER_PORT))
    })
}

/// `AssetPlugin`'s `file_path` — the `assets/` directory this client reads from, as a `String`
/// (what `AssetPlugin` wants). Delegates to the shared, unit-tested resolver at `crate::assets`;
/// the resolution logic (macOS `.app` → `Contents/Resources/assets`, flat Windows/Linux archive →
/// `exe_dir/assets`, env overrides) lives there so the tank bake (`bake`, which compiles without the
/// `net` feature) resolves the exact same directory. See `crate::assets::asset_root`.
fn asset_root() -> String {
    crate::assets::asset_root().to_string_lossy().into_owned()
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

/// Opt-in ownership trace (`OVERMATCH_OWNERSHIP_TRACE`): once per second, dump every replicated
/// tank's ownership markers. For a two-client loopback verification this is the ground truth — the
/// OWN tank must be the only one with `Controlled` (the game marker `claim_input_slot` inserts),
/// `InputMarker<TankCommand>`, and `Predicted`; every opponent must be `Interpolated`+`Remote` only.
/// Any opponent showing `Controlled`/`Predicted` is the ownership-misroute this fix targets.
/// Throttled to ~1 Hz via a `Local` deadline so it never floods the log.
#[expect(clippy::type_complexity, reason = "one-off diagnostic marker snapshot")]
fn log_tank_ownership(
    tanks: Query<
        (
            Entity,
            Has<GameControlled>,
            Has<InputMarker<TankCommand>>,
            Has<Predicted>,
            Has<Interpolated>,
            Has<Remote>,
        ),
        With<NetTank>,
    >,
    time: Res<Time>,
    mut next: Local<f32>,
) {
    let now = time.elapsed_secs();
    if now < *next {
        return;
    }
    *next = now + 1.0;
    for (entity, controlled, input_marker, predicted, interpolated, remote) in &tanks {
        info!(
            "ownership: {entity} controlled={controlled} input_marker={input_marker} predicted={predicted} interpolated={interpolated} remote={remote}"
        );
    }
}

/// Barrel recoil kicks awaiting the sim clock: `(shooter root, weapon slot)` per opponent shot,
/// captured at render rate by [`receive_fire_events`] and consumed on the fixed clock by
/// [`apply_pending_recoil_kicks`]. The queue is the seam between the render-rate message drain (which
/// must stay render-rate — lightyear clears an undrained receiver every frame in `Last`) and the
/// fixed-clock `TankSim` write (which must be on the sim clock — see [`apply_pending_recoil_kicks`]).
#[derive(Resource, Default)]
struct PendingRecoilKicks(Vec<(Entity, usize)>);

/// Drain the server's cosmetic `FireEvent`s (`net::protocol::FireEvent`) and, for each: re-raise a
/// local `FireShell` (the visible tracer) AND enqueue the shot's recoil CAUSE onto
/// [`PendingRecoilKicks`] — the CLIENT half of the opponent-fire seam. A remote (interpolated) tank
/// runs no local `fire`, so this is how its shots become visible AND how its barrel kicks: the
/// re-raised `FireShell` flies through the same `integrate_projectiles` (already damage/hit-gated off
/// under `ClientReplica`, so the shell is cosmetic BY CONSTRUCTION), and the enqueued
/// `(shooter, slot)` lets `apply_pending_recoil_kicks` derive the spring kick from this client's own
/// local spec on the sim clock.
///
/// `MessageReceiver<FireEvent>` is a required component of the `Client` (the `ServerToClient`
/// direction registered in `net::protocol`), so it rides the client link entity. `shooter: None` on
/// the re-raised `FireShell`: the client never re-broadcasts (only the server owns attribution).
/// A `FireEvent` whose direction fails the `Dir3` guard is skipped entirely (no tracer, no kick),
/// mirroring `fire`'s bore guard.
///
/// **Ignore a `FireEvent` naming a tank THIS client simulates locally** (one carrying
/// `ActionState<TankCommand>` — exactly the tanks that run `shooting::fire` here and have therefore
/// already flown their shell and kicked their own barrel). `broadcast_fire` normally excludes the
/// shooter (`AllExceptSingle(owner)`), but its `All` fallback (owner link mid-disconnect, no
/// `RemoteId`) can deliver a client its OWN shot; without this guard that would double the tracer
/// AND, worse, add a recoil kick to the own tank's `local_rollback::<TankSim>()`-tracked sim from a
/// message OUTSIDE rollback. The guard is on `ActionState`, not `Predicted`/`Controlled`, because it
/// is semantic ("don't touch a tank that fires locally") and survives the predict-everyone change,
/// where remote tanks gain `ActionState` and `FireEvent` is deleted outright. Skipping the whole
/// event covers BOTH the tracer spawn and the recoil enqueue (the kick is only ever queued here).
///
/// **Why this stays in `Update` (render rate), NOT the fixed clock.** Verified against vendored
/// `lightyear_messages` 0.28: `MessageReceiver<M>.recv` is a plain `Vec` that `receive()` drains, and
/// the plugin schedules a `clear` system in `Last` every frame (`MessagePlugin`, plugin.rs) that
/// empties any receiver NOT drained that frame — messages are received in `PreUpdate`
/// (`MessageSystems::Receive`) and live for exactly one frame. `RunFixedMainLoop` (hence `FixedUpdate`)
/// runs BEFORE `Update`/`Last` and executes 0..N times per frame, so draining from a fixed schedule
/// would drop every `FireEvent` arriving on a 0-tick frame — common above 64 Hz render, near-total in
/// the headless `2 ms` runner. Draining here, in `Update` (always once per frame, before `Last`),
/// loses none; only the `TankSim` write is deferred to the sim clock. `Update` is also outside every
/// rollback replay (replays run inside `RunFixedMainLoop`), so the drain and the cosmetic-shell spawn
/// can't be re-run by a rollback — preserving today's render-rate shell-spawn timing exactly.
fn receive_fire_events(
    mut receivers: Query<&mut MessageReceiver<FireEvent>>,
    // The set of tanks THIS client simulates locally (own predicted tank; later, under
    // predict-everyone, every predicted tank). They run `shooting::fire` and kick themselves, so a
    // `FireEvent` naming one of them is our own shot echoed back and must be ignored — see the doc.
    locally_fired: Query<(), With<ActionState<TankCommand>>>,
    mut pending: ResMut<PendingRecoilKicks>,
    mut commands: Commands,
) {
    for mut receiver in &mut receivers {
        for event in receiver.receive() {
            // `event.shooter` is already entity-mapped to the local replica. If that tank fires
            // locally, drop the whole event: no duplicate tracer, no self-kick into rollback state.
            if locally_fired.contains(event.shooter) {
                continue;
            }
            let Ok(direction) = Dir3::new(event.direction) else {
                continue; // corrupt bore off the wire — hold the tracer rather than fire NaN
            };
            commands.trigger(FireShell {
                origin: event.origin,
                direction,
                speed: event.speed,
                caliber: event.caliber,
                mass: event.mass,
                shooter: None,
            });
            // Capture the CAUSE (which tank's which weapon fired); the fixed-clock applier below
            // derives the spring kick from this client's own local spec. `event.shooter` is already
            // entity-mapped to the local replica; `event.weapon` is bounds-checked at apply time.
            pending.0.push((event.shooter, event.weapon as usize));
        }
    }
}

/// Kick each opponent shot's barrel recoil spring, on the SIM clock — the "derive the consequence"
/// half of remote recoil. Drains [`PendingRecoilKicks`] (captured at render rate by
/// [`receive_fire_events`]) and, for each `(shooter, slot)`, finds the firing weapon on THIS client's
/// own local rig and hands `(sim, slot, weapon)` to the shared [`crate::shooting::kick_recoil`] — the
/// SAME model `shooting::fire` uses for a locally-fired shot (barrel + recoil gate included), so the
/// shooter's own recoil and every opponent's view of that shot can't diverge. Nothing about the
/// impulse rides the wire — only which weapon fired; each machine derives the identical kick from its
/// shared RON spec (the muzzle carries the `Weapon` config, keyed by `WeaponIndex`).
/// `shooting::apply_recoil` then springs the barrel back home from this velocity.
///
/// Scheduled `FixedUpdate`, `.before(GameplaySet)` so `apply_recoil` (in the set) sees the kick the
/// same tick; gated `in_state(Playing)` to match that consumer (see the registration); and gated
/// `not(is_in_rollback)`: `TankSim` is fixed-clock sim truth (a render-rate write would be a
/// render→sim leak, non-deterministic across 0/1/2-tick frames), and a rollback replays `FixedMain`
/// N times — draining the queue only on a real tick applies each one-shot kick exactly once.
///
/// The shooter is normally an interpolated remote (a player's own `FireEvent` is excluded by
/// `broadcast_fire`'s `AllExceptSingle(owner)`; the bot is owned by no one), whose `TankSim` is not
/// rollback-checked. But `broadcast_fire`'s `All` fallback CAN deliver a client its own shot, which
/// would kick the predicted own tank's `local_rollback::<TankSim>()`-tracked sim from a message —
/// so [`receive_fire_events`] drops any shot whose shooter carries `ActionState<TankCommand>` (the
/// locally-fired set) before it ever reaches this queue. Nothing rollback-tracked is kicked here.
///
/// Skips silently on a missing tank, a slot with no matching muzzle, an out-of-range slot, or a
/// recoil-less weapon (a coax) — a replica may not have finished spawning its rig, exactly as the
/// `FireEvent` direction guard tolerates a bad bore.
fn apply_pending_recoil_kicks(
    mut pending: ResMut<PendingRecoilKicks>,
    muzzles: Query<(&WeaponIndex, &Weapon, &TankRoot), With<Muzzle>>,
    mut sims: Query<&mut TankSim>,
) {
    for (shooter, slot) in pending.0.drain(..) {
        // Find the firing weapon on THIS machine's local rig, keyed by the slot; `kick_recoil` owns
        // the rest of the decision (barrel + recoil present, slot valid) so it can't diverge from
        // `shooting::fire`. A missing muzzle is a rig still spawning — skip.
        let Some((_, weapon, _)) = muzzles
            .iter()
            .find(|(index, _, root)| root.0 == shooter && index.0 == slot)
        else {
            continue;
        };
        if let Ok(mut sim) = sims.get_mut(shooter) {
            crate::shooting::kick_recoil(&mut sim, slot, weapon);
        }
    }
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

/// `apply_pending_recoil_kicks` derives a remote shot's barrel recoil from the LOCAL spec — these
/// exercise that derivation directly against a minimal rig. An external integration test can't reach
/// the sim types (`crate::tank` is a private module — `TankSim`/`Weapon`/`WeaponIndex` are not
/// externally nameable), so the honest test lives here in-crate, over real ECS state.
#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::spec::{RecoilSpec, Trigger};
    use crate::tank::WeaponState;

    /// A one-weapon `Weapon` config with the given `recoil` spec and `barrel` node — the only two
    /// fields `kick_recoil`'s gate reads; the rest are filled to satisfy the struct.
    fn weapon(recoil: Option<RecoilSpec>, barrel: Option<Entity>) -> Weapon {
        Weapon {
            name: "MainGun".into(),
            speed: 800.0,
            caliber: 0.088,
            mass: 10.2,
            reload: 8.0,
            recoil,
            barrel,
            fire: Vec::new(),
            load: Vec::new(),
            trigger: Trigger::Primary,
        }
    }

    /// A `RecoilSpec` with the given kick (stiffness/damping are irrelevant to the derive).
    fn recoil(kick: f32) -> RecoilSpec {
        RecoilSpec {
            kick,
            stiffness: 100.0,
            damping: 5.0,
        }
    }

    /// Spawn a tank root with an `n`-slot `TankSim` plus a muzzle at `slot` whose `Weapon` carries
    /// `recoil` and `barrel`; returns the root entity.
    fn spawn_rig(
        world: &mut World,
        slots: usize,
        slot: usize,
        recoil: Option<RecoilSpec>,
        barrel: Option<Entity>,
    ) -> Entity {
        let root = world
            .spawn(TankSim {
                weapons: vec![WeaponState::default(); slots],
                ..default()
            })
            .id();
        world.spawn((
            Muzzle,
            WeaponIndex(slot),
            TankRoot(root),
            weapon(recoil, barrel),
        ));
        root
    }

    /// A `FireEvent` for a named slot on a weapon with BOTH a recoil spec and a barrel node raises
    /// exactly that slot's `recoil_velocity` by the LOCAL spec's `kick`, leaving other slots at rest.
    #[test]
    fn kick_lands_on_named_slot() {
        let mut world = World::new();
        let kick = 3.5;
        // A real barrel node — `kick_recoil` gates on its presence (`Some(_)`).
        let barrel = world.spawn_empty().id();
        let root = spawn_rig(&mut world, 2, 1, Some(recoil(kick)), Some(barrel));
        world.insert_resource(PendingRecoilKicks(vec![(root, 1)]));

        world.run_system_once(apply_pending_recoil_kicks).unwrap();

        let sim = world.get::<TankSim>(root).unwrap();
        assert_eq!(sim.weapons[1].recoil_velocity, kick, "named slot kicks");
        assert_eq!(
            sim.weapons[0].recoil_velocity, 0.0,
            "an unfired slot stays at rest"
        );
    }

    /// The regression guard for the barrel-gate fix: a weapon with a recoil spec but NO barrel node
    /// kicks NOTHING — `apply_recoil` has no `RecoilParams` to step (built on the barrel node), so a
    /// kick here would accumulate in rollback-tracked `recoil_velocity` and never decay. The gate
    /// lives in the shared `kick_recoil` so this holds identically on the server's `fire` path too.
    #[test]
    fn barrel_less_weapon_is_noop() {
        let mut world = World::new();
        // Recoil spec present, barrel absent — the exact case the old client path wrongly kicked.
        let root = spawn_rig(&mut world, 1, 0, Some(recoil(3.5)), None);
        world.insert_resource(PendingRecoilKicks(vec![(root, 0)]));

        world.run_system_once(apply_pending_recoil_kicks).unwrap();

        let sim = world.get::<TankSim>(root).unwrap();
        assert_eq!(
            sim.weapons[0].recoil_velocity, 0.0,
            "a barrel-less weapon must not kick — the velocity would never decay",
        );
    }

    /// A malformed slot off the wire — out of range, or naming no muzzle on this rig — is a silent
    /// no-op: no panic, no out-of-bounds index, no spurious kick on any slot.
    #[test]
    fn bad_slot_is_noop() {
        let mut world = World::new();
        let barrel = world.spawn_empty().id();
        let root = spawn_rig(&mut world, 1, 0, Some(recoil(3.5)), Some(barrel));
        // Slot 9 exists on neither the muzzle set nor the 1-slot `TankSim`.
        world.insert_resource(PendingRecoilKicks(vec![(root, 9)]));

        world.run_system_once(apply_pending_recoil_kicks).unwrap();

        let sim = world.get::<TankSim>(root).unwrap();
        assert_eq!(
            sim.weapons[0].recoil_velocity, 0.0,
            "a bad slot kicks nothing"
        );
    }

    /// A recoil-less weapon (a coax: `recoil: None`) contributes no kick even with a barrel and a
    /// correctly named slot.
    #[test]
    fn recoilless_weapon_is_noop() {
        let mut world = World::new();
        let barrel = world.spawn_empty().id();
        let root = spawn_rig(&mut world, 1, 0, None, Some(barrel));
        world.insert_resource(PendingRecoilKicks(vec![(root, 0)]));

        world.run_system_once(apply_pending_recoil_kicks).unwrap();

        let sim = world.get::<TankSim>(root).unwrap();
        assert_eq!(
            sim.weapons[0].recoil_velocity, 0.0,
            "no recoil spec, no kick"
        );
    }
}
