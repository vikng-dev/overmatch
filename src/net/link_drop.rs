//! Real-UDP probe of what a client keeps when its link ends. lightyear 0.30.1 despawns every
//! replicated entity on the client when the link's `Connected` is removed, unless the receiver is
//! `Persistent` (`lightyear_replication` client.rs, `on_replication_disconnect`). Both ways a
//! match's link ends are probed: the client's own `Disconnect` (LEAVE MATCH) and a server that
//! goes away (its app dropped mid-match: the link fails, by the closed port's transport error or by
//! netcode's timeout, whichever comes first). Either way the match's world leaves with its link, so a
//! client returning to the lobby has no replica to clean up.

use core::time::Duration;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Instant;

use bevy::prelude::*;
use lightyear::prelude::client::{
    Client, ClientPlugins, Connect, Connected, Disconnect, NetcodeClient, NetcodeConfig,
};
use lightyear::prelude::server::{
    NetcodeConfig as ServerNetcodeConfig, NetcodeServer, ServerPlugins, ServerUdpIo, Start,
};
use lightyear::prelude::*;

use super::protocol::{NetTank, protocol_id};
use super::test_harness::{TICK, base_app, finish, free_port, lock_real_udp_test};

const CONNECT_STEPS: u32 = 2_000;
const REPLICATE_STEPS: u32 = 500;
/// Steps after the client's `Disconnect` within which the replica must be gone.
const LEAVE_STEPS: u32 = 8;
/// Wall-clock bound on the link failing after the server goes: netcode's client timeout (the
/// token's `client_timeout_secs`, 3 s by lightyear's default) plus slack.
const DROP_DEADLINE: Duration = Duration::from_secs(10);

fn build_server(port: u16) -> App {
    let mut app = base_app();
    app.add_plugins(ServerPlugins {
        tick_duration: TICK,
    });
    super::protocol::plugin(&mut app);
    app.add_observer(super::server::attach_replication_sender);
    let server = app
        .world_mut()
        .spawn((
            NetcodeServer::new(ServerNetcodeConfig {
                protocol_id: protocol_id(),
                private_key: [0; 32],
                ..default()
            }),
            LocalAddr(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port)),
            ServerUdpIo::default(),
        ))
        .id();
    app.world_mut().commands().trigger(Start { entity: server });
    app
}

fn build_client(port: u16) -> (App, Entity) {
    let mut app = base_app();
    app.add_plugins(ClientPlugins {
        tick_duration: TICK,
    });
    super::protocol::plugin(&mut app);
    let server_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);
    let link = app
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
                    client_id: 1,
                    private_key: [0; 32],
                    protocol_id: protocol_id(),
                },
                NetcodeConfig::default(),
            )
            .expect("manual dev token should always build"),
            UdpIo::default(),
        ))
        .id();
    app.world_mut().commands().trigger(Connect { entity: link });
    (app, link)
}

fn step(server: Option<&mut App>, client: &mut App) {
    if let Some(server) = server {
        server.update();
    }
    std::thread::sleep(Duration::from_micros(300));
    client.update();
    std::thread::sleep(Duration::from_micros(300));
}

fn connected(client: &mut App, link: Entity) -> bool {
    client.world().entity(link).contains::<Connected>()
}

fn replicas(client: &mut App) -> usize {
    client
        .world_mut()
        .query_filtered::<(), (With<NetTank>, With<Replicated>)>()
        .iter(client.world())
        .count()
}

/// Connect, have the server replicate one tank root, and wait until the client holds it.
fn in_match(port: u16) -> (App, App, Entity) {
    let mut server = build_server(port);
    let (mut client, link) = build_client(port);
    finish(&mut server);
    finish(&mut client);
    let mut up = false;
    for _ in 0..CONNECT_STEPS {
        step(Some(&mut server), &mut client);
        if connected(&mut client, link) {
            up = true;
            break;
        }
    }
    assert!(
        up,
        "the client never connected within {CONNECT_STEPS} steps"
    );
    server
        .world_mut()
        .spawn((NetTank, Replicate::to_clients(NetworkTarget::All)));
    for _ in 0..REPLICATE_STEPS {
        step(Some(&mut server), &mut client);
        if replicas(&mut client) == 1 {
            return (server, client, link);
        }
    }
    panic!("the replicated root never reached the client within {REPLICATE_STEPS} steps");
}

/// LEAVE MATCH's `Disconnect` takes the match's replicas with the link, at once.
#[test]
fn a_left_link_takes_its_replicas() {
    let _udp = lock_real_udp_test();
    let (mut server, mut client, link) = in_match(free_port());
    client
        .world_mut()
        .commands()
        .trigger(Disconnect { entity: link });
    for _ in 0..LEAVE_STEPS {
        step(Some(&mut server), &mut client);
    }
    assert!(!connected(&mut client, link), "the link is down");
    assert_eq!(replicas(&mut client), 0, "no replica outlives the link");
}

/// A server that goes away: the link fails, and the replicas go with it.
#[test]
fn a_dropped_link_takes_its_replicas() {
    let _udp = lock_real_udp_test();
    let (server, mut client, link) = in_match(free_port());
    drop(server);
    let start = Instant::now();
    while connected(&mut client, link) && start.elapsed() < DROP_DEADLINE {
        step(None, &mut client);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        !connected(&mut client, link),
        "netcode never timed the link out within {DROP_DEADLINE:?}"
    );
    step(None, &mut client);
    assert_eq!(replicas(&mut client), 0, "no replica outlives the link");
}
