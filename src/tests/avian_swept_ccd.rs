//! Under translation, thin sensors and disabled colliders neither make a thick body fast nor
//! clamp a fast body's motion. The server relies on non-solid attachments leaving solid
//! collision unchanged.
//! Marker transitions refresh the cached thickness even with the centre of mass held fixed.
//! The guard reads Avian's existing IS_FAST diagnostic; it adds no engine instrumentation.
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
fn a_thin_non_solid_attachment_does_not_make_a_thick_body_fast() {
    for sensor in [true, false] {
        let mut app = physics_app();
        app.insert_resource(Gravity(Vec3::ZERO));
        let body = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                NoAutoCenterOfMass,
                SweptCcd::default().with_threshold(0.5),
                Collider::cuboid(2.0, 2.0, 2.0),
                Transform::default(),
            ))
            .id();
        let child = app
            .world_mut()
            .spawn((
                ChildOf(body),
                Collider::cuboid(0.04, 1.0, 1.0),
                Transform::default(),
            ))
            .id();
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
            .insert(LinearVelocity(Vec3::X * (0.125 / step)));
        app.update();
        assert!(
            fast(&app, body),
            "the enabled thin collider must exercise the fast-body threshold"
        );
        if sensor {
            app.world_mut().entity_mut(child).insert(Sensor);
        } else {
            app.world_mut().entity_mut(child).insert(ColliderDisabled);
        }
        app.update();
        assert!(
            !fast(&app, body),
            "a non-solid attachment made the thick body fast (sensor={sensor})"
        );
        if sensor {
            app.world_mut().entity_mut(child).remove::<Sensor>();
        } else {
            app.world_mut()
                .entity_mut(child)
                .remove::<ColliderDisabled>();
        }
        app.update();
        assert!(
            fast(&app, body),
            "restoring a thin solid collider must restore its CCD thickness"
        );
    }
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
