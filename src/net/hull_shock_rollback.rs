//! The receiving half of combat, end to end on the PRODUCTION registration.
//!
//! `arrival_rollback` proved the mechanism in the abstract — an owner-predicted component the
//! client did not predict forces the rollback by itself, and the rollback delivers a sub-threshold
//! hull velocity bit-for-bit — using `WeaponGate` as a stand-in probe. This runs the same fixture
//! against the component that actually exists for it, `ballistics::HullShock`, and asserts the
//! thing the player is owed: the LIVE hull velocity after the schedule, not merely the value seen
//! at the replay boundary.
//!
//! The negative control is the game as it shipped before REV 22: identical fixture, identical
//! deposited authority, only the shock counter agreeing. Nothing is delivered, because a hit's Δv
//! is an order of magnitude under `ROLLBACK_VELOCITY` and the velocity comparator says "close
//! enough". That control is the whole justification for the component.

use avian3d::prelude::{
    AngularInertia, AngularVelocity, CenterOfMass, GravityScale, LinearVelocity, Mass,
    NoAutoAngularInertia, NoAutoCenterOfMass, NoAutoMass, Position, RigidBody, Rotation,
};
use bevy::prelude::*;
use bevy_replicon::client::confirm_history::ConfirmHistory;
use bevy_replicon::prelude::RepliconTick;
use lightyear::core::confirmed_history::ConfirmedHistory;
use lightyear::prelude::client::{Client, ClientPlugins, Connected, Remote};
use lightyear::prelude::{
    InputTimeline, IsSynced, LocalTimeline, PeerId, Predicted, PredictionHistory, RemoteId,
    ReplicationCheckpointMap, Tick,
};

use crate::ballistics::{HullShock, HullShockLedger, ShockCause};
use crate::tank::Tank;

/// The tick the authority closed the shock episode on, and the tick the client has already
/// predicted past by the time the message lands. The gap is the replay window.
const PRODUCING_TICK: Tick = Tick(100);
const PRESENT_TICK: Tick = Tick(108);

const HULL_MASS: f32 = 100.0;
const HULL_INERTIA: f32 = 50.0;

/// The hull velocity the authority recorded at `PRODUCING_TICK` — an 88 mm hit's measured Δv, the
/// number the player is owed. The client's own prediction never contains it.
const AUTHORITY_LINEAR: Vec3 = Vec3::new(0.0, 0.0, -0.138_3);
const AUTHORITY_ANGULAR: Vec3 = Vec3::new(0.191_0, 0.0, 0.052_0);
/// What the client predicted instead: an untouched hull.
const PREDICTED_LINEAR: Vec3 = Vec3::ZERO;
const PREDICTED_ANGULAR: Vec3 = Vec3::ZERO;
/// What the live hull is doing when the message lands — neither value, so no assertion below can be
/// satisfied by the fixture simply never having been written.
const LIVE_LINEAR: Vec3 = Vec3::new(3.0, 0.0, 3.0);
const LIVE_ANGULAR: Vec3 = Vec3::new(3.0, 3.0, 0.0);

/// The episode the authority closed at `PRODUCING_TICK`, and the counter the client still holds.
fn authority_shock() -> HullShock {
    HullShock {
        count: 1,
        tick: PRODUCING_TICK.0,
        // A hull's first episode has no open window to defer behind, so it closes on the tick it
        // was armed and spans exactly that tick.
        opened: PRODUCING_TICK.0,
        cause: ShockCause::Perforation,
    }
}

fn never_shot() -> HullShock {
    HullShock::default()
}

/// Whether the client's copy of the shock counter agrees with the authority. `Disagrees` is the
/// real situation: the client had no way to know it was shot.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Prediction {
    Agrees,
    Disagrees,
}

/// What the run produced: the hull state captured at the first replayed tick, the live hull state
/// after the schedule, and the owner's realization mark.
#[derive(Resource, Default)]
struct Delivered {
    restored_linear: Option<Vec3>,
    restored_angular: Option<Vec3>,
    live_linear: Vec3,
    live_angular: Vec3,
    realized_count: u32,
    replayed_ticks: Vec<u32>,
}

fn observe_replay_start(
    timeline: Res<LocalTimeline>,
    hull: Query<(&LinearVelocity, &AngularVelocity), With<Predicted>>,
    mut delivered: ResMut<Delivered>,
) {
    let tick = timeline.tick();
    delivered.replayed_ticks.push(tick.0);
    if tick != PRODUCING_TICK + 1 || delivered.restored_linear.is_some() {
        return;
    }
    let Ok((linear, angular)) = hull.single() else {
        return;
    };
    delivered.restored_linear = Some(linear.0);
    delivered.restored_angular = Some(angular.0);
}

/// Build a real lightyear client on the production protocol registration, deposit the authority's
/// end-of-episode sample at `PRODUCING_TICK`, advance to `PRESENT_TICK`, and run the real
/// `PreUpdate` rollback schedule. Nothing here forces a rollback.
fn run_arrival(prediction: Prediction) -> Delivered {
    let mut app = crate::net::test_harness::base_app();
    app.add_plugins(ClientPlugins {
        tick_duration: crate::net::test_harness::TICK,
    });
    crate::state::sim_plugin(&mut app);
    super::protocol::plugin(&mut app);
    app.insert_state(crate::state::AppState::Playing);
    app.init_resource::<Delivered>();
    app.add_systems(FixedPreUpdate, observe_replay_start);
    crate::net::test_harness::finish(&mut app);

    app.world_mut().spawn((
        Client::default(),
        RemoteId(PeerId::Server),
        Connected,
        crate::net::test_harness::prediction_manager(),
        IsSynced::<InputTimeline>::default(),
    ));

    let predicted_shock = match prediction {
        Prediction::Agrees => authority_shock(),
        Prediction::Disagrees => never_shot(),
    };
    let mut confirmed_shock = ConfirmedHistory::<HullShock>::default();
    confirmed_shock.insert_present_explicit(PRODUCING_TICK, authority_shock());
    let mut predicted_shock_history = PredictionHistory::<HullShock>::default();
    predicted_shock_history.add_predicted(PRODUCING_TICK, Some(predicted_shock));

    // The hull sample the rollback is supposed to deliver, deposited for BOTH arms: the negative
    // control proves it stays undelivered when the only disagreement is sub-threshold.
    let mut confirmed_linear = ConfirmedHistory::<LinearVelocity>::default();
    confirmed_linear.insert_present_explicit(PRODUCING_TICK, LinearVelocity(AUTHORITY_LINEAR));
    let mut predicted_linear = PredictionHistory::<LinearVelocity>::default();
    predicted_linear.add_predicted(PRODUCING_TICK, Some(LinearVelocity(PREDICTED_LINEAR)));
    let mut confirmed_angular = ConfirmedHistory::<AngularVelocity>::default();
    confirmed_angular.insert_present_explicit(PRODUCING_TICK, AngularVelocity(AUTHORITY_ANGULAR));
    let mut predicted_angular = PredictionHistory::<AngularVelocity>::default();
    predicted_angular.add_predicted(PRODUCING_TICK, Some(AngularVelocity(PREDICTED_ANGULAR)));

    // The owner's local ledger, rewound with everything else: its pre-shock value is what replay
    // must re-realize the arriving count against.
    let mut ledger_history = PredictionHistory::<HullShockLedger>::default();
    ledger_history.add_predicted(PRODUCING_TICK, Some(HullShockLedger::default()));

    let root = app
        .world_mut()
        .spawn((
            Predicted,
            // Every client-side tank arrived by replication; the owner half of the shock seam is
            // gated on exactly this marker.
            Remote,
            // Anchored away from the producing replicon tick, so `ConfirmHistory::contains` is
            // false there and `check_rollback` must dispatch our registered comparator itself.
            ConfirmHistory::new(RepliconTick::new(1)),
            Tank,
            Position::default(),
            Rotation::default(),
            predicted_shock,
            predicted_shock_history,
            confirmed_shock,
            crate::CombatantId(1),
        ))
        .id();
    app.world_mut()
        .entity_mut(root)
        .insert((HullShockLedger::default(), ledger_history));
    app.world_mut().entity_mut(root).insert((
        Transform::default(),
        RigidBody::Dynamic,
        Mass(HULL_MASS),
        AngularInertia::new(Vec3::splat(HULL_INERTIA)),
        CenterOfMass(Vec3::ZERO),
        NoAutoMass,
        NoAutoAngularInertia,
        NoAutoCenterOfMass,
        // Gravity off and no contacts, so replay neither adds to nor bleeds the delivered shove —
        // the live velocity after the schedule is exactly what the rollback handed the hull.
        GravityScale(0.0),
    ));
    app.world_mut().entity_mut(root).insert((
        LinearVelocity(LIVE_LINEAR),
        predicted_linear,
        confirmed_linear,
        AngularVelocity(LIVE_ANGULAR),
        predicted_angular,
        confirmed_angular,
    ));
    app.world_mut().flush();

    // The completed mutate tick: the authority has certified every replicated component at
    // `PRODUCING_TICK`. This is what receive would have published.
    {
        let mut checkpoints = app.world_mut().resource_mut::<ReplicationCheckpointMap>();
        checkpoints.record(RepliconTick::new(50), PRODUCING_TICK);
        checkpoints.record_last_confirmed_tick(RepliconTick::new(50));
    }

    app.world_mut()
        .resource_mut::<LocalTimeline>()
        .apply_delta(PRESENT_TICK.0 as i32);
    for _ in 0..PRESENT_TICK.0 {
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(crate::net::test_harness::TICK);
    }

    assert_eq!(
        app.world().get::<LinearVelocity>(root).unwrap().0,
        LIVE_LINEAR,
        "confirmed-history arrival alone must not write live simulation state",
    );

    app.world_mut().run_schedule(PreUpdate);

    let live_linear = app.world().get::<LinearVelocity>(root).unwrap().0;
    let live_angular = app.world().get::<AngularVelocity>(root).unwrap().0;
    let realized_count = app.world().get::<HullShockLedger>(root).unwrap().applied();
    let mut delivered = app
        .world_mut()
        .remove_resource::<Delivered>()
        .expect("the evidence resource outlives the schedule");
    delivered.live_linear = live_linear;
    delivered.live_angular = live_angular;
    delivered.realized_count = realized_count;
    delivered
}

/// THE RECEIVING HALF. A bump the client could not predict arrives, the registered exact
/// comparator forces the rollback with nothing pre-recorded to consume, and the authority's hull
/// velocity — an order of magnitude under the velocity gate that would otherwise discard it —
/// becomes the client's LIVE velocity.
#[test]
fn an_authority_shock_bump_reaches_the_clients_live_hull_velocity() {
    let delivered = run_arrival(Prediction::Disagrees);

    assert_eq!(
        delivered.restored_linear,
        Some(AUTHORITY_LINEAR),
        "the rollback must restore the producing tick's hull velocity exactly",
    );
    assert_eq!(delivered.restored_angular, Some(AUTHORITY_ANGULAR));
    assert_eq!(
        delivered.live_linear, AUTHORITY_LINEAR,
        "the shove must survive replay into the LIVE hull velocity — this is what the player feels",
    );
    assert_eq!(delivered.live_angular, AUTHORITY_ANGULAR);
    assert_eq!(
        delivered.realized_count,
        authority_shock().count,
        "the owner's rewindable mark must realize the arriving episode during replay",
    );
}

/// NEGATIVE CONTROL, and a faithful reproduction of the game before REV 22. Same fixture, same
/// deposited authority, same schedule — only the shock counter now agrees, so the only remaining
/// disagreement is the hull velocity itself. Nothing is delivered: a hit's Δv is under
/// `ROLLBACK_VELOCITY`, the comparator returns false, and the shove the server computed is
/// discarded in confirmed history. That is the bug this component exists to fix.
#[test]
fn an_agreeing_shock_leaves_the_sub_threshold_shove_discarded() {
    assert!(
        AUTHORITY_LINEAR.length() < super::protocol::ROLLBACK_VELOCITY,
        "this control only means anything while a hit's Δv ({} m/s) is under the velocity gate \
         ({} m/s) — if the gate ever drops below it, re-derive what HullShock is for",
        AUTHORITY_LINEAR.length(),
        super::protocol::ROLLBACK_VELOCITY,
    );

    let delivered = run_arrival(Prediction::Agrees);

    assert_eq!(
        delivered.restored_linear, None,
        "nothing may be restored when the only disagreement is sub-threshold",
    );
    assert!(
        delivered.replayed_ticks.is_empty(),
        "an agreeing arrival must not replay any tick, got {:?}",
        delivered.replayed_ticks,
    );
    assert_eq!(
        delivered.live_linear, LIVE_LINEAR,
        "without the shock the live hull keeps its own mispredicted velocity",
    );
}
