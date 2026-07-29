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
//! assumed: [`restore_carries_the_shove`] asks whether the sample `prepare_rollback` actually
//! restored the hull's velocities from is at or after the producing tick, because a state rollback
//! whose newest confirmed sample predates the hit restores a PRE-hit velocity and carries nothing.
//! A best-effort rule with a bypass counter is honest; a guarantee this shape cannot keep is not.
//!
//! One bypass route is structural and worth naming on its own. `HullShock` is still registered
//! `.with_rollback_condition(..)` in `net::protocol`, and that comparator is a pure function of two
//! component values — it cannot consult [`ImpactPresentation`]. Its receive-time dispatch is gated
//! on `confirmed_tick < current_tick`, which is FALSE only at the zero/negative lead loopback
//! produces and TRUE on every link with real latency, so it is inert on loopback and live in WAN
//! play. Making this module the SOLE delivery route — registering an inert condition on `HullShock`
//! so lightyear never rolls back on it directly — is a one-line change plus a rewrite of
//! `net::hull_shock_rollback`'s positive control, but it retires the only delivery path that has
//! ever run on a real link in favour of one whose readiness gates have only been exercised in
//! fixtures, so it wants its own evidence rather than being bundled here. It would SHRINK the hole,
//! not close it: every other rollback cause still restores the same state.

use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use bevy_replicon::prelude::RepliconTick;
use lightyear::core::confirmed_history::ConfirmedHistory;
use lightyear::prelude::client::Remote;
use lightyear::prelude::*;

#[cfg(test)]
use super::protocol::ROLLBACK_VELOCITY;
use super::protocol::{SHOCK_EPISODE_TICKS, hull_shock_mismatch};
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
/// must stay SHARP. `net::render_error` is the consumer that acts on the distinction; this slice
/// only carries the tag.
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
    claim: Option<(Tick, AdoptionCause)>,
    /// The tick and cause of the forced rollback lightyear ACTUALLY installed this frame.
    installed: Option<(Tick, AdoptionCause)>,
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

    /// The tick and cause of the forced rollback installed this frame, if any.
    ///
    /// This is the surface `net::render_error` reads to decide whether to smooth a correction away
    /// or leave it sharp.
    pub(crate) fn installed(&self) -> Option<(Tick, AdoptionCause)> {
        self.installed
    }
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
    /// order restored the authority's post-hit hull state while the fact was still waiting for its
    /// spark. THE honesty number — the ordering rule is best effort, and this is the size of the
    /// gap. It counts only rollbacks whose restore is ESTABLISHED to have carried the shove
    /// ([`restore_carries_the_shove`]): one that restored from the client's own prediction, or from
    /// a confirmed sample older than the hit, leaves the fact staged and is not counted here.
    pub(crate) bypassed: u32,
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

/// How many presented hits are retained. DERIVED: an episode cannot span more than
/// [`SHOCK_EPISODE_TICKS`] ticks (it publishes the tick its window expires, and it opened after the
/// previous one closed), and a fact is staged for at most [`ORDERING_BUDGET_TICKS`], so nothing older
/// than `SHOCK_EPISODE_TICKS + ORDERING_BUDGET_TICKS` = 32 ticks can still be asked for. 32 entries
/// is one per tick of that span — more than four times what the game's fastest weapon can deposit
/// into it (900 rpm cyclic is one hit per ~4.3 ticks).
/// Overflowing it drops the OLDEST, which can only cost a release-on-impact that the budget then
/// covers loudly; it can never release a shove early.
const MAX_PRESENTED_HITS: usize = (SHOCK_EPISODE_TICKS + ORDERING_BUDGET_TICKS as u32) as usize;

/// What this client has SHOWN, and what the ordering rule did with it.
#[derive(Resource, Default)]
pub(crate) struct ImpactPresentation {
    /// The authority-resolved armor hits this client has drawn, oldest first.
    presented: Vec<PresentedHit>,
    tally: OrderingTally,
}

impl ImpactPresentation {
    /// Record that an armor impact belonging to `hit` was drawn.
    fn present(&mut self, hit: PresentedHit) {
        self.presented.push(hit);
        if self.presented.len() > MAX_PRESENTED_HITS {
            self.presented.remove(0);
        }
    }

    /// Whether one of the hits `claim` is made of has been drawn.
    fn shown_for(&self, claim: VisualClaim) -> bool {
        self.presented.iter().any(|hit| claim.covers(*hit))
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
    /// Whether the staged fact has already cleared the ordering rule, and with what verdict. Latched
    /// so a retried request tallies [`ImpactPresentation`] once per FACT rather than once per frame.
    ordering: Option<bool>,
    adopted: Vec<FactId>,
    watermarks: Vec<FactWatermark>,
}

impl AuthorityAdoption {
    /// Offer a fact for unconditional adoption. Idempotent: a producer re-derives its offer from
    /// replicated state every frame and hands it over unconditionally; this decides.
    ///
    /// `now` is the caller's LOCAL tick and is recorded only on the frame the fact is first staged,
    /// which is what makes it a patience clock rather than a re-offer clock.
    pub(crate) fn offer(&mut self, fact: AuthoritativeFact, now: Tick) -> Offer {
        if self.adopted.contains(&fact.id) {
            return Offer::AlreadyAdopted;
        }
        if !self.is_newer(&fact) {
            return Offer::NotNewer;
        }
        match self.staged {
            Some(staged) if staged.id == fact.id => Offer::Staged,
            Some(_) => Offer::SlotBusy,
            None => {
                self.staged = Some(fact);
                self.staged_at = Some(now);
                self.requested = false;
                self.ordering = None;
                Offer::Staged
            }
        }
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
        self.ordering = None;
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
    *adoption = default();
    *slot = default();
    // The presented hits are ticks from the old timeline; the tallies are session instrumentation
    // and deliberately survive, so a reconnect does not erase the number that decides the barrier.
    presentation.presented.clear();
}

/// Whether a state restore at `tick` would replace this component with AUTHORITY, or leave the
/// client's own prediction standing under a tick-`tick` label.
///
/// Mirrors the branch `prepare_rollback` actually takes, and FAILS CLOSED on both of its negative
/// cases. A component that HAS a [`ConfirmedHistory`] is restored from
/// `get_state_at_or_before(rollback_tick)`; with no sample there, lightyear leaves the live value
/// alone. A component with NO confirmed history takes the other branch entirely — `prepare_rollback`
/// restores it from [`PredictionHistory`] even on a state rollback, which for a hull the client never
/// knew was hit is its own un-hit prediction. Treating a missing history as "fine" was therefore
/// backwards: it is the case in which authority provably does NOT reach.
fn authority_reaches<C: Component + Clone>(
    confirmed: Option<&ConfirmedHistory<C>>,
    tick: Tick,
) -> bool {
    confirmed.is_some_and(|history| history.get_state_at_or_before(tick).is_some())
}

/// Whether a state rollback that STARTED at `started` actually put the authority's post-hit hull
/// velocity on the live hull — the predicate [`OrderingTally::bypassed`] means.
///
/// `prepare_rollback` restores each velocity from the newest confirmed sample at or before the
/// rollback's own start tick. A rollback deep enough to reach [`AuthoritativeFact::produced_at`]
/// therefore delivers the shove only if that sample is itself at or after the producing tick; if the
/// newest thing the authority has said about this hull's velocity predates the hit, the restore
/// installs a PRE-hit value and carries nothing, whatever its start tick claims. Both velocities are
/// required, because half a restored rigid body is not a state either peer ever had.
///
/// Fails closed on a missing history for the same reason [`authority_reaches`] does.
fn restore_carries_the_shove(
    linear: Option<&ConfirmedHistory<LinearVelocity>>,
    angular: Option<&ConfirmedHistory<AngularVelocity>>,
    started: Tick,
    produced_at: Tick,
) -> bool {
    fn sample_is_post_hit<C>(
        confirmed: Option<&ConfirmedHistory<C>>,
        started: Tick,
        produced_at: Tick,
    ) -> bool {
        confirmed
            .and_then(|history| newest_present_at_or_before(history, started))
            .is_some_and(|(sample, _)| sample - produced_at >= 0)
    }
    sample_is_post_hit(linear, started, produced_at)
        && sample_is_post_hit(angular, started, produced_at)
}

/// FIRST CONSUMER. Offer the authority's hull-shock episodes for adoption.
///
/// The trigger is the EXACT comparator [`hull_shock_mismatch`], not a magnitude: every field of
/// `HullShock` is discrete, and the whole point is that no magnitude gate can see a hit. What this
/// function owns beyond the trigger is the component-specific half of the readiness proof — that
/// the hull's prediction history is retained at the producing tick and that every authority-tracked
/// part of its rigid-body state can be restored there. The generic half lives in
/// [`request_staged_adoption`].
#[allow(clippy::type_complexity)]
fn offer_hull_shock_adoptions(
    timeline: Res<LocalTimeline>,
    checkpoints: Option<Res<ReplicationCheckpointMap>>,
    hulls: Query<
        (
            Entity,
            &ConfirmedHistory<HullShock>,
            &PredictionHistory<HullShock>,
            Option<&crate::CombatantId>,
            Option<&ConfirmedHistory<Position>>,
            Option<&ConfirmedHistory<Rotation>>,
            Option<&ConfirmedHistory<LinearVelocity>>,
            Option<&ConfirmedHistory<AngularVelocity>>,
        ),
        (With<Predicted>, With<Remote>, Without<DisableRollback>),
    >,
    mut adoption: ResMut<AuthorityAdoption>,
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
    for (hull, confirmed_shock, predicted_shock, combatant, position, rotation, linear, angular) in
        &hulls
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
        // The hull's whole rigid-body state has to come back from authority together — a pose
        // restored to `produced_at` beside a velocity that stayed at `now` is not a tick that ever
        // existed on either peer. If completeness cannot be proven, wait.
        if !authority_reaches(position, produced_at)
            || !authority_reaches(rotation, produced_at)
            || !authority_reaches(linear, produced_at)
            || !authority_reaches(angular, produced_at)
        {
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
                // THE EPISODE'S OWN HITS, in the authority's terms — its own `opened`/`tick` pair,
                // not `produced_at`. `produced_at` is the confirmed sample's tick, which is the right
                // RESTORE target but may sit later than the close if replication did not send that
                // tick; the span belongs to the episode and rides with it.
                visual: combatant.map(|victim| VisualClaim::for_episode(*victim, authority)),
            },
            now,
        );
    }
}

/// Claim the forced-rollback slot for the staged fact, once every generic readiness condition holds.
fn request_staged_adoption(
    timeline: Res<LocalTimeline>,
    checkpoints: Option<Res<ReplicationCheckpointMap>>,
    managers: Query<&PredictionManager>,
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
        adoption.close(fact);
        return;
    }
    // LOCAL patience: ticks this client has held the fact, counted from the frame it was staged.
    let waited = adoption.staged_at.map_or(0, |staged| now - staged);
    if !clear_to_order(&mut adoption, &mut presentation, fact, waited) {
        return;
    }
    adoption.requested = slot.claim(metadata, target, fact.cause);
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
/// `waited` is LOCAL ticks since staging, not the fact's age.
fn clear_to_order(
    adoption: &mut AuthorityAdoption,
    presentation: &mut ImpactPresentation,
    fact: AuthoritativeFact,
    waited: i32,
) -> bool {
    let Some(claim) = fact.visual else {
        return true;
    };
    if adoption.ordering.is_some() {
        return true;
    }
    let shown = presentation.shown_for(claim);
    if !shown && waited < ORDERING_BUDGET_TICKS {
        return false;
    }
    adoption.ordering = Some(shown);
    presentation.resolve(shown, waited);
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
#[allow(clippy::type_complexity)]
fn confirm_forced_rollback(
    managers: Query<(&PredictionManager, Option<&Rollback>)>,
    hulls: Query<(
        Option<&ConfirmedHistory<LinearVelocity>>,
        Option<&ConfirmedHistory<AngularVelocity>>,
    )>,
    mut slot: ResMut<ForcedRollbackSlot>,
    mut adoption: ResMut<AuthorityAdoption>,
    mut presentation: ResMut<ImpactPresentation>,
) {
    let manager = managers.single().ok();
    let started = manager.and_then(|(manager, _)| manager.get_rollback_start_tick());
    let kind = manager
        .and_then(|(_, rollback)| rollback)
        .map(RestoresFrom::of);
    slot.installed = slot.claim.take().filter(|(tick, _)| started == Some(*tick));
    if let Some((tick, cause)) = slot.installed() {
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
    let carried = hulls.get(fact.id.entity).is_ok_and(|(linear, angular)| {
        restore_carries_the_shove(linear, angular, started, fact.produced_at)
    });
    match retirement(&adoption, slot.installed(), started, kind, carried) {
        None | Some(Retirement::Keep) => return,
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
             module did not order — already released by the ordering rule, so nothing was missed",
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
    /// A confirmed-state rollback this module did not order restored the fact's producing tick from
    /// authority, so the shove is already on the live hull. `spark_pending` is whether it beat the
    /// visual — a BYPASS.
    Delivered { spark_pending: bool },
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
/// `Adopted` deliberately does NOT consult `carried`. Our own claim was installed and
/// `prepare_rollback` did whatever the histories allowed; re-requesting the identical rollback would
/// loop on a tick that cannot improve. The readiness gate is what keeps that case honest —
/// [`authority_reaches`] refuses to offer a fact whose hull cannot be restored from authority at the
/// producing tick at all.
fn retirement(
    adoption: &AuthorityAdoption,
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
        return Some(Retirement::Adopted);
    }
    // A rollback SHALLOWER than the producing tick restores pre-hit state and replays forward from
    // it, so it carries none of the authority's post-hit hull velocity. Nothing to retire. Same
    // answer, for the same reason, when a deep-enough rollback restored from a confirmed sample that
    // predates the hit.
    if started - fact.produced_at < 0 || !carried {
        return Some(Retirement::Keep);
    }
    Some(Retirement::Delivered {
        spark_pending: adoption.ordering.is_none(),
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

    /// A COALESCED episode closed at `tick`: its first hit landed a full window earlier and was
    /// deferred, which is the widest span the authority can publish. Built through the same
    /// [`VisualClaim::for_episode`] the production offer uses, so a fixture cannot assert a claim
    /// shape production does not produce.
    fn fact(entity: Entity, sequence: u32, checkpoint: u32, tick: u32) -> AuthoritativeFact {
        episode_fact(
            entity,
            sequence,
            checkpoint,
            HullShock {
                count: sequence,
                tick,
                opened: tick - (SHOCK_EPISODE_TICKS - 1),
                cause: crate::ballistics::ShockCause::Perforation,
            },
        )
    }

    /// The offer `offer_hull_shock_adoptions` builds for `episode`.
    fn episode_fact(
        entity: Entity,
        sequence: u32,
        checkpoint: u32,
        episode: HullShock,
    ) -> AuthoritativeFact {
        AuthoritativeFact {
            id: FactId {
                source: FactSource::HullShock,
                entity,
                sequence,
                checkpoint: RepliconTick::new(checkpoint),
            },
            cause: AdoptionCause::ExternalEvent,
            produced_at: Tick(episode.tick),
            visual: Some(VisualClaim::for_episode(VICTIM, &episode)),
        }
    }

    /// A COALESCED episode: its first hit landed at `opened` inside an already-open episode's
    /// window, so it published at `tick` when that window expired rather than on its own tick.
    fn deferred_episode(opened: u32, tick: u32) -> HullShock {
        HullShock {
            count: 2,
            tick,
            opened,
            cause: crate::ballistics::ShockCause::Perforation,
        }
    }

    /// The episode a FRESH `HullShockLedger` publishes for its first hit: no open episode to defer
    /// behind, so it closes on the tick it was armed and spans that single tick.
    fn first_episode(tick: u32) -> HullShock {
        HullShock {
            count: 1,
            tick,
            opened: tick,
            cause: crate::ballistics::ShockCause::Perforation,
        }
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
        let episode = fact(hull(), 1, 50, 100);

        assert_eq!(adoption.offer(episode, STAGED_AT), Offer::Staged);
        assert_eq!(adoption.offer(episode, STAGED_AT + 3), Offer::Staged);
        assert_eq!(adoption.staged, Some(episode));
        assert_eq!(
            adoption.staged_at,
            Some(STAGED_AT),
            "the patience clock starts when the fact is FIRST staged; a re-offer must not reset it",
        );

        adoption.close(episode);
        assert_eq!(adoption.staged, None);
        assert_eq!(adoption.staged_at, None);
        assert_eq!(adoption.offer(episode, STAGED_AT), Offer::AlreadyAdopted);
    }

    /// A re-send of the SAME episode under a later checkpoint has a different [`FactId`], so exact
    /// dedupe alone would let it through and buy a second rollback for one hit. The per-entity
    /// sequence watermark is what stops it.
    #[test]
    fn a_later_checkpoint_carrying_the_same_episode_is_not_a_new_fact() {
        let mut adoption = AuthorityAdoption::default();
        adoption.close(fact(hull(), 1, 50, 100));

        for stale in [fact(hull(), 1, 51, 104), fact(hull(), 0, 51, 104)] {
            assert_eq!(adoption.offer(stale, STAGED_AT), Offer::NotNewer);
        }
        assert_eq!(
            adoption.offer(fact(hull(), 2, 51, 104), STAGED_AT),
            Offer::Staged,
        );
    }

    /// A despawn/respawn gives the replicated hull a new `Entity`, and the new incarnation's first
    /// episode must not be suppressed by the old one's watermark.
    #[test]
    fn a_new_entity_incarnation_starts_a_new_sequence() {
        let mut adoption = AuthorityAdoption::default();
        adoption.close(fact(hull(), 9, 50, 100));

        let reborn = Entity::from_raw_u32(8).expect("a second non-placeholder test entity");
        assert_eq!(
            adoption.offer(fact(reborn, 1, 51, 104), STAGED_AT),
            Offer::Staged
        );
    }

    /// One transaction at a time. A second entity's fact waits instead of overwriting the staged
    /// one; its producer re-offers it next frame.
    #[test]
    fn a_second_fact_does_not_evict_the_staged_one() {
        let mut adoption = AuthorityAdoption::default();
        let first = fact(hull(), 1, 50, 100);
        let other = Entity::from_raw_u32(8).expect("a second non-placeholder test entity");

        assert_eq!(adoption.offer(first, STAGED_AT), Offer::Staged);
        assert_eq!(
            adoption.offer(fact(other, 1, 50, 100), STAGED_AT),
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
        let deferred = episode_fact(hull(), 2, 51, deferred_episode(104, 116));
        assert_eq!(adoption.offer(deferred, STAGED_AT), Offer::Staged);

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
        let deferred = episode_fact(hull(), 2, 51, deferred_episode(104, 116));
        assert_eq!(adoption.offer(deferred, STAGED_AT), Offer::Staged);

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
        let episode = episode_fact(hull(), 1, 50, first_episode(100));
        assert_eq!(adoption.offer(episode, STAGED_AT), Offer::Staged);

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
        let old_hull = hull();
        let died_at = 100;
        for tick in (died_at - 15)..died_at {
            presentation.present(drew(tick, VICTIM));
        }
        adoption.close(episode_fact(old_hull, 4, 50, first_episode(died_at - 15)));

        // The life that began: a new `Entity`, the same combatant, a fresh ledger — so the first
        // episode it publishes is `count: 1` again, closed the tick its first hit landed.
        let reborn = Entity::from_raw_u32(8).expect("a second non-placeholder test entity");
        let first = episode_fact(reborn, 1, 51, first_episode(died_at));
        assert_eq!(adoption.offer(first, STAGED_AT), Offer::Staged);

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
    #[test]
    fn a_hit_on_another_combatant_never_releases_this_hulls_shove() {
        let mut adoption = AuthorityAdoption::default();
        let mut presentation = ImpactPresentation::default();
        let episode = fact(hull(), 1, 50, 100);
        assert_eq!(adoption.offer(episode, STAGED_AT), Offer::Staged);

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
        assert_eq!(adoption.offer(episode, STAGED_AT), Offer::Staged);

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
        assert_eq!(adoption.offer(episode, STAGED_AT), Offer::Staged);
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
        let episode = fact(hull(), 1, 50, 100);
        assert_eq!(adoption.offer(episode, STAGED_AT), Offer::Staged);

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
        let episode = fact(hull(), 1, 50, 100);
        assert_eq!(adoption.offer(episode, STAGED_AT), Offer::Staged);

        assert_eq!(
            retirement(&adoption, None, Tick(100), RestoresFrom::Authority, CARRIED),
            Some(Retirement::Delivered {
                spark_pending: true
            }),
            "a rollback nobody here claimed is a BYPASS, never an adoption",
        );
        adoption.requested = true;
        assert_eq!(
            retirement(
                &adoption,
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
                Some((Tick(100), AdoptionCause::ExternalEvent)),
                Tick(100),
                RestoresFrom::Authority,
                CARRIED,
            ),
            Some(Retirement::Adopted),
        );
    }

    /// The other half of the same rule. A rollback SHALLOWER than the producing tick restores
    /// pre-hit state and replays forward from it, so it carries none of the shove: the fact must
    /// stay staged and be requested again rather than be silently spent.
    #[test]
    fn a_shallower_rollback_retires_nothing() {
        let mut adoption = AuthorityAdoption::default();
        let episode = fact(hull(), 1, 50, 100);
        assert_eq!(adoption.offer(episode, STAGED_AT), Offer::Staged);

        assert_eq!(
            retirement(&adoption, None, Tick(99), RestoresFrom::Authority, CARRIED),
            Some(Retirement::Keep)
        );
        assert_eq!(
            retirement(&adoption, None, Tick(101), RestoresFrom::Authority, CARRIED),
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
        let episode = fact(hull(), 1, 50, 100);
        assert_eq!(adoption.offer(episode, STAGED_AT), Offer::Staged);
        adoption.requested = true;

        for started in [Tick(100), Tick(101), Tick(140)] {
            assert_eq!(
                retirement(
                    &adoption,
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

    /// A bypass is only a bypass while the ordering rule is still holding the fact. Once the rule
    /// has released it, an unrelated rollback that carries the shove missed nothing.
    #[test]
    fn a_released_fact_carried_by_someone_elses_rollback_is_not_a_bypass() {
        let mut adoption = AuthorityAdoption::default();
        let mut presentation = ImpactPresentation::default();
        let episode = fact(hull(), 1, 50, 100);
        assert_eq!(adoption.offer(episode, STAGED_AT), Offer::Staged);

        presentation.present(drew(100, VICTIM));
        assert!(order(&mut adoption, &mut presentation, episode, 0));
        assert_eq!(
            retirement(&adoption, None, Tick(100), RestoresFrom::Authority, CARRIED),
            Some(Retirement::Delivered {
                spark_pending: false
            }),
        );
    }

    /// FINDING THE SLICE-3.6 REVIEW CAUGHT. `bypassed` claims the shove LANDED, so it has to be
    /// decided from the histories `prepare_rollback` actually restored from — not from the rollback's
    /// start tick, which says only how deep it went.
    ///
    /// The three ways a state rollback at a deep-enough tick still carries nothing, over real
    /// [`ConfirmedHistory`] buffers: no history at all (lightyear's `prepare_rollback` takes the
    /// other branch and restores the client's own prediction), a history whose newest sample predates
    /// the hit, and one velocity resolved while the other is not.
    #[test]
    fn a_restore_carries_the_shove_only_when_its_samples_are_at_or_past_the_hit() {
        const PRODUCED_AT: Tick = Tick(100);
        const STARTED: Tick = Tick(104);

        let post_hit = || {
            let mut history = ConfirmedHistory::<LinearVelocity>::default();
            history.insert_present_explicit(PRODUCED_AT, LinearVelocity(Vec3::NEG_Z * 0.138_3));
            history
        };
        let pre_hit = || {
            let mut history = ConfirmedHistory::<AngularVelocity>::default();
            history.insert_present_explicit(Tick(96), AngularVelocity(Vec3::ZERO));
            history
        };
        let post_hit_angular = || {
            let mut history = ConfirmedHistory::<AngularVelocity>::default();
            history.insert_present_explicit(PRODUCED_AT, AngularVelocity(Vec3::Y * 0.191));
            history
        };

        assert!(
            restore_carries_the_shove(
                Some(&post_hit()),
                Some(&post_hit_angular()),
                STARTED,
                PRODUCED_AT,
            ),
            "both velocities resolve to the authority's own post-hit sample",
        );
        assert!(
            !restore_carries_the_shove(None, Some(&post_hit_angular()), STARTED, PRODUCED_AT),
            "with no confirmed history `prepare_rollback` restores the client's own prediction — \
             the un-hit one — so nothing was delivered and nothing was bypassed",
        );
        assert!(
            !restore_carries_the_shove(Some(&post_hit()), Some(&pre_hit()), STARTED, PRODUCED_AT),
            "an angular sample older than the hit restores a PRE-hit value; half a restored rigid \
             body is not the shove",
        );
        let stale_linear = {
            let mut history = ConfirmedHistory::<LinearVelocity>::default();
            history.insert_present_explicit(Tick(96), LinearVelocity(Vec3::ZERO));
            history
        };
        assert!(
            !restore_carries_the_shove(
                Some(&stale_linear),
                Some(&post_hit_angular()),
                STARTED,
                PRODUCED_AT,
            ),
            "a rollback that started PAST the producing tick still carries nothing when the newest \
             thing the authority said about this velocity predates the hit",
        );
    }

    /// THE SAME RULE THROUGH THE SYSTEM THAT DECIDES IT, not around it. `retirement` is a pure
    /// function and the fixtures above hand it a verdict; this one runs `confirm_forced_rollback` on
    /// a real `PredictionManager` mid-rollback, with real confirmed histories on the staged fact's
    /// own entity, and reads the tally the diagnostics line reports.
    #[test]
    fn the_bypass_counter_only_moves_for_a_rollback_that_really_carried_the_shove() {
        use bevy::ecs::system::RunSystemOnce;

        /// Build the world mid-`FromState`-rollback at `started`, with the staged fact's hull
        /// carrying confirmed velocities sampled at `sampled_at`.
        fn run(started: Tick, sampled_at: Tick) -> OrderingTally {
            let mut world = World::new();
            let hull = world.spawn_empty().id();
            let mut linear = ConfirmedHistory::<LinearVelocity>::default();
            linear.insert_present_explicit(sampled_at, LinearVelocity(Vec3::NEG_Z * 0.138_3));
            let mut angular = ConfirmedHistory::<AngularVelocity>::default();
            angular.insert_present_explicit(sampled_at, AngularVelocity(Vec3::Y * 0.191));
            world.entity_mut(hull).insert((linear, angular));

            let manager = PredictionManager::default();
            manager.set_rollback_tick(started);
            world.spawn((manager, Rollback::FromState));

            let mut adoption = AuthorityAdoption::default();
            let episode = episode_fact(hull, 1, 50, first_episode(100));
            assert_eq!(adoption.offer(episode, STAGED_AT), Offer::Staged);
            world.insert_resource(adoption);
            world.init_resource::<ForcedRollbackSlot>();
            world.init_resource::<ImpactPresentation>();

            world
                .run_system_once(confirm_forced_rollback)
                .expect("the retirement seam runs");
            world.resource::<ImpactPresentation>().tally()
        }

        assert_eq!(
            run(Tick(104), Tick(100)),
            OrderingTally {
                bypassed: 1,
                ..default()
            },
            "a rollback this module did not order restored the authority's tick-100 velocities \
             while the fact was still waiting for its spark — that IS the gap the counter measures",
        );
        assert_eq!(
            run(Tick(104), Tick(96)),
            OrderingTally::default(),
            "same depth, but the newest confirmed sample predates the hit, so the restore installed \
             a PRE-hit velocity: nothing was delivered, so nothing may be counted as bypassed",
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
}
