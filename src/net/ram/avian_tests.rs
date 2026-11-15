//! Physics-work accounting for the game's server physics, game state only.
//! The collapse reader needs actual dispatched work, independent of settlement outcomes.

use avian3d::dynamics::solver::constraint_graph::COLOR_OVERFLOW_INDEX;
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
/// per substep, plus one restitution pass, including its zero-restitution no-ops. On a compute
/// pool of more than one thread the fork's solver dispatches a color through parallel chunks from
/// 64 constraints (`par_for_each(.., 64, ..)` in its `dynamics/solver/plugin.rs`), so a small batch's
/// visits are all serial and a large one's all parallel; a dispatch counted in the other column
/// breaks the classification.
#[test]
fn contact_visits_cover_each_pass_and_reset_each_step() {
    // Fixture: a pool of four threads, unless another test of this process built the global pool
    // first; Bevy's defaults give more than one compute thread from four cores.
    let pool = bevy::tasks::ComputeTaskPool::get_or_init(|| {
        bevy::tasks::TaskPoolBuilder::new().num_threads(4).build()
    });
    assert!(
        pool.thread_num() > 1,
        "fixture: classifying the dispatch needs a compute pool of more than one thread"
    );
    // Fixture sizes: one batch below the parallel floor, one above it.
    for (count, parallel) in [(4, false), (128, true)] {
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
            let visits = step.constraints * (3 * substeps + 1);
            let expected = if parallel { (0, visits) } else { (visits, 0) };
            assert_eq!(
                (step.serial_contact_visits, step.parallel_contact_visits),
                expected,
                "{count} contacts: (serial, parallel) visits"
            );
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
