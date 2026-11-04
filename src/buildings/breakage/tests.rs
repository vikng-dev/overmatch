//! The breakage invariants, about the game's environment destruction (game state only), on the
//! shipped construction and minimal generic fixtures: which chunk a test reads is chosen by a class rule (a breakable
//! element's chunk nearest its centre, a chunk with no anchor), never by its number.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::time::TimeUpdateStrategy;

use super::*;
use crate::ballistics::MarchProbe;
use crate::buildings::fracture::FIT_ONLY_PRODUCTS;
use crate::buildings::schema::ElementKind;
use crate::buildings::tests::shipped_house;
use crate::scatter::MapInstance;

/// A fixture physics step, s: the harness's 64 Hz tick (fixture data).
const STEP: f32 = 1.0 / 64.0;

/// The breakage sim on the offline physics with the solver's contact reading, the march probe,
/// and the shipped house spawned at the origin (unit scale), one fixed tick per update. `replica`
/// adds the replica marker.
fn composed(replica: bool) -> (App, Entity) {
    composed_kind(replica, &shipped_house())
}

fn composed_kind(replica: bool, kind: &Arc<BuildingKind>) -> (App, Entity) {
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
    let world = app.world_mut();
    let mut queue = bevy::ecs::world::CommandQueue::default();
    let mut commands = Commands::new(&mut queue, world);
    let root = crate::buildings::spawn(
        &mut commands,
        kind,
        MapInstance {
            id: "house_0001".to_owned(),
            ordinal: 3,
        },
        Transform::IDENTITY,
    );
    queue.apply(world);
    (app, root)
}

/// Fixture: a column of three unit cubes, joined over their full faces, anchored at its base.
/// It has no house topology or view asset; all masses derive from the registry's material density.
fn column_construction() -> Arc<BuildingKind> {
    let registry = crate::substances::SubstanceRegistry::shipped_static();
    let density = Masonry::of(registry, "Brick").unwrap().unwrap().density;
    let hull = |lo: f32, hi: f32| {
        [-0.5, 0.5]
            .into_iter()
            .flat_map(|x| {
                [lo, hi]
                    .into_iter()
                    .flat_map(move |y| [-0.5, 0.5].map(|z| [x, y, z]))
            })
            .collect()
    };
    let asset = BuildingAsset {
        id: "column_fixture".into(),
        format: schema::FORMAT,
        foundation_y_m: 0.0,
        materials: vec!["Brick".into()],
        attachments: vec!["mortared".into()],
        components: vec!["column".into()],
        elements: vec![schema::Element {
            name: "column".into(),
            kind: ElementKind::Pier,
            component: 0,
            material: 0,
            surface_frame: schema::SurfaceFrame::Upright,
            hull: hull(0.0, 3.0),
            chunks: [0, 3],
        }],
        chunks: (0..3)
            .map(|c| schema::Chunk {
                element: 0,
                material: 0,
                hull: hull(c as f32, c as f32 + 1.0),
                volume_m3: 1.0,
                mass_kg: density,
                centroid: [0.0, c as f32 + 0.5, 0.0],
            })
            .collect(),
        joints: (0..2)
            .map(|a| schema::Joint {
                a,
                b: a + 1,
                area_m2: 1.0,
                centroid: [0.0, a as f32 + 1.0, 0.0],
                normal: [0.0, 1.0, 0.0],
                bond: schema::Bond::Seam,
            })
            .collect(),
        anchors: vec![schema::Anchor {
            chunk: 0,
            attachment: 0,
            area_m2: 1.0,
            centroid: [0.0; 3],
        }],
    };
    Arc::new(
        BuildingKind::resolve(
            &asset.id,
            &ron::ser::to_string(&asset).unwrap(),
            registry,
            Attachments::shipped_static(),
        )
        .unwrap(),
    )
}

/// The game's environment destruction model: authority, replicas and presentation rely on
/// complete spawn state and stable source addresses through every split. The production plugin
/// must strike, release and split a non-house construction without loading a view, retaining
/// existing wear and shared geometry. A deferred constructor or a split that renumbers or heals
/// the surviving chunk breaks this witness. Fixture loads remove supports without adding motion.
#[test]
fn a_construction_is_complete_at_spawn_and_keeps_its_state_through_splits() {
    let kind = column_construction();
    let (mut app, root) = composed_kind(false, &kind);
    let initial = app.world().get::<Breakage>(root).unwrap();
    let structure = initial.live.structure.clone();
    assert_eq!(
        initial.live.state.pool,
        structure
            .chunks
            .iter()
            .map(fracture::full_pool)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        initial.live.state.fate,
        vec![Fate::Standing; structure.chunks.len()]
    );
    assert_eq!(
        initial.live.state.cracked,
        vec![false; structure.joints.len()]
    );
    assert_eq!(initial.live.swapped, vec![false; kind.asset.elements.len()]);
    assert_eq!(
        initial.live.holes,
        vec![Vec::<Hole>::new(); structure.chunks.len()]
    );
    let construction = initial.construction;
    // Runtime ownership is the source ordinal, independent of a map-placement marker.
    app.world_mut().entity_mut(root).remove::<MapInstance>();
    app.update();
    app.update();
    let deposit = |app: &mut App, owner: Entity, chunk: usize, share: f32| {
        let breakage = app.world().get::<Breakage>(owner).unwrap();
        let matter = &breakage.live.structure.chunks[chunk];
        let frame = breakage.physics_frame((
            app.world().get::<Position>(owner).unwrap(),
            app.world().get::<Rotation>(owner).unwrap(),
        ));
        let at = frame.translation + frame.rotation * matter.centroid;
        let (from, to) = (
            at - frame.rotation * Vec3::X * 0.5,
            at + frame.rotation * Vec3::X * 0.5,
        );
        let energy = share * fracture::full_pool(matter) * matter.material.crush;
        let volume = app
            .world_mut()
            .query::<(Entity, &ChildOf, Option<&BuildingChunk>, &BallisticVolume)>()
            .iter(app.world())
            .find(|(_, parent, address, _)| {
                parent.parent() == owner
                    && address.is_none_or(|address| usize::from(address.0) == chunk)
            })
            .unwrap()
            .0;
        let shell = app.world_mut().spawn(PenetrationMarks::default()).id();
        app.world_mut().trigger(ShellMarched {
            shell,
            marks_from: 0,
            cones_from: 0,
            burst: None,
            lines: vec![DeliveredLine {
                contacts: vec![(from, to)],
                crossing: crate::ballistics::VolumeCrossing {
                    volume,
                    entry: from,
                    end: to,
                    chord: from.distance(to),
                    cost: 0.0,
                    energy,
                    perforated: true,
                },
                entrance_energy: 0.0,
                momentum: Vec3::ZERO,
                entrance_momentum: Vec3::ZERO,
            }],
        });
        app.update();
        app.update();
        app.world_mut().despawn(shell);
    };
    deposit(&mut app, root, 2, 0.01);
    let worn = app.world().get::<Breakage>(root).unwrap().live.state.pool[2];
    assert!(worn < fracture::full_pool(&structure.chunks[2]));
    let holes = app.world().get::<Breakage>(root).unwrap().live.holes[2].clone();
    assert!(
        !holes.is_empty(),
        "the delivered line records a visual channel"
    );
    deposit(&mut app, root, 0, 1.25);
    let (parent, piece) = app
        .world_mut()
        .query::<(Entity, &WallPiece)>()
        .single(app.world())
        .unwrap();
    assert_eq!(
        piece.chunks,
        vec![1, 2],
        "source addresses survive departure"
    );
    assert_eq!(piece.building, construction);
    let parent_state = app.world().get::<Breakage>(parent).unwrap();
    assert!(Arc::ptr_eq(&parent_state.kind, &kind));
    assert!(Arc::ptr_eq(&parent_state.live.structure, &structure));
    assert_eq!(
        parent_state.live.state.pool[2], worn,
        "departure keeps wear"
    );
    assert_eq!(
        parent_state.live.holes[2], holes,
        "departure keeps the chunk's view state"
    );
    deposit(&mut app, parent, 1, 1.25);
    assert!(
        app.world().get_entity(parent).is_err(),
        "a split retires its parent"
    );
    let (child, piece) = app
        .world_mut()
        .query::<(Entity, &WallPiece)>()
        .single(app.world())
        .unwrap();
    assert_eq!(
        piece.chunks,
        vec![2],
        "splitting never compacts source addresses"
    );
    assert_eq!(piece.building, construction);
    let child_state = app.world().get::<Breakage>(child).unwrap();
    assert_eq!(child_state.construction, construction);
    assert!(Arc::ptr_eq(&child_state.live.structure, &structure));
    assert_eq!(
        child_state.live.state.pool[2], worn,
        "splitting never heals wear"
    );
    assert_eq!(
        child_state.live.holes[2], holes,
        "splitting keeps the chunk's view state"
    );
    assert_eq!(
        child_state.live.state.fate,
        vec![Fate::Dust, Fate::Dust, Fate::Standing]
    );
    let height = app.world().get::<Position>(child).unwrap().y;
    app.insert_resource(Gravity(Vec3::NEG_Y * crate::physical_laws::gravity_mps2()));
    for _ in 0..8 {
        app.update();
    }
    assert!(
        app.world().get::<Position>(child).unwrap().y < height,
        "the owned group falls through production physics"
    );

    let record = app.world().resource::<WallRecord>().clone();
    let (mut replica, replica_root) = composed_kind(true, &kind);
    let replica_geometry = replica
        .world()
        .get::<Breakage>(replica_root)
        .unwrap()
        .live
        .structure
        .clone();
    replica
        .world_mut()
        .entity_mut(replica_root)
        .remove::<MapInstance>();
    replica.insert_resource(record);
    replica.update();
    assert_eq!(
        replica
            .world()
            .get::<Breakage>(replica_root)
            .unwrap()
            .shape()
            .fate,
        vec![Fate::Dust, Fate::Dust, Fate::Fallen],
        "the replica keeps the source addresses of split records"
    );
    {
        let mut record = replica.world_mut().resource_mut::<WallRecord>();
        record.entries.clear();
        record.generation += 1;
    }
    replica.update();
    let reset = replica.world().get::<Breakage>(replica_root).unwrap();
    assert_eq!(reset.shape(), Shape::intact(&kind.layout));
    assert!(
        Arc::ptr_eq(&reset.live.structure, &replica_geometry),
        "a new ledger reuses geometry"
    );
}

/// The game's environment destruction model: a moving owner's strength check reads spin after
/// earlier owners' plastic returns. Independent owners can evaluate together; crossing this
/// dependency or serializing every independent owner violates the evaluation contract.
#[test]
fn owner_evaluation_waits_only_for_earlier_source_returns() {
    let mut world = World::new();
    let mut roots: Vec<_> = (0..4).map(|_| world.spawn_empty().id()).collect();
    roots.sort_unstable();
    let load = Load {
        chunk: 0,
        at: Vec3::ZERO,
        energy: 0.0,
        crushed: 0.0,
        blow: Vec3::X,
        press: Vec3::ZERO,
        peak: None,
        origin: Origin::Body {
            body: roots[2],
            mass: 1.0,
            speed: 1.0,
            step: STEP,
        },
    };
    let taken = roots
        .iter()
        .map(|&root| {
            (
                root,
                Taken {
                    contacts: if root == roots[0] {
                        vec![load]
                    } else {
                        Vec::new()
                    },
                    ..default()
                },
            )
        })
        .collect();
    let batches: Vec<_> = owner_batches(taken).collect();
    let batch_of = |root| {
        batches
            .iter()
            .position(|batch| batch.contains_key(&root))
            .unwrap()
    };
    assert!(
        batch_of(roots[0]) < batch_of(roots[2]),
        "returns commit before their receiver reads spin"
    );
    assert_eq!(
        batch_of(roots[0]),
        batch_of(roots[1]),
        "an independent owner shares the first batch"
    );
    assert_eq!(
        batch_of(roots[2]),
        batch_of(roots[3]),
        "an independent owner shares the next batch"
    );
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

/// Element `e`'s thickness axis: across its thinnest span ([`Block::thinnest`]).
fn across(kind: &BuildingKind, e: usize) -> Vec3 {
    kind.layout.element_blocks[e].thinnest().0
}

/// A probe ray through chunk `c` across its element's thinnest span ([`Block::thinnest`]), from
/// half a metre before its near face to `beyond` m past its far face.
fn through(kind: &BuildingKind, e: usize, c: usize, beyond: f32) -> (Vec3, Dir3, f32) {
    let (w, near, far) = kind.layout.element_blocks[e].thinnest();
    let centroid = Vec3::from_array(kind.asset.chunks[kind.layout.chunks[c] as usize].centroid);
    let start = centroid + w * (near - 0.5 - centroid.dot(w));
    (start, Dir3::new(w).unwrap(), far - near + 0.5 + beyond)
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
        let live = breakage.live.as_mut();
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
    let hits: Vec<(usize, Vec3, f32, Vec3)> = neighbours
        .iter()
        .map(|&c| {
            let chunk = &structure.chunks[c];
            let energy = 2.0 * fracture::full_pool(chunk) * chunk.material.crush;
            (c, chunk.centroid, energy, Vec3::ZERO)
        })
        .collect();
    let changes = run_laws(
        &kind.layout.element_of,
        &mut authority,
        &Load::field(&hits, |c| structure.chunks[c].material.crush),
        Vec::new(),
        Vec3::ZERO,
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

/// A round's line through law chunk `c` (building frame, unit scale): 10 cm along unit `w` about
/// the chunk's centroid, carrying `share` of its full pool.
fn line_through_chunk(
    structure: &Structure,
    w: Vec3,
    c: usize,
    share: f32,
) -> (usize, Vec3, Vec3, f32) {
    let chunk = &structure.chunks[c];
    (
        c,
        chunk.centroid - w * 0.05,
        chunk.centroid + w * 0.05,
        share * fracture::full_pool(chunk) * chunk.material.crush,
    )
}

/// Rounds' lines through `structure` as the laws read them with no blow ([`Load::line`]).
fn line_loads(structure: &Structure, lines: &[(usize, Vec3, Vec3, f32)]) -> Vec<Load> {
    lines
        .iter()
        .flat_map(|&(c, entry, end, energy)| {
            Load::line(
                c,
                &structure.chunks[c].material,
                entry,
                end,
                energy,
                Vec3::ZERO,
            )
        })
        .collect()
}

/// A fixture crushed-volume load on law chunk `c` at its centroid, taking `share` of its pool, with
/// no cavity energy, impulse or peak force: the removal half of a blast, independent of its
/// pressure-driven section loading. ASSUMED fixture volume, not an energy-to-crush adapter.
fn blast_on(structure: &Structure, c: usize, share: f32) -> Vec<Load> {
    let chunk = &structure.chunks[c];
    let crushed = share * fracture::full_pool(chunk);
    vec![Load {
        chunk: c,
        at: chunk.centroid,
        energy: 0.0,
        blow: Vec3::ZERO,
        press: Vec3::ZERO,
        peak: None,
        origin: Origin::Field,
        crushed,
    }]
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
        &authority.structure,
        across(&kind, e),
        channel_chunk,
        0.05,
    )];
    let loads = line_loads(&authority.structure, &lines);
    let first = run_laws(
        &kind.layout.element_of,
        &mut authority,
        &loads,
        holes_of(&kind, 1.0, &lines, &[]),
        Vec3::ZERO,
    );
    let blast = blast_on(&authority.structure, dish_chunk, 0.05);
    let holes = holes_of(&kind, 1.0, &[], &blast);
    let second = run_laws(
        &kind.layout.element_of,
        &mut authority,
        &blast,
        holes,
        Vec3::ZERO,
    );
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
/// round twice (its line and its push, fixture data under the chunk's punch-ring energy, so the
/// chunk stands for the second), one cutting the line's hole and one not; the second round meets
/// the holed chunk whole. Their events but the holes, their loose groups, structural
/// failures, laws' state, swaps and chunk table are the same. A replica fed the record without its
/// holes, then the holes, keeps the very same colliders and shape. The owner's decision (a hole is
/// drawn only) rests on it: a hole read by a law, the mass or the colliders fails here.
#[test]
fn a_hole_changes_no_collider_mass_or_structural_result() {
    let kind = shipped_house();
    let (e, [c, ..]) = fixture_chunks(&kind);
    let mut cut = Live::new(&kind, 1.0);
    let mut whole = Live::new(&kind, 1.0);
    let line = line_through_chunk(&cut.structure, across(&kind, e), c, 0.3);
    let push = Load {
        chunk: c,
        at: cut.structure.chunks[c].centroid,
        energy: 0.0,
        crushed: 0.0,
        blow: (line.2 - line.1).normalize() * 10.0,
        press: Vec3::ZERO,
        peak: None,
        origin: Origin::Field,
    };
    let loads: Vec<Load> = line_loads(&cut.structure, &[line])
        .into_iter()
        .chain([push])
        .collect();
    let holes = holes_of(&kind, 1.0, &[line], &[]);
    let mut record = Vec::new();
    for tick in [7, 9] {
        let with = run_laws(
            &kind.layout.element_of,
            &mut cut,
            &loads,
            holes.clone(),
            Vec3::ZERO,
        );
        let without = run_laws(
            &kind.layout.element_of,
            &mut whole,
            &loads,
            Vec::new(),
            Vec3::ZERO,
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

/// ONLY THE POOL DECIDES DUST, THE CAP ONLY BOUNDS WHAT IS DRAWN: lines far too weak to spend a
/// chunk's pool each keep a hole up to [`HOLES_PER_CHUNK`]; every later line still wears the pool,
/// records no hole and leaves the chunk standing, until the volume they crushed spends the pool.
/// The view's table and the shared record rely on the bound and the laws on the pool alone: a count
/// that dusts a chunk, or a wear dropped past the cap, fails this. ASSUMED fixture: one repeated
/// line sized so that four lines past the cap spend the pool, on seams too strong to open first.
#[test]
fn past_the_cap_a_chunk_stands_until_its_pool_is_spent() {
    let kind = shipped_house();
    let (e, [c, ..]) = fixture_chunks(&kind);
    let mut live = Live::new(&kind, 1.0);
    let to_spend = HOLES_PER_CHUNK + 4;
    let line = line_through_chunk(&live.structure, across(&kind, e), c, 1.0 / to_spend as f32);
    let holes = holes_of(&kind, 1.0, &[line], &[]);
    assert_eq!(holes.len(), 1);
    let loads = line_loads(&live.structure, &[line]);
    for joint in &mut Arc::make_mut(&mut live.structure).joints {
        if let Bond::Bonded(seam) = &mut joint.bond {
            *seam = fracture::Seam {
                tensile: 1e12,
                fracture_energy: 1e12,
                shear: fracture::Shear::OpeningOnly,
            };
        }
    }
    let spent_at = (1..=to_spend + 1)
        .find(|&k| {
            let pool = live.state.pool[c];
            let changes = run_laws(
                &kind.layout.element_of,
                &mut live,
                &loads,
                holes.clone(),
                Vec3::ZERO,
            );
            if changes.events.contains(&WallEvent::Dust(c as u16)) {
                return true;
            }
            let holed = changes
                .events
                .iter()
                .any(|event| matches!(event, WallEvent::Hole(h, _) if usize::from(*h) == c));
            assert!(live.state.pool[c] < pool, "line {k} wears the pool");
            assert_eq!(live.state.fate[c], Fate::Standing, "line {k}");
            assert_eq!(
                holed,
                k <= HOLES_PER_CHUNK,
                "line {k}: {:?}",
                changes.events
            );
            assert_eq!(live.holes[c].len(), k.min(HOLES_PER_CHUNK));
            false
        })
        .expect("the crushed volume spends the pool");
    assert!(
        spent_at.abs_diff(to_spend) <= 1,
        "the pool, not the count, emptied at line {spent_at} of {to_spend}"
    );
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

/// A HOLE HOLDS THE VOLUME ITS LOAD CRUSHED: a line's channel (its cylinder about the chord,
/// narrower here than the ball) holds `energy / crush` and a blast's dish (its half ball) its
/// load's crushed volume, the volume each takes from the chunk's pool, to the millimetre rounding
/// of their radius and ends. The size rule rests on it: a hole is never
/// the size of a chunk still standing, since that volume is a spent pool and recorded as dust. A
/// radius off by a constant factor fails here.
#[test]
fn a_hole_holds_the_volume_its_load_crushed() {
    use std::f32::consts::PI;
    let kind = shipped_house();
    let (e, [c, ..]) = fixture_chunks(&kind);
    let structure = kind.layout.structure(&kind.asset, 1.0);
    let crush = structure.chunks[c].material.crush;
    let metres = |p: [i16; 3]| Vec3::from_array(p.map(|v| f32::from(v) / 1000.0));

    let line = line_through_chunk(&structure, across(&kind, e), c, 0.01);
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
        crushed: blast[0].crushed,
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

/// THE ROOF'S FIXINGS AND RESTS ARE DISTINCT: a supported roof stays after a local strike;
/// losing its masonry supports overloads fixed joints and frees the unsupported portion. On the shipped house (fixture data; the parts chosen by class: the
/// roof planes, the plate a plane is fixed to, the masonry that plate bears on, and the gables of
/// the walls the planes rest on), a round through a plane's middle chunk wears it and moves
/// nothing, and a blast that spends that chunk carves the plane, which stands. Shooting out the
/// long wall under the plate (every standing unburied chunk of it turned to dust) leaves the whole
/// roof on the house: the plate hangs between the gables, the planes still rest on them. Shooting
/// out the gables too leaves the roof hanging from the other plate by nails, which its weight
/// breaks locally (weight failures across overloaded fixed joints alone): the plane fixed to the
/// plate that lost its wall leaves whole, and no touched chunk stands unassessed. The other
/// plane, nailed at its eave to a plate whose wall still stands with its ridge free, is not
/// pinned: whether eave fixings hold a cantilevered plane is the continued-fracture law's open
/// question (`.agents/docs/design/wall-destruction-sweep-2.md`). A roof laid
/// unfixed on its plates (a rest at a 3 : 4 pitch, steeper than the friction of wood on wood)
/// slides off at the first touch and fails here; so does a law that let a rest carry tension.
#[test]
fn a_nailed_roof_distinguishes_rests_from_local_fixing_failure() {
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
    // Fixture data: the line runs along the plane's declared grain, the boards' length.
    let schema::SurfaceFrame::Grain(grain) = elements[roof_element].surface_frame else {
        panic!("the shipped roof declares its grain");
    };
    let line = line_through_chunk(&live.structure, Vec3::from_array(grain), middle, 0.1);
    let push = Load {
        chunk: middle,
        at: line.1,
        energy: 0.0,
        crushed: 0.0,
        blow: (line.2 - line.1).normalize() * (line.3 / 773.0),
        press: Vec3::ZERO,
        peak: None,
        origin: Origin::Field,
    };
    let loads: Vec<Load> = line_loads(&live.structure, &[line])
        .into_iter()
        .chain([push])
        .collect();
    let changes = run_laws(
        &layout.element_of,
        &mut live,
        &loads,
        Vec::new(),
        Vec3::ZERO,
    );
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
    let changes = run_laws(
        &layout.element_of,
        &mut live,
        &blast,
        Vec::new(),
        Vec3::ZERO,
    );
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
            GroupLoads::new(&[], Vec3::ZERO),
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
         standing {} of {} ({} touched chunks unassessed)",
        dusted,
        weighed
            .failures
            .iter()
            .map(|f| (f.cause, f.joints.len(), f.demand, f.capacity))
            .collect::<Vec<_>>(),
        groups.iter().map(Vec::len).collect::<Vec<_>>(),
        standing(&live, &roof),
        roof.len(),
        weighed.unassessed.len()
    );
    assert!(!weighed.failures.is_empty());
    for failure in &weighed.failures {
        assert_eq!(failure.cause, fracture::Cause::Weight, "{failure:?}");
        assert!(failure.joints.iter().all(|&j| fixed(&joints[j])));
    }
    assert_eq!(
        standing(&live, &plane),
        0,
        "the plane fixed to the plate that lost its wall and its rests leaves whole"
    );
    assert!(
        !weighed.unassessed.is_empty(),
        "the locally deferred fixing/contact section remains counted: {:?}",
        weighed.unassessed
    );
    probe_remaining_roof(&mut live, &roof);
}

/// Recheck every surviving roof chunk, not just the neighbours of the last removal. Class
/// invariants only: a standing, unburied part holds by a live boundary, and a locally deferred
/// section stays counted even when an enclosing wall section was assessed. What that boundary is
/// made of is the house's construction, measured in the destruction record, never a pass
/// condition here.
fn probe_remaining_roof(live: &mut Live, roof: &[usize]) {
    let remaining: Vec<_> = roof
        .iter()
        .copied()
        .filter(|&c| live.state.standing(c))
        .collect();
    let boundary = live
        .structure
        .joints
        .iter()
        .enumerate()
        .filter(|(j, joint)| {
            !live.state.cracked[*j]
                && live.state.live(joint)
                && match joint.b {
                    Side::Chunk(b) => remaining.contains(&joint.a) != remaining.contains(&b),
                    Side::Ground => remaining.contains(&joint.a),
                }
        })
        .count();
    assert!(
        boundary > 0,
        "a standing, unburied part holds by a live boundary"
    );
    let rechecked = fracture::weigh(
        &live.structure,
        &mut live.state,
        &[],
        &[],
        GroupLoads::new(&[], Vec3::ZERO),
        &remaining,
        &mut live.sections,
    );
    assert!(
        !rechecked.unassessed.is_empty(),
        "a locally deferred eave must be counted even when a larger wall section was assessed"
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
    let w = across(&kind, e);
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

/// A charge whose peak overpressure a metre out, square on, beats the shipped brick's dynamic
/// compressive strength, J (fixture data: 200 kg TNT-equivalent; DERIVED about 175 MPa there
/// against 86 MPa), so a chunk its rays reach is crushed (Law 1): the witness that they reached
/// it. The 88 mm fixture burst's peak falls under that strength within 0.2 m.
const CRUSHING_BLAST_J: f32 = 200.0 * 4.184e6;

/// Set off the fixture charge at `at`, and run the tick that applies it and the one after.
fn charge(app: &mut App, at: Vec3) {
    set_off(app, at, Blast::of(FIXTURE_BLAST_J, 1e-3, FIT_ONLY_PRODUCTS));
}

/// Set off `blast` at `at`, and run the tick that applies it and the one after.
fn set_off(app: &mut App, at: Vec3, blast: Blast) {
    app.world_mut().trigger(WallCharge { at, blast });
    app.update();
    app.update();
}

/// Fixture data for ray shielding: one anchored cube, 0.2 m per side. The invariant needs
/// a crushable surface and a blocker, not a house's structural failure search.
fn shielding_cube() -> Arc<BuildingKind> {
    let registry = crate::substances::SubstanceRegistry::shipped_static();
    let element = schema::Element {
        name: "fixture/cube".to_owned(),
        kind: ElementKind::Pier,
        component: 0,
        material: 0,
        surface_frame: schema::SurfaceFrame::Upright,
        hull: [-0.1, 0.1]
            .into_iter()
            .flat_map(|x| [0.0, 0.2].map(|y| [-0.1, 0.1].map(|z| [x, y, z])))
            .flatten()
            .collect(),
        chunks: [0, 1],
    };
    let volume = 0.2_f32.powi(3);
    let asset = BuildingAsset {
        id: "shielding_fixture".to_owned(),
        format: schema::FORMAT,
        foundation_y_m: 0.0,
        materials: vec!["Brick".to_owned()],
        attachments: vec!["mortared".to_owned()],
        components: vec!["fixture".to_owned()],
        chunks: vec![schema::Chunk {
            element: 0,
            material: 0,
            hull: element
                .points()
                .into_iter()
                .map(|point| point.to_array())
                .collect(),
            volume_m3: volume,
            centroid: [0.0, 0.1, 0.0],
            mass_kg: volume * Masonry::of(registry, "Brick").unwrap().unwrap().density,
        }],
        elements: vec![element],
        joints: Vec::new(),
        anchors: vec![schema::Anchor {
            chunk: 0,
            attachment: 0,
            area_m2: 0.04,
            centroid: [0.0; 3],
        }],
    };
    Arc::new(
        BuildingKind::resolve(
            &asset.id,
            &ron::ser::to_string(&asset).unwrap(),
            registry,
            Attachments::shipped_static(),
        )
        .unwrap(),
    )
}

/// A BLAST RAY THAT STOPS ON A MOVING BODY PUSHES IT BY ITS SHARE AND DEPOSITS NOTHING: a loose
/// piece, or a dynamic body that is no piece (a launched turret's stand-in), between a fixture burst
/// and a standing fixture chunk leaves with the momentum of the rays that meet it, each its
/// share's reflected impulse at the distance and on the face avian's cast gives ([`Blast::push`]),
/// and the chunk it shades keeps its full pool, though the same burst crushes it with nothing
/// there ([`CRUSHING_BLAST_J`]).
/// A partial shadow leaves the other fixture rays free to wear the chunk. A body's armour
/// volume is no surface: the rays pass it and wear the chunk. A ray that pushed a
/// body and went on into the wall, a push of the wrong share, distance or face, a moving body the
/// rays pass through, or one they meet by kind rather than by being a moving body, fails here;
/// every rubble pile and hull a blast meets rests on it.
#[test]
fn a_ray_that_stops_on_a_moving_body_pushes_it_by_its_share_and_deposits_nothing() {
    let kind = shielding_cube();
    let (e, c) = (0, 0);
    let (near, w, _) = through(&kind, e, c, 0.0);
    // The burst a metre before the chunk's near face, the body half-way (fixture data).
    let origin = near - *w * 0.5;
    let blast = Blast::of(CRUSHING_BLAST_J, 1e-3, FIT_ONLY_PRODUCTS);
    let after = |spawn: Option<&dyn Fn(&mut App) -> Entity>| {
        let (mut app, root) = composed_kind(false, &kind);
        let body = spawn.map(|spawn| spawn(&mut app));
        app.update();
        app.update();
        // The fixture stands at the origin at unit scale, so its frame is the world.
        // Keep rays that meet the body before the fixture surface.
        let intact = Shape::intact(&kind.layout);
        let rays: Vec<_> = body.map_or_else(Vec::new, |body| {
            rays_on(&mut app, body, origin)
                .into_iter()
                .filter(|&(dir, _, d, _)| {
                    first_chunk(&kind.layout, &intact, true, origin, dir).is_none_or(|(_, t)| t > d)
                })
                .collect()
        });
        let aimed = fracture::blast_directions()
            .iter()
            .filter(|sample| first_chunk(&kind.layout, &intact, true, origin, sample.0).is_some())
            .count();
        let shadowed = rays
            .iter()
            .filter(|ray| first_chunk(&kind.layout, &intact, true, origin, ray.0).is_some())
            .count();
        let expected: Vec3 = rays
            .iter()
            .map(|&(dir, share, d, normal)| blast.push(share, d, dir, normal))
            .sum();
        set_off(&mut app, origin, blast);
        let momentum = body.map_or(Vec3::ZERO, |body| {
            let world = app.world();
            world.get::<ComputedMass>(body).unwrap().value()
                * world.get::<LinearVelocity>(body).unwrap().0
        });
        let breakage = app.world().get::<Breakage>(root).unwrap();
        let live = &breakage.live;
        let wears = live.state.pool[c] < fracture::full_pool(&live.structure.chunks[c]);
        (rays.len(), momentum, expected, shadowed, aimed, wears)
    };
    let (.., bare) = after(None);
    assert!(bare, "with nothing there the burst crushes the chunk");
    let piece = |app: &mut App| loose_cube(app, near, 0.4);
    let other = |app: &mut App| moving_cube(app, near, 0.4, Layer::Vehicle);
    for spawn in [&piece as &dyn Fn(&mut App) -> Entity, &other] {
        let (rays, momentum, expected, shadowed, aimed, wears) = after(Some(spawn));
        assert_eq!(shadowed, aimed, "the large body shades every fixture ray");
        assert!(rays > 100, "the body is in the field");
        assert!(
            (momentum - expected).length() <= 1e-3 * expected.length(),
            "the body leaves with {momentum}, its rays pushed {expected}"
        );
        assert!(!wears, "the shaded chunk takes nothing");
    }
    // Fixture: a smaller body covers only part of the cube's angular silhouette. Rays
    // outside that shadow still wear the surface; one stopped ray cannot suppress the field.
    let partial = |app: &mut App| loose_cube(app, near, 0.05);
    let (rays, momentum, expected, shadowed, aimed, wears) = after(Some(&partial));
    assert!(
        shadowed > 0 && shadowed < aimed,
        "the body shades some fixture rays and leaves others open"
    );
    assert!(rays > 0 && (momentum - expected).length() <= 1e-3 * expected.length());
    assert!(
        wears,
        "rays outside the body's shadow still reach the chunk"
    );

    let armour = |app: &mut App| moving_cube(app, near, 0.4, Layer::Armor);
    let (rays, momentum, _, _, _, wears) = after(Some(&armour));
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
    let (origin, w, reach) = through(&kind, e, c, 0.0);
    // Each cube 0.3 m past the wall's far face, or as far out on the burst's side (fixture data).
    let out = reach + 0.3;
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
/// fixture burst's side of a crushable surface, pushed with nothing between, takes nothing behind a static
/// slab on the ground's layer (the layer of the ground and of a standing tree); and a burst inside
/// a dynamic body's proxy still crushes the chunk it faces ([`CRUSHING_BLAST_J`]). Rays that pass
/// the ground (a house below the horizon taking a push), or a field blanked at 0 m by the body it
/// bursts in, fail here.
#[test]
fn the_ground_stops_a_blast_and_a_body_holding_the_burst_shades_nothing() {
    let kind = shielding_cube();
    let (e, c) = (0, 0);
    let (origin, w, reach) = through(&kind, e, c, 0.0);
    let out = reach + 0.3;
    let speed_of_open = |slab: bool| {
        let (mut app, _) = composed_kind(false, &kind);
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

    let (mut app, root) = composed_kind(false, &kind);
    moving_cube(&mut app, origin, 0.4, Layer::Vehicle);
    app.update();
    app.update();
    set_off(
        &mut app,
        origin,
        Blast::of(CRUSHING_BLAST_J, 1e-3, FIT_ONLY_PRODUCTS),
    );
    let breakage = app.world().get::<Breakage>(root).unwrap();
    let live = breakage.live.as_ref();
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

/// An implicit belt contact enters the wall's actual queued reading without a source collider.
/// The wall laws rely on the same load path as solver contacts, with no body-kind exception.
/// FIXTURE DATA: a 20 N press on a breakable outer wall; the source is clear of solver contacts.
#[test]
fn an_implicit_belt_press_reaches_the_wall_laws_reading() {
    let (mut app, root) = composed_keeping_contacts();
    let kind = shipped_house();
    let (element, face, into) = wall_face(&kind);
    let source = contact_cube(&mut app, Vec3::splat(50.0), Vec3::ZERO);
    app.update();
    let collider = app
        .world_mut()
        .query::<(Entity, &BuildingElement, &ChildOf)>()
        .iter(app.world())
        .find(|(_, index, parent)| usize::from(index.0) == element && parent.parent() == root)
        .map(|(entity, ..)| entity)
        .expect("the intact fixture wall has its element collider");
    let force = into * -20.0;
    app.world_mut().resource_mut::<ContactTransfers>().stage(
        crate::contact_transfer::AppliedContact {
            structure: root,
            structure_collider: collider,
            body: source,
            point: face,
            normal: -into,
            force,
            closing_speed: 0.0,
        },
    );
    app.world_mut().resource_mut::<QueuedContacts>().0.clear();
    app.update();
    let queued = &app.world().resource::<QueuedContacts>().0;
    let loads: Vec<_> = queued
        .iter()
        .flatten()
        .filter(|(_, load)| matches!(load.origin, Origin::Body { body, .. } if body == source))
        .collect();
    assert_eq!(
        loads.len(),
        1,
        "the belt press must reach the wall reader exactly once"
    );
    assert_eq!(loads[0].0, root);
    assert_eq!(loads[0].1.blow, Vec3::ZERO);
    assert!((loads[0].1.press + force).length() < 1e-5);
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
                    born: 0,
                    retired: None,
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
                crushed: 0.0,
                blow: Vec3::Z * (mass * 3.0),
                press: Vec3::ZERO,
                peak: None,
                origin: Origin::Body {
                    body: Entity::PLACEHOLDER,
                    mass,
                    speed: 3.0,
                    step: STEP,
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
                crushed: 0.0,
                blow: Vec3::Z * (mass * 3.0),
                press: Vec3::ZERO,
                peak: None,
                origin: Origin::Body {
                    body: Entity::PLACEHOLDER,
                    mass,
                    speed: 3.0,
                    step: STEP,
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
                    step: STEP,
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
        crushed: 0.0,
        blow: Vec3::Z * 0.001,
        press: Vec3::X * 1000.0,
        peak: None,
        origin: Origin::Body {
            body: Entity::PLACEHOLDER,
            mass: 1000.0,
            speed: 3.0,
            step: STEP,
        },
    };
    let held = run_laws(
        &kind.layout.element_of,
        &mut live,
        &[load],
        Vec::new(),
        Vec3::ZERO,
    );
    assert!(
        held.groups.is_empty() && held.returns.is_empty(),
        "a held wall keeps the solver's blow"
    );
    let dust = Load {
        energy: live.state.pool[c] * live.structure.chunks[c].material.crush,
        crushed: live.state.pool[c],
        origin: Origin::Field,
        blow: Vec3::ZERO,
        ..load
    };
    let changes = run_laws(
        &kind.layout.element_of,
        &mut live,
        &[load, dust],
        Vec::new(),
        Vec3::ZERO,
    );
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
    let changes = run_laws(
        &kind.layout.element_of,
        &mut live,
        &[press, dust],
        Vec::new(),
        Vec3::ZERO,
    );
    assert!(
        changes.returns.is_empty(),
        "neither a press nor a field is returned"
    );
    let mut live = Live::new(&kind, 1.0);
    live.state.fate.fill(Fate::Dust);
    live.state.fate[c] = Fate::Standing;
    live.state.cracked.fill(true);
    let changes = run_laws(
        &kind.layout.element_of,
        &mut live,
        &[load],
        Vec::new(),
        Vec3::ZERO,
    );
    assert_eq!(
        changes.groups,
        vec![(vec![c], vec![load])],
        "a freed chunk takes its body load to launch once"
    );
    assert!(
        changes.returns.is_empty(),
        "a fallen load returns only after launch chooses its share"
    );
    let repeated = run_laws(
        &kind.layout.element_of,
        &mut live,
        &[load],
        Vec::new(),
        Vec3::ZERO,
    );
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
        "a later deposit on the departed chunk returns the whole body blow: kept, the static \
         stop would take the source's momentum into no body"
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
            crushed: 0.0,
            blow: Vec3::Z * (source_mass * 3.0),
            press: Vec3::ZERO,
            peak: None,
            origin: Origin::Body {
                body,
                mass: source_mass,
                speed: 3.0,
                step: STEP,
            },
        };
        let departure = |loads: &[Load]| Departure {
            kind: kind.clone(),
            scale: 1.0,
            tick: 0,
            live: Live::new(&kind, 1.0).group(&[c]),
            building: 0,
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
            let live = breakage.live.as_mut();
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

/// DEGENERATE CHUNK HULLS ARE REFUSED BEFORE PLAY (the game's environment destruction model).
/// Launch and collider reconciliation rely on solid baked hulls; four coplanar corners or a
/// nonfinite corner must fail the load gate with the field and fix, rather than panic on contact.
#[test]
fn degenerate_chunk_hulls_are_refused_before_play() {
    let kind = shipped_house();
    let masonry: Vec<Option<Masonry>> = kind
        .asset
        .materials
        .iter()
        .map(|name| {
            Masonry::of(crate::substances::SubstanceRegistry::shipped_static(), name).unwrap()
        })
        .collect();
    let attachments = Attachments::shipped_static();
    let c = kind.layout.chunks[0] as usize;
    for hull in [
        vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
        ],
        vec![[f32::NAN, 0.0, 0.0]; 4],
    ] {
        let mut asset = kind.asset.clone();
        asset.chunks[c].hull = hull;
        let error = Layout::of(&asset, &masonry, attachments).unwrap_err();
        assert!(
            error.contains(&format!("chunks[{c}].hull")) && error.contains("rebake"),
            "{error}"
        );
    }
}

/// A TINY CONTACT KEEPS THE SAME PLASTIC SHARE (the game's environment destruction model).
/// The authority's shared exchange cap must square in f64, not underflow its positive-response
/// check in f32. Scaling the input changes the launch by that scale, including subnormal inputs.
/// Speeds and mass ratio below are ASSUMED fixture data.
#[test]
fn tiny_exchanges_keep_their_plastic_share_without_underflow() {
    let kind = shipped_house();
    let structure = kind.layout.structure(&kind.asset, 1.0);
    let (_, [c, _, _]) = fixture_chunks(&kind);
    let free = launch(&kind, &structure, 1.0, &[c], &[]);
    let com = free.centroid + departure_mass(&structure, &free).center_of_mass;
    let body = Entity::PLACEHOLDER;
    for speed in [3.0, 1e-20, 1e-30, f32::from_bits(1)] {
        let load = Load {
            chunk: c,
            at: com,
            energy: 0.0,
            crushed: 0.0,
            blow: Vec3::Z * speed,
            press: Vec3::ZERO,
            peak: None,
            origin: Origin::Body {
                body,
                mass: 1.0,
                speed,
                step: STEP,
            },
        };
        let mut departures = [Departure {
            kind: kind.clone(),
            scale: 1.0,
            tick: 0,
            live: Live::new(&kind, 1.0).group(&[c]),
            building: 0,
            root: Transform::IDENTITY,
            group: vec![c],
            launch: launch(&kind, &structure, 1.0, &[c], &[load]),
        }];
        cap_exchanges(&mut departures, |_| crate::contact_transfer::BodyInertia {
            centre: com,
            inverse_mass: Vec3::ONE,
            inverse_inertia: Mat3::ZERO,
        });
        let launched = &departures[0].launch;
        assert!(launched.velocity.is_finite() && launched.spin.is_finite());
        let expected = f64::from(speed) / (1.0 + f64::from(launched.mass));
        let error = (f64::from(launched.velocity.z) - expected).abs();
        assert!(
            error <= expected * 2e-5 + f64::from(f32::from_bits(1)),
            "speed {speed}: {error}"
        );
        assert!(
            (launched.returns[0].impulse + launched.velocity * launched.mass - load.blow)
                .as_dvec3()
                .length()
                <= f64::from(speed) * 2e-5 + f64::from(f32::from_bits(1))
        );
    }
}

/// IDENTICAL HELD PRESSES REUSE THEIR VERDICT (the game's environment destruction model).
/// The authority avoids repeating a steady push's strength walk, but a changed point, magnitude,
/// or topology must recheck. Decreasing a stabilizing load cannot reuse the previous verdict.
#[test]
fn held_presses_recheck_changed_force_points_and_topology() {
    let kind = shipped_house();
    let c = kind
        .layout
        .structure(&kind.asset, 1.0)
        .chunks
        .iter()
        .position(|chunk| chunk.buried)
        .expect("the fixture has a soil-held chunk");
    let mut live = Live::new(&kind, 1.0);
    let mut press = Load {
        chunk: c,
        at: live.structure.chunks[c].centroid,
        energy: 0.0,
        crushed: 0.0,
        blow: Vec3::ZERO,
        press: Vec3::X,
        peak: None,
        origin: Origin::Field,
    };
    let run = |live: &mut Live, loads: &[Load]| {
        run_laws(&kind.layout.element_of, live, loads, Vec::new(), Vec3::ZERO)
    };
    assert!(run(&mut live, &[press]).weight_steps > 0);
    assert_eq!(
        run(&mut live, &[press]).weight_steps,
        0,
        "identical press holds"
    );
    press.press *= 0.5;
    assert!(
        run(&mut live, &[press]).weight_steps > 0,
        "even a decrease rechecks"
    );
    assert_eq!(run(&mut live, &[press]).weight_steps, 0);
    let removed = Load {
        press: Vec3::ZERO,
        ..press
    };
    assert!(
        run(&mut live, &[removed]).weight_steps > 0,
        "removal rechecks gravity at the old point"
    );
    assert_eq!(
        run(&mut live, &[removed]).weight_steps,
        0,
        "removal has been checked"
    );
    press.press *= 4.0;
    assert!(run(&mut live, &[press]).weight_steps > 0, "growth rechecks");
    press.at += Vec3::Y * 0.01;
    assert!(run(&mut live, &[press]).weight_steps > 0, "line moves");
    assert_eq!(run(&mut live, &[press]).weight_steps, 0);
    let (_, [other, _, _]) = fixture_chunks(&kind);
    let dust = Load {
        chunk: other,
        at: live.structure.chunks[other].centroid,
        energy: live.state.pool[other] * live.structure.chunks[other].material.crush,
        crushed: live.state.pool[other],
        press: Vec3::ZERO,
        ..press
    };
    assert!(
        run(&mut live, &[press, dust]).weight_steps > 0,
        "topology changes"
    );
    assert_eq!(
        run(&mut live, &[press]).weight_steps,
        0,
        "the topology change already checked the final held verdict"
    );
}

/// A STEP'S PRESSES JOIN EVERY DEPOSIT ONCE (the game's environment destruction model).
/// The authority's strength check needs awake and sleeping presses while processing any load,
/// in either deposit order. Dropping the awake press after its own contact deposit clears the
/// held gate; counting it twice doubles the force. The soil-held fixture isolates that input
/// invariant from fracture, using ASSUMED small forces and a zero-energy crossing.
#[test]
fn awake_and_sleeping_presses_join_every_deposit_once() {
    use bevy::ecs::system::RunSystemOnce;
    for contact_first in [false, true] {
        let (mut app, root) = composed(false);
        let kind = app.world().get::<Breakage>(root).unwrap().kind.clone();
        let structure = kind.layout.structure(&kind.asset, 1.0);
        let c = structure
            .chunks
            .iter()
            .position(|chunk| chunk.buried)
            .unwrap();
        let at = structure.chunks[c].centroid;
        let volume = app
            .world_mut()
            .query::<(Entity, &BuildingElement, &ChildOf)>()
            .iter(app.world())
            .find(|(_, element, parent)| {
                parent.parent() == root && element.0 == kind.layout.element_of[c]
            })
            .unwrap()
            .0;
        let active = Load {
            chunk: c,
            at,
            energy: 0.0,
            crushed: 0.0,
            blow: Vec3::ZERO,
            press: Vec3::X,
            peak: None,
            origin: Origin::Field,
        };
        let sleeping = Load {
            at: at + Vec3::Y * 0.001,
            press: Vec3::X * 2.0,
            ..active
        };
        app.world_mut()
            .resource_mut::<RestingLoads>()
            .0
            .insert(root, vec![sleeping]);
        let contacts = RawDeposit {
            contacts: vec![(root, active)],
            ..default()
        };
        let crossing = RawDeposit {
            crossings: vec![DeliveredLine {
                contacts: vec![(at - Vec3::X * 0.001, at + Vec3::X * 0.001)],
                crossing: crate::ballistics::VolumeCrossing {
                    volume,
                    entry: at - Vec3::X * 0.001,
                    end: at + Vec3::X * 0.001,
                    chord: 0.002,
                    cost: 0.0,
                    energy: 0.0,
                    perforated: true,
                },
                entrance_energy: 0.0,
                momentum: Vec3::ZERO,
                entrance_momentum: Vec3::ZERO,
            }],
            ..default()
        };
        let deposits = if contact_first {
            vec![contacts, crossing]
        } else {
            vec![crossing, contacts]
        };
        app.world_mut().resource_mut::<WallQueue>().0 = deposits;
        app.world_mut().run_system_once(apply_deposits).unwrap();
        let mut breakage = app.world_mut().get_mut::<Breakage>(root).unwrap();
        let live = breakage.live.as_mut();
        let force: Vec3 = live.held_presses.iter().map(|&(_, _, force)| force).sum();
        assert_eq!(force, Vec3::X * 3.0, "contact first: {contact_first}");
        assert_eq!(live.held_presses.len(), 2, "each point once");
    }
}

/// NO ELEMENT'S KIND OR NAME ENTERS THE LAWS (the game's environment destruction model, the #237
/// study's test 8; ADR-0034). A wall chunk and a roof board take one law, from their material rows
/// and their construction: the law's view of the shipped house (its layout and structure) is the
/// same whatever its elements are called and whichever kind each is labelled, so one blast's loads
/// change it identically, while the same field on the house with its elements' materials
/// swapped wears the volume pools differently. A mutant that reads an element's kind or name into the law's view, or
/// branches the laws on either, fails here. The charge and its point are ASSUMED fixture data.
#[test]
fn no_elements_kind_or_name_enters_the_laws() {
    let kind = shipped_house();
    let registry = crate::substances::SubstanceRegistry::shipped_static();
    let masonry: Vec<Option<Masonry>> = kind
        .asset
        .materials
        .iter()
        .map(|name| Masonry::of(registry, name).unwrap())
        .collect();
    let attachments = Attachments::shipped_static();
    let kinds = [
        ElementKind::Pier,
        ElementKind::Sill,
        ElementKind::Lintel,
        ElementKind::Gable,
        ElementKind::WallPlate,
        ElementKind::Roof,
    ];
    let mut relabelled = kind.asset.clone();
    for (k, element) in relabelled.elements.iter_mut().enumerate() {
        let at = kinds.iter().position(|&of| of == element.kind).unwrap();
        element.kind = kinds[(at + 1) % kinds.len()];
        element.name = format!("relabelled/{k}");
    }
    let mut swapped = kind.asset.clone();
    let materials = swapped.materials.len() as u8;
    for element in &mut swapped.elements {
        element.material = (element.material + 1) % materials;
    }
    let (lo, hi) = kind.bounds();
    // ASSUMED fixture charge: the peak exceeds the materials' dynamic crush strengths at the
    // nearest face, so material-dependent wear is exercised without a chemical-energy cavity.
    let blast = Blast::of(CRUSHING_BLAST_J, 0.05, FIT_ONLY_PRODUCTS);
    assert!(
        masonry
            .iter()
            .flatten()
            .all(|material| blast.peak_pressure(1.0, 1.0) > material.compaction.strength)
    );
    let origin = Vec3::new(hi.x + 1.0, lo.y + 1.0, 0.5 * (lo.z + hi.z));
    let run = |asset: &BuildingAsset| {
        let layout = Layout::of(asset, &masonry, attachments).unwrap();
        let structure = layout.structure(asset, 1.0);
        let mut live = Live {
            state: WallState::new(&structure),
            swapped: vec![false; layout.elements.len()],
            sections: SectionCache::default(),
            held_presses: Vec::new(),
            holes: vec![Vec::new(); structure.chunks.len()],
            structure: Arc::new(structure),
        };
        let hits = fracture::sample_blocks(
            &layout.blocks,
            |c| live.state.standing(c),
            |c| live.structure.chunks[c].material,
            origin,
            blast,
            fracture::blast_directions(),
        );
        let contacts: Vec<Joint> = live.state.contacts(&live.structure).collect();
        let (loads, _) = Load::blast(&hits, |c| fracture::Backing {
            scale: 1.0,
            block: &layout.blocks[c],
            material: live.structure.chunks[c].material,
            contacts: &contacts,
        });
        let changes = run_laws(
            &layout.element_of,
            &mut live,
            &loads,
            Vec::new(),
            Vec3::ZERO,
        );
        (layout, live.structure, live.state, changes)
    };
    let (layout, structure, state, changes) = run(&kind.asset);
    assert!(
        !changes.events.is_empty(),
        "the fixture charge changes the house"
    );
    let (other_layout, other_structure, other_state, other_changes) = run(&relabelled);
    assert_eq!(other_layout, layout);
    assert_eq!(
        (&other_structure.chunks, &other_structure.joints),
        (&structure.chunks, &structure.joints)
    );
    assert_eq!(other_state, state, "kinds and names change no pool wear");
    assert_eq!(other_changes, changes, "kinds and names change no law");
    let (_, other_structure, other_state, _) = run(&swapped);
    assert_ne!(other_structure.chunks, structure.chunks);
    assert_ne!(
        other_state.pool, state.pool,
        "a material changes the field's crushed volume"
    );
}

/// A BURST WHOSE TIP TOUCHES A FACE CRUSHES IT FROM THE FILLER CENTRE (the game's environment
/// destruction model). The live shell observer must preserve the authored origin and radius
/// through the deposit queue; a tip-origin field or zero-area contact patch fails here. The
/// contact crater relies on it. The round and house are ASSUMED fixture data; the pass condition
/// is the generic Law 1 threshold, not a particular breach or tank.
#[test]
fn a_burst_touching_a_face_crushes_it_from_its_filler_center() {
    let (mut app, root) = composed(false);
    let kind = shipped_house();
    let (element, [chunk, ..]) = fixture_chunks(&kind);
    let (ahead, axis, _) = through(&kind, element, chunk, 0.0);
    let tip = ahead + axis * 0.5;
    let burst = crate::tank::WeaponRound::resolve("88mm_HE")
        .unwrap()
        .round
        .burst
        .unwrap();
    let field_origin = burst.filler_center(tip, axis);
    let blast = Blast::of(burst.blast, burst.charge_radius, burst.products_momentum());
    assert!(
        blast.peak_pressure(burst.filler_setback, 1.0)
            > kind.layout.materials[chunk].compaction.strength,
        "the fixture field exceeds the material's crush threshold"
    );
    let shell = app
        .world_mut()
        .spawn(PenetrationMarks {
            log: vec![ArmourMark::Burst {
                at: tip,
                field_origin,
                speed: 0.0,
                after: 0.0,
                sideways: burst.sideways_speed,
                nose: burst.nose.mass,
                tank: None,
            }],
            ..default()
        })
        .id();
    app.world_mut().trigger(ShellMarched {
        shell,
        marks_from: 0,
        cones_from: 0,
        lines: Vec::new(),
        burst: Some(burst),
    });
    let queued = app.world().resource::<WallQueue>();
    assert_eq!(queued.0.len(), 1);
    assert_eq!(queued.0[0].blast, Some((field_origin, blast)));
    app.update();
    app.update();
    let live = app.world().get::<Breakage>(root).unwrap().live.as_ref();
    assert!(
        live.state.pool[chunk] < fracture::full_pool(&live.structure.chunks[chunk]),
        "a contacting burst crushes the face above its material threshold"
    );
    app.world_mut().despawn(shell);
}

/// THE GAME'S CASING LOADS CARRY ONE PHYSICAL ENERGY BUDGET AND ITS EJECTION MOMENTUM (study
/// test 5). The environment adapter must not amplify physical transport with the HP field's
/// potency, nor lose a contact ray's direction. Both the wall's volume pool and its launch rely
/// on it. ASSUMED fixture round, angular profiles and stop points; no particular breach is pinned.
#[test]
fn casing_loads_count_each_joule_once_and_carry_their_ejection_momentum() {
    use crate::ballistics::burst::{Burst, BurstInputs, Parts};
    use crate::ballistics::spall_cone::FieldDescriptor;
    use crate::ballistics::{BurstField, RoundBody, SpallRay};
    let body = RoundBody {
        mass: 10.0,
        caliber: 0.088,
        nose_factor: 0.5,
    };
    let inputs = BurstInputs {
        trigger: crate::rounds::fuze::Trigger::Impact {
            sensitivity_n: 30.0,
        },
        delay: 0.0,
        core_mass: 0.06,
        core_density: 1650.0,
        speed_constant: 2680.0,
        chemical_energy: 4.184e6,
        base_mass: 0.33,
        cavity_diameter: 0.0357,
        filler_setback: 0.18,
    };
    let burst = Burst::derived(body, 7800.0, &inputs).unwrap();
    let driven = Parts::of(body, 7800.0, &inputs).unwrap().wall_mass;
    // ASSUMED capacity fixture: finite energy and finite momentum must not overflow the
    // intermediate doubling of the energy. The adapter's datatype gate relies on it.
    let large = Burst {
        energy: f32::MAX * 0.75,
        sideways_speed: 1000.0,
        ..burst
    };
    let field = BurstField {
        origin: Vec3::ZERO,
        axis: Dir3::Z,
        field: FieldDescriptor::ball(0.0),
        strength: 1.0,
        rays: vec![SpallRay {
            direction: Dir3::X,
            end: Vec3::ZERO,
            strength: 1.0,
            doses: Vec::new(),
        }],
    };
    let payload = RayDeposit::casing(&field, &large);
    let expected = (2.0 * f64::from(large.energy) / f64::from(large.sideways_speed)) as f32;
    assert!(payload[0].blow.is_finite());
    assert_eq!(payload[0].blow, Vec3::X * expected);
    for potency in [1.0_f32, 9.0] {
        let field = BurstField {
            origin: Vec3::ZERO,
            axis: Dir3::Z,
            field: FieldDescriptor::ball(0.0),
            strength: burst.energy * potency,
            rays: vec![
                SpallRay {
                    direction: Dir3::X,
                    end: Vec3::ZERO,
                    strength: 2.0 * potency,
                    doses: Vec::new(),
                },
                SpallRay {
                    direction: Dir3::Y,
                    end: Vec3::new(1.0, 2.0, 3.0),
                    strength: 6.0 * potency,
                    doses: Vec::new(),
                },
            ],
        };
        let rays = RayDeposit::casing(&field, &burst);
        let energy: f64 = rays.iter().map(|ray| f64::from(ray.energy)).sum();
        let chemistry = inputs.core_mass * inputs.chemical_energy;
        assert!(
            ((energy + f64::from(burst.blast)) / chemistry - 1.0).abs()
                <= 2.0 * f64::from(f32::EPSILON)
        );
        let momentum: f64 = rays.iter().map(|ray| f64::from(ray.blow.length())).sum();
        assert!(
            (momentum / (driven * f64::from(burst.sideways_speed)) - 1.0).abs()
                <= 2.0 * f64::from(f32::EPSILON)
        );
        assert!(
            rays[0].blow.x > 0.0 && rays[0].end == Vec3::ZERO,
            "a contact stop retains the emitted direction and momentum"
        );
        let (mut app, _) = composed(false);
        let shell = app
            .world_mut()
            .spawn((
                PenetrationMarks::default(),
                SpallMarks {
                    burst: Some(field),
                    ..default()
                },
            ))
            .id();
        app.world_mut().trigger(ShellMarched {
            shell,
            marks_from: 0,
            cones_from: 0,
            lines: Vec::new(),
            burst: Some(burst),
        });
        let queue = app.world().resource::<WallQueue>();
        assert_eq!(queue.0.len(), 1);
        let delivered: Vec3 = queue.0[0].rays.iter().map(|ray| ray.blow).sum();
        let expected: Vec3 = rays.iter().map(|ray| ray.blow).sum();
        assert_eq!(
            delivered, expected,
            "the live shell observer queues the physical blow"
        );
        app.world_mut().despawn(shell);
    }
}

/// Convert building fixture departures to the shared mechanics input.
fn cap_exchanges(
    departures: &mut [Departure],
    inertia: impl Fn(Entity) -> crate::contact_transfer::BodyInertia,
) {
    let mut motions: Vec<_> = departures
        .iter_mut()
        .map(|departure| (&departure.root, &mut departure.launch))
        .collect();
    crate::debris::launch::cap_exchanges(&mut motions, inertia);
}

/// A reproducible shove into the shipped house through the production contact reader and
/// laws, with no view or timing measurement. ASSUMED fixture body: a heavy cuboid at the
/// capture's closing speed. Group counts are evidence, never an asset-specific pass condition,
/// so it pins no law (the weight check's own fixtures do) and runs on demand.
#[test]
#[ignore = "a measurement bed, run on demand"]
fn a_heavy_body_push_reports_the_house_release_groups() {
    let kind = shipped_house();
    let (mut app, _) = composed_keeping_contacts();
    app.insert_resource(Gravity(Vec3::NEG_Y * crate::physical_laws::gravity_mps2()));
    app.world_mut().spawn((
        RigidBody::Static,
        Position(Vec3::new(0.0, -0.5, 0.0)),
        Collider::cuboid(100.0, 1.0, 100.0),
        CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
    ));
    let (lo, hi) = kind.bounds();
    let start = Vec3::new(lo.x - 5.0, 0.8, 0.5 * (lo.z + hi.z));
    let (_, distance, _) =
        fracture::first_hit(&kind.layout.element_blocks, |_| true, start, Vec3::X)
            .expect("the shove fixture faces a wall at hull height");
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Position(start + Vec3::X * (distance - 1.52)),
            Collider::cuboid(3.0, 1.6, 2.6),
            ColliderDensity(47000.0 / (3.0 * 1.6 * 2.6)),
            piece_layers(),
            LinearVelocity(Vec3::X * 1.2),
            ConstantForce(Vec3::X * 350000.0),
        ))
        .id();
    for _ in 0..256 {
        app.update();
    }
    let groups: Vec<usize> = app
        .world_mut()
        .query::<&WallPiece>()
        .iter(app.world())
        .map(|piece| piece.chunks.len())
        .collect();
    assert!(
        app.world()
            .resource::<QueuedContacts>()
            .0
            .iter()
            .any(|step| !step.is_empty()),
        "the fixture body must actually load the shipped wall"
    );
    assert!(app.world().get_entity(body).is_ok());
    println!(
        "MEASURED heavy-body push: {} chunks freed in {} groups, sizes {:?}",
        groups.iter().sum::<usize>(),
        groups.len(),
        groups
    );
}

/// ASSUMED geometry fixture selected by class: three collinear neighbours in one element,
/// sharing two seams and a common extrusion. The data supplies every hull and mass.
fn landing_slab() -> (Arc<BuildingKind>, Vec<usize>) {
    let mut kind = (*shipped_house()).clone();
    let structure = kind.layout.structure(&kind.asset, 1.0);
    let adjacent = |a: usize, b: usize| {
        kind.layout.element_of[a] == kind.layout.element_of[b]
            && (structure.chunks[a].centroid.y - structure.chunks[b].centroid.y).abs() < 0.001
            && !structure.chunks[a].buried
            && !structure.chunks[b].buried
    };
    let mut chosen = None;
    for joint in &structure.joints {
        let Side::Chunk(b) = joint.b else {
            continue;
        };
        if !adjacent(joint.a, b) || joint.normal.z.abs() < 0.999 {
            continue;
        }
        for other in &structure.joints {
            let Side::Chunk(c) = other.b else {
                continue;
            };
            if other.a != b || c == joint.a || !adjacent(b, c) || other.normal.z.abs() < 0.999 {
                continue;
            }
            let mut group = vec![joint.a, b, c];
            group.sort_unstable();
            chosen = Some(group);
            break;
        }
        if chosen.is_some() {
            break;
        }
    }
    let group = chosen.expect("the fixture asset contains a row of three chunks");
    let mut internal = 0;
    for joint in &mut kind.layout.joints {
        if let Side::Chunk(b) = joint.b
            && group.contains(&joint.a)
            && group.contains(&b)
        {
            let mut seam = joint.seam().expect("the row has bonded seams");
            // ASSUMED fixture characterization, fixed before either drop: enough strength for
            // gentle support, lower work and strength on one seam than on its neighbour.
            seam.tensile = if internal == 0 { 1e5 } else { 1e9 };
            seam.fracture_energy = if internal == 0 { 1.0 } else { 1e6 };
            joint.bond = Bond::Bonded(seam);
            internal += 1;
        }
    }
    assert_eq!(internal, 2, "the fixture is a chain, not a loop");
    (Arc::new(kind), group)
}

fn drop_slab(gently: bool) -> (Vec<usize>, Vec<Vec<u16>>) {
    let (mut app, building) = composed(false);
    app.insert_resource(Gravity(Vec3::NEG_Y * 9.81));
    let (kind, group) = landing_slab();
    {
        let mut fixed = app
            .world_mut()
            .get_mut::<Breakage>(building)
            .expect("the fixture's placed root");
        let live = fixed.live.as_mut();
        for &c in &group {
            live.state.fate[c] = Fate::Fallen;
            live.swapped[usize::from(kind.layout.element_of[c])] = true;
        }
    }
    let rotation = if gently {
        Quat::IDENTITY
    } else {
        Quat::from_rotation_x(0.25)
    } * Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
    let low = group
        .iter()
        .flat_map(|&c| kind.layout.blocks[c].points.iter())
        .map(|p| (rotation * *p).y)
        .fold(f32::INFINITY, f32::min);
    let root = Transform::from_xyz(30.0, if gently { 0.005 } else { 3.0 } - low, 0.0)
        .with_rotation(rotation);
    let live = Live::new(&kind, 1.0).group(&group);
    let launch = launch(&kind, &live.structure, 1.0, &group, &[]);
    {
        let world = app.world_mut();
        world.spawn((
            RigidBody::Static,
            Position(Vec3::new(30.0, -0.5, 0.0)),
            Collider::cuboid(40.0, 1.0, 40.0),
            CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
        ));
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, world);
        spawn_group(
            &mut commands,
            Departure {
                kind,
                group: group.clone(),
                root,
                live,
                scale: 1.0,
                tick: 0,
                building: 3,
                launch,
            },
            false,
        );
        queue.apply(world);
    }
    for _ in 0..192 {
        app.update();
    }
    let pieces = app
        .world_mut()
        .query::<&WallPiece>()
        .iter(app.world())
        .map(|piece| piece.chunks.clone())
        .collect();
    (group, pieces)
}

/// Real Avian landing contacts, not an injected law load, must split a falling slab. The two
/// child groups partition the original chunk data; a rigid-only loose path fails this test.
#[test]
fn physics_landing_splits_a_slab_and_gentle_support_does_not() {
    for gently in [false, true] {
        let (group, pieces) = drop_slab(gently);
        let mut owned: Vec<_> = pieces.iter().flatten().map(|&c| usize::from(c)).collect();
        owned.sort_unstable();
        assert_eq!(
            owned, group,
            "split geometry partitions the parent without duplication"
        );
        assert_eq!(
            pieces.len(),
            if gently { 1 } else { 2 },
            "gentle={gently}: {pieces:?}"
        );
    }
}

/// Splitting preserves the occupied collider mass, linear momentum, angular momentum and
/// kinetic energy of the parent's rigid velocity field. No split may manufacture a launch.
#[test]
fn split_children_inherit_the_parents_rigid_momentum() {
    use bevy::ecs::system::RunSystemOnce;
    let (mut app, _) = composed(false);
    let (kind, group) = landing_slab();
    let root = Transform::from_xyz(30.0, 20.0, 0.0).with_rotation(Quat::from_rotation_y(0.7));
    let live = Live::new(&kind, 1.0).group(&group);
    let parent = launch(&kind, &live.structure, 1.0, &group, &[]);
    let centre = root.translation + root.rotation * parent.centre;
    let velocity = Vec3::new(2.0, -3.0, 1.0);
    let spin = Vec3::new(0.3, 0.5, -0.2);
    let inertia = Mat3::from_quat(root.rotation)
        * parent.inertia
        * Mat3::from_quat(root.rotation).transpose();
    let energy = 0.5 * parent.mass * velocity.length_squared() + 0.5 * spin.dot(inertia * spin);
    let total_mass = parent.mass;
    let momentum = parent.mass * velocity;
    let angular = inertia * spin;
    let children = [group[..1].to_vec(), group[1..].to_vec()]
        .into_iter()
        .map(|part| {
            let child = live.group(&part);
            (part, child)
        })
        .collect();
    {
        let world = app.world_mut();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, world);
        spawn_group(
            &mut commands,
            Departure {
                kind: kind.clone(),
                root,
                group,
                live,
                scale: 1.0,
                tick: 0,
                building: 3,
                launch: parent,
            },
            false,
        );
        queue.apply(world);
    }
    let entity = app
        .world_mut()
        .query_filtered::<Entity, With<WallPiece>>()
        .single(app.world())
        .unwrap();
    // The group's exact COM offset is obtained synchronously from its data-built colliders.
    let breakage = app.world().get::<Breakage>(entity).unwrap();
    let exact = launch(
        &kind,
        &breakage.live.as_ref().structure,
        1.0,
        &app.world()
            .get::<WallPiece>(entity)
            .unwrap()
            .chunks
            .iter()
            .map(|&c| usize::from(c))
            .collect::<Vec<_>>(),
        &[],
    );
    let offset = exact.centre - exact.centroid;
    app.world_mut().entity_mut(entity).insert((
        LinearVelocity(velocity),
        AngularVelocity(spin),
        ComputedCenterOfMass(offset),
        PendingSplit { children, tick: 1 },
    ));
    app.world_mut().run_system_once(split_groups).unwrap();
    let mut sum_mass = 0.0;
    let (mut sum_p, mut sum_l, mut sum_e) = (Vec3::ZERO, Vec3::ZERO, 0.0);
    let world = app.world_mut();
    for (piece, pose, rotation, velocity, spin, breakage) in world
        .query::<(
            &WallPiece,
            &Position,
            &Rotation,
            &LinearVelocity,
            &AngularVelocity,
            &Breakage,
        )>()
        .iter(world)
    {
        let group: Vec<_> = piece.chunks.iter().map(|&c| usize::from(c)).collect();
        let geometry = launch(&kind, &breakage.live.as_ref().structure, 1.0, &group, &[]);
        let r = Mat3::from_quat(rotation.0);
        let inertia = r * geometry.inertia * r.transpose();
        let at = pose.0 + rotation.0 * (geometry.centre - geometry.centroid);
        sum_mass += geometry.mass;
        sum_p += geometry.mass * velocity.0;
        sum_l += inertia * spin.0 + (at - centre).cross(geometry.mass * velocity.0);
        sum_e +=
            0.5 * geometry.mass * velocity.0.length_squared() + 0.5 * spin.0.dot(inertia * spin.0);
    }
    assert!((sum_p - momentum).length() <= 1e-4 * momentum.length());
    assert!((sum_l - angular).length() <= 1e-4 * angular.length());
    assert!((sum_e - energy).abs() <= 1e-4 * energy);
    assert!((sum_mass - total_mass).abs() < 1e-5 * total_mass);
}

/// A moving group holds only its own chunks: an unbreakable element stays with the placed
/// building, so a ray through where it stood, in a group's construction frame, meets nothing
/// there. A blast or field ray stopped by it would shade the group's real neighbours. ASSUMED
/// fixture: one shipped element relabelled unbreakable, a group that owns no chunk on the ray.
#[test]
fn an_unbreakable_element_blocks_only_its_placed_building() {
    let kind = shipped_house();
    let (e, _) = fixture_chunks(&kind);
    let mut layout = kind.layout.clone();
    layout.elements[e] = None;
    let block = &layout.element_blocks[e];
    let centre = block.points.iter().copied().sum::<Vec3>() / block.points.len() as f32;
    let origin = centre + Vec3::Y * 100.0;
    let mut shape = Shape::intact(&layout);
    shape.fate.fill(Fate::Dust);
    assert!(matches!(
        first_chunk(&layout, &shape, true, origin, Vec3::NEG_Y),
        Some((None, _))
    ));
    assert_eq!(
        first_chunk(&layout, &shape, false, origin, Vec3::NEG_Y),
        None
    );
}

/// A moving group's field blows reach its body only where their chunk still stands after the
/// laws: a blow on a chunk it turned to dust went into crushing it, as on a standing building,
/// and must not reach the parent velocity its split children inherit. ASSUMED fixture loads.
#[test]
fn a_field_blow_on_a_dusted_chunk_moves_no_moving_group() {
    let kind = shipped_house();
    let (_, [a, b, _]) = fixture_chunks(&kind);
    let live = Live::new(&kind, 1.0);
    let mut state = live.state.clone();
    state.fate[a] = Fate::Dust;
    let blow = |chunk: usize| Load {
        chunk,
        at: live.structure.chunks[chunk].centroid,
        energy: 1.0,
        crushed: 0.0,
        blow: Vec3::X,
        press: Vec3::ZERO,
        peak: None,
        origin: Origin::Field,
    };
    let loads = [blow(a), blow(b)];
    let moved: Vec<usize> = field_blows(&loads, &state).map(|load| load.chunk).collect();
    assert_eq!(moved, vec![b]);
}

/// Median, 95th percentile, mean and maximum of `values` (µs).
fn priced(mut values: Vec<f64>) -> (f64, f64, f64, f64) {
    values.sort_by(f64::total_cmp);
    let at = |q: f64| values[((values.len() - 1) as f64 * q).round() as usize];
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    (at(0.5), at(0.95), mean, values[values.len() - 1])
}

/// A PRICING BED, NOT A GATE (#202): the wall time of the game's environment-destruction weight
/// check on the shipped house (fixture data), game state only. Per change: every standing unburied
/// chunk turned to dust alone, from the intact house, then [`fracture::weigh`] on it. After a
/// long-wall shot: the long wall under the roof's fixed plate turned to dust at once, then the
/// gables, as `a_nailed_roof_distinguishes_rests_from_local_fixing_failure` removes them. Each
/// weigh runs on a clone of the prepared house, so only the check itself is timed. It asserts
/// nothing about time; run it under the `perf` profile with the machine quiet.
#[test]
#[ignore = "a pricing bed, run on demand"]
fn the_weight_check_priced_per_change_and_after_a_long_wall_shot() {
    use std::time::Instant;
    let kind = shipped_house();
    let elements = &kind.asset.elements;
    let of_kind = |kind_of: ElementKind| {
        (0..elements.len())
            .filter(|&e| elements[e].kind == kind_of)
            .collect::<Vec<_>>()
    };
    let base = Live::new(&kind, 1.0);
    let fixed = |joint: &Joint| matches!(joint.bond, Bond::Bonded(_));
    let resting = |joint: &Joint| joint.bond == Bond::Resting;
    let mut roof: Vec<usize> = of_kind(ElementKind::Roof)
        .into_iter()
        .flat_map(|e| kind.layout.chunks_of(e))
        .collect();
    roof.sort_unstable();
    let plane = component_chunks(&kind, elements[of_kind(ElementKind::Roof)[0]].component);
    let plate = of_kind(ElementKind::WallPlate)
        .into_iter()
        .map(|e| elements[e].component)
        .find(|c| joined_to(&kind, &base.structure, &plane, false, fixed).contains(c))
        .expect("the plane is fixed to a wall plate");
    let wall_chunks: Vec<usize> = joined_to(
        &kind,
        &base.structure,
        &component_chunks(&kind, plate),
        true,
        fixed,
    )
    .iter()
    .flat_map(|&c| component_chunks(&kind, c))
    .collect();
    let rests = joined_to(&kind, &base.structure, &roof, true, resting);
    let gables: Vec<usize> = of_kind(ElementKind::Gable)
        .into_iter()
        .filter(|&e| rests.contains(&elements[e].component))
        .flat_map(|e| kind.layout.chunks_of(e))
        .collect();
    let dust = |live: &mut Live, chunks: &[usize]| {
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
        dusted
    };
    let weigh_times = |prepared: &Live, dusted: &[usize], repeats: usize| {
        let mut micros = Vec::with_capacity(repeats);
        let mut steps = 0;
        let mut failures = 0;
        for _ in 0..repeats {
            let mut live = prepared.clone();
            let start = Instant::now();
            let weighed = fracture::weigh(
                &live.structure,
                &mut live.state,
                dusted,
                &[],
                GroupLoads::new(&[], Vec3::ZERO),
                &[],
                &mut live.sections,
            );
            micros.push(start.elapsed().as_secs_f64() * 1e6);
            steps = weighed.steps;
            failures = weighed.failures.len();
            std::hint::black_box(&weighed);
        }
        (micros, steps, failures)
    };

    let candidates: Vec<usize> = (0..base.structure.chunks.len())
        .filter(|&c| base.state.standing(c) && !base.structure.chunks[c].buried)
        .collect();
    let mut medians = Vec::with_capacity(candidates.len());
    let mut steps = Vec::with_capacity(candidates.len());
    for &c in &candidates {
        let mut prepared = base.clone();
        let dusted = dust(&mut prepared, &[c]);
        let (micros, n, _) = weigh_times(&prepared, &dusted, 5);
        medians.push(priced(micros).0);
        steps.push(n as f64);
    }
    let (p50, p95, mean, max) = priced(medians);
    let (s50, s95, smean, smax) = priced(steps);
    println!(
        "MEASURED per change: {} single-chunk removals, weigh µs p50 {p50:.1} p95 {p95:.1} mean \
         {mean:.1} max {max:.1}; steps p50 {s50:.0} p95 {s95:.0} mean {smean:.0} max {smax:.0}",
        candidates.len()
    );

    let mut long = base;
    let dusted = dust(&mut long, &wall_chunks);
    let (micros, n, failures) = weigh_times(&long, &dusted, 25);
    let (p50, p95, mean, max) = priced(micros);
    println!(
        "MEASURED long-wall shot: {} chunks to dust, weigh µs p50 {p50:.1} p95 {p95:.1} mean \
         {mean:.1} max {max:.1}; {n} steps, {failures} failures",
        dusted.len()
    );
    fracture::weigh(
        &long.structure,
        &mut long.state,
        &dusted,
        &[],
        GroupLoads::new(&[], Vec3::ZERO),
        &[],
        &mut long.sections,
    );
    fracture::fall(&long.structure, &mut long.state);
    let dusted = dust(&mut long, &gables);
    let (micros, n, failures) = weigh_times(&long, &dusted, 25);
    let (p50, p95, mean, max) = priced(micros);
    println!(
        "MEASURED gables after it: {} chunks to dust, weigh µs p50 {p50:.1} p95 {p95:.1} mean \
         {mean:.1} max {max:.1}; {n} steps, {failures} failures",
        dusted.len()
    );
}

/// Pricing bed for the game's building destruction model, game state only. One deposit on
/// the shipped construction, with a one-sided patch of externally loaded chunks. Fixture:
/// nearest unburied centroids to the minimum-X face, parallel 10 N s blows and 100 kN peaks,
/// no crushing or cavity energy. Setup and fresh-state cloning are outside the timed deposit;
/// the server's complete `run_laws` path is inside it. No assertion depends on elapsed time.
#[test]
#[ignore = "a pricing bed, run on demand under perf and the machine lock"]
fn a_seeded_blast_deposit_priced_by_loaded_chunks() {
    let kind = shipped_house();
    let base = Live::new(&kind, 1.0);
    let mut candidates: Vec<usize> = (0..base.structure.chunks.len())
        .filter(|&c| !base.structure.chunks[c].buried)
        .collect();
    let centre = base.structure.chunks[candidates[0]].centroid;
    let face = candidates
        .iter()
        .map(|&c| base.structure.chunks[c].centroid.x)
        .min_by(f32::total_cmp)
        .unwrap();
    let source = Vec3::new(face, centre.y, centre.z);
    candidates.sort_by(|&a, &b| {
        base.structure.chunks[a]
            .centroid
            .distance_squared(source)
            .total_cmp(&base.structure.chunks[b].centroid.distance_squared(source))
            .then(a.cmp(&b))
    });
    for n in [10, 50, 200] {
        let loads: Vec<_> = candidates[..n]
            .iter()
            .map(|&chunk| Load {
                chunk,
                at: base.structure.chunks[chunk].centroid,
                energy: 0.0,
                crushed: 0.0,
                blow: Vec3::X * 10.0,
                press: Vec3::ZERO,
                peak: Some(Vec3::X * 100_000.0),
                origin: Origin::Field,
            })
            .collect();
        let mut micros = Vec::new();
        let mut outcome = None;
        // Fixture: seven fresh deposits per size; report distribution, never a time gate.
        for _ in 0..7 {
            let mut live = base.clone();
            let start = std::time::Instant::now();
            let changes = run_laws(
                &kind.layout.element_of,
                &mut live,
                &loads,
                Vec::new(),
                Vec3::ZERO,
            );
            micros.push(start.elapsed().as_secs_f64() * 1e6);
            let observed = (
                changes.failures.len(),
                changes.events.len(),
                changes.groups.len(),
            );
            if let Some(expected) = outcome {
                assert_eq!(observed, expected, "fresh deposits have the same outcome");
            }
            outcome = Some(observed);
            std::hint::black_box(changes);
        }
        let (p50, p95, mean, max) = priced(micros);
        println!(
            "MEASURED seeded deposit: {} chunks, N {n}; us p50 {p50:.1} p95 {p95:.1} \
             mean {mean:.1} max {max:.1}; failures/events/groups {outcome:?}",
            base.structure.chunks.len(),
        );
    }
}

/// A WALL CRUSHES WITH THE ENERGY A CONE CARRIES (the game's environment destruction model, game
/// state only). The production shell reader queues each of a cone's directions at `S / N` joules,
/// so a chunk every direction stops on crushes the energy the cone carries, its round's lost
/// energy times the exit's strength ratio ([`crate::ballistics::spall_cone::field_strength`]),
/// over the chunk's crushing energy density: no HP unit, the cone's or the line's, enters it. A
/// reader that converts a ray's joules through a unit, or drops the `1 / N` share, fails here.
/// Who relies on it: every building a spall cone meets. Fixture data: 2.7 MJ carried at a ratio
/// of 1, `λ` 2, 64 directions, all stopping on one law chunk of the shipped house.
#[test]
fn a_wall_crushes_with_the_energy_a_cone_carries() {
    use crate::ballistics::spall_cone::{AxisRule, ConeShape, FieldDescriptor, field_strength};
    use crate::ballistics::{SpallCone, SpallRay};
    let kind = shipped_house();
    let structure = kind.layout.structure(&kind.asset, 1.0);
    let (_, [c, ..]) = fixture_chunks(&kind);
    let crush = structure.chunks[c].material.crush;
    let carried = 2_700_000.0_f32; // Fixture data: J.
    let strength = field_strength(carried);
    let spread = ConeShape::new(AxisRule::ExitDirection, 64)
        .unwrap()
        .spread(FieldDescriptor::ball(2.0))
        .unwrap();
    let rays = spread
        .directions(Dir3::NEG_Z)
        .map(|(direction, f)| SpallRay {
            direction,
            end: Vec3::ZERO,
            strength: strength * f,
            doses: Vec::new(),
        })
        .collect();
    let cone = SpallCone {
        exited: Vec::new(),
        origin: Vec3::ZERO,
        axis: Dir3::NEG_Z,
        field: spread.field(),
        strength,
        rays,
    };
    let (mut app, _) = composed(false);
    let shell = app
        .world_mut()
        .spawn((
            PenetrationMarks::default(),
            SpallMarks {
                cones: vec![cone],
                ..default()
            },
        ))
        .id();
    app.world_mut().trigger(ShellMarched {
        shell,
        marks_from: 0,
        cones_from: 0,
        lines: Vec::new(),
        burst: None,
    });
    let queue = app.world().resource::<WallQueue>();
    let hits: Vec<_> = queue.0[0]
        .rays
        .iter()
        .map(|ray| (c, ray.end, ray.energy, ray.blow))
        .collect();
    let crushed: f64 = fracture::Load::field(&hits, |_| crush)
        .iter()
        .map(|load| f64::from(load.crushed))
        .sum();
    let want = f64::from(carried) / f64::from(crush);
    assert!(
        (crushed - want).abs() <= 1e-6 * want,
        "crushed {crushed} m³ against {want} m³ for {carried} J at {crush} J/m³"
    );
}

/// A shipped house whose fixture element `e` is two chunks stacked along +Z, `front` over z 0–1 m
/// and `rear` over z 1–2 m, each 4 m by 2 m about the Z axis; every other chunk of it lies far off.
/// Fixture crush energy resolves channels above the record's millimetre quantum.
fn stacked_chunks() -> (App, Entity, BuildingKind, usize, [usize; 2]) {
    let (app, root) = composed(false);
    let mut kind = (*shipped_house()).clone();
    let (e, [front, rear, _]) = fixture_chunks(&kind);
    for c in kind.layout.chunks_of(e) {
        kind.layout.blocks[c] = Block::cuboid(Vec3::splat(100.0), Vec3::splat(101.0));
    }
    kind.layout.blocks[front] = Block::cuboid(Vec3::new(-2.0, -1.0, 0.0), Vec3::new(2.0, 1.0, 1.0));
    kind.layout.blocks[rear] = Block::cuboid(Vec3::new(-2.0, -1.0, 1.0), Vec3::new(2.0, 1.0, 2.0));
    kind.layout.materials[front].crush = 1000.0;
    kind.layout.materials[rear].crush = 1000.0;
    (app, root, kind, e, [front, rear])
}

/// What the shipped shell observer and [`take_lines`] make of a line through element `e` of `kind`,
/// from `a` to `b` in `root`'s frame turned by `rotation`, spending the fixture's 7 J of which 5 J
/// at entry, and delivering `transit` N·s along it and `entry_blow` N·s at entry. `None` when no
/// chunk took the line.
fn marched_line(
    app: &mut App,
    root: Entity,
    kind: &BuildingKind,
    e: usize,
    rotation: Quat,
    (a, b): (Vec3, Vec3),
    (transit, entry_blow): (Vec3, Vec3),
) -> Option<Taken> {
    use crate::ballistics::{DeliveredLine, VolumeCrossing};
    use bevy::ecs::system::RunSystemOnce;
    app.world_mut().get_mut::<Breakage>(root).unwrap().kind = Arc::new(kind.clone());
    let frame = Frame::new(&Transform::from_rotation(rotation), 1.0, kind.bounds());
    let volume = app
        .world_mut()
        .query::<(Entity, &BuildingElement, &ChildOf)>()
        .iter(app.world())
        .find(|(_, index, parent)| usize::from(index.0) == e && parent.parent() == root)
        .map(|(entity, ..)| entity)
        .unwrap();
    let shell = app.world_mut().spawn(PenetrationMarks::default()).id();
    app.world_mut().trigger(ShellMarched {
        shell,
        marks_from: 0,
        cones_from: 0,
        burst: None,
        lines: vec![DeliveredLine {
            // Fixture disc: only exposed rays enter the delivered record.
            contacts: [-0.08, 0.0, 0.08]
                .into_iter()
                .filter_map(|x| {
                    let (from, to) = (a + Vec3::X * x, b + Vec3::X * x);
                    kind.layout
                        .chunks_of(e)
                        .any(|c| {
                            kind.layout.blocks[c]
                                .span(from, (to - from).normalize())
                                .is_some_and(|(lo, hi)| hi.min(from.distance(to)) > lo.max(0.0))
                        })
                        .then_some((rotation * from, rotation * to))
                })
                .collect(),
            crossing: VolumeCrossing {
                volume,
                entry: rotation * a,
                end: rotation * b,
                chord: a.distance(b),
                cost: 0.0,
                energy: 7.0,
                perforated: true,
            },
            momentum: rotation * (entry_blow + transit),
            entrance_momentum: rotation * entry_blow,
            entrance_energy: 5.0,
        }],
    });
    app.world_mut()
        .run_system_once(move |p: DepositParam| {
            let deposit = p
                .queue
                .0
                .last()
                .expect("the shipped shell observer queues its line");
            let mut taken = BTreeMap::new();
            take_lines(&p, &BTreeMap::from([(root, frame)]), deposit, &mut taken);
            taken.remove(&root)
        })
        .unwrap()
}

/// The game's damage model delivers a point exchange to the contacted construction chunk.
/// Bounce consumers rely on no invented chord or dropped work. Fixture: the stacked element's
/// front face receives the helper's point work and impulse, with no transit momentum.
#[test]
fn a_point_exchange_reaches_the_chunk_at_its_contact() {
    let (mut app, root, kind, e, [front, _]) = stacked_chunks();
    let blow = Vec3::X * 2.0;
    let taken = marched_line(
        &mut app,
        root,
        &kind,
        e,
        Quat::IDENTITY,
        (Vec3::ZERO, Vec3::ZERO),
        (Vec3::ZERO, blow),
    )
    .unwrap();
    assert!(
        taken
            .line_loads
            .iter()
            .all(|load| load.chunk == front && load.at == Vec3::ZERO)
    );
    assert!((taken.line_loads.iter().map(|load| load.blow).sum::<Vec3>() - blow).length() < 1e-6);
    assert_eq!(
        taken.line_loads.iter().map(|load| load.energy).sum::<f32>(),
        7.0
    );
}

/// The game's damage model reports a crossing on the disc's axis, so a disc whose rim alone bit an
/// intact element has an axis clear of every chunk. Destruction relies on that line's whole work
/// and momentum wearing the chunk the rim met. Fixture: the axis runs 30 mm clear of one side, past each chunk.
#[test]
fn a_line_only_the_rim_met_wears_the_contacted_chunk() {
    for (near, z) in [(0, 0.1..0.6), (1, 1.4..1.9)] {
        let (mut app, root, kind, e, chunks) = stacked_chunks();
        let (a, b) = (Vec3::new(2.03, 0.0, z.start), Vec3::new(2.03, 0.0, z.end));
        let (transit, entry_blow) = (Vec3::Z * 30.0, Vec3::X * 12.0);
        let taken = marched_line(
            &mut app,
            root,
            &kind,
            e,
            Quat::from_rotation_y(0.7),
            (a, b),
            (transit, entry_blow),
        )
        .expect("the element's chunk takes the rim's line");
        let &[(c, from, to, energy)] = taken.lines.as_slice() else {
            panic!("one chunk takes the whole line: {:?}", taken.lines);
        };
        assert_eq!(c, chunks[near]);
        assert_eq!(energy, 7.0);
        assert!(from.distance(a) < 1e-4 && to.distance(b) < 1e-4);
        assert!(taken.line_loads.iter().all(|load| load.chunk == c));
        let total: Vec3 = taken.line_loads.iter().map(|load| load.blow).sum();
        assert!((total - transit - entry_blow).length() < 1e-4);
        let work: f32 = taken.line_loads.iter().map(|load| load.energy).sum();
        assert!((work - 7.0).abs() < 1e-5);
    }
}

/// The game's damage model allocates a disc's work and momentum over every chunk it met.
/// Fixture: equal-depth chunks and a disc straddling their side; crossing that side with the
/// axis must preserve the split, which the destruction laws rely on for continuous wear.
#[test]
fn a_rim_crossing_shares_the_work_across_the_chunks_it_met() {
    for x in [1.97, 1.9999, 2.0001, 2.03] {
        let (mut app, root, kind, e, [front, rear]) = stacked_chunks();
        let taken = marched_line(
            &mut app,
            root,
            &kind,
            e,
            Quat::IDENTITY,
            (Vec3::new(x, 0.0, 0.1), Vec3::new(x, 0.0, 1.9)),
            (Vec3::Z * 30.0, Vec3::X * 12.0),
        )
        .unwrap();
        let work = |c| {
            taken
                .line_loads
                .iter()
                .filter(|load| load.chunk == c)
                .map(|load| load.energy)
                .sum::<f32>()
        };
        let blow = |c| {
            taken
                .line_loads
                .iter()
                .filter(|load| load.chunk == c)
                .map(|load| load.blow)
                .sum::<Vec3>()
        };
        assert!(
            (work(front) - 6.0).abs() < 1e-4,
            "axis {x}: front {} J",
            work(front)
        );
        assert!(
            (work(rear) - 1.0).abs() < 1e-4,
            "axis {x}: rear {} J",
            work(rear)
        );
        assert!((blow(front) - Vec3::Z * 15.0 - Vec3::X * 12.0).length() < 1e-4);
        assert!((blow(rear) - Vec3::Z * 15.0).length() < 1e-4);
    }
}

/// The game's damage model: a skid does all its work at the face, so its line has no chord and
/// names no chunk. Destruction relies on the chunk at its point taking the work and blow whole,
/// with no panic on the authority's per-round path.
#[test]
fn a_skids_line_with_no_chord_wears_the_chunk_at_its_point() {
    for (near, z) in [(0, 0.1), (1, 1.9)] {
        let (mut app, root, kind, e, chunks) = stacked_chunks();
        let at = Vec3::new(1.9, 0.0, z);
        let entry_blow = Vec3::X * 12.0;
        let taken = marched_line(
            &mut app,
            root,
            &kind,
            e,
            Quat::IDENTITY,
            (at, at),
            (Vec3::ZERO, entry_blow),
        )
        .expect("the chunk at the skid's point takes its line");
        assert!(
            taken
                .line_loads
                .iter()
                .all(|load| load.chunk == chunks[near]),
            "{:?}",
            taken.line_loads
        );
        let total: Vec3 = taken.line_loads.iter().map(|load| load.blow).sum();
        assert!((total - entry_blow).length() < 1e-4);
        let work: f32 = taken.line_loads.iter().map(|load| load.energy).sum();
        assert!((work - 7.0).abs() < 1e-5);
    }
}

/// The game's damage model preserves work when world rounding collapses a resolved axis.
/// Construction reads a surviving sample chord first; only a point without one uses its location.
/// Fixture: at z = 1000 m, the 1000.00001 m stop rounds to entry, beside a lateral chunk seam.
#[test]
fn a_far_collapsed_axis_reads_surviving_contacts_before_its_point() {
    use crate::ballistics::{DeliveredLine, VolumeCrossing};
    use bevy::ecs::system::RunSystemOnce;
    for contacted in [false, true] {
        let (mut app, root, mut kind, e, [left, right]) = stacked_chunks();
        kind.layout.blocks[left] =
            Block::cuboid(Vec3::new(-2.0, -1.0, 0.0), Vec3::new(0.0, 1.0, 1.0));
        kind.layout.blocks[right] =
            Block::cuboid(Vec3::new(0.0, -1.0, 0.0), Vec3::new(2.0, 1.0, 1.0));
        app.world_mut().get_mut::<Breakage>(root).unwrap().kind = Arc::new(kind.clone());
        let volume = app
            .world_mut()
            .query::<(Entity, &BuildingElement, &ChildOf)>()
            .iter(app.world())
            .find(|(_, index, parent)| usize::from(index.0) == e && parent.parent() == root)
            .map(|(entity, ..)| entity)
            .unwrap();
        let entry = Vec3::new(0.02, 0.0, 1000.0);
        let end = Vec3::new(0.02, 0.0, 1000.00001_f64 as f32);
        assert_eq!(
            entry, end,
            "fixture must lose the resolved axis chord to world rounding"
        );
        let contacts = if contacted {
            vec![(
                Vec3::new(-0.02, 0.0, 1000.0),
                Vec3::new(-0.02, 0.0, 1000.05),
            )]
        } else {
            vec![(entry, end)]
        };
        let blow = Vec3::Z * 30.0 + Vec3::X * 12.0;
        let deposit = RawDeposit {
            crossings: vec![DeliveredLine {
                crossing: VolumeCrossing {
                    volume,
                    entry,
                    end,
                    chord: 0.00001,
                    cost: 0.0,
                    energy: 7.0,
                    perforated: false,
                },
                contacts,
                momentum: blow,
                entrance_energy: 5.0,
                entrance_momentum: Vec3::X * 12.0,
            }],
            ..default()
        };
        let frame = Frame::new(&Transform::from_xyz(0.0, 0.0, 1000.0), 1.0, kind.bounds());
        let taken = app
            .world_mut()
            .run_system_once(move |p: DepositParam| {
                let mut taken = BTreeMap::new();
                take_lines(&p, &BTreeMap::from([(root, frame)]), &deposit, &mut taken);
                taken
                    .remove(&root)
                    .expect("a collapsed line retains its exchange")
            })
            .unwrap();
        let owner = if contacted { left } else { right };
        assert!(
            taken.line_loads.iter().all(|load| load.chunk == owner),
            "contacted {contacted}: {:?}",
            taken.line_loads
        );
        assert!((taken.line_loads.iter().map(|load| load.energy).sum::<f32>() - 7.0).abs() < 1e-5);
        assert!(
            (taken.line_loads.iter().map(|load| load.blow).sum::<Vec3>() - blow).length() < 1e-4
        );
    }
}

/// An element that never breaks owns no chunk ([`Layout::chunks_of`]) and still has its collider,
/// so a round crossing it is routine. Destruction relies on that line wearing nothing and ending
/// no session: a mutant that restores the allocation panic fails here.
#[test]
fn a_line_through_an_element_that_never_breaks_wears_nothing() {
    let (mut app, root, mut kind, e, _) = stacked_chunks();
    kind.layout.elements[e] = None;
    let taken = marched_line(
        &mut app,
        root,
        &kind,
        e,
        Quat::IDENTITY,
        (Vec3::new(0.0, 0.0, -0.1), Vec3::new(0.0, 0.0, 2.1)),
        (Vec3::Z * 30.0, Vec3::X * 12.0),
    );
    assert!(taken.is_none());
}

/// The game's damage model sends a nonzero line through the shipped message reader and chunk
/// allocator. Destruction relies on the carrier staying at entry and the core following its chord.
/// Fixture: equal-depth front and rear chunks, head-on and oblique paths, rotated in world;
/// 5 J stop at entry and the core's 2 J divide equally. Momentum has its own independent budget.
#[test]
fn a_marched_line_keeps_carrier_work_and_bend_at_entry() {
    for slope in [0.0, 0.4] {
        let (mut app, root, kind, e, [front, rear]) = stacked_chunks();
        let a = Vec3::new(-0.1 * slope, 0.0, -0.1);
        let b = Vec3::new(2.1 * slope, 0.0, 2.1);
        let dir = (b - a).normalize();
        let entry_axis = dir
            .cross(if dir.x.abs() < 0.9 { Vec3::X } else { Vec3::Y })
            .normalize();
        let entry_blow = entry_axis * 12.0;
        let transit_blow = dir * 30.0;
        let taken = marched_line(
            &mut app,
            root,
            &kind,
            e,
            Quat::from_rotation_y(0.7),
            (a, b),
            (transit_blow, entry_blow),
        )
        .unwrap();
        assert_eq!(
            taken
                .lines
                .iter()
                .map(|line| line.0)
                .collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from([front, rear])
        );
        let holes = holes_of(&kind, 1.0, &taken.lines, &[]);
        assert_eq!(holes.len(), 2, "both crossed chunks keep a visual channel");
        for &(c, from, to, _) in &taken.lines {
            let energy = if c == front { 6.0 } else { 1.0 };
            let volume = energy / kind.layout.materials[c].crush;
            let radius = (volume / (std::f32::consts::PI * from.distance(to)))
                .sqrt()
                .min((0.75 * volume / std::f32::consts::PI).cbrt());
            let hole = holes.iter().find(|(chunk, _)| *chunk == c).unwrap().1;
            assert_eq!(
                hole.radius,
                (radius * 1000.0).round() as u16,
                "the game's damage-model view must include the entrance work in its first chunk's hole"
            );
        }
        let loads = taken.line_loads;
        let total: Vec3 = loads.iter().map(|load| load.blow).sum();
        assert!((total - entry_blow - transit_blow).length() < 1e-4);
        assert!((loads.iter().map(|load| f64::from(load.energy)).sum::<f64>() - 7.0).abs() < 1e-5);
        let first_chunk = loads
            .iter()
            .min_by(|a, b| a.at.dot(dir).total_cmp(&b.at.dot(dir)))
            .unwrap()
            .chunk;
        let mut chunks = BTreeMap::<usize, Vec3>::new();
        let mut energies = BTreeMap::<usize, f32>::new();
        for load in loads {
            *energies.entry(load.chunk).or_default() += load.energy;
            *chunks.entry(load.chunk).or_default() += load.blow;
        }
        assert!(
            chunks.len() > 1,
            "the fixture exercises allocation across chunks"
        );
        for (chunk, energy) in energies {
            let expected = if chunk == first_chunk { 6.0 } else { 1.0 };
            assert!(
                (energy - expected).abs() < 1e-4,
                "slope {slope}, chunk {chunk}: retained {energy} J, expected {expected} J"
            );
        }
        for (chunk, blow) in chunks {
            let bend = blow.dot(entry_axis);
            assert!((bend - if chunk == first_chunk { 12.0 } else { 0.0 }).abs() < 1e-4);
        }
    }
}

/// `group` freed where it stood in a composed house `offset` from the origin, its neighbours still
/// standing and gravity on (fixture 9.81 m/s²): the app, and the body the launch expects, as mass,
/// inertia and centre about the collider frame.
fn freed_in_place(group: &[usize], offset: Vec3) -> (App, (f32, Mat3, Vec3)) {
    let kind = shipped_house();
    let (mut app, root) = composed(false);
    app.insert_resource(Gravity(Vec3::NEG_Y * 9.81));
    app.world_mut()
        .get_mut::<Transform>(root)
        .unwrap()
        .translation = offset;
    for _ in 0..2 {
        app.update();
    }
    let live = {
        let mut fixed = app.world_mut().get_mut::<Breakage>(root).unwrap();
        let live = fixed.live.as_mut();
        for &c in group {
            live.state.fate[c] = Fate::Fallen;
            live.swapped[usize::from(kind.layout.element_of[c])] = true;
        }
        live.clone()
    };
    for _ in 0..2 {
        app.update();
    }
    let piece_live = live.group(group);
    let launched = launch(&kind, &piece_live.structure, 1.0, group, &[]);
    let body = (
        launched.mass,
        launched.inertia,
        launched.centre - launched.centroid,
    );
    let world = app.world_mut();
    let mut queue = bevy::ecs::world::CommandQueue::default();
    let mut commands = Commands::new(&mut queue, world);
    spawn_group(
        &mut commands,
        Departure {
            kind,
            group: group.to_vec(),
            root: Transform::from_translation(offset),
            live: piece_live,
            scale: 1.0,
            tick: 0,
            building: 3,
            launch: launched,
        },
        false,
    );
    queue.apply(world);
    (app, body)
}

/// A FREED CHUNK MOVES AS THE MATERIAL IT OCCUPIES (the game's environment destruction model). The
/// launch holds the mass the structure's weight law carried, and the spawned body holds the
/// launch's mass, inertia and centre, so a blow leaves the velocity its load computed.
/// The two chunks are fixture data.
#[test]
fn a_freed_chunk_moves_as_the_material_it_occupies() {
    let kind = shipped_house();
    let structure = kind.layout.structure(&kind.asset, 1.0);
    for c in 0..kind.layout.chunks.len() {
        let mass = launch(&kind, &structure, 1.0, &[c], &[]).mass;
        let ratio = mass / structure.chunks[c].mass;
        assert!(
            (ratio - 1.0).abs() < 1e-3,
            "chunk {c}: launch/structure mass {ratio}"
        );
    }
    let (_, [c, other, _]) = fixture_chunks(&kind);
    let (mut app, (mass, inertia, centre)) = freed_in_place(&[c, other], Vec3::ZERO);
    app.update();
    let mut bodies = app.world_mut().query_filtered::<(
        &ComputedMass,
        &ComputedAngularInertia,
        &ComputedCenterOfMass,
    ), With<WallPiece>>();
    let (body_mass, body_inertia, body_centre) = bodies.single(app.world()).unwrap();
    // Resolution: relative 1e-5 exceeds f32 aggregation error but catches a unit-density solid
    // accidentally contributing mass beside the occupied contact properties.
    assert!((body_mass.value() / mass - 1.0).abs() < 1e-5);
    let largest = |m: Mat3| {
        m.to_cols_array()
            .map(f32::abs)
            .into_iter()
            .fold(0.0, f32::max)
    };
    assert!(largest(body_inertia.tensor().to_mat3() - inertia) < 1e-3 * largest(inertia));
    assert!((body_centre.0 - centre).length() < 1e-3);
}

/// A CHUNK FREED IN PLACE TAKES NO ENERGY FROM ITS NEIGHBOURS (the game's environment destruction
/// model). It touched them standing, so the first contact must not resolve that touch as a blow:
/// kinetic energy stays under gravity's work on it, tick by tick, at the origin and where f32
/// rounding of a position is coarse. Contact on the whole hull fails: MEASURED +0.22 J/kg and a
/// 5.5 cm kick. Fixture: the middle chunk of at most eight sampled breakable elements.
#[test]
fn a_chunk_freed_in_place_takes_no_energy_from_its_neighbours() {
    let kind = shipped_house();
    let layout = &kind.layout;
    let structure = layout.structure(&kind.asset, 1.0);
    let elements: Vec<usize> = (0..layout.elements.len())
        .filter(|&e| !layout.chunks_of(e).is_empty())
        .collect();
    for e in elements.iter().step_by(elements.len().div_ceil(8)) {
        let chunks = layout.chunks_of(*e);
        let c = chunks.start + chunks.len() / 2;
        // No release frees a chunk the soil holds.
        if structure.chunks[c].buried {
            continue;
        }
        for offset in [Vec3::ZERO, Vec3::splat(1000.0)] {
            let (mut app, (mass, inertia, _)) = freed_in_place(&[c], offset);
            let start_height = offset.y + structure.chunks[c].centroid.y;
            for tick in 0..128 {
                app.update();
                let mut bodies = app.world_mut().query_filtered::<(
                    &Position,
                    &Rotation,
                    &LinearVelocity,
                    &AngularVelocity,
                ), With<WallPiece>>();
                let (at, turn, velocity, spin) = bodies
                    .single(app.world())
                    .unwrap_or_else(|e| panic!("chunk {c} at {offset}, tick {tick}: {e}"));
                let turn = Mat3::from_quat(turn.0);
                let kinetic = 0.5 * mass * velocity.0.length_squared()
                    + 0.5 * spin.0.dot(turn * inertia * turn.transpose() * spin.0);
                // The height's own f32 rounding bounds how well gravity's work is known.
                let rounding = f32::EPSILON * start_height.abs().max(1.0);
                let work = mass * 9.81 * (start_height - at.0.y + rounding);
                assert!(
                    kinetic <= work,
                    "chunk {c} at {offset}, tick {tick}: {kinetic} J from {work} J of gravity"
                );
            }
        }
    }
}

/// The game's damage model marches a freed chunk's occupied hull. Contact separation must not
/// shorten either face of its ballistic chord. This reads the collider spawn_group actually
/// attached to a live piece, so retaining BallisticVolume on the contact inset fails.
#[test]
fn a_freed_chunks_ballistic_chord_is_its_occupied_chord() {
    let kind = shipped_house();
    let (_, [c, ..]) = fixture_chunks(&kind);
    let (mut app, _) = freed_in_place(&[c], Vec3::ZERO);
    app.insert_resource(Gravity::ZERO);
    app.update();
    let origin = app
        .world_mut()
        .query_filtered::<&Breakage, With<WallPiece>>()
        .single(app.world())
        .unwrap()
        .origin;
    let occupied = chunk_collider(&kind, c, 1.0, origin);
    let centroid = kind.layout.structure(&kind.asset, 1.0).chunks[c].centroid - origin;
    let mut volumes = app
        .world_mut()
        .query::<(&Collider, &BallisticVolume, &ChildOf)>();
    let colliders: Vec<_> = volumes
        .iter(app.world())
        .filter(|(_, _, parent)| app.world().get::<WallPiece>(parent.parent()).is_some())
        .map(|(hull, _, _)| hull)
        .collect();
    assert_eq!(colliders.len(), 1);
    // Fixture rays in three independent directions, long enough to cross this chunk.
    for dir in [Vec3::X, Vec3::Y, Vec3::Z] {
        let reach = (kind.layout.blocks[c].max - kind.layout.blocks[c].min).length();
        for sign in [-1.0, 1.0] {
            let ray = dir * sign;
            let from = centroid - ray * reach;
            let cast = |hull: &Collider| {
                hull.cast_ray(Vec3::ZERO, Quat::IDENTITY, from, ray, reach * 2.0, true)
                    .unwrap()
                    .0
            };
            assert!(
                (cast(colliders[0]) - cast(&occupied)).abs() < 1e-5,
                "contact separation changed the game's ballistic face on {ray}"
            );
        }
    }
}

/// The game's damage model cannot silently discard a malformed empty exit field.
#[test]
#[should_panic(expected = "a recorded cone has its admitted nonempty direction set")]
fn construction_refuses_a_recorded_cone_without_its_direction_set() {
    let (mut app, _) = composed(false);
    let shell = app
        .world_mut()
        .spawn((
            PenetrationMarks::default(),
            crate::ballistics::SpallMarks {
                cones: vec![crate::ballistics::SpallCone {
                    exited: Vec::new(),
                    origin: Vec3::ZERO,
                    axis: Dir3::Z,
                    field: crate::ballistics::spall_cone::FieldDescriptor::ball(0.0),
                    strength: 1.0,
                    rays: Vec::new(),
                }],
                burst: None,
            },
        ))
        .id();
    app.world_mut().trigger(ShellMarched {
        shell,
        marks_from: 0,
        cones_from: 0,
        burst: None,
        lines: Vec::new(),
    });
}

/// The game's environment destruction assigns occupied mass once. This control removes the
/// sensor marker from the actual spawned solid; a missing marker at spawn fails the mass guard.
#[test]
fn a_ballistic_sensor_does_not_add_a_second_occupied_mass() {
    let kind = shipped_house();
    let (_, [c, ..]) = fixture_chunks(&kind);
    let (mut app, (mass, _, _)) = freed_in_place(&[c], Vec3::ZERO);
    app.update();
    let (solid, parent) = app
        .world_mut()
        .query_filtered::<(Entity, &ChildOf), (With<Sensor>, With<BallisticVolume>)>()
        .iter(app.world())
        .map(|(e, parent)| (e, parent.parent()))
        .find(|(_, parent)| app.world().get::<WallPiece>(*parent).is_some())
        .unwrap();
    assert!((app.world().get::<ComputedMass>(parent).unwrap().value() / mass - 1.0).abs() < 1e-5);
    let extra = app
        .world()
        .get::<Collider>(solid)
        .unwrap()
        .mass_properties(1.0)
        .mass;
    app.world_mut().entity_mut(solid).remove::<Sensor>();
    app.world_mut().entity_mut(parent).insert(
        avian3d::dynamics::rigid_body::mass_properties::components::RecomputeMassProperties,
    );
    app.update();
    let added = app.world().get::<ComputedMass>(parent).unwrap().value() - mass;
    assert!(
        added > extra * 0.9,
        "the no-sensor control must expose the duplicate mass: {added} of {extra}"
    );
}
