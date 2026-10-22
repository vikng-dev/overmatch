//! The matchmaking exits from a match through the installed lifecycle, over real UDP: the client
//! is [`run`](super::run)'s matchmaking link composition (`install_matchmaking_link`, the
//! connection driver, the screen derivation) without the sim or the presentation layer, against a
//! real server.
//!
//! - **LEAVE MATCH.** A [`LeaveMatch`] disconnects the link — the server sees the client go well
//!   inside netcode's timeout, so the disconnect was delivered — the link is removed, and the
//!   client is on the lobby screen with [`MatchClosed::Left`].
//! - **Server gone.** The server's app is dropped; the client keeps reconnecting until a retry
//!   fired [`RECONNECT_GIVE_UP_S`] of real time after the drop fails, then closes the match as
//!   ended and is on the lobby screen. Each client frame advances real time by [`SLOW_FRAME`] —
//!   virtual time clamps a frame that long — so the give-up lands within [`GONE_FRAMES`] frames
//!   only on the real clock.
//!
//!   Either way the match's replicas leave with its link: lightyear 0.30.1 despawns every
//!   replicated entity on the client when the link's `Connected` is removed, unless the receiver
//!   is `Persistent` (`lightyear_replication` client.rs, `on_replication_disconnect`), so a client
//!   returning to the lobby has no replica to clean up. Both exits replicate one tank root first
//!   and fail if it outlives the link.
//! - **A stall over a live server.** The link drops (the server stops answering), then one client
//!   frame spans [`LONG_STALL`], past the give-up, while the server comes back. The client must
//!   try again on resume and stay in the match: the give-up counts failed attempts, not a stall.
//!
//! The asset gate on the first connect is stood in for: the test marks the attempt initiated and
//! triggers `Connect` itself, as `drive_connection` does once the tank assets load.

use bevy::time::TimeUpdateStrategy;
use lightyear::connection::client_of::ClientOf;

use super::*;
use crate::flow::Flow;
use crate::net::matchmaking::{LeaveMatch, MatchClosed, MatchReady, ReadyMatch};
use crate::net::test_harness::{TICK, base_app, finish, free_port, lock_real_udp_test};

const CONNECT_STEPS: u32 = 2_000;
const REPLICATE_STEPS: u32 = 500;
/// Frames after LEAVE MATCH by which the link is gone and the lobby is up.
const LEAVE_FRAMES: u32 = 8;
/// Server frames after LEAVE MATCH by which the server has dropped the client: 20 × 15.6 ms of
/// server time, against netcode's 3 s timeout (DERIVED), so only a delivered disconnect does it.
const SERVER_DROP_STEPS: u32 = 20;
/// A slow client frame.
const SLOW_FRAME: Duration = Duration::from_secs(1);
/// Frames of [`SLOW_FRAME`] within which the give-up must land: [`RECONNECT_GIVE_UP_S`] of real
/// time plus the 5 s backoff cap and slack. On virtual time (its 250 ms clamp per frame) the same
/// give-up takes 120 frames.
const GONE_FRAMES: u32 = 45;
/// One frame of a suspended or hitched client, twice the give-up.
const LONG_STALL: Duration = Duration::from_secs(60);
/// Slow frames of a server outage: past netcode's 3 s client timeout, so the link is down.
const OUTAGE_FRAMES: u32 = 8;

#[derive(Resource, Default)]
struct Closed(Vec<MatchClosed>);

fn record_closed(mut closed: MessageReader<MatchClosed>, mut log: ResMut<Closed>) {
    log.0.extend(closed.read().cloned());
}

fn client_app() -> App {
    let mut app = base_app();
    app.add_plugins(ClientPlugins {
        tick_duration: TICK,
    });
    crate::net::protocol::plugin(&mut app);
    // The lobby screen's card draws with the bundled font.
    app.init_asset::<Font>()
        .add_plugins((crate::ui_font::plugin, crate::flow::plugin))
        .insert_state(AppState::Playing);
    app.init_resource::<ConnectRetry>()
        .add_systems(Update, drive_connection);
    // A closed local port: the lobby's location fetch fails at once, which no exit depends on.
    install_matchmaking_link(&mut app, "http://127.0.0.1:9");
    app.add_systems(
        Update,
        derive_flow
            .after(drive_connection)
            .before(crate::overlay::OverlaySet::Declare),
    );
    app.insert_resource(ClientLinkTemplate {
        client_id: 7,
        conditioner: None,
        seeded: None,
    });
    app.init_resource::<Closed>()
        .add_systems(Update, record_closed);
    app
}

fn step(server: Option<&mut App>, client: &mut App) {
    if let Some(server) = server {
        server.update();
    }
    std::thread::sleep(Duration::from_micros(300));
    client.update();
    std::thread::sleep(Duration::from_micros(300));
}

fn flow(client: &App) -> Flow {
    *client.world().resource::<State<Flow>>().get()
}

fn links(client: &mut App) -> Vec<Entity> {
    client
        .world_mut()
        .query_filtered::<Entity, With<NetcodeClient>>()
        .iter(client.world())
        .collect()
}

fn server_clients(server: &mut App) -> usize {
    server
        .world_mut()
        .query_filtered::<(), (With<ClientOf>, With<Connected>)>()
        .iter(server.world())
        .count()
}

/// A ready match → the link → connected, in the match on both ends.
fn in_match() -> (App, App) {
    let port = free_port();
    let mut server = crate::net::link_drop::build_server(port);
    let mut client = client_app();
    finish(&mut server);
    finish(&mut client);
    // The screen lands the frame after its facts (`flow`'s module doc).
    for _ in 0..2 {
        step(Some(&mut server), &mut client);
    }
    assert_eq!(flow(&client), Flow::Lobby, "no link: the lobby");

    client.world_mut().write_message(MatchReady(ReadyMatch {
        addr: SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port),
        key: [0; 32],
    }));
    step(Some(&mut server), &mut client);
    let link = *links(&mut client)
        .first()
        .expect("a ready match spawns the link");
    client.world_mut().resource_mut::<ConnectRetry>().initiated = true;
    client
        .world_mut()
        .commands()
        .trigger(Connect { entity: link });
    for _ in 0..CONNECT_STEPS {
        step(Some(&mut server), &mut client);
        if flow(&client) == Flow::InMatch && server_clients(&mut server) == 1 {
            return (server, client);
        }
    }
    panic!("the client never reached the match within {CONNECT_STEPS} steps");
}

fn replicas(client: &mut App) -> usize {
    client
        .world_mut()
        .query_filtered::<(), (With<NetTank>, With<Remote>)>()
        .iter(client.world())
        .count()
}

/// The server replicates one tank root, and the client holds it.
fn replicate_one_root(server: &mut App, client: &mut App) {
    server
        .world_mut()
        .spawn((NetTank, Replicate::to_clients(NetworkTarget::All)));
    for _ in 0..REPLICATE_STEPS {
        step(Some(&mut *server), client);
        if replicas(client) == 1 {
            return;
        }
    }
    panic!("the replicated root never reached the client within {REPLICATE_STEPS} steps");
}

#[test]
fn leave_match_disconnects_drops_the_link_and_returns_to_the_lobby() {
    let _udp = lock_real_udp_test();
    let (mut server, mut client) = in_match();
    replicate_one_root(&mut server, &mut client);
    client.world_mut().write_message(LeaveMatch);
    for _ in 0..LEAVE_FRAMES {
        step(Some(&mut server), &mut client);
    }
    assert!(links(&mut client).is_empty(), "the left link is removed");
    assert_eq!(replicas(&mut client), 0, "no replica outlives the link");
    assert_eq!(flow(&client), Flow::Lobby);
    assert_eq!(
        client.world().resource::<Closed>().0,
        vec![MatchClosed::Left]
    );
    let mut dropped = false;
    for _ in 0..SERVER_DROP_STEPS {
        if server_clients(&mut server) == 0 {
            dropped = true;
            break;
        }
        step(Some(&mut server), &mut client);
    }
    assert!(
        dropped,
        "the server still holds the client {SERVER_DROP_STEPS} steps after the leave: the \
         disconnect never reached it"
    );
}

#[test]
fn a_gone_server_ends_the_match_after_the_real_time_give_up() {
    let _udp = lock_real_udp_test();
    let (mut server, mut client) = in_match();
    replicate_one_root(&mut server, &mut client);
    drop(server);
    client.insert_resource(TimeUpdateStrategy::ManualDuration(SLOW_FRAME));
    let mut frames = 0;
    while flow(&client) != Flow::Lobby && frames < GONE_FRAMES {
        step(None, &mut client);
        frames += 1;
    }
    assert_eq!(
        flow(&client),
        Flow::Lobby,
        "still in the match after {GONE_FRAMES} frames of {SLOW_FRAME:?}"
    );
    assert!(links(&mut client).is_empty(), "the lost link is removed");
    assert_eq!(replicas(&mut client), 0, "no replica outlives the link");
    assert_eq!(
        client.world().resource::<Closed>().0,
        vec![MatchClosed::Ended(
            "the match server stopped answering".into()
        )]
    );
    assert!(
        frames as f64 * SLOW_FRAME.as_secs_f64() >= RECONNECT_GIVE_UP_S,
        "not before the give-up ({frames} frames)"
    );
}

#[test]
fn a_stall_past_the_give_up_over_a_live_server_reconnects_into_the_match() {
    let _udp = lock_real_udp_test();
    let (mut server, mut client) = in_match();
    let link = links(&mut client)[0];
    client.insert_resource(TimeUpdateStrategy::ManualDuration(SLOW_FRAME));
    for _ in 0..OUTAGE_FRAMES {
        step(None, &mut client);
    }
    assert!(
        !client.world().entity(link).contains::<Connected>(),
        "fixture: the outage dropped the link"
    );
    assert!(
        client
            .world()
            .resource::<ConnectRetry>()
            .down_since
            .is_some(),
        "fixture: the give-up clock runs from before the stall"
    );

    client.insert_resource(TimeUpdateStrategy::ManualDuration(LONG_STALL));
    step(None, &mut client);
    client.insert_resource(TimeUpdateStrategy::ManualDuration(TICK));
    let mut back = false;
    for _ in 0..CONNECT_STEPS {
        step(Some(&mut server), &mut client);
        assert_eq!(
            flow(&client),
            Flow::InMatch,
            "a stall over a live server never leaves the match: {:?}",
            client.world().resource::<Closed>().0
        );
        assert!(
            client.world().get_entity(link).is_ok(),
            "a stall over a live server keeps the match's link: {:?}",
            client.world().resource::<Closed>().0
        );
        if client.world().get::<Connected>(link).is_some() {
            back = true;
            break;
        }
    }
    assert!(back, "the link never reconnected after the stall");
    assert!(client.world().resource::<Closed>().0.is_empty());
}
