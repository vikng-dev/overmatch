//! Hosted mode: the dedicated server as a match started by the matchmaking Worker.
//!
//! The Worker's game-server contract (`backend/README.md`, "Game-server contract";
//! `backend/contract/server-env.json`, `heartbeat.request.json`, `end.request.json`) is the spec.
//! Hosted mode is on exactly when `OVERMATCH_SERVER_ID` is set; without it the server runs as it
//! always has (all-zero netcode key, no reporting, no player cap below lightyear's default).
//!
//! Entry points, in `server::run` order: [`HostedConfig::from_env`] (fatal on a malformed value),
//! [`install_panic_hook`] (fatal panic reporting before app construction),
//! [`Hosted::start`] (spawns the reporter thread, idle until the socket listens),
//! [`Hosted::install`] (the Bevy systems), [`Hosted::finish`] (after `App::run` returns: stops the
//! reporter and posts `end` unless the Worker already forgot this server).
//!
//! Normal HTTP reports use [`HTTP_TIMEOUT`]; panic reporting uses [`PANIC_REPORT_TIMEOUT`].
//! The fixed loop only reads and writes atomics. Hosting code never logs configured secret values;
//! arbitrary panic output is preserved locally and sanitised by the Worker before storage.

use core::fmt;
use core::time::Duration;
use std::backtrace::Backtrace;
use std::ffi::OsString;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Instant;

use bevy::prelude::*;
use lightyear::prelude::server::{ClientOf, NetcodeServer, Started};
use lightyear::prelude::{Connected, Linked};
use serde::Serialize;

use super::admission::JoinAdmission;

/// Resolution: cap HTTP work so a failed Worker cannot stall shutdown indefinitely.
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

/// Resolution: allow the Worker's deployed log-fetch timeout plus time for archive writes.
/// The whole hook shares this deadline, including default output, capture, HTTP and failure logging.
const PANIC_REPORT_TIMEOUT: Duration = Duration::from_secs(15);

/// Resolution: observe the deadline within 10 ms without spinning while the server is healthy.
const PANIC_WATCH_INTERVAL: Duration = Duration::from_millis(10);

static PANIC_DEADLINE: OnceLock<PanicDeadline> = OnceLock::new();

struct PanicDeadline {
    epoch: Instant,
    at_ns: AtomicU64,
}

impl PanicDeadline {
    fn arm(&self) -> bool {
        // Saturating the clock representation can only shorten the deadline, never postpone exit.
        let at = self
            .epoch
            .elapsed()
            .saturating_add(PANIC_REPORT_TIMEOUT)
            .as_nanos()
            .min(u128::from(u64::MAX)) as u64;
        self.at_ns
            .compare_exchange(0, at, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn watch(&self) {
        loop {
            let at = self.at_ns.load(Ordering::Acquire);
            if at != 0 && self.epoch.elapsed().as_nanos() >= u128::from(at) {
                panic_exit();
            }
            std::thread::sleep(PANIC_WATCH_INTERVAL);
        }
    }
}

/// No allocation, output, unwinding or Rust/C exit handlers may delay a fatal hosted panic.
fn panic_exit() -> ! {
    // SAFETY: `_exit` has no preconditions and immediately terminates the process with the
    // conventional Rust panic status. Bypassing cleanup avoids stdout/stderr locks and OS dialogs.
    unsafe { libc::_exit(101) }
}

fn await_panic_exit() {
    // Reporter-only fixtures have no installed hook. A shipped hosted process installs it first.
    while PANIC_DEADLINE
        .get()
        .is_some_and(|deadline| deadline.at_ns.load(Ordering::Acquire) != 0)
    {
        std::thread::park_timeout(PANIC_WATCH_INTERVAL);
    }
}

/// The largest `OVERMATCH_CAPACITY` accepted: lightyear netcode's own default client ceiling, 256 in
/// 0.30.1.
const MAX_CAPACITY: u32 = 256;

const SERVER_ID: &str = "OVERMATCH_SERVER_ID";
const API: &str = "OVERMATCH_API";
const NETCODE_KEY: &str = "OVERMATCH_NETCODE_KEY";
const REPORT_SECRET: &str = "OVERMATCH_REPORT_SECRET";
const CAPACITY: &str = "OVERMATCH_CAPACITY";
const HEARTBEAT_INTERVAL: &str = "OVERMATCH_HEARTBEAT_INTERVAL_S";
const EMPTY_EXIT: &str = "OVERMATCH_EMPTY_EXIT_S";

/// The hosted-mode settings, parsed and validated from the environment.
#[derive(Debug)]
pub(super) struct HostedConfig {
    /// URL-unreserved characters only (`A-Z a-z 0-9 . _ ~ -`), so it enters the report URLs
    /// verbatim.
    server_id: String,
    /// `http://` or `https://`, no trailing `/`.
    api: String,
    netcode_key: Secret<[u8; 32]>,
    report_secret: Secret<String>,
    /// `1..=MAX_CAPACITY`: lightyear's netcode handshake denies every connection past it.
    capacity: u32,
    heartbeat_interval: Duration,
    empty_exit: Duration,
}

/// A value whose `Debug` never shows it.
struct Secret<T>(T);

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// A fatal startup error. Names the variable and the expected shape, never the value.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ConfigError {
    var: &'static str,
    problem: &'static str,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.var, self.problem)
    }
}

impl HostedConfig {
    /// `Ok(None)` when `OVERMATCH_SERVER_ID` is unset: not hosted, every other variable ignored.
    pub(super) fn from_env() -> Result<Option<Self>, ConfigError> {
        Self::from_lookup(|name| std::env::var_os(name))
    }

    fn from_lookup(lookup: impl Fn(&str) -> Option<OsString>) -> Result<Option<Self>, ConfigError> {
        let read = |var: &'static str| -> Result<Option<String>, ConfigError> {
            lookup(var)
                .map(|value| {
                    value.into_string().map_err(|_| ConfigError {
                        var,
                        problem: "not valid UTF-8",
                    })
                })
                .transpose()
        };
        let required = |var: &'static str| -> Result<String, ConfigError> {
            read(var)?.ok_or(ConfigError {
                var,
                problem: "required in hosted mode",
            })
        };
        let Some(server_id) = read(SERVER_ID)? else {
            return Ok(None);
        };
        let server_id = parse_server_id(&server_id).ok_or(ConfigError {
            var: SERVER_ID,
            problem: "expected 1-256 URL-unreserved characters (A-Z a-z 0-9 . _ ~ -)",
        })?;
        let api = parse_api(&required(API)?).ok_or(ConfigError {
            var: API,
            problem: "expected an http:// or https:// base URL",
        })?;
        let netcode_key = parse_key(&required(NETCODE_KEY)?).ok_or(ConfigError {
            var: NETCODE_KEY,
            problem: "expected 64 hex characters",
        })?;
        let report_secret = parse_token(required(REPORT_SECRET)?).ok_or(ConfigError {
            var: REPORT_SECRET,
            problem: "expected a non-empty token of visible ASCII characters",
        })?;
        let capacity = required(CAPACITY)?
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=MAX_CAPACITY).contains(n))
            .ok_or(ConfigError {
                var: CAPACITY,
                problem: "expected an integer from 1 to 256",
            })?;
        let seconds = |var: &'static str, default: u64| -> Result<Duration, ConfigError> {
            match read(var)? {
                None => Ok(Duration::from_secs(default)),
                Some(raw) => parse_seconds(&raw).ok_or(ConfigError {
                    var,
                    problem: "expected a positive number of seconds, at most 86400",
                }),
            }
        };
        Ok(Some(Self {
            server_id,
            api,
            netcode_key: Secret(netcode_key),
            report_secret: Secret(report_secret),
            capacity,
            heartbeat_interval: seconds(HEARTBEAT_INTERVAL, 5)?,
            empty_exit: seconds(EMPTY_EXIT, 300)?,
        }))
    }

    /// The netcode private key the Worker hands joining clients.
    pub(super) fn netcode_key(&self) -> [u8; 32] {
        self.netcode_key.0
    }

    /// The player cap, as lightyear's `NetcodeConfig::max_clients`.
    pub(super) fn max_clients(&self) -> usize {
        self.capacity as usize
    }
}

fn parse_server_id(raw: &str) -> Option<String> {
    let unreserved = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '-');
    (!raw.is_empty() && raw.len() <= 256 && raw.chars().all(unreserved)).then(|| raw.to_owned())
}

fn parse_api(raw: &str) -> Option<String> {
    let base = raw.trim_end_matches('/');
    let host = base
        .strip_prefix("https://")
        .or_else(|| base.strip_prefix("http://"))?;
    (!host.is_empty() && base.chars().all(|c| c.is_ascii_graphic())).then(|| base.to_owned())
}

fn parse_key(raw: &str) -> Option<[u8; 32]> {
    let (pairs, []) = raw.as_bytes().as_chunks::<2>() else {
        return None;
    };
    if pairs.len() != 32 {
        return None;
    }
    let nibble = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut key = [0u8; 32];
    for (byte, [high, low]) in key.iter_mut().zip(pairs) {
        *byte = nibble(*high)? << 4 | nibble(*low)?;
    }
    Some(key)
}

/// A bearer token must be a valid header value: visible ASCII, no spaces.
fn parse_token(raw: String) -> Option<String> {
    (!raw.is_empty() && raw.bytes().all(|b| b.is_ascii_graphic())).then_some(raw)
}

fn parse_seconds(raw: &str) -> Option<Duration> {
    let secs = raw.parse::<f64>().ok()?;
    (secs.is_finite() && secs > 0.0 && secs <= 86_400.0).then(|| Duration::from_secs_f64(secs))
}

/// The heartbeat body: `backend/contract/heartbeat.request.json`, a full record every time.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
struct Heartbeat {
    players: u32,
    capacity: u32,
    state: ServerState,
}

/// `open` accepts joins; `closed` keeps the Battle live without joins; `closing` exits.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum ServerState {
    Open,
    Closed,
    Closing,
}

/// The end body: `backend/contract/end.request.json`.
#[derive(Serialize)]
struct End<'a> {
    reason: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    crash: Option<&'a str>,
}

/// How the Worker answered a report.
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    /// 2xx.
    Accepted,
    /// 410: the Worker does not know this server; it must exit without reporting again.
    Unknown,
    /// Any other status (401 bad secret, 400, 5xx…).
    Refused(u16),
}

/// The two report calls, against `{api}/v1/servers/{id}/…` with the bearer secret.
#[derive(Clone)]
struct Api {
    agent: ureq::Agent,
    base: String,
    secret: Arc<Secret<String>>,
}

impl Api {
    fn new(config: &HostedConfig, timeout: Duration) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build()
            .new_agent();
        Self {
            agent,
            base: format!("{}/v1/servers/{}", config.api, config.server_id),
            secret: Arc::new(Secret(config.report_secret.0.clone())),
        }
    }

    fn heartbeat(&self, heartbeat: &Heartbeat) -> Result<Answer, ureq::Error> {
        self.post("heartbeat", heartbeat)
    }

    fn end(&self, reason: &str) -> Result<Answer, ureq::Error> {
        self.post(
            "end",
            &End {
                reason,
                crash: None,
            },
        )
    }

    fn post(&self, call: &str, body: &impl Serialize) -> Result<Answer, ureq::Error> {
        let body = serde_json::to_string(body).map_err(std::io::Error::other)?;
        let response = self
            .agent
            .post(format!("{}/{call}", self.base))
            .header("authorization", format!("Bearer {}", self.secret.0))
            .content_type("application/json")
            .send(body)?;
        Ok(match response.status().as_u16() {
            200..=299 => Answer::Accepted,
            410 => Answer::Unknown,
            status => Answer::Refused(status),
        })
    }
}

/// Install before the hosted server constructs its app, so startup and task-thread panics report.
/// Backtrace capture stays on the panicking thread; symbolication and HTTP use a fresh helper,
/// independent of heartbeat locks and of any async runtime the panicking thread occupies.
pub(super) fn install_panic_hook(config: &HostedConfig) -> std::io::Result<()> {
    let deadline = PANIC_DEADLINE.get_or_init(|| PanicDeadline {
        epoch: Instant::now(),
        at_ns: AtomicU64::new(0),
    });
    std::thread::Builder::new()
        .name("hosting-panic-deadline".into())
        .spawn(move || deadline.watch())?;
    let api = Api::new(config, PANIC_REPORT_TIMEOUT);
    let (send, receive) = mpsc::channel::<(String, Backtrace, mpsc::Sender<bool>)>();
    std::thread::Builder::new()
        .name("hosting-crash-report".into())
        .spawn(move || {
            // No panic is the normal lifetime of a healthy server; process exit ends this helper.
            if let Ok((summary, backtrace, reply)) = receive.recv() {
                let crash = format!("{summary}\n{backtrace}");
                let accepted = matches!(
                    api.post(
                        "end",
                        &End {
                            reason: "panic",
                            crash: Some(&crash)
                        }
                    ),
                    Ok(Answer::Accepted)
                );
                // A timed-out hook is already terminating the process.
                let _ = reply.send(accepted);
            }
        })?;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // The atomic arm precedes output, allocation, locks, stack walking and network work.
        let first = deadline.arm();
        previous(info);
        if !first {
            await_panic_exit();
            return;
        }
        let (reply, answer) = mpsc::channel();
        let report = (info.to_string(), Backtrace::force_capture(), reply);
        // Await the reply while the container still exists: /end reads its log before stopping it.
        // The watchdog owns the deadline even when any operation here blocks indefinitely.
        if send.send(report).is_err()
            || !matches!(answer.recv_timeout(PANIC_REPORT_TIMEOUT), Ok(true))
        {
            let _ = writeln!(
                std::io::stderr(),
                "hosting: panic report failed or timed out"
            );
        }
        // An isolated task panic must end the process as soon as its report completes.
        panic_exit();
    }));
    Ok(())
}

/// Why the app stopped, as far as hosting cares. Unrecorded = a clean exit from elsewhere (Ctrl-C).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitReason {
    /// No connected player for the configured span.
    Empty(Duration),
    /// The Worker answered 410: no `end` call follows.
    Unknown,
}

impl ExitReason {
    /// The `end` body's reason; `None` when no `end` is sent.
    fn end_reason(self) -> Option<String> {
        match self {
            Self::Empty(span) => Some(format!("empty for {} s", span.as_secs_f64())),
            Self::Unknown => None,
        }
    }
}

/// State the fixed loop and the reporter thread share.
#[derive(Default)]
struct Shared {
    players: AtomicU32,
    closing: AtomicBool,
    closed: AtomicBool,
    unknown: AtomicBool,
    exit: Mutex<Option<ExitReason>>,
}

impl Shared {
    fn heartbeat(&self, capacity: u32) -> Heartbeat {
        Heartbeat {
            players: self.players.load(Ordering::Relaxed),
            capacity,
            state: if self.closing.load(Ordering::Relaxed) {
                ServerState::Closing
            } else if self.closed.load(Ordering::Relaxed) {
                ServerState::Closed
            } else {
                ServerState::Open
            },
        }
    }

    /// Record the first exit decision; later ones keep it.
    fn decide_exit(&self, reason: ExitReason) {
        self.closing.store(true, Ordering::Relaxed);
        let mut exit = self.exit.lock().unwrap_or_else(|p| p.into_inner());
        exit.get_or_insert(reason);
    }
}

/// What the app tells the reporter thread.
enum Control {
    /// The UDP socket is bound: start heartbeating.
    Listening,
    Stop,
}

/// The reporter loop. Idle until [`Control::Listening`]; then a heartbeat at once and every
/// `interval` from the previous one's start (never a catch-up burst after a stall), until
/// [`Control::Stop`], a dropped sender, or a 410. A failure streak logs once at its start and once
/// at its recovery.
fn report(
    api: Api,
    shared: Arc<Shared>,
    capacity: u32,
    interval: Duration,
    control: mpsc::Receiver<Control>,
) {
    let Ok(Control::Listening) = control.recv() else {
        return;
    };
    let mut failing = false;
    let mut next = Instant::now();
    loop {
        let failure = match api.heartbeat(&shared.heartbeat(capacity)) {
            Ok(Answer::Accepted) => None,
            Ok(Answer::Unknown) => {
                warn!("hosting: the Worker answered 410 to a heartbeat; shutting down");
                shared.unknown.store(true, Ordering::Relaxed);
                return;
            }
            Ok(Answer::Refused(status)) => Some(format!("HTTP {status}")),
            Err(err) => Some(err.to_string()),
        };
        match (failure, failing) {
            (Some(failure), false) => {
                warn!("hosting: heartbeat failing ({failure}); retrying every {interval:?}");
                failing = true;
            }
            (None, true) => {
                info!("hosting: heartbeat delivered again");
                failing = false;
            }
            _ => {}
        }
        let now = Instant::now();
        next = (next + interval).max(now);
        match control.recv_timeout(next - now) {
            Err(RecvTimeoutError::Timeout) | Ok(Control::Listening) => {}
            Ok(Control::Stop) | Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// The running hosted mode, owned by `server::run` outside the app so it outlives `App::run`.
pub(super) struct Hosted {
    api: Api,
    shared: Arc<Shared>,
    control: mpsc::Sender<Control>,
    reporter: JoinHandle<()>,
    empty_exit: Duration,
}

impl Hosted {
    /// Spawn the reporter thread; it sends nothing until [`Hosted::install`]'s systems see the
    /// server listening.
    pub(super) fn start(config: &HostedConfig) -> std::io::Result<Self> {
        info!(
            "hosting: server {} reporting to {} — capacity {}, heartbeat every {:?}, exit after \
             {:?} empty",
            config.server_id,
            config.api,
            config.capacity,
            config.heartbeat_interval,
            config.empty_exit
        );
        let api = Api::new(config, HTTP_TIMEOUT);
        let shared = Arc::new(Shared::default());
        let (control, receiver) = mpsc::channel();
        let reporter = std::thread::Builder::new()
            .name("hosting-reporter".into())
            .spawn({
                let (api, shared) = (api.clone(), shared.clone());
                let (capacity, interval) = (config.capacity, config.heartbeat_interval);
                move || report(api, shared, capacity, interval, receiver)
            })?;
        Ok(Self {
            api,
            shared,
            control,
            reporter,
            empty_exit: config.empty_exit,
        })
    }

    pub(super) fn install(&self, app: &mut App) {
        assert!(
            app.world().contains_resource::<JoinAdmission>(),
            "the server root must install the JoinAdmission shared with its netcode handler before hosting"
        );
        app.insert_resource(HostingLink {
            shared: self.shared.clone(),
            control: self.control.clone(),
            listening: false,
            empty: EmptyTimer::new(self.empty_exit),
        })
        .add_systems(
            Update,
            (announce_listening, track_players, exit_when_unknown).chain(),
        );
    }

    /// After `App::run` returns: stop and join the reporter, then post `end` unless the Worker
    /// answered 410. One attempt, bounded by [`HTTP_TIMEOUT`]; a server that fails to report its end
    /// is dropped by the Worker's heartbeat deadline instead.
    pub(super) fn finish(self) {
        await_panic_exit();
        // A send error means the reporter has already returned.
        let _ = self.control.send(Control::Stop);
        if self.reporter.join().is_err() {
            error!("hosting: the reporter thread panicked");
        }
        let decided = *self.shared.exit.lock().unwrap_or_else(|p| p.into_inner());
        let reason = if self.shared.unknown.load(Ordering::Relaxed) {
            None
        } else {
            decided.map_or(Some("shutdown".to_owned()), ExitReason::end_reason)
        };
        if let Some(reason) = reason {
            match self.api.end(&reason) {
                Ok(Answer::Accepted) => info!("hosting: reported end ({reason})"),
                Ok(answer) => warn!("hosting: end ({reason}) answered {answer:?}"),
                Err(err) => warn!("hosting: end ({reason}) failed: {err}"),
            }
        } else {
            info!("hosting: exiting without an end call (the Worker no longer knows this server)");
        }
        // A panic can arm while shutdown waits for heartbeat/end HTTP; it owns process exit now.
        await_panic_exit();
    }
}

/// The app's side of [`Hosted`].
#[derive(Resource)]
struct HostingLink {
    shared: Arc<Shared>,
    control: mpsc::Sender<Control>,
    listening: bool,
    empty: EmptyTimer,
}

/// Fires once the player count has stayed zero for `limit`, measured from the first empty
/// observation after the last occupied one.
struct EmptyTimer {
    limit: Duration,
    empty_since: Option<Duration>,
}

impl EmptyTimer {
    fn new(limit: Duration) -> Self {
        Self {
            limit,
            empty_since: None,
        }
    }

    fn expired(&mut self, players: u32, now: Duration) -> bool {
        if players > 0 {
            self.empty_since = None;
            return false;
        }
        now.saturating_sub(*self.empty_since.get_or_insert(now)) >= self.limit
    }
}

/// Wake the reporter once the netcode server has started and its UDP socket is bound.
fn announce_listening(
    server: Query<(), (With<NetcodeServer>, With<Started>, With<Linked>)>,
    mut link: ResMut<HostingLink>,
) {
    if link.listening || server.is_empty() {
        return;
    }
    link.listening = true;
    info!("hosting: listening; heartbeats start");
    // A send error means the reporter has already returned (410); `exit_when_unknown` handles it.
    let _ = link.control.send(Control::Listening);
}

/// Publish the connected-player count and exit after `OVERMATCH_EMPTY_EXIT_S` with none.
fn track_players(
    clients: Query<(), (With<ClientOf>, With<Connected>)>,
    admission: Res<JoinAdmission>,
    battles: Query<&super::battle::BattleState>,
    time: Res<Time<Real>>,
    mut link: ResMut<HostingLink>,
    mut exit: MessageWriter<AppExit>,
) {
    let players = clients.iter().count() as u32;
    link.shared.players.store(players, Ordering::Relaxed);
    link.shared
        .closed
        .store(!admission.is_open(), Ordering::Relaxed);
    let limit = link.empty.limit;
    let battle = battles
        .single()
        .expect("the hosted server starts its Battle before tracking players");
    if link.empty.expired(players, time.elapsed()) && !battle.is_combat() {
        info!("hosting: no players for {limit:?}; exiting");
        link.shared.decide_exit(ExitReason::Empty(limit));
        exit.write(AppExit::Success);
    }
}

fn exit_when_unknown(link: Res<HostingLink>, mut exit: MessageWriter<AppExit>) {
    if link.shared.unknown.load(Ordering::Relaxed) {
        link.shared.decide_exit(ExitReason::Unknown);
        exit.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpListener};
    use std::path::Path;

    use bevy::time::TimeUpdateStrategy;
    use lightyear::prelude::client::{Client, ClientPlugins, Connect, NetcodeClient};
    use lightyear::prelude::server::{ServerPlugins, ServerUdpIo, Start};
    use lightyear::prelude::*;

    use super::*;
    use crate::net::protocol::protocol_id;
    use crate::net::test_harness::{TICK, base_app, finish, free_port, lock_real_udp_test};

    fn contract(name: &str) -> serde_json::Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("backend/contract")
            .join(name);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("{} must be readable: {err}", path.display()));
        serde_json::from_str(&text).expect("contract files are JSON")
    }

    /// The contract's own example environment, as a lookup.
    fn contract_env() -> BTreeMap<String, String> {
        contract("server-env.json")
            .as_object()
            .expect("server-env.json is an object")
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    v.as_str().expect("env values are strings").into(),
                )
            })
            .collect()
    }

    fn parse(env: &BTreeMap<String, String>) -> Result<Option<HostedConfig>, ConfigError> {
        HostedConfig::from_lookup(|name| env.get(name).map(OsString::from))
    }

    fn hosted(env: &BTreeMap<String, String>) -> HostedConfig {
        parse(env).expect("valid environment").expect("hosted mode")
    }

    #[test]
    fn the_contract_environment_parses_into_hosted_mode() {
        let env = contract_env();
        let config = hosted(&env);
        assert_eq!(config.server_id, env[SERVER_ID]);
        assert_eq!(config.api, env[API]);
        assert_eq!(config.report_secret.0, env[REPORT_SECRET]);
        assert_eq!(config.capacity, 10);
        assert_eq!(config.max_clients(), 10);
        assert_eq!(config.heartbeat_interval, Duration::from_secs(5));
        assert_eq!(config.empty_exit, Duration::from_secs(300));
        let key = config.netcode_key();
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, env[NETCODE_KEY].to_ascii_lowercase());
        let stats = &env["OVERMATCH_SERVER_STATS_S"];
        assert_eq!(
            super::super::server_stats::period_from(Some(stats)).expect("valid stats contract"),
            Some(Duration::from_secs_f64(stats.parse().unwrap())),
            "the Worker enables the shipped server's stats reader"
        );
    }

    #[test]
    fn without_a_server_id_the_server_is_not_hosted() {
        let mut env = contract_env();
        env.remove(SERVER_ID);
        assert!(parse(&env).expect("not an error").is_none());
        assert!(parse(&BTreeMap::new()).expect("not an error").is_none());
    }

    #[test]
    fn optional_settings_take_their_contract_defaults() {
        let mut env = contract_env();
        for var in [HEARTBEAT_INTERVAL, EMPTY_EXIT] {
            env.remove(var);
        }
        let config = hosted(&env);
        assert_eq!(config.capacity, 10);
        assert_eq!(config.heartbeat_interval, Duration::from_secs(5));
        assert_eq!(config.empty_exit, Duration::from_secs(300));
        env.insert(API.into(), "http://127.0.0.1:8787/".into());
        env.insert(HEARTBEAT_INTERVAL.into(), "2.5".into());
        let config = hosted(&env);
        assert_eq!(config.api, "http://127.0.0.1:8787");
        assert_eq!(config.heartbeat_interval, Duration::from_millis(2500));
    }

    /// The operator must state capacity in hosted mode; omitting it refuses startup and names
    /// the variable, so the Worker and the server never silently disagree about admission.
    #[test]
    fn a_malformed_setting_names_its_variable_and_never_its_value() {
        for var in [API, NETCODE_KEY, REPORT_SECRET, CAPACITY] {
            let mut env = contract_env();
            env.remove(var);
            let err = parse(&env).expect_err("missing required setting");
            assert_eq!(err.var, var);
            assert!(err.to_string().starts_with(var), "{err}");
        }
        // 62 hex characters: secret-shaped, one byte short of a netcode key.
        let secret_shaped = "41d7c2e9b0a5f3e8d1c6b4a2f9e7d5c3b1a0f8e6d4c2b0a9f7e5d3c1b9a7f5";
        let cases: [(&str, String); 13] = [
            (SERVER_ID, String::new()),
            (SERVER_ID, "a/b".into()),
            (API, "matchmaking.example".into()),
            (API, "https:///".into()),
            (NETCODE_KEY, secret_shaped.into()),
            (NETCODE_KEY, format!("{secret_shaped}zz")),
            (REPORT_SECRET, format!("{secret_shaped} x")),
            (REPORT_SECRET, String::new()),
            (CAPACITY, "-3".into()),
            (CAPACITY, "257".into()),
            (HEARTBEAT_INTERVAL, "0.0".into()),
            (HEARTBEAT_INTERVAL, "NaN".into()),
            (EMPTY_EXIT, "-5".into()),
        ];
        for (var, value) in cases {
            let mut env = contract_env();
            env.insert(var.into(), value.clone());
            let err = parse(&env).expect_err("malformed setting");
            assert_eq!(err.var, var, "{value:?}");
            let text = format!("{err} {err:?}");
            assert!(text.contains(var), "{text}");
            if !value.is_empty() {
                assert!(!text.contains(&value), "error text leaks the value: {text}");
            }
            assert!(!text.contains(secret_shaped), "{text}");
        }
    }

    #[test]
    fn the_config_debug_string_hides_both_secrets() {
        let env = contract_env();
        let text = format!("{:?}", hosted(&env));
        assert!(!text.contains(&env[REPORT_SECRET]), "{text}");
        assert!(!text.contains(&env[NETCODE_KEY]), "{text}");
        let key = hosted(&env).netcode_key();
        assert!(!text.contains(&format!("{key:?}")), "{text}");
    }

    /// One HTTP request as the fake Worker saw it.
    #[derive(Debug, Clone)]
    struct Seen {
        method: String,
        path: String,
        headers: BTreeMap<String, String>,
        body: serde_json::Value,
    }

    /// A one-request-per-connection HTTP/1.1 responder on loopback, answering each request with the
    /// next status of `statuses` (the last repeats).
    struct FakeWorker {
        base: String,
        seen: mpsc::Receiver<Seen>,
    }

    impl FakeWorker {
        fn start(statuses: Vec<u16>) -> Self {
            Self::with_reply_gate(statuses, None)
        }

        fn with_reply_gate(statuses: Vec<u16>, mut gate: Option<mpsc::Receiver<()>>) -> Self {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback TCP");
            let base = format!("http://{}", listener.local_addr().unwrap());
            let (sender, seen) = mpsc::channel();
            std::thread::spawn(move || {
                for (index, stream) in listener.incoming().enumerate() {
                    let Ok(mut stream) = stream else { return };
                    let sender = sender.clone();
                    let gate = gate.take();
                    let status = statuses[index.min(statuses.len() - 1)];
                    std::thread::spawn(move || {
                        let mut reader = BufReader::new(stream.try_clone().unwrap());
                        let mut line = String::new();
                        reader.read_line(&mut line).unwrap();
                        let mut parts = line.split_whitespace();
                        let method = parts.next().unwrap_or_default().to_owned();
                        let path = parts.next().unwrap_or_default().to_owned();
                        let mut headers = BTreeMap::new();
                        loop {
                            let mut header = String::new();
                            reader.read_line(&mut header).unwrap();
                            let header = header.trim_end();
                            if header.is_empty() {
                                break;
                            }
                            let (name, value) = header.split_once(':').unwrap();
                            headers
                                .insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
                        }
                        let length = headers
                            .get("content-length")
                            .map_or(0, |n| n.parse::<usize>().unwrap());
                        let mut body = vec![0; length];
                        reader.read_exact(&mut body).unwrap();
                        let request = Seen {
                            method,
                            path,
                            headers,
                            body: serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
                        };
                        let held = gate.is_some();
                        if let Some(gate) = gate {
                            sender.send(request.clone()).unwrap();
                            gate.recv_timeout(PANIC_REPORT_TIMEOUT).unwrap();
                        }
                        write!(
                            stream,
                            "HTTP/1.1 {status} X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                        )
                        .unwrap();
                        if !held {
                            // The observational channel closes when the parent test has finished.
                            let _ = sender.send(request);
                        }
                    });
                }
            });
            Self { base, seen }
        }

        fn next(&self) -> Seen {
            self.seen
                .recv_timeout(Duration::from_secs(10))
                .expect("the fake Worker saw no request")
        }
    }

    fn hosted_against(worker: &FakeWorker, interval: Duration) -> HostedConfig {
        let mut env = contract_env();
        env.insert(API.into(), worker.base.clone());
        env.insert(
            HEARTBEAT_INTERVAL.into(),
            interval.as_secs_f64().to_string(),
        );
        hosted(&env)
    }

    fn assert_authorized_json_post(seen: &Seen, path: &str) {
        let env = contract_env();
        assert_eq!(seen.method, "POST");
        assert_eq!(seen.path, path);
        assert_eq!(
            seen.headers.get("authorization"),
            Some(&format!("Bearer {}", env[REPORT_SECRET]))
        );
        assert_eq!(
            seen.headers.get("content-type").map(String::as_str),
            Some("application/json")
        );
    }

    // Only mutual exclusion is shared; a failed assertion must not poison another child's checks.
    static PANIC_CHILD_LOCK: Mutex<()> = Mutex::new(());

    /// Runs in a subprocess because a panic hook belongs to the whole process, including tests.
    #[test]
    #[ignore = "subprocess fixture for panic reporting"]
    fn panic_hook_child() {
        let config = HostedConfig::from_env().unwrap().unwrap();
        let previous = std::panic::take_hook();
        let both = std::sync::Barrier::new(2);
        std::panic::set_hook(Box::new(move |info| {
            previous(info);
            both.wait();
        }));
        install_panic_hook(&config).unwrap();
        std::thread::spawn(|| panic!("hosted panic fixture A"));
        std::thread::spawn(|| panic!("hosted panic fixture B"));
        // Keep the fixture alive while its parents check concurrent reporting.
        loop {
            std::thread::park();
        }
    }

    #[test]
    #[ignore = "subprocess fixture: exactly one isolated panic"]
    fn single_panic_child() {
        let config = HostedConfig::from_env().unwrap().unwrap();
        install_panic_hook(&config).unwrap();
        std::thread::spawn(|| panic!("single hosted panic fixture"));
        loop {
            std::thread::park();
        }
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "subprocess fixture: an undrained stderr pipe"]
    fn blocked_stderr_child() {
        let config = HostedConfig::from_env().unwrap().unwrap();
        install_panic_hook(&config).unwrap();
        // Fixture writes saturate the actual pipe before the default hook can write to it.
        let bytes = [b'x'; 4096];
        // SAFETY: stderr is an open inherited pipe; the buffer is valid for every write. Restoring
        // its original descriptor flags makes the subsequent panic output block on that full pipe.
        unsafe {
            let flags = libc::fcntl(libc::STDERR_FILENO, libc::F_GETFL);
            assert_ne!(flags, -1);
            assert_ne!(
                libc::fcntl(libc::STDERR_FILENO, libc::F_SETFL, flags | libc::O_NONBLOCK),
                -1
            );
            while libc::write(libc::STDERR_FILENO, bytes.as_ptr().cast(), bytes.len()) >= 0 {}
            assert_eq!(
                std::io::Error::last_os_error().kind(),
                std::io::ErrorKind::WouldBlock
            );
            assert_ne!(libc::fcntl(libc::STDERR_FILENO, libc::F_SETFL, flags), -1);
        }
        panic!("stderr must not hold a crashing process alive");
    }

    #[test]
    #[ignore = "subprocess fixture: heartbeat retirement during a panic"]
    fn shutdown_during_panic_child() {
        let config = HostedConfig::from_env().unwrap().unwrap();
        install_panic_hook(&config).unwrap();
        let hosting = Hosted::start(&config).unwrap();
        std::thread::spawn(|| panic!("shutdown race panic fixture"));
        let mut command = [0];
        std::io::stdin().read_exact(&mut command).unwrap();
        hosting.control.send(Control::Listening).unwrap();
        while !hosting.reporter.is_finished() {
            std::thread::sleep(Duration::from_millis(1));
        }
        hosting.finish();
        // The shipped main also treats a successful return as process success.
        std::process::exit(0);
    }

    #[test]
    #[ignore = "subprocess fixture for the shipped server entry point"]
    fn hosted_entry_child() {
        crate::run_server();
    }

    fn panic_child(worker: &FakeWorker, entry: &str) -> std::process::Child {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", entry, "--ignored", "--nocapture"])
            .envs(contract_env())
            .env(API, &worker.base)
            .env("OVERMATCH_UNATTENDED", "1")
            .env("OVERMATCH_WINDOW", "headless")
            .env("OVERMATCH_MUTE", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        command.spawn().unwrap()
    }

    fn wait_for_panic_child(child: std::process::Child) -> std::process::Output {
        wait_for_child_within(child, PANIC_REPORT_TIMEOUT + Duration::from_secs(5))
    }

    fn wait_for_child_within(
        mut child: std::process::Child,
        allowance: Duration,
    ) -> std::process::Output {
        let deadline = Instant::now() + allowance;
        loop {
            if child.try_wait().unwrap().is_some() {
                let output = child.wait_with_output().unwrap();
                assert!(
                    !output.status.success(),
                    "reporting must not swallow the panic"
                );
                return output;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!(
                    "panic reporting hung the process: {}",
                    String::from_utf8_lossy(&output.stderr[..output.stderr.len().min(512)])
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The operator's panic deadline includes default output blocked behind a full stderr pipe.
    #[cfg(unix)]
    #[test]
    fn panic_deadline_covers_a_full_undrained_stderr_pipe() {
        let _child = PANIC_CHILD_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let worker = FakeWorker::start(vec![204]);
        let output = wait_for_panic_child(panic_child(
            &worker,
            "net::hosting::tests::blocked_stderr_child",
        ));
        assert_eq!(output.status.code(), Some(101));
    }

    /// The first panic must exit after its reply; a second panic or deadline must not mask its exit.
    #[test]
    fn single_panic_exits_after_its_report_without_waiting_for_the_watchdog() {
        let _child = PANIC_CHILD_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let worker = FakeWorker::start(vec![204]);
        let child = panic_child(&worker, "net::hosting::tests::single_panic_child");
        assert!(worker.next().path.ends_with("/end"));
        // Fixture allowance is shorter than the panic deadline, exposing a missing first exit.
        let output = wait_for_child_within(child, Duration::from_secs(2));
        assert_eq!(output.status.code(), Some(101));
    }

    /// A real heartbeat 410 may retire the app while its panic report still awaits log capture.
    #[test]
    fn clean_finish_waits_for_an_inflight_panic_and_cannot_return_success() {
        let _child = PANIC_CHILD_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (release, gate) = mpsc::channel();
        let worker = FakeWorker::with_reply_gate(vec![204, 410], Some(gate));
        let mut child = panic_child(&worker, "net::hosting::tests::shutdown_during_panic_child");
        assert!(worker.next().path.ends_with("/end"));
        child.stdin.as_mut().unwrap().write_all(b"x").unwrap();
        assert!(worker.next().path.ends_with("/heartbeat"));
        std::thread::sleep(Duration::from_millis(100));
        let alive_until_reply = child.try_wait().unwrap().is_none();
        release.send(()).unwrap();
        let output = wait_for_child_within(child, Duration::from_secs(2));
        assert!(
            alive_until_reply,
            "clean shutdown cut off the panic report before the Worker replied"
        );
        assert_eq!(output.status.code(), Some(101));
    }

    /// The operator needs the first panic's payload and source even when another thread panics.
    #[test]
    fn panic_reports_message_location_and_backtrace_once_and_preserves_default_output() {
        let _child = PANIC_CHILD_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (release, gate) = mpsc::channel();
        let worker = FakeWorker::with_reply_gate(vec![204], Some(gate));
        let mut child = panic_child(&worker, "net::hosting::tests::panic_hook_child");
        let seen = worker.next();
        // Fixture delay exposes a hook that sends but exits before the Worker captures the log.
        std::thread::sleep(Duration::from_millis(100));
        let alive_until_reply = child.try_wait().unwrap().is_none();
        release.send(()).unwrap();
        let output = wait_for_panic_child(child);
        assert!(
            alive_until_reply,
            "the Worker must finish log capture before the process exits"
        );
        let config = hosted(&contract_env());
        assert_authorized_json_post(&seen, &format!("/v1/servers/{}/end", config.server_id));
        assert_eq!(seen.body["reason"], "panic");
        let crash = seen.body["crash"].as_str().unwrap();
        assert!(crash.contains("hosted panic fixture"), "{crash}");
        let location = crash
            .split("src/net/hosting.rs:")
            .nth(1)
            .expect("panic source file");
        let mut coordinates = location.split(':');
        assert!(
            coordinates.next().unwrap().parse::<u32>().is_ok(),
            "panic line: {crash}"
        );
        assert!(
            coordinates.next().unwrap().parse::<u32>().is_ok(),
            "panic column: {crash}"
        );
        assert!(
            crash.contains("panic_hook_child"),
            "backtrace must capture the panicking thread: {crash}"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("hosted panic fixture A") && stderr.contains("hosted panic fixture B")
        );
        assert!(
            worker.seen.recv_timeout(Duration::from_millis(50)).is_err(),
            "a second panic must not report again"
        );
    }

    /// Refusal and a Worker that never replies cannot keep a crashing process alive.
    #[test]
    fn failed_and_stalled_panic_reports_release_the_panicking_thread_within_the_bound() {
        let _child = PANIC_CHILD_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let refused = FakeWorker::start(vec![503]);
        let output = wait_for_panic_child(panic_child(
            &refused,
            "net::hosting::tests::panic_hook_child",
        ));
        assert_eq!(
            String::from_utf8_lossy(&output.stderr)
                .matches("hosting: panic report failed or timed out")
                .count(),
            1,
            "an HTTP refusal logs once despite concurrent panics"
        );
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let (_send, seen) = mpsc::channel();
        let stalled = FakeWorker {
            base: format!("http://{}", listener.local_addr().unwrap()),
            seen,
        };
        // The listening socket accepts TCP handshakes but never reads or answers HTTP.
        let start = Instant::now();
        let output = wait_for_panic_child(panic_child(
            &stalled,
            "net::hosting::tests::panic_hook_child",
        ));
        assert!(start.elapsed() < PANIC_REPORT_TIMEOUT + Duration::from_secs(5));
        assert_eq!(
            output.status.code(),
            Some(101),
            "the watchdog owns a stalled report's exit"
        );
    }

    /// Deleting the hook installation from server::run must lose this real startup refusal report.
    #[test]
    fn shipped_hosted_entry_reports_a_startup_asset_panic() {
        let _child = PANIC_CHILD_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let worker = FakeWorker::start(vec![204]);
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        let empty =
            std::env::temp_dir().join(format!("overmatch-crash-report-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        let child = command
            .args([
                "--exact",
                "net::hosting::tests::hosted_entry_child",
                "--ignored",
                "--nocapture",
            ])
            .envs(contract_env())
            .env(API, &worker.base)
            .env("BEVY_ASSET_ROOT", &empty)
            .env("OVERMATCH_UNATTENDED", "1")
            .env("OVERMATCH_WINDOW", "headless")
            .env("OVERMATCH_MUTE", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let output = wait_for_panic_child(child);
        std::fs::remove_dir(&empty).unwrap();
        let report = worker.next();
        assert!(report.path.ends_with("/end"));
        assert_eq!(report.body["reason"], "panic");
        assert!(
            report.body["crash"].as_str().unwrap().contains("assets"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn heartbeat_and_end_requests_match_the_contract() {
        let worker = FakeWorker::start(vec![204]);
        let config = hosted_against(&worker, Duration::from_secs(5));
        let api = Api::new(&config, HTTP_TIMEOUT);
        let id = &config.server_id;

        let heartbeat = Heartbeat {
            players: 4,
            capacity: config.capacity,
            state: ServerState::Open,
        };
        assert_eq!(api.heartbeat(&heartbeat).unwrap(), Answer::Accepted);
        let seen = worker.next();
        assert_authorized_json_post(&seen, &format!("/v1/servers/{id}/heartbeat"));
        assert_eq!(seen.body, contract("heartbeat.request.json"));

        let reason = ExitReason::Empty(config.empty_exit).end_reason().unwrap();
        assert_eq!(api.end(&reason).unwrap(), Answer::Accepted);
        let seen = worker.next();
        assert_authorized_json_post(&seen, &format!("/v1/servers/{id}/end"));
        assert_eq!(seen.body, contract("end.request.json"));
    }

    #[test]
    fn the_closing_state_serializes_as_the_contract_spells_it() {
        let shared = Shared::default();
        shared.players.store(3, Ordering::Relaxed);
        assert_eq!(
            serde_json::to_value(shared.heartbeat(10)).unwrap()["state"],
            "open"
        );
        shared.players.store(4, Ordering::Relaxed);
        shared.closed.store(true, Ordering::Relaxed);
        assert_eq!(
            serde_json::to_value(shared.heartbeat(30)).expect("heartbeat JSON"),
            contract("heartbeat.closed.request.json")
        );
        shared.decide_exit(ExitReason::Empty(Duration::from_secs(1)));
        let body = serde_json::to_value(shared.heartbeat(10)).unwrap();
        assert_eq!(body["state"], "closing");
        assert_eq!(body["players"], 4);
    }

    #[test]
    fn the_reporter_waits_for_listening_then_survives_failures() {
        let worker = FakeWorker::start(vec![503, 500, 204, 204]);
        let config = hosted_against(&worker, Duration::from_millis(20));
        let hosted = Hosted::start(&config).expect("the test reporter thread starts");
        assert!(
            worker
                .seen
                .recv_timeout(Duration::from_millis(200))
                .is_err(),
            "no heartbeat before the socket listens"
        );
        hosted.control.send(Control::Listening).unwrap();
        for _ in 0..4 {
            assert!(worker.next().path.ends_with("/heartbeat"));
        }
        assert!(!hosted.shared.unknown.load(Ordering::Relaxed));
        hosted.finish();
        let end = worker.next();
        assert!(end.path.ends_with("/end"));
        assert_eq!(end.body["reason"], "shutdown");
    }

    #[test]
    fn a_410_stops_the_reporter_exits_the_app_and_skips_end() {
        let worker = FakeWorker::start(vec![410]);
        let config = hosted_against(&worker, Duration::from_millis(20));
        let hosted = Hosted::start(&config).expect("the test reporter thread starts");
        hosted.control.send(Control::Listening).unwrap();
        assert!(worker.next().path.ends_with("/heartbeat"));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !hosted.reporter.is_finished() {
            assert!(
                Instant::now() < deadline,
                "the reporter kept running after 410"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(hosted.shared.unknown.load(Ordering::Relaxed));

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<AppExit>()
            .init_resource::<JoinAdmission>();
        app.world_mut()
            .spawn(super::super::battle::BattleState::default());
        hosted.install(&mut app);
        app.update();
        assert_eq!(app.should_exit(), Some(AppExit::Success));

        // `finish` posts `end` synchronously, so any call it made is already answered; the wait
        // covers only the fake Worker's hand-off after its reply.
        hosted.finish();
        assert!(
            worker.seen.recv_timeout(Duration::from_millis(50)).is_err(),
            "no end call after a 410"
        );
    }

    #[test]
    fn the_empty_timer_fires_only_after_a_full_empty_span() {
        let limit = Duration::from_secs(300);
        let mut timer = EmptyTimer::new(limit);
        let at = Duration::from_secs;
        assert!(!timer.expired(0, at(10)));
        assert!(!timer.expired(0, at(309)));
        assert!(!timer.expired(2, at(309)), "a player resets the span");
        assert!(!timer.expired(0, at(320)));
        assert!(!timer.expired(0, at(619)));
        assert!(timer.expired(0, at(620)));
        assert_eq!(
            ExitReason::Empty(limit).end_reason().as_deref(),
            Some("empty for 300 s")
        );
    }

    #[test]
    fn the_empty_exit_writes_app_exit_and_records_its_reason() {
        let worker = FakeWorker::start(vec![204]);
        let mut config = hosted_against(&worker, Duration::from_secs(5));
        config.empty_exit = Duration::from_millis(50);
        let hosted = Hosted::start(&config).expect("the test reporter thread starts");
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<AppExit>()
            .init_resource::<JoinAdmission>();
        app.world_mut()
            .spawn(super::super::battle::BattleState::default());
        hosted.install(&mut app);
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.should_exit().is_none() {
            assert!(Instant::now() < deadline, "the empty exit never fired");
            app.update();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(hosted.shared.closing.load(Ordering::Relaxed));
        hosted.finish();
        let end = worker.next();
        assert!(end.path.ends_with("/end"));
        assert_eq!(end.body["reason"], "empty for 0.05 s");
    }

    /// The hosting empty timeout must not shorten the Battle's promised full combat span.
    /// Once combat reaches results, an already-expired empty timeout can retire the server.
    #[test]
    fn an_empty_server_finishes_combat_before_its_empty_timeout_retires_it() {
        use crate::net::battle::{BattleOutcome, BattlePhase, BattleState};
        let mut app = App::new();
        let shared = Arc::new(Shared::default());
        let (control, _receiver) = mpsc::channel();
        app.add_plugins(MinimalPlugins)
            .add_message::<AppExit>()
            .init_resource::<JoinAdmission>()
            .insert_resource(TimeUpdateStrategy::ManualDuration(TICK))
            .insert_resource(HostingLink {
                shared: shared.clone(),
                control,
                listening: true,
                // Fixture timeout: expires well before the fixture combat's deadline.
                empty: EmptyTimer::new(TICK),
            })
            .add_systems(Update, track_players);
        let battle = app.world_mut().spawn(BattleState::combat_fixture()).id();
        for _ in 0..4 {
            app.update();
        }
        assert!(app.should_exit().is_none(), "empty combat runs in full");
        assert!(!shared.closing.load(Ordering::Relaxed));
        app.world_mut()
            .get_mut::<BattleState>(battle)
            .expect("Battle")
            .phase = BattlePhase::Results {
            ends_at: crate::sim_tick::SimTick::new(100),
            outcome: BattleOutcome::NoContest,
        };
        app.update();
        assert!(
            app.should_exit().is_some(),
            "expired emptiness retires after combat"
        );
        assert!(shared.closing.load(Ordering::Relaxed));
    }

    fn capped_server(port: u16, config: &HostedConfig) -> App {
        let mut app = base_app();
        app.add_plugins(ServerPlugins {
            tick_duration: TICK,
        });
        crate::net::protocol::plugin(&mut app);
        let admission = JoinAdmission::default();
        app.insert_resource(admission.clone());
        let server = app
            .world_mut()
            .spawn((
                crate::net::server::netcode_server(Some(config), admission),
                LocalAddr(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port)),
                ServerUdpIo::default(),
            ))
            .id();
        app.world_mut().commands().trigger(Start { entity: server });
        app
    }

    fn client(port: u16, client_id: u64, key: [u8; 32]) -> App {
        let mut app = base_app();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(TICK));
        app.add_plugins(ClientPlugins {
            tick_duration: TICK,
        });
        crate::net::protocol::plugin(&mut app);
        let server_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);
        let entity = app
            .world_mut()
            .spawn((
                Client,
                Link::default(),
                LocalAddr(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0)),
                PeerAddr(server_addr),
                ReplicationReceiver,
                NetcodeClient::new(
                    Authentication::Manual {
                        server_addr,
                        client_id,
                        private_key: key,
                        protocol_id: protocol_id(),
                    },
                    lightyear::prelude::client::NetcodeConfig::default(),
                )
                .expect("manual token"),
                UdpIo::default(),
            ))
            .id();
        app.world_mut().commands().trigger(Connect { entity });
        app
    }

    fn client_state(app: &mut App) -> (bool, Option<String>) {
        let world = app.world_mut();
        let connected = world
            .query_filtered::<(), (With<Client>, With<Connected>)>()
            .iter(world)
            .next()
            .is_some();
        let denied = world
            .query_filtered::<&Disconnected, With<Client>>()
            .iter(world)
            .next()
            .map(|d| d.reason.to_string());
        (connected, denied)
    }

    /// A server at capacity denies the next client at the netcode handshake (`ServerFull`), and the
    /// hosted key admits a client holding it.
    #[test]
    fn a_full_server_denies_the_next_client_at_the_handshake() {
        let _udp = lock_real_udp_test();
        let mut env = contract_env();
        env.insert(CAPACITY.into(), "1".into());
        let config = hosted(&env);
        let port = free_port();
        let mut server = capped_server(port, &config);
        let mut first = client(port, 1, config.netcode_key());
        finish(&mut server);
        finish(&mut first);
        let step = |server: &mut App, clients: &mut [&mut App]| {
            server.update();
            std::thread::sleep(Duration::from_micros(300));
            for client in clients.iter_mut() {
                client.update();
            }
            std::thread::sleep(Duration::from_micros(300));
        };
        let connected_on_server = |server: &mut App| {
            let world = server.world_mut();
            world
                .query_filtered::<(), (With<ClientOf>, With<Connected>)>()
                .iter(world)
                .count()
        };

        let mut steps = 0;
        while !client_state(&mut first).0 {
            assert!(steps < 2_000, "the first client never connected");
            step(&mut server, &mut [&mut first]);
            steps += 1;
        }
        let mut second = client(port, 2, config.netcode_key());
        finish(&mut second);
        let mut steps = 0;
        let reason = loop {
            assert!(steps < 2_000, "the second client was never denied");
            step(&mut server, &mut [&mut first, &mut second]);
            let (connected, reason) = client_state(&mut second);
            assert!(!connected, "the server admitted a client past its capacity");
            if let Some(reason) = reason.filter(|r| r.contains("ConnectionDenied")) {
                break reason;
            }
            steps += 1;
        };
        assert!(client_state(&mut first).0, "the admitted client stays");
        assert_eq!(connected_on_server(&mut server), 1, "{reason}");
    }
    /// The server's short-mode headless run uses the production clock and wire registrations:
    /// fill-up, open combat, closed combat, results and another occupied fill-up all replicate.
    /// Heartbeats follow the same admission gate. A late join is refused with its typed cause,
    /// and the production joining Text shows it while the original player remains connected.
    #[test]
    fn a_short_mode_cycles_replicates_and_refuses_a_late_join_with_its_reason() {
        use crate::CombatantId;
        use crate::net::admission::JoinRefusal;
        use crate::net::battle::{BattleOutcome, BattlePhase, BattleState, LateJoin};
        use crate::net::score::{Respawn, Scoreboard, Standing};

        let _udp = lock_real_udp_test();
        let config = hosted(&contract_env());
        let port = free_port();
        let mut server = capped_server(port, &config);
        let mode =
            crate::net::battle_mode::BattleMode::parse(include_str!("fixtures/battle-short.ron"));
        crate::net::server::install_battle(&mut server, mode, TICK);
        server.add_observer(crate::net::server::attach_replication_sender);
        let shared = Arc::new(Shared::default());
        let (control, _receiver) = mpsc::channel();
        server
            .insert_resource(HostingLink {
                shared: shared.clone(),
                control,
                listening: true,
                empty: EmptyTimer::new(Duration::from_secs(3600)),
            })
            .add_message::<AppExit>()
            .add_systems(Update, track_players);
        let mut first = client(port, 1, config.netcode_key());
        finish(&mut server);
        finish(&mut first);
        let mut late: Option<App> = None;
        let mut refused = false;
        let mut scored = false;
        let mut score_replicated = false;
        let mut server_phases = std::collections::BTreeSet::new();
        let mut client_phases = std::collections::BTreeSet::new();
        let mut published = Vec::new();
        let phase_key = |state: BattleState| {
            (
                state.number,
                match state.phase {
                    BattlePhase::FillUp { ends_at: None } => "idle",
                    BattlePhase::FillUp { ends_at: Some(_) } => "fill-up",
                    BattlePhase::Combat {
                        late_join: LateJoin::Open { .. },
                        ..
                    } => "combat-open",
                    BattlePhase::Combat {
                        late_join: LateJoin::Closed,
                        ..
                    } => "combat-closed",
                    BattlePhase::Results { .. } => "results",
                },
            )
        };
        for step in 0..2_000 {
            server.update();
            std::thread::sleep(Duration::from_micros(300));
            first.update();
            if let Some(late) = &mut late {
                late.update();
            }
            std::thread::sleep(Duration::from_micros(300));
            let state = *server
                .world_mut()
                .query::<&BattleState>()
                .single(server.world())
                .expect("one Battle");
            published.push(state);
            server_phases.insert(phase_key(state));
            let heartbeat = shared.heartbeat(config.capacity);
            assert_eq!(
                heartbeat.state,
                if state.accepts_joins() {
                    ServerState::Open
                } else {
                    ServerState::Closed
                }
            );
            if matches!(state.phase, BattlePhase::Combat { .. }) && !scored {
                // Fixture standing makes replication and the authority's result observable.
                server
                    .world_mut()
                    .query::<&mut Scoreboard>()
                    .single_mut(server.world_mut())
                    .expect("one board")
                    .0
                    .push(Standing {
                        combatant: CombatantId(77),
                        knockouts: 3,
                        losses: 1,
                        respawn: Respawn::NeverKnockedOut,
                    });
                scored = true;
            }
            if matches!(
                state.phase,
                BattlePhase::Combat {
                    late_join: LateJoin::Closed,
                    ..
                }
            ) && late.is_none()
            {
                let mut newcomer = client(port, 2, config.netcode_key());
                crate::net::client::mount_refusal_probe(&mut newcomer);
                finish(&mut newcomer);
                late = Some(newcomer);
            }
            if let BattlePhase::Results { outcome, .. } = state.phase {
                assert_eq!(outcome, BattleOutcome::Winner(CombatantId(77)));
            }
            let world = first.world_mut();
            for (received, board) in world.query::<(&BattleState, &Scoreboard)>().iter(world) {
                assert!(
                    published.contains(received),
                    "clients receive an authority state including its exact deadline"
                );
                client_phases.insert(phase_key(*received));
                score_replicated |= board.0.iter().any(|standing| {
                    standing.combatant == CombatantId(77)
                        && standing.knockouts == 3
                        && standing.losses == 1
                });
                if received.number == 2 {
                    assert!(board.0.is_empty(), "the new Battle resets its score");
                }
            }
            if let Some(late) = &mut late {
                let (reason, label) = crate::net::client::refusal_probe(late);
                if let Some(reason) = reason {
                    assert_eq!(reason, JoinRefusal::BattleClosed);
                    assert_eq!(label, reason.message());
                    assert!(!client_state(late).0, "a refused player never connects");
                    refused = true;
                }
            }
            if state.number == 2 && client_phases.contains(&(2, "fill-up")) {
                assert!(
                    client_state(&mut first).0,
                    "the admitted player stays connected across Battles"
                );
                assert!(refused && score_replicated);
                for phase in ["fill-up", "combat-open", "combat-closed", "results"] {
                    assert!(server_phases.contains(&(1, phase)), "server missed {phase}");
                    assert!(
                        client_phases.contains(&(1, phase)),
                        "replica missed {phase}"
                    );
                }
                eprintln!(
                    "MEASURED: short-mode headless run observed all cycle phases and matching heartbeats; typed late-join refusal rendered; score and exact phase deadlines replicated; completed at fixture step {step}"
                );
                return;
            }
        }
        panic!(
            "the short-mode cycle never completed: server {server_phases:?}, client {client_phases:?}"
        );
    }

    /// A ticket's handshake can start while open and return its challenge after admission closes.
    /// The second handshake stage must recheck the same gate, and preserve the refusal for the UI.
    #[test]
    fn a_challenge_issued_while_open_cannot_connect_after_admission_closes() {
        use crate::net::admission::JoinRefusal;
        let _udp = lock_real_udp_test();
        let config = hosted(&contract_env());
        let port = free_port();
        let mut server = capped_server(port, &config);
        let admission = server.world().resource::<JoinAdmission>().clone();
        let mut joining = client(port, 10, config.netcode_key());
        crate::net::client::mount_refusal_probe(&mut joining);
        finish(&mut server);
        finish(&mut joining);
        let mut challenged = false;
        for _ in 0..2_000 {
            server.update();
            if !challenged
                && !server
                    .world_mut()
                    .query_filtered::<(), (With<LinkOf>, With<Connecting>)>()
                    .iter(server.world())
                    .collect::<Vec<_>>()
                    .is_empty()
            {
                admission.set_open(false);
                challenged = true;
            }
            joining.update();
            std::thread::sleep(Duration::from_micros(300));
            let (refusal, label) = crate::net::client::refusal_probe(&mut joining);
            if let Some(refusal) = refusal {
                assert!(challenged, "the initial request passed while open");
                assert_eq!(refusal, JoinRefusal::BattleClosed);
                assert_eq!(label, refusal.message());
                assert!(!client_state(&mut joining).0);
                assert_eq!(
                    server
                        .world_mut()
                        .query_filtered::<(), (With<ClientOf>, With<Connected>)>()
                        .iter(server.world())
                        .count(),
                    0
                );
                return;
            }
        }
        panic!("the challenge-crossing refusal never arrived");
    }
}
