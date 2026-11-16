//! Guards for the game's required-state gate (`avian_ext::required_state`) and Avian's stationary
//! miss cache (`vendor/avian3d-0.8.0-overmatch`, `OVERMATCH_PATCH.md`). Game state only: the server's
//! contacts between bodies. The gate refuses incomplete posed participants while preserving
//! unposed and disabled states; the cache skips only geometry queries whose inputs remain equal.

use avian3d::collision::CollisionDiagnostics;
use avian3d::prelude::*;
use bevy::prelude::*;

use super::avian_child_pose_churn::physics_app;

/// Fixture: separated spheres with overlapping AABBs, at a scale whose gap exceeds the configured
/// contact tolerance. Sensors keep contact tests independent of solver response. One fixed step per
/// update, at whatever the fixed timestep is. Returns the app, the static sphere, the dynamic sphere
/// and their radius (m).
fn stationary_pair() -> (App, Entity, Entity, f32) {
    let mut app = physics_app();
    crate::headless_test::start_fixed_clock(&mut app);
    app.insert_resource(Gravity::ZERO);
    let tolerance = app
        .world()
        .resource::<NarrowPhaseConfig>()
        .contact_tolerance
        * app.world().resource::<PhysicsLengthUnit>().0;
    // DERIVED: sqrt(2) * 1.5r - 2r > tolerance for r = 1 + 10t.
    let radius = 1.0 + 10.0 * tolerance;
    let fixed = app
        .world_mut()
        .spawn((
            RigidBody::Static,
            Transform::IDENTITY,
            Collider::sphere(radius),
        ))
        .id();
    let moving = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Sensor,
            Transform::from_xyz(1.5 * radius, 1.5 * radius, 0.0),
            Collider::sphere(radius),
        ))
        .id();
    for _ in 0..4 {
        app.update();
    }
    (app, fixed, moving, radius)
}

/// Contact users need a pair whose collider lost state the narrow phase reads refused, not left
/// with its last update. A mutant that skips every pair it cannot read keeps it silently.
#[test]
#[should_panic(expected = "a posed enabled collider retains CollisionLayers and EnlargedAabb")]
fn a_pair_whose_collider_lost_its_layers_is_refused() {
    let (mut app, fixed, _, _) = stationary_pair();
    app.world_mut()
        .entity_mut(fixed)
        .remove::<CollisionLayers>();
    app.update();
}

/// The client's replication may lift a body's pose for a frame (`crate::tests::avian_body_query_gap`),
/// and the narrow phase skips its pairs for that frame. A mutant that refuses every pair it cannot
/// read stops the client there.
#[test]
fn a_pair_whose_collider_lost_its_pose_is_skipped() {
    let (mut app, fixed, moving, _) = stationary_pair();
    let pose = *app.world().get::<Position>(fixed).unwrap();
    app.world_mut().entity_mut(fixed).remove::<Position>();
    app.update();
    app.world_mut().entity_mut(fixed).insert(pose);
    app.update();
    assert!(
        app.world()
            .resource::<ContactGraph>()
            .get(fixed, moving)
            .is_some()
    );
}

/// Contact users need a pair whose body lost state the narrow phase reads refused, not read as
/// bodiless. A mutant that reads every body it cannot fetch as absent keeps it silently.
#[test]
#[should_panic(expected = "a posed enabled body retains LinearVelocity")]
fn a_pair_whose_body_lost_its_velocity_is_refused() {
    let (mut app, _, moving, _) = stationary_pair();
    app.world_mut()
        .entity_mut(moving)
        .remove::<LinearVelocity>();
    app.update();
}

/// A disabled body's pairs generate no constraints, and the narrow phase reads the body as absent
/// (as it does a client body whose pose replication lifted: `crate::tests::avian_body_query_gap`).
/// A mutant that refuses every body it cannot fetch stops there.
#[test]
fn a_pair_whose_body_is_disabled_reads_it_as_absent() {
    let (mut app, fixed, _, radius) = stationary_pair();
    let disabled = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            RigidBodyDisabled,
            Transform::from_xyz(-1.5 * radius, 0.0, 0.0),
            Collider::sphere(radius),
        ))
        .id();
    app.update();
    let (_, pair) = app
        .world()
        .resource::<ContactGraph>()
        .get(fixed, disabled)
        .expect("the broad phase pairs a disabled body's collider");
    assert!(pair.is_touching() && !pair.generates_constraints());
}

fn miss_work(app: &App) -> (u32, u32) {
    let work = app.world().resource::<CollisionDiagnostics>();
    (
        work.stationary_non_touching_pairs,
        work.stationary_non_touching_queries,
    )
}

/// The server's collapse reader needs per-step counts of eligible pairs and actual geometry work.
/// Re-testing a held miss, counting moving pairs or retaining a previous tally breaks it.
#[test]
fn stationary_misses_reuse_geometry_and_reset_each_step() {
    let (mut app, _, moving, _) = stationary_pair();
    for _ in 0..2 {
        app.update();
        assert_eq!(miss_work(&app), (1, 0));
        assert_eq!(
            app.world()
                .resource::<ContactGraph>()
                .iter_active_touching()
                .count(),
            0
        );
    }
    // Fixture velocity: preserve bounds overlap while making the owner nonstationary.
    app.world_mut()
        .entity_mut(moving)
        .insert(LinearVelocity(Vec3::Z * 0.1));
    app.update();
    assert_eq!(
        app.world().resource::<CollisionDiagnostics>().contact_count,
        1
    );
    assert_eq!(miss_work(&app), (0, 0));
    app.world_mut().entity_mut(moving).despawn();
    app.update();
    assert_eq!(miss_work(&app), (0, 0));
}

/// A zero-velocity teleport must discover contact on that step. The solver also relies on
/// touching pairs refreshing their geometry every step, even while their inputs stay fixed.
#[test]
fn stationary_miss_retests_on_motion_and_touching_pairs_always_query() {
    let (mut app, fixed, moving, radius) = stationary_pair();
    app.world_mut().get_mut::<Position>(moving).unwrap().0 = Vec3::X * radius;
    app.update();
    assert_eq!(
        miss_work(&app),
        (1, 1),
        "the moved pair queries immediately"
    );
    assert!(
        app.world()
            .resource::<ContactGraph>()
            .get(fixed, moving)
            .unwrap()
            .1
            .is_touching()
    );
    for _ in 0..2 {
        // Keep TOUCHING and every query input unchanged; stale manifold reuse cannot repair this.
        app.world_mut()
            .resource_mut::<ContactGraph>()
            .active_pairs_mut()[0]
            .manifolds
            .clear();
        app.update();
        let graph = app.world().resource::<ContactGraph>();
        let pair = graph.get(fixed, moving).unwrap().1;
        assert!(pair.is_touching() && !pair.manifolds.is_empty());
        assert_eq!(miss_work(&app), (0, 0));
    }
}

/// The server must re-query when any independent geometry/pruning key field changes.
/// Static owners retain authored velocities without integrating their pose; a small linear
/// velocity stays under the tolerance floor, isolating it from the speculative margin.
/// All changed magnitudes are Fixture data.
#[test]
fn stationary_miss_invalidates_each_geometry_and_pruning_input() {
    for change in [
        "rotation",
        "fixed pose",
        "shape",
        "fixed shape",
        "fixed owner",
        "relative linear velocity",
        "angular velocity",
        "COM",
        "step length",
        "collider margin",
        "body margin",
        "tolerance",
        "scale",
    ] {
        let (mut app, fixed, moving, radius) = stationary_pair();
        let collider = if change == "body margin" {
            // Make the shape a child so the body owns its inherited margin.
            app.world_mut().entity_mut(moving).remove::<Collider>();
            let child = app
                .world_mut()
                .spawn((
                    ChildOf(moving),
                    Transform::IDENTITY,
                    Collider::sphere(radius),
                    Sensor,
                ))
                .id();
            for _ in 0..3 {
                app.update();
            }
            child
        } else {
            moving
        };
        assert_eq!(miss_work(&app), (1, 0), "fixture primes a miss: {change}");
        match change {
            "rotation" => {
                app.world_mut().get_mut::<Rotation>(moving).unwrap().0 = Quat::from_rotation_z(0.1)
            }
            "fixed pose" => {
                app.world_mut().get_mut::<Position>(fixed).unwrap().0.z = 0.1 * radius;
            }
            "fixed shape" => {
                app.world_mut()
                    .entity_mut(fixed)
                    .insert(Collider::sphere(radius));
            }
            "fixed owner" => {
                let owner = app.world_mut().spawn(RigidBody::Static).id();
                app.world_mut()
                    .entity_mut(fixed)
                    .insert(ColliderOf { body: owner });
            }
            "relative linear velocity" => {
                let tolerance = app
                    .world()
                    .resource::<NarrowPhaseConfig>()
                    .contact_tolerance
                    * app.world().resource::<PhysicsLengthUnit>().0;
                app.world_mut().get_mut::<LinearVelocity>(fixed).unwrap().0 = Vec3::X * tolerance;
            }
            "angular velocity" => {
                app.world_mut().get_mut::<AngularVelocity>(fixed).unwrap().0 = Vec3::Z;
            }
            "COM" => {
                app.world_mut()
                    .get_mut::<ComputedCenterOfMass>(fixed)
                    .unwrap()
                    .0 = Vec3::X;
            }
            "step length" => {
                let mut time = app.world_mut().resource_mut::<Time<Fixed>>();
                let timestep = time.timestep();
                time.set_timestep(timestep * 2);
            }
            "shape" => {
                app.world_mut()
                    .entity_mut(collider)
                    .insert(Collider::sphere(radius));
            }
            "collider margin" => {
                app.world_mut()
                    .entity_mut(collider)
                    .insert(CollisionMargin(radius));
            }
            "body margin" => {
                app.world_mut()
                    .entity_mut(moving)
                    .insert(CollisionMargin(radius));
            }
            "tolerance" => {
                app.world_mut()
                    .resource_mut::<NarrowPhaseConfig>()
                    .contact_tolerance = radius
            }
            "scale" => app.world_mut().resource_mut::<PhysicsLengthUnit>().0 *= 100.0,
            _ => unreachable!(),
        }
        app.update();
        assert_eq!(miss_work(&app), (1, 1), "same-step invalidation: {change}");
    }
}

/// Waking alone cannot change the pure geometry result; waking and moving must discover
/// contact on the same step, including after an intervening sleep episode.
#[test]
fn stationary_miss_retests_when_a_woken_body_moves() {
    for resleep in [false, true] {
        let (mut app, fixed, moving, radius) = stationary_pair();
        app.world_mut().entity_mut(moving).insert(Sleeping);
        for _ in 0..3 {
            app.update();
        }
        assert!(app.world().get::<Sleeping>(moving).is_some());
        assert_eq!(miss_work(&app), (1, 0));
        app.world_mut().entity_mut(moving).remove::<Sleeping>();
        if resleep {
            app.world_mut().entity_mut(moving).insert(Sleeping);
        }
        app.update();
        assert_eq!(
            miss_work(&app),
            (1, 0),
            "unchanged inputs preserve the miss"
        );
        app.world_mut().entity_mut(moving).remove::<Sleeping>();
        app.world_mut().get_mut::<Position>(moving).unwrap().0 = Vec3::X * radius;
        app.update();
        assert_eq!(miss_work(&app), (1, 1));
        assert!(
            app.world()
                .resource::<ContactGraph>()
                .get(fixed, moving)
                .unwrap()
                .1
                .is_touching()
        );
    }
}

/// A stationary miss belongs to one pair; a newly inserted neighbor must still discover contact.
#[test]
fn stationary_miss_does_not_hide_a_new_neighbor() {
    let (mut app, _, moving, radius) = stationary_pair();
    let position = *app.world().get::<Position>(moving).unwrap();
    let newcomer = app
        .world_mut()
        .spawn((RigidBody::Static, position, Collider::sphere(radius)))
        .id();
    app.update();
    assert_eq!(miss_work(&app), (2, 1));
    assert!(
        app.world()
            .resource::<ContactGraph>()
            .get(moving, newcomer)
            .unwrap()
            .1
            .is_touching()
    );
}

/// Child pose changes and ownership changes invalidate the pair even while both roots stay still.
#[test]
fn stationary_miss_follows_child_motion_and_rehoming() {
    let (mut app, _, body, radius) = stationary_pair();
    app.world_mut().entity_mut(body).remove::<Collider>();
    let child = app
        .world_mut()
        .spawn((
            ChildOf(body),
            Transform::IDENTITY,
            Collider::sphere(radius),
            Sensor,
        ))
        .id();
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(miss_work(&app), (1, 0));
    app.world_mut()
        .get_mut::<Transform>(child)
        .unwrap()
        .translation
        .z = 0.1 * radius;
    app.update();
    assert_eq!(miss_work(&app), (1, 1));
    app.update();
    assert_eq!(miss_work(&app), (1, 0));
    let position = *app.world().get::<Position>(body).unwrap();
    let other = app.world_mut().spawn((RigidBody::Dynamic, position)).id();
    app.world_mut().entity_mut(child).insert(ChildOf(other));
    app.update();
    assert_eq!(miss_work(&app), (1, 1));
}

#[derive(Resource)]
struct AdmitContact(bool);

#[derive(bevy::ecs::system::SystemParam)]
struct AdmissionHook<'w> {
    admit: Res<'w, AdmitContact>,
}

impl CollisionHooks for AdmissionHook<'_> {
    fn modify_contacts(&self, _: &mut ContactPair, _: &mut Commands) -> bool {
        self.admit.0
    }
}

/// The game's environment destruction sweep can reject a real look-ahead contact for this
/// step only. A hook rejection is never a cached geometry miss; changed sweep state must be read.
#[test]
fn stationary_hook_rejection_is_reconsidered_on_the_next_step() {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::transform::TransformPlugin,
        AssetPlugin::default(),
        PhysicsPlugins::default().with_collision_hooks::<AdmissionHook<'static>>(),
    ))
    .init_asset::<Mesh>()
    .insert_resource(Gravity::ZERO)
    // Recycling off, as the game composes physics: a recycled pair returns before its hook.
    .insert_resource(NarrowPhaseConfig {
        recycle_distance: 0.0,
        ..default()
    })
    .insert_resource(AdmitContact(false));
    crate::headless_test::start_fixed_clock(&mut app);
    crate::headless_test::finish_plugins(&mut app);
    // Fixture: overlapping sensor spheres, so the solver cannot move either participant.
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::sphere(1.0),
        ActiveCollisionHooks::MODIFY_CONTACTS,
    ));
    app.world_mut()
        .spawn((RigidBody::Dynamic, Collider::sphere(1.0), Sensor));
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(
        app.world()
            .resource::<ContactGraph>()
            .iter_active_touching()
            .count(),
        0
    );
    app.world_mut().resource_mut::<AdmitContact>().0 = true;
    app.update();
    assert_eq!(
        app.world()
            .resource::<ContactGraph>()
            .iter_active_touching()
            .count(),
        1
    );
}
