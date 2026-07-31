//! The receiving half of combat at POSITIVE lead, end to end on the PRODUCTION registration —
//! which since REV 25 registers `HullShock` with a permanently inert rollback condition.
//!
//! Until REV 25 this file was the positive control proving the OPPOSITE premise: that the native
//! comparator forces the rollback by itself at positive lead. The 5-seed capture
//! (`.agents/docs/design/hullshock-delivery-capture-2026-07-31.md`) showed that trigger's only
//! production effect was defeating the ordering rule — every belt-first shove landed 1–3 ticks
//! before its spark — so ownership moved to `net::adoption` at every lead, and this file now
//! proves the three facts that define the new world at the lead regime the old one owned:
//!
//! - NATIVE NEGATIVE CONTROL: a disagreeing shock counter, observed and staged by adoption with
//!   no spark drawn, no longer rolls anything back on its arrival frame. The mismatch is proven
//!   real (the fact stages) and the dispatch prerequisites are proven live (positive lead,
//!   `ConfirmHistory` anchored away so the unchanged-entity scan dispatches the registered
//!   condition) — the same frame delivered before REV 25.
//! - POSITIVE ADOPTION CONTROL, the belt-start sequence: the shock arrives BEFORE its spark, is
//!   staged and HELD (live hull untouched — the exact hold the old trigger defeated), the real
//!   `Impact` draws, and the next frame adoption requests, lightyear restores, and the
//!   sub-threshold shove becomes the LIVE hull velocity with one sharp correction and
//!   `released_on_impact` on the tally.
//! - STRUCTURAL CARVE-OUT: a `(Some, None)` presence mismatch still rolls back natively — that
//!   route never consults the comparator (`lightyear_prediction` registry.rs:254-290) and is
//!   framework recovery, not an application-selected delivery path. No production lifecycle
//!   reaches it (the component rides every spawn bundle and respawn replaces the entity), but the
//!   claim "adoption is the sole intentional present-value trigger" is only honest with this
//!   exception pinned.
//!
//! The REV-22 origin control also survives below: with the counters AGREEING, nothing at all is
//! delivered — a hit's Δv is an order of magnitude under `ROLLBACK_VELOCITY`, so without the
//! shock fact the shove dies in confirmed history. That control is still the whole justification
//! for the component's existence.

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

use super::adoption::{AuthorityAdoption, ImpactPresentation, OrderingTally, SharpCorrection};
use crate::ballistics::{
    AuthorityImpact, HullShock, HullShockLedger, Impact, ImpactSurface, ShockCause,
};
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

const VICTIM: crate::CombatantId = crate::CombatantId(1);

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

/// How much of a deliverable hull the fixture builds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hull {
    /// Velocity histories only, NO confirmed/predicted pose histories: `net::adoption`'s readiness
    /// predicate fails closed on the missing pose, so the fact stages and then WAITS forever.
    /// This is the arm that isolates the registered condition — before REV 25 it delivered.
    Unready,
    /// All four confirmed/predicted history pairs, the shape every replicated tank actually has.
    /// Adoption's readiness passes and the only thing holding the fact is its spark.
    Deliverable,
}

/// The client's `PredictionHistory<HullShock>` shape.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ShockHistory {
    /// A predicted sample exists at the producing tick — the `(Some, Some)` compare the (inert)
    /// registered condition governs.
    Predicted,
    /// An explicit REMOVED marker at the producing tick: the `(Some, None)` presence mismatch,
    /// which lightyear answers with a rollback WITHOUT consulting the registered condition. It
    /// must be an explicit removal — an EMPTY prediction history is "no retained state" and the
    /// completed-tick scan SKIPS the check entirely rather than reading it as absence
    /// (`lightyear_prediction` registry.rs:359-377).
    Removed,
}

/// What happens after the arrival frame.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase2 {
    /// Stop after the arrival frame: the negative-control window, exactly where the pre-REV-25
    /// native trigger delivered in one `PreUpdate`.
    None,
    /// Draw the episode's own hit and run a second frame — the belt-start sequence.
    Spark,
    /// Never draw anything and spend the whole ordering budget: the missing-visual arm. The
    /// budget release must still deliver, sharply, through adoption's own request.
    SpendBudget,
}

/// What the run produced: the hull state captured at the first replayed tick, the live hull state
/// after the schedule, adoption's ledgers, and the sharp-correction occurrences.
#[derive(Resource, Default)]
struct Delivered {
    restored_linear: Option<Vec3>,
    restored_angular: Option<Vec3>,
    live_linear: Vec3,
    live_angular: Vec3,
    realized_count: u32,
    replayed_ticks: Vec<u32>,
    staged_after_first_frame: bool,
    held_linear: Vec3,
    ordering: OrderingTally,
    sharp: Vec<Entity>,
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

fn collect_sharp_corrections(
    mut corrections: MessageReader<SharpCorrection>,
    mut delivered: ResMut<Delivered>,
) {
    for correction in corrections.read() {
        delivered.sharp.push(correction.entity);
    }
}

/// One run on the production protocol registration: build a real lightyear client, deposit the
/// authority's end-of-episode sample at `PRODUCING_TICK`, advance to `PRESENT_TICK`, run the real
/// `PreUpdate`. For a `Deliverable` hull, then draw the real spark and run a second frame — the
/// belt-start sequence. Nothing here forces a rollback by hand.
fn run_arrival(
    prediction: Prediction,
    hull_shape: Hull,
    shock_history: ShockHistory,
    phase2: Phase2,
) -> Delivered {
    let mut app = crate::net::test_harness::base_app();
    app.add_plugins(ClientPlugins {
        tick_duration: crate::net::test_harness::TICK,
    });
    crate::state::sim_plugin(&mut app);
    super::protocol::plugin(&mut app);
    app.insert_state(crate::state::AppState::Playing);
    app.init_resource::<Delivered>();
    app.add_systems(FixedPreUpdate, observe_replay_start);
    app.add_systems(
        PreUpdate,
        collect_sharp_corrections.after(lightyear::prelude::RollbackSystems::EndRollback),
    );
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
    match shock_history {
        ShockHistory::Predicted => {
            predicted_shock_history.add_predicted(PRODUCING_TICK, Some(predicted_shock));
        }
        ShockHistory::Removed => {
            predicted_shock_history.add_predicted(PRODUCING_TICK, None);
        }
    }

    // The hull sample the rollback is supposed to deliver, deposited for EVERY arm: the negative
    // controls prove it stays undelivered.
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
            // false there and `check_rollback` must dispatch the registered condition itself —
            // the dispatch route the inert registration exists to disarm.
            ConfirmHistory::new(RepliconTick::new(1)),
            Tank,
            Position::default(),
            Rotation::default(),
            predicted_shock,
            predicted_shock_history,
            confirmed_shock,
            VICTIM,
        ))
        .id();
    app.world_mut()
        .entity_mut(root)
        .insert((HullShockLedger::default(), ledger_history));
    if hull_shape == Hull::Deliverable {
        // The pose history pairs `net::adoption`'s readiness predicate requires — a replicated,
        // predicted tank has all four, and readiness fails CLOSED on any missing one because that
        // is the case where `prepare_rollback` restores from the client's own prediction instead.
        let mut confirmed_position = ConfirmedHistory::<Position>::default();
        confirmed_position.insert_present_explicit(PRODUCING_TICK, Position::default());
        let mut predicted_position = PredictionHistory::<Position>::default();
        predicted_position.add_predicted(PRODUCING_TICK, Some(Position::default()));
        let mut confirmed_rotation = ConfirmedHistory::<Rotation>::default();
        confirmed_rotation.insert_present_explicit(PRODUCING_TICK, Rotation::default());
        let mut predicted_rotation = PredictionHistory::<Rotation>::default();
        predicted_rotation.add_predicted(PRODUCING_TICK, Some(Rotation::default()));
        app.world_mut().entity_mut(root).insert((
            confirmed_position,
            predicted_position,
            confirmed_rotation,
            predicted_rotation,
        ));
    }
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

    advance(&mut app, PRESENT_TICK.0 as i32);

    assert_eq!(
        app.world().get::<LinearVelocity>(root).unwrap().0,
        LIVE_LINEAR,
        "confirmed-history arrival alone must not write live simulation state",
    );

    // Frame one: the arrival frame. No spark has been drawn — the belt-start shape — so whatever
    // this frame does, it does without one.
    app.world_mut().run_schedule(PreUpdate);
    let staged_after_first_frame = app.world().resource::<AuthorityAdoption>().is_staged();
    let held_linear = app.world().get::<LinearVelocity>(root).unwrap().0;

    match phase2 {
        Phase2::None => {}
        Phase2::Spark => {
            // Frame two: the march presents the episode's own hit — the real trigger
            // `vfx::impact` renders from and `net::adoption` records its ledger from — and the
            // ordering rule releases the held fact.
            advance(&mut app, 2);
            app.world_mut().trigger(Impact {
                position: Vec3::ZERO,
                normal: Vec3::Z,
                caliber: 0.088,
                surface: ImpactSurface::Armor,
                penetrated: true,
                deflection: None,
                authority: Some(AuthorityImpact {
                    tick: PRODUCING_TICK.0,
                    victim: Some(VICTIM),
                }),
            });
            app.world_mut().flush();
            app.world_mut().run_schedule(PreUpdate);
        }
        Phase2::SpendBudget => {
            // No spark, ever. One tick past the budget the ordering rule stops holding and the
            // same frame requests, restores, and replays — the shove lands anyway, because a
            // shove that never arrives is the one failure this seam is not allowed to have.
            advance(
                &mut app,
                i32::try_from(crate::ballistics::RICOCHET_HOLD_TICKS).unwrap() + 1,
            );
            app.world_mut().run_schedule(PreUpdate);
        }
    }

    let live_linear = app.world().get::<LinearVelocity>(root).unwrap().0;
    let live_angular = app.world().get::<AngularVelocity>(root).unwrap().0;
    let realized_count = app.world().get::<HullShockLedger>(root).unwrap().applied();
    let ordering = app.world().resource::<ImpactPresentation>().tally();
    let mut delivered = app
        .world_mut()
        .remove_resource::<Delivered>()
        .expect("the evidence resource outlives the schedule");
    delivered.live_linear = live_linear;
    delivered.live_angular = live_angular;
    delivered.realized_count = realized_count;
    delivered.staged_after_first_frame = staged_after_first_frame;
    delivered.held_linear = held_linear;
    delivered.ordering = ordering;
    delivered
}

/// Walk both the tick counter and `Time<Fixed>` forward, keeping them consistent — the rollback
/// path reads the timeline, the replay loop reads the fixed clock.
fn advance(app: &mut App, ticks: i32) {
    app.world_mut()
        .resource_mut::<LocalTimeline>()
        .apply_delta(ticks);
    for _ in 0..ticks {
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(crate::net::test_harness::TICK);
    }
}

/// NATIVE NEGATIVE CONTROL. The mismatch is real — the fact STAGES, which is adoption observing
/// the same disagreement the registered condition is handed — and every dispatch prerequisite of
/// the pre-REV-25 trigger is live: positive lead, a completed checkpoint, `ConfirmHistory`
/// anchored away from it so the unchanged-entity scan dispatches the registered condition on the
/// `(Some, Some)` pair. Before REV 25 this exact arrival frame delivered the shove in ONE
/// `PreUpdate` (this file's own git history is the proof). Now the condition is inert and no
/// spark has been drawn, so the frame may deliver NOTHING: no replayed tick, no restored sample,
/// the live hull untouched, the fact staged and held.
#[test]
fn a_disagreeing_shock_no_longer_rolls_back_on_the_native_comparator() {
    let delivered = run_arrival(
        Prediction::Disagrees,
        Hull::Deliverable,
        ShockHistory::Predicted,
        Phase2::None,
    );

    assert!(
        delivered.staged_after_first_frame,
        "the fact must STAGE — an unobserved mismatch would make this control vacuous: the run \
         has to prove the disagreement was seen and still triggered nothing",
    );
    assert!(
        delivered.replayed_ticks.is_empty(),
        "the inert condition must not order a rollback, got replayed ticks {:?}",
        delivered.replayed_ticks,
    );
    assert_eq!(
        delivered.restored_linear, None,
        "no restore may happen on the sparkless arrival frame now that the native trigger is inert",
    );
    assert_eq!(
        delivered.live_linear, LIVE_LINEAR,
        "the live hull must be untouched — before REV 25 this exact frame delivered the authority \
         velocity here, which is the ownership this control pins as moved",
    );
}

/// THE MISSING-VISUAL ARM, on the production inert registration: the spark never draws — the
/// cosmetic carrier rides a loss-bounded unordered channel and CAN vanish — and one tick past the
/// ordering budget the shove lands anyway, through adoption's own request, still sharp. The
/// player gets an unexplained nudge either way; what this pins is that the nudge cannot be LOST,
/// and that it stays an adoption (`released_on_budget`), not a silent fall-through.
#[test]
fn a_missing_spark_spends_the_budget_and_still_delivers_sharply() {
    let delivered = run_arrival(
        Prediction::Disagrees,
        Hull::Deliverable,
        ShockHistory::Predicted,
        Phase2::SpendBudget,
    );

    assert_eq!(
        delivered.live_linear, AUTHORITY_LINEAR,
        "a shove whose spark never draws must still land — losing it is the one failure this \
         seam is not allowed to have",
    );
    assert_eq!(delivered.live_angular, AUTHORITY_ANGULAR);
    assert_eq!(
        delivered.ordering,
        OrderingTally {
            released_on_budget: 1,
            max_wait_ticks: i32::try_from(crate::ballistics::RICOCHET_HOLD_TICKS).unwrap() + 1,
            ..default()
        },
        "the release must be ON BUDGET, once, with the wait recorded",
    );
    assert_eq!(
        delivered.sharp.len(),
        1,
        "a budget release is still an authoritative event and must stay sharp, got {:?}",
        delivered.sharp,
    );
}

/// POSITIVE ADOPTION CONTROL — the belt-start sequence at the lead regime the native comparator
/// used to own. The shock arrives with NO spark drawn; the arrival frame stages and HOLDS it (the
/// live hull keeps its own velocity — the hold the old trigger defeated on every belt-first round
/// of the capture); the episode's own hit then draws, and the next frame adoption requests the
/// producing tick, lightyear restores it, and the sub-threshold shove is the LIVE hull velocity,
/// released on impact with exactly one sharp correction naming this hull.
#[test]
fn a_belt_first_shock_is_held_for_its_spark_and_adopted_when_it_draws() {
    let delivered = run_arrival(
        Prediction::Disagrees,
        Hull::Deliverable,
        ShockHistory::Predicted,
        Phase2::Spark,
    );

    assert!(
        delivered.staged_after_first_frame,
        "the arrival frame must stage the fact",
    );
    assert_eq!(
        delivered.held_linear, LIVE_LINEAR,
        "the arrival frame must HOLD: no spark has been drawn, so the shove may not be live yet — \
         this is the ordering the inert comparator exists to stop defeating",
    );
    assert_eq!(
        delivered.restored_linear,
        Some(AUTHORITY_LINEAR),
        "the adoption's rollback must restore the producing tick's hull velocity exactly",
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
    assert_eq!(
        delivered.ordering,
        OrderingTally {
            released_on_impact: 1,
            // The two ticks between arrival and the spark's frame — the hold is SUPPOSED to cost
            // exactly the spark's lateness, and the capture measured 1–3 ticks in play.
            max_wait_ticks: 2,
            ..default()
        },
        "the fact must release ON IMPACT — its own spark was drawn within the budget — with no \
         bypass, no budget release, and nothing undelivered",
    );
    assert_eq!(
        delivered.sharp.len(),
        1,
        "an adopted external event must emit exactly one sharp correction, got {:?}",
        delivered.sharp,
    );
}

/// THE STRUCTURAL CARVE-OUT, pinned. A confirmed `HullShock` with NO predicted sample at the
/// producing tick is a `(Some, None)` presence mismatch, and lightyear answers those with a
/// rollback WITHOUT consulting the registered condition — inert or not. No production lifecycle
/// reaches this shape (the component rides every spawn bundle and respawn replaces the entity, so
/// init/seed never records a mismatch), but the ADR's claim is "sole INTENTIONAL present-value
/// trigger", and this is the exception that keeps the word honest. If a lightyear bump makes this
/// stop rolling back, the carve-out has silently widened into a delivery gap and both the ADR and
/// the registration commentary need re-deriving.
#[test]
fn a_presence_mismatch_still_rolls_back_without_the_comparator() {
    let delivered = run_arrival(
        Prediction::Disagrees,
        Hull::Unready,
        ShockHistory::Removed,
        Phase2::None,
    );

    assert!(
        !delivered.replayed_ticks.is_empty(),
        "a (Some, None) presence mismatch must still order a native rollback — the comparator is \
         inert, but presence recovery is lightyear's own and was never ours to disarm",
    );
    assert_eq!(
        delivered.live_linear, AUTHORITY_LINEAR,
        "the presence-mismatch rollback restores confirmed state wholesale, hull velocity included",
    );
}

/// NEGATIVE CONTROL from REV 22's origin story, still true and still the component's whole
/// justification: with the counters AGREEING, the only remaining disagreement is the hull velocity
/// itself, a hit's Δv is an order of magnitude under `ROLLBACK_VELOCITY`, and the shove the server
/// computed is discarded in confirmed history. Nothing is delivered — which is why `HullShock`
/// exists at all.
#[test]
fn an_agreeing_shock_leaves_the_sub_threshold_shove_discarded() {
    assert!(
        AUTHORITY_LINEAR.length() < super::protocol::ROLLBACK_VELOCITY,
        "this control only means anything while a hit's Δv ({} m/s) is under the velocity gate \
         ({} m/s) — if the gate ever drops below it, re-derive what HullShock is for",
        AUTHORITY_LINEAR.length(),
        super::protocol::ROLLBACK_VELOCITY,
    );

    let delivered = run_arrival(
        Prediction::Agrees,
        Hull::Unready,
        ShockHistory::Predicted,
        Phase2::None,
    );

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
