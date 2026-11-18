//! Network-client composition root.
//!
//! Every hull — the own tank included — renders from the interpolated server stream; client
//! presentation never pauses the simulation while the authority continues ticking.

use super::admission::JoinRefusal;
use super::compatibility::Compatibility;
use core::time::Duration;
use std::hash::{BuildHasher, Hasher};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};

use super::view_clock::ViewClock;
use bevy::app::ScheduleRunnerPlugin;
use bevy::ecs::query::QuerySingleError;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use lightyear::prelude::client::*;
use lightyear::prelude::{Controlled as NetControlled, *};
use serde_json::json;

use super::cursor_queue::CursorQueue;
use super::protocol::{
    DamageConfirm, DamageReceipt, FireEvent, FireVisualBatch, FireVisualFact, NetTank,
};
use super::{diagnostics, harness, open_gameplay_gate, physics, rig};
use crate::ballistics::{
    FireShell, FireShellOrigin, FlightOutcome, MAX_COSMETIC_CATCH_UP_TICKS, SanctionedInsert,
    SanctionedShots, ShotSource,
};
use crate::command::{DeviceCommand, TankCommand};
use crate::run_window::RunWindow;
use crate::state::{AppState, GameplaySet};
use crate::tank::{
    Controlled as GameControlled, Muzzle, PendingTankAssets, Rig, TankRoot, TankSim, Weapon,
    WeaponIndex, load_tank_assets,
};
use crate::ui_font::UiFonts;
use crate::{CombatantId, NetClientPlugin, ShotId, SimPlugin};

mod policy;
use policy::client_policy;

const RECONNECT_LABEL: &str = "Connection lost, trying to reconnect…";

const SERVER_PORT: u16 = 5888;
const LOCAL_SERVER: &str = "127.0.0.1:5888";
/// The matchmaking Worker's base URL baked in at build time (the release workflow sets
/// `OVERMATCH_DEFAULT_API` and refuses to ship a client without it); when set, a client without
/// `OVERMATCH_SERVER` starts in the Join menu instead of connecting to [`LOCAL_SERVER`].
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

/// `OVERMATCH_SERVER` selects its direct path whatever else is set; otherwise a non-empty Worker
/// URL — `OVERMATCH_API`, then the baked one — selects matchmaking; otherwise the local server.
fn choose_mode<'a>(
    server: Option<&'a str>,
    api: Option<&'a str>,
    baked_api: Option<&'a str>,
) -> ClientMode<'a> {
    let direct = ClientMode::Direct(choose_server(server));
    if server.is_some() {
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
    /// See the client-id note in [`configure_link`].
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
    burst: Option<harness::Burst>,
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
            netcode_link(
                server_addr,
                self.client_id,
                private_key,
                Compatibility::local().clone(),
            ),
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
            if let Some(burst) = link.burst {
                seeded = seeded.with_burst(burst, lightyear::core::time::Instant::now());
            }
            seeded
        })
    }
}

/// The compatibility record a client link's token states; a refusal is read against it.
#[derive(Component, Debug)]
pub(super) struct StatedRecord(Compatibility);

/// A client link's netcode endpoint, its token stating `record`.
pub(super) fn netcode_link(
    server_addr: SocketAddr,
    client_id: u64,
    private_key: [u8; 32],
    record: Compatibility,
) -> (NetcodeClient, StatedRecord) {
    let defaults = NetcodeConfig::default();
    let token =
        lightyear::netcode::ConnectToken::build(server_addr, record.tag(), client_id, private_key)
            .timeout_seconds(defaults.client_timeout_secs)
            .expire_seconds(-1)
            .user_data(record.user_data())
            .generate()
            .expect("a token for one address under a 32-byte key always builds");
    (
        NetcodeClient::new(Authentication::Token(token), defaults)
            .expect("a generated token always reads back"),
        StatedRecord(record),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerSource {
    Environment,
    LocalDefault,
}

impl ServerSource {
    const fn label(self) -> &'static str {
        match self {
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

fn choose_server(environment: Option<&str>) -> ServerSelection<'_> {
    if let Some(raw) = environment {
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

/// A burst selects deterministic epoch-phase conditioning (no RNG), defaulting unset knobs to
/// zero. Otherwise seeded conditioning needs explicit seed, positive latency and jitter;
/// latency alone uses stock conditioning, and zero latency disables it.
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

/// Where the client link goes: a server resolved once at startup, or a Worker whose Join menu
/// finds one.
enum LinkTarget {
    Direct {
        server_addr: SocketAddr,
        source: ServerSource,
    },
    Matchmaking {
        api: String,
        source: ApiSource,
    },
}

/// The client link as the arguments and environment configure it, read once at startup.
struct LinkSetup {
    target: LinkTarget,
    template: ClientLinkTemplate,
}

/// Typed startup failures retain address syntax, DNS and operator-input causes.
#[derive(Debug)]
enum LinkSetupError {
    Lever(harness::LeverError),
    Server {
        target: String,
        origin: &'static str,
        cause: EndpointError,
    },
}

impl From<harness::LeverError> for LinkSetupError {
    fn from(error: harness::LeverError) -> Self {
        Self::Lever(error)
    }
}

impl std::fmt::Display for LinkSetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lever(error) => write!(f, "{error}"),
            Self::Server {
                target,
                origin,
                cause,
            } => write!(
                f,
                "cannot resolve target server {target:?} from {origin}: {cause}"
            ),
        }
    }
}
impl std::error::Error for LinkSetupError {}

#[derive(Debug)]
enum EndpointError {
    Empty,
    EmptyHost,
    InvalidPort(String),
    InvalidAddress,
    Resolution(io::Error),
}

impl std::fmt::Display for EndpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("address is empty"),
            Self::EmptyHost => f.write_str("hostname is empty"),
            Self::InvalidPort(port) => write!(f, "invalid port {port:?}"),
            Self::InvalidAddress => f.write_str("invalid socket address or hostname"),
            Self::Resolution(error) => write!(f, "{error}"),
        }
    }
}
impl std::error::Error for EndpointError {}

/// Reads `OVERMATCH_SERVER`, `OVERMATCH_API` and the `SPIKE_*` receive-conditioning
/// knobs. A direct target that does not resolve is the error: the client has nowhere to connect.
fn configure_link(simulate: bool, tick_duration: Duration) -> Result<LinkSetup, LinkSetupError> {
    // A direct target is resolved once at startup. Raw socket addresses bypass DNS; hostnames
    // select their resolver's first IPv4 result because the UDP transport binds an IPv4 wildcard
    // (`ClientLinkTemplate::bundle`). The scripted harness has no menu to click, so it always
    // connects directly.
    let environment = harness::env_text("OVERMATCH_SERVER")?;
    let api = harness::env_text("OVERMATCH_API")?;
    let mode = if simulate {
        ClientMode::Direct(choose_server(environment.as_deref()))
    } else {
        choose_mode(environment.as_deref(), api.as_deref(), DEFAULT_API)
    };
    let target = match mode {
        ClientMode::Direct(selection) => {
            let server_addr = parse_server_endpoint(selection.raw)
                .and_then(|endpoint| {
                    resolve_server_endpoint(&endpoint).map_err(EndpointError::Resolution)
                })
                .map_err(|cause| LinkSetupError::Server {
                    target: selection.raw.to_owned(),
                    origin: selection.source.label(),
                    cause,
                })?;
            LinkTarget::Direct {
                server_addr,
                source: selection.source,
            }
        }
        ClientMode::Matchmaking { api, source } => LinkTarget::Matchmaking {
            api: api.to_owned(),
            source,
        },
    };
    // Random u64, never PID: netcode permits duplicate ids, which overwrite `PeerId → Entity`
    // and misroute raw-id ownership (`ControlledBy` / `NetworkingMetadata::peer_map`), yielding
    // turret desync, input contention or one client driving both tanks. OS-seeded `RandomState`
    // supplies a fresh id per launch without another dependency; it stays fixed for the session
    // and avoids deterministic collisions with a previous session's disconnect timeout.
    let client_id = std::hash::RandomState::new().build_hasher().finish();
    // Operator diagnostics (`SPIKE_LATENCY_MS` / `SPIKE_JITTER_MS`), off by default so real
    // packets and HUD Ping take no artificial delay.
    let latency_ms: u64 = harness::env_parse("SPIKE_LATENCY_MS")?.unwrap_or(0);
    let jitter_ms: u64 = harness::env_parse("SPIKE_JITTER_MS")?.unwrap_or(0);
    let jitter_seed: Option<u64> = harness::env_parse("SPIKE_JITTER_SEED")?;
    // Both burst knobs arm it; MEASURED field-link reproduction: `60 / 33`.
    let burst_ms: u64 = harness::env_parse("SPIKE_BURST_MS")?.unwrap_or(0);
    let burst_period_ticks: u32 = harness::env_parse("SPIKE_BURST_PERIOD_TICKS")?.unwrap_or(0);
    let burst = harness::Burst::parse(burst_ms, burst_period_ticks, tick_duration)?;
    // Lightyear's stock conditioner stores milliseconds and adds jitter in i32. Check before
    // its narrowing conversion so operator input cannot wrap or overflow inside the dependency.
    if latency_ms > i32::MAX as u64 || jitter_ms > i32::MAX as u64 - latency_ms {
        return Err(harness::LeverError::invalid(
            "SPIKE_LATENCY_MS / SPIKE_JITTER_MS",
            format!("{latency_ms}/{jitter_ms}"),
            "a sum no larger than i32::MAX milliseconds (Lightyear's representation)",
        )
        .into());
    }
    let (conditioner, seeded) = match recv_conditioner_mode(
        latency_ms,
        jitter_ms,
        jitter_seed,
        burst.is_some(),
    ) {
        RecvConditionerMode::Stock => {
            info!(
                "client: RecvLinkConditioner ON — latency={latency_ms}ms jitter={jitter_ms}ms (SPIKE_*)"
            );
            let stock = LinkConditionerConfig::default()
                .with_incoming_latency(Duration::from_millis(latency_ms))
                .with_incoming_jitter(Duration::from_millis(jitter_ms))
                .with_fixed_loss(0.0);
            (Some(stock), None)
        }
        RecvConditionerMode::Seeded(seed) => {
            info!(
                "client: SeededRecvConditioner ON — latency={latency_ms}ms jitter={jitter_ms}ms \
                 seed={seed} burst={burst_ms}ms/{burst_period_ticks}t (SPIKE_*)"
            );
            let seeded = SeededLink {
                seed,
                latency_ms,
                jitter_ms,
                burst,
            };
            (None, Some(seeded))
        }
        RecvConditionerMode::Off => (None, None),
    };
    Ok(LinkSetup {
        target,
        template: ClientLinkTemplate {
            client_id,
            conditioner,
            seeded,
        },
    })
}

pub fn run() -> AppExit {
    client_policy();
    let script = harness::script();
    let simulate = script.is_some();
    // A scripted client is GPU-less unless the run asks for a window; a player's always has one.
    let window = RunWindow::of_run(if simulate {
        RunWindow::Headless
    } else {
        RunWindow::Visible
    });
    let headless = window == RunWindow::Headless;
    if !simulate && headless {
        harness::refuse_start::<()>(crate::run_window::WindowModeError::HeadlessWindowedRoot);
    }

    let mut app = App::new();
    if headless {
        // Resolution: 2 ms polling resolves network arrivals within the fixed gameplay tick.
        app.add_plugins(crate::gpu_less_default_plugins(None))
            .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_millis(2)));
        // After `LogPlugin` has built, like `run_window::mount`'s line on a windowed root.
        info!("window: {}", window.name());
        app.insert_resource(harness::SimulateInput::new(
            script.expect("a headless client runs a script"),
        ));
        // bevy_image 0.20.1 panics transcoding mipped UASTC KTX2 under `CompressedImageFormats::NONE`,
        // then required-view loading aborts (MEASURED with a packaged client). `headless_test`
        // owns the mechanism and retirement tripwire for this shared GPU-less workaround.
        app.insert_resource(bevy::image::CompressedImageFormatSupport(
            bevy::image::CompressedImageFormats::ASTC_LDR,
        ));
    } else {
        // `load_at_boot` inserts settings and the boot report before window construction.
        let settings = crate::settings::load_at_boot(&mut app);
        // `windowed_runtime` reads the same mode, defaulting to visible for unscripted runs.
        crate::windowed_runtime(
            &mut app,
            Window {
                // `Unprobed` on purpose: the capability probe needs a surface, which needs
                // this window — the mapping self-negotiates until the probe lands.
                present_mode: settings.present_mode(crate::settings::PresentCaps::Unprobed),
                // Apply persisted fullscreen at creation to avoid a window flash.
                // `at_window_creation` avoids Bevy caching an unresolved indexed display and
                // blocking Startup correction (`settings::DisplaySelection`). Hidden runs
                // stay plain windows (`run_window`).
                mode: settings
                    .window_mode
                    .to_window_mode(settings.display.at_window_creation()),
                ..default()
            },
        );
        // `WinitSettings::game()` throttles unfocused windows to 60 Hz, below the 64 Hz tick;
        // clients drift behind and resync on refocus (lightyear #1113's jitter class). Hidden
        // clients also use `continuous()`: MEASURED warm-cache four-run catch-up p50 7–8 ticks,
        // fact lag 7, matching visible clients. Cost: free-running CPU, since an invisible
        // surface's `SurfaceError::Occluded` skips the vsync block.
        //
        // Rejected as unproven: `UpdateMode::reactive(1/120)`. MEASURED 78 of ~980 fire_rx
        // events, catch-up p50 34 (baseline 8), max 65 (~13), fact lag p50 27 (7). Confound:
        // cold Metal shader cache, independently observed to stall >10 s compiling pipelines
        // after connect, lose keepalive and reconnect repeatedly; warm reruns were clean.
        // `continuous()` is the measured-good smaller delta; any reactive A/B needs a warm cache.
        app.insert_resource(bevy::winit::WinitSettings::continuous());
        if let Some(script) = script {
            app.insert_resource(harness::SimulateInput::new(script));
        }
    }

    // Register the shared protocol before creating the client link. The step lightyear publishes as
    // `TickDuration`, also the simulation-time unit for the view clock's pacing bound.
    let tick_duration = crate::net::server::TICK;
    // Transport labels free-run; arrival input and presentation never follow their estimate.
    app.add_plugins(ClientPlugins { tick_duration });
    app.add_plugins(super::plugin);
    app.add_plugins(physics::physics_plugins());
    // Shared server/offline systems drive cosmetic shells, servos and recoil here too.
    app.add_plugins(SimPlugin);
    // Server-authoritative combat: replicas fly shells cosmetically but never deposit HP or apply
    // hit impulse. Private crew detail and public life state arrive independently from the server;
    // the client never runs `DamageConsequences` to decide either one.
    app.insert_resource(crate::ClientReplica);
    // Windowed devices snapshot before the fixed loop and send once after it. Scripts own the
    // frame input in headless and windowed automation alike.
    if !headless {
        app.add_plugins(NetClientPlugin);
    }
    if !simulate {
        // Documentation frames to disk (the Join menu, a connect screen); exits once they are.
        // Inert unless a capture plan takes frames.
        app.add_plugins(crate::dev_capture::CapturePlugin(
            crate::dev_capture::Anchor::Startup,
        ));
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

    let LinkSetup { target, template } = match configure_link(simulate, tick_duration) {
        Ok(link) => link,
        Err(error) => harness::refuse_start(error),
    };
    if template.seeded.is_some() {
        app.add_systems(
            PreUpdate,
            harness::apply_seeded_recv_conditioner
                .in_set(lightyear::link::LinkReceiveSystems::ApplyConditioner),
        );
    }
    super::view_clock::install(&mut app);
    super::frontier::install(&mut app);
    // The connection state machine: gate the FIRST connect on the tank assets, then auto-retry a
    // failed/dropped connection on a short backoff. See [`drive_connection`] for the full states.
    install_connection_lifecycle(&mut app);
    let client_id = template.client_id;
    match target {
        LinkTarget::Direct {
            server_addr,
            source,
        } => {
            // A direct link exists from startup, with the all-zero dev key the direct-connect
            // servers hold; its endpoint is fixed for the session, so it is logged once here.
            info!(
                "client: target server {server_addr} (source: {}), client_id={client_id}",
                source.label()
            );
            install_direct_link(&mut app, &template, server_addr);
            if !headless {
                install_direct_connect_screen(&mut app);
            }
        }
        LinkTarget::Matchmaking { api, source } => {
            // No link until a ticket is ready: the Join menu owns the screen until then.
            info!(
                "client: matchmaking via {api} (source: {}), client_id={client_id}",
                source.label()
            );
            install_matchmaking_link(&mut app, &api);
            if !headless {
                app.add_plugins(super::matchmaking::leave_ui_plugin);
            }
        }
    }
    app.insert_resource(template);
    app.add_systems(Startup, load_tank_assets);
    // Connect screens share `NetClientPlugin`'s window gate; headless automation observes
    // `drive_connection` logs instead.
    if !headless {
        app.add_plugins(connect_screen_plugin);
        install_screen_flow(&mut app);
    }

    install_replica_consumers(&mut app);
    if simulate {
        install_script_watchdog(&mut app);
        app.add_systems(FixedPreUpdate, harness::buffer_input);
    } else {
        install_device_feed(&mut app);
    }

    // `NetClientPlugin` mounts the windowed overlay authority: Esc/focus presence →
    // `overlay::input_blocked` → sole `cursor_owner` and `PlayerInputSet`'s locked-cursor gate,
    // including initial grab. Headless scripts have no overlay authority; scripted clients
    // omit `feed_frame_input`, so overlay idling cannot fight `harness::buffer_input`.

    app.run()
}

/// Latch the presented tank before deriving this frame's screen, after connection state and
/// before any owner declares an overlay from it. Requires `flow::plugin` and `ConnectRetry`.
fn install_screen_flow(app: &mut App) {
    app.add_systems(
        Update,
        (latch_tank_seen, derive_flow)
            .chain()
            .after(drive_connection)
            .before(crate::overlay::OverlaySet::Declare),
    );
}

/// The game's netcode and damage-model presentation: the common replica consumer schedule,
/// shared by the client and socket-free stream-presence proof. Requires protocol, physics and sim;
/// link setup, the view clock and input authorship stay with the composition root. Keeping the mount shared makes a new consumer part of the replay too.
pub(super) fn install_replica_consumers(app: &mut App) {
    app.add_observer(diagnostics::log_connected)
        .add_observer(reset_shot_receive_state)
        .add_observer(retire_link_shells)
        .add_plugins(control_claim_plugin)
        .add_systems(
            Update,
            (
                rig::attach_replicated_rig().run_if(|view: Res<ViewClock>| view.now().is_some()),
                rig::present_streamed_aim(),
                diagnostics::nan_tripwire,
                open_gameplay_gate,
            ),
        );
    install_fire_ingress(app);
    // The pen cam's explanations addressed to this player, and whether each impact is drawn.
    super::shot_explanation::install_client(app);
    // Select the authority world before consuming its common construction stream.
    super::world_reset::install_client(app);
    // The debris stress lever's fragments and the constructions' fallen pieces, drawn where the
    // server's carriers say they are.
    super::debris_stress::install_client(app);
    // The construction ledger into the record the constructions derive from.
    super::construction::install_client(app);
    app.add_systems(
        FixedUpdate,
        apply_pending_recoil_kicks
            .before(GameplaySet)
            // Match `shooting::apply_recoil`'s Playing gate or kicks could accumulate unreleased.
            .run_if(in_state(AppState::Playing)),
    );
    // Log the arrived role unconditionally: captures must identify the own hull's clock.
    app.add_systems(Update, log_local_tank_role);
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct FireIngress;

/// The game's damage model: expire facts already retained over view progress, then admit this
/// frame's arrivals. New facts cannot inherit time before their receipt, including across a cut.
/// Ordered ingress also gives the shared shot trace one declared append order. Path presentation
/// consumes the resulting buffer in PostUpdate.
pub(super) fn install_fire_ingress(app: &mut App) {
    app.init_resource::<PendingRecoilKicks>()
        .init_resource::<SeenShots>()
        .init_resource::<HeldFireEvents>()
        .init_resource::<PendingFireEvents>()
        .init_resource::<SeenDamage>()
        .init_resource::<SanctionedShots>()
        .init_resource::<crate::ballistics::PathClock>()
        .add_systems(Update, age_sanctioned_shots.before(FireIngress))
        .add_systems(
            Update,
            (receive_fire_events, receive_damage_confirms)
                .chain()
                .in_set(FireIngress),
        );
}

/// State for the connect/reconnect driver ([`drive_connection`]). Lives across every mode (the
/// headless harness drives it too), so it is init'd unconditionally.
#[derive(Resource, Default)]
struct ConnectRetry {
    /// First `Connect` triggered after assets load. Spawn-time `Disconnected` is pre-connect
    /// rest, not a failed attempt to retry.
    initiated: bool,
    /// We reached `Connected` at least once on this link. Distinguishes "never connected yet" (the
    /// joining screen) from "was in game, lost the link" (the match with the reconnect overlay).
    connected_once: bool,
    /// Connect attempts that have already failed and been retried — drives the `(retry N)` suffix
    /// and the backoff schedule. Reset to 0 on a successful `Connected`.
    attempts: u32,
    /// Next retry deadline (`Time<Real>::elapsed_secs_f64`); armed on observed failure, cleared
    /// on retry. All ledger times are real: virtual time pauses/scales/clamps long frames,
    /// while give-up must compare elapsed wall time.
    next_retry_at: Option<f64>,
    /// When a once-connected link was first seen down; cleared on `Connected`.
    down_since: Option<f64>,
    /// When the last retry `Connect` fired.
    last_attempt_at: Option<f64>,
    /// A retry fired past the [`client_policy`] reconnect budget has failed. A hitch still
    /// permits an attempt on resume; `Connected` clears exhaustion.
    exhausted: bool,
    /// This link's match has presented the client's own tank at least once
    /// ([`latch_tank_seen`]): the loading screen gives way to the match.
    tank_seen: bool,
}

/// Retry growth and cap are required client policy data.
fn reconnect_backoff_secs(attempts: u32) -> f64 {
    let policy = client_policy();
    (policy.reconnect_base_s.0 + policy.reconnect_step_s.0 * f64::from(attempts))
        .min(policy.reconnect_cap_s.0)
}

/// Observe live success and authenticated denial before terminal handling: a successful retry
/// clears its failed-attempt ledger, and a denial outranks scripted completion in the same frame.
fn observe_connection(
    links: Query<(&NetcodeClient, Option<&StatedRecord>, Has<Connected>)>,
    mut retry: ResMut<ConnectRetry>,
    mut gone: MessageWriter<super::matchmaking::ServerGone>,
) {
    let (netcode, stated, connected) = match links.single() {
        Ok(link) => link,
        // No link before matchmaking, or after a terminal close.
        Err(QuerySingleError::NoEntities(_)) => return,
        Err(error @ QuerySingleError::MultipleEntities(_)) => {
            panic!(
                "at most one client link exists (`spawn_matched_link` refuses a second): {error}"
            )
        }
    };
    if let Some(reason) = netcode.inner.denied_reason() {
        let stated = stated.expect("every client link states its record (`netcode_link`)");
        let refusal = JoinRefusal::from_netcode(reason, &stated.0);
        warn!("client: refused by the server: {}", refusal.message());
        gone.write(super::matchmaking::ServerGone::Admission(refusal));
        return;
    }
    if connected {
        retry.connected_once = true;
        retry.attempts = 0;
        retry.next_retry_at = None;
        retry.down_since = None;
        retry.last_attempt_at = None;
        retry.exhausted = false;
    }
}

/// Frame-rate first-connect/retry driver over the single link's markers.
///
/// * `Connected`: [`observe_connection`] has reset the ledger; leave the healthy link alone.
/// * Uninitiated: await tank assets so the replicated root assembles whole from extracted data
///   with view pop-in of CLAIMED ~one frame. `SimPlugin` / `world::plugin` supplies real terrain
///   on both peers for track probes; no local ground spawn.
/// * `Connecting`: await netcode's SendingConnectionRequest/ChallengeResponse.
/// * Authenticated denial: the lifecycle closes the link before retries.
/// * Failed/disconnected: back off, then trigger `Connect` on the same link/client id. A transient
///   disconnect can remain `Linked`; UDP `LinkStart` then skips it, reusing the source socket.
fn drive_connection(
    real: Res<Time<Real>>,
    assets: Res<PendingTankAssets>,
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
    let (entity, connected, connecting, disconnected) = match client.single() {
        Ok(link) => link,
        // No link to drive: the Join menu before a ticket, or the one link is being left.
        Err(QuerySingleError::NoEntities(_)) => return,
        Err(error @ QuerySingleError::MultipleEntities(_)) => {
            panic!(
                "at most one client link exists (`spawn_matched_link` refuses a second): {error}"
            )
        }
    };

    let now = real.elapsed_secs_f64();
    if connected {
        return;
    }
    if retry.connected_once && retry.down_since.is_none() {
        retry.down_since = Some(now);
    }

    if !retry.initiated {
        if !assets.loaded(&asset_server) {
            return;
        }
        retry.initiated = true;
        commands.trigger(Connect { entity });
        info!("client: tank assets loaded — connecting");
        return;
    }

    if connecting {
        return;
    }

    if let (Some(down), Some(attempted)) = (retry.down_since, retry.last_attempt_at)
        && attempted - down >= client_policy().reconnect_give_up_s.0
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
            // Dev breadcrumb only; the player is told once `client_policy`'s
            // `unanswered_after_attempts` is reached.
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

/// A headless adapter for the shipped lifecycle and its rendered connection label.
#[cfg(test)]
pub(super) fn mount_refusal_probe(app: &mut App) {
    // The probe starts initiated, so the asset gate is never read: an unloaded handle stands in.
    app.insert_resource(PendingTankAssets {
        spec: Handle::default(),
        scene: None,
        effects: Vec::new(),
    });
    install_connection_lifecycle(app);
    app.add_systems(Update, update_joining_screen.after(drive_connection));
    app.world_mut().resource_mut::<ConnectRetry>().initiated = true;
    app.world_mut().spawn((Text::default(), JoiningText));
}

#[cfg(test)]
pub(super) fn refusal_probe(app: &mut App) -> (Option<super::matchmaking::ServerGone>, String) {
    let refusal = app.world().resource::<ClosedConnection>().cause.clone();
    let label = app
        .world_mut()
        .query_filtered::<&Text, With<JoiningText>>()
        .single(app.world())
        .expect("the joining probe has one label")
        .0
        .clone();
    (refusal, label)
}

/// Latch [`ConnectRetry::tank_seen`] once the own tank is presented: its view has bound
/// (`TankViewReady`) and a combat or results snapshot has arrived.
/// `tank::view::bind_tank_view` binds only a root carrying `SimParts`, which
/// the body assembly inserts with the local `Tank` after the rig has attached to the replicated
/// `Position` and `Rotation`, so the bound view implies the pose and the complete body.
fn latch_tank_seen(
    own: Query<
        (),
        (
            With<GameControlled>,
            With<NetTank>,
            With<crate::tank::TankViewReady>,
        ),
    >,
    battles: Query<&super::battle::BattleState>,
    mut retry: ResMut<ConnectRetry>,
) {
    // A tank may precede the combat snapshot. Keep loading rather than exposing an empty phase.
    let playable = match battles.single() {
        Ok(battle) => !matches!(battle.phase, super::battle::BattlePhase::WaitingForPlayer),
        Err(bevy::ecs::query::QuerySingleError::NoEntities(_)) => false,
        Err(bevy::ecs::query::QuerySingleError::MultipleEntities(_)) => {
            panic!("a client replicates exactly one Battle before presenting its own tank")
        }
    };
    if retry.connected_once && !retry.tank_seen && !own.is_empty() && playable {
        retry.tank_seen = true;
    }
}

/// The screen this client is on, from its link (`flow::flow_of`): no link is the Join menu, a link
/// whose match has not yet shown this client's tank is the loading screen, and one that has is the
/// match.
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
            tank_seen: retry.tank_seen,
        },
    );
}

/// Matchmaking mode: spawn the client link for a ready match, with that match's key, and start the
/// connection state machine afresh for it. At most one link exists: a ready match arriving while
/// one does is dropped.
fn spawn_matched_link(
    mut ready: MessageReader<super::matchmaking::MatchReady>,
    template: Res<ClientLinkTemplate>,
    mut terminal: ResMut<ClosedConnection>,
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
    terminal.cause = None;
    terminal.connected_once = false;
}

/// The connection driver and terminal handling shared by every client composition.
pub(super) fn install_connection_lifecycle(app: &mut App) {
    app.init_resource::<ConnectRetry>()
        .init_resource::<ClosedConnection>()
        .add_message::<super::matchmaking::ServerGone>()
        .add_message::<super::matchmaking::MatchClosed>()
        // Refusals remove the link before the driver can issue another Connect.
        .add_systems(
            Update,
            (
                observe_connection,
                close_lost_match.in_set(RefusalHandling),
                drive_connection,
            )
                .chain(),
        );
}

/// Update producers finish admission before terminal handling can remove the link.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct RefusalHandling;

/// Script exit observes terminal handling before it can report success or a timeout.
pub(super) fn install_script_watchdog(app: &mut App) {
    app.add_systems(Update, harness::simulate_watchdog.after(close_lost_match));
}

/// None means no terminal failure: before connection, while healthy, or after a voluntary leave.
/// A cause survives message expiry and retry-ledger reset until the player joins another match.
#[derive(Resource, Default)]
pub(super) struct ClosedConnection {
    pub(super) cause: Option<super::matchmaking::ServerGone>,
    /// Selects the lobby's failed-join or lost-server wording after the retry ledger is cleared.
    connected_once: bool,
}

/// A fixed endpoint uses the direct-connect server's development key.
fn install_direct_link(app: &mut App, template: &ClientLinkTemplate, server_addr: SocketAddr) {
    let mut link = app.world_mut().spawn(template.bundle(server_addr, [0; 32]));
    if let Some(seeded) = template.seeded() {
        link.insert(seeded);
    }
}

/// A direct window has no lobby card; its no-link screen keeps the terminal cause visible.
/// Requires the connection lifecycle, `flow::plugin` and the shared connect screen.
fn install_direct_connect_screen(app: &mut App) {
    app.add_systems(OnEnter(crate::flow::Flow::Lobby), spawn_joining_screen);
}

/// Matchmaking's tickets and voluntary exits. Requires [`install_connection_lifecycle`] for
/// terminal failures; [`run`] and the hosted end-to-end test share both mounts.
pub(super) fn install_matchmaking_link(app: &mut App, api: &str) {
    super::matchmaking::install(app, api);
    app.add_systems(
        Update,
        (
            spawn_matched_link.before(observe_connection),
            close_exhausted_match
                .after(observe_connection)
                .before(close_lost_match),
            start_leaving.before(close_lost_match),
            finish_leaving.after(close_lost_match),
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

/// Only matchmade links give up on silence; a direct endpoint keeps retrying indefinitely.
fn close_exhausted_match(
    retry: Res<ConnectRetry>,
    mut gone: MessageWriter<super::matchmaking::ServerGone>,
) {
    use super::matchmaking::ServerGone;
    if !retry.connected_once {
        if retry.attempts >= client_policy().unanswered_after_attempts.0 {
            gone.write(ServerGone::Unreachable);
        }
    } else if retry.exhausted {
        gone.write(ServerGone::StoppedAnswering);
    }
}

/// Keep the first typed cause even without a link. A voluntary leave owns its disconnect. The link's
/// removal prevents reconnects; MatchClosed is the matchmaking lobby's existing display channel.
fn close_lost_match(
    mut retry: ResMut<ConnectRetry>,
    mut gone: MessageReader<super::matchmaking::ServerGone>,
    mut terminal: ResMut<ClosedConnection>,
    links: Query<Entity, (With<NetcodeClient>, Without<Leaving>)>,
    leaving: Query<(), With<Leaving>>,
    mut closed: MessageWriter<super::matchmaking::MatchClosed>,
    mut commands: Commands,
) {
    let first = gone.read().next().cloned();
    gone.clear();
    if terminal.cause.is_some() || !leaving.is_empty() {
        return;
    }
    let Some(cause) = first else {
        return;
    };
    match links.single() {
        Ok(link) => commands.entity(link).despawn(),
        // A decoded refusal can arrive after the link has gone; its cause must still be shown.
        Err(QuerySingleError::NoEntities(_)) => {}
        Err(error @ QuerySingleError::MultipleEntities(_)) => {
            panic!(
                "at most one client link exists (`spawn_matched_link` refuses a second): {error}"
            )
        }
    };
    warn!(
        "client: closing the link after {} attempts: {cause}",
        retry.attempts
    );
    let reason = cause.to_string();
    let verdict = if retry.connected_once {
        super::matchmaking::MatchClosed::Ended(reason)
    } else {
        super::matchmaking::MatchClosed::Unreachable(reason)
    };
    terminal.cause = Some(cause);
    terminal.connected_once = retry.connected_once;
    *retry = ConnectRetry::default();
    closed.write(verdict);
}

/// Requires the connection lifecycle's retry ledger and terminal cause. In a match, link loss declares
/// the reconnect overlay in `OverlaySet::Declare`, before the cursor owner derives input licensing.
pub(super) fn connect_screen_plugin(app: &mut App) {
    use crate::flow::{Flow, InMatchSystems};
    crate::overlay::look::get();
    app.add_systems(Startup, spawn_connect_status)
        .add_systems(OnEnter(Flow::Joining), spawn_joining_screen)
        .add_systems(
            Update,
            (
                update_connect_status
                    .in_set(crate::overlay::OverlaySet::Declare)
                    .in_set(InMatchSystems),
                update_joining_screen
                    .run_if(in_state(Flow::Joining).or_else(in_state(Flow::Lobby))),
            ),
        );
}

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
        RECONNECT_LABEL,
        (),
        Some(crate::overlay::look::color(
            crate::overlay::look::get().connect_scrim_srgba,
        )),
    );
}

/// The connect label also owns a direct client's terminal screen; both cover the world.
fn spawn_joining_screen(
    mut commands: Commands,
    fonts: Res<UiFonts>,
    retry: Res<ConnectRetry>,
    terminal: Res<ClosedConnection>,
    flow: Res<State<crate::flow::Flow>>,
) {
    crate::ui_font::spawn_overlay(
        &mut commands,
        &fonts.hud,
        crate::flow::FlowScoped(*flow.get()),
        connection_label(&retry, &terminal),
        JoiningText,
        Some(crate::overlay::look::color(
            crate::overlay::look::get().joining_scrim_srgba,
        )),
    );
}

/// Loading assets or the own tank, reconnecting, or waiting for an initial server response.
fn screen_label(retry: &ConnectRetry) -> String {
    if !retry.initiated {
        "LOADING…".to_owned()
    } else if retry.connected_once && retry.down_since.is_some() {
        RECONNECT_LABEL.to_owned()
    } else if retry.connected_once {
        "LOADING…".to_owned()
    } else {
        joining_label(retry.attempts)
    }
}

/// The joining screen's label after `attempts` unanswered connects. A server of another build
/// answers with its cause ([`JoinRefusal::Incompatible`]), so past the [`client_policy`] threshold
/// the silence is the server's: down, unreachable, or holding another key.
fn joining_label(attempts: u32) -> String {
    if attempts >= client_policy().unanswered_after_attempts.0 {
        "CAN'T CONNECT — the server is not answering\n(check that it is running and reachable)"
            .to_string()
    } else if attempts == 0 {
        "CONNECTING…".to_string()
    } else {
        format!("CONNECTING… (retry {attempts})")
    }
}

/// Terminal causes use the same label as connection progress, with network text sanitized.
fn connection_label(retry: &ConnectRetry, terminal: &ClosedConnection) -> String {
    match &terminal.cause {
        Some(cause) => super::matchmaking::connection_failure_label(
            &cause.to_string(),
            terminal.connected_once,
        ),
        None => screen_label(retry),
    }
}

fn update_joining_screen(
    retry: Res<ConnectRetry>,
    terminal: Res<ClosedConnection>,
    mut text: Query<&mut Text, With<JoiningText>>,
) {
    let label = connection_label(&retry, &terminal);
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
fn parse_server_endpoint(raw: &str) -> Result<ServerEndpoint, EndpointError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(EndpointError::Empty);
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
                .map_err(|_| EndpointError::InvalidPort(port.to_owned()))?;
            (name, port)
        }
        Some(_) => return Err(EndpointError::InvalidAddress),
        None => (raw, SERVER_PORT),
    };
    if name.is_empty() {
        return Err(EndpointError::EmptyHost);
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

/// Player-facing ownership waits for the complete body. `Rig` and `TankViews` enter
/// together in `tank::template::TankTemplate::assemble`, after the body's children are assembled.
fn control_claim_plugin(app: &mut App) {
    app.add_observer(claim_complete_tank);
}

fn claim_complete_tank(
    add: On<Add<(NetControlled, Rig)>>,
    complete: Query<(), (With<NetControlled>, With<Rig>)>,
    mut commands: Commands,
) {
    // Either ownership or the body may arrive first. Until both exist, there is no local tank.
    if complete.contains(add.entity) {
        commands.entity(add.entity).insert(GameControlled);
    }
}

/// Name the replication role the server gave this client's own tank, the first frame that role is
/// observable: the interpolated server stream on the view clock. Latches on
/// the first resolved role, so a respawn does not re-log a decision that cannot change mid-session.
fn log_local_tank_role(
    tank: Query<Entity, (With<GameControlled>, With<NetTank>, With<Interpolated>)>,
    mut logged: Local<bool>,
) {
    if *logged {
        return;
    }
    let entity = match tank.single() {
        Ok(entity) => entity,
        // No own tank yet: before the first spawn.
        Err(QuerySingleError::NoEntities(_)) => return,
        Err(error @ QuerySingleError::MultipleEntities(_)) => {
            panic!("a client controls at most one tank (the server grants one input slot): {error}")
        }
    };
    *logged = true;
    info!("client: own tank {entity} drives INTERPOLATED (server stream)");
}

/// `(shooter root, weapon slot)` causes from render-rate [`receive_fire_events`] to
/// fixed-clock [`apply_pending_recoil_kicks`]. Those systems own the drain/write clock constraints.
#[derive(Resource, Default)]
pub(super) struct PendingRecoilKicks(Vec<(Entity, usize)>);

impl PendingRecoilKicks {
    pub(super) fn push(&mut self, shooter: Entity, weapon: usize) {
        self.0.push((shooter, weapon));
    }

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
    // Resolution: memory for 30 combatants x 2 weapons x ceil(750/60 rounds/s x
    // 100/64 s catch-up) = 1,200 ids (DERIVED sizing example, not a vehicle law).
    // The next power of two covers that horizon; overflow retires the oldest id.
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

/// The game's damage model: a fire parsed at receipt has a finite origin and a unit bore.
/// Every later queue holds this type, so presentation never re-checks the wire's raw vectors.
#[derive(Clone)]
pub(super) struct ReceivedFire {
    event: FireEvent,
    direction: Dir3,
}

impl ReceivedFire {
    fn parse(event: &FireEvent) -> Result<Self, FireRefusal> {
        if !event.origin.is_finite() {
            return Err(FireRefusal::Origin);
        }
        if !event.birth.is_finite() {
            return Err(FireRefusal::Birth);
        }
        let direction = Dir3::new(event.direction).map_err(|_| FireRefusal::Bore)?;
        Ok(Self {
            event: event.clone(),
            direction,
        })
    }

    fn shot_id(&self) -> ShotId {
        self.event.shot_id()
    }
}

/// The game's damage model: handshake-matched tank data fixes weapons, rounds and fuze modes
/// (`protocol::protocol_id`). These are malformed outside facts: logged/dropped, never deferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FireRefusal {
    /// The origin is not finite.
    Origin,
    /// The game's damage model: the received birth is not finite.
    Birth,
    /// The bore has no finite nonzero direction.
    Bore,
    /// The shooter's assembled rig has no weapon in this slot.
    WeaponSlot { weapon: u8 },
    /// The slot's weapon lists no round at this index.
    Round { weapon: u8, round: u8 },
    /// The round's fuze lists no mode at this index.
    FuzeMode {
        weapon: u8,
        round: u8,
        fuze_mode: u8,
    },
}

/// Log and trace one refused fire fact.
fn refuse_fire(
    shot: ShotId,
    refusal: FireRefusal,
    now: impl Into<Option<Tick>>,
    shot_trace: &mut Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    warn!(?shot, ?refusal, "client: refused an authority fire fact");
    crate::shot_trace::record(
        shot_trace,
        "drop",
        now.into().map(|t| t.0),
        shot,
        || json!({ "s": "fire", "res": "refused", "why": format!("{refusal:?}") }),
    );
}

/// A fire visual that arrived before its replica root. Facts are keyed by stable [`ShotId`], never
/// by the transient entity mapping which may be absent or stale during replication catch-up.
#[derive(Clone)]
struct PendingFire {
    fire: ReceivedFire,
    received_tick: Tick,
}

/// Bounded, expiring fire visuals awaiting a unique live shooter root.
#[derive(Resource)]
pub(super) struct PendingFireEvents {
    ttl_ticks: i32,
    order: std::collections::VecDeque<ShotId>,
    by_shot: bevy::platform::collections::HashMap<ShotId, PendingFire>,
}

impl FromWorld for PendingFireEvents {
    fn from_world(world: &mut World) -> Self {
        world.init_resource::<super::timing::Timing>();
        let horizon = world.resource::<super::timing::Timing>().view_pending_s;
        Self {
            ttl_ticks: (horizon / crate::sim_tick::sim_timestep().as_secs_f64()).ceil() as i32,
            order: default(),
            by_shot: default(),
        }
    }
}

impl PendingFireEvents {
    // Resolution: 256 exceeds a 30-combatant x 2-weapon volley (60 fires, DERIVED),
    // below the 2,048-id dedup allocation. Overflow visibly omits the oldest cosmetic.
    const CAP: usize = 256;

    /// Insert one unresolved fire. The return value is an older event evicted for bounded memory.
    fn insert(&mut self, fire: ReceivedFire, received_tick: Tick) -> Option<PendingFire> {
        let shot = fire.shot_id();
        if self.by_shot.contains_key(&shot) {
            return None;
        }
        self.order.push_back(shot);
        self.by_shot.insert(
            shot,
            PendingFire {
                fire,
                received_tick,
            },
        );
        (self.order.len() > Self::CAP)
            .then(|| self.order.pop_front())
            .flatten()
            .and_then(|old| self.by_shot.remove(&old))
    }

    fn pop_front(&mut self) -> Option<PendingFire> {
        self.order.pop_front().map(|shot| {
            self.by_shot
                .remove(&shot)
                .expect("each pending queue id has exactly one fire in the pending map")
        })
    }

    fn requeue(&mut self, pending: PendingFire) {
        let shot = pending.fire.shot_id();
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
pub(super) struct HeldFireEvents(CursorQueue<ReceivedFire>);

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

/// The game's damage model: this composition has one server link, and its cosmetic paths
/// cannot receive a terminal after that link ends. Retire those shells with the clock's scope.
fn retire_link_shells(
    _ended: On<Remove<Connected>>,
    shells: Query<Entity, With<crate::ballistics::Projectile>>,
    mut commands: Commands,
) {
    for shell in &shells {
        // A received path end can already have queued retirement in this same command flush.
        commands.entity(shell).try_despawn();
    }
}

/// A new transport connection starts a new receipt/presentation identity scope.
///
/// INVARIANT: Lightyear does not carry message-channel backlog across connections. Clearing these
/// client-only ledgers therefore cannot discard an in-flight fact from the new connection, and it
/// prevents a later server connection that reuses match-local ids from colliding with the previous
/// link. `ShotId` must remain unique for the connection, including across Battles.
fn reset_shot_receive_state(
    _connected: On<Add<Connected>>,
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
    fires.order.clear();
    fires.by_shot.clear();
    *damage = default();
    *sanctioned = default();
}

/// Public batch, fire and outcome receivers drained by [`receive_fire_events`].
#[derive(SystemParam)]
pub(super) struct FireInbox<'w, 's> {
    batch_receivers: Query<'w, 's, &'static mut MessageReceiver<FireVisualBatch>>,
    fire_receivers: Query<'w, 's, &'static mut MessageReceiver<FireEvent>>,
    outcome_receivers: Query<'w, 's, &'static mut MessageReceiver<FlightOutcome>>,
}

/// The game's damage-model presentation ledgers and path cursor, written by [`receive_fire_events`].
#[derive(SystemParam)]
pub(super) struct FireLedgers<'w> {
    pending: ResMut<'w, PendingRecoilKicks>,
    seen: ResMut<'w, SeenShots>,
    held: ResMut<'w, HeldFireEvents>,
    pending_fires: ResMut<'w, PendingFireEvents>,
    sanctioned: ResMut<'w, SanctionedShots>,
    path_clock: ResMut<'w, crate::ballistics::PathClock>,
}

/// The game's damage model: drain fire announcements and resolved public paths.
///
/// New fires are validated and held for the interpolation cursor ([`HeldFireEvents`]); the release
/// pass in this same drain spawns each one's cosmetic shell and enqueues its recoil when the cursor
/// crosses the fire tick (immediately, for an event the cursor already passed). Paths enter the
/// validated buffer at arrival and sample the same fractional cursor in `ballistics::path`. Damage confirmation is owner-private and handled by
/// [`receive_damage_confirms`].
///
/// This must run in `Update`: Lightyear clears undrained message receivers in `Last`, while a frame
/// can run no fixed steps. Draining in `FixedUpdate` would lose arrivals on such a frame. Only the
/// `TankSim` recoil write crosses to the fixed clock.
pub(super) fn receive_fire_events(
    inbox: FireInbox,
    shooters: FireShooters,
    // Waiting for the first completed stamp pair holds announcements without presenting them.
    cursor: Res<ViewClock>,
    ledgers: FireLedgers,
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
    mut commands: Commands,
) {
    let FireInbox {
        mut batch_receivers,
        mut fire_receivers,
        mut outcome_receivers,
    } = inbox;
    let FireLedgers {
        mut pending,
        mut seen,
        mut held,
        mut pending_fires,
        mut sanctioned,
        mut path_clock,
    } = ledgers;
    // Waiting for a completed stamp pair is a named absence, also null in trace rows.
    let now = cursor.now().map(|now| now.tick());
    let cursor = cursor
        .now()
        .map(|now| (now.tick(), f64::from(now.overstep().to_f32())));
    if let Some((tick, fraction)) = cursor {
        path_clock.tick = tick.0;
        path_clock.fraction = fraction;
    }
    for mut receiver in &mut batch_receivers {
        for batch in receiver.receive() {
            for fact in batch.facts {
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
            consume_fire_event(&event, now, cursor, &mut seen, &mut held, &mut shot_trace);
        }
    }
    for mut receiver in &mut outcome_receivers {
        for outcome in receiver.receive() {
            consume_flight_outcome(outcome, now, &mut sanctioned, &mut shot_trace);
        }
    }
    let Some(now) = now else {
        return;
    }; // No epoch yet: only receipt and holding can run.
    release_due_fire_events(
        now,
        cursor,
        &shooters,
        &mut pending,
        &mut held,
        &mut pending_fires,
        &mut shot_trace,
        &mut commands,
    );
    resolve_pending_fire_events(
        now,
        cursor,
        &shooters,
        &mut pending,
        &mut pending_fires,
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
    now: Option<Tick>,
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
        FireVisualFact::Outcome(outcome) => {
            consume_flight_outcome(outcome, now, sanctioned, shot_trace)
        }
    }
}

/// Dedup and validate one arriving fire, then hold it for the interpolation cursor. Presentation
/// happens at release ([`release_due_fire_events`], same drain), never here.
fn consume_fire_event(
    event: &FireEvent,
    now: Option<Tick>,
    cursor: Option<(Tick, f64)>,
    seen: &mut SeenShots,
    held: &mut HeldFireEvents,
    shot_trace: &mut Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    let shot = event.shot_id();
    let fresh = seen.mark_new(shot);
    crate::shot_trace::record(shot_trace, "fire_rx", now.map(|t| t.0), shot, || {
        json!({
            "dup": !fresh,
            "cu": now.and_then(|now| fire_catch_up_ticks(event.fire_tick, now)),
            "itick": cursor_itick(cursor),
        })
    });
    if !fresh {
        return;
    }
    let fire = match ReceivedFire::parse(event) {
        Ok(fire) => fire,
        Err(refusal) => return refuse_fire(shot, refusal, now, shot_trace),
    };
    if cursor.is_some_and(|(tick, _)| fire_catch_up_ticks(event.fire_tick, tick).is_none()) {
        // Older than the cosmetic catch-up bound (a late reliable resend, a stalled client): a
        // legitimate absence, the shell's flight already over. Nothing presents.
        crate::shot_trace::record(
            shot_trace,
            "drop",
            now.map(|t| t.0),
            shot,
            || json!({ "s": "fire", "res": "stale_fire" }),
        );
        return;
    }
    if let Some((_, evicted)) = held.0.hold(event.fire_tick, fire) {
        crate::shot_trace::record(
            shot_trace,
            "drop",
            now.map(|t| t.0),
            evicted.shot_id(),
            || json!({ "s": "fire", "res": "held_capacity" }),
        );
    }
}

/// The game's damage model validates an outside path once, at network ingress.
fn consume_flight_outcome(
    outcome: FlightOutcome,
    now: Option<Tick>,
    sanctioned: &mut SanctionedShots,
    shot_trace: &mut Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    let (shot, sequence) = (outcome.shot, outcome.sequence);
    let result = sanctioned.insert(outcome);
    if let SanctionedInsert::Refused(error) = result {
        warn!(
            ?shot,
            ?error,
            "client: refused invalid public flight outcome"
        );
    }
    crate::shot_trace::record(
        shot_trace,
        "path_rx",
        now.map(|t| t.0),
        shot,
        || json!({ "seq": sequence, "result": format!("{result:?}") }),
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

/// What a released fire resolves against on this replica: the live shooter roots, and the weapons
/// whose round lists bind the fired round.
#[derive(SystemParam)]
pub(super) struct FireShooters<'w, 's> {
    tanks: Query<'w, 's, (Entity, &'static CombatantId), With<NetTank>>,
    weapons:
        Query<'w, 's, (&'static Weapon, &'static WeaponIndex, &'static TankRoot), With<Muzzle>>,
}

/// A released fire against this replica's live roots and rigs.
enum FireBinding {
    /// The shooter root and the round its weapon slot fires, in the fact's fuze mode.
    Bound(Entity, Box<crate::rounds::Round>),
    /// Replication has not caught up: no live root carries the combatant, two do, or the root's
    /// rig (every muzzle at once, `rig::attach_replicated_rig`) has not attached. Deferred to
    /// [`PendingFireEvents`], whose expiry is measured in view time.
    Deferred,
    /// The assembled rig cannot fire this fact.
    Refused(FireRefusal),
}

impl FireShooters<'_, '_> {
    /// Bind the fire's shooter root and the round that root's weapon slot lists at `event.round`,
    /// from this replica's own tank data, in the event's fuze mode.
    fn resolve(&self, event: &FireEvent) -> FireBinding {
        let ShooterResolution::Resolved(shooter) = resolve_shooter(event, &self.tanks) else {
            return FireBinding::Deferred;
        };
        let mut weapons = self
            .weapons
            .iter()
            .filter(|(.., root)| root.0 == shooter)
            .peekable();
        if weapons.peek().is_none() {
            return FireBinding::Deferred;
        }
        let (weapon, round, fuze_mode) = (event.weapon, event.round, event.fuze_mode);
        let Some((fired, ..)) = weapons.find(|(_, slot, _)| slot.0 == usize::from(weapon)) else {
            return FireBinding::Refused(FireRefusal::WeaponSlot { weapon });
        };
        let Some(fired) = fired.rounds.get(usize::from(round)) else {
            return FireBinding::Refused(FireRefusal::Round { weapon, round });
        };
        match fired.in_mode(fuze_mode) {
            Some(bound) => FireBinding::Bound(shooter, Box::new(bound)),
            None => FireBinding::Refused(FireRefusal::FuzeMode {
                weapon,
                round,
                fuze_mode,
            }),
        }
    }
}

/// Release at cursor crossing, deferring unresolved shooters to [`PendingFireEvents`]
/// and its replica-race TTL. No cursor yet means hold.
#[expect(
    clippy::too_many_arguments,
    reason = "one release boundary owns cursor crossing and shooter resolution"
)]
fn release_due_fire_events(
    now: Tick,
    cursor: Option<(Tick, f64)>,
    shooters: &FireShooters,
    pending_recoil: &mut PendingRecoilKicks,
    held: &mut HeldFireEvents,
    pending_fires: &mut PendingFireEvents,
    shot_trace: &mut Option<ResMut<crate::shot_trace::ShotTrace>>,
    commands: &mut Commands,
) {
    let due = match cursor {
        Some((tick, overstep)) => held.0.release(tick, overstep),
        None => Vec::new(),
    };
    for (_, fire) in due {
        match shooters.resolve(&fire.event) {
            FireBinding::Bound(shooter, round) => spawn_reconstructed_fire(
                &fire,
                shooter,
                *round,
                now,
                cursor,
                pending_recoil,
                shot_trace,
                commands,
            ),
            FireBinding::Deferred => {
                if let Some(evicted) = pending_fires.insert(fire, now) {
                    crate::shot_trace::record(
                        shot_trace,
                        "drop",
                        now.0,
                        evicted.fire.shot_id(),
                        || json!({ "s": "fire", "res": "pending_capacity" }),
                    );
                }
            }
            FireBinding::Refused(refusal) => refuse_fire(fire.shot_id(), refusal, now, shot_trace),
        }
    }
}

fn resolve_pending_fire_events(
    now: Tick,
    cursor: Option<(Tick, f64)>,
    shooters: &FireShooters,
    pending_recoil: &mut PendingRecoilKicks,
    pending_fires: &mut PendingFireEvents,
    shot_trace: &mut Option<ResMut<crate::shot_trace::ShotTrace>>,
    commands: &mut Commands,
) {
    if cursor.is_none() {
        return;
    } // No presentation epoch yet; keep unresolved facts held.
    let pending_count = pending_fires.len();
    for _ in 0..pending_count {
        let pending = pending_fires.pop_front().expect(
            "the captured pending count guarantees one queued fire per pass; deferred fires requeue",
        );
        let shot = pending.fire.shot_id();
        if now - pending.received_tick > pending_fires.ttl_ticks {
            crate::shot_trace::record(
                shot_trace,
                "drop",
                now.0,
                shot,
                || json!({ "s": "fire", "res": "pending_expired" }),
            );
            continue;
        }
        match shooters.resolve(&pending.fire.event) {
            FireBinding::Bound(shooter, round) => spawn_reconstructed_fire(
                &pending.fire,
                shooter,
                *round,
                now,
                cursor,
                pending_recoil,
                shot_trace,
                commands,
            ),
            FireBinding::Deferred => pending_fires.requeue(pending),
            FireBinding::Refused(refusal) => refuse_fire(shot, refusal, now, shot_trace),
        }
    }
}

/// One released shot's wire/state facts — everything presentation needs except its age, which is
/// the seam's to measure.
#[derive(Clone, Copy)]
struct ReleasedShot {
    birth: Vec3,
    origin: Vec3,
    direction: Dir3,
    /// The fired round, bound from the shooter's weapon data.
    round: crate::rounds::Round,
    tracer: bool,
    mechanism: crate::spec::FireMechanism,
    shooter: ShotSource,
    shot: crate::ShotId,
}

/// The game's damage-model presentation: one announced round, muzzle dressing and recoil kick.
/// Age is measured on the releasing cursor, not the leading local timeline, so a crossing release
/// starts at the muzzle. `None` names a release beyond [`MAX_COSMETIC_CATCH_UP_TICKS`]; the caller
/// records its omission.
fn present_released_shot(
    released_tick: Tick,
    (cursor_tick, overstep): (Tick, f64),
    shot: ReleasedShot,
    recoil: &mut PendingRecoilKicks,
    commands: &mut Commands,
) -> Option<u32> {
    // A release happens at or after the crossing, so the age is non-negative by construction; the
    // clamp mirrors `fire_catch_up_ticks`' don't-rewind rule for a malformed tick.
    let age = (f64::from(cursor_tick - released_tick) + overstep).max(0.0);
    // Whole ticks feed the fixed-tick catch-up march; the dropped fraction is under one tick of
    // shell flight.
    let catch_up_ticks = age as u32;
    if catch_up_ticks > MAX_COSMETIC_CATCH_UP_TICKS {
        return None;
    }
    present_shot_at_age(catch_up_ticks, shot, recoil, commands);
    Some(catch_up_ticks)
}

/// The game's damage-model presentation: one shell and one barrel kick per released server fact.
fn present_shot_at_age(
    catch_up_ticks: u32,
    shot: ReleasedShot,
    recoil: &mut PendingRecoilKicks,
    commands: &mut Commands,
) {
    commands.trigger(FireShell {
        shot_origin: FireShellOrigin::Reconstructed,
        catch_up_ticks,
        ..FireShell::of_round(
            &shot.round,
            shot.birth,
            shot.origin,
            shot.direction,
            shot.mechanism,
            shot.tracer,
            Some(shot.shooter),
            Some(shot.shot),
        )
    });
    recoil.push(shot.shooter.tank, shot.shooter.weapon);
}

#[expect(
    clippy::too_many_arguments,
    reason = "one presentation boundary owns reconstruction and its release record"
)]
fn spawn_reconstructed_fire(
    fire: &ReceivedFire,
    shooter: Entity,
    round: crate::rounds::Round,
    now: Tick,
    cursor: Option<(Tick, f64)>,
    pending_recoil: &mut PendingRecoilKicks,
    shot_trace: &mut Option<ResMut<crate::shot_trace::ShotTrace>>,
    commands: &mut Commands,
) {
    let event = &fire.event;
    let facts = ReleasedShot {
        birth: event.birth,
        origin: event.origin,
        direction: fire.direction,
        round,
        tracer: event.tracer,
        mechanism: event.mechanism,
        shooter: ShotSource {
            tank: shooter,
            weapon: event.weapon as usize,
            round: event.round,
        },
        shot: event.shot_id(),
    };
    let cursor = cursor.expect("held fire releases only after the view clock has a cursor");
    let Some(catch_up_ticks) =
        present_released_shot(event.fire_tick, cursor, facts, pending_recoil, commands)
    else {
        crate::shot_trace::record(
            shot_trace,
            "drop",
            now.0,
            event.shot_id(),
            || json!({ "s": "fire", "res": "stale_release" }),
        );
        return;
    };
    crate::shot_trace::record(
        shot_trace,
        "present",
        now.0,
        event.shot_id(),
        || json!({ "itick": cursor_itick(Some(cursor)), "cu": catch_up_ticks }),
    );
}

/// Drain the owner-private `DamageConfirm` channel and raise one presentation event per receipt.
///
/// The channel target already proves ownership; receipt handling must not query a currently controlled
/// tank, because the player can receive a valid confirmation after death and before respawn.
/// This runs in `Update` for the same receiver-clear scheduling invariant as [`receive_fire_events`].
pub(super) fn receive_damage_confirms(
    mut receivers: Query<&mut MessageReceiver<DamageConfirm>>,
    view: Res<ViewClock>,
    mut seen: ResMut<SeenDamage>,
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
    mut commands: Commands,
) {
    let received_tick = view.now().map(|now| now.tick());
    for mut receiver in &mut receivers {
        for damage in receiver.receive() {
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
    received_tick: Option<Tick>,
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
    crate::shot_trace::record(
        shot_trace,
        "dmg_rx",
        received_tick.map(|t| t.0),
        shot,
        || {
            json!({
                "own": true,
                "dup": !fresh,
                "dt": damage.damage_tick.0,
            })
        },
    );
    if fresh {
        commands.trigger(super::hit_feel::LocalHitConfirmed {
            receipt: damage.receipt,
            received_tick: received_tick.map(|t| t.0),
            damage_tick: damage.damage_tick.0,
        });
    }
}

/// The game's damage model: path retention advances only with presentation. A stalled cursor
/// cannot expire a fact before its cosmetic shell has had the chance to consume it.
fn age_sanctioned_shots(
    mut sanctioned: ResMut<SanctionedShots>,
    view: Res<ViewClock>,
    tick: Res<lightyear::core::tick::TickDuration>,
    mut previous: Local<(u64, Option<lightyear::core::time::TickInstant>)>,
) {
    if previous.0 != view.epoch {
        *previous = (view.epoch, None);
    }
    let Some(now) = view.now() else {
        previous.1 = None; // No server presentation epoch before samples or between connections.
        return;
    };
    if let Some(before) = previous.1.replace(now) {
        assert!(
            now >= before,
            "the connected view cursor never moves backward; a waiting epoch resets path aging"
        );
        sanctioned.age((now - before).to_f32() * tick.0.as_secs_f32());
    }
}

/// The game's damage model: whole-tick age bound for late server facts at the view cursor.
/// `None` refuses an expired cosmetic; `Some(0)` admits a fact still ahead of presentation.
///
/// Lightyear's monotonic u32 `Tick` difference saturates to i32, so outside-input extremes stay
/// bounded; network publication refuses timeline exhaustion rather than privately wrapping.
/// Ahead-of-cursor fire ticks yield zero; elapsed ticks through
/// [`MAX_COSMETIC_CATCH_UP_TICKS`] fast-forward (normal case DERIVED approximately 10); older
/// ticks refuse without a catch-up loop.
fn fire_catch_up_ticks(fire: Tick, now: Tick) -> Option<u32> {
    let elapsed = now - fire;
    if elapsed < 0 {
        return Some(0);
    }
    let elapsed = elapsed as u32;
    (elapsed <= MAX_COSMETIC_CATCH_UP_TICKS).then_some(elapsed)
}

/// The game's damage-model presentation: derive barrel kicks from local `Weapon` specs via
/// `shooting::kick_recoil`, shared with local fire (barrel/recoil gates included). Wire facts
/// carry only shooter/slot; `WeaponIndex` selects the muzzle's spec, and `apply_recoil` springs
/// the velocity home. Every authority fire fact uses this queue.
///
/// `FixedUpdate`, before `GameplaySet`, with `apply_recoil`'s Playing gate: kicks spring in the
/// same tick. Render-rate TankSim writes would vary across 0/1/2-tick frames.
/// Queued causes already resolved an assembled rig; missing muzzle/root
/// means despawn between enqueue and tick (knocked out/removed or link ended), so skip.
/// `kick_recoil` owns legitimate recoil-less weapons such as a coax.
fn apply_pending_recoil_kicks(
    mut pending: ResMut<PendingRecoilKicks>,
    muzzles: Query<(&WeaponIndex, &Weapon, &TankRoot), With<Muzzle>>,
    mut sims: Query<&mut TankSim>,
) {
    for (shooter, slot) in pending.0.drain(..) {
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

/// Sample the frame after devices, before the immediate message send.
pub(super) fn install_device_feed(app: &mut App) {
    app.add_systems(
        PostUpdate,
        feed_frame_input.before(super::input_arrival::send_inputs),
    );
}
fn feed_frame_input(
    overlays: Res<crate::overlay::Overlays>,
    windows: Query<&Window, With<PrimaryWindow>>,
    drive: Res<crate::command::DriveInput>,
    mut device: ResMut<DeviceCommand>,
    mut own: Query<&mut TankCommand, With<GameControlled>>,
    mut frame: ResMut<super::input_arrival::FrameInput>,
    (bindings, keys, mouse): (
        Res<crate::command::Bindings>,
        Res<ButtonInput<KeyCode>>,
        Res<ButtonInput<MouseButton>>,
    ),
) {
    let focused = match windows.single() {
        Ok(window) => Some(window.focused),
        Err(QuerySingleError::NoEntities(_)) => None,
        Err(error @ QuerySingleError::MultipleEntities(_)) => panic!("one primary window: {error}"),
    };
    frame.command = match focused {
        _ if own.is_empty() || crate::overlay::input_blocked(&overlays, true) => default(),
        Some(true) => device.0,
        Some(false) => TankCommand {
            throttle: drive.set_throttle(),
            ..default()
        },
        None => default(),
    };
    frame.secondary_cycle = focused == Some(true)
        && !own.is_empty()
        && !crate::overlay::input_blocked(&overlays, true)
        && bindings.fire_secondary.just_pressed(&keys, &mouse)
        && bindings.fire_secondary.just_released(&keys, &mouse);
    device.0.clear_edges();
    for mut command in &mut own {
        command.clear_edges();
    }
}

/// Two headless matchmaking clients against the live Worker (`#[ignore]`d; needs `OVERMATCH_API`).
#[cfg(test)]
mod hosted_e2e;

/// The matchmaking exits through the installed lifecycle, over real UDP.
#[cfg(test)]
pub(super) mod match_exit;

/// In-crate ECS tests can reach private `tank` types (`TankSim` / `Weapon` / `WeaponIndex`)
/// unavailable to external integration tests. Recoil probes exercise the local-spec derivation.
#[cfg(test)]
mod tests {

    /// Lightyear ownership can precede the body; the player's camera must survive that join gap.
    #[test]
    fn joining_before_body_attachment_keeps_the_camera_alive() {
        use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<avian3d::collider_tree::ColliderTrees>()
            .init_resource::<AccumulatedMouseMotion>()
            .init_resource::<AccumulatedMouseScroll>()
            .init_resource::<crate::sight::SightMode>()
            .init_resource::<crate::aim::CommittedAim>()
            .init_resource::<crate::firecontrol::Ranging>()
            .add_plugins(crate::camera::plugin)
            .add_plugins(control_claim_plugin);
        let own = app.world_mut().spawn((NetControlled, NetTank)).id();
        app.update();
        assert!(app.world().get::<GameControlled>(own).is_none());
        attach_control_fixture(app.world_mut(), own);
        assert!(app.world().get::<GameControlled>(own).is_some());
        app.update();
    }

    fn attach_control_fixture(world: &mut World, root: Entity) {
        let turret = world.spawn((ChildOf(root), Transform::IDENTITY)).id();
        // Fixture: any admitted optic; the ownership contract is independent of vehicle data.
        let views = crate::tests::tank_views(crate::spec::Optics::Unaided(60.0).vertical_fov());
        world.entity_mut(root).insert((
            Transform::IDENTITY,
            views,
            Rig {
                hull: root,
                turret,
                gunner_yaw: turret,
                gun: turret,
                muzzle: turret,
            },
        ));
    }

    /// A body arriving before ownership must remain remote, then claim immediately when owned.
    /// The servo pump relies on that claim being visible with the complete rig.
    #[test]
    fn ownership_after_body_attachment_claims_the_complete_tank() {
        let mut app = App::new();
        app.add_plugins(control_claim_plugin);
        let remote = app.world_mut().spawn(NetTank).id();
        attach_control_fixture(app.world_mut(), remote);
        assert!(app.world().get::<GameControlled>(remote).is_none());
        app.world_mut().entity_mut(remote).insert(NetControlled);
        assert!(app.world().get::<GameControlled>(remote).is_some());
    }

    /// Loading needs the own bound view and a playable snapshot, in either arrival order.
    /// Another tank's view cannot release it; later tank removal retains the once-ready latch.
    #[test]
    fn loading_waits_for_the_own_tanks_bound_view() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.insert_resource(ConnectRetry {
            connected_once: true,
            ..default()
        });
        let playable = world
            .spawn(super::super::battle::BattleState::combat_fixture())
            .id();
        world.spawn((NetTank, crate::tank::TankViewReady));
        let own = world.spawn((GameControlled, NetTank)).id();
        world.run_system_once(latch_tank_seen).unwrap();
        assert!(
            !world.resource::<ConnectRetry>().tank_seen,
            "a playable phase and another tank's view cannot present the own tank"
        );
        world.despawn(playable);
        world.entity_mut(own).insert(crate::tank::TankViewReady);
        world.run_system_once(latch_tank_seen).unwrap();
        assert!(
            !world.resource::<ConnectRetry>().tank_seen,
            "a bound view without its phase still loads"
        );
        let battle = world
            .spawn(super::super::battle::BattleState::default())
            .id();
        world.run_system_once(latch_tank_seen).unwrap();
        assert!(
            !world.resource::<ConnectRetry>().tank_seen,
            "a stale empty-server snapshot cannot expose a waiting screen"
        );
        *world
            .get_mut::<super::super::battle::BattleState>(battle)
            .unwrap() = super::super::battle::BattleState::combat_fixture();
        world.run_system_once(latch_tank_seen).unwrap();
        assert!(world.resource::<ConnectRetry>().tank_seen);
        world.despawn(own);
        world.run_system_once(latch_tank_seen).unwrap();
        assert!(
            world.resource::<ConnectRetry>().tank_seen,
            "later tank removal must not reopen loading"
        );
    }

    /// The device feed must expose duplicate primary windows as a construction bug: treating
    /// them as an absent window silently makes a live player's controls inert.
    #[test]
    #[should_panic(expected = "one primary window")]
    fn device_feed_refuses_duplicate_primary_windows() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.init_resource::<crate::overlay::Overlays>();
        world.init_resource::<crate::command::DriveInput>();
        world.init_resource::<DeviceCommand>();
        world.init_resource::<super::super::input_arrival::FrameInput>();
        world.init_resource::<crate::command::Bindings>();
        world.init_resource::<ButtonInput<KeyCode>>();
        world.init_resource::<ButtonInput<MouseButton>>();
        for _ in 0..2 {
            world.spawn((PrimaryWindow, Window::default()));
        }
        world.run_system_once(feed_frame_input).unwrap();
    }

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

    /// One precedence law: `OVERMATCH_SERVER` beats any Worker URL (`OVERMATCH_API` over the
    /// baked one, a blank one unset), which beats the local default.
    #[test]
    fn direct_paths_win_over_any_worker_url() {
        let environment = ServerSelection {
            raw: "env.example",
            source: ServerSource::Environment,
        };
        let local_default = ServerSelection {
            raw: LOCAL_SERVER,
            source: ServerSource::LocalDefault,
        };
        assert_eq!(choose_server(Some("env.example")), environment);
        assert_eq!(choose_server(None), local_default);

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
                choose_mode(Some("env.example"), api, baked_api),
                ClientMode::Direct(environment),
            );
        }

        assert_eq!(
            choose_mode(
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
            choose_mode(None, Some(""), Some("https://baked-api.example")),
            ClientMode::Matchmaking {
                api: "https://baked-api.example",
                source: ApiSource::BakedDefault,
            },
            "an empty OVERMATCH_API is unset"
        );
        assert_eq!(
            choose_mode(None, None, Some("  ")),
            ClientMode::Direct(local_default),
            "with no Worker URL anywhere the client connects to the local server"
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
        view: Res<ViewClock>,
        mut seen: ResMut<SeenDamage>,
        mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
        mut commands: Commands,
    ) {
        for damage in &copies.0 {
            consume_damage_confirm(
                damage,
                view.now().map(|now| now.tick()),
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

    use crate::spec::{FireCycle, RecoilSpec, Trigger};
    use crate::tank::WeaponState;

    /// A one-weapon `Weapon` config with the given `recoil` spec and `barrel` node — the only two
    /// fields `kick_recoil`'s gate reads; the rest are filled to satisfy the struct.
    fn weapon(recoil: Option<RecoilSpec>, barrel: Option<Entity>) -> Weapon {
        Weapon {
            bore_length: 0.4,
            name: "MainGun".into(),
            rounds: vec![crate::tank::WeaponRound::resolve("88mm_AP").expect("a shipped round")],
            fire_cycle: FireCycle::Single {
                reload: crate::sim_tick::TickSpan::from_secs(8.0).expect("fixture reload"),
            },
            recoil,
            barrel,
            fire: Vec::new(),
            load: Vec::new(),
            trigger: Trigger::Primary,
            report_clips: Vec::new(),
        }
    }

    /// A `RecoilSpec` with the given kick (stiffness/damping/stop are irrelevant to the derive).
    fn recoil(kick: f32) -> RecoilSpec {
        RecoilSpec {
            kick,
            stiffness: 100.0,
            damping: 5.0,
            max_length: crate::provenance::Sourced(0.5),
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

    /// `kick_recoil` kicks only a weapon with BOTH a barrel node and a recoil spec. A barrel-less
    /// kick would accumulate in `recoil_velocity` and never decay (`apply_recoil` steps
    /// `RecoilParams`, built on the barrel node); a recoil-less weapon (a coax) has nothing to
    /// kick. A slot off the wire reaches it only through the local muzzle that holds it
    /// (`apply_pending_recoil_kicks`). The gate is shared, so it holds on the server's `fire` path
    /// too.
    #[test]
    fn barrel_less_weapon_is_noop() {
        let barrel = Some(Entity::PLACEHOLDER);
        let mut sim = TankSim {
            weapons: vec![WeaponState::default(); 1],
        };
        for (slot, weapon, case) in [
            (0, weapon(Some(recoil(3.5)), None), "a barrel-less weapon"),
            (0, weapon(None, barrel), "a recoil-less weapon"),
        ] {
            crate::shooting::kick_recoil(&mut sim, slot, &weapon);
            assert_eq!(sim.weapons[0].recoil_velocity, 0.0, "{case} kicks nothing");
        }
    }

    /// Catch-up is 0 for a fire tick at or ahead of now (clock skew or a malicious tick never
    /// rewinds the shell), the elapsed ticks up to `MAX_COSMETIC_CATCH_UP_TICKS` inclusive, and
    /// `None` beyond (a stale or corrupt fire is rejected, never a 10^6-step loop). Tick arithmetic
    /// SATURATES rather than wrapping (lightyear 0.31.0's `Tick` is monotonic), so the extreme pairs
    /// land in the bounded arms — never a ~4-billion-tick catch-up loop. Fires if lightyear
    /// re-introduces wrapping ticks.
    #[test]
    fn extreme_tick_pairs_saturate_into_the_bounded_arms() {
        let bound = MAX_COSMETIC_CATCH_UP_TICKS;
        for (fire, now, expected) in [
            (500, 500, Some(0)),
            (503, 500, Some(0)),
            (500, 505, Some(5)),
            (500, 500 + bound, Some(bound)),
            (500, 500 + bound + 1, None),
            (0, 1_000_000, None),
            (u32::MAX, 0, Some(0)),
            (0, u32::MAX, None),
            (u32::MAX - 2, 3, Some(0)),
        ] {
            assert_eq!(
                fire_catch_up_ticks(Tick(fire), Tick(now)),
                expected,
                "fire tick {fire} at now {now}"
            );
        }
    }

    fn shot(tick: u32) -> ShotId {
        ShotId {
            combatant: crate::CombatantId(1),
            weapon: 0,
            fire_tick: tick,
        }
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

    /// The game's damage model: a disconnected server cannot finish a cosmetic path.
    /// A new connection must never inherit a round frozen on the previous server's cursor.
    #[test]
    fn a_link_ending_retires_its_server_clocked_cosmetic_shells() {
        let mut app = App::new();
        app.add_observer(retire_link_shells);
        let shell = app
            .world_mut()
            .spawn(crate::ballistics::Projectile::view_test_round(0.088))
            .id();
        let link = app
            .world_mut()
            .spawn((RemoteId(PeerId::Netcode(99)), Connected))
            .id();
        app.world_mut().entity_mut(link).remove::<Connected>();
        app.world_mut().flush();
        assert!(app.world().get_entity(shell).is_err());
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
            let mut view = ViewClock::default();
            view.set_now(Tick(13).into());
            let mut app = App::new();
            app.insert_resource(view);
            app.init_resource::<SeenDamage>();
            app.insert_resource(crate::shot_trace::ShotTrace::for_test(&path, Some(13)));
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
            birth: Vec3::ZERO,
            origin: Vec3::ZERO,
            direction: Vec3::NEG_Z,
            mechanism: crate::spec::FireMechanism::Automatic,
            tracer: true,
            shooter,
            combatant: CombatantId(combatant),
            weapon: 0,
            round: 0,
            fuze_mode: 0,
            fire_tick: Tick(tick),
        }
    }

    /// A shooter root for `combatant` carrying the weapon slots the fixture fires name: slot 0
    /// loads the MG round, slot 1 the two cannon rounds, default first. A receiver binds every
    /// fire's round from these lists, so a root without them never presents.
    fn spawn_shooter(world: &mut World, combatant: u64) -> Entity {
        let root = world.spawn((NetTank, CombatantId(combatant))).id();
        let slots: [&[&str]; 2] = [&["7.9mm_AP"], &["88mm_AP", "88mm_APCR"]];
        for (slot, keys) in slots.into_iter().enumerate() {
            let mut fixture = weapon(None, None);
            fixture.rounds = keys
                .iter()
                .map(|key| crate::tank::WeaponRound::resolve(key).expect("a shipped round"))
                .collect();
            world.spawn((Muzzle, WeaponIndex(slot), TankRoot(root), fixture));
        }
        root
    }

    /// A fire may beat its root's replica. It is retained by stable shot identity and resolves when
    /// exactly one matching combatant root exists, even if the original entity map was unresolved;
    /// two live roots with one `CombatantId` fail closed.
    #[test]
    fn pending_fire_resolves_when_its_combatant_root_arrives() {
        let mut world = World::new();
        let unmapped = Entity::from_raw_u32(999).expect("non-placeholder entity");
        let event = fire_event(unmapped, 7, 42);
        let mut pending = PendingFireEvents::from_world(&mut World::new());
        let fire = ReceivedFire::parse(&event).expect("the fixture bore is a unit direction");
        assert!(pending.insert(fire, Tick(42)).is_none());
        assert_eq!(pending.len(), 1, "unresolved fire is retained");

        let before = world
            .run_system_once(move |tanks: Query<(Entity, &CombatantId), With<NetTank>>| {
                resolve_shooter(&event, &tanks)
            })
            .expect("system runs");
        assert_eq!(before, ShooterResolution::Missing);

        let root = world.spawn((NetTank, CombatantId(7))).id();
        let pending_event = pending
            .pop_front()
            .expect("queued event remains")
            .fire
            .event;
        let ambiguous_event = pending_event.clone();
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

        world.spawn((NetTank, CombatantId(7)));
        let ambiguous = world
            .run_system_once(move |tanks: Query<(Entity, &CombatantId), With<NetTank>>| {
                resolve_shooter(&ambiguous_event, &tanks)
            })
            .expect("system runs");
        assert_eq!(
            ambiguous,
            ShooterResolution::Ambiguous,
            "two live roots with one CombatantId must not receive an arbitrary visual/recoil event",
        );
    }

    /// Consume one arriving fire through the production gate, then run the release pass at the
    /// given cursor (`None` = waiting for the first completed stamp pair).
    fn consume_and_release(
        world: &mut World,
        event: FireEvent,
        now: Tick,
        cursor: Option<(Tick, f64)>,
    ) {
        world
            .run_system_once(
                move |shooters: FireShooters,
                      mut recoil: ResMut<PendingRecoilKicks>,
                      mut seen: ResMut<SeenShots>,
                      mut held: ResMut<HeldFireEvents>,
                      mut pending: ResMut<PendingFireEvents>,
                      mut commands: Commands| {
                    let mut trace: Option<ResMut<crate::shot_trace::ShotTrace>> = None;
                    consume_fire_event(&event, Some(now), cursor, &mut seen, &mut held, &mut trace);
                    release_due_fire_events(
                        now,
                        cursor,
                        &shooters,
                        &mut recoil,
                        &mut held,
                        &mut pending,
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
                move |shooters: FireShooters,
                      mut recoil: ResMut<PendingRecoilKicks>,
                      mut held: ResMut<HeldFireEvents>,
                      mut pending: ResMut<PendingFireEvents>,
                      mut commands: Commands| {
                    let mut trace: Option<ResMut<crate::shot_trace::ShotTrace>> = None;
                    release_due_fire_events(
                        now,
                        cursor,
                        &shooters,
                        &mut recoil,
                        &mut held,
                        &mut pending,
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
            .init_resource::<ReconstructedFireShells>()
            .add_observer(record_reconstructed_fire_shell);
        app
    }

    /// The game's damage model: an arrival before the view epoch must wait for its first
    /// crossing. A local-clock fallback would present the shell and kick before its hull exists.
    #[test]
    fn no_view_epoch_holds_fire_until_the_first_cursor_crossing() {
        let mut app = fire_receive_app();
        let world = app.world_mut();
        let root = spawn_shooter(world, 7);
        let event = fire_event(root, 7, 42); // Fixture identity and release tick.
        consume_and_release(world, event, Tick(100), None);
        release_at(world, Tick(101), None);
        assert!(world.resource::<ReconstructedFireShells>().0.is_empty());
        release_at(world, Tick(42), Some((Tick(42), 0.0)));
        assert_eq!(world.resource::<ReconstructedFireShells>().0.len(), 1);
        assert_eq!(world.resource::<PendingRecoilKicks>().queued(), [(root, 0)]);
    }

    /// The game's damage model: a public path must remain available while the view cursor
    /// holds, even if many local fixed ticks elapse. Moving retention back to a local clock
    /// loses server facts before their presentation crossing.
    #[test]
    fn public_path_retention_ages_only_when_the_view_advances() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<ViewClock>()
            .init_resource::<SanctionedShots>()
            .insert_resource(lightyear::core::tick::TickDuration(
                super::super::server::TICK,
            ))
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                Duration::from_secs(1),
            ))
            .add_systems(Update, age_sanctioned_shots);
        let shot = crate::ShotId {
            combatant: CombatantId(7),
            weapon: 0,
            fire_tick: 100,
        }; // Fixture identity.
        app.world_mut()
            .resource_mut::<ViewClock>()
            .set_now(Tick(100).into());
        app.world_mut()
            .resource_mut::<SanctionedShots>()
            .insert(crate::ballistics::path_fixture(shot, 102));
        for _ in 0..(SanctionedShots::MAX_AGE_SECS.ceil() as u32 + 2) {
            app.update();
        }
        assert!(app.world().resource::<SanctionedShots>().has_shot(shot));
        let expired = 100
            + (SanctionedShots::MAX_AGE_SECS / super::super::server::TICK.as_secs_f32()).ceil()
                as u32
            + 1;
        app.world_mut()
            .resource_mut::<ViewClock>()
            .set_now(Tick(expired).into());
        app.update();
        assert!(!app.world().resource::<SanctionedShots>().has_shot(shot));
    }

    /// The game's damage model: a cut retires old facts before fresh network arrivals enter.
    /// The shipped ingress installer owns this ordering; aging after ingress drops a newly
    /// received path immediately whenever the cut exceeds its retention horizon.
    #[test]
    fn a_view_cut_cannot_expire_a_path_that_arrives_on_that_frame() {
        use bevy::ecs::schedule::{LogLevel, ScheduleBuildSettings};
        #[derive(Resource)]
        struct Arrival(Option<FlightOutcome>);
        fn inject(mut arrival: ResMut<Arrival>, mut paths: ResMut<SanctionedShots>) {
            if let Some(path) = arrival.0.take() {
                let mut trace = None;
                consume_flight_outcome(path, None, &mut paths, &mut trace);
            }
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<ViewClock>()
            .init_resource::<LocalTimeline>()
            .insert_resource(lightyear::core::tick::TickDuration(
                super::super::server::TICK,
            ))
            .insert_resource(Arrival(None));
        install_fire_ingress(&mut app);
        app.add_systems(
            Update,
            inject.in_set(FireIngress).before(receive_fire_events),
        );
        app.edit_schedule(Update, |schedule| {
            schedule.set_build_settings(ScheduleBuildSettings {
                ambiguity_detection: LogLevel::Error,
                ..ScheduleBuildSettings::new()
            });
        });
        let old = ShotId {
            combatant: CombatantId(7),
            weapon: 0,
            fire_tick: 100,
        }; // Fixture identity.
        app.world_mut()
            .resource_mut::<ViewClock>()
            .set_now(Tick(100).into());
        app.world_mut()
            .resource_mut::<SanctionedShots>()
            .insert(crate::ballistics::path_fixture(old, 102));
        app.update();
        let after = 100
            + (SanctionedShots::MAX_AGE_SECS / super::super::server::TICK.as_secs_f32()).ceil()
                as u32
            + 1;
        let fresh = ShotId {
            fire_tick: after,
            ..old
        };
        app.world_mut()
            .resource_mut::<ViewClock>()
            .set_now(Tick(after).into());
        app.world_mut().resource_mut::<Arrival>().0 =
            Some(crate::ballistics::path_fixture(fresh, after + 2));
        app.update();
        let paths = app.world().resource::<SanctionedShots>();
        assert!(
            !paths.has_shot(old),
            "the old path spent the view-time retention horizon"
        );
        assert!(
            paths.has_shot(fresh),
            "the new receipt has spent no view time yet"
        );
    }

    /// The game's damage-model presentation: the owning client must consume every received
    /// server path, whether its announcement precedes or follows the replicated belt change.
    /// An already
    /// received fire can still be held for the cursor when the belt arrives.
    #[test]
    fn belt_arrival_cannot_discard_an_announced_round_or_its_path() {
        use crate::ballistics::{PathClock, PenetrationMarks, Projectile, Shot};
        use crate::tank::{WeaponGate, WeaponGateState, WeaponLoad, WeaponLoads};
        use lightyear::core::time::TickInstant;

        // Fixture clocks and counts: one consumption at server tick 100, observed at local 116.
        for announcement_first in [false, true] {
            for path_first in [false, true] {
                let mut app = fire_receive_app();
                app.add_plugins((MinimalPlugins, crate::ballistics::sim_plugin));
                app.insert_resource(crate::ClientReplica)
                    .init_resource::<SanctionedShots>()
                    .insert_resource(lightyear::core::tick::TickDuration(
                        crate::net::test_harness::TICK,
                    ))
                    .insert_resource(Time::<Fixed>::from_duration(crate::net::test_harness::TICK));
                let mut local = LocalTimeline::default();
                local.apply_delta(116);
                app.insert_resource(local);
                app.insert_resource(crate::ShotClock(crate::sim_tick::SimTick::new(116)));
                let mut cursor = ViewClock::default();
                cursor.set_now(TickInstant::from(Tick(90)));

                app.insert_resource(cursor);
                let world = app.world_mut();
                let root = spawn_shooter(world, 7);
                world.entity_mut(root).insert((
                    GameControlled,
                    Interpolated,
                    TankSim {
                        weapons: vec![crate::tank::WeaponState::default(); 2],
                    },
                    WeaponGate {
                        weapons: vec![
                            WeaponGateState {
                                timing: crate::tank::WeaponTiming::Ready,
                                belt_remaining: 17,
                            };
                            2
                        ],
                    },
                    WeaponLoads {
                        weapons: vec![
                            WeaponLoad {
                                loaded: 0,
                                selected: 0,
                                fuze_modes: Vec::new(),
                            };
                            2
                        ],
                    },
                ));
                let muzzles: Vec<_> = world
                    .query_filtered::<Entity, With<Muzzle>>()
                    .iter(world)
                    .collect();
                for muzzle in muzzles {
                    world.entity_mut(muzzle).insert(GlobalTransform::IDENTITY);
                }
                // Arm from a complete snapshot and close the initial arrival bracket.
                let event = fire_event(root, 7, 100);
                let shot = event.shot_id();
                let round = world
                    .query::<(&Weapon, &WeaponIndex, &TankRoot)>()
                    .iter(world)
                    .find(|(_, index, tank)| index.0 == 0 && tank.0 == root)
                    .unwrap()
                    .0
                    .rounds[0]
                    .round;
                let mut path = crate::ballistics::path_fixture(shot, 102);
                path.body = round.penetrator;
                path.drag_k = round.drag_k;
                // Fixture sanctioned bounce leaves the original bore, making playback observable.
                path.resume = Vec3::X * 20.0;
                path.impact = Some(crate::ballistics::path::PathImpact::Bounce {
                    at: path.resume,
                    normal: Vec3::Z,
                    direction: Vec3::X,
                    ordinal: 0,
                    feedback: crate::substances::SurfaceFeedback::fixture(),
                });
                let mut trace = None;
                if path_first {
                    consume_flight_outcome(
                        path.clone(),
                        Some(Tick(116)),
                        &mut world.resource_mut::<SanctionedShots>(),
                        &mut trace,
                    );
                }
                if announcement_first {
                    consume_and_release(world, event.clone(), Tick(116), Some((Tick(90), 0.0)));
                }
                world.get_mut::<WeaponGate>(root).unwrap().weapons[0].belt_remaining -= 1;
                world
                    .resource_mut::<ViewClock>()
                    .set_now(TickInstant::from(Tick(105)));
                if announcement_first {
                    release_at(world, Tick(116), Some((Tick(105), 0.0)));
                } else {
                    assert_eq!(
                        world
                            .query_filtered::<Entity, With<Projectile>>()
                            .iter(world)
                            .count(),
                        0,
                        "a belt count cannot author an unidentified trajectory"
                    );
                    consume_and_release(world, event.clone(), Tick(116), Some((Tick(105), 0.0)));
                }
                // Bounded redundant copies must remain deduplicated after the belt observation.
                consume_and_release(world, event, Tick(116), Some((Tick(105), 0.0)));
                if !path_first {
                    consume_flight_outcome(
                        path,
                        Some(Tick(116)),
                        &mut world.resource_mut::<SanctionedShots>(),
                        &mut trace,
                    );
                }
                *world.resource_mut::<PathClock>() = PathClock {
                    tick: 105,
                    fraction: 0.0,
                };
                world.run_schedule(PostUpdate);
                let shells: Vec<_> = world.query_filtered::<(Option<&Shot>, &Transform, &PenetrationMarks),
                    With<Projectile>>().iter(world).collect();
                assert_eq!(shells.len(), 1, "one announced round, one presentation");
                let (identity, pose, marks) = shells[0];
                assert_eq!(
                    identity.map(|identity| identity.0),
                    Some(shot),
                    "the owning client must retain the announcement's path identity"
                );
                assert!(
                    pose.translation.x >= 20.0,
                    "the server path replaces the original bore"
                );
                assert_eq!(marks.ricochets.len(), 1, "the sanctioned bounce plays once");
                assert_eq!(
                    world.resource::<PendingRecoilKicks>().queued(),
                    &[(root, 0)],
                    "one authority round gives one recoil presentation"
                );
            }
        }
    }

    #[test]
    fn pending_fire_duplicate_is_suppressed_by_shot_id() {
        let mut app = fire_receive_app();
        let world = app.world_mut();
        let unmapped = Entity::from_raw_u32(999).expect("non-placeholder entity");
        let event = fire_event(unmapped, 7, 42);
        let shot = event.shot_id();

        consume_and_release(world, event.clone(), Tick(42), Some((Tick(42), 0.0)));
        let root = spawn_shooter(world, 7);
        world
            .run_system_once(
                |shooters: FireShooters,
                 mut recoil: ResMut<PendingRecoilKicks>,
                 mut pending: ResMut<PendingFireEvents>,
                 mut commands: Commands| {
                    let mut trace: Option<ResMut<crate::shot_trace::ShotTrace>> = None;
                    resolve_pending_fire_events(
                        Tick(42),
                        Some((Tick(42), 0.0)),
                        &shooters,
                        &mut recoil,
                        &mut pending,
                        &mut trace,
                        &mut commands,
                    );
                },
            )
            .expect("pending fire resolves through the production repair path");
        consume_and_release(world, event, Tick(42), Some((Tick(42), 0.0)));

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
    /// reading — and presents exactly once when the cursor crosses its fire tick. A fire the
    /// cursor already crossed presents on the SAME drain it arrived on (late join, long RTT).
    #[test]
    fn a_fire_ahead_of_the_cursor_presents_only_at_the_crossing() {
        let mut app = fire_receive_app();
        let world = app.world_mut();
        let root = spawn_shooter(world, 7);
        let event = fire_event(root, 7, 100);
        let shot = event.shot_id();

        // Fixture: local present 108, cursor 93.5; DERIVED fire-tick lead 6.5 ticks.
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

        let behind = fire_event(root, 7, 104);
        let behind_shot = behind.shot_id();
        consume_and_release(world, behind, Tick(120), Some((Tick(112), 0.25)));
        assert_eq!(
            world.resource::<ReconstructedFireShells>().0,
            vec![(shot, root), (behind_shot, root)],
            "an already-crossed fire presents at arrival",
        );
    }

    /// The game's damage-model presentation: fixture burst through production consume/release
    /// gates, observing reconstructed `FireShell` events. DERIVED 768 rpm at 64 Hz (one every
    /// five ticks), plus sparse shots. Each event must be unique, tracer-enabled and under the
    /// flash age gate. Age zero gives DERIVED `catch_up · v · Δt = 0 m` muzzle offset, under
    /// one flight tick (DERIVED 12.5 m at 800 m/s, 64 Hz). Redundant copies emit nothing;
    /// actual sound/flash/tracer consumers are not mounted.
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
        let root = spawn_shooter(world, 7);

        // The burst: 20 MG rounds every 5 ticks from tick 100, plus 2 cannon rounds. Every
        // round is a tracer, so the tracer counter must equal the bang counter.
        let mut events: Vec<FireEvent> =
            (0..20).map(|i| fire_event(root, 7, 100 + 5 * i)).collect();
        for cannon_tick in [103, 158] {
            let mut cannon = fire_event(root, 7, cannon_tick);
            cannon.mechanism = crate::spec::FireMechanism::Single;
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

    /// A FIRE BINDS ITS ROUND FROM THE RECEIVER'S OWN TANK DATA: the event names the round by its
    /// index in the shooter's weapon list, and the reconstructed shell flies that round, body and
    /// penetrator, with the index on its source. An index the receiver's assembled list lacks is a
    /// refused fact: it presents nothing and is not deferred, since the handshake-matched tank data
    /// cannot grow the index later. A mutant that bound the default round, or fell back to it, or
    /// dropped the penetrator, would draw a shell the authority never fired; one that deferred the
    /// refusal would hold it as replication debt until the TTL.
    #[test]
    fn a_fire_binds_the_round_its_index_names_in_the_shooter_s_weapon() {
        #[derive(Resource, Default)]
        struct Bound(Vec<(u8, [f32; 7])>);

        fn bind(fire: On<FireShell>, mut bound: ResMut<Bound>) {
            if fire.shot_origin == FireShellOrigin::Reconstructed
                && let Some(source) = fire.shooter
            {
                bound.0.push((
                    source.round,
                    [
                        fire.speed,
                        fire.caliber,
                        fire.mass,
                        fire.nose_factor,
                        fire.penetrator_caliber,
                        fire.penetrator_mass,
                        fire.wound,
                    ],
                ));
            }
        }

        let mut app = fire_receive_app();
        app.init_resource::<Bound>();
        app.add_observer(bind);
        let world = app.world_mut();
        let root = spawn_shooter(world, 7);
        let named = crate::tank::WeaponRound::resolve("88mm_APCR")
            .expect("a shipped round")
            .round;

        let mut second = fire_event(root, 7, 42);
        second.mechanism = crate::spec::FireMechanism::Single;
        second.weapon = 1;
        second.round = 1;
        consume_and_release(world, second, Tick(42), Some((Tick(42), 0.0)));
        assert_eq!(
            world.resource::<Bound>().0,
            [(
                1,
                [
                    named.muzzle_speed,
                    named.body.caliber,
                    named.body.mass,
                    named.body.nose_factor,
                    named.penetrator.caliber,
                    named.penetrator.mass,
                    named.wound,
                ],
            )],
            "the shell flies the round the index names in the shooter's list",
        );

        let mut unknown = fire_event(root, 7, 43);
        unknown.weapon = 1;
        unknown.round = 2;
        consume_and_release(world, unknown, Tick(43), Some((Tick(43), 0.0)));
        assert_eq!(
            world.resource::<Bound>().0.len(),
            1,
            "an index past the list presents nothing",
        );
        assert_eq!(
            world.resource::<PendingFireEvents>().len(),
            0,
            "an index past the assembled list is refused, never deferred",
        );
    }

    /// The game's damage model: a non-finite network origin must be refused before any
    /// presentation queue or shell is created; otherwise a peer can poison the client's pose.
    #[test]
    fn a_non_finite_fire_origin_is_refused_at_receipt() {
        let mut app = fire_receive_app();
        let world = app.world_mut();
        let root = spawn_shooter(world, 7);
        for (index, poison) in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY]
            .into_iter()
            .enumerate()
        {
            let mut event = fire_event(root, 7, 42 + index as u32);
            event.origin.x = poison;
            assert_eq!(ReceivedFire::parse(&event).err(), Some(FireRefusal::Origin));
            consume_and_release(
                world,
                event,
                Tick(42 + index as u32),
                Some((Tick(42 + index as u32), 0.0)),
            );
            let mut event = fire_event(root, 7, 52 + index as u32);
            event.birth.x = poison;
            assert_eq!(ReceivedFire::parse(&event).err(), Some(FireRefusal::Birth));
            consume_and_release(
                world,
                event,
                Tick(52 + index as u32),
                Some((Tick(52 + index as u32), 0.0)),
            );
        }
        assert!(world.resource::<ReconstructedFireShells>().0.is_empty());
        assert_eq!(world.resource::<PendingFireEvents>().len(), 0);
        assert!(
            world
                .resource_mut::<HeldFireEvents>()
                .0
                .release(Tick(u32::MAX - 1), 0.0)
                .is_empty()
        );
    }

    /// REPLICATION DEBT DEFERS, A MALFORMED FACT DOES NOT: a fire whose root has replicated but
    /// whose rig has not attached (no muzzle yet) waits in [`PendingFireEvents`] and presents once
    /// the rig arrives; a bore without a finite nonzero direction is refused at the message edge
    /// and never held. Mutants: refusing the rig-pending fire loses an opponent's shot on every
    /// join; holding the malformed bore carries an unparsed fact into presentation.
    #[test]
    fn a_rig_pending_fire_defers_and_a_malformed_bore_is_refused_at_receipt() {
        let mut app = fire_receive_app();
        let world = app.world_mut();
        let root = world.spawn((NetTank, CombatantId(7))).id();
        let event = fire_event(root, 7, 42);
        let shot = event.shot_id();
        consume_and_release(world, event, Tick(42), Some((Tick(42), 0.0)));
        assert_eq!(
            world.resource::<PendingFireEvents>().len(),
            1,
            "a root without its rig defers the fire"
        );
        world.despawn(root);
        let root = spawn_shooter(world, 7);
        world
            .run_system_once(
                |shooters: FireShooters,
                 mut recoil: ResMut<PendingRecoilKicks>,
                 mut pending: ResMut<PendingFireEvents>,
                 mut commands: Commands| {
                    let mut trace: Option<ResMut<crate::shot_trace::ShotTrace>> = None;
                    resolve_pending_fire_events(
                        Tick(43),
                        Some((Tick(43), 0.0)),
                        &shooters,
                        &mut recoil,
                        &mut pending,
                        &mut trace,
                        &mut commands,
                    );
                },
            )
            .expect("the repair pass runs");
        assert_eq!(
            world.resource::<ReconstructedFireShells>().0,
            vec![(shot, root)],
            "the deferred fire presents once its rig attaches"
        );

        let mut malformed = fire_event(root, 7, 44);
        malformed.direction = Vec3::ZERO;
        consume_and_release(world, malformed, Tick(44), Some((Tick(44), 0.0)));
        assert_eq!(world.resource::<PendingFireEvents>().len(), 0);
        assert_eq!(
            world.resource::<ReconstructedFireShells>().0.len(),
            1,
            "a malformed bore presents nothing"
        );
        assert_eq!(
            ReceivedFire::parse(&fire_event(root, 7, 45))
                .map(|fire| fire.direction)
                .ok(),
            Some(Dir3::NEG_Z),
            "a unit bore parses to its direction"
        );
        let mut far = fire_event(root, 7, 46);
        far.direction = Vec3::new(f32::NAN, 0.0, 1.0);
        assert_eq!(
            ReceivedFire::parse(&far).err(),
            Some(FireRefusal::Bore),
            "a non-finite bore is refused"
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
        let mapped_here = spawn_shooter(world, 7);
        let mapped_elsewhere = spawn_shooter(world, 7);
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
                move |shooters: FireShooters,
                      mut recoil: ResMut<PendingRecoilKicks>,
                      mut seen: ResMut<SeenShots>,
                      mut held: ResMut<HeldFireEvents>,
                      mut pending: ResMut<PendingFireEvents>,
                      mut sanctioned: ResMut<SanctionedShots>,
                      mut commands: Commands| {
                    let mut trace: Option<ResMut<crate::shot_trace::ShotTrace>> = None;
                    consume_fire_visual_fact(
                        FireVisualFact::Fire(here.clone()),
                        Some(Tick(42)),
                        Some((Tick(43), 0.0)),
                        &mut seen,
                        &mut held,
                        &mut sanctioned,
                        &mut trace,
                    );
                    consume_fire_visual_fact(
                        FireVisualFact::Fire(elsewhere.clone()),
                        Some(Tick(42)),
                        Some((Tick(43), 0.0)),
                        &mut seen,
                        &mut held,
                        &mut sanctioned,
                        &mut trace,
                    );
                    consume_fire_visual_fact(
                        FireVisualFact::Outcome(crate::ballistics::path_fixture(shot, 43)),
                        Some(Tick(43)),
                        Some((Tick(43), 0.0)),
                        &mut seen,
                        &mut held,
                        &mut sanctioned,
                        &mut trace,
                    );
                    release_due_fire_events(
                        Tick(43),
                        Some((Tick(43), 0.0)),
                        &shooters,
                        &mut recoil,
                        &mut held,
                        &mut pending,
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

    /// The `ShotId` dedup in front of the `FireShell` trigger admits a shot once, holds every
    /// duplicate the catch-up gate can still accept, and stays bounded at `SeenShots::CAP`.
    #[test]
    fn seen_shots_retains_the_full_accepted_thirty_player_fire_horizon() {
        let mut seen = SeenShots::default();
        let oldest = shot(0);
        assert!(seen.mark_new(oldest));
        assert!(
            !seen.mark_new(oldest),
            "an echo is not new — no second shell"
        );
        assert!(
            seen.mark_new(ShotId {
                weapon: 1,
                ..oldest
            }),
            "same tank+tick, other weapon = distinct shot"
        );

        // Fixture traffic envelope: 30 players, two weapons each, at most 750 RPM per weapon.
        // DERIVED count from the catch-up owner's window and the shipped timestep.
        let ticks_per_second = 1.0 / crate::net::test_harness::TICK.as_secs_f64();
        let per_weapon = (750.0 / 60.0 * f64::from(MAX_COSMETIC_CATCH_UP_TICKS) / ticks_per_second)
            .ceil() as u32;
        let accepted_horizon = 30 * 2 * per_weapon;
        for fire_tick in 1..=accepted_horizon {
            assert!(seen.mark_new(shot(fire_tick)));
        }

        assert!(
            !seen.mark_new(oldest),
            "a duplicate that can pass catch-up validation must remain deduplicated"
        );

        for fire_tick in accepted_horizon + 1..SeenShots::CAP as u32 + 10 {
            assert!(seen.mark_new(shot(fire_tick)), "each fresh tick is new");
        }
        assert!(
            seen.order.len() <= SeenShots::CAP,
            "the ring never exceeds its cap (got {})",
            seen.order.len(),
        );
    }

    /// The backoff never exceeds the cap, no matter how many attempts have piled up — an
    /// indefinitely-retrying client still re-checks for a server at least that often. Also asserts
    /// monotonic growth so a broken schedule cannot hide behind a still-correct cap.
    #[test]
    fn reconnect_backoff_is_capped() {
        let policy = client_policy();
        assert_eq!(reconnect_backoff_secs(0), policy.reconnect_base_s.0);
        assert_eq!(
            reconnect_backoff_secs(1),
            (policy.reconnect_base_s.0 + policy.reconnect_step_s.0).min(policy.reconnect_cap_s.0)
        );
        let reaches_cap = ((policy.reconnect_cap_s.0 - policy.reconnect_base_s.0)
            / policy.reconnect_step_s.0)
            .ceil() as u32;
        assert_eq!(
            reconnect_backoff_secs(reaches_cap),
            policy.reconnect_cap_s.0
        );
        assert_eq!(
            reconnect_backoff_secs(u32::MAX),
            client_policy().reconnect_cap_s.0
        );
        for n in 0..20 {
            assert!(reconnect_backoff_secs(n) <= reconnect_backoff_secs(n + 1));
            assert!(reconnect_backoff_secs(n) <= client_policy().reconnect_cap_s.0);
        }
    }

    #[derive(Resource, Default)]
    struct CursorShells(Vec<(Option<ShotSource>, u32)>);

    fn capture_cursor_shell(fire: On<FireShell>, mut captured: ResMut<CursorShells>) {
        captured.0.push((fire.shooter, fire.catch_up_ticks));
    }

    /// A shot released at the cursor presents exactly once — one `FireShell`, one recoil
    /// kick — younger than its first tick (`catch_up_ticks` 0), which keeps it under the muzzle
    /// flash's stale gate. The fixture plants a `LocalTimeline` 16 ticks past the release as
    /// wrong-clock bait: a mutant that ages the shot on the local/arrival timeline reports 16
    /// (flash stale-gated, shell 16 ticks downrange) and reds the 0.
    #[test]
    fn a_cursor_released_shot_presents_once_younger_than_its_first_tick() {
        let mut app = App::new();
        app.init_resource::<CursorShells>();
        app.init_resource::<PendingRecoilKicks>();
        app.add_observer(capture_cursor_shell);
        let mut local = LocalTimeline::default();
        local.apply_delta(116);
        app.insert_resource(local);
        let shooter = app.world_mut().spawn_empty().id();
        let points = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_points = points.clone();
        app.add_observer(move |fire: On<FireShell>| {
            captured_points
                .lock()
                .unwrap()
                .push((fire.origin, fire.birth))
        });
        let facts = ReleasedShot {
            birth: Vec3::new(1.0, 2.0, 8.0),
            origin: Vec3::new(1.0, 2.0, 3.0),
            direction: Dir3::NEG_Z,
            round: crate::tank::WeaponRound::resolve("88mm_AP")
                .expect("a shipped round")
                .round,
            tracer: true,
            mechanism: crate::spec::FireMechanism::Single,
            shooter: ShotSource {
                tank: shooter,
                weapon: 0,
                round: 0,
            },
            shot: crate::ShotId {
                combatant: crate::CombatantId(7),
                weapon: 0,
                fire_tick: 100,
            },
        };
        let catch_up = app
            .world_mut()
            .run_system_once(
                move |mut recoil: ResMut<PendingRecoilKicks>, mut commands: Commands| {
                    present_released_shot(
                        Tick(100),
                        (Tick(100), 0.25),
                        facts,
                        &mut recoil,
                        &mut commands,
                    )
                },
            )
            .expect("the seam runs");
        assert_eq!(
            catch_up,
            Some(0),
            "released a quarter-overstep before the cursor: age < 1 tick, never the local \
             timeline's 16",
        );
        assert_eq!(
            *points.lock().unwrap(),
            vec![(Vec3::new(1.0, 2.0, 3.0), Vec3::new(1.0, 2.0, 8.0))],
            "the echoed birth and visual muzzle remain separate through presentation"
        );
        let world = app.world();
        let captured = world.resource::<CursorShells>();
        let &[(source, presented_catch_up)] = captured.0.as_slice() else {
            panic!("exactly one bang presents, got {}", captured.0.len());
        };
        let source = source.expect("the bang carries its shooter");
        assert_eq!((source.tank, source.weapon), (shooter, 0));
        assert_eq!(presented_catch_up, 0);
        assert_eq!(
            world.resource::<PendingRecoilKicks>().queued(),
            &[(shooter, 0)],
            "one kick, through the same seam as the bang",
        );
    }
}

#[cfg(test)]
mod arrival_device_tests {
    use super::super::input_arrival::FrameInput;
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    fn fixture() -> (World, Entity) {
        let mut world = World::new();
        world.init_resource::<crate::overlay::Overlays>();
        world
            .resource_mut::<crate::overlay::Overlays>()
            .declare_in_match(true);
        world.init_resource::<crate::command::DriveInput>();
        world.init_resource::<crate::command::Bindings>();
        world.init_resource::<ButtonInput<KeyCode>>();
        world.init_resource::<ButtonInput<MouseButton>>();
        world.init_resource::<DeviceCommand>();
        world.init_resource::<FrameInput>();
        world.spawn((
            PrimaryWindow,
            Window {
                focused: true,
                ..default()
            },
        ));
        let tank = world.spawn((GameControlled, TankCommand::default())).id();
        (world, tank)
    }

    /// Fast rendering must not reissue a click from the raw latch before the next fixed tick.
    #[test]
    fn a_frame_send_drains_the_device_edge_even_without_a_fixed_tick() {
        let (mut world, tank) = fixture();
        world.resource_mut::<DeviceCommand>().0.fire_primary = true;
        world.get_mut::<TankCommand>(tank).unwrap().fire_primary = true;
        world.run_system_once(feed_frame_input).unwrap();
        assert!(world.resource::<FrameInput>().command.fire_primary);
        assert!(!world.get::<TankCommand>(tank).unwrap().fire_primary);
        world.resource_mut::<DeviceCommand>().0 = *world.get::<TankCommand>(tank).unwrap();
        world.run_system_once(feed_frame_input).unwrap();
        assert!(!world.resource::<FrameInput>().command.fire_primary);
    }

    /// Device-feed consumers retain cruise on focus loss, but closing the window must neutralize
    /// drive and drain clicks. Treating a closed window as unfocused would leave cruise engaged.
    #[test]
    fn an_unfocused_window_keeps_cruise_but_a_closed_window_files_idle() {
        let (mut world, tank) = fixture();
        world.init_resource::<crate::firecontrol::Ranging>();
        world.init_resource::<Time<Real>>();
        let key = world.resource::<crate::command::Bindings>().throttle.pos;
        let mut keys = world.resource_mut::<ButtonInput<KeyCode>>();
        keys.press(key);
        keys.release(key);
        world
            .run_system_once(crate::command::gather_commands)
            .unwrap();
        let notch = world
            .resource::<crate::command::DriveInput>()
            .set_throttle();
        assert_ne!(
            notch, 0.0,
            "fixture: a tap sets cruise through the shipped gather"
        );
        let window = world
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(&world)
            .unwrap();
        world.get_mut::<Window>(window).unwrap().focused = false;
        for closed in [false, true] {
            if closed {
                world.despawn(window);
            }
            world.resource_mut::<DeviceCommand>().0 = TankCommand {
                throttle: 1.0,
                steer: 1.0,
                fire_primary: true,
                fire_secondary: true,
                ..default()
            };
            world.get_mut::<TankCommand>(tank).unwrap().fire_primary = true;
            world.run_system_once(feed_frame_input).unwrap();
            assert_eq!(
                world.resource::<FrameInput>().command,
                TankCommand {
                    throttle: if closed { 0.0 } else { notch },
                    ..default()
                }
            );
            assert!(!world.resource::<DeviceCommand>().0.has_edge());
            assert!(!world.get::<TankCommand>(tank).unwrap().has_edge());
            assert!(!world.resource::<FrameInput>().secondary_cycle);
        }
    }

    /// Bevy can expose both transitions in one poll; a final released level must retain the tap.
    #[test]
    fn the_device_feed_retains_a_sub_frame_trigger_tap() {
        let (mut world, _) = fixture();
        let binding = world.resource::<crate::command::Bindings>().fire_secondary;
        match binding {
            crate::command::ButtonBinding::Key(key) => {
                let mut keys = world.resource_mut::<ButtonInput<KeyCode>>();
                keys.press(key);
                keys.release(key);
            }
            crate::command::ButtonBinding::Mouse(button) => {
                let mut mouse = world.resource_mut::<ButtonInput<MouseButton>>();
                mouse.press(button);
                mouse.release(button);
            }
        }
        world.run_system_once(feed_frame_input).unwrap();
        assert!(world.resource::<FrameInput>().secondary_cycle);
        assert!(!world.resource::<FrameInput>().command.fire_secondary);
    }
}

#[cfg(test)]
pub(super) mod refusals;
