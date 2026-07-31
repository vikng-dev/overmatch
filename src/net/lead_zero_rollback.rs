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
//!
//! # And two fixtures about the RESTORE, not the route
//!
//! Everything above is about whether an adoption is REQUESTED. Two later fixtures ask what
//! `prepare_rollback` actually put on the hull, because `net::adoption`'s unit tests can only hand
//! its classifier a verdict and read the counter that follows — they never execute the restore, and a
//! counter that faithfully follows a wrong classifier is exactly how a lost shove reads as a healthy
//! system. Both run lightyear's own `RollbackSystems::Prepare` and then read the LIVE hull velocity:
//!
//! - [`a_fact_whose_restore_cannot_carry_the_shove_is_never_requested`] — the authority's newest
//!   confirmed velocity predates the episode's close, so no rollback to the producing tick can
//!   deliver anything. Nothing may be requested and no counter may move.
//! - [`a_rollback_this_module_did_not_order_delivers_the_shove_and_is_counted`] — somebody else's
//!   rollback restores the authority's velocities while the fact waits for a spark. The hull moves,
//!   so `bypassed` must move with it.
//!
//! # And one about the HISTORY MOVING between staging and requesting
//!
//! Both of the above build a confirmed history once and never touch it again, which is the shape
//! every fixture in this arc had and the reason a stale-readiness defect survived five reviews. An
//! `ExternalEvent` fact is staged on one frame and requested on a later one, and confirmed history
//! is not append-only in between: lightyear middle-inserts in tick order and re-resolves unchanged
//! markers against late preceding samples. [`a_late_replicated_change_is_revalidated_before_the_request`]
//! is the fixture that moves it, through lightyear's own insertion API, and pins that the module
//! WAITS rather than spending the fact on the answer it got at staging.
//!
//! [`a_late_pose_removal_is_revalidated_before_the_request`] is its other half, and it exists
//! because the first fix was only half a fix: readiness is a claim about all FOUR of the hull's
//! rigid-body histories, and re-proving only the two the shove rides on let a late `Position`
//! removal through to a claimed rollback that deleted the component while every counter in the
//! module reported a clean adoption.
//!
//! # And one about the hull leaving the RESTORE, not the histories changing
//!
//! All three of those move a confirmed history. [`a_hull_excluded_from_prepare_is_never_requested_and_never_adopted`]
//! moves the hull's ARCHETYPE instead: `DisableRollback` arrives between the frame that staged the
//! fact and the frame that would request it — which is what `net::rig`'s late-prediction promotion
//! does in `Update`, one schedule position after the `FixedLast` that would remove it. lightyear's
//! `prepare_rollback` then skips the hull for every component, and a delivery verdict read off
//! `ConfirmedHistory` cannot see that, because that buffer answers what a restore WOULD resolve.
//!
//! # And one that is a CONTRACT rather than a scenario
//!
//! Every fixture above asserts what this module does in a situation.
//! [`prepare_restores_exactly_the_components_the_predicate_names`] asserts something about the
//! DEPENDENCY: `net::adoption::prepare_restores` is a hand-written mirror of `prepare_rollback`'s
//! query, and a mirror is only true at the moment it is written. It runs the real
//! `RollbackSystems::Prepare` over all 32 archetypes the two membership conditions can produce and
//! checks, per component, that what was restored is what the predicate says — so a lightyear bump
//! that adds, drops or changes a membership condition fails here instead of silently making a
//! paraphrase wrong. Its source-side twin lives in `net::adoption`
//! (`the_three_participation_sites_ask_one_shared_question`) and pins that the condition is spelled
//! once and consumed through one predicate.

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
    DisableRollback, InputTimeline, InputTimelineConfig, IsSynced, LocalTimeline, PeerId,
    PingManager, Predicted, PredictionHistory, RemoteId, ReplicationCheckpointMap, RollbackSystems,
    StateRollbackMetadata, SyncConfig, Tick, VisualCorrection,
};
use lightyear_core::time::TickInstant;
use lightyear_core::timeline::NetworkTimeline;
use lightyear_sync::prelude::client::RemoteTimeline;
use lightyear_sync::timeline::sync::{SyncTargetTimeline, SyncedTimeline};

use super::adoption::{
    AdoptionCause, AuthorityAdoption, ForcedRollbackSlot, ImpactPresentation,
    ORDERING_BUDGET_TICKS, OrderingTally, SharpCorrection,
};
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

/// The SECOND episode of the contention control: closes four ticks after the head's, while the
/// head is still staged and holding for its spark. Its velocities are distinct so delivery is
/// attributable — an assertion satisfied by the head's values would be reading the wrong restore.
const FOLLOW_UP_TICK: Tick = Tick(104);
const FOLLOW_UP_LINEAR: Vec3 = Vec3::new(0.0, 0.0, -0.276_6);
const FOLLOW_UP_ANGULAR: Vec3 = Vec3::new(0.382_0, 0.0, 0.104_0);

fn follow_up_shock() -> HullShock {
    HullShock {
        count: isolated_shock().count + 1,
        tick: FOLLOW_UP_TICK.0,
        opened: FOLLOW_UP_TICK.0,
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

    /// Where the client stands when the checkpoint that CARRIES the episode lands, which is
    /// `arrival` — the tick replication materialized it, not the tick the episode settled on. The
    /// two are the same in every scenario but [`REPLICATION_LAG_TICKS`].
    fn present_tick(self, arrival: Tick) -> Tick {
        Tick(arrival.0.wrapping_add_signed(self.ticks()))
    }
}

/// How far behind the checkpoint that carries it an episode can have SETTLED.
///
/// Replication stamps a component change with the tick the message went out, so an episode that
/// closed at [`PRODUCING_TICK`] materializes in the client's `ConfirmedHistory<HullShock>` at the
/// next send tick. Any send interval above one tick produces this, and it is the shape that
/// separates `AuthoritativeFact::produced_at` (the confirmed sample, and the restore target) from
/// `AuthoritativeFact::settled_at` (the episode's close, and what a restore has to reach to have
/// delivered anything). Four ticks is arbitrary in size and load-bearing in sign.
const REPLICATION_LAG_TICKS: u32 = 4;

/// A late replicated change to the hull's confirmed LINEAR-velocity history, applied AFTER the
/// arrival frame staged the fact and BEFORE any frame that could request it.
///
/// WHAT EACH OF THE TWO WRITES DOES, because a fixture that names a mechanism it does not exercise
/// is worse than one that names nothing. Applied in field order, against a history holding the
/// authority's tick-[`PRODUCING_TICK`] sample, with the restore target at `arrival`:
///
/// 1. `newer_sample_at` carries the value the history already resolves to, so lightyear stores it as
///    a `SameAsPrecedent` marker rather than a second copy. It sits strictly between `removed_at`
///    and the restore target, which is what puts it ON the lookup's path:
///    `get_state_at_or_before(target)` lands on THIS entry and has to walk back through it.
/// 2. `removed_at` then MIDDLE-inserts an authoritative removal in front of it. That is the write
///    that changes the verdict — the marker now resolves backwards to `Removed` instead of to the
///    tick-100 value, which is lightyear's dynamic unchanged-marker behaviour and the reason the
///    predicate cannot be evaluated once at staging.
///
/// Both go through `ConfirmedHistory`'s own insertion API — the one
/// `record_confirmed_and_maybe_check` calls when a mutation is deserialized — so the fixture
/// exercises lightyear's real sorted middle insertion and unchanged-state compression rather than a
/// hand-assembled buffer that merely looks like their result.
#[derive(Clone, Copy)]
struct LateVelocityChange {
    /// The tick a later authoritative sample is stamped with. Between `removed_at` and the restore
    /// target, so the target's lookup RESOLVES THROUGH the unchanged marker it is stored as.
    newer_sample_at: Tick,
    /// The tick an authoritative REMOVAL is stamped with. Strictly between the episode's close and
    /// the restore target, so it MIDDLE-inserts and `get_state_at_or_before(target)` resolves it.
    removed_at: Tick,
}

impl LateVelocityChange {
    fn apply(self, history: &mut ConfirmedHistory<LinearVelocity>) {
        assert!(
            self.removed_at - self.newer_sample_at < 0,
            "the removal has to land strictly BEFORE the marker, or the marker resolves to the \
             pre-removal value and this fixture is claiming a mechanism it does not exercise",
        );
        history.insert_present(self.newer_sample_at, LinearVelocity(AUTHORITY_LINEAR));
        history.insert_removed(self.removed_at);
    }
}

/// The late replication both revalidation fixtures run on, stated ONCE because the arithmetic is
/// the whole coverage claim and two copies of it can drift apart.
///
/// Against a restore target of [`PRODUCING_TICK`] + [`REPLICATION_LAG_TICKS`] = 104: the removal
/// lands at 102 and the unchanged marker at 103, so `get_state_at_or_before(104)` lands on the
/// MARKER and resolves back through it to the removal. An earlier version put the marker past the
/// target, where the lookup never reached it and only the removal was doing any work.
fn a_removal_the_restore_target_resolves() -> LateVelocityChange {
    LateVelocityChange {
        newer_sample_at: Tick(PRODUCING_TICK.0 + 3),
        removed_at: Tick(PRODUCING_TICK.0 + 2),
    }
}

/// A forced rollback ordered by a subsystem that is NOT `net::adoption`, and the tick it targets.
///
/// Staged through `ForcedRollbackSlot::claim` tagged `Misprediction`, which is literally the call
/// `net::watchdog` makes — so this is a real production claimant, not a fixture poking
/// `request_forced_rollback` behind the one-slot rule the source scan in `net::adoption` pins.
#[derive(Resource, Clone, Copy)]
struct CompetingClaim(Tick);

/// Claim the slot once, on the first `PreUpdate` that runs after `net::adoption` has had its turn.
fn claim_the_slot_for_someone_else(
    claim: Option<Res<CompetingClaim>>,
    mut metadata: ResMut<StateRollbackMetadata>,
    mut slot: ResMut<ForcedRollbackSlot>,
    mut claimed: Local<bool>,
) {
    let Some(claim) = claim else {
        return;
    };
    if *claimed {
        return;
    }
    *claimed = slot.claim(&mut metadata, claim.0, AdoptionCause::Misprediction);
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
    /// `StateRollbackMetadata::forced_rollback_tick()` after the run's LAST `PreUpdate`. The
    /// same-frame consumption invariant reads through this: lightyear consumes a forced request in
    /// the same `PreUpdate` that claims it, so a `Some` here means a claim outlived its frame —
    /// either the schedule moved (`request_staged_adoption` no longer precedes
    /// `RollbackSystems::Check`) or the consumption gate diverged from the claim gate. Both are
    /// the setup for a rejected request that suppresses a frame's native policy checks.
    forced_tick_after: Option<Tick>,
    /// `net::adoption`'s ordering tallies after the run — which way the shove was released, and
    /// what the wait cost.
    ordering: OrderingTally,
    /// Whether the fact is STILL mid-transaction at the end of the run. What tells a WAIT (it will
    /// be reconsidered) from a DROP (it was closed and can never be requested again) — the two are
    /// otherwise indistinguishable from a hull that did not move.
    still_staged: bool,
    /// Whether the hull ended the run with a velocity component AT ALL. A restore that resolves an
    /// authoritative REMOVAL deletes it, which is a distinct failure from restoring a stale value
    /// and would otherwise read as an `unwrap` panic with nothing to say.
    live_velocity_removed: bool,
    /// The same question for the POSE, and it needs its own field: the shove is a velocity impulse,
    /// so every other piece of evidence here can read "delivered" while `prepare_rollback` has taken
    /// `Position` or `Rotation` off the hull.
    live_pose_removed: bool,
    /// The staged fact's AGE at the end of the run — the last local tick minus the restore target.
    /// What lets a fixture about the replay window pin the boundary it NAMES rather than assert
    /// against "some tick past it".
    final_age: i32,
    /// Every [`SharpCorrection`] the run emitted, drained at the schedule point
    /// `net::render_error::capture_render_error` drains them at.
    ///
    /// This is the PRESENTATION half of the transaction and it has to be evidence from the same run
    /// as the delivery half: "the shove reached the live hull" and "the view was told to show it
    /// sharp" are two different claims, and a fixture that only checks the first cannot see a
    /// presentation rule that smooths a delivered hit away.
    sharp: Vec<Entity>,
    /// The predicted hull the run built, so a `sharp` entry can be checked against the entity it
    /// must name rather than merely counted.
    hull: Option<Entity>,
    /// Present only when the run mounts the shipping render-error composition.
    render_offset: Option<(Vec3, Quat)>,
    /// Whether Lightyear retained a duplicate position/rotation correction after render capture.
    duplicate_visual_correction: bool,
}

/// Drain the presentation occurrences exactly where the client's consumer drains them.
///
/// `net::render_error` is a CLIENT plugin and this fixture builds the shared protocol only, so
/// nothing here would otherwise consume the queue. Draining it — rather than reading it at the end
/// of the run — is what makes the recorded list per-frame occurrences rather than an accumulation
/// that a one-shot bug could not be told apart from.
fn collect_sharp_corrections(
    mut occurrences: ResMut<bevy::ecs::message::Messages<SharpCorrection>>,
    mut delivered: ResMut<Delivered>,
) {
    delivered
        .sharp
        .extend(occurrences.drain().map(|occurrence| occurrence.entity));
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
    run_scenario(Scenario::new(lead, visual))
}

/// The client link, with or without `IsSynced<InputTimeline>`. Synced is the shipping steady
/// state; unsynced is the connected-but-not-yet-synced window [`Scenario::synced`] exists to
/// reach, where lightyear's `check_rollback` skips and a forced claim would go unconsumed.
fn spawn_client_link(app: &mut App, synced: bool) -> Entity {
    let link = app
        .world_mut()
        .spawn((
            Client::default(),
            RemoteId(PeerId::Server),
            Connected,
            crate::net::test_harness::prediction_manager(),
        ))
        .id();
    if synced {
        app.world_mut()
            .entity_mut(link)
            .insert(IsSynced::<InputTimeline>::default());
    }
    link
}

/// Everything about a run that a fixture is allowed to move, so that what a fixture DID move is
/// visible at its call site. [`Scenario::new`] is the shape every pre-existing test was written
/// against: replication carries the episode on the tick it closed, the authority's velocities are
/// confirmed there too, and nobody else touches the forced-rollback slot.
#[derive(Clone, Copy)]
struct Scenario {
    lead: Lead,
    visual: Visual,
    /// Whether the client link carries `IsSynced<InputTimeline>`. `true` is the shipping steady
    /// state every pre-existing test runs in. `false` is the connected-but-not-yet-synced window:
    /// lightyear's `check_rollback` skips on its `Single<…, With<IsSynced<…>>>` there, so a forced
    /// request claimed on such a frame would sit unconsumed while the first sync rewrites
    /// `LocalTimeline` under it — the one interleaving that can turn a valid claim into a
    /// rejected one that suppresses a frame's native policy checks.
    synced: bool,
    /// The tick replication MATERIALIZED the episode on — the confirmed `HullShock` sample's tick,
    /// which is `AuthoritativeFact::produced_at` and the restore target. At or after the tick the
    /// episode closed; see [`REPLICATION_LAG_TICKS`].
    arrival: Tick,
    /// The tick the authority's hull velocities were last confirmed at. Moving it BEFORE the
    /// episode's close is how a fixture asks "what if the restore cannot carry the shove?".
    velocities_confirmed_at: Tick,
    /// A forced rollback claimed by a subsystem other than `net::adoption`, and its target tick.
    competitor: Option<Tick>,
    /// Replication moving the confirmed history AFTER the fact is staged. `None` is the static
    /// history every other fixture here builds.
    late_change: Option<LateVelocityChange>,
    /// The tick an authoritative REMOVAL of the hull's `Position` is stamped with, applied at the
    /// same point in the run as [`Scenario::late_change`]. The POSE half of the same question: this
    /// component carries none of the shove, so the two velocity histories can go on answering
    /// "delivered" while a restore here deletes half the rigid body.
    late_position_removal: Option<Tick>,
    /// Whether to put `DisableRollback` on the hull at the same point in the run as
    /// [`Scenario::late_change`] — after the arrival frame staged the fact and before any frame that
    /// could request it. The ARCHETYPE half of the same question: the two history knobs above move
    /// what a restore would resolve, and this one moves whether the hull is in the restore at all.
    late_rollback_disable: bool,
    /// A SECOND episode arriving while the head is staged and held: the single staging slot's
    /// contention case. The newer fact is re-offered every frame (`SlotBusy`), stages the frame
    /// after the head's transaction closes, holds for ITS OWN spark, and must then be adopted —
    /// or the slot design has turned backpressure into loss.
    follow_up_episode: bool,
    /// Extra LOCAL ticks to spend past the run's own last frame, each with a `PreUpdate`. How a
    /// fixture asks what bounds a wait.
    extra_ticks: i32,
    /// Mount the shipping frame-interpolation and render-error plugins, so the real adoption's
    /// `SharpCorrection` is consumed rather than collected by this fixture.
    render_error: bool,
}

impl Scenario {
    fn new(lead: Lead, visual: Visual) -> Self {
        Self {
            lead,
            visual,
            arrival: PRODUCING_TICK,
            velocities_confirmed_at: PRODUCING_TICK,
            competitor: None,
            late_change: None,
            late_position_removal: None,
            late_rollback_disable: false,
            follow_up_episode: false,
            extra_ticks: 0,
            render_error: false,
            synced: true,
        }
    }
}

fn scenario_app(render_error: bool) -> App {
    let mut app = if render_error {
        crate::net::test_harness::net_physics_app()
    } else {
        crate::net::test_harness::base_app()
    };
    app.add_plugins(ClientPlugins {
        tick_duration: crate::net::test_harness::TICK,
    });
    crate::state::sim_plugin(&mut app);
    super::protocol::plugin(&mut app);
    if render_error {
        super::rig::client_smoothing_plugin(&mut app);
        super::render_error::plugin(&mut app);
    }
    app.insert_state(crate::state::AppState::Playing);
    app
}

fn scenario_poses(render_error: bool) -> (Vec3, Quat, Vec3, Quat) {
    if render_error {
        (
            AUTHORITY_POSITION,
            authority_rotation(),
            LIVE_POSITION,
            live_rotation(),
        )
    } else {
        (Vec3::ZERO, Quat::IDENTITY, Vec3::ZERO, Quat::IDENTITY)
    }
}

fn arm_scenario_render_error(app: &mut App, root: Entity) {
    app.world_mut()
        .entity_mut(root)
        .insert(super::protocol::NetTank);
    app.world_mut().flush();
    // Both predicates are production Update systems. The frame-interpolation marker arrives on the
    // first pass; the strictly narrower render-error arming sees it on the second.
    app.world_mut().run_schedule(Update);
    app.world_mut().run_schedule(Update);
    assert!(
        app.world()
            .get::<super::render_error::RenderErrorOffset>(root)
            .is_some(),
        "the real shipping arming path must own the integration root",
    );
}

/// The mid-transaction mutations: replication (or production markers) moving under a staged
/// fact. Split from [`run_scenario`] for length only — every knob still reads at the call site.
fn apply_late_mutations(
    app: &mut App,
    root: Entity,
    arrival: Tick,
    late_change: Option<LateVelocityChange>,
    late_position_removal: Option<Tick>,
    late_rollback_disable: bool,
) {
    // REPLICATION MOVES UNDER THE STAGED FACT. The arrival frame has staged it and the ordering rule
    // is holding it; every frame from here reads a history that is no longer the one the offer's
    // gate saw. This is the only fixture in the file that does not freeze the history after setup.
    if let Some(change) = late_change {
        assert!(
            change.newer_sample_at - arrival <= 0,
            "the unchanged marker must sit at or before the restore target ({}), or the target's \
             lookup never reaches it and the fixture is not exercising marker resolution at all",
            arrival.0,
        );
        let mut history = app
            .world_mut()
            .get_mut::<ConfirmedHistory<LinearVelocity>>(root)
            .expect("the hull's confirmed linear-velocity history");
        change.apply(&mut history);
    }
    if let Some(removed_at) = late_position_removal {
        let mut history = app
            .world_mut()
            .get_mut::<ConfirmedHistory<Position>>(root)
            .expect("the hull's confirmed position history");
        history.insert_removed(removed_at);
    }
    if late_rollback_disable {
        // THE PRODUCTION INSERTION, at the point in the run production reaches it. `net::rig`'s
        // `upgrade_predicted_to_dynamic` puts this marker on in `Update`; Bevy runs
        // `RunFixedMainLoop` — and with it the `FixedLast` remover — BEFORE `Update`, so the marker
        // necessarily survives to at least the next `PreUpdate`. This fixture writes it directly
        // rather than driving the promotion path, because that path also flips the body to Dynamic
        // and would change what the replay integrates; what is under test is the MARKER's effect on
        // the rollback transaction, and lightyear reads nothing else off it.
        app.world_mut().entity_mut(root).insert(DisableRollback);
        app.world_mut().flush();
    }
}

fn run_scenario(scenario: Scenario) -> Delivered {
    let Scenario {
        lead,
        visual,
        arrival,
        velocities_confirmed_at,
        competitor,
        late_change,
        late_position_removal,
        late_rollback_disable,
        follow_up_episode,
        extra_ticks,
        render_error,
        synced,
    } = scenario;
    let mut app = scenario_app(render_error);
    app.init_resource::<Delivered>();
    app.add_systems(FixedPreUpdate, observe_replay);
    // Drain at EndRollback, immediately after `net::render_error::capture_render_error` does in the
    // shipping client. This fixture does not mount that consumer, so the occurrences remain for us.
    if !render_error {
        app.add_systems(
            PreUpdate,
            collect_sharp_corrections.after(RollbackSystems::EndRollback),
        );
    }
    if let Some(tick) = competitor {
        app.insert_resource(CompetingClaim(tick));
        // AFTER the watchdog set, which `net::adoption::request_staged_adoption` runs before: the
        // competing claim must be unambiguously LATER than this module's own chance to claim, so
        // "nobody here asked for this rollback" is a fact about the schedule and not a race.
        app.add_systems(
            PreUpdate,
            claim_the_slot_for_someone_else
                .after(super::watchdog::RollbackWatchdog)
                .before(RollbackSystems::Check),
        );
    }
    crate::net::test_harness::finish(&mut app);

    spawn_client_link(&mut app, synced);

    // THE EPISODE, stamped with the tick replication carried it — which is `arrival`, at or after
    // the tick `visual.shock().tick` says it closed on.
    let mut confirmed_shock = ConfirmedHistory::<HullShock>::default();
    confirmed_shock.insert_present_explicit(arrival, visual.shock());
    let mut predicted_shock_history = PredictionHistory::<HullShock>::default();
    predicted_shock_history.add_predicted(PRODUCING_TICK, Some(HullShock::default()));

    // THE VALUE FOLLOWS THE TICK, and it has to: a confirmed sample stamped BEFORE the episode
    // closed is the hull as the authority had it before the impulse, which is the same un-hit state
    // the client predicted for itself. Stamping the post-hit velocity on a pre-hit tick would put a
    // history on the wire that no authority can produce, and would make "the restore carried
    // nothing" indistinguishable from delivery by value alone.
    let (confirmed_linear_value, confirmed_angular_value) =
        if velocities_confirmed_at.0 >= visual.shock().tick {
            (AUTHORITY_LINEAR, AUTHORITY_ANGULAR)
        } else {
            (PREDICTED_LINEAR, PREDICTED_ANGULAR)
        };
    let mut confirmed_linear = ConfirmedHistory::<LinearVelocity>::default();
    confirmed_linear.insert_present_explicit(
        velocities_confirmed_at,
        LinearVelocity(confirmed_linear_value),
    );
    let mut predicted_linear = PredictionHistory::<LinearVelocity>::default();
    predicted_linear.add_predicted(PRODUCING_TICK, Some(LinearVelocity(PREDICTED_LINEAR)));
    let mut confirmed_angular = ConfirmedHistory::<AngularVelocity>::default();
    confirmed_angular.insert_present_explicit(
        velocities_confirmed_at,
        AngularVelocity(confirmed_angular_value),
    );
    let mut predicted_angular = PredictionHistory::<AngularVelocity>::default();
    predicted_angular.add_predicted(PRODUCING_TICK, Some(AngularVelocity(PREDICTED_ANGULAR)));

    // THE POSE, and it is not decoration. `net::adoption`'s readiness gate requires that EVERY
    // authority-tracked part of the rigid body can be restored at the producing tick, and it fails
    // CLOSED on a missing `ConfirmedHistory` because that is the case in which lightyear's
    // `prepare_rollback` restores from the client's own prediction instead. A replicated, predicted
    // tank has all four histories; a fixture that omitted two would be offering the adoption path a
    // hull the shipping build never produces.
    let (authority_position, authority_rotation, live_position, live_rotation) =
        scenario_poses(render_error);
    let mut confirmed_position = ConfirmedHistory::<Position>::default();
    confirmed_position.insert_present_explicit(PRODUCING_TICK, Position(authority_position));
    let mut predicted_position = PredictionHistory::<Position>::default();
    predicted_position.add_predicted(PRODUCING_TICK, Some(Position(live_position)));
    let mut confirmed_rotation = ConfirmedHistory::<Rotation>::default();
    confirmed_rotation.insert_present_explicit(PRODUCING_TICK, Rotation(authority_rotation));
    let mut predicted_rotation = PredictionHistory::<Rotation>::default();
    predicted_rotation.add_predicted(PRODUCING_TICK, Some(Rotation(live_rotation)));

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
            Position(live_position),
            confirmed_position,
            predicted_position,
            Rotation(live_rotation),
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
        Transform {
            translation: live_position,
            rotation: live_rotation,
            ..default()
        },
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
    if render_error {
        arm_scenario_render_error(&mut app, root);
    }

    // The completed mutate tick: the authority has certified every replicated component through
    // `arrival`, the tick this checkpoint went out on. This is what receive would have published.
    {
        let mut checkpoints = app.world_mut().resource_mut::<ReplicationCheckpointMap>();
        checkpoints.record(producing_replicon_tick(), arrival);
        checkpoints.record_last_confirmed_tick(producing_replicon_tick());
    }

    advance_to(&mut app, lead.present_tick(arrival));
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
        advance_to(&mut app, arrival);
        app.world_mut().run_schedule(PreUpdate);
    }

    // The arrival frame is over. For an ISOLATED hit no spark can have been drawn for it yet — that
    // is the schedule, not a fixture convenience: `ImpactConfirm` is a message drained in `Update`,
    // so the earliest the ballistics march can present it is a later frame. This is what the player
    // would have felt if the shove were allowed to outrun its own spark. (For a COALESCED episode
    // the spark came first and the shove has correctly already landed by here.)
    let held_linear = app.world().get::<LinearVelocity>(root).unwrap().0;

    apply_late_mutations(
        &mut app,
        root,
        arrival,
        late_change,
        late_position_removal,
        late_rollback_disable,
    );

    if follow_up_episode {
        deposit_follow_up_episode(&mut app, root);
    }

    // The later frame, with the march's presentation in it — and the ticks that separate the two.
    if visual == Visual::DrawnAfterArrival {
        let present = app.world().resource::<LocalTimeline>().tick() + PRESENTATION_DELAY_TICKS;
        advance_to(&mut app, present);
        present_impact(&mut app, PRODUCING_TICK.0);
        app.world_mut().run_schedule(PreUpdate);
    }

    if follow_up_episode {
        run_follow_up_delivery(&mut app);
    }

    if visual == Visual::Missing {
        // Spend the ordering budget. A visual that has not been drawn by now is not coming, so the
        // shove must land anyway rather than be held for it forever.
        advance_to(
            &mut app,
            Tick(arrival.0.wrapping_add_signed(ORDERING_BUDGET_TICKS)),
        );
        app.world_mut().run_schedule(PreUpdate);
    }

    // Ticks the fixture spends deliberately past everything above, one `PreUpdate` each, to ask what
    // BOUNDS a wait rather than only that one happened.
    for _ in 0..extra_ticks {
        let next = app.world().resource::<LocalTimeline>().tick() + 1;
        advance_to(&mut app, next);
        app.world_mut().run_schedule(PreUpdate);
    }

    let ordering = app.world().resource::<ImpactPresentation>().tally();
    let still_staged = app.world().resource::<AuthorityAdoption>().is_staged();
    // A restore can RESOLVE AN AUTHORITATIVE REMOVAL, and `prepare_rollback` answers one by taking
    // the component off the hull — so "the velocity is gone" is a state a fixture must be able to
    // report rather than panic on. `Vec3::NAN` compares unequal to everything, so every existing
    // `assert_eq!` on these still fails, and it fails with its own message instead of an `unwrap`.
    let live_velocity_removed = app.world().get::<LinearVelocity>(root).is_none()
        || app.world().get::<AngularVelocity>(root).is_none();
    let live_pose_removed =
        app.world().get::<Position>(root).is_none() || app.world().get::<Rotation>(root).is_none();
    // `arrival` is the confirmed `HullShock` sample's tick, which IS `AuthoritativeFact::produced_at`
    // and the restore target the age is measured against.
    let final_age = app.world().resource::<LocalTimeline>().tick() - arrival;
    let live_linear = app
        .world()
        .get::<LinearVelocity>(root)
        .map_or(Vec3::NAN, |velocity| velocity.0);
    let live_angular = app
        .world()
        .get::<AngularVelocity>(root)
        .map_or(Vec3::NAN, |velocity| velocity.0);
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
    delivered.forced_tick_after = app
        .world()
        .resource::<StateRollbackMetadata>()
        .forced_rollback_tick();
    delivered.ordering = ordering;
    delivered.still_staged = still_staged;
    delivered.live_velocity_removed = live_velocity_removed;
    delivered.live_pose_removed = live_pose_removed;
    delivered.final_age = final_age;
    delivered.hull = Some(root);
    delivered.render_offset = app
        .world()
        .get::<super::render_error::RenderErrorOffset>(root)
        .map(|offset| (offset.translation, offset.rotation));
    delivered.duplicate_visual_correction = app
        .world()
        .get::<VisualCorrection<Position>>(root)
        .is_some()
        || app
            .world()
            .get::<VisualCorrection<Rotation>>(root)
            .is_some();
    delivered
}

/// The contention deposit: while the head is staged and holding, a newer episode's confirmed
/// samples and checkpoint land. From here every offer of the newer fact answers `SlotBusy` until
/// the head's transaction closes. The head is UNAFFECTED: its readiness and restore read the
/// newest samples at or before ITS tick, and the newer checkpoint still certifies it.
fn deposit_follow_up_episode(app: &mut App, root: Entity) {
    let mut shock = app
        .world_mut()
        .get_mut::<ConfirmedHistory<HullShock>>(root)
        .expect("the hull's confirmed shock history");
    shock.insert_present_explicit(FOLLOW_UP_TICK, follow_up_shock());
    let mut linear = app
        .world_mut()
        .get_mut::<ConfirmedHistory<LinearVelocity>>(root)
        .expect("the hull's confirmed linear history");
    linear.insert_present_explicit(FOLLOW_UP_TICK, LinearVelocity(FOLLOW_UP_LINEAR));
    let mut angular = app
        .world_mut()
        .get_mut::<ConfirmedHistory<AngularVelocity>>(root)
        .expect("the hull's confirmed angular history");
    angular.insert_present_explicit(FOLLOW_UP_TICK, AngularVelocity(FOLLOW_UP_ANGULAR));
    let mut checkpoints = app.world_mut().resource_mut::<ReplicationCheckpointMap>();
    checkpoints.record(RepliconTick::new(51), FOLLOW_UP_TICK);
    checkpoints.record_last_confirmed_tick(RepliconTick::new(51));
}

/// The contention delivery: the head has retired, the slot is free. One frame past the newer
/// episode's tick it stages and holds for its own spark; the spark draws; the next frame adopts.
fn run_follow_up_delivery(app: &mut App) {
    advance_to(app, FOLLOW_UP_TICK + 1);
    app.world_mut().run_schedule(PreUpdate);
    present_impact(app, FOLLOW_UP_TICK.0);
    let next = app.world().resource::<LocalTimeline>().tick() + 1;
    advance_to(app, next);
    app.world_mut().run_schedule(PreUpdate);
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

/// THE SINGLE STAGING SLOT UNDER CONTENTION — the backpressure case the inert comparator makes
/// load-bearing. A second episode arrives while the head is staged and HELD for its spark; every
/// offer of the newer fact answers `SlotBusy` until the head's transaction closes; the newer fact
/// then stages, holds for its OWN spark, and is adopted with the SECOND episode's velocities. The
/// old native trigger would have delivered the newer episode early and unordered; without it, a
/// slot that turned backpressure into loss would fail exactly here.
#[test]
fn a_newer_fact_behind_a_held_head_is_adopted_after_the_head_releases() {
    let delivered = run_scenario(Scenario {
        follow_up_episode: true,
        ..Scenario::new(Lead::Zero, Visual::DrawnAfterArrival)
    });

    assert_eq!(
        delivered.live_linear, FOLLOW_UP_LINEAR,
        "the LIVE hull must end on the SECOND episode's velocities — ending on the head's means \
         the newer fact was never delivered",
    );
    assert_eq!(delivered.live_angular, FOLLOW_UP_ANGULAR);
    assert_eq!(
        delivered.realized_count,
        follow_up_shock().count,
        "the ledger must have realized BOTH episodes",
    );
    assert_eq!(
        delivered.ordering,
        OrderingTally {
            released_on_impact: 2,
            // The head's spark cost the schedule's own delay; the newer fact's wait spans its
            // SlotBusy frames plus its own spark hold.
            max_wait_ticks: delivered.ordering.max_wait_ticks,
            ..default()
        },
        "both facts must release on their own impacts — no bypass, no budget, no loss",
    );
    assert_eq!(
        delivered.sharp.len(),
        2,
        "each adoption must emit its own sharp correction, got {:?}",
        delivered.sharp,
    );
    assert_eq!(
        delivered.forced_tick_after, None,
        "no claim may outlive the run",
    );
}

/// THE SAME-FRAME CONSUMPTION INVARIANT, pinned. `request_staged_adoption` claims the forced slot
/// and lightyear's `check_rollback` consumes it later in the SAME `PreUpdate`, reading the same
/// `LocalTimeline` — which is why the replay-window arithmetic on the two sides can never
/// disagree, and why a rejected forced request (which would suppress every native policy check
/// for its frame, rollback.rs consumes the flag before the policy branches) is unreachable in the
/// shipping composition. Nothing enforces that but the schedule: this fails if the claim ever
/// outlives the `PreUpdate` that made it — a dependency bump moving the consumption, a gate
/// diverging between the two, or a re-registration losing the `.before(RollbackSystems::Check)`.
#[test]
fn the_forced_request_is_consumed_in_the_same_preupdate_that_claims_it() {
    let delivered = run_arrival(Lead::Zero, Visual::DrawnAfterArrival);

    assert_eq!(
        delivered.live_linear, AUTHORITY_LINEAR,
        "the run must actually exercise a claim — an undelivered shove means no request was made \
         and the invariant was vacuously true",
    );
    assert_eq!(
        delivered.forced_tick_after, None,
        "a forced-rollback claim survived past the end of the run: the same-PreUpdate consumption \
         invariant is broken, and a claim judged on a later frame's clock can be rejected as \
         outside the replay window — which silently suppresses that frame's native policy checks",
    );
}

/// The connected-but-not-yet-synced window, held closed. Pre-sync, lightyear's `check_rollback`
/// skips on its `Single<…, With<IsSynced<InputTimeline>>>` while the forced slot still accepts
/// claims — and the first sync then rewrites `LocalTimeline` in `PostUpdate`, so a pre-sync claim
/// would be judged next frame against a clock it was never made under. `request_staged_adoption`
/// carries the same `IsSynced` gate as the watchdog precisely so no claim can enter that window:
/// the fact stays staged and untallied until the timeline it would be measured against exists.
#[test]
fn a_claim_made_before_the_input_timeline_syncs_is_never_stranded() {
    let delivered = run_scenario(Scenario {
        synced: false,
        ..Scenario::new(Lead::Zero, Visual::DrawnBeforeArrival)
    });

    assert_eq!(
        delivered.forced_tick_after, None,
        "adoption claimed the forced slot on an unsynced link — nothing consumes it there, and \
         the first sync's timeline rewrite turns it into a rejected request that suppresses a \
         frame's native policy checks",
    );
    assert!(
        delivered.still_staged,
        "the fact must WAIT for sync, not be spent or dropped: its ticks only mean something on \
         the synced timeline",
    );
    assert_eq!(
        delivered.ordering,
        OrderingTally::default(),
        "no ordering verdict may be latched on a frame the request could not have been made — the \
         tally would report a wait the player never got",
    );
    assert_eq!(
        delivered.live_linear, LIVE_LINEAR,
        "no rollback ran, so the live hull velocity must be untouched",
    );
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

#[test]
fn a_real_age_zero_adoption_keeps_its_differing_pose_sharp_through_render_capture() {
    let delivered = run_scenario(Scenario {
        render_error: true,
        ..Scenario::new(Lead::Zero, Visual::DrawnBeforeArrival)
    });

    assert_eq!(
        delivered.replayed_ticks,
        Vec::<u32>::new(),
        "the integration must remain the age-zero adoption path",
    );
    assert_eq!(delivered.live_linear, AUTHORITY_LINEAR);
    assert_eq!(
        delivered.render_offset,
        Some((Vec3::ZERO, Quat::IDENTITY)),
        "the real adoption retirement must emit the sharp occurrence that capture consumes; the \
         deliberately differing authority pose may not accumulate any compensating offset",
    );
    assert!(
        !delivered.duplicate_visual_correction,
        "capture must consume the one-shot correction inputs before Lightyear can retain a \
         duplicate visual correction",
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
    assert_eq!(
        delivered.sharp,
        vec![delivered.hull.expect("the run built a hull")],
        "an ADOPTED fact must also tell the view to keep the seam sharp — exactly once, naming this \
         hull. `net::render_error` refuses to smooth the correction it names, which is the whole \
         reason the shove is visible on the frame it lands.",
    );
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

/// THE OTHER HALF OF THE SLICE-3.7 FINDING, at the schedule rather than at the predicate: a fact
/// whose restore CANNOT carry the shove must never be requested, and must never be recorded as
/// delivered.
///
/// The authority's newest confirmed hull velocity here predates the episode's close, so
/// `prepare_rollback` at the producing tick would resolve a PRE-hit value. Lightyear would restore it
/// happily — `get_state_at_or_before` finds the older sample and `authority_reaches` says yes — and
/// the earlier rule then closed the fact as `Adopted` off its own installed claim without asking what
/// the restore had actually put on the hull. The shove was lost in silence, permanently, on the
/// success path.
///
/// What must happen instead: nothing. No rollback is ordered (the hull keeps the value it was
/// predicting), no ordering verdict is reached, and above all no counter moves — a fact that was
/// never requested cannot be `undelivered`, and the episode stays offerable if the authority ever
/// confirms a velocity that reaches it.
#[test]
fn a_fact_whose_restore_cannot_carry_the_shove_is_never_requested() {
    let delivered = run_scenario(Scenario {
        velocities_confirmed_at: Tick(PRODUCING_TICK.0 - 4),
        ..Scenario::new(Lead::Zero, Visual::Missing)
    });

    assert_eq!(
        delivered.live_linear, LIVE_LINEAR,
        "the client's own hull state must be left alone: a forced rollback here would install the \
         authority's PRE-hit velocity under the producing tick's label, which is a render hitch \
         that delivers nothing. Evidence: live shock = {:?}, ticks replayed = {:?}",
        delivered.live_shock, delivered.replayed_ticks,
    );
    assert_eq!(delivered.live_angular, LIVE_ANGULAR);
    assert_eq!(
        delivered.ordering,
        OrderingTally::default(),
        "and NOTHING may be tallied. `released_on_budget` would mean the ordering rule spent a fact \
         that was never deliverable; `undelivered` would mean we asked for a rollback we could have \
         known was useless. The whole point of establishing delivery BEFORE requesting is that this \
         line reads all zeroes.",
    );
    assert!(
        delivered.sharp.is_empty(),
        "and NOTHING may be told to render sharp. Every retirement here is `Keep`: no rollback \
         carried this fact, so any correction on screen is ordinary misprediction and smoothing it \
         hides nothing the player is owed. Emitted: {:?}",
        delivered.sharp,
    );
}

/// FINDING THE SLICE-3.7 REVIEW CAUGHT: `OrderingTally::bypassed` claims a shove LANDED, and until
/// now nothing executed the restore that would have landed it. The unit fixtures in `net::adoption`
/// hand `confirm_forced_rollback` a rollback tick and read the counter, which proves the counter
/// follows the classifier — not that the hull moved.
///
/// This runs lightyear's own `RollbackSystems::Prepare` and reads the LIVE hull velocity afterwards.
/// The rollback is ordered by somebody else through the same `ForcedRollbackSlot::claim` the
/// watchdog uses, tagged `Misprediction`, while `net::adoption` is still holding the fact for a spark
/// that never comes. That is the hole the ADR documents, and this is its size measured on a real
/// restore.
///
/// AND IT IS THE SHAPE THE OLD CLASSIFIER UNDERCOUNTED. The episode SETTLED at
/// [`PRODUCING_TICK`] and replication materialized it [`REPLICATION_LAG_TICKS`] later, so the
/// confirmed `HullShock` sample — `AuthoritativeFact::produced_at`, and the restore target — sits at
/// 104 while the velocities the restore resolves are the authority's tick-100 samples.
/// `prepare_rollback` restores `get_state_at_or_before(104)`, which IS those samples, so the shove
/// really is on the live hull; a classifier demanding the sample be at or after 104 called that
/// nothing and left `bypassed` at zero. Both assertions below have to hold together — a live hull
/// carrying the authority's velocity while the counter reads zero is precisely the dishonest state
/// the counter exists to prevent.
#[test]
fn a_rollback_this_module_did_not_order_delivers_the_shove_and_is_counted() {
    let arrival = Tick(PRODUCING_TICK.0 + REPLICATION_LAG_TICKS);
    let delivered = run_scenario(Scenario {
        arrival,
        competitor: Some(arrival),
        ..Scenario::new(Lead::Zero, Visual::Missing)
    });

    assert_eq!(
        delivered.held_linear, AUTHORITY_LINEAR,
        "`prepare_rollback` restored the hull from the authority's confirmed velocities on the \
         ARRIVAL frame — before the ordering rule released anything. The restore is real: this is \
         the live component after lightyear's own Prepare seam, not a predicate's opinion of it. \
         Evidence: live shock = {:?}, ticks replayed = {:?}",
        delivered.live_shock, delivered.replayed_ticks,
    );
    assert_eq!(delivered.live_linear, AUTHORITY_LINEAR);
    assert_eq!(delivered.live_angular, AUTHORITY_ANGULAR);
    assert_eq!(
        delivered.ordering,
        OrderingTally {
            bypassed: 1,
            ..default()
        },
        "the shove landed through a rollback this module did not order, while the fact was still \
         waiting for its spark — one BYPASS, and nothing else. `released_on_budget` staying zero is \
         the other half: the fact was spent by the bypass, so the budget never had to release it, \
         and it was not re-requested for state already live.",
    );
    assert_eq!(
        delivered.sharp,
        vec![delivered.hull.expect("the run built a hull")],
        "AND THE VIEW MUST BE TOLD, which is the case a reader of the cause tag gets backwards. \
         The slot was claimed by somebody else and tagged `Misprediction`, so the tag says \
         'hide this seam' while the restore put the authority's post-hit velocity on the live hull. \
         The signal is derived from the RETIREMENT — `Delivered` — and not from the tag. \
         Emitted: {:?}",
        delivered.sharp,
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

/// FINDING THE SLICE-3.9 REVIEW CAUGHT: READINESS WAS PROVEN AT STAGING AND ACTED ON LATER.
///
/// Every other fixture in this file — and every unit fixture in `net::adoption` — builds a confirmed
/// history during setup and never touches it again, so "the offer's gate settled this" and "the
/// request's own answer" were the same sentence and no test could tell them apart. That is why the
/// defect survived five reviews.
///
/// THE SHAPE. The arrival frame stages the fact: the authority's velocities are confirmed at the
/// episode's close, so a restore at the producing tick would carry the shove and the offer's gate
/// says yes. The ordering rule then holds it for a spark that never comes ([`Visual::Missing`]) — up
/// to `ORDERING_BUDGET_TICKS` LOCAL ticks, which is many frames. In between, replication moves the
/// history: a newer sample lands (stored as an unchanged marker, because its value matches) and then
/// an authoritative REMOVAL arrives stamped BEFORE it, middle-inserting between the episode's close
/// and the restore target. Both writes go through lightyear's own `ConfirmedHistory` API, so this
/// exercises its real sorted middle insertion and its dynamic `SameAsPrecedent` resolution.
///
/// WHAT `prepare_rollback` WOULD NOW DO: the restore target is the confirmed `HullShock` sample's
/// tick — [`PRODUCING_TICK`] + [`REPLICATION_LAG_TICKS`] = 104, NOT the episode's close at 100 — and
/// `get_state_at_or_before(104)` lands on the marker at 103 and resolves back through it to the
/// removal at 102. So the restore would take `LinearVelocity` off the hull rather than install a
/// velocity. The shove is not merely stale — it is not there.
///
/// WHAT MUST HAPPEN: the request revalidates and WAITS. The fact stays staged, nothing is claimed,
/// the hull keeps the value it was predicting, and no counter moves — not `undelivered` (we asked
/// for a rollback we could have known was useless), not `released_on_budget` (the ordering rule
/// spent a fact it could not deliver), not `bypassed`.
///
/// Before the fix this ran to the budget, claimed the slot on the staging-time answer, and spent the
/// fact permanently on a restore that carried nothing.
#[test]
fn a_late_replicated_change_is_revalidated_before_the_request() {
    let delivered = run_scenario(Scenario {
        arrival: Tick(PRODUCING_TICK.0 + REPLICATION_LAG_TICKS),
        late_change: Some(a_removal_the_restore_target_resolves()),
        ..Scenario::new(Lead::Zero, Visual::Missing)
    });

    assert!(
        !delivered.live_velocity_removed,
        "the restore this frame would order resolves an authoritative REMOVAL, so ordering it takes \
         `LinearVelocity` OFF the hull entirely — the hull ends the run without a velocity at all. \
         Evidence: live shock = {:?}, ticks replayed = {:?}",
        delivered.live_shock, delivered.replayed_ticks,
    );
    assert_eq!(
        delivered.live_linear, LIVE_LINEAR,
        "and the client's own hull state must be left alone otherwise: nothing may be installed on \
         the strength of a readiness answer that stopped being true while the fact waited",
    );
    assert_eq!(delivered.live_angular, LIVE_ANGULAR);
    assert_eq!(
        delivered.ordering,
        OrderingTally::default(),
        "and NOTHING may be tallied. The readiness answer this fact was staged on stopped being \
         true while it waited, and acting on it is what spends a fact on a rollback that carries \
         nothing.",
    );
    assert!(
        delivered.still_staged,
        "a failed revalidation is a WAIT, not a drop: the answer is expected to change, so the fact \
         must stay staged and be reconsidered rather than be closed and deduped forever",
    );
}

/// THE OTHER HALF OF THAT RULE: the wait is BOUNDED, and by something that says so out loud.
///
/// The same run, carried to `RollbackPolicy::max_rollback_ticks` and then one tick past it. A fact
/// whose restore never becomes deliverable would otherwise sit in the single staging slot forever,
/// blocking every later fact on every hull — a wait with no bound is its own defect, and "it stays
/// staged" is only the right answer while something else ends it. The replay-window check runs first
/// in `request_staged_adoption` every frame and closes the fact with a WARN once no replay could
/// reach its producing tick.
///
/// BOTH SIDES OF THE BOUNDARY, because only one of them is the claim this fixture's name makes. An
/// earlier version observed the fact staged at age {`ORDERING_BUDGET_TICKS`} and closed at age
/// `window` + 1, and a rule that gave up anywhere in between would have passed it just as happily —
/// which is a bound but not THIS bound, and "waits exactly as long as a replay could still reach
/// the tick" is the whole reason the check is a window and not a timeout. So the run is done twice,
/// one tick apart, and the ages are asserted rather than assumed.
#[test]
fn a_revalidation_that_never_passes_is_dropped_at_the_replay_window() {
    let window = i32::from(
        crate::net::test_harness::prediction_manager()
            .rollback_policy
            .max_rollback_ticks,
    );
    // The `Visual::Missing` run already spends the ordering budget, so the fact's age when that run
    // ends is exactly the budget; every extra tick past it adds one to the age.
    let carried_to = |age: i32| {
        run_scenario(Scenario {
            arrival: Tick(PRODUCING_TICK.0 + REPLICATION_LAG_TICKS),
            late_change: Some(a_removal_the_restore_target_resolves()),
            extra_ticks: age - ORDERING_BUDGET_TICKS,
            ..Scenario::new(Lead::Zero, Visual::Missing)
        })
    };

    let inside = carried_to(window);
    assert_eq!(
        inside.final_age, window,
        "the fixture must actually stand ON the boundary, or the assertion below pins nothing",
    );
    assert!(
        inside.still_staged,
        "at age exactly {window} a replay CAN still reach the producing tick, so the fact must \
         still be staged. Giving up here would be a timeout wearing the window's name.",
    );
    assert_eq!(
        inside.ordering,
        OrderingTally::default(),
        "and nothing may have been tallied on the way to the boundary either",
    );

    let outside = carried_to(window + 1);
    assert_eq!(outside.final_age, window + 1);
    assert!(
        !outside.still_staged,
        "one tick further, past the {window}-tick replay window, no rollback could reach the \
         producing tick — so the fact must be closed rather than retried forever. That is what \
         stops the wait from being an unbounded stall on the single staging slot, and the pair of \
         runs is what pins the give-up to THIS tick rather than to some tick.",
    );
    assert_eq!(
        outside.live_linear, LIVE_LINEAR,
        "and it must be dropped, not adopted: nothing may be installed on the hull on the way out",
    );
    assert_eq!(
        outside.ordering,
        OrderingTally::default(),
        "a fact that was never requested cannot be `undelivered`, and one the ordering rule never \
         released cannot be counted against the budget",
    );
}

/// FINDING THE SLICE-3.10 REVIEW CAUGHT: THE REVALIDATION COVERED HALF THE PREDICATE.
///
/// [`a_late_replicated_change_is_revalidated_before_the_request`] closed the stale-readiness hole
/// for the two VELOCITY histories. The offer proves more than that — it also proves the hull's
/// `Position` and `Rotation` can be restored at the producing tick, because a pose restored to one
/// tick beside a velocity left at another is not a state either peer ever had. The request re-proved
/// only the velocities, so half the readiness answer was still being acted on frames after it was
/// taken.
///
/// THE SHAPE, and it is the same one, moved to the other half of the rigid body. The arrival frame
/// stages the fact with all four histories restorable. While the ordering rule holds it for a spark
/// that never comes, an authoritative REMOVAL of `Position` middle-inserts between the episode's
/// close and the restore target. The offer pass now correctly skips the hull — and that changes
/// nothing, because re-offering never touches an already-staged fact. The velocity-only
/// revalidation still said yes, the slot was claimed, and `prepare_rollback` answered the removal by
/// taking `Position` OFF the hull. Then `confirm_forced_rollback` asked only whether the VELOCITIES
/// were carried, which they were, and closed the fact as `Retirement::Adopted`.
///
/// SO NOTHING FIRED. Not `undelivered` — that counter is defined on the velocity predicate, which
/// passed. Not `bypassed`. The hull ended the frame without a position and the module recorded a
/// success. That is precisely the silent-loss shape the whole arc exists to remove, which is why the
/// fix is the predicate at the request and not another counter.
///
/// WHAT MUST HAPPEN: the same WAIT. Nothing claimed, nothing tallied, the pose intact, and the fact
/// still staged to be reconsidered when the authority confirms a pose there again.
#[test]
fn a_late_pose_removal_is_revalidated_before_the_request() {
    let delivered = run_scenario(Scenario {
        arrival: Tick(PRODUCING_TICK.0 + REPLICATION_LAG_TICKS),
        // Between the episode's close and the restore target, so `get_state_at_or_before(104)`
        // resolves it exactly as the velocity fixture's removal is resolved.
        late_position_removal: Some(Tick(PRODUCING_TICK.0 + 2)),
        ..Scenario::new(Lead::Zero, Visual::Missing)
    });

    assert!(
        !delivered.live_pose_removed,
        "the restore this frame would order resolves an authoritative REMOVAL of `Position`, so \
         ordering it deletes half the hull's rigid body — and every velocity-shaped piece of \
         evidence would still read as a clean delivery. Evidence: live shock = {:?}, ticks \
         replayed = {:?}",
        delivered.live_shock, delivered.replayed_ticks,
    );
    assert!(
        !delivered.live_velocity_removed,
        "and the velocities must be untouched too — this fixture must fail on the POSE or it is \
         re-testing the sibling above",
    );
    assert_eq!(
        delivered.live_linear, LIVE_LINEAR,
        "nothing may be installed on the strength of a readiness answer that covered only the \
         components the fact happens to deliver",
    );
    assert_eq!(delivered.live_angular, LIVE_ANGULAR);
    assert_eq!(
        delivered.ordering,
        OrderingTally::default(),
        "and NOTHING may be tallied. Before the fix this line read `released_on_budget: 1` and the \
         fact closed as ADOPTED, because both velocities were genuinely carried — the counters \
         cannot see a pose that was deleted beside them.",
    );
    assert!(
        delivered.still_staged,
        "a failed revalidation is a WAIT, not a drop, on this half of the predicate too: the \
         authority can confirm a pose at the target again, and the replay window is what ends the \
         wait if it never does",
    );
}

/// FINDING THE SLICE-3.11 REVIEW CAUGHT: THE HULL'S PARTICIPATION IN `Prepare` WAS LATCHED AT THE
/// OFFER.
///
/// The two fixtures above move a confirmed HISTORY under a staged fact. This one moves the hull's
/// ARCHETYPE, which is the other way the offer's answer stops being true — and it was the way with
/// no revalidation at all. `offer_hull_shock_adoptions` filtered `Without<DisableRollback>` and
/// neither the request nor the post-`Prepare` proof re-established it.
///
/// THE PRODUCTION CHAIN, which is why this is a defect and not a hypothetical. `net::rig`'s
/// `upgrade_predicted_to_dynamic` inserts `DisableRollback` in `Update` when a late `Predicted`
/// marker promotes an already-attached rig; `enable_rollback_after_first_tick` removes it in
/// `FixedLast`. Bevy runs `RunFixedMainLoop` before `Update`, so the remover cannot run until the
/// NEXT frame's fixed loop — after that frame's `PreUpdate`. A fact staged while the hull was
/// eligible is therefore requested, at least once, on a hull that is not.
///
/// WHAT LIGHTYEAR THEN DOES: `prepare_rollback`'s query is filtered `Without<DisableRollback>`, so
/// the hull is skipped for EVERY component. Not "restored to a stale value" — not restored at all.
///
/// WHAT THE MODULE USED TO DO: claim the slot (the request never asked), let the rollback install,
/// and then compute delivery from `ConfirmedHistory` — a lookup answering "what WOULD a restore
/// resolve here", which is unchanged by the hull having been skipped. `carried` came back true and
/// the fact closed as `Retirement::Adopted`. Nothing was delivered, nothing was counted, and the
/// success path recorded a success.
///
/// WHAT MUST HAPPEN: the same WAIT the history fixtures get. Nothing claimed, nothing tallied, the
/// hull untouched, and the fact still staged — the marker is removed on the next fixed tick, so this
/// is a wait that production actually ends, and the replay window ends it if production does not.
///
/// AND IF A FUTURE EDIT DOES CLAIM ANYWAY, the outcome must be `Retirement::Undelivered` — loud and
/// counted — never `Adopted`. `undelivered` staying zero here is asserted for the first reason and
/// would be a PASS for the second; what makes the two distinguishable is `still_staged`.
#[test]
fn a_hull_excluded_from_prepare_is_never_requested_and_never_adopted() {
    let delivered = run_scenario(Scenario {
        arrival: Tick(PRODUCING_TICK.0 + REPLICATION_LAG_TICKS),
        late_rollback_disable: true,
        ..Scenario::new(Lead::Zero, Visual::Missing)
    });

    assert!(
        delivered.still_staged,
        "the fact was CLOSED. `prepare_rollback` cannot touch a hull carrying `DisableRollback`, so \
         whatever closed it recorded a delivery that did not happen — and if it closed as \
         `Adopted`, silently. Evidence: ordering = {:?}, live shock = {:?}, ticks replayed = {:?}",
        delivered.ordering, delivered.live_shock, delivered.replayed_ticks,
    );
    assert_eq!(
        delivered.ordering,
        OrderingTally::default(),
        "and NOTHING may be tallied. `released_on_budget` would mean the ordering rule spent a fact \
         on a rollback that could not reach its hull; `bypassed` would mean somebody else's \
         rollback delivered a shove to an entity it was filtered out of.",
    );
    assert_eq!(
        delivered.live_linear, LIVE_LINEAR,
        "and the hull keeps exactly what it was predicting — which is the honest reading of a \
         rollback that skipped it, and is what every counter above must agree with",
    );
    assert_eq!(delivered.live_angular, LIVE_ANGULAR);
    assert!(
        !delivered.live_velocity_removed && !delivered.live_pose_removed,
        "the hull must still HAVE its rigid body: an excluded hull is untouched, not stripped",
    );
    assert!(
        delivered.sharp.is_empty(),
        "and the view must NOT be told to keep anything sharp. A rollback that skipped the hull \
         delivered no hit, so refusing to smooth its correction would expose a seam for nothing. \
         Emitted: {:?}",
        delivered.sharp,
    );
}

/// The pose the authority holds at the restore target, and what the live hull is doing instead.
/// Distinct from each other and from `default()`, so "restored" cannot be satisfied by a component
/// that was simply never written.
const AUTHORITY_POSITION: Vec3 = Vec3::new(11.0, 12.0, 13.0);
const LIVE_POSITION: Vec3 = Vec3::new(-7.0, -8.0, -9.0);
fn authority_rotation() -> Quat {
    Quat::from_rotation_y(0.75)
}
fn live_rotation() -> Quat {
    Quat::from_rotation_x(-0.4)
}

/// One cell of the participation matrix: build a client, put the hull in the archetype the cell
/// describes, order a real forced rollback, and report which of the four rigid-body components
/// `prepare_rollback` ACTUALLY replaced with the authority's value.
///
/// The rollback is claimed through [`ForcedRollbackSlot::claim`] — the call `net::watchdog` makes —
/// so this is lightyear's own forced-rollback path and not a hand-installed rollback tick. The
/// target is the client's current tick, which makes the replay loop run zero times: everything read
/// afterwards was written by `prepare_rollback` itself and by nothing downstream of it.
///
/// The order of the returned flags is `Position`, `Rotation`, `LinearVelocity`, `AngularVelocity` —
/// the order `net::adoption::prepare_restores` takes them in.
fn components_prepare_restored(excluded: bool, histories: [bool; 4]) -> [bool; 4] {
    let mut app = crate::net::test_harness::base_app();
    app.add_plugins(ClientPlugins {
        tick_duration: crate::net::test_harness::TICK,
    });
    crate::state::sim_plugin(&mut app);
    super::protocol::plugin(&mut app);
    app.insert_state(crate::state::AppState::Playing);
    app.insert_resource(CompetingClaim(PRODUCING_TICK));
    app.add_systems(
        PreUpdate,
        claim_the_slot_for_someone_else
            .after(super::watchdog::RollbackWatchdog)
            .before(RollbackSystems::Check),
    );
    crate::net::test_harness::finish(&mut app);

    app.world_mut().spawn((
        Client::default(),
        RemoteId(PeerId::Server),
        Connected,
        crate::net::test_harness::prediction_manager(),
        IsSynced::<InputTimeline>::default(),
    ));

    let mut confirmed_position = ConfirmedHistory::<Position>::default();
    confirmed_position.insert_present_explicit(PRODUCING_TICK, Position(AUTHORITY_POSITION));
    let mut confirmed_rotation = ConfirmedHistory::<Rotation>::default();
    confirmed_rotation.insert_present_explicit(PRODUCING_TICK, Rotation(authority_rotation()));
    let mut confirmed_linear = ConfirmedHistory::<LinearVelocity>::default();
    confirmed_linear.insert_present_explicit(PRODUCING_TICK, LinearVelocity(AUTHORITY_LINEAR));
    let mut confirmed_angular = ConfirmedHistory::<AngularVelocity>::default();
    confirmed_angular.insert_present_explicit(PRODUCING_TICK, AngularVelocity(AUTHORITY_ANGULAR));

    let hull = app
        .world_mut()
        .spawn((
            Predicted,
            Remote,
            ConfirmHistory::new(producing_replicon_tick()),
            Tank,
            VICTIM,
            Position(LIVE_POSITION),
            Rotation(live_rotation()),
            LinearVelocity(LIVE_LINEAR),
            AngularVelocity(LIVE_ANGULAR),
            confirmed_position,
            confirmed_rotation,
            confirmed_linear,
            confirmed_angular,
        ))
        .id();
    app.world_mut().entity_mut(hull).insert((
        Transform::default(),
        RigidBody::Dynamic,
        Mass(HULL_MASS),
        AngularInertia::new(Vec3::splat(HULL_INERTIA)),
        CenterOfMass(Vec3::ZERO),
        NoAutoMass,
        NoAutoAngularInertia,
        NoAutoCenterOfMass,
        GravityScale(0.0),
    ));

    app.world_mut().flush();

    // THE CELL, applied by REMOVAL rather than by omission — and that is a property of lightyear,
    // not a fixture style. `add_prediction_history::<C>` is an OBSERVER on `Add` of `C` or
    // `Predicted`, so every predicted component gets its buffer the instant the hull is spawned; a
    // cell built by not inserting one would silently be the all-present cell. (It is also why the
    // audit table records the per-component half of this condition as never having been a live
    // defect: nothing removes the buffer on a surviving entity.)
    let mut hull_mut = app.world_mut().entity_mut(hull);
    if !histories[0] {
        hull_mut.remove::<PredictionHistory<Position>>();
    }
    if !histories[1] {
        hull_mut.remove::<PredictionHistory<Rotation>>();
    }
    if !histories[2] {
        hull_mut.remove::<PredictionHistory<LinearVelocity>>();
    }
    if !histories[3] {
        hull_mut.remove::<PredictionHistory<AngularVelocity>>();
    }
    if excluded {
        hull_mut.insert(DisableRollback);
    }
    app.world_mut().flush();

    // The archetype the cell asked for is the archetype the hull has. Without this a removal that
    // stopped working would turn half the matrix into copies of the all-present cell, silently.
    let built = [
        app.world()
            .get::<PredictionHistory<Position>>(hull)
            .is_some(),
        app.world()
            .get::<PredictionHistory<Rotation>>(hull)
            .is_some(),
        app.world()
            .get::<PredictionHistory<LinearVelocity>>(hull)
            .is_some(),
        app.world()
            .get::<PredictionHistory<AngularVelocity>>(hull)
            .is_some(),
    ];
    assert_eq!(
        built, histories,
        "the fixture failed to build the archetype it is testing: `PredictionHistory` presence is \
         {built:?} where the cell asked for {histories:?}",
    );
    assert_eq!(
        app.world().get::<DisableRollback>(hull).is_some(),
        excluded,
        "and the whole-entity half of the cell has to be real too",
    );

    {
        let mut checkpoints = app.world_mut().resource_mut::<ReplicationCheckpointMap>();
        checkpoints.record(producing_replicon_tick(), PRODUCING_TICK);
        checkpoints.record_last_confirmed_tick(producing_replicon_tick());
    }

    advance_to(&mut app, PRODUCING_TICK);
    app.world_mut().run_schedule(PreUpdate);

    let world = app.world();
    [
        world
            .get::<Position>(hull)
            .is_some_and(|value| value.0 == AUTHORITY_POSITION),
        world
            .get::<Rotation>(hull)
            .is_some_and(|value| value.0 == authority_rotation()),
        world
            .get::<LinearVelocity>(hull)
            .is_some_and(|value| value.0 == AUTHORITY_LINEAR),
        world
            .get::<AngularVelocity>(hull)
            .is_some_and(|value| value.0 == AUTHORITY_ANGULAR),
    ]
}

/// THE RUNTIME CONFORMANCE MATRIX, and the answer to the concern the slice-3.11 handoff filed
/// against itself: `net::adoption::prepare_restores` MIRRORS lightyear's query rather than observing
/// its effect, so a lightyear change that keeps the archetype and changes the restore would pass it
/// silently.
///
/// This makes the mirror a CHECKED contract. Over `DisableRollback` present/absent × each of the
/// four `PredictionHistory<C>` present/absent — 32 archetypes — it runs lightyear's own
/// `RollbackSystems::Prepare` and asserts two things per cell:
///
/// 1. exactly which components the restore replaced with the authority's value, per component;
/// 2. that `prepare_restores`' whole-body verdict is the conjunction of those four answers.
///
/// The second is what the module actually consumes; the first is what makes a failure diagnosable
/// and is strictly stronger, because it pins WHICH condition governs WHICH component rather than
/// only that some condition governs something.
///
/// WHAT IT WOULD HAVE CAUGHT: the seventh review's High, from the other side. That defect was a
/// participation condition asked in one place and not the other two — and this fixture is what says
/// the condition is real and exactly these two, so a module that stopped asking it produces a
/// visible disagreement between the predicate and the restore rather than an argument.
///
/// IT CANNOT PASS VACUOUSLY. The cell `(excluded: false, all four histories present)` demands all
/// four components carry the authority's value, so a run in which no rollback installed at all fails
/// immediately; and the live values are distinct from the authority's and from `default()`, so
/// "restored" cannot be satisfied by a component that was never written.
///
/// WHAT IT IS NOT: a claim about what `prepare_rollback` restores components FROM. That is
/// `restore_carries_the_shove`'s question and has its own fixtures. This one is only about
/// membership — who is in the restore.
#[test]
fn prepare_restores_exactly_the_components_the_predicate_names() {
    for excluded in [false, true] {
        for mask in 0..16u8 {
            let histories = [mask & 1 != 0, mask & 2 != 0, mask & 4 != 0, mask & 8 != 0];
            let restored = components_prepare_restored(excluded, histories);
            let expected = histories.map(|present| present && !excluded);

            assert_eq!(
                restored, expected,
                "lightyear's `prepare_rollback` no longer restores the components \
                 `net::adoption::prepare_restores` says it does. Cell: DisableRollback = \
                 {excluded}, PredictionHistory presence (Position, Rotation, LinearVelocity, \
                 AngularVelocity) = {histories:?}. Restored = {restored:?}, expected = \
                 {expected:?}. That predicate is a paraphrase of a DEPENDENCY's query filter, and \
                 `net::adoption` decides whether to claim a rollback and whether a shove was \
                 delivered from it — so a membership condition added, dropped or changed upstream \
                 has to be re-mirrored here, not discovered in play.",
            );
            assert_eq!(
                super::adoption::prepare_restores(excluded, histories).is_ok(),
                restored.iter().all(|component| *component),
                "the whole-body verdict and the restore disagree in cell DisableRollback = \
                 {excluded}, histories = {histories:?}. This is the answer `restore_is_deliverable` \
                 gates the request on: `Ok` while any component was skipped means claiming a \
                 rollback that restores a partial rigid body, and `Err` while all four were \
                 restored means refusing a shove that would have landed.",
            );
        }
    }
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
