//! The hosted-match flow end to end, against a LIVE matchmaking Worker: two headless clients join
//! through the real pipeline and must land on the same server.
//!
//! ```text
//! OVERMATCH_API=https://overmatch-backend.vikng.workers.dev \
//!     cargo test --locked --lib hosted_e2e -- --ignored --nocapture
//! ```
//!
//! Each client is [`run`](super::run)'s matchmaking composition without the window or the
//! presentation layer: the Join menu's state, Worker link and match-link systems
//! (`install_matchmaking_link`), the connection driver, the screen derivation and the sim. The join is the menu's own (`matchmaking::try_join`: the selection set, then the
//! Join button's handler), selecting only [`LOCATION`]. Client B joins [`B_JOIN_DELAY`] after A, so
//! the Worker places B on the server A's join claimed, whether it is still starting or ready. Once
//! both are in the match and each sees both tanks, both disconnect cleanly, which starts the
//! server's empty timer.
//!
//! A run may start a billed Edgegap deployment, which then lives until its empty exit.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use super::*;
use crate::flow::Flow;
use crate::net::matchmaking::{self, MatchClosed, MatchReady, ReadyMatch};

/// The only location selected: a Worker location id from `GET /v1/locations`.
const LOCATION: &str = "Singapore";

/// From Join to a ready ticket: an Edgegap deployment start plus its first heartbeat.
const READY_DEADLINE: Duration = Duration::from_secs(240);
/// From a ready ticket to the match: the asset-gated first connect and the own tank's arrival.
const MATCH_DEADLINE: Duration = Duration::from_secs(90);
/// How long after A's join B joins.
const B_JOIN_DELAY: Duration = Duration::from_secs(5);
/// Frames run after `Disconnect` so its packets leave before the app drops.
const DISCONNECT_FRAMES: usize = 20;
/// The frame pacing of the headless runner (`run`'s `ScheduleRunnerPlugin::run_loop`).
const FRAME: Duration = Duration::from_millis(2);

/// What a client saw, in seconds since its Join.
#[derive(Debug)]
struct Outcome {
    label: &'static str,
    matched: ReadyMatch,
    ready_s: f64,
    connected_s: f64,
    in_match_s: f64,
    /// Both clients in the match and this one seeing both tanks.
    both_s: f64,
}

#[derive(Resource, Default)]
struct Probe {
    joined_at: Option<Instant>,
    ready: Option<(ReadyMatch, Instant)>,
    connected_at: Option<Instant>,
    unreachable: Option<String>,
}

fn record_ready(mut ready: MessageReader<MatchReady>, mut probe: ResMut<Probe>) {
    if let Some(MatchReady(matched)) = ready.read().last() {
        probe.ready.get_or_insert((*matched, Instant::now()));
    }
}

fn record_unreachable(mut closed: MessageReader<MatchClosed>, mut probe: ResMut<Probe>) {
    for closed in closed.read() {
        if let MatchClosed::Unreachable(reason) = closed {
            probe.unreachable = Some(reason.clone());
        }
    }
}

fn record_connected(_: On<Add, Connected>, mut probe: ResMut<Probe>) {
    probe.connected_at.get_or_insert_with(Instant::now);
}

/// `run`'s matchmaking-mode composition on a GPU-less, window-less floor. Only the first app in
/// the process installs the global log subscriber.
fn client_app(api: &str, log: bool) -> App {
    let mut app = App::new();
    let plugins = crate::gpu_less_default_plugins(None).set(AssetPlugin {
        file_path: asset_root(),
        ..default()
    });
    if log {
        app.add_plugins(plugins);
    } else {
        app.add_plugins(plugins.disable::<bevy::log::LogPlugin>());
    }
    // The GPU-less tank-glb workaround `run`'s headless path carries (see `headless_test`).
    app.insert_resource(bevy::image::CompressedImageFormatSupport(
        bevy::image::CompressedImageFormats::ASTC_LDR,
    ));

    let tick_duration = Duration::from_secs_f64(1.0 / 64.0);
    app.add_plugins(ClientPlugins { tick_duration });
    app.add_plugins(crate::net::plugin);
    app.add_plugins(physics::physics_plugins());
    app.add_plugins(crate::net::fire_presentation::plugin);
    app.add_plugins(SimPlugin);
    app.insert_resource(crate::ClientReplica);
    // The Join menu's state lives beside its UI tree, which needs the fonts and the overlay set.
    app.add_plugins((
        crate::ui_font::plugin,
        crate::flow::plugin,
        crate::overlay::plugin,
    ));

    let input_delay = shipping_input_delay();
    let sync_config = SyncConfig {
        jitter_multiple: 2,
        jitter_margin: 1.0,
        ..default()
    };
    let interpolation = crate::net::interp_delay::install(&mut app, tick_duration);
    let interpolation = crate::net::sync_margin::install(
        &mut app,
        tick_duration,
        sync_config,
        input_delay,
        interpolation,
    );
    crate::net::extrapolate::install(&mut app);
    app.insert_resource(InputTimelineConfig::new(sync_config, input_delay));
    app.insert_resource(interpolation);

    app.init_resource::<ConnectRetry>()
        .add_systems(Update, drive_connection);
    install_matchmaking_link(&mut app, api);
    app.add_systems(
        Update,
        derive_flow
            .after(drive_connection)
            .before(crate::overlay::OverlaySet::Declare),
    );
    app.insert_resource(ClientLinkTemplate {
        client_id: std::hash::RandomState::new().build_hasher().finish(),
        conditioner: None,
        seeded: None,
    });
    app.add_systems(Startup, load_tank_assets);
    // `run`'s receive wiring, verbatim; no input writer, so the own tank's command stays idle.
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
                log_local_tank_role,
            ),
        );
    app.init_resource::<PendingRecoilKicks>()
        .init_resource::<SeenShots>()
        .init_resource::<HeldFireEvents>()
        .init_resource::<PendingFireEvents>()
        .init_resource::<SeenDamage>()
        .init_resource::<SanctionedShots>()
        .add_systems(FixedUpdate, age_sanctioned_shots)
        .add_systems(
            FixedUpdate,
            apply_pending_recoil_kicks
                .before(GameplaySet)
                .run_if(in_state(AppState::Playing)),
        );
    install_input_buffer_guard(&mut app);
    install_input_send_cadence(&mut app);

    app.init_resource::<Probe>()
        .add_observer(record_connected)
        .add_systems(Update, (record_ready, record_unreachable));
    // `App::run` finishes the plugins; a hand-driven `update` loop must (lightyear and avian
    // register resources their systems require in `Plugin::finish`).
    crate::net::test_harness::finish(&mut app);
    app
}

/// In a match: the screen is the match's and the server's tank for this client has arrived.
fn in_match(app: &mut App) -> bool {
    let world = app.world_mut();
    *world.resource::<State<Flow>>().get() == Flow::InMatch
        && world
            .query_filtered::<(), With<NetControlled>>()
            .iter(world)
            .next()
            .is_some()
}

fn tanks(app: &mut App) -> usize {
    let world = app.world_mut();
    world
        .query_filtered::<(), With<NetTank>>()
        .iter(world)
        .count()
}

/// What the two client threads share: how many are in the match, and whether either gave up.
#[derive(Default)]
struct Shared {
    in_match: AtomicUsize,
    failed: AtomicBool,
}

/// One client from launch to a clean disconnect. Joins `delay` after `go` fires, and then fires
/// `joined`; disconnects once both clients are in the match and it sees both tanks.
fn drive(
    label: &'static str,
    api: String,
    log: bool,
    go: mpsc::Receiver<()>,
    delay: Duration,
    joined: Option<mpsc::Sender<()>>,
    shared: Arc<Shared>,
) -> Result<Outcome, String> {
    let result = session(label, &api, log, &go, delay, joined.as_ref(), &shared);
    if result.is_err() {
        shared.failed.store(true, Ordering::SeqCst);
    }
    result
}

fn session(
    label: &'static str,
    api: &str,
    log: bool,
    go: &mpsc::Receiver<()>,
    delay: Duration,
    joined: Option<&mpsc::Sender<()>>,
    shared: &Shared,
) -> Result<Outcome, String> {
    let mut app = client_app(api, log);
    let mut armed_at = None;
    let mut in_match_at = None;
    let fail = |stage: &str, reason: String| Err(format!("{label}: {stage}: {reason}"));
    let boot = Instant::now();
    loop {
        app.update();
        thread::sleep(FRAME);
        if shared.failed.load(Ordering::SeqCst) {
            return fail("abandoned", "the other client failed".into());
        }
        let joined_at = app.world().resource::<Probe>().joined_at;
        let Some(joined_at) = joined_at else {
            if boot.elapsed() > Duration::from_secs(90) {
                return fail("lobby", "no join within 90 s of boot".into());
            }
            if armed_at.is_none() && go.try_recv().is_ok() {
                armed_at = Some(Instant::now() + delay);
            }
            if armed_at.is_some_and(|at| Instant::now() >= at)
                && matchmaking::try_join(app.world_mut(), &[LOCATION])
            {
                app.world_mut().resource_mut::<Probe>().joined_at = Some(Instant::now());
                println!("[{label}] join sent, locations [{LOCATION}]");
                if let Some(joined) = joined {
                    let _ = joined.send(());
                }
            }
            continue;
        };
        if let Some(reason) = matchmaking::join_failure(app.world()) {
            return fail("join", reason);
        }
        if let Some(reason) = app.world().resource::<Probe>().unreachable.clone() {
            return fail("connect", reason);
        }
        let probe = app.world().resource::<Probe>();
        let (connected_at, ready) = (probe.connected_at, probe.ready);
        let seconds = |at: Instant| at.duration_since(joined_at).as_secs_f64();
        let Some((matched, ready_at)) = ready else {
            if joined_at.elapsed() > READY_DEADLINE {
                return fail("ticket", format!("not ready after {READY_DEADLINE:?}"));
            }
            continue;
        };
        if ready_at.elapsed() > MATCH_DEADLINE {
            let stage = if in_match_at.is_some() {
                "both"
            } else {
                "match"
            };
            return fail(
                stage,
                format!(
                    "not done {MATCH_DEADLINE:?} after the ticket (in match: {}, tanks seen: {})",
                    shared.in_match.load(Ordering::SeqCst),
                    tanks(&mut app)
                ),
            );
        }
        if in_match_at.is_none() {
            if !in_match(&mut app) {
                continue;
            }
            in_match_at = Some(Instant::now());
            shared.in_match.fetch_add(1, Ordering::SeqCst);
            println!("[{label}] in the match");
        }
        if shared.in_match.load(Ordering::SeqCst) < 2 || tanks(&mut app) < 2 {
            continue;
        }
        let outcome = Outcome {
            label,
            matched,
            ready_s: seconds(ready_at),
            connected_s: seconds(connected_at.expect("in a match implies connected")),
            in_match_s: seconds(in_match_at.expect("set above")),
            both_s: joined_at.elapsed().as_secs_f64(),
        };
        // The other client's own wait reads the count, so hold the link a moment past it.
        for _ in 0..DISCONNECT_FRAMES {
            app.update();
            thread::sleep(FRAME);
        }
        let link = app
            .world_mut()
            .query_filtered::<Entity, With<NetcodeClient>>()
            .single(app.world())
            .expect("a client in a match has its one link");
        app.world_mut().trigger(Disconnect { entity: link });
        for _ in 0..DISCONNECT_FRAMES {
            app.update();
            thread::sleep(FRAME);
        }
        println!("[{label}] disconnected");
        return Ok(outcome);
    }
}

#[test]
#[ignore = "live: needs OVERMATCH_API and may start a billed server"]
fn two_clients_join_the_same_hosted_match() {
    let api = std::env::var("OVERMATCH_API")
        .ok()
        .filter(|api| !api.is_empty())
        .expect("OVERMATCH_API names the live Worker");
    let shared = Arc::new(Shared::default());
    let (go_a, a_go) = mpsc::channel();
    let (a_joined, b_go) = mpsc::channel();
    let a = {
        let (api, shared) = (api.clone(), shared.clone());
        thread::spawn(move || drive("A", api, true, a_go, Duration::ZERO, Some(a_joined), shared))
    };
    let b = thread::spawn(move || drive("B", api, false, b_go, B_JOIN_DELAY, None, shared));
    go_a.send(()).expect("A is waiting to join");
    let a = a.join().expect("client A ran");
    let b = b.join().expect("client B ran");
    let (a, b) = (a.unwrap(), b.unwrap());
    for outcome in [&a, &b] {
        println!(
            "[{}] server {} | join->ready {:.1} s | join->connected {:.1} s (ready->connected \
             {:.1} s) | join->in match {:.1} s | join->both in, both tanks seen {:.1} s",
            outcome.label,
            outcome.matched.addr,
            outcome.ready_s,
            outcome.connected_s,
            outcome.connected_s - outcome.ready_s,
            outcome.in_match_s,
            outcome.both_s,
        );
    }
    assert_eq!(
        a.matched.addr, b.matched.addr,
        "both clients must be sent to the same server"
    );
    assert!(
        a.matched.key == b.matched.key,
        "both clients must hold the same match's key"
    );
}
