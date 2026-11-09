//! Physics-work accounting for the game's server physics, game state only.
//! The collapse reader needs actual dispatched work, independent of settlement outcomes.

use avian3d::collision::CollisionDiagnostics;
use avian3d::dynamics::solver::{SolverDiagnostics, constraint_graph::COLOR_OVERFLOW_INDEX};
use avian3d::prelude::*;
use bevy::prelude::*;

use super::AvianDiagnostics;

fn physics_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        TransformPlugin,
        AssetPlugin::default(),
        crate::offline_physics(),
        crate::contact_transfer::sim_plugin,
    ))
    .init_asset::<Mesh>()
    .insert_resource(Gravity::ZERO);
    crate::headless_test::start_fixed_clock(&mut app);
    crate::headless_test::finish_plugins(&mut app);
    app
}

fn snapshot(app: &mut App) -> crate::cost::AvianStep {
    app.world_mut()
        .run_system_cached(|mut diagnostics: AvianDiagnostics| diagnostics.step())
        .unwrap()
}

/// Omitting a contact pass, multiplying by points or accumulating across steps breaks the
/// collapse reader's work account. DERIVED visits: warm start, biased solve and relaxation
/// per substep, plus one restitution pass, including its zero-restitution no-ops.
#[test]
fn contact_visits_cover_each_pass_and_reset_each_step() {
    // Fixture sizes exercise small and large independent contact batches.
    for count in [4, 128] {
        let mut app = physics_app();
        app.world_mut().spawn((
            RigidBody::Static,
            Transform::from_xyz(2.0 * count as f32, -0.5, 0.0),
            Collider::cuboid(4.0 * count as f32, 1.0, 4.0),
        ));
        for i in 0..count {
            app.world_mut().spawn((
                RigidBody::Dynamic,
                Transform::from_xyz(4.0 * i as f32, 0.5, 0.0),
                Collider::cuboid(1.0, 1.0, 1.0),
                Restitution::ZERO,
                SleepingDisabled,
            ));
        }
        for _ in 0..4 {
            app.update();
        }
        for _ in 0..2 {
            app.update();
            let step = snapshot(&mut app);
            let substeps = app.world().resource::<SubstepCount>().0;
            assert!(step.constraints > 0, "fixture must supply contact work");
            assert_eq!(
                step.max_dynamic_manifold_degree, 1,
                "each dynamic cube owns one manifold; the shared static floor must be excluded"
            );
            assert_eq!(
                step.serial_contact_visits + step.parallel_contact_visits,
                step.constraints * (3 * substeps + 1)
            );
            if bevy::tasks::ComputeTaskPool::get().thread_num() == 1 {
                assert_eq!(step.parallel_contact_visits, 0);
            }
        }
    }
}

/// The collapse reader needs the busiest dynamic owner, including its serial overflow.
/// Dropping overflow or counting points instead of manifolds breaks this star graph's
/// incidence identity. All dimensions/counts are Fixture data.
#[test]
fn dynamic_degree_includes_overflow_and_exports_inside_avian() {
    let mut app = physics_app();
    let count = COLOR_OVERFLOW_INDEX + 2;
    app.world_mut().spawn((
        RigidBody::Dynamic,
        Transform::from_xyz(1.5 * count as f32, 1.5, 0.0),
        Collider::cuboid(3.0 * count as f32, 1.0, 4.0),
        SleepingDisabled,
    ));
    for i in 0..count {
        app.world_mut().spawn((
            RigidBody::Static,
            Transform::from_xyz(3.0 * i as f32, 0.0, 0.0),
            Collider::cuboid(1.0, 2.0, 1.0),
        ));
    }
    for _ in 0..4 {
        app.update();
    }
    let step = snapshot(&mut app);
    assert!(step.overflow_constraints > 0, "fixture must reach overflow");
    assert!(
        step.points > step.constraints,
        "fixture must distinguish points from manifolds"
    );
    assert_eq!(step.max_dynamic_manifold_degree, step.constraints);
    let substeps = app.world().resource::<SubstepCount>().0;
    let visits_per_constraint = 3 * substeps + 1;
    assert!(step.serial_contact_visits >= step.overflow_constraints * visits_per_constraint);
    assert_eq!(
        step.serial_contact_visits + step.parallel_contact_visits,
        step.constraints * visits_per_constraint
    );
    let mut tick = crate::cost::TickSample::idle();
    tick.avian = step;
    let row = serde_json::to_value(tick).unwrap();
    assert_eq!(
        row["avian"]["serial_contact_visits"],
        step.serial_contact_visits
    );
    assert_eq!(
        row["avian"]["max_dynamic_manifold_degree"],
        step.constraints
    );
    assert!(row.get("solve_constraints_us").is_none());

    // No prepared constraints means no dynamic incidence, even with live bodies remaining.
    app.world_mut()
        .resource_mut::<avian3d::dynamics::solver::constraint_graph::ConstraintGraph>()
        .colors
        .iter_mut()
        .for_each(|color| color.contact_constraints.clear());
    assert_eq!(snapshot(&mut app).max_dynamic_manifold_degree, 0);
}

// Fixture: separated spheres with overlapping AABBs, at a scale whose gap exceeds the
// configured contact tolerance. Sensors keep contact tests independent of solver response.
fn stationary_pair() -> (App, Entity, Entity, f32) {
    let mut app = physics_app();
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

fn miss_work(app: &App) -> (u32, u32) {
    let work = app.world().resource::<CollisionDiagnostics>();
    (
        work.stationary_non_touching_pairs,
        work.stationary_non_touching_queries,
    )
}

/// The collapse reader needs per-step counts of eligible pairs and actual geometry work.
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
    assert_eq!(
        app.world()
            .resource::<SolverDiagnostics>()
            .serial_contact_visits,
        0
    );
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

/// The narrow phase must invalidate a held miss for rotation, shape revision, either margin
/// owner, global tolerance/scale and a wake, even when linear velocity is zero.
#[test]
fn stationary_miss_invalidates_geometry_changes_and_wake() {
    for change in [
        "rotation",
        "shape",
        "collider margin",
        "body margin",
        "tolerance",
        "scale",
        "wake",
        "resleep",
    ] {
        let (mut app, _, moving, radius) = stationary_pair();
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
        if matches!(change, "wake" | "resleep") {
            app.world_mut().entity_mut(moving).insert(Sleeping);
            for _ in 0..3 {
                app.update();
            }
            assert!(app.world().get::<Sleeping>(moving).is_some());
        }
        assert_eq!(miss_work(&app), (1, 0), "fixture primes a miss: {change}");
        match change {
            "rotation" => {
                app.world_mut().get_mut::<Rotation>(moving).unwrap().0 = Quat::from_rotation_z(0.1)
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
            "wake" => {
                app.world_mut().entity_mut(moving).remove::<Sleeping>();
            }
            "resleep" => {
                app.world_mut().entity_mut(moving).remove::<Sleeping>();
                app.world_mut().entity_mut(moving).insert(Sleeping);
            }
            _ => unreachable!(),
        }
        app.update();
        assert_eq!(miss_work(&app), (1, 1), "same-step invalidation: {change}");
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
        TransformPlugin,
        AssetPlugin::default(),
        PhysicsPlugins::default().with_collision_hooks::<AdmissionHook<'static>>(),
    ))
    .init_asset::<Mesh>()
    .insert_resource(Gravity::ZERO)
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
