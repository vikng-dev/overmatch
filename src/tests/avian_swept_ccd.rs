//! Compound-body CCD follows each solid collider's risk; non-solid attachments never clamp
//! solid motion. The server relies on thin children leaving unrelated thick contacts discrete
//! and on fast thin and offset rotating colliders retaining sweep protection.
//! Dimensions, velocities and step counts are Fixture data.

use avian3d::dynamics::solver::solver_body::{SolverBodies, SolverBodyFlags, SolverBodyIndex};
use avian3d::prelude::*;
use bevy::prelude::*;

use super::avian_child_pose_churn::physics_app;

fn fast(app: &App, body: Entity) -> bool {
    let index = *app
        .world()
        .get::<SolverBodyIndex>(body)
        .expect("an awake dynamic body has a solver index");
    app.world()
        .resource::<SolverBodies>()
        .get(index)
        .expect("the index names a solver body")
        .flags
        .contains(SolverBodyFlags::IS_FAST)
}

#[test]
fn a_fast_bodys_non_solid_attachment_does_not_stop_it_before_its_solid_wall_contact() {
    for sensor in [true, false] {
        let mut app = physics_app();
        app.insert_resource(Gravity(Vec3::ZERO));
        let wall_center = 1.5;
        let wall_width = 0.02;
        let wall_near = wall_center - wall_width / 2.0;
        let half_solid = 0.02;
        let attachment_offset = 0.75;
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::cuboid(wall_width, 4.0, 4.0),
            Transform::from_xyz(wall_center, 0.0, 0.0),
        ));
        let body = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                SweptCcd::default().with_threshold(0.5),
                Collider::cuboid(2.0 * half_solid, 1.0, 1.0),
                Transform::default(),
            ))
            .id();
        let child = app
            .world_mut()
            .spawn((
                ChildOf(body),
                Collider::cuboid(2.0 * half_solid, 1.0, 1.0),
                Transform::from_xyz(attachment_offset, 0.0, 0.0),
            ))
            .id();
        if sensor {
            app.world_mut().entity_mut(child).insert(Sensor);
        } else {
            app.world_mut().entity_mut(child).insert(ColliderDisabled);
        }
        for _ in 0..3 {
            app.update();
        }
        let step = app
            .world()
            .resource::<Time<Fixed>>()
            .timestep()
            .as_secs_f32();
        app.world_mut()
            .entity_mut(body)
            .insert(LinearVelocity(Vec3::X * (2.0 / step)));
        app.update();
        assert!(
            fast(&app, body),
            "the solid collider itself must require CCD"
        );
        let x = app
            .world()
            .get::<Position>(body)
            .expect("the body keeps its pose")
            .x;
        assert!(
            x > wall_near - attachment_offset,
            "the non-solid attachment clamped the body at its own impact: {x}, sensor={sensor}"
        );
        // Resolution: allow float roundoff at the analytically defined solid/wall plane.
        assert!(
            x + half_solid <= wall_near + 1e-4,
            "the solid collider crossed the wall: {x}, sensor={sensor}"
        );
    }
}

// Fixture: separated children and walls isolate CCD; these also define the asserted planes.
const THIN_HALF: f32 = 0.02;
const THICK_HALF: f32 = 1.0;
const WALL_WIDTH: f32 = 0.01;
const THIN_WALL_X: f32 = 0.08;
const THICK_WALL_X: f32 = 1.06;
const CCD_THRESHOLD: f32 = 0.5;
const TRAVEL: f32 = 0.125;

fn wall_step(compound: bool, thin_wall: bool) -> f32 {
    let mut app = physics_app();
    app.insert_resource(Gravity(Vec3::ZERO));
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            SweptCcd::default().with_threshold(CCD_THRESHOLD),
            Transform::default(),
        ))
        .id();
    app.world_mut().spawn((
        ChildOf(body),
        Collider::cuboid(2.0 * THIN_HALF, 0.4, 0.4),
        Transform::from_xyz(0.0, 4.0, 0.0),
    ));
    if compound {
        app.world_mut().spawn((
            ChildOf(body),
            Collider::cuboid(2.0 * THICK_HALF, 2.0, 2.0),
            Transform::default(),
        ));
    }
    let (wall_x, wall_y) = if thin_wall {
        (THIN_WALL_X, 4.0)
    } else {
        (THICK_WALL_X, 0.0)
    };
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(WALL_WIDTH, 0.5, 0.5),
        Transform::from_xyz(wall_x, wall_y, 0.0),
    ));
    for _ in 0..3 {
        app.update();
    }
    let step = app
        .world()
        .resource::<Time<Fixed>>()
        .timestep()
        .as_secs_f32();
    let before = app
        .world()
        .get::<Position>(body)
        .expect("the body has a pose")
        .x;
    app.world_mut()
        .entity_mut(body)
        .insert(LinearVelocity(Vec3::X * (TRAVEL / step)));
    app.update();
    app.world()
        .get::<Position>(body)
        .expect("the body has a pose")
        .x
        - before
}

#[test]
fn a_thin_child_does_not_force_a_slow_thick_child_to_sweep() {
    // DERIVED from the fixture: travel exceeds the thin threshold but not the thick one.
    let moved = wall_step(true, false);
    assert!(
        moved + THICK_HALF > THICK_WALL_X - WALL_WIDTH / 2.0,
        "a slow thick child was swept: moved {moved}"
    );
}

#[test]
fn a_fast_thin_child_and_a_single_slab_still_stop_at_a_thin_wall() {
    // Single-collider users retain upstream protection; a compound's thin child needs it too.
    for compound in [false, true] {
        let moved = wall_step(compound, true);
        assert!(
            moved > 0.0 && moved + THIN_HALF <= THIN_WALL_X - WALL_WIDTH / 2.0 + 1e-4,
            "tunneled or froze: compound={compound}, moved={moved}"
        );
    }
}

#[test]
fn an_offset_rotating_child_still_sweeps() {
    // Compound-body users need rotation about the body's COM even when it is stationary.
    let mut app = physics_app();
    app.insert_resource(Gravity(Vec3::ZERO));
    // Fixture: a cube rotates about an offset pivot toward a horizontal wall.
    let half = 0.1;
    let radius = 5.0;
    let wall_y = 0.3;
    let wall_width = 0.01;
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            SweptCcd::default().with_threshold(CCD_THRESHOLD),
            CenterOfMass(Vec3::ZERO),
            NoAutoCenterOfMass,
            Transform::default(),
        ))
        .id();
    app.world_mut().spawn((
        ChildOf(body),
        Collider::cuboid(2.0 * half, 2.0 * half, 2.0 * half),
        Transform::from_xyz(radius, 0.0, 0.0),
    ));
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(1.0, wall_width, 1.0),
        Transform::from_xyz(radius, wall_y, 0.0),
    ));
    for _ in 0..3 {
        app.update();
    }
    let step = app
        .world()
        .resource::<Time<Fixed>>()
        .timestep()
        .as_secs_f32();
    app.world_mut()
        .entity_mut(body)
        .insert(AngularVelocity(Vec3::Z * (0.125 / step)));
    app.update();
    let angle = app
        .world()
        .get::<Rotation>(body)
        .expect("body rotation")
        .0
        .to_axis_angle()
        .1;
    let position = app.world().get::<Position>(body).expect("body position");
    // DERIVED: the +X/+Y corner leads under positive Z rotation.
    let leading_y = position.y + (radius + half) * angle.sin() + half * angle.cos();
    // Resolution: allow float roundoff at the fixture's analytically defined wall plane.
    assert!(
        angle > 0.0 && leading_y <= wall_y - wall_width / 2.0 + 1e-4,
        "the offset child's leading corner crossed its wall: angle={angle}, y={leading_y}"
    );
}
