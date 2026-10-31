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

/// The breakage sim on the offline physics with the solver's contact reading, the march probe,
/// and the shipped house spawned at the origin (unit scale), one fixed tick per update. `replica`
/// adds the replica marker.
fn composed(replica: bool) -> (App, Entity) {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::transform::TransformPlugin,
        AssetPlugin::default(),
        crate::offline_physics(),
        sim_plugin,
        crate::debris::sim_plugin,
        crate::contact_transfer::sim_plugin,
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
        &Load::field(&hits),
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
            (3, lifted),
            &[c],
            launch(&kind, &live.structure, 1.0, &[c], &[]),
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
            (3, at),
            &[c],
            launch(&kind, &live.structure, 1.0, &[c], &[]),
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

/// Rounds' lines as the laws read them with no blow ([`Load::line`]).
fn line_loads(lines: &[(usize, Vec3, Vec3, f32)]) -> Vec<Load> {
    lines
        .iter()
        .flat_map(|&(c, entry, end, energy)| Load::line(c, entry, end, energy, Vec3::ZERO))
        .collect()
}

/// A blast's single ray on law chunk `c` at its centroid, carrying `share` of its full pool and
/// no impulse, as the laws read it.
fn blast_on(structure: &Structure, c: usize, share: f32) -> Vec<Load> {
    let chunk = &structure.chunks[c];
    Load::blast(&[fracture::RayHit {
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
        &line_loads(&lines),
        holes_of(&kind, 1.0, &lines, &[]),
    );
    let blast = blast_on(&authority.structure, dish_chunk, 0.05);
    let holes = holes_of(&kind, 1.0, &[], &blast);
    let second = run_laws(&kind.layout, &mut authority, &blast, holes);
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
    let push = Load {
        chunk: c,
        at: cut.structure.chunks[c].centroid,
        energy: 0.0,
        blow: (line.2 - line.1).normalize() * 100.0,
        press: Vec3::ZERO,
        origin: Origin::Field,
    };
    let loads: Vec<Load> = line_loads(&[line]).into_iter().chain([push]).collect();
    let holes = holes_of(&kind, 1.0, &[line], &[]);
    let mut record = Vec::new();
    for tick in [7, 9] {
        let with = run_laws(&kind.layout, &mut cut, &loads, holes.clone());
        let without = run_laws(&kind.layout, &mut whole, &loads, Vec::new());
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
    let holes = holes_of(&kind, 1.0, &[line], &[]);
    assert_eq!(holes.len(), 1);
    for k in 1..=HOLES_PER_CHUNK + 1 {
        let changes = run_laws(&kind.layout, &mut live, &line_loads(&[line]), holes.clone());
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
    let [(_, channel)] = holes_of(&kind, 1.0, &[line], &[])[..] else {
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

    let blast = blast_on(&structure, c, 0.01);
    let [(_, dish)] = holes_of(&kind, 1.0, &[], &blast)[..] else {
        panic!("one dish");
    };
    let r = f32::from(dish.radius) / 1000.0;
    Volume {
        held: 2.0 / 3.0 * PI * r.powi(3),
        crushed: blast[0].energy / crush,
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

/// The components whose chunks the chunks of `part` join through a joint `holds` keeps, and, with
/// `under`, only those it bears on (the joint's normal toward `part` rising), ascending.
fn joined_to(
    kind: &BuildingKind,
    structure: &Structure,
    part: &[usize],
    under: bool,
    holds: impl Fn(&Joint) -> bool,
) -> Vec<u16> {
    let component =
        |c: usize| kind.asset.elements[usize::from(kind.layout.element_of[c])].component;
    let mut joined: Vec<u16> = structure
        .joints
        .iter()
        .filter(|joint| holds(joint))
        .filter_map(|joint| {
            let Side::Chunk(b) = joint.b else {
                return None;
            };
            let (held, other) = match (part.contains(&joint.a), part.contains(&b)) {
                (true, false) => (joint.a, b),
                (false, true) => (b, joint.a),
                _ => return None,
            };
            let toward = structure.chunks[held].centroid - joint.centroid;
            let bears = joint.normal.dot(toward).signum() * joint.normal.y > 0.0;
            (bears || !under).then(|| component(other))
        })
        .collect();
    joined.sort_unstable();
    joined.dedup();
    joined
}

/// THE ROOF IS NAILED TO ITS PLATES: STRUCK IT STAYS, AND IT BREAKS ITS NAILS AND FALLS ONCE ITS
/// LONG WALL AND THE GABLES GO. On the shipped house (fixture data; the parts chosen by class: the
/// roof planes, the plate a plane is fixed to, the masonry that plate bears on, and the gables of
/// the walls the planes rest on), a round through a plane's middle chunk wears it and moves
/// nothing, and a blast that spends that chunk carves the plane, which stands. Shooting out the
/// long wall under the plate (every standing unburied chunk of it turned to dust) leaves the whole
/// roof on the house: the plate hangs between the gables, the planes still rest on them. Shooting
/// out the gables too leaves the roof hanging from the other plate by nails, which its weight
/// breaks (weight failures, across fixed joints alone), and every roof chunk falls. A roof laid
/// unfixed on its plates (a rest at a 3 : 4 pitch, steeper than the friction of wood on wood)
/// slides off at the first touch and fails here; so does a law that let a rest carry tension.
#[test]
fn a_nailed_roof_stays_when_struck_and_breaks_its_nails_when_its_walls_go() {
    let kind = shipped_house();
    let layout = &kind.layout;
    let elements = &kind.asset.elements;
    let of_kind = |kind_of: ElementKind| {
        (0..elements.len())
            .filter(|&e| elements[e].kind == kind_of)
            .collect::<Vec<_>>()
    };
    let fresh = || Live::new(&kind, 1.0);
    let base = fresh();
    let fixed = |joint: &Joint| matches!(joint.bond, Bond::Bonded(_));
    let resting = |joint: &Joint| joint.bond == Bond::Resting;
    let mut roof: Vec<usize> = of_kind(ElementKind::Roof)
        .into_iter()
        .flat_map(|e| layout.chunks_of(e))
        .collect();
    roof.sort_unstable();
    let roof_element = of_kind(ElementKind::Roof)[0];
    let plane = component_chunks(&kind, elements[roof_element].component);
    let plate = of_kind(ElementKind::WallPlate)
        .into_iter()
        .map(|e| elements[e].component)
        .find(|c| joined_to(&kind, &base.structure, &plane, false, fixed).contains(c))
        .expect("the plane is fixed to a wall plate");
    let plate_chunks = component_chunks(&kind, plate);
    let wall = joined_to(&kind, &base.structure, &plate_chunks, true, fixed);
    let rests = joined_to(&kind, &base.structure, &roof, true, resting);
    let gables: Vec<usize> = of_kind(ElementKind::Gable)
        .into_iter()
        .filter(|&e| rests.contains(&elements[e].component))
        .flat_map(|e| layout.chunks_of(e))
        .collect();
    let name = |c: &u16| &kind.asset.components[usize::from(*c)];
    println!(
        "MEASURED: plane {} of {} chunks is fixed to plate {}, which bears on {:?}; the roof's {} \
         chunks rest on {:?}, whose gables hold {} chunks",
        name(&elements[roof_element].component),
        plane.len(),
        name(&plate),
        wall.iter().map(name).collect::<Vec<_>>(),
        roof.len(),
        rests.iter().map(name).collect::<Vec<_>>(),
        gables.len()
    );
    assert!(!wall.is_empty() && !gables.is_empty());
    let standing =
        |live: &Live, chunks: &[usize]| chunks.iter().filter(|&&c| live.state.standing(c)).count();

    // Struck: a round's line through the plane's chunk nearest its centre, a tenth of its pool
    // pushing with the momentum a round at 773 m/s loses giving that energy up (fixture data,
    // `ΔE / v`), then a blast that spends the chunk.
    let centre = layout.element_blocks[roof_element].center();
    let middle = *plane
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
    let push = Load {
        chunk: middle,
        at: line.1,
        energy: 0.0,
        blow: (line.2 - line.1).normalize() * (line.3 / 773.0),
        press: Vec3::ZERO,
        origin: Origin::Field,
    };
    let loads: Vec<Load> = line_loads(&[line]).into_iter().chain([push]).collect();
    let changes = run_laws(layout, &mut live, &loads, Vec::new());
    println!(
        "MEASURED: a round through the plane's middle ({:.0} J, {:.0} N s): {} failures, {} groups",
        line.3,
        push.blow.length(),
        changes.failures.len(),
        changes.groups.len()
    );
    assert!(changes.groups.is_empty() && changes.failures.is_empty());
    assert_eq!(standing(&live, &roof), roof.len(), "a round moves nothing");
    let blast = blast_on(&live.structure, middle, 1.0);
    let changes = run_laws(layout, &mut live, &blast, Vec::new());
    assert!(!live.state.standing(middle), "the blast spent the chunk");
    assert!(changes.groups.is_empty(), "{:?}", changes.failures);
    assert_eq!(
        standing(&live, &roof),
        roof.len() - 1,
        "carved, the roof stands"
    );

    // Shoot out the long wall under the plate, then the gables: every standing unburied chunk's
    // pool spent by the removal alone (a load's energy would also stress the joints about it), then
    // the laws that follow a removal with no blow, the weight check on the dust and support.
    let mut live = fresh();
    let shoot_out = |live: &mut Live, chunks: &[usize]| {
        let mut dusted = Vec::new();
        for &c in chunks {
            let chunk = &live.structure.chunks[c];
            if live.state.standing(c)
                && !chunk.buried
                && fracture::wear(
                    &live.structure,
                    &mut live.state,
                    c,
                    fracture::full_pool(chunk),
                )
            {
                dusted.push(c);
            }
        }
        dusted.sort_unstable();
        let weighed = fracture::weigh(
            &live.structure,
            &mut live.state,
            &dusted,
            &[],
            &mut live.sections,
        );
        let groups = fracture::fall(&live.structure, &mut live.state);
        (dusted.len(), weighed, groups)
    };
    let wall_chunks: Vec<usize> = wall
        .iter()
        .flat_map(|&c| component_chunks(&kind, c))
        .collect();
    let (dusted, weighed, groups) = shoot_out(&mut live, &wall_chunks);
    println!(
        "MEASURED: the long wall shot away ({} chunks to dust): {} failures, groups {:?}, plate \
         standing {} of {}, roof standing {} of {}, weight steps {}",
        dusted,
        weighed.failures.len(),
        groups.iter().map(Vec::len).collect::<Vec<_>>(),
        standing(&live, &plate_chunks),
        plate_chunks.len(),
        standing(&live, &roof),
        roof.len(),
        weighed.steps
    );
    assert!(weighed.failures.is_empty(), "{:?}", weighed.failures);
    assert_eq!(standing(&live, &plate_chunks), plate_chunks.len());
    assert_eq!(standing(&live, &roof), roof.len(), "the roof stands");

    let (dusted, weighed, groups) = shoot_out(&mut live, &gables);
    let joints = &live.structure.joints;
    println!(
        "MEASURED: the gables shot away too ({} chunks to dust): failures {:?}, groups {:?}, roof \
         standing {} of {}",
        dusted,
        weighed
            .failures
            .iter()
            .map(|f| (f.cause, f.joints.len(), f.demand, f.capacity))
            .collect::<Vec<_>>(),
        groups.iter().map(Vec::len).collect::<Vec<_>>(),
        standing(&live, &roof),
        roof.len()
    );
    assert!(!weighed.failures.is_empty());
    for failure in &weighed.failures {
        assert_eq!(failure.cause, fracture::Cause::Weight, "{failure:?}");
        assert!(failure.joints.iter().all(|&j| fixed(&joints[j])));
    }
    assert_eq!(
        standing(&live, &roof),
        0,
        "the roof broke its nails and fell"
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
    let [(_, channel)] = holes_of(&kind, 1.0, &[(c, entry, exit, energy)], &[])[..] else {
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
    let [(_, blind)] = holes_of(&kind, 1.0, &[(c, entry, rest, energy)], &[])[..] else {
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

/// THE GROUND STOPS A BLAST'S RAYS, AND A BODY HOLDING THE BURST SHADES NOTHING: a loose cube on a
/// fixture burst's side of a house wall, pushed with nothing between, takes nothing behind a static
/// slab on the ground's layer (the layer of the ground and of a standing tree); and a burst inside
/// a dynamic body's proxy still wears the chunk it faces. Rays that pass the ground (a house below
/// the horizon taking a push), or a field blanked at 0 m by the body it bursts in, fail here.
#[test]
fn the_ground_stops_a_blast_and_a_body_holding_the_burst_shades_nothing() {
    let kind = shipped_house();
    let (e, [c, ..]) = fixture_chunks(&kind);
    let element = &kind.asset.elements[e];
    let (origin, w, _) = through(&kind, e, c, 0.0);
    let out = 0.5 + element.depth[1] - element.depth[0] + 0.3;
    let speed_of_open = |slab: bool| {
        let (mut app, _) = composed(false);
        let open = loose_cube(&mut app, origin - *w * out, 0.1);
        if slab {
            // A 5 cm slab 4 m square across `w`, half-way to the cube (fixture geometry).
            app.world_mut().spawn((
                RigidBody::Static,
                Position(origin - *w * (out / 2.0)),
                Rotation(Quat::from_rotation_arc(Vec3::X, *w)),
                Collider::cuboid(0.05, 4.0, 4.0),
                CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
            ));
        }
        app.update();
        app.update();
        charge(&mut app, origin);
        app.world().get::<LinearVelocity>(open).unwrap().length()
    };
    assert!(
        speed_of_open(false) > 0.0,
        "with nothing between, the cube is pushed"
    );
    assert_eq!(speed_of_open(true), 0.0, "the ground's layer shades it");

    let (mut app, root) = composed(false);
    moving_cube(&mut app, origin, 0.4, Layer::Vehicle);
    app.update();
    app.update();
    charge(&mut app, origin);
    let breakage = app.world().get::<Breakage>(root).unwrap();
    let live = breakage.live.as_ref().expect("the burst reached the house");
    assert!(
        live.state.pool[c] < fracture::full_pool(&live.structure.chunks[c]),
        "the body holding the burst shades nothing"
    );
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
            (3, Transform::from_xyz(30.0, 0.0, 0.0)),
            &[c],
            launch(&kind, &live.structure, 1.0, &[c], &[]),
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

/// Per update, the contact loads [`read_contacts`] queued in it, each with its building's root.
#[derive(Resource, Default)]
struct QueuedContacts(Vec<Vec<(Entity, Load)>>);

fn keep_queued_contacts(queue: Res<WallQueue>, mut kept: ResMut<QueuedContacts>) {
    kept.0.push(
        queue
            .0
            .iter()
            .flat_map(|deposit| deposit.contacts.iter().copied())
            .collect(),
    );
}

/// [`composed`] (the authority's) with each tick's queued contact loads kept between the reader
/// and the laws ([`QueuedContacts`]), its clocks started.
fn composed_keeping_contacts() -> (App, Entity) {
    let (mut app, root) = composed(false);
    app.init_resource::<QueuedContacts>().add_systems(
        FixedPostUpdate,
        keep_queued_contacts
            .after(read_contacts)
            .before(apply_deposits),
    );
    for _ in 0..3 {
        app.update();
    }
    app.world_mut().resource_mut::<QueuedContacts>().0.clear();
    (app, root)
}

/// The side of the fixture's cubes, m, and their mass at brick density, kg (fixture data).
const CUBE_SIDE: f32 = 0.4;
const CUBE_KG: f32 = 1845.0 * CUBE_SIDE * CUBE_SIDE * CUBE_SIDE;

/// A dynamic cube of [`CUBE_SIDE`] at brick density, colliding as a loose piece does, centred at
/// `at` and moving at `velocity` (fixture data).
fn contact_cube(app: &mut App, at: Vec3, velocity: Vec3) -> Entity {
    app.world_mut()
        .spawn((
            RigidBody::Dynamic,
            Position(at),
            Rotation::default(),
            Collider::cuboid(CUBE_SIDE, CUBE_SIDE, CUBE_SIDE),
            ColliderDensity(1845.0),
            piece_layers(),
            LinearVelocity(velocity),
        ))
        .id()
}

/// A flat patch of the shipped house's outer wall met from outside along +x: a breakable
/// element, a point on its face and the unit direction into it, where the face stands square to
/// the approach and the rays through the centre and the four corners of a [`CUBE_SIDE`] square
/// about the point all meet that element first at the same depth (the fixture's choice by class,
/// searched from the middle of the bounds outward).
fn wall_face(kind: &BuildingKind) -> (usize, Vec3, Vec3) {
    let layout = &kind.layout;
    let (lo, hi) = kind.bounds();
    let dir = Vec3::X;
    let half = CUBE_SIDE / 2.0;
    let fractions = [0.5, 0.4, 0.6, 0.3, 0.7, 0.2, 0.8];
    for fy in fractions {
        for fz in fractions {
            let origin = Vec3::new(lo.x - 1.0, lo.y.lerp(hi.y, fy), lo.z.lerp(hi.z, fz));
            let first = |offset: Vec3| {
                fracture::first_hit(&layout.element_blocks, |_| true, origin + offset, dir)
            };
            let Some((e, t, normal)) = first(Vec3::ZERO) else {
                continue;
            };
            let flat = [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)]
                .into_iter()
                .all(|(a, b)| {
                    first(Vec3::new(0.0, a * half, b * half))
                        .is_some_and(|(k, s, _)| k == e && (s - t).abs() < 1e-3)
                });
            if layout.elements[e].is_some() && normal.dot(dir) < -0.9999 && flat {
                return (e, origin + dir * t, dir);
            }
        }
    }
    panic!("the shipped house shows a flat patch of breakable wall to +x");
}

/// NO BODY'S KIND ENTERS A CONTACT'S LOADS (the game's environment destruction model): a plain
/// dynamic body (a hull's or a launched turret's stand-in) and a wall piece, alike in shape, mass
/// and motion, striking the shipped house's wall at the same point at 3 m/s, put the same loads on
/// the house, step by step, each naming its own body; the strike's blow is the cube's closing
/// momentum into the wall within 1 % (DERIVED: a cube's corner points' effective masses sum to its
/// own) and each load lies on a chunk of the struck element, on the chunk's hull within the step's
/// approach (fixture data, 5 cm), and spends no energy. The collision model rests on it (#232: no
/// marker for which bodies may break things): a reader that reads a marker (a piece's, a tank's)
/// to keep, drop or scale a contact fails here, as does a contact loaded off its chunk, and a
/// blow capped point by point (88 % of the closing momentum here, MEASURED: the solver's own split
/// of the contact among the corners, [`ContactTransfers::split`]).
#[test]
fn no_bodys_kind_enters_a_contacts_loads() {
    let kind = shipped_house();
    let (e, face, w) = wall_face(&kind);
    let start = face - w * (CUBE_SIDE / 2.0 + 0.3);
    let strike = |wall_piece: bool| {
        let (mut app, root) = composed_keeping_contacts();
        let cube = contact_cube(&mut app, start, w * 3.0);
        if wall_piece {
            app.world_mut().entity_mut(cube).insert((
                WallPiece {
                    building: 3,
                    chunks: vec![0],
                },
                crate::debris::DebrisPiece,
            ));
        }
        for _ in 0..30 {
            app.update();
        }
        let ticks = std::mem::take(&mut app.world_mut().resource_mut::<QueuedContacts>().0);
        assert_eq!(ticks.len(), 30, "one physics step and one read an update");
        for (on, load) in ticks.iter().flatten() {
            assert_eq!(*on, root, "the house takes the loads");
            assert!(
                matches!(load.origin, Origin::Body { body, mass, .. } if body == cube && mass > 0.0),
                "each load names its body and effective mass: {load:?}"
            );
        }
        ticks
    };
    let (plain, piece) = (strike(false), strike(true));
    let strip = |ticks: &[Vec<(Entity, Load)>]| -> Vec<Vec<Load>> {
        ticks
            .iter()
            .map(|tick| {
                tick.iter()
                    .map(|&(_, load)| Load {
                        origin: Origin::Field,
                        ..load
                    })
                    .collect()
            })
            .collect()
    };
    assert_eq!(
        strip(&plain),
        strip(&piece),
        "a wall piece's loads are a plain body's"
    );
    let loads: Vec<Load> = plain.into_iter().flatten().map(|(_, load)| load).collect();
    assert!(!loads.is_empty(), "the cube strikes the wall");
    let blow: Vec3 = loads.iter().map(|load| load.blow).sum();
    let closing = CUBE_KG * 3.0;
    assert!(
        (blow - w * closing).length() <= 0.01 * closing,
        "the strike's blow {blow} against its closing momentum {closing} N·s along {w}"
    );
    let layout = &kind.layout;
    for load in &loads {
        assert!(
            layout.chunks_of(e).contains(&load.chunk),
            "{load:?} on the struck element's chunks"
        );
        assert!(
            layout.blocks[load.chunk].outside(load.at) <= 0.05,
            "{load:?} on its chunk's hull"
        );
        assert_eq!(load.energy, 0.0, "a contact crushes nothing");
    }
}

/// A BODY AT REST AGAINST A BUILDING PRESSES IT ONCE AND STARTS NO RUN (the game's environment
/// destruction model): a cube a steady 400 N holds on the shipped house's wall falls asleep
/// there, and from then on its building's resting loads ([`RestingLoads`]) hold its press, 400 N
/// into the wall within 0.5 % with no blow, its points once, the same every tick it sleeps, while
/// the reader queues nothing for it, so the laws run on the house only when something else loads
/// it; once the cube is gone its press goes. The weight check a crack starts relies on a sleeping
/// pile's press (the wall design, "Bodies in contact"), and the server's per-tick cost of rubble
/// at rest on walls on its starting no run. A reading that drops a sleeping pair (a press of
/// nothing), keeps it after its body goes, adds it again each tick, or replays it as a new
/// deposit fails here. Fixture numbers are fixture data.
#[test]
fn a_body_at_rest_against_a_building_presses_it_once_and_starts_no_run() {
    let kind = shipped_house();
    let (_, face, w) = wall_face(&kind);
    let (mut app, root) = composed_keeping_contacts();
    let cube = contact_cube(&mut app, face - w * (CUBE_SIDE / 2.0 + 0.002), Vec3::ZERO);
    app.world_mut()
        .entity_mut(cube)
        .insert(ConstantForce(w * 400.0));
    (0..640)
        .find(|_| {
            app.update();
            app.world().get::<Sleeping>(cube).is_some()
        })
        .expect("the held cube came to rest asleep within 10 s");
    app.world_mut().resource_mut::<QueuedContacts>().0.clear();
    let mut held: Option<Vec<Load>> = None;
    for tick in 0..40 {
        app.update();
        assert!(
            app.world().get::<Sleeping>(cube).is_some(),
            "tick {tick}: the cube sleeps"
        );
        let queued = std::mem::take(&mut app.world_mut().resource_mut::<QueuedContacts>().0);
        assert!(
            queued.iter().all(Vec::is_empty),
            "tick {tick}: a sleeping body's contact queues nothing: {queued:?}"
        );
        let resting = app.world().resource::<RestingLoads>();
        let loads = resting.0.get(&root).cloned().unwrap_or_default();
        assert_eq!(
            resting.0.len(),
            1,
            "tick {tick}: one building holds a press"
        );
        assert!(!loads.is_empty(), "tick {tick}: the house holds the press");
        let press: Vec3 = loads.iter().map(|load| load.press).sum();
        assert!(
            (press - w * 400.0).length() <= 0.005 * 400.0,
            "tick {tick}: the resting press {press} against 400 N along {w}"
        );
        for load in &loads {
            assert_eq!(
                (load.blow, load.energy),
                (Vec3::ZERO, 0.0),
                "tick {tick}: a resting load is its body's press alone"
            );
            assert!(
                matches!(load.origin, Origin::Body { body, mass, .. } if body == cube && mass > 0.0)
            );
        }
        match &held {
            Some(held) => assert_eq!(held, &loads, "tick {tick}: the press stands unchanged"),
            None => held = Some(loads),
        }
    }
    app.world_mut().entity_mut(cube).despawn();
    app.update();
    assert!(
        app.world().resource::<RestingLoads>().0.is_empty(),
        "the press goes with its body"
    );
}

/// The game's damage model delivers each march before its shell despawns. Building deposits
/// rely on that ordering: a missing shell is a bug, never permission to discard delivered lines.
#[test]
#[should_panic(expected = "ShellMarched is observed before its shell despawns")]
fn a_march_without_its_shell_is_a_bug() {
    let mut app = App::new();
    app.init_resource::<WallQueue>().add_observer(read_shell);
    app.world_mut().trigger(ShellMarched {
        shell: Entity::PLACEHOLDER,
        marks_from: 0,
        cones_from: 0,
        lines: Vec::new(),
        burst: None,
    });
}

/// The exact departure mass properties, about the collider frame (fixture data); the tests use
/// their tensor to audit both linear and angular momentum, including an asymmetric group's COM.
fn departure_mass(
    structure: &Structure,
    launch: &Launch,
) -> avian3d::dynamics::rigid_body::mass_properties::bevy_heavy::MassProperties3d {
    use avian3d::dynamics::rigid_body::mass_properties::bevy_heavy::{
        ComputeMassProperties3d, MassProperties3d,
    };
    launch
        .colliders
        .iter()
        .map(|(c, shape)| shape.mass_properties(structure.chunks[*c].material.density))
        .fold(MassProperties3d::default(), |sum, part| sum + part)
}

/// MOMENTUM IS KEPT THROUGH A BREAK (the game's environment destruction model). A source
/// stopped by the rigid solver regains only the blow beyond the free group's plastic share.
/// The source's net loss plus the departure is zero in linear AND angular momentum. The
/// authority's source body and spawned carrier rely on it; a missing, doubled or reversed
/// give-back, or one applied at the COM instead of the contact point, fails the sweep.
/// Mass ratios and off-centre contact heights are ASSUMED fixture data, not asset pass criteria.
#[test]
fn a_break_keeps_linear_and_angular_momentum() {
    let kind = shipped_house();
    let structure = kind.layout.structure(&kind.asset, 1.0);
    let (_, [c, other, _]) = fixture_chunks(&kind);
    let group = [c, other];
    let unloaded = launch(&kind, &structure, 1.0, &group, &[]);
    let props = departure_mass(&structure, &unloaded);
    let com = unloaded.centroid + props.center_of_mass;
    let mut worst = 0.0_f32;
    for ratio in [0.001, 0.1, 1.0, 10.0, 1000.0] {
        for height in [0.0, 1e-4, 0.1, 1.0, 5.0] {
            let mass = props.mass * ratio;
            let load = Load {
                chunk: c,
                at: com + Vec3::Y * height,
                energy: 0.0,
                blow: Vec3::Z * (mass * 3.0),
                press: Vec3::ZERO,
                origin: Origin::Body {
                    body: Entity::PLACEHOLDER,
                    mass,
                    speed: 3.0,
                },
            };
            let launched = launch(&kind, &structure, 1.0, &group, &[load]);
            assert_eq!(launched.returns.len(), 1);
            let returned = launched.returns[0];
            assert_eq!(returned.at, load.at, "give-back retains the contact point");
            let net = returned.impulse - load.blow;
            let departure = launched.velocity * props.mass;
            let error = (net + departure).length() / load.blow.length();
            worst = worst.max(error);
            assert!(
                error < 2e-6,
                "ratio {ratio}, height {height}: momentum residual {error}"
            );
            let angular = load.at.cross(net)
                + com.cross(departure)
                + props.angular_inertia_tensor() * launched.spin;
            assert!(
                angular.length() < 2e-5 * load.blow.length() * (1.0 + load.at.length()),
                "ratio {ratio}, height {height}: angular residual {angular}"
            );
        }
    }
    println!("MEASURED break sweep maximum relative momentum residual: {worst:e}");
}

/// NO ENERGY IS CREATED THROUGH A BREAK (the game's environment destruction model). Across
/// light and heavy sources and contact points arbitrarily close to a held hinge, the actual
/// free carrier's translation plus spin never exceeds the source's kinetic energy loss.
/// Carrier physics relies on using free effective mass, never hinge mass. DERIVED: the plastic
/// loss is half the reduced mass times closing speed squared. Sweep numbers are fixture data.
#[test]
fn a_break_launches_with_free_mass_and_creates_no_energy() {
    let kind = shipped_house();
    let structure = kind.layout.structure(&kind.asset, 1.0);
    let (_, [c, _, _]) = fixture_chunks(&kind);
    let group = [c];
    let unloaded = launch(&kind, &structure, 1.0, &group, &[]);
    let props = departure_mass(&structure, &unloaded);
    let com = unloaded.centroid + props.center_of_mass;
    let inverse = props.angular_inertia_tensor().inverse();
    let mut least_loss = f64::INFINITY;
    for ratio in [0.001, 0.1, 1.0, 10.0, 1000.0] {
        for height in [0.0, 1e-6, 1e-4, 0.1, 1.0, 5.0] {
            let mass = props.mass * ratio;
            let load = Load {
                chunk: c,
                at: com + Vec3::Y * height,
                energy: 0.0,
                blow: Vec3::Z * (mass * 3.0),
                press: Vec3::ZERO,
                origin: Origin::Body {
                    body: Entity::PLACEHOLDER,
                    mass,
                    speed: 3.0,
                },
            };
            let launched = launch(&kind, &structure, 1.0, &group, &[load]);
            let arm = (load.at - com).cross(Vec3::Z);
            let free_inverse = 1.0 / props.mass + arm.dot(inverse * arm);
            let expected = load.blow / (1.0 + mass * free_inverse);
            let actual = launched.velocity * props.mass;
            assert!(
                (actual - expected).length() <= 2e-6 * expected.length(),
                "ratio {ratio}, height {height}: free-mass share {actual} vs {expected}"
            );
            let source_before = load.blow.as_dvec3().length_squared() / (2.0 * f64::from(mass));
            let source_after =
                launched.returns[0].impulse.as_dvec3().length_squared() / (2.0 * f64::from(mass));
            let freed = 0.5 * f64::from(props.mass) * launched.velocity.as_dvec3().length_squared()
                + 0.5
                    * f64::from(
                        launched
                            .spin
                            .dot(props.angular_inertia_tensor() * launched.spin),
                    );
            let lost = source_before - source_after;
            assert!(
                freed <= lost + 2e-5 * source_before,
                "ratio {ratio}, height {height}: carrier {freed} J exceeds source loss {lost} J"
            );
            let plastic = 0.5 * f64::from(mass / (1.0 + mass * free_inverse)) * 9.0;
            assert!(
                (lost - freed - plastic).abs() <= 2e-5 * source_before,
                "ratio {ratio}, height {height}: plastic budget {} vs {plastic}",
                lost - freed
            );
            least_loss = least_loss.min((lost - freed) / source_before);
            let field = Load {
                origin: Origin::Field,
                ..load
            };
            let limit = Load {
                origin: Origin::Body {
                    body: Entity::PLACEHOLDER,
                    mass: 1e-12,
                    speed: 3.0,
                },
                ..load
            };
            let field = launch(&kind, &structure, 1.0, &group, &[field]);
            let limit = launch(&kind, &structure, 1.0, &group, &[limit]);
            assert_eq!(
                (field.velocity, field.spin),
                (limit.velocity, limit.spin),
                "a field is the zero-source-mass limit for launch, too"
            );
            assert!(field.returns.is_empty(), "a field has no body to return to");
        }
    }
    println!("MEASURED break sweep least plastic loss / initial source energy: {least_loss:e}");
}

/// GIVE-BACK CHANGES THE SOURCE AT THE WORLD CONTACT IN THE LAW PASS (the game's environment
/// destruction model). The authority relies on the actual Avian point impulse, including torque
/// and a rotated, translated building frame. A return deferred to another tick, rotated twice,
/// applied at the building origin or changing linear velocity alone fails here. Fixture data.
#[test]
fn give_back_applies_the_world_point_impulse_immediately() {
    use bevy::ecs::system::RunSystemOnce;
    let (mut app, _) = composed(false);
    let pose = Transform::from_xyz(100.0, 30.0, -70.0).with_rotation(Quat::from_rotation_y(0.7));
    let frame = Frame::new(&pose, 1.0, (Vec3::ZERO, Vec3::ONE));
    let at = Vec3::new(2.0, 1.0, -3.0);
    let world_at = pose.translation + pose.rotation * at;
    let source_at = world_at + Vec3::new(0.3, -0.2, 0.4);
    let source = moving_cube(&mut app, source_at, CUBE_SIDE, Layer::Debris);
    app.update();
    app.update();
    let returned = BodyReturn {
        body: source,
        at,
        impulse: Vec3::Z * 50.0,
    };
    let mass = app.world().get::<ComputedMass>(source).unwrap().value();
    let inertia = *app.world().get::<ComputedAngularInertia>(source).unwrap();
    let impulse = pose.rotation * returned.impulse;
    let expected_spin = inertia.inverse() * (world_at - source_at).cross(impulse);
    app.world_mut()
        .run_system_once(move |mut bodies: Query<(Forces, &RigidBody)>| {
            give_back(&mut bodies, frame, &[returned]);
        })
        .unwrap();
    let velocity = app.world().get::<LinearVelocity>(source).unwrap().0;
    let spin = app.world().get::<AngularVelocity>(source).unwrap().0;
    assert!((velocity - impulse / mass).length() < 1e-6);
    assert!(
        (spin - expected_spin).length() < 1e-5,
        "{spin} vs {expected_spin}"
    );
}

/// ONLY DEPARTING CHUNKS GIVE BACK A BODY'S BLOW (the game's environment destruction model).
/// The authority must leave held contacts with the solver, return dust's whole blow (no carrier),
/// and send fallen chunks' loads to launch once. Presses and fields supply no give-back.
#[test]
fn only_departing_chunks_give_back_their_body_blows() {
    let kind = shipped_house();
    let (_, [c, _, _]) = fixture_chunks(&kind);
    let mut live = Live::new(&kind, 1.0);
    let load = Load {
        chunk: c,
        at: live.structure.chunks[c].centroid,
        energy: 0.0,
        blow: Vec3::Z * 0.001,
        press: Vec3::X * 1000.0,
        origin: Origin::Body {
            body: Entity::PLACEHOLDER,
            mass: 1000.0,
            speed: 3.0,
        },
    };
    let held = run_laws(&kind.layout, &mut live, &[load], Vec::new());
    assert!(
        held.groups.is_empty() && held.returns.is_empty(),
        "a held wall keeps the solver's blow"
    );
    let dust = Load {
        energy: live.state.pool[c],
        origin: Origin::Field,
        blow: Vec3::ZERO,
        ..load
    };
    let changes = run_laws(&kind.layout, &mut live, &[load, dust], Vec::new());
    assert_eq!(
        changes.returns,
        vec![BodyReturn {
            body: Entity::PLACEHOLDER,
            at: load.at,
            impulse: load.blow
        }]
    );
    let press = Load {
        blow: Vec3::ZERO,
        ..load
    };
    let changes = run_laws(&kind.layout, &mut live, &[press, dust], Vec::new());
    assert!(
        changes.returns.is_empty(),
        "neither a press nor a field is returned"
    );
    let mut live = Live::new(&kind, 1.0);
    live.state.fate.fill(Fate::Dust);
    live.state.fate[c] = Fate::Standing;
    live.state.cracked.fill(true);
    let changes = run_laws(&kind.layout, &mut live, &[load], Vec::new());
    assert_eq!(
        changes.groups,
        vec![(vec![c], vec![load])],
        "a freed chunk takes its body load to launch once"
    );
    assert!(
        changes.returns.is_empty(),
        "a fallen load returns only after launch chooses its share"
    );
    let repeated = run_laws(&kind.layout, &mut live, &[load], Vec::new());
    assert!(
        repeated.groups.is_empty(),
        "a chunk freed by an earlier deposit launches once"
    );
    assert_eq!(
        repeated.returns,
        vec![BodyReturn {
            body: Entity::PLACEHOLDER,
            at: load.at,
            impulse: load.blow
        }],
        "a later deposit on the departed chunk returns the whole body blow"
    );
}

/// SIMULTANEOUS EXCHANGES SHARE ONE KINETIC BUDGET (the game's environment destruction
/// model). Carriers and source bodies rely on a plastic common velocity: several bodies freeing
/// one group, or one body freeing several groups, cannot each spend the same momentum again.
/// Independent launch shares, or a cap applied separately per carrier/source point, fail here.
/// Sweep ratios and speed are ASSUMED fixture data; common velocities are DERIVED.
#[test]
fn simultaneous_exchanges_keep_momentum_and_spend_one_kinetic_budget() {
    use crate::contact_transfer::BodyInertia;
    let kind = shipped_house();
    let structure = kind.layout.structure(&kind.asset, 1.0);
    let (_, [c, _, _]) = fixture_chunks(&kind);
    let unloaded = launch(&kind, &structure, 1.0, &[c], &[]);
    let props = departure_mass(&structure, &unloaded);
    let com = unloaded.centroid + props.center_of_mass;
    let mut world = World::new();
    let bodies: Vec<Entity> = (0..3).map(|_| world.spawn_empty().id()).collect();
    for ratio in [0.01, 0.1, 1.0, 10.0, 1000.0] {
        let source_mass = props.mass * ratio;
        let load = |body| Load {
            chunk: c,
            at: com,
            energy: 0.0,
            blow: Vec3::Z * (source_mass * 3.0),
            press: Vec3::ZERO,
            origin: Origin::Body {
                body,
                mass: source_mass,
                speed: 3.0,
            },
        };
        let departure = |loads: &[Load]| Departure {
            kind: kind.clone(),
            building: 0,
            scale: 1.0,
            root: Transform::IDENTITY,
            group: vec![c],
            launch: launch(&kind, &structure, 1.0, &[c], loads),
        };
        let source = BodyInertia {
            centre: com,
            inverse_mass: Vec3::splat(1.0 / source_mass),
            inverse_inertia: Mat3::ZERO,
        };
        let mut many_sources = vec![departure(
            &bodies.iter().map(|&b| load(b)).collect::<Vec<_>>(),
        )];
        cap_exchanges(&mut many_sources, |_| source);
        let launched = &many_sources[0].launch;
        let expected = 3.0 * (3.0 * source_mass) / (3.0 * source_mass + props.mass);
        assert!(
            (launched.velocity.z - expected).abs() <= 2e-6 * expected,
            "ratio {ratio}: several sources give common velocity {}, expected {expected}",
            launched.velocity.z
        );
        let source_after: Vec<Vec3> = launched
            .returns
            .iter()
            .map(|r| r.impulse / source_mass)
            .collect();
        let momentum =
            launched.velocity * props.mass + source_after.iter().sum::<Vec3>() * source_mass;
        assert!((momentum - Vec3::Z * (9.0 * source_mass)).length() <= 2e-6 * 9.0 * source_mass);
        let before = 3.0 * 0.5 * source_mass * 9.0;
        let after = 0.5 * props.mass * launched.velocity.length_squared()
            + source_after
                .iter()
                .map(|v| 0.5 * source_mass * v.length_squared())
                .sum::<f32>();
        assert!(
            after <= before * (1.0 + 2e-6),
            "several sources: {after} J vs {before} J"
        );

        let mut many_groups: Vec<Departure> =
            (0..3).map(|_| departure(&[load(bodies[0])])).collect();
        // The solver's one source blow is shared over these contacts, never repeated whole.
        for group in &mut many_groups {
            let mut point_load = load(bodies[0]);
            point_load.blow /= 3.0;
            group.launch = launch(&kind, &structure, 1.0, &[c], &[point_load]);
        }
        cap_exchanges(&mut many_groups, |_| source);
        let expected = 3.0 * source_mass / (source_mass + 3.0 * props.mass);
        // A cap never adds impulse: the split source blow can supply less than the coupled
        // exchange, and then the nominal shares remain unchanged.
        let nominal = source_mass / (source_mass + props.mass);
        let expected = expected.min(nominal);
        for group in &many_groups {
            assert!(
                (group.launch.velocity.z - expected).abs() <= 3e-6 * expected,
                "ratio {ratio}: several groups give {}, expected {expected}",
                group.launch.velocity.z
            );
        }
        let returned: Vec3 = many_groups
            .iter()
            .flat_map(|g| &g.launch.returns)
            .map(|r| r.impulse)
            .sum();
        let source_velocity = returned / source_mass;
        let carrier_momentum: Vec3 = many_groups
            .iter()
            .map(|g| g.launch.velocity * props.mass)
            .sum();
        assert!(
            (carrier_momentum + returned - Vec3::Z * (3.0 * source_mass)).length()
                < 2e-6 * 3.0 * source_mass
        );
        let after = 0.5 * source_mass * source_velocity.length_squared()
            + many_groups
                .iter()
                .map(|g| 0.5 * props.mass * g.launch.velocity.length_squared())
                .sum::<f32>();
        assert!(
            after <= 0.5 * source_mass * 9.0 * (1.0 + 2e-6),
            "several groups: kinetic budget"
        );

        let field = Load {
            origin: Origin::Field,
            blow: Vec3::Z * (props.mass * 6.0),
            ..load(bodies[0])
        };
        let field_only = launch(&kind, &structure, 1.0, &[c], &[field]);
        let mut mixed = vec![departure(&[field, load(bodies[0])])];
        cap_exchanges(&mut mixed, |_| source);
        assert!(
            (mixed[0].launch.velocity - field_only.velocity).length() < 2e-6,
            "a group already moving away faster than its source keeps its field blow only"
        );
        assert_eq!(
            mixed[0].launch.returns[0].impulse,
            load(bodies[0]).blow,
            "a body exchanges nothing with a faster departing group"
        );
        let unrelated_load = load(bodies[1]);
        let unrelated = departure(&[unrelated_load]);
        let unrelated_velocity = unrelated.launch.velocity;
        let mut disjoint = vec![departure(&[field, load(bodies[0])]), unrelated];
        cap_exchanges(&mut disjoint, |_| source);
        assert!(
            (disjoint[0].launch.velocity - field_only.velocity).length() < 2e-6,
            "an unrelated source cannot subsidize a separating contact"
        );
        assert!(
            (disjoint[1].launch.velocity - unrelated_velocity).length() < 2e-6,
            "an unrelated separating contact cannot reduce this source's exchange"
        );
    }
}

/// A BREAK LAUNCHES AND RETURNS IN ONE AUTHORITY PASS (the game's environment destruction
/// model). The composition relies on the real contact reader, source inertia, law pass, carrier
/// spawn and Avian give-back agreeing. After the solver's strike, releasing the contacted fixture
/// chunk conserves momentum including the wall's retained press, and never raises kinetic energy;
/// an empty subsequent queue returns
/// nothing twice. No strength assertion is made: this fixture opens the bonds explicitly.
#[test]
fn a_break_launches_and_returns_in_one_authority_pass() {
    use bevy::ecs::system::RunSystemOnce;
    #[derive(Resource, Default)]
    struct Released(Option<usize>);
    let kind = shipped_house();
    let (_, face, direction) = wall_face(&kind);
    let (mut app, root) = composed_keeping_contacts();
    let source = contact_cube(
        &mut app,
        face - direction * (CUBE_SIDE / 2.0 + 0.3),
        direction * 3.0,
    );
    app.init_resource::<Released>().add_systems(
        FixedPostUpdate,
        (move |queue: Res<WallQueue>,
               mut buildings: Query<&mut Breakage>,
               mut released: ResMut<Released>| {
            if released.0.is_some() {
                return;
            }
            let Some(load) = queue
                .0
                .iter()
                .flat_map(|d| &d.contacts)
                .filter(|(r, _)| *r == root)
                .map(|(_, load)| load)
                .find(|load| load.blow != Vec3::ZERO)
            else {
                return;
            };
            let mut breakage = buildings.get_mut(root).unwrap();
            let live = breakage.live();
            live.state.fate.fill(Fate::Dust);
            live.state.fate[load.chunk] = Fate::Standing;
            live.state.cracked.fill(true);
            released.0 = Some(load.chunk);
        })
        .after(keep_queued_contacts)
        .before(apply_deposits),
    );
    for _ in 0..30 {
        app.update();
        if app.world().resource::<Released>().0.is_some() {
            break;
        }
    }
    let c = app
        .world()
        .resource::<Released>()
        .0
        .expect("the fixture source meets the wall");
    let (piece_velocity, piece_spin) = app
        .world_mut()
        .query_filtered::<(&LinearVelocity, &AngularVelocity), With<WallPiece>>()
        .single(app.world())
        .map(|(v, w)| (v.0, w.0))
        .expect("the contacted chunk leaves as one carrier");
    let structure = kind.layout.structure(&kind.asset, 1.0);
    let free = launch(&kind, &structure, 1.0, &[c], &[]);
    let props = departure_mass(&structure, &free);
    let source_mass = app.world().get::<ComputedMass>(source).unwrap().value();
    let source_velocity = app.world().get::<LinearVelocity>(source).unwrap().0;
    let source_spin = app.world().get::<AngularVelocity>(source).unwrap().0;
    let inertia = app.world().get::<ComputedAngularInertia>(source).unwrap();
    let press: Vec3 = app
        .world()
        .resource::<QueuedContacts>()
        .0
        .last()
        .unwrap()
        .iter()
        .map(|(_, load)| load.press)
        .sum();
    let held_impulse = press * app.world().resource::<Time<Fixed>>().delta_secs();
    let momentum = source_velocity * source_mass + piece_velocity * props.mass + held_impulse;
    assert!(
        (momentum - direction * (source_mass * 3.0)).length() < 2e-6 * source_mass * 3.0,
        "source, carrier and the press retained by the wall conserve momentum: {momentum}"
    );
    let kinetic = 0.5 * source_mass * source_velocity.length_squared()
        + 0.5 * source_spin.dot(inertia.value() * source_spin)
        + 0.5 * props.mass * piece_velocity.length_squared()
        + 0.5 * piece_spin.dot(props.angular_inertia_tensor() * piece_spin);
    assert!(
        kinetic <= 0.5 * source_mass * 9.0,
        "the actual source and carrier retain the energy bound"
    );
    assert!(
        piece_velocity.dot(direction) > 0.0 && source_velocity.dot(direction) > 0.0,
        "both bodies depart in the approach direction"
    );
    assert!(app.world().resource::<WallQueue>().0.is_empty());
    app.world_mut().run_system_once(apply_deposits).unwrap();
    assert_eq!(
        app.world().get::<LinearVelocity>(source).unwrap().0,
        source_velocity,
        "the next empty law pass replays no give-back"
    );
    println!(
        "MEASURED composed break: momentum residual {:.6e}, source + carrier kinetic {:.3} / initial {:.3} J",
        (momentum - direction * (source_mass * 3.0)).length() / (source_mass * 3.0),
        kinetic,
        0.5 * source_mass * 9.0
    );
}
