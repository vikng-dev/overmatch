//! Unconditional adoption of authoritative facts the client structurally cannot predict.
//!
//! # Two mechanisms, two jobs
//!
//! `net::protocol`'s comparator thresholds ([`ROLLBACK_VELOCITY`] and its siblings) answer ONE
//! question: is the owner's own prediction wrong enough that re-simulating is worth the CPU? That
//! is a COST decision about solver noise, and tightening it is how this project has twice re-opened
//! a jitter-storm class. It is deliberately coarse and it stays coarse.
//!
//! This module answers a different question. The authority knows something the client had no
//! information to predict — that it was shot — and the client must adopt it whether or not the
//! resulting state difference clears any bar. A hit's Δv is ~0.14 m/s, an order of magnitude under
//! [`ROLLBACK_VELOCITY`]; no threshold that is safe against noise will ever pass it. Every shipped
//! engine separates the two: Source's `RestoreData` copies every networked predicted field with
//! error checking OFF, and the tolerance it does keep feeds only `cl_showerror` and a decaying
//! camera offset. `net::render_error` is already our `cl_smooth`. This is the missing half.
//!
//! # Why this cannot ride the registered comparator
//!
//! `.agents/docs/upstream/lightyear-confirmed-state-at-or-ahead-of-local-tick-never-reconciled.md`
//! documents it in full. At the client lead our shipping sync configuration actually produces (0,
//! and −1 under the sync deadband) lightyear never dispatches a registered comparator for an entity
//! the server explicitly confirms: the receive-time check requires `confirmed_tick < current_tick`,
//! and the completed-tick scan returns early for any entity whose `ConfirmHistory` contains the
//! completed checkpoint — which is every actively driven tank. Both skips are out of reach of
//! anything we register. `StateRollbackMetadata::request_forced_rollback` is not: `check_rollback`
//! consumes it BEFORE every policy branch and regardless of `RollbackMode`.
//!
//! # The restore shape, and why the zero-replay case is safe
//!
//! A forced rollback to tick `T` restores end-of-`T` state and then replays `T+1 ..= current`. At a
//! lead of 0 that replay loop runs ZERO times, so anything that depends on a system re-running is
//! silently dropped. Two shapes are therefore possible, and they are not interchangeable:
//!
//! - a **state checkpoint** produced at `T` → restore end of `T`; correct at zero replay because
//!   `prepare_rollback` itself writes the live component, before the loop that does not run;
//! - an **event that must EXECUTE at `T`** → restore end of `T−1`, then replay `T`.
//!
//! [`AuthoritativeFact`] is deliberately state-checkpoint-shaped ([`AuthoritativeFact::produced_at`]
//! IS the restore tick), because that is the only shape its first consumer can have. Nothing about
//! a hit's impulse rides the wire — [`HullShock`] carries an episode counter, a tick, and a cause
//! tag, and no force, direction, or application point. The client therefore cannot re-derive the
//! shove by replaying `T`; the shove exists only as the authority's end-of-`T` hull velocity, which
//! is exactly what [`ConfirmedHistory`] holds. Restoring `T−1` would replay `T` from the client's
//! own un-hit state and deliver nothing.
//!
//! A future consumer whose fact must be re-EXECUTED cannot reuse this shape. It needs the `T−1`
//! variant, and with it a proof that its realizing systems are idempotent under replay. The
//! readiness gate here — `current_tick >= produced_at`, the client must have RUN the producing tick
//! — is what makes that extension safe when it is written: it already guarantees the `T−1` shape at
//! least one replay tick.
//!
//! # The ledger is OUTSIDE the rollback world
//!
//! [`AuthorityAdoption`] is a plain resource and is never registered for `local_rollback`. This is
//! not an oversight. `crate::ballistics::HullShockLedger::realize` IS rollback-tracked, correctly:
//! it answers "has replay re-realized this episode from the restored history?", which must rewind
//! with everything else. Using that same mark to suppress network-level requests would rewind the
//! "I already asked" bit on every rollback and request forever. Two ledgers, two questions.
//!
//! # BEST EFFORT: the shove should not outrun its own spark
//!
//! The shove and the impact visual reach the player on two different carriers, and the schedule
//! makes the shove systematically the faster one. `net::protocol::ImpactConfirm` is a MESSAGE: it is
//! drained in `Update` by `net::client::receive_fire_events`, which is AFTER the frame's fixed loop,
//! so the earliest the ballistics march can present it is the NEXT frame. `HullShock` is REPLICATED
//! STATE: it is offered and installed here in `PreUpdate`, in the frame it arrives. Two facts the
//! authority produced in one tick and sent in one server frame therefore land one client frame
//! apart, shove first.
//!
//! Backwards is the one order that reads as broken. A shove that lands after its spark reads as
//! impact-then-reaction, which is what a real impact does; a hull that lurches before anything is
//! seen to hit it reads as a bug. So [`request_staged_adoption`] holds an adoption that carries a
//! [`VisualClaim`] until this client has PRESENTED one of the HITS that claim names
//! ([`ImpactPresentation`]).
//!
//! ## Correlating a spark with a fact is an IDENTITY test, not a time window
//!
//! A shove and a spark are the same event only if they name the same hit. Two facts already on the
//! wire say so, and the rule is a set-membership test on them rather than a tolerance:
//!
//! - WHICH EPISODE. `crate::ballistics::AuthorityImpact::tick` is the SERVER tick the authority
//!   resolved the impact on — the same tick `HullShockLedger::arm` ran on. The episode names its own
//!   span: `HullShock::opened` is the tick of its FIRST impulse and `HullShock::tick` the tick it
//!   CLOSED on, so `[opened, tick]` is the set of impulse ticks it covers and membership is a
//!   comparison, not a tolerance.
//! - WHOSE HULL. `crate::ballistics::AuthorityImpact::victim` names the body the authority gave the
//!   impulse to — literally the body whose ledger it armed — so a spark on another tank cannot
//!   release this hull's shove.
//!
//! Neither half works in the client's own clock, and that is the point. A watermark stamped with
//! the LOCAL tick a spark was drawn measures transit and frame scheduling, not which hit it was;
//! no window over it can tell "my spark, drawn early because the ledger coalesced the episode" from
//! "a different hit's spark". Both facts here are the authority's, so the comparison is exact.
//!
//! ### Why the span is CARRIED and not derived from the close tick
//!
//! A window of `(close - SHOCK_EPISODE_TICKS, close]` looks like it must be the same set, and it is
//! not. `close_episode` defers only behind an OPEN episode: the first impulse a fresh
//! `HullShockLedger` ever sees finds `last_bump_tick` unset and publishes IMMEDIATELY, so that
//! episode spans one tick and the derived window claims fifteen more it never covered. That is not a
//! rare corner. `tank::spawn` gives every fresh hull a `HullShockLedger::default()`, `net::server`
//! respawns by despawning the old entity and spawning a new one, and it deliberately keeps the same
//! `crate::CombatantId` — so those fifteen ticks are ticks in which the PREVIOUS life of the same
//! combatant could have been hit and drawn. A prior life's spark releasing a fresh hull's first
//! shove is exactly the failure the identity test exists to prevent.
//!
//! Carrying `opened` makes the span exact for every episode, by construction rather than by
//! arithmetic. `close_episode` stamps it on the first tick it observes a pending impulse and clears
//! it on publication, so `opened ≤ close` and the NEXT episode's `opened` is strictly greater than
//! this one's `close`. Consecutive episodes on one hull therefore span disjoint ranges, and a fresh
//! ledger's first span reaches back over nothing at all — its lower bound is at or after the tick
//! the entity was spawned, which is at or after every impulse the previous incarnation took.
//!
//! ORDER, NOT WRAP-GENERAL ORDER — the assumption is worth stating rather than implying. That
//! argument is plain numeric ordering on the authority's tick counter, and it is sound because
//! lightyear 0.28's `Tick` is a u32 whose comparison is plain `u32::cmp` and whose arithmetic
//! SATURATES (`lightyear_utils::wrapping_id`), on the documented assumption that a session never
//! reaches the ~828-day boundary. At that boundary the timeline freezes rather than wrapping, so a
//! pending episode stalls — it does not wrap into a span that falsely covers an older spark. The
//! ordering claim is therefore exact for every session this game can have, and is NOT a wrap-general
//! numeric proof; anything that made the tick counter genuinely wrap would have to re-derive it.
//!
//! This is an ordering PREFERENCE. It is not a commit barrier, and — the part that must not be
//! overstated — it is not a guarantee; see the hole below. Nothing about the shove is weakened by
//! waiting: the restore target stays [`AuthoritativeFact::produced_at`], so the impulse is still
//! applied at the authority's tick, and the wait only deepens the replay by the frames it waited.
//! Damage (`NetCrew`) is not held for it at all. And the wait is bounded by
//! [`ORDERING_BUDGET_TICKS`]: past it the shove lands unordered rather than never, because a visual
//! that has not arrived by then is not coming.
//!
//! ## THE HOLE: trigger arbitration cannot order what it does not trigger
//!
//! lightyear restores every registered predicted component that HAS a [`ConfirmedHistory`] from
//! that history on every CONFIRMED-STATE rollback, whatever caused it, and nothing in that restore
//! consults this module. So a state rollback to any tick at or past
//! [`AuthoritativeFact::produced_at`] — a `Position` mismatch, `net::watchdog`'s claim, or
//! lightyear's own registered `HullShock` comparator — puts the authority's post-hit hull velocity
//! on the live hull while the fact staged here is still waiting for its spark. This module owns
//! which rollback it ASKS for; it does not own which rollbacks happen. Only a barrier on the shove's
//! APPLICATION could close that, and this slice does not build one.
//!
//! An INPUT rollback's RESTORE is not in that set and must never be counted as one:
//! `prepare_rollback` restores from [`PredictionHistory`], not [`ConfirmedHistory`], for
//! `Rollback::FromInputs`, so it re-installs the client's own no-hit prediction. [`retirement`] is
//! told the rollback KIND for exactly this reason. Our own client disables input rollback outright
//! (`net::client::shipping_rollback_policy`), and a test in this module holds that in place — but
//! the helper does not RELY on it, because a masked defect with no tripwire is how this returns.
//!
//! What an input rollback's REPLAY can still do is narrower and worth stating exactly, because the
//! stronger claim ("an input rollback at any depth delivers nothing") is not what lightyear does.
//! `snap_to_confirmed_during_rollback` takes `Single<&Rollback>` and does NOT branch on the variant:
//! at every replayed tick it overwrites any predicted component that has an EXACT confirmed sample
//! there. So an input rollback that starts BEFORE [`AuthoritativeFact::produced_at`] replays through
//! it and installs the authority's post-hit velocity anyway. [`retirement`] still answers `Keep` —
//! it cannot see the replay, and the fact's restore-time delivery genuinely did not happen — so this
//! module then asks for a state rollback that re-installs state already live. That costs one
//! redundant rollback, which is a render hitch and nothing else: the impulse is never re-executed by
//! the client, it only exists as restored state. Accepted and recorded rather than detected.
//!
//! The gap is therefore MEASURED rather than claimed away. [`confirm_forced_rollback`] counts every
//! rollback it did not order that delivered a still-waiting fact — [`OrderingTally::bypassed`],
//! reported on the same diagnostics line as the ordering tallies. "Delivered" is ESTABLISHED and not
//! assumed: [`restore_carries_the_shove`] asks whether the state `prepare_rollback` actually resolved
//! the hull's velocities to is one the authority produced at or after the episode SETTLED
//! ([`AuthoritativeFact::settled_at`]), because a state rollback whose newest confirmed sample
//! predates the hit restores a PRE-hit velocity and carries nothing. A best-effort rule with a bypass
//! counter is honest; a guarantee this shape cannot keep is not.
//!
//! # DELIVERY IS ESTABLISHED AT THE REQUEST, NOT AT THE OFFER AND NOT AFTERWARDS
//!
//! [`restore_carries_the_shove`] is the whole answer to "what if our own rollback restores a pre-hit
//! velocity?". `prepare_rollback` restores from `get_state_at_or_before(rollback_tick)`, so proving
//! that a sample EXISTS there ([`authority_reaches`]) proves only that lightyear will restore
//! something — not that the something carries the event. The predicate asks the stronger question at
//! the tick the restore would target, over the same buffers lightyear reads.
//!
//! WHERE it is asked is the part that took a fifth review. It is asked TWICE, and only the second
//! one is load-bearing:
//!
//! - [`offer_hull_shock_adoptions`] asks it before staging, as an ECONOMY: there is one staging slot
//!   and a fact that provably cannot be delivered right now should not occupy it.
//! - [`request_staged_adoption`] asks it AGAIN, immediately before it claims the forced-rollback
//!   slot, and that answer is the one the transaction rests on.
//!
//! WHAT is asked took a sixth. Readiness is a claim about the hull's WHOLE rigid body — the two
//! velocities that carry the shove and the two pose components that must survive the restore beside
//! them — and the second ask covered only the velocities, so a late `Position` removal reached a
//! claimed rollback that deleted the component and still closed as an adoption. Both sites now go
//! through [`restore_is_deliverable`], which is where the per-component predicates and the reason
//! they DIFFER are written down.
//!
//! A SEVENTH found the same shape one level up, and closed the CLASS. Everything above is about
//! what a restore would RESOLVE; none of it asks whether the hull is in the restore at all.
//! `prepare_rollback`'s query excludes a hull carrying `DisableRollback` and any component with no
//! `PredictionHistory`, and that condition was expressed once — as a `Without` filter on the offer.
//! `net::rig` inserts that marker in `Update` on the late-prediction promotion path and removes it
//! in `FixedLast`, which Bevy runs FIRST, so the marker necessarily survives into a later
//! `PreUpdate`: the request claimed the slot on the staging-time archetype, the restore skipped the
//! hull, and the fact closed as an adoption having delivered nothing — because `carried` was read
//! from a `ConfirmedHistory` lookup no restore had performed. [`prepare_restores`] is that question,
//! asked at the request AND after `RollbackSystems::Prepare`, and the same round's audit of every
//! other value this module latches at one schedule point and consumes at another is recorded in
//! ADR-0032.
//!
//! The second is not belt-and-braces. A fact is staged on one frame and requested on a LATER one —
//! an [`AdoptionCause::ExternalEvent`] waits for its spark, for up to [`ORDERING_BUDGET_TICKS`] —
//! and confirmed history is not append-only underneath it: `ConfirmedHistory::insert_raw` does a
//! sorted MIDDLE insertion with same-tick replacement, a `SameAsPrecedent` entry re-resolves when a
//! late PRECEDING sample lands (lightyear ships a test for exactly that), and replicon's mutation
//! transport is unordered with history-enabled entities accepting older mutations. Nor does a later
//! offer pass rescue it: re-offering the same identity leaves the staged fact untouched, and an
//! offer whose own gate now fails just skips the hull. An earlier version of this module proved
//! readiness at staging and acted on it frames later without re-asking, and no test noticed because
//! every fixture built a static history.
//!
//! A FAILED REVALIDATION IS A WAIT, NOT A DROP — the answer is expected to change, so nothing is
//! claimed, nothing is tallied, and the fact stays staged. What bounds the wait is the replay-window
//! check that runs first in the same function: once the fact's age passes
//! `RollbackPolicy::max_rollback_ticks` it is closed with a WARN naming it. The local tick advances
//! at least once per tick, so the stall is bounded and the give-up is loud.
//!
//! That leaves [`retirement`] free to insist on the stronger fact rather than assume it. Our own
//! installed claim retires the fact as [`Retirement::Adopted`] only when the restore is established
//! to have carried it; installed-but-not-carried is [`Retirement::Undelivered`] — logged at ERROR,
//! counted in [`OrderingTally::undelivered`], and closed. Closed, not retried: a retry re-reads the
//! confirmed histories this restore just read at the same target tick, so it cannot improve, and
//! looping on it is the storm this module exists to avoid. The branch is unreachable against the
//! pinned lightyear — see [`retirement`] for the three facts that make it so and for why it is kept
//! anyway.
//!
//! Since REV 25 this module is the SOLE INTENTIONAL present-value `HullShock` rollback policy at
//! every prediction lead: the registered condition in `net::protocol` is permanently inert. It
//! used to be a live competitor — a pure function of two component values that could not consult
//! [`ImpactPresentation`] — and the 5-seed capture
//! (`.agents/docs/design/hullshock-delivery-capture-2026-07-31.md`) measured its only production
//! effect: it ordered exactly the four belt-first-round rollbacks per run that landed the shove
//! 1–3 ticks before its spark, the bypass class this module's ordering rule exists to prevent,
//! while every mid-belt fact was already adopted here (which path runs is set by whether latency
//! exceeds what input delay absorbs, not by loopback-vs-WAN — the measured lead at 40/5 is
//! NEGATIVE, `.agents/docs/design/timelines-and-shear.md`).
//!
//! SOLE INTENTIONAL, with two structural residues that are lightyear's and not route selection:
//! presence mismatches (`(Some, None)` / `(None, Some)`) order rollback without ever calling the
//! registered condition — no production lifecycle reaches that shape (the component rides every
//! spawn bundle, is never removed, and respawn replaces the entity, so the client always receives
//! it on the no-mismatch init/seed path), and `net::hull_shock_rollback` pins the carve-out — and
//! once ANY state rollback is ordered, `prepare_rollback` restores `HullShock` from confirmed
//! history regardless of trigger. The second residue is why [`Retirement::Delivered`] and
//! [`OrderingTally::bypassed`] survive: ownership of the TRIGGER moved here, but delivery by an
//! unrelated confirmed-state rollback remains an observable preemption, classified and presented
//! sharp exactly like an adoption.

use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use bevy_replicon::prelude::RepliconTick;
use lightyear::core::confirmed_history::ConfirmedHistory;
use lightyear::core::history_buffer::HistoryState;
use lightyear::prelude::client::Remote;
use lightyear::prelude::*;

use super::protocol::hull_shock_mismatch;
#[cfg(test)]
use super::protocol::{ROLLBACK_VELOCITY, SHOCK_EPISODE_TICKS};
use super::watchdog::newest_present_at_or_before;
use crate::ballistics::{HullShock, Impact, ImpactSurface, RICOCHET_HOLD_TICKS};

/// DERIVED from `net::grip`'s `MAX_CHECKPOINT_LEDGERS`, for the same reason: one owner needs one
/// entry, and 16 absorbs a reconnect's overlapping despawn/spawn without letting a per-entity
/// ledger grow without bound.
const MAX_FACT_LEDGERS: usize = 16;

/// How long an external-event adoption waits LOCALLY for its impact visual before landing unordered.
///
/// WHAT IT BOUNDS: local staging patience — ticks elapsed since the fact was first staged on THIS
/// client, not the fact's age. [`AuthoritativeFact::produced_at`] is a SERVER tick and already
/// carries the network transit, so a budget measured from it would be spent before the client ever
/// saw the fact and would adopt instantly in exactly the high-latency case the rule exists for.
///
/// DERIVED from [`RICOCHET_HOLD_TICKS`], the window a cosmetic shell ALREADY spends frozen at armor
/// waiting for the same authority verdict this fact rode beside. Past it the shell quietly dissolves
/// and no impact is ever presented for that shot, so a longer wait cannot buy a visual — it can only
/// make the shove lag something that is never coming.
///
/// Because the wait is now spent ON TOP of the fact's transit age rather than inside it, it can push
/// a late-arriving fact past `RollbackPolicy::max_rollback_ticks` (100). That is handled, not
/// ignored: [`request_staged_adoption`] re-checks the replay window every frame ahead of the
/// ordering rule and drops an unreachable fact loudly rather than adopting a tick replay cannot
/// reach. A budget an order of magnitude under the window keeps that case rare.
pub(super) const ORDERING_BUDGET_TICKS: i32 = RICOCHET_HOLD_TICKS as i32;

/// WHY authority is overriding prediction. Every forced-rollback request carries one.
///
/// The two are not cosmetic variants of one thing. "I mispredicted my own position" is an ERROR:
/// the correct state was always knowable locally, the seam is the client's fault, and the view
/// layer should hide it as hard as it can. "The server is telling me something hit me" is a correct
/// physical EVENT arriving late: smoothing it away would be smoothing away the hit itself, so it
/// must stay SHARP.
///
/// THE TAG IS NOT THE PRESENTATION SIGNAL, and slice 4 established that it cannot be. It records the
/// INTENT of whoever claimed the forced-rollback slot; what `net::render_error` needs is what the
/// rollback DELIVERED, which only [`retirement`] can say and which disagrees with the tag in both
/// directions. See [`SharpCorrection`]. The tag's remaining jobs are same-tick slot arbitration
/// ([`AdoptionCause::wins_over`]), logging, and the `ExternalEvent` predicate on that message.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AdoptionCause {
    /// The client's own prediction drifted from authority. Hide the seam.
    Misprediction,
    /// The authority applied something to this entity that the client had no information to
    /// predict. Keep the seam sharp.
    ExternalEvent,
}

impl AdoptionCause {
    /// Which cause survives when two subsystems claim the same forced-rollback tick in one frame.
    ///
    /// [`AdoptionCause::ExternalEvent`] wins, always: a real event that also happens to coincide
    /// with a misprediction is still a real event, and mis-tagging it would let the view layer
    /// smooth away a hit. The reverse mistake only leaves an error un-hidden.
    fn wins_over(self, other: Self) -> bool {
        self == Self::ExternalEvent && other == Self::Misprediction
    }
}

/// The ONE global forced-rollback slot, and the cause its claimant attached to it.
///
/// `StateRollbackMetadata` holds a single `Option<Tick>`; a second claimant in the same frame does
/// not queue, it silently narrows the first one's target (lightyear keeps the smaller tick). Every
/// in-tree caller therefore goes through [`ForcedRollbackSlot::claim`], which claims only an empty
/// or already-agreeing slot and reports whether the caller now owns the request. A unit test in
/// this module pins that `request_forced_rollback` has no other production call site.
#[derive(Resource, Default)]
pub(crate) struct ForcedRollbackSlot {
    /// This frame's claim, cleared by [`confirm_forced_rollback`] whether or not it was installed.
    ///
    /// THE ONLY FIELD, and slice 4 is why. There used to be a second one — `installed`, the
    /// confirmed claim — stored here after `RollbackSystems::Prepare` so a future consumer could
    /// read it. ADR-0032's latch audit carried it as "safe TODAY, and the reason has an expiry
    /// date: it has no external consumer yet". The expiry arrived, and the resolution was to DELETE
    /// the field rather than to guard it: [`confirm_forced_rollback`] now takes the claim into a
    /// local value and consumes it in the same statement sequence, so there is no confirmed value
    /// stored anywhere for a later schedule point to read stale.
    claim: Option<(Tick, AdoptionCause)>,
}

impl ForcedRollbackSlot {
    /// Claim the forced-rollback slot for `tick`, tagging the request with why it exists.
    ///
    /// Returns whether this caller owns the request. A busy slot targeting a different tick is left
    /// alone — narrowing it would wedge the other subsystem's correction on a tick it never chose.
    pub(crate) fn claim(
        &mut self,
        metadata: &mut StateRollbackMetadata,
        tick: Tick,
        cause: AdoptionCause,
    ) -> bool {
        let owned = match metadata.forced_rollback_tick() {
            Some(selected) => selected == tick,
            None => {
                metadata.request_forced_rollback(tick);
                metadata.forced_rollback_tick() == Some(tick)
            }
        };
        if owned
            && self
                .claim
                .is_none_or(|(_, existing)| cause.wins_over(existing))
        {
            self.claim = Some((tick, cause));
        }
        owned
    }
}

/// ONE PREDICTED ROOT whose correction on THIS rollback must be presented SHARP.
///
/// # Why the cause tag is not this signal, and never could be
///
/// Slice 4's original brief was "`net::render_error` reads the [`AdoptionCause`] the forced-rollback
/// slot was tagged with". That is the wrong fact, semantically and not merely by timing. The tag
/// records who CLAIMED the slot; this message records what the rollback DELIVERED, which is what
/// [`retirement`] establishes and the two disagree in both directions:
///
/// - [`Retirement::Delivered`] — another subsystem's confirmed-state rollback carried the staged
///   hit. The shove is live on the hull and must stay sharp, while the slot's cause reads `None` or
///   [`AdoptionCause::Misprediction`].
/// - [`Retirement::Undelivered`] — our own [`AdoptionCause::ExternalEvent`] claim was installed and
///   `prepare_rollback` restored a PRE-hit velocity. There is no hit in that correction to keep
///   sharp, while the slot's cause says there is.
///
/// No scheduling guard repairs that; the tag answers a different question.
///
/// # Why it names an ENTITY, and why it is a message
///
/// A rollback is world-wide and sharpness is per-root: two predicted roots can be corrected by one
/// rollback and only one of them was shot. A global `Option` carries no target, so a replacement
/// root spawned after a despawn would inherit the previous victim's sharpness — Bevy's `Entity`
/// generation is what stops that, and it only helps if the signal carries one.
///
/// It is a MESSAGE and not stored state because the consumer is one system, two system boundaries
/// later, inside the same `PreUpdate` rollback transaction: `net::render_error`'s capture DRAINS the
/// queue every frame, whether or not any root matches. Nothing is left to be read on a later frame,
/// so this is not a latch and needs no row in ADR-0032's audit table.
#[derive(Message, Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct SharpCorrection {
    /// The predicted root the delivered fact is about — [`FactId::entity`], generation included.
    pub(super) entity: Entity,
    /// The tick the rollback that delivered it restored from. Diagnostic: the consumer keys on the
    /// entity, and the queue never survives the frame, so nothing needs this to disambiguate.
    pub(super) restored_from: Tick,
}

/// Everything the best-effort ordering rule has done so far. Read by `net::diagnostics`.
///
/// Every field is named for what it MEASURES rather than for the intent behind it: this rule cannot
/// stop a rollback it did not order, so "ordered" would claim more than any of these numbers carry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct OrderingTally {
    /// Shoves released because one of the hits the fact is made of had been drawn.
    pub(crate) released_on_impact: u32,
    /// Shoves released because [`ORDERING_BUDGET_TICKS`] of local patience ran out with none of the
    /// fact's own hits drawn at all.
    pub(crate) released_on_budget: u32,
    /// Shoves that landed WITHOUT this module's decision: a CONFIRMED-STATE rollback it did not
    /// order restored the authority's post-hit hull state while NONE of the fact's own hits had been
    /// drawn. THE honesty number — the ordering rule is best effort, and this is the size of the
    /// gap, so it is also the number that would justify replacing the rule with a real presentation
    /// barrier. Everything it counts is therefore ESTABLISHED rather than assumed:
    ///
    /// - the restore reached this hull — see [`prepare_restores`]; a rollback that skipped it
    ///   delivered nothing whatever its start tick says;
    /// - it carried the event — see [`restore_carries_the_shove`]; one restoring from the client's
    ///   own prediction, or from a confirmed sample older than the hit, leaves the fact staged;
    /// - and it beat the SPARK, asked of [`ImpactPresentation`] rather than of the ordering latch.
    ///   A fact the rule released, and a fact with no visual to be ordered against, bypassed
    ///   nothing; reading the latch's absence counted both.
    pub(crate) bypassed: u32,
    /// Shoves this module ordered a rollback for and LOST: its own claim was installed, and
    /// `prepare_rollback` restored a hull velocity older than the event's settling tick, so the fact
    /// was spent having delivered nothing. Should be permanently zero — [`request_staged_adoption`]
    /// re-establishes the same predicate over the same buffers in the same frame, immediately before
    /// it claims — and is counted rather than asserted so a disagreement between that revalidation
    /// and lightyear's restore shows up as a number instead of as a missing shove. See
    /// [`retirement`] for what the unreachability proof rests on and how a dependency bump breaks it.
    pub(crate) undelivered: u32,
    /// Longest LOCAL wait the rule has imposed on a shove, in ticks.
    pub(crate) max_wait_ticks: i32,
}

/// One authority-resolved hit this client has DRAWN, recorded in the AUTHORITY's terms.
///
/// The local tick it was drawn on is deliberately absent: it measures transit and frame scheduling,
/// and no comparison against it can say which hit a spark was. See the module doc.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct PresentedHit {
    /// The server tick the authority resolved this impact on.
    tick: Tick,
    /// The body the authority gave the impulse to, when the struck volume belonged to one.
    victim: Option<crate::CombatantId>,
}

/// What a spark must BE for a fact to count as ordered against it.
///
/// The producer builds it, because only the producer knows how its own facts group hits; this
/// module only checks membership. A fact with no claim is never held — see [`clear_to_order`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct VisualClaim {
    /// The combatant the fact is about. A spark on another body is another tank's business.
    pub(crate) victim: crate::CombatantId,
    /// The authority ticks whose impacts belong to this fact, as the CLOSED range `[from, through]`.
    /// Both bounds are the authority's own: `from` is the tick the episode opened on and `through`
    /// the tick it closed on, so neither is inferred and neither overlaps a neighbouring episode.
    pub(crate) from: Tick,
    pub(crate) through: Tick,
}

impl VisualClaim {
    /// The hits an arriving episode is made of, in the authority's terms.
    ///
    /// The ONE place a claim is built, so the production offer and every fixture agree by
    /// construction rather than by two copies of the same arithmetic staying in step. Both bounds
    /// are read off [`HullShock`]; nothing here derives a window.
    fn for_episode(victim: crate::CombatantId, episode: &HullShock) -> Self {
        Self {
            victim,
            from: Tick(episode.opened),
            through: Tick(episode.tick),
        }
    }

    fn covers(self, hit: PresentedHit) -> bool {
        hit.victim == Some(self.victim) && hit.tick - self.from >= 0 && hit.tick - self.through <= 0
    }
}

/// How many presented hits are retained.
///
/// NOT DERIVED FROM TICKS, and the eighth review found the derivation that said it was. It read:
/// an episode spans at most `SHOCK_EPISODE_TICKS`, a fact is staged for at most
/// [`ORDERING_BUDGET_TICKS`], so 32 entries is one per tick of the widest span anything can still
/// ask about. The two halves do not meet. The capacity is a budget in TICKS while the eviction is
/// per ENTRY, and nothing bounds entries per tick: every authority armor impact is appended
/// ([`note_presented_impact`]) and impacts are broadcast to every client, so one tick of a busy
/// server can evict the whole ledger. A staged fact's own spark could be drawn and then pushed out
/// by 32 unrelated hits, after which [`retirement`] read "never drawn" and reported a BYPASS for a
/// spark the player had already seen.
///
/// What carries the staged fact's answer now is [`WatchedSpark`], which is per-fact and outside this
/// FIFO entirely, and the latch closes the window from staging onward completely.
///
/// # THE RESIDUAL, AT ITS REAL SCOPE — the ninth review found the previous wording too kind twice
///
/// This buffer's remaining job is to SEED that latch, so an eviction still matters for a spark drawn
/// BEFORE its fact was staged.
///
/// **That exposed interval is the whole pre-staging window, not "one replication gap".** The
/// authority coalesces impulses for up to [`SHOCK_EPISODE_TICKS`] before it publishes the episode at
/// all, and a coalesced episode's FIRST hit has its own `ImpactConfirm` broadcast immediately — so
/// the early spark can precede the episode's publication, then its replication, then the frame the
/// client stages the fact on. Every impact from every combatant lands in this one buffer over that
/// whole span, and nothing bounds entries per tick, so **64 is headroom over that window and not a
/// bound on it.** A busy enough server evicts through any fixed depth.
///
/// **And a lost entry is not always conservative.** In [`clear_to_order`] it is — an unseeded latch
/// reads "not drawn", the fact waits and is released on budget, which logs loudly. But the same
/// unseeded latch is what [`retirement`] reads through [`ImpactPresentation::shown_for`], and there
/// a false "not drawn" becomes `spark_pending: true` and an INFLATED `bypassed` for a spark the
/// player did see. That is the same direction of error slice 3.12 was fixing; the latch narrowed it
/// from the entire lifetime of a staged fact to the pre-staging window, and did not remove it.
/// It is accepted as a known best-effort limitation OF THE TELEMETRY — no delivery decision reads
/// this buffer, and `bypassed` is the number that would justify replacing the ordering rule with a
/// real presentation barrier, so it must be read as an upper bound. Removing the residual means
/// keying the drawn state before the fact exists, which means an authority-keyed ledger with its own
/// eviction policy; that is a design change, not a bigger constant.
const MAX_PRESENTED_HITS: usize = 64;

/// The staged claim's own drawn state, held OUTSIDE the eviction FIFO.
///
/// A staged fact is one bounded, known thing; the impact stream is not, so the fact's answer may not
/// be stored as a search over the stream. `drawn` is seeded from the ledger when the fact is staged
/// (a spark drawn before its fact arrived still counts) and latches true from then on.
///
/// KEYED BY THE CLAIM, which is what makes a stale watch harmless rather than merely unread: the key
/// IS the authority identity `(victim, from, through)`, so the only question a leftover watch can
/// answer is the one it was seeded for. A later episode on the same hull opens after the previous
/// one closed and therefore carries a different span.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct WatchedSpark {
    claim: VisualClaim,
    drawn: bool,
}

/// What this client has SHOWN, and what the ordering rule did with it.
#[derive(Resource, Default)]
pub(crate) struct ImpactPresentation {
    /// The authority-resolved armor hits this client has drawn, oldest first.
    presented: Vec<PresentedHit>,
    /// The staged fact's claim and whether one of ITS hits has been drawn. See [`WatchedSpark`].
    watched: Option<WatchedSpark>,
    tally: OrderingTally,
}

impl ImpactPresentation {
    /// Record that an armor impact belonging to `hit` was drawn.
    fn present(&mut self, hit: PresentedHit) {
        if let Some(watched) = self.watched.as_mut() {
            watched.drawn |= watched.claim.covers(hit);
        }
        self.presented.push(hit);
        if self.presented.len() > MAX_PRESENTED_HITS {
            self.presented.remove(0);
        }
    }

    /// Start retaining `claim`'s drawn state independently of the FIFO. Called from the ONE place a
    /// fact is staged, [`AuthorityAdoption::offer`], which is why that function takes this resource.
    ///
    /// IDEMPOTENT, and it has to be: a producer re-offers the same fact every frame, and re-seeding
    /// from the FIFO on a re-offer would hand back exactly the eviction the latch exists to survive.
    fn watch(&mut self, claim: VisualClaim) {
        if self.watched.is_some_and(|watched| watched.claim == claim) {
            return;
        }
        self.watched = Some(WatchedSpark {
            claim,
            drawn: self.presented.iter().any(|hit| claim.covers(*hit)),
        });
    }

    /// Whether one of the hits `claim` is made of has been drawn.
    ///
    /// The latch answers for the claim it is keyed to. The FIFO scan is the fallback for a claim
    /// nothing is watching, which production cannot reach — every staged fact with a visual was
    /// watched when it was staged — and which is what a fixture asking about an unstaged claim gets.
    fn shown_for(&self, claim: VisualClaim) -> bool {
        match self.watched {
            Some(watched) if watched.claim == claim => watched.drawn,
            _ => self.presented.iter().any(|hit| claim.covers(*hit)),
        }
    }

    /// Tally one resolved ordering decision. Called once per staged fact, not once per retry.
    fn resolve(&mut self, shown: bool, waited: i32) {
        if shown {
            self.tally.released_on_impact += 1;
        } else {
            self.tally.released_on_budget += 1;
        }
        self.tally.max_wait_ticks = self.tally.max_wait_ticks.max(waited);
    }

    /// Tally one shove that landed through a rollback this module did not order.
    fn note_bypass(&mut self) {
        self.tally.bypassed += 1;
    }

    /// Tally one shove this module ordered a rollback for and did not receive.
    fn note_undelivered(&mut self) {
        self.tally.undelivered += 1;
    }

    /// What the ordering rule has done so far.
    pub(crate) fn tally(&self) -> OrderingTally {
        self.tally
    }
}

/// Record the impact the view layer is about to draw, in the authority's terms.
///
/// `crate::ballistics::Impact` IS the presentation signal — `vfx::impact` renders off this same
/// trigger — so this records what the player SEES, not what arrived on the wire. The march that
/// raises it is skipped entirely on a replayed tick (`crate::Replaying`), so a rollback never
/// re-stamps or rewinds this.
///
/// An impact with no [`crate::ballistics::AuthorityImpact`] is the resolver's OWN read, which on a
/// client means an unkeyed shell that never had an authority fact to correlate with. It is drawn and
/// not recorded: recording it would put an entry with no identity into the ledger, and the only
/// thing an identity-less entry can do is match a claim it does not belong to.
fn note_presented_impact(impact: On<Impact>, mut presentation: ResMut<ImpactPresentation>) {
    if impact.surface != ImpactSurface::Armor {
        return;
    }
    if let Some(authority) = impact.authority {
        presentation.present(PresentedHit {
            tick: Tick(authority.tick),
            victim: authority.victim,
        });
        // The spark's side of the ordering question, joinable against the fact rows: whether a
        // belt-first fact staged BEFORE its spark is a comparison of `staged.staged_at` against
        // this row's payload tick — the authority tick the drawn impact resolved on.
        crate::trace::note_fact_event(|| {
            serde_json::json!({
                "ev": "spark",
                "at": authority.tick,
                "victim": authority.victim.map(|victim| victim.0),
            })
        });
    }
}

/// Which subsystem produced a fact. Two subsystems must never collide in the delivery ledger.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FactSource {
    /// `crate::ballistics::HullShock` — the authority applied an impulse to this hull.
    HullShock,
}

/// Stable identity of one authoritative fact.
///
/// `HullShock { count, tick }` alone is NOT an identity: it says nothing about which entity, which
/// entity incarnation, or which replication checkpoint carried it, so a reconnect or a re-send
/// cannot be told apart from a new episode.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct FactId {
    pub(crate) source: FactSource,
    /// The predicted entity the fact is about. Bevy's `Entity` carries its own generation, so a
    /// despawn/respawn cycle yields a value that compares unequal — the entity epoch is already in
    /// here and does not need a parallel counter.
    pub(crate) entity: Entity,
    /// The producer's own monotonic (wrapping) sequence for this entity — `HullShock::count`.
    pub(crate) sequence: u32,
    /// The completed replication checkpoint that certified the fact. Two checkpoints carrying the
    /// same sequence are distinguishable here; whether the second one is worth a second rollback is
    /// decided by the watermark, not by this field.
    pub(crate) checkpoint: RepliconTick,
}

/// A fact the authority knows and the client structurally cannot predict, offered for unconditional
/// adoption.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct AuthoritativeFact {
    pub(crate) id: FactId,
    pub(crate) cause: AdoptionCause,
    /// The authority tick whose END-OF-TICK state carries the fact. This is the restore target; see
    /// the module doc on why this primitive is state-checkpoint-shaped.
    pub(crate) produced_at: Tick,
    /// The EARLIEST authority tick whose end-of-tick state already carries the WHOLE fact.
    ///
    /// Not the same tick as [`Self::produced_at`], and conflating them cost a review round.
    /// `produced_at` is the tick of the CONFIRMED SAMPLE that certified the fact, and replication
    /// stamps a value with the tick it was SENT, which can sit later than the tick the authority's
    /// state actually settled on. `settled_at` is that settling tick: for a [`HullShock`] episode it
    /// is the tick the episode CLOSED, because every impulse the episode is made of landed at a tick
    /// in `[opened, tick]` (`arm` runs inside the ballistic march and `close_episode` runs after it,
    /// on the same authority tick), so the authority's end-of-`tick` velocity contains all of them.
    ///
    /// It is what [`restore_carries_the_shove`] compares against, because `prepare_rollback` restores
    /// the effective authoritative state AT OR BEFORE its target: a restore that resolves to any
    /// sample in `[settled_at, produced_at]` has delivered the fact in full, and demanding
    /// `produced_at` exactly would reject real deliveries and, worse, would pass readiness on facts
    /// whose restore installs a PRE-event value.
    ///
    /// INVARIANT: `settled_at <= produced_at`. It is not asserted, it is self-enforcing — the
    /// readiness gate resolves a sample at or before `produced_at` and requires it at or after
    /// `settled_at`, so a fact violating the invariant can never satisfy the gate and is never
    /// requested.
    pub(crate) settled_at: Tick,
    /// What a spark has to be for this fact to count as ordered against it, when the fact has a
    /// visual at all. `None` means "nothing to wait for" and the fact is never held: that is
    /// correct for an [`AdoptionCause::Misprediction`], which has no visual by construction, and it
    /// is a producer's own responsibility to supply one for an event that does.
    pub(crate) visual: Option<VisualClaim>,
}

/// What [`AuthorityAdoption::offer`] did with an offered fact.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Offer {
    /// The fact is this frame's staged adoption (or already was).
    Staged,
    /// This exact fact has already been through the transaction.
    AlreadyAdopted,
    /// An equal-or-older sequence for this source and entity. A re-send is not a new episode.
    NotNewer,
    /// A different fact is mid-transaction. Offer again next frame.
    SlotBusy,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct FactWatermark {
    source: FactSource,
    entity: Entity,
    sequence: u32,
}

/// Whether `candidate` is strictly newer than `current` on a wrapping counter.
pub(super) fn wrapping_newer(candidate: u32, current: u32) -> bool {
    candidate != current && candidate.wrapping_sub(current) as i32 > 0
}

/// WHERE THE STAGED FACT STANDS with the best-effort ordering rule.
///
/// Three states, because the `Option<bool>` this replaces had two and was being read for three. Its
/// `None` meant "the rule has not been consulted" (every early return in
/// [`request_staged_adoption`] ahead of [`clear_to_order`] leaves it there), and "there is no visual
/// to be ordered against" (a [`AdoptionCause::Misprediction`] returns before latching), and "the
/// rule has the fact and is holding it for a spark". [`retirement`] read that one `None` as the
/// third meaning and counted [`OrderingTally::bypassed`] for all three.
///
/// No simulation state ever depended on the confusion — but `bypassed` is the number that decides
/// whether this rule needs to become a real presentation barrier, so a count inflated by facts the
/// rule never held would mislead exactly the decision it exists to inform.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Ordering {
    /// No frame has put the question yet: every request so far returned before reaching
    /// [`clear_to_order`], because the fact was not requestable on it.
    #[default]
    Unasked,
    /// The rule HAS the fact and is holding it: none of its own hits has been drawn and
    /// [`ORDERING_BUDGET_TICKS`] has not run out.
    HoldingForSpark,
    /// The rule is not holding the fact — it either released it (by either verdict, tallied once)
    /// or never had anything to hold it for.
    Released,
}

/// The delivery ledger for authoritative facts. NOT rollback-tracked — see the module doc.
#[derive(Resource, Default)]
pub(crate) struct AuthorityAdoption {
    staged: Option<AuthoritativeFact>,
    /// The LOCAL tick the staged fact was FIRST staged on. This — not
    /// [`AuthoritativeFact::produced_at`] — is the clock [`ORDERING_BUDGET_TICKS`] runs on; see that
    /// constant for why a server tick cannot serve.
    staged_at: Option<Tick>,
    /// Whether [`request_staged_adoption`] currently owns the forced-rollback slot for the staged
    /// fact. Bookkeeping only: it is re-derived from the slot every frame rather than trusted.
    requested: bool,
    /// Where the staged fact stands with the ordering rule. Latched so a retried request tallies
    /// [`ImpactPresentation`] once per FACT rather than once per frame.
    ordering: Ordering,
    adopted: Vec<FactId>,
    watermarks: Vec<FactWatermark>,
}

impl AuthorityAdoption {
    /// Offer a fact for unconditional adoption. Idempotent: a producer re-derives its offer from
    /// replicated state every frame and hands it over unconditionally; this decides.
    ///
    /// `now` is the caller's LOCAL tick and is recorded only on the frame the fact is first staged,
    /// which is what makes it a patience clock rather than a re-offer clock.
    ///
    /// `presentation` is taken because THIS is the one place a fact becomes staged, and a staged
    /// fact's spark has to start being retained the moment it is — see [`WatchedSpark`]. Passing the
    /// ledger through the signature is what makes that structural: a future producer added at
    /// [`OfferAuthoritativeFacts`] cannot stage a fact without handing over the ledger its claim
    /// will be answered from, so it cannot silently inherit the evicting search this replaced.
    pub(crate) fn offer(
        &mut self,
        fact: AuthoritativeFact,
        now: Tick,
        presentation: &mut ImpactPresentation,
    ) -> Offer {
        if self.adopted.contains(&fact.id) {
            return Offer::AlreadyAdopted;
        }
        if !self.is_newer(&fact) {
            return Offer::NotNewer;
        }
        let offer = match self.staged {
            Some(staged) if staged.id == fact.id => Offer::Staged,
            Some(_) => Offer::SlotBusy,
            None => {
                self.staged = Some(fact);
                self.staged_at = Some(now);
                self.requested = false;
                self.ordering = Ordering::Unasked;
                // The per-fact ownership row the A/B instrument accounts every fact from. The
                // claim span rides along because it is recorded nowhere else machine-readable —
                // it is what decides whether a nearby spark covers this fact (the seed-5 ricochet
                // ambiguity). Repeat offers take the `Staged` arm above and emit nothing;
                // `SlotBusy` re-offers every frame and is deliberately not a row — the losing
                // fact either stages later (its own row) or is superseded, which the sequence
                // accounting shows as a jump.
                crate::trace::note_fact_event(|| {
                    serde_json::json!({
                        "ev": "staged",
                        "seq": fact.id.sequence,
                        "ent": format!("{}", fact.id.entity),
                        "at": fact.produced_at.0,
                        "settled": fact.settled_at.0,
                        "staged_at": now.0,
                        "span": fact.visual.map(|claim| [claim.from.0, claim.through.0]),
                    })
                });
                Offer::Staged
            }
        };
        // Only for the fact that HOLDS the slot. `SlotBusy` means somebody else's claim is staged and
        // watching a loser's claim would answer the wrong question; `watch` is idempotent, so the
        // winner's re-offer every frame neither re-seeds nor drops what it has already latched.
        if offer == Offer::Staged
            && let Some(claim) = fact.visual
        {
            presentation.watch(claim);
        }
        offer
    }

    /// The staged fact's sequence, for fixtures that need to know WHICH fact holds the slot —
    /// the contention control's evidence that a newer offerable fact answered `SlotBusy`.
    #[cfg(test)]
    pub(super) fn staged_sequence(&self) -> Option<u32> {
        self.staged.map(|fact| fact.id.sequence)
    }

    /// Whether a fact is currently mid-transaction.
    ///
    /// Exists so a fixture can tell a WAIT (the fact is still staged and will be reconsidered) from
    /// a DROP (it was closed), which is the whole difference [`request_staged_adoption`]'s
    /// revalidation turns on and is otherwise invisible from outside this module.
    #[cfg(test)]
    pub(super) fn is_staged(&self) -> bool {
        self.staged.is_some()
    }

    fn is_newer(&self, fact: &AuthoritativeFact) -> bool {
        !self.watermarks.iter().any(|watermark| {
            watermark.source == fact.id.source
                && watermark.entity == fact.id.entity
                && !wrapping_newer(fact.id.sequence, watermark.sequence)
        })
    }

    /// Close the transaction on `fact`: it will never be offered or requested again.
    ///
    /// Both outcomes end here — an adoption lightyear installed, and one abandoned as unreachably
    /// old. Retrying either is how a delivery mechanism becomes a storm.
    fn close(&mut self, fact: AuthoritativeFact) {
        if !self.adopted.contains(&fact.id) {
            self.adopted.push(fact.id);
            if self.adopted.len() > MAX_FACT_LEDGERS {
                self.adopted.remove(0);
            }
        }
        match self.watermarks.iter_mut().find(|watermark| {
            watermark.source == fact.id.source && watermark.entity == fact.id.entity
        }) {
            Some(watermark) => watermark.sequence = fact.id.sequence,
            None => {
                self.watermarks.push(FactWatermark {
                    source: fact.id.source,
                    entity: fact.id.entity,
                    sequence: fact.id.sequence,
                });
                if self.watermarks.len() > MAX_FACT_LEDGERS {
                    self.watermarks.remove(0);
                }
            }
        }
        self.staged = None;
        self.staged_at = None;
        self.requested = false;
        self.ordering = Ordering::Unasked;
    }
}

/// A connection is a new timeline: every tick, entity, and checkpoint identity in the ledgers
/// belongs to the old one.
fn reset_adoption_state(
    _connected: On<Add, Connected>,
    mut adoption: ResMut<AuthorityAdoption>,
    mut slot: ResMut<ForcedRollbackSlot>,
    mut presentation: ResMut<ImpactPresentation>,
) {
    // The per-fact rows promise every `staged` a terminal; a reconnect abandoning a held fact
    // must say so or the acceptance analyzer reads an intentional timeline reset as a lost fact.
    if let Some(fact) = adoption.staged {
        crate::trace::note_fact_event(|| {
            serde_json::json!({
                "ev": "dropped",
                "seq": fact.id.sequence,
                "why": "reconnect",
            })
        });
    }
    *adoption = default();
    *slot = default();
    // The presented hits and the staged claim's latch are ticks from the old timeline; the tallies
    // are session instrumentation and deliberately survive, so a reconnect does not erase the number
    // that decides the barrier.
    presentation.presented.clear();
    presentation.watched = None;
}

/// Whether a state restore at `tick` would replace this component with AUTHORITY, or leave the
/// client's own prediction standing under a tick-`tick` label.
///
/// Mirrors the branch `prepare_rollback` actually takes, and FAILS CLOSED on all three of its
/// negative cases. A component that HAS a [`ConfirmedHistory`] is restored from
/// `get_state_at_or_before(rollback_tick)`; with no sample there, lightyear leaves the live value
/// alone. A component with NO confirmed history takes the other branch entirely — `prepare_rollback`
/// restores it from [`PredictionHistory`] even on a state rollback, which for a hull the client never
/// knew was hit is its own un-hit prediction. Treating a missing history as "fine" was therefore
/// backwards: it is the case in which authority provably does NOT reach.
///
/// The third case is an authoritative REMOVAL. `ConfirmedHistory` stores removals as ordinary
/// entries, `get_state_at_or_before` resolves them like any other state, and `prepare_rollback`
/// answers one by taking the component OFF the hull. A restore that deletes half the rigid body is
/// not a restore this module may build on, so `Removed` is a NO here rather than a `Some`.
fn authority_reaches<C: Component + Clone>(
    confirmed: Option<&ConfirmedHistory<C>>,
    tick: Tick,
) -> bool {
    matches!(
        confirmed.and_then(|history| history.get_state_at_or_before(tick)),
        Some(HistoryState::Updated(_)),
    )
}

/// Whether a state restore targeting `restored_at` puts the authority's POST-EVENT hull velocities
/// on the live hull. ONE predicate, asked in the only two places the answer matters.
///
/// - READINESS, at `restored_at = `[`AuthoritativeFact::produced_at`]: may this module request a
///   rollback at all? A request whose restore installs a pre-event velocity buys a render hitch and
///   delivers nothing.
/// - BYPASS, at `restored_at = ` the start tick of a rollback somebody else ordered: did that
///   rollback already land the shove? That is what [`OrderingTally::bypassed`] claims.
///
/// It MIRRORS `prepare_rollback` and does not paraphrase it. lightyear restores each component from
/// `ConfirmedHistory::get_state_at_or_before(rollback_tick)` — the EFFECTIVE authoritative state at
/// or before its target, with no requirement on how old the underlying sample is — so the question is
/// not "was a sample stamped at the producing tick" but "is the state that lookup resolves one the
/// authority produced at or after the event SETTLED". Hence the comparison is against
/// [`AuthoritativeFact::settled_at`], never against `produced_at`: requiring the sample to be at or
/// after the CONFIRMED-SAMPLE tick rejected genuine deliveries whenever replication materialized the
/// fact later than the tick its state settled on, which is the ordinary shape at any send interval
/// above one tick.
///
/// Both velocities are required, because half a restored rigid body is not a state either peer ever
/// had. Fails closed on a missing history for the same reason [`authority_reaches`] does.
///
/// # THE ANSWER CAN CHANGE UNDER IT, so ask it where the answer is USED
///
/// A confirmed history is NOT an append-only log and this predicate must not be evaluated as though
/// it were: `ConfirmedHistory::insert_raw` does a sorted MIDDLE insertion with same-tick
/// replacement, replicon's mutation transport is unordered, and a `SameAsPrecedent` entry resolves
/// to whatever explicit sample most recently precedes it — so a late message can change what a
/// lookup at a FIXED tick returns. That is why this function is called at the request transaction
/// ([`request_staged_adoption`]) and not only at the offer; see [`offer_hull_shock_adoptions`].
///
/// TWO LOOKUPS, because one of them alone does not mirror `prepare_rollback`:
///
/// - `get_state_at_or_before(restored_at)` is literally the call lightyear makes, and it is what
///   distinguishes a value from an authoritative REMOVAL. A removal middle-inserted between the
///   event and the restore target shadows the older sample it follows, and `prepare_rollback`
///   answers it by deleting the velocity rather than by installing one.
/// - [`newest_present_at_or_before`] then supplies the TICK that value is known at, which
///   `get_state_at_or_before` does not return. A `SameAsPrecedent` entry counts at its OWN tick and
///   that is correct, not a leniency: the marker asserts the authority still held that value there,
///   so a restore resolving it installs the authority's genuine state at that tick.
///
/// When the first lookup resolves `Updated`, the newest present entry at or before `restored_at` IS
/// the entry it resolved, so the pair is one question and not two.
fn restore_carries_the_shove(
    linear: Option<&ConfirmedHistory<LinearVelocity>>,
    angular: Option<&ConfirmedHistory<AngularVelocity>>,
    restored_at: Tick,
    settled_at: Tick,
) -> bool {
    fn resolves_post_event<C>(
        confirmed: Option<&ConfirmedHistory<C>>,
        restored_at: Tick,
        settled_at: Tick,
    ) -> bool {
        let Some(history) = confirmed else {
            return false;
        };
        if !matches!(
            history.get_state_at_or_before(restored_at),
            Some(HistoryState::Updated(_)),
        ) {
            return false;
        }
        newest_present_at_or_before(history, restored_at)
            .is_some_and(|(sample, _)| sample - settled_at >= 0)
    }
    resolves_post_event(linear, restored_at, settled_at)
        && resolves_post_event(angular, restored_at, settled_at)
}

/// Why a state restore at a given tick is not something this module may build on YET. Every variant
/// is a WAIT; see [`request_staged_adoption`] for what bounds it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Unready {
    /// The restore would not bring the hull's POSE back from authority.
    Pose,
    /// The restore would bring the pose back but would not carry the event's velocity change.
    Shove,
    /// There is no hull to read histories off — a despawn between staging and this frame.
    Hull,
    /// The hull carries `DisableRollback`, so `prepare_rollback` skips it for EVERY component and a
    /// rollback ordered for it restores nothing at all.
    Excluded,
    /// The hull is not excluded, but one of the four rigid-body components has no
    /// `PredictionHistory` — lightyear's rollback-membership marker — so the restore would reach
    /// only part of the body.
    Unhistoried,
}

impl Unready {
    /// What the wait is waiting for, for the two logs that report one.
    fn reason(self) -> &'static str {
        match self {
            Unready::Pose => {
                "the confirmed `Position`/`Rotation` a restore there resolves is an authoritative \
                 REMOVAL, or is missing — so the restore would delete part of the rigid body, or \
                 leave the client's own prediction standing under the authority's tick label"
            }
            Unready::Shove => {
                "the newest confirmed hull VELOCITY a restore there resolves predates the event's \
                 settling tick, so the restore would install a PRE-hit value and carry nothing"
            }
            Unready::Hull => "the hull no longer exists, so no restore can be established at all",
            Unready::Excluded => {
                "the hull carries `DisableRollback`, so `prepare_rollback` skips it entirely and \
                 the rollback would restore NONE of its live components"
            }
            Unready::Unhistoried => {
                "one of the hull's four rigid-body components has no `PredictionHistory`, which is \
                 the membership `prepare_rollback`'s query REQUIRES — so the restore would reach \
                 only part of the body"
            }
        }
    }
}

/// THE HULL'S PARTICIPATION in `prepare_rollback`'s restore, as ONE piece of query data.
///
/// Every site that asks whether a rollback reaches this hull names this type, so the conditions are
/// spelled once and read identically everywhere. That is not tidiness: the seventh review's High was
/// the SAME condition expressed two different ways — a `Without<DisableRollback>` filter on the
/// offer's query and nothing at all on the request's — and a filter is the one shape that cannot be
/// handed to a shared predicate, so the divergence was invisible to every reader of either site.
///
/// It is also what makes the divergence mechanically detectable: the source scan
/// `the_three_participation_sites_ask_one_shared_question` pins that `DisableRollback` and the four
/// rigid-body `PredictionHistory<..>` types are named in THIS declaration and nowhere else in the
/// module, that this type itself is named a known number of times with every occurrence accounted
/// for by name — so a new consumer is red wherever in the file it is written — and that every
/// consumer it can read routes through the shared predicate. That scan is a
/// GUARD RAIL and its own doc states its limits: it catches an offer-only re-expression written by
/// accident, which is the real hazard, and it does not and cannot catch one written to evade a
/// lexical reader. The checked contract underneath it is
/// `net::lead_zero_rollback::prepare_restores_exactly_the_components_the_predicate_names`.
#[derive(bevy::ecs::query::QueryData)]
struct RollbackParticipation {
    /// `prepare_rollback`'s query filter, read as DATA. Excludes the hull from EVERY component's
    /// restore at once.
    excluded: Has<DisableRollback>,
    position: Has<PredictionHistory<Position>>,
    rotation: Has<PredictionHistory<Rotation>>,
    linear: Has<PredictionHistory<LinearVelocity>>,
    angular: Has<PredictionHistory<AngularVelocity>>,
}

impl RollbackParticipationItem<'_, '_> {
    /// Membership for the WHOLE rigid body — the four components a readiness verdict is defined on.
    fn whole_body(&self) -> Result<(), Unready> {
        prepare_restores(
            self.excluded,
            [self.position, self.rotation, self.linear, self.angular],
        )
    }

    /// Membership for the two components the shove RIDES ON, which is what a post-`Prepare` delivery
    /// verdict is defined on. Asking about the pose here would answer a question this verdict does
    /// not make: a hull whose `Position` history went missing still had its velocities restored, and
    /// the shove really did land.
    fn velocities(&self) -> Result<(), Unready> {
        prepare_restores(self.excluded, [self.linear, self.angular])
    }
}

/// EVERY ARCHETYPE CONDITION lightyear's `prepare_rollback` puts on a hull, asked in ONE place.
///
/// The two questions this module asks about a restore are separable and both are necessary. The
/// history predicates above ask what a restore WOULD RESOLVE if it happened. This asks whether the
/// hull is in the restore at all — and the seventh review found it asked once, as a query filter on
/// the offer, and never again.
///
/// `prepare_rollback::<C>` is registered once per predicted component; its query is
/// `(Entity, Option<&mut C>, &mut PredictionHistory<C>, Option<&mut ConfirmedHistory<C>>)` filtered
/// `Without<DisableRollback>`. Membership is therefore exactly two things, mirrored here rather than
/// paraphrased:
///
/// - `DisableRollback` excludes the hull from every component's restore AT ONCE. It is a marker this
///   codebase itself inserts in `Update` (`net::rig`'s late-prediction promotion) and removes in
///   `FixedLast`, and Bevy runs `RunFixedMainLoop` BEFORE `Update`, so an insertion necessarily
///   survives to the next `PreUpdate` — the schedule this module lives in. Reachable, not theoretical.
/// - `PredictionHistory<C>` is REQUIRED in that query, not optional. A component without one is
///   skipped even on a hull that is otherwise rolled back.
///
/// Asked at the REQUEST it refuses to claim a slot for a rollback that would restore nothing. Asked
/// after `RollbackSystems::Prepare` it is what makes the delivery verdict a statement about what
/// happened rather than a counterfactual: `restore_carries_the_shove` reads `ConfirmedHistory`, which
/// answers "what WOULD a restore here resolve" whether or not any restore touched this hull, so on
/// an excluded hull it reports a delivery that provably did not occur.
///
/// `N` is however many components the asking site's verdict covers — all four for readiness, the two
/// velocities for the post-`Prepare` delivery proof. Passing a subset is not a weaker question, it is
/// the question that site's answer is defined on. The two subsets are named on
/// [`RollbackParticipation`]; no site assembles its own.
///
/// A MIRROR OF A DEPENDENCY'S QUERY IS ONLY A MIRROR AT THE MOMENT IT IS WRITTEN, which was the
/// eighth review's point and is why this one is also CHECKED against the real thing:
/// `net::lead_zero_rollback`'s conformance matrix runs lightyear's own `RollbackSystems::Prepare`
/// over every combination of the conditions below and asserts that what was actually restored, per
/// component, is what this function says. A lightyear bump that adds or drops a membership condition
/// fails there instead of silently making this paraphrase wrong.
pub(super) fn prepare_restores<const N: usize>(
    rollback_disabled: bool,
    prediction_histories: [bool; N],
) -> Result<(), Unready> {
    if rollback_disabled {
        return Err(Unready::Excluded);
    }
    if !prediction_histories.into_iter().all(|present| present) {
        return Err(Unready::Unhistoried);
    }
    Ok(())
}

/// EVERY condition on a state restore at `restored_at`, asked in ONE place.
///
/// The offer and the request have to ask this of the SAME components. A fact is staged on one frame
/// and requested many frames later, so both are real evaluations of a moving answer — and the sixth
/// review found the request re-asking about only the two velocity histories. A late `Position`
/// removal therefore passed revalidation, the slot was claimed, `prepare_rollback` took `Position`
/// off the hull, and the fact closed as [`Retirement::Adopted`] because both velocities were carried.
/// This signature is what stops that recurring: no caller can ask half the question, and the query
/// that feeds it cannot silently shrink.
///
/// The seventh review found the same shape one level up, in the condition the histories are read
/// UNDER rather than in the histories themselves — whether the hull is in `prepare_rollback`'s query
/// at all. [`prepare_restores`] is that question and it is asked FIRST here, because a hull the
/// restore skips makes every history answer below a counterfactual.
///
/// # TWO PREDICATES, ONE PER ROLE — and the asymmetry is the point, not an oversight
///
/// - `LinearVelocity` and `AngularVelocity` CARRY the fact. A `HullShock` episode is a velocity
///   impulse and nothing else, so "was the shove delivered" is a question about these two alone.
///   They get the strong predicate, [`restore_carries_the_shove`]: the restore must resolve a value
///   the authority held at or after [`AuthoritativeFact::settled_at`].
/// - `Position` and `Rotation` carry NONE of the event. The impulse changes velocity; the pose only
///   moves as subsequent ticks integrate it, so at `settled_at` the authority's pose is still the
///   pre-impulse pose and a pose sample from before the close is the authority's genuine pose at the
///   restore target. The only thing that can go wrong for them is that the restore DESTROYS the
///   component or never reaches authority at all, which is exactly what [`authority_reaches`] asks —
///   the predicate the offer was already using for them.
///
/// # WHY ONE PREDICATE FOR ALL FOUR IS NOT AVAILABLE
///
/// It fails in both directions, so neither of the two can serve as the single answer.
///
/// `authority_reaches` on the velocities is the slice-3.7 defect verbatim: existence is satisfied by
/// a PRE-hit sample, so the module requests a rollback that installs a pre-hit velocity and then
/// retires the shove against it.
///
/// `restore_carries_the_shove` on the pose is strictly STRONGER than `authority_reaches` — it is
/// that predicate plus a recency clause — and a recency clause on the pose asks the restore to prove
/// something about a component the event never touched. Every verdict it changes turns a perfectly
/// restorable pose into a WAIT that protects nothing, and a wait costs a shove: the fact sits in the
/// single staging slot until the replay window drops it.
///
/// A PREVIOUS VERSION OF THIS PARAGRAPH JUSTIFIED THAT WITH A FALSE REACHABILITY CLAIM, and the
/// seventh review killed it. It said replication transmits only components that CHANGED, so a hull
/// standing still when it was shot publishes no new `Position` and the strong predicate would stall
/// it. Wrong: `net::physics` disables Avian's island sleeping for network physics, Avian's solver
/// writeback takes `&mut Position` and `&mut Rotation` for every solver body on every physics step,
/// and the hit and the episode's close both happen in `FixedUpdate` — before that step. So even a
/// numerically stationary hull has both pose components marked changed before the checkpoint that
/// carries its `HullShock`, and the pose is confirmed at or after `settled_at` in the ordinary case.
///
/// The asymmetry survives its own justification because it never rested on that shape. It rests on
/// what the components MEAN for this fact, which is the first half above and is not a claim about
/// replication at all. Codex confirmed the other half of the old argument independently: a forced
/// request makes `check_rollback` skip its policy branch, so the `SameAsPrecedent` markers that
/// branch writes are not available to date a pose forward. That is a real property; it is just not
/// what makes the weak predicate correct. NO SHIPPING SHAPE IS CLAIMED HERE, and the unit fixture
/// that pins the policy pins the policy and says so.
fn restore_is_deliverable(
    // The WHOLE-BODY membership verdict, which only `RollbackParticipation::whole_body` produces in
    // production. Taken as a verdict rather than as raw flags so that this signature cannot be
    // satisfied by a site that re-derived membership its own way.
    participates: Result<(), Unready>,
    position: Option<&ConfirmedHistory<Position>>,
    rotation: Option<&ConfirmedHistory<Rotation>>,
    linear: Option<&ConfirmedHistory<LinearVelocity>>,
    angular: Option<&ConfirmedHistory<AngularVelocity>>,
    restored_at: Tick,
    settled_at: Tick,
) -> Result<(), Unready> {
    // FIRST, because it decides whether the rest of this function is about anything. A hull outside
    // `prepare_rollback`'s query keeps every live component it has, whatever these histories hold.
    participates?;
    // The hull's whole rigid-body state has to come back from authority together — a pose restored
    // to `restored_at` beside a velocity that stayed at `now` is not a tick that ever existed on
    // either peer.
    if !authority_reaches(position, restored_at) || !authority_reaches(rotation, restored_at) {
        return Err(Unready::Pose);
    }
    if !restore_carries_the_shove(linear, angular, restored_at, settled_at) {
        return Err(Unready::Shove);
    }
    Ok(())
}

/// FIRST CONSUMER. Offer the authority's hull-shock episodes for adoption.
///
/// The trigger is the EXACT comparator [`hull_shock_mismatch`], not a magnitude: every field of
/// `HullShock` is discrete, and the whole point is that no magnitude gate can see a hit. What this
/// function owns beyond the trigger is the component-specific half of the readiness proof — that
/// the hull's prediction history is retained at the producing tick and that every authority-tracked
/// part of its rigid-body state can be restored there. The generic half lives in
/// [`request_staged_adoption`].
fn offer_hull_shock_adoptions(
    timeline: Res<LocalTimeline>,
    checkpoints: Option<Res<ReplicationCheckpointMap>>,
    hulls: Query<
        (
            Entity,
            &ConfirmedHistory<HullShock>,
            &PredictionHistory<HullShock>,
            Option<&crate::CombatantId>,
            // ROLLBACK MEMBERSHIP, read as DATA rather than expressed as a `Without` filter. The
            // filter was the shape the seventh review found: it made the condition invisible to the
            // request and to the post-`Prepare` proof, and a filter cannot be handed to the one
            // function that is supposed to own the whole question.
            RollbackParticipation,
            Option<&ConfirmedHistory<Position>>,
            Option<&ConfirmedHistory<Rotation>>,
            Option<&ConfirmedHistory<LinearVelocity>>,
            Option<&ConfirmedHistory<AngularVelocity>>,
        ),
        (With<Predicted>, With<Remote>),
    >,
    mut adoption: ResMut<AuthorityAdoption>,
    mut presentation: ResMut<ImpactPresentation>,
) {
    let Some(checkpoints) = checkpoints else {
        return;
    };
    // Replication is complete only through the last COMPLETED mutate checkpoint. A confirmed sample
    // past it may still be missing its siblings, and the rollback would restore a half-arrived tick.
    let (Some(completed), Some(checkpoint)) = (
        checkpoints.last_confirmed_tick(),
        checkpoints.last_confirmed_replicon_tick(),
    ) else {
        return;
    };
    let now = timeline.tick();
    for (
        hull,
        confirmed_shock,
        predicted_shock,
        combatant,
        participation,
        position,
        rotation,
        linear,
        angular,
    ) in &hulls
    {
        let Some((produced_at, authority)) =
            newest_present_at_or_before(confirmed_shock, completed)
        else {
            continue;
        };
        // The client has not RUN the producing tick yet. `PredictionHistory::get` is a floor
        // lookup, so asking for a future tick would compare the authority against an earlier
        // prediction under the future tick's label. Wait; the offer is re-derived every frame.
        if now - produced_at < 0 {
            continue;
        }
        let Some(predicted) = predicted_shock.get(produced_at) else {
            continue;
        };
        if !hull_shock_mismatch(authority, predicted) {
            continue;
        }
        // THE READINESS GATE, and an ECONOMY one — not the authoritative evaluation. There is one
        // staging slot, and a fact whose restore provably cannot be built on right now should not
        // occupy it. [`restore_is_deliverable`] is the whole question, over all four of the hull's
        // rigid-body histories, at the tick a rollback would target.
        //
        // IT DOES NOT SPEAK FOR THE FRAME THE FACT IS ACTUALLY REQUESTED ON, which can be many
        // frames later and over histories that have changed since. `request_staged_adoption` re-runs
        // this same predicate immediately before it claims the slot, and THAT is the evaluation the
        // transaction rests on; this one only keeps the slot free.
        //
        // This is a WAIT, not a drop: the offer is re-derived from replicated state every frame, and
        // it costs no request, no slot, and no rollback while it is unsatisfied. The velocity half is
        // also not a regime the shipping build reaches — every impulse of the episode CHANGES the
        // hull's velocity, so the message that first carries the episode's `HullShock` carries the
        // resulting velocity in the same replication group and stamps both with the same tick.
        let settled_at = Tick(authority.tick);
        if let Err(unready) = restore_is_deliverable(
            participation.whole_body(),
            position,
            rotation,
            linear,
            angular,
            produced_at,
            settled_at,
        ) {
            debug!(
                "client: hull-shock episode #{} on {} is not deliverable at tick {} — {}. Waiting; \
                 nothing has been requested.",
                authority.count,
                hull,
                produced_at.0,
                unready.reason(),
            );
            continue;
        }
        adoption.offer(
            AuthoritativeFact {
                id: FactId {
                    source: FactSource::HullShock,
                    entity: hull,
                    sequence: authority.count,
                    checkpoint,
                },
                cause: AdoptionCause::ExternalEvent,
                produced_at,
                settled_at,
                // THE EPISODE'S OWN HITS, in the authority's terms — its own `opened`/`tick` pair,
                // not `produced_at`. `produced_at` is the confirmed sample's tick, which is the right
                // RESTORE target but may sit later than the close if replication did not send that
                // tick; the span belongs to the episode and rides with it.
                visual: combatant.map(|victim| VisualClaim::for_episode(*victim, authority)),
            },
            now,
            &mut presentation,
        );
    }
}

/// Claim the forced-rollback slot for the staged fact, once every generic readiness condition holds.
///
/// THE WHOLE READINESS PREDICATE IS RE-ESTABLISHED HERE, and this — not the offer's gate — is the
/// authoritative evaluation. A fact is staged on one frame and can be requested many frames later,
/// because an [`AdoptionCause::ExternalEvent`] waits out [`ORDERING_BUDGET_TICKS`] for its spark;
/// re-offering the same identity deliberately leaves the staged fact untouched
/// ([`AuthorityAdoption::offer`]), and an offer pass whose own gate now fails simply skips the hull
/// and leaves the staged fact alone. Meanwhile the confirmed histories the answer is read from keep
/// changing under it — see [`restore_carries_the_shove`]. Proving readiness once at staging and
/// acting on it later is therefore acting on a stale answer, which is how a fact gets permanently
/// spent on a rollback that carries nothing.
///
/// WHOLE, not the delivery half. The sixth review found this re-asking only about the two VELOCITY
/// histories while the offer had proven all four, so a late `Position` removal survived into a
/// claimed rollback that deleted the component — and closed as an adoption, because every counter
/// this module keeps is defined on the velocities. [`restore_is_deliverable`] is now the single
/// question both sites ask, and its signature is what stops the query above shrinking again.
///
/// A failed revalidation is a WAIT, not a drop: the whole point is that the answer can change, so
/// nothing is claimed, nothing is tallied, and the fact stays staged to be reconsidered next frame.
/// The wait is bounded by the rollback-window check ABOVE it, which runs first every frame and
/// closes the fact loudly once `age` passes `RollbackPolicy::max_rollback_ticks`. `age` grows by at
/// least one per tick, so the stall cannot be unbounded and the give-up cannot be silent.
///
/// WHOLE also means the hull's ROLLBACK MEMBERSHIP, which the seventh review found latched at the
/// offer and never re-established. The offer's `Without<DisableRollback>` filter spoke for the frame
/// the fact was staged on; `net::rig` inserts that marker in `Update` on the late-prediction
/// promotion path and removes it in `FixedLast`, and Bevy runs `RunFixedMainLoop` before `Update`,
/// so an insertion necessarily survives into a later `PreUpdate`. The request claimed the slot on the
/// staging-time archetype, `prepare_rollback` skipped the marked hull entirely, and the fact closed
/// as an adoption having restored NOTHING. The condition is data on both queries now, and
/// [`prepare_restores`] is where it is asked.
fn request_staged_adoption(
    timeline: Res<LocalTimeline>,
    checkpoints: Option<Res<ReplicationCheckpointMap>>,
    // `IsSynced` gate, same spelling as `rollback_watchdog`'s: a claim made on a `Connected` but
    // not-yet-synced frame goes into a slot nobody consumes — lightyear's `check_rollback` skips
    // on its `Single<…, With<IsSynced<InputTimeline>>>` while the forced request survives — and
    // the first sync then rewrites `LocalTimeline` in `PostUpdate`, so the request is judged next
    // frame against a clock it was never claimed under and can be rejected as outside the replay
    // window, which suppresses every native policy check for that frame (rollback.rs consumes the
    // forced flag before the policy branches, rejected or not). Post-sync, the claim and its
    // consumption read the same `LocalTimeline` in the same `PreUpdate`, so the window arithmetic
    // here and in lightyear's `do_rollback` agree exactly; this gate is what makes that
    // same-clock invariant hold by construction instead of by the pre-sync tick accident.
    managers: Query<&PredictionManager, With<IsSynced<InputTimeline>>>,
    // ALL FOUR, because the offer proved all four — plus [`RollbackParticipation`], the archetype
    // conditions that decide whether `prepare_rollback` reaches this hull at all. Re-proving a
    // subset is proving a later frame's transaction on an earlier frame's world for whatever it left
    // out; [`restore_is_deliverable`]'s signature is what keeps this tuple honest, and the source
    // scan is what keeps the participation half from being spelled a second way.
    hulls: Query<(
        RollbackParticipation,
        Option<&ConfirmedHistory<Position>>,
        Option<&ConfirmedHistory<Rotation>>,
        Option<&ConfirmedHistory<LinearVelocity>>,
        Option<&ConfirmedHistory<AngularVelocity>>,
    )>,
    mut metadata: Option<ResMut<StateRollbackMetadata>>,
    mut slot: ResMut<ForcedRollbackSlot>,
    mut adoption: ResMut<AuthorityAdoption>,
    mut presentation: ResMut<ImpactPresentation>,
) {
    let Some(fact) = adoption.staged else {
        return;
    };
    let (Some(checkpoints), Ok(manager), Some(metadata)) =
        (checkpoints, managers.single(), metadata.as_mut())
    else {
        return;
    };
    let target = fact.produced_at;
    if adoption.requested {
        if metadata.forced_rollback_tick() == Some(target)
            || manager.get_rollback_start_tick() == Some(target)
        {
            return;
        }
        // The request was consumed without installing this fact. Retry rather than wedge the staged
        // adoption behind a stale bookkeeping flag.
        adoption.requested = false;
    }
    if checkpoints
        .last_confirmed_tick()
        .is_none_or(|confirmed| confirmed - target < 0)
    {
        return;
    }
    let now = timeline.tick();
    // The fact's AGE, which is how deep a replay reaching it would have to be. It is not the
    // ordering clock — see `waited` below and [`ORDERING_BUDGET_TICKS`].
    let age = now - target;
    if age < 0 {
        return;
    }
    if age > i32::from(manager.rollback_policy.max_rollback_ticks) {
        // No replay could reach the state that carries this fact. Say so and stop: retrying forever
        // is a storm, and manufacturing a client-side impulse would be a lie about what happened.
        warn!(
            "client: dropping authoritative fact {:?} #{} on {} — its producing tick {} is {age} \
             ticks old, past the {}-tick rollback window",
            fact.id.source,
            fact.id.sequence,
            fact.id.entity,
            target.0,
            manager.rollback_policy.max_rollback_ticks,
        );
        crate::trace::note_fact_event(|| {
            serde_json::json!({
                "ev": "dropped",
                "seq": fact.id.sequence,
                "age": age,
            })
        });
        adoption.close(fact);
        return;
    }
    // THE REVALIDATION. Read the histories AS THEY ARE NOW, at the tick this frame would target,
    // and read ALL of them: the offer's identical gate ran on the frame the fact was staged and
    // cannot speak for this one, for any component it covered.
    let unready = match hulls.get(fact.id.entity) {
        Err(_) => Some(Unready::Hull),
        Ok((participation, position, rotation, linear, angular)) => restore_is_deliverable(
            participation.whole_body(),
            position,
            rotation,
            linear,
            angular,
            target,
            fact.settled_at,
        )
        .err(),
    };
    if let Some(unready) = unready {
        debug!(
            "client: authoritative fact {:?} #{} on {} is not deliverable at tick {} right now \
             (event settled at {}) — {}. Waiting; nothing has been claimed and the fact stays \
             staged.",
            fact.id.source,
            fact.id.sequence,
            fact.id.entity,
            target.0,
            fact.settled_at.0,
            unready.reason(),
        );
        // Once per frame the fact stalls — the analyzer dedups, the cap absorbs a pathological
        // burst. A readiness stall is exactly what the ownership rows exist to make visible.
        crate::trace::note_fact_event(|| {
            serde_json::json!({
                "ev": "waiting",
                "seq": fact.id.sequence,
                "why": unready.reason(),
            })
        });
        return;
    }
    // LOCAL patience: ticks this client has held the fact, counted from the frame it was staged.
    //
    // Deliberately AFTER the revalidation: `clear_to_order` LATCHES a verdict and tallies it once
    // per fact, and spending that verdict on a frame the fact could not have been requested on
    // would report a wait shorter than the one the player actually got. It touches nothing but this
    // module's own two resources, so it cannot move a confirmed history between the revalidation
    // and the claim below.
    let waited = adoption.staged_at.map_or(0, |staged| now - staged);
    if !clear_to_order(&mut adoption, &mut presentation, fact, waited) {
        return;
    }
    adoption.requested = slot.claim(metadata, target, fact.cause);
    if adoption.requested {
        crate::trace::note_fact_event(|| {
            serde_json::json!({
                "ev": "requested",
                "seq": fact.id.sequence,
                "target": target.0,
                "waited": waited,
            })
        });
    }
}

/// THE ORDERING RULE, which is BEST EFFORT. Whether this module will request the staged fact yet,
/// given what the player has been SHOWN.
///
/// A fact carrying a [`VisualClaim`] waits for one of ITS OWN hits to have been drawn, so the shove
/// this module orders does not outrun its own spark; see the module doc for why that direction is
/// the only one that reads as broken, for how the claim identifies the hit, and for why nothing here
/// can stop a rollback ordered elsewhere from landing the same shove early. A fact with no claim —
/// an [`AdoptionCause::Misprediction`] — has no visual to be ordered against and is never held. The
/// verdict latches on [`AuthorityAdoption::ordering`], so the tally counts facts, not the frames a
/// fact spent being retried.
///
/// EVERY EXIT WRITES THAT LATCH, which is what makes it readable as a state rather than as an
/// absence. The three ways out are distinct facts about the rule — it had nothing to hold this fact
/// for, it is holding it, it let it go — and [`retirement`] needs the middle one alone. The state it
/// must NOT be able to observe is the one no exit here writes: `Ordering::Unasked`, meaning this
/// function was never reached on this fact.
///
/// `waited` is LOCAL ticks since staging, not the fact's age.
fn clear_to_order(
    adoption: &mut AuthorityAdoption,
    presentation: &mut ImpactPresentation,
    fact: AuthoritativeFact,
    waited: i32,
) -> bool {
    let Some(claim) = fact.visual else {
        // Nothing to be ordered against, so the rule is not holding this fact and never will be.
        // That is RELEASED, not pending: it is precisely the state a bypass cannot occur in.
        adoption.ordering = Ordering::Released;
        return true;
    };
    if adoption.ordering == Ordering::Released {
        return true;
    }
    let shown = presentation.shown_for(claim);
    if !shown && waited < ORDERING_BUDGET_TICKS {
        adoption.ordering = Ordering::HoldingForSpark;
        return false;
    }
    adoption.ordering = Ordering::Released;
    presentation.resolve(shown, waited);
    // The release verdict, latched once per fact: `shown` separates on-impact from on-budget —
    // the same split the tally counts, but per fact and joinable against `staged`/`spark` rows.
    crate::trace::note_fact_event(|| {
        serde_json::json!({
            "ev": "released",
            "seq": fact.id.sequence,
            "shown": shown,
            "waited": waited,
        })
    });
    if !shown {
        // Not a correctness failure — the shove still lands at its authoritative tick. It is the
        // measurement the relaxed ordering requirement asked for: how often the visual never showed
        // up inside the budget, which is what would justify a presentation commit barrier.
        warn!(
            "client: authoritative fact {:?} #{} on {} landed UNORDERED — no armor impact on \
             combatant {} was drawn for authority ticks {}..={} within {ORDERING_BUDGET_TICKS} \
             ticks of staging ({} released on budget so far)",
            fact.id.source,
            fact.id.sequence,
            fact.id.entity,
            claim.victim.0,
            claim.from.0,
            claim.through.0,
            presentation.tally().released_on_budget,
        );
    }
    true
}

/// Record what lightyear ACTUALLY installed, and close the staged transaction against it.
///
/// Runs after `RollbackSystems::Prepare`, so `PredictionManager::get_rollback_start_tick` is the
/// installed target and `prepare_rollback` has already restored every history against it.
///
/// Two ways a staged fact ends here, and they must not be conflated. OURS: this module's own claim
/// is what lightyear installed, so the rollback it started is the one that was ordered against the
/// spark. NOT OURS: some other subsystem's CONFIRMED-STATE rollback landed at or past the producing
/// tick AND its restore is established to have carried the authority's post-hit velocities onto the
/// live hull. The fact is spent either way — re-requesting it would buy a second render hitch for
/// state that is already live — but only the first is an adoption this module ordered, and the
/// second is counted as a BYPASS whenever it beat the spark. A rollback SHALLOWER than the producing
/// tick, one whose confirmed samples predate the hit, and an INPUT rollback's restore at any depth
/// carry none of this and retire nothing.
///
/// `carried` is asked of BOTH routes, ours included: an installed claim of our own that restored a
/// pre-event velocity is [`Retirement::Undelivered`], not an adoption. See [`retirement`].
///
/// THE CONFIRMED CLAIM IS A LOCAL, and slice 4 made it one. It used to be written back to
/// [`ForcedRollbackSlot`] so `net::render_error` could read it in `PostUpdate`; the field is gone
/// and the value now lives only in this function's frame. The presentation signal this function
/// emits instead — [`SharpCorrection`] — is derived from the ESTABLISHED [`Retirement`] below,
/// after `carried` has been read, and not from the claim's cause tag.
fn confirm_forced_rollback(
    managers: Query<(&PredictionManager, Option<&Rollback>)>,
    hulls: Query<(
        RollbackParticipation,
        Option<&ConfirmedHistory<LinearVelocity>>,
        Option<&ConfirmedHistory<AngularVelocity>>,
    )>,
    mut slot: ResMut<ForcedRollbackSlot>,
    mut adoption: ResMut<AuthorityAdoption>,
    mut presentation: ResMut<ImpactPresentation>,
    mut sharp: MessageWriter<SharpCorrection>,
) {
    let manager = managers.single().ok();
    let started = manager.and_then(|(manager, _)| manager.get_rollback_start_tick());
    let kind = manager
        .and_then(|(_, rollback)| rollback)
        .map(RestoresFrom::of);
    // TAKEN INTO A LOCAL and used below without being stored: the claim is cleared every frame
    // whether or not lightyear installed it, and the confirmation it produces exists only here.
    let installed = slot.claim.take().filter(|(tick, _)| started == Some(*tick));
    if let Some((tick, cause)) = installed {
        debug!(
            "client: forced rollback installed at tick {} — cause {cause:?}",
            tick.0,
        );
    }
    let (Some(fact), Some(started), Some(kind)) = (adoption.staged, started, kind) else {
        return;
    };
    // WHAT THE RESTORE ACTUALLY PUT ON THE HULL. Read from the same histories `prepare_rollback`
    // read, so "someone else's rollback delivered the shove" is established rather than inferred
    // from its start tick. A despawned hull answers `Err` and carries nothing.
    //
    // TWO QUESTIONS, and the seventh review found only one of them being asked. A confirmed history
    // answers "what WOULD a restore at `started` resolve here" — it says nothing about whether any
    // restore touched this hull. [`prepare_restores`] is the other half: unless the hull was in
    // `prepare_rollback`'s query, the lookup below is a counterfactual and reading a delivery out of
    // it records a shove that provably never landed.
    //
    // VELOCITIES ONLY, and deliberately NOT [`restore_is_deliverable`]. This is a question about the
    // PAST — the rollback has already been prepared — and the only thing that can retire the fact is
    // whether the event's Δv is now on the live hull. Whatever that restore did to the pose is
    // lightyear's doing and is not this fact's to answer for; re-asking the pose predicate here
    // would leave a fact staged for a shove that is already live, and it would be re-requested for
    // state that cannot change. The readiness question that DOES cover the pose is asked before the
    // claim, where refusing still buys something. `DisableRollback` is entity-level so it is fully
    // covered by the velocity half; the per-component half is asked of exactly the two components
    // this verdict speaks about.
    //
    // THE ARCHETYPE CANNOT HAVE MOVED between `Prepare` and here. Both are in one `PreUpdate` with
    // only Bevy's own sync point between them; `prepare_rollback`'s commands touch the restored
    // component and `PreviousVisual` and nothing else; and the only writer of `DisableRollback` in
    // lightyear is `check_rollback`'s deterministic skip-despawn bookkeeping, which runs BEFORE
    // `Prepare` and only over entities in `PredictionManager::deterministic_skip_despawn`.
    let carried = hulls
        .get(fact.id.entity)
        .is_ok_and(|(participation, linear, angular)| {
            participation.velocities().is_ok()
                && restore_carries_the_shove(linear, angular, started, fact.settled_at)
        });
    let outcome = retirement(&adoption, &presentation, installed, started, kind, carried);
    // THE PRESENTATION SIGNAL, produced from the ESTABLISHED retirement and not from the raw cause.
    // `keeps_the_seam_sharp` is an exhaustive match over [`Retirement`], so a new outcome cannot be
    // added without deciding this question for it.
    if outcome.is_some_and(Retirement::keeps_the_seam_sharp)
        && fact.cause == AdoptionCause::ExternalEvent
    {
        sharp.write(SharpCorrection {
            entity: fact.id.entity,
            restored_from: started,
        });
    }
    // The terminal ownership row: which route spent the fact, and whether the restore carried the
    // shove. `bypassed` is `Delivered { spark_pending: true }` — a rollback this module did not
    // order landing before the spark — kept as its own label so the A/B can count the class the
    // inert comparator exists to close.
    if let Some(route) = outcome
        && route != Retirement::Keep
    {
        crate::trace::note_fact_event(|| {
            serde_json::json!({
                "ev": "retired",
                "seq": fact.id.sequence,
                "at": fact.produced_at.0,
                "route": match route {
                    Retirement::Adopted => "adopted",
                    Retirement::Delivered { spark_pending: false } => "delivered",
                    Retirement::Delivered { spark_pending: true } => "bypassed",
                    Retirement::Undelivered => "undelivered",
                    Retirement::Keep => unreachable!("filtered above"),
                },
                "started": started.0,
                "carried": carried,
            })
        });
    }
    match outcome {
        None | Some(Retirement::Keep) => return,
        Some(Retirement::Undelivered) => {
            presentation.note_undelivered();
            error!(
                "client: authoritative fact {:?} #{} on {} was ADOPTED BUT NOT DELIVERED — this \
                 module's own forced rollback was installed at tick {} and `prepare_rollback` \
                 either skipped this hull (`DisableRollback`, or a missing `PredictionHistory`) or \
                 restored a hull velocity older than the event's settling tick {}. The shove is \
                 LOST: the fact is closed rather than re-requested, because the confirmed histories \
                 a retry would read are the same ones this restore just read. \
                 `request_staged_adoption` revalidated exactly this in this frame immediately \
                 before claiming, so reaching here means that revalidation and `prepare_rollback` \
                 disagree — suspect a lightyear change to `check_rollback`'s forced-request \
                 shortcut, to `prepare_rollback`'s query or lookup, or a new writer of \
                 `DisableRollback` between `RollbackSystems::Check` and `RollbackSystems::Prepare` \
                 ({} undelivered so far)",
                fact.id.source,
                fact.id.sequence,
                fact.id.entity,
                started.0,
                fact.settled_at.0,
                presentation.tally().undelivered,
            );
        }
        Some(Retirement::Adopted) => debug!(
            "client: adopted authoritative fact {:?} #{} on {} (cause {:?}, checkpoint {:?}) — \
             restored end of tick {}",
            fact.id.source,
            fact.id.sequence,
            fact.id.entity,
            fact.cause,
            fact.id.checkpoint,
            fact.produced_at.0,
        ),
        Some(Retirement::Delivered {
            spark_pending: false,
        }) => debug!(
            "client: authoritative fact {:?} #{} on {} was carried by a rollback at tick {} this \
             module did not order — its spark was already drawn, or it had none to wait for, so \
             nothing was missed",
            fact.id.source, fact.id.sequence, fact.id.entity, started.0,
        ),
        Some(Retirement::Delivered {
            spark_pending: true,
        }) => {
            presentation.note_bypass();
            warn!(
                "client: authoritative fact {:?} #{} on {} was BYPASSED — a rollback this client \
                 did not order started at tick {} and restored the fact's tick {} while it was \
                 still waiting for its spark ({} bypassed so far)",
                fact.id.source,
                fact.id.sequence,
                fact.id.entity,
                started.0,
                fact.produced_at.0,
                presentation.tally().bypassed,
            );
        }
    }
    adoption.close(fact);
}

/// What a staged fact's transaction does against the rollback lightyear installed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Retirement {
    /// This rollback carries nothing of the fact. It stays staged and is requested again.
    Keep,
    /// This module's own claim was installed: the fact landed the way it was ordered.
    Adopted,
    /// This module's own claim was installed AND the restore carried nothing. The fact is spent
    /// having delivered nothing, and says so — see [`OrderingTally::undelivered`].
    Undelivered,
    /// A confirmed-state rollback this module did not order restored the fact's producing tick from
    /// authority ONTO A HULL IT REACHED, so the shove is already live. `spark_pending` is whether it
    /// beat the visual — a BYPASS — and is a question about what has been DRAWN, not about whether
    /// the ordering rule happens to have latched a verdict yet.
    Delivered { spark_pending: bool },
}

impl Retirement {
    /// Whether the correction this rollback produces on the fact's own root carries an authoritative
    /// EVENT, and must therefore be presented sharp rather than smoothed away.
    ///
    /// The question is "is the hit on the live hull because of this rollback", which is exactly what
    /// the two DELIVERING outcomes assert and the two others deny:
    ///
    /// - [`Retirement::Adopted`] — our claim was installed AND `carried`. The hit is live.
    /// - [`Retirement::Delivered`] — somebody else's confirmed-state rollback carried it onto a hull
    ///   it reached. The hit is equally live, and `spark_pending` is a telemetry question about
    ///   ordering, not about delivery, so BOTH of its variants are sharp.
    /// - [`Retirement::Keep`] — nothing of the fact landed; the correction, if any, is ordinary
    ///   misprediction and smoothing it hides nothing the player is owed.
    /// - [`Retirement::Undelivered`] — our claim was installed and the restore carried a PRE-hit
    ///   velocity. There is no hit in this correction to keep sharp; going sharp here would expose a
    ///   seam for nothing. This is the case a reader of [`AdoptionCause`] would get backwards.
    ///
    /// EXHAUSTIVE ON PURPOSE — no wildcard arm, so a fifth outcome is a compile error here rather
    /// than a silent "not sharp".
    fn keeps_the_seam_sharp(self) -> bool {
        match self {
            Retirement::Adopted | Retirement::Delivered { .. } => true,
            Retirement::Keep | Retirement::Undelivered => false,
        }
    }
}

/// WHERE a rollback restores predicted state FROM. The one property of a rollback that decides
/// whether it can carry an authoritative fact at all.
///
/// `prepare_rollback` branches on `lightyear::prelude::Rollback`: `FromState` restores a component
/// that has a [`ConfirmedHistory`] from that history, `FromInputs` restores every component from the
/// client's own [`PredictionHistory`]. Named for the branch rather than for the cause because the
/// branch is the whole reason the distinction matters here.
///
/// SCOPE: this describes the RESTORE only. lightyear's `snap_to_confirmed_during_rollback` runs on
/// every replayed tick of every rollback kind — it takes `Single<&Rollback>` and never reads the
/// variant — so an input rollback's REPLAY can still install an exact confirmed sample it passes
/// through. See the module doc; that path costs one redundant rollback and never a lost shove.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RestoresFrom {
    /// The authority's confirmed history. Only this can deliver a fact the client cannot predict at
    /// RESTORE time, which is the only time this module can observe.
    Authority,
    /// The client's own prediction history — for a hull the client never knew was hit, its own
    /// un-hit state. The restore re-installs the misprediction and delivers nothing.
    OwnPrediction,
}

impl RestoresFrom {
    fn of(rollback: &Rollback) -> Self {
        match rollback {
            Rollback::FromState => Self::Authority,
            Rollback::FromInputs => Self::OwnPrediction,
        }
    }
}

/// THE RETIREMENT RULE, split out from [`confirm_forced_rollback`] because a `PredictionManager`'s
/// rollback state cannot be fabricated in a unit test and this decision is where the whole
/// correctness of the transaction sits.
///
/// Two things have to hold before a rollback spends a staged fact, and each has cost a review round:
///
/// - It must restore from AUTHORITY. An input rollback re-installs the client's own prediction, so
///   closing a fact against one loses the shove outright. Our client disables input rollback, but
///   this decides from the rollback in hand rather than from that configuration.
/// - OURS is a claim IDENTITY, not a coincidence of ticks. [`ForcedRollbackSlot`] is global and
///   lightyear rolls back for its own reasons — a `Position` mismatch, `net::watchdog` — so "a
///   rollback started at our target tick" is satisfied by rollbacks this module never asked for, and
///   retiring against one as an ADOPTION would credit the ordering rule for a shove it did not order.
/// - DELIVERY IS ESTABLISHED, not inferred from depth. `carried` is
///   [`restore_carries_the_shove`]'s answer, read off the same confirmed histories
///   `prepare_rollback` restored from. A deep-enough start tick over a confirmed history whose newest
///   sample predates the hit restores a PRE-hit velocity; closing the fact against that would spend
///   it having delivered nothing and would inflate [`OrderingTally::bypassed`] with rollbacks that
///   bypassed nothing.
///
/// OUR OWN INSTALLED CLAIM IS NOT PROOF OF DELIVERY EITHER, and treating it as one was the fourth
/// review's finding. The earlier rule returned `Adopted` from the claim identity alone, without
/// consulting `carried`, so a rollback we ordered onto a confirmed history that resolved to a
/// pre-event velocity closed the fact permanently having delivered nothing — silently, because
/// `Adopted` is the success path. The argument for it ("otherwise it would loop") explains why blind
/// re-requesting is undesirable; it does not justify RECORDING a delivery that did not happen.
///
/// So `Adopted` now means installed AND carried, and installed-but-not-carried is
/// [`Retirement::Undelivered`]: loud, counted, and still closed — closing is what bounds it, since
/// the histories a retry would read are the ones this restore just read, and nothing about the same
/// target tick can improve.
///
/// # IS `Undelivered` REACHABLE? The proof, and what it rests on
///
/// Against the PINNED lightyear 0.28 it is unreachable, and the argument is three steps rather than
/// an assurance. It replaces a false one — "the offer's gate settled this" — which ignored that the
/// offer runs on a different FRAME from the request.
///
/// 1. THE BRANCH IS SAME-FRAME. It needs `adoption.requested` AND
///    `installed == Some((produced_at, cause))`. `installed` is a LOCAL in
///    [`confirm_forced_rollback`], derived from [`ForcedRollbackSlot::claim`], which that function
///    takes every frame — so it is non-`None` only in the `PreUpdate` run in which
///    [`request_staged_adoption`] claimed. That system revalidates [`restore_carries_the_shove`]
///    immediately before claiming.
/// 2. NOTHING WRITES CONFIRMED HISTORY IN BETWEEN. Between the revalidation and
///    `RollbackSystems::Prepare` lie `net::watchdog`'s rollback check (read-only),
///    `RollbackSystems::Check` and `RollbackSystems::RemoveDisable`. `check_rollback` consumes the
///    forced request FIRST and then skips its whole policy branch — the only writer of
///    `ConfirmedHistory` REACHABLE IN THIS GAP — and our claim guarantees the request is pending.
///    Replicon receive already ran, before this module.
///
///    That is a claim about the GAP and not about the codebase, and the difference matters because
///    the stronger version of it is false. `ConfirmedHistory::add_unchanged` has a second caller,
///    `ConfirmedHistory::push_unchanged`, which lightyear's interpolation invokes. It is irrelevant
///    here for a reason that has nothing to do with call counts: that path runs in `Update`, on
///    `Interpolated` entities, and this gap is inside one `PreUpdate` on a `Predicted` hull.
/// 3. THE TWO LOOKUPS AGREE. `prepare_rollback` restores
///    `ConfirmedHistory::get_state_at_or_before(rollback_tick)` and the predicate asks that exact
///    call plus the tick it resolved at, over the same component, at the same tick.
///
/// Steps 2 and 3 are properties of a DEPENDENCY, not of this crate, and a lightyear bump can retire
/// either without touching a line here. That is why the branch survives its own proof: the counter
/// is the tripwire that says the proof stopped holding, and "unreachable" is what the previous three
/// rounds each believed about the branch they were about to lose a shove in.
///
/// # `spark_pending` IS A QUESTION ABOUT WHAT WAS DRAWN, not about a latch
///
/// [`OrderingTally::bypassed`] claims a rollback this module did not order beat the fact's own
/// spark, so the only fact that can answer it is whether that spark has been drawn — read from
/// [`ImpactPresentation`], the same ledger [`clear_to_order`] reads. The seventh review found it
/// being answered from `AuthorityAdoption::ordering.is_none()` instead, which was true in three
/// different situations: the rule had not been consulted, the fact had no visual at all, and the
/// rule was genuinely holding it. Two of those inflate the count.
///
/// ONE RESIDUAL SURVIVES AND `bypassed` MUST BE READ AS AN UPPER BOUND. A spark drawn before its
/// fact was staged is known to this module only through [`MAX_PRESENTED_HITS`]'s FIFO, and if it was
/// evicted before the staging that seeds [`WatchedSpark`] then `shown_for` answers false here for a
/// spark the player saw. See the note on that constant for why the exposed window is the whole
/// pre-staging span rather than one replication gap. No delivery decision reads it; this counter
/// does.
///
/// The latch's remaining job here is the one it can do: RELEASED is released, by either verdict, and
/// a rollback carrying a fact the rule already let go missed nothing. Everything else asks the
/// ledger, so a spark drawn since the frame the rule last ran counts.
fn retirement(
    adoption: &AuthorityAdoption,
    presentation: &ImpactPresentation,
    installed: Option<(Tick, AdoptionCause)>,
    started: Tick,
    restores_from: RestoresFrom,
    carried: bool,
) -> Option<Retirement> {
    let fact = adoption.staged?;
    if restores_from == RestoresFrom::OwnPrediction {
        return Some(Retirement::Keep);
    }
    if adoption.requested && installed == Some((fact.produced_at, fact.cause)) {
        return Some(if carried {
            Retirement::Adopted
        } else {
            Retirement::Undelivered
        });
    }
    // A rollback SHALLOWER than the producing tick restores pre-hit state and replays forward from
    // it, so it carries none of the authority's post-hit hull velocity. Nothing to retire. Same
    // answer, for the same reason, when a deep-enough rollback restored from a confirmed sample that
    // predates the hit.
    if started - fact.produced_at < 0 || !carried {
        return Some(Retirement::Keep);
    }
    Some(Retirement::Delivered {
        spark_pending: adoption.ordering != Ordering::Released
            && fact
                .visual
                .is_some_and(|claim| !presentation.shown_for(claim)),
    })
}

/// THE EXTENSION POINT. A new kind of authoritative fact adds its producer here — a system that
/// derives its offer from replicated state and hands it to [`AuthorityAdoption::offer`]
/// unconditionally every frame. Everything after the offer (readiness, single-slot arbitration,
/// dedupe, give-up) is already written.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct OfferAuthoritativeFacts;

/// Mounted from the SHARED `net::protocol::plugin`, like both halves of the hull-shock seam it
/// serves. Every system here is inert without a `PredictionManager` and a `StateRollbackMetadata`,
/// so the authority mounts it and never acts on it.
pub(super) fn plugin(app: &mut App) {
    app.init_resource::<AuthorityAdoption>()
        .init_resource::<ForcedRollbackSlot>()
        .init_resource::<ImpactPresentation>()
        // The presentation signal `net::render_error` consumes. Registered HERE, with the producer,
        // so the queue exists on every composition that can emit one — including the server, which
        // never reaches the emitting branch, and every fixture that mounts only this plugin.
        .add_message::<SharpCorrection>()
        .add_observer(reset_adoption_state)
        .add_observer(note_presented_impact)
        // The request must exist before lightyear's rollback check consumes it, and before the
        // watchdog's — an authoritative event outranks a coarse "my prediction drifted" claim on
        // the single slot.
        .add_systems(
            PreUpdate,
            (
                offer_hull_shock_adoptions.in_set(OfferAuthoritativeFacts),
                request_staged_adoption.after(OfferAuthoritativeFacts),
            )
                .after(ReplicationSystems::Receive)
                .before(super::watchdog::RollbackWatchdog)
                .before(RollbackSystems::Check),
        )
        // Lightyear has installed the rollback tick and restored every history at this seam.
        .add_systems(
            PreUpdate,
            confirm_forced_rollback
                .after(RollbackSystems::Prepare)
                .before(RollbackSystems::Rollback),
        );
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// The combatant every fixture's hull belongs to.
    const VICTIM: crate::CombatantId = crate::CombatantId(3);
    /// Somebody else, being shot at the same time.
    const BYSTANDER: crate::CombatantId = crate::CombatantId(4);
    /// [`restore_carries_the_shove`]'s verdict, for the [`retirement`] fixtures that are about the
    /// OTHER two questions. The predicate itself has its own tests, over real histories.
    const CARRIED: bool = true;

    /// The authority's post-hit hull velocities — the measured 88 mm Δv the sibling fixtures deposit.
    const AUTHORITY_LINEAR: Vec3 = Vec3::new(0.0, 0.0, -0.138_3);
    const AUTHORITY_ANGULAR: Vec3 = Vec3::new(0.191_0, 0.0, 0.052_0);

    /// The episode a hull's `sequence`-th [`HullShock`] publishes, closed at `tick`.
    ///
    /// THE SPAN IS DERIVED FROM THE SEQUENCE, and that is the point of the helper rather than an
    /// economy. Production derives both from one `HullShockLedger`, so they cannot disagree: a FIRST
    /// episode has no open episode to defer behind — `close_episode` publishes it on the tick it was
    /// armed — so it spans that single tick, and only a LATER episode can have been deferred, by at
    /// most `SHOCK_EPISODE_TICKS − 1` ticks. The previous helper took the span and the sequence
    /// separately and always built a deferred span, so every `sequence: 1` caller was asserting on
    /// exactly the `count: 1, opened != tick` shape the slice before this one removed from the wire
    /// fixtures. No caller here can ask for it.
    fn episode(sequence: u32, tick: u32) -> HullShock {
        HullShock {
            count: sequence,
            tick,
            opened: if sequence > 1 {
                tick - (SHOCK_EPISODE_TICKS - 1)
            } else {
                tick
            },
            cause: crate::ballistics::ShockCause::Perforation,
        }
    }

    /// The offer `offer_hull_shock_adoptions` builds for a hull's `sequence`-th episode, in the
    /// widest span that sequence can have.
    fn fact(entity: Entity, sequence: u32, checkpoint: u32, tick: u32) -> AuthoritativeFact {
        episode_fact(entity, checkpoint, episode(sequence, tick))
    }

    /// The offer `offer_hull_shock_adoptions` builds for `episode`.
    ///
    /// The identity sequence is READ OFF the episode, never passed beside it: production takes both
    /// from `HullShock::count` (`sequence: authority.count`), so a fixture that could set them apart
    /// could assert on a fact no authority can publish. `produced_at` and `settled_at` coincide here
    /// because the sample tick is the close tick; [`arriving_at`] is how a fixture separates them.
    fn episode_fact(entity: Entity, checkpoint: u32, episode: HullShock) -> AuthoritativeFact {
        AuthoritativeFact {
            id: FactId {
                source: FactSource::HullShock,
                entity,
                sequence: episode.count,
                checkpoint: RepliconTick::new(checkpoint),
            },
            cause: AdoptionCause::ExternalEvent,
            produced_at: Tick(episode.tick),
            settled_at: Tick(episode.tick),
            visual: Some(VisualClaim::for_episode(VICTIM, &episode)),
        }
    }

    /// The same fact, as it looks when REPLICATION materialized it later than the tick its state
    /// settled on — the ordinary shape at any send interval above one tick, and the one that
    /// separates the restore target from the event's own tick.
    fn arriving_at(fact: AuthoritativeFact, produced_at: Tick) -> AuthoritativeFact {
        assert!(
            produced_at - fact.settled_at >= 0,
            "a confirmed sample cannot predate the state it carries",
        );
        AuthoritativeFact {
            produced_at,
            ..fact
        }
    }

    /// A COALESCED episode: its first hit landed at `opened` inside an already-open episode's
    /// window, so it published at `tick` when that window expired rather than on its own tick.
    ///
    /// `count: 2` is forced, not chosen: a deferred episode had something to defer behind, so it is
    /// never a hull's first.
    fn deferred_episode(opened: u32, tick: u32) -> HullShock {
        assert!(
            opened < tick && tick - opened < SHOCK_EPISODE_TICKS,
            "a deferred episode opens strictly before it closes and inside one window",
        );
        HullShock {
            opened,
            ..episode(2, tick)
        }
    }

    /// The episode a FRESH `HullShockLedger` publishes for its first hit: no open episode to defer
    /// behind, so it closes on the tick it was armed and spans that single tick.
    fn first_episode(tick: u32) -> HullShock {
        episode(1, tick)
    }

    /// A hit the authority resolved at server tick `tick`, on `victim`, that this client has drawn.
    fn drew(tick: u32, victim: crate::CombatantId) -> PresentedHit {
        PresentedHit {
            tick: Tick(tick),
            victim: Some(victim),
        }
    }

    fn hull() -> Entity {
        Entity::from_raw_u32(7).expect("a non-placeholder test entity")
    }

    /// The local tick a fixture stages on. Deliberately NOT `produced_at`: the two clocks are
    /// different and every assertion below has to survive them disagreeing.
    const STAGED_AT: Tick = Tick(140);

    /// The delivery ledger's whole job: a producer that re-offers the same fact every frame — which
    /// is exactly what `offer_hull_shock_adoptions` does — must request exactly once.
    #[test]
    fn a_re_offered_fact_is_requested_once() {
        let mut adoption = AuthorityAdoption::default();
        let mut presentation = ImpactPresentation::default();
        let episode = fact(hull(), 1, 50, 100);

        assert_eq!(
            adoption.offer(episode, STAGED_AT, &mut presentation),
            Offer::Staged
        );
        assert_eq!(
            adoption.offer(episode, STAGED_AT + 3, &mut presentation),
            Offer::Staged
        );
        assert_eq!(adoption.staged, Some(episode));
        assert_eq!(
            adoption.staged_at,
            Some(STAGED_AT),
            "the patience clock starts when the fact is FIRST staged; a re-offer must not reset it",
        );

        adoption.close(episode);
        assert_eq!(adoption.staged, None);
        assert_eq!(adoption.staged_at, None);
        assert_eq!(
            adoption.offer(episode, STAGED_AT, &mut presentation),
            Offer::AlreadyAdopted
        );
    }

    /// A re-send of the SAME episode under a later checkpoint has a different [`FactId`], so exact
    /// dedupe alone would let it through and buy a second rollback for one hit. The per-entity
    /// sequence watermark is what stops it.
    #[test]
    fn a_later_checkpoint_carrying_the_same_episode_is_not_a_new_fact() {
        let mut adoption = AuthorityAdoption::default();
        let mut presentation = ImpactPresentation::default();
        adoption.close(fact(hull(), 1, 50, 100));

        for stale in [fact(hull(), 1, 51, 104), fact(hull(), 0, 51, 104)] {
            assert_eq!(
                adoption.offer(stale, STAGED_AT, &mut presentation),
                Offer::NotNewer
            );
        }
        assert_eq!(
            adoption.offer(fact(hull(), 2, 51, 104), STAGED_AT, &mut presentation),
            Offer::Staged,
        );
    }

    /// A despawn/respawn gives the replicated hull a new `Entity`, and the new incarnation's first
    /// episode must not be suppressed by the old one's watermark.
    #[test]
    fn a_new_entity_incarnation_starts_a_new_sequence() {
        let mut adoption = AuthorityAdoption::default();
        let mut presentation = ImpactPresentation::default();
        adoption.close(fact(hull(), 9, 50, 100));

        let reborn = Entity::from_raw_u32(8).expect("a second non-placeholder test entity");
        assert_eq!(
            adoption.offer(fact(reborn, 1, 51, 104), STAGED_AT, &mut presentation),
            Offer::Staged
        );
    }

    /// One transaction at a time. A second entity's fact waits instead of overwriting the staged
    /// one; its producer re-offers it next frame.
    #[test]
    fn a_second_fact_does_not_evict_the_staged_one() {
        let mut adoption = AuthorityAdoption::default();
        let mut presentation = ImpactPresentation::default();
        let first = fact(hull(), 1, 50, 100);
        let other = Entity::from_raw_u32(8).expect("a second non-placeholder test entity");

        assert_eq!(
            adoption.offer(first, STAGED_AT, &mut presentation),
            Offer::Staged
        );
        assert_eq!(
            adoption.offer(fact(other, 1, 50, 100), STAGED_AT, &mut presentation),
            Offer::SlotBusy,
        );
        assert_eq!(adoption.staged, Some(first));
    }

    /// The single-slot rule: a busy slot pointing elsewhere is left alone, an agreeing one is
    /// shared, and the sharp cause wins the tag when two subsystems land on the same tick.
    #[test]
    fn the_forced_rollback_slot_admits_one_target_and_the_sharp_cause() {
        let mut slot = ForcedRollbackSlot::default();
        let mut metadata = StateRollbackMetadata::default();

        assert!(slot.claim(&mut metadata, Tick(90), AdoptionCause::Misprediction));
        assert_eq!(metadata.forced_rollback_tick(), Some(Tick(90)));
        assert!(!slot.claim(&mut metadata, Tick(80), AdoptionCause::ExternalEvent));
        assert_eq!(
            metadata.forced_rollback_tick(),
            Some(Tick(90)),
            "a losing claim must not narrow the winner's target",
        );

        assert!(slot.claim(&mut metadata, Tick(90), AdoptionCause::ExternalEvent));
        assert_eq!(slot.claim, Some((Tick(90), AdoptionCause::ExternalEvent)));
        assert!(slot.claim(&mut metadata, Tick(90), AdoptionCause::Misprediction));
        assert_eq!(
            slot.claim,
            Some((Tick(90), AdoptionCause::ExternalEvent)),
            "a correct physical event must never be re-tagged as the client's own error",
        );
    }

    /// Stage a fact and run the ordering rule against it exactly as `request_staged_adoption` does.
    /// `waited` is LOCAL ticks since staging, which is the only clock the budget runs on.
    fn order(
        adoption: &mut AuthorityAdoption,
        presentation: &mut ImpactPresentation,
        episode: AuthoritativeFact,
        waited: i32,
    ) -> bool {
        clear_to_order(adoption, presentation, episode, waited)
    }

    /// THE ORDERING RULE. A shove waits for one of ITS OWN hits to have been drawn, and lands the
    /// moment one has — including a hit the authority resolved well before the tick the coalesced
    /// episode finally closed on.
    ///
    /// `HullShockLedger` defers every hit inside [`SHOCK_EPISODE_TICKS`] of the last published one
    /// into a single later fact, so "the spark came first" is the NORMAL shape for a burst. The
    /// fixture is the burst: hit A closed its own episode at 100, hit B landed at 104 and could not
    /// publish until 116, and B's spark was drawn 12 ticks before B's own fact exists.
    #[test]
    fn a_shove_waits_for_one_of_its_own_hits() {
        let mut adoption = AuthorityAdoption::default();
        let mut presentation = ImpactPresentation::default();
        let deferred = episode_fact(hull(), 51, deferred_episode(104, 116));
        assert_eq!(
            adoption.offer(deferred, STAGED_AT, &mut presentation),
            Offer::Staged
        );

        assert!(
            !order(&mut adoption, &mut presentation, deferred, 0),
            "nothing has been drawn yet, so the shove would precede its own visual",
        );
        presentation.present(drew(104, VICTIM));
        assert!(
            order(&mut adoption, &mut presentation, deferred, 2),
            "the hit the authority resolved at 104 IS what this deferred episode is made of",
        );
        assert_eq!(
            presentation.tally(),
            OrderingTally {
                released_on_impact: 1,
                max_wait_ticks: 2,
                ..default()
            },
        );
    }

    /// THE COUNTER-EXAMPLE THE SLICE-3.5 REVIEW CAUGHT, executable.
    ///
    /// Hit A lands and publishes at 100. Hit B lands at 104 but the open episode defers it to 116,
    /// where it publishes as a SECOND fact. If B's own spark never arrives, the only armor impact
    /// this client has drawn is A's — and A's is the previous episode's, so it must not release B.
    /// A window measured in LOCAL ticks accepted it; the episode's own `opened` does not, because
    /// B opened at 104 and A's hit is older than that.
    #[test]
    fn an_earlier_episodes_hit_cannot_release_the_next_episode() {
        let mut adoption = AuthorityAdoption::default();
        let mut presentation = ImpactPresentation::default();
        let deferred = episode_fact(hull(), 51, deferred_episode(104, 116));
        assert_eq!(
            adoption.offer(deferred, STAGED_AT, &mut presentation),
            Offer::Staged
        );

        presentation.present(drew(100, VICTIM));
        presentation.present(drew(103, VICTIM));
        assert!(
            !order(&mut adoption, &mut presentation, deferred, 1),
            "tick 100 is the tick the PREVIOUS episode closed on and 103 is still inside its \
             deferral gap, so neither spark was published by THIS episode",
        );
        assert_eq!(
            presentation.tally(),
            OrderingTally::default(),
            "a fact still waiting has resolved nothing",
        );
    }

    /// FINDING THE SLICE-3.6 REVIEW CAUGHT, half one: THE FIRST EPISODE.
    ///
    /// A fresh `HullShockLedger` has no open episode to defer behind, so its first hit publishes on
    /// the tick it landed and the episode spans that ONE tick. The old rule derived the span as
    /// `(close − SHOCK_EPISODE_TICKS, close]` and therefore claimed fifteen ticks the episode never
    /// covered; any spark drawn in them released the shove. Now the episode carries its own `opened`
    /// and only its own tick matches.
    #[test]
    fn a_fresh_hulls_first_episode_claims_only_the_tick_it_landed_on() {
        let mut adoption = AuthorityAdoption::default();
        let mut presentation = ImpactPresentation::default();
        let episode = episode_fact(hull(), 50, first_episode(100));
        assert_eq!(
            adoption.offer(episode, STAGED_AT, &mut presentation),
            Offer::Staged
        );

        for stale in [85, 90, 99] {
            presentation.present(drew(stale, VICTIM));
        }
        assert!(
            !order(&mut adoption, &mut presentation, episode, 1),
            "a first episode published the tick it was armed covers no earlier tick at all — a \
             window derived from its close tick would have released it against any of these",
        );

        presentation.present(drew(100, VICTIM));
        assert!(
            order(&mut adoption, &mut presentation, episode, 2),
            "its own tick, and only its own tick, releases it",
        );
    }

    /// FINDING THE SLICE-3.6 REVIEW CAUGHT, half two: RESPAWN.
    ///
    /// `net::server` respawns by despawning the hull and spawning a new one, deliberately keeping the
    /// same `CombatantId`; the new entity gets a `HullShockLedger::default()`. `ImpactPresentation`
    /// is a per-CLIENT ledger keyed on victim and authority tick, and it is NOT cleared on respawn —
    /// so the previous life's sparks are still in it, under the same combatant. The fresh hull's
    /// first episode must not be released by any of them.
    ///
    /// It cannot be, by construction: the fresh ledger's first episode spans the single tick it was
    /// armed on, which is at or after the tick the new entity was spawned, which is at or after every
    /// impulse the previous incarnation could have taken.
    #[test]
    fn a_prior_lifes_spark_cannot_release_a_respawned_hulls_first_shove() {
        let mut adoption = AuthorityAdoption::default();
        let mut presentation = ImpactPresentation::default();

        // The life that ended: an episode on the old entity, and the sparks the client drew for it.
        // Its identity sequence and its `count` are ONE number — the old hull had been hit before,
        // so both are 4. The earlier version of this fixture closed a `sequence: 4` fact carrying a
        // `count: 1` episode, which no ledger can publish.
        let old_hull = hull();
        let died_at = 100;
        for tick in (died_at - 15)..died_at {
            presentation.present(drew(tick, VICTIM));
        }
        adoption.close(episode_fact(old_hull, 50, episode(4, died_at - 15)));

        // The life that began: a new `Entity`, the same combatant, a fresh ledger — so the first
        // episode it publishes is `count: 1` again, closed the tick its first hit landed.
        let reborn = Entity::from_raw_u32(8).expect("a second non-placeholder test entity");
        let first = episode_fact(reborn, 51, first_episode(died_at));
        assert_eq!(
            adoption.offer(first, STAGED_AT, &mut presentation),
            Offer::Staged
        );

        assert!(
            !order(&mut adoption, &mut presentation, first, 1),
            "every spark in the ledger belongs to the hull that died; none of them is a hit this \
             incarnation took, and a window derived from the close tick would have covered all 15",
        );
        assert_eq!(
            presentation.tally(),
            OrderingTally::default(),
            "a fact still waiting has resolved nothing",
        );

        presentation.present(drew(died_at, VICTIM));
        assert!(
            order(&mut adoption, &mut presentation, first, 2),
            "the new hull's own hit releases it",
        );
    }

    /// The other half of the identity. Two tanks hit inside one episode window is ordinary in a
    /// duel; a spark on the other one explains nothing about THIS hull lurching.
    ///
    /// A COALESCED episode (`sequence: 2`), because the fixture needs a span with room inside it:
    /// a first episode covers only its own tick, and then "the tick is inside the window" would be
    /// true of nothing and the victim test would pass for the wrong reason.
    #[test]
    fn a_hit_on_another_combatant_never_releases_this_hulls_shove() {
        let mut adoption = AuthorityAdoption::default();
        let mut presentation = ImpactPresentation::default();
        let episode = fact(hull(), 2, 50, 100);
        assert_eq!(
            adoption.offer(episode, STAGED_AT, &mut presentation),
            Offer::Staged
        );

        presentation.present(drew(99, BYSTANDER));
        assert!(
            !order(&mut adoption, &mut presentation, episode, 1),
            "the tick is inside the window, but the hull is somebody else's",
        );
        presentation.present(PresentedHit {
            tick: Tick(99),
            victim: None,
        });
        assert!(
            !order(&mut adoption, &mut presentation, episode, 2),
            "an impact the authority never attributed to a body cannot be claimed by one",
        );
        presentation.present(drew(99, VICTIM));
        assert!(order(&mut adoption, &mut presentation, episode, 3));
    }

    /// The wait is bounded. A visual that has not arrived within [`ORDERING_BUDGET_TICKS`] LOCAL
    /// ticks is not coming, so the shove lands anyway — and says so, because that count is half the
    /// instrument (the other half is [`OrderingTally::bypassed`]).
    #[test]
    fn the_budget_releases_a_shove_that_never_got_a_visual() {
        let mut adoption = AuthorityAdoption::default();
        let mut presentation = ImpactPresentation::default();
        let episode = fact(hull(), 1, 50, 100);
        assert_eq!(
            adoption.offer(episode, STAGED_AT, &mut presentation),
            Offer::Staged
        );

        assert!(!order(
            &mut adoption,
            &mut presentation,
            episode,
            ORDERING_BUDGET_TICKS - 1
        ));
        assert!(order(
            &mut adoption,
            &mut presentation,
            episode,
            ORDERING_BUDGET_TICKS
        ));
        assert_eq!(
            presentation.tally(),
            OrderingTally {
                released_on_budget: 1,
                max_wait_ticks: ORDERING_BUDGET_TICKS,
                ..default()
            },
            "a shove that landed with no visual behind it must be counted",
        );
    }

    /// THE CLOCK. The budget measures LOCAL patience, not the fact's age. A fact that arrives having
    /// already spent the whole budget in transit gets its full wait here — that high-latency case is
    /// the one the rule exists for, and measuring from `produced_at` would adopt it instantly.
    #[test]
    fn a_fact_that_arrives_old_still_gets_its_full_local_wait() {
        let mut adoption = AuthorityAdoption::default();
        let mut presentation = ImpactPresentation::default();
        // Produced at 100 and not staged until 140: a link with real latency.
        let episode = fact(hull(), 1, 50, 100);
        assert_eq!(
            adoption.offer(episode, STAGED_AT, &mut presentation),
            Offer::Staged
        );
        assert!(
            STAGED_AT - episode.produced_at > ORDERING_BUDGET_TICKS,
            "the fixture only says something if the fact's AGE alone would already release it",
        );

        let waited = STAGED_AT - adoption.staged_at.expect("the fact is staged");
        assert_eq!(
            waited, 0,
            "arrival is tick zero of the wait, whatever the fact's age"
        );
        assert!(
            !order(&mut adoption, &mut presentation, episode, waited),
            "a fact that spent the budget in TRANSIT has spent none of its local patience",
        );
        assert_eq!(presentation.tally(), OrderingTally::default());
    }

    /// The verdict latches per FACT. A retried request — the slot was busy, or the request was
    /// consumed without installing — must not tally the same episode twice.
    #[test]
    fn a_retried_request_tallies_its_episode_once() {
        let mut adoption = AuthorityAdoption::default();
        let mut presentation = ImpactPresentation::default();
        // Coalesced, so tick 96 is one of the episode's own hits rather than a stale spark.
        let episode = fact(hull(), 2, 50, 100);
        assert_eq!(
            adoption.offer(episode, STAGED_AT, &mut presentation),
            Offer::Staged
        );

        presentation.present(drew(96, VICTIM));
        for _ in 0..4 {
            assert!(order(&mut adoption, &mut presentation, episode, 3));
        }
        assert_eq!(
            presentation.tally(),
            OrderingTally {
                released_on_impact: 1,
                max_wait_ticks: 3,
                ..default()
            },
        );
    }

    /// Only a correct physical EVENT has a visual to be ordered against. The client's own
    /// misprediction is an error the view layer hides, and holding it would delay a correction for a
    /// spark that will never be drawn.
    #[test]
    fn a_misprediction_correction_is_never_held_for_a_visual() {
        let mut adoption = AuthorityAdoption::default();
        let mut presentation = ImpactPresentation::default();
        let mut drift = fact(hull(), 1, 50, 100);
        drift.cause = AdoptionCause::Misprediction;
        drift.visual = None;

        assert!(order(&mut adoption, &mut presentation, drift, 0));
        assert_eq!(presentation.tally(), OrderingTally::default());
    }

    /// FINDING THE SLICE-3 REVIEW CAUGHT. A rollback that merely STARTS at the fact's producing tick
    /// is not this module's rollback: lightyear rolls back for a `Position` mismatch or
    /// `net::watchdog`'s claim, and crediting one of those as an ADOPTION would report an ordering
    /// this module never performed. Only its own installed claim counts as one.
    #[test]
    fn only_this_modules_own_installed_claim_counts_as_an_adoption() {
        let mut adoption = AuthorityAdoption::default();
        // Nothing drawn: the fact's own spark is genuinely pending, which is the state
        // `Retirement::Delivered` reports as a bypass.
        let mut presentation = ImpactPresentation::default();
        let episode = fact(hull(), 1, 50, 100);
        assert_eq!(
            adoption.offer(episode, STAGED_AT, &mut presentation),
            Offer::Staged
        );

        assert_eq!(
            retirement(
                &adoption,
                &presentation,
                None,
                Tick(100),
                RestoresFrom::Authority,
                CARRIED
            ),
            Some(Retirement::Delivered {
                spark_pending: true
            }),
            "a rollback nobody here claimed is a BYPASS, never an adoption",
        );
        adoption.requested = true;
        assert_eq!(
            retirement(
                &adoption,
                &presentation,
                Some((Tick(100), AdoptionCause::Misprediction)),
                Tick(100),
                RestoresFrom::Authority,
                CARRIED,
            ),
            Some(Retirement::Delivered {
                spark_pending: true
            }),
            "the tick agreeing is not enough — a Misprediction claim is somebody else's",
        );
        assert_eq!(
            retirement(
                &adoption,
                &presentation,
                Some((Tick(100), AdoptionCause::ExternalEvent)),
                Tick(100),
                RestoresFrom::Authority,
                CARRIED,
            ),
            Some(Retirement::Adopted),
        );
    }

    /// FINDING THE SLICE-3.7 REVIEW CAUGHT, and the combination no earlier fixture covered:
    /// REQUESTED, our own claim INSTALLED, and the restore carried NOTHING.
    ///
    /// The old rule answered `Adopted` from the claim identity alone, without consulting `carried`,
    /// and `Adopted` closes the fact forever. So a rollback this module ordered onto a confirmed
    /// history that resolved to a pre-event velocity retired the fact having delivered nothing, and
    /// the shove was lost in silence — on the SUCCESS path, which is why three rounds of review over
    /// the neighbouring branches never reached it.
    ///
    /// It must not answer `Keep` either: a retry re-reads the same buffers at the same target tick
    /// and cannot improve, so `Keep` here is an unbounded re-request loop. The answer is a THIRD
    /// outcome — spent, loud, and counted.
    #[test]
    fn an_installed_claim_that_restored_nothing_is_not_an_adoption() {
        let mut adoption = AuthorityAdoption::default();
        // Nothing drawn: the fact's own spark is genuinely pending, which is the state
        // `Retirement::Delivered` reports as a bypass.
        let mut presentation = ImpactPresentation::default();
        let episode = fact(hull(), 1, 50, 100);
        assert_eq!(
            adoption.offer(episode, STAGED_AT, &mut presentation),
            Offer::Staged
        );
        adoption.requested = true;
        let ours = Some((Tick(100), AdoptionCause::ExternalEvent));

        assert_eq!(
            retirement(
                &adoption,
                &presentation,
                ours,
                Tick(100),
                RestoresFrom::Authority,
                false
            ),
            Some(Retirement::Undelivered),
            "our own rollback was installed and `prepare_rollback` restored a PRE-hit velocity — \
             recording that as an adoption is recording a delivery that did not happen",
        );
        assert_eq!(
            retirement(
                &adoption,
                &presentation,
                ours,
                Tick(100),
                RestoresFrom::Authority,
                CARRIED
            ),
            Some(Retirement::Adopted),
            "the same claim over a restore that DID carry the shove is the ordinary success",
        );
        assert!(
            !Retirement::Undelivered.keeps_the_seam_sharp(),
            "an installed ExternalEvent claim that carried NOTHING has no hit in its correction — \
             a presentation signal read off the cause tag would go sharp here for nothing, which \
             is the direction of the error that made the tag unusable as the signal",
        );
    }

    /// WHICH RETIREMENTS PRODUCE A SHARP SEAM, stated over every outcome rather than over the two
    /// this module happens to reach today.
    ///
    /// The rule is DELIVERY, not intent: sharp exactly when the authority's post-hit state is on the
    /// live hull because of this rollback. Both `Delivered` variants qualify — `spark_pending` is a
    /// telemetry question about ORDERING and says nothing about whether the shove landed — and both
    /// non-delivering outcomes do not.
    ///
    /// [`Retirement::Undelivered`] is the one this fixture can reach and no schedule-level fixture
    /// can: the branch is unreachable against the pinned lightyear (see [`retirement`] for the
    /// three-step proof) and is kept as a tripwire, so its presentation verdict has to be pinned
    /// here or nowhere.
    #[test]
    fn only_the_two_retirements_that_delivered_the_fact_keep_the_seam_sharp() {
        for (outcome, sharp) in [
            (Retirement::Keep, false),
            (Retirement::Adopted, true),
            (Retirement::Undelivered, false),
            (
                Retirement::Delivered {
                    spark_pending: false,
                },
                true,
            ),
            (
                Retirement::Delivered {
                    spark_pending: true,
                },
                true,
            ),
        ] {
            assert_eq!(
                outcome.keeps_the_seam_sharp(),
                sharp,
                "{outcome:?} must {} keep the seam sharp",
                if sharp { "" } else { "NOT" },
            );
        }
    }

    /// The other half of the same rule. A rollback SHALLOWER than the producing tick restores
    /// pre-hit state and replays forward from it, so it carries none of the shove: the fact must
    /// stay staged and be requested again rather than be silently spent.
    #[test]
    fn a_shallower_rollback_retires_nothing() {
        let mut adoption = AuthorityAdoption::default();
        // Nothing drawn: the fact's own spark is genuinely pending, which is the state
        // `Retirement::Delivered` reports as a bypass.
        let mut presentation = ImpactPresentation::default();
        let episode = fact(hull(), 1, 50, 100);
        assert_eq!(
            adoption.offer(episode, STAGED_AT, &mut presentation),
            Offer::Staged
        );

        assert_eq!(
            retirement(
                &adoption,
                &presentation,
                None,
                Tick(99),
                RestoresFrom::Authority,
                CARRIED
            ),
            Some(Retirement::Keep)
        );
        assert_eq!(
            retirement(
                &adoption,
                &presentation,
                None,
                Tick(101),
                RestoresFrom::Authority,
                CARRIED
            ),
            Some(Retirement::Delivered {
                spark_pending: true
            }),
            "a DEEPER rollback restores the fact's tick along the way, so the shove is already live",
        );
    }

    /// FINDING THE SLICE-3.5 REVIEW CAUGHT. An INPUT rollback restores from the client's own
    /// `PredictionHistory` (`prepare_rollback`'s non-`FromState` branch), which for a hull the
    /// client never knew was hit is its un-hit prediction. Retiring against one at ANY depth would
    /// close the fact having delivered nothing, and the shove would be lost outright.
    #[test]
    fn an_input_rollback_delivers_nothing_and_retires_nothing() {
        let mut adoption = AuthorityAdoption::default();
        // Nothing drawn: the fact's own spark is genuinely pending, which is the state
        // `Retirement::Delivered` reports as a bypass.
        let mut presentation = ImpactPresentation::default();
        let episode = fact(hull(), 1, 50, 100);
        assert_eq!(
            adoption.offer(episode, STAGED_AT, &mut presentation),
            Offer::Staged
        );
        adoption.requested = true;

        for started in [Tick(100), Tick(101), Tick(140)] {
            assert_eq!(
                retirement(
                    &adoption,
                    &presentation,
                    Some((Tick(100), AdoptionCause::ExternalEvent)),
                    started,
                    RestoresFrom::OwnPrediction,
                    CARRIED,
                ),
                Some(Retirement::Keep),
                "an input rollback at tick {} restored the client's own no-hit prediction — the \
                 fact must stay staged, whatever the slot says",
                started.0,
            );
        }
    }

    /// A reconnect is a NEW TIMELINE, and a fact held for its spark when it happens is abandoned
    /// on purpose — but the per-fact rows promise every `staged` a terminal, so the abandonment
    /// must say so. The acceptance analyzer otherwise reads an intentional reset as a lost fact.
    #[test]
    fn a_reconnect_abandons_the_staged_fact_with_a_terminal_row() {
        crate::trace::arm_fact_rows_for_test();
        let mut world = World::new();
        world.init_resource::<ForcedRollbackSlot>();
        let hull = world.spawn_empty().id();
        let mut adoption = AuthorityAdoption::default();
        let mut presentation = ImpactPresentation::default();
        let fact = AuthoritativeFact {
            id: FactId {
                source: FactSource::HullShock,
                entity: hull,
                sequence: 7,
                checkpoint: RepliconTick::new(9),
            },
            cause: AdoptionCause::ExternalEvent,
            produced_at: Tick(40),
            settled_at: Tick(40),
            visual: None,
        };
        assert_eq!(
            adoption.offer(fact, Tick(41), &mut presentation),
            Offer::Staged,
        );
        world.insert_resource(adoption);
        world.insert_resource(presentation);
        // Discard the staged row (and whatever parallel armed tests pushed) so the assertion
        // below reads only what the reconnect emits.
        crate::trace::drain_fact_events_for_test();

        world.add_observer(reset_adoption_state);
        // `Connected`'s on_add hook demands the link shape a real connection has.
        world.spawn((
            lightyear::prelude::client::Client::default(),
            RemoteId(PeerId::Server),
            Connected,
        ));
        world.flush();

        let rows = crate::trace::drain_fact_events_for_test();
        assert!(
            rows.iter()
                .any(|row| row["ev"] == "dropped" && row["why"] == "reconnect" && row["seq"] == 7),
            "the abandoned fact must get a terminal `dropped` row naming the reconnect, got {rows:?}",
        );
        assert!(
            !world.resource::<AuthorityAdoption>().is_staged(),
            "the reset itself must still clear the slot",
        );
    }

    /// THE PRODUCTION INVARIANT, held in place rather than relied on. `net::client` disables input
    /// rollback outright, which is why the case above has never shipped. [`retirement`] handles it
    /// regardless; this fails the moment someone switches the policy back on without reading why.
    #[test]
    fn the_shipping_client_disables_input_rollback() {
        assert!(
            matches!(
                super::super::client::shipping_rollback_policy().input,
                RollbackMode::Disabled
            ),
            "input rollback is on again. `net::adoption::retirement` already refuses to retire a \
             staged fact against one, so nothing is lost — but `net::hull_shock_rollback` and the \
             lead-0 fixtures were all written against a client that never takes that branch, and \
             they now cover less than they claim.",
        );
    }

    /// FINDING THE SLICE-3.11 REVIEW CAUGHT, at the readiness predicate: the hull's PARTICIPATION in
    /// `prepare_rollback` was asked once, as a query filter on the offer, and never again.
    ///
    /// The four confirmed histories here are the healthiest possible: the pose is restorable and both
    /// velocities resolve the authority's post-event samples. That is the whole point — a confirmed
    /// history answers "what WOULD a restore at this tick resolve", and its answer is IDENTICAL for a
    /// hull lightyear's query filters out. Only the archetype tells the two apart, so only the
    /// archetype can be the thing that is asked.
    ///
    /// BOTH exclusions, because `prepare_rollback`'s query has exactly two and a fix aimed at one of
    /// them lands on one of them: `Without<DisableRollback>` on the entity, and a REQUIRED
    /// `&mut PredictionHistory<C>` per component.
    ///
    /// And it is refused BEFORE the histories are consulted, which is why the two reasons are
    /// distinct `Unready` variants rather than a shared one: a hull the restore skips is not waiting
    /// for a better confirmed sample, and a log that said so would send the next reader to the wrong
    /// buffer.
    #[test]
    fn a_hull_outside_prepares_query_is_refused_whatever_its_histories_say() {
        const SETTLED_AT: Tick = Tick(100);
        const PRODUCED_AT: Tick = Tick(104);
        let deliverable = |rollback_disabled: bool, prediction_histories: [bool; 4]| {
            restore_is_deliverable(
                prepare_restores(rollback_disabled, prediction_histories),
                Some(&confirmed_position(SETTLED_AT)),
                Some(&confirmed_rotation(SETTLED_AT)),
                Some(&confirmed_linear(SETTLED_AT)),
                Some(&confirmed_angular(SETTLED_AT)),
                PRODUCED_AT,
                SETTLED_AT,
            )
        };

        assert_eq!(
            deliverable(false, [true; 4]),
            Ok(()),
            "the control: a hull in the query, over histories that carry the shove, is deliverable — \
             or the refusals below prove nothing about the archetype",
        );
        assert_eq!(
            deliverable(true, [true; 4]),
            Err(Unready::Excluded),
            "`DisableRollback` takes the hull out of EVERY `prepare_rollback::<C>` at once, so the \
             rollback this fact would order restores nothing at all — and the module used to claim \
             the slot for it and then read a delivery out of the untouched confirmed history",
        );
        for absent in 0..4 {
            let mut prediction_histories = [true; 4];
            prediction_histories[absent] = false;
            assert_eq!(
                deliverable(false, prediction_histories),
                Err(Unready::Unhistoried),
                "`PredictionHistory` is REQUIRED in that query, not optional, so a hull missing the \
                 one for component {absent} is skipped for it — and a rigid body restored in part \
                 is not a tick either peer ever had, which is the same rule the pose predicate \
                 exists for",
            );
        }
    }

    /// FINDING THE SLICE-3.11 REVIEW CAUGHT, at the metric. `spark_pending` decides
    /// [`OrderingTally::bypassed`], which is the ONE number that would justify turning this
    /// best-effort rule into a real presentation barrier — so a count inflated by facts the rule
    /// never held would corrupt exactly the decision it exists to inform.
    ///
    /// It used to be read off `AuthorityAdoption::ordering.is_none()`, and that `None` carried three
    /// different situations. This walks all three over ONE staged fact so the distinction cannot be
    /// read as a coincidence of fixtures, and it asks the question the counter's own wording asks:
    /// had this fact's own spark been drawn?
    #[test]
    fn only_a_fact_whose_own_spark_is_still_undrawn_counts_as_bypassed() {
        let mut adoption = AuthorityAdoption::default();
        let mut presentation = ImpactPresentation::default();
        let episode = fact(hull(), 1, 50, 100);
        assert_eq!(
            adoption.offer(episode, STAGED_AT, &mut presentation),
            Offer::Staged
        );
        let bypassed = |adoption: &AuthorityAdoption, presentation: &ImpactPresentation| {
            retirement(
                adoption,
                presentation,
                None,
                Tick(100),
                RestoresFrom::Authority,
                CARRIED,
            ) == Some(Retirement::Delivered {
                spark_pending: true,
            })
        };

        assert_eq!(
            adoption.ordering,
            Ordering::Unasked,
            "state 1: no frame has reached the rule yet — every early return in \
             `request_staged_adoption` ahead of `clear_to_order` leaves it here",
        );
        assert!(
            bypassed(&adoption, &presentation),
            "and with nothing drawn for this episode that IS a bypass: the shove landed through \
             somebody else's rollback before the player was shown anything",
        );

        assert!(!order(&mut adoption, &mut presentation, episode, 0));
        assert_eq!(
            adoption.ordering,
            Ordering::HoldingForSpark,
            "state 2: the rule has the fact and is holding it inside the budget",
        );
        assert!(bypassed(&adoption, &presentation), "still a bypass");

        // The spark arrives, through the same ledger the rule reads. Nothing re-runs the rule — this
        // is the frame ordering the schedule actually produces, since `confirm_forced_rollback` runs
        // after `RollbackSystems::Prepare` and the request ran before `RollbackSystems::Check`.
        presentation.present(drew(100, VICTIM));
        assert!(
            !bypassed(&adoption, &presentation),
            "the fact's own hit HAS been drawn now, so a rollback carrying the shove did not \
             outrun it — reading the LATCH instead of the ledger would still call this a bypass, \
             and the latch is the thing that has not been updated",
        );

        assert!(order(&mut adoption, &mut presentation, episode, 0));
        assert_eq!(adoption.ordering, Ordering::Released);
        assert!(
            !bypassed(&adoption, &presentation),
            "state 3: released. A rollback carrying a fact the rule already let go missed nothing.",
        );

        let mut unheld = AuthorityAdoption::default();
        let mut drift = fact(hull(), 1, 50, 100);
        drift.cause = AdoptionCause::Misprediction;
        drift.visual = None;
        assert_eq!(
            unheld.offer(drift, STAGED_AT, &mut presentation),
            Offer::Staged
        );
        assert!(order(&mut unheld, &mut presentation, drift, 0));
        assert_eq!(
            unheld.ordering,
            Ordering::Released,
            "a fact with no visual is RELEASED, not pending: there is nothing for it to wait on, \
             and the rule records that rather than leaving an absence for `retirement` to guess at",
        );
        assert!(
            !bypassed(&unheld, &presentation),
            "so a rollback that carries a misprediction correction can never be a BYPASS — there \
             was no spark for it to beat. This is the state the old `is_none()` reading counted.",
        );
    }

    /// FINDING THE EIGHTH REVIEW CAUGHT, and it is the exact eviction the previous round's own
    /// safety argument said could not happen.
    ///
    /// [`MAX_PRESENTED_HITS`] was derived as a span in TICKS — an episode's width plus the ordering
    /// budget — on the reasoning that nothing older than that span can still be asked about. But the
    /// capacity is spent per ENTRY, and nothing bounds entries per tick: every authority armor impact
    /// is appended and impacts are broadcast to every client, so unrelated hits on other combatants
    /// consume the same ledger. Draw the staged fact's own spark, let one busy moment of somebody
    /// else's firefight push it out, and [`retirement`] read "never drawn" and reported a BYPASS —
    /// inflating the ONE number that decides whether this rule has to become a real presentation
    /// barrier, on a frame where the player had already seen the hit.
    ///
    /// THE FIXTURE IS DELIBERATELY OVER-CAPACITY BY ONE and asserts the eviction happened, because a
    /// regression test for a FIFO that quietly grew a larger capacity would pass while proving
    /// nothing. What holds the answer now is the per-fact latch, not the depth of this buffer, and
    /// both consumers read it.
    #[test]
    fn an_evicted_spark_is_still_the_staged_facts_own_spark() {
        let mut adoption = AuthorityAdoption::default();
        let mut presentation = ImpactPresentation::default();
        let episode = fact(hull(), 1, 50, 100);

        // The spark comes FIRST, which is the ordinary shape: a hit's `ImpactConfirm` is a message
        // drained in `Update` and its `HullShock` is replicated state, so the two arrive apart.
        presentation.present(drew(100, VICTIM));
        assert_eq!(
            adoption.offer(episode, STAGED_AT, &mut presentation),
            Offer::Staged
        );

        // Somebody else's firefight, on a combatant this fact knows nothing about.
        for tick in 0..=MAX_PRESENTED_HITS as u32 {
            presentation.present(drew(200 + tick, BYSTANDER));
        }
        let claim = episode
            .visual
            .expect("an external-event fact carries its claim");
        assert!(
            !presentation.presented.iter().any(|hit| claim.covers(*hit)),
            "the fixture has to actually overflow the ledger past this fact's own hit, or it is \
             asserting the fix on a ledger that never lost anything",
        );

        assert!(
            presentation.shown_for(claim),
            "the spark WAS drawn, and the staged fact's answer may not be a search over a buffer \
             other combatants can evict it from",
        );
        assert_eq!(
            retirement(
                &adoption,
                &presentation,
                None,
                Tick(100),
                RestoresFrom::Authority,
                CARRIED
            ),
            Some(Retirement::Delivered {
                spark_pending: false
            }),
            "so an unrelated rollback that carries this shove BEAT NOTHING — the player had \
             already been shown the hit, and counting it as a bypass would corrupt the metric that \
             decides whether the ordering rule needs to become a barrier",
        );

        // And the rule's own consumer agrees: released ON IMPACT, not on the budget.
        assert!(order(&mut adoption, &mut presentation, episode, 0));
        assert_eq!(
            presentation.tally(),
            OrderingTally {
                released_on_impact: 1,
                ..default()
            },
            "an evicted spark released on the BUDGET would log a shove as unordered that the \
             player watched land — conservative for the shove, and a lie in the diagnostics",
        );
    }

    /// A bypass is only a bypass while the ordering rule is still holding the fact. Once the rule
    /// has released it, an unrelated rollback that carries the shove missed nothing.
    #[test]
    fn a_released_fact_carried_by_someone_elses_rollback_is_not_a_bypass() {
        let mut adoption = AuthorityAdoption::default();
        let mut presentation = ImpactPresentation::default();
        let episode = fact(hull(), 1, 50, 100);
        assert_eq!(
            adoption.offer(episode, STAGED_AT, &mut presentation),
            Offer::Staged
        );

        presentation.present(drew(100, VICTIM));
        assert!(order(&mut adoption, &mut presentation, episode, 0));
        assert_eq!(
            retirement(
                &adoption,
                &presentation,
                None,
                Tick(100),
                RestoresFrom::Authority,
                CARRIED
            ),
            Some(Retirement::Delivered {
                spark_pending: false
            }),
        );
    }

    /// A hull-velocity history holding one authority sample at `sampled_at`.
    fn confirmed_linear(sampled_at: Tick) -> ConfirmedHistory<LinearVelocity> {
        let mut history = ConfirmedHistory::<LinearVelocity>::default();
        history.insert_present_explicit(sampled_at, LinearVelocity(AUTHORITY_LINEAR));
        history
    }

    fn confirmed_angular(sampled_at: Tick) -> ConfirmedHistory<AngularVelocity> {
        let mut history = ConfirmedHistory::<AngularVelocity>::default();
        history.insert_present_explicit(sampled_at, AngularVelocity(AUTHORITY_ANGULAR));
        history
    }

    /// FINDING THE SLICE-3.6 REVIEW CAUGHT, corrected by 3.7. `bypassed` claims the shove LANDED, so
    /// it has to be decided from the histories `prepare_rollback` actually restored from — not from
    /// the rollback's start tick, which says only how deep it went.
    ///
    /// AND NOT FROM THE PRODUCING TICK EITHER, which is what 3.7 caught. `prepare_rollback` restores
    /// `get_state_at_or_before(rollback_tick)`: the EFFECTIVE authoritative state at or before its
    /// target, with no condition on how old the underlying sample is. Requiring the sample to be at
    /// or after [`AuthoritativeFact::produced_at`] therefore rejected the ordinary production shape —
    /// an episode that settled at 100 whose `HullShock` only materialized in the checkpoint at 104 —
    /// and undercounted a real delivery. The comparison is against
    /// [`AuthoritativeFact::settled_at`]: the tick the authority's own state stopped moving on
    /// account of this event.
    #[test]
    fn a_restore_carries_the_shove_when_it_resolves_to_a_settled_sample() {
        /// The deferred episode Codex's counter-example is built on: opened at 88, closed at 100.
        const SETTLED_AT: Tick = Tick(100);
        /// Where replication materialized it — LATER than the close, and later than the velocity
        /// samples the same restore resolves.
        const PRODUCED_AT: Tick = Tick(104);

        assert!(
            restore_carries_the_shove(
                Some(&confirmed_linear(SETTLED_AT)),
                Some(&confirmed_angular(SETTLED_AT)),
                PRODUCED_AT,
                SETTLED_AT,
            ),
            "a rollback to 104 resolves the authority's tick-100 velocities as the effective state \
             there, and tick 100 is when this episode settled — that IS a delivery, and the rule \
             that compared against the tick-104 confirmed sample called it nothing",
        );
        assert!(
            restore_carries_the_shove(
                Some(&confirmed_linear(SETTLED_AT)),
                Some(&confirmed_angular(SETTLED_AT)),
                SETTLED_AT,
                SETTLED_AT,
            ),
            "the boundary case: a restore exactly at the settling tick carries the whole episode",
        );
        assert!(
            !restore_carries_the_shove(
                Some(&confirmed_linear(Tick(99))),
                Some(&confirmed_angular(SETTLED_AT)),
                PRODUCED_AT,
                SETTLED_AT,
            ),
            "one tick short of the close is one impulse short of the episode: the linear velocity \
             restored there is a value the authority held BEFORE the episode finished",
        );
        assert!(
            !restore_carries_the_shove(
                Some(&confirmed_linear(SETTLED_AT)),
                Some(&confirmed_angular(Tick(96))),
                PRODUCED_AT,
                SETTLED_AT,
            ),
            "half a restored rigid body is not the shove, whichever half is stale",
        );
        assert!(
            !restore_carries_the_shove(
                None,
                Some(&confirmed_angular(SETTLED_AT)),
                PRODUCED_AT,
                SETTLED_AT,
            ),
            "with no confirmed history `prepare_rollback` takes the other branch and restores the \
             client's own prediction — the un-hit one — so nothing was delivered",
        );
        assert!(
            !restore_carries_the_shove(
                Some(&confirmed_linear(SETTLED_AT)),
                Some(&confirmed_angular(SETTLED_AT)),
                Tick(99),
                SETTLED_AT,
            ),
            "a restore SHALLOWER than the settling tick resolves nothing at or after it, so depth \
             is still a necessary condition — it just was never a sufficient one",
        );
    }

    /// THE MUTATION THE TICK COMPARISON ALONE CANNOT SEE, and the reason the predicate makes
    /// `prepare_rollback`'s own lookup its FIRST question.
    ///
    /// `ConfirmedHistory` stores an authoritative REMOVAL as an ordinary entry and middle-inserts it
    /// in tick order, so a removal that arrives late can land BETWEEN the event and the restore
    /// target. `get_state_at_or_before` then resolves `Removed` and `prepare_rollback` answers by
    /// taking `LinearVelocity` OFF the hull — the shove is not merely stale, the component is gone.
    /// The present-value iterator skips removals and would report the tick-100 sample the removal
    /// shadows, i.e. would call this a delivery.
    #[test]
    fn a_removal_between_the_event_and_the_target_carries_nothing() {
        const SETTLED_AT: Tick = Tick(100);
        const PRODUCED_AT: Tick = Tick(104);

        let mut linear = confirmed_linear(SETTLED_AT);
        assert!(
            restore_carries_the_shove(
                Some(&linear),
                Some(&confirmed_angular(SETTLED_AT)),
                PRODUCED_AT,
                SETTLED_AT,
            ),
            "the same history one insertion earlier IS a delivery — so what follows is the \
             insertion's doing and not a fixture that never passed",
        );

        // The REAL insertion path, with its sorted middle insertion and its unchanged-state
        // compression, rather than a hand-built buffer: a later sample equal to the effective one is
        // stored as an unchanged marker, and the removal then MIDDLE-inserts in front of it.
        //
        // The marker goes at 103 — BEFORE the restore target, not past it. Past it the target's
        // lookup never reaches the marker and only the removal does any work, which is a coverage
        // claim this fixture used to make and not keep. At 103 `get_state_at_or_before(104)` lands
        // ON the marker and resolves back through it to the removal.
        linear.insert_present(Tick(103), LinearVelocity(AUTHORITY_LINEAR));
        linear.insert_removed(Tick(102));

        assert!(
            !restore_carries_the_shove(
                Some(&linear),
                Some(&confirmed_angular(SETTLED_AT)),
                PRODUCED_AT,
                SETTLED_AT,
            ),
            "a restore at 104 now resolves an authoritative REMOVAL, so it delivers no velocity at \
             all. `newest_present_at_or_before` alone still answers tick 100 here, which is exactly \
             the disagreement with `prepare_rollback` that the state lookup closes.",
        );
        assert!(
            !authority_reaches(Some(&linear), PRODUCED_AT),
            "and the completeness predicate has to fail the same way, for the same reason: a \
             restore that deletes half the rigid body is not authority reaching the hull",
        );
    }

    /// THE READINESS DIRECTION OF THE SAME PREDICATE, which is what keeps the loss in
    /// [`an_installed_claim_that_restored_nothing_is_not_an_adoption`] off the shipping path. It is
    /// the predicate that does that, at the frame the request is made — see [`retirement`]; asking
    /// it only at the offer, as an earlier version did, decides a later frame's transaction on an
    /// earlier frame's histories.
    ///
    /// [`authority_reaches`] answers "will lightyear restore something from authority here?" and
    /// nothing more. Over a velocity history whose newest sample at or before the producing tick
    /// predates the episode, the answer is YES and the restored value is a PRE-hit velocity. So the
    /// gate the offer runs has to be the delivery predicate at the tick it would target, not the
    /// existence predicate.
    #[test]
    fn readiness_and_delivery_disagree_exactly_where_the_shove_would_be_lost() {
        const SETTLED_AT: Tick = Tick(100);
        const PRODUCED_AT: Tick = Tick(104);
        let stale_linear = confirmed_linear(Tick(96));
        let stale_angular = confirmed_angular(Tick(96));

        assert!(
            authority_reaches(Some(&stale_linear), PRODUCED_AT)
                && authority_reaches(Some(&stale_angular), PRODUCED_AT),
            "lightyear WILL restore these from authority: `get_state_at_or_before(104)` resolves \
             the tick-96 sample. Existence is all this predicate ever claimed.",
        );
        assert!(
            !restore_carries_the_shove(
                Some(&stale_linear),
                Some(&stale_angular),
                PRODUCED_AT,
                SETTLED_AT,
            ),
            "and what it restores is the hull as it was four ticks BEFORE the episode closed. \
             Offering this fact would buy a forced rollback that installs a pre-hit velocity and \
             then retire the shove against it.",
        );
    }

    /// A pose history holding one authority sample at `sampled_at`.
    fn confirmed_position(sampled_at: Tick) -> ConfirmedHistory<Position> {
        let mut history = ConfirmedHistory::<Position>::default();
        history.insert_present_explicit(sampled_at, Position::default());
        history
    }

    fn confirmed_rotation(sampled_at: Tick) -> ConfirmedHistory<Rotation> {
        let mut history = ConfirmedHistory::<Rotation>::default();
        history.insert_present_explicit(sampled_at, Rotation::default());
        history
    }

    /// FINDING THE SLICE-3.10 REVIEW CAUGHT, at the predicate. Two components carry the fact and two
    /// only have to survive the restore, and [`restore_is_deliverable`] must apply the matching
    /// question to each — a single predicate across all four is wrong in one direction or the other.
    ///
    /// The integration half is
    /// `net::lead_zero_rollback::a_late_pose_removal_is_revalidated_before_the_request`, which runs
    /// the real restore. This one pins the ASYMMETRY, which that fixture cannot see: it would pass
    /// just as well against a module that had tightened the pose to the velocity predicate.
    ///
    /// WHAT THIS PINS IS THE POLICY, AND ONLY THE POLICY. A previous version of this comment called
    /// the fixture's 20-tick-old pose "the ordinary shape for a hull that was not moving, since
    /// replication only transmits components that CHANGED", and the seventh review found that
    /// premise false: `net::physics` disables Avian island sleeping for network physics, Avian's
    /// solver writeback takes `&mut Position` and `&mut Rotation` for every solver body every step,
    /// and both the hit and the episode's close happen in `FixedUpdate` ahead of that step — so even
    /// a stationary hull has a changed pose before the checkpoint carrying its `HullShock`. No
    /// shipping shape is claimed here. What is asserted is the RULE the module applies: the pose is
    /// asked whether a restore leaves a rigid body standing, never whether it is recent, because a
    /// recency clause on a component the event never touched can only turn restorable facts into
    /// waits — and a wait costs a shove.
    #[test]
    fn the_pose_is_asked_a_weaker_question_than_the_shove_and_that_is_deliberate() {
        const SETTLED_AT: Tick = Tick(100);
        const PRODUCED_AT: Tick = Tick(104);
        /// A pose sample far enough behind the episode that the two predicates give visibly
        /// different verdicts on it. Chosen to separate them, not claimed to be what the wire
        /// produces; see the fixture doc.
        const POSE_SAMPLED_AT: Tick = Tick(80);

        let stale_pose_position = confirmed_position(POSE_SAMPLED_AT);
        let stale_pose_rotation = confirmed_rotation(POSE_SAMPLED_AT);
        assert_eq!(
            restore_is_deliverable(
                prepare_restores(false, [true; 4]),
                Some(&stale_pose_position),
                Some(&stale_pose_rotation),
                Some(&confirmed_linear(SETTLED_AT)),
                Some(&confirmed_angular(SETTLED_AT)),
                PRODUCED_AT,
                SETTLED_AT,
            ),
            Ok(()),
            "a pose sample 20 ticks older than the episode is still the authority's genuine pose at \
             the restore target: the impulse changes VELOCITY, and the pose only moves as later \
             ticks integrate it. Requiring pose recency here would refuse a restore that is sound, \
             and every refusal is a fact held in the single staging slot until the replay window \
             drops it — a shove lost to a clause that protects nothing.",
        );
        assert!(
            !restore_carries_the_shove(
                Some(&confirmed_linear(POSE_SAMPLED_AT)),
                Some(&confirmed_angular(POSE_SAMPLED_AT)),
                PRODUCED_AT,
                SETTLED_AT,
            ),
            "and the SAME sample tick on a velocity history is a refusal — which is the whole \
             asymmetry, stated over one number so it cannot be read as a coincidence of fixtures",
        );

        // A removal middle-inserted between the episode and the restore target, the same shape the
        // velocity fixtures use, through lightyear's own insertion API.
        let mut removed_position = confirmed_position(SETTLED_AT);
        removed_position.insert_present(Tick(103), Position::default());
        removed_position.insert_removed(Tick(102));
        assert_eq!(
            restore_is_deliverable(
                prepare_restores(false, [true; 4]),
                Some(&removed_position),
                Some(&confirmed_rotation(SETTLED_AT)),
                Some(&confirmed_linear(SETTLED_AT)),
                Some(&confirmed_angular(SETTLED_AT)),
                PRODUCED_AT,
                SETTLED_AT,
            ),
            Err(Unready::Pose),
            "but a restore that would DELETE the position is refused even though both velocities \
             carry the shove perfectly — that combination is exactly what the request used to wave \
             through, and `Retirement::Adopted` is what it then recorded",
        );
        assert_eq!(
            restore_is_deliverable(
                prepare_restores(false, [true; 4]),
                None,
                Some(&confirmed_rotation(SETTLED_AT)),
                Some(&confirmed_linear(SETTLED_AT)),
                Some(&confirmed_angular(SETTLED_AT)),
                PRODUCED_AT,
                SETTLED_AT,
            ),
            Err(Unready::Pose),
            "and so is a missing pose history: `prepare_rollback` takes the other branch there and \
             restores the client's own prediction under the authority's tick label",
        );
        assert_eq!(
            restore_is_deliverable(
                prepare_restores(false, [true; 4]),
                Some(&confirmed_position(SETTLED_AT)),
                Some(&confirmed_rotation(SETTLED_AT)),
                Some(&confirmed_linear(Tick(96))),
                Some(&confirmed_angular(SETTLED_AT)),
                PRODUCED_AT,
                SETTLED_AT,
            ),
            Err(Unready::Shove),
            "with the pose sound, a pre-hit velocity is still a refusal, and it is reported as the \
             OTHER reason — the two waits have different causes and the log has to say which",
        );
    }

    /// THE SAME RULE THROUGH THE SYSTEM THAT DECIDES IT, not around it. `retirement` is a pure
    /// function and the fixtures above hand it a verdict; this one runs `confirm_forced_rollback` on
    /// a real `PredictionManager` mid-rollback, with real confirmed histories on the staged fact's
    /// own entity, and reads the tally the diagnostics line reports.
    ///
    /// WHAT IT DOES NOT PROVE, stated because the previous version of this comment implied it did:
    /// `prepare_rollback` is not run here, so this pins the SEAM — that the counter follows the
    /// predicate over the fact's own entity — and not that the hull was restored. The restore itself
    /// is executed for real in `net::lead_zero_rollback`
    /// (`a_rollback_this_module_did_not_order_delivers_the_shove_and_is_counted`), which reads the
    /// live hull velocity after lightyear's own `RollbackSystems::Prepare`.
    #[test]
    fn the_bypass_counter_only_moves_for_a_rollback_that_really_carried_the_shove() {
        use bevy::ecs::system::RunSystemOnce;

        /// Build the world mid-`FromState`-rollback at `started`, with the staged fact's hull
        /// carrying confirmed velocities sampled at `sampled_at`. The episode is the DEFERRED shape
        /// that separates the two ticks: it settled at 100 and replication materialized it at 104.
        /// WHETHER `prepare_rollback` TOUCHED THIS HULL AT ALL — the archetype half of the same
        /// question, which the fixture could not previously express because the module never asked
        /// it. `Restored` is the shape every earlier version of this fixture built by accident and
        /// asserted deliveries against; the other two are the two ways lightyear's query excludes a
        /// hull, and a confirmed history reads exactly the same in all three.
        #[derive(Clone, Copy)]
        enum InPrepare {
            /// In the query: no `DisableRollback`, and a `PredictionHistory` per component.
            Restored,
            /// `DisableRollback` — excluded for EVERY component at once.
            Disabled,
            /// No `PredictionHistory<AngularVelocity>`, which that query REQUIRES.
            Unhistoried,
        }

        /// The tallies AND the presentation occurrences one run produced. Both, from one run:
        /// "the shove landed" and "the view was told to show it sharp" are separate claims, and a
        /// fixture that read only the first could not see a presentation rule that smooths a
        /// delivered hit away.
        fn run(
            started: Tick,
            sampled_at: Tick,
            membership: InPrepare,
        ) -> (OrderingTally, Vec<Entity>, Entity) {
            let mut world = World::new();
            let hull = world.spawn_empty().id();
            world
                .entity_mut(hull)
                .insert((confirmed_linear(sampled_at), confirmed_angular(sampled_at)));
            // The buffers are empty because nothing here reads them; what the fixture varies is
            // whether the hull is in the ARCHETYPE `prepare_rollback` iterates.
            match membership {
                InPrepare::Restored => {
                    world.entity_mut(hull).insert((
                        PredictionHistory::<LinearVelocity>::default(),
                        PredictionHistory::<AngularVelocity>::default(),
                    ));
                }
                InPrepare::Disabled => {
                    world.entity_mut(hull).insert((
                        PredictionHistory::<LinearVelocity>::default(),
                        PredictionHistory::<AngularVelocity>::default(),
                        DisableRollback,
                    ));
                }
                InPrepare::Unhistoried => {
                    world
                        .entity_mut(hull)
                        .insert(PredictionHistory::<LinearVelocity>::default());
                }
            }

            let manager = crate::net::test_harness::prediction_manager();
            manager.set_rollback_tick(started);
            world.spawn((manager, Rollback::FromState));

            let mut adoption = AuthorityAdoption::default();
            let mut presentation = ImpactPresentation::default();
            let episode = arriving_at(episode_fact(hull, 50, deferred_episode(88, 100)), Tick(104));
            assert_eq!(
                adoption.offer(episode, STAGED_AT, &mut presentation),
                Offer::Staged
            );
            world.insert_resource(adoption);
            world.init_resource::<ForcedRollbackSlot>();
            // THE LEDGER THE FACT WAS STAGED AGAINST, not a fresh one: the staged claim's spark
            // retention is established at the offer, so a run that swapped in an empty resource here
            // would be asking `retirement` a question production never asks it.
            world.insert_resource(presentation);
            world.init_resource::<bevy::ecs::message::Messages<SharpCorrection>>();

            world
                .run_system_once(confirm_forced_rollback)
                .expect("the retirement seam runs");
            let sharp = world
                .resource_mut::<bevy::ecs::message::Messages<SharpCorrection>>()
                .drain()
                .map(|occurrence| occurrence.entity)
                .collect();
            (world.resource::<ImpactPresentation>().tally(), sharp, hull)
        }

        let (tally, sharp, hull) = run(Tick(104), Tick(100), InPrepare::Restored);
        assert_eq!(
            tally,
            OrderingTally {
                bypassed: 1,
                ..default()
            },
            "a rollback this module did not order resolved the authority's tick-100 velocities as \
             the effective state at 104 while the fact was still waiting for its spark. The sample \
             is OLDER than the tick-104 confirmed `HullShock` that certified the episode, and the \
             rule that compared against that tick counted this real delivery as nothing.",
        );
        assert_eq!(
            sharp,
            vec![hull],
            "AND THE VIEW MUST BE TOLD. This rollback was somebody else's — the slot carries no \
             claim of ours at all — so a presentation signal read off the cause tag would smooth \
             a hit that is already on the live hull. The signal is derived from the RETIREMENT.",
        );
        // FINDING THE SLICE-3.11 REVIEW CAUGHT, at this seam. Identical histories, identical depth,
        // identical `Rollback::FromState` — and a hull `prepare_rollback`'s query filters out. The
        // predicate above reads the same "yes" in every one of these runs, because a confirmed
        // history answers what a restore WOULD resolve and knows nothing about whether one happened.
        for excluded in [InPrepare::Disabled, InPrepare::Unhistoried] {
            let (tally, sharp, _) = run(Tick(104), Tick(100), excluded);
            assert_eq!(
                tally,
                OrderingTally::default(),
                "a hull outside `prepare_rollback`'s query was not restored, so nothing was \
                 delivered and `bypassed` may not move. Counting it would inflate the ONE number \
                 that is supposed to say how often this rule is circumvented — with rollbacks that \
                 did not reach the hull at all.",
            );
            assert!(
                sharp.is_empty(),
                "and the view may not be told to keep anything sharp either: refusing to smooth a \
                 correction that carries no hit exposes a seam for nothing. Emitted: {sharp:?}",
            );
        }
        let (tally, sharp, _) = run(Tick(104), Tick(96), InPrepare::Restored);
        assert_eq!(
            tally,
            OrderingTally::default(),
            "same depth, but the newest confirmed sample predates the episode's close, so the \
             restore installed a PRE-hit velocity: nothing was delivered, nothing may be counted",
        );
        assert!(
            sharp.is_empty(),
            "nor told to render anything sharp — the correction on screen is the client's own \
             misprediction and hiding it hides nothing the player is owed. Emitted: {sharp:?}",
        );
    }

    /// The premise the whole module rests on, stated where it can fail loudly: a hit's Δv is under
    /// the velocity gate, so the comparator is not merely unreliable here — it is blind by design.
    #[test]
    fn a_hits_delta_v_is_under_the_velocity_gate() {
        // The 88 mm measured hull Δv the older fixtures deposit.
        let hit = Vec3::new(0.0, 0.0, -0.138_3);
        assert!(
            hit.length() < ROLLBACK_VELOCITY,
            "a hit's Δv ({} m/s) is no longer under the gross-desync gate ({} m/s) — re-derive \
             what this module is for before relaxing anything",
            hit.length(),
            ROLLBACK_VELOCITY,
        );
    }

    /// SOURCE SCAN. `StateRollbackMetadata::request_forced_rollback` has exactly one production
    /// call site, [`ForcedRollbackSlot::claim`], because the single-slot arbitration and the cause
    /// tag are only invariants if there is no way around them. Test setup that stages a competing
    /// claim is legitimate and is why the scan stops at a file's in-file test module.
    ///
    /// SAME LIMITATION AS [`the_three_participation_sites_ask_one_shared_question`], stated here
    /// too because the ninth review found the other one claiming more than it delivers. This is a
    /// LEXICAL scan: it defends against a second call site written by somebody who did not know the
    /// slot existed, which is the real threat model. It does not defend against evasion — an import
    /// alias, a macro-generated call, or a `#[cfg(test)] mod` header spelled differently enough to
    /// truncate the file early all pass it. It is also coarser than the participation scan on
    /// purpose: it does not strip comments, so a file that merely NAMES `request_forced_rollback(`
    /// in prose is an offender. That direction is the safe one for a bare-existence rule.
    #[test]
    fn only_the_forced_rollback_slot_requests_a_forced_rollback() {
        let mut offenders = Vec::new();
        let mut stack = vec![Path::new("src").to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("src/ is readable") {
                let path = entry.expect("a readable directory entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|extension| extension != "rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("a readable source file");
                let production = source
                    .split_once("\n#[cfg(test)]\nmod ")
                    .map_or(source.as_str(), |(before, _)| before);
                if production.contains("request_forced_rollback(") && path != Path::new(file!()) {
                    offenders.push(path.display().to_string());
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "`request_forced_rollback` must be reached only through \
             `net::adoption::ForcedRollbackSlot::claim`, which is what makes the ONE-slot rule and \
             the cause tag enforceable. Offending files: {offenders:?}",
        );
    }

    /// `source` with every comment and every string, byte-string and character literal blanked to
    /// spaces, so a scan reads what the COMPILER reads rather than what the prose says. Newlines
    /// survive, so item structure and any offset a failure reports still mean something.
    ///
    /// SLICE 3.13 WIDENED THIS AND THE NARROW VERSION WAS A REAL HOLE. It blanked whole-line `//`
    /// comments only, so an inline comment or a panic message satisfied a `contains` check — and
    /// this module's panic messages quote the exact spellings the scan below forbids.
    ///
    /// It is a lexer, not a parser: it knows nothing about macros, `cfg`, or meaning.
    fn code_only(source: &str) -> String {
        let chars: Vec<char> = source.chars().collect();
        let mut out = String::with_capacity(source.len());
        let mut index = 0;
        // Blanked, not deleted: a newline stays a newline so the line count is preserved.
        let blanked = |character: char| if character == '\n' { '\n' } else { ' ' };
        while index < chars.len() {
            let current = chars[index];
            let next = chars.get(index + 1).copied();

            // `//` — and `///`, and `//!` — to the end of the line.
            if current == '/' && next == Some('/') {
                while index < chars.len() && chars[index] != '\n' {
                    out.push(' ');
                    index += 1;
                }
                continue;
            }

            // `/* .. */`, nested, possibly spanning lines.
            if current == '/' && next == Some('*') {
                let mut depth = 0usize;
                while index < chars.len() {
                    if chars[index] == '/' && chars.get(index + 1) == Some(&'*') {
                        depth += 1;
                        out.push_str("  ");
                        index += 2;
                        continue;
                    }
                    if chars[index] == '*' && chars.get(index + 1) == Some(&'/') {
                        depth -= 1;
                        out.push_str("  ");
                        index += 2;
                        if depth == 0 {
                            break;
                        }
                        continue;
                    }
                    out.push(blanked(chars[index]));
                    index += 1;
                }
                continue;
            }

            // A raw string: `r` then any run of `#` then `"`. No other valid Rust reaches that
            // shape — a raw IDENTIFIER (`r#type`) has no quote, and an identifier ending in `r`
            // cannot be followed by a bare string — so this needs no look-behind.
            if current == 'r' {
                let mut hashes = 0;
                while chars.get(index + 1 + hashes) == Some(&'#') {
                    hashes += 1;
                }
                if chars.get(index + 1 + hashes) == Some(&'"') {
                    let terminator: String = std::iter::once('"')
                        .chain(std::iter::repeat_n('#', hashes))
                        .collect();
                    let opening = index;
                    index += hashes + 2;
                    let tail: String = chars[index..].iter().collect();
                    let length = tail
                        .find(&terminator)
                        .map_or(chars.len() - index, |offset| {
                            tail[..offset].chars().count() + terminator.chars().count()
                        });
                    let closing = (index + length).min(chars.len());
                    for character in &chars[opening..closing] {
                        out.push(blanked(*character));
                    }
                    index = closing;
                    continue;
                }
            }

            // A character literal, which a LIFETIME is not: `'a'` closes two characters on, and an
            // escape (`'\n'`) always starts with a backslash. Anything else after `'` is `'a` in a
            // type position and stays code.
            if current == '\'' {
                if next != Some('\\') && chars.get(index + 2) != Some(&'\'') {
                    out.push(current);
                    index += 1;
                    continue;
                }
                out.push(' ');
                index += 1;
                while index < chars.len() {
                    if chars[index] == '\\' {
                        out.push(' ');
                        out.push(chars.get(index + 1).copied().map_or(' ', blanked));
                        index += 2;
                        continue;
                    }
                    let closing = chars[index] == '\'';
                    out.push(blanked(chars[index]));
                    index += 1;
                    if closing {
                        break;
                    }
                }
                continue;
            }

            // An ordinary string. `b"..."` arrives here with its `b` already emitted as code, which
            // is harmless — the `b` is not a spelling anything below asks about.
            if current == '"' {
                out.push(' ');
                index += 1;
                while index < chars.len() {
                    if chars[index] == '\\' {
                        out.push(' ');
                        out.push(chars.get(index + 1).copied().map_or(' ', blanked));
                        index += 2;
                        continue;
                    }
                    let closing = chars[index] == '"';
                    out.push(blanked(chars[index]));
                    index += 1;
                    if closing {
                        break;
                    }
                }
                continue;
            }

            out.push(current);
            index += 1;
        }
        out
    }

    /// This module's production source, as the compiler reads it. See [`code_only`].
    fn production_source_as_the_compiler_reads_it() -> String {
        let source = std::fs::read_to_string(file!()).expect("this file is readable");
        let production = source
            .split_once("\n#[cfg(test)]\nmod tests {")
            .map_or(source.as_str(), |(before, _)| before);
        code_only(production)
    }

    /// The text of one top-level item, from its `fn`/`struct`/`impl` line to the `}` in column zero
    /// that closes it. Panics if the item is gone, which is the point: a scan that silently covers
    /// nothing is worse than no scan.
    fn item_body<'a>(source: &'a str, opener: &str) -> &'a str {
        let start = source
            .find(opener)
            .unwrap_or_else(|| panic!("`{opener}` is still in this module's production source"));
        let rest = &source[start..];
        let end = rest
            .find("\n}\n")
            .unwrap_or_else(|| panic!("`{opener}` is closed by a `}}` in column zero"));
        &rest[..end]
    }

    /// Every top-level `fn` in `source`, as `(name, body)`.
    ///
    /// DERIVED RATHER THAN LISTED, which is the ninth review's point: slice 3.12 hard-coded three
    /// function names, so a FOURTH consumer of [`RollbackParticipation`] was invisible until
    /// somebody remembered to extend the list — the same "remember to update the copy" shape this
    /// arc keeps finding in prose.
    ///
    /// COLUMN-ZERO ONLY, AND THAT IS A HOLE, NOT A DESIGN. A line beginning with whitespace is
    /// skipped, so a method in an `impl` and a `fn` in a nested `mod` are not items here — and the
    /// tenth review is right that an `impl` or a nested module is ORDINARY ORGANIZATION rather than
    /// evasion, so a real fourth consumer could be written that way by accident. This function is
    /// therefore no longer the only thing between a new consumer and a green test: the occurrence
    /// count in [`the_three_participation_sites_ask_one_shared_question`] is, and it is blind to
    /// indentation and to line shape. What this adds on top is per-consumer: it checks that each
    /// site it CAN see routes its participation data through the shared predicate.
    fn top_level_functions(source: &str) -> Vec<(&str, &str)> {
        let mut items = Vec::new();
        let mut offset = 0;
        for line in source.split_inclusive('\n') {
            let start = offset;
            offset += line.len();
            if line.starts_with(char::is_whitespace) {
                continue;
            }
            let Some(signature) = line
                .strip_prefix("fn ")
                .or_else(|| line.split_once(" fn ").map(|(_, rest)| rest))
            else {
                continue;
            };
            let name = signature
                .split(|character: char| !character.is_alphanumeric() && character != '_')
                .next()
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let rest = &source[start..];
            let end = rest
                .find("\n}\n")
                .unwrap_or_else(|| panic!("`{name}` is closed by a `}}` in column zero"));
            items.push((name, &rest[..end]));
        }
        items
    }

    /// [`code_only`] is only worth trusting if it is itself checked, and every rule below rests on
    /// it. Each line here is a shape this module's real source contains.
    #[test]
    fn the_scan_reads_code_and_not_prose() {
        let source = concat!(
            "let a = \"Without<DisableRollback>\"; // Has<DisableRollback>\n",
            "/* Has<PredictionHistory<Position>> */ let b = 'x';\n",
            "impl Wrapper<'_, '_> { }\n",
            "let c = r#\"Has<PredictionHistory<Rotation>>\"#;\n",
            "let d = \"an escaped quote \\\" and Has<PredictionHistory<LinearVelocity>>\";\n",
            "let e = prepare_restores(true, [false]);\n",
        );
        let stripped = code_only(source);

        for hidden in [
            "Without<DisableRollback>",
            "Has<DisableRollback>",
            "Has<PredictionHistory<Position>>",
            "Has<PredictionHistory<Rotation>>",
            "Has<PredictionHistory<LinearVelocity>>",
        ] {
            assert!(
                !stripped.contains(hidden),
                "`{hidden}` survived the strip — a comment or literal can satisfy a `contains` \
                 rule again, which is the hole slice 3.13 closed:\n{stripped}",
            );
        }
        assert!(
            stripped.contains("let a = ") && stripped.contains("let e = prepare_restores(true, "),
            "the strip ate CODE, so every count below would be wrong:\n{stripped}",
        );
        assert!(
            stripped.contains("impl Wrapper<'_, '_> {"),
            "a lifetime was mistaken for a character literal, which desyncs the rest of the \
             file:\n{stripped}",
        );
        assert_eq!(
            stripped.lines().count(),
            source.lines().count(),
            "line structure was not preserved, so item boundaries and reported offsets lie",
        );
    }

    /// Every shape [`code_only`] has to get right, pinned. It is one hand-written lexer and EVERY
    /// rule below rests on it, so its blast radius is the whole scan; until the tenth review these
    /// cases were traced by hand and found correct but were not held there by anything.
    ///
    /// Each row hides `HIDE` inside something the compiler does not read and leaves `KEEP` in code.
    /// A lexer that desynchronises — mistaking a lifetime for a character literal, honouring a
    /// comment marker inside a string, letting a short terminator close a long raw string — either
    /// leaks a `HIDE` or eats a `KEEP`, and the char and line invariants catch a strip that changes
    /// the file's shape underneath every offset a failure reports.
    #[test]
    fn the_lexer_reads_every_literal_and_comment_shape_the_module_can_contain() {
        for (shape, source) in [
            (
                "nested block comments below the top level",
                "let KEEP = 0; /* HIDE /* HIDE /* HIDE */ HIDE */ HIDE */ let KEEP = 1;\n",
            ),
            (
                "a block comment opened by the three-character `/*/`",
                "/*/ HIDE */ let KEEP = 1;\n",
            ),
            (
                "an unterminated block comment at end of file",
                "let KEEP = 1;\n/* HIDE\n",
            ),
            (
                "a multi-hash raw string containing its own shorter terminator",
                "let KEEP = r##\"HIDE \"# HIDE\"##; let KEEP = 1;\n",
            ),
            (
                "a raw string containing a line-comment opener",
                "let KEEP = r\"HIDE // HIDE\"; let KEEP = 1;\n",
            ),
            (
                "raw byte strings, hashed and not",
                "let KEEP = br\"HIDE\"; let KEEP = br#\"HIDE\"#; let KEEP = 1;\n",
            ),
            (
                "a raw IDENTIFIER, which is not a raw string",
                "let r#KEEP = 1; let KEEP = \"HIDE\";\n",
            ),
            (
                "a string containing a block-comment opener",
                "let KEEP = \"HIDE /* HIDE\"; let KEEP = 1;\n",
            ),
            (
                "a string containing a block-comment closer",
                "let KEEP = \"HIDE */ HIDE\"; let KEEP = 1;\n",
            ),
            (
                "a string containing a line-comment opener",
                "let KEEP = \"HIDE // HIDE\"; let KEEP = 1;\n",
            ),
            (
                "an unterminated string at end of file",
                "let KEEP = 1; let KEEP = \"HIDE\n",
            ),
            (
                "a line comment containing an unterminated quote",
                "// HIDE it's HIDE\nlet KEEP = 1;\n",
            ),
            (
                "a block comment containing an unterminated quote",
                "/* HIDE don't HIDE */ let KEEP = 1;\n",
            ),
            (
                "a block comment containing a raw-string opener",
                "/* HIDE r#\" HIDE */ let KEEP = 1;\n",
            ),
            (
                "a doc comment, inner and outer",
                "/// HIDE\n//! HIDE\nlet KEEP = 1;\n",
            ),
            (
                "the escaped-quote character literal",
                "let KEEP = '\\''; let KEEP = 1;\n",
            ),
            (
                "the escaped-backslash character literal",
                "let KEEP = '\\\\'; let KEEP = \"HIDE\";\n",
            ),
            (
                "a character literal containing a double quote",
                "let KEEP = '\"'; let KEEP = \"HIDE\"; let KEEP = 1;\n",
            ),
            (
                "byte characters, escaped and not",
                "let KEEP = b'x'; let KEEP = b'\\''; let KEEP = \"HIDE\";\n",
            ),
            (
                // The middle `KEEP` sits BETWEEN the two lifetimes deliberately. A lexer that
                // reads `'a` as a character literal blanks from there to the next apostrophe, and
                // without a marker inside that span the row passes while the regression is live.
                "a lifetime followed by a quote, and by a character literal",
                "fn KEEP<'a>(KEEP: &'a str) -> char { 'q' }\nlet KEEP = \"HIDE\";\n",
            ),
        ] {
            let stripped = code_only(source);
            assert_eq!(
                stripped.matches("HIDE").count(),
                0,
                "{shape}: text the compiler does not read survived the strip, so a comment or a \
                 literal can satisfy a `contains` rule again:\n{stripped}",
            );
            assert_eq!(
                stripped.matches("KEEP").count(),
                source.matches("KEEP").count(),
                "{shape}: the strip ate CODE, so every count and every derived list below reads a \
                 file the compiler never saw:\n{stripped}",
            );
            assert_eq!(
                stripped.chars().count(),
                source.chars().count(),
                "{shape}: blanking changed the file's length, so every reported offset lies",
            );
            assert_eq!(
                stripped.lines().count(),
                source.lines().count(),
                "{shape}: line structure was not preserved, so item boundaries lie",
            );
        }
    }

    /// [`top_level_functions`] is the other half the DERIVED consumer list rests on, and this test
    /// pins both what it finds and what it cannot reach.
    ///
    /// AN EARLIER VERSION OF THIS TEST ASSERTED THE OMISSION AND STOPPED, which read as though the
    /// omission were intended — the tenth review's blocking finding, and the fifth time this arc has
    /// caught a document or a test claiming more than the code delivers. A method in an `impl` and a
    /// `fn` in a nested `mod` are indented, so this helper cannot see them, and they are ordinary
    /// organization rather than evasion. So the test now also demonstrates the compensating rule:
    /// the occurrence count in [`the_three_participation_sites_ask_one_shared_question`] sees all
    /// three consumers below, including the two the item scan misses.
    #[test]
    fn the_item_scan_is_column_zero_only_and_the_occurrence_count_covers_the_rest() {
        let source = concat!(
            "fn plain(hulls: Query<RollbackParticipation>) {\n    fn nested() {}\n}\n",
            "pub(super) fn shared<const N: usize>(value: bool) -> bool {\n    value\n}\n",
            "impl Thing {\n    fn method(&self, hulls: Query<RollbackParticipation>) {}\n}\n",
            "mod extra {\n    fn fourth(hulls: Query<RollbackParticipation>) {}\n}\n",
        );
        let code = code_only(source);
        let names: Vec<&str> = top_level_functions(&code)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            names,
            ["plain", "shared"],
            "the item scan disagrees with what a column-zero `fn` is, so the per-consumer routing \
             check it feeds cannot be trusted",
        );
        assert_eq!(
            code.matches("RollbackParticipation").count(),
            3,
            "the occurrence count must see ALL THREE consumers — the column-zero one and the two \
             the item scan structurally cannot reach. If it does not, the derived list is once \
             again the only thing standing between a new consumer and a green test, which is \
             exactly what the tenth review blocked on:\n{code}",
        );
    }

    /// SOURCE SCAN — A GUARD RAIL, NOT THE CONTRACT. The hull's participation in
    /// `prepare_rollback`'s restore is expressed ONCE, in [`RollbackParticipation`], and every site
    /// that consumes it routes through the shared predicate.
    ///
    /// THE LOAD-BEARING CONTRACT IS THE RUNTIME MATRIX. What holds [`prepare_restores`] to
    /// lightyear's real behaviour is
    /// `net::lead_zero_rollback::prepare_restores_exactly_the_components_the_predicate_names`,
    /// which runs the dependency's own `RollbackSystems::Prepare` over all 32 archetypes the two
    /// membership conditions can produce and compares the restore, per component, against this
    /// module's answer. This test asks a cheaper and much narrower question — is the condition
    /// still spelled in one place — and its value is bounded by that.
    ///
    /// WHAT IT WOULD HAVE CAUGHT: the seventh review's High verbatim. That defect was not a missing
    /// check — it was the same condition written two ways, `Without<DisableRollback>` as a query
    /// FILTER on the offer and nothing at all on the request and the confirmation. A filter is
    /// invisible to any function that is supposed to own the question, so no reader of either site
    /// could see the divergence. Every rule below is red on that source.
    ///
    /// # WHAT IT GUARANTEES, AND WHAT IT CANNOT
    ///
    /// This is a LEXICAL scan of one file, read with comments and literals stripped ([`code_only`]).
    /// **It defends against a future author ACCIDENTALLY re-expressing the condition** — putting the
    /// `Without` filter back on a query, adding a consumer that asks the question its own way,
    /// calling the predicate directly with a subset of its own choosing, renaming a site out from
    /// under a hard-coded list. That is the real threat model in this module: every defect this arc
    /// found was written by somebody who believed the condition was already asked.
    ///
    /// **It does not defend against deliberate evasion, and no lexical scan can.** A macro can
    /// expand to a query type this scan never sees. A consumer can keep a live-looking
    /// `.whole_body()` call and branch on something else entirely — the scan sees the call, not the
    /// dataflow. `prepare_restores` is `pub(super)`, and this scan reads only this file, so a caller
    /// in another `net` module is out of range.
    ///
    /// **AST ANALYSIS, RE-DECIDED WITH THE TENTH REVIEW'S CORRECTION.** Slice 3.13 declined a `syn`
    /// item walk on the grounds that it defends only against deliberate evasion. That reasoning was
    /// WRONG, and knowing why matters more than the verdict: [`top_level_functions`] skips every
    /// indented line, so a consumer written as a method or inside a nested `mod` was invisible to
    /// it — and that is ordinary organization, not a dodge. A real item walk would have covered it.
    /// What changes the trade is that the occurrence count above covers it too, at no cost and with
    /// no parser: it is blind to indentation, to line shape and to rustfmt. What is left for an AST
    /// walk is only the deliberate list in the paragraph above — macro expansion, dataflow, and
    /// callers in other files — none of which a `syn` item walk over this one file would close
    /// either. So it stays declined, now for a reason that survives reading the helper.
    ///
    /// **ADR-0032 claimed the offer-only re-expression "cannot be written without turning it red".
    /// The ninth review killed that, correctly.** It can be, deliberately. Accidentally, it cannot.
    /// Overstating a guard rail into a proof is the exact failure this arc has spent nine rounds
    /// catching in its own documents.
    ///
    /// WHY THIS AND NOT THE GENERAL SCANNER. The saturating form the audit table imagines — every
    /// resource field × every cross-schedule reader — is not buildable honestly: Bevy's schedule
    /// order is a composed partial order across plugins, sets and `run_if`s; reads hide behind
    /// methods; and whether a second site RELIES on a condition is semantic. It would need an
    /// allowlist, and the allowlist would be another hand-maintained copy of the table.
    ///
    /// NOT VACUOUS, in the two ways that matter. Every rule names an item that must exist and
    /// [`item_body`] panics if it does not, so deleting a site fails the test instead of emptying
    /// it. And no consumer list is hand-maintained: `RollbackParticipation` is pinned by OCCURRENCE
    /// COUNT, so a fourth consumer that names the type is red wherever it is written — column zero,
    /// a method in an `impl`, a `fn` in a nested `mod` — rather than only where a line-shape
    /// heuristic happens to look. And the count is EXHAUSTIVE over naming sites, which is the one
    /// place this scan's single-file range costs nothing: `RollbackParticipation` is private to this
    /// module, so no code outside this file can name it at all. (`prepare_restores` is `pub(super)`
    /// and its count carries the usual single-file caveat.) A fourth consumer that never names the
    /// type is still invisible here — that one belongs to the runtime matrix and to review.
    #[test]
    fn the_three_participation_sites_ask_one_shared_question() {
        let production = production_source_as_the_compiler_reads_it();

        assert!(
            !production.contains("Without<DisableRollback>"),
            "`Without<DisableRollback>` is back. The condition must be read as DATA — \
             `RollbackParticipation` — because a query FILTER is the one shape that cannot be handed \
             to `prepare_restores`, and expressing it as a filter on one site while the other two \
             cannot see it IS the seventh review's High.",
        );

        // The membership conditions, pinned on the TYPE NAMES rather than on one `Has<..>` spelling
        // — so a re-expression cannot slip past by writing the query data differently or by
        // importing the marker under another name. `DisableRollback` reaches this module through
        // `lightyear::prelude::*`, so an alias would be a second occurrence and is red here.
        let declaration = item_body(&production, "\nstruct RollbackParticipation {");
        for condition in [
            "DisableRollback",
            "PredictionHistory<Position>",
            "PredictionHistory<Rotation>",
            "PredictionHistory<LinearVelocity>",
            "PredictionHistory<AngularVelocity>",
        ] {
            assert_eq!(
                production.matches(condition).count(),
                1,
                "`{condition}` is named more than once in production — outside \
                 `RollbackParticipation`, or aliased into it. Every site asks this through that one \
                 type, so the conditions cannot drift apart between the offer, the request and the \
                 post-`Prepare` proof, which is exactly how they drifted before.",
            );
            assert_eq!(
                declaration.matches(&format!("Has<{condition}>")).count(),
                1,
                "`RollbackParticipation` must carry `Has<{condition}>` exactly once, read as DATA — \
                 it is the mirror of `prepare_rollback`'s query, and `net::lead_zero_rollback`'s \
                 conformance matrix is what checks that mirror against the real restore.",
            );
        }

        // THE CONSUMERS, PINNED BY OCCURRENCE COUNT — the same rule shape the membership types
        // above use, and the only shape that is blind to how the source is FORMATTED.
        // `RollbackParticipation` is named a known number of times in production and every
        // occurrence is accounted for by name: the declaration, the accessor `impl`'s header, and
        // one query per consumer. A new consumer is a new occurrence WHEREVER it is written — a
        // method in an `impl`, a `fn` in a nested `mod`, indented to any depth, split across lines
        // however rustfmt likes — and it goes red here with no line-shape heuristic involved. The
        // derived list below is column-zero only and would not see any of those, and the tenth
        // review was right that an `impl` or a nested module is ordinary organization rather than
        // evasion.
        const CONSUMERS: [&str; 3] = [
            "confirm_forced_rollback",
            "offer_hull_shock_adoptions",
            "request_staged_adoption",
        ];
        // `struct RollbackParticipation` and `impl RollbackParticipationItem`, the latter matching
        // on the same prefix.
        const DECLARATIONS: usize = 2;
        assert_eq!(
            production.matches("RollbackParticipation").count(),
            DECLARATIONS + CONSUMERS.len(),
            "`RollbackParticipation` is named {} times in production, not {}. Every occurrence is \
             accounted for by name: the declaration, the `RollbackParticipationItem` accessor \
             `impl`, and one query in each of {CONSUMERS:?}. An EXTRA occurrence is a new site \
             asking whether `prepare_rollback` reaches the hull — check it against the audit table \
             in ADR-0032, confirm it routes through the shared predicate, and then account for it \
             here. A MISSING one is a site that stopped asking.",
            production.matches("RollbackParticipation").count(),
            DECLARATIONS + CONSUMERS.len(),
        );

        // THE CONSUMERS THIS SCAN CAN READ, DERIVED. On top of the count, every top-level function
        // that names the type has to route the data through the shared predicate rather than
        // branching on the flags itself.
        let mut consumers: Vec<&str> = top_level_functions(&production)
            .into_iter()
            .filter(|(_, body)| body.contains("RollbackParticipation"))
            .map(|(name, body)| {
                assert!(
                    body.contains(".whole_body()") || body.contains(".velocities()"),
                    "`{name}` carries the participation data without routing it through \
                     `prepare_restores`. Branching on the flags here is how the question gets asked \
                     two slightly different ways again.",
                );
                name
            })
            .collect();
        consumers.sort_unstable();
        assert_eq!(
            consumers, CONSUMERS,
            "the set of COLUMN-ZERO functions asking whether `prepare_rollback` reaches the hull \
             has changed. All three of the listed ones have to ask: gating only the request leaves \
             the post-`Prepare` proof reading a `ConfirmedHistory` lookup no restore performed, \
             which is the half-fix the seventh review named. A MISSING name is a site that stopped \
             asking. A NEW name is a new consumer — the occurrence count above is what catches one \
             written anywhere else in the file.",
        );

        // And the accessors are the only route: nothing else in production may reach the predicate,
        // or "routes through the shared predicate" stops meaning anything. Counted on the bare
        // identifier rather than on `prepare_restores(`, so a turbofish or a path-qualified call is
        // counted too; the definition is the one remaining occurrence.
        let accessors = item_body(&production, "\nimpl RollbackParticipationItem<'_, '_> {");
        assert_eq!(
            production.matches("prepare_restores").count(),
            accessors.matches("prepare_restores").count() + 1,
            "`prepare_restores` is named in production outside `RollbackParticipation`'s two \
             accessors and its own definition. The accessors are what name WHICH components a \
             site's verdict is defined on; a direct caller picks its own subset, which is the sixth \
             review's finding in a new place.",
        );
    }
}
