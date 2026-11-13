//! Avian resting-contact and island lifecycle guards for the game’s server physics. Resting bodies require a wake
//! source; joining a sleeper must not erase an awake body's accumulated rest time.
//! Geometry and observation counts below are Fixture data, not settlement targets.
//! Mechanism and upstream draft: `upstream/avian-island-rest-history.md`.

use avian3d::dynamics::solver::{
    islands::{BodyIslandNode, PhysicsIslands, WakeIslands},
    joint_graph::JointGraph,
};
use avian3d::prelude::*;
use bevy::ecs::{entity_disabling::Disabled, system::RunSystemOnce};
use bevy::prelude::*;

use super::avian_child_pose_churn::physics_app;

fn body(app: &mut App, position: Vec3) -> Entity {
    app.world_mut()
        .spawn((
            RigidBody::Dynamic,
            Position(position),
            Collider::cuboid(0.4, 0.4, 0.4),
        ))
        .id()
}

fn until_asleep(app: &mut App, body: Entity) {
    for _ in 0..640 {
        app.update();
        if app.world().get::<Sleeping>(body).is_some() {
            return;
        }
    }
    panic!("fixture body must settle naturally before the wake witness");
}

/// Server physics relies on a new stationary contact supplying no wake source. A mutant that
/// wakes every island on contact creation reactivates the resting body.
#[test]
fn stationary_contact_does_not_wake_a_resting_island() {
    let mut app = physics_app();
    app.insert_resource(Gravity(Vec3::NEG_Y * 9.81));
    let slab = |app: &mut App| {
        app.world_mut()
            .spawn((
                RigidBody::Static,
                Position(Vec3::NEG_Y * 0.5),
                Collider::cuboid(20.0, 1.0, 20.0),
            ))
            .id()
    };
    slab(&mut app);
    let resting = body(&mut app, Vec3::Y * 0.2);
    until_asleep(&mut app, resting);
    for _ in 0..64 {
        app.update();
        assert!(
            app.world().get::<Sleeping>(resting).is_some(),
            "no input must leave a sleeper asleep"
        );
    }
    let support = slab(&mut app);
    app.update();
    assert!(
        app.world()
            .resource::<ContactGraph>()
            .get(support, resting)
            .is_some_and(|(edge, _)| edge.is_touching()),
        "the new stationary support reaches the sleeper"
    );
    assert!(
        app.world().get::<Sleeping>(resting).is_some(),
        "a stationary support supplies no awake non-static participant"
    );
    let (edge, _) = app
        .world()
        .resource::<ContactGraph>()
        .get(support, resting)
        .unwrap();
    assert!(
        edge.is_sleeping(),
        "the stationary contact must leave the active pair set"
    );
    assert!(
        edge.constraint_handles.is_empty(),
        "a sleeping contact must leave no solver handles"
    );
}

/// Island users rely on rest history belonging to each body. A mutant that resets every member
/// when a mixed awake/sleeping island wakes erases the already-awake member's timer.
#[test]
fn waking_a_merged_island_preserves_already_awake_rest_time() {
    let mut app = physics_app();
    app.insert_resource(Gravity::ZERO);
    let sleeper = body(&mut app, Vec3::ZERO);
    until_asleep(&mut app, sleeper);
    let awake = body(&mut app, Vec3::X * 10.0);
    for _ in 0..4 {
        app.update();
    }
    let before = app.world().get::<SleepTimer>(awake).unwrap().0;
    assert!(before > 0.0 && app.world().get::<Sleeping>(awake).is_none());
    app.world_mut()
        .run_system_once(
            move |mut nodes: Query<
                &mut BodyIslandNode,
                Or<(With<Disabled>, Without<Disabled>)>,
            >,
                  mut islands: ResMut<PhysicsIslands>,
                  mut contacts: ResMut<ContactGraph>,
                  mut joints: ResMut<JointGraph>,
                  mut commands: Commands| {
                let id =
                    islands.merge_islands(awake, sleeper, &mut nodes, &mut contacts, &mut joints);
                commands.queue(WakeIslands(vec![id]));
            },
        )
        .unwrap();
    assert!(
        app.world().get::<Sleeping>(sleeper).is_none(),
        "the sleeping participant wakes"
    );
    assert_eq!(
        app.world().get::<SleepTimer>(sleeper).unwrap().0,
        0.0,
        "newly woken body starts its rest interval"
    );
    assert_eq!(
        app.world().get::<SleepTimer>(awake).unwrap().0,
        before,
        "merging cannot erase an already-awake body's rest history"
    );
}

/// Explicit wakes restart rest eligibility even after the Sleeping marker has already gone.
/// Server support-loss observers and physics users rely on this component-removal boundary.
#[test]
fn removing_sleeping_restarts_the_woken_bodys_rest_interval() {
    let mut app = physics_app();
    app.insert_resource(Gravity::ZERO);
    let resting = body(&mut app, Vec3::ZERO);
    until_asleep(&mut app, resting);
    app.world_mut().entity_mut(resting).remove::<Sleeping>();
    app.world_mut().flush();
    assert_eq!(app.world().get::<SleepTimer>(resting).unwrap().0, 0.0);
    app.update();
    assert!(
        app.world().get::<Sleeping>(resting).is_none(),
        "a deliberate wake must not immediately resleep"
    );
}

/// Server physics relies on a woken body's first step taking that step's gravity alone: a resting
/// tank's momentum account closes on the tick drive demand wakes it. A mutant that builds the
/// velocity increments of a sleeping body without clearing them (Avian 0.7 as published) carries a
/// stale increment into that step. Fixture: a free body held asleep for a second under gravity.
#[test]
fn a_woken_body_takes_only_its_first_steps_gravity() {
    let mut app = physics_app();
    let gravity = Vec3::NEG_Y * 9.81;
    app.insert_resource(Gravity(gravity));
    let free = body(&mut app, Vec3::ZERO);
    app.update();
    app.update();
    app.world_mut().entity_mut(free).insert(Sleeping);
    for _ in 0..64 {
        app.update();
    }
    assert!(app.world().get::<Sleeping>(free).is_some());
    let before = app.world().get::<LinearVelocity>(free).unwrap().0;
    app.world_mut().entity_mut(free).remove::<Sleeping>();
    app.update();
    let step = app
        .world()
        .resource::<Time<Fixed>>()
        .timestep()
        .as_secs_f32();
    let gained = app.world().get::<LinearVelocity>(free).unwrap().0 - before;
    // Resolution: f32 summation over the substeps, far below a substep's stale increment.
    assert!(
        (gained - gravity * step).length() <= 1e-4 * (gravity * step).length(),
        "the first step awake gained {gained}, gravity gives {}",
        gravity * step
    );
}

/// Physics users need corrupt island membership refused rather than a partly woken island.
/// Missing required body state must not terminate the linked-list walk silently.
#[test]
#[should_panic(expected = "every linked island body retains BodyIslandNode and SleepTimer")]
fn a_linked_body_without_sleep_timer_is_refused() {
    let mut app = physics_app();
    app.insert_resource(Gravity::ZERO);
    let resting = body(&mut app, Vec3::ZERO);
    until_asleep(&mut app, resting);
    let island = app
        .world()
        .get::<BodyIslandNode>(resting)
        .unwrap()
        .island_id();
    app.world_mut().entity_mut(resting).remove::<SleepTimer>();
    WakeIslands(vec![island]).apply(app.world_mut());
}

/// Solver users need the wake source to survive every merge in one narrow phase. A mutant
/// carrying an island ID from the first contact can lose it when a later sleeping contact
/// merges that island into a larger one, leaving finite-mass bodies asleep during the impact.
#[test]
fn an_impact_wakes_an_island_that_merges_again_before_the_solver() {
    fn no_mixed_island_before_solver(bodies: Query<(&BodyIslandNode, Has<Sleeping>)>) {
        let mut states = std::collections::HashMap::<u32, (bool, bool)>::new();
        for (node, sleeping) in &bodies {
            let state = states.entry(node.island_id().0).or_default();
            if sleeping {
                state.1 = true;
            } else {
                state.0 = true;
            }
        }
        assert!(
            states.values().all(|&(awake, asleep)| !(awake && asleep)),
            "the solver must not meet a partly sleeping island after a real impact"
        );
    }
    let mut app = physics_app();
    app.insert_resource(Gravity::ZERO);
    // Fixture gap lies outside touching tolerance but inside the sum of the AABB growths.
    let gap = app
        .world()
        .resource::<NarrowPhaseConfig>()
        .contact_tolerance
        * 1.5;
    let incoming = body(&mut app, Vec3::X * -(0.4 + gap));
    app.world_mut()
        .entity_mut(incoming)
        .insert(SleepingDisabled);
    let single = body(&mut app, Vec3::ZERO);
    until_asleep(&mut app, single);
    let first_pair = app
        .world()
        .resource::<ContactGraph>()
        .get(incoming, single)
        .expect("the expanded bounds retain the incoming speculative pair");
    assert!(
        !first_pair.0.is_touching(),
        "the gap must keep the islands separate"
    );
    let first_id = first_pair.0.id;
    let group = [0.0, 0.4, 0.8].map(|offset| body(&mut app, Vec3::X * (0.4 + gap + offset)));
    for resting in group {
        until_asleep(&mut app, resting);
    }
    assert_ne!(
        app.world()
            .get::<BodyIslandNode>(single)
            .unwrap()
            .island_id(),
        app.world()
            .get::<BodyIslandNode>(group[0])
            .unwrap()
            .island_id()
    );
    app.world_mut()
        .entity_mut(single)
        .insert(Collider::cuboid(0.4 + 2.0 * gap, 0.4, 0.4));
    app.world_mut()
        .entity_mut(incoming)
        .insert(LinearVelocity(Vec3::X));
    app.add_systems(
        PhysicsSchedule,
        no_mixed_island_before_solver
            .after(PhysicsStepSystems::NarrowPhase)
            .before(PhysicsStepSystems::Solver),
    );
    app.update();
    let contacts = app.world().resource::<ContactGraph>();
    for (a, b) in [(incoming, single), (single, group[0])] {
        assert!(
            contacts
                .get(a, b)
                .is_some_and(|(edge, _)| edge.is_touching()),
            "both contacts must reach the wake boundary"
        );
    }
    assert!(
        first_id.0 < contacts.get(single, group[0]).unwrap().0.id.0,
        "the fixture must process the impact before the later sleeping merge"
    );
}

/// Server physics relies on the configured contact tolerance retaining quiet contacts. A mutant
/// that floors only the geometry query, then prunes against velocity alone, loses the inner pair.
#[test]
fn stationary_contacts_keep_the_configured_minimum_speculative_margin() {
    let mut app = physics_app();
    app.insert_resource(Gravity::ZERO);
    let tolerance = app
        .world()
        .resource::<avian3d::collision::narrow_phase::NarrowPhaseConfig>()
        .contact_tolerance
        * app.world().resource::<PhysicsLengthUnit>().0;
    // Fixture: a level support and identical cubes, one inside and one outside the contact band.
    let support = app
        .world_mut()
        .spawn((
            RigidBody::Static,
            Position(Vec3::NEG_Y * 0.5),
            Collider::cuboid(20.0, 1.0, 20.0),
        ))
        .id();
    let inside = body(&mut app, Vec3::new(0.0, 0.2 + tolerance / 2.0, 0.0));
    let outside = body(&mut app, Vec3::new(4.0, 0.2 + 2.0 * tolerance, 0.0));
    app.update();
    app.update();
    let contacts = app.world().resource::<ContactGraph>();
    assert!(
        contacts
            .get(support, inside)
            .is_some_and(|(edge, _)| edge.is_touching()),
        "zero relative velocity must retain a contact inside the configured tolerance"
    );
    assert!(
        !contacts
            .get(support, outside)
            .is_some_and(|(edge, _)| edge.is_touching()),
        "zero relative velocity must not extend the contact band"
    );
}

/// Scenery and vehicles share one rest law: the same angular speed can be negligible roll
/// energy or visible transverse motion. Energy extends the engine's original velocity test.
#[test]
fn rest_depends_on_energy_per_mass_and_spin_axis() {
    let mut app = physics_app();
    app.insert_resource(Gravity::ZERO);
    // Fixture: a slender bar and a small cube; rotation is about each body's principal axes.
    let bar = Vec3::new(6.6, 0.2, 0.05);
    let chunk = Vec3::splat(0.2);
    let rest = SleepThreshold::default().linear;
    let mut bodies = Vec::new();
    for (size, spin, sleeps) in [(bar, Vec3::X * 0.45, true), (bar, Vec3::Z * 0.45, false)] {
        bodies.push((
            app.world_mut()
                .spawn((
                    RigidBody::Dynamic,
                    Position(Vec3::X * bodies.len() as f32 * 20.0),
                    Collider::cuboid(size.x, size.y, size.z),
                    AngularVelocity(spin),
                ))
                .id(),
            sleeps,
        ));
    }
    for fraction in [0.5, 2.0] {
        // Fixture: either side of the resource-owned rest energy.
        for size in [chunk, bar] {
            // DERIVED: I_z/m = (x²+y²)/12 for a uniform box; the factor 1/2 cancels from E/m.
            let spin = rest * fraction / ((size.x * size.x + size.y * size.y) / 12.0).sqrt();
            bodies.push((
                app.world_mut()
                    .spawn((
                        RigidBody::Dynamic,
                        Position(Vec3::X * bodies.len() as f32 * 20.0),
                        Collider::cuboid(size.x, size.y, size.z),
                        AngularVelocity(Vec3::Z * spin),
                    ))
                    .id(),
                fraction < 1.0,
            ));
        }
    }
    app.update();
    let dt = app
        .world()
        .resource::<Time<Fixed>>()
        .timestep()
        .as_secs_f32();
    let ticks = (app.world().resource::<TimeToSleep>().0 / dt).ceil() as usize + 2;
    for _ in 0..ticks {
        app.update();
    }
    for (body, sleeps) in bodies {
        assert_eq!(
            app.world().get::<Sleeping>(body).is_some(),
            sleeps,
            "body {body}: rest must measure energy per mass, including the spin axis"
        );
    }
}

/// Rest uses physical world-frame inertia; the local principal-frame calculation is independent.
/// Degenerate mass properties use Avian's reciprocal convention without NaNs.
#[test]
fn rest_energy_is_frame_invariant_and_handles_degenerate_mass() {
    use avian3d::dynamics::solver::solver_body::SolverBody;
    // Fixture: anisotropic inertia, a generic rotation and an off-axis world spin.
    let body = SolverBody {
        linear_velocity: Vec3::X,
        angular_velocity: Vec3::new(0.7, -1.3, 2.1),
        ..default()
    };
    let mass = ComputedMass::new(12.0);
    let principal = Vec3::new(2.0, 6.0, 12.0);
    let inertia = ComputedAngularInertia::new(principal);
    let rotation = Rotation(Quat::from_euler(EulerRot::XYZ, 0.37, -0.61, 0.83));
    // DERIVED independently in the local principal frame: sum(I_i * omega_i²)/m + v².
    let local_spin = rotation.0.inverse() * body.angular_velocity;
    let expected = 1.0
        + (principal.x * local_spin.x.powi(2)
            + principal.y * local_spin.y.powi(2)
            + principal.z * local_spin.z.powi(2))
            / 12.0;
    assert!((body.kinetic_speed_squared(mass, inertia, rotation) - expected).abs() < 1e-5);
    for mass in [ComputedMass::new(0.0), ComputedMass::INFINITY] {
        assert_eq!(body.kinetic_speed_squared(mass, inertia, rotation), 1.0);
    }
    assert_eq!(
        body.kinetic_speed_squared(mass, ComputedAngularInertia::INFINITY, rotation),
        1.0
    );
}

/// Server rest eligibility retains every state accepted by the original velocity test,
/// even when combined translation/rotation exceeds the energy limit.
#[test]
fn the_original_velocity_test_still_allows_rest() {
    let mut app = physics_app();
    app.insert_resource(Gravity::ZERO);
    let threshold = SleepThreshold::default();
    // Fixture: large radius of gyration and both velocities just below their original limits.
    let resting = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(20.0, 0.2, 0.2),
            LinearVelocity(Vec3::X * threshold.linear * 0.9),
            AngularVelocity(Vec3::Z * threshold.angular * 0.9),
        ))
        .id();
    app.update();
    let dt = app
        .world()
        .resource::<Time<Fixed>>()
        .timestep()
        .as_secs_f32();
    let ticks = (app.world().resource::<TimeToSleep>().0 / dt).ceil() as usize;
    // Allow one step to expose the first fixed tick and one to commit sleeping.
    for _ in 0..ticks + 2 {
        app.update();
    }
    assert!(app.world().get::<Sleeping>(resting).is_some());
}

/// The engine's angular limit must retain the resting see-saw case for large scenery pieces;
/// energy alone would reject this transverse motion despite both original limits passing.
#[test]
fn a_long_bar_see_sawing_below_the_angular_limit_still_rests() {
    let mut app = physics_app();
    app.insert_resource(Gravity::ZERO);
    // Fixture: uniform long bar; transverse spin is below the resource-owned original limit.
    let bar = app
        .world_mut()
        .spawn((RigidBody::Dynamic, Collider::cuboid(6.6, 0.2, 0.05)))
        .id();
    app.update();
    let dt = app
        .world()
        .resource::<Time<Fixed>>()
        .timestep()
        .as_secs_f32();
    let ticks = (app.world().resource::<TimeToSleep>().0 / dt).ceil() as usize;
    let spin = SleepThreshold::default().angular * 0.9;
    for tick in 0..ticks + 2 {
        if app.world().get::<Sleeping>(bar).is_some() {
            break;
        }
        app.world_mut().get_mut::<AngularVelocity>(bar).unwrap().0 =
            Vec3::Z * if tick.is_multiple_of(2) { spin } else { -spin };
        app.update();
    }
    assert!(app.world().get::<Sleeping>(bar).is_some());
}

/// A supported scenery bar must accumulate rest through low-energy long-axis roll pulses.
/// The original angular limit resets the timer at every pulse, before TimeToSleep.
#[test]
fn a_supported_bar_rests_through_periodic_one_tick_roll_pulses() {
    use avian3d::dynamics::solver::solver_body::SolverBody;
    #[derive(Component)]
    struct PulsedBar;
    fn pulse(
        mut bars: Query<(&mut SolverBody, &mut AngularVelocity, &Rotation), With<PulsedBar>>,
        mut tick: Local<usize>,
    ) {
        // Fixture: one-tick roll at 0.4 rad/s every ten ticks (MEASURED pile blocker scale).
        for (mut body, mut spin, rotation) in &mut bars {
            let velocity = if tick.is_multiple_of(10) { 0.4 } else { 0.0 };
            body.angular_velocity = rotation.0 * Vec3::X * velocity;
            spin.0 = body.angular_velocity;
        }
        *tick += 1;
    }
    let mut app = physics_app();
    app.insert_resource(Gravity(Vec3::NEG_Y * crate::physical_laws::gravity_mps2()));
    // Fixture: level support, initially supported bar, no collapse or launch trajectory.
    app.world_mut().spawn((
        RigidBody::Static,
        Position(Vec3::NEG_Y * 0.5),
        Collider::cuboid(20.0, 1.0, 20.0),
    ));
    let bar = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            PulsedBar,
            Position(Vec3::Y * 0.1),
            Collider::cuboid(6.6, 0.2, 0.05),
        ))
        .id();
    app.add_systems(
        PhysicsSchedule,
        pulse
            .after(PhysicsStepSystems::Solver)
            .before(PhysicsStepSystems::Sleeping),
    );
    app.update();
    let dt = app
        .world()
        .resource::<Time<Fixed>>()
        .timestep()
        .as_secs_f32();
    let ticks = (app.world().resource::<TimeToSleep>().0 / dt).ceil() as usize;
    // DERIVED allowance: one initial fixed tick and one deferred sleeping commit.
    for _ in 0..ticks + 2 {
        if app.world().get::<Sleeping>(bar).is_some() {
            break;
        }
        app.update();
    }
    assert!(
        app.world().get::<Sleeping>(bar).is_some(),
        "low-energy roll pulses must not reset the supported bar's rest timer"
    );
}
