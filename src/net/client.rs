//! Network-client composition root.
//!
//! Every hull — the own tank included — renders from the interpolated server stream; client
//! presentation never pauses the simulation while the authority continues ticking.

use core::time::Duration;
use std::hash::{BuildHasher, Hasher};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};

use bevy::app::ScheduleRunnerPlugin;
use bevy::asset::AssetPlugin;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowMode};
use lightyear::interpolation::timeline::InterpolationTimeline;
use lightyear::prelude::client::*;
use lightyear::prelude::input::client::InputSystems;
use lightyear::prelude::input::native::{ActionState, InputMarker, NativeBuffer};
use lightyear::prelude::{Controlled as NetControlled, *};
use serde_json::json;

use super::cursor_queue::CursorQueue;
use super::protocol::{
    DamageConfirm, DamageReceipt, FireEvent, FireVisualBatch, FireVisualFact, ImpactConfirm,
    NetTank, RicochetKeyframe, protocol_id,
};
use super::{diagnostics, harness, open_gameplay_gate, physics, rig};
use crate::ballistics::{
    MAX_COSMETIC_CATCH_UP_TICKS, SanctionedBounce, SanctionedBounceInsert, SanctionedShots,
    SanctionedTerminal, ShotSource, TerminalVerdict,
};
use crate::command::TankCommand;
use crate::state::{AppState, GameplaySet};
use crate::tank::{
    Controlled as GameControlled, Muzzle, PendingTankAssets, TankRoot, TankSim, Weapon,
    WeaponIndex, load_tank_assets,
};
use crate::ui_font::UiFonts;
use crate::{CombatantId, NetClientPlugin, ShotId, SimPlugin};

const SERVER_PORT: u16 = 5888;
const LOCAL_SERVER: &str = "127.0.0.1:5888";
/// The matchmaking Worker's base URL baked in at build time (the release workflow sets
/// `OVERMATCH_DEFAULT_API` and refuses to ship a client without it); when set, a client with
/// neither `--local` nor `OVERMATCH_SERVER` starts in the Join menu instead of connecting to
/// [`LOCAL_SERVER`].
const DEFAULT_API: Option<&str> = option_env!("OVERMATCH_DEFAULT_API");

/// Where the Worker base URL came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApiSource {
    Environment,
    BakedDefault,
}

impl ApiSource {
    const fn label(self) -> &'static str {
        match self {
            Self::Environment => "OVERMATCH_API",
            Self::BakedDefault => "baked default API",
        }
    }
}

/// How this client finds its server: a fixed address, or the Join menu against a Worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientMode<'a> {
    Direct(ServerSelection<'a>),
    Matchmaking { api: &'a str, source: ApiSource },
}

/// `--local` and `OVERMATCH_SERVER` select their direct paths whatever else is set; otherwise a
/// non-empty Worker URL — `OVERMATCH_API`, then the baked one — selects matchmaking; otherwise the
/// local server.
fn choose_mode<'a>(
    local: bool,
    server: Option<&'a str>,
    api: Option<&'a str>,
    baked_api: Option<&'a str>,
) -> ClientMode<'a> {
    let direct = ClientMode::Direct(choose_server(local, server));
    if local || server.is_some() {
        return direct;
    }
    let present = |url: Option<&'a str>| url.map(str::trim).filter(|url| !url.is_empty());
    if let Some(api) = present(api) {
        ClientMode::Matchmaking {
            api,
            source: ApiSource::Environment,
        }
    } else if let Some(api) = present(baked_api) {
        ClientMode::Matchmaking {
            api,
            source: ApiSource::BakedDefault,
        }
    } else {
        direct
    }
}

/// Everything the client link entity is built from except its server and key, fixed at startup so
/// the link can be spawned then (direct mode) or when a match is ready (matchmaking mode).
#[derive(Resource, Clone)]
struct ClientLinkTemplate {
    /// See the client-id note in [`run`].
    client_id: u64,
    conditioner: Option<LinkConditionerConfig>,
    seeded: Option<SeededLink>,
}

#[derive(Clone, Copy)]
struct SeededLink {
    seed: u64,
    latency_ms: u64,
    jitter_ms: u64,
    /// `(burst, period)`.
    burst: Option<(Duration, Duration)>,
}

impl ClientLinkTemplate {
    /// The single client connection entity's components — found by the retry driver via
    /// `With<NetcodeClient>` (there is at most one), so its id need not be threaded through.
    fn bundle(&self, server_addr: SocketAddr, private_key: [u8; 32]) -> impl Bundle {
        (
            Name::new("Client"),
            Client,
            Link::default()
                .with_conditioner(self.conditioner.clone().map(RecvLinkConditioner::new)),
            LocalAddr(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0)),
            PeerAddr(server_addr),
            // Lightyear's client replication systems all filter on this marker: without it the
            // link connects and nothing replicates.
            ReplicationReceiver,
            NetcodeClient::new(
                Authentication::Manual {
                    server_addr,
                    client_id: self.client_id,
                    private_key,
                    // Must match the server's `NetcodeConfig.protocol_id`.
                    protocol_id: protocol_id(),
                },
                NetcodeConfig {
                    client_timeout_secs: 3,
                    token_expire_secs: -1,
                    ..default()
                },
            )
            .expect("a manual token always builds"),
            UdpIo::default(),
        )
    }

    /// The seeded receive conditioner, inserted beside [`Self::bundle`] when configured. Its burst
    /// epoch is the spawn instant; the phase needs no cross-endpoint agreement, only a fixed
    /// anchor, because the signature is periodic.
    fn seeded(&self) -> Option<harness::SeededRecvConditioner> {
        self.seeded.map(|link| {
            let mut seeded =
                harness::SeededRecvConditioner::new(link.seed, link.latency_ms, link.jitter_ms);
            if let Some((burst, period)) = link.burst {
                seeded = seeded.with_burst(burst, period, lightyear::core::time::Instant::now());
            }
            seeded
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerSource {
    LocalFlag,
    Environment,
    LocalDefault,
}

impl ServerSource {
    const fn label(self) -> &'static str {
        match self {
            Self::LocalFlag => "--local",
            Self::Environment => "OVERMATCH_SERVER",
            Self::LocalDefault => "local default",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ServerSelection<'a> {
    raw: &'a str,
    source: ServerSource,
}

fn choose_server(local: bool, environment: Option<&str>) -> ServerSelection<'_> {
    if local {
        ServerSelection {
            raw: LOCAL_SERVER,
            source: ServerSource::LocalFlag,
        }
    } else if let Some(raw) = environment {
        ServerSelection {
            raw,
            source: ServerSource::Environment,
        }
    } else {
        ServerSelection {
            raw: LOCAL_SERVER,
            source: ServerSource::LocalDefault,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ServerEndpoint {
    Socket(SocketAddr),
    Host { name: String, port: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecvConditionerMode {
    Off,
    Stock,
    Seeded(u64),
}

/// Preserve the existing receive path unless all seeded-jitter prerequisites are explicit — or a
/// burst is configured, which is deterministic by construction (epoch-phase, no RNG) and therefore
/// activates the seeded conditioner on its own with every unset knob at zero.
fn recv_conditioner_mode(
    latency_ms: u64,
    jitter_ms: u64,
    jitter_seed: Option<u64>,
    burst: bool,
) -> RecvConditionerMode {
    if burst {
        RecvConditionerMode::Seeded(jitter_seed.unwrap_or(0))
    } else if latency_ms == 0 {
        RecvConditionerMode::Off
    } else if jitter_ms > 0
        && let Some(seed) = jitter_seed
    {
        RecvConditionerMode::Seeded(seed)
    } else {
        RecvConditionerMode::Stock
    }
}

/// Input delay is fixed: Lightyear input-buffer end ticks must advance exactly once per local tick.
/// See `tests/net_fire_release.rs`.
pub(crate) const SHIPPING_INPUT_DELAY_TICKS: u16 = 3;

/// Return an RTT-independent fixed input delay.
pub(crate) fn shipping_input_delay() -> InputDelayConfig {
    InputDelayConfig::fixed_input_delay(SHIPPING_INPUT_DELAY_TICKS)
}

pub fn run() {
    let simulate = std::env::args().any(|a| a == "--simulate-input")
        || harness::env_flag("SPIKE_SIMULATE_INPUT", false);
    let sim_windowed = simulate && harness::env_flag("SPIKE_SIM_WINDOWED", false);
    // The capture scripts are the only users of `SPIKE_SIM_WINDOWED`, and what they want is the
    // RenderApp (KTX2 transcode needs a real GPU device), not a window — so the window defaults to
    // hidden and stops stealing focus from the operator. `SPIKE_SIM_VISIBLE=1` restores the
    // eyes-on diagnostic.
    // A documentation capture (`dev_capture`, `OVERMATCH_CAPTURE`) is hidden for the same reason.
    let capture = !simulate && std::env::var_os("OVERMATCH_CAPTURE").is_some();
    let hidden = (sim_windowed && !harness::env_flag("SPIKE_SIM_VISIBLE", false)) || capture;

    let mut app = App::new();
    if simulate && !sim_windowed {
        app.add_plugins(crate::gpu_less_default_plugins(None).set(AssetPlugin {
            file_path: asset_root(),
            ..default()
        }))
        .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_millis(2)))
        .init_resource::<harness::SimulateInput>();
        // UPSTREAM WORKAROUND, same as every other GPU-less boot of the tank glb: without it,
        // bevy_image 0.19 PANICS transcoding the Tiger's mipped UASTC KTX2 textures under
        // `CompressedImageFormats::NONE`, and `tank::spawn` then aborts on the required view
        // artifact — MEASURED here 2026-08-16 against a packaged client. Canonical mechanism
        // comment + retirement tripwire: the identical insertion in `headless_test`.
        app.insert_resource(bevy::image::CompressedImageFormatSupport(
            bevy::image::CompressedImageFormats::ASTC_LDR,
        ));
    } else {
        // Read the player's settings BEFORE the window is described — see `load_at_boot`, which
        // also inserts the values and the boot report into the app.
        let settings = crate::settings::load_at_boot(&mut app);
        // Exe-relative asset root (see `asset_root`): a double-clicked `overmatch`/`overmatch.exe`
        // (or a macOS `.app`) finds `assets/` beside it no matter the launch cwd.
        app.add_plugins(
            DefaultPlugins
                // On macOS this drops the GPU-query wgpu features whose timestamp query set Metal
                // refuses to allocate (tracy builds DeviceLost-quit without it); see the helper's
                // doc for the full causal chain and the vendored citations.
                .set(crate::client_render_plugin())
                .set(AssetPlugin {
                    file_path: asset_root(),
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        // `Unprobed` on purpose: the capability probe needs a surface, which needs
                        // this window — the mapping self-negotiates until the probe lands.
                        present_mode: settings.present_mode(crate::settings::PresentCaps::Unprobed),
                        // Described at creation so a persisted fullscreen boots fullscreen instead
                        // of flashing a window first. The monitor is `at_window_creation`'s and NOT
                        // the stored rung: naming an indexed display here would be cached
                        // unresolved by bevy and would make the `Startup` correction
                        // unrepresentable — see `settings::DisplaySelection`. A hidden capture
                        // client must NOT boot into the player's persisted fullscreen — it stays a
                        // plain window.
                        mode: if hidden {
                            WindowMode::Windowed
                        } else {
                            settings
                                .window_mode
                                .to_window_mode(settings.display.at_window_creation())
                        },
                        // macOS invariant: bevy creates every winit window `with_visible(false)`
                        // (for AccessKit) and then calls `set_visible(window.visible)`, which on
                        // macOS is `makeKeyAndOrderFront` — key + front REGARDLESS of `focused`
                        // (vendored bevy_winit winit_windows.rs:71,315-317; winit 0.30
                        // macos/window_delegate.rs:904-909). So `visible` is the only lever that
                        // stops a capture client stealing focus; `focused: false` would be a dead,
                        // misleading field.
                        //
                        // A hidden window's present returns `SurfaceError::Occluded` every frame
                        // (wgpu-hal metal surface.rs:122-157 workaround), so nothing ever
                        // vsync-blocks and the hidden frame loop free-runs — the ACCEPTED cost;
                        // see the measured note on `WinitSettings::continuous()` below.
                        visible: !hidden,
                        ..default()
                    }),
                    ..default()
                }),
        );
        // Never drop below the 64 Hz tick: the default `WinitSettings::game()` throttles an
        // UNFOCUSED window to 60 Hz reactive updates — under tick rate, so an alt-tabbed client
        // drifts behind the server and resyncs on refocus (lightyear #1113's jitter class).
        //
        // `continuous()` holds for HIDDEN capture clients too. `UpdateMode::reactive(1/120)`
        // was tried here (2026-07-31) and the capture degraded badly — the target received 78
        // of ~980 fire_rx events, fire catch-up p50 34 ticks (baseline 8, max 65 vs ~13), fact
        // lag p50 27 (baseline 7) — BUT that measurement is CONFOUNDED: it was the first run
        // after a fresh build, i.e. a COLD Metal shader cache, which was later shown to stall
        // clients all by itself (>10 s pipeline compilation after connect, keepalive death,
        // reconnects burning lanes — clean on the warm rerun). Verdict: reactive is UNPROVEN
        // either way; `continuous()` is kept because it is measured-good on a warm cache (four
        // clean runs, catch-up p50 7-8, lag 7, metric-identical to the visible baseline) and is
        // the smaller delta. The accepted cost is free-running CPU on capture runs (the
        // invisible surface's `SurfaceError::Occluded` skips the vsync block). Anyone re-trying
        // reactive must A/B it on a WARM cache.
        app.insert_resource(bevy::winit::WinitSettings::continuous());
        if sim_windowed {
            app.init_resource::<harness::SimulateInput>();
        }
        if hidden {
            // The RUNTIME half of the hidden-window guard: the creation-time `mode`/`visible`
            // fields above are not enough on their own, because `settings::apply_settings` would
            // re-apply a persisted fullscreen a frame later. The marker is settings-OWNED (this
            // side only inserts it) — see its doc for the layering reason.
            app.insert_resource(crate::settings::CaptureWindowPinned);
            // UNREACHABLE for a normal client: winit's `applicationDidFinishLaunching` makes even
            // an invisible client the ACTIVE macOS app — it forces `ActivationPolicy::Regular`
            // for an unbundled binary and then calls `activateIgnoringOtherApps` unconditionally
            // (vendored winit 0.30 macos/app_state.rs:110-136) — so the capture process takes the
            // menu bar and the keystroke sink once per launch, and the user's typing vanishes
            // into a window that does not exist. bevy 0.19 exposes no macOS `EventLoopBuilder`
            // hook to pre-empt that (`WinitPlugin`'s builder extensions cover
            // x11/wayland/windows/android only), so the revocation must happen POST-launch:
            // `Startup` runs after winit's didFinishLaunching, so it both revokes the policy and
            // hands focus straight back.
            #[cfg(target_os = "macos")]
            app.add_systems(Startup, crate::platform::revoke_macos_activation);
        }
    }

    // Register the shared protocol before creating the client link. The step lightyear publishes as
    // `TickDuration`; `net::interp_delay` derives its gap term from the same binding.
    let tick_duration = Duration::from_secs_f64(1.0 / 64.0);
    app.add_plugins(ClientPlugins { tick_duration });
    app.add_plugins(super::plugin);
    app.add_plugins(physics::physics_plugins());
    // The own-fire presentation ledger (client only): the arriving gate snapshot is a legality
    // report, not permission to draw. This layer holds the gate the local cadence left, reconciles
    // the snapshot forward, and feeds `shooting::fire` the consumables this client authored rather
    // than the attested echo.
    app.add_plugins(super::fire_presentation::plugin);
    // The real sim — same `SimPlugin` the server mounts: cosmetic shells, servos, and the recoil
    // microsim run against the actual driving/aim/shooting systems, not a stub.
    app.add_plugins(SimPlugin);
    // Server-authoritative combat: replicas fly shells cosmetically but never deposit HP or apply
    // hit impulse. Private crew detail and public life state arrive independently from the server;
    // the client never runs `DamageConsequences` to decide either one.
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
    if capture {
        // Documentation frames to disk (the Join menu, a connect screen); exits once they are.
        app.add_plugins(crate::dev_capture::plugin);
    }
    // Passive jitter-trace recorder: frame + tick rows with link/interpolation extras. Idle unless
    // `SPIKE_TRACE` is set.
    app.add_plugins(crate::trace::client_plugin);
    // Per-fixed-tick sim-cost recorder: idle unless `SPIKE_COST_TRACE` is set (the MG-march cost spike).
    app.add_plugins(crate::cost::client_plugin);
    // Per-render-pass cost recorder: idle unless `SPIKE_RENDER_COST` is set. Harmless in GPU-less
    // simulate mode (no render app — the diagnostics it samples simply never appear).
    app.add_plugins(crate::render_cost::client_plugin);
    // Per-frame wall-clock recorder: idle unless `SPIKE_FRAME_COST` is set (the frame-budget sweep).
    app.add_plugins(crate::frame_cost::client_plugin);
    // Shot-lifecycle recorder: public-shot arrivals, owner-private receipt/marker boundaries, and the
    // cosmetic shell lifecycle, all keyed by stable `ShotId`. Idle unless `SPIKE_SHOT_TRACE` is set.
    app.add_plugins(crate::shot_trace::client_plugin);
    // (FPS + frame-time diagnostics used to be registered here. They now belong to their SOLE
    // consumer, `net::debug_hud::plugin`, which `NetClientPlugin` mounts.)

    // A direct target is resolved once at startup. Raw socket addresses bypass DNS; hostnames
    // select their resolver's first IPv4 result because the UDP transport binds an IPv4 wildcard
    // below. The scripted harness has no menu to click, so it always connects directly.
    let local = std::env::args().any(|arg| arg == "--local");
    let environment = std::env::var("OVERMATCH_SERVER").ok();
    let api = std::env::var("OVERMATCH_API").ok();
    let mode = if simulate {
        ClientMode::Direct(choose_server(local, environment.as_deref()))
    } else {
        choose_mode(local, environment.as_deref(), api.as_deref(), DEFAULT_API)
    };
    let direct_target = match mode {
        ClientMode::Direct(selection) => {
            match parse_server_endpoint(selection.raw).and_then(|endpoint| {
                resolve_server_endpoint(&endpoint).map_err(|error| error.to_string())
            }) {
                Ok(server_addr) => Some((server_addr, selection)),
                Err(error) => {
                    error!(
                        "client: cannot resolve target server {:?} from {}: {error}; exiting",
                        selection.raw,
                        selection.source.label()
                    );
                    return;
                }
            }
        }
        ClientMode::Matchmaking { .. } => None,
    };
    // A per-process RANDOM client id, generated once at startup. NOT the PID (the old
    // `u64::from(std::process::id())`): netcode does NOT enforce client-id uniqueness, so a duplicate
    // id silently OVERWRITES the server's `PeerId → Entity` mapping, and ownership routing resolves by
    // RAW id value — `ControlledBy` and `NetworkingMetadata::peer_map` both key on the value, not on
    // which machine sent it. Two machines that happened to share a PID would therefore collide: the server
    // misroutes `ControlledBy`, and an opponent's tank arrives on the wrong client carrying
    // `Controlled` (turret desync, one client driving both tanks, input contention). A well-distributed random u64 makes a
    // cross-machine collision vanishingly unlikely. `RandomState::new()` is seeded from OS randomness on
    // every call, so a fresh hasher's `finish()` yields a random u64 with NO new dependency. Generated
    // once here and stable for the session; a fresh identity across restarts is fine (back-to-back runs
    // still can't collide inside the server's disconnect timeout — the whole reason the PID existed).
    let client_id = std::hash::RandomState::new().build_hasher().finish();
    // Optional receive conditioning for local diagnostics — OFF by default. A non-zero default
    // silently inflated HUD Ping (and every inbound packet) by ~100 ms on real servers; set
    // `SPIKE_LATENCY_MS` / `SPIKE_JITTER_MS` explicitly when you want a fake path.
    let latency_ms: u64 = harness::env_parse("SPIKE_LATENCY_MS").unwrap_or(0);
    let jitter_ms: u64 = harness::env_parse("SPIKE_JITTER_MS").unwrap_or(0);
    let jitter_seed: Option<u64> = harness::env_parse("SPIKE_JITTER_SEED");
    // The periodic burst (both values set = armed): `60 / 33` reproduces the measured field link.
    let burst_ms: u64 = harness::env_parse("SPIKE_BURST_MS").unwrap_or(0);
    let burst_period_ticks: u64 = harness::env_parse("SPIKE_BURST_PERIOD_TICKS").unwrap_or(0);
    let burst = (burst_ms > 0 && burst_period_ticks > 0).then(|| {
        let ticks = u32::try_from(burst_period_ticks).expect("a burst period fits ticks in u32");
        (Duration::from_millis(burst_ms), tick_duration * ticks)
    });
    let conditioner_mode =
        recv_conditioner_mode(latency_ms, jitter_ms, jitter_seed, burst.is_some());
    let conditioner = match conditioner_mode {
        RecvConditionerMode::Stock => {
            info!(
                "client: RecvLinkConditioner ON — latency={latency_ms}ms jitter={jitter_ms}ms (SPIKE_*)"
            );
            Some(
                LinkConditionerConfig::default()
                    .with_incoming_latency(Duration::from_millis(latency_ms))
                    .with_incoming_jitter(Duration::from_millis(jitter_ms))
                    .with_fixed_loss(0.0),
            )
        }
        RecvConditionerMode::Seeded(seed) => {
            info!(
                "client: SeededRecvConditioner ON — latency={latency_ms}ms jitter={jitter_ms}ms \
                 seed={seed} burst={burst_ms}ms/{burst_period_ticks}t (SPIKE_*)"
            );
            None
        }
        RecvConditionerMode::Off => None,
    };
    let input_delay = shipping_input_delay();
    // The install-time sync margins hold only over a session's pre-arm window; `net::sync_margin`'s
    // derived law rewrites both once the link is measured.
    let sync_config = SyncConfig {
        jitter_multiple: 2,
        jitter_margin: 1.0,
        ..default()
    };
    info!(
        "client: input delay = fixed_input_delay({SHIPPING_INPUT_DELAY_TICKS}) — CONSTANT by \
         design (~{:.0}ms)",
        f64::from(SHIPPING_INPUT_DELAY_TICKS) * 1000.0 / 64.0
    );
    if matches!(conditioner_mode, RecvConditionerMode::Seeded(_)) {
        app.add_systems(
            PreUpdate,
            harness::apply_seeded_recv_conditioner
                .in_set(lightyear::link::LinkReceiveSystems::ApplyConditioner),
        );
    }
    // Mounts the deriving system and resolves `OVERMATCH_INTERP_DELAY_MS`; the returned config is
    // the entity's starting value, rewritten every frame unless the env var pins it.
    let interpolation = super::interp_delay::install(&mut app, tick_duration);
    // Mounts the sync-margin laws on both wire timelines: the input timeline's floor/deadband
    // (rewritten every frame once the own tank's role resolves interpolated, or pinned by
    // `OVERMATCH_INPUT_MARGIN_MS`) and the interpolation timeline's sync margins (static for the
    // session, or pinned by `OVERMATCH_INTERP_MARGIN_MS`). Receives the SAME `input_delay` the
    // timeline below is built with, so its rebuilds cannot change the delay's shape.
    let interpolation = super::sync_margin::install(
        &mut app,
        tick_duration,
        sync_config,
        input_delay,
        interpolation,
    );
    // Buffer-edge starvation instruments and the bounded extrapolation gap-filler.
    super::extrapolate::install(&mut app);
    // Both timeline configs are app-global resources. Inserted AFTER `ClientPlugins`, so
    // lightyear's `On<Insert, InputTimelineConfig>` observer exists and seeds the input delay
    // from `input_delay` at once rather than at the first timeline shift. Inputs are stamped and
    // sent on this timeline regardless of any predicted view.
    app.insert_resource(InputTimelineConfig::new(sync_config, input_delay));
    // Every replica — the own hull included — rides the interpolation buffer behind the server
    // estimate, so it is also the dominant own-input latency term. `net::interp_delay` owns the
    // sizing law and rewrites `min_delay` every frame.
    app.insert_resource(interpolation);
    let template = ClientLinkTemplate {
        client_id,
        conditioner,
        seeded: match conditioner_mode {
            RecvConditionerMode::Seeded(seed) => Some(SeededLink {
                seed,
                latency_ms,
                jitter_ms,
                burst,
            }),
            _ => None,
        },
    };
    // The connection state machine: gate the FIRST connect on the tank assets, then auto-retry a
    // failed/dropped connection on a short backoff. See [`drive_connection`] for the full states.
    app.init_resource::<ConnectRetry>()
        .add_systems(Update, drive_connection);
    match (mode, direct_target) {
        (_, Some((server_addr, selection))) => {
            // A direct link exists from startup, with the all-zero dev key the direct-connect
            // servers hold; its endpoint is fixed for the session, so it is logged once here.
            info!(
                "client: target server {server_addr} (source: {}), client_id={client_id}",
                selection.source.label()
            );
            let mut client_entity = app.world_mut().spawn(template.bundle(server_addr, [0; 32]));
            if let Some(seeded) = template.seeded() {
                client_entity.insert(seeded);
            }
        }
        (ClientMode::Matchmaking { api, source }, None) => {
            // No link until a ticket is ready: the Join menu owns the screen until then.
            info!(
                "client: matchmaking via {api} (source: {}), client_id={client_id}",
                source.label()
            );
            install_matchmaking_link(&mut app, api);
            if !simulate || sim_windowed {
                app.add_plugins(super::matchmaking::leave_ui_plugin);
            }
        }
        (ClientMode::Direct(_), None) => unreachable!("a direct mode resolved its target above"),
    }
    app.insert_resource(template);
    app.add_systems(Startup, load_tank_assets);
    // The connect screens (joining, and "RECONNECTING…" in a match) are windowed presentation only —
    // headless automation has no window to draw it, and verifies the state machine via the log
    // lines `drive_connection` emits instead. Mounted on the same condition as `NetClientPlugin`.
    if !simulate || sim_windowed {
        app.add_plugins(connect_screen_plugin)
            // The screen is derived after this frame's connection state and before any owner
            // declares an overlay from it.
            .add_systems(
                Update,
                derive_flow
                    .after(drive_connection)
                    .before(crate::overlay::OverlaySet::Declare),
            );
    }

    app.add_observer(diagnostics::log_connected)
        .add_observer(reset_shot_receive_state)
        .add_observer(reset_link_state)
        .add_observer(claim_input_slot)
        .add_systems(
            Update,
            (
                rig::attach_replicated_rig,
                receive_fire_events,
                receive_damage_confirms,
                diagnostics::nan_tripwire,
                open_gameplay_gate,
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
    //     across 0/1/2-tick frames).
    app.init_resource::<PendingRecoilKicks>();
    // Client-only shot dedup, deferred fire resolution, private-receipt dedup, and sanctioned
    // outcomes. The ballistics march reads `SanctionedShots` optionally everywhere else.
    app.init_resource::<SeenShots>();
    app.init_resource::<HeldFireEvents>();
    app.init_resource::<PendingFireEvents>();
    app.init_resource::<SeenDamage>();
    app.init_resource::<SanctionedShots>();
    // Expiry is sim-time coarse, so a real tick is the right cadence.
    app.add_systems(FixedUpdate, age_sanctioned_shots);
    // The pen cam's explanations addressed to this player, and whether each impact is drawn.
    super::shot_explanation::install_client(&mut app);
    app.add_systems(
        FixedUpdate,
        apply_pending_recoil_kicks
            .before(GameplaySet)
            // Gate on `Playing` to match its consumer: `shooting::apply_recoil` lives in
            // `GameplaySet` (Playing-only), so the applier that WRITES the kick and the system that
            // SPRINGS it must agree on when they may run — otherwise a `FireEvent` draining outside
            // `Playing` writes a kick into `TankSim` that `apply_recoil` never releases.
            .run_if(in_state(AppState::Playing)),
    );
    // The server's spawn decision, read off the markers that actually arrived. Unconditional: which
    // clock the own hull moves on is the first thing any capture of this session must state.
    app.add_systems(Update, log_local_tank_role);
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
                // `stamp_input_tick` chained AFTER the writer: it stamps whatever command the writer
                // just placed, and must be the last word before lightyear buffers it.
                // `record_own_intent` chains after the stamp: the client's own copy of what it
                // filed is keyed by the tick the stamp names.
                (
                    harness::buffer_input,
                    stamp_input_tick,
                    super::fire_presentation::record_own_intent,
                )
                    .chain()
                    .in_set(InputSystems::WriteClientInputs),
            );
    } else {
        app.add_systems(
            FixedPreUpdate,
            (
                feed_action_state,
                stamp_input_tick,
                super::fire_presentation::record_own_intent,
            )
                .chain()
                .in_set(InputSystems::WriteClientInputs),
        );
    }
    install_input_buffer_guard(&mut app);
    install_input_send_cadence(&mut app);

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
    /// We reached `Connected` at least once on this link. Distinguishes "never connected yet" (the
    /// joining screen) from "was in game, lost the link" (the match with `RECONNECTING…` over it).
    connected_once: bool,
    /// Connect attempts that have already failed and been retried — drives the `(retry N)` suffix
    /// and the backoff schedule. Reset to 0 on a successful `Connected`.
    attempts: u32,
    /// Real-time deadline (`Time<Real>::elapsed_secs_f64`) for the next retry `Connect`. Armed the
    /// frame we first observe the failed/`Disconnected` state, cleared the frame we fire the retry.
    /// Every time in this ledger is real time: virtual time pauses, scales and clamps a long
    /// frame's delta, and the give-up below compares against these.
    next_retry_at: Option<f64>,
    /// When a once-connected link was first seen down; cleared on `Connected`.
    down_since: Option<f64>,
    /// When the last retry `Connect` fired.
    last_attempt_at: Option<f64>,
    /// A retry fired at least [`RECONNECT_GIVE_UP_S`] after `down_since` has failed: the match is
    /// given up. Cleared on `Connected`.
    exhausted: bool,
}

/// Seconds of real time a once-connected matchmaking link keeps reconnecting before the client
/// gives the match up and returns to the lobby: the match ends when a retry fired at least this
/// long after the link went down fails. Measured over attempts, not across a stall — a client
/// whose frames stopped for longer (a suspend, a hitch) still tries once on resume, and a live
/// server takes it back. TUNED: twice the Worker's 15 s heartbeat deadline, after which the
/// Worker has dropped a silent server (`decisions.md`, "Reconnect give-up").
const RECONNECT_GIVE_UP_S: f64 = 30.0;

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
/// re-checks for a server at least this often. In direct mode retries are indefinite (no attempt
/// cap): the primary workflow is "launch the client, then the server", where the wait can exceed
/// any small cap, and a mid-game drop keeps reconnecting while the window is open. Matchmaking mode
/// closes a link that never connects, or a match down for [`RECONNECT_GIVE_UP_S`]
/// ([`close_verdict`]).
const RECONNECT_BACKOFF_CAP: f64 = 5.0;

/// After this many failed connect attempts the status overlay stops saying only "CONNECTING…" and
/// adds the version-mismatch hint. WHY a count and not a specific "PROTOCOL MISMATCH" line: a
/// fingerprint-skewed peer — a different wire manifest OR a different map — is refused by netcode
/// dropping the undecryptable connect token silently (see `net::protocol::protocol_id`), which the
/// client observes as
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
///     `world::plugin` builds the real terrain on both sides; the track force probes sample it.)
///   - **`Connecting`** — a connect is in flight (netcode `SendingConnectionRequest`/
///     `ChallengeResponse`); wait it out.
///   - **`Disconnected` after initiating** — the attempt failed (timeout / denied / link drop) and
///     netcode inserted `Disconnected`. Arm a backoff, then fire a fresh `Connect` when it elapses.
///     A fresh `Connect` re-runs netcode's handshake on the same link: while the entity is still
///     `Linked` (a transient netcode disconnect), lightyear's UDP `LinkStart` skips it, so the retry
///     reuses the source socket. The client_id stays the session-stable one.
fn drive_connection(
    real: Res<Time<Real>>,
    assets: Option<Res<PendingTankAssets>>,
    asset_server: Res<AssetServer>,
    client: Query<
        (
            Entity,
            Has<Connected>,
            Has<Connecting>,
            Option<&Disconnected>,
        ),
        (With<NetcodeClient>, Without<Leaving>),
    >,
    mut retry: ResMut<ConnectRetry>,
    mut commands: Commands,
) {
    let Ok((entity, connected, connecting, disconnected)) = client.single() else {
        return;
    };

    let now = real.elapsed_secs_f64();
    if connected {
        retry.connected_once = true;
        retry.attempts = 0;
        retry.next_retry_at = None;
        retry.down_since = None;
        retry.last_attempt_at = None;
        retry.exhausted = false;
        return;
    }
    if retry.connected_once && retry.down_since.is_none() {
        retry.down_since = Some(now);
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
    // `Disconnected`. A failed retry fired past the give-up exhausts the match; otherwise retry on
    // a backoff.
    if let (Some(down), Some(attempted)) = (retry.down_since, retry.last_attempt_at)
        && attempted - down >= RECONNECT_GIVE_UP_S
    {
        retry.exhausted = true;
    }
    match retry.next_retry_at {
        None => {
            let delay = reconnect_backoff_secs(retry.attempts);
            retry.next_retry_at = Some(now + delay);
        }
        Some(at) if now >= at => {
            retry.attempts += 1;
            retry.next_retry_at = None;
            retry.last_attempt_at = Some(now);
            commands.trigger(Connect { entity });
            // Log netcode's terminal reason for the breadcrumb it gives (`ConnectionRequestTimedOut`
            // vs `ConnectionDenied` etc.). It does NOT distinguish a fingerprint mismatch from an
            // unreachable server — both time out — so the overlay shows the combined hint after
            // `MISMATCH_HINT_AFTER_ATTEMPTS`; this is purely for a dev reading the console.
            let reason = disconnected
                .map_or_else(|| "no reason reported".to_owned(), |d| d.reason.to_string());
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

/// The screen this client is on, from its link (`flow::flow_of`): no link is the Join menu, a link
/// that has never connected is the connect screen, and one that has is the match.
fn derive_flow(
    links: Query<(), With<NetcodeClient>>,
    retry: Res<ConnectRetry>,
    state: Res<State<AppState>>,
    mut next: ResMut<NextState<crate::flow::Flow>>,
) {
    crate::flow::write(
        &mut next,
        crate::flow::FlowFacts::Net {
            link: !links.is_empty(),
            connected_once: retry.connected_once,
            playing: *state.get() != AppState::Loading,
        },
    );
}

/// Matchmaking mode: spawn the client link for a ready match, with that match's key, and start the
/// connection state machine afresh for it. At most one link exists: a ready match arriving while
/// one does is dropped.
fn spawn_matched_link(
    mut ready: MessageReader<super::matchmaking::MatchReady>,
    template: Res<ClientLinkTemplate>,
    links: Query<(), With<NetcodeClient>>,
    mut commands: Commands,
) {
    let Some(super::matchmaking::MatchReady(matched)) = ready.read().last() else {
        return;
    };
    if !links.is_empty() {
        warn!(
            "client: a ready match at {} arrived while a link exists; ignored",
            matched.addr
        );
        return;
    }
    info!(
        "client: target server {} (source: matchmaking), client_id={}",
        matched.addr, template.client_id
    );
    let mut link = commands.spawn(template.bundle(matched.addr, matched.key));
    if let Some(seeded) = template.seeded() {
        link.insert(seeded);
    }
    commands.insert_resource(ConnectRetry::default());
}

/// Matchmaking mode's link systems: spawn the link for a ready match, and close it on every exit
/// from it — never connected, the server gone, or the player leaving. Shared by [`run`] and the
/// hosted end-to-end test.
pub(super) fn install_matchmaking_link(app: &mut App, api: &str) {
    super::matchmaking::install(app, api);
    app.add_systems(
        Update,
        (
            spawn_matched_link,
            start_leaving,
            finish_leaving,
            close_lost_match.after(drive_connection),
        ),
    );
}

/// Marks a link the player is leaving: [`drive_connection`] no longer reconnects it, the reconnect
/// overlay ignores it, and [`finish_leaving`] despawns it on a later frame than its `Disconnect`,
/// so the disconnect packets leave first. `seen` is set the first frame `finish_leaving` sees it.
#[derive(Component, Default)]
struct Leaving {
    seen: bool,
}

/// The player asked to leave: disconnect the link and mark it [`Leaving`].
fn start_leaving(
    mut leave: MessageReader<super::matchmaking::LeaveMatch>,
    links: Query<Entity, (With<NetcodeClient>, Without<Leaving>)>,
    mut commands: Commands,
) {
    if leave.read().last().is_none() {
        return;
    }
    for link in &links {
        info!("client: leaving the match");
        commands.entity(link).insert(Leaving::default());
        commands.trigger(Disconnect { entity: link });
    }
}

/// Despawn a left link one frame after it was marked, and return the lobby to idle.
fn finish_leaving(
    mut links: Query<(Entity, &mut Leaving)>,
    mut retry: ResMut<ConnectRetry>,
    mut closed: MessageWriter<super::matchmaking::MatchClosed>,
    mut commands: Commands,
) {
    for (link, mut leaving) in &mut links {
        if !leaving.seen {
            leaving.seen = true;
            continue;
        }
        commands.entity(link).despawn();
        *retry = ConnectRetry::default();
        closed.write(super::matchmaking::MatchClosed::Left);
    }
}

/// Whether, and how, the match's link is closed this frame: the server reported gone (`gone`), a
/// link that never connected after [`MISMATCH_HINT_AFTER_ATTEMPTS`] attempts, or a once-connected
/// link whose reconnects are exhausted ([`RECONNECT_GIVE_UP_S`]). A link that connected once is the
/// match's: its close is the match's end; one that never connected is a failed join.
fn close_verdict(
    retry: &ConnectRetry,
    gone: Option<String>,
) -> Option<super::matchmaking::MatchClosed> {
    use super::matchmaking::MatchClosed;
    if let Some(reason) = gone {
        return Some(if retry.connected_once {
            MatchClosed::Ended(reason)
        } else {
            MatchClosed::Unreachable(reason)
        });
    }
    if !retry.connected_once {
        return (retry.attempts >= MISMATCH_HINT_AFTER_ATTEMPTS)
            .then(|| MatchClosed::Unreachable("could not reach the match server".into()));
    }
    retry
        .exhausted
        .then(|| MatchClosed::Ended("the match server stopped answering".into()))
}

/// Close the match's link on a [`close_verdict`]: despawn it, start the connection ledger afresh
/// and tell the lobby why.
fn close_lost_match(
    mut retry: ResMut<ConnectRetry>,
    mut gone: MessageReader<super::matchmaking::ServerGone>,
    links: Query<Entity, (With<NetcodeClient>, Without<Leaving>)>,
    mut closed: MessageWriter<super::matchmaking::MatchClosed>,
    mut commands: Commands,
) {
    let gone = gone.read().last().map(|gone| gone.0.clone());
    let Some(verdict) = close_verdict(&retry, gone) else {
        return;
    };
    let Ok(link) = links.single() else { return };
    warn!(
        "client: closing the match link after {} attempts: {verdict:?}",
        retry.attempts
    );
    commands.entity(link).despawn();
    *retry = ConnectRetry::default();
    closed.write(verdict);
}

/// The connect screens. The first connect is the [`Flow::Joining`](crate::flow::Flow) screen,
/// spawned on entering it and labelled from [`ConnectRetry`]; a match's dropped link is
/// `Overlay::ConnectStatus` ("RECONNECTING…"), declared only in a match, in the `Declare` phase,
/// before the cursor owner derives the input license from the set.
pub(super) fn connect_screen_plugin(app: &mut App) {
    use crate::flow::{Flow, InMatchSystems};
    app.init_resource::<ConnectRetry>()
        .add_systems(Startup, spawn_connect_status)
        .add_systems(OnEnter(Flow::Joining), spawn_joining_screen)
        .add_systems(
            Update,
            (
                update_connect_status
                    .in_set(crate::overlay::OverlaySet::Declare)
                    .in_set(InMatchSystems),
                update_joining_screen.run_if(in_state(Flow::Joining)),
            ),
        );
}

/// The joining screen's label.
#[derive(Component)]
struct JoiningText;

/// Spawn the persistent reconnect overlay; [`update_connect_status`] declares its presence, while
/// `overlay::apply_overlay_visibility` owns visibility. The `OverlayNode` hook stamps its z.
fn spawn_connect_status(mut commands: Commands, fonts: Res<UiFonts>) {
    crate::ui_font::spawn_overlay(
        &mut commands,
        &fonts.hud,
        (
            crate::overlay::OverlayNode(crate::overlay::Overlay::ConnectStatus),
            Visibility::Hidden,
        ),
        "RECONNECTING…",
        (),
        Some(Color::srgba(0.0, 0.0, 0.0, 0.6)),
    );
}

/// Spawn the joining screen: the match world is not yet anyone's view, so its scrim is near opaque.
fn spawn_joining_screen(mut commands: Commands, fonts: Res<UiFonts>) {
    crate::ui_font::spawn_overlay(
        &mut commands,
        &fonts.hud,
        crate::flow::FlowScoped(crate::flow::Flow::Joining),
        joining_label(0),
        JoiningText,
        Some(Color::srgba(0.01, 0.02, 0.03, 0.94)),
    );
}

/// The joining screen's label after `attempts` failed connects. Past
/// [`MISMATCH_HINT_AFTER_ATTEMPTS`] it names both causes a refusal can have (see that constant):
/// only a session that has never connected can be a build mismatch, and this screen is only ever
/// shown to one.
fn joining_label(attempts: u32) -> String {
    if attempts >= MISMATCH_HINT_AFTER_ATTEMPTS {
        "CAN'T CONNECT — server down or client/server build mismatch\n\
         (update the client or redeploy the server)"
            .to_string()
    } else if attempts == 0 {
        "CONNECTING…".to_string()
    } else {
        format!("CONNECTING… (retry {attempts})")
    }
}

/// Rewrite the joining label when the attempt count it renders changes.
fn update_joining_screen(retry: Res<ConnectRetry>, mut text: Query<&mut Text, With<JoiningText>>) {
    let label = joining_label(retry.attempts);
    for mut text in &mut text {
        if text.0 != label {
            text.0.clone_from(&label);
        }
    }
}

/// Declare the reconnect overlay while the match's link is down; a link being left declares
/// nothing.
fn update_connect_status(
    links: Query<Has<Connected>, (With<NetcodeClient>, Without<Leaving>)>,
    mut overlays: ResMut<crate::overlay::Overlays>,
) {
    let down = links.iter().any(|connected| !connected);
    overlays.declare(crate::overlay::Overlay::ConnectStatus, down);
}

/// Parse without touching DNS so address shape and source precedence remain unit-testable offline.
fn parse_server_endpoint(raw: &str) -> Result<ServerEndpoint, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("address is empty".to_string());
    }
    if let Ok(address) = raw.parse::<SocketAddr>() {
        return Ok(ServerEndpoint::Socket(address));
    }
    if let Ok(ip) = raw.parse::<IpAddr>() {
        return Ok(ServerEndpoint::Socket(SocketAddr::new(ip, SERVER_PORT)));
    }
    if let Some(ip) = raw
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .and_then(|value| value.parse::<IpAddr>().ok())
    {
        return Ok(ServerEndpoint::Socket(SocketAddr::new(ip, SERVER_PORT)));
    }

    let (name, port) = match raw.rsplit_once(':') {
        Some((name, port)) if !name.contains(':') => {
            let port = port
                .parse::<u16>()
                .map_err(|_| format!("invalid port {port:?}"))?;
            (name, port)
        }
        Some(_) => return Err("invalid socket address or hostname".to_string()),
        None => (raw, SERVER_PORT),
    };
    if name.is_empty() {
        return Err("hostname is empty".to_string());
    }
    Ok(ServerEndpoint::Host {
        name: name.to_string(),
        port,
    })
}

fn resolve_server_endpoint(endpoint: &ServerEndpoint) -> io::Result<SocketAddr> {
    resolve_server_endpoint_with(endpoint, |name, port| (name, port).to_socket_addrs())
}

fn resolve_server_endpoint_with<I>(
    endpoint: &ServerEndpoint,
    resolve_hostname: impl FnOnce(&str, u16) -> io::Result<I>,
) -> io::Result<SocketAddr>
where
    I: IntoIterator<Item = SocketAddr>,
{
    match endpoint {
        ServerEndpoint::Socket(address) => Ok(*address),
        ServerEndpoint::Host { name, port } => resolve_hostname(name, *port)?
            .into_iter()
            .find(SocketAddr::is_ipv4)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    format!("{name}:{port} resolved without an IPv4 address"),
                )
            }),
    }
}

/// `AssetPlugin`'s `file_path` — the `assets/` directory this client reads from, as a `String`
/// (what `AssetPlugin` wants). Delegates to the shared, unit-tested resolver at `crate::assets`;
/// the resolution logic (macOS `.app` → `Contents/Resources/assets`, flat Windows/Linux archive →
/// `exe_dir/assets`, env overrides) lives there so the tank bake (`bake`, which compiles without the
/// `net` feature) resolves the exact same directory. See `crate::assets::asset_root`.
fn asset_root() -> String {
    crate::assets::asset_root().to_string_lossy().into_owned()
}

/// Claim the locally controlled replicated tank for input and presentation.
fn claim_input_slot(add: On<Add, NetControlled>, mut commands: Commands) {
    info!("client: controlled entity {} — input slot", add.entity);
    commands.entity(add.entity).insert((
        InputMarker::<TankCommand>::default(),
        ActionState::<TankCommand>::default(),
        GameControlled,
    ));
}

/// Fixed ticks that have buffered local input, the count at the last staged input message, and
/// whether this frame stages another.
#[derive(Resource, Default)]
struct InputSendCadence {
    buffered_ticks: u64,
    staged_at: Option<u64>,
    due: bool,
}

/// Stage at most one input message per fixed tick. Lightyear's `prepare_input_message` runs once
/// per `PostUpdate` frame and, on a frame that ran no fixed tick, re-stages the same `end_tick` with
/// the same window, so without this gate the upload scales with frame rate rather than tick rate.
///
/// The gate keys on a count of fixed ticks that ran `InputSystems::BufferClientInputs`, never on
/// tick numbers: a `LocalTimelineShift` or an input-delay change cannot alias a freshly buffered
/// tick onto the last staged one. The first frame after each buffering tick stages, so the input
/// lead is unchanged.
///
/// Redundancy is lightyear's, untouched: each message carries the last `packet_redundancy` (5)
/// ticks, so at a steady 64 Hz a tick's command rides the messages of its own tick and the next
/// four — first-to-last copy span 62.5 ms (DERIVED: 4 × 15.625 ms), surviving four consecutive
/// lost messages. A frame that runs several fixed ticks stages one message ending on the latest;
/// past five ticks in one frame the earliest falls outside that window. Both predate this gate.
pub(super) fn install_input_send_cadence(app: &mut App) {
    app.init_resource::<InputSendCadence>()
        .add_systems(
            FixedPreUpdate,
            count_buffered_tick.after(InputSystems::BufferClientInputs),
        )
        .add_systems(
            PostUpdate,
            decide_input_send.before(InputSystems::PrepareInputMessage),
        )
        .configure_sets(
            PostUpdate,
            InputSystems::PrepareInputMessage.run_if(input_send_due),
        );
}

fn count_buffered_tick(mut cadence: ResMut<InputSendCadence>) {
    cadence.buffered_ticks += 1;
}

/// Unsynced, lightyear skips `prepare_input_message` itself (its `SyncedLocalTimeline` parameter),
/// so nothing is recorded as staged.
fn decide_input_send(sync: Res<LocalTimelineSync>, mut cadence: ResMut<InputSendCadence>) {
    let buffered = cadence.buffered_ticks;
    cadence.due = sync.is_synced() && cadence.staged_at != Some(buffered);
    if cadence.due {
        cadence.staged_at = Some(buffered);
    }
}

fn input_send_due(cadence: Res<InputSendCadence>) -> bool {
    cadence.due
}

/// Observable count of buffers cleared before Lightyear's native input encoder.
#[derive(Resource, Default)]
pub(super) struct InputBufferGuardMetrics {
    pub(super) cleared: u64,
}

/// Mount the input-buffer safety boundary for every composition that installs `ClientPlugins` and
/// can attach an [`InputMarker`]. It must precede `PrepareInputMessage` in the same `PostUpdate` and
/// relies on the production invariant of exactly one client connection per [`App`].
pub(super) fn install_input_buffer_guard(app: &mut App) {
    app.init_resource::<InputBufferGuardMetrics>().add_systems(
        PostUpdate,
        drop_stranded_input_buffer.before(InputSystems::PrepareInputMessage),
    );
}

fn clear_stranded_input_buffer(
    buffer: &mut NativeBuffer<TankCommand>,
    delayed_tick: Tick,
) -> Option<Tick> {
    let start = buffer.start_tick?;
    if start - delayed_tick <= 0 {
        return None;
    }
    *buffer = NativeBuffer::default();
    Some(start)
}

/// Remove an input buffer that leads Lightyear's next send tick, preventing an invalid range from
/// reaching the native encoder. Remove this guard only after verifying that the upstream encoder
/// rejects inverted ranges; `tests/net_input_buffer_wrap.rs` pins the safe enablers, not the
/// unbounded encoder call.
fn drop_stranded_input_buffer(
    timeline: Res<LocalTimeline>,
    sync: Res<LocalTimelineSync>,
    sender: Query<Entity, With<Client>>,
    mut buffers: Query<&mut NativeBuffer<TankCommand>, With<InputMarker<TankCommand>>>,
    mut metrics: ResMut<InputBufferGuardMetrics>,
) {
    // There is no encoder work, and therefore no range-wrap risk, before an input buffer exists.
    // Checking this first also leaves startup/teardown alone when no locally controlled tank is
    // present. Once a buffer exists, one client connection is the invariant that makes the global
    // input timeline's delayed encoder tick meaningful.
    if buffers.is_empty() {
        return;
    }
    let client = match sender.single() {
        Ok(client) => client,
        Err(error) => {
            let mut cleared = 0_u64;
            for mut buffer in &mut buffers {
                if buffer.start_tick.is_some()
                    || !buffer.is_empty()
                    || buffer.last_remote_tick.is_some()
                {
                    cleared += 1;
                }
                *buffer = NativeBuffer::default();
            }
            if cleared == 0 {
                return;
            }
            metrics.cleared += cleared;
            error!(
                ?error,
                cleared,
                "client: input-buffer guard cleared marked buffers because it requires exactly one Client"
            );
            debug_assert!(
                false,
                "input-buffer guard requires exactly one Client while a local input buffer exists: {error:?}"
            );
            return;
        }
    };
    let delayed_tick = timeline.tick() + sync.input_delay() as i32;
    for mut buffer in &mut buffers {
        let buffered_states = buffer.len();
        if let Some(start) = clear_stranded_input_buffer(&mut buffer, delayed_tick) {
            metrics.cleared += 1;
            let lead: i32 = start - delayed_tick;
            warn!(
                "client: input-buffer guard clear; MEASURED client={client:?} start={start:?} \
                 delayed_tick={delayed_tick:?} lead={lead} buffered_states={buffered_states}; observed \
                 start ahead of encoder tick, likely after backward connection resync"
            );
        }
    }
}

/// Name the replication role the server gave this client's own tank, the first frame that role is
/// observable: the interpolated server stream, RTT/2 + the interpolation delay behind. Latches on
/// the first resolved role, so a respawn does not re-log a decision that cannot change mid-session.
fn log_local_tank_role(
    tank: Query<Entity, (With<GameControlled>, With<NetTank>, With<Interpolated>)>,
    mut logged: Local<bool>,
) {
    if *logged {
        return;
    }
    let Ok(entity) = tank.single() else {
        return;
    };
    *logged = true;
    info!("client: own tank {entity} drives INTERPOLATED (server stream)");
}

/// Opt-in ownership trace. Only the local tank may carry `Controlled` and `InputMarker`.
#[expect(clippy::type_complexity, reason = "one-off diagnostic marker snapshot")]
fn log_tank_ownership(
    tanks: Query<
        (
            Entity,
            Has<GameControlled>,
            Has<InputMarker<TankCommand>>,
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
    for (entity, controlled, input_marker, interpolated, remote) in &tanks {
        info!(
            "ownership: {entity} controlled={controlled} input_marker={input_marker} interpolated={interpolated} remote={remote}"
        );
    }
}

/// Barrel recoil kicks awaiting the sim clock: `(shooter root, weapon slot)` per opponent shot,
/// captured at render rate by [`receive_fire_events`] and consumed on the fixed clock by
/// [`apply_pending_recoil_kicks`]. The queue is the seam between the render-rate message drain (which
/// must stay render-rate — lightyear clears an undrained receiver every frame in `Last`) and the
/// fixed-clock `TankSim` write (which must be on the sim clock — see [`apply_pending_recoil_kicks`]).
#[derive(Resource, Default)]
pub(super) struct PendingRecoilKicks(Vec<(Entity, usize)>);

impl PendingRecoilKicks {
    /// Enqueue one kick cause. `net::fire_presentation`'s recovered rounds share this seam so a
    /// fallback bang recoils exactly like an echoed one.
    pub(super) fn push(&mut self, shooter: Entity, weapon: usize) {
        self.0.push((shooter, weapon));
    }

    /// Test-only inspection of the queued causes.
    #[cfg(test)]
    pub(super) fn queued(&self) -> &[(Entity, usize)] {
        &self.0
    }
}

/// Bounded [`ShotId`] dedup cache across repeated visual batches and direct reliable facts.
#[derive(Resource, Default)]
pub(super) struct SeenShots {
    order: std::collections::VecDeque<ShotId>,
    set: bevy::platform::collections::HashSet<ShotId>,
}

impl SeenShots {
    // DERIVED: 30 combatants x two weapons x ceil(750 RPM x `MAX_COSMETIC_CATCH_UP_TICKS` / 64 Hz)
    // is 1,200 ids. The next power of two covers every duplicate the catch-up gate can accept.
    const CAP: usize = 2_048;
    /// Record `shot`; return whether it was not already present.
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

/// A fire visual that arrived before its replica root. Facts are keyed by stable [`ShotId`], never
/// by the transient entity mapping which may be absent or stale during replication catch-up.
#[derive(Clone)]
struct PendingFire {
    event: FireEvent,
    received_tick: Tick,
}

/// Bounded, expiring fire visuals awaiting a unique live shooter root.
#[derive(Resource, Default)]
pub(super) struct PendingFireEvents {
    order: std::collections::VecDeque<ShotId>,
    by_shot: bevy::platform::collections::HashMap<ShotId, PendingFire>,
}

impl PendingFireEvents {
    // DERIVED: 256 is larger than the 60-fire synchronized 30-tank volley and stays below the
    // 2,048-entry ShotId dedup horizon, so a pending id cannot outlive its duplicate guard.
    const CAP: usize = 256;
    // DERIVED: 16 ticks is the server automatic-visual repair TTL; a later fire would be stale
    // presentation debt rather than a useful tracer.
    const TTL_TICKS: i32 = 16;

    /// Insert one unresolved fire. The return value is an older event evicted for bounded memory.
    fn insert(&mut self, event: FireEvent, received_tick: Tick) -> Option<PendingFire> {
        let shot = event.shot_id();
        if self.by_shot.contains_key(&shot) {
            return None;
        }
        self.order.push_back(shot);
        self.by_shot.insert(
            shot,
            PendingFire {
                event,
                received_tick,
            },
        );
        (self.order.len() > Self::CAP)
            .then(|| self.order.pop_front())
            .flatten()
            .and_then(|old| self.by_shot.remove(&old))
    }

    fn pop_front(&mut self) -> Option<PendingFire> {
        self.order
            .pop_front()
            .and_then(|shot| self.by_shot.remove(&shot))
    }

    fn requeue(&mut self, pending: PendingFire) {
        let shot = pending.event.shot_id();
        self.order.push_back(shot);
        self.by_shot.insert(shot, pending);
    }

    fn len(&self) -> usize {
        self.order.len()
    }
}

/// Validated fire announcements awaiting the interpolation cursor (`net::cursor_queue`): each
/// releases when the cursor crosses its `fire_tick`, so the bang/flash/kick lands as the stream
/// draws the shooter's hull at the pose it fired from. Dedup ([`SeenShots`]) and wire validation
/// stay at arrival; shooter resolution happens at release, where [`PendingFireEvents`] already owns
/// the missing-replica case.
#[derive(Resource, Default)]
pub(super) struct HeldFireEvents(CursorQueue<FireEvent>);

/// Connection-lifetime dedup ledger for owner-private damage confirmations.
///
/// INVARIANT: a [`DamageReceipt`] is never reused within a match. Forgetting one would permit a late
/// duplicate to raise a second authoritative confirmation, so this ledger has the receipt's lifetime
/// rather than the bounded presentation lifetime used by automatic-fire visuals.
#[derive(Resource, Default)]
pub(super) struct SeenDamage {
    set: bevy::platform::collections::HashSet<DamageReceipt>,
}

impl SeenDamage {
    fn mark_new(&mut self, receipt: DamageReceipt) -> bool {
        self.set.insert(receipt)
    }
}

/// The link ended: every estimator and ledger keyed to that server's tick clock starts empty for
/// the next connection. A new server's timeline starts behind the old one's, so a carried-over
/// newest tick would reject the new server's arrivals ([`ArrivalDelay`](super::sync_margin::ArrivalDelay))
/// and this client's newly authored inputs ([`AuthoredIntent`](super::fire_presentation::AuthoredIntent)),
/// and a pending trigger edge would replay onto the next tank; the impulse ticks
/// ([`ImpulseTicks`](super::extrapolate::ImpulseTicks)) would classify the new server's gaps by
/// the old server's ticks.
fn reset_link_state(
    _ended: On<Remove, Connected>,
    mut arrival: ResMut<super::sync_margin::ArrivalDelay>,
    mut intent: ResMut<super::fire_presentation::AuthoredIntent>,
    mut impulses: ResMut<super::extrapolate::ImpulseTicks>,
) {
    arrival.reset_for_new_link();
    *intent = default();
    *impulses = default();
}

/// A new transport connection starts a new receipt/presentation identity scope.
///
/// INVARIANT: Lightyear does not carry message-channel backlog across connections. Clearing these
/// client-only ledgers therefore cannot discard an in-flight fact from the new connection, and it
/// prevents a later server connection that reuses match-local ids from colliding with the previous
/// link. A connection spanning multiple Battles is forbidden until `ShotId` carries a Battle epoch.
fn reset_shot_receive_state(
    _connected: On<Add, Connected>,
    mut recoil: ResMut<PendingRecoilKicks>,
    mut shots: ResMut<SeenShots>,
    mut held: ResMut<HeldFireEvents>,
    mut fires: ResMut<PendingFireEvents>,
    mut damage: ResMut<SeenDamage>,
    mut sanctioned: ResMut<SanctionedShots>,
) {
    *recoil = default();
    *shots = default();
    held.0.clear();
    *fires = default();
    *damage = default();
    *sanctioned = default();
}

/// Drain public fire, keyframe, and terminal facts.
///
/// New fires are validated and held for the interpolation cursor ([`HeldFireEvents`]); the release
/// pass in this same drain spawns each one's cosmetic shell and enqueues its recoil when the cursor
/// crosses the fire tick (immediately, for an event the cursor already passed). Keyframes and
/// terminals arm the sanctioned outcome buffer at arrival — their presentation is slaved to the
/// shell's own flight, not to this seam. Damage confirmation is owner-private and handled by
/// [`receive_damage_confirms`].
///
/// This must run in `Update`: Lightyear clears undrained message receivers in `Last`, while a frame
/// can run no fixed steps. Draining in `FixedUpdate` would lose arrivals on such a frame. Only the
/// `TankSim` recoil write crosses to the fixed clock.
pub(super) fn receive_fire_events(
    mut batch_receivers: Query<&mut MessageReceiver<FireVisualBatch>>,
    mut fire_receivers: Query<&mut MessageReceiver<FireEvent>>,
    mut keyframe_receivers: Query<&mut MessageReceiver<RicochetKeyframe>>,
    mut confirm_receivers: Query<&mut MessageReceiver<ImpactConfirm>>,
    // Fire's display/recoil entity must resolve before a cosmetic shell can be created.
    tanks: Query<(Entity, &CombatantId), With<NetTank>>,
    // Fast-forward remote shells to this client's current tick.
    timeline: Res<LocalTimeline>,
    // The release clock: the fractional interpolation cursor every held announcement compares its
    // fire tick against. Synced-only — a pre-sync client has no interpolated motion to fuse with,
    // so its events present at arrival.
    cursor: Option<Res<InterpolationTimeline>>,
    mut pending: ResMut<PendingRecoilKicks>,
    mut seen: ResMut<SeenShots>,
    mut held: ResMut<HeldFireEvents>,
    mut pending_fires: ResMut<PendingFireEvents>,
    mut sanctioned: ResMut<SanctionedShots>,
    // Nested: the optional lifecycle recorder for wire arrivals/dedup results, the impulse
    // ledger for the buffer-edge gap∧impulse coincidence instrument (a fire announcement marks
    // the tick the shooter's hull surged), and the own-fire ledger + counters for swallowing a
    // late echo the fallback already presented. A tuple param keeps the system under Bevy's
    // 16-param bound.
    (mut shot_trace, mut impulses, mut own_ledgers, mut own_diag): (
        Option<ResMut<crate::shot_trace::ShotTrace>>,
        ResMut<super::extrapolate::ImpulseTicks>,
        Query<&mut super::fire_presentation::OwnFirePresentation>,
        ResMut<super::fire_presentation::OwnFireDiag>,
    ),
    mut commands: Commands,
) {
    let now = timeline.tick();
    let cursor = cursor
        .filter(|timeline| timeline.is_synced())
        .map(|timeline| (timeline.tick(), f64::from(timeline.overstep().to_f32())));
    for mut receiver in &mut batch_receivers {
        for batch in receiver.receive() {
            for fact in batch.facts {
                if let FireVisualFact::Fire(event) = &fact {
                    impulses.record(event.fire_tick, super::extrapolate::ImpulseClass::Fire);
                }
                consume_fire_visual_fact(
                    fact,
                    now,
                    cursor,
                    &mut seen,
                    &mut held,
                    &mut sanctioned,
                    &mut shot_trace,
                );
            }
        }
    }
    for mut receiver in &mut fire_receivers {
        for event in receiver.receive() {
            impulses.record(event.fire_tick, super::extrapolate::ImpulseClass::Fire);
            consume_fire_event(&event, now, cursor, &mut seen, &mut held, &mut shot_trace);
        }
    }
    for mut receiver in &mut keyframe_receivers {
        for keyframe in receiver.receive() {
            consume_ricochet_keyframe(&keyframe, now, &mut sanctioned, &mut shot_trace);
        }
    }
    for mut receiver in &mut confirm_receivers {
        for confirm in receiver.receive() {
            consume_impact_confirm(&confirm, now, &mut sanctioned, &mut shot_trace);
        }
    }
    release_due_fire_events(
        now,
        cursor,
        &tanks,
        &mut pending,
        &mut held,
        &mut pending_fires,
        &mut own_ledgers,
        &mut own_diag,
        &mut shot_trace,
        &mut commands,
    );
    resolve_pending_fire_events(
        now,
        cursor,
        &tanks,
        &mut pending,
        &mut pending_fires,
        &mut own_ledgers,
        &mut own_diag,
        &mut shot_trace,
        &mut commands,
    );
}

/// The fractional cursor as one number — `trace`'s `itick` — for the lifecycle rows.
fn cursor_itick(cursor: Option<(Tick, f64)>) -> Option<f64> {
    cursor.map(|(tick, overstep)| f64::from(tick.0) + overstep)
}

fn consume_fire_visual_fact(
    fact: FireVisualFact,
    now: Tick,
    cursor: Option<(Tick, f64)>,
    seen: &mut SeenShots,
    held: &mut HeldFireEvents,
    sanctioned: &mut SanctionedShots,
    shot_trace: &mut Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    match fact {
        FireVisualFact::Fire(event) => {
            consume_fire_event(&event, now, cursor, seen, held, shot_trace)
        }
        FireVisualFact::Ricochet(keyframe) => {
            consume_ricochet_keyframe(&keyframe, now, sanctioned, shot_trace)
        }
        FireVisualFact::Impact(confirm) => {
            consume_impact_confirm(&confirm, now, sanctioned, shot_trace)
        }
    }
}

/// Dedup and validate one arriving fire, then hold it for the interpolation cursor. Presentation
/// happens at release ([`release_due_fire_events`], same drain), never here.
fn consume_fire_event(
    event: &FireEvent,
    now: Tick,
    cursor: Option<(Tick, f64)>,
    seen: &mut SeenShots,
    held: &mut HeldFireEvents,
    shot_trace: &mut Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    let shot = event.shot_id();
    let fresh = seen.mark_new(shot);
    crate::shot_trace::record(shot_trace, "fire_rx", now.0, shot, || {
        json!({
            "dup": !fresh,
            "cu": fire_catch_up_ticks(event.fire_tick, now),
            "itick": cursor_itick(cursor),
        })
    });
    if !fresh {
        return;
    }
    if Dir3::new(event.direction).is_err() || fire_catch_up_ticks(event.fire_tick, now).is_none() {
        crate::shot_trace::record(
            shot_trace,
            "drop",
            now.0,
            shot,
            || json!({ "s": "fire", "res": "invalid_fire" }),
        );
        return;
    }
    if let Some((_, evicted)) = held.0.hold(event.fire_tick, event.clone()) {
        crate::shot_trace::record(
            shot_trace,
            "drop",
            now.0,
            evicted.shot_id(),
            || json!({ "s": "fire", "res": "held_capacity" }),
        );
    }
}

fn consume_ricochet_keyframe(
    keyframe: &RicochetKeyframe,
    now: Tick,
    sanctioned: &mut SanctionedShots,
    shot_trace: &mut Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    if Dir3::new(keyframe.direction).is_err() {
        return;
    }
    let result = sanctioned.insert(
        keyframe.shot,
        SanctionedBounce {
            origin: keyframe.origin,
            direction: keyframe.direction,
            speed: keyframe.speed,
            bounce_tick: keyframe.bounce_tick.0,
            sequence: keyframe.sequence,
        },
    );
    let duplicate = result == SanctionedBounceInsert::Duplicate;
    crate::shot_trace::record(
        shot_trace,
        "kf_rx",
        now.0,
        keyframe.shot,
        || json!({ "dup": duplicate, "bt": keyframe.bounce_tick.0, "seq": keyframe.sequence }),
    );
    if result == SanctionedBounceInsert::Capacity {
        crate::shot_trace::record(
            shot_trace,
            "drop",
            now.0,
            keyframe.shot,
            || json!({ "s": "kf", "res": "sanctioned_bounce_capacity", "seq": keyframe.sequence }),
        );
    }
}

fn consume_impact_confirm(
    confirm: &ImpactConfirm,
    now: Tick,
    sanctioned: &mut SanctionedShots,
    shot_trace: &mut Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    // A terminal entered the material; a confirm naming any other verdict is malformed and is not
    // buffered as one.
    let Ok(verdict) = TerminalVerdict::try_from(confirm.verdict) else {
        crate::shot_trace::record(
            shot_trace,
            "cf_rx",
            now.0,
            confirm.shot,
            || json!({ "res": "not_terminal" }),
        );
        return;
    };
    let fresh = sanctioned.insert_terminal(
        confirm.shot,
        SanctionedTerminal {
            position: confirm.position,
            normal: confirm.normal,
            verdict,
            impact_tick: confirm.impact_tick.0,
            after_bounces: confirm.after_bounces,
        },
    );
    crate::shot_trace::record(
        shot_trace,
        "cf_rx",
        now.0,
        confirm.shot,
        || json!({ "dup": !fresh, "it": confirm.impact_tick.0, "pen": confirm.verdict.entered() }),
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShooterResolution {
    Resolved(Entity),
    Missing,
    Ambiguous,
}

/// Resolve one visual fire to a root. The mapped entity wins only when its immutable identity agrees;
/// a stale map falls back to exactly one live root with the fact's [`CombatantId`].
fn resolve_shooter(
    event: &FireEvent,
    tanks: &Query<(Entity, &CombatantId), With<NetTank>>,
) -> ShooterResolution {
    if let Ok((entity, combatant)) = tanks.get(event.shooter)
        && *combatant == event.combatant
    {
        return ShooterResolution::Resolved(entity);
    }

    let mut matches = tanks
        .iter()
        .filter_map(|(entity, combatant)| (*combatant == event.combatant).then_some(entity));
    let Some(unique) = matches.next() else {
        return ShooterResolution::Missing;
    };
    if matches.next().is_some() {
        ShooterResolution::Ambiguous
    } else {
        ShooterResolution::Resolved(unique)
    }
}

/// The release half of the cursor hold: pop every fire whose tick the cursor has crossed and
/// present it. A missing/ambiguous shooter defers to [`PendingFireEvents`], which owns the
/// replica-race case (and its TTL) exactly as it did when presentation was at arrival. `None`
/// cursor = release everything (pre-sync fallback: arrival presentation).
#[expect(
    clippy::too_many_arguments,
    reason = "one release boundary owns cursor crossing and shooter resolution"
)]
fn release_due_fire_events(
    now: Tick,
    cursor: Option<(Tick, f64)>,
    tanks: &Query<(Entity, &CombatantId), With<NetTank>>,
    pending_recoil: &mut PendingRecoilKicks,
    held: &mut HeldFireEvents,
    pending_fires: &mut PendingFireEvents,
    own_ledgers: &mut Query<&mut super::fire_presentation::OwnFirePresentation>,
    own_diag: &mut super::fire_presentation::OwnFireDiag,
    shot_trace: &mut Option<ResMut<crate::shot_trace::ShotTrace>>,
    commands: &mut Commands,
) {
    let due = match cursor {
        Some((tick, overstep)) => held.0.release(tick, overstep),
        None => held.0.release_all(),
    };
    for (_, event) in due {
        match resolve_shooter(&event, tanks) {
            ShooterResolution::Resolved(shooter) => spawn_reconstructed_fire(
                &event,
                shooter,
                now,
                cursor,
                pending_recoil,
                own_ledgers,
                own_diag,
                shot_trace,
                commands,
            ),
            ShooterResolution::Missing | ShooterResolution::Ambiguous => {
                if let Some(evicted) = pending_fires.insert(event, now) {
                    crate::shot_trace::record(
                        shot_trace,
                        "drop",
                        now.0,
                        evicted.event.shot_id(),
                        || json!({ "s": "fire", "res": "pending_capacity" }),
                    );
                }
            }
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "one repair boundary owns expiry and shooter re-resolution"
)]
fn resolve_pending_fire_events(
    now: Tick,
    cursor: Option<(Tick, f64)>,
    tanks: &Query<(Entity, &CombatantId), With<NetTank>>,
    pending_recoil: &mut PendingRecoilKicks,
    pending_fires: &mut PendingFireEvents,
    own_ledgers: &mut Query<&mut super::fire_presentation::OwnFirePresentation>,
    own_diag: &mut super::fire_presentation::OwnFireDiag,
    shot_trace: &mut Option<ResMut<crate::shot_trace::ShotTrace>>,
    commands: &mut Commands,
) {
    let pending_count = pending_fires.len();
    for _ in 0..pending_count {
        let Some(pending) = pending_fires.pop_front() else {
            return;
        };
        let shot = pending.event.shot_id();
        if pending_fire_expired(pending.received_tick, now) {
            crate::shot_trace::record(
                shot_trace,
                "drop",
                now.0,
                shot,
                || json!({ "s": "fire", "res": "pending_expired" }),
            );
            continue;
        }
        match resolve_shooter(&pending.event, tanks) {
            ShooterResolution::Resolved(shooter) => spawn_reconstructed_fire(
                &pending.event,
                shooter,
                now,
                cursor,
                pending_recoil,
                own_ledgers,
                own_diag,
                shot_trace,
                commands,
            ),
            ShooterResolution::Missing | ShooterResolution::Ambiguous => {
                pending_fires.requeue(pending)
            }
        }
    }
}

fn pending_fire_expired(received_tick: Tick, now: Tick) -> bool {
    now - received_tick > PendingFireEvents::TTL_TICKS
}

#[expect(
    clippy::too_many_arguments,
    reason = "one presentation boundary owns the owed swallow and the release record"
)]
fn spawn_reconstructed_fire(
    event: &FireEvent,
    shooter: Entity,
    now: Tick,
    cursor: Option<(Tick, f64)>,
    pending_recoil: &mut PendingRecoilKicks,
    own_ledgers: &mut Query<&mut super::fire_presentation::OwnFirePresentation>,
    own_diag: &mut super::fire_presentation::OwnFireDiag,
    shot_trace: &mut Option<ResMut<crate::shot_trace::ShotTrace>>,
    commands: &mut Commands,
) {
    // An own echo arriving so late the belt-delta fallback already presented its round: one
    // consumption, one bang — swallow the duplicate (matched by weapon slot and signed fire-tick
    // order against the recorded reveal, so a future round can never match).
    if let Ok(mut ledger) = own_ledgers.get_mut(shooter)
        && ledger.try_swallow_owed(event.weapon as usize, event.fire_tick.0)
    {
        own_diag.swallowed += 1;
        crate::shot_trace::record(
            shot_trace,
            "drop",
            now.0,
            event.shot_id(),
            || json!({ "s": "fire", "res": "owed_swallow" }),
        );
        return;
    }
    let Ok(direction) = Dir3::new(event.direction) else {
        return;
    };
    let facts = super::fire_presentation::ReleasedShot {
        origin: event.origin,
        direction,
        speed: event.speed,
        caliber: event.caliber,
        mass: event.mass,
        nose_factor: event.nose_factor,
        tracer: event.tracer,
        mechanism: event.mechanism,
        shooter: ShotSource {
            tank: shooter,
            weapon: event.weapon as usize,
        },
        shot: Some(event.shot_id()),
    };
    let catch_up_ticks = match cursor {
        // CURSOR clock: the seam measures the shot's age on the same clock that released it.
        Some(cursor) => super::fire_presentation::present_released_shot(
            event.fire_tick,
            cursor,
            facts,
            pending_recoil,
            commands,
        ),
        // ARRIVAL clock — the surviving arrival-assigned presentation site: a pre-sync client has
        // no cursor (the queue released everything at arrival), so the shot ages against the local
        // timeline exactly as it did before the cursor existed.
        None => fire_catch_up_ticks(event.fire_tick, now).inspect(|&catch_up_ticks| {
            super::fire_presentation::present_shot_at_age(
                catch_up_ticks,
                facts,
                pending_recoil,
                commands,
            );
        }),
    };
    let Some(catch_up_ticks) = catch_up_ticks else {
        return;
    };
    crate::shot_trace::record(
        shot_trace,
        "present",
        now.0,
        event.shot_id(),
        || json!({ "itick": cursor_itick(cursor), "cu": catch_up_ticks }),
    );
}

/// Drain the owner-private `DamageConfirm` channel and raise one presentation event per receipt.
///
/// The channel target already proves ownership; receipt handling must not query a currently controlled
/// tank, because the player can receive a valid confirmation after death and before respawn.
/// This runs in `Update` for the same receiver-clear scheduling invariant as [`receive_fire_events`].
pub(super) fn receive_damage_confirms(
    mut receivers: Query<&mut MessageReceiver<DamageConfirm>>,
    timeline: Res<LocalTimeline>,
    mut seen: ResMut<SeenDamage>,
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
    // Impulse ledger for the buffer-edge gap∧impulse coincidence instrument: a damage confirm
    // marks the tick the victim's hull took the shot's impulse.
    mut impulses: ResMut<super::extrapolate::ImpulseTicks>,
    mut commands: Commands,
) {
    let received_tick = timeline.tick();
    for mut receiver in &mut receivers {
        for damage in receiver.receive() {
            impulses.record(damage.damage_tick, super::extrapolate::ImpulseClass::Damage);
            consume_damage_confirm(
                &damage,
                received_tick,
                &mut seen,
                &mut shot_trace,
                &mut commands,
            );
        }
    }
}

/// Record each owner-private confirmation, then pass only the first occurrence to presentation.
///
/// INVARIANT: `DamageReceipt` is stable match-local data, so reconstructing its [`ShotId`] cannot
/// depend on a replica that may have despawned between damage and receipt.
fn consume_damage_confirm(
    damage: &DamageConfirm,
    received_tick: Tick,
    seen: &mut SeenDamage,
    shot_trace: &mut Option<ResMut<crate::shot_trace::ShotTrace>>,
    commands: &mut Commands,
) {
    let shot = ShotId {
        combatant: damage.receipt.combatant,
        weapon: damage.receipt.weapon,
        fire_tick: damage.receipt.fire_tick,
    };
    let fresh = seen.mark_new(damage.receipt);
    crate::shot_trace::record(shot_trace, "dmg_rx", received_tick.0, shot, || {
        json!({
            "own": true,
            "dup": !fresh,
            "dt": damage.damage_tick.0,
        })
    });
    if fresh {
        commands.trigger(super::hit_feel::LocalHitConfirmed {
            receipt: damage.receipt,
            received_tick: received_tick.0,
            damage_tick: damage.damage_tick.0,
        });
    }
}

/// Age the observer's server-sanctioned bounce buffer each fixed tick and evict entries past a shell's
/// lifetime — the `net::client` half of the buffer's ring discipline (the ballistics march only READS
/// it). Fixed clock so expiry is measured in sim time, matching the shells that consume it; the buffer
/// lives only on the client, so this is the sole ager.
pub(super) fn age_sanctioned_shots(mut sanctioned: ResMut<SanctionedShots>, time: Res<Time>) {
    sanctioned.age(time.delta_secs());
}

/// LOCAL/ARRIVAL-clock shell age: ticks from `fire` to the local present `now`
/// (`LocalTimeline::tick()`). Two call sites only — the consume-time validity bar (a fire whose
/// tick is absurd against the local present is dropped before it is held), and the pre-sync
/// arrival-presentation fallback in [`spawn_reconstructed_fire`]. A CURSOR-released shot never
/// ages through this: its age is the seam's `cursor − released_tick`
/// (`net::fire_presentation::present_released_shot`). `Some(n)` fast-forwards `n` ticks (`n == 0`
/// = spawn at the muzzle and fly normally); `None` REJECTS the shot as absurd (the caller skips
/// the tracer AND the recoil).
///
/// Bounded by construction: lightyear's `Tick` is a MONOTONIC `u32` that does not wrap, and
/// `Sub<Tick>` returns the signed difference saturated to the `i32` range — never a naive `u32`
/// subtraction that would underflow. (A `u32` tick is exhausted after ~777 days at 64 Hz; no
/// session gets near it.) So any pair of ticks, however far apart, lands in one of the arms below,
/// which is what the saturation test pins.
///   - elapsed < 0: the fire tick is AHEAD of our current tick. The server fires at a tick ≤ its
///     own now, and the local (input) timeline runs ahead of the server, so this only happens on
///     clock skew or a malicious tick — don't rewind; spawn at the muzzle (`Some(0)`).
///   - 0 ≤ elapsed ≤ [`MAX_COSMETIC_CATCH_UP_TICKS`]: fast-forward that many ticks (the normal
///     case is DERIVED approximately 10).
///   - elapsed > [`MAX_COSMETIC_CATCH_UP_TICKS`]: absurd / stale nonsense — reject
///     (`None`), no loop.
fn fire_catch_up_ticks(fire: Tick, now: Tick) -> Option<u32> {
    let elapsed = now - fire;
    if elapsed < 0 {
        return Some(0);
    }
    let elapsed = elapsed as u32;
    (elapsed <= MAX_COSMETIC_CATCH_UP_TICKS).then_some(elapsed)
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
/// same tick; gated `in_state(Playing)` to match that consumer (see the registration). `TankSim` is
/// fixed-clock sim truth — a render-rate write would be a render→sim leak, non-deterministic across
/// 0/1/2-tick frames.
///
/// Public fire facts reach the shooter too: under fused own fire the echo carries the own barrel
/// kick through this same queue (the local fire tick applied none).
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
/// input plugin buffers it for the wire. The reverse bridge (net::protocol)
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

/// Stamp `TankCommand::for_tick` — **the one place input provenance is created**, and the reason the
/// sim can ever trust a command it reads out of an input buffer.
///
/// Runs in `InputSystems::WriteClientInputs`, chained AFTER whichever writer filled the
/// `ActionState` this tick (`feed_action_state` windowed, `harness::buffer_input` headless), and
/// therefore BEFORE lightyear's `buffer_action_state` (`InputSystems::BufferClientInputs`, which
/// lightyear orders strictly after `WriteClientInputs`). It stamps the command with `local_tick +
/// input_delay` — computed EXACTLY as `buffer_action_state` computes the slot it is about to file
/// the command into — so the stamp names the tick the command will be READ back on, on this client
/// and on the server alike.
///
/// From here the stamp is inert: it rides the `InputBuffer`, the wire, and the server's
/// `InputBuffer` without ever being rewritten. `net::protocol`'s bridge compares it against
/// the tick actually being simulated, and any command lightyear inherited, repeated, fabricated or
/// froze necessarily names a DIFFERENT tick. That is the whole mechanism — see
/// `bridge_action_state_to_tank_command` for the four ways it happens and
/// `TankCommand::fail_consumables_closed` for what we refuse to do about them.
///
/// The delay is lightyear's own `LocalTimelineSync::input_delay()` — the one its
/// `buffer_action_state` files the command under — so the stamp and the buffer slot cannot name
/// different ticks. Pre-sync the buffer is not written at all (lightyear gates it on
/// `SyncedLocalTimeline`), so a pre-sync stamp never reaches the wire.
pub(super) fn stamp_input_tick(
    timeline: Res<LocalTimeline>,
    sync: Res<LocalTimelineSync>,
    mut slots: Query<&mut ActionState<TankCommand>, With<InputMarker<TankCommand>>>,
) {
    let delay = sync.input_delay() as i32;
    let for_tick = timeline.tick() + delay;
    for mut state in &mut slots {
        state.0.for_tick = for_tick.0;
    }
}

/// Two headless matchmaking clients against the live Worker (`#[ignore]`d; needs `OVERMATCH_API`).
#[cfg(test)]
mod hosted_e2e;

/// The matchmaking exits through the installed lifecycle, over real UDP.
#[cfg(test)]
mod match_exit;

/// `apply_pending_recoil_kicks` derives a remote shot's barrel recoil from the LOCAL spec — these
/// exercise that derivation directly against a minimal rig. An external integration test can't reach
/// the sim types (`crate::tank` is a private module — `TankSim`/`Weapon`/`WeaponIndex` are not
/// externally nameable), so the honest test lives here in-crate, over real ECS state.
#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::ballistics::{FireShell, FireShellOrigin};

    #[derive(Resource)]
    struct TestDamageCopies(Vec<DamageConfirm>);

    #[derive(Resource, Default)]
    struct MarkerPulses(usize);

    #[test]
    fn server_endpoint_parsing_distinguishes_hostnames_and_raw_ips_without_dns() {
        assert_eq!(
            parse_server_endpoint("192.0.2.10").unwrap(),
            ServerEndpoint::Socket("192.0.2.10:5888".parse().unwrap())
        );
        assert_eq!(
            parse_server_endpoint("192.0.2.10:6000").unwrap(),
            ServerEndpoint::Socket("192.0.2.10:6000".parse().unwrap())
        );
        assert_eq!(
            parse_server_endpoint("[2001:db8::10]:6000").unwrap(),
            ServerEndpoint::Socket("[2001:db8::10]:6000".parse().unwrap())
        );
        assert_eq!(
            parse_server_endpoint("server.example").unwrap(),
            ServerEndpoint::Host {
                name: "server.example".to_string(),
                port: SERVER_PORT,
            }
        );
        assert_eq!(
            parse_server_endpoint("server.example:6000").unwrap(),
            ServerEndpoint::Host {
                name: "server.example".to_string(),
                port: 6000,
            }
        );
    }

    #[test]
    fn hostname_resolution_chooses_the_first_ipv4_without_network_access() {
        let endpoint = parse_server_endpoint("example.test:6000").unwrap();
        let resolved = resolve_server_endpoint_with(&endpoint, |name, port| {
            assert_eq!(name, "example.test");
            assert_eq!(port, 6000);
            Ok(vec![
                "[2001:db8::1]:6000".parse().unwrap(),
                "192.0.2.20:6000".parse().unwrap(),
                "192.0.2.21:6000".parse().unwrap(),
            ])
        })
        .unwrap();
        assert_eq!(resolved, "192.0.2.20:6000".parse().unwrap());
    }

    #[test]
    fn local_server_flag_beats_environment_beats_local_default() {
        let selected = choose_server(true, Some("environment.example:6000"));
        assert_eq!(
            selected,
            ServerSelection {
                raw: LOCAL_SERVER,
                source: ServerSource::LocalFlag,
            }
        );

        assert_eq!(
            choose_server(false, Some("environment.example")),
            ServerSelection {
                raw: "environment.example",
                source: ServerSource::Environment,
            }
        );
        assert_eq!(
            choose_server(false, None),
            ServerSelection {
                raw: LOCAL_SERVER,
                source: ServerSource::LocalDefault,
            }
        );
    }

    /// `--local` and `OVERMATCH_SERVER` keep exactly today's direct selection whatever Worker URL
    /// is set or baked.
    #[test]
    fn direct_paths_win_over_any_worker_url() {
        for (api, baked_api) in [
            (None, None),
            (Some("https://api.example"), None),
            (None, Some("https://baked-api.example")),
            (
                Some("https://api.example"),
                Some("https://baked-api.example"),
            ),
        ] {
            assert_eq!(
                choose_mode(true, Some("env.example"), api, baked_api),
                ClientMode::Direct(choose_server(true, Some("env.example"))),
            );
            assert_eq!(
                choose_mode(false, Some("env.example"), api, baked_api),
                ClientMode::Direct(choose_server(false, Some("env.example"))),
            );
        }
    }

    #[test]
    fn worker_url_selects_matchmaking_runtime_first() {
        assert_eq!(
            choose_mode(
                false,
                None,
                Some(" https://api.example "),
                Some("https://baked-api.example"),
            ),
            ClientMode::Matchmaking {
                api: "https://api.example",
                source: ApiSource::Environment,
            }
        );
        assert_eq!(
            choose_mode(false, None, Some(""), Some("https://baked-api.example"),),
            ClientMode::Matchmaking {
                api: "https://baked-api.example",
                source: ApiSource::BakedDefault,
            },
            "an empty OVERMATCH_API is unset"
        );
    }

    /// With no Worker URL anywhere the client connects to the local server.
    #[test]
    fn no_worker_url_falls_back_to_the_local_server() {
        assert_eq!(
            choose_mode(false, None, None, Some("  ")),
            ClientMode::Direct(ServerSelection {
                raw: LOCAL_SERVER,
                source: ServerSource::LocalDefault,
            })
        );
    }

    #[test]
    fn seeded_conditioner_requires_an_explicit_active_jitter_path() {
        assert_eq!(
            recv_conditioner_mode(80, 10, None, false),
            RecvConditionerMode::Stock,
            "an unset seed must preserve the stock conditioner",
        );
        assert_eq!(
            recv_conditioner_mode(0, 10, Some(7), false),
            RecvConditionerMode::Off,
            "zero base latency must preserve the existing disabled path",
        );
        assert_eq!(
            recv_conditioner_mode(80, 0, Some(7), false),
            RecvConditionerMode::Stock,
            "a seed without active jitter must not replace the stock latency path",
        );
        assert_eq!(
            recv_conditioner_mode(80, 10, Some(0), false),
            RecvConditionerMode::Seeded(0),
            "zero is a valid deterministic seed",
        );
        // A configured burst is deterministic without any jitter prerequisite: it must activate
        // the seeded path on its own (seed 0 default), from any other-knob state.
        assert_eq!(
            recv_conditioner_mode(0, 0, None, true),
            RecvConditionerMode::Seeded(0),
            "a burst alone must activate the seeded conditioner",
        );
        assert_eq!(
            recv_conditioner_mode(80, 10, Some(7), true),
            RecvConditionerMode::Seeded(7),
            "a burst must keep an explicit seed",
        );
    }

    /// The production `FireShell` trigger is the cosmetic-shell spawn boundary. This records only
    /// reconstructed shells, so tests can observe the fire-receive gate without inspecting it.
    #[derive(Resource, Default)]
    struct ReconstructedFireShells(Vec<(ShotId, Entity)>);

    fn feed_test_damage_copies(
        copies: Res<TestDamageCopies>,
        timeline: Res<LocalTimeline>,
        mut seen: ResMut<SeenDamage>,
        mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
        mut commands: Commands,
    ) {
        for damage in &copies.0 {
            consume_damage_confirm(
                damage,
                timeline.tick(),
                &mut seen,
                &mut shot_trace,
                &mut commands,
            );
        }
    }

    fn count_marker_pulse(
        _: On<super::super::hit_feel::LocalHitConfirmed>,
        mut pulses: ResMut<MarkerPulses>,
    ) {
        pulses.0 += 1;
    }

    fn record_reconstructed_fire_shell(
        fire: On<FireShell>,
        mut shells: ResMut<ReconstructedFireShells>,
    ) {
        if fire.shot_origin == FireShellOrigin::Reconstructed
            && let (Some(shot), Some(shooter)) = (fire.shot, fire.shooter)
        {
            shells.0.push((shot, shooter.tank));
        }
    }

    /// TRIPWIRE — **the shipping input delay must be CONSTANT.**
    ///
    /// This fails the moment someone swaps [`shipping_input_delay`] back to an ADAPTIVE config
    /// (`InputDelayConfig::balanced()`, or any config whose minimum and maximum differ), which is
    /// exactly the change that reintroduces the MG release leak — a delay that wobbles breaks
    /// lightyear's `Δend_tick == 1` assumption and either strands a stale PRESSED input on the
    /// server or fabricates a `SameAsPrecedent` one on both ends. See [`SHIPPING_INPUT_DELAY_TICKS`]
    /// for the full mechanism and `tests/net_fire_release.rs` for it driven over real lightyear.
    ///
    /// The assertion is on the config's SHAPE rather than on a swept `input_delay_ticks()` because
    /// that function is private to `lightyear_sync`. `minimum == maximum` is precisely its
    /// RTT-independence condition: with `min == max == n`, `rtt_ticks <= n` returns `n`, and every
    /// remaining branch returns `maximum_input_delay_before_prediction == n` for any `rtt_ticks <=
    /// maximum_predicted_ticks + n`. Only past that (~1.6 s of ping at 64 Hz — long past playable)
    /// does it grow again.
    ///
    /// If you genuinely need adaptive input delay back, the fix is NOT to delete this test: it is to
    /// make lightyear's client stop re-authoring a buffer tick it has already sent (upstream write-up
    /// in `upstream/lightyear-absent-anchor-input-freeze.md`).
    #[test]
    fn input_delay_is_constant() {
        let config = shipping_input_delay();
        assert_eq!(
            config.minimum_input_delay_ticks, config.maximum_input_delay_before_prediction,
            "the shipping input delay must be RTT-INDEPENDENT (minimum == maximum). An adaptive \
             delay makes lightyear's input-buffer write tick stall or jump, which strands stale \
             PRESSED inputs on the server and fabricates SameAsPrecedent ones on both ends — the \
             MG-fires-after-release leak. See SHIPPING_INPUT_DELAY_TICKS.",
        );
        assert_eq!(
            config.minimum_input_delay_ticks, SHIPPING_INPUT_DELAY_TICKS,
            "shipping input delay changed — intended?",
        );
    }

    /// A buffer holding one (absent) slot at `start`, with a remote high-water mark beside it.
    fn one_slot_buffer(start: Tick, last_remote: Tick) -> NativeBuffer<TankCommand> {
        let mut buffer = NativeBuffer::<TankCommand>::default();
        buffer.set_empty(start);
        buffer.last_remote_tick = Some(last_remote);
        buffer
    }

    /// A buffer with a start tick and a remote mark but no slots — the shape a stranded buffer
    /// is left in when every slot has been popped.
    fn slotless_buffer(start: Tick, last_remote: Tick) -> NativeBuffer<TankCommand> {
        let mut buffer = NativeBuffer::<TankCommand>::default();
        buffer.start_tick = Some(start);
        buffer.last_remote_tick = Some(last_remote);
        buffer
    }

    #[test]
    fn input_buffer_guard_clears_an_inverted_encoder_range() {
        let mut buffer = one_slot_buffer(Tick(313), Tick(312));

        assert_eq!(
            clear_stranded_input_buffer(&mut buffer, Tick(20)),
            Some(Tick(313))
        );
        assert_eq!(buffer.start_tick, None);
        assert!(buffer.is_empty());
        assert_eq!(buffer.last_remote_tick, None);
    }

    #[test]
    fn input_buffer_guard_preserves_a_current_encoder_range() {
        let mut buffer = one_slot_buffer(Tick(20), Tick(20));

        assert_eq!(clear_stranded_input_buffer(&mut buffer, Tick(20)), None);
        assert_eq!(buffer.start_tick, Some(Tick(20)));
        assert_eq!(buffer.len(), 1);
        assert_eq!(buffer.last_remote_tick, Some(Tick(20)));
    }

    /// Exercise the installed PostUpdate guard alone: this App deliberately does not install
    /// `ClientPlugins`, so `PrepareInputMessage` and its unsafe native encoder are absent. The
    /// schedule run still proves the guard's real system query updates the metric as well as the
    /// buffer, rather than only proving the extracted helper in isolation.
    #[test]
    fn installed_input_buffer_guard_clears_only_inverted_ranges_and_counts_them() {
        let mut app = App::new();
        install_input_buffer_guard(&mut app);

        let mut timeline = LocalTimeline::default();
        timeline.apply_delta(20);
        app.insert_resource(timeline);
        app.init_resource::<LocalTimelineSync>();
        app.world_mut().spawn(Client);

        let stranded = app
            .world_mut()
            .spawn((
                InputMarker::<TankCommand>::default(),
                slotless_buffer(Tick(313), Tick(312)),
            ))
            .id();
        let current = app
            .world_mut()
            .spawn((
                InputMarker::<TankCommand>::default(),
                slotless_buffer(Tick(20), Tick(20)),
            ))
            .id();

        app.world_mut().run_schedule(PostUpdate);

        let stranded = app
            .world()
            .get::<NativeBuffer<TankCommand>>(stranded)
            .unwrap();
        assert_eq!(stranded.start_tick, None);
        assert!(stranded.is_empty());
        assert_eq!(stranded.last_remote_tick, None);

        let current = app
            .world()
            .get::<NativeBuffer<TankCommand>>(current)
            .unwrap();
        assert_eq!(current.start_tick, Some(Tick(20)));
        assert!(current.is_empty());
        assert_eq!(current.last_remote_tick, Some(Tick(20)));
        assert_eq!(app.world().resource::<InputBufferGuardMetrics>().cleared, 1);
    }

    /// A malformed composition must not leave a marked buffer for Lightyear's encoder. Debug and
    /// test builds panic after the defensive clear; release records the error and keeps running.
    /// As above, no `ClientPlugins` are present, so this cannot invoke the hazardous encoder.
    #[cfg(debug_assertions)]
    #[test]
    fn installed_input_buffer_guard_clears_before_rejecting_multiple_clients() {
        let mut app = App::new();
        install_input_buffer_guard(&mut app);
        app.insert_resource(LocalTimeline::default());
        app.init_resource::<LocalTimelineSync>();
        app.world_mut().spawn(Client);
        app.world_mut().spawn(Client);
        let buffer = app
            .world_mut()
            .spawn((
                InputMarker::<TankCommand>::default(),
                slotless_buffer(Tick(313), Tick(312)),
            ))
            .id();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            app.world_mut().run_schedule(PostUpdate);
        }));
        assert!(result.is_err(), "debug builds must reject multiple clients");

        let buffer = app
            .world()
            .get::<NativeBuffer<TankCommand>>(buffer)
            .unwrap();
        assert_eq!(buffer.start_tick, None);
        assert!(buffer.is_empty());
        assert_eq!(buffer.last_remote_tick, None);
        assert_eq!(app.world().resource::<InputBufferGuardMetrics>().cleared, 1);
    }

    /// A resync that shifts the timeline AND shrinks the input delay must not suppress the next
    /// freshly buffered tick. DERIVED counterexample: local tick 100 at delay 4 stages input tick
    /// 104; a +20 shift recomputes the delay to 3; the next fixed tick (121) buffers input tick 124
    /// — the same number a watermark shifted by +20 would hold.
    #[test]
    fn a_resync_that_shrinks_the_input_delay_still_stages_the_next_tick() {
        let mut app = crate::net::test_harness::base_app();
        app.add_plugins(ClientPlugins {
            tick_duration: crate::net::test_harness::TICK,
        });
        super::super::protocol::plugin(&mut app);
        install_input_send_cadence(&mut app);
        let set_delay = |app: &mut App, ticks: u16| {
            app.insert_resource(InputTimelineConfig::new(
                SyncConfig::default(),
                InputDelayConfig::fixed_input_delay(ticks),
            ));
        };
        let fixed_tick = |app: &mut App| {
            app.world_mut().run_schedule(FixedFirst);
            app.world_mut().run_schedule(FixedPreUpdate);
        };
        let frame_stages = |app: &mut App| {
            app.world_mut().run_system_once(decide_input_send).unwrap();
            app.world().resource::<InputSendCadence>().due
        };

        set_delay(&mut app, 4);
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(99);
        app.world_mut()
            .resource_mut::<LocalTimelineSync>()
            .set_synced(true);
        fixed_tick(&mut app);
        assert_eq!(app.world().resource::<LocalTimeline>().tick(), Tick(100));
        assert_eq!(app.world().resource::<LocalTimelineSync>().input_delay(), 4);
        assert!(frame_stages(&mut app), "tick 100 stages input tick 104");
        assert!(
            !frame_stages(&mut app),
            "a frame with no new tick stages nothing"
        );

        // The resync, as lightyear's sync applies it between this frame's staging and the next
        // (lightyear's own observer moves `LocalTimeline` by the delta).
        app.world_mut().trigger(LocalTimelineShift { delta: 20 });
        assert_eq!(app.world().resource::<LocalTimeline>().tick(), Tick(120));
        set_delay(&mut app, 3);
        assert_eq!(app.world().resource::<LocalTimelineSync>().input_delay(), 3);

        fixed_tick(&mut app);
        assert_eq!(app.world().resource::<LocalTimeline>().tick(), Tick(121));
        assert!(
            frame_stages(&mut app),
            "the first frame of fixed tick 121 (input tick 124) must stage its message"
        );
        assert!(!frame_stages(&mut app), "and only once");
    }

    /// The stamp must name the tick lightyear will FILE the command under (`local_tick +
    /// input_delay`), not the tick that authored it — otherwise the bridge's comparison is off by
    /// the delay and every consumable fails closed forever. This inserts the same fixed-delay
    /// `InputTimelineConfig` as the shipping client, after `ClientPlugins` exactly as the client
    /// does, so removing `+ delay` — or inserting the config where lightyear's seeding observer
    /// cannot see it — fails the assertion.
    #[test]
    fn stamp_names_the_tick_the_command_will_be_read_on() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .add_plugins(ClientPlugins {
                tick_duration: Duration::from_secs_f64(1.0 / 64.0),
            });
        let world = app.world_mut();
        let mut timeline = LocalTimeline::default();
        timeline.apply_delta(42);
        world.insert_resource(timeline);
        world.insert_resource(InputTimelineConfig::new(
            SyncConfig::default(),
            shipping_input_delay(),
        ));
        world.spawn((Client, Link::default()));
        let entity = world
            .spawn((
                ActionState(TankCommand::default()),
                InputMarker::<TankCommand>::default(),
            ))
            .id();

        world.run_system_once(stamp_input_tick).unwrap();

        assert_eq!(
            world
                .get::<ActionState<TankCommand>>(entity)
                .unwrap()
                .0
                .for_tick,
            42 + u32::from(SHIPPING_INPUT_DELAY_TICKS),
            "the stamp names Lightyear's delayed input-buffer tick",
        );
    }
    use crate::spec::{FireMode, RecoilSpec, Trigger};
    use crate::tank::WeaponState;

    /// A one-weapon `Weapon` config with the given `recoil` spec and `barrel` node — the only two
    /// fields `kick_recoil`'s gate reads; the rest are filled to satisfy the struct.
    fn weapon(recoil: Option<RecoilSpec>, barrel: Option<Entity>) -> Weapon {
        Weapon {
            name: "MainGun".into(),
            round: "88mm_AP".into(),
            speed: 800.0,
            caliber: 0.088,
            mass: 10.2,
            nose_factor: 0.3,
            fire_mode: FireMode::Single { reload_secs: 8.0 },
            recoil,
            barrel,
            fire: Vec::new(),
            load: Vec::new(),
            trigger: Trigger::Primary,
            report_clips: Vec::new(),
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
    /// kick here would accumulate in `recoil_velocity` and never decay. The gate
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

    /// A shot fired ON our current tick needs no catch-up: spawn at the muzzle, fly normally.
    #[test]
    fn fire_tick_equal_to_now_is_zero_catch_up() {
        assert_eq!(fire_catch_up_ticks(Tick(500), Tick(500)), Some(0));
    }

    /// A fire tick AHEAD of our current tick (only reachable via clock skew or a malicious tick,
    /// since the server fires at a tick <= its now and the local timeline leads the server) clamps
    /// to 0, never rewinds the shell.
    #[test]
    fn future_fire_tick_clamps_to_zero() {
        assert_eq!(fire_catch_up_ticks(Tick(503), Tick(500)), Some(0));
    }

    /// A shot fired a few ticks before our current tick fast-forwards by exactly that many ticks.
    #[test]
    fn elapsed_within_bound_fast_forwards() {
        assert_eq!(fire_catch_up_ticks(Tick(500), Tick(505)), Some(5));
        assert_eq!(
            fire_catch_up_ticks(Tick(500), Tick(500 + MAX_COSMETIC_CATCH_UP_TICKS)),
            Some(MAX_COSMETIC_CATCH_UP_TICKS),
            "exactly at the bound still fast-forwards",
        );
    }

    /// A fire tick far in the past — a stale/lost `FireEvent`, or corrupt nonsense off the wire
    /// — is REJECTED (no tracer, no loop over 10^6 steps), the same reject-off-the-wire discipline as
    /// the bore guard.
    #[test]
    fn far_past_fire_tick_is_rejected() {
        assert_eq!(
            fire_catch_up_ticks(Tick(500), Tick(500 + MAX_COSMETIC_CATCH_UP_TICKS + 1)),
            None
        );
        assert_eq!(fire_catch_up_ticks(Tick(0), Tick(1_000_000)), None);
    }

    /// Tick arithmetic SATURATES rather than wrapping (lightyear 0.30's `Tick` is monotonic), so the
    /// extreme pairs land in the bounded arms: a fire tick at `u32::MAX` against a present at 0 reads
    /// as far in the future (spawn at the muzzle), and the reverse as absurdly stale (rejected) —
    /// never a ~4-billion-tick catch-up loop. Fires if lightyear re-introduces wrapping ticks.
    #[test]
    fn extreme_tick_pairs_saturate_into_the_bounded_arms() {
        assert_eq!(fire_catch_up_ticks(Tick(u32::MAX), Tick(0)), Some(0));
        assert_eq!(fire_catch_up_ticks(Tick(0), Tick(u32::MAX)), None);
        assert_eq!(fire_catch_up_ticks(Tick(u32::MAX - 2), Tick(3)), Some(0));
    }

    fn shot(tick: u32) -> ShotId {
        ShotId {
            combatant: crate::CombatantId(1),
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

    /// Owner-private damage receipts do not name a replica entity, so duplicate suppression remains
    /// valid through the death-to-respawn interval with no controlled tank root.
    #[test]
    fn seen_damage_dedups_a_receipt_without_a_live_controlled_tank() {
        let mut seen = SeenDamage::default();
        let receipt = DamageReceipt {
            combatant: crate::CombatantId(1),
            weapon: 1,
            fire_tick: 77,
        };

        assert!(
            seen.mark_new(receipt),
            "the first private confirmation reaches presentation"
        );
        assert!(
            !seen.mark_new(receipt),
            "a redundancy-window echo cannot pulse a second marker"
        );
        assert!(
            seen.mark_new(DamageReceipt {
                combatant: crate::CombatantId(1),
                weapon: 1,
                fire_tick: 78,
            }),
            "the next fire tick is a distinct confirmation"
        );
    }

    #[test]
    fn damage_receipt_remains_deduped_after_more_than_the_old_ring_horizon() {
        let mut seen = SeenDamage::default();
        let first = DamageReceipt {
            combatant: crate::CombatantId(1),
            weapon: 1,
            fire_tick: 1,
        };
        assert!(seen.mark_new(first));
        for fire_tick in 2..=256 {
            assert!(seen.mark_new(DamageReceipt { fire_tick, ..first }));
        }
        assert!(
            !seen.mark_new(first),
            "an authoritative confirmation stays exactly-once for the connection scope"
        );
    }

    #[test]
    fn a_new_connection_resets_match_local_shot_receive_state() {
        let mut app = App::new();
        app.init_resource::<PendingRecoilKicks>()
            .init_resource::<SeenShots>()
            .init_resource::<HeldFireEvents>()
            .init_resource::<PendingFireEvents>()
            .init_resource::<SeenDamage>()
            .init_resource::<SanctionedShots>()
            .add_observer(reset_shot_receive_state);
        let receipt = DamageReceipt {
            combatant: crate::CombatantId(1),
            weapon: 0,
            fire_tick: 1,
        };
        assert!(
            app.world_mut()
                .resource_mut::<SeenDamage>()
                .mark_new(receipt)
        );
        app.world_mut()
            .resource_mut::<SeenShots>()
            .mark_new(ShotId {
                combatant: receipt.combatant,
                weapon: receipt.weapon,
                fire_tick: receipt.fire_tick,
            });

        app.world_mut()
            .spawn((RemoteId(PeerId::Netcode(99)), Connected));

        assert!(app.world().resource::<SeenDamage>().set.is_empty());
        assert!(app.world().resource::<SeenShots>().set.is_empty());
    }

    /// A link ending — a drop, a leave, a despawn — empties the state keyed to that server's tick
    /// clock, so the next server's earlier ticks are accepted.
    #[test]
    fn a_link_ending_resets_its_clock_keyed_state() {
        use super::super::extrapolate::{ImpulseClass, ImpulseTicks};
        use super::super::fire_presentation::AuthoredIntent;
        use super::super::sync_margin::ArrivalDelay;
        let mut app = App::new();
        app.insert_resource(ArrivalDelay::test_with_newest_remote(Tick(5_000)))
            .insert_resource(AuthoredIntent::test_with_authored(5_000))
            .init_resource::<ImpulseTicks>()
            .add_observer(reset_link_state);
        app.world_mut()
            .resource_mut::<ImpulseTicks>()
            .record(Tick(5_000), ImpulseClass::Fire);
        let link = app
            .world_mut()
            .spawn((RemoteId(PeerId::Netcode(99)), Connected))
            .id();
        let world = app.world();
        assert!(
            world.resource::<ArrivalDelay>().newest_remote().is_some()
                && !world.resource::<AuthoredIntent>().is_empty()
                && !world.resource::<ImpulseTicks>().is_empty(),
            "fixture: a connected link's state"
        );
        app.world_mut().entity_mut(link).remove::<Connected>();
        let world = app.world();
        assert_eq!(world.resource::<ArrivalDelay>().newest_remote(), None);
        assert!(world.resource::<AuthoredIntent>().is_empty());
        assert!(world.resource::<ImpulseTicks>().is_empty());
    }

    #[test]
    fn duplicate_damage_receipts_trace_two_receives_and_one_marker_boundary() {
        let path = std::env::temp_dir().join(format!(
            "overmatch-damage-receipt-{}-{}.jsonl",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        {
            let receipt = DamageReceipt {
                combatant: crate::CombatantId(9),
                weapon: 1,
                fire_tick: 10,
            };
            let damage = DamageConfirm {
                receipt,
                damage_tick: Tick(12),
            };
            let mut timeline = LocalTimeline::default();
            timeline.apply_delta(13);
            let mut app = App::new();
            app.insert_resource(timeline);
            app.init_resource::<SeenDamage>();
            app.insert_resource(crate::shot_trace::ShotTrace::for_test(&path));
            app.insert_resource(TestDamageCopies(vec![damage.clone(), damage]));
            app.init_resource::<MarkerPulses>();
            super::super::hit_feel::mount_test_marker_boundary(&mut app);
            app.add_observer(count_marker_pulse)
                .add_systems(Update, feed_test_damage_copies);

            app.update();
            assert_eq!(
                app.world().resource::<MarkerPulses>().0,
                1,
                "receipt dedup permits exactly one presentation pulse"
            );
        }

        let rows: Vec<serde_json::Value> = std::fs::read_to_string(&path)
            .expect("test trace readable after app drop")
            .lines()
            .map(|line| serde_json::from_str(line).expect("trace row is JSON"))
            .collect();
        let damage_rows: Vec<_> = rows.iter().filter(|row| row["k"] == "dmg_rx").collect();
        let marker_rows: Vec<_> = rows.iter().filter(|row| row["k"] == "marker").collect();
        assert_eq!(damage_rows.len(), 2, "every redundancy copy is traced");
        assert_eq!(
            marker_rows.len(),
            1,
            "only the first receipt reaches the marker"
        );
        assert_eq!(damage_rows[0]["dup"], false);
        assert_eq!(damage_rows[1]["dup"], true);
        assert_eq!(damage_rows[0]["t"], 13);
        assert_eq!(damage_rows[0]["dt"], 12);
        assert_eq!(marker_rows[0]["t"], 13);
        assert_eq!(marker_rows[0]["dt"], 12);
        std::fs::remove_file(path).expect("test trace removed");
    }

    fn fire_event(shooter: Entity, combatant: u64, tick: u32) -> FireEvent {
        FireEvent {
            origin: Vec3::ZERO,
            direction: Vec3::NEG_Z,
            speed: 800.0,
            caliber: 0.0079,
            mass: 0.0118,
            nose_factor: 0.5,
            mechanism: crate::spec::FireMechanism::Automatic,
            tracer: true,
            shooter,
            combatant: CombatantId(combatant),
            weapon: 0,
            fire_tick: Tick(tick),
        }
    }

    /// A fire may beat its root's replica. It is retained by stable shot identity and resolves when
    /// exactly one matching combatant root exists, even if the original entity map was unresolved.
    #[test]
    fn pending_fire_resolves_when_its_combatant_root_arrives() {
        let mut world = World::new();
        let unmapped = Entity::from_raw_u32(999).expect("non-placeholder entity");
        let event = fire_event(unmapped, 7, 42);
        let mut pending = PendingFireEvents::default();
        assert!(pending.insert(event.clone(), Tick(42)).is_none());
        assert_eq!(pending.len(), 1, "unresolved fire is retained");

        let before = world
            .run_system_once(move |tanks: Query<(Entity, &CombatantId), With<NetTank>>| {
                resolve_shooter(&event, &tanks)
            })
            .expect("system runs");
        assert_eq!(before, ShooterResolution::Missing);

        let root = world.spawn((NetTank, CombatantId(7))).id();
        let pending_event = pending.pop_front().expect("queued event remains").event;
        let after = world
            .run_system_once(move |tanks: Query<(Entity, &CombatantId), With<NetTank>>| {
                resolve_shooter(&pending_event, &tanks)
            })
            .expect("system runs");
        assert_eq!(
            after,
            ShooterResolution::Resolved(root),
            "the stable combatant id repairs an unresolved entity mapping",
        );
    }

    /// Consume one arriving fire through the production gate, then run the release pass at the
    /// given cursor (`None` = the pre-sync fallback: release everything).
    fn consume_and_release(
        world: &mut World,
        event: FireEvent,
        now: Tick,
        cursor: Option<(Tick, f64)>,
    ) {
        world
            .run_system_once(
                move |tanks: Query<(Entity, &CombatantId), With<NetTank>>,
                      mut recoil: ResMut<PendingRecoilKicks>,
                      mut seen: ResMut<SeenShots>,
                      mut held: ResMut<HeldFireEvents>,
                      mut pending: ResMut<PendingFireEvents>,
                      mut own_ledgers: Query<
                    &mut crate::net::fire_presentation::OwnFirePresentation,
                >,
                      mut own_diag: ResMut<crate::net::fire_presentation::OwnFireDiag>,
                      mut commands: Commands| {
                    let mut trace: Option<ResMut<crate::shot_trace::ShotTrace>> = None;
                    consume_fire_event(&event, now, cursor, &mut seen, &mut held, &mut trace);
                    release_due_fire_events(
                        now,
                        cursor,
                        &tanks,
                        &mut recoil,
                        &mut held,
                        &mut pending,
                        &mut own_ledgers,
                        &mut own_diag,
                        &mut trace,
                        &mut commands,
                    );
                },
            )
            .expect("production fire-consumption gate runs");
    }

    /// Run only the release pass at the given cursor.
    fn release_at(world: &mut World, now: Tick, cursor: Option<(Tick, f64)>) {
        world
            .run_system_once(
                move |tanks: Query<(Entity, &CombatantId), With<NetTank>>,
                      mut recoil: ResMut<PendingRecoilKicks>,
                      mut held: ResMut<HeldFireEvents>,
                      mut pending: ResMut<PendingFireEvents>,
                      mut own_ledgers: Query<
                    &mut crate::net::fire_presentation::OwnFirePresentation,
                >,
                      mut own_diag: ResMut<crate::net::fire_presentation::OwnFireDiag>,
                      mut commands: Commands| {
                    let mut trace: Option<ResMut<crate::shot_trace::ShotTrace>> = None;
                    release_due_fire_events(
                        now,
                        cursor,
                        &tanks,
                        &mut recoil,
                        &mut held,
                        &mut pending,
                        &mut own_ledgers,
                        &mut own_diag,
                        &mut trace,
                        &mut commands,
                    );
                },
            )
            .expect("production release pass runs");
    }

    fn fire_receive_app() -> App {
        let mut app = App::new();
        app.init_resource::<PendingRecoilKicks>()
            .init_resource::<SeenShots>()
            .init_resource::<HeldFireEvents>()
            .init_resource::<PendingFireEvents>()
            .init_resource::<crate::net::fire_presentation::OwnFireDiag>()
            .init_resource::<ReconstructedFireShells>()
            .add_observer(record_reconstructed_fire_shell);
        app
    }

    #[test]
    fn pending_fire_duplicate_is_suppressed_by_shot_id() {
        let mut app = fire_receive_app();
        let world = app.world_mut();
        let unmapped = Entity::from_raw_u32(999).expect("non-placeholder entity");
        let event = fire_event(unmapped, 7, 42);
        let shot = event.shot_id();

        consume_and_release(world, event.clone(), Tick(42), None);
        let root = world.spawn((NetTank, CombatantId(7))).id();
        world
            .run_system_once(
                |tanks: Query<(Entity, &CombatantId), With<NetTank>>,
                 mut recoil: ResMut<PendingRecoilKicks>,
                 mut pending: ResMut<PendingFireEvents>,
                 mut own_ledgers: Query<
                    &mut crate::net::fire_presentation::OwnFirePresentation,
                >,
                 mut own_diag: ResMut<crate::net::fire_presentation::OwnFireDiag>,
                 mut commands: Commands| {
                    let mut trace: Option<ResMut<crate::shot_trace::ShotTrace>> = None;
                    resolve_pending_fire_events(
                        Tick(42),
                        None,
                        &tanks,
                        &mut recoil,
                        &mut pending,
                        &mut own_ledgers,
                        &mut own_diag,
                        &mut trace,
                        &mut commands,
                    );
                },
            )
            .expect("pending fire resolves through the production repair path");
        consume_and_release(world, event, Tick(42), None);

        assert_eq!(
            world.resource::<PendingFireEvents>().len(),
            0,
            "the deferred visual resolves before its redundant transport copy arrives"
        );
        assert_eq!(
            world.resource::<ReconstructedFireShells>().0,
            vec![(shot, root)],
            "the real receive gate admits one reconstructed shell across pending repair and echo"
        );
    }

    /// THE CURSOR HOLD, over the production receive gate: a fire whose tick is ahead of the
    /// interpolation cursor presents nothing — however many release passes run at a stalled
    /// reading — and presents exactly once when the cursor crosses its fire tick.
    #[test]
    fn a_fire_ahead_of_the_cursor_presents_only_at_the_crossing() {
        let mut app = fire_receive_app();
        let world = app.world_mut();
        let root = world.spawn((NetTank, CombatantId(7))).id();
        let event = fire_event(root, 7, 100);
        let shot = event.shot_id();

        // Arrival: predicted present 108, cursor 93.5 — the fire tick is 6.5 ticks ahead.
        consume_and_release(world, event, Tick(108), Some((Tick(93), 0.5)));
        for _ in 0..3 {
            release_at(world, Tick(108), Some((Tick(93), 0.5)));
        }
        assert!(
            world.resource::<ReconstructedFireShells>().0.is_empty(),
            "a stalled cursor holds the presentation, and holds it across repeated drains",
        );

        release_at(world, Tick(110), Some((Tick(100), 0.0)));
        assert_eq!(
            world.resource::<ReconstructedFireShells>().0,
            vec![(shot, root)],
            "the crossing releases the one presentation",
        );

        release_at(world, Tick(111), Some((Tick(101), 0.0)));
        assert_eq!(
            world.resource::<ReconstructedFireShells>().0.len(),
            1,
            "a released fire never presents again",
        );
    }

    /// A fire whose tick the cursor already crossed presents on the SAME drain it arrived on — the
    /// late-join / long-RTT case is immediate, exactly as it was at arrival presentation.
    #[test]
    fn a_fire_behind_the_cursor_presents_immediately() {
        let mut app = fire_receive_app();
        let world = app.world_mut();
        let root = world.spawn((NetTank, CombatantId(7))).id();
        let event = fire_event(root, 7, 100);
        let shot = event.shot_id();

        consume_and_release(world, event, Tick(120), Some((Tick(112), 0.25)));
        assert_eq!(
            world.resource::<ReconstructedFireShells>().0,
            vec![(shot, root)],
            "an already-crossed fire presents at arrival",
        );
    }

    /// CERTIFICATION — the released burst, over the production consume/release gates: a 750-rpm
    /// MG belt plus cannon rounds arrive ahead of the cursor and release as it sweeps. The
    /// counters must agree — bang = flash = tracer = consumed rounds — and every round must
    /// spawn younger than one tick (`catch_up_ticks` 0), which bounds its tracer's spawn
    /// distance from the muzzle to `catch_up · v · Δt = 0 m`, strictly under one tick of shell
    /// flight (`v · Δt` = 12.5 m at 800 m/s). The one named shot-skip class — a redundant
    /// transport copy of an already-presented shot — is probed and presents nothing.
    #[test]
    fn released_burst_presents_every_consumed_round_exactly_once() {
        use crate::ballistics::STALE_FIRE_TICKS;

        #[derive(Resource, Default)]
        struct Certified(Vec<(u32, bool)>);

        fn certify(fire: On<FireShell>, mut certified: ResMut<Certified>) {
            if fire.shot_origin == FireShellOrigin::Reconstructed {
                certified.0.push((fire.catch_up_ticks, fire.tracer));
            }
        }

        let mut app = fire_receive_app();
        app.init_resource::<Certified>();
        app.add_observer(certify);
        let world = app.world_mut();
        let root = world.spawn((NetTank, CombatantId(7))).id();

        // The burst: 20 MG rounds every 5 ticks from tick 100, plus 2 cannon rounds. Every
        // round is a tracer, so the tracer counter must equal the bang counter.
        let mut events: Vec<FireEvent> =
            (0..20).map(|i| fire_event(root, 7, 100 + 5 * i)).collect();
        for cannon_tick in [103, 158] {
            let mut cannon = fire_event(root, 7, cannon_tick);
            cannon.mechanism = crate::spec::FireMechanism::Single;
            cannon.speed = 773.0;
            cannon.caliber = 0.088;
            cannon.mass = 10.2;
            cannon.weapon = 1;
            events.push(cannon);
        }
        let consumed = events.len();
        let duplicate = events[0].clone();

        // The link: a round fired at tick f arrives at local present f+8, while the cursor
        // renders 16 ticks behind the present — ahead of the cursor at arrival, crossed 8
        // ticks later with the cursor half a tick into f.
        for now in 100u32..=225 {
            let cursor = Some((Tick(now - 16), 0.5));
            let mut arrived = false;
            for event in events.iter().filter(|e| e.fire_tick.0 + 8 == now) {
                consume_and_release(world, event.clone(), Tick(now), cursor);
                arrived = true;
            }
            if !arrived {
                release_at(world, Tick(now), cursor);
            }
        }

        // The shot-skip probe: the redundant transport copy of an already-presented shot is
        // behind the cursor (would present immediately) and must be swallowed by its ShotId.
        consume_and_release(world, duplicate, Tick(230), Some((Tick(214), 0.5)));

        let certified = &world.resource::<Certified>().0;
        let bang = certified.len();
        let flash = certified
            .iter()
            .filter(|(catch_up, _)| *catch_up <= STALE_FIRE_TICKS)
            .count();
        let tracer = certified.iter().filter(|(_, tracer)| *tracer).count();
        println!("certification: consumed={consumed} bang={bang} flash={flash} tracer={tracer}",);
        assert_eq!(
            (bang, flash, tracer),
            (consumed, consumed, consumed),
            "every consumed round presents exactly once, under the flash stale gate, tracer lit",
        );
        for &(catch_up, _) in certified {
            assert_eq!(
                catch_up, 0,
                "released at the crossing: age < 1 tick, spawn AT the muzzle — never the \
                 local timeline's arrival lead",
            );
        }
    }

    /// FUSED OWN FIRE: the echo of a locally-fired round is the round's one presentation — the
    /// local fire tick drew nothing, so the echo must present the shell and carry the kick.
    #[test]
    fn the_own_echo_is_the_rounds_one_presentation() {
        let mut app = fire_receive_app();
        let world = app.world_mut();
        let own = world
            .spawn((
                NetTank,
                CombatantId(7),
                ActionState::<TankCommand>::default(),
                InputMarker::<TankCommand>::default(),
            ))
            .id();

        let fused_event = fire_event(own, 7, 43);
        let fused_shot = fused_event.shot_id();
        consume_and_release(world, fused_event, Tick(50), None);
        assert_eq!(
            world.resource::<ReconstructedFireShells>().0,
            vec![(fused_shot, own)],
            "fused: the echo is the own round's one presentation",
        );
        assert_eq!(
            world.resource::<PendingRecoilKicks>().0,
            vec![(own, 0)],
            "fused: the echo carries the barrel kick",
        );
    }

    #[test]
    fn ambiguous_combatant_roots_fail_closed() {
        let mut world = World::new();
        let stale_map = world.spawn_empty().id();
        world.spawn((NetTank, CombatantId(7)));
        world.spawn((NetTank, CombatantId(7)));
        let event = fire_event(stale_map, 7, 42);

        let resolution = world
            .run_system_once(move |tanks: Query<(Entity, &CombatantId), With<NetTank>>| {
                resolve_shooter(&event, &tanks)
            })
            .expect("system runs");
        assert_eq!(
            resolution,
            ShooterResolution::Ambiguous,
            "two live roots with one CombatantId must not receive an arbitrary visual/recoil event",
        );
    }

    #[test]
    fn pending_fire_expires_after_the_visual_ttl() {
        let received = Tick(100);
        let last_live = Tick(100 + PendingFireEvents::TTL_TICKS as u32);
        assert!(
            !pending_fire_expired(received, last_live),
            "the last tick inside the TTL remains eligible for root repair"
        );
        assert!(
            pending_fire_expired(received, last_live + 1),
            "a stale visual is discarded instead of becoming delayed cosmetic debt"
        );
    }

    /// A mapped `FireEvent.shooter` is only a display/recoil handle. Two receiver-local mappings
    /// for the same wire fact must share its stable identity, so the production visual-fact seam
    /// spawns once and its sanctioned outcome remains available under that one [`ShotId`].
    #[test]
    fn shot_identity_is_independent_of_entity_mapping() {
        let mut app = fire_receive_app();
        app.init_resource::<SanctionedShots>();
        let world = app.world_mut();
        let mapped_here = world.spawn((NetTank, CombatantId(7))).id();
        let mapped_elsewhere = world.spawn((NetTank, CombatantId(7))).id();
        let here = fire_event(mapped_here, 7, 42);
        let elsewhere = fire_event(mapped_elsewhere, 7, 42);
        let shot = here.shot_id();

        assert_ne!(mapped_here, mapped_elsewhere, "receiver mappings differ");
        assert_eq!(
            shot,
            elsewhere.shot_id(),
            "stable wire fields, not an ECS mapping, define the ShotId"
        );

        world
            .run_system_once(
                move |tanks: Query<(Entity, &CombatantId), With<NetTank>>,
                      mut recoil: ResMut<PendingRecoilKicks>,
                      mut seen: ResMut<SeenShots>,
                      mut held: ResMut<HeldFireEvents>,
                      mut pending: ResMut<PendingFireEvents>,
                      mut sanctioned: ResMut<SanctionedShots>,
                      mut own_ledgers: Query<
                    &mut crate::net::fire_presentation::OwnFirePresentation,
                >,
                      mut own_diag: ResMut<crate::net::fire_presentation::OwnFireDiag>,
                      mut commands: Commands| {
                    let mut trace: Option<ResMut<crate::shot_trace::ShotTrace>> = None;
                    consume_fire_visual_fact(
                        FireVisualFact::Fire(here.clone()),
                        Tick(42),
                        None,
                        &mut seen,
                        &mut held,
                        &mut sanctioned,
                        &mut trace,
                    );
                    consume_fire_visual_fact(
                        FireVisualFact::Fire(elsewhere.clone()),
                        Tick(42),
                        None,
                        &mut seen,
                        &mut held,
                        &mut sanctioned,
                        &mut trace,
                    );
                    consume_fire_visual_fact(
                        FireVisualFact::Ricochet(RicochetKeyframe {
                            shot,
                            origin: Vec3::ZERO,
                            direction: Vec3::NEG_Z,
                            speed: 600.0,
                            bounce_tick: Tick(43),
                            sequence: 0,
                        }),
                        Tick(43),
                        None,
                        &mut seen,
                        &mut held,
                        &mut sanctioned,
                        &mut trace,
                    );
                    release_due_fire_events(
                        Tick(43),
                        None,
                        &tanks,
                        &mut recoil,
                        &mut held,
                        &mut pending,
                        &mut own_ledgers,
                        &mut own_diag,
                        &mut trace,
                        &mut commands,
                    );
                },
            )
            .expect("production visual-fact seam runs");

        assert_eq!(
            world.resource::<ReconstructedFireShells>().0,
            vec![(shot, mapped_here)],
            "the first mapped view reconstructs one shell; its remapped echo is deduped"
        );
        assert!(
            world.resource::<SanctionedShots>().has_shot(shot),
            "the sanctioned bounce is retrievable through either mapped view's stable ShotId"
        );
    }

    /// The dedup ring is bounded while recent visual copies remain suppressed.
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
        // Capacity eviction never removes the newest ids.
        assert!(
            !seen.mark_new(shot(SeenShots::CAP as u32 + 5)),
            "a recent id is still deduped"
        );
    }

    #[test]
    fn seen_shots_retains_the_full_accepted_thirty_player_fire_horizon() {
        let mut seen = SeenShots::default();
        let oldest = shot(0);
        assert!(seen.mark_new(oldest));

        // DERIVED: 30 combatants x two weapons x ceil(750 RPM x MAX_COSMETIC_CATCH_UP_TICKS / 64 Hz).
        let accepted_horizon: u32 = 30 * 2 * 20;
        for fire_tick in 1..=accepted_horizon {
            assert!(seen.mark_new(shot(fire_tick)));
        }

        assert!(
            !seen.mark_new(oldest),
            "a duplicate that can pass catch-up validation must remain deduplicated"
        );
    }

    /// A matchmaking link closes on exactly its three exits — a server reported gone (at once), a
    /// link that never connected after the hint threshold, a once-connected link whose reconnects
    /// are exhausted — and a never-connected close is a failed join, a once-connected one the
    /// match's end. Time alone closes nothing: only an exhausted ledger does.
    #[test]
    fn match_link_closes_on_each_exit_and_not_before() {
        use super::super::matchmaking::MatchClosed;
        let joining = ConnectRetry {
            initiated: true,
            attempts: MISMATCH_HINT_AFTER_ATTEMPTS - 1,
            ..default()
        };
        assert_eq!(close_verdict(&joining, None), None);
        assert_eq!(
            close_verdict(
                &ConnectRetry {
                    attempts: MISMATCH_HINT_AFTER_ATTEMPTS,
                    ..joining
                },
                None
            ),
            Some(MatchClosed::Unreachable(
                "could not reach the match server".into()
            ))
        );
        assert_eq!(
            close_verdict(&joining, Some("start failed".into())),
            Some(MatchClosed::Unreachable("start failed".into())),
            "a server gone before the first connect is a failed join"
        );

        let down = ConnectRetry {
            initiated: true,
            connected_once: true,
            attempts: 40,
            down_since: Some(10.0),
            last_attempt_at: Some(10.0 + 10.0 * RECONNECT_GIVE_UP_S),
            ..default()
        };
        assert_eq!(
            close_verdict(&down, None),
            None,
            "a dropped match keeps reconnecting until a retry past the give-up has failed"
        );
        let exhausted = ConnectRetry {
            exhausted: true,
            ..down
        };
        assert_eq!(
            close_verdict(&exhausted, None),
            Some(MatchClosed::Ended(
                "the match server stopped answering".into()
            ))
        );
        assert_eq!(
            close_verdict(&down, Some("server stopped".into())),
            Some(MatchClosed::Ended("server stopped".into())),
            "the Worker's verdict closes at once"
        );
    }

    /// The backoff never exceeds the cap, no matter how many attempts have piled up — an
    /// indefinitely-retrying client still re-checks for a server at least that often. Also asserts
    /// monotonic growth so a broken BASE/STEP schedule can't hide behind a still-correct cap.
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
