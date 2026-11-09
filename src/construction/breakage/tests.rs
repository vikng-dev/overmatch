//! The breakage invariants, about the game's environment destruction (game state only), on the
//! shipped construction and minimal generic fixtures: which chunk a test reads is chosen by a class rule (a breakable
//! element's chunk nearest its centre, a chunk with no anchor), never by its number.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::time::TimeUpdateStrategy;

use super::*;
use crate::ballistics::MarchProbe;
use crate::construction::fracture::FIT_ONLY_PRODUCTS;
use crate::construction::tests::{column_construction, shipped_house};
use crate::scatter::MapInstance;

/// A fixture physics step, s: the harness's 64 Hz tick (fixture data).
const STEP: f32 = 1.0 / 64.0;

/// Fixture: the source cube's side, m.
const CUBE_SIDE_M: f32 = 0.4;

/// Fixture: a dynamic cube at rest, with principal inertia equal to its mass in kg m².
fn cube(start: Vec3, mass: f32) -> impl Bundle {
    (
        RigidBody::Dynamic,
        Position(start),
        Rotation::default(),
        Collider::cuboid(CUBE_SIDE_M, CUBE_SIDE_M, CUBE_SIDE_M),
        Mass(mass),
        NoAutoMass,
        AngularInertia::new(Vec3::splat(mass)),
        CenterOfMass::default(),
        LinearVelocity::ZERO,
        piece_layers(),
    )
}

/// The breakage sim on the offline physics with the solver's contact reading, the march probe,
/// and the shipped house spawned at the origin (unit scale), one fixed tick per update. `replica`
/// adds the replica marker.
fn composed(replica: bool) -> (App, Entity) {
    composed_kind(replica, &shipped_house())
}

fn composed_kind(replica: bool, kind: &Arc<ConstructionKind>) -> (App, Entity) {
    composed_kind_with_physics(replica, kind, crate::offline_physics())
}

fn composed_kind_with_physics(
    replica: bool,
    kind: &Arc<ConstructionKind>,
    physics: bevy::app::PluginGroupBuilder,
) -> (App, Entity) {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::transform::TransformPlugin,
        AssetPlugin::default(),
        physics,
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
    let root = crate::construction::spawn(
        &mut commands,
        kind,
        MapInstance {
            id: "house_0001".to_owned(),
            ordinal: 3,
        },
        Isometry3d::IDENTITY,
    );
    queue.apply(world);
    (app, root)
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
            .query::<(Entity, &ChildOf, Option<&ChunkCollider>, &BallisticVolume)>()
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
        .query::<(Entity, &Piece)>()
        .single(app.world())
        .unwrap();
    assert_eq!(
        piece.chunks,
        vec![1, 2],
        "source addresses survive departure"
    );
    assert_eq!(piece.construction, construction);
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
        .query::<(Entity, &Piece)>()
        .single(app.world())
        .unwrap();
    assert_eq!(
        piece.chunks,
        vec![2],
        "splitting never compacts source addresses"
    );
    assert_eq!(piece.construction, construction);
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

    let record = app.world().resource::<Record>().clone();
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
        let mut record = replica.world_mut().resource_mut::<Record>();
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

/// Fixture support loss: sever every bond joining `group` to other matter or the ground and
/// request a zero-work evaluation on its first chunk.
fn isolate(app: &mut App, root: Entity, group: &[usize]) {
    let chunk = group[0];
    let at = {
        let mut body = app.world_mut().get_mut::<Breakage>(root).unwrap();
        let live = &mut body.live;
        for (index, joint) in live.structure.joints.iter().enumerate() {
            let inside = match joint.b {
                Side::Chunk(b) => group.contains(&b),
                Side::Ground => false,
            };
            if group.contains(&joint.a) != inside {
                live.state.cracked[index] = true;
                live.state.separated[index] = true;
            }
        }
        live.structure.chunks[chunk].centroid
    };
    app.world_mut().resource_mut::<Queue>().0.push(RawDeposit {
        contacts: vec![(
            root,
            Load {
                chunk,
                at,
                energy: 0.0,
                crushed: 0.0,
                blow: Vec3::ZERO,
                press: Vec3::ZERO,
                peak: None,
                origin: Origin::Field,
            },
        )],
        ..default()
    });
}

/// Every construction kind the shipped map admits, by asset.
fn admitted_kinds() -> Vec<(String, Arc<ConstructionKind>)> {
    let manifest = crate::map::tests::shipped_manifest();
    let assets: BTreeSet<_> = manifest
        .prototypes
        .values()
        .filter_map(|prototype| match prototype {
            crate::map::Prototype::Construction { asset } => Some(asset.clone()),
            _ => None,
        })
        .collect();
    assets
        .into_iter()
        .map(|asset| {
            let kind = manifest.constructions.get(&asset).unwrap().clone();
            (asset, kind)
        })
        .collect()
}

/// The game's environment destruction: on every admitted kind, each chunk bears by the grain its
/// element declares, laid along that element's axis, and a chunk of an element that declares
/// none bears alike every way. The bearing law (`fracture::Material::bearing`) reads no other
/// grain, so a layout that drops or crosses an element's frame fails here.
#[test]
fn admitted_kinds_lay_each_chunks_grain_along_its_elements() {
    use crate::construction::fracture::Grain;
    use crate::construction::schema::SurfaceFrame;
    let mut grained = 0;
    for (asset, kind) in admitted_kinds() {
        for (c, material) in kind.layout.materials.iter().enumerate() {
            let element = &kind.asset.elements[usize::from(kind.layout.element_of[c])];
            match (element.surface_frame, material.grain) {
                (SurfaceFrame::Upright, Grain::None) => {}
                (SurfaceFrame::Grain(axis), Grain::Laid { axis: laid, .. }) => {
                    grained += 1;
                    assert!(
                        laid.dot(Vec3::from_array(axis)) > 1.0 - 1e-6,
                        "{asset}: chunk {c} bears along {laid}, its element's grain {axis:?}"
                    );
                }
                (frame, grain) => panic!("{asset}: chunk {c} framed {frame:?} bears by {grain:?}"),
            }
        }
    }
    assert!(grained > 0, "an admitted kind holds grained matter");
}

/// Spawns prepared departures as the departure boundary does, through one command queue.
fn spawn_departures(app: &mut App, departures: Vec<PreparedDeparture>) {
    let mut queue = bevy::ecs::world::CommandQueue::default();
    let mut commands = Commands::new(&mut queue, app.world());
    for departure in departures {
        spawn_prepared(&mut commands, departure);
    }
    queue.apply(app.world_mut());
}

/// Above-ground matter of `structure` grown from its first above-ground chunk across bonded
/// joints, ascending, until it stays a server body: the fixture's choice by class. `None` when
/// its connected matter never reaches one.
fn resolved_group(structure: &Structure) -> Option<Vec<usize>> {
    let first = structure
        .chunks
        .iter()
        .position(|chunk| !chunk.buried)
        .expect("the caller found above-ground matter");
    let (mut group, mut frontier) = (Vec::new(), BTreeSet::from([first]));
    while let Some(c) = frontier.pop_first() {
        group.push(c);
        group.sort_unstable();
        if crate::debris::crumble::server_body(structure, &group) {
            return Some(group);
        }
        for joint in &structure.joints {
            let Side::Chunk(b) = joint.b else { continue }; // The ground grows no group.
            let other = if joint.a == c {
                b
            } else if b == c {
                joint.a
            } else {
                continue;
            };
            if joint.seam().is_some()
                && !structure.chunks[other].buried
                && group.binary_search(&other).is_err()
            {
                frontier.insert(other);
            }
        }
    }
    None
}

/// The game's environment destruction routes unsupported matter to centroid-local server bodies
/// or to decor and the dust ledger. The Driver and presentation rely on these contracts for every
/// admitted kind whose data reaches that route; fixture constructions guarantee both routes.
/// A mutant dropping a body's local colliders or a decoration's dust record fails here.
#[test]
fn admitted_map_kinds_detach_with_common_centroid_local_colliders() {
    let floor = crate::physical_laws::PhysicalLaws::shipped().server_rubble_min_size_m;
    // Fixture: each large cube exceeds the floor; the entire small column stays below it.
    let body = scaled_column(2.0 * floor);
    let decoration = scaled_column(0.25 * floor);
    assert!(resolved_group(&body.structure).is_some());
    assert!(resolved_group(&decoration.structure).is_none());
    let mut kinds = vec![
        ("server_body_fixture".to_owned(), body),
        ("decoration_fixture".to_owned(), decoration),
    ];
    kinds.extend(admitted_kinds());
    for (asset, kind) in kinds {
        let kind = &kind;
        let structure = &kind.structure;
        if structure.chunks.iter().all(|chunk| chunk.buried) {
            continue; // A valid construction can have no above-ground matter to release.
        }
        let group = resolved_group(structure);
        let start = || {
            let (mut app, root) = composed_kind(false, kind);
            app.init_resource::<Sheds>()
                .add_systems(FixedPostUpdate, keep_sheds.after(LawSet));
            app.update();
            app.update();
            (app, root)
        };
        let decor = (0..structure.chunks.len()).find(|&c| {
            !structure.chunks[c].buried && !crate::debris::crumble::server_body(structure, &[c])
        });
        if let Some(chunk) = decor {
            let (mut app, root) = start();
            isolate(&mut app, root, &[chunk]);
            app.update();
            assert_eq!(
                app.world().get::<Breakage>(root).unwrap().live.state.fate[chunk],
                Fate::Dust
            );
            assert!(
                app.world()
                    .resource::<Sheds>()
                    .0
                    .iter()
                    .any(|shed| shed.chunks.contains(&(chunk as u16)))
            );
            assert!(
                !app.world_mut()
                    .query::<&Piece>()
                    .iter(app.world())
                    .any(|piece| piece.chunks.contains(&(chunk as u16)))
            );
            println!("MEASURED {asset}: chunk {chunk} releases as dust with a decoration shed");
        }
        let Some(group) = group else {
            continue; // This component is valid decoration below the server-body floor.
        };
        let (mut app, root) = start();
        let chunk = group[0];
        let mass: f32 = group.iter().map(|&c| structure.chunks[c].mass).sum();
        let at = group
            .iter()
            .map(|&c| structure.chunks[c].centroid * structure.chunks[c].mass)
            .sum::<Vec3>()
            / mass;
        isolate(&mut app, root, &group);
        app.update();
        let (owner, piece) = app
            .world_mut()
            .query::<(Entity, &Piece)>()
            .iter(app.world())
            .find(|(_, piece)| piece.chunks.contains(&(chunk as u16)))
            .expect("unsupported matter releases through the common plugin");
        assert_eq!(
            piece.construction,
            app.world().get::<Breakage>(root).unwrap().construction
        );
        assert_eq!(
            piece.chunks,
            group.iter().map(|&c| c as u16).collect::<Vec<_>>()
        );
        assert_eq!(
            app.world().get::<Position>(owner).unwrap().0,
            at,
            "a released group's dynamic frame starts at its occupied centre of mass"
        );
        assert_eq!(
            app.world().get::<RigidBody>(owner),
            Some(&RigidBody::Dynamic)
        );
        assert!(Arc::ptr_eq(
            &app.world().get::<Breakage>(owner).unwrap().kind,
            kind
        ));
        let contact: Vec<_> = app
            .world_mut()
            .query::<(Entity, &ChildOf, &CollisionLayers, &ChunkCollider)>()
            .iter(app.world())
            .filter(|(_, parent, layers, _)| {
                parent.parent() == owner && layers.filters != LayerMask::NONE
            })
            .map(|(entity, _, _, chunk)| (entity, usize::from(chunk.0)))
            .collect();
        assert_eq!(
            contact.len(),
            group.len(),
            "one contact hull per released chunk"
        );
        app.world_mut()
            .entity_mut(owner)
            .insert(LinearVelocity(Vec3::X)); // Fixture motion.
        let before = app.world().get::<Position>(owner).unwrap().0;
        for _ in 0..8 {
            app.update();
        } // Fixture observation window.
        let after = app.world().get::<Position>(owner).unwrap().0;
        assert!(after.x > before.x, "the common dynamic owner advances");
        for (entity, chunk) in contact {
            use crate::track::rubble::{PieceData, RubbleBodies, RubbleQuery, Slab};
            use bevy::ecs::system::RunSystemOnce;

            let world = app.world();
            assert_eq!(world.get::<ColliderOf>(entity).unwrap().body, owner);
            assert_eq!(
                world.get::<Transform>(entity).unwrap().translation,
                kind.structure.chunks[chunk].centroid
                    - app.world().get::<Breakage>(owner).unwrap().origin,
                "a permanent chunk frame moves relative to the dynamic owner"
            );
            let bound = world.get::<ColliderAabb>(entity).unwrap();
            let at = world.get::<Position>(entity).unwrap().0;
            let rotation = world.get::<Rotation>(entity).unwrap().0;
            let expected = world.get::<Collider>(entity).unwrap().aabb(at, rotation);
            // Resolution: the engine's collision margin may expand the broad-phase bounds;
            // containment, rather than equality, is the spatial query contract.
            assert!(
                bound.min.cmple(expected.min).all() && bound.max.cmpge(expected.max).all(),
                "the common collider's live bounds contain its posed hull"
            );
            let centre = (bound.min + bound.max) * 0.5;
            let radius = (bound.max - bound.min).length();
            app.world_mut()
                .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                    let near = rubble.near(centre, radius);
                    assert!(
                        near.contains(&entity),
                        "the belt spatial query must find the freed hull"
                    );
                    assert_eq!(rubble.body_of(entity), owner);
                    let gathered = RubbleBodies::gather(&rubble, &near, &bodies);
                    assert!(gathered.is_dynamic(entity));
                    // Fixture footprint encloses the hull; its positive projected area must support.
                    let slab = Slab {
                        origin: centre - Vec3::X * radius,
                        axis: Vec3::X,
                        lat: Vec3::Z,
                        out: Vec3::NEG_Y,
                        len: 2.0 * radius,
                        lateral: (-radius, radius),
                        face: 0.0,
                        reach: radius,
                        travel: radius,
                    };
                    assert!(
                        gathered.in_slab(&slab).iter().any(|overlap| {
                            gathered.collider(overlap.piece) == entity && !overlap.faces.is_empty()
                        }),
                        "the freed occupied hull must reach the belt footprint reader"
                    );
                })
                .unwrap();
            assert!(
                app.world()
                    .get::<Collider>(entity)
                    .unwrap()
                    .shape()
                    .as_convex_polyhedron()
                    .is_some(),
                "the freed contact hull is direct centroid-local convex geometry"
            );
        }
        assert_eq!(
            app.world().get::<Breakage>(root).unwrap().live.state.fate[chunk],
            Fate::Fallen
        );
        println!(
            "MEASURED {asset}: common detachment retains chunks {group:?} (decor witness {decor:?}), direct contact hulls, moving bounds and production belt support"
        );
    }
}

/// A breakable element with at least three chunks, and its three chunks whose centroids lie
/// nearest the element's centre, nearest first: the fixture's choice by class.
fn fixture_chunks(kind: &ConstructionKind) -> (usize, [usize; 3]) {
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
fn across(kind: &ConstructionKind, e: usize) -> Vec3 {
    kind.layout.element_blocks[e].thinnest().0
}

/// A probe ray through chunk `c` across its element's thinnest span ([`Block::thinnest`]), from
/// half a metre before its near face to `beyond` m past its far face.
fn through(kind: &ConstructionKind, e: usize, c: usize, beyond: f32) -> (Vec3, Dir3, f32) {
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
        .query::<(Entity, &Element)>()
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
        .query::<(Entity, &ChunkCollider)>()
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
    let mut authority = Live::new(&kind);
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
        STEP,
    );
    for kind_of in [
        |e: &Event| matches!(e, Event::Dust(_)),
        |e: &Event| matches!(e, Event::Crack(_)),
    ] {
        assert!(changes.events.iter().any(kind_of), "{:?}", changes.events);
    }
    assert_eq!(authority.state.fate[lone], Fate::Fallen);

    let (mut app, root) = composed(true);
    app.world_mut()
        .resource_mut::<Record>()
        .entries
        .extend(changes.events.iter().map(|&event| Entry {
            construction: 3,
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
        world.query::<&ChunkCollider>().iter(world).count(),
        present,
        "one collider per standing chunk of a swapped element"
    );
    let swapped = derived.swapped.iter().filter(|s| **s).count();
    assert_eq!(
        world.query::<&Element>().iter(world).count(),
        kind.asset.elements.len() - swapped
    );

    let mut record = world.resource_mut::<Record>();
    record.entries.clear();
    record.generation += 1;
    app.update();
    let world = app.world_mut();
    assert_eq!(
        world.get::<Breakage>(root).unwrap().shape(),
        Shape::intact(&kind.layout),
        "a replaced record stands the house whole"
    );
    assert_eq!(world.query::<&ChunkCollider>().iter(world).count(), 0);
    assert_eq!(
        world.query::<&Element>().iter(world).count(),
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
    let live = Live::new(&kind);
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
            launch(&kind, &live.structure, &[c], &[]),
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
    assert!(world.get::<Piece>(body).is_some());
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
    let live = Live::new(&kind);
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
            launch(&kind, &live.structure, &[c], &[]),
        );
        queue.apply(world);
    }
    let piece = app
        .world_mut()
        .query_filtered::<Entity, With<Piece>>()
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

/// The game's environment destruction: freed matter at rest must sleep within Avian's configured
/// rest window. The server relies on that eligibility surviving small contact gaps. Fixture: an
/// admitted construction loses its ground supports on triangulated terrain, without a mover.
#[test]
fn settled_freed_matter_sleeps_within_avians_rest_window() {
    let kind = shipped_house();
    let (mut app, root) = composed_kind(false, &kind);
    crate::debris::crumble::sim_plugin(&mut app);
    crate::headless_test::start_fixed_clock(&mut app);
    app.insert_resource(Gravity(Vec3::NEG_Y * crate::physical_laws::gravity_mps2()));
    let manifest = crate::map::tests::shipped_manifest();
    let grid = crate::terrain_grid::tests::shipped_grid();
    // Fixture: the second matching placement includes an oblique contact patch.
    let instance = manifest
        .instances
        .iter()
        .filter(|i| {
            matches!(
                manifest.prototypes[&i.prototype],
                crate::map::Prototype::Construction { ref asset } if *asset == kind.asset.id
            )
        })
        .nth(1)
        .unwrap();
    let xz = instance.pose.xz();
    let pose = Transform::from_isometry(instance.pose.at_height(grid.height_at(xz.x, xz.y)));
    app.world_mut().entity_mut(root).insert((
        pose,
        Position(pose.translation),
        Rotation(pose.rotation),
    ));
    app.world_mut().spawn((
        RigidBody::Static,
        Position(Vec3::ZERO),
        crate::terrain_grid::heightfield_collider(&grid),
        crate::ground_types::GroundTypes::load_shipped().terrain_friction(),
        CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
    ));
    app.update();
    app.update();
    let supports: BTreeSet<_> = kind.asset.anchors.iter().map(|a| a.chunk).collect();
    let contacts = kind
        .structure
        .chunks
        .iter()
        .enumerate()
        .filter(|(c, chunk)| chunk.buried || supports.contains(&kind.layout.chunks[*c]))
        .map(|(chunk, matter)| {
            (
                root,
                Load {
                    chunk,
                    at: matter.centroid,
                    // DERIVED: twice the pool removes every support without a launch impulse.
                    energy: 2.0 * matter.volume * matter.material.crush,
                    crushed: 2.0 * fracture::full_pool(matter),
                    blow: Vec3::ZERO,
                    press: Vec3::ZERO,
                    peak: None,
                    origin: Origin::Field,
                },
            )
        })
        .collect();
    app.world_mut().resource_mut::<Queue>().0.push(RawDeposit {
        contacts,
        ..default()
    });
    let dt = app
        .world()
        .resource::<Time<Fixed>>()
        .timestep()
        .as_secs_f32();
    let rest = app.world().resource::<TimeToSleep>().0;
    let length_unit = app.world().resource::<PhysicsLengthUnit>().0;
    // DERIVED: one step exposes the contact solve and one commits island sleeping.
    let allowance = 2.0 * dt;
    let mut quiet = 0.0;
    // Fixture watchdog for the collapse transient, separate from the sleep eligibility deadline.
    for _ in 0..((30.0 + rest + allowance) / dt).ceil() as usize {
        app.update();
        let pieces: Vec<_> = app
            .world_mut()
            .query_filtered::<Entity, With<Piece>>()
            .iter(app.world())
            .collect();
        let resting = !pieces.is_empty()
            && pieces.iter().all(|&piece| {
                let threshold = app.world().get::<SleepThreshold>(piece).unwrap();
                app.world().get::<LinearVelocity>(piece).unwrap().0.length()
                    < threshold.linear * length_unit
                    && app
                        .world()
                        .get::<AngularVelocity>(piece)
                        .unwrap()
                        .0
                        .length()
                        < threshold.angular
            });
        quiet = if resting { quiet + dt } else { 0.0 };
        if quiet >= rest + allowance {
            assert!(
                pieces
                    .iter()
                    .all(|&piece| app.world().get::<Sleeping>(piece).is_some()),
                "every piece rested for {quiet} s without a mover, exceeding Avian's {rest} s rest window"
            );
            return;
        }
    }
    panic!("Fixture did not sustain rest for Avian's {rest} s sleep window");
}

/// THE DEPOSITS' WORLD BALL NEVER DROPS WHAT THE CONSTRUCTION'S BOUNDS ACCEPT: for the shipped house
/// (fixture data) at a fixture pose, every blast direction whose ray from a fixture
/// point the bounds' own test meets ([`Block::span`] in the construction frame) passes
/// [`Frame::may_meet`], from points inside, beside, above and 1.4 km from the house; and every
/// fixture point the field-stop test admits passes [`Frame::may_hold`]. A ball that clipped the
/// bounds would drop a blast ray or a field stop the construction took, and with it a deposit; the
/// deposits rely on the ball only to skip work.
#[test]
fn the_world_ball_keeps_every_ray_the_bounds_meet() {
    let kind = shipped_house();
    let (lo, hi) = kind.bounds();
    let pose = Transform::from_xyz(312.5, -4.25, -187.0).with_rotation(Quat::from_rotation_y(0.7));
    let frame = Frame::new(&pose, (lo, hi));
    let bounds = Block::cuboid(lo, hi);
    let world = |p: Vec3| pose.translation + pose.rotation * p;
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
        let local = frame.local(origin);
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
    let slack = Vec3::splat(STOP_TOLERANCE_M);
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

/// A round's line through law chunk `c` (construction frame, unit scale): 10 cm along unit `w` about
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
fn entries(changes: &Changes, tick: u32) -> Vec<Entry> {
    changes
        .events
        .iter()
        .map(|&event| Entry {
            construction: 3,
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
    let mut authority = Live::new(&kind);
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
        holes_of(&kind, &lines, &[]),
        Vec3::ZERO,
        STEP,
    );
    let blast = blast_on(&authority.structure, dish_chunk, 0.05);
    let holes = holes_of(&kind, &[], &blast);
    let second = run_laws(
        &kind.layout.element_of,
        &mut authority,
        &blast,
        holes,
        Vec3::ZERO,
        STEP,
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
            .resource_mut::<Record>()
            .entries
            .extend(entries(changes, tick));
        growing.update();
    }
    let (mut late, late_root) = composed(true);
    late.world_mut().resource_mut::<Record>().entries =
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
    let mut cut = Live::new(&kind);
    let mut whole = Live::new(&kind);
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
    let holes = holes_of(&kind, &[line], &[]);
    let mut record = Vec::new();
    for tick in [7, 9] {
        let with = run_laws(
            &kind.layout.element_of,
            &mut cut,
            &loads,
            holes.clone(),
            Vec3::ZERO,
            STEP,
        );
        let without = run_laws(
            &kind.layout.element_of,
            &mut whole,
            &loads,
            Vec::new(),
            Vec3::ZERO,
            STEP,
        );
        assert!(
            with.events.contains(&Event::Hole(c as u16, holes[0].1)),
            "round {tick} holes the chunk: {:?}",
            with.events
        );
        let kept: Vec<Event> = with
            .events
            .iter()
            .copied()
            .filter(|event| !matches!(event, Event::Hole(..)))
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
    let (holed, unholed): (Vec<Entry>, Vec<Entry>) = record
        .into_iter()
        .partition(|entry| matches!(entry.event, Event::Hole(..)));
    app.world_mut().resource_mut::<Record>().entries = unholed;
    app.update();
    let colliders = |app: &mut App| {
        let world = app.world_mut();
        let mut chunks: Vec<(Entity, u16)> = world
            .query::<(Entity, &ChunkCollider)>()
            .iter(world)
            .map(|(entity, chunk)| (entity, chunk.0))
            .collect();
        chunks.sort_unstable();
        let mut elements: Vec<Entity> = world
            .query_filtered::<Entity, With<Element>>()
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
        .resource_mut::<Record>()
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
    let mut live = Live::new(&kind);
    let to_spend = HOLES_PER_CHUNK + 4;
    let line = line_through_chunk(&live.structure, across(&kind, e), c, 1.0 / to_spend as f32);
    let holes = holes_of(&kind, &[line], &[]);
    assert_eq!(holes.len(), 1);
    let loads = line_loads(&live.structure, &[line]);
    for joint in &mut Arc::make_mut(&mut live.structure).joints {
        if let Bond::Bonded(seam) = &mut joint.bond {
            *seam = fracture::Seam {
                fixing: seam.fixing,
                tensile: 1e12,
                fracture_energy: 1e12,
                shear: fracture::Shear {
                    cohesion: 1e12,
                    energy: 1e12,
                },
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
                STEP,
            );
            if changes.events.contains(&Event::Dust(c as u16)) {
                return true;
            }
            let holed = changes
                .events
                .iter()
                .any(|event| matches!(event, Event::Hole(h, _) if usize::from(*h) == c));
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
    let structure = kind.layout.structure(&kind.asset);
    let crush = structure.chunks[c].material.crush;
    let metres = |p: [i16; 3]| Vec3::from_array(p.map(|v| f32::from(v) / 1000.0));

    let line = line_through_chunk(&structure, across(&kind, e), c, 0.01);
    let [(_, channel)] = holes_of(&kind, &[line], &[])[..] else {
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
    let [(_, dish)] = holes_of(&kind, &[], &blast)[..] else {
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
    let structure = kind.layout.structure(&kind.asset);
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
    let [(_, channel)] = holes_of(&kind, &[(c, entry, exit, energy)], &[])[..] else {
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
    let [(_, blind)] = holes_of(&kind, &[(c, entry, rest, energy)], &[])[..] else {
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

/// A loose piece: a debris body that is no construction piece.
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
    app.world_mut().trigger(Charge { at, blast });
    app.update();
    app.update();
}

/// Fixture data for ray shielding: one anchored cube, 0.2 m per side. The invariant needs
/// a crushable surface and a blocker, not a house's structural failure search.
fn shielding_cube() -> Arc<ConstructionKind> {
    let registry = crate::substances::SubstanceRegistry::shipped_static();
    let element = schema::Element {
        name: "fixture/cube".to_owned(),
        part: "pier".into(),
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
    let asset = ConstructionAsset {
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
            mass_kg: volume * Material::of(registry, "Brick").unwrap().unwrap().density,
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
        ConstructionKind::resolve(
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
        assert!(rays > 0, "the body is in the field");
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
    assert!(rays > 0, "the armour volume is in the field");
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

/// A SLEEPING LOOSE PIECE WAKES TO A BLAST: a construction piece asleep at rest on a slab is woken by a
/// fixture burst beside it, and leaves away from the burst. An impulse that leaves a sleeping body
/// asleep, or is held back from one, fails here; rubble settled for most of a match relies on it.
#[test]
fn a_sleeping_loose_piece_wakes_to_a_blast() {
    let (mut app, _) = composed(false);
    app.insert_resource(Gravity(Vec3::NEG_Y * 9.81));
    let kind = shipped_house();
    let (_, [c, ..]) = fixture_chunks(&kind);
    let live = Live::new(&kind);
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
            launch(&kind, &live.structure, &[c], &[]),
        );
        queue.apply(world);
    }
    let piece = app
        .world_mut()
        .query_filtered::<Entity, With<Piece>>()
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

/// Per update, the contact loads [`read_contacts`] queued in it, each with its construction's root.
#[derive(Resource, Default)]
struct QueuedContacts(Vec<Vec<(Entity, Load)>>);

fn keep_queued_contacts(queue: Res<Queue>, mut kept: ResMut<QueuedContacts>) {
    kept.0.push(
        queue
            .0
            .iter()
            .flat_map(|deposit| deposit.contacts.iter().copied())
            .collect(),
    );
}

/// [`composed_kind`] with queued contact loads retained between the reader and the laws,
/// its clocks started.
fn composed_keeping_contacts(kind: &Arc<ConstructionKind>) -> (App, Entity) {
    let (mut app, root) = composed_kind(false, kind);
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
/// The construction laws rely on the same load path as solver contacts, with no body-kind exception.
/// FIXTURE DATA: a 20 N press on a breakable outer wall; the source is clear of solver contacts.
#[test]
fn an_implicit_belt_press_reaches_the_wall_laws_reading() {
    let kind = shipped_house();
    let (mut app, root) = composed_keeping_contacts(&kind);
    let (element, face, into) = wall_face(&kind);
    let source = contact_cube(&mut app, Vec3::splat(50.0), Vec3::ZERO);
    app.update();
    let collider = app
        .world_mut()
        .query::<(Entity, &Element, &ChildOf)>()
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
        .filter(|(_, load)| matches!(load.origin, Origin::Contact { other, .. } if other == source))
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
pub(crate) const CUBE_SIDE: f32 = 0.4;
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
pub(crate) fn wall_face(kind: &ConstructionKind) -> (usize, Vec3, Vec3) {
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

/// The game's environment destruction: equal motion and geometry produce equal contact forces
/// for a plain body and a construction carrier. The structure check relies on no body-kind gate;
/// the unbroken fixture is deliberately too slow to sweep its wall: its stop's peak force at
/// 0.1 m/s stays under the struck patch's cut.
#[test]
fn no_bodys_kind_enters_a_contacts_loads() {
    let kind = shipped_house();
    let (e, face, w) = wall_face(&kind);
    // Fixture: 2 cm short of the face at 0.1 m/s, so the cube meets it within the run.
    let (gap, speed) = (0.02, 0.1);
    let start = face - w * (CUBE_SIDE / 2.0 + gap);
    let strike = |construction_piece: bool| {
        let (mut app, root) = composed_keeping_contacts(&kind);
        let cube = contact_cube(&mut app, start, w * speed);
        if construction_piece {
            // A free group carries its source tree from spawn: here, its own collider.
            let hull = app.world().get::<Collider>(cube).unwrap().clone();
            app.world_mut().entity_mut(cube).insert((
                Piece {
                    construction: 3,
                    chunks: vec![0],
                    born: 0,
                    retired: None,
                },
                crate::debris::DebrisPiece,
                super::super::sweep::SourceShape(Arc::new(super::super::sweep::SourceTree::new(
                    vec![super::super::sweep::Hull {
                        surface: None,
                        hull,
                        position: Vec3::ZERO,
                        rotation: Quat::IDENTITY,
                    }],
                ))),
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
                matches!(load.origin, Origin::Contact { other, .. } if other == cube),
                "each load names its body and effective mass: {load:?}"
            );
        }
        let momentum = app.world().get::<LinearVelocity>(cube).unwrap().0 * CUBE_KG;
        (ticks, momentum)
    };
    let ((plain, final_momentum), (piece, _)) = (strike(false), strike(true));
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
        "a construction piece's loads are a plain body's"
    );
    let loads: Vec<Load> = plain.into_iter().flatten().map(|(_, load)| load).collect();
    assert!(!loads.is_empty(), "the cube strikes the wall");
    let blow: Vec3 = loads.iter().map(|load| load.blow + load.press * STEP).sum();
    let closing = CUBE_KG * speed;
    let transferred = w * closing - final_momentum;
    assert!(
        (blow - transferred).length() <= 0.01 * closing,
        "the strike's blow {blow} against its momentum change {transferred} N·s"
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

/// The game's environment destruction: a sleeping cube's 400 N press stays cached, with no
/// new queued deposit, until its body leaves. The structure law relies on sleeping piles keeping
/// their supported press without repeating the work each tick. Fixture: loading the column's
/// crown puts every supporting joint in compression, below its bulk capacity.
#[test]
fn a_body_at_rest_against_a_construction_presses_it_once_and_starts_no_run() {
    let kind = column_construction();
    let block = kind.layout.blocks.last().unwrap();
    let face = Vec3::new(
        (block.min.x + block.max.x) / 2.0,
        block.max.y,
        (block.min.z + block.max.z) / 2.0,
    );
    let w = Vec3::NEG_Y;
    let (mut app, root) = composed_keeping_contacts(&kind);
    // Fixture: 400 N normal press and 2 mm approach gap, centred on the crown.
    let force_n = 400.0;
    let cube = contact_cube(&mut app, face - w * (CUBE_SIDE / 2.0 + 0.002), Vec3::ZERO);
    app.world_mut().entity_mut(cube).insert((
        ConstantForce(w * force_n),
        crate::contact_transfer::StructureBody,
    ));
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
        assert!(
            app.world()
                .resource::<ContactTransfers>()
                .resting()
                .iter()
                .all(|rest| rest.transfer.reference == root),
            "only the static structural side retains resting points"
        );
        let resting = app.world().resource::<RestingLoads>();
        let loads = resting.0.get(&root).cloned().unwrap_or_default();
        assert_eq!(
            resting.0.len(),
            1,
            "tick {tick}: one construction holds a press"
        );
        assert!(!loads.is_empty(), "tick {tick}: the column holds the press");
        let press: Vec3 = loads.iter().map(|load| load.press).sum();
        assert!(
            (press - w * force_n).length() <= 0.005 * force_n,
            "tick {tick}: the resting press {press} against {force_n} N along {w}"
        );
        for load in &loads {
            assert_eq!(
                (load.blow, load.energy),
                (Vec3::ZERO, 0.0),
                "tick {tick}: a resting load is its body's press alone"
            );
            assert!(matches!(load.origin, Origin::Contact { other, .. } if other == cube));
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

/// The game's damage model delivers each march before its shell despawns. Construction deposits
/// rely on that ordering: a missing shell is a bug, never permission to discard delivered lines.
#[test]
#[should_panic(expected = "ShellMarched is observed before its shell despawns")]
fn a_march_without_its_shell_is_a_bug() {
    let mut app = App::new();
    app.init_resource::<Queue>().add_observer(read_shell);
    app.world_mut().trigger(ShellMarched {
        shell: Entity::PLACEHOLDER,
        marks_from: 0,
        cones_from: 0,
        lines: Vec::new(),
        burst: None,
    });
}

/// DEGENERATE CHUNK HULLS ARE REFUSED BEFORE PLAY (the game's environment destruction model).
/// Launch and collider reconciliation rely on solid baked hulls; four coplanar corners or a
/// nonfinite corner must fail the load gate with the field and fix, rather than panic on contact.
#[test]
fn degenerate_chunk_hulls_are_refused_before_play() {
    let kind = shipped_house();
    let masonry: Vec<Option<Material>> = kind
        .asset
        .materials
        .iter()
        .map(|name| {
            Material::of(crate::substances::SubstanceRegistry::shipped_static(), name).unwrap()
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

/// The game's environment destruction model reads reconstructed law blocks, not only the
/// collider. A slab half as thick as the law's plane tolerance ([`fracture::ON_PLANE`]) remains a
/// convex collider, but a plane on one face would hold the other; neither chunk nor element
/// geometry may reach play so. Fixture: an 8 cm square.
#[test]
fn collider_valid_hulls_with_collapsed_law_boundaries_are_refused_at_load() {
    let kind = shipped_house();
    let registry = crate::substances::SubstanceRegistry::shipped_static();
    let masonry = kind
        .asset
        .materials
        .iter()
        .map(|name| Material::of(registry, name).unwrap())
        .collect::<Vec<_>>();
    let thin = (0..8)
        .map(|i| {
            [
                [0.0, 0.08][i & 1],
                [0.0, 0.08][(i >> 1) & 1],
                [0.0, fracture::ON_PLANE / 2.0][i >> 2],
            ]
        })
        .collect::<Vec<_>>();
    assert!(
        Collider::convex_hull(thin.iter().copied().map(Vec3::from_array).collect()).is_some(),
        "the fixture must reach the law-block gate"
    );
    let c = kind.layout.chunks[0] as usize;
    let e = usize::from(kind.asset.chunks[c].element);
    for chunk in [true, false] {
        let mut asset = kind.asset.clone();
        let field = if chunk {
            asset.chunks[c].hull = thin.clone();
            format!("chunks[{c}].hull")
        } else {
            asset.elements[e].hull = thin.clone();
            format!("elements[{e}].hull")
        };
        let Err(error) = Layout::of(&asset, &masonry, Attachments::shipped_static()) else {
            panic!("collider admission must also certify the law boundary");
        };
        for required in [field.as_str(), "law block", "rebake"] {
            assert!(error.contains(required), "{error}");
        }
    }
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
        let structure = kind.layout.structure(&kind.asset);
        let c = structure
            .chunks
            .iter()
            .position(|chunk| chunk.buried)
            .unwrap();
        let at = structure.chunks[c].centroid;
        let volume = app
            .world_mut()
            .query::<(Entity, &Element, &ChildOf)>()
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
        app.world_mut().resource_mut::<Queue>().0 = deposits;
        app.world_mut().run_system_once(apply_deposits).unwrap();
        let mut breakage = app.world_mut().get_mut::<Breakage>(root).unwrap();
        let live = breakage.live.as_mut();
        let force = live.assessment.applied_force().as_vec3();
        assert_eq!(force.x, 3.0, "contact first: {contact_first}");
    }
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
    let queued = app.world().resource::<Queue>();
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

/// A LAUNCH SUMS THE MASS BAKED AT LOAD AS THE CHUNKS' OWN HULLS WOULD GIVE IT. A collapse must
/// not build a hull per chunk on the tick it falls, so [`launch`] reads each chunk's properties
/// from load; a departed group's body relies on them being the hulls'. A mutant shifting them by
/// the wrong offset moves the centre of mass and every lever arm taken about it.
#[test]
fn a_launch_from_baked_chunk_mass_matches_the_chunks_hulls() {
    use crate::debris::launch::launch_colliders;
    let kind = shipped_house();
    let live = Live::new(&kind);
    let chunks = &kind.structure.chunks;
    let group: Vec<usize> = (0..chunks.len())
        .filter(|&c| !chunks[c].buried)
        .take(64)
        .collect();
    let baked = launch(&kind, &live.structure, &group, &[]);
    let hulls = group
        .iter()
        .map(|&c| (c, chunk_collider(&kind, c, baked.centroid)))
        .collect();
    let direct = launch_colliders(baked.centroid, hulls, |c| chunks[c].material.density, &[]);
    let (a, b) = (baked.mass_properties, direct.mass_properties);
    // DERIVED allowance: f32 rounding of a 64-term sum of hull properties.
    assert!((a.mass - b.mass).abs() <= 1e-3 * b.mass);
    assert!(a.center_of_mass.distance(b.center_of_mass) <= 1e-3);
    let (ia, ib) = (
        a.angular_inertia_tensor().to_mat3(),
        b.angular_inertia_tensor().to_mat3(),
    );
    assert!(
        (ia - ib)
            .to_cols_array()
            .iter()
            .all(|d| d.abs() <= 1e-3 * ib.x_axis.x.abs())
    );
}

/// THE GAME'S CASING LOADS CARRY ONE PHYSICAL ENERGY BUDGET AND ITS EJECTION MOMENTUM.
/// The environment adapter must not amplify physical transport with the HP field's
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
        let queue = app.world().resource::<Queue>();
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

/// ASSUMED geometry fixture selected by class: three collinear neighbours in one element,
/// sharing two seams and a common extrusion. The data supplies every hull and mass.
fn landing_slab() -> (Arc<ConstructionKind>, Vec<usize>) {
    let mut kind = (*shipped_house()).clone();
    let structure = kind.layout.structure(&kind.asset);
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
    kind.structure = Arc::new(kind.layout.checked_structure(&kind.asset).unwrap());
    kind.equilibrium = Arc::new(fracture::parts::Assessment::at_rest(&kind.structure));
    (Arc::new(kind), group)
}

/// The sheds the authority wrote, in order.
#[derive(Resource, Default)]
struct Sheds(Vec<crate::debris::crumble::Shed>);

fn keep_sheds(mut written: MessageReader<crate::debris::crumble::Shed>, mut kept: ResMut<Sheds>) {
    kept.0.extend(written.read().cloned());
}

/// [`landing_slab`]'s group released tilted, its lowest corner 3 m over flat ground (fixture
/// placement), and run 3 s on the shipped sim with the crumble mounted: the group, the live pieces
/// then, the sheds and the record.
fn drop_slab() -> (
    Arc<ConstructionKind>,
    Vec<usize>,
    Vec<Piece>,
    Vec<crate::debris::crumble::Shed>,
    Record,
) {
    let (mut app, construction) = composed_kind(false, &geometry_fixture(&shipped_house()));
    crate::debris::crumble::sim_plugin(&mut app);
    app.init_resource::<Sheds>()
        .add_systems(FixedPostUpdate, keep_sheds.after(LawSet));
    app.insert_resource(Gravity(Vec3::NEG_Y * crate::physical_laws::gravity_mps2()));
    let (kind, group) = landing_slab();
    {
        let mut fixed = app
            .world_mut()
            .get_mut::<Breakage>(construction)
            .expect("the fixture's placed root");
        let live = fixed.live.as_mut();
        for &c in &group {
            live.state.fate[c] = Fate::Fallen;
            live.swapped[usize::from(kind.layout.element_of[c])] = true;
        }
    }
    let rotation = Quat::from_rotation_x(0.25) * Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
    let low = group
        .iter()
        .flat_map(|&c| kind.layout.blocks[c].points.iter())
        .map(|p| (rotation * *p).y)
        .fold(f32::INFINITY, f32::min);
    let root = Transform::from_xyz(30.0, 3.0 - low, 0.0).with_rotation(rotation);
    let live = Live::new(&kind).group(&group);
    let launch = launch(&kind, &live.structure, &group, &[]);
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
                kind: kind.clone(),
                group: group.clone(),
                root,
                live,
                tick: 0,
                construction: 3,
                launch,
            },
        );
        queue.apply(world);
    }
    for _ in 0..192 {
        app.update();
    }
    let pieces = app
        .world_mut()
        .query::<&Piece>()
        .iter(app.world())
        .cloned()
        .collect();
    assert!(app.world().get_entity(construction).is_ok());
    let sheds = std::mem::take(&mut app.world_mut().resource_mut::<Sheds>().0);
    (
        kind,
        group,
        pieces,
        sheds,
        app.world().resource::<Record>().clone(),
    )
}

/// A RELEASED PART LANDS, CRUMBLES AND SHEDS ITS SMALL FRAGMENTS, end to end on the authority's
/// shipped plugins and real Avian contacts: the live bodies and the sheds partition the part,
/// each live body reaches the cut-off and each shed falls short of it, and the record writes
/// every shed chunk as dust at its shed's tick, so every player's view hands the parent's chunks
/// over. A mutant that drops the sheds, keeps a below-floor fragment as a body or leaves a shed chunk
/// standing in the record breaks it. Which fragments form is the law's (its own tests); this
/// witness asserts no count.
#[test]
fn a_released_part_lands_crumbles_and_sheds_its_small_fragments() {
    let (kind, group, pieces, sheds, record) = drop_slab();
    let server = |chunks: &[u16]| {
        let chunks: Vec<_> = chunks.iter().map(|&c| usize::from(c)).collect();
        crate::debris::crumble::server_body(&kind.structure, &chunks)
    };
    let mut owned: Vec<usize> = pieces
        .iter()
        .flat_map(|piece| &piece.chunks)
        .chain(sheds.iter().flat_map(|shed| &shed.chunks))
        .map(|&c| usize::from(c))
        .collect();
    owned.sort_unstable();
    assert_eq!(
        owned, group,
        "the bodies and sheds partition the released part"
    );
    for piece in &pieces {
        assert!(server(&piece.chunks), "a kept body {:?}", piece.chunks);
    }
    for shed in &sheds {
        assert!(!server(&shed.chunks), "a shed {:?}", shed.chunks);
        for &c in &shed.chunks {
            assert!(record.entries.contains(&Entry {
                construction: 3,
                event: Event::Dust(c),
                tick: shed.tick,
            }));
        }
    }
    println!(
        "MEASURED landing: {} bodies {:?}, {} sheds {:?}",
        pieces.len(),
        pieces.iter().map(|piece| &piece.chunks).collect::<Vec<_>>(),
        sheds.len(),
        sheds.iter().map(|shed| &shed.chunks).collect::<Vec<_>>()
    );
}

/// Splitting preserves the occupied collider mass, linear momentum, angular momentum and
/// kinetic energy of the parent's rigid velocity field. No split may manufacture a launch.
#[test]
fn split_children_inherit_the_parents_rigid_momentum() {
    use bevy::ecs::system::RunSystemOnce;
    let (mut app, _) = composed(false);
    let (kind, group) = landing_slab();
    let root = Transform::from_xyz(30.0, 20.0, 0.0).with_rotation(Quat::from_rotation_y(0.7));
    let live = Live::new(&kind).group(&group);
    let parent = launch(&kind, &live.structure, &group, &[]);
    let centre = root.translation + root.rotation * parent.centre;
    let velocity = Vec3::new(2.0, -3.0, 1.0);
    let spin = Vec3::new(0.3, 0.5, -0.2);
    let inertia = Mat3::from_quat(root.rotation)
        * parent.mass_properties.angular_inertia_tensor().to_mat3()
        * Mat3::from_quat(root.rotation).transpose();
    let energy = 0.5 * parent.mass_properties.mass * velocity.length_squared()
        + 0.5 * spin.dot(inertia * spin);
    let total_mass = parent.mass_properties.mass;
    let momentum = parent.mass_properties.mass * velocity;
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
                tick: 0,
                construction: 3,
                launch: parent,
            },
        );
        queue.apply(world);
    }
    let entity = app
        .world_mut()
        .query_filtered::<Entity, With<Piece>>()
        .single(app.world())
        .unwrap();
    // The group's exact COM offset is obtained synchronously from its data-built colliders.
    let breakage = app.world().get::<Breakage>(entity).unwrap();
    let exact = launch(
        &kind,
        &breakage.live.as_ref().structure,
        &app.world()
            .get::<Piece>(entity)
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
            &Piece,
            &Position,
            &Rotation,
            &LinearVelocity,
            &AngularVelocity,
            &Breakage,
        )>()
        .iter(world)
    {
        let group: Vec<_> = piece.chunks.iter().map(|&c| usize::from(c)).collect();
        let geometry = launch(&kind, &breakage.live.as_ref().structure, &group, &[]);
        let r = Mat3::from_quat(rotation.0);
        let inertia =
            r * geometry.mass_properties.angular_inertia_tensor().to_mat3() * r.transpose();
        let at = pose.0 + rotation.0 * (geometry.centre - geometry.centroid);
        sum_mass += geometry.mass_properties.mass;
        sum_p += geometry.mass_properties.mass * velocity.0;
        sum_l += inertia * spin.0 + (at - centre).cross(geometry.mass_properties.mass * velocity.0);
        sum_e += 0.5 * geometry.mass_properties.mass * velocity.0.length_squared()
            + 0.5 * spin.0.dot(inertia * spin.0);
    }
    assert!((sum_p - momentum).length() <= 1e-4 * momentum.length());
    assert!((sum_l - angular).length() <= 1e-4 * angular.length());
    assert!((sum_e - energy).abs() <= 1e-4 * energy);
    assert!((sum_mass - total_mass).abs() < 1e-5 * total_mass);
}

/// A moving group holds only its own chunks: an unbreakable element stays with the placed
/// construction, so a ray through where it stood, in a group's construction frame, meets nothing
/// there. A blast or field ray stopped by it would shade the group's real neighbours. ASSUMED
/// fixture: one shipped element relabelled unbreakable, a group that owns no chunk on the ray.
#[test]
fn an_unbreakable_element_blocks_only_its_placed_construction() {
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
/// laws: a blow on a chunk it turned to dust went into crushing it, as on a standing construction,
/// and must not reach the parent velocity its split children inherit. ASSUMED fixture loads.
#[test]
fn a_field_blow_on_a_dusted_chunk_moves_no_moving_group() {
    let kind = shipped_house();
    let (_, [a, b, _]) = fixture_chunks(&kind);
    let live = Live::new(&kind);
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

/// Pricing bed for the game's construction destruction model, game state only. One deposit on
/// the shipped construction, with a one-sided patch of externally loaded chunks. Fixture:
/// nearest unburied centroids to the minimum-X face, parallel 10 N s blows and 100 kN peaks,
/// no crushing or cavity energy. Setup and fresh-state cloning are outside the timed deposit;
/// the server's complete `run_laws` path is inside it. No assertion depends on elapsed time.
#[test]
#[ignore = "a pricing bed, run on demand under perf and the machine lock"]
fn a_seeded_blast_deposit_priced_by_loaded_chunks() {
    let kind = shipped_house();
    let base = Live::new(&kind);
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
                STEP,
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
/// Who relies on it: every construction a spall cone meets. Fixture data: 2.7 MJ carried at a ratio
/// of 1, `λ` 2, 64 directions, all stopping on one law chunk of the shipped house.
#[test]
fn a_wall_crushes_with_the_energy_a_cone_carries() {
    use crate::ballistics::spall_cone::{AxisRule, ConeShape, FieldDescriptor, field_strength};
    use crate::ballistics::{SpallCone, SpallRay};
    let kind = shipped_house();
    let structure = kind.layout.structure(&kind.asset);
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
    let queue = app.world().resource::<Queue>();
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
fn stacked_chunks() -> (App, Entity, ConstructionKind, usize, [usize; 2]) {
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
    kind: &ConstructionKind,
    e: usize,
    rotation: Quat,
    (a, b): (Vec3, Vec3),
    (transit, entry_blow): (Vec3, Vec3),
) -> Option<Taken> {
    use crate::ballistics::{DeliveredLine, VolumeCrossing};
    use bevy::ecs::system::RunSystemOnce;
    app.world_mut().get_mut::<Breakage>(root).unwrap().kind = Arc::new(kind.clone());
    let frame = Frame::new(&Transform::from_rotation(rotation), kind.bounds());
    let volume = app
        .world_mut()
        .query::<(Entity, &Element, &ChildOf)>()
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
            take_lines(
                ComputeTaskPool::get(),
                &p,
                &BTreeMap::from([(root, frame)]),
                deposit,
                &mut taken,
            );
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
            .query::<(Entity, &Element, &ChildOf)>()
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
        let frame = Frame::new(&Transform::from_xyz(0.0, 0.0, 1000.0), kind.bounds());
        let taken = app
            .world_mut()
            .run_system_once(move |p: DepositParam| {
                let mut taken = BTreeMap::new();
                take_lines(
                    ComputeTaskPool::get(),
                    &p,
                    &BTreeMap::from([(root, frame)]),
                    &deposit,
                    &mut taken,
                );
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
        let holes = holes_of(&kind, &taken.lines, &[]);
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

/// Fixture: the surrounding geometry is held by high cohesive strength in both modes.
/// These probes challenge collider hand-off and contact energy, independently of structural
/// failure elsewhere in the construction. Density, geometry and stiffness remain the asset's.
fn geometry_fixture(kind: &Arc<ConstructionKind>) -> Arc<ConstructionKind> {
    let mut kind = (**kind).clone();
    let seam = fracture::Seam {
        fixing: None,
        tensile: 1e12,
        fracture_energy: 1e12,
        shear: fracture::Shear {
            cohesion: 1e12,
            energy: 1e12,
        },
    };
    for joint in &mut kind.layout.joints {
        joint.bond = Bond::Bonded(fracture::Seam {
            fixing: joint.seam().and_then(|s| s.fixing),
            ..seam
        });
    }
    for material in &mut kind.layout.materials {
        material.seam = seam;
    }
    kind.structure = Arc::new(kind.layout.checked_structure(&kind.asset).unwrap());
    kind.equilibrium = Arc::new(fracture::parts::Assessment::at_rest(&kind.structure));
    Arc::new(kind)
}

/// `group` freed where it stood in a composed house `offset` from the origin, its neighbours still
/// standing and gravity on (fixture 9.81 m/s²): the app, and the body the launch expects, as mass,
/// inertia and centre about the collider frame.
fn freed_in_place(group: &[usize], offset: Vec3) -> (App, (f32, Mat3, Vec3)) {
    let kind = geometry_fixture(&shipped_house());
    let (mut app, root) = composed_kind(false, &kind);
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
    let launched = launch(&kind, &piece_live.structure, group, &[]);
    let body = (
        launched.mass_properties.mass,
        launched.mass_properties.angular_inertia_tensor().to_mat3(),
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
            tick: 0,
            construction: 3,
            launch: launched,
        },
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
    let structure = kind.layout.structure(&kind.asset);
    for c in 0..kind.layout.chunks.len() {
        let mass = launch(&kind, &structure, &[c], &[]).mass_properties.mass;
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
    ), With<Piece>>();
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
    let structure = layout.structure(&kind.asset);
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
                ), With<Piece>>();
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

/// The game's environment destruction separates contact only where two bodies' coplanar faces
/// meet. On every admitted kind, read from the joints' own geometry rather than the code that
/// picks faces: each contact hull is its chunk's half-spaces with the faces at its joints set back
/// `LOOSE_GAP_M`, so two chunks freed in place stand at least twice that apart, and no other face
/// moves. A mutant that sets back a face opposite its joint leaves the joint flush and fails; a
/// proportional inset moves farther faces by more and fails.
#[test]
fn contact_hulls_set_back_only_joint_faces_by_the_contact_gap() {
    // Resolution: f32 construction-frame coordinates (ulp 3.8 µm below 32 m, DERIVED).
    let tolerance = 2e-5;
    for (_, kind) in admitted_kinds() {
        let structure = &kind.structure;
        let mut expected = vec![BTreeSet::new(); structure.chunks.len()];
        for (j, joint) in structure.joints.iter().enumerate() {
            let sides = match joint.b {
                Side::Chunk(b) => vec![joint.a, b],
                Side::Ground => vec![joint.a],
            };
            for c in sides {
                let block = &kind.layout.blocks[c];
                let centroid = structure.chunks[c].centroid;
                let inward = if joint.normal.dot(centroid - joint.centroid) > 0.0 {
                    joint.normal
                } else {
                    -joint.normal
                };
                // The face at the joint is the one facing out against the inward direction.
                let (face, &(normal, offset)) = block
                    .planes
                    .iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| (-inward).dot(a.0).total_cmp(&(-inward).dot(b.0)))
                    .unwrap();
                let on_face = (normal.dot(joint.centroid) - offset).abs()
                    <= crate::construction::fracture::ON_PLANE;
                if !on_face {
                    assert_eq!(
                        joint.b,
                        Side::Ground,
                        "joint {j} lies on a face of chunk {c}"
                    );
                    continue; // An anchor on no face sets none back.
                }
                expected[c].insert(face);
            }
        }
        for (c, block) in kind.layout.blocks.iter().enumerate() {
            let centroid = structure.chunks[c].centroid;
            let hull = kind.sweep.contact_hull(c);
            let points = hull.shape().as_convex_polyhedron().unwrap().points();
            let set_back: BTreeSet<_> = block
                .planes
                .iter()
                .enumerate()
                .filter(|(_, (normal, offset))| {
                    let reach = points
                        .iter()
                        .map(|&p| normal.dot(p + centroid) - offset)
                        .fold(f32::NEG_INFINITY, f32::max);
                    reach < -LOOSE_GAP_M / 2.0
                })
                .map(|(face, _)| face)
                .collect();
            assert_eq!(set_back, expected[c], "chunk {c}'s set-back faces");
            let planes: Vec<_> = block
                .planes
                .iter()
                .enumerate()
                .map(|(face, &(normal, offset))| {
                    let gap = if expected[c].contains(&face) {
                        LOOSE_GAP_M
                    } else {
                        0.0
                    };
                    (normal, offset - gap)
                })
                .collect();
            // The hull is the set-back half-spaces' intersection: no corner passes one of their
            // planes, and every corner lies on three. A face its neighbours' setbacks consume
            // (a bake sliver narrower than the gap) leaves no corner on its plane.
            for &corner in points {
                let at = corner + centroid;
                let mut on = 0;
                for &(normal, offset) in &planes {
                    let past = normal.dot(at) - offset;
                    assert!(
                        past < tolerance,
                        "chunk {c}: a corner passes {normal} by {past} m"
                    );
                    on += usize::from(past.abs() < tolerance);
                }
                assert!(
                    on >= 3,
                    "chunk {c}: corner {at} lies on {on} set-back planes"
                );
            }
        }
    }
}

/// The game's damage-model rounds march a chunk's occupied hull, standing after its element's first
/// crack and freed alike: contact separation must not shorten either face of a ballistic chord.
/// This reads the colliders reconcile and spawn_group attach, so a ballistic volume on a contact
/// hull fails.
#[test]
fn a_freed_chunks_ballistic_chord_is_its_occupied_chord() {
    let kind = shipped_house();
    let structure = kind.layout.structure(&kind.asset);
    let (_, [c, standing, _]) = fixture_chunks(&kind);
    let (mut app, _) = freed_in_place(&[c], Vec3::ZERO);
    app.insert_resource(Gravity::ZERO);
    app.update();
    let mut volumes = app
        .world_mut()
        .query_filtered::<(&Collider, &ChunkCollider, &ChildOf, Has<Sensor>), With<BallisticVolume>>();
    let colliders: Vec<_> = volumes
        .iter(app.world())
        .filter(|(_, chunk, ..)| [c, standing].contains(&usize::from(chunk.0)))
        .map(|(hull, chunk, parent, sensor)| {
            let freed = app.world().get::<Piece>(parent.parent()).is_some();
            assert_eq!(
                sensor, freed,
                "only a freed chunk's occupied collider is a sensor"
            );
            (usize::from(chunk.0), freed, hull.clone())
        })
        .collect();
    let mut seen: Vec<_> = colliders
        .iter()
        .map(|&(chunk, freed, _)| (chunk, freed))
        .collect();
    seen.sort_unstable();
    let mut expected = [(c, true), (standing, false)];
    expected.sort_unstable();
    assert_eq!(
        seen, expected,
        "one ballistic collider per chunk, freed and standing"
    );
    for (chunk, _, hull) in &colliders {
        // Both hulls lie in the chunk's centroid frame, the collider's own.
        let occupied = chunk_collider(&kind, *chunk, structure.chunks[*chunk].centroid);
        let reach = (kind.layout.blocks[*chunk].max - kind.layout.blocks[*chunk].min).length();
        // Fixture rays in three independent directions through the centroid, long enough to
        // cross this chunk.
        for ray in [Vec3::X, Vec3::Y, Vec3::Z]
            .into_iter()
            .flat_map(|d| [d, -d])
        {
            let from = -ray * reach;
            let cast = |hull: &Collider| {
                hull.cast_ray(Vec3::ZERO, Quat::IDENTITY, from, ray, reach * 2.0, true)
                    .unwrap()
                    .0
            };
            assert!(
                (cast(hull) - cast(&occupied)).abs() < 1e-5,
                "contact separation changed chunk {chunk}'s ballistic face on {ray}"
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

/// The game's construction field geometry reduces by ray and source entity, independent of
/// worker assignment or input iteration. Equal-distance stops retain the first source entity.
#[test]
fn field_geometry_is_independent_of_workers_and_owner_iteration() {
    use bevy::tasks::{ParallelSlice, TaskPoolBuilder};
    // Fixture: overlapping columns provide exact distance ties; a translated column adds nearer
    // and farther stops. Two field rays pin source order within each owner.
    let kind = column_construction();
    let mut world = World::new();
    let roots: Vec<_> = (0..3).map(|_| world.spawn_empty().id()).collect();
    let owners: Vec<_> = roots
        .iter()
        .enumerate()
        .map(|(i, &root)| {
            let breakage = Breakage::new(kind.clone(), i as u16);
            let pose = Transform::from_xyz(if i == 2 { -2.0 } else { 0.0 }, 0.0, 0.0);
            let frame = Frame::new(&pose, kind.bounds());
            (root, frame, breakage)
        })
        .collect();
    let deposit = RawDeposit {
        rays: vec![1.0, 2.0]
            .into_iter()
            .map(|energy| RayDeposit {
                end: Vec3::new(0.5, 1.5, 0.0),
                direction: Dir3::NEG_X,
                energy,
                blow: Vec3::NEG_X * energy,
            })
            .collect(),
        blast: Some((
            Vec3::new(4.0, 1.5, 0.0),
            Blast::of(CRUSHING_BLAST_J, 0.05, FIT_ONLY_PRODUCTS),
        )),
        ..default()
    };
    let observe = |geometry| {
        let (fields, stops) = reduce_geometry(geometry, true);
        assert!(!fields.is_empty());
        assert!(stops.iter().flatten().any(|stop| stop.surface.is_some()));
        for (_, field) in &fields {
            assert_eq!(
                field.iter().map(|stop| stop.2).collect::<Vec<_>>(),
                vec![1.0, 2.0]
            );
        }
        let mut stops = stops.into_iter();
        let hits = fracture::trace_blast(
            deposit.blast.unwrap().1,
            fracture::blast_directions(),
            |_| stops.next().unwrap().and_then(|stop| stop.surface),
        );
        (fields, hits)
    };
    let expected = observe(
        owners
            .iter()
            .map(|(root, frame, breakage)| owner_geometry(*root, *frame, breakage, true, &deposit))
            .collect(),
    );
    let first = *roots[..2].iter().min().unwrap();
    assert!(
        expected
            .1
            .iter()
            .all(|(stop, _)| !matches!(stop, Stop::Chunk(root, _) if *root != first))
    );
    for threads in [1, 2] {
        let pool = TaskPoolBuilder::new().num_threads(threads).build();
        for reverse in [false, true] {
            let mut order: Vec<_> = owners.iter().collect();
            if reverse {
                order.reverse();
            }
            let geometry = order
                .par_splat_map(&pool, None, |_, owners| {
                    owners
                        .iter()
                        .map(|(root, frame, breakage)| {
                            owner_geometry(*root, *frame, breakage, true, &deposit)
                        })
                        .collect::<Vec<_>>()
                })
                .into_iter()
                .flatten()
                .collect();
            assert_eq!(observe(geometry), expected);
        }
    }
}

/// The game's jointless owners still wear, keep drawing holes and launch. Having no internal
/// stress degrees of freedom must not swallow the deposit bookkeeping.
#[test]
fn a_jointless_owner_still_wears_keeps_holes_and_launches() {
    // Fixture: the middle cube of a column, transferred alone with a quarter-volume deposit.
    let kind = column_construction();
    let mut live = Live::new(&kind).group(&[1]);
    let pool = live.state.pool[1];
    let load = Load {
        chunk: 1,
        at: live.structure.chunks[1].centroid,
        crushed: pool / 4.0,
        energy: 0.0,
        blow: Vec3::Z,
        press: Vec3::X,
        peak: None,
        origin: Origin::Field,
    };
    let hole = Hole {
        from: [0; 3],
        to: [1; 3],
        radius: 1,
    };
    let changes = run_laws(
        &kind.layout.element_of,
        &mut live,
        &[load],
        vec![(1, hole)],
        Vec3::ZERO,
        STEP,
    );
    assert_eq!(live.state.pool[1], pool - load.crushed);
    assert_eq!(live.holes[1], vec![hole]);
    assert!(changes.events.contains(&Event::Hole(1, hole)));
    assert_eq!(live.state.fate[1], Fate::Fallen);
    assert_eq!(changes.groups, vec![(vec![1], vec![load])]);
    assert!(changes.failures.is_empty());
    assert_eq!(changes.solves, 0);
}

/// The game's independent construction owners have no cross-owner field effects. Worker count
/// and owner iteration must not change their law state, record, splits or prepared launches.
#[test]
fn owner_laws_and_prepared_groups_do_not_depend_on_worker_assignment() {
    use bevy::tasks::{ParallelSliceMut, TaskPoolBuilder};
    // Fixture: a placed column, a moving pair and a jointless owner, all taking external fields.
    let kind = column_construction();
    let mut world = World::new();
    let roots: Vec<_> = (0..3).map(|_| world.spawn_empty().id()).collect();
    let run = |threads, reverse| {
        let mut cases: Vec<_> = roots
            .iter()
            .enumerate()
            .map(|(i, &root)| {
                let mut breakage = Breakage::new(kind.clone(), i as u16);
                let owned = match i {
                    0 => None,
                    1 => Some(vec![1_u16, 2]),
                    _ => Some(vec![1_u16]),
                };
                if let Some(owned) = &owned {
                    let group: Vec<_> = owned.iter().map(|&c| usize::from(c)).collect();
                    breakage.live = Box::new(breakage.live.group(&group));
                }
                // Fixture separated partitions exercise launches and splits without a strength calibration.
                if i < 2 {
                    breakage.live.state.cracked.fill(true);
                    breakage.live.state.separated.fill(true);
                }
                let c = if i == 2 { 1 } else { 2 };
                let taken = Taken {
                    line_loads: vec![Load {
                        chunk: c,
                        at: breakage.live.structure.chunks[c].centroid,
                        energy: 0.0,
                        crushed: breakage.live.state.pool[c] / 4.0,
                        blow: Vec3::X * 1000.0,
                        press: Vec3::ZERO,
                        peak: None,
                        origin: Origin::Field,
                    }],
                    ..default()
                };
                (root, breakage, owned, taken)
            })
            .collect();
        if reverse {
            cases.reverse();
        }
        let pool = TaskPoolBuilder::new().num_threads(threads).build();
        let mut results: Vec<_> = cases
            .par_splat_map_mut(&pool, None, |_, cases| {
                cases
                    .iter_mut()
                    .map(|(root, breakage, owned, taken)| {
                        let pose = Transform::IDENTITY;
                        let effect = evaluate_owner(
                            *root,
                            breakage,
                            owned.as_deref(),
                            pose,
                            OwnerInputs {
                                step: STEP,
                                pool: &pool,
                                reusable: &ReusableColliders::Standing(BTreeMap::new()),
                                taken,
                                pressing: &[],
                                resting: &[],
                                // Fixture spin, 4 rad/s about the construction's Z.
                                motion: sweep_intake::ReleasedMotion {
                                    velocity: Vec3::ZERO,
                                    spin: pose.rotation * Vec3::Z * 4.0,
                                    centre: Vec3::ZERO,
                                },
                                tick: 1,
                            },
                        );
                        let splits: Vec<_> = effect
                            .split
                            .into_iter()
                            .flat_map(|split| split.children)
                            .map(|(group, live)| (group, live.state))
                            .collect();
                        let departures: Vec<_> = effect
                            .departures
                            .into_iter()
                            .map(prepared_group_state)
                            .collect();
                        (
                            *root,
                            breakage.live.state.clone(),
                            effect.events,
                            effect.fields,
                            splits,
                            departures,
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .into_iter()
            .flatten()
            .collect();
        results.sort_unstable_by_key(|result| result.0);
        results
    };
    let expected = run(1, false);
    assert!(
        expected.iter().any(|result| !result.2.is_empty()),
        "fields change game state"
    );
    assert!(
        expected.iter().any(|result| !result.5.is_empty()),
        "a placed owner prepares a launch"
    );
    assert!(
        expected.iter().any(|result| !result.4.is_empty()),
        "a moving owner prepares a split"
    );
    for threads in [1, 2] {
        for reverse in [false, true] {
            assert_eq!(run(threads, reverse), expected);
        }
    }
}

fn prepared_group_state(prepared: PreparedDeparture) -> impl std::fmt::Debug + PartialEq {
    let departure = prepared.departure;
    let launch = departure.launch;
    (
        (
            departure.construction,
            departure.root,
            departure.tick,
            departure.group,
            departure.live.state,
            departure.live.holes,
        ),
        (
            launch.centroid,
            launch.centre,
            launch.mass_properties.mass,
            launch.mass_properties.angular_inertia_tensor().to_mat3(),
            launch.velocity,
            launch.spin,
        ),
        prepared
            .chunks
            .into_iter()
            .map(|chunk| {
                let occupied = match chunk.occupied {
                    ColliderSource::Rehome(entity) => Err(entity),
                    ColliderSource::Fresh(occupied) => {
                        Ok((occupied.0.mass_properties(1.0), occupied.1, occupied.3))
                    }
                };
                let contact = match chunk.contact {
                    ColliderSource::Rehome(entity) => Err(entity),
                    ColliderSource::Fresh(contact) => {
                        Ok((contact.0.mass_properties(1.0), contact.1, contact.2))
                    }
                };
                (chunk.local, occupied, contact)
            })
            .collect::<Vec<_>>(),
    )
}

/// The game's split preparation must finish in child order with identical geometry, wear and
/// inherited motion on any worker. Ordered retirement and publication rely on those bundles.
#[test]
fn split_preparation_keeps_child_order_and_state_across_workers() {
    use bevy::tasks::TaskPoolBuilder;
    // Fixture: two groups in deliberately non-address order, with wear on the last cube.
    let kind = column_construction();
    let mut breakage = Breakage::new(kind, 7);
    breakage.live.state.pool[2] /= 2.0;
    let groups = [vec![2], vec![0, 1]];
    let split = PendingSplit {
        children: groups
            .iter()
            .map(|group| (group.clone(), breakage.live.group(group)))
            .collect(),
        tick: 11,
    };
    let root = Transform::from_xyz(20.0, 10.0, -3.0).with_rotation(Quat::from_rotation_y(0.7));
    let motion = sweep_intake::ReleasedMotion {
        velocity: Vec3::new(2.0, -3.0, 1.0),
        spin: Vec3::new(0.3, 0.5, -0.2),
        centre: Vec3::new(0.0, 1.5, 0.0),
    };
    // Fixture: each chunk's two colliders, as a piece carries them.
    let mut world = World::new();
    let reusable = ReusableColliders::Piece(
        (0..3)
            .map(|c| (c, [(); 2].map(|_| world.spawn_empty().id())))
            .collect(),
    );
    let run = |threads| {
        let pool = TaskPoolBuilder::new().num_threads(threads).build();
        let prepared = split.prepare(
            &pool,
            &breakage,
            breakage.construction,
            root,
            motion,
            &reusable,
        );
        assert_eq!(
            prepared
                .iter()
                .map(|prepared| &prepared.departure.group)
                .collect::<Vec<_>>(),
            groups.iter().collect::<Vec<_>>()
        );
        prepared
            .into_iter()
            .map(prepared_group_state)
            .collect::<Vec<_>>()
    };
    assert_eq!(run(1), run(2));
}

/// Replicas rely on append-only records within one generation; silently resetting colliders
/// on a shortening would conceal a broken record writer.
#[test]
#[should_panic(expected = "a construction record never shrinks within its generation")]
fn deriving_a_shorter_same_generation_record_is_a_bug() {
    let mut app = App::new();
    app.add_plugins(TaskPoolPlugin::default())
        .init_resource::<Record>()
        .add_systems(Update, derive_from_record);
    app.world_mut()
        .spawn(Breakage::new(column_construction(), 3));
    app.world_mut()
        .resource_mut::<Record>()
        .entries
        .push(Entry {
            construction: 3,
            event: Event::Wear(0),
            tick: 0,
        });
    app.update();
    app.world_mut().resource_mut::<Record>().entries.clear();
    app.update();
}

/// The game's damage model reads the same occupied matter while intact, released, and asleep at
/// a different pose. Flight, field, corridor and aim consumers rely on the common constructors
/// keeping that geometry readable. Fixture support loss isolates the column's middle chunk.
#[test]
fn occupied_matter_queries_follow_common_release_and_sleeping_pose() {
    let kind = column_construction();
    let (mut app, root) = composed_kind(false, &kind);
    app.update();
    app.update();
    let occupied = |app: &mut App, owner| {
        app.world_mut()
            .query::<(Entity, &ChildOf, &BallisticVolume)>()
            .iter(app.world())
            .find(|(_, parent, _)| parent.parent() == owner)
            .unwrap()
            .0
    };
    let volume = occupied(&mut app, root);
    crate::ballistics::assert_occupied_matter_queries(&mut app, volume);
    let chunk = kind.layout.blocks.len() / 2; // Fixture: the column's middle cube.
    isolate(&mut app, root, &[chunk]);
    app.update();
    app.update();
    let owner = app
        .world_mut()
        .query::<(Entity, &Piece)>()
        .iter(app.world())
        .find(|(_, piece)| piece.chunks == [chunk as u16])
        .expect("common support loss releases the isolated chunk")
        .0;
    // Fixture poses separate the released chunk from all other matter, then turn it sideways.
    for (at, turn, sleeping) in [
        (Vec3::new(10.0, 5.0, 3.0), Quat::IDENTITY, false),
        (
            Vec3::new(15.0, 2.0, 4.0),
            Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            true,
        ),
    ] {
        app.world_mut().entity_mut(owner).insert((
            Transform::from_translation(at).with_rotation(turn),
            Position(at),
            Rotation(turn),
            LinearVelocity::ZERO,
            AngularVelocity::ZERO,
        ));
        app.update();
        app.update();
        if sleeping {
            app.world_mut().entity_mut(owner).insert(Sleeping);
            app.update();
            app.update();
            assert!(app.world().get::<Sleeping>(owner).is_some());
        }
        let volume = occupied(&mut app, owner);
        crate::ballistics::assert_occupied_matter_queries(&mut app, volume);
    }
}

/// The game's environment destruction intake preserves source order and frame conversion across
/// worker assignments, while moving receivers contribute no structural load.
#[test]
fn contact_preparation_preserves_source_order_across_workers() {
    use bevy::ecs::system::{RunSystemOnce, SystemState};
    use bevy::tasks::TaskPoolBuilder;
    let kind = column_construction();
    let mut world = World::new();
    let mut inputs = Vec::new();
    for ordinal in 0..3 {
        let root = world
            .spawn((
                Position(Vec3::X * ordinal as f32),
                Rotation::default(),
                Breakage::new(kind.clone(), ordinal),
            ))
            .id();
        if ordinal == 1 {
            world.entity_mut(root).insert(Piece {
                construction: ordinal,
                chunks: vec![0],
                born: 0,
                retired: None,
            });
        }
        let collider = world.spawn((Element(0), ChildOf(root))).id();
        for sequence in 0..4 {
            inputs.push(ContactInput {
                point: ContactTransfer {
                    reference: root,
                    reference_collider: collider,
                    other: root,
                    other_collider: None,
                    point: Vec3::X * ordinal as f32 + kind.layout.element_blocks[0].points[0],
                    normal: Vec3::Y,
                    impulse: Vec3::ZERO,
                    effective_mass: 7.0,
                    closing_speed: 2.0,
                },
                press: Vec3::Y * (sequence + 1) as f32,
                step: STEP,
            });
        }
    }
    // Fixture: interleave owners while retaining each source sequence.
    inputs.rotate_left(3);
    let mut state: SystemState<(
        Colliders,
        Query<((&Position, &Rotation), &Breakage, Option<&Piece>)>,
    )> = SystemState::new(&mut world);
    let (colliders, constructions) = state.get(&world).unwrap();
    let expected: Vec<_> = inputs
        .iter()
        .map(|i| contact_load(&colliders, &constructions, &i.point, i.press, i.step))
        .collect();
    assert!(
        expected.iter().any(Option::is_some) && expected.iter().any(Option::is_none),
        "the fixture has standing receivers and moving groups with no structural load"
    );
    for threads in [1, 2] {
        let pool = TaskPoolBuilder::new().num_threads(threads).build();
        assert_eq!(
            prepare_contacts(
                &pool,
                &inputs
                    .iter()
                    .copied()
                    .map(ContactSource::Point)
                    .collect::<Vec<_>>(),
                &ContactTransfers::default(),
                &colliders,
                &constructions,
                None
            ),
            expected
        );
    }
    let groups = inputs
        .iter()
        .map(|input| {
            vec![ContactTransfer {
                impulse: -input.press * input.step,
                ..input.point
            }]
        })
        .collect();
    world.insert_resource(ContactTransfers::fixture(groups, STEP));
    world.init_resource::<Stages>();
    world.init_resource::<Queue>();
    world.init_resource::<RestingLoads>();
    ComputeTaskPool::get_or_init(Default::default);
    world.run_system_once(read_contacts).unwrap();
    let queued: Vec<_> = world
        .resource::<Queue>()
        .0
        .iter()
        .flat_map(|deposit| deposit.contacts.iter().copied())
        .collect();
    assert_eq!(
        queued,
        expected.iter().flatten().copied().collect::<Vec<_>>(),
        "the intake keeps exactly the standing loads and their source order"
    );
    let roots: BTreeSet<_> = expected.iter().flatten().map(|(root, _)| *root).collect();
    assert_eq!(
        world.resource::<Stages>().take_counts().values[Work::ContactReadRoots as usize],
        roots.len() as u64,
        "moving receivers never enter the owner task grouping"
    );
}

/// Collider preparation and its committed geometry are independent of worker assignment.
/// The existing march/refit witness checks same-update visibility through the shipped plugin.
#[test]
fn reconciliation_prepares_the_same_ordered_geometry_across_workers() {
    use bevy::ecs::system::RunSystemOnce;
    use bevy::tasks::TaskPoolBuilder;
    let run = |threads| {
        let kind = column_construction();
        let (mut app, first) = composed_kind(false, &kind);
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, app.world());
        let second = crate::construction::spawn(
            &mut commands,
            &kind,
            MapInstance {
                id: "fixture-second".into(),
                ordinal: 2,
            },
            Isometry3d::from_translation(Vec3::new(5.0, 0.0, 0.0)),
        );
        queue.apply(app.world_mut());
        {
            let mut breakage = app.world_mut().get_mut::<Breakage>(first).unwrap();
            breakage.live.swapped.fill(true);
            breakage.live.state.fate[0] = Fate::Dust;
        }
        // Leave the other owner intact to prepare both element and chunk effects.
        let children: Vec<_> = app
            .world()
            .get::<Children>(second)
            .unwrap()
            .iter()
            .collect();
        for child in children {
            app.world_mut().despawn(child);
        }
        app.world_mut()
            .run_system_once(
                move |owners: Query<
                    (Entity, &Transform, &Breakage, Option<&Children>),
                    (Changed<Breakage>, Without<Piece>),
                >,
                      elements: Query<&Element>,
                      chunks: Query<&ChunkCollider>,
                      mut commands: Commands| {
                    let pool = TaskPoolBuilder::new().num_threads(threads).build();
                    for change in prepare_reconciliations(&pool, &owners, &elements, &chunks)
                        .into_iter()
                        .flat_map(|(_, changes)| changes)
                    {
                        change.apply(&mut commands);
                    }
                },
            )
            .unwrap();
        let world = app.world_mut();
        let mut result: Vec<_> = world
            .query::<(
                &ChildOf,
                Option<&Element>,
                Option<&ChunkCollider>,
                &Collider,
                &Position,
                &Rotation,
                &Friction,
            )>()
            .iter(world)
            .map(
                |(parent, element, chunk, collider, position, rotation, friction)| {
                    let ordinal = world.get::<Breakage>(parent.parent()).unwrap().construction;
                    let aabb = collider.aabb(position.0, *rotation);
                    (
                        ordinal,
                        element.map(|e| e.0),
                        chunk.map(|c| c.0),
                        position.0,
                        rotation.0,
                        aabb.min,
                        aabb.max,
                        *friction,
                    )
                },
            )
            .collect();
        result.sort_by_key(|(ordinal, element, chunk, ..)| (*ordinal, *element, *chunk));
        assert!(result.iter().any(|(_, element, ..)| element.is_some()));
        assert!(result.iter().any(|(_, _, chunk, ..)| chunk.is_some()));
        result
    };
    assert_eq!(run(1), run(2));
}

/// The game's fracture work is spent once across departing owners. Copying the complete work
/// onto each side, or adding source-point energies instead of coherent impulses, fails this gate.
#[test]
fn released_owners_share_one_openings_work_and_each_load_has_one_owner() {
    let material = Material::of(
        crate::substances::SubstanceRegistry::shipped_static(),
        "Brick",
    )
    .unwrap()
    .unwrap();
    // Fixture: two unit-volume nodes, a unit-area seam and a central impulse on each side.
    let area = 1.0;
    let mass = material.density;
    let work = material.seam.fracture_energy * area;
    let impulse = (4.0 * work * mass).sqrt();
    let structure = Structure {
        geometry: crate::construction::fracture::bondsolve::Geometry::CircularFixture,
        part_of: vec![0, 1],
        chunks: [-0.5, 0.5]
            .map(|x| Chunk {
                volume: 1.0,
                mass,
                centroid: Vec3::X * x,
                material,
                buried: false,
            })
            .to_vec(),
        joints: vec![Joint {
            a: 0,
            b: Side::Chunk(1),
            area,
            centroid: Vec3::ZERO,
            normal: Vec3::X,
            bond: Bond::Bonded(material.seam),
        }],
    };
    let loads: Vec<_> = structure
        .chunks
        .iter()
        .enumerate()
        .map(|(chunk, c)| Load {
            chunk,
            at: c.centroid,
            blow: Vec3::X * impulse,
            press: Vec3::ZERO,
            energy: 0.0,
            crushed: 0.0,
            peak: None,
            origin: Origin::Field,
        })
        .collect();
    let failures = [fracture::Failure {
        joint: 0,
        cause: fracture::Cause::Tension,
        demand: material.seam.tensile,
        capacity: material.seam.tensile,
        work,
    }];
    let groups = departure_loads(&structure, vec![vec![0], vec![1]], &loads, &failures);
    assert_eq!(
        groups.iter().map(|(_, loads)| loads.len()).sum::<usize>(),
        loads.len()
    );
    let initial: f32 = loads
        .iter()
        .map(|l| l.blow.length_squared() / (2.0 * mass))
        .sum();
    let remaining: f32 = groups
        .iter()
        .flat_map(|(_, loads)| loads)
        .map(|l| l.blow.length_squared() / (2.0 * mass))
        .sum();
    assert!(
        (initial - remaining - work).abs() <= 1e-5 * work,
        "{initial} - {remaining} != {work}"
    );
}

/// Initial gravity is construction data prepared before play. Live owners share it, but a
/// changed owner's warm iteration and topology can never write into another owner's solution.
#[test]
fn owners_share_initial_equilibrium_until_their_first_changed_load() {
    let kind = column_construction();
    let mut first = Live::new(&kind);
    let second = Live::new(&kind);
    assert!(Arc::ptr_eq(&first.assessment, &second.assessment));
    assert!(
        !first.assessment.needs_tick(),
        "the supported column is at rest"
    );
    let c = first.structure.chunks.len() - 1;
    let load = Load {
        chunk: c,
        at: first.structure.chunks[c].centroid,
        press: Vec3::X,
        blow: Vec3::ZERO,
        energy: 0.0,
        crushed: 0.0,
        peak: None,
        origin: Origin::Field,
    };
    run_laws(
        &kind.layout.element_of,
        &mut first,
        &[load],
        Vec::new(),
        Vec3::ZERO,
        STEP,
    );
    assert!(!Arc::ptr_eq(&first.assessment, &second.assessment));
    assert!(Arc::ptr_eq(&second.assessment, &kind.equilibrium));
    assert!(!second.assessment.needs_tick());
}

/// The game's environment destruction: a cut that swaps an element to chunks keeps the belt
/// support staged on its standing matter. The reaction on the chunk that leaves is cancelled on
/// its source; the one on the chunk that stands reaches the physics step's reading on that
/// chunk's own collider, and its force stays on the source. Cancelling every reaction on the
/// retiring element collider takes a tick of real support away; keeping the one on departed
/// matter leaves a reaction with no surface to come from.
#[test]
fn a_partial_cut_keeps_belt_support_on_the_matter_that_stands() {
    use bevy::ecs::system::RunSystemOnce;
    let kind = resolved_column();
    let (mut app, root) = composed_kind(false, &kind);
    for _ in 0..3 {
        app.update();
    }
    let chunks = &kind.structure.chunks;
    let side = chunks[1].centroid.y - chunks[0].centroid.y;
    // Fixture: a 1e6 kg cube one chunk side from the middle, travelling that distance per tick;
    // it frees the middle and top chunks while the anchored bottom chunk stands.
    let (mass, speed) = (1e6, side / STEP);
    let start = chunks[1].centroid - Vec3::X * side;
    let source = app.world_mut().spawn(cube(start, mass)).id();
    app.update();
    let element = app
        .world_mut()
        .query::<(Entity, &Element, &ChildOf)>()
        .iter(app.world())
        .find(|(_, _, parent)| parent.parent() == root)
        .unwrap()
        .0;
    // Fixture: belt reactions on the element's face at the chunk that leaves and at the one that
    // stands, with support large enough to expose a lost reaction in the momentum assertion.
    let belts = [1, 0].map(|c| crate::contact_transfer::AppliedContact {
        structure: root,
        structure_collider: element,
        body: source,
        point: chunks[c].centroid - Vec3::X * side / 2.0,
        normal: Vec3::NEG_X,
        force: Vec3::Y * 1e7,
        closing_speed: 0.0,
    });
    app.world_mut()
        .run_system_once(move |mut bodies: Query<Forces>| {
            for belt in belts {
                bodies
                    .get_mut(source)
                    .unwrap()
                    .apply_force_at_point(belt.force, belt.point);
            }
        })
        .unwrap();
    for belt in belts {
        app.world_mut()
            .resource_mut::<ContactTransfers>()
            .stage(belt);
    }
    app.world_mut().get_mut::<LinearVelocity>(source).unwrap().0 = Vec3::X * speed;
    app.update();
    let cancelled: Vec<_> = (app.world().resource::<SweepAccount>().cancelled_belts)
        .iter()
        .map(|belt| belt.point)
        .collect();
    assert_eq!(
        cancelled,
        [belts[0].point],
        "only departed matter's reaction ceases"
    );
    let kept: Vec<_> = app
        .world()
        .resource::<ContactTransfers>()
        .groups()
        .flatten()
        .filter(|point| point.other == source && point.other_collider.is_none())
        .map(|point| {
            let collider = app.world().entity(point.reference_collider);
            (
                collider.get::<ChunkCollider>().copied(),
                collider.get::<ChildOf>().map(ChildOf::parent),
                point.impulse,
            )
        })
        .collect();
    assert_eq!(
        kept,
        [(Some(ChunkCollider(0)), Some(root), belts[1].force * STEP)],
        "the standing chunk's own collider carries its reaction"
    );
    let rubble: DVec3 = app
        .world_mut()
        .query::<(&Piece, &ComputedMass, &LinearVelocity)>()
        .iter(app.world())
        .filter(|(piece, ..)| piece.construction == 3)
        .map(|(_, m, v)| f64::from(m.value()) * v.0.as_dvec3())
        .sum();
    let after = app.world().get::<LinearVelocity>(source).unwrap().0;
    let expected = f64::from(mass * speed) * DVec3::X + (belts[1].force * STEP).as_dvec3();
    let total = f64::from(mass) * after.as_dvec3() + rubble;
    // Resolution: f32 velocity and mass roundoff, about 1e-7 relative (DERIVED), with margin.
    assert!(
        (total - expected).length() < 1e-4 * expected.length(),
        "the standing support stays on the source: {total:?} against {expected:?}"
    );
}

/// The game's environment destruction: the production pre-physics sweep removes the minimum-cut
/// plug containing its occupied source patch on first contact, before the solver. At zero
/// gravity the hull's momentum plus its freed rubble's is conserved over the tick: a stop or
/// exchange against the standing construction, or a stale belt reaction left uncancelled, is
/// external to that pair and breaks it. Matter the sweep frees this tick neither brakes nor lifts
/// the hull in the solver: the hull ends the tick at its post-exchange velocity and travels on it.
#[test]
fn a_sweep_opens_the_swept_chunk_before_the_solver_and_keeps_momentum() {
    let kind = resolved_column();
    let (mut app, root) = composed_kind(false, &kind);
    for _ in 0..3 {
        app.update();
    }
    // Fixture: a centred 0.4 m cube one chunk side short of the middle chunk's centre, crossing
    // only that chunk within the tick.
    let middle = kind.structure.chunks[1].centroid;
    let side = middle.y - kind.structure.chunks[0].centroid.y;
    let start = middle - Vec3::X * side;
    let speed = 128.0;
    let mass = 1e6;
    let source = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Position(start),
            Rotation::default(),
            Collider::cuboid(0.4, 0.4, 0.4),
            Mass(mass),
            NoAutoMass,
            AngularInertia::new(Vec3::splat(mass)),
            CenterOfMass::default(),
            LinearVelocity(Vec3::X * speed),
            piece_layers(),
        ))
        .id();
    // First preparation creates the collider hierarchy; start outside this tick's sweep.
    app.world_mut().get_mut::<LinearVelocity>(source).unwrap().0 = Vec3::ZERO;
    app.update();
    use bevy::ecs::system::RunSystemOnce;
    let old = app
        .world_mut()
        .query::<(Entity, &Element, &ChildOf)>()
        .iter(app.world())
        .find(|(_, _, parent)| parent.parent() == root)
        .unwrap()
        .0;
    // Fixture: a stale belt reaction whose tick impulse (1.6e5 N s) exceeds the 1.3e4 N s
    // conservation tolerance below.
    let force = Vec3::NEG_X * 1e7;
    app.world_mut()
        .run_system_once(move |mut bodies: Query<Forces>| {
            bodies
                .get_mut(source)
                .unwrap()
                .apply_force_at_point(force, start);
        })
        .unwrap();
    app.world_mut().resource_mut::<ContactTransfers>().stage(
        crate::contact_transfer::AppliedContact {
            structure: root,
            structure_collider: old,
            body: source,
            point: middle - Vec3::X * side / 2.0,
            normal: Vec3::NEG_X,
            force,
            closing_speed: speed,
        },
    );
    app.world_mut().get_mut::<LinearVelocity>(source).unwrap().0 = Vec3::X * speed;
    app.update();
    let account = app.world().resource::<SweepAccount>();
    let selected: Vec<_> = account
        .chunks
        .iter()
        .filter(|(ordinal, _, _)| *ordinal == 3)
        .copied()
        .collect();
    assert!(
        selected.iter().any(|chunk| chunk.1 == 1),
        "the occupied middle chunk belongs to the plug"
    );
    let (_, _, exchanged, _) = *account
        .source_changes
        .iter()
        .find(|(body, ..)| *body == source)
        .unwrap();
    let after = app.world().get::<LinearVelocity>(source).unwrap().0;
    let rubble: DVec3 = app
        .world_mut()
        .query::<(&Piece, &ComputedMass, &LinearVelocity)>()
        .iter(app.world())
        .filter(|(piece, ..)| piece.construction == 3)
        .map(|(_, m, v)| f64::from(m.value()) * v.0.as_dvec3())
        .sum();
    assert!(rubble != DVec3::ZERO, "the plug leaves as server rubble");
    let before = f64::from(mass * speed) * DVec3::X;
    let total = f64::from(mass) * after.as_dvec3() + rubble;
    // Resolution: f32 velocity and mass roundoff, about 1e-7 relative (DERIVED), with margin.
    assert!(
        (total - before).length() < 1e-4 * before.length(),
        "hull and freed rubble keep the hull's momentum: {total:?} against {before:?}"
    );
    // Resolution: f32 velocity roundoff at 128 m/s.
    assert!(
        (after - exchanged).length() < 1e-5 * speed,
        "the solver changes no exchanged hull velocity: {after:?} against {exchanged:?}"
    );
    // The plug's centre of mass lies off the hull's line, so the exchange alone moves it vertically.
    let rise = app.world().get::<Position>(source).unwrap().0.y - start.y;
    assert!(
        (rise - exchanged.y * STEP).abs() < 1e-5 * speed * STEP,
        "freed matter lifts no hull: rose {rise} m at {} m/s exchanged",
        exchanged.y
    );
    assert!(
        !app.world()
            .get::<Breakage>(root)
            .unwrap()
            .live
            .state
            .standing(1)
    );
    let mut shape = Shape::intact(&kind.layout);
    for entry in &app.world().resource::<Record>().entries {
        shape.apply(entry.event, &kind.layout);
    }
    let live = Live::new(&kind);
    shape.derive_departures(&kind.layout, &live);
    for &(_, c, _) in &selected {
        assert_eq!(
            shape.fate[usize::from(c)],
            app.world().get::<Breakage>(root).unwrap().live.state.fate[usize::from(c)],
            "the streamed ledger agrees with the authority for all freed matter"
        );
    }
}

/// The game's environment destruction: the collision hook keeps Avian's contact for a body the
/// sweep did not cast this tick, and drops look-ahead only where a cast says its body does not
/// reach. Read as a cast that missed, no cast would take every look-ahead point on standing
/// matter from a departure past the round cap or a body woken in the step, matter no cut judged.
#[test]
fn a_body_the_sweep_did_not_cast_keeps_avians_contact() {
    use bevy::ecs::system::RunSystemOnce;
    let kind = resolved_column();
    let (mut app, root) = composed_kind(false, &kind);
    for _ in 0..3 {
        app.update();
    }
    let middle = kind.structure.chunks[1].centroid;
    let side = middle.y - kind.structure.chunks[0].centroid.y;
    // Fixture: two cubes clear of the middle chunk's face; at zero gravity the one at rest has no
    // motion to cast, and the other moves away from the column.
    let [resting, leaving] = [1.0, 2.0].map(|offset| {
        app.world_mut()
            .spawn(cube(middle - Vec3::X * side * offset, 1e3))
            .id()
    });
    app.update(); // First preparation creates the collider hierarchy at rest.
    app.world_mut()
        .get_mut::<LinearVelocity>(leaving)
        .unwrap()
        .0 = Vec3::NEG_X;
    app.update();
    let element = app
        .world_mut()
        .query::<(Entity, &Element, &ChildOf)>()
        .iter(app.world())
        .find(|(_, _, parent)| parent.parent() == root)
        .unwrap()
        .0;
    let face = middle - Vec3::X * side / 2.0;
    let read = app
        .world_mut()
        .run_system_once(move |hooks: SweepContacts| {
            [resting, leaving].map(|body| hooks.unjudged(element, body, face))
        })
        .unwrap();
    assert_eq!(
        read,
        [Some(false), Some(true)],
        "no cast keeps Avian's contact; a cast that misses covered matter drops its look-ahead"
    );
}

/// The game's environment destruction: a detected target reaches the mounted intake and keeps
/// the solver's look-ahead point, including a target with no inward witness or cut verdict.
/// Filtering intake by witness presence would erase the unresolved target's solver contact.
#[test]
fn detected_targets_keep_lookahead_contacts_through_intake() {
    use super::super::sweep::tests::{
        failed_contact_hulls, failed_contact_kind, failed_contact_motion, failed_linear_motion,
        failed_overlap_motion,
    };
    use bevy::ecs::system::RunSystemOnce;
    let kind = failed_contact_kind();
    let (hull, target) = failed_contact_hulls();
    for (motion, has_witness) in [
        (failed_contact_motion(&hull), true),
        (failed_overlap_motion(&hull), false),
        (failed_linear_motion(&hull), true),
    ] {
        let (mut app, root) = composed_kind(false, &kind);
        for _ in 0..3 {
            app.update();
        }
        // Fixture: a light source, prepared away from contact at rest, so the captured motion
        // cannot release the anchored target. Its COM and shape retain the capture's frame.
        let source = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Position(motion.position + Vec3::Y * 10.0),
                Rotation(motion.rotation),
                hull.clone(),
                Mass(1e-6),
                NoAutoMass,
                AngularInertia::new(Vec3::ONE),
                NoAutoAngularInertia,
                CenterOfMass(motion.rotation.inverse() * (motion.centre - motion.position)),
                NoAutoCenterOfMass,
                LinearVelocity::ZERO,
                AngularVelocity::ZERO,
                piece_layers(),
            ))
            .id();
        app.update();
        app.world_mut().entity_mut(source).insert((
            Position(motion.position),
            LinearVelocity(motion.velocity),
            AngularVelocity(motion.spin),
        ));
        app.update();
        let owner = app.world().get::<Breakage>(root).unwrap();
        assert!(
            owner.live.state.standing(0),
            "the fixture target must remain standing"
        );
        let account = app.world().resource::<SweepAccount>();
        assert!(
            account.sources > 0 && account.work.casts > 0,
            "the mounted sweep cast the source against the target"
        );
        assert_eq!(
            account.contacts.contains_key(&(source, owner.construction)),
            has_witness,
            "only usable inward geometry may produce a load"
        );
        let element = app
            .world_mut()
            .query::<(Entity, &Element, &ChildOf)>()
            .iter(app.world())
            .find(|(_, _, parent)| parent.parent() == root)
            .unwrap()
            .0;
        // Fixture: a solver look-ahead point on the fixed target's face, separated by 1 mm.
        let face = Vec3::new(target.aabb(Vec3::ZERO, Quat::IDENTITY).max.x, 0.0, 0.0);
        let (unjudged, kept) = app
            .world_mut()
            .run_system_once(move |hooks: SweepContacts, mut commands: Commands| {
                let unjudged = hooks.unjudged(element, source, face);
                let mut pair = ContactPair::new(
                    element,
                    source,
                    avian3d::collision::contact_types::ContactId::PLACEHOLDER,
                );
                pair.body1 = Some(root);
                pair.body2 = Some(source);
                pair.manifolds.push(ContactManifold::new(
                    [ContactPoint::new(Vec3::ZERO, Vec3::ZERO, face, -0.001)],
                    Vec3::X,
                ));
                let kept = avian3d::prelude::CollisionHooks::modify_contacts(
                    &hooks,
                    &mut pair,
                    &mut commands,
                );
                (unjudged, kept)
            })
            .unwrap();
        assert_eq!(
            (unjudged, kept),
            (Some(false), true),
            "a detected target keeps Avian's look-ahead point"
        );
    }
}

/// Two 1e6 kg, 0.4 m cubes at rest one chunk side short of `resolved_column`'s middle chunk and
/// of chunk `later`, the first by entity order at the middle; the first is launched at 128 m/s across its chunk,
/// then the second, in the same tick or the next. Returns the app after the second's tick.
fn two_sources(
    kind: &Arc<ConstructionKind>,
    later: usize,
    same_tick: bool,
) -> (App, Entity, Entity) {
    let (mut app, _) = composed_kind(false, kind);
    for _ in 0..3 {
        app.update();
    }
    let chunks = &kind.structure.chunks;
    let side = chunks[1].centroid.y - chunks[0].centroid.y;
    let mut bodies = [1, later].map(|_| {
        app.world_mut()
            .spawn((
                RigidBody::Dynamic,
                Rotation::default(),
                Collider::cuboid(0.4, 0.4, 0.4),
                Mass(1e6),
                NoAutoMass,
                AngularInertia::new(Vec3::splat(1e6)),
                CenterOfMass::default(),
                LinearVelocity::ZERO,
                piece_layers(),
            ))
            .id()
    });
    // Entity order is the sweep's commit order: the earlier entity takes the middle chunk.
    bodies.sort();
    let [a, b] = bodies;
    for (body, chunk) in [(a, 1), (b, later)] {
        let start = chunks[chunk].centroid - Vec3::X * side;
        app.world_mut()
            .entity_mut(body)
            .insert((Position(start), Transform::from_translation(start)));
    }
    app.update(); // First preparation creates the collider hierarchy at rest.
    for (body, update) in [(a, !same_tick), (b, true)] {
        app.world_mut().get_mut::<LinearVelocity>(body).unwrap().0 = Vec3::X * 128.0;
        if update {
            app.update();
        }
    }
    (app, a, b)
}

/// The game's environment destruction: the sweep split tells a source whose bounds met no
/// standing owner from one whose bounds did. Reading `roots` wrong, or crediting a far source a
/// cast it never made, breaks the count the reader prices a no-owner source with.
#[test]
fn a_source_far_from_any_owner_counts_as_meeting_none() {
    let kind = resolved_column();
    let (mut app, _) = composed_kind(false, &kind);
    app.insert_resource(crate::cost::stages::Capture { active: true });
    for _ in 0..3 {
        app.update();
    }
    let chunks = &kind.structure.chunks;
    let side = chunks[1].centroid.y - chunks[0].centroid.y;
    let near = chunks[1].centroid - Vec3::X * side;
    // Fixture: one body at the column's middle chunk and one a kilometre from any construction.
    let bodies = [near, near + Vec3::X * 1000.0].map(|at| {
        app.world_mut()
            .spawn((
                RigidBody::Dynamic,
                Rotation::default(),
                Position(at),
                Transform::from_translation(at),
                Collider::cuboid(0.4, 0.4, 0.4),
                Mass(1e6),
                NoAutoMass,
                AngularInertia::new(Vec3::splat(1e6)),
                CenterOfMass::default(),
                LinearVelocity::ZERO,
                piece_layers(),
            ))
            .id()
    });
    app.update(); // First preparation creates the collider hierarchy at rest.
    for body in bodies {
        app.world_mut().get_mut::<LinearVelocity>(body).unwrap().0 = Vec3::X * 128.0;
    }
    app.update();
    let account = app.world().resource::<SweepAccount>();
    assert_eq!(
        account.sources_no_owner, 1,
        "only the far body met no owner"
    );
    assert!(account.sources > account.sources_no_owner);
    assert!(
        account.work.casts > 0,
        "the near body cast against the column"
    );
    assert!(
        (1..=account.work.casts).contains(&account.max_casts_per_source),
        "the busiest source's casts are among the tick's casts"
    );
}

/// The game's environment destruction: a later source on a construction an earlier source cut
/// this tick resolves on the topology that cut left, with the gravity field solved on it. Matter
/// the earlier cut freed is no later plug; matter still standing yields the plug and load a sweep
/// on that state reads a tick later, once the law has re-solved the owner. The account keeps the
/// resolution that committed.
#[test]
fn a_later_source_resolves_on_the_cut_an_earlier_one_left() {
    let kind = resolved_column();
    let exchanged = |app: &App, body: Entity| {
        app.world()
            .resource::<SweepAccount>()
            .source_changes
            .iter()
            .any(|(source, ..)| *source == body)
    };
    // The middle's cut leaves the crown unsupported.
    let (app, a, b) = two_sources(&kind, 2, true);
    assert!(exchanged(&app, a), "the earlier source frees its plug");
    assert!(
        !exchanged(&app, b),
        "matter an earlier cut freed this tick is no later source's plug"
    );
    // The base still stands below the middle's cut.
    let (contested, a, b) = two_sources(&kind, 0, true);
    assert!(exchanged(&contested, a) && exchanged(&contested, b));
    let (later, ..) = two_sources(&kind, 0, false);
    let resolution = |app: &App| {
        let account = app.world().resource::<SweepAccount>();
        let (.., plug, _) = account
            .patches
            .iter()
            .find(|(body, ..)| *body == b)
            .unwrap();
        let (.., load) = account
            .samples
            .iter()
            .find(|(body, ..)| *body == b)
            .unwrap();
        (plug.clone(), *load)
    };
    let ((plug, load), (expected_plug, expected)) = (resolution(&contested), resolution(&later));
    assert_eq!(plug, expected_plug);
    // Resolution: f32 casts and loads at a 1e6 kg, 128 m/s fixture.
    for (got, want) in [
        (load.0, expected.0),
        (load.1, expected.1),
        (load.3, expected.3),
    ] {
        assert!(
            (got - want).abs() <= 1e-4 * want.abs().max(1.0),
            "the re-resolved load is the one its state reads: {load:?} against {expected:?}"
        );
    }
}

/// The game's environment destruction: an end contact on a slender source rotates it away,
/// reducing the Hertz demand. The production intake must use that contact response, rather
/// than total mass, when deciding whether the standing construction holds.
#[test]
fn a_sweep_reads_the_slender_sources_contact_mass() {
    use crate::contact_law::{Contactor, peak_force};
    // Fixture: M = 128 kg, L = 2 m, transverse inertia ML²/12 = M/3 kg m².
    // The sphere is a contact proxy centred either at the COM or one metre above it.
    let mass = 128.0;
    let speed = 64.0;
    let mut peaks = Vec::new();
    for end in [false, true] {
        let kind = column_construction();
        let (mut app, _) = composed_kind(false, &kind);
        for _ in 0..3 {
            app.update();
        }
        let offset = if end { Vec3::NEG_Y } else { Vec3::ZERO };
        let position = Vec3::new(-1.0, 1.5, 0.0);
        let body = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Position(position),
                Rotation::default(),
                Collider::sphere(0.1),
                Mass(mass),
                NoAutoMass,
                AngularInertia::new(Vec3::splat(mass / 3.0)),
                NoAutoAngularInertia,
                CenterOfMass(offset),
                NoAutoCenterOfMass,
                LinearVelocity::ZERO,
                piece_layers(),
            ))
            .id();
        app.update();
        app.world_mut().get_mut::<LinearVelocity>(body).unwrap().0 = Vec3::X * speed;
        app.update();
        let account = app.world().resource::<SweepAccount>();
        let (_, _, _, witness) = account.patches.iter().find(|(b, ..)| *b == body).unwrap();
        let arm = witness.at - position - offset - Vec3::X * speed * witness.reach;
        let torque = arm.cross(witness.normal);
        let effective = (mass.recip() + torque.length_squared() / (mass / 3.0)).recip();
        let expected_mass = if end { mass / 4.0 } else { mass };
        println!(
            "DERIVED ideal contact mass={expected_mass}; MEASURED cast witness mass={effective}"
        );
        let demand = account
            .samples
            .iter()
            .find(|(b, ..)| *b == body)
            .unwrap()
            .3
            .0;
        let expected = peak_force(
            f64::from(effective),
            f64::from(witness.speed),
            Contactor::chunk(&kind.structure, witness.chunk),
            Contactor::RIGID,
        ) as f32;
        assert!(
            (demand / expected - 1.0).abs() < 1e-4,
            "peak {demand} != {expected}"
        );
        peaks.push(demand);
    }
    assert!(peaks[1] < peaks[0], "rotation reduces the contact peak");
}

/// Fixture for the game's environment destruction latency gate: an impact whose Hertz demand
/// exceeds a shear-cut bound derived without asking the sweep for its verdict.
#[test]
fn an_over_capacity_fixture_opens_within_two_contact_ticks() {
    // Fixture: column side lengths, m; each impact independently exceeds its cut capacity.
    for scale in [2.0, 3.0] {
        use crate::contact_law::{Contactor, peak_force, peak_speed};
        let kind = scaled_column(scale);
        let (mut app, root) = composed_kind(false, &kind);
        for _ in 0..3 {
            app.update();
        }
        let structure = &kind.structure;
        // DERIVED: sliding the upper two chunks costs at most the bottom seam's cohesion times
        // area plus friction under the entire column's weight (an upper bound on its preload).
        let seam = &structure.joints[0];
        let weight = structure
            .chunks
            .iter()
            .map(|c| f64::from(c.mass))
            .sum::<f64>()
            * f64::from(crate::physical_laws::gravity_mps2());
        let capacity = f64::from(seam.area * seam.seam().unwrap().shear.cohesion)
            + fracture::bondsolve::pair_friction(structure, seam) * weight;
        // Fixture: a 1e6 kg, 0.4 m cube with principal inertia equal to its mass in kg m².
        // DERIVED: any face witness has r_y² + r_z² <= 0.08 m², so m/2 bounds effective mass below.
        let mass = 1e6;
        let effective = f64::from(mass) / 2.0;
        let patch = Contactor::chunk(structure, 1);
        // Fixture: twice the cut bound; travel at least one chunk side per tick.
        let speed = (peak_speed(effective, 2.0 * capacity, patch, Contactor::RIGID) as f32)
            .max(scale / STEP);
        assert!(peak_force(effective, f64::from(speed), patch, Contactor::RIGID) > capacity);
        let middle = structure.chunks[1].centroid;
        // DERIVED: a half-side face gap, crossed within this tick at the chosen speed.
        let start = middle - Vec3::X * (scale + CUBE_SIDE_M / 2.0);
        let source = app.world_mut().spawn(cube(start, mass)).id();
        app.update(); // Prepare the collider hierarchy before the driving stimulus.
        assert!(
            app.world()
                .get::<Breakage>(root)
                .unwrap()
                .live
                .state
                .standing(1)
        );
        app.world_mut().get_mut::<LinearVelocity>(source).unwrap().0 = Vec3::X * speed;
        let ordinal = app.world().get::<Breakage>(root).unwrap().construction;
        let (mut contact, mut opening) = (None, None);
        for tick in 0..=2 {
            app.update();
            if app
                .world()
                .resource::<SweepAccount>()
                .contacts
                .contains_key(&(source, ordinal))
            {
                contact.get_or_insert(tick);
            }
            if !app
                .world()
                .get::<Breakage>(root)
                .unwrap()
                .live
                .state
                .standing(1)
            {
                opening.get_or_insert(tick);
            }
        }
        let first = contact.expect("the fixture reaches occupied matter within one tick");
        assert!(
            opening.is_some_and(|tick| tick <= first + 2),
            "an independently over-capacity fixture opens within two contact ticks"
        );
        println!("MEASURED fixture side={scale} m: contact={first}, opening={opening:?}");
    }
}

/// Fixture: keep unit-column proportions while each chunk exceeds the authored rubble floor.
fn resolved_column() -> Arc<ConstructionKind> {
    let scale = crate::physical_laws::PhysicalLaws::shipped()
        .server_rubble_min_size_m
        .max(1.0)
        * 2.0;
    scaled_column(scale)
}

/// Fixture: a column scaled uniformly, with geometry, mass and joint areas kept consistent.
fn scaled_column(scale: f32) -> Arc<ConstructionKind> {
    let mut asset = column_construction().asset.clone();
    let point = |p: &mut [f32; 3]| {
        for x in p {
            *x *= scale;
        }
    };
    asset.foundation_y_m *= scale;
    for element in &mut asset.elements {
        for p in &mut element.hull {
            point(p);
        }
    }
    for chunk in &mut asset.chunks {
        for p in &mut chunk.hull {
            point(p);
        }
        point(&mut chunk.centroid);
        chunk.volume_m3 *= scale.powi(3);
        chunk.mass_kg *= scale.powi(3);
    }
    for joint in &mut asset.joints {
        point(&mut joint.centroid);
        joint.area_m2 *= scale.powi(2);
    }
    for anchor in &mut asset.anchors {
        point(&mut anchor.centroid);
        anchor.area_m2 *= scale.powi(2);
    }
    Arc::new(
        ConstructionKind::resolve(
            &asset.id,
            &ron::ser::to_string(&asset).unwrap(),
            crate::substances::SubstanceRegistry::shipped_static(),
            Attachments::shipped_static(),
        )
        .unwrap(),
    )
}

/// The game's environment destruction keeps a resolved hull plug's matter and rigid motion.
/// The server body must retain occupied mass; internal seams and contact hooks stay untouched.
#[test]
fn a_swept_patch_keeps_its_server_body_mass_and_rigid_motion() {
    let kind = resolved_column();
    let (mut app, root) = composed_kind(false, &kind);
    crate::headless_test::start_fixed_clock(&mut app);
    app.update(); // Bevy's first real-time update initializes its clock with zero delta.
    app.update();
    let mut owner = app.world_mut().get_mut::<Breakage>(root).unwrap();
    let mut record = Record::default();
    // Fixture: the upper pair leaves with translation and spin about the construction origin.
    let group = vec![1, 2];
    assert!(crate::debris::crumble::server_body(
        &owner.live.structure,
        &group
    ));
    let expected_mass: f32 = group
        .iter()
        .map(|&c| kind.sweep.part(c, &Transform::IDENTITY).mass as f32)
        .sum();
    let cut = sweep_intake::cut_fragments(&mut owner, group.clone(), 7);
    let (departures, sheds) = sweep_intake::sweep_departure(
        &mut owner,
        Transform::IDENTITY,
        &cut,
        sweep_intake::ReleasedMotion {
            velocity: Vec3::X * 2.0,
            spin: Vec3::Z,
            centre: Vec3::ZERO,
        },
        7,
        &mut record,
        &ReusableColliders::Standing(BTreeMap::new()),
    );
    assert!(sheds.is_empty());
    assert_eq!(departures.len(), 1);
    let launched = &departures[0].departure;
    assert_eq!(launched.group, group);
    assert_eq!(
        launched.launch.velocity,
        Vec3::X * 2.0 + Vec3::Z.cross(launched.launch.centre)
    );
    assert_eq!(launched.launch.spin, Vec3::Z);
    let internal = kind
        .layout
        .joints
        .iter()
        .position(|j| group.contains(&j.a) && matches!(j.b, Side::Chunk(b) if group.contains(&b)))
        .unwrap();
    assert!(!owner.live.state.separated[internal]);
    for &c in &group {
        assert_eq!(owner.live.state.fate[c], Fate::Fallen);
    }
    assert!(record.entries.iter().all(|entry| entry.tick == 7));
    assert!(
        record
            .entries
            .iter()
            .all(|entry| !matches!(entry.event, Event::Dust(_)))
    );
    spawn_departures(&mut app, departures);
    app.update();
    let (piece, mass) = app
        .world_mut()
        .query::<(&Piece, &ComputedMass)>()
        .single(app.world())
        .unwrap();
    assert_eq!(piece.chunks, [1, 2]);
    assert!(
        (mass.value() / expected_mass - 1.0).abs() < 1e-5,
        "occupied mass: {} kg on the server, {expected_mass} kg in the freed source geometry",
        mass.value()
    );
}

#[derive(Debug, PartialEq)]
struct ChunkFact {
    owner: Vec<u16>,
    chunk: u16,
    sensor: bool,
    points: Vec<Vec3>,
    layers: CollisionLayers,
    friction: Option<Friction>,
    volume: Option<(String, crate::ballistics::VolumeMaterial)>,
    /// A contact collider's insertion-hook mass; a sensor's adds nothing to its body.
    cached_mass: Option<ColliderMassProperties>,
}
fn chunk_facts(world: &mut World) -> Vec<ChunkFact> {
    let mut facts: Vec<_> = world
        .query::<(
            Entity,
            &ChildOf,
            &ChunkCollider,
            &Collider,
            &CollisionLayers,
            Has<Sensor>,
            Option<&Friction>,
            Option<&BallisticVolume>,
            &ColliderOf,
            &ColliderMassProperties,
        )>()
        .iter(world)
        .filter_map(
            |(
                entity,
                parent,
                chunk,
                collider,
                layers,
                sensor,
                friction,
                volume,
                of,
                cached_mass,
            )| {
                let piece = world.get::<Piece>(parent.parent())?;
                assert_eq!(
                    of.body,
                    parent.parent(),
                    "physics and hierarchy name the same owner"
                );
                assert_eq!(
                    sensor,
                    volume.is_some(),
                    "a freed chunk's occupied collider meets rounds and no contact"
                );
                let breakage = world.get::<Breakage>(parent.parent()).unwrap();
                let c = usize::from(chunk.0);
                let expected = if sensor {
                    assert_eq!(
                        *layers,
                        CollisionLayers::new([Layer::Armor], LayerMask::NONE)
                    );
                    &breakage.kind.sweep.occupied(c).0
                } else {
                    assert_eq!(*layers, piece_layers());
                    breakage.kind.sweep.contact_hull(c)
                };
                assert_eq!(
                    collider.shape().as_convex_polyhedron().unwrap().points(),
                    expected.shape().as_convex_polyhedron().unwrap().points(),
                    "batching retains the occupied and inset contact shapes from construction data"
                );
                assert_eq!(friction, Some(&breakage.kind.chunk_friction(c)));
                if !sensor {
                    let source = world
                        .get::<super::super::sweep::SourceShape>(parent.parent())
                        .expect("a free group's query geometry exists at spawn and split");
                    let leaf =
                        &source.0.hulls()[piece.chunks.iter().position(|&c| c == chunk.0).unwrap()];
                    assert_eq!(
                        leaf.position,
                        world.get::<Transform>(entity).unwrap().translation
                    );
                    assert_eq!(leaf.rotation, Quat::IDENTITY);
                    assert_eq!(
                        leaf.hull
                            .shape_scaled()
                            .as_convex_polyhedron()
                            .unwrap()
                            .points(),
                        collider
                            .shape_scaled()
                            .as_convex_polyhedron()
                            .unwrap()
                            .points(),
                        "the sweep's source is the contact collider's shape"
                    );
                }

                assert_eq!(
                    world.get::<Transform>(entity).unwrap().translation,
                    breakage.kind.structure.chunks[usize::from(chunk.0)].centroid - breakage.origin
                );
                Some(ChunkFact {
                    owner: piece.chunks.clone(),
                    chunk: chunk.0,
                    sensor,
                    points: collider
                        .shape_scaled()
                        .as_convex_polyhedron()
                        .unwrap()
                        .points()
                        .to_vec(),
                    layers: *layers,
                    friction: friction.copied(),
                    volume: volume.map(|v| (v.substance.clone(), v.material)),
                    cached_mass: (!sensor).then_some(*cached_mass),
                })
            },
        )
        .collect();
    facts.sort_by_key(|f| (f.chunk, f.sensor));
    facts
}

/// Driving and sweeps read these before Avian prepares physics. Every fresh or reused
/// departure must already hold its complete occupied properties, including split children.
fn assert_spawned_group_properties(world: &mut World) {
    let mut chunks = world.query::<(Entity, &ChildOf, &ChunkCollider, Has<Sensor>)>();
    for (entity, piece, owner, mass, inertia, com, colliders) in world
        .query::<(
            Entity,
            &Piece,
            &Breakage,
            &ComputedMass,
            &avian3d::prelude::ComputedAngularInertia,
            &avian3d::prelude::ComputedCenterOfMass,
            &RigidBodyColliders,
        )>()
        .iter(world)
    {
        let unique: BTreeSet<_> = colliders.iter().collect();
        assert_eq!(
            colliders.len(),
            unique.len(),
            "the body's attachment list has no duplicate collider"
        );
        assert_eq!(
            unique.len(),
            2 * piece.chunks.len(),
            "every occupied and contact collider is attached before physics or retirement"
        );
        let members: Vec<_> = chunks
            .iter(world)
            .filter(|(_, parent, ..)| parent.parent() == entity)
            .collect();
        assert_eq!(
            unique,
            members.iter().map(|(entity, ..)| *entity).collect(),
            "the physics list names exactly this group's live chunk colliders"
        );
        assert_eq!(
            members
                .iter()
                .map(|(_, _, chunk, sensor)| (chunk.0, *sensor))
                .collect::<BTreeSet<_>>(),
            piece
                .chunks
                .iter()
                .flat_map(|&c| [(c, false), (c, true)])
                .collect(),
            "each chunk contributes its own occupied and contact collider"
        );
        let group: Vec<_> = piece.chunks.iter().map(|&c| usize::from(c)).collect();
        let expected = launch(&owner.kind, &owner.live.structure, &group, &[]).mass_properties;
        // Resolution: convex integration, diagonalization and f32 quaternion rounding at fixture scale.
        let tolerance = 1e-5;
        assert!(
            (mass.value() - expected.mass).abs() <= expected.mass * tolerance,
            "complete occupied mass is ready at spawn: {} vs {}",
            mass.value(),
            expected.mass
        );
        let expected_inertia = expected.angular_inertia_tensor().to_mat3();
        let scale = expected_inertia
            .to_cols_array()
            .into_iter()
            .map(f32::abs)
            .fold(0.0, f32::max);
        assert!(
            (inertia.tensor().to_mat3() - expected_inertia)
                .to_cols_array()
                .into_iter()
                .all(|v| v.abs() <= scale * tolerance),
            "complete occupied inertia is ready at spawn"
        );
        assert!(
            (com.0 - expected.center_of_mass).length() <= tolerance,
            "occupied COM is ready in the new body's frame"
        );
    }
}

/// Settle `root`, then mark every element replaced so `reconcile` gives each chunk its standing
/// collider.
fn install_standing_colliders(app: &mut App, root: Entity) {
    use bevy::ecs::system::RunSystemOnce;
    app.update();
    app.update();
    app.world_mut()
        .get_mut::<Breakage>(root)
        .unwrap()
        .live
        .swapped
        .fill(true);
    app.world_mut().run_system_once(reconcile).unwrap();
}

/// The game's environment destruction model: bookkeeping may reuse entity IDs, but every
/// physics tick retains the same chunk shapes, ownership, material and solver properties.
/// Fresh departure is also exercised: an intact element has no per-chunk entities to reuse.
#[test]
fn departure_and_split_preserve_each_chunks_collider_contract() {
    use bevy::ecs::system::{RunSystemOnce, SystemState};
    #[derive(Resource, Default)]
    struct ShapeInsertions(usize);
    let run = |reuse: bool| {
        let kind = column_construction();
        let (mut app, root) = composed_kind(false, &kind);
        // Fixture: a nontrivial parent pose challenges world-to-local initialization.
        let frame =
            Transform::from_xyz(20.0, 7.0, -13.0).with_rotation(Quat::from_rotation_y(0.37));
        app.world_mut().entity_mut(root).insert((
            frame,
            Position(frame.translation),
            Rotation(frame.rotation),
        ));
        install_standing_colliders(&mut app, root);
        app.init_resource::<ShapeInsertions>().add_observer(
            |_: On<Insert, Collider>, mut insertions: ResMut<ShapeInsertions>| insertions.0 += 1,
        );
        // Fixture: the upper two members leave together, then split into individual bodies.
        let group = vec![1, 2];
        let mut query = SystemState::<ChunkChildren>::new(app.world_mut());
        let ReusableColliders::Standing(standing) = reusable_colliders(
            app.world().get::<Children>(root),
            &query.get(app.world()).unwrap(),
        ) else {
            panic!("a placed construction's colliders are a standing owner's");
        };
        let old: Vec<_> = standing
            .iter()
            .filter(|(chunk, _)| group.contains(&usize::from(**chunk)))
            .map(|(_, &entity)| entity)
            .collect();
        let departure = {
            let mut body = app.world_mut().get_mut::<Breakage>(root).unwrap();
            for &c in &group {
                body.live.state.fate[c] = Fate::Fallen;
            }
            Departure {
                kind: kind.clone(),
                construction: body.construction,
                root: frame,
                live: body.live.group(&group),
                launch: launch(&kind, &body.live.structure, &group, &[]),
                group,
                tick: 0,
            }
        };
        let reusable = ReusableColliders::Standing(if reuse { standing } else { BTreeMap::new() });
        spawn_departures(&mut app, vec![prepare_group(departure, &reusable)]);
        assert_spawned_group_properties(app.world_mut());
        app.world_mut().run_system_once(reconcile).unwrap();
        if reuse {
            assert!(
                old.iter()
                    .all(|&e| app.world().get::<Collider>(e).is_some()),
                "departing chunk entities survive the handoff"
            );
        }
        assert_eq!(
            app.world().resource::<ShapeInsertions>().0,
            if reuse { 2 } else { 4 },
            "a departure rehomes each occupied collider without replacing its shape and adds \
             one contact collider"
        );
        app.world_mut().resource_mut::<ShapeInsertions>().0 = 0;
        let mut snapshots = vec![chunk_facts(app.world_mut())];
        assert_eq!(
            snapshots[0].len(),
            4,
            "each freed chunk has one occupied and one contact collider"
        );
        for _ in 0..2 {
            app.update();
            snapshots.push(chunk_facts(app.world_mut()));
        }
        let parent = app
            .world_mut()
            .query_filtered::<Entity, With<Piece>>()
            .single(app.world())
            .unwrap();
        let old: Vec<_> = app
            .world()
            .get::<Children>(parent)
            .unwrap()
            .iter()
            .collect();
        let children = {
            let body = app.world().get::<Breakage>(parent).unwrap();
            [vec![1], vec![2]]
                .into_iter()
                .map(|g| {
                    let live = body.live.group(&g);
                    (g, live)
                })
                .collect()
        };
        app.world_mut()
            .entity_mut(parent)
            .insert(PendingSplit { children, tick: 1 });
        app.world_mut().run_system_once(split_groups).unwrap();
        assert_spawned_group_properties(app.world_mut());
        assert!(
            old.iter()
                .all(|&e| app.world().get::<Collider>(e).is_some()),
            "split children survive their previous body's retirement"
        );
        assert_eq!(
            app.world().resource::<ShapeInsertions>().0,
            0,
            "splitting keeps every chunk's permanent shape"
        );
        snapshots.push(chunk_facts(app.world_mut()));
        for _ in 0..2 {
            app.update();
            snapshots.push(chunk_facts(app.world_mut()));
        }
        assert!(snapshots.iter().all(|snapshot| snapshot.len() == 4));
        snapshots
    };
    assert_eq!(run(false), run(true));
}

/// The game's environment destruction model: the shipped law/refit chain must transfer a
/// standing collider before reconciliation can delete it; direct helper coverage is insufficient.
#[test]
fn the_law_installer_reuses_departing_standing_colliders() {
    let kind = resolved_column();
    let (mut app, root) = composed_kind(false, &kind);
    install_standing_colliders(&mut app, root);
    // Fixture: the upper member is free when its sole support seam separates.
    let chunk = 2;
    let old = app
        .world()
        .get::<Children>(root)
        .unwrap()
        .iter()
        .find(|&e| {
            app.world()
                .get::<ChunkCollider>(e)
                .is_some_and(|c| c.0 == chunk)
        })
        .unwrap();
    isolate(&mut app, root, &[usize::from(chunk)]);
    app.update();
    let parent = app
        .world()
        .get::<ChildOf>(old)
        .expect("a departing collider survives reconciliation")
        .parent();
    assert!(app.world().get::<Piece>(parent).is_some());
    assert_eq!(app.world().get::<ColliderOf>(old).unwrap().body, parent);
}

/// The game's environment destruction: removing a middle support frees the crown immediately,
/// before reconciliation can construct a standing collider for it. Only the swept chunk inherits
/// the hull's exchange; an unsupported neighbour starts at rest and meets the hull through Avian.
#[test]
fn a_cut_frees_unsupported_matter_without_building_standing_colliders() {
    use bevy::ecs::system::RunSystemOnce;
    let kind = column_construction();
    let (mut app, root) = composed_kind(false, &kind);
    app.update();
    app.update();
    let mut record = Record::default();
    let (departures, sheds) = {
        let mut owner = app.world_mut().get_mut::<Breakage>(root).unwrap();
        let cut = sweep_intake::cut_fragments(&mut owner, vec![1], 7);
        sweep_intake::sweep_departure(
            &mut owner,
            Transform::IDENTITY,
            &cut,
            sweep_intake::ReleasedMotion {
                velocity: Vec3::X * 2.0,
                spin: Vec3::ZERO,
                centre: Vec3::ZERO,
            },
            7,
            &mut record,
            &ReusableColliders::Standing(BTreeMap::new()),
        )
    };
    let freed: Vec<_> = departures
        .iter()
        .flat_map(|p| p.departure.group.iter().copied())
        .chain(
            sheds
                .iter()
                .flat_map(|shed| shed.chunks.iter().map(|&c| usize::from(c))),
        )
        .collect();
    assert_eq!(
        freed.len(),
        2,
        "every freed chunk leaves exactly once through the size route"
    );
    assert_eq!(
        freed.into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([1, 2]),
        "the cut and the matter it disconnected both leave before reconciliation"
    );
    let crown = departures
        .iter()
        .find(|p| p.departure.group.contains(&2))
        .expect("the unsupported crown leaves as server rubble");
    assert_eq!(crown.departure.launch.velocity, Vec3::ZERO);
    assert_eq!(crown.departure.launch.spin, Vec3::ZERO);
    spawn_departures(&mut app, departures);
    app.world_mut().run_system_once(reconcile).unwrap();
    for (chunk, parent) in app
        .world_mut()
        .query::<(&ChunkCollider, &ChildOf)>()
        .iter(app.world())
    {
        assert_eq!(
            app.world().get::<Piece>(parent.parent()).is_some(),
            chunk.0 != 0,
            "only grounded matter needs a standing collider"
        );
    }
    let mut shape = Shape::intact(&kind.layout);
    for entry in record.entries {
        shape.apply(entry.event, &kind.layout);
    }
    shape.derive_departures(&kind.layout, &Live::new(&kind));
    assert_eq!(
        shape.fate,
        app.world().get::<Breakage>(root).unwrap().live.state.fate,
        "the shared ledger derives every immediate departure"
    );
}

/// The game's environment destruction: below-floor merging of a moving cut and stationary unsupported
/// matter preserves their combined momentum without adding kinetic energy. This fixture drives
/// one of two equal unit cubes; mass and inertia come from the admitted occupied geometry.
#[test]
fn absorbed_cut_and_unsupported_matter_keep_their_combined_momentum() {
    let kind = column_construction();
    // Fixture translation, rotation and rigid motion exercise both COM and spin terms.
    let pose = Transform::from_xyz(20.0, 7.0, -13.0).with_rotation(Quat::from_rotation_y(0.37));
    let group = [1, 2];
    let driven = BTreeSet::from([1]);
    let motion = sweep_intake::ReleasedMotion {
        velocity: Vec3::X * 2.0,
        spin: Vec3::Z * 0.3,
        centre: pose.translation,
    };
    let merged = sweep_intake::fragment_motion(&kind, &pose, &group, &driven, motion);
    let centre = merged.centre.as_dvec3();
    let totals = |after: bool| {
        let mut p = DVec3::ZERO;
        let mut l = DVec3::ZERO;
        let mut energy = 0.0;
        for c in group {
            let part = kind.sweep.part(c, &pose);
            let (v, w) = if after {
                (
                    merged.velocity.as_dvec3() + merged.spin.as_dvec3().cross(part.centre - centre),
                    merged.spin.as_dvec3(),
                )
            } else if driven.contains(&c) {
                (
                    motion.velocity.as_dvec3()
                        + motion
                            .spin
                            .as_dvec3()
                            .cross(part.centre - motion.centre.as_dvec3()),
                    motion.spin.as_dvec3(),
                )
            } else {
                (DVec3::ZERO, DVec3::ZERO)
            };
            p += part.mass * v;
            l += part.inertia * w + (part.centre - centre).cross(part.mass * v);
            energy += 0.5 * (part.mass * v.length_squared() + w.dot(part.inertia * w));
        }
        (p, l, energy)
    };
    let (before_p, before_l, before_energy) = totals(false);
    let (after_p, after_l, after_energy) = totals(true);
    // Resolution: f32 world-pose and twist rounding; the independent accounting is f64.
    let tolerance = 1e-5;
    assert!((after_p - before_p).length() < tolerance * before_p.length());
    assert!((after_l - before_l).length() < tolerance * before_l.length());
    assert!(after_energy <= before_energy * (1.0 + tolerance));
}

/// The game's environment physics: prepared contact mass survives insertion and follows shape,
/// scale and density changes. Tripwire for the vendored `with_mass_properties_cache` patch
/// (`vendor/avian3d-0.7.0-child-pose/OVERMATCH_PATCH.md`), which every chunk collider that is no
/// sensor prepares on workers.
#[test]
fn prepared_contact_mass_is_valid_at_insertion_and_invalidates_with_its_inputs() {
    use avian3d::dynamics::rigid_body::mass_properties::bevy_heavy::ComputeMassProperties3d;
    // Fixture: an asymmetric convex box exposes centre and inertia as well as mass.
    let points = [0.0, 1.0]
        .into_iter()
        .flat_map(|x| {
            [0.0, 2.0]
                .into_iter()
                .flat_map(move |y| [0.0, 3.0].map(move |z| Vec3::new(x, y, z)))
        })
        .collect();
    let hull = Collider::convex_hull(points).unwrap();
    let density = ColliderDensity::default().0;
    let expected = hull.mass_properties(density);
    let cached = hull.clone().with_mass_properties_cache(density);
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, crate::offline_physics()));
    app.finish();
    app.cleanup();
    let entity = app.world_mut().spawn((Transform::default(), cached)).id();
    assert_eq!(
        *app.world().get::<ColliderMassProperties>(entity).unwrap(),
        ColliderMassProperties::from(expected)
    );
    let mut cached = app.world().get::<Collider>(entity).unwrap().clone();
    assert_eq!(
        cached.mass_properties(density * 2.0),
        hull.mass_properties(density * 2.0)
    );
    cached.set_scale(Vec3::splat(2.0), 10);
    let mut scaled = hull;
    scaled.set_scale(Vec3::splat(2.0), 10);
    assert_eq!(
        cached.mass_properties(density),
        scaled.mass_properties(density)
    );
    cached.set_shape(Collider::sphere(1.0).shape().clone());
    let mut replaced = Collider::sphere(1.0);
    replaced.set_scale(Vec3::splat(2.0), 10);
    assert_eq!(
        cached.mass_properties(density),
        replaced.mass_properties(density)
    );
}

/// The game's scenery recorder distinguishes retained collider reassignment from fresh shapes;
/// the server's split-work census must count each reused entity exactly once at application.
#[test]
fn collider_batch_counts_rehomed_entities_and_excludes_fresh_shapes() {
    let mut world = World::new();
    world.init_resource::<Stages>();
    let former = world.spawn_empty().id();
    let next = world.spawn_empty().id();
    let retained = world
        .spawn((Collider::cuboid(1.0, 1.0, 1.0), ChildOf(former)))
        .id();
    let pose = || {
        (
            Position(Vec3::ZERO),
            Rotation(Quat::IDENTITY),
            GlobalTransform::IDENTITY,
        )
    };
    let mut batch = ColliderBatch::default();
    batch.push(
        ColliderSource::Fresh(Collider::cuboid(1.0, 1.0, 1.0)),
        ChildOf(next),
        pose(),
    );
    batch.push(ColliderSource::Rehome(retained), ChildOf(next), pose());
    batch.apply(&mut world);
    let counts = serde_json::to_value(world.resource::<Stages>().take_counts()).unwrap();
    assert_eq!(counts["rehomed_colliders"], 1);
    assert_eq!(world.get::<ChildOf>(retained).unwrap().parent(), next);
    assert_eq!(
        world.resource::<Stages>().take_counts().values[Work::RehomedColliders as usize],
        0
    );
}
