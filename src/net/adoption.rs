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

use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use bevy_replicon::prelude::RepliconTick;
use lightyear::core::confirmed_history::ConfirmedHistory;
use lightyear::prelude::client::Remote;
use lightyear::prelude::*;

#[cfg(test)]
use super::protocol::ROLLBACK_VELOCITY;
use super::protocol::hull_shock_mismatch;
use super::watchdog::newest_present_at_or_before;
use crate::ballistics::HullShock;

/// DERIVED from `net::grip`'s `MAX_CHECKPOINT_LEDGERS`, for the same reason: one owner needs one
/// entry, and 16 absorbs a reconnect's overlapping despawn/spawn without letting a per-entity
/// ledger grow without bound.
const MAX_FACT_LEDGERS: usize = 16;

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
    /// Whether [`request_staged_adoption`] currently owns the forced-rollback slot for the staged
    /// fact. Bookkeeping only: it is re-derived from the slot every frame rather than trusted.
    requested: bool,
    adopted: Vec<FactId>,
    watermarks: Vec<FactWatermark>,
}

impl AuthorityAdoption {
    /// Offer a fact for unconditional adoption. Idempotent: a producer re-derives its offer from
    /// replicated state every frame and hands it over unconditionally; this decides.
    pub(crate) fn offer(&mut self, fact: AuthoritativeFact) -> Offer {
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
                self.requested = false;
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
        self.requested = false;
    }
}

/// A connection is a new timeline: every tick, entity, and checkpoint identity in the ledgers
/// belongs to the old one.
fn reset_adoption_state(
    _connected: On<Add, Connected>,
    mut adoption: ResMut<AuthorityAdoption>,
    mut slot: ResMut<ForcedRollbackSlot>,
) {
    *adoption = default();
    *slot = default();
}

/// Whether a forced restore at `tick` would replace this component with AUTHORITY, or silently
/// leave the client's own live value standing under a tick-`tick` label.
///
/// Mirrors the branch `prepare_rollback` actually takes. A component that HAS a [`ConfirmedHistory`]
/// is restored from it and only from it, at-or-before the target; with no sample there, lightyear
/// leaves the live value alone and the restored "tick" is a state neither peer ever had. A
/// component with no confirmed history is a local-rollback component, restored from prediction
/// history by contract, and proves nothing either way.
fn authority_reaches<C: Component + Clone>(
    confirmed: Option<&ConfirmedHistory<C>>,
    tick: Tick,
) -> bool {
    confirmed.is_none_or(|history| history.get_state_at_or_before(tick).is_some())
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
    for (hull, confirmed_shock, predicted_shock, position, rotation, linear, angular) in &hulls {
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
        adoption.offer(AuthoritativeFact {
            id: FactId {
                source: FactSource::HullShock,
                entity: hull,
                sequence: authority.count,
                checkpoint,
            },
            cause: AdoptionCause::ExternalEvent,
            produced_at,
        });
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
    adoption.requested = slot.claim(metadata, target, fact.cause);
}

/// Record what lightyear ACTUALLY installed, and close the staged transaction only if it was ours.
///
/// Runs after `RollbackSystems::Prepare`, so `PredictionManager::get_rollback_start_tick` is the
/// installed target and `prepare_rollback` has already restored every history against it. Marking a
/// fact adopted any earlier would spend it on a rollback another subsystem won.
fn confirm_forced_rollback(
    managers: Query<&PredictionManager>,
    mut slot: ResMut<ForcedRollbackSlot>,
    mut adoption: ResMut<AuthorityAdoption>,
) {
    let started = managers
        .single()
        .ok()
        .and_then(PredictionManager::get_rollback_start_tick);
    slot.installed = slot.claim.take().filter(|(tick, _)| started == Some(*tick));
    if let Some((tick, cause)) = slot.installed() {
        debug!(
            "client: forced rollback installed at tick {} — cause {cause:?}",
            tick.0,
        );
    }
    let Some(fact) = adoption.staged else {
        return;
    };
    if started != Some(fact.produced_at) {
        return;
    }
    debug!(
        "client: adopted authoritative fact {:?} #{} on {} (cause {:?}, checkpoint {:?}) — \
         restored end of tick {}",
        fact.id.source,
        fact.id.sequence,
        fact.id.entity,
        fact.cause,
        fact.id.checkpoint,
        fact.produced_at.0,
    );
    adoption.close(fact);
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
        .add_observer(reset_adoption_state)
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

    fn fact(entity: Entity, sequence: u32, checkpoint: u32, tick: u32) -> AuthoritativeFact {
        AuthoritativeFact {
            id: FactId {
                source: FactSource::HullShock,
                entity,
                sequence,
                checkpoint: RepliconTick::new(checkpoint),
            },
            cause: AdoptionCause::ExternalEvent,
            produced_at: Tick(tick),
        }
    }

    fn hull() -> Entity {
        Entity::from_raw_u32(7).expect("a non-placeholder test entity")
    }

    /// The delivery ledger's whole job: a producer that re-offers the same fact every frame — which
    /// is exactly what `offer_hull_shock_adoptions` does — must request exactly once.
    #[test]
    fn a_re_offered_fact_is_requested_once() {
        let mut adoption = AuthorityAdoption::default();
        let episode = fact(hull(), 1, 50, 100);

        assert_eq!(adoption.offer(episode), Offer::Staged);
        assert_eq!(adoption.offer(episode), Offer::Staged);
        assert_eq!(adoption.staged, Some(episode));

        adoption.close(episode);
        assert_eq!(adoption.staged, None);
        assert_eq!(adoption.offer(episode), Offer::AlreadyAdopted);
    }

    /// A re-send of the SAME episode under a later checkpoint has a different [`FactId`], so exact
    /// dedupe alone would let it through and buy a second rollback for one hit. The per-entity
    /// sequence watermark is what stops it.
    #[test]
    fn a_later_checkpoint_carrying_the_same_episode_is_not_a_new_fact() {
        let mut adoption = AuthorityAdoption::default();
        adoption.close(fact(hull(), 1, 50, 100));

        assert_eq!(adoption.offer(fact(hull(), 1, 51, 104)), Offer::NotNewer);
        assert_eq!(adoption.offer(fact(hull(), 0, 51, 104)), Offer::NotNewer);
        assert_eq!(adoption.offer(fact(hull(), 2, 51, 104)), Offer::Staged);
    }

    /// A despawn/respawn gives the replicated hull a new `Entity`, and the new incarnation's first
    /// episode must not be suppressed by the old one's watermark.
    #[test]
    fn a_new_entity_incarnation_starts_a_new_sequence() {
        let mut adoption = AuthorityAdoption::default();
        adoption.close(fact(hull(), 9, 50, 100));

        let reborn = Entity::from_raw_u32(8).expect("a second non-placeholder test entity");
        assert_eq!(adoption.offer(fact(reborn, 1, 51, 104)), Offer::Staged);
    }

    /// One transaction at a time. A second entity's fact waits instead of overwriting the staged
    /// one; its producer re-offers it next frame.
    #[test]
    fn a_second_fact_does_not_evict_the_staged_one() {
        let mut adoption = AuthorityAdoption::default();
        let first = fact(hull(), 1, 50, 100);
        let other = Entity::from_raw_u32(8).expect("a second non-placeholder test entity");

        assert_eq!(adoption.offer(first), Offer::Staged);
        assert_eq!(adoption.offer(fact(other, 1, 50, 100)), Offer::SlotBusy);
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
