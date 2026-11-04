//! The game's environment destruction and damage model, game state only: a new Battle must
//! reconstruct the same map identities, colliders and full chunk pools as startup.

use super::*;
use crate::construction::Element;
use crate::construction::breakage::{Breakage, ChunkCollider, Entry, Event, Piece, Record};
use crate::construction::fracture::Fate;
use crate::net::battle::BattlePhase;
use crate::net::construction::Ledger;
use crate::net::debris_stress::DebrisPose;
use crate::net::test_harness::{TICK, base_app, finish, free_port, lock_real_udp_test};
use crate::scatter::MapInstance;
use avian3d::prelude::*;
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use std::net::{Ipv4Addr, SocketAddr};

// Fixture inputs: a flat surface, short mode and loopback peers. Comparisons below concern
// construction and generation, not the fixture assets' particular dimensions or tuning.
fn build_scene(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    let manifest = crate::map::tests::construction_fixture_manifest();
    let grid = crate::terrain_grid::HeightGrid::new(vec![0.0; 9].into(), 3, manifest.extent);
    let obstacles = crate::scatter::spawn(&mut commands, &mut meshes, &manifest, &grid, None);
    commands.insert_resource(crate::ground_field::GroundField::unpainted(
        &crate::ground_types::GroundTypes::load_shipped(),
    ));
    commands.insert_resource(crate::world::TerrainMap {
        revision: 0,
        blocks: Vec::new(),
    });
    commands.insert_resource(crate::world::SpawnObstacles(obstacles));
    commands.insert_resource(grid);
}

fn environment(app: &mut App) {
    app.add_plugins((
        bevy::transform::TransformPlugin,
        crate::contact_transfer::sim_plugin,
        crate::construction::breakage::sim_plugin,
        crate::debris::sim_plugin,
    ))
    .add_systems(Startup, build_scene);
}

// Direct record injection has no network presentation timeline. The UDP fixture mounts the
// complete client installer instead.
fn replica() -> App {
    let mut app = base_app();
    environment(&mut app);
    app.init_resource::<crate::ClientReplica>();
    install_client(&mut app);
    crate::net::construction::install_client(&mut app);
    finish(&mut app);
    app
}

#[derive(Debug, PartialEq)]
struct Census {
    identities: Vec<(String, u16, Transform)>,
    elements: Vec<(String, u16)>,
    pools: Vec<(String, Vec<f32>)>,
    colliders: usize,
    chunks: usize,
    loose: usize,
}

#[derive(Resource, Default)]
struct FixedVisits(u32);

fn census(world: &mut World) -> Census {
    let mut identities: Vec<_> = world
        .query::<(&MapInstance, &Transform)>()
        .iter(world)
        .map(|(id, pose)| (id.id.clone(), id.ordinal, *pose))
        .collect();
    identities.sort_by(|a, b| a.0.cmp(&b.0));
    let mut elements: Vec<_> = world
        .query::<(&ChildOf, &Element)>()
        .iter(world)
        .map(|(parent, element)| {
            (
                world
                    .get::<MapInstance>(parent.parent())
                    .unwrap()
                    .id
                    .clone(),
                element.0,
            )
        })
        .collect();
    elements.sort();
    let mut pools: Vec<_> = world
        .query_filtered::<(&MapInstance, &Breakage), Without<Piece>>()
        .iter(world)
        .map(|(identity, b)| (identity.id.clone(), b.live.state.pool.clone()))
        .collect();
    pools.sort_by(|a, b| a.0.cmp(&b.0));
    Census {
        identities,
        elements,
        pools,
        colliders: world.query::<&Collider>().iter(world).count(),
        chunks: world.query::<&ChunkCollider>().iter(world).count(),
        loose: world
            .query_filtered::<(), With<crate::debris::DebrisPiece>>()
            .iter(world)
            .count(),
    }
}

fn roots(world: &mut World) -> Vec<Entity> {
    world
        .query_filtered::<Entity, With<MapInstance>>()
        .iter(world)
        .collect()
}

/// Fixture causes: isolate one above-grade chunk of every construction by opening all of its
/// incident joints. Transport/reset users rely on all kinds sharing these same addresses.
fn topology(world: &mut World) -> Vec<Entry> {
    let mut entries = Vec::new();
    for (identity, breakage) in world
        .query_filtered::<(&MapInstance, &Breakage), Without<Piece>>()
        .iter(world)
    {
        let structure = &breakage.live.structure;
        let chunk = structure.chunks.iter().position(|c| !c.buried).unwrap();
        entries.extend(
            structure
                .joints
                .iter()
                .enumerate()
                .filter(|(_, j)| {
                    j.a == chunk || j.b == crate::construction::fracture::Side::Chunk(chunk)
                })
                .map(|(j, _)| Entry {
                    construction: identity.ordinal,
                    event: Event::Crack(j as u16),
                    tick: 0,
                }),
        );
    }
    entries
}

fn snapshots(world: &mut World, number: u32, entries: Vec<Entry>) {
    world.spawn((WorldGeneration(number), Ledger(entries)));
}

/// NO RECORD IS CONSUMED BEFORE ITS BATTLE, and old/future records cannot cross the transition.
/// Client collision users rely on reconstruction and record selection happening in the same
/// frame, regardless of which independently replicated entity arrives first.
#[test]
fn record_arrival_order_cannot_mix_worlds_or_consume_a_snapshot_before_construction() {
    for phase_first in [false, true] {
        let mut app = replica();
        app.init_resource::<FixedVisits>()
            .add_systems(FixedUpdate, |mut visits: ResMut<FixedVisits>| visits.0 += 1);
        app.update();
        let fresh = census(app.world_mut());
        let entries = topology(app.world_mut());
        snapshots(app.world_mut(), 1, entries.clone());
        app.update();
        assert_eq!(
            census(app.world_mut()),
            fresh,
            "no Battle has selected these records yet"
        );
        let battle = app.world_mut().spawn(BattleState::combat_fixture()).id();
        app.update();
        assert_ne!(
            census(app.world_mut()),
            fresh,
            "the held initial snapshot must apply after construction"
        );
        let old_roots = roots(app.world_mut());
        let old_carrier = app
            .world_mut()
            .spawn((
                WorldGeneration(1),
                DebrisPose {
                    position: Vec3::Y * 10.0,
                    rotation: Quat::IDENTITY,
                },
                Visibility::Hidden,
            ))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(old_carrier),
            Some(&Visibility::Inherited),
            "a current diagnostic carrier becomes visible before its world retires"
        );
        if !phase_first {
            snapshots(app.world_mut(), 2, Vec::new());
            app.update();
            assert_ne!(
                census(app.world_mut()),
                fresh,
                "a future record cannot reset the current map"
            );
        }
        *app.world_mut().get_mut::<BattleState>(battle).unwrap() = BattleState {
            number: 2,
            phase: BattlePhase::WaitingForPlayer,
        };
        let fixed_visits = app.world().resource::<FixedVisits>().0;
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::ZERO,
        ));
        app.update();
        assert_eq!(
            app.world().resource::<FixedVisits>().0,
            fixed_visits,
            "reconstruction must not wait for a physics tick"
        );
        assert_eq!(
            census(app.world_mut()),
            fresh,
            "the phase alone constructs a fresh world before consumers"
        );
        assert!(
            old_roots
                .iter()
                .all(|root| app.world().get_entity(*root).is_err())
        );
        assert_eq!(
            app.world().get::<Visibility>(old_carrier),
            Some(&Visibility::Hidden)
        );
        assert!(app.world().resource::<Record>().entries.is_empty());
        if phase_first {
            snapshots(app.world_mut(), 2, Vec::new());
            app.update();
        }
        assert_eq!(
            census(app.world_mut()),
            fresh,
            "retired records never replay, including equal-length ledgers"
        );
        // The current Battle's own records apply, not only the first Battle's: its ledgers grow.
        for (belongs, mut ledger) in app
            .world_mut()
            .query::<(&WorldGeneration, &mut Ledger)>()
            .iter_mut(app.world_mut())
        {
            if *belongs == WorldGeneration(2) {
                ledger.0.clone_from(&entries);
            }
        }
        app.update();
        assert_eq!(
            app.world().resource::<Record>().entries,
            entries,
            "the later Battle's consumer must apply its own causes for every construction"
        );
        assert_ne!(
            census(app.world_mut()).elements,
            fresh.elements,
            "the later Battle's construction records must reach collision construction"
        );
        let previous_construction = roots(app.world_mut());
        app.world_mut().despawn(battle);
        type OldWire = Or<(With<Ledger>, With<DebrisPose>)>;
        let old_wire_entities: Vec<_> = app
            .world_mut()
            .query_filtered::<Entity, OldWire>()
            .iter(app.world())
            .collect();
        for entity in old_wire_entities {
            app.world_mut().despawn(entity);
        }
        app.update();
        assert!(app.world().resource::<CurrentWorld>().0.is_none());
        app.world_mut().spawn(BattleState {
            number: 2,
            phase: BattlePhase::WaitingForPlayer,
        });
        snapshots(app.world_mut(), 2, Vec::new());
        app.update();
        assert_eq!(census(app.world_mut()), fresh);
        assert!(
            previous_construction
                .iter()
                .all(|root| app.world().get_entity(*root).is_err()),
            "another authority with the same Battle number still constructs its own world"
        );
    }
}

/// Startup and reset install the same common stream, and a composition without it stays without it.
#[test]
fn reconstruction_constructs_only_the_environment_records_installed_at_startup() {
    let _udp = lock_real_udp_test();
    for installed in [false, true] {
        let mut server = crate::net::link_drop::build_server(free_port());
        environment(&mut server);
        if installed {
            crate::net::construction::install_server(&mut server);
        }
        crate::net::server::install_battle(
            &mut server,
            crate::net::battle_mode::BattleMode::parse(include_str!(
                "../fixtures/battle-short.ron"
            )),
            TICK,
        );
        finish(&mut server);
        server.update();
        let records = |world: &mut World| {
            world
                .query_filtered::<Entity, With<Ledger>>()
                .iter(world)
                .collect::<Vec<_>>()
        };
        let initial = records(server.world_mut());
        assert_eq!(initial.len(), usize::from(installed));
        start_next(server.world_mut(), 2);
        let rebuilt = records(server.world_mut());
        assert_eq!(rebuilt.len(), usize::from(installed));
        assert!(
            initial
                .iter()
                .all(|&entity| server.world().get_entity(entity).is_err()),
            "a ledger identity cannot span generations"
        );
        assert!(
            rebuilt
                .iter()
                .all(|&entity| server.world().get::<WorldGeneration>(entity)
                    == Some(&WorldGeneration(2)))
        );
    }
}

fn network_client(port: u16, id: u64) -> App {
    let mut app = base_app();
    app.add_plugins(ClientPlugins {
        tick_duration: TICK,
    });
    crate::net::protocol::plugin(&mut app);
    environment(&mut app);
    app.init_resource::<crate::ClientReplica>();
    install_client(&mut app);
    crate::net::construction::install_client(&mut app);
    crate::net::debris_stress::install_client(&mut app);
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
                    client_id: id,
                    private_key: [0; 32],
                    protocol_id: crate::net::protocol::protocol_id(),
                },
                lightyear::prelude::client::NetcodeConfig::default(),
            )
            .unwrap(),
            UdpIo::default(),
        ))
        .id();
    app.world_mut().commands().trigger(Connect { entity: link });
    finish(&mut app);
    app
}

fn battle(world: &mut World) -> Option<BattleState> {
    world.query::<&BattleState>().iter(world).next().copied()
}

/// The authority's selected generation and every ledger it publishes name the current Battle.
fn assert_authority_generation(server: &mut App, number: u32) {
    assert_eq!(
        server.world().resource::<AuthorityWorld>().0,
        WorldGeneration(number)
    );
    type Ledgers = With<Ledger>;
    assert!(
        server
            .world_mut()
            .query_filtered::<&WorldGeneration, Ledgers>()
            .iter(server.world())
            .all(|generation| *generation == WorldGeneration(number)),
        "authority publication carries the current Battle's nonempty generation"
    );
}

/// Fixture-only publication of isolated chunks for transport and reset. The common law and
/// production detachment witnesses own physical release; this harness owns link lifetimes.
fn publish_fixture_departures(world: &mut World) {
    let entries = topology(world);
    let mut releases = Vec::new();
    for mut breakage in world
        .query_filtered::<&mut Breakage, Without<Piece>>()
        .iter_mut(world)
    {
        let kind = breakage.kind.clone();
        let mut shape = breakage.shape();
        for entry in entries
            .iter()
            .filter(|entry| entry.construction == breakage.construction)
        {
            shape.apply(entry.event, &kind.layout);
        }
        shape.derive_departures(&kind.layout, &breakage.live);
        let chunks: Vec<_> = shape
            .fate
            .iter()
            .enumerate()
            .filter_map(|(c, fate)| (*fate == Fate::Fallen).then_some(c as u16))
            .collect();
        assert!(
            !chunks.is_empty(),
            "isolating a nonburied fixture chunk removes support"
        );
        releases.push(Piece {
            construction: breakage.construction,
            chunks,
            born: 0,
            retired: None,
        });
        breakage.live.swapped = shape.swapped;
        breakage.live.state.fate = shape.fate;
        breakage.live.state.cracked = shape.cracked;
    }
    world.resource_mut::<Record>().entries.extend(entries);
    for piece in releases {
        world.spawn((
            crate::debris::DebrisPiece,
            piece,
            RigidBody::Dynamic,
            // Fixture body geometry for the transport/reset witness; production detachment
            // geometry is exercised by the common construction plugin's own witnesses.
            Collider::cuboid(1.0, 1.0, 1.0),
            ColliderDensity(1.0),
            Friction::ZERO,
            crate::construction::breakage::piece_layers(),
            Position(Vec3::Y * 20.0),
            Rotation::default(),
            Transform::from_translation(Vec3::Y * 20.0),
        ));
    }
}

/// FRESH BATTLE EQUALS STARTUP BY COUNT AND IDENTITY, on authority, connected client and late
/// joiners. The game's environment destruction model: each construction releases a chunk during
/// combat; the next Battle replaces them and every chunk pool. Keeping a ledger, a loose body, or
/// a damaged root would fail this comparison for the next Battle's players.
#[test]
fn a_destroyed_world_is_startup_again_at_next_battle_over_real_udp() {
    let _udp = lock_real_udp_test();
    let port = free_port();
    let mut server = crate::net::link_drop::build_server(port);
    environment(&mut server);
    crate::net::construction::install_server(&mut server);
    crate::net::debris_stress::install_server(&mut server).unwrap();
    let mut mode =
        crate::net::battle_mode::BattleMode::parse(include_str!("../fixtures/battle-short.ron"));
    // Fixture: allow loopback peers to join throughout this combat window.
    mode.combat = std::time::Duration::from_secs(8);
    mode.late_join = mode.combat;
    crate::net::server::install_battle(&mut server, mode, TICK);
    finish(&mut server);
    let mut clients = vec![network_client(port, 1)];
    let mut fresh = None;
    let mut old_roots = Vec::new();
    let mut destroyed = false;
    let mut saw_destroyed = false;
    let mut combat_joined = false;
    let mut next_joined = false;
    let mut reconnect_link = None;
    let mut reconnect_requested = false;
    let mut reconnected = false;
    let mut previous_client_roots = Vec::new();
    for step in 0..2000 {
        server.update();
        std::thread::sleep(std::time::Duration::from_micros(300));
        for client in &mut clients {
            client.update();
        }
        std::thread::sleep(std::time::Duration::from_micros(300));
        let state = battle(server.world_mut()).unwrap();
        assert_authority_generation(&mut server, state.number);
        if fresh.is_none() {
            fresh = Some(census(server.world_mut()));
        }
        if state.is_combat() && !destroyed {
            old_roots = roots(server.world_mut());
            publish_fixture_departures(server.world_mut());
            destroyed = true;
        }
        if state.number == 1 && state.is_combat() && destroyed && !saw_destroyed {
            saw_destroyed = clients.iter_mut().all(|client| {
                let world = client.world_mut();
                !world.resource::<Record>().entries.is_empty()
                    && world
                        .query_filtered::<&Breakage, Without<Piece>>()
                        .iter(world)
                        .all(|b| b.shape().fate.contains(&Fate::Fallen))
                    && world
                        .query::<(
                            &Piece,
                            &crate::construction::breakage::AdmittedPiece,
                            &DebrisPose,
                        )>()
                        .iter(world)
                        .count()
                        == world
                            .query_filtered::<&Breakage, Without<Piece>>()
                            .iter(world)
                            .count()
                    && world
                        .query_filtered::<&WorldGeneration, With<DebrisPose>>()
                        .iter(world)
                        .all(|generation| *generation == WorldGeneration(1))
            });
            if saw_destroyed {
                clients.push(network_client(port, 2));
            }
        }
        if saw_destroyed && !combat_joined {
            let world = clients[1].world_mut();
            combat_joined = battle(world).is_some_and(|b| b.number == 1 && b.is_combat())
                && !world.resource::<Record>().entries.is_empty()
                && world
                    .query::<(
                        &Piece,
                        &crate::construction::breakage::AdmittedPiece,
                        &DebrisPose,
                    )>()
                    .iter(world)
                    .count()
                    == world
                        .query_filtered::<&Breakage, Without<Piece>>()
                        .iter(world)
                        .count();
        }
        if combat_joined && reconnect_link.is_none() && state.number == 1 {
            let client = &mut clients[0];
            previous_client_roots = roots(client.world_mut());
            let link = client
                .world_mut()
                .query_filtered::<Entity, With<NetcodeClient>>()
                .single(client.world())
                .unwrap();
            client.world_mut().trigger(Disconnect { entity: link });
            reconnect_link = Some(link);
        }
        if let Some(link) = reconnect_link {
            let client = &mut clients[0];
            if !reconnect_requested && battle(client.world_mut()).is_none() {
                assert!(client.world().resource::<CurrentWorld>().0.is_none());
                assert_eq!(
                    client
                        .world_mut()
                        .query::<&DebrisPose>()
                        .iter(client.world())
                        .count(),
                    0,
                    "disconnect must retire the previous link's carriers"
                );
                client.world_mut().trigger(Connect { entity: link });
                reconnect_requested = true;
            }
            if reconnect_requested
                && !reconnected
                && battle(client.world_mut()).is_some_and(|b| b.number == 1)
                && client.world().resource::<Record>().entries
                    == server.world().resource::<Record>().entries
                && client
                    .world_mut()
                    .query::<(
                        &Piece,
                        &crate::construction::breakage::AdmittedPiece,
                        &DebrisPose,
                    )>()
                    .iter(client.world())
                    .count()
                    == client
                        .world_mut()
                        .query_filtered::<&Breakage, Without<Piece>>()
                        .iter(client.world())
                        .count()
            {
                assert!(
                    previous_client_roots
                        .iter()
                        .all(|root| client.world().get_entity(*root).is_err()),
                    "reconnecting to the same Battle still reconstructs map roots"
                );
                reconnected = true;
            }
        }
        if state.number == 2 {
            assert_eq!(&census(server.world_mut()), fresh.as_ref().unwrap());
            assert!(
                old_roots
                    .iter()
                    .all(|root| server.world().get_entity(*root).is_err())
            );
            assert!(server.world().resource::<Record>().entries.is_empty());
            assert!(
                server
                    .world_mut()
                    .query_filtered::<&WorldGeneration, With<DebrisPose>>()
                    .iter(server.world())
                    .all(|generation| *generation == WorldGeneration(2)),
                "the boundary retires every previous Battle's carrier on the authority"
            );
            if !next_joined {
                clients.push(network_client(port, 3));
                next_joined = true;
            }
            if clients
                .iter_mut()
                .all(|client| battle(client.world_mut()).is_some_and(|b| b.number == 2))
            {
                assert!(
                    saw_destroyed && combat_joined && reconnected,
                    "the first Battle, late join and same-Battle reconnect must observe common construction state"
                );
                for client in &mut clients {
                    assert_eq!(&census(client.world_mut()), fresh.as_ref().unwrap());
                    assert!(client.world().resource::<Record>().entries.is_empty());
                }
                eprintln!(
                    "MEASURED: world cycle rebuilt {} map identities and {} colliders on authority \
                     and {} clients; joins across Battles isolated at fixture step {step}",
                    fresh.as_ref().unwrap().identities.len(),
                    fresh.as_ref().unwrap().colliders,
                    clients.len()
                );
                return;
            }
        }
    }
    panic!("the world cycle did not replicate a fresh Battle to every client");
}
