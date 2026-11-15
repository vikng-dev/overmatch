use std::time::Duration;

use avian3d::collision::collider::contact_query::intersection_test;
use avian3d::prelude::*;
use bevy::time::TimeUpdateStrategy;

use super::*;
use crate::contact_transfer::{ContactTransfers, StructureBody};

/// FIXTURE: the game's fixed clock and its physics, shoe-face hook included, with no rendering or
/// window plugins.
fn physics(gravity: Vec3) -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::transform::TransformPlugin,
        AssetPlugin::default(),
        crate::offline_physics(),
        crate::contact_transfer::sim_plugin,
        super::super::soft_run::plugin,
    ))
    .init_asset::<Mesh>()
    .insert_resource(Gravity(gravity))
    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
        15_625,
    )));
    while app.plugins_state() == bevy::app::PluginsState::Adding {
        std::thread::sleep(Duration::from_millis(1));
    }
    app.finish();
    app.cleanup();
    for _ in 0..3 {
        app.update();
    }
    app
}

fn points(collider: &Collider) -> &[Vec3] {
    collider
        .shape()
        .as_convex_polyhedron()
        .expect("track shapes are convex prisms")
        .points()
}

/// The same constructor game tanks use, with prescribed pivot motion or a free dynamic body.
/// Mass and inertia dimensions are fixture data read from the tank's declaration; the belts stand
/// still, the state the shoe-face hook reads.
fn body(app: &mut App, geom: &RigGeom, shapes: &TrackColliders, role: RigidBody) -> Entity {
    let spec = super::super::rig_geom::tiger_spec();
    let (x, y, z) = spec.inertia_extents;
    let at = Vec3::Y * geom.hull_rest_y;
    let body = app
        .world_mut()
        .spawn((
            role,
            Transform::from_translation(at),
            Position(at),
            Rotation::default(),
            Mass(spec.mass),
            AngularInertia::from_shape(&Cuboid::new(x, y, z), spec.mass),
            NoAutoMass,
            NoAutoAngularInertia,
            NoAutoCenterOfMass,
            CenterOfMass(Vec3::ZERO),
            super::super::sim::TrackDrive::default(),
        ))
        .id();
    spawn_track_colliders(&mut app.world_mut().commands(), body, shapes);
    app.world_mut().flush();
    body
}

/// Every tank's side contact must start at its measured shoe faces and end at the belt's bottom,
/// rather than a symmetric pin-plane approximation. Driving relies on both edges meeting a
/// structure or another hull at the shoe's true faces; changing the fixture's shoe bias must move
/// both faces. Geometry comparisons allow 1e-5 m (Resolution: f32 round-off).
#[test]
fn side_plates_follow_the_true_shoe_faces_and_belt_bottom() {
    let mut geom = super::super::rig_geom::tiger_rig();
    let params = super::super::rig_geom::tiger_spec()
        .track
        .suspension
        .params();
    // FIXTURE variants: lateral bias and width change independently of the pin plane.
    for (bias, width_scale) in [(0.0, 1.0), (0.13, 1.4)] {
        geom.model.lateral_min += bias;
        geom.model.lateral_max = geom.model.lateral_min + geom.model.width * width_scale;
        geom.model.width = geom.model.lateral_max - geom.model.lateral_min;
        let shapes = TrackColliders::new(
            &geom,
            &params,
            super::super::rig_geom::tiger_spec().track.friction(),
        );
        for side in Side::ALL {
            for (edge_index, plate) in shapes.side_plates.get(side).iter().enumerate() {
                let edge = geom.grip_columns(side)[edge_index * 2].0;
                let inward = if edge_index == 0 {
                    side.sign()
                } else {
                    -side.sign()
                };
                let depth = geom.thickness.min(geom.model.width / 2.0);
                let xs: Vec<_> = points(plate)
                    .iter()
                    .map(|p| (p.x - edge) * inward)
                    .collect();
                assert!(xs.iter().all(|&x| x >= -1e-5 && x <= depth + 1e-5));
                assert!(
                    xs.iter().any(|x| x.abs() < 1e-5),
                    "the exposed face is the true edge"
                );
                let low = points(plate)
                    .iter()
                    .map(|p| p.y)
                    .fold(f32::INFINITY, f32::min);
                assert!(
                    (low + geom.hull_rest_y).abs() < 1e-5,
                    "the plate reaches the outer belt face"
                );
            }
        }
    }
    let layers = side_plate_layers();
    assert!(!layers.interacts_with(CollisionLayers::new([Layer::Terrain], LayerMask::ALL)));
    assert!(layers.interacts_with(crate::construction::element_layers()));
    assert!(
        !layers.interacts_with(crate::construction::breakage::piece_layers()),
        "loose pieces meet the soft run and the stop, never a side plate"
    );
    assert!(layers.interacts_with(CollisionLayers::new([Layer::Vehicle], LayerMask::ALL)));
    assert!(layers.interacts_with(layers));
    assert!(!layers.interacts_with(CollisionLayers::new([Layer::Armor], LayerMask::NONE)));
}

/// The running gear's calibrated support law, as the shipped tank's construction publishes it.
fn shipped_law() -> super::super::forces::SupportLaw {
    let mut app = App::new();
    app.add_plugins((crate::bake::plugin, super::super::sim::sim_plugin));
    app.world_mut().run_schedule(Startup);
    app.world_mut().run_schedule(PreUpdate);
    *app.world().resource::<super::super::forces::SupportLaw>()
}

/// A piece beside any shoe face starts OUTSIDE every collider it meets and entirely below the
/// stop. A pivot must shove it sideways through the run's soft contact, carrying the shipped law,
/// within the run's maximum surface speed plus 0.1 m/s (Resolution: the discrete contact's
/// allowance, well below the solver's 4 m/s default overlap separation speed), by more than its
/// width. Driving relies on this to push pieces aside rather than throw them at first contact.
/// Terrain deliberately intersects the run geometrically, but must never form a contact pair with
/// it.
#[test]
fn a_pivoting_track_shoves_a_piece_without_throwing_it_or_touching_terrain() {
    let law = shipped_law();
    let geom = super::super::rig_geom::tiger_rig();
    let params = super::super::rig_geom::tiger_spec()
        .track
        .suspension
        .params();
    let shapes = TrackColliders::new(
        &geom,
        &params,
        super::super::rig_geom::tiger_spec().track.friction(),
    );
    let clearance = points(shapes.colliders.get(Side::Right))
        .iter()
        .map(|p| p.y + geom.hull_rest_y)
        .fold(f32::INFINITY, f32::min);
    // DERIVED fixture dimensions: the piece fits wholly in the band below the rigid stop.
    let size = clearance * 0.5;
    let top = clearance * 0.1;
    let z = (geom.rest.get(Side::Right)[0].0.x - geom.rest.get(Side::Right).last().unwrap().0.x)
        .abs()
        * 0.25;
    let terrain_shape = Collider::cuboid(20.0, 1.0, 20.0); // FIXTURE ground extent.
    let terrain_at = Vec3::Y * (top - 0.5);
    for side in Side::ALL {
        let run = shapes.runs.get(side).collider();
        for edge_index in 0..2 {
            let mut app = physics(Vec3::NEG_Y * super::super::derive::G);
            app.insert_resource(law);
            let tank = body(&mut app, &geom, &shapes, RigidBody::Kinematic);
            let out = if edge_index == 0 {
                -side.sign()
            } else {
                side.sign()
            };
            let edge = geom.grip_columns(side)[edge_index * 2].0;
            let initial = Vec3::new(edge + out * size * 0.75, top + size * 0.5, z);
            let piece_shape = Collider::cuboid(size, size, size);
            for collider in shapes.colliders.values().chain(
                shapes
                    .runs
                    .values()
                    .map(super::super::soft_run::RunShape::collider),
            ) {
                assert!(
                    !intersection_test(
                        collider,
                        Vec3::Y * geom.hull_rest_y,
                        Quat::IDENTITY,
                        &piece_shape,
                        initial,
                        Quat::IDENTITY
                    )
                    .unwrap(),
                    "the loose piece must start outside the track, never in a deep overlap"
                );
            }
            assert!(
                intersection_test(
                    run,
                    Vec3::Y * geom.hull_rest_y,
                    Quat::IDENTITY,
                    &terrain_shape,
                    terrain_at,
                    Quat::IDENTITY,
                )
                .unwrap(),
                "the terrain filter must exclude an actual geometric overlap"
            );
            let terrain = app
                .world_mut()
                .spawn((
                    RigidBody::Static,
                    Transform::from_translation(terrain_at),
                    terrain_shape.clone(),
                    CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
                ))
                .id();
            let piece = app
                .world_mut()
                .spawn((
                    crate::debris::DebrisPiece,
                    RigidBody::Dynamic,
                    Transform::from_translation(initial),
                    piece_shape,
                    ColliderDensity(1800.0), // FIXTURE material density, kg/m^3.
                    crate::construction::breakage::piece_layers(),
                    Friction::new(0.6), // FIXTURE piece friction.
                    Restitution::ZERO,
                ))
                .id();
            // FIXTURE imposed pivot, rad/s; a motor's surface motion is the only sideways input.
            let omega = Vec3::Y * out * 0.2;
            app.world_mut()
                .entity_mut(tank)
                .insert(AngularVelocity(omega));
            let surface_speed = points(run)
                .iter()
                .map(|p| omega.cross(*p).length())
                .fold(0.0f32, f32::max);
            let mut fastest = 0.0f32;
            let mut touched = false;
            for _ in 0..128 {
                // DERIVED: two seconds at the fixture clock.
                app.update();
                let world = app.world();
                let speed = world.get::<LinearVelocity>(piece).unwrap().length();
                fastest = fastest.max(speed);
                assert!(
                    speed <= surface_speed + 0.1,
                    "{side:?} edge {edge_index}: piece {speed} m/s, run {surface_speed} m/s"
                );
                for pair in world.resource::<ContactGraph>().iter_active_touching() {
                    let is_run = |e| world.get::<super::super::soft_run::SoftRun>(e).is_some();
                    assert!(
                        !((is_run(pair.collider1) && pair.collider2 == terrain)
                            || (is_run(pair.collider2) && pair.collider1 == terrain)),
                        "a run never carries terrain support"
                    );
                    touched |= (is_run(pair.collider1) && pair.collider2 == piece)
                        || (is_run(pair.collider2) && pair.collider1 == piece);
                }
            }
            let displacement = (app.world().get::<Position>(piece).unwrap().x - initial.x) * out;
            println!(
                "MEASURED {side:?} edge {edge_index}: shove {displacement:.4} m, \
                fastest {fastest:.4} m/s; DERIVED run surface bound {surface_speed:.4} m/s"
            );
            assert!(touched, "the run must be the collider that meets the piece");
            assert!(
                displacement > size,
                "the piece must move sideways by more than its width"
            );
        }
    }
}

/// The game's environment destruction model reads a plate's structure contacts exactly like a
/// hull proxy's. A low standing element that only the plate reaches must take momentum through
/// ContactTransfers away from the shoe's exposed face, the one face a plate bears through; a new
/// layer that misses standing structures, a reader bypassed, or a plate bearing through its inner
/// face fails.
#[test]
fn side_plate_contacts_reach_the_structure_contact_reader() {
    let geom = super::super::rig_geom::tiger_rig();
    let params = super::super::rig_geom::tiger_spec()
        .track
        .suspension
        .params();
    let shapes = TrackColliders::new(
        &geom,
        &params,
        super::super::rig_geom::tiger_spec().track.friction(),
    );
    let mut app = physics(Vec3::ZERO);
    let tank = body(&mut app, &geom, &shapes, RigidBody::Dynamic);
    let clearance = points(shapes.colliders.get(Side::Right))
        .iter()
        .map(|p| p.y + geom.hull_rest_y)
        .fold(f32::INFINITY, f32::min);
    let size = clearance * 0.5;
    let edge = geom.grip_columns(Side::Right)[2].0;
    // FIXTURE contact on the front half, wholly below the bump stop, initially separated.
    let at = Vec3::new(edge + size, size * 0.5, 1.0);
    let wall = app
        .world_mut()
        .spawn((
            RigidBody::Static,
            Transform::from_translation(at),
            StructureBody,
        ))
        .id();
    app.world_mut().spawn((
        ChildOf(wall),
        Transform::IDENTITY,
        Collider::cuboid(size, size, size),
        crate::construction::element_layers(),
        Friction::new(0.6),
    ));
    app.world_mut()
        .entity_mut(tank)
        .insert(AngularVelocity(Vec3::Y * 0.2));
    let mut transferred = 0.0;
    for _ in 0..64 {
        // DERIVED: one second at the fixture clock.
        app.update();
        let world = app.world();
        transferred += world
            .resource::<ContactTransfers>()
            .presses()
            .filter(|(point, ..)| {
                point.other == tank
                    && point.reference == wall
                    && point
                        .other_collider
                        .is_some_and(|collider| world.get::<SidePlate>(collider).is_some())
            })
            // The transfer moves `impulse` into the tank; the wall takes its opposite.
            .map(|(point, ..)| -point.impulse.x * Side::Right.sign())
            .sum::<f32>();
    }
    println!("MEASURED outward plate momentum read by structure: {transferred:.4} N s");
    assert!(
        transferred > 0.0,
        "ordinary plate contacts must reach the structure laws, pressing it outward"
    );
}
