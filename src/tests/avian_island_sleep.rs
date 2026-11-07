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
