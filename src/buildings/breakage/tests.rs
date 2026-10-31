//! The breakage invariants, about the game's environment destruction (game state only), on the
//! shipped house as fixture data: which chunk a test reads is chosen by a class rule (a breakable
//! element's chunk nearest its centre, a chunk with no anchor), never by its number.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::time::TimeUpdateStrategy;

use super::*;
use crate::ballistics::MarchProbe;
use crate::buildings::schema::ElementKind;
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

/// A LOOSE PIECE CARRIES AN ENVELOPE HOLDING EVERY COLLIDER IT FALLS WITH (the game's damage
/// model: a round's crossing of the piece runs until it has left the envelope, at both of the
/// moving body's poses, `ballistics::Envelope`). A group of three chunks spawned through
/// `spawn_piece` from a turned building (fixture pose): a piece spawned without its envelope, or
/// with one that misses a chunk, fails here.
#[test]
fn a_loose_piece_carries_an_envelope_holding_its_colliders() {
    let kind = shipped_house();
    let (_, chunks) = fixture_chunks(&kind);
    let live = Live::new(&kind, 1.0);
    let at = Transform::from_xyz(30.0, 0.0, 0.0).with_rotation(Quat::from_rotation_y(0.7));
    let mut world = World::new();
    let mut queue = bevy::ecs::world::CommandQueue::default();
    let mut commands = Commands::new(&mut queue, &world);
    spawn_piece(
        &mut commands,
        &kind,
        &live.structure,
        (3, 1.0, at),
        &chunks,
        &[],
    );
    queue.apply(&mut world);
    let piece = world
        .query_filtered::<Entity, With<WallPiece>>()
        .single(&world)
        .expect("one piece");
    assert_eq!(
        crate::ballistics::assert_envelope_holds(&world, piece),
        chunks.len()
    );
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

/// A round's line through law chunk `c` of element `e` (building frame, unit scale): 10 cm along
/// the element's extrusion axis about the chunk's centroid, carrying `share` of its full pool.
fn line_through_chunk(
    kind: &BuildingKind,
    structure: &Structure,
    e: usize,
    c: usize,
    share: f32,
) -> (usize, Vec3, Vec3, f32) {
    let w = kind.asset.elements[e].frame.w().normalize();
    let chunk = &structure.chunks[c];
    (
        c,
        chunk.centroid - w * 0.05,
        chunk.centroid + w * 0.05,
        share * fracture::full_pool(chunk),
    )
}

/// A blast's single ray on law chunk `c` at its centroid, carrying `share` of its full pool, as
/// the laws read it, with its push.
fn blast_on(structure: &Structure, c: usize, share: f32) -> (Loading, Vec<Push>) {
    let chunk = &structure.chunks[c];
    Loading::blast(&[fracture::RayHit {
        chunk: c,
        at: chunk.centroid,
        energy: share * fracture::full_pool(chunk),
        impulse: Vec3::ZERO,
    }])
}

/// The record entries of `changes` on the composed house (ordinal 3) at `tick`.
fn entries(changes: &Changes, tick: u32) -> Vec<WallEntry> {
    changes
        .events
        .iter()
        .map(|&event| WallEntry {
            building: 3,
            event,
            tick,
        })
        .collect()
}

/// The holes a replica's house holds.
fn held_holes(app: &App, root: Entity) -> Vec<Vec<Hole>> {
    app.world()
        .get::<Breakage>(root)
        .unwrap()
        .live
        .as_ref()
        .expect("the record touched the house")
        .holes
        .clone()
}

/// THE CLIENTS HOLD THE AUTHORITY'S HOLES, A LATE JOINER TOO: two deposits on the authority's
/// house, a round's line through one chunk and a blast's ray on another, each cut a hole the laws
/// keep (a channel along the line, a dish about the stop); a replica fed the record as it grows and
/// a late joiner fed it whole hold every chunk's holes as the authority does, in order. The view
/// draws from a replica's lists, so a derive that dropped a hole event, put it in another chunk or
/// kept it twice fails here.
#[test]
fn the_clients_hold_the_authoritys_holes_a_late_joiner_too() {
    let kind = shipped_house();
    let (e, [channel_chunk, dish_chunk, _]) = fixture_chunks(&kind);
    let mut authority = Live::new(&kind, 1.0);
    let lines = [line_through_chunk(
        &kind,
        &authority.structure,
        e,
        channel_chunk,
        0.05,
    )];
    let first = run_laws(
        &kind.layout,
        &mut authority,
        &Loading::lines(&lines),
        Vec::new(),
        holes_of(&kind, 1.0, &lines, &Loading::default()),
    );
    let (blast, pushes) = blast_on(&authority.structure, dish_chunk, 0.05);
    let holes = holes_of(&kind, 1.0, &[], &blast);
    let second = run_laws(&kind.layout, &mut authority, &blast, pushes, holes);
    let [channel] = authority.holes[channel_chunk][..] else {
        panic!("one channel: {:?}", authority.holes[channel_chunk]);
    };
    let [dish] = authority.holes[dish_chunk][..] else {
        panic!("one dish: {:?}", authority.holes[dish_chunk]);
    };
    assert_ne!(channel.from, channel.to, "a channel runs along its line");
    assert_eq!(dish.from, dish.to, "a dish is a ball about its stop");

    let (mut growing, root) = composed(true);
    for (changes, tick) in [(&first, 7), (&second, 9)] {
        growing
            .world_mut()
            .resource_mut::<WallRecord>()
            .entries
            .extend(entries(changes, tick));
        growing.update();
    }
    let (mut late, late_root) = composed(true);
    late.world_mut().resource_mut::<WallRecord>().entries =
        [entries(&first, 7), entries(&second, 9)].concat();
    late.update();
    assert_eq!(held_holes(&growing, root), authority.holes, "growing");
    assert_eq!(held_holes(&late, late_root), authority.holes, "late joiner");
}

/// A HOLE CHANGES NO COLLIDER, MASS OR STRUCTURAL RESULT: two copies of the house take the same
/// round twice (its line and its push), one cutting the line's hole and one not; the second
/// round meets the holed chunk whole. Their events but the holes, their loose groups, structural
/// failures, laws' state, swaps and chunk table are the same. A replica fed the record without its
/// holes, then the holes, keeps the very same colliders and shape. The owner's decision (a hole is
/// drawn only) rests on it: a hole read by a law, the mass or the colliders fails here.
#[test]
fn a_hole_changes_no_collider_mass_or_structural_result() {
    let kind = shipped_house();
    let (e, [c, ..]) = fixture_chunks(&kind);
    let mut cut = Live::new(&kind, 1.0);
    let mut whole = Live::new(&kind, 1.0);
    let line = line_through_chunk(&kind, &cut.structure, e, c, 0.3);
    let push = Push {
        chunk: c,
        at: cut.structure.chunks[c].centroid,
        impulse: (line.2 - line.1).normalize() * 100.0,
    };
    let holes = holes_of(&kind, 1.0, &[line], &Loading::default());
    let mut record = Vec::new();
    for tick in [7, 9] {
        let with = run_laws(
            &kind.layout,
            &mut cut,
            &Loading::lines(&[line]),
            vec![push],
            holes.clone(),
        );
        let without = run_laws(
            &kind.layout,
            &mut whole,
            &Loading::lines(&[line]),
            vec![push],
            Vec::new(),
        );
        assert!(
            with.events.contains(&WallEvent::Hole(c as u16, holes[0].1)),
            "round {tick} holes the chunk: {:?}",
            with.events
        );
        let kept: Vec<WallEvent> = with
            .events
            .iter()
            .copied()
            .filter(|event| !matches!(event, WallEvent::Hole(..)))
            .collect();
        assert_eq!(kept, without.events);
        assert_eq!(with.groups, without.groups);
        assert_eq!(with.failures, without.failures);
        record.extend(entries(&with, tick));
    }
    assert_eq!(cut.state, whole.state);
    assert_eq!(cut.swapped, whole.swapped);
    assert_eq!(cut.structure.chunks, whole.structure.chunks);

    let (mut app, root) = composed(true);
    let (holed, unholed): (Vec<WallEntry>, Vec<WallEntry>) = record
        .into_iter()
        .partition(|entry| matches!(entry.event, WallEvent::Hole(..)));
    app.world_mut().resource_mut::<WallRecord>().entries = unholed;
    app.update();
    let colliders = |app: &mut App| {
        let world = app.world_mut();
        let mut chunks: Vec<(Entity, u16)> = world
            .query::<(Entity, &BuildingChunk)>()
            .iter(world)
            .map(|(entity, chunk)| (entity, chunk.0))
            .collect();
        chunks.sort_unstable();
        let mut elements: Vec<Entity> = world
            .query_filtered::<Entity, With<BuildingElement>>()
            .iter(world)
            .collect();
        elements.sort_unstable();
        (
            chunks,
            elements,
            world.get::<Breakage>(root).unwrap().shape(),
        )
    };
    let before = colliders(&mut app);
    assert!(!before.0.is_empty(), "the round swapped the element");
    app.world_mut()
        .resource_mut::<WallRecord>()
        .entries
        .extend(holed);
    app.update();
    assert_eq!(held_holes(&app, root)[c].len(), 2);
    assert_eq!(colliders(&mut app), before, "the holes moved no collider");
}

/// PAST THE CAP A CHUNK CRUMBLES: a chunk keeps a hole from each of [`HOLES_PER_CHUNK`] rounds'
/// lines, each far too weak to spend its pool; the next round, which would carry it past the cap,
/// turns it to dust instead of holing it, so the record says `Dust` and the list stops at the cap.
/// A cap that never fired would grow a chunk's list (and the record) without bound; one that fired
/// early would crumble a chunk the view still draws.
#[test]
fn past_the_cap_a_chunk_crumbles() {
    let kind = shipped_house();
    let (e, [c, ..]) = fixture_chunks(&kind);
    let mut live = Live::new(&kind, 1.0);
    let line = line_through_chunk(&kind, &live.structure, e, c, 1e-3);
    let holes = holes_of(&kind, 1.0, &[line], &Loading::default());
    assert_eq!(holes.len(), 1);
    for k in 1..=HOLES_PER_CHUNK + 1 {
        let changes = run_laws(
            &kind.layout,
            &mut live,
            &Loading::lines(&[line]),
            Vec::new(),
            holes.clone(),
        );
        let holed = changes
            .events
            .iter()
            .any(|event| matches!(event, WallEvent::Hole(h, _) if usize::from(*h) == c));
        let dusted = changes.events.contains(&WallEvent::Dust(c as u16));
        if k <= HOLES_PER_CHUNK {
            assert!(holed && !dusted, "round {k}: {:?}", changes.events);
            assert_eq!(live.holes[c].len(), k);
        } else {
            assert!(dusted && !holed, "round {k}: {:?}", changes.events);
            assert_eq!(live.state.fate[c], Fate::Dust);
            assert_eq!(live.holes[c].len(), HOLES_PER_CHUNK);
        }
    }
}

/// A hole's volume against the crushed volume it should hold, and the relative bound the
/// millimetre rounding allows.
#[derive(Debug)]
struct Volume {
    held: f32,
    crushed: f32,
    bound: f32,
}

impl Volume {
    fn check(&self) {
        let error = (self.held - self.crushed).abs() / self.crushed;
        assert!(error <= self.bound, "{self:?}: off by {error}");
    }
}

/// A HOLE HOLDS THE VOLUME ITS ENERGY CRUSHED: a line's channel (its cylinder about the chord,
/// narrower here than the ball) and a blast's dish (its half ball) each hold `energy / crush`, to
/// the millimetre rounding of their radius and ends. The size rule rests on it: a hole is never
/// the size of a chunk still standing, since that volume is a spent pool and recorded as dust. A
/// radius off by a constant factor fails here.
#[test]
fn a_hole_holds_the_volume_its_energy_crushed() {
    use std::f32::consts::PI;
    let kind = shipped_house();
    let (e, [c, ..]) = fixture_chunks(&kind);
    let structure = kind.layout.structure(&kind.asset, 1.0);
    let crush = structure.chunks[c].material.crush;
    let metres = |p: [i16; 3]| Vec3::from_array(p.map(|v| f32::from(v) / 1000.0));

    let line = line_through_chunk(&kind, &structure, e, c, 0.01);
    let [(_, channel)] = holes_of(&kind, 1.0, &[line], &Loading::default())[..] else {
        panic!("one channel");
    };
    let r = f32::from(channel.radius) / 1000.0;
    let length = metres(channel.from).distance(metres(channel.to));
    let crushed = line.3 / crush;
    assert!(
        r < (0.75 * crushed / PI).cbrt(),
        "the cylinder is the narrower"
    );
    // DERIVED bounds: the radius rounds by up to half a millimetre, each end by up to half a
    // millimetre per axis (the chord by up to √3 mm).
    Volume {
        held: PI * r * r * length,
        crushed,
        bound: 2.0 * 0.0005 / r + 0.0018 / length,
    }
    .check();

    let (blast, _) = blast_on(&structure, c, 0.01);
    let [(_, dish)] = holes_of(&kind, 1.0, &[], &blast)[..] else {
        panic!("one dish");
    };
    let r = f32::from(dish.radius) / 1000.0;
    Volume {
        held: 2.0 / 3.0 * PI * r.powi(3),
        crushed: blast.energies[0].1 / crush,
        bound: 3.0 * 0.0005 / r,
    }
    .check();
}

/// The law chunks of every element of `component` (an index of the asset's components).
fn component_chunks(kind: &BuildingKind, component: u16) -> Vec<usize> {
    (0..kind.asset.elements.len())
        .filter(|&e| kind.asset.elements[e].component == component)
        .flat_map(|e| kind.layout.chunks_of(e))
        .collect()
}

/// The components whose chunks the chunks of `part` rest on (each a live rest whose normal toward
/// `part` rises), ascending.
fn rests_on(kind: &BuildingKind, structure: &Structure, part: &[usize]) -> Vec<u16> {
    let component =
        |c: usize| kind.asset.elements[usize::from(kind.layout.element_of[c])].component;
    let mut under: Vec<u16> = structure
        .joints
        .iter()
        .filter(|joint| joint.bond == Bond::Resting)
        .filter_map(|joint| {
            let Side::Chunk(b) = joint.b else {
                return None;
            };
            let (held, support) = match (part.contains(&joint.a), part.contains(&b)) {
                (true, false) => (joint.a, b),
                (false, true) => (b, joint.a),
                _ => return None,
            };
            let toward = structure.chunks[held].centroid - joint.centroid;
            (joint.normal.dot(toward).signum() * joint.normal.y > 0.0).then(|| component(support))
        })
        .collect();
    under.sort_unstable();
    under.dedup();
    under
}

/// A ROOF PLANE STANDS ON ITS RESTS, BREAKABLE, AND FALLS WHEN THEY GO: on the shipped house
/// (fixture data; the parts chosen by class: a roof plane, the components it rests on, the
/// masonry its plate rests on), a light round through one of the plane's chunks wears it and
/// moves nothing; a blast that turns one of its chunks to dust carves it and the plane stands.
/// Shooting out the long wall under its plate (every standing unburied chunk of the masonry the
/// plate rests on turned to dust) drops the plate, which bore on that wall alone; the plane
/// stands on what else it rests on while its centre of mass lies over those rests, and falls
/// whole once they go too. A check that let a rest carry tension would hold the plate up; one
/// that read a roof's seams as a pull it could lose would break the plane apart.
#[test]
fn a_roof_plane_stands_on_its_rests_and_falls_when_they_go() {
    let kind = shipped_house();
    let layout = &kind.layout;
    let elements = &kind.asset.elements;
    let kinds = |kind_of: ElementKind| {
        (0..elements.len())
            .filter(|&e| elements[e].kind == kind_of)
            .collect::<Vec<_>>()
    };
    let roof_element = kinds(ElementKind::Roof)[0];
    let roof_component = kind.asset.elements[roof_element].component;
    let roof = component_chunks(&kind, roof_component);
    let fresh = || Live::new(&kind, 1.0);
    let base = fresh();
    let under_roof = rests_on(&kind, &base.structure, &roof);
    let plate = kinds(ElementKind::WallPlate)
        .into_iter()
        .map(|e| kind.asset.elements[e].component)
        .find(|c| under_roof.contains(c))
        .expect("the roof plane rests on a wall plate");
    let plate_chunks = component_chunks(&kind, plate);
    let wall: Vec<u16> = rests_on(&kind, &base.structure, &plate_chunks);
    let standing =
        |live: &Live, chunks: &[usize]| chunks.iter().filter(|&&c| live.state.standing(c)).count();
    println!(
        "MEASURED: roof plane {} of {} chunks rests on components {:?}; its plate {} rests on {:?}",
        kind.asset.components[usize::from(roof_component)],
        roof.len(),
        under_roof
            .iter()
            .map(|&c| &kind.asset.components[usize::from(c)])
            .collect::<Vec<_>>(),
        kind.asset.components[usize::from(plate)],
        wall.iter()
            .map(|&c| &kind.asset.components[usize::from(c)])
            .collect::<Vec<_>>()
    );

    // Hit lightly: a round's line through the plane's chunk nearest its centre, a tenth of its
    // pool, then a blast that spends one chunk's pool.
    let centre = layout.element_blocks[roof_element].center();
    let middle = *roof
        .iter()
        .min_by(|&&a, &&b| {
            base.structure.chunks[a]
                .centroid
                .distance(centre)
                .total_cmp(&base.structure.chunks[b].centroid.distance(centre))
        })
        .unwrap();
    let mut live = fresh();
    let line = line_through_chunk(&kind, &live.structure, roof_element, middle, 0.1);
    let loading = Loading::lines(&[line]);
    let changes = run_laws(layout, &mut live, &loading, Vec::new(), Vec::new());
    assert!(changes.groups.is_empty() && changes.failures.is_empty());
    assert_eq!(
        standing(&live, &roof),
        roof.len(),
        "a light round moves nothing"
    );
    let (blast, pushes) = blast_on(&live.structure, middle, 1.0);
    let changes = run_laws(layout, &mut live, &blast, pushes, Vec::new());
    assert!(!live.state.standing(middle), "the blast spent the chunk");
    assert!(changes.groups.is_empty(), "{:?}", changes.failures);
    assert_eq!(
        standing(&live, &roof),
        roof.len() - 1,
        "carved, the plane stands"
    );

    // Shoot out the long wall under the plate.
    let mut live = fresh();
    let wall_chunks: Vec<usize> = wall
        .iter()
        .flat_map(|&c| component_chunks(&kind, c))
        .filter(|&c| !live.structure.chunks[c].buried)
        .collect();
    let dust = |live: &Live, chunks: &[usize]| Loading {
        sources: Vec::new(),
        energies: chunks
            .iter()
            .filter(|&&c| live.state.standing(c))
            .map(|&c| (c, fracture::full_pool(&live.structure.chunks[c])))
            .collect(),
    };
    let loading = dust(&live, &wall_chunks);
    let changes = run_laws(layout, &mut live, &loading, Vec::new(), Vec::new());
    let rests = |changes: &Changes| {
        changes
            .failures
            .iter()
            .filter(|f| f.load == fracture::Load::Rest)
            .count()
    };
    println!(
        "MEASURED: the long wall shot away ({} chunks to dust): {} rests opened, groups {:?}, plate \
         standing {} of {}, roof plane standing {} of {}, weight steps {}",
        loading.energies.len(),
        rests(&changes),
        changes
            .groups
            .iter()
            .map(|(g, _)| g.len())
            .collect::<Vec<_>>(),
        standing(&live, &plate_chunks),
        plate_chunks.len(),
        standing(&live, &roof),
        roof.len(),
        changes.weight_steps
    );
    assert_eq!(standing(&live, &plate_chunks), 0, "the plate fell");
    let others: Vec<u16> = under_roof.iter().copied().filter(|&c| c != plate).collect();
    let others_hold = !others.is_empty();
    assert_eq!(
        standing(&live, &roof),
        if others_hold { roof.len() } else { 0 },
        "the plane stands only on what else it rests on"
    );

    // Then what else it rests on.
    let rest_chunks: Vec<usize> = others
        .iter()
        .flat_map(|&c| component_chunks(&kind, c))
        .filter(|&c| !live.structure.chunks[c].buried)
        .collect();
    let loading = dust(&live, &rest_chunks);
    let changes = run_laws(layout, &mut live, &loading, Vec::new(), Vec::new());
    println!(
        "MEASURED: its other rests shot away ({} chunks to dust): {} rests opened, groups {:?}, \
         roof plane standing {} of {}",
        loading.energies.len(),
        rests(&changes),
        changes
            .groups
            .iter()
            .map(|(g, _)| g.len())
            .collect::<Vec<_>>(),
        standing(&live, &roof),
        roof.len()
    );
    assert_eq!(
        standing(&live, &roof),
        0,
        "with its rests gone the plane fell"
    );
    assert!(
        changes
            .groups
            .iter()
            .any(|(g, _)| roof.iter().all(|c| g.contains(c))),
        "it fell whole, one body"
    );
}

/// A ROUND'S CHANNEL IS THE CYLINDER IT CROSSED, OPEN ACROSS EACH FACE: a line through a house
/// chunk's centroid at 30° to its element's axis (fixture data) leaves a hole whose capsule holds,
/// on each face the line crosses, every point of the cylinder's ellipse that lies on that face, to
/// the record's millimetre, where the capsule about the bare entry and exit points leaves part of
/// that rim closed. A line that stops inside the chunk keeps its end there, so its far face stays
/// closed. A channel stored from entry to exit, whose rounded ends close a crescent at each
/// oblique rim, fails here; the opening the view draws through a perforated wall rests on it.
#[test]
fn a_channel_is_the_cylinder_it_crossed_open_across_each_face() {
    use std::f32::consts::{PI, TAU};
    let kind = shipped_house();
    let (e, [c, ..]) = fixture_chunks(&kind);
    let block = &kind.layout.blocks[c];
    let structure = kind.layout.structure(&kind.asset, 1.0);
    let centroid = Vec3::from_array(kind.asset.chunks[kind.layout.chunks[c] as usize].centroid);
    let w = kind.asset.elements[e].frame.w().normalize();
    // Tilted along the chunk's widest direction across the axis, so both ends sit inside their
    // faces with room for the rim.
    let extent = |t: Vec3| {
        let along = || block.points.iter().map(move |p| p.dot(t));
        along().fold(f32::MIN, f32::max) - along().fold(f32::MAX, f32::min)
    };
    let (a, b) = w.any_orthonormal_pair();
    let across = (0..16)
        .map(|k| {
            let phi = k as f32 / 16.0 * PI;
            a * phi.cos() + b * phi.sin()
        })
        .max_by(|x, y| extent(*x).total_cmp(&extent(*y)))
        .expect("sixteen directions");
    let (sin, cos) = 30.0_f32.to_radians().sin_cos();
    let dir = (w * cos + across * sin).normalize();
    let (t0, t1) = block
        .span(centroid - dir * 10.0, dir)
        .expect("the line crosses the chunk");
    let (entry, exit) = (centroid + dir * (t0 - 10.0), centroid + dir * (t1 - 10.0));
    // A 2 cm channel (fixture data): its energy crushes that cylinder about the chord.
    let r = 0.02;
    let energy = PI * r * r * entry.distance(exit) * structure.chunks[c].material.crush;
    let metres = |p: [i16; 3]| centroid + Vec3::from_array(p.map(|v| f32::from(v) / 1000.0));
    let to_segment = |x: Vec3, a: Vec3, b: Vec3| {
        let ab = b - a;
        let s = ((x - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
        x.distance(a + ab * s)
    };
    let [(_, channel)] = holes_of(&kind, 1.0, &[(c, entry, exit, energy)], &Loading::default())[..]
    else {
        panic!("one channel");
    };
    let radius = f32::from(channel.radius) / 1000.0;
    let (from, to) = (metres(channel.from), metres(channel.to));
    let inside = |x: Vec3| block.planes.iter().all(|&(n, o)| n.dot(x) <= o + 1e-4);
    let (e1, e2) = dir.any_orthonormal_pair();
    let mut closed_by_caps = 0;
    for end in [entry, exit] {
        let &(n, o) = block
            .planes
            .iter()
            .max_by(|a, b| (a.0.dot(end) - a.1).total_cmp(&(b.0.dot(end) - b.1)))
            .expect("a hull has planes");
        let mut rim = 0;
        for k in 0..64 {
            let phi = k as f32 / 64.0 * TAU;
            // Where the cylinder's line at angle `phi` about the axis meets the face's plane.
            let base = end + (e1 * phi.cos() + e2 * phi.sin()) * radius;
            let x = base + dir * ((o - n.dot(base)) / n.dot(dir));
            if !inside(x) {
                continue;
            }
            rim += 1;
            assert!(
                to_segment(x, from, to) <= radius + 0.002,
                "the rim at {x} on the face through {end} is open"
            );
            closed_by_caps += usize::from(to_segment(x, entry, exit) > radius + 0.002);
        }
        assert!(rim >= 8, "the face through {end} holds its rim");
    }
    assert!(
        closed_by_caps > 0,
        "the bare entry-to-exit capsule closes part of the rim"
    );

    let rest = (entry + exit) / 2.0;
    let [(_, blind)] = holes_of(&kind, 1.0, &[(c, entry, rest, energy)], &Loading::default())[..]
    else {
        panic!("one blind channel");
    };
    assert!(
        metres(blind.to).distance(rest) < 0.002,
        "a channel ends where its round came to rest"
    );
    assert!(
        metres(blind.from).distance(entry) > 0.002,
        "and opens past the face it entered"
    );
}

/// A fixture burst's blast energy, J: the 88 mm HE burst's (fixture data).
const FIXTURE_BLAST_J: f32 = 1.47e6;

/// A dynamic cube of brick density on `layer` that collides with nothing, so only a blast moves
/// it: side `side` m at `at` (fixture data).
fn moving_cube(app: &mut App, at: Vec3, side: f32, layer: Layer) -> Entity {
    app.world_mut()
        .spawn((
            RigidBody::Dynamic,
            Position(at),
            Rotation::default(),
            Collider::cuboid(side, side, side),
            ColliderDensity(1845.0),
            CollisionLayers::new([layer], LayerMask::NONE),
        ))
        .id()
}

/// A loose piece: a debris body that is no wall piece.
fn loose_cube(app: &mut App, at: Vec3, side: f32) -> Entity {
    let cube = moving_cube(app, at, side, Layer::Debris);
    app.world_mut()
        .entity_mut(cube)
        .insert(crate::debris::DebrisPiece);
    cube
}

/// Every blast direction whose ray from `origin` meets a collider of `body`, through avian's own
/// cast: the direction, its share of the field, the distance and the normal there.
fn rays_on(app: &mut App, body: Entity, origin: Vec3) -> Vec<(Vec3, f32, f32, Vec3)> {
    use bevy::ecs::system::RunSystemOnce;
    app.world_mut()
        .run_system_once(move |spatial: SpatialQuery, of: Query<&ColliderOf>| {
            fracture::blast_directions()
                .iter()
                .filter_map(|&(dir, share)| {
                    let hit = spatial.cast_ray_predicate(
                        origin,
                        Dir3::new(dir).ok()?,
                        1000.0,
                        true,
                        &SpatialQueryFilter::default(),
                        &|collider| of.get(collider).is_ok_and(|of| of.body == body),
                    )?;
                    Some((dir, share, hit.distance, hit.normal))
                })
                .collect::<Vec<_>>()
        })
        .expect("the cast runs")
}

/// Set off the fixture charge at `at`, and run the tick that applies it and the one after.
fn charge(app: &mut App, at: Vec3) {
    app.world_mut().trigger(WallCharge {
        at,
        blast: Blast::of(FIXTURE_BLAST_J),
    });
    app.update();
    app.update();
}

/// A BLAST RAY THAT STOPS ON A MOVING BODY PUSHES IT BY ITS SHARE AND DEPOSITS NOTHING: a loose
/// piece, or a dynamic body that is no piece (a launched turret's stand-in), between a fixture burst
/// and a standing chunk of the house leaves with the momentum of the rays that meet it, each its
/// share's reflected impulse at the distance and on the face avian's cast gives ([`Blast::push`]),
/// and the chunk it shades keeps its full pool, though the same burst wears it with nothing there.
/// A body's armour volume is no surface: the rays pass it and wear the chunk. A ray that pushed a
/// body and went on into the wall, a push of the wrong share, distance or face, a moving body the
/// rays pass through, or one they meet by kind rather than by being a moving body, fails here;
/// every rubble pile and hull a blast meets rests on it.
#[test]
fn a_ray_that_stops_on_a_moving_body_pushes_it_by_its_share_and_deposits_nothing() {
    let kind = shipped_house();
    let (e, [c, ..]) = fixture_chunks(&kind);
    let (near, w, _) = through(&kind, e, c, 0.0);
    // The burst a metre before the chunk's near face, the body half-way (fixture data).
    let origin = near - *w * 0.5;
    let blast = Blast::of(FIXTURE_BLAST_J);
    let after = |spawn: Option<&dyn Fn(&mut App) -> Entity>| {
        let (mut app, root) = composed(false);
        let body = spawn.map(|spawn| spawn(&mut app));
        app.update();
        app.update();
        // The rays that meet the body before any of the house's blockers (the house stands at the
        // origin at unit scale, so its frame is the world).
        let intact = Shape::intact(&kind.layout);
        let rays: Vec<_> = body.map_or_else(Vec::new, |body| {
            rays_on(&mut app, body, origin)
                .into_iter()
                .filter(|&(dir, _, d, _)| {
                    first_chunk(&kind.layout, &intact, origin, dir).is_none_or(|(_, t)| t > d)
                })
                .collect()
        });
        let expected: Vec3 = rays
            .iter()
            .map(|&(dir, share, d, normal)| blast.push(share, d, dir, normal))
            .sum();
        charge(&mut app, origin);
        let momentum = body.map_or(Vec3::ZERO, |body| {
            let world = app.world();
            world.get::<ComputedMass>(body).unwrap().value()
                * world.get::<LinearVelocity>(body).unwrap().0
        });
        let breakage = app.world().get::<Breakage>(root).unwrap();
        let live = breakage.live.as_ref().expect("the burst reached the house");
        let wears = live.state.pool[c] < fracture::full_pool(&live.structure.chunks[c]);
        (rays.len(), momentum, expected, wears)
    };
    let (.., bare) = after(None);
    assert!(bare, "with nothing there the burst wears the chunk");
    let piece = |app: &mut App| loose_cube(app, near, 0.4);
    let other = |app: &mut App| moving_cube(app, near, 0.4, Layer::Vehicle);
    for spawn in [&piece as &dyn Fn(&mut App) -> Entity, &other] {
        let (rays, momentum, expected, wears) = after(Some(spawn));
        assert!(rays > 100, "the body is in the field");
        assert!(
            (momentum - expected).length() <= 1e-3 * expected.length(),
            "the body leaves with {momentum}, its rays pushed {expected}"
        );
        assert!(!wears, "the shaded chunk takes nothing");
    }
    let armour = |app: &mut App| moving_cube(app, near, 0.4, Layer::Armor);
    let (rays, momentum, _, wears) = after(Some(&armour));
    assert!(rays > 100, "the armour volume is in the field");
    assert_eq!(momentum, Vec3::ZERO, "an armour volume takes no push");
    assert!(wears, "and the rays pass it to the chunk");
}

/// A LOOSE PIECE SHADOWED BY A STANDING CHUNK TAKES NOTHING: a fixture burst half a metre before a
/// house wall pushes a loose cube on its own side and leaves the same cube behind the wall, where
/// every ray toward it meets a standing chunk first, at rest. Pieces tested ahead of the chunks, or
/// a ray that passes a standing chunk to the piece behind it, fail here.
#[test]
fn a_loose_piece_shadowed_by_a_standing_chunk_takes_nothing() {
    let kind = shipped_house();
    let (e, [c, ..]) = fixture_chunks(&kind);
    let element = &kind.asset.elements[e];
    let (origin, w, _) = through(&kind, e, c, 0.0);
    // Each cube 0.3 m past the wall's far face, or as far out on the burst's side (fixture data).
    let out = 0.5 + element.depth[1] - element.depth[0] + 0.3;
    let (mut app, _) = composed(false);
    let behind = loose_cube(&mut app, origin + *w * out, 0.1);
    let open = loose_cube(&mut app, origin - *w * out, 0.1);
    app.update();
    app.update();
    assert!(
        !rays_on(&mut app, behind, origin).is_empty(),
        "the cube behind the wall is in the field"
    );
    charge(&mut app, origin);
    let speed = |body: Entity| app.world().get::<LinearVelocity>(body).unwrap().length();
    assert!(speed(open) > 0.0, "the burst pushes the cube on its side");
    assert_eq!(speed(behind), 0.0, "the wall shades the cube behind it");
}

/// A SLEEPING LOOSE PIECE WAKES TO A BLAST: a wall piece asleep at rest on a slab is woken by a
/// fixture burst beside it, and leaves away from the burst. An impulse that leaves a sleeping body
/// asleep, or is held back from one, fails here; rubble settled for most of a match relies on it.
#[test]
fn a_sleeping_loose_piece_wakes_to_a_blast() {
    let (mut app, _) = composed(false);
    app.insert_resource(Gravity(Vec3::NEG_Y * 9.81));
    let kind = shipped_house();
    let (_, [c, ..]) = fixture_chunks(&kind);
    let live = Live::new(&kind, 1.0);
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
            (3, 1.0, Transform::from_xyz(30.0, 0.0, 0.0)),
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
    (0..1280)
        .find(|_| {
            app.update();
            app.world().get::<Sleeping>(piece).is_some()
        })
        .expect("the piece came to rest asleep within 20 s");
    let at = app.world().get::<Position>(piece).unwrap().0;
    charge(&mut app, at + Vec3::X * 0.8);
    assert!(
        app.world().get::<Sleeping>(piece).is_none(),
        "the burst wakes the piece"
    );
    let velocity = app.world().get::<LinearVelocity>(piece).unwrap().0;
    assert!(
        velocity.x < 0.0,
        "it leaves away from the burst: {velocity}"
    );
}

/// A CAPTURE'S DUST SPENDS THE CHUNKS IT NAMES THAT STAND ABOVE GRADE, AND NOTHING ELSE: every chunk
/// of a breakable element with chunks on both sides of grade (fixture data; chosen by class) named
/// to [`WallDust`] is recorded as dust once the deposit runs, except the buried ones the soil
/// holds, and no hole is cut. The capture's wall cue shoots out whole walls this way, as the roof
/// test does in the laws; a dust that reached a buried chunk, missed a named one or cut holes
/// would frame a different break than the one the laws were tested on.
#[test]
fn a_capture_dust_spends_the_named_chunks_above_grade_and_nothing_else() {
    let kind = shipped_house();
    let layout = &kind.layout;
    let structure = layout.structure(&kind.asset, 1.0);
    let e = (0..layout.elements.len())
        .find(|&e| {
            let buried = |c: usize| structure.chunks[c].buried;
            layout.chunks_of(e).any(buried) && !layout.chunks_of(e).all(buried)
        })
        .expect("the house has an element on both sides of grade");
    let named: Vec<usize> = layout.chunks_of(e).collect();
    let (mut app, root) = composed(false);
    app.world_mut().trigger(WallDust {
        building: root,
        chunks: named.clone(),
    });
    app.update();
    app.update();
    let record = &app.world().resource::<WallRecord>().entries;
    let mut dusted: Vec<usize> = record
        .iter()
        .filter_map(|entry| match entry.event {
            WallEvent::Dust(c) => Some(usize::from(c)),
            _ => None,
        })
        .collect();
    dusted.sort_unstable();
    let above: Vec<usize> = named
        .into_iter()
        .filter(|&c| !structure.chunks[c].buried)
        .collect();
    assert_eq!(dusted, above);
    assert!(
        !record
            .iter()
            .any(|entry| matches!(entry.event, WallEvent::Hole(..)))
    );
}
