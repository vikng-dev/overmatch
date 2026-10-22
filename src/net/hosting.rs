//! Hosted mode: the dedicated server as a match started by the matchmaking Worker.
//!
//! The Worker's game-server contract (`backend/README.md`, "Game-server contract";
//! `backend/contract/server-env.json`, `heartbeat.request.json`, `end.request.json`) is the spec.
//! Hosted mode is on exactly when `OVERMATCH_SERVER_ID` is set; without it the server runs as it
//! always has (all-zero netcode key, no reporting, no player cap below lightyear's default).
//!
//! Entry points, in `server::run` order: [`HostedConfig::from_env`] (fatal on a malformed value),
//! [`Hosted::start`] (spawns the reporter thread, idle until the socket listens),
//! [`Hosted::install`] (the Bevy systems), [`Hosted::finish`] (after `App::run` returns: stops the
//! reporter and posts `end` unless the Worker already forgot this server).
//!
//! Every HTTP call runs on the reporter thread or after the app has stopped, bounded by
//! [`HTTP_TIMEOUT`]; the fixed loop only reads and writes atomics. No secret value (report secret,
//! netcode key) reaches a log line, an error message or a `Debug` string.

use core::fmt;
use core::time::Duration;
use std::ffi::OsString;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use bevy::prelude::*;
use lightyear::prelude::server::{ClientOf, NetcodeServer, Started};
use lightyear::prelude::{Connected, Linked};
use serde::Serialize;

/// Upper bound on one heartbeat or end call, connect to last body byte.
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

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
        let capacity = match read(CAPACITY)? {
            None => 10,
            Some(raw) => raw
                .parse::<u32>()
                .ok()
                .filter(|n| (1..=MAX_CAPACITY).contains(n))
                .ok_or(ConfigError {
                    var: CAPACITY,
                    problem: "expected an integer from 1 to 256",
                })?,
        };
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

/// `open` accepts joins; `closing` tells the Worker this server has decided to exit.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum ServerState {
    Open,
    Closing,
}

/// The end body: `backend/contract/end.request.json`.
#[derive(Serialize)]
struct End<'a> {
    reason: &'a str,
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
    fn new(config: &HostedConfig) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(HTTP_TIMEOUT))
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
        self.post("end", &End { reason })
    }

    fn post(&self, call: &str, body: &impl Serialize) -> Result<Answer, ureq::Error> {
        let body = serde_json::to_string(body).expect("report bodies are plain JSON objects");
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
    pub(super) fn start(config: &HostedConfig) -> Self {
        info!(
            "hosting: server {} reporting to {} — capacity {}, heartbeat every {:?}, exit after \
             {:?} empty",
            config.server_id,
            config.api,
            config.capacity,
            config.heartbeat_interval,
            config.empty_exit
        );
        let api = Api::new(config);
        let shared = Arc::new(Shared::default());
        let (control, receiver) = mpsc::channel();
        let reporter = std::thread::Builder::new()
            .name("hosting-reporter".into())
            .spawn({
                let (api, shared) = (api.clone(), shared.clone());
                let (capacity, interval) = (config.capacity, config.heartbeat_interval);
                move || report(api, shared, capacity, interval, receiver)
            })
            .expect("the hosting reporter thread must spawn");
        Self {
            api,
            shared,
            control,
            reporter,
            empty_exit: config.empty_exit,
        }
    }

    pub(super) fn install(&self, app: &mut App) {
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
        let Some(reason) = reason else {
            info!("hosting: exiting without an end call (the Worker no longer knows this server)");
            return;
        };
        match self.api.end(&reason) {
            Ok(Answer::Accepted) => info!("hosting: reported end ({reason})"),
            Ok(answer) => warn!("hosting: end ({reason}) answered {answer:?}"),
            Err(err) => warn!("hosting: end ({reason}) failed: {err}"),
        }
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
    time: Res<Time<Real>>,
    mut link: ResMut<HostingLink>,
    mut exit: MessageWriter<AppExit>,
) {
    let players = clients.iter().count() as u32;
    link.shared.players.store(players, Ordering::Relaxed);
    let limit = link.empty.limit;
    if link.empty.expired(players, time.elapsed()) {
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
        for var in [CAPACITY, HEARTBEAT_INTERVAL, EMPTY_EXIT] {
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

    #[test]
    fn a_malformed_setting_names_its_variable_and_never_its_value() {
        for var in [API, NETCODE_KEY, REPORT_SECRET] {
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
    #[derive(Debug)]
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
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback TCP");
            let base = format!("http://{}", listener.local_addr().unwrap());
            let (sender, seen) = mpsc::channel();
            std::thread::spawn(move || {
                for (index, stream) in listener.incoming().enumerate() {
                    let Ok(mut stream) = stream else { return };
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
                        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
                    }
                    let length = headers
                        .get("content-length")
                        .map_or(0, |n| n.parse::<usize>().unwrap());
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    let status = statuses[index.min(statuses.len() - 1)];
                    write!(
                        stream,
                        "HTTP/1.1 {status} X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    )
                    .unwrap();
                    let body = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
                    if sender
                        .send(Seen {
                            method,
                            path,
                            headers,
                            body,
                        })
                        .is_err()
                    {
                        return;
                    }
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

    #[test]
    fn heartbeat_and_end_requests_match_the_contract() {
        let worker = FakeWorker::start(vec![204]);
        let config = hosted_against(&worker, Duration::from_secs(5));
        let api = Api::new(&config);
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
        shared.decide_exit(ExitReason::Empty(Duration::from_secs(1)));
        let body = serde_json::to_value(shared.heartbeat(10)).unwrap();
        assert_eq!(body["state"], "closing");
        assert_eq!(body["players"], 3);
    }

    #[test]
    fn the_reporter_waits_for_listening_then_survives_failures() {
        let worker = FakeWorker::start(vec![503, 500, 204, 204]);
        let config = hosted_against(&worker, Duration::from_millis(20));
        let hosted = Hosted::start(&config);
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
        let hosted = Hosted::start(&config);
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
        app.add_message::<AppExit>();
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
        let hosted = Hosted::start(&config);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<AppExit>();
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

    fn capped_server(port: u16, config: &HostedConfig) -> App {
        let mut app = base_app();
        app.add_plugins(ServerPlugins {
            tick_duration: TICK,
        });
        crate::net::protocol::plugin(&mut app);
        let server = app
            .world_mut()
            .spawn((
                crate::net::server::netcode_server(Some(config)),
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
}
