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
use bevy::window::PrimaryWindow;
// Not in any prelude: the snapshot-interpolation timeline config (the remote-tank delay knob).
use lightyear::interpolation::timeline::InterpolationConfig;
use lightyear::prediction::correction::CorrectionPolicy;
use lightyear::prelude::client::*;
use lightyear::prelude::input::client::InputSystems;
use lightyear::prelude::input::native::{ActionState, InputMarker, NativeBuffer};
use lightyear::prelude::{Controlled as NetControlled, *};

use super::protocol::{FireBurst, NetTank, PROTOCOL_FINGERPRINT};
use super::{client_smoothing_plugin, diagnostics, harness, open_gameplay_gate, physics, rig};
use crate::ballistics::{FireShell, SanctionedBounce, SanctionedShots, SanctionedTerminal};
use crate::command::TankCommand;
use crate::state::{AppState, GameplaySet};
use crate::tank::{
    Controlled as GameControlled, Muzzle, PendingTankAssets, TankRoot, TankSim, Weapon,
    WeaponIndex, load_tank_assets,
};
use crate::ui_font::UiFonts;
use crate::{NetClientPlugin, ShotId, SimPlugin};

const SERVER_PORT: u16 = 5888;

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
    // server's replicated combat snapshot (`net::protocol::NetCrew`) instead of a divergent local
    // kill. This resource also gates the whole `damage::DamageConsequences` chain off here (the
    // client derives those from `NetCrew`). Only the net client sets this; SP / sandbox / server
    // stay authorities.
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
    // Per-fixed-tick sim-cost recorder: idle unless `SPIKE_COST_TRACE` is set (the MG-march cost spike).
    app.add_plugins(crate::cost::client_plugin);
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
            "balanced (<=3-tick input delay absorbs ~50ms before prediction)".to_string(),
        ),
        Some(0) => (
            InputDelayConfig::no_input_delay(),
            "no_input_delay (SPIKE_INPUT_DELAY_TICKS=0 - old max-prediction behavior)".to_string(),
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
    // The single client connection entity — found by the retry driver via `With<NetcodeClient>`
    // (there is exactly one), so its id need not be threaded through.
    app.world_mut().spawn((
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
        // THE REMOTE-TANK TELEPORT FIX (2026-07-11): pin the interpolation delay to a real
        // buffer. Interpolated remotes render at `I = server_estimate − (delay + jitter_margin)`
        // where `delay = max(send_interval·1.7, min_delay)` — and our server replicates every
        // tick, advertising `send_interval = 0` (lightyear's `ReplicationMetadata` default), so
        // the ratio path is dead (acknowledged upstream hole: the `TODO: deal with
        // server_send_interval = 0` in `lightyear_interpolation` timeline.rs; issues #890/#423).
        // Without this insert the required-component default applies and delay collapses to
        // `min_delay = 5 ms`. That is not merely thin — it is NEGATIVE on a real link: the
        // server estimate sits RTT/2 AHEAD of the newest keyframe actually received, so the
        // headroom behind the newest keyframe is `RTT/2 − (min_delay + 1 tick + 4·jitter)` —
        // past zero whenever RTT ≳ 41 ms. The interpolation clock then chronically overruns the
        // buffer, and lightyear CLAMPS (holds newest, no extrapolation) — remote hulls freeze
        // and step keyframe-to-keyframe: the "teleports along the driving path" jitter, immune
        // to `SPIKE_LATENCY_MS=0` because real WAN RTT alone crosses the bar.
        //
        // 100 ms ≈ RTT/2 + 4–5 keyframes of headroom for droplet-range links (RTT ≲ 100 ms),
        // sized by `min_delay ≥ RTT/2 + (N−1)·15.625 ms − 4·jitter`. Cost: interpolated remotes
        // render a further ~100 ms in the past — the honest model for a non-predicted opponent
        // (see `FireEvent::fire_tick`'s timeline taxonomy; shells deliberately do NOT live on
        // `I`). Own-tank prediction is untouched, as is the per-tick confirmed stream feeding
        // rollback — which is exactly why the fix is HERE and not a server send-interval: the
        // lightyear examples' `ReplicationMetadata::new(100ms)` pattern would throttle the whole
        // sender, starving prediction reconciliation to 10 Hz. Tune down toward ~60 ms if
        // remote-lead feel demands, no lower (back into clamp territory at droplet RTT). The
        // eventual robust form is RTT-adaptive delay (upstream filing candidate — parked, see
        // `.agents/scratch/wave-a-adoption-memo.md` §interp-delay); `tests/net_interp_delay.rs`
        // tripwires the upstream degenerate this insert compensates for.
        InterpolationConfig {
            min_delay: Duration::from_millis(100),
            ..Default::default()
        },
        NetcodeClient::new(
            Authentication::Manual {
                server_addr,
                client_id,
                private_key: [0; 32],
                // The protocol-compatibility guard: bake this build's `PROTOCOL_FINGERPRINT` into the
                // connect token's `protocol_id`. netcode folds it into the token AEAD, so a server
                // built from a different revision (mismatched fingerprint) cannot decrypt the token
                // and drops the request — the connection is refused at the handshake, BEFORE
                // replication starts, instead of "succeeding" and then spamming replicon
                // apply-failures forever (the 2026-07-11 skew incident). Must match the server's
                // `NetcodeConfig.protocol_id` (`net::server`).
                protocol_id: PROTOCOL_FINGERPRINT,
            },
            NetcodeConfig {
                client_timeout_secs: 3,
                token_expire_secs: -1,
                ..default()
            },
        )
        .expect("manual dev token should always build"),
        UdpIo::default(),
    ));
    // The connection state machine: gate the FIRST connect on the tank assets, then auto-retry a
    // failed/dropped connection on a short backoff. See [`drive_connection`] for the full states;
    // the target endpoint is fixed for the session, so record it once for the retry driver's log.
    info!("client: target server {server_addr}, client_id={client_id}");
    app.init_resource::<ConnectRetry>()
        .add_systems(Update, drive_connection);
    app.add_systems(Startup, load_tank_assets);
    // The connect-status overlay ("CONNECTING…" / "RECONNECTING…") is windowed presentation only —
    // headless automation has no window to draw it, and verifies the state machine via the log
    // lines `drive_connection` emits instead. Mounted on the same condition as `NetClientPlugin`.
    if !simulate || sim_windowed {
        app.add_systems(Startup, spawn_connect_status)
            // Declares `Overlay::ConnectStatus` presence into the overlay authority, so it must run in
            // the `Declare` phase before the cursor owner derives the input license from the set.
            .add_systems(
                Update,
                update_connect_status.in_set(crate::overlay::OverlaySet::Declare),
            );
    }

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
    // The fire-event dedup set (redundancy overlap) and the server-sanctioned bounce buffer (the
    // ballistics march consumes it; this ager evicts stale entries). Client-only — the buffer's
    // `Option<Res>` read in `integrate_projectiles` is absent everywhere else.
    app.init_resource::<SeenShots>();
    app.init_resource::<SanctionedShots>();
    app.add_systems(
        FixedUpdate,
        // Gated `not(is_in_rollback)` so a replay (which re-runs `FixedMain` N times) does not
        // over-age the buffer; expiry is sim-time coarse, so a real tick is the right cadence.
        age_sanctioned_shots.run_if(not(is_in_rollback)),
    );
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
        app.add_systems(
            FixedPreUpdate,
            // Same rollback gate as `buffer_input`: during replay lightyear restores the historical
            // `ActionState` per tick — overwriting it with the *current* gathered command would
            // corrupt the replay's input.
            feed_action_state
                .in_set(InputSystems::WriteClientInputs)
                .run_if(not(is_in_rollback)),
        );
    }
    // §7 connect-hang guard: must beat `prepare_input_message` to the stranded buffer within the
    // same frame — `.before` the set, not `.after(SyncSystems::Sync)`, because lightyear orders
    // PrepareInputMessage BEFORE Sync in PostUpdate and the strand persists across frames anyway.
    app.add_systems(
        PostUpdate,
        drop_stranded_input_buffer.before(InputSystems::PrepareInputMessage),
    );

    // The Esc menu, the alt-tab focus watcher, and the ONE cursor owner now live in the overlay
    // authority (`crate::overlay::plugin`, mounted by `NetClientPlugin` on same windowed condition):
    // presence declared into `Overlays`, the cursor moved solely by `overlay::cursor_owner` from the
    // `input_blocked` license, and the initial grab handled there too (no more `OnEnter(Playing)`
    // grab). `gate_player_input`'s "captured cursor = license" gate still opens on `grab_mode ==
    // Locked`, which the cursor owner drives. Headless simulate mounts no overlay authority (no window);
    // `feed_action_state` stays out of sim_windowed (the scripted `buffer_input` owns the wire), so the
    // authority's command-idling never fights the script.

    app.run();
}

/// State for the connect/reconnect driver ([`drive_connection`]). Lives across every mode (the
/// headless harness drives it too), so it is init'd unconditionally.
#[derive(Resource, Default)]
struct ConnectRetry {
    /// The first `Connect` has been triggered (tank assets finished loading). Until then the retry
    /// driver stays idle: the client entity carries `Disconnected` from spawn (netcode's `#[require]`
    /// default), which is the pre-connect resting state — NOT a failure to retry.
    initiated: bool,
    /// We reached `Connected` at least once this session. Distinguishes "never connected yet"
    /// (`CONNECTING…`) from "was in game, lost the link" (`RECONNECTING…`) for the status overlay.
    connected_once: bool,
    /// Connect attempts that have already failed and been retried — drives the `(retry N)` suffix
    /// and the backoff schedule. Reset to 0 on a successful `Connected`.
    attempts: u32,
    /// Wall-clock deadline (`Time::elapsed_secs_f64`) for the next retry `Connect`. Armed the frame
    /// we first observe the failed/`Disconnected` state, cleared the frame we fire the retry.
    next_retry_at: Option<f64>,
}

/// Backoff before the next reconnect attempt, in seconds. `attempts` is how many connect attempts
/// have already failed (0 = the first retry, right after the initial attempt fell over). A short,
/// mildly-growing delay capped at [`RECONNECT_BACKOFF_CAP`]: long enough not to hammer a truly-down
/// server, short enough that a server started mid-retry is picked up within a couple seconds. Pure
/// so the schedule is unit-testable without a running app.
fn reconnect_backoff_secs(attempts: u32) -> f64 {
    const BASE: f64 = 1.0;
    const STEP: f64 = 0.5;
    (BASE + STEP * f64::from(attempts)).min(RECONNECT_BACKOFF_CAP)
}

/// The ceiling on [`reconnect_backoff_secs`] — a client that has been retrying for a while still
/// re-checks for a server at least this often. Retries are indefinite (no attempt cap): the primary
/// workflow is "launch the client, then the server", where the wait can exceed any small cap, and a
/// mid-game drop should keep trying to reconnect for as long as the player leaves the window open.
const RECONNECT_BACKOFF_CAP: f64 = 5.0;

/// After this many failed connect attempts the status overlay stops saying only "CONNECTING…" and
/// adds the version-mismatch hint. WHY a count and not a specific "PROTOCOL MISMATCH" line: a
/// fingerprint-skewed peer is refused by netcode dropping the undecryptable connect token silently
/// (see `net::protocol::PROTOCOL_FINGERPRINT`), which the client observes as
/// `ConnectionRequestTimedOut` — byte-for-byte the same terminal state as a down/unreachable server
/// (verified against vendored `lightyear_netcode` client.rs `update_state`). The two are transport-
/// indistinguishable, so the honest message names BOTH causes. Three attempts (~a few seconds of
/// backoff) is enough to rule out a server still starting up before showing it.
const MISMATCH_HINT_AFTER_ATTEMPTS: u32 = 3;

/// The connection state machine, run every frame in every mode. It owns both the FIRST connect
/// (asset-gated) and every retry after a failed or dropped link, reading the connection state off
/// the single client entity's lightyear markers:
///   - **`Connected`** — the link is up. Clear the retry ledger (so a LATER drop starts a fresh
///     backoff) and latch `connected_once` (so that later drop presents as `RECONNECTING…`, not a
///     first connect). Nothing else to do; possession + the game HUD take over.
///   - **not yet initiated** — gate the first `Connect` on the tank assets, exactly as the old
///     asset-gate did: the sim body spawns whole from extracted data the moment the replicated root
///     lands, and preloading keeps view pop-in to ~a frame. (No local ground spawn: `SimPlugin` →
///     `world::plugin` builds the real terrain on both sides; rollback replays collide with it and
///     the suspension rays hit it.)
///   - **`Connecting`** — a connect is in flight (netcode `SendingConnectionRequest`/
///     `ChallengeResponse`); wait it out.
///   - **`Disconnected` after initiating** — the attempt failed (timeout / denied / link drop) and
///     netcode inserted `Disconnected`. Arm a backoff, then fire a fresh `Connect` when it elapses.
///     A fresh `Connect` re-runs `LinkStart`, binding a NEW source socket — which is what sidesteps
///     the server's lingering-slot `ClientEntityInUse` drop after a hard client kill (the client_id
///     stays the session-stable one; netcode does not key the collision on it).
fn drive_connection(
    time: Res<Time>,
    assets: Option<Res<PendingTankAssets>>,
    asset_server: Res<AssetServer>,
    client: Query<
        (
            Entity,
            Has<Connected>,
            Has<Connecting>,
            Option<&Disconnected>,
        ),
        With<NetcodeClient>,
    >,
    mut retry: ResMut<ConnectRetry>,
    mut commands: Commands,
) {
    let Ok((entity, connected, connecting, disconnected)) = client.single() else {
        return;
    };

    if connected {
        retry.connected_once = true;
        retry.attempts = 0;
        retry.next_retry_at = None;
        return;
    }

    if !retry.initiated {
        let Some(assets) = assets else { return };
        if !assets.loaded(&asset_server) {
            return;
        }
        retry.initiated = true;
        commands.trigger(Connect { entity });
        info!("client: tank assets loaded — connecting");
        return;
    }

    // A connect is in flight — leave the timer alone and wait for it to resolve to Connected or
    // fall back to Disconnected.
    if connecting {
        return;
    }

    // Initiated, not connected, not connecting => the attempt failed and netcode inserted
    // `Disconnected`. Retry on a backoff.
    let now = time.elapsed_secs_f64();
    match retry.next_retry_at {
        None => {
            let delay = reconnect_backoff_secs(retry.attempts);
            retry.next_retry_at = Some(now + delay);
        }
        Some(at) if now >= at => {
            retry.attempts += 1;
            retry.next_retry_at = None;
            commands.trigger(Connect { entity });
            // Log netcode's terminal reason for the breadcrumb it gives (`ConnectionRequestTimedOut`
            // vs `ConnectionDenied` etc.). It does NOT distinguish a fingerprint mismatch from an
            // unreachable server — both time out — so the overlay shows the combined hint after
            // `MISMATCH_HINT_AFTER_ATTEMPTS`; this is purely for a dev reading the console.
            let reason = disconnected
                .and_then(|d| d.reason.as_deref())
                .unwrap_or("no reason reported");
            info!(
                "client: reconnect attempt {} ({}) — last netcode state: {reason}",
                retry.attempts,
                if retry.connected_once {
                    "lost in-game connection"
                } else {
                    "never connected"
                }
            );
        }
        Some(_) => {}
    }
}

/// The connect-status message node inside the connect overlay — its text is rewritten from
/// [`ConnectRetry`] by [`update_connect_status`]. The backdrop node itself carries no bespoke marker:
/// it is an `overlay::OverlayNode(ConnectStatus)`, so its z and scrim visibility are the shared
/// authority's job; only this text child needs its own handle for the in-place label rewrite.
#[derive(Component)]
struct ConnectStatusText;

/// Spawn the connect-status overlay once, hidden by default (the first frame is pre-connect anyway;
/// [`update_connect_status`] reveals it). Mirrors the menu/death overlays' node+text shape so the
/// three read as one UI family.
fn spawn_connect_status(mut commands: Commands, fonts: Res<UiFonts>) {
    // The only site to mark the text child too (`ConnectStatusText`), so `update_connect_status` can
    // rewrite the label in place. Spawned with default (visible) `Visibility` — the first frame is
    // pre-connect, and `update_connect_status` drives it from there.
    // The `OverlayNode` marker stamps the one-scrim `GlobalZIndex` (highest of the four, so a
    // connect/reconnect covers everything) via its hook and hands visibility to the shared
    // `overlay::apply_overlay_visibility` reconciler; only the text child keeps a bespoke marker.
    crate::ui_font::spawn_overlay(
        &mut commands,
        &fonts.hud,
        crate::overlay::OverlayNode(crate::overlay::Overlay::ConnectStatus),
        "CONNECTING…",
        ConnectStatusText,
        Some(Color::srgba(0.0, 0.0, 0.0, 0.6)),
    );
}

/// Declare the connect-status overlay's presence from the live connection state and drive its LABEL
/// from [`ConnectRetry`]: `CONNECTING…` / `CONNECTING… (retry N)` before a first connect, the
/// build-mismatch hint after enough first-connect failures, and `RECONNECTING…` after an in-game drop.
/// Presence declaration runs in [`crate::overlay::OverlaySet::Declare`]; the backdrop's z and scrim
/// visibility are the shared `OverlayNode` authority's, so this system no longer touches `Visibility`.
///
/// The label write is guarded on ACTUAL change — rebuilt only when the `(connected_once, attempts)`
/// pair it renders changes. That pair is the label's sole input, so memoizing it in a `Local` skips
/// both the `format!` allocation and the `Text` change-detection churn on the steady-state frames
/// (which are almost all of them — the overlay sits on one message for seconds at a time).
///
/// The mismatch hint is gated on `!connected_once`: only a session that has NEVER connected can be a
/// build mismatch (the fingerprints are transport-indistinguishable from a down server); once we have
/// connected, a later drop is a transient link loss and stays `RECONNECTING…` for as long as the
/// retries run, never accusing a mid-game player's client of being out of date.
fn update_connect_status(
    retry: Res<ConnectRetry>,
    connected: Query<(), (With<Connected>, With<NetcodeClient>)>,
    mut overlays: ResMut<crate::overlay::Overlays>,
    mut text: Query<&mut Text, With<ConnectStatusText>>,
    mut shown: Local<Option<(bool, u32)>>,
) {
    use crate::overlay::Overlay;
    let is_connected = !connected.is_empty();
    // Declare presence into the overlay authority: the connect screen is latched whenever the link is
    // not up. It is the highest-priority overlay, so it will own the scrim whenever present — and the
    // shared `overlay::apply_overlay_visibility` reconciler swaps the backdrop's visibility from that.
    // This system only owns the label text now (its zindex/visibility moved to the `OverlayNode`).
    overlays.declare(Overlay::ConnectStatus, !is_connected);
    if is_connected {
        return;
    }

    // The label is a pure function of `(connected_once, attempts)` — rebuild it (and rewrite the
    // `Text`) only when that pair changes, not every frame. `attempts` is clamped into the memo key at
    // the hint threshold, so once the persistent-failure hint is showing the key stops changing and
    // the `format!`/`Text`-write churn stops too (the label is constant past that point). The clamp
    // is safe for BOTH paths: a never-connected session climbs to the hint and pins there, while a
    // reconnecting session (`connected_once`) never reaches the mismatch branch below, so its key just
    // pins at the clamp with the label constant on "RECONNECTING…".
    let key = (
        retry.connected_once,
        retry.attempts.min(MISMATCH_HINT_AFTER_ATTEMPTS),
    );
    if *shown != Some(key) {
        let label = if retry.connected_once {
            // We connected earlier this session, so the fingerprints PROVABLY matched — a mid-game
            // drop is a transient link loss, never a build mismatch. Keep "RECONNECTING…" indefinitely
            // (retries are uncapped) rather than ever accusing a connected player's client of being
            // out of date.
            "RECONNECTING…".to_string()
        } else if retry.attempts >= MISMATCH_HINT_AFTER_ATTEMPTS {
            // Never connected this session AND enough attempts have failed to rule out a server still
            // starting up. The refusal a fingerprint mismatch produces is indistinguishable from an
            // unreachable server (see `MISMATCH_HINT_AFTER_ATTEMPTS`), so name both causes.
            "CAN'T CONNECT — server down or client/server build mismatch\n\
             (update the client or redeploy the server)"
                .to_string()
        } else if retry.attempts == 0 {
            "CONNECTING…".to_string()
        } else {
            format!("CONNECTING… (retry {})", retry.attempts)
        };
        if let Ok(mut text) = text.single_mut() {
            *text = Text::new(label);
        }
        *shown = Some(key);
    }
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

/// §7 connect-hang guard: drop the own-tank `InputBuffer` whenever it is stranded AHEAD of the
/// timeline, before lightyear's `prepare_input_message` consumes it.
///
/// The wedge this prevents (2026-07-11, caught live under CPU saturation — thread samples in the
/// §7 doc section): the first connect-window `SyncEvent` can re-anchor `LocalTimeline` BACKWARD
/// (the RTT estimate is polluted by scheduler delay on a loaded machine, and `LocalTimeline` has
/// been ticking since app start, so the freshly-computed server objective can land below it).
/// `InputBuffer::set_raw` refuses writes below its `start_tick`, so the buffer stays anchored at
/// the high pre-jump ticks forever — and `NativeStateSequence::build_from_input_buffer`
/// (`lightyear_inputs_native` input_message.rs:94) computes `(end_tick + 1 - buffer_start_tick)
/// as usize` with `Tick - Tick → i32`: a negative difference sign-extends to ~2^64, and its
/// per-iteration `states.push` allocates unboundedly — main loop parked on the wedged worker,
/// RSS balloons, OS SIGKILL at 40–96 s. `tests/net_input_buffer_wrap.rs` pins the two upstream
/// enablers plus the degenerate itself; if that tripwire fires, upstream clamped the range and
/// this guard becomes removable insurance.
///
/// The invariant enforced: the buffer's oldest retained tick must never LEAD `current_tick +
/// input_delay` (the exact `end_tick` `prepare_input_message` will use). In steady state the
/// buffer always trails it (writes happen AT the delayed tick), so this only fires in the
/// already-broken post-backward-resync regime; the cost when it fires is a few ticks of
/// not-yet-sent input the server would have hold-last extrapolated anyway. Rollback replays are
/// no hazard: they restore `ActionState` from history without moving `start_tick` backward, and
/// `prepare_input_message` is `Without<Rollback>` besides. Cheap enough to run unconditionally
/// (one predicted tank, two Tick compares).
fn drop_stranded_input_buffer(
    timeline: Res<LocalTimeline>,
    sender: Query<&InputTimeline, With<Client>>,
    mut buffers: Query<&mut NativeBuffer<TankCommand>, With<InputMarker<TankCommand>>>,
) {
    let Ok(input_timeline) = sender.single() else {
        return;
    };
    let delayed_tick = timeline.tick() + input_timeline.input_delay() as i32;
    for mut buffer in &mut buffers {
        let Some(start) = buffer.start_tick else {
            continue;
        };
        if start - delayed_tick > 0 {
            warn!(
                "client: input buffer stranded ahead of timeline (start {:?} > delayed tick {:?}, \
                 backward connect resync) — dropping it to dodge lightyear's unbounded \
                 input-message loop (§7 guard)",
                start, delayed_tick
            );
            buffer.start_tick = None;
            buffer.buffer.clear();
            buffer.last_remote_tick = None;
        }
    }
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

/// Bounded set of shots already turned into a cosmetic shell, so a redundantly-retransmitted
/// [`FireEvent`](super::protocol::FireEvent) (piece 3 — each burst re-carries the last N) spawns
/// EXACTLY ONE tracer, not N. A
/// simple ring: a `VecDeque` for FIFO eviction + a `HashSet` mirror for O(1) membership. The cap need
/// only exceed the redundancy window (a shot reappears in at most `FIRE_WINDOW`-1 later bursts, ~4
/// shots), so 128 is deep orders of magnitude past any duplicate — an evicted id can never be
/// re-offered. Keyframes need no such set: the `SanctionedShots` insert is idempotent by
/// `(ShotId, sequence)`, so a redundant keyframe is a no-op there.
#[derive(Resource, Default)]
struct SeenShots {
    order: std::collections::VecDeque<ShotId>,
    set: bevy::platform::collections::HashSet<ShotId>,
}

impl SeenShots {
    const CAP: usize = 128;
    /// Record `shot` as seen; returns `true` if it is NEW (was not already present). Evicts the oldest
    /// once over cap.
    fn mark_new(&mut self, shot: ShotId) -> bool {
        if !self.set.insert(shot) {
            return false;
        }
        self.order.push_back(shot);
        if self.order.len() > Self::CAP
            && let Some(old) = self.order.pop_front()
        {
            self.set.remove(&old);
        }
        true
    }
}

/// Drain the server's cosmetic `FireBurst`s and process each embedded event — the CLIENT half of the
/// opponent-fire seam. For each NEW `FireEvent` (deduped by [`ShotId`] against [`SeenShots`], since the
/// sliding window re-carries recent events): re-raise a local `FireShell` (the visible tracer, stamped
/// with its `ShotId` so a bounce keyframe can re-seed it) AND enqueue the shot's recoil CAUSE onto
/// [`PendingRecoilKicks`]. For each `RicochetKeyframe`: store the server-sanctioned bounce in
/// [`SanctionedShots`] (idempotent by `(ShotId, sequence)`). For each `ImpactConfirm`: store the
/// shot's terminal there too (idempotent by `ShotId` — at most one per shot). The ballistics march
/// consumes both — the shot state machine's sanctioned outcomes.
///
/// A remote (interpolated) tank runs no local `fire`, so this is how its shots become visible AND how
/// its barrel kicks and how its ricochets/impacts carry through: the re-raised `FireShell` flies
/// through the same `integrate_projectiles` (damage/hit-gated off under `ClientReplica`, cosmetic BY
/// CONSTRUCTION), re-seeding from a sanctioned bounce at armor contact — or resolving at the
/// sanctioned terminal with the full honest armor read — instead of improvising either.
///
/// `MessageReceiver<FireBurst>` is a required component of the `Client` (the `ServerToClient` direction
/// registered in `net::protocol`), so it rides the client link entity. `shooter: None` on the re-raised
/// `FireShell` (the client never re-broadcasts — only the server owns attribution); a `FireEvent` /
/// `RicochetKeyframe` whose direction fails the `Dir3` guard, or whose tick is absurd, is skipped
/// (no tracer/bounce), the same "reject off the wire" discipline as `fire`'s bore guard.
///
/// **Ignore a FIRE event naming a tank THIS client simulates locally** (one carrying
/// `ActionState<TankCommand>` — exactly the tanks that run `shooting::fire` here and have already flown
/// their shell and kicked their barrel). Bursts are sent to `All` (`broadcast_fire`), so a client
/// always receives its OWN shots' echoes; this guard drops them — no duplicate tracer, no self-kick
/// into `local_rollback::<TankSim>()` state. The guard is on `ActionState`, not
/// `Predicted`/`Controlled`, because it is semantic ("don't touch a tank that fires locally") and
/// survives the predict-everyone change. **KEYFRAMES are deliberately NOT guarded**: a keyframe spawns
/// nothing (no duplicate risk — it re-seeds an existing shell by `ShotId`), and the shooter's own
/// predicted shell is precisely the one that most needs its ricochet carried through (see the loop).
///
/// **Why this stays in `Update` (render rate), NOT the fixed clock.** Verified against vendored
/// `lightyear_messages` 0.28: `MessageReceiver<M>.recv` is a plain `Vec` that `receive()` drains, and
/// the plugin schedules a `clear` system in `Last` every frame (`MessagePlugin`, plugin.rs) that
/// empties any receiver NOT drained that frame — messages are received in `PreUpdate`
/// (`MessageSystems::Receive`) and live for exactly one frame. `RunFixedMainLoop` (hence `FixedUpdate`)
/// runs BEFORE `Update`/`Last` and executes 0..N times per frame, so draining from a fixed schedule
/// would drop every `FireBurst` arriving on a 0-tick frame — common above 64 Hz render, near-total in
/// the headless `2 ms` runner. Draining here, in `Update` (always once per frame, before `Last`),
/// loses none; only the `TankSim` write is deferred to the sim clock. `Update` is also outside every
/// rollback replay (replays run inside `RunFixedMainLoop`), so the drain and the cosmetic-shell spawn
/// can't be re-run by a rollback — preserving today's render-rate shell-spawn timing exactly.
fn receive_fire_events(
    mut receivers: Query<&mut MessageReceiver<FireBurst>>,
    // The set of tanks THIS client simulates locally (own predicted tank; later, under
    // predict-everyone, every predicted tank). They run `shooting::fire` and kick themselves, so a
    // `FireEvent`/`RicochetKeyframe` naming one of them is our own shot echoed back and is ignored.
    locally_fired: Query<(), With<ActionState<TankCommand>>>,
    // The client's PREDICTED present (`P`): the tick this client's OWN tank is simulated at, ahead of
    // the server (see `net::protocol::FireEvent::fire_tick` for why the shell ages to THIS tick and
    // not the confirmed or server-now frame). `LocalTimeline` is always present on a client (mounted by
    // lightyear's `TimelinePlugin`, as `bridge_action_state_to_tank_command` also reads it non-optional).
    timeline: Res<LocalTimeline>,
    mut pending: ResMut<PendingRecoilKicks>,
    mut seen: ResMut<SeenShots>,
    mut sanctioned: ResMut<SanctionedShots>,
    mut commands: Commands,
) {
    let now = timeline.tick();
    for mut receiver in &mut receivers {
        for burst in receiver.receive() {
            for event in &burst.fires {
                // `event.shooter` is already entity-mapped to the local replica. Skip our own shot
                // echoed back, and any duplicate the redundancy window re-carried (dedup by ShotId).
                if locally_fired.contains(event.shooter) {
                    continue;
                }
                let shot = event.shot_id();
                if !seen.mark_new(shot) {
                    continue;
                }
                let Ok(direction) = Dir3::new(event.direction) else {
                    continue; // corrupt bore off the wire — hold the tracer rather than fire NaN
                };
                // How far along its flight the shell already is at OUR predicted present. An absurd /
                // stale / wrapped fire tick rejects the event — no tracer, no recoil.
                let Some(catch_up_ticks) = fire_catch_up_ticks(event.fire_tick, now) else {
                    continue;
                };
                commands.trigger(FireShell {
                    origin: event.origin,
                    direction,
                    speed: event.speed,
                    caliber: event.caliber,
                    mass: event.mass,
                    // The shooter decided this round's tracer-ness from its belt; carry it so this
                    // remote client dresses the shell identically (streak or invisible).
                    tracer: event.tracer,
                    shooter: None,
                    catch_up_ticks,
                    // Stamp the shell with its network identity so a bounce keyframe re-seeds exactly
                    // this shell (`ballistics::Shot`, keyframe-eligible).
                    shot: Some(shot),
                });
                // Capture the CAUSE (which tank's which weapon fired); the fixed-clock applier below
                // derives the spring kick from this client's own local spec. `event.weapon` is
                // bounds-checked at apply time.
                pending.0.push((event.shooter, event.weapon as usize));
            }
            for keyframe in &burst.keyframes {
                // NO `locally_fired` skip here, deliberately — unlike the fires loop above. That guard
                // exists to prevent a duplicate shell SPAWN from our own shot's echo; a keyframe spawns
                // nothing — it re-seeds an existing shell — and the shooter's OWN predicted shell is
                // exactly the one that most needs the bounce carried through (the fall-of-shot read on
                // a ricocheted round, the gunnery loop's core feedback). The own shell carries the SAME
                // `ShotId` this keyframe names (`protocol::stamp_shot_ids` — same root the mapper
                // resolves `shot.shooter` to, same tick-indexed fire tick), so it holds at contact and
                // re-seeds from this bounce like any observer shell. Guard the bore, then store
                // (idempotent by `(ShotId, sequence)`, so the redundancy window's duplicates are no-ops).
                if Dir3::new(keyframe.direction).is_err() {
                    continue;
                }
                sanctioned.insert(
                    keyframe.shot,
                    SanctionedBounce {
                        origin: keyframe.origin,
                        direction: keyframe.direction,
                        speed: keyframe.speed,
                        // `bounce_tick` rides the wire (audit / future RTT-adaptive re-aging) but the
                        // sim re-ages by HOLD duration instead (which equals present − bounce tick), so
                        // it is not carried into the buffer — see `ballistics::Held`.
                        sequence: keyframe.sequence,
                    },
                );
            }
            for confirm in &burst.confirms {
                // Same rule as keyframes: NO `locally_fired` skip — a confirm spawns nothing, it
                // RESOLVES an existing shell (the shooter's own included: the honest armor read on
                // their own hit), so the fires-echo guard has no business here. No `Dir3` guard
                // either: the normal feeds `Impact::normal`, whose consumers normalize with a
                // fallback by contract. Store idempotently (first wins — at most one terminal per
                // shot, so a redundancy-window duplicate is a no-op); the march consumes it, ordered
                // behind the shot's bounces by `after_bounces`. `impact_tick` rides the wire for
                // audit only — the client resolves on receipt.
                sanctioned.insert_terminal(
                    confirm.shot,
                    SanctionedTerminal {
                        position: confirm.position,
                        normal: confirm.normal,
                        penetrated: confirm.penetrated,
                        after_bounces: confirm.after_bounces,
                    },
                );
            }
        }
    }
}

/// Age the observer's server-sanctioned bounce buffer each fixed tick and evict entries past a shell's
/// lifetime — the `net::client` half of the buffer's ring discipline (the ballistics march only READS
/// it). Fixed clock so expiry is measured in sim time, matching the shells that consume it; the buffer
/// lives only on the client, so this is the sole ager.
fn age_sanctioned_shots(mut sanctioned: ResMut<SanctionedShots>, time: Res<Time>) {
    sanctioned.age(time.delta_secs());
}

/// The largest catch-up a `FireEvent` may request. A shot older than the deepest state window we would
/// ever reconcile is stale — its server shell has long since resolved on the authority, so a fresh
/// cosmetic tracer for it is meaningless, and fast-forwarding it would burn that many ballistic steps
/// for nothing. 100 ticks matches `RollbackPolicy`'s default `max_rollback_ticks` (the deepest replay
/// this client runs — see the `PredictionManager` in [`run`]) and is ≈ 1.56 s / ~1.25 km of pre-drag
/// flight at 800 m/s. A value this large is only ever reached by a corrupt/wrapped tick off the wire or
/// a `FireEvent` delayed far past any cosmetic use — either way, skip rather than loop.
const CATCH_UP_MAX_TICKS: u32 = 100;

/// Ticks to fast-forward an opponent shell so it sits at OUR predicted present `P` — co-indexed with
/// the client's own predicted hull (see `net::protocol::FireEvent::fire_tick` for why `P`, not the
/// confirmed or server-now frame). `now` is `LocalTimeline::tick()`. `Some(n)` fast-forwards `n` ticks
/// (`n == 0` = spawn at the muzzle and fly normally); `None` REJECTS the shot as absurd (the caller
/// skips the tracer AND the recoil).
///
/// Wrap-safe by construction: `Tick` is a wrapping `u32` (`lightyear_core::tick`, via `wrapping_id!`)
/// and implements `Sub<Tick>` returning the difference as an `i32` — lightyear's OWN tick difference
/// (`(now as i64 − fire as i64) as i32`, bit-identical to its `wrapping_diff` helper and correct across
/// the `u32::MAX` boundary), not a naive `u32` subtraction that would underflow. (A `u32` tick never
/// actually wraps in a session — ~777 days at 64 Hz — but the arithmetic is correct at the boundary
/// regardless, which is what the wraparound test pins.)
///   - elapsed < 0: the fire tick is AHEAD of our predicted present. The server fires at a tick ≤ its
///     own now, and `P` runs ahead of the server, so this only happens on clock skew or a malicious /
///     wrapped tick — don't rewind; spawn at the muzzle (`Some(0)`).
///   - 0 ≤ elapsed ≤ [`CATCH_UP_MAX_TICKS`]: fast-forward that many ticks (the normal case is ~10).
///   - elapsed > [`CATCH_UP_MAX_TICKS`]: absurd / stale / wrapped nonsense — reject (`None`), no loop.
fn fire_catch_up_ticks(fire: Tick, now: Tick) -> Option<u32> {
    let elapsed = now - fire;
    if elapsed < 0 {
        return Some(0);
    }
    let elapsed = elapsed as u32;
    (elapsed <= CATCH_UP_MAX_TICKS).then_some(elapsed)
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
/// the point. When input is blocked = a default command: the tank coasts to a stop instead of holding
/// the last input, and clicks in the menu don't fire.
fn feed_action_state(
    overlays: Res<crate::overlay::Overlays>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut slots: Query<(&TankCommand, &mut ActionState<TankCommand>), With<InputMarker<TankCommand>>>,
) {
    // The ONE license, `overlay::input_blocked` — the same authority that feeds the cursor owner and
    // (via the released cursor + `state::cursor_locked`) gates `PlayerInputSet`. Blocked (menu / connect
    // screen up, or window unfocused) → send a default command, so the moment we stop reading devices
    // the tank coasts to a stop instead of holding the last input forever. Because the identical
    // derivation also releases the cursor, every system that could latch anything into the command
    // (drive gather, aim commit, respawn request) is already frozen whenever this zeroing is active —
    // no second inference to keep in sync.
    let idle = crate::overlay::input_blocked(&overlays, window.focused);
    for (command, mut state) in &mut slots {
        state.0 = if idle {
            TankCommand::default()
        } else {
            *command
        };
    }
}

/// `apply_pending_recoil_kicks` derives a remote shot's barrel recoil from the LOCAL spec — these
/// exercise that derivation directly against a minimal rig. An external integration test can't reach
/// the sim types (`crate::tank` is a private module — `TankSim`/`Weapon`/`WeaponIndex` are not
/// externally nameable), so the honest test lives here in-crate, over real ECS state.
#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::spec::{FireMode, RecoilSpec, Trigger};
    use crate::tank::WeaponState;

    /// A one-weapon `Weapon` config with the given `recoil` spec and `barrel` node — the only two
    /// fields `kick_recoil`'s gate reads; the rest are filled to satisfy the struct.
    fn weapon(recoil: Option<RecoilSpec>, barrel: Option<Entity>) -> Weapon {
        Weapon {
            name: "MainGun".into(),
            speed: 800.0,
            caliber: 0.088,
            mass: 10.2,
            fire_mode: FireMode::Single { reload_secs: 8.0 },
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

    /// A shot fired ON our predicted present needs no catch-up: spawn at the muzzle, fly normally.
    #[test]
    fn fire_tick_equal_to_now_is_zero_catch_up() {
        assert_eq!(fire_catch_up_ticks(Tick(500), Tick(500)), Some(0));
    }

    /// A fire tick AHEAD of our predicted present (only reachable via clock skew / a malicious or
    /// wrapped tick, since the server fires at a tick <= its now and `P` leads the server) clamps to
    /// 0, never rewinds the shell.
    #[test]
    fn future_fire_tick_clamps_to_zero() {
        assert_eq!(fire_catch_up_ticks(Tick(503), Tick(500)), Some(0));
    }

    /// A shot fired a few ticks before our predicted present fast-forwards by exactly that many ticks.
    #[test]
    fn elapsed_within_bound_fast_forwards() {
        assert_eq!(fire_catch_up_ticks(Tick(500), Tick(505)), Some(5));
        assert_eq!(
            fire_catch_up_ticks(Tick(500), Tick(500 + CATCH_UP_MAX_TICKS)),
            Some(CATCH_UP_MAX_TICKS),
            "exactly at the bound still fast-forwards",
        );
    }

    /// A fire tick far in the past — a stale/lost `FireEvent`, or corrupt/wrapped nonsense off the wire
    /// — is REJECTED (no tracer, no loop over 10^6 steps), the same reject-off-the-wire discipline as
    /// the bore guard.
    #[test]
    fn far_past_fire_tick_is_rejected() {
        assert_eq!(
            fire_catch_up_ticks(Tick(500), Tick(500 + CATCH_UP_MAX_TICKS + 1)),
            None
        );
        assert_eq!(fire_catch_up_ticks(Tick(0), Tick(1_000_000)), None);
    }

    /// Tick arithmetic WRAPS: a fire tick just below `u32::MAX` with a predicted-present tick a few
    /// ticks past the wrap yields the small true elapsed (6 here), NOT a ~4-billion-tick nonsense that
    /// would be rejected or loop. `Tick`'s `Sub` (lightyear's own wrap-correct difference) makes it hold.
    #[test]
    fn wraparound_near_max_behaves() {
        // MAX-2 → MAX-1 → MAX → 0 → 1 → 2 → 3 is 6 ticks across the wrap boundary.
        assert_eq!(fire_catch_up_ticks(Tick(u32::MAX - 2), Tick(3)), Some(6));
    }

    fn shot(tick: u32) -> ShotId {
        ShotId {
            shooter: Entity::PLACEHOLDER,
            weapon: 0,
            fire_tick: tick,
        }
    }

    /// The `ShotId` dedup that makes redundant delivery spawn EXACTLY ONE cosmetic shell:
    /// `mark_new` returns `true` the first time it sees a shot and `false` on every echo — which is the
    /// gate `receive_fire_events` puts in front of the `FireShell` trigger.
    #[test]
    fn seen_shots_dedups_redundant_events() {
        let mut seen = SeenShots::default();
        assert!(seen.mark_new(shot(5)), "first sighting is new — spawn once");
        assert!(
            !seen.mark_new(shot(5)),
            "an echo is not new — no second shell"
        );
        assert!(
            !seen.mark_new(shot(5)),
            "still not new after further echoes"
        );
        assert!(seen.mark_new(shot(6)), "a different shot is new");
        // Distinct on every field of the id.
        let other_weapon = ShotId {
            weapon: 1,
            ..shot(5)
        };
        assert!(
            seen.mark_new(other_weapon),
            "same tank+tick, other weapon = distinct shot"
        );
    }

    /// The sliding-window REDUNDANCY property: with a window of N, if each burst is lost at most once
    /// but the window still covers every event, every shot spawns EXACTLY ONCE — none dropped, none
    /// doubled. Models bursts carrying the last N fire ticks, drops a subset of bursts, and asserts the
    /// dedup over the SURVIVORS yields one spawn per unique shot.
    #[test]
    fn redundancy_window_delivers_every_shot_exactly_once_under_loss() {
        const WINDOW: usize = 4; // matches `net::server::FIRE_WINDOW`
        const SHOTS: u32 = 8;
        // Drop bursts 2, 4, 6 — never two adjacent, so within a window of 4 every event still rides a
        // delivered burst (shot t appears in bursts t..=t+WINDOW-1).
        let dropped = [2u32, 4, 6];

        let mut seen = SeenShots::default();
        let mut spawned: bevy::platform::collections::HashSet<u32> = Default::default();
        let mut doubled = false;
        for k in 1..=SHOTS {
            if dropped.contains(&k) {
                continue; // this burst was lost
            }
            let lo = k.saturating_sub(WINDOW as u32 - 1).max(1);
            for tick in lo..=k {
                if seen.mark_new(shot(tick)) && !spawned.insert(tick) {
                    doubled = true;
                }
            }
        }

        assert!(
            !doubled,
            "no shot spawned twice despite the redundancy overlap"
        );
        assert_eq!(
            spawned.len() as u32,
            SHOTS,
            "every shot spawned despite lost bursts — the window repaired the loss",
        );
        for t in 1..=SHOTS {
            assert!(spawned.contains(&t), "shot {t} spawned exactly once");
        }
    }

    /// The dedup ring is BOUNDED: past its cap the oldest ids are evicted. (An evicted id can never be
    /// re-offered — a shot only reappears within `FIRE_WINDOW` bursts, far inside the cap — so this
    /// only bounds memory, it never re-admits a duplicate in practice.)
    #[test]
    fn seen_shots_ring_is_bounded() {
        let mut seen = SeenShots::default();
        for t in 0..(SeenShots::CAP as u32 + 10) {
            assert!(seen.mark_new(shot(t)), "each fresh tick is new");
        }
        assert!(
            seen.order.len() <= SeenShots::CAP,
            "the ring never exceeds its cap (got {})",
            seen.order.len(),
        );
        // The oldest evicted ids are gone (so re-offering one reads as new again — acceptable, it can't
        // happen inside the redundancy window); the newest are still remembered.
        assert!(
            !seen.mark_new(shot(SeenShots::CAP as u32 + 5)),
            "a recent id is still deduped"
        );
    }

    /// The first retry waits the base delay; subsequent retries grow by a fixed step. Short enough
    /// that a server started mid-retry is picked up within a couple seconds.
    #[test]
    fn reconnect_backoff_grows_from_base() {
        assert_eq!(reconnect_backoff_secs(0), 1.0, "first retry = base");
        assert_eq!(reconnect_backoff_secs(1), 1.5);
        assert_eq!(reconnect_backoff_secs(2), 2.0);
    }

    /// The backoff never exceeds the cap, no matter how many attempts have piled up — an
    /// indefinitely-retrying client still re-checks for a server at least that often.
    #[test]
    fn reconnect_backoff_is_capped() {
        assert_eq!(reconnect_backoff_secs(100), RECONNECT_BACKOFF_CAP);
        assert_eq!(reconnect_backoff_secs(u32::MAX), RECONNECT_BACKOFF_CAP);
        // Monotonic non-decreasing up to the cap.
        for n in 0..20 {
            assert!(reconnect_backoff_secs(n) <= reconnect_backoff_secs(n + 1));
            assert!(reconnect_backoff_secs(n) <= RECONNECT_BACKOFF_CAP);
        }
    }
}
