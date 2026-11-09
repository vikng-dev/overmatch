//! The game's server physics: batch installation must retain every attachment, and fully
//! explicit mass properties must stay independent of child colliders. These guard the Avian
//! 0.7 patches in `OVERMATCH_PATCH.md`; automatic and partially explicit bodies still aggregate.
//! All dimensions, densities, properties and observation steps below are Fixture data.

use std::collections::BTreeMap;

use avian3d::dynamics::rigid_body::mass_properties::components::RecomputeMassProperties;
use avian3d::prelude::*;
use bevy::prelude::*;

use super::avian_child_pose_churn::physics_app;

#[derive(Resource, Default)]
struct Attachments(usize);

/// The complete spawn bundle already names the body. Reattaching from hierarchy observers
/// rebuilds the same tree leaf and rewrites its local pose for no change of ownership.
#[test]
fn an_explicit_spawn_attachment_is_installed_once() {
    let mut app = physics_app();
    app.init_resource::<Attachments>()
        .add_observer(|_: On<Insert, ColliderOf>, mut count: ResMut<Attachments>| count.0 += 1);
    let body = app
        .world_mut()
        .spawn((RigidBody::Static, Transform::from_xyz(3.0, 1.0, -2.0)))
        .id();
    let child = app
        .world_mut()
        .spawn((
            ChildOf(body),
            ColliderOf { body },
            Transform::from_xyz(0.5, 1.0, 0.25),
            Collider::cuboid(1.0, 2.0, 0.5),
        ))
        .id();
    assert_eq!(app.world().resource::<Attachments>().0, 1);
    assert_eq!(
        app.world()
            .get::<RigidBodyColliders>(body)
            .unwrap()
            .as_ref(),
        &[child]
    );
    assert_eq!(app.world().get::<ColliderOf>(child).unwrap().body, body);
}

#[derive(Resource, Default)]
struct MassRequests(BTreeMap<Entity, usize>);

fn assert_mass(actual: f32, expected: f32) {
    // Resolution: ComputedMass stores the reciprocal; the round trip rounds twice.
    assert!((actual - expected).abs() <= 2.0 * f32::EPSILON * expected);
}

fn assert_explicit_properties(world: &World, body: Entity) {
    assert_mass(
        world.get::<ComputedMass>(body).unwrap().value(),
        world.get::<Mass>(body).unwrap().0,
    );
    let inertia = world.get::<AngularInertia>(body).unwrap();
    assert_eq!(
        *world.get::<ComputedAngularInertia>(body).unwrap(),
        ComputedAngularInertia::new_with_local_frame(inertia.principal, inertia.local_frame)
    );
    assert_eq!(
        world.get::<ComputedCenterOfMass>(body).unwrap().0,
        world.get::<CenterOfMass>(body).unwrap().0
    );
}

/// Children can change a body's automatic outputs, but cannot change any fully explicit output.
/// The queue requests one aggregate per affected owner; explicit property edits still propagate.
#[test]
fn collider_mass_changes_request_one_aggregate_only_when_the_owner_uses_it() {
    for (fixed_mass, fixed_all) in [(false, false), (true, false), (true, true)] {
        let mut app = physics_app();
        app.insert_resource(Gravity::ZERO)
            .init_resource::<MassRequests>()
            .add_observer(
                |insert: On<Insert, RecomputeMassProperties>, mut counts: ResMut<MassRequests>| {
                    *counts.0.entry(insert.entity).or_default() += 1;
                },
            );
        crate::headless_test::start_fixed_clock(&mut app);
        let body = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Transform::default(),
                RigidBodyColliders::default(),
            ))
            .id();
        if fixed_mass {
            app.world_mut()
                .entity_mut(body)
                .insert((Mass(7.0), NoAutoMass));
        }
        if fixed_all {
            app.world_mut().entity_mut(body).insert((
                AngularInertia::new(Vec3::splat(5.0)),
                CenterOfMass(Vec3::new(0.3, 0.2, -0.1)),
                NoAutoAngularInertia,
                NoAutoCenterOfMass,
            ));
        }
        let children: Vec<_> = app
            .world_mut()
            .spawn_batch([0.0, 2.0].map(|x| {
                (
                    ChildOf(body),
                    ColliderOf { body },
                    Transform::from_xyz(x, 0.0, 0.0),
                    Collider::cuboid(1.0, 1.0, 1.0),
                    ColliderDensity(3.0),
                )
            }))
            .collect();
        app.update();
        app.update();
        app.world_mut().resource_mut::<MassRequests>().0.clear();
        for &child in &children {
            app.world_mut().get_mut::<ColliderDensity>(child).unwrap().0 *= 2.0;
        }
        app.update();
        assert_eq!(
            app.world()
                .resource::<MassRequests>()
                .0
                .get(&body)
                .copied()
                .unwrap_or(0),
            usize::from(!fixed_all),
            "child mass changes queue at most one aggregate, and no unused aggregate"
        );
        let expected_mass = if fixed_mass {
            7.0
        } else {
            children
                .iter()
                .map(|&child| {
                    app.world()
                        .get::<ColliderMassProperties>(child)
                        .unwrap()
                        .mass
                })
                .sum()
        };
        assert_mass(
            app.world().get::<ComputedMass>(body).unwrap().value(),
            expected_mass,
        );
        if !fixed_all {
            // Independent unit-cube integration: I = m/6 per axis, plus the parallel-axis
            // terms about the weighted centre. A Mass-only body adds a point mass at its origin.
            let local_mass = app.world().get::<Mass>(body).map_or(0.0, |m| m.0);
            let points: Vec<_> = children
                .iter()
                .map(|&child| {
                    (
                        app.world().get::<ColliderDensity>(child).unwrap().0,
                        app.world().get::<Transform>(child).unwrap().translation,
                    )
                })
                .collect();
            let total = local_mass + points.iter().map(|(mass, _)| mass).sum::<f32>();
            let centre = points.iter().map(|(mass, at)| *mass * *at).sum::<Vec3>() / total;
            let parallel_axis = |r: Vec3| {
                Mat3::from_diagonal(Vec3::splat(r.length_squared()))
                    - Mat3::from_cols(r * r.x, r * r.y, r * r.z)
            };
            let tensor =
                points
                    .iter()
                    .fold(local_mass * parallel_axis(-centre), |sum, (mass, at)| {
                        sum + *mass
                            * (Mat3::from_diagonal(Vec3::splat(1.0 / 6.0))
                                + parallel_axis(*at - centre))
                    })
                    * (expected_mass / total);
            // Resolution: f32 aggregation and principal-axis diagonalization of fixture cubes.
            let tolerance = 1e-5;
            assert!(
                (app.world().get::<ComputedCenterOfMass>(body).unwrap().0 - centre).length()
                    < tolerance
            );
            let actual = app
                .world()
                .get::<ComputedAngularInertia>(body)
                .unwrap()
                .tensor()
                .to_mat3();
            assert!(
                (actual - tensor)
                    .to_cols_array()
                    .iter()
                    .all(|v| v.abs() < tolerance)
            );
        }
        if fixed_all {
            assert_explicit_properties(app.world(), body);
            app.world_mut().entity_mut(body).insert((
                Mass(11.0),
                AngularInertia::new(Vec3::splat(9.0)),
                CenterOfMass(Vec3::new(-0.2, 0.4, 0.1)),
            ));
            app.update();
            assert_explicit_properties(app.world(), body);
        }
    }
}

/// A body's own collider is self-attached. Its explicit property edits must reach the computed
/// values even though its collider's geometry contributes nothing to those values.
#[test]
fn an_explicit_body_with_its_own_collider_still_updates_its_properties() {
    let mut app = physics_app();
    app.insert_resource(Gravity::ZERO);
    crate::headless_test::start_fixed_clock(&mut app);
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Transform::default(),
            Collider::cuboid(1.0, 1.0, 1.0),
            Mass(7.0),
            AngularInertia::new(Vec3::splat(5.0)),
            CenterOfMass(Vec3::new(0.3, 0.2, -0.1)),
            NoAutoMass,
            NoAutoAngularInertia,
            NoAutoCenterOfMass,
        ))
        .id();
    app.update();
    app.update();
    assert_eq!(app.world().get::<ColliderOf>(body).unwrap().body, body);
    assert_explicit_properties(app.world(), body);
    app.world_mut().entity_mut(body).insert((
        Mass(11.0),
        AngularInertia::new(Vec3::splat(9.0)),
        CenterOfMass(Vec3::new(-0.2, 0.4, 0.1)),
    ));
    app.update();
    assert_explicit_properties(app.world(), body);
}
