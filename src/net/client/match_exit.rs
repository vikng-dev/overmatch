//! The matchmaking exits from a match through the installed lifecycle, over real UDP: the client
//! is [`run`](super::run)'s matchmaking link composition (`install_matchmaking_link`, the
//! connection driver, the screen derivation) without the sim or the presentation layer, against a
//! real server.
//!
//! - **LEAVE MATCH.** A [`LeaveMatch`] disconnects the link — the server sees the client go well
//!   inside netcode's timeout, so the disconnect was delivered — the link is removed, and the
//!   client is on the lobby screen with [`MatchClosed::Left`].
//! - **Server gone.** The server's app is dropped; the client keeps reconnecting until
//!   [`RECONNECT_GIVE_UP_S`] of real time has passed, then closes the match as ended and is on the
//!   lobby screen. Each client frame advances real time by [`STALL`] — virtual time clamps a frame
//!   that long — so the give-up lands within [`GONE_FRAMES`] frames only on the real clock.
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
/// Frames after LEAVE MATCH by which the link is gone and the lobby is up.
const LEAVE_FRAMES: u32 = 8;
/// Server frames after LEAVE MATCH by which the server has dropped the client: 20 × 15.6 ms of
/// server time, against netcode's 3 s timeout (DERIVED), so only a delivered disconnect does it.
const SERVER_DROP_STEPS: u32 = 20;
/// A stalled client frame.
const STALL: Duration = Duration::from_secs(1);
/// Frames of [`STALL`] within which the give-up must land: [`RECONNECT_GIVE_UP_S`] of real time
/// plus slack. On virtual time (its 250 ms clamp per frame) the same give-up takes 120 frames.
const GONE_FRAMES: u32 = 45;

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

#[test]
fn leave_match_disconnects_drops_the_link_and_returns_to_the_lobby() {
    let _udp = lock_real_udp_test();
    let (mut server, mut client) = in_match();
    client.world_mut().write_message(LeaveMatch);
    for _ in 0..LEAVE_FRAMES {
        step(Some(&mut server), &mut client);
    }
    assert!(links(&mut client).is_empty(), "the left link is removed");
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
    let (server, mut client) = in_match();
    drop(server);
    client.insert_resource(TimeUpdateStrategy::ManualDuration(STALL));
    let mut frames = 0;
    while flow(&client) != Flow::Lobby && frames < GONE_FRAMES {
        step(None, &mut client);
        frames += 1;
    }
    assert_eq!(
        flow(&client),
        Flow::Lobby,
        "still in the match after {GONE_FRAMES} frames of {STALL:?}"
    );
    assert!(links(&mut client).is_empty(), "the lost link is removed");
    assert_eq!(
        client.world().resource::<Closed>().0,
        vec![MatchClosed::Ended(
            "the match server stopped answering".into()
        )]
    );
    assert!(
        frames as f64 * STALL.as_secs_f64() >= RECONNECT_GIVE_UP_S,
        "not before the give-up ({frames} frames)"
    );
}
