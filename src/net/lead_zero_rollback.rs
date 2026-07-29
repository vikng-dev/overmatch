//! THE REGIME THE EXISTING FIXTURES CANNOT REACH: a client that is level with the server.
//!
//! # What the older fixtures test
//!
//! `net::arrival_rollback` and `net::hull_shock_rollback` both build the same world: the authority
//! closes an episode at tick 100 and the client is already at tick 108. Eight ticks of lead, and
//! the entity's Replicon `ConfirmHistory` is deliberately anchored at a *different* replicon tick
//! from the completed checkpoint, so `ConfirmHistory::contains` is false there. Both choices were
//! made to isolate the mechanism under test — and both of them route around the two gates that
//! actually decide whether an authoritative shove is delivered on the shipping build.
//!
//! # What the shipping config actually produces
//!
//! lightyear places the client's input timeline at
//!
//! ```text
//! objective = remote + rtt/2 + (jitter · jitter_multiple + tick · jitter_margin) + 1 + error_margin
//!             - input_delay
//! ```
//!
//! (`SyncedTimeline::sync_objective` for `InputTimeline`). On loopback both `rtt` and `jitter` are
//! zero, so every jitter-scaled term vanishes and only the CONSTANTS survive:
//! `jitter_margin` 1.0 + the fixed 1 + `error_margin` 1.0 − `SHIPPING_INPUT_DELAY_TICKS` 3 = **0**.
//! `net::client::run` writes `SyncConfig { jitter_multiple, ..default() }`, so `jitter_margin` and
//! `error_margin` keep their 1.0 upstream defaults and the cancellation is exact, not incidental.
//! [`shipping_loopback_client_lead_is_exactly_zero_ticks`] derives that number from the configured
//! values rather than restating it.
//!
//! Worse, the sync controller is allowed to sit up to `error_margin` behind the objective without
//! correcting (`SyncContext::speed_adjustment` only acts once `offset.abs() > error_margin`), so
//! the realised integer lead ranges over `{-1, 0}`.
//!
//! And an actively driven tank is explicitly confirmed at every checkpoint, so its `ConfirmHistory`
//! *contains* the completed replicon tick — the opposite of what the older fixtures set up.
//!
//! # The two gates that then discard the shove
//!
//! - RECEIVE TIME: `record_confirmed_and_maybe_check` only runs the registered comparator when
//!   `confirmed_tick < current_tick`. At lead 0 they are equal, so the comparator never runs and
//!   no mismatch is ever recorded.
//! - COMPLETED-TICK FALLBACK: `check_rollback`'s unchanged-entity scan returns early for any
//!   entity whose `ConfirmHistory::contains` resolves the completed replicon tick — which is every
//!   entity the server explicitly updated, i.e. every driven tank.
//!
//! Neither gate can be observed at a lead of 8 with an anchored-away `ConfirmHistory`. That is why
//! the fixtures below exist: they were RED on the pre-slice-2 design and were its acceptance test.
//!
//! The lead-arithmetic guard stands apart from all of that: it asserts the premise the other
//! fixtures are BUILT on, and must fail loudly the moment someone edits a sync constant without
//! understanding that these four numbers cancel.

use core::time::Duration;

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
    InputTimeline, InputTimelineConfig, IsSynced, LocalTimeline, PeerId, PingManager, Predicted,
    PredictionHistory, PredictionManager, RemoteId, ReplicationCheckpointMap,
    StateRollbackMetadata, SyncConfig, Tick,
};
use lightyear_core::time::TickInstant;
use lightyear_core::timeline::NetworkTimeline;
use lightyear_sync::prelude::client::RemoteTimeline;
use lightyear_sync::timeline::sync::{SyncTargetTimeline, SyncedTimeline};

use super::adoption::{ImpactPresentation, ORDERING_BUDGET_TICKS, OrderingTally};
use crate::ballistics::{
    AuthorityImpact, HullShock, HullShockLedger, Impact, ImpactSurface, ShockCause,
};
use crate::tank::Tank;

/// The tick the authority closes the shock episode on, and the completed Replicon checkpoint that
/// certifies it.
const PRODUCING_TICK: Tick = Tick(100);

/// The combatant the fixture's hull belongs to — the victim the authority names on the impact fact
/// this client re-draws, and on the `HullShock` episode it must be matched with.
const VICTIM: crate::CombatantId = crate::CombatantId(1);

/// The FLOOR, in ticks, on the gap between a checkpoint's arrival and the frame that can present its
/// spark — and what this fixture advances the clock by.
///
/// Not a fixture convenience: `ImpactConfirm` is a message drained in `Update`
/// (`net::client::receive_fire_events`), which is AFTER the frame's fixed loop, so the ballistics
/// march cannot present it before the NEXT fixed tick, and lightyear advances `LocalTimeline` in
/// `FixedFirst`. One fixed step must therefore pass, and the fixture pays it rather than freezing the
/// clock and reporting a zero it did not earn.
///
/// It is a FLOOR and NOT a shipping constant. Bevy runs `FixedMain` zero or more times per frame
/// (`bevy_app::main_schedule`, `bevy_time::fixed`), so a catch-up frame can advance several ticks
/// between the `PreUpdate` that received the checkpoint and the one that adopts. What the assertions
/// below pin is that the rule costs the schedule's MINIMUM when the visual is there — never that
/// shipping always produces this number.
const PRESENTATION_DELAY_TICKS: i32 = 1;

/// A hit the authority resolved 12 ticks before the episode carrying it closed, and drawn long
/// before the shock arrives. See [`Visual::DrawnBeforeArrival`] and [`coalesced_shock`].
const EARLY_HIT_TICK: u32 = PRODUCING_TICK.0 - 12;
fn producing_replicon_tick() -> RepliconTick {
    RepliconTick::new(50)
}

const HULL_MASS: f32 = 100.0;
const HULL_INERTIA: f32 = 50.0;

/// The hull velocity the authority recorded at [`PRODUCING_TICK`] — an 88 mm hit's measured Δv, the
/// number the player is owed. Sub-`net::protocol::ROLLBACK_VELOCITY` by an order of magnitude, so
/// the velocity comparator alone will never fetch it.
const AUTHORITY_LINEAR: Vec3 = Vec3::new(0.0, 0.0, -0.138_3);
const AUTHORITY_ANGULAR: Vec3 = Vec3::new(0.191_0, 0.0, 0.052_0);
/// What the client predicted instead: an untouched hull.
const PREDICTED_LINEAR: Vec3 = Vec3::ZERO;
const PREDICTED_ANGULAR: Vec3 = Vec3::ZERO;
/// What the live hull is doing when the message lands — neither value, so no assertion below can be
/// satisfied by the fixture simply never having been written.
const LIVE_LINEAR: Vec3 = Vec3::new(3.0, 0.0, 3.0);
const LIVE_ANGULAR: Vec3 = Vec3::new(3.0, 3.0, 0.0);

/// THE ISOLATED HIT: a fresh `HullShockLedger` finds no open episode, so the hit publishes on the
/// tick it landed and the episode spans that one tick. The client's copy is `HullShock::default()`:
/// it had no way to know it was shot.
fn isolated_shock() -> HullShock {
    HullShock {
        count: 1,
        tick: PRODUCING_TICK.0,
        opened: PRODUCING_TICK.0,
        cause: ShockCause::Perforation,
    }
}

/// THE COALESCED EPISODE: an earlier episode closed at [`EARLY_HIT_TICK`] − 4, this hit landed at
/// [`EARLY_HIT_TICK`] four ticks later — inside the open window — and was deferred to
/// [`PRODUCING_TICK`], `SHOCK_EPISODE_TICKS` after that close.
///
/// The `count: 2` is not decoration. An episode that opened 12 ticks before it closed CANNOT be a
/// hull's first: `close_episode` publishes the first impulse a fresh ledger sees immediately, so a
/// deferred span implies a previous episode, hence a previous count. A fixture asserting
/// `count: 1` with `opened != tick` would be proving something on a premise the authority cannot
/// produce.
fn coalesced_shock() -> HullShock {
    HullShock {
        count: 2,
        tick: PRODUCING_TICK.0,
        opened: EARLY_HIT_TICK,
        cause: ShockCause::Perforation,
    }
}

/// Where the client's timeline sits relative to the completed checkpoint. Both values are reachable
/// on the shipping loopback build; see the module doc.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lead {
    /// The steady-state loopback lead: client tick == confirmed tick.
    Zero,
    /// The controller's allowed deadband drift: the checkpoint is one tick in the CLIENT's future.
    MinusOne,
}

impl Lead {
    fn ticks(self) -> i32 {
        match self {
            Lead::Zero => 0,
            Lead::MinusOne => -1,
        }
    }

    fn present_tick(self) -> Tick {
        Tick(PRODUCING_TICK.0.wrapping_add_signed(self.ticks()))
    }
}

/// WHEN this client drew the impact its shock belongs to, relative to the shock's arrival. The two
/// drawn cases are two real shapes the authority produces, not two fixture conveniences.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Visual {
    /// THE COALESCED-EPISODE SHAPE, and the common one under automatic fire. `HullShockLedger`
    /// defers every hit inside `SHOCK_EPISODE_TICKS` of an open episode, so the episode publishes
    /// LATER than the hits it is made of and their sparks were drawn before the shock ever arrived.
    /// [`EARLY_HIT_TICK`] is such a hit.
    DrawnBeforeArrival,
    /// THE ISOLATED-HIT SHAPE. The hit finds no open episode, so it closes on its own tick and the
    /// shock and its `ImpactConfirm` leave the authority together — but the confirm is a MESSAGE
    /// drained in `Update`, so the march cannot present it until a later fixed tick. This is the
    /// case the ordering rule was written for, and it costs [`PRESENTATION_DELAY_TICKS`].
    DrawnAfterArrival,
    /// The jitter/loss case the ordering budget exists for — a lost fire fact, or a cosmetic shell
    /// that quietly dissolved, so no spark is ever drawn for this hit.
    Missing,
}

impl Visual {
    /// The episode the authority published, which the drawn case DETERMINES rather than decorates: a
    /// spark drawn before the shock arrives is only one of the episode's own hits if the episode
    /// coalesced, and a coalesced episode is never a hull's first.
    fn shock(self) -> HullShock {
        match self {
            Visual::DrawnBeforeArrival => coalesced_shock(),
            Visual::DrawnAfterArrival | Visual::Missing => isolated_shock(),
        }
    }
}

/// What the run produced after the real `PreUpdate` schedule.
#[derive(Resource, Default)]
struct Delivered {
    live_linear: Vec3,
    live_angular: Vec3,
    /// Live hull velocity at the moment the fact FIRST became eligible on every gate but ordering —
    /// what the player would have felt if the shove were allowed to outrun its own spark.
    held_linear: Vec3,
    live_shock: HullShock,
    realized_count: u32,
    replayed_ticks: Vec<u32>,
    /// `StateRollbackMetadata::last_processed_tick` immediately after the checkpoint's FIRST
    /// `PreUpdate`. This is what separates the two leads: a checkpoint level with the client is
    /// examined and marked processed, one in the client's future is neither.
    processed_on_arrival: Option<Tick>,
    /// `net::adoption`'s ordering tallies after the run — which way the shove was released, and
    /// what the wait cost.
    ordering: OrderingTally,
}

fn observe_replay(timeline: Res<LocalTimeline>, mut delivered: ResMut<Delivered>) {
    delivered.replayed_ticks.push(timeline.tick().0);
}

/// Build a real lightyear client on the production registration, deposit the authority's
/// end-of-episode sample at [`PRODUCING_TICK`], place the client at `lead` ticks relative to it,
/// and run the real `PreUpdate` rollback schedule.
///
/// Two things separate this from `net::hull_shock_rollback`, and they are the whole point: the
/// entity's `ConfirmHistory` is anchored ON [`producing_replicon_tick`] (an actively driven tank is
/// explicitly confirmed at every checkpoint), and no mismatch is seeded by hand.
///
/// The frame ordering is the SHIPPING one, and that is load-bearing for the ordering assertions:
/// the arrival `PreUpdate` runs with no spark drawn, because `ImpactConfirm` is a message drained in
/// `Update` and the march cannot present it until a later frame. The clock then ADVANCES by
/// [`PRESENTATION_DELAY_TICKS`] before [`present_impact`] and the `PreUpdate` that may adopt, so the
/// wait the tallies report is the wait the schedule actually imposes. A fixture that drew the spark
/// before the arrival frame, or that held the clock still across it, would be asserting a timing the
/// real schedule cannot produce.
///
/// SCOPE, honestly stated: like both older fixtures this deposits confirmed history directly rather
/// than deserializing a replication message, so it executes the completed-tick scan gate and not
/// the receive-time one. Not seeding a mismatch is the faithful stand-in for the receive-time gate:
/// at a lead of 0 that path could not have recorded one, because it runs the comparator only when
/// `confirmed_tick < current_tick`. [`shipping_loopback_client_lead_is_exactly_zero_ticks`] is what
/// holds that premise in place.
fn run_arrival(lead: Lead, visual: Visual) -> Delivered {
    let mut app = crate::net::test_harness::base_app();
    app.add_plugins(ClientPlugins {
        tick_duration: crate::net::test_harness::TICK,
    });
    crate::state::sim_plugin(&mut app);
    super::protocol::plugin(&mut app);
    app.insert_state(crate::state::AppState::Playing);
    app.init_resource::<Delivered>();
    app.add_systems(FixedPreUpdate, observe_replay);
    crate::net::test_harness::finish(&mut app);

    app.world_mut().spawn((
        Client::default(),
        RemoteId(PeerId::Server),
        Connected,
        PredictionManager::default(),
        IsSynced::<InputTimeline>::default(),
    ));

    let mut confirmed_shock = ConfirmedHistory::<HullShock>::default();
    confirmed_shock.insert_present_explicit(PRODUCING_TICK, visual.shock());
    let mut predicted_shock_history = PredictionHistory::<HullShock>::default();
    predicted_shock_history.add_predicted(PRODUCING_TICK, Some(HullShock::default()));

    let mut confirmed_linear = ConfirmedHistory::<LinearVelocity>::default();
    confirmed_linear.insert_present_explicit(PRODUCING_TICK, LinearVelocity(AUTHORITY_LINEAR));
    let mut predicted_linear = PredictionHistory::<LinearVelocity>::default();
    predicted_linear.add_predicted(PRODUCING_TICK, Some(LinearVelocity(PREDICTED_LINEAR)));
    let mut confirmed_angular = ConfirmedHistory::<AngularVelocity>::default();
    confirmed_angular.insert_present_explicit(PRODUCING_TICK, AngularVelocity(AUTHORITY_ANGULAR));
    let mut predicted_angular = PredictionHistory::<AngularVelocity>::default();
    predicted_angular.add_predicted(PRODUCING_TICK, Some(AngularVelocity(PREDICTED_ANGULAR)));

    // THE POSE, and it is not decoration. `net::adoption`'s readiness gate requires that EVERY
    // authority-tracked part of the rigid body can be restored at the producing tick, and it fails
    // CLOSED on a missing `ConfirmedHistory` because that is the case in which lightyear's
    // `prepare_rollback` restores from the client's own prediction instead. A replicated, predicted
    // tank has all four histories; a fixture that omitted two would be offering the adoption path a
    // hull the shipping build never produces.
    let mut confirmed_position = ConfirmedHistory::<Position>::default();
    confirmed_position.insert_present_explicit(PRODUCING_TICK, Position::default());
    let mut predicted_position = PredictionHistory::<Position>::default();
    predicted_position.add_predicted(PRODUCING_TICK, Some(Position::default()));
    let mut confirmed_rotation = ConfirmedHistory::<Rotation>::default();
    confirmed_rotation.insert_present_explicit(PRODUCING_TICK, Rotation::default());
    let mut predicted_rotation = PredictionHistory::<Rotation>::default();
    predicted_rotation.add_predicted(PRODUCING_TICK, Some(Rotation::default()));

    let mut ledger_history = PredictionHistory::<HullShockLedger>::default();
    ledger_history.add_predicted(PRODUCING_TICK, Some(HullShockLedger::default()));

    let root = app
        .world_mut()
        .spawn((
            Predicted,
            Remote,
            // THE EVASION THE OLDER FIXTURES RELY ON, REMOVED. A tank the server is actively
            // driving is explicitly confirmed at every checkpoint, so `ConfirmHistory::contains`
            // resolves the completed replicon tick and the unchanged-entity scan skips it.
            ConfirmHistory::new(producing_replicon_tick()),
            Tank,
            Position::default(),
            confirmed_position,
            predicted_position,
            Rotation::default(),
            confirmed_rotation,
            predicted_rotation,
            HullShock::default(),
            predicted_shock_history,
            confirmed_shock,
            VICTIM,
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
        // Gravity off and no contacts, so replay neither adds to nor bleeds the delivered shove.
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

    advance_to(&mut app, lead.present_tick());
    if visual == Visual::DrawnBeforeArrival {
        present_impact(&mut app, EARLY_HIT_TICK);
    }
    app.world_mut().run_schedule(PreUpdate);
    let processed_on_arrival = app
        .world()
        .resource::<StateRollbackMetadata>()
        .last_processed_tick();

    // A checkpoint one tick in the client's future cannot be acted on when it lands, and lightyear
    // does not mark it processed, so it is re-examined on the next frame. Give the client that
    // frame: a shove must not be lost merely because it arrived a tick early.
    if lead == Lead::MinusOne {
        advance_to(&mut app, PRODUCING_TICK);
        app.world_mut().run_schedule(PreUpdate);
    }

    // The arrival frame is over. For an ISOLATED hit no spark can have been drawn for it yet — that
    // is the schedule, not a fixture convenience: `ImpactConfirm` is a message drained in `Update`,
    // so the earliest the ballistics march can present it is a later frame. This is what the player
    // would have felt if the shove were allowed to outrun its own spark. (For a COALESCED episode
    // the spark came first and the shove has correctly already landed by here.)
    let held_linear = app.world().get::<LinearVelocity>(root).unwrap().0;

    // The later frame, with the march's presentation in it — and the ticks that separate the two.
    if visual == Visual::DrawnAfterArrival {
        let present = app.world().resource::<LocalTimeline>().tick() + PRESENTATION_DELAY_TICKS;
        advance_to(&mut app, present);
        present_impact(&mut app, PRODUCING_TICK.0);
        app.world_mut().run_schedule(PreUpdate);
    }

    if visual == Visual::Missing {
        // Spend the ordering budget. A visual that has not been drawn by now is not coming, so the
        // shove must land anyway rather than be held for it forever.
        advance_to(
            &mut app,
            Tick(PRODUCING_TICK.0.wrapping_add_signed(ORDERING_BUDGET_TICKS)),
        );
        app.world_mut().run_schedule(PreUpdate);
    }

    let ordering = app.world().resource::<ImpactPresentation>().tally();
    let live_linear = app.world().get::<LinearVelocity>(root).unwrap().0;
    let live_angular = app.world().get::<AngularVelocity>(root).unwrap().0;
    let live_shock = *app.world().get::<HullShock>(root).unwrap();
    let realized_count = app.world().get::<HullShockLedger>(root).unwrap().applied();
    let mut delivered = app
        .world_mut()
        .remove_resource::<Delivered>()
        .expect("the evidence resource outlives the schedule");
    delivered.live_linear = live_linear;
    delivered.live_angular = live_angular;
    delivered.held_linear = held_linear;
    delivered.live_shock = live_shock;
    delivered.realized_count = realized_count;
    delivered.processed_on_arrival = processed_on_arrival;
    delivered.ordering = ordering;
    delivered
}

/// Draw the armor impact for the hit the authority resolved at `authority_tick`.
///
/// `crate::ballistics::Impact` is the real presentation signal — `vfx::impact` renders off it and
/// `net::adoption` records its ordering ledger from it — so raising it here is the faithful stand-in
/// for "the march consumed this shot's `ImpactConfirm` and the player saw the spark". The
/// [`AuthorityImpact`] is what `ballistics::finish_at_sanctioned_terminal` copies off the sanctioned
/// terminal: the SERVER tick the impact resolved on and the body the authority gave the impulse to.
/// It is the correlation handle, so a fixture without it would present a spark belonging to nothing.
fn present_impact(app: &mut App, authority_tick: u32) {
    app.world_mut().trigger(Impact {
        position: Vec3::ZERO,
        normal: Vec3::Z,
        caliber: 0.088,
        surface: ImpactSurface::Armor,
        penetrated: true,
        deflection: None,
        authority: Some(AuthorityImpact {
            tick: authority_tick,
            victim: Some(VICTIM),
        }),
    });
    app.world_mut().flush();
}

/// Walk both the tick counter and `Time<Fixed>` forward to `target`, keeping them consistent — the
/// rollback path reads the timeline, the replay loop reads the fixed clock.
fn advance_to(app: &mut App, target: Tick) {
    let current = app.world().resource::<LocalTimeline>().tick();
    let steps = target - current;
    assert!(steps >= 0, "the fixture only ever moves the clock forward");
    app.world_mut()
        .resource_mut::<LocalTimeline>()
        .apply_delta(steps);
    for _ in 0..steps {
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(crate::net::test_harness::TICK);
    }
}

/// The steady-state shipping condition: the client is level with the completed checkpoint and the
/// tank is explicitly confirmed there. The authority applied a hull impulse and published it; the
/// client must end up moving. GREEN since slice 2, via `net::adoption` — neither lightyear route
/// runs here, and neither has to.
#[test]
fn an_authority_shock_reaches_the_live_hull_at_zero_lead() {
    assert_delivered(Lead::Zero);
}

/// The sibling the sync deadband makes reachable: the checkpoint lands one tick ahead of the
/// client, which then catches up. The shove must survive the wait.
#[test]
fn an_authority_shock_reaches_the_live_hull_at_minus_one_lead() {
    assert_delivered(Lead::MinusOne);
}

/// THE ZERO-REPLAY PROOF, and the reason `net::adoption` restores end-of-`T` rather than end-of-
/// `T−1`. At a lead of 0 the rollback's replay loop body runs ZERO times
/// (`num_rollback_ticks = current_tick − rollback_tick = 0`), so nothing that depends on a system
/// re-running can be delivered. The shove still arrives, because `prepare_rollback` — not the loop —
/// is what writes the live hull velocity. Asserting the empty replay list is what stops a future
/// edit from "fixing" the restore shape into one that silently drops the effect at this lead.
///
/// [`Visual::DrawnBeforeArrival`] is what makes the regime reachable at all now that the shove waits
/// for its spark: an episode that publishes on the SAME tick the client is standing on can only be
/// adopted with no replay if the spark is already drawn, which is exactly the coalesced-burst shape
/// — and [`coalesced_shock`] is a burst the authority can actually publish, not a first episode with
/// a span it could never have had. An isolated hit pays a tick; see
/// [`a_drawn_spark_costs_the_shove_no_more_than_the_schedules_own_delay`].
#[test]
fn the_zero_lead_shove_arrives_with_no_replayed_tick_at_all() {
    let delivered = run_arrival(Lead::Zero, Visual::DrawnBeforeArrival);

    assert_eq!(
        delivered.replayed_ticks,
        Vec::<u32>::new(),
        "a lead-0 forced rollback has no replay window; if this list is non-empty the fixture no \
         longer reproduces the regime it exists for",
    );
    assert_eq!(delivered.live_linear, AUTHORITY_LINEAR);
    assert_eq!(delivered.live_angular, AUTHORITY_ANGULAR);
    assert_eq!(
        delivered.ordering,
        OrderingTally {
            released_on_impact: 1,
            ..default()
        },
        "a hit drawn {} ticks before its episode closed IS one of that episode's hits, so the rule \
         costs nothing at all here — it neither waited nor fell through to the budget",
        PRODUCING_TICK.0 - EARLY_HIT_TICK,
    );
}

/// The contract both delivery fixtures assert: whatever route the adoption takes, the server's hull
/// velocity has to become the client's LIVE hull velocity.
fn assert_delivered(lead: Lead) {
    let delivered = run_arrival(lead, Visual::DrawnAfterArrival);

    assert_eq!(
        delivered.live_linear,
        AUTHORITY_LINEAR,
        "at a client lead of {} tick(s) the authority's hull shove must still reach the client's \
         LIVE hull velocity — this is what the player feels. Evidence: live shock = {:?} (the \
         authority published {:?}), ledger realized {} episode(s), ticks replayed = {:?}, \
         checkpoint processed on arrival = {:?}",
        lead.ticks(),
        delivered.live_shock,
        Visual::DrawnAfterArrival.shock(),
        delivered.realized_count,
        delivered.replayed_ticks,
        delivered.processed_on_arrival,
    );
    assert_eq!(delivered.live_angular, AUTHORITY_ANGULAR);
}

/// THE ORDERING RULE, at the lead where the schedule makes it bite. The shock and its
/// `ImpactConfirm` leave the authority together, but the confirm is a MESSAGE drained in `Update`
/// and presented by a LATER frame's march, while the shock is replicated state adopted here in
/// `PreUpdate`. Without a rule the hull would lurch a frame before anything was seen to hit it.
///
/// So: no spark drawn, no shove — and then the budget releases it anyway, because a shove that never
/// arrives is the one failure this seam is not allowed to have.
#[test]
fn a_shove_waits_for_its_spark_and_the_budget_still_delivers_it() {
    let delivered = run_arrival(Lead::Zero, Visual::Missing);

    assert_eq!(
        delivered.held_linear, LIVE_LINEAR,
        "with no armor impact drawn, the shove must NOT have landed yet — a hull that lurches \
         before its spark reads as broken",
    );
    assert_eq!(
        delivered.live_linear, AUTHORITY_LINEAR,
        "after {ORDERING_BUDGET_TICKS} ticks the visual is not coming, and the shove must land \
         anyway: ordering is a preference, delivery is not. Evidence: live shock = {:?}, ticks \
         replayed = {:?}",
        delivered.live_shock, delivered.replayed_ticks,
    );
    assert_eq!(delivered.live_angular, AUTHORITY_ANGULAR);
    assert_eq!(
        delivered.ordering,
        OrderingTally {
            released_on_budget: 1,
            max_wait_ticks: ORDERING_BUDGET_TICKS,
            ..default()
        },
        "the release must be attributed to the budget, and the wait it cost must be reported",
    );
}

/// The same run with the spark drawn: the shove lands on the FIRST `PreUpdate` after the
/// presentation, so the rule adds nothing to the delay the schedule already imposes. Without this
/// the fixture above could pass on a rule that always waits out the full budget.
///
/// WHAT IS PINNED, precisely: with the clock advanced by the ONE fixed step the schedule's ordering
/// forces, `max_wait_ticks` is that one tick and not more. That is a FLOOR being met, not a shipping
/// constant — `FixedMain` runs zero or more times per frame, so a catch-up frame can put several
/// ticks between the arrival `PreUpdate` and the adopting one, and the rule would then report the
/// larger number without being any slower. A rule that waited an extra frame of its own, or a fixture
/// that froze the clock and reported a zero it did not earn, fails here.
#[test]
fn a_drawn_spark_costs_the_shove_no_more_than_the_schedules_own_delay() {
    let delivered = run_arrival(Lead::Zero, Visual::DrawnAfterArrival);

    assert_eq!(
        delivered.live_linear, AUTHORITY_LINEAR,
        "the frame after the spark is drawn, the shove must land",
    );
    assert_eq!(
        delivered.ordering,
        OrderingTally {
            released_on_impact: 1,
            max_wait_ticks: PRESENTATION_DELAY_TICKS,
            ..default()
        },
        "released against the spark after the {PRESENTATION_DELAY_TICKS} tick(s) this run advanced \
         the clock by — not by the budget, and not bypassed",
    );
}

/// GUARD ON THE LEAD ARITHMETIC.
///
/// Reads the values `net::client::run` actually installs — `net::client::shipping_input_delay`,
/// `net::harness::jitter_multiple`, and the `SyncConfig` defaults the `..default()` in that
/// composition leaves alone — and asks lightyear's own `sync_objective` where the client's input
/// timeline lands relative to a zero-RTT, zero-jitter server. The answer is the number every fixture
/// above is built on, so it is derived here and nowhere else.
#[test]
fn shipping_loopback_client_lead_is_exactly_zero_ticks() {
    let pings = PingManager::default();
    assert_eq!(
        (pings.rtt(), pings.jitter()),
        (Duration::ZERO, Duration::ZERO),
        "the loopback premise: a fresh PingManager must report no latency and no jitter, so every \
         jitter-scaled term in the objective drops out and only the constants remain",
    );

    let lead = loopback_client_lead_ticks();

    assert!(
        lead.abs() < 1.0 / 256.0,
        "the shipping loopback client lead is {lead} ticks, not 0.\n\
         \n\
         WHAT THIS NUMBER IS: how far the client's predicted tick runs ahead of the server tick \
         whose replicated state it is comparing against, on a zero-latency link. lightyear \
         computes it as jitter_margin + 1 + error_margin - input_delay, which with our \
         configuration is 1.0 + 1 + 1.0 - {} = 0.\n\
         \n\
         WHY IT MATTERS: at a lead of 0 the receive-time mismatch check is skipped outright \
         (`record_confirmed_and_maybe_check` requires confirmed_tick < current_tick), so an \
         authoritative fact the client could not predict has exactly one remaining route into the \
         simulation — the completed-tick scan — and that route skips every entity the server \
         explicitly confirmed at the checkpoint. Changing SHIPPING_INPUT_DELAY_TICKS, \
         net::harness::jitter_multiple, or the SyncConfig jitter_margin / error_margin defaults \
         moves this number and silently changes which of those routes is live. If you meant to \
         move it, re-derive the ignored fixtures in this module against the new lead.",
        shipping_input_delay_ticks(),
    );
}

/// The client's steady-state lead over the server's confirmed tick at loopback, in ticks, computed
/// from the production configuration rather than restated.
///
/// `InputTimeline::default()` carries an input delay of 0 in its context (the field is private to
/// lightyear_sync and only the sync plugin writes it), so `sync_objective` returns the lead BEFORE
/// the input-delay subtraction. That subtraction is a plain `- input_delay` in the objective, so
/// applying it here from our own configured delay reproduces it exactly.
fn loopback_client_lead_ticks() -> f32 {
    let config = InputTimelineConfig::new(
        SyncConfig {
            jitter_multiple: super::harness::jitter_multiple(),
            ..default()
        },
        super::client::shipping_input_delay(),
    );
    let mut remote = RemoteTimeline::default();
    // Anchored away from tick 0 so a negative lead would read as a negative delta rather than
    // wrapping the fixed-point instant.
    remote.set_now(TickInstant::from(PRODUCING_TICK));
    let objective = InputTimeline::default().sync_objective(
        &remote,
        &config,
        &PingManager::default(),
        crate::net::test_harness::TICK,
    );
    let before_input_delay = (objective - remote.current_estimate()).to_f32();
    before_input_delay - f32::from(shipping_input_delay_ticks())
}

/// The fixed input delay the client ships, read off the production config. `net::client` already
/// pins that this config is constant-shaped (minimum == maximum), which is what makes reading one
/// field the whole answer for any RTT.
fn shipping_input_delay_ticks() -> u16 {
    let config = super::client::shipping_input_delay();
    assert_eq!(
        config.minimum_input_delay_ticks, config.maximum_input_delay_before_prediction,
        "shipping_input_delay stopped being a constant delay — the loopback lead is no longer \
         readable from one field, and this module's whole premise needs re-deriving",
    );
    config.minimum_input_delay_ticks
}
