//! UPSTREAM TRIPWIRE for the avian3d 0.7 narrow-phase defect that the net client's all-static
//! physics world routes around (`apply_launched_turret_pose` in `net::protocol` launches the
//! client's turret as a static body). Full decode and suggested upstream fix:
//! `upstream/avian-body-query-gap-stale-constraints.md`.
//!
//! The mechanism, in avian3d 0.7 (`collision/narrow_phase/system_param.rs`, `update_contacts` and
//! `update`): a pair generates contact constraints only while both colliders' bodies match the
//! narrow phase's body query, whose `RigidBodyQuery` needs the body's `Position`. When a body
//! leaves that query, its pairs' generate-constraints flag clears but the constraint graph keeps
//! their manifold handles. When the body comes back, the pair's started-generating status pushes
//! one handle per manifold again on top of the kept ones. `prepare_contact_constraints`
//! (`dynamics/solver/plugin.rs:398`) indexes the pair's manifolds with every handle before it looks
//! at the flag, so the first handle past the end panics: "index out of bounds: the len is 1 but the
//! index is 1". The gap staged here is the one lightyear's interpolation makes on the client: at a
//! resync of its timeline it removes `Position` from every interpolated body for a frame, then
//! inserts it again.
//!
//! **IF `a_body_query_gap_still_panics_the_solver` FAILS, avian fixed the bookkeeping.** The test
//! is inverted on purpose: it passes while upstream is broken, so its failure is the retirement
//! signal, not a regression. On that day the net client may hold a non-static body again; delete
//! this file and its `mod` line in `src/tests.rs`, and mark the upstream record fixed, citing the
//! fix.
//!
//! `IslandPlugin` is off. With it on, a build with debug assertions stops one call earlier on the
//! same double add, at the island manager's `debug_assert!(contact.island.is_none())`
//! (`dynamics/solver/islands/mod.rs:518`); a release build, the shipped client, goes on to the
//! solver's index panic.
//!
//! The panic is raised on a compute-pool thread and resumed on this one, so `catch_unwind` observes
//! it. A `thread ... panicked at ... plugin.rs:398` line in the test output is EXPECTED here.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

use avian3d::dynamics::solver::islands::{IslandPlugin, IslandSleepingPlugin};
use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

/// Avian's default plugins without islands, one 64 Hz step per `update`.
fn physics_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::transform::TransformPlugin,
        // Avian's collider cache reads `AssetEvent<Mesh>`.
        AssetPlugin::default(),
        PhysicsPlugins::default()
            .build()
            .disable::<IslandSleepingPlugin>()
            .disable::<IslandPlugin>(),
    ))
    .init_asset::<Mesh>()
    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
        15_625,
    )));
    while app.plugins_state() == bevy::app::PluginsState::Adding {
        std::thread::sleep(Duration::from_millis(1));
    }
    app.finish();
    app.cleanup();
    app
}

/// THE TRIPWIRE. A kinematic box rests 1 cm into a static body's box; the static body's collider is
/// its child, as a tank's are, so the collider stays queryable while the body's `Position` is gone.
/// Upstream is broken while this passes; see the module header for what its failure demands.
#[test]
fn a_body_query_gap_still_panics_the_solver() {
    let mut app = physics_app();
    let ground = app
        .world_mut()
        .spawn((RigidBody::Static, Position(Vec3::new(0.0, -0.5, 0.0))))
        .id();
    app.world_mut().spawn((
        ChildOf(ground),
        Transform::IDENTITY,
        Collider::cuboid(4.0, 1.0, 4.0),
    ));
    app.world_mut().spawn((
        RigidBody::Kinematic,
        Position(Vec3::new(0.0, 0.49, 0.0)),
        Collider::cuboid(1.0, 1.0, 1.0),
    ));
    for _ in 0..4 {
        app.update();
    }
    let pose = *app
        .world()
        .get::<Position>(ground)
        .expect("the body's pose");
    app.world_mut().entity_mut(ground).remove::<Position>();
    app.update();
    app.world_mut().entity_mut(ground).insert(pose);

    let outcome = catch_unwind(AssertUnwindSafe(|| app.update()));
    let payload = outcome.err().unwrap_or_else(|| {
        panic!(
            "avian no longer panics when a touching body leaves and re-enters the narrow phase's \
             body query: upstream fixed the constraint bookkeeping. The net client may hold a \
             non-static body again; delete this test and update \
             upstream/avian-body-query-gap-stale-constraints.md (see module header)."
        )
    });
    // Pin the mechanism, not just "something panicked": a different panic means the defect moved,
    // and the client's all-static rule has to be re-derived before it is trusted.
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .unwrap_or_else(|| "<non-string panic payload>".into());
    assert!(
        message.contains("index out of bounds: the len is 1 but the index is 1"),
        "expected the plugin.rs:398 manifold-index panic, got a different one: {message}",
    );
}
