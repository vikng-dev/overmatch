//! The game's environment destruction and damage model, game state only: a new Battle must
//! reconstruct the same map identities, colliders and full segment pools as startup.

use super::*;
use crate::ballistics::ComponentHealth;
use crate::buildings::BuildingElement;
use crate::buildings::breakage::{
    Breakage, BuildingChunk, WallEntry, WallEvent, WallPiece, WallRecord,
};
use crate::buildings::fracture::Fate;
use crate::net::battle::BattlePhase;
use crate::net::debris_stress::DebrisPose;
use crate::net::destruction::{DestructionLedger, LedgerEntry};
use crate::net::test_harness::{TICK, base_app, finish, free_port, lock_real_udp_test};
use crate::net::walls::WallLedger;
use crate::scatter::MapInstance;
use crate::trees::{TreeBody, TreeParts, TreeSegment, TreeState};
use avian3d::prelude::*;
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use std::net::{Ipv4Addr, SocketAddr};

// Fixture inputs: a flat surface, short mode and loopback peers. Comparisons below concern
// construction and generation, not the fixture assets' particular dimensions or tuning.
fn build_scene(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    let manifest = crate::map::tests::mixed_fixture_manifest();
    let grid = crate::terrain_grid::HeightGrid::new(vec![0.0; 9].into(), 3, manifest.extent);
    let mut blocks = Vec::new();
    let obstacles = crate::scatter::spawn(
        &mut commands,
        &mut meshes,
        &manifest,
        &grid,
        &mut blocks,
        None,
    );
    commands.insert_resource(crate::ground_field::GroundField::unpainted(
        &crate::ground_types::GroundTypes::load_shipped(),
    ));
    commands.insert_resource(crate::world::TerrainMap {
        revision: 0,
        blocks,
    });
    commands.insert_resource(crate::world::SpawnObstacles(obstacles));
    commands.insert_resource(grid);
}

fn environment(app: &mut App) {
    app.add_plugins((
        bevy::transform::TransformPlugin,
        crate::contact_transfer::sim_plugin,
        crate::buildings::breakage::sim_plugin,
        crate::trees::sim_plugin,
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
    crate::net::walls::install_client(&mut app);
    crate::net::destruction::install_apply(&mut app);
    finish(&mut app);
    app
}

#[derive(Debug, PartialEq)]
struct Census {
    identities: Vec<(String, u16, Transform)>,
    elements: Vec<(String, u16)>,
    pools: Vec<(String, u8, ComponentHealth)>,
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
        .query::<(&ChildOf, &BuildingElement)>()
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
    let mut pools = Vec::new();
    for (identity, parts) in world.query::<(&MapInstance, &TreeParts)>().iter(world) {
        for part in &parts.0 {
            pools.push((
                identity.id.clone(),
                world.get::<TreeSegment>(*part).unwrap().0,
                *world.get::<ComponentHealth>(*part).unwrap(),
            ));
        }
    }
    pools.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
    Census {
        identities,
        elements,
        pools,
        colliders: world.query::<&Collider>().iter(world).count(),
        chunks: world.query::<&BuildingChunk>().iter(world).count(),
        loose: world
            .query_filtered::<(), Or<(
                With<crate::debris::DebrisPiece>,
                With<crate::trees::LooseTrunk>,
            )>>()
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

fn topology(world: &mut World) -> (Vec<WallEntry>, TreeState, u16) {
    let (identity, breakage) = world
        .query::<(&MapInstance, &Breakage)>()
        .single(world)
        .unwrap();
    let element = (0..breakage.kind.layout.elements.len())
        .find(|&e| !breakage.kind.layout.chunks_of(e).is_empty())
        .unwrap();
    let chunk = breakage.kind.layout.chunks_of(element).next().unwrap();
    let entries = [
        WallEvent::Swap(element as u16),
        WallEvent::Fall(chunk as u16),
    ]
    .map(|event| WallEntry {
        building: identity.ordinal,
        event,
        tick: 0,
    })
    .to_vec();
    let (tree, body) = world
        .query::<(&MapInstance, &TreeBody)>()
        .single(world)
        .unwrap();
    let state = TreeState::Down {
        cut: (body.kind.segments.len() / 2) as u8,
        dir: 0,
    };
    (entries, state, tree.ordinal)
}

fn snapshots(world: &mut World, number: u32, walls: Vec<WallEntry>, state: TreeState, tree: u16) {
    world.spawn((WorldGeneration(number), WallLedger(walls)));
    world.spawn((
        WorldGeneration(number),
        DestructionLedger(vec![LedgerEntry::State {
            index: tree,
            word: state.encode(),
        }]),
    ));
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
        let (walls, state, tree) = topology(app.world_mut());
        snapshots(app.world_mut(), 1, walls.clone(), state, tree);
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
                crate::trees::TreePiece {
                    tree,
                    cut: match state {
                        TreeState::Down { cut, .. } => cut,
                        _ => unreachable!(),
                    },
                },
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
            "a current non-wall carrier becomes visible before its world retires"
        );
        if !phase_first {
            snapshots(app.world_mut(), 2, Vec::new(), TreeState::Standing, tree);
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
        assert!(app.world().resource::<WallRecord>().entries.is_empty());
        if phase_first {
            snapshots(app.world_mut(), 2, Vec::new(), TreeState::Standing, tree);
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
            .query::<(&WorldGeneration, &mut WallLedger)>()
            .iter_mut(app.world_mut())
        {
            if *belongs == WorldGeneration(2) {
                ledger.0.clone_from(&walls);
            }
        }
        for (belongs, mut ledger) in app
            .world_mut()
            .query::<(&WorldGeneration, &mut DestructionLedger)>()
            .iter_mut(app.world_mut())
        {
            if *belongs == WorldGeneration(2) {
                ledger.0.push(LedgerEntry::State {
                    index: tree,
                    word: state.encode(),
                });
            }
        }
        app.update();
        assert_eq!(
            app.world().resource::<WallRecord>().entries,
            walls,
            "the later Battle's wall consumer must apply its own records independently of trees"
        );
        assert_eq!(
            app.world_mut()
                .query::<&TreeState>()
                .single(app.world())
                .unwrap(),
            &state,
            "the later Battle's tree consumer must apply its own records independently of walls"
        );
        assert_ne!(
            census(app.world_mut()).elements,
            fresh.elements,
            "the later Battle's wall records must reach collision construction"
        );
        let previous_construction = roots(app.world_mut());
        app.world_mut().despawn(battle);
        type OldWire = Or<(With<WallLedger>, With<DestructionLedger>, With<DebrisPose>)>;
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
        snapshots(app.world_mut(), 2, Vec::new(), TreeState::Standing, tree);
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

/// Startup and rebuild must select environment records from the same installed subsystems.
/// A composition with no wall or tree publisher must not gain that publisher's ledger later.
#[test]
fn rebuilding_constructs_only_the_environment_records_installed_at_startup() {
    let _udp = lock_real_udp_test();
    for (walls, trees) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut server = crate::net::link_drop::build_server(free_port());
        environment(&mut server);
        if walls {
            crate::net::walls::install_server(&mut server);
        }
        if trees {
            crate::net::destruction::install_server(&mut server);
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
            (
                world
                    .query_filtered::<Entity, With<WallLedger>>()
                    .iter(world)
                    .collect::<Vec<_>>(),
                world
                    .query_filtered::<Entity, With<DestructionLedger>>()
                    .iter(world)
                    .collect::<Vec<_>>(),
            )
        };
        let initial = records(server.world_mut());
        assert_eq!(
            (initial.0.len(), initial.1.len()),
            (usize::from(walls), usize::from(trees))
        );
        start_next(server.world_mut(), 2);
        let rebuilt = records(server.world_mut());
        assert_eq!(
            (rebuilt.0.len(), rebuilt.1.len()),
            (usize::from(walls), usize::from(trees)),
            "rebuilding must not install an absent subsystem"
        );
        assert!(
            initial
                .0
                .iter()
                .chain(&initial.1)
                .all(|&entity| server.world().get_entity(entity).is_err()),
            "a ledger identity cannot span generations"
        );
        assert!(
            rebuilt
                .0
                .iter()
                .chain(&rebuilt.1)
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
    crate::net::walls::install_client(&mut app);
    crate::net::destruction::install_client(&mut app);
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
    type Ledgers = Or<(With<WallLedger>, With<DestructionLedger>)>;
    assert!(
        server
            .world_mut()
            .query_filtered::<&WorldGeneration, Ledgers>()
            .iter(server.world())
            .all(|generation| *generation == WorldGeneration(number)),
        "authority publication carries the current Battle's nonempty generation"
    );
}

/// FRESH BATTLE EQUALS STARTUP BY COUNT AND IDENTITY, on authority, connected client and late
/// joiners. The game's damage model: a wall chunk and trunk leave their map roots during combat;
/// the next Battle replaces them and every segment pool. Keeping a ledger, a loose body, or
/// a damaged root would fail this comparison for the next Battle's players.
#[test]
fn a_destroyed_world_is_startup_again_at_next_battle_over_real_udp() {
    let _udp = lock_real_udp_test();
    let port = free_port();
    let mut server = crate::net::link_drop::build_server(port);
    environment(&mut server);
    crate::net::walls::install_server(&mut server);
    crate::net::destruction::install_server(&mut server);
    crate::net::debris_stress::install_server(&mut server).unwrap();
    let mut mode =
        crate::net::battle_mode::BattleMode::parse(include_str!("../fixtures/battle-short.ron"));
    // Fixture: allow loopback peers to join throughout this combat window.
    mode.combat = std::time::Duration::from_secs(4);
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
            let (walls, state, tree) = topology(server.world_mut());
            let (root, _) = server
                .world_mut()
                .query::<(Entity, &Breakage)>()
                .single(server.world())
                .unwrap();
            let WallEvent::Swap(element) = walls[0].event else {
                unreachable!()
            };
            let WallEvent::Fall(chunk) = walls[1].event else {
                unreachable!()
            };
            {
                let mut breakage = server.world_mut().get_mut::<Breakage>(root).unwrap();
                let live = breakage.live.as_mut();
                live.swapped[usize::from(element)] = true;
                live.state.fate[usize::from(chunk)] = Fate::Fallen;
            }
            server
                .world_mut()
                .resource_mut::<WallRecord>()
                .entries
                .extend(walls.clone());
            server.world_mut().spawn((
                crate::debris::DebrisPiece,
                WallPiece {
                    building: walls[0].building,
                    chunks: vec![chunk],
                    born: 0,
                    retired: None,
                },
                RigidBody::Dynamic,
                // Fixture: frictionless, 1 m sides, 1 kg/m³. The probe observes carrier retirement.
                Collider::cuboid(1.0, 1.0, 1.0),
                ColliderDensity(1.0),
                Friction::ZERO,
                crate::buildings::breakage::piece_layers(),
                Position(Vec3::Y * 20.0),
                Rotation::default(),
                Transform::from_translation(Vec3::Y * 20.0),
            ));
            let world = server.world_mut();
            let tree_root = world
                .query::<(Entity, &MapInstance, &TreeBody)>()
                .iter(world)
                .find(|(_, id, _)| id.ordinal == tree)
                .unwrap()
                .0;
            *world.get_mut::<TreeState>(tree_root).unwrap() = state;
            let part = world.get::<TreeParts>(tree_root).unwrap().0[0];
            world.get_mut::<ComponentHealth>(part).unwrap().set_held(0);
            destroyed = true;
        }
        if state.number == 1 && state.is_combat() && destroyed && !saw_destroyed {
            saw_destroyed = clients.iter_mut().all(|client| {
                let world = client.world_mut();
                world
                    .query::<&TreeState>()
                    .iter(world)
                    .any(|state| *state != TreeState::Standing)
                    && world
                        .query::<&Breakage>()
                        .iter(world)
                        .any(|b| b.shape().fate.contains(&Fate::Fallen))
                    && world
                        .query::<&crate::trees::LooseTrunk>()
                        .iter(world)
                        .next()
                        .is_some()
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
                && world
                    .query::<&TreeState>()
                    .iter(world)
                    .any(|s| *s != TreeState::Standing)
                && !world.resource::<WallRecord>().entries.is_empty()
                && world
                    .query::<&crate::trees::LooseTrunk>()
                    .iter(world)
                    .next()
                    .is_some();
        }
        if state.number == 2 {
            assert_eq!(&census(server.world_mut()), fresh.as_ref().unwrap());
            assert!(
                old_roots
                    .iter()
                    .all(|root| server.world().get_entity(*root).is_err())
            );
            assert!(server.world().resource::<WallRecord>().entries.is_empty());
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
                    saw_destroyed && combat_joined,
                    "the first Battle and its late join must observe destruction"
                );
                for client in &mut clients {
                    assert_eq!(&census(client.world_mut()), fresh.as_ref().unwrap());
                    assert!(client.world().resource::<WallRecord>().entries.is_empty());
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
