//! CONFIRMATION RUN for the receiving half of combat: does the *arrival* of an owner-predicted
//! component the client can never predict force the rollback that delivers the server's shove?
//!
//! The existing storm-killer test in `protocol.rs` proves the second half — that a rollback
//! restores the producing tick and replay re-derives identical state — but it reaches that
//! rollback through `StateRollbackMetadata::request_forced_rollback`. That leaves the first half
//! unproven: nothing in-tree showed that a replicated `.predict()` component whose value the
//! client did not predict *causes* the rollback on its own.
//!
//! That is the entire mechanism behind the planned hit-shock component, so it is pinned here
//! rather than assumed. Both production routes into `check_rollback` are exercised:
//!
//! 1. the receive-time route, where `write_history` runs the registered comparator as the message
//!    is deserialized and calls `StateRollbackMetadata::record_mismatch`, which `check_rollback`
//!    then consumes at the completed mutate tick; and
//! 2. the unchanged-entity scan, where `check_rollback` dispatches the registered comparator
//!    itself against `ConfirmedHistory` and `PredictionHistory`.
//!
//! A negative control holds the fixture fixed and makes only the prediction agree, so a passing
//! run cannot be explained by the harness rolling back unconditionally.
//!
//! `WeaponGate` is the probe because it already carries the exact (bit-for-bit) comparator the
//! shock component will copy; the mechanism under test is the registration shape, not the payload.

use avian3d::prelude::{
    AngularInertia, AngularVelocity, CenterOfMass, GravityScale, LinearVelocity, Mass,
    NoAutoAngularInertia, NoAutoCenterOfMass, NoAutoMass, Position, RigidBody, Rotation,
};
use bevy::prelude::*;
use bevy_replicon::client::confirm_history::ConfirmHistory;
use bevy_replicon::prelude::RepliconTick;
use lightyear::core::confirmed_history::ConfirmedHistory;
use lightyear::prelude::client::{Client, ClientPlugins, Connected};
use lightyear::prelude::{
    InputTimeline, IsSynced, LocalTimeline, PeerId, Predicted, PredictionHistory, RemoteId,
    ReplicationCheckpointMap, StateRollbackMetadata, Tick,
};

use crate::tank::{Tank, WeaponGate, WeaponGateState};

/// The tick the authority produced the shove on, and the tick the client has already predicted
/// past by the time the message lands. The gap is the replay window.
const PRODUCING_TICK: Tick = Tick(100);
const PRESENT_TICK: Tick = Tick(108);
/// Replicon's view of `PRODUCING_TICK`. Deliberately not the tick the fixture's `ConfirmHistory`
/// is anchored at, so the entity reads as "not explicitly confirmed here" and route 2 is taken.
fn producing_replicon_tick() -> RepliconTick {
    RepliconTick::new(50)
}

const HULL_MASS: f32 = 100.0;
const HULL_INERTIA: f32 = 50.0;

/// The hull velocity the authority recorded at `PRODUCING_TICK` — an 88 mm hit's Δv, the number
/// the player is owed. The client's own prediction never contains it.
const AUTHORITY_LINEAR: Vec3 = Vec3::new(0.0, 0.0, -0.138_3);
const AUTHORITY_ANGULAR: Vec3 = Vec3::new(0.191_0, 0.0, 0.052_0);
/// What the client predicted instead: an untouched hull.
const PREDICTED_LINEAR: Vec3 = Vec3::ZERO;
const PREDICTED_ANGULAR: Vec3 = Vec3::ZERO;

/// What the live hull is doing when the message lands — neither value, so a passing assertion
/// cannot be satisfied by the fixture simply never having been written.
const LIVE_LINEAR: Vec3 = Vec3::new(3.0, 0.0, 3.0);
const LIVE_ANGULAR: Vec3 = Vec3::new(3.0, 3.0, 0.0);

/// The gate value the authority produced, and the one the client predicted. The client cannot
/// predict a hit, so these differ exactly as the shock component's counter would.
fn authority_gate() -> WeaponGate {
    WeaponGate {
        weapons: vec![WeaponGateState {
            ready_tick: Some(102),
            paused_at_tick: None,
            belt_remaining: 2,
        }],
    }
}

fn client_predicted_gate() -> WeaponGate {
    WeaponGate {
        weapons: vec![WeaponGateState {
            ready_tick: Some(105),
            paused_at_tick: None,
            belt_remaining: 1,
        }],
    }
}

/// The live hull state captured at the first replayed tick — after `prepare_rollback` has restored
/// the producing tick and before any replayed physics has touched it.
#[derive(Resource, Default)]
struct RestoredAtReplayStart {
    linear: Option<Vec3>,
    angular: Option<Vec3>,
    gate: Option<WeaponGate>,
    replayed_ticks: Vec<u32>,
}

fn observe_replay_start(
    timeline: Res<LocalTimeline>,
    hull: Query<(&LinearVelocity, &AngularVelocity, &WeaponGate), With<Predicted>>,
    mut restored: ResMut<RestoredAtReplayStart>,
) {
    let tick = timeline.tick();
    restored.replayed_ticks.push(tick.0);
    if tick != PRODUCING_TICK + 1 || restored.linear.is_some() {
        return;
    }
    let Ok((linear, angular, gate)) = hull.single() else {
        return;
    };
    restored.linear = Some(linear.0);
    restored.angular = Some(angular.0);
    restored.gate = Some(gate.clone());
}

/// Whether the client's prediction agrees with the authority. `Disagrees` is the shock component's
/// situation: the client had no way to know.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Prediction {
    Agrees,
    Disagrees,
}

/// How the mismatch reaches `check_rollback`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Route {
    /// Route 1: the comparator ran at deserialize time and `write_history` recorded the mismatch.
    RecordedOnReceive,
    /// Route 2: nothing was recorded; `check_rollback` must dispatch the comparator itself.
    ComparatorDispatch,
}

/// Build a real lightyear client, deposit an authoritative sample at `PRODUCING_TICK` that the
/// client's prediction does or does not match, advance to `PRESENT_TICK`, and run the real
/// `PreUpdate` rollback schedule. Nothing here forces a rollback.
fn run_arrival(prediction: Prediction, route: Route) -> RestoredAtReplayStart {
    let mut app = crate::net::test_harness::base_app();
    app.add_plugins(ClientPlugins {
        tick_duration: crate::net::test_harness::TICK,
    });
    crate::state::sim_plugin(&mut app);
    super::protocol::plugin(&mut app);
    app.insert_state(crate::state::AppState::Playing);
    app.init_resource::<RestoredAtReplayStart>();
    app.add_systems(FixedPreUpdate, observe_replay_start);
    crate::net::test_harness::finish(&mut app);

    app.world_mut().spawn((
        Client::default(),
        RemoteId(PeerId::Server),
        Connected,
        crate::net::test_harness::prediction_manager(),
        IsSynced::<InputTimeline>::default(),
    ));

    let predicted_gate_value = match prediction {
        Prediction::Agrees => authority_gate(),
        Prediction::Disagrees => client_predicted_gate(),
    };
    let mut confirmed_gate = ConfirmedHistory::<WeaponGate>::default();
    confirmed_gate.insert_present_explicit(PRODUCING_TICK, authority_gate());
    let mut predicted_gate = PredictionHistory::<WeaponGate>::default();
    predicted_gate.add_predicted(PRODUCING_TICK, Some(predicted_gate_value.clone()));

    // The hull sample the rollback is supposed to deliver. It is deposited for both arms: the
    // negative control proves it stays undelivered when the prediction agrees.
    let mut confirmed_linear = ConfirmedHistory::<LinearVelocity>::default();
    confirmed_linear.insert_present_explicit(PRODUCING_TICK, LinearVelocity(AUTHORITY_LINEAR));
    let mut predicted_linear = PredictionHistory::<LinearVelocity>::default();
    predicted_linear.add_predicted(PRODUCING_TICK, Some(LinearVelocity(PREDICTED_LINEAR)));
    let mut confirmed_angular = ConfirmedHistory::<AngularVelocity>::default();
    confirmed_angular.insert_present_explicit(PRODUCING_TICK, AngularVelocity(AUTHORITY_ANGULAR));
    let mut predicted_angular = PredictionHistory::<AngularVelocity>::default();
    predicted_angular.add_predicted(PRODUCING_TICK, Some(AngularVelocity(PREDICTED_ANGULAR)));

    let root = app
        .world_mut()
        .spawn((
            Predicted,
            // Anchored away from `producing_replicon_tick`, so `ConfirmHistory::contains` is false
            // there and `check_rollback` takes the comparator-dispatch branch.
            ConfirmHistory::new(RepliconTick::new(1)),
            Tank,
            Position::default(),
            Rotation::default(),
            predicted_gate_value,
            predicted_gate,
            confirmed_gate,
            crate::CombatantId(1),
        ))
        .id();
    app.world_mut().entity_mut(root).insert((
        Transform::default(),
        RigidBody::Dynamic,
        Mass(HULL_MASS),
        AngularInertia::new(Vec3::splat(HULL_INERTIA)),
        CenterOfMass(Vec3::ZERO),
        NoAutoMass,
        NoAutoAngularInertia,
        NoAutoCenterOfMass,
        // Gravity off so the restored sample is readable bit-for-bit at the replay boundary.
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
        checkpoints.record(producing_replicon_tick(), PRODUCING_TICK);
        checkpoints.record_last_confirmed_tick(producing_replicon_tick());
    }
    if route == Route::RecordedOnReceive && prediction == Prediction::Disagrees {
        app.world_mut()
            .resource_mut::<StateRollbackMetadata>()
            .record_mismatch(PRODUCING_TICK);
    }

    app.world_mut()
        .resource_mut::<LocalTimeline>()
        .apply_delta(PRESENT_TICK.0 as i32);
    for _ in 0..PRESENT_TICK.0 {
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(crate::net::test_harness::TICK);
    }

    // The live hull must still be untouched here: depositing authority into confirmed history is
    // not itself a write into simulation state.
    assert_eq!(
        app.world().get::<LinearVelocity>(root).unwrap().0,
        LIVE_LINEAR,
        "confirmed-history arrival alone must not write live simulation state",
    );

    app.world_mut().run_schedule(PreUpdate);
    app.world_mut()
        .remove_resource::<RestoredAtReplayStart>()
        .expect("the evidence resource outlives the schedule")
}

/// ROUTE 1 — the production path for a component that arrives in a replication message. The
/// comparator runs inside `write_history`, the mismatch is recorded against the producing tick,
/// and `check_rollback` consumes it with no forced rollback anywhere. The hull velocity the client
/// never predicted arrives bit-for-bit.
#[test]
fn a_receive_time_mismatch_alone_delivers_the_authoritative_hull_velocity() {
    let restored = run_arrival(Prediction::Disagrees, Route::RecordedOnReceive);

    assert_eq!(
        restored.linear,
        Some(AUTHORITY_LINEAR),
        "arrival of an unpredicted authoritative sample must roll back and restore the hull \
         velocity exactly — this is the shove the player is owed",
    );
    assert_eq!(
        restored.angular,
        Some(AUTHORITY_ANGULAR),
        "the angular half of the shove must arrive with the linear half",
    );
    assert_eq!(
        restored.gate,
        Some(authority_gate()),
        "the component that triggered the rollback must itself be restored to authority",
    );
}

/// ROUTE 2 — `check_rollback` dispatches the registered comparator itself. This is the link that
/// was previously only reasoned about: our own `with_rollback_condition` returning true is what
/// causes the rollback, with nothing pre-recorded to consume.
#[test]
fn our_registered_comparator_alone_triggers_the_rollback() {
    let restored = run_arrival(Prediction::Disagrees, Route::ComparatorDispatch);

    assert_eq!(
        restored.linear,
        Some(AUTHORITY_LINEAR),
        "a registered exact comparator returning true must be sufficient to cause the rollback \
         and deliver the authoritative hull velocity",
    );
    assert_eq!(restored.angular, Some(AUTHORITY_ANGULAR));
}

/// NEGATIVE CONTROL, and a faithful reproduction of what the game does today. Same fixture, same
/// deposited authority, same schedule — only the client's prediction now agrees on the gate.
///
/// The hull sample is still deposited and still disagrees, so this is not a fixture that has
/// nothing to deliver: it is the production situation exactly. A hit's Δv is an order of magnitude
/// under `ROLLBACK_VELOCITY`, so the velocity comparator returns false, no rollback is requested,
/// and the shove the server computed stays in confirmed history and is discarded.
///
/// That is why the shock component has to exist. It is not a nudge added on top of a shove that
/// already lands — it is the only thing that makes the shove land at all.
#[test]
fn an_agreeing_prediction_does_not_roll_back_so_a_sub_threshold_shove_is_discarded() {
    assert!(
        AUTHORITY_LINEAR.length() < super::protocol::ROLLBACK_VELOCITY,
        "this control only means anything while a hit's Δv ({} m/s) is under the velocity gate \
         ({} m/s) — if the gate ever drops below it, re-derive what the shock component is for",
        AUTHORITY_LINEAR.length(),
        super::protocol::ROLLBACK_VELOCITY,
    );

    let restored = run_arrival(Prediction::Agrees, Route::ComparatorDispatch);

    assert_eq!(
        restored.linear, None,
        "nothing may be restored when the only disagreement is sub-threshold — a rollback here \
         would mean the mechanism fires on arrival rather than on disagreement",
    );
    assert!(
        restored.replayed_ticks.is_empty(),
        "an agreeing arrival must not replay any tick, got {:?}",
        restored.replayed_ticks,
    );
}
