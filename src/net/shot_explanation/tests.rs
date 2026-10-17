//! Real-UDP routing: one server and three clients — the shooter, the struck tank's player and a
//! bystander.

use core::time::Duration;
use std::net::{Ipv4Addr, SocketAddr};

use lightyear::connection::client_of::ClientOf;
use lightyear::prelude::client::{
    Client, ClientPlugins, Connect, Connected, NetcodeClient, NetcodeConfig,
};
use lightyear::prelude::server::{
    NetcodeConfig as ServerNetcodeConfig, NetcodeServer, ServerPlugins, ServerUdpIo, Start,
};

use super::*;
use crate::CombatantLife;
use crate::damage_cam::delivery::tests::{SHOOTER, STRUCK, recorded, report, vocabulary};
use crate::damage_cam::wire::{Audience, TankLife};
use crate::net::client::install_input_buffer_guard;
use crate::net::protocol::protocol_id;
use crate::net::server::attach_replication_sender;
use crate::net::test_harness::{TICK, base_app, finish, free_port, lock_real_udp_test};

const BYSTANDER: CombatantId = CombatantId(3);

/// One tank per connected client, its combatant the client's netcode id.
fn spawn_player_tanks(
    clients: Query<(Entity, &RemoteId), (With<ClientOf>, With<Connected>)>,
    tanks: Query<&ControlledBy>,
    mut commands: Commands,
) {
    for (link, remote) in &clients {
        let PeerId::Netcode(id) = remote.0 else {
            continue;
        };
        if tanks.iter().any(|controlled| controlled.owner == link) {
            continue;
        }
        commands.spawn((
            NetTank,
            CombatantId(id),
            ControlledBy {
                owner: link,
                lifetime: default(),
            },
        ));
    }
}

fn build_server(port: u16, vocabulary: ShotVocabulary) -> App {
    let mut app = base_app();
    app.add_plugins(ServerPlugins {
        tick_duration: TICK,
    });
    super::super::protocol::plugin(&mut app);
    install_server(&mut app);
    app.add_observer(attach_replication_sender);
    app.insert_resource(vocabulary);
    app.add_systems(Update, spawn_player_tanks);
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

fn build_client(port: u16, combatant: CombatantId, vocabulary: ShotVocabulary) -> App {
    let mut app = base_app();
    app.add_plugins(ClientPlugins {
        tick_duration: TICK,
    });
    install_input_buffer_guard(&mut app);
    super::super::protocol::plugin(&mut app);
    install_client(&mut app);
    app.insert_resource(vocabulary);
    let server_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);
    let client = app
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
                    client_id: combatant.0,
                    private_key: [0; 32],
                    protocol_id: protocol_id(),
                },
                NetcodeConfig::default(),
            )
            .expect("manual dev token should always build"),
            UdpIo::default(),
        ))
        .id();
    app.world_mut()
        .commands()
        .trigger(Connect { entity: client });
    app
}

fn step(server: &mut App, clients: &mut [App]) {
    server.update();
    std::thread::sleep(Duration::from_micros(500));
    for client in clients {
        client.update();
    }
    std::thread::sleep(Duration::from_micros(500));
}

fn connected(client: &mut App) -> bool {
    client
        .world_mut()
        .query_filtered::<(), (With<Client>, With<Connected>)>()
        .iter(client.world())
        .next()
        .is_some()
}

fn stored(client: &App) -> Vec<ShotExplanation> {
    client
        .world()
        .resource::<ShotExplanations>()
        .iter()
        .map(|(_, stored)| stored.explanation.clone())
        .collect()
}

/// The shooter receives exactly the bytes of its projection, the struck tank's player exactly
/// those of its own, and the bystander nothing. The struck player's explanation is larger than one
/// transport fragment, so it crosses the link fragmented.
#[test]
fn each_explanation_reaches_only_its_recipient() {
    let _udp = lock_real_udp_test();
    let mut report = report("s1_through_crew");
    // Fixture: the record's cone repeated, so the struck player's explanation spans more than one
    // transport fragment; a one-cone record at the shipped direction count fits in one (MEASURED
    // in `damage_cam::wire`).
    let cone = report.spall[0].clone();
    report.spall.extend(std::iter::repeat_n(cone, 3));
    let vocabulary = vocabulary(&report);
    let struck = TankLife {
        combatant: STRUCK,
        life: CombatantLife(0),
    };
    let built =
        BuiltExplanation::build(&vocabulary, &recorded(&report, SHOOTER, 1200, struck)).unwrap();
    let projections = built.projections();
    let bytes = |explanation: &ShotExplanation| postcard::to_allocvec(explanation).unwrap();
    let (victim, shooter) = (&projections[0], &projections[1]);
    assert_eq!((victim.0, shooter.0), (STRUCK, SHOOTER));
    assert_eq!(shooter.1.audience, Audience::Shooter);
    assert_ne!(bytes(&victim.1), bytes(&shooter.1));
    let fragment =
        lightyear_transport::packet::fragment_size_for_min_mtu(lightyear::link::DEFAULT_MTU)
            .unwrap();
    println!(
        "MEASURED explanation sizes: victim {} B, shooter {} B; fragment size {fragment} B",
        bytes(&victim.1).len(),
        bytes(&shooter.1).len(),
    );
    assert!(bytes(&victim.1).len() > fragment);

    let port = free_port();
    let mut server = build_server(port, vocabulary.clone());
    let mut clients: Vec<App> = [SHOOTER, STRUCK, BYSTANDER]
        .into_iter()
        .map(|combatant| build_client(port, combatant, vocabulary.clone()))
        .collect();
    finish(&mut server);
    for client in &mut clients {
        finish(client);
    }
    let mut ready = false;
    for _ in 0..1_200 {
        step(&mut server, &mut clients);
        let tanks = server
            .world_mut()
            .query_filtered::<(), With<NetTank>>()
            .iter(server.world())
            .count();
        if clients.iter_mut().all(connected) && tanks == 3 {
            ready = true;
            break;
        }
    }
    assert!(ready, "the three clients never connected");

    server
        .world_mut()
        .write_message(recorded(&report, SHOOTER, 1200, struck));
    let mut arrived = false;
    for _ in 0..600 {
        step(&mut server, &mut clients);
        if stored(&clients[0]).len() == 1 && stored(&clients[1]).len() == 1 {
            arrived = true;
            break;
        }
    }
    assert!(
        arrived,
        "the shooter's and the victim's explanations never arrived"
    );
    for _ in 0..64 {
        step(&mut server, &mut clients);
    }

    let shooter_stored = stored(&clients[0]);
    assert_eq!(shooter_stored.len(), 1);
    assert_eq!(bytes(&shooter_stored[0]), bytes(&shooter.1));
    let victim_stored = stored(&clients[1]);
    assert_eq!(victim_stored.len(), 1);
    assert_eq!(bytes(&victim_stored[0]), bytes(&victim.1));
    assert!(stored(&clients[2]).is_empty(), "the bystander received one");
    for client in &clients {
        assert_eq!(client.world().resource::<ShotExplanations>().refused(), 0);
    }
}

/// The delivery trace logs each explanation once on arrival with its presence then, and once more
/// when it leaves pending, drawn or timed out; one stored drawn is never logged again.
#[test]
fn the_trace_logs_each_arrival_and_release_once() {
    let report = report("s1_through_crew");
    let vocabulary = vocabulary(&report);
    let struck = TankLife {
        combatant: STRUCK,
        life: CombatantLife(0),
    };
    let victim = |fire_tick| {
        let built =
            BuiltExplanation::build(&vocabulary, &recorded(&report, SHOOTER, fire_tick, struck))
                .unwrap();
        built.projections().swap_remove(0).1
    };
    let (waiting, drawn, already) = (victim(1200), victim(1400), victim(1600));
    let path = std::env::temp_dir().join(format!(
        "overmatch-pencam-trace-{}-{}.jsonl",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    {
        let mut app = App::new();
        app.init_resource::<Time<Virtual>>()
            .init_resource::<LocalTimeline>()
            .init_resource::<ShotExplanations>()
            .insert_resource(ShotTrace::for_test(&path))
            .add_systems(Update, trace_presence);
        let mut store = app.world_mut().resource_mut::<ShotExplanations>();
        store.drew(already.shot, already.outcome.unwrap());
        store.receive(waiting.clone(), &vocabulary, 0.0);
        store.receive(drawn.clone(), &vocabulary, 0.0);
        store.receive(already.clone(), &vocabulary, 0.0);
        app.update();
        let mut store = app.world_mut().resource_mut::<ShotExplanations>();
        store.drew(drawn.shot, drawn.outcome.unwrap());
        app.update();
        app.world_mut()
            .resource_mut::<ShotExplanations>()
            .expire(crate::damage_cam::delivery::IMPACT_TIMEOUT_S);
        app.update();
        app.update();
    }
    let rows: Vec<(String, u32, String)> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|row| row["k"] != "meta")
        .map(|row| {
            assert!(
                row["us"].as_u64().unwrap() > 0,
                "every row carries the wall clock"
            );
            (
                row["k"].as_str().unwrap().to_owned(),
                u32::try_from(row["ft"].as_u64().unwrap()).unwrap(),
                row["pres"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    std::fs::remove_file(&path).unwrap();
    let row = |k: &str, ft: u32, pres: &str| (k.to_owned(), ft, pres.to_owned());
    assert_eq!(
        rows,
        [
            row("xrx", 1200, "pending"),
            row("xrx", 1400, "pending"),
            row("xrx", 1600, "presented"),
            row("xpres", 1400, "presented"),
            row("xpres", 1200, "timed_out"),
        ]
    );
}
