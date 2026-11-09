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

/// The collapse reader distinguishes active stationary misses from touching/sleeping pairs.
/// Dropping the geometry call, counting moving pairs or retaining last step's tally breaks it.
#[test]
fn stationary_misses_count_queries_and_reset_each_step() {
    let mut app = physics_app();
    let tolerance = app
        .world()
        .resource::<NarrowPhaseConfig>()
        .contact_tolerance
        * app.world().resource::<PhysicsLengthUnit>().0;
    // Fixture scale. DERIVED: sqrt(2) * 1.5r - 2r > tolerance for r = 1 + 10t.
    let radius = 1.0 + 10.0 * tolerance;
    app.world_mut().spawn((
        RigidBody::Static,
        Transform::IDENTITY,
        Collider::sphere(radius),
    ));
    let moving = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Transform::from_xyz(1.5 * radius, 1.5 * radius, 0.0),
            Collider::sphere(radius),
        ))
        .id();
    for _ in 0..4 {
        app.update();
    }
    for _ in 0..2 {
        app.update();
        let work = app.world().resource::<CollisionDiagnostics>();
        assert_eq!(work.stationary_non_touching_pairs, 1);
        assert_eq!(work.stationary_non_touching_queries, 1);
        assert_eq!(
            app.world()
                .resource::<ContactGraph>()
                .iter_active_touching()
                .count(),
            0
        );
    }
    // Fixture velocity: preserve bounds overlap while making the dynamic owner nonstationary.
    app.world_mut()
        .entity_mut(moving)
        .insert(LinearVelocity(Vec3::Z * 0.1));
    app.update();
    let work = app.world().resource::<CollisionDiagnostics>();
    assert_eq!(work.contact_count, 1, "moving pair must remain active");
    assert_eq!(work.stationary_non_touching_pairs, 0);
    assert_eq!(work.stationary_non_touching_queries, 0);
    app.world_mut().entity_mut(moving).despawn();
    app.update();
    let work = app.world().resource::<CollisionDiagnostics>();
    assert_eq!(work.stationary_non_touching_pairs, 0);
    assert_eq!(work.stationary_non_touching_queries, 0);
    assert_eq!(
        app.world()
            .resource::<SolverDiagnostics>()
            .serial_contact_visits,
        0
    );
}
