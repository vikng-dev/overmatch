//! Predicted-client anchor comparison, exact-checkpoint admission, and rollback correction.

use avian3d::prelude::{ComputedAngularInertia, ComputedMass, Rotation};
use bevy::prelude::*;
use lightyear::prelude::client::Remote;
use lightyear::prelude::*;

use crate::CombatantId;
use crate::net::protocol::{
    GripCheckpointChunk, GripRequestChannel, GripResyncRequest, NetTrackGripAnchor,
};
use crate::trace::{GripAnchorTrace, GripRequestReason, TraceWriter};
use crate::track::sim::{TrackGear, TrackGripEffect, TrackGripElements, coarse_grip_digest};

use super::checkpoint::{CheckpointAssembler, CheckpointKey, ExactCheckpoint, fields_bit_equal};
use super::{
    DEFERRED_CHECKPOINT_EXPIRY_TICKS, DIGEST_PERSIST_ANCHORS,
    EFFECT_ANGULAR_VELOCITY_THRESHOLD_RADPS, EFFECT_BELT_SPEED_THRESHOLD_MPS,
    EFFECT_LINEAR_VELOCITY_THRESHOLD_MPS, GripRequestLimiter, MAX_CHECKPOINT_LEDGERS,
    MAX_DEFERRED_CHECKPOINT_CHUNKS,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CheckpointWatermark {
    tank: Entity,
    epoch: u32,
    tick: Tick,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AnchorEpoch {
    tank: Entity,
    epoch: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AdmissionError {
    Duplicate,
    NoRollbackBaseline,
    NotNewerThanWatermark,
    OlderThanAnchorEpoch,
}

#[derive(Resource, Default)]
pub(super) struct PendingGripCorrection {
    pub(super) checkpoint: Option<ExactCheckpoint>,
    pub(super) rollback_requested: bool,
    applied: Vec<CheckpointKey>,
    watermarks: Vec<CheckpointWatermark>,
    anchor_epochs: Vec<AnchorEpoch>,
}

fn wrapping_newer(candidate: u32, current: u32) -> bool {
    candidate != current && candidate.wrapping_sub(current) as i32 > 0
}

fn checkpoint_position_is_newer(
    candidate_epoch: u32,
    candidate_tick: Tick,
    current_epoch: u32,
    current_tick: Tick,
) -> bool {
    if candidate_epoch == current_epoch {
        candidate_tick.0 > current_tick.0
    } else {
        wrapping_newer(candidate_epoch, current_epoch)
    }
}

impl PendingGripCorrection {
    pub(super) fn observe_anchor_epoch(&mut self, tank: Entity, epoch: u32) {
        let anchor_index = self
            .anchor_epochs
            .iter()
            .position(|anchor| anchor.tank == tank);
        let advanced = match anchor_index {
            Some(index) if wrapping_newer(epoch, self.anchor_epochs[index].epoch) => {
                self.anchor_epochs[index].epoch = epoch;
                true
            }
            Some(_) => false,
            None => {
                self.anchor_epochs.push(AnchorEpoch { tank, epoch });
                if self.anchor_epochs.len() > MAX_CHECKPOINT_LEDGERS {
                    self.anchor_epochs.remove(0);
                }
                true
            }
        };
        if advanced
            && self.checkpoint.as_ref().is_some_and(|checkpoint| {
                checkpoint.tank == tank
                    && checkpoint.epoch != epoch
                    && !wrapping_newer(checkpoint.epoch, epoch)
            })
        {
            self.checkpoint = None;
            self.rollback_requested = false;
        }
    }

    pub(super) fn stage(&mut self, checkpoint: ExactCheckpoint) -> Result<(), AdmissionError> {
        // Exact state is installed at the end of the tick before `state_entering_tick`. Tick zero
        // has no preceding timeline state, so it cannot provide a valid rollback baseline.
        if checkpoint.state_entering_tick.0 == 0 {
            return Err(AdmissionError::NoRollbackBaseline);
        }
        let key = CheckpointKey::from(&checkpoint);
        if self.applied.contains(&key) {
            return Err(AdmissionError::Duplicate);
        }
        if self.anchor_epochs.iter().any(|anchor| {
            anchor.tank == checkpoint.tank
                && checkpoint.epoch != anchor.epoch
                && !wrapping_newer(checkpoint.epoch, anchor.epoch)
        }) {
            return Err(AdmissionError::OlderThanAnchorEpoch);
        }
        if self.watermarks.iter().any(|watermark| {
            watermark.tank == checkpoint.tank
                && !checkpoint_position_is_newer(
                    checkpoint.epoch,
                    checkpoint.state_entering_tick,
                    watermark.epoch,
                    watermark.tick,
                )
        }) {
            return Err(AdmissionError::NotNewerThanWatermark);
        }

        if let Some(watermark) = self
            .watermarks
            .iter_mut()
            .find(|watermark| watermark.tank == checkpoint.tank)
        {
            watermark.epoch = checkpoint.epoch;
            watermark.tick = checkpoint.state_entering_tick;
        } else {
            self.watermarks.push(CheckpointWatermark {
                tank: checkpoint.tank,
                epoch: checkpoint.epoch,
                tick: checkpoint.state_entering_tick,
            });
            if self.watermarks.len() > MAX_CHECKPOINT_LEDGERS {
                self.watermarks.remove(0);
            }
        }
        self.checkpoint = Some(checkpoint);
        self.rollback_requested = false;
        Ok(())
    }

    pub(super) fn mark_applied(&mut self, key: CheckpointKey) {
        if !self.applied.contains(&key) {
            self.applied.push(key);
            if self.applied.len() > MAX_CHECKPOINT_LEDGERS {
                self.applied.remove(0);
            }
        }
        if let Some(watermark) = self
            .watermarks
            .iter_mut()
            .find(|watermark| watermark.tank == key.tank)
            && checkpoint_position_is_newer(key.epoch, key.tick, watermark.epoch, watermark.tick)
        {
            watermark.epoch = key.epoch;
            watermark.tick = key.tick;
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct DigestWatch {
    tank: Entity,
    epoch: u32,
    mismatches: u8,
}

#[derive(Clone, Copy, Debug)]
struct RepairEvidence {
    tank: Entity,
    epoch: u32,
    digest: u32,
    spent: bool,
}

impl Default for DigestWatch {
    fn default() -> Self {
        Self {
            tank: Entity::PLACEHOLDER,
            epoch: 0,
            mismatches: 0,
        }
    }
}

#[derive(Resource, Default)]
pub(super) struct AnchorWatch {
    digests: Vec<DigestWatch>,
    evidence: Vec<RepairEvidence>,
    traces: Vec<(Entity, GripAnchorTrace)>,
    compared_anchors: Vec<(Entity, Tick)>,
}

impl AnchorWatch {
    pub(super) fn evidence_spent(&mut self, tank: Entity, epoch: u32, digest: u32) -> bool {
        if let Some(evidence) = self.evidence.iter_mut().find(|entry| entry.tank == tank) {
            if evidence.epoch != epoch || evidence.digest != digest {
                *evidence = RepairEvidence {
                    tank,
                    epoch,
                    digest,
                    spent: false,
                };
            }
            return evidence.spent;
        }
        self.evidence.push(RepairEvidence {
            tank,
            epoch,
            digest,
            spent: false,
        });
        if self.evidence.len() > MAX_CHECKPOINT_LEDGERS {
            self.evidence.remove(0);
        }
        false
    }

    pub(super) fn spend_evidence(&mut self, tank: Entity, epoch: u32, digest: u32) {
        let _ = self.evidence_spent(tank, epoch, digest);
        if let Some(evidence) = self.evidence.iter_mut().find(|entry| entry.tank == tank) {
            evidence.spent = true;
        }
    }

    fn remember_trace(&mut self, tank: Entity, trace: GripAnchorTrace) {
        if let Some((_, existing)) = self.traces.iter_mut().find(|(entry, _)| *entry == tank) {
            *existing = trace;
            return;
        }
        self.traces.push((tank, trace));
        if self.traces.len() > MAX_CHECKPOINT_LEDGERS {
            self.traces.remove(0);
        }
    }

    fn trace_for(&self, tank: Entity) -> Option<GripAnchorTrace> {
        self.traces
            .iter()
            .find_map(|(entry, trace)| (*entry == tank).then_some(*trace))
    }

    pub(super) fn anchor_was_compared(&self, tank: Entity, producing_tick: Tick) -> bool {
        self.compared_anchors.contains(&(tank, producing_tick))
    }

    pub(super) fn mark_anchor_compared(&mut self, tank: Entity, producing_tick: Tick) {
        if let Some(entry) = self
            .compared_anchors
            .iter_mut()
            .find(|(entry_tank, _)| *entry_tank == tank)
        {
            *entry = (tank, producing_tick);
            return;
        }
        self.compared_anchors.push((tank, producing_tick));
        if self.compared_anchors.len() > MAX_CHECKPOINT_LEDGERS {
            self.compared_anchors.remove(0);
        }
    }
}

#[derive(Clone, Debug)]
struct DeferredCheckpointChunk {
    received_at: Tick,
    chunk: GripCheckpointChunk,
}

/// Chunks whose stable combatant identity has not yet resolved to exactly one local controlled tank.
#[derive(Resource, Default)]
pub(super) struct DeferredCheckpointChunks {
    chunks: Vec<DeferredCheckpointChunk>,
}

impl DeferredCheckpointChunks {
    pub(super) fn defer(&mut self, chunk: GripCheckpointChunk, now: Tick) {
        if self.chunks.len() >= MAX_DEFERRED_CHECKPOINT_CHUNKS {
            let expired = self.chunks.remove(0);
            warn!(
                "client: expired deferred grip checkpoint chunk for combatant {:?} epoch={} \
                 entering_tick={} chunk={}/{}: bounded queue is full",
                expired.chunk.combatant,
                expired.chunk.epoch,
                expired.chunk.state_entering_tick.0,
                expired.chunk.chunk_index,
                expired.chunk.chunk_count,
            );
        }
        self.chunks.push(DeferredCheckpointChunk {
            received_at: now,
            chunk,
        });
    }

    pub(super) fn take_resolvable(
        &mut self,
        now: Tick,
        mut resolve: impl FnMut(CombatantId) -> Option<Entity>,
    ) -> Vec<(Entity, GripCheckpointChunk)> {
        let mut retained = Vec::with_capacity(self.chunks.len());
        let mut ready = Vec::new();
        for deferred in std::mem::take(&mut self.chunks) {
            let elapsed = now.0.wrapping_sub(deferred.received_at.0);
            if elapsed > DEFERRED_CHECKPOINT_EXPIRY_TICKS {
                warn!(
                    "client: expired deferred grip checkpoint chunk for combatant {:?} epoch={} \
                     entering_tick={} chunk={}/{} after {} ticks: target is not uniquely resolvable",
                    deferred.chunk.combatant,
                    deferred.chunk.epoch,
                    deferred.chunk.state_entering_tick.0,
                    deferred.chunk.chunk_index,
                    deferred.chunk.chunk_count,
                    elapsed,
                );
            } else if let Some(tank) = resolve(deferred.chunk.combatant) {
                ready.push((tank, deferred.chunk));
            } else {
                retained.push(deferred);
            }
        }
        self.chunks = retained;
        ready
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.chunks.len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CheckpointDisposition {
    NeedsRollback,
    NoOp,
}

fn historical_field_at_baseline(
    history: &PredictionHistory<TrackGripElements>,
    baseline: Tick,
    current_tick: Tick,
) -> Option<&TrackGripElements> {
    // `HistoryBuffer::get` is a floor lookup. Before the client has run `baseline`, asking for that
    // future tick would silently return the newest older field and manufacture a false no-op.
    if current_tick - baseline < 0 {
        return None;
    }
    history.get(baseline)
}

fn complete_staged_no_op(
    pending: &mut PendingGripCorrection,
    watch: &mut AnchorWatch,
    field_history: &PredictionHistory<TrackGripElements>,
    current_tick: Tick,
) -> Option<ExactCheckpoint> {
    let checkpoint = pending.checkpoint.as_ref()?;
    let baseline = checkpoint_rollback_baseline(checkpoint.state_entering_tick);
    let baseline_field = historical_field_at_baseline(field_history, baseline, current_tick)?;
    if !fields_bit_equal(&checkpoint.field, baseline_field) {
        return None;
    }

    let checkpoint = pending
        .checkpoint
        .take()
        .expect("the bit-identical staged checkpoint still exists");
    let key = CheckpointKey::from(&checkpoint);
    // The exact authority field already matches the state rollback would restore at its baseline.
    // Replaying from that same state cannot repair the effect/history discrepancy that requested
    // bytes, so the evidence is exhausted until epoch or digest advances.
    let (evidence_epoch, evidence_digest) = watch.trace_for(checkpoint.tank).map_or(
        (checkpoint.epoch, coarse_grip_digest(&checkpoint.field)),
        |trace| (trace.epoch, trace.authority.field_digest),
    );
    watch.spend_evidence(checkpoint.tank, evidence_epoch, evidence_digest);
    pending.mark_applied(key);
    pending.rollback_requested = false;
    Some(checkpoint)
}

pub(super) fn admit_completed_checkpoint(
    pending: &mut PendingGripCorrection,
    watch: &mut AnchorWatch,
    checkpoint: ExactCheckpoint,
    field_history: &PredictionHistory<TrackGripElements>,
    current_tick: Tick,
) -> Result<CheckpointDisposition, AdmissionError> {
    pending.stage(checkpoint)?;
    if complete_staged_no_op(pending, watch, field_history, current_tick).is_some() {
        Ok(CheckpointDisposition::NoOp)
    } else {
        Ok(CheckpointDisposition::NeedsRollback)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct EffectErrors {
    pub(super) linear_velocity: f32,
    pub(super) angular_velocity: f32,
    pub(super) belt_speed: [f32; 2],
}

pub(super) fn reset_client_grip_state(
    _connected: On<Add, Connected>,
    mut assembler: ResMut<CheckpointAssembler>,
    mut deferred: ResMut<DeferredCheckpointChunks>,
    mut pending: ResMut<PendingGripCorrection>,
    mut limiter: ResMut<GripRequestLimiter>,
    mut watch: ResMut<AnchorWatch>,
) {
    *assembler = default();
    *deferred = default();
    *pending = default();
    *limiter = default();
    *watch = default();
}

pub(super) fn receive_checkpoint_chunks(
    timeline: Res<LocalTimeline>,
    mut receivers: Query<&mut MessageReceiver<GripCheckpointChunk>, With<Client>>,
    fields: Query<
        (
            Entity,
            &CombatantId,
            &TrackGripElements,
            &PredictionHistory<TrackGripElements>,
        ),
        (With<Remote>, With<Controlled>),
    >,
    mut deferred: ResMut<DeferredCheckpointChunks>,
    mut assembler: ResMut<CheckpointAssembler>,
    mut pending: ResMut<PendingGripCorrection>,
    mut watch: ResMut<AnchorWatch>,
    mut trace: Option<ResMut<TraceWriter>>,
) {
    let now = timeline.tick();
    let resolve_combatant = |combatant| {
        let mut matches = fields
            .iter()
            .filter(|(_, candidate, _, _)| **candidate == combatant);
        let tank = matches.next().map(|(tank, _, _, _)| tank)?;
        matches.next().is_none().then_some(tank)
    };
    let mut ready = Vec::new();
    for mut receiver in &mut receivers {
        for chunk in receiver.receive() {
            if let Some(tank) = resolve_combatant(chunk.combatant) {
                ready.push((tank, chunk));
            } else {
                deferred.defer(chunk, now);
            }
        }
    }
    ready.extend(deferred.take_resolvable(now, resolve_combatant));
    for (tank, chunk) in ready {
        let Ok((_, _, field, field_history)) = fields.get(tank) else {
            continue;
        };
        let expected_elements_per_side = field.sides[0].strain.len();
        if field.sides.iter().any(|side| {
            side.strain.len() != expected_elements_per_side
                || side.dwell.len() != expected_elements_per_side
        }) {
            warn!(
                "client: rejected grip checkpoint for {tank}: target exact field slab is malformed"
            );
            continue;
        }
        match assembler.push(chunk, tank, expected_elements_per_side) {
            Ok(Some(checkpoint)) => {
                let combatant = checkpoint.combatant;
                let epoch = checkpoint.epoch;
                let state_entering_tick = checkpoint.state_entering_tick.0;
                let checkpoint_hash = checkpoint.hash;
                match admit_completed_checkpoint(
                    &mut pending,
                    &mut watch,
                    checkpoint,
                    field_history,
                    now,
                ) {
                    Ok(CheckpointDisposition::NoOp) => {
                        if let Some(trace) = trace.as_deref_mut() {
                            trace.record_grip_checkpoint_apply(
                                now.0,
                                combatant,
                                epoch,
                                state_entering_tick,
                                checkpoint_hash,
                                false,
                                false,
                            );
                        }
                    }
                    Ok(CheckpointDisposition::NeedsRollback) => {}
                    Err(error) => {
                        warn!("client: rejected completed grip checkpoint for {tank}: {error:?}");
                    }
                }
            }
            Ok(None) => {}
            Err(error) => warn!("client: rejected grip checkpoint chunk: {error:?}"),
        }
    }
}

pub(super) fn effect_errors(
    authoritative: &NetTrackGripAnchor,
    predicted: &TrackGripEffect,
    dt: f32,
    inverse_mass: f32,
    world_inverse_inertia: avian3d::math::SymmetricMatrix,
    belt_inertia: f32,
) -> EffectErrors {
    let force_delta = authoritative.traction_force - predicted.traction_force;
    let torque_delta = authoritative.traction_torque - predicted.traction_torque;
    let mut belt_speed = [0.0; 2];
    if belt_inertia > 0.0 {
        for (side, error) in belt_speed.iter_mut().enumerate() {
            *error = dt * (authoritative.belt_reaction[side] - predicted.belt_reaction[side]).abs()
                / belt_inertia;
        }
    }
    EffectErrors {
        linear_velocity: dt * force_delta.length() * inverse_mass,
        angular_velocity: dt * (world_inverse_inertia * torque_delta).length(),
        belt_speed,
    }
}

pub(super) fn historical_anchor_state<'a>(
    effect_history: &'a PredictionHistory<TrackGripEffect>,
    rotation_history: &'a PredictionHistory<Rotation>,
    producing_tick: Tick,
    current_tick: Tick,
) -> Option<(&'a TrackGripEffect, &'a Rotation)> {
    // `HistoryBuffer::get` returns the latest stored value at or before its argument. A replicated
    // anchor can arrive one tick ahead of the client; wait until that producing tick has actually
    // run instead of comparing the anchor with the prior tick under the future tick's label.
    if current_tick - producing_tick < 0 {
        return None;
    }
    Some((
        effect_history.get(producing_tick)?,
        rotation_history.get(producing_tick)?,
    ))
}

fn request_resync(
    tank: Entity,
    combatant: CombatantId,
    epoch: u32,
    now: Tick,
    limiter: &mut GripRequestLimiter,
    senders: &mut Query<&mut MessageSender<GripResyncRequest>, With<Client>>,
) -> bool {
    if !limiter.permit(tank, epoch, now) {
        return false;
    }
    let Ok(mut sender) = senders.single_mut() else {
        return false;
    };
    sender.send::<GripRequestChannel>(GripResyncRequest { combatant, epoch });
    true
}

#[allow(clippy::too_many_arguments)]
pub(super) fn compare_track_grip_anchors(
    timeline: Res<LocalTimeline>,
    fixed_time: Res<Time<Fixed>>,
    gear: Option<Res<TrackGear>>,
    mut tanks: Query<
        (
            Entity,
            &CombatantId,
            Ref<NetTrackGripAnchor>,
            &PredictionHistory<TrackGripEffect>,
            &PredictionHistory<Rotation>,
            &ComputedMass,
            &ComputedAngularInertia,
        ),
        (With<Remote>, With<Controlled>),
    >,
    mut watch: ResMut<AnchorWatch>,
    mut pending: ResMut<PendingGripCorrection>,
    mut limiter: ResMut<GripRequestLimiter>,
    mut senders: Query<&mut MessageSender<GripResyncRequest>, With<Client>>,
    mut trace: Option<ResMut<TraceWriter>>,
) {
    let Some(gear) = gear else { return };
    let now = timeline.tick();
    for (tank, combatant, anchor, effect_history, rotation_history, mass, inertia) in &mut tanks {
        // A server anchor may arrive before this client has run its producing tick. Do not use
        // change detection as the retry latch: `Ref::is_changed` is false by the time the local
        // timeline catches up. A producing tick is spent only after a real comparison.
        if watch.anchor_was_compared(tank, anchor.producing_tick) {
            continue;
        }
        pending.observe_anchor_epoch(tank, anchor.rest_epoch);
        let Some((predicted, rotation)) =
            historical_anchor_state(effect_history, rotation_history, anchor.producing_tick, now)
        else {
            continue;
        };
        watch.mark_anchor_compared(tank, anchor.producing_tick);
        let errors = effect_errors(
            &anchor,
            predicted,
            fixed_time.delta_secs(),
            mass.inverse(),
            inertia.rotated(rotation.0).inverse(),
            gear.belt_inertia(),
        );
        let effect_mismatch = errors.linear_velocity >= EFFECT_LINEAR_VELOCITY_THRESHOLD_MPS
            || errors.angular_velocity >= EFFECT_ANGULAR_VELOCITY_THRESHOLD_RADPS
            || errors
                .belt_speed
                .iter()
                .any(|error| *error >= EFFECT_BELT_SPEED_THRESHOLD_MPS);
        let digest_mismatch = anchor.field_digest != predicted.field_digest;
        let watch_index = watch
            .digests
            .iter()
            .position(|entry| entry.tank == tank)
            .unwrap_or_else(|| {
                watch.digests.push(DigestWatch {
                    tank,
                    epoch: anchor.rest_epoch,
                    mismatches: 0,
                });
                watch.digests.len() - 1
            });
        let digest = &mut watch.digests[watch_index];
        let epoch_changed = digest.epoch != anchor.rest_epoch;
        if epoch_changed || !digest_mismatch {
            digest.epoch = anchor.rest_epoch;
            digest.mismatches = 0;
        }
        if digest_mismatch {
            digest.mismatches = digest.mismatches.saturating_add(1);
        }
        let persistent_digest = digest.mismatches >= DIGEST_PERSIST_ANCHORS;
        let digest_due = digest_mismatch && (epoch_changed || persistent_digest);
        let request_reason = match (effect_mismatch, digest_due) {
            (true, true) => GripRequestReason::EffectAndDigest,
            (true, false) => GripRequestReason::Effect,
            (false, true) => GripRequestReason::Digest,
            (false, false) => GripRequestReason::None,
        };
        let evidence_spent = watch.evidence_spent(tank, anchor.rest_epoch, anchor.field_digest);
        let request_due = !evidence_spent && request_reason != GripRequestReason::None;
        let sample = GripAnchorTrace {
            tick: now.0,
            combatant: *combatant,
            anchor_producing_tick: anchor.producing_tick.0,
            history_tick: anchor.producing_tick.0,
            authority: TrackGripEffect {
                traction_force: anchor.traction_force,
                traction_torque: anchor.traction_torque,
                belt_reaction: anchor.belt_reaction,
                field_digest: anchor.field_digest,
            },
            predicted: *predicted,
            e_v: errors.linear_velocity,
            e_omega: errors.angular_velocity,
            e_belt: errors.belt_speed,
            epoch: anchor.rest_epoch,
        };
        watch.remember_trace(tank, sample);
        let request_sent = if request_due {
            // A digest is only request evidence. The exact checkpoint arrival—not this branch—is
            // what can request forced rollback.
            request_resync(
                tank,
                *combatant,
                anchor.rest_epoch,
                now,
                &mut limiter,
                &mut senders,
            )
        } else {
            false
        };
        if let Some(trace) = trace.as_deref_mut() {
            trace.record_grip_anchor_compare(
                sample,
                request_reason,
                evidence_spent,
                request_due,
                request_sent,
            );
            if request_sent {
                trace.record_grip_resync_request(sample, request_reason);
            }
        }
    }
}

pub(super) fn checkpoint_rollback_baseline(state_entering_tick: Tick) -> Tick {
    Tick(state_entering_tick.0.saturating_sub(1))
}

pub(super) fn claim_checkpoint_rollback(
    state_metadata: &mut StateRollbackMetadata,
    baseline: Tick,
) -> bool {
    match state_metadata.forced_rollback_tick() {
        Some(selected) => selected == baseline,
        None => {
            state_metadata.request_forced_rollback(baseline);
            state_metadata.forced_rollback_tick() == Some(baseline)
        }
    }
}

pub(super) fn request_checkpoint_rollback(
    timeline: Res<LocalTimeline>,
    checkpoints: Option<Res<ReplicationCheckpointMap>>,
    managers: Query<&PredictionManager>,
    histories: Query<&PredictionHistory<TrackGripElements>>,
    mut state_metadata: Option<ResMut<StateRollbackMetadata>>,
    mut pending: ResMut<PendingGripCorrection>,
    mut watch: ResMut<AnchorWatch>,
    mut limiter: ResMut<GripRequestLimiter>,
    mut senders: Query<&mut MessageSender<GripResyncRequest>, With<Client>>,
    mut trace: Option<ResMut<TraceWriter>>,
) {
    let Some((checkpoint_tank, checkpoint_combatant, checkpoint_epoch, state_entering_tick)) =
        pending.checkpoint.as_ref().map(|checkpoint| {
            (
                checkpoint.tank,
                checkpoint.combatant,
                checkpoint.epoch,
                checkpoint.state_entering_tick,
            )
        })
    else {
        return;
    };
    let (Some(checkpoints), Ok(manager), Some(state_metadata)) =
        (checkpoints, managers.single(), state_metadata.as_mut())
    else {
        return;
    };
    let target = checkpoint_rollback_baseline(state_entering_tick);
    if pending.rollback_requested {
        if state_metadata.forced_rollback_tick() == Some(target)
            || manager.get_rollback_start_tick() == Some(target)
        {
            return;
        }
        // The request was consumed without installing this checkpoint. Retry instead of wedging the
        // staged correction behind a stale bookkeeping flag.
        pending.rollback_requested = false;
    }
    if checkpoints
        .last_confirmed_tick()
        .is_none_or(|confirmed| confirmed - target < 0)
    {
        return;
    }
    let now = timeline.tick();
    let history = histories.get(checkpoint_tank).ok();
    let retained = history.and_then(|history| history.get(target)).is_some();
    let age = now - target;
    let stale = age < 0 || age > i32::from(manager.rollback_policy.max_rollback_ticks) || !retained;
    if stale {
        if request_resync(
            checkpoint_tank,
            checkpoint_combatant,
            checkpoint_epoch,
            now,
            &mut limiter,
            &mut senders,
        ) {
            if let Some(trace) = trace.as_deref_mut() {
                if let Some(mut sample) = watch.trace_for(checkpoint_tank) {
                    sample.tick = now.0;
                    trace.record_grip_resync_request(sample, GripRequestReason::StaleCheckpoint);
                } else {
                    trace.record_grip_resync_without_anchor(
                        checkpoint_combatant,
                        now.0,
                        checkpoint_epoch,
                        GripRequestReason::StaleCheckpoint,
                    );
                }
            }
            pending.checkpoint = None;
        }
        return;
    }
    if let Some(checkpoint) =
        history.and_then(|history| complete_staged_no_op(&mut pending, &mut watch, history, now))
    {
        if let Some(trace) = trace.as_deref_mut() {
            trace.record_grip_checkpoint_apply(
                now.0,
                checkpoint.combatant,
                checkpoint.epoch,
                checkpoint.state_entering_tick.0,
                checkpoint.hash,
                false,
                false,
            );
        }
        return;
    }
    // A different subsystem already owns this frame's forced rollback. Let it finish, then retry
    // this checkpoint; claiming success here would wedge installation on the other selected tick.
    pending.rollback_requested = claim_checkpoint_rollback(state_metadata, target);
}

pub(super) fn install_checkpoint_after_history_restore(
    timeline: Res<LocalTimeline>,
    managers: Query<&PredictionManager>,
    mut fields: Query<(
        &mut TrackGripElements,
        &mut PredictionHistory<TrackGripElements>,
    )>,
    mut pending: ResMut<PendingGripCorrection>,
    mut trace: Option<ResMut<TraceWriter>>,
) {
    let Some(checkpoint) = pending.checkpoint.as_ref() else {
        return;
    };
    let Ok(manager) = managers.single() else {
        return;
    };
    let baseline = checkpoint_rollback_baseline(checkpoint.state_entering_tick);
    if manager.get_rollback_start_tick() != Some(baseline) {
        return;
    }
    let Ok((mut field, mut history)) = fields.get_mut(checkpoint.tank) else {
        return;
    };
    let field_bits_changed = !fields_bit_equal(&field, &checkpoint.field);
    if let Some(trace) = trace.as_deref_mut() {
        trace.record_grip_checkpoint_apply(
            timeline.tick().0,
            checkpoint.combatant,
            checkpoint.epoch,
            checkpoint.state_entering_tick.0,
            checkpoint.hash,
            field_bits_changed,
            true,
        );
    }
    *field = checkpoint.field.clone();
    // `Prepare` truncated the abandoned future and restored the local end-of-baseline value. Replace
    // that history anchor too, so a later ordinary rollback cannot resurrect the field this exact
    // checkpoint just corrected. Lightyear begins FixedUpdate at the following entering tick.
    history.add_predicted(baseline, Some(checkpoint.field.clone()));
    let key = CheckpointKey::from(checkpoint);
    pending.mark_applied(key);
    pending.checkpoint = None;
    pending.rollback_requested = false;
}
