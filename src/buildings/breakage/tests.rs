//! The breakage invariants, about the game's environment destruction (game state only), on the
//! shipped house as fixture data: which chunk a test reads is chosen by a class rule (a breakable
//! element's chunk nearest its centre, a chunk with no anchor), never by its number.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::time::TimeUpdateStrategy;

use super::*;
use crate::ballistics::MarchProbe;
use crate::buildings::tests::shipped_house;

/// The breakage sim on the offline physics, the march probe, and the shipped house spawned at
/// the origin (unit scale), one fixed tick per update. `replica` adds the replica marker.
fn composed(replica: bool) -> (App, Entity) {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::transform::TransformPlugin,
        AssetPlugin::default(),
        crate::offline_physics(),
        sim_plugin,
        crate::debris::sim_plugin,
        crate::ballistics::mount_march_probe,
    ))
    .init_asset::<Mesh>()
    .insert_resource(Gravity::ZERO)
    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
        15_625,
    )));
    if replica {
        app.init_resource::<ClientReplica>();
    }
    while app.plugins_state() == bevy::app::PluginsState::Adding {
        std::thread::sleep(Duration::from_millis(1));
    }
    app.finish();
    app.cleanup();
    let kind = shipped_house();
    let world = app.world_mut();
    let mut queue = bevy::ecs::world::CommandQueue::default();
    let mut commands = Commands::new(&mut queue, world);
    let root = crate::buildings::spawn(
        &mut commands,
        &kind,
        MapInstance {
            id: "house_0001".to_owned(),
            ordinal: 3,
        },
        Transform::IDENTITY,
    );
    queue.apply(world);
    (app, root)
}

/// A breakable element with at least three chunks, and its three chunks whose centroids lie
/// nearest the element's centre, nearest first: the fixture's choice by class.
fn fixture_chunks(kind: &BuildingKind) -> (usize, [usize; 3]) {
    let layout = &kind.layout;
    let e = (0..layout.elements.len())
        .find(|&e| layout.chunks_of(e).len() >= 3)
        .expect("the house has a breakable element of three chunks");
    let centre = layout.element_blocks[e].center();
    let centroid =
        |c: usize| Vec3::from_array(kind.asset.chunks[layout.chunks[c] as usize].centroid);
    let mut chunks: Vec<usize> = layout.chunks_of(e).collect();
    chunks.sort_by(|&a, &b| {
        centroid(a)
            .distance(centre)
            .total_cmp(&centroid(b).distance(centre))
    });
    (e, [chunks[0], chunks[1], chunks[2]])
}

/// A probe ray through chunk `c` along its element's extrusion axis, from half a metre before its
/// near face to `beyond` m past its far face.
fn through(kind: &BuildingKind, e: usize, c: usize, beyond: f32) -> (Vec3, Dir3, f32) {
    let element = &kind.asset.elements[e];
    let w = element.frame.w().normalize();
    let origin = Vec3::from_array(element.frame.origin);
    let centroid = Vec3::from_array(kind.asset.chunks[kind.layout.chunks[c] as usize].centroid);
    let depth = (centroid - origin).dot(w);
    let near = centroid - w * (depth - element.depth[0] + 0.5);
    let length = element.depth[1] - element.depth[0] + 0.5 + beyond;
    (near, Dir3::new(w).unwrap(), length)
}

/// A CHUNK TURNED TO DUST OR FALLEN NEVER BLOCKS A SHOT AS THE INTACT ELEMENT: before the break a
/// round through two chunks of a whole element meets the element; once the laws swap the element,
/// turn one chunk to dust and drop the other, the march of the next tick finds no collider where
/// either stood, and a round through a third, standing chunk meets that chunk's own collider, not
/// the element. A swap that kept the element's collider, or a derive that left a gone chunk's
/// collider (or none for a standing one), or a refit left to the next physics step, fails here.
#[test]
fn a_chunk_gone_never_blocks_as_the_intact_element() {
    let (mut app, root) = composed(false);
    let kind = shipped_house();
    let (e, [dusted, fallen, standing]) = fixture_chunks(&kind);
    {
        let mut probe = app.world_mut().resource_mut::<MarchProbe>();
        probe.radius = 0.044;
        probe.rays = [dusted, fallen, standing]
            .map(|c| through(&kind, e, c, 0.3))
            .to_vec();
    }
    app.update();
    app.update();
    let element = app
        .world_mut()
        .query::<(Entity, &BuildingElement)>()
        .iter(app.world())
        .find(|(_, k)| usize::from(k.0) == e)
        .map(|(entity, _)| entity)
        .unwrap();
    assert_eq!(
        app.world().resource::<MarchProbe>().reached.last(),
        Some(&vec![Some(element); 3]),
        "whole, the element stops every ray"
    );
    {
        let mut breakage = app.world_mut().get_mut::<Breakage>(root).unwrap();
        let live = breakage.live();
        live.swapped[e] = true;
        live.state.fate[dusted] = Fate::Dust;
        live.state.fate[fallen] = Fate::Fallen;
    }
    let ticks = app.world().resource::<MarchProbe>().reached.len();
    app.update();
    app.update();
    let world = app.world_mut();
    let chunk_of = world
        .query::<(Entity, &BuildingChunk)>()
        .iter(world)
        .find(|(_, c)| usize::from(c.0) == standing)
        .map(|(entity, _)| entity)
        .expect("the standing chunk is a collider");
    let reached = &world.resource::<MarchProbe>().reached;
    assert!(reached.len() > ticks);
    assert_eq!(
        reached.last(),
        Some(&vec![None, None, Some(chunk_of)]),
        "broken, the gone chunks block nothing and the standing one blocks as itself"
    );
    assert!(world.get_entity(element).is_err(), "the element is gone");
}

/// THE RECORD A REPLICA DERIVES FROM IS THE AUTHORITY'S STATE: the laws run on the authority's
/// copy of the house (a chunk above grade with no anchor whose every neighbour takes more than
/// its pool, so it is left with no joint and falls; the deposit's sources crack seams around),
/// and a replica
/// fed only the record's events derives the same swaps, fates and cracks, and stands exactly the
/// standing chunks of the swapped elements as colliders. A replaced record (another link's) stands
/// the house whole again with its elements. A replica that missed an event kind, or derived from
/// anything but the record, fails here.
#[test]
fn the_record_a_replica_derives_from_is_the_authoritys() {
    let kind = shipped_house();
    let mut authority = Live::new(&kind, 1.0);
    let structure = authority.structure.clone();
    let anchored: Vec<bool> = (0..structure.chunks.len())
        .map(|c| {
            structure.chunks[c].buried
                || structure
                    .joints
                    .iter()
                    .any(|j| j.a == c && j.b == Side::Ground)
        })
        .collect();
    let (_, [centre, ..]) = fixture_chunks(&kind);
    let lone = if anchored[centre] {
        (0..structure.chunks.len()).find(|&c| !anchored[c]).unwrap()
    } else {
        centre
    };
    let neighbours: Vec<usize> = structure
        .joints
        .iter()
        .filter_map(|j| match j.b {
            Side::Chunk(b) if j.a == lone => Some(b),
            Side::Chunk(b) if b == lone => Some(j.a),
            _ => None,
        })
        .collect();
    assert!(!neighbours.is_empty());
    let hits: Vec<(usize, Vec3, f32)> = neighbours
        .iter()
        .map(|&c| {
            let chunk = &structure.chunks[c];
            (c, chunk.centroid, 2.0 * fracture::full_pool(chunk))
        })
        .collect();
    let changes = run_laws(
        &kind.layout,
        &mut authority,
        &Loading::field(&hits),
        Vec::new(),
    );
    for kind_of in [
        |e: &WallEvent| matches!(e, WallEvent::Swap(_)),
        |e: &WallEvent| matches!(e, WallEvent::Dust(_)),
        |e: &WallEvent| matches!(e, WallEvent::Crack(_)),
        |e: &WallEvent| matches!(e, WallEvent::Fall(_)),
    ] {
        assert!(changes.events.iter().any(kind_of), "{:?}", changes.events);
    }
    assert!(changes.events.contains(&WallEvent::Fall(lone as u16)));

    let (mut app, root) = composed(true);
    app.world_mut()
        .resource_mut::<WallRecord>()
        .entries
        .extend(changes.events.iter().map(|&event| WallEntry {
            building: 3,
            event,
            tick: 7,
        }));
    app.update();
    let world = app.world_mut();
    let derived = world.get::<Breakage>(root).unwrap().shape();
    assert_eq!(
        derived,
        authority.shape(),
        "the replica's shape is the authority's"
    );
    let present: usize = (0..kind.layout.chunks.len())
        .filter(|&c| derived.chunk_present(&kind.layout, c))
        .count();
    assert_eq!(
        world.query::<&BuildingChunk>().iter(world).count(),
        present,
        "one collider per standing chunk of a swapped element"
    );
    let swapped = derived.swapped.iter().filter(|s| **s).count();
    assert_eq!(
        world.query::<&BuildingElement>().iter(world).count(),
        kind.asset.elements.len() - swapped
    );

    let mut record = world.resource_mut::<WallRecord>();
    record.entries.clear();
    record.generation += 1;
    app.update();
    let world = app.world_mut();
    assert_eq!(
        world.get::<Breakage>(root).unwrap().shape(),
        Shape::intact(&kind.layout),
        "a replaced record stands the house whole"
    );
    assert_eq!(world.query::<&BuildingChunk>().iter(world).count(), 0);
    assert_eq!(
        world.query::<&BuildingElement>().iter(world).count(),
        kind.asset.elements.len()
    );
}

/// A ROUND MEETS A FALLEN PIECE AS ITS SUBSTANCE: a fallen group's body carries each chunk as a
/// collider on the layer the walk reads, a ballistic volume of its element's substance, so the
/// march of the next tick through the piece reaches its chunk. A piece spawned out of ballistics
/// (the sandbox's old default layer) or as no volume fails here.
#[test]
fn a_round_meets_a_fallen_piece_as_its_substance() {
    let (mut app, _) = composed(false);
    let kind = shipped_house();
    let (e, [c, ..]) = fixture_chunks(&kind);
    let live = Live::new(&kind, 1.0);
    let lifted = Transform::from_xyz(30.0, 0.0, 0.0);
    {
        let world = app.world_mut();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, world);
        spawn_piece(
            &mut commands,
            &kind,
            &live.structure,
            (3, 1.0, lifted),
            &[c],
            &[],
        );
        queue.apply(world);
    }
    let (origin, dir, length) = through(&kind, e, c, 0.3);
    app.world_mut().resource_mut::<MarchProbe>().rays =
        vec![(origin + lifted.translation, dir, length)];
    app.world_mut().resource_mut::<MarchProbe>().radius = 0.044;
    app.update();
    app.update();
    let world = app.world();
    let hit = world
        .resource::<MarchProbe>()
        .reached
        .last()
        .and_then(|rays| rays[0])
        .expect("the probe meets the piece");
    let volume = world
        .get::<BallisticVolume>(hit)
        .expect("a ballistic volume");
    let element = &kind.asset.elements[e];
    assert_eq!(
        volume.substance,
        kind.asset.materials[usize::from(element.material)]
    );
    let body = world.get::<ChildOf>(hit).unwrap().parent();
    assert!(world.get::<WallPiece>(body).is_some());
}

/// A FALLEN PIECE COMES TO REST ASLEEP, A DYNAMIC BODY STILL: a group spawned by `spawn_piece`
/// above a static slab falls, lands and sleeps there (never in the air), and keeps
/// `RigidBody::Dynamic`. The server's per-tick cost of settled rubble rests on it: a spawn that
/// disables sleeping on a piece (or makes it static at rest) leaves every one in the solver or
/// rebuilds its contacts on every wake. Built through `spawn_piece`, so the mutant can go red.
#[test]
fn a_fallen_piece_comes_to_rest_asleep_a_dynamic_body_still() {
    let (mut app, _) = composed(false);
    app.insert_resource(Gravity(Vec3::NEG_Y * 9.81));
    let kind = shipped_house();
    let (_, [c, ..]) = fixture_chunks(&kind);
    let live = Live::new(&kind, 1.0);
    let at = Transform::from_xyz(30.0, 0.0, 0.0);
    {
        let world = app.world_mut();
        world.spawn((
            RigidBody::Static,
            Position(Vec3::new(30.0, -8.5, 0.0)),
            Collider::cuboid(40.0, 1.0, 40.0),
            CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
        ));
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, world);
        spawn_piece(
            &mut commands,
            &kind,
            &live.structure,
            (3, 1.0, at),
            &[c],
            &[],
        );
        queue.apply(world);
    }
    let piece = app
        .world_mut()
        .query_filtered::<Entity, With<WallPiece>>()
        .single(app.world())
        .expect("one piece");
    let tick = (0..1280)
        .find(|_| {
            app.update();
            app.world().get::<Sleeping>(piece).is_some()
        })
        .expect("the piece fell, came to rest and slept within 20 s");
    let height = app.world().get::<Position>(piece).unwrap().0.y;
    assert!(
        height < -7.0,
        "asleep at tick {tick} at {height} m, on the slab, not in the air"
    );
    assert_eq!(
        app.world().get::<RigidBody>(piece),
        Some(&RigidBody::Dynamic)
    );
}

/// THE DEPOSITS' WORLD BALL NEVER DROPS WHAT THE BUILDING'S BOUNDS ACCEPT: for the shipped house
/// (fixture data) at a fixture pose and scale, every blast direction whose ray from a fixture
/// point the bounds' own test meets ([`Block::span`] in the building frame) passes
/// [`Frame::may_meet`], from points inside, beside, above and 1.4 km from the house; and every
/// fixture point the field-stop test admits passes [`Frame::may_hold`]. A ball that clipped the
/// bounds would drop a blast ray or a field stop the building took, and with it a deposit; the
/// deposits rely on the ball only to skip work.
#[test]
fn the_world_ball_keeps_every_ray_the_bounds_meet() {
    let kind = shipped_house();
    let (lo, hi) = kind.bounds();
    let scale = 1.137;
    let pose = Transform::from_xyz(312.5, -4.25, -187.0).with_rotation(Quat::from_rotation_y(0.7));
    let frame = Frame::new(&pose, scale, (lo, hi));
    let bounds = Block::cuboid(lo, hi);
    let world = |p: Vec3| pose.translation + pose.rotation * (p * scale);
    let middle = (lo + hi) / 2.0;
    let origins = [
        world(middle),
        world(Vec3::new(hi.x + 0.3, middle.y, middle.z)),
        world(Vec3::new(middle.x, hi.y + 2.0, lo.z - 0.5)),
        world(hi + Vec3::splat(6.0)),
        world(middle) + Vec3::new(1400.0, 3.0, -50.0),
    ];
    let mut met = 0;
    for origin in origins {
        let local = frame.unscaled(origin);
        for &(dir, _) in fracture::blast_directions() {
            if bounds.span(local, frame.turn(dir)).is_some() {
                met += 1;
                assert!(
                    frame.may_meet(origin, dir),
                    "{dir} from {origin} meets the bounds"
                );
            }
        }
    }
    assert!(
        met > fracture::BLAST_RAYS,
        "the fixture points meet the bounds"
    );
    let slack = Vec3::splat(STOP_TOLERANCE_M / scale);
    for corner in 0..8 {
        let pick = |bit: usize, a: f32, b: f32| if corner >> bit & 1 == 0 { a } else { b };
        let local = Vec3::new(
            pick(0, lo.x - slack.x, hi.x + slack.x),
            pick(1, lo.y - slack.y, hi.y + slack.y),
            pick(2, lo.z - slack.z, hi.z + slack.z),
        );
        assert!(
            frame.may_hold(world(local), STOP_TOLERANCE_M),
            "corner {corner}"
        );
    }
}
