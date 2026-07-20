//! Predicted-client anchor comparison, exact-checkpoint admission, and rollback correction.

use avian3d::prelude::{ComputedAngularInertia, ComputedMass, Rotation};
use bevy::prelude::*;
use lightyear::prelude::client::Remote;
use lightyear::prelude::*;

use crate::net::protocol::{
    GripCheckpointChunk, GripRequestChannel, GripResyncRequest, NetTrackGripAnchor,
};
use crate::track::sim::{TrackGear, TrackGripEffect, TrackGripElements};

use super::checkpoint::{CheckpointAssembler, CheckpointKey, ExactCheckpoint};
use super::{
    DIGEST_PERSIST_ANCHORS, EFFECT_ANGULAR_VELOCITY_THRESHOLD_RADPS,
    EFFECT_BELT_SPEED_THRESHOLD_MPS, EFFECT_LINEAR_VELOCITY_THRESHOLD_MPS, GripRequestLimiter,
    MAX_CHECKPOINT_LEDGERS,
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
    mut pending: ResMut<PendingGripCorrection>,
    mut limiter: ResMut<GripRequestLimiter>,
    mut watch: ResMut<AnchorWatch>,
) {
    *assembler = default();
    *pending = default();
    *limiter = default();
    *watch = default();
}

pub(super) fn receive_checkpoint_chunks(
    mut receivers: Query<&mut MessageReceiver<GripCheckpointChunk>, With<Client>>,
    fields: Query<&TrackGripElements, (With<Remote>, With<Controlled>)>,
    mut assembler: ResMut<CheckpointAssembler>,
    mut pending: ResMut<PendingGripCorrection>,
) {
    for mut receiver in &mut receivers {
        for chunk in receiver.receive() {
            let tank = chunk.tank;
            let Ok(field) = fields.get(tank) else {
                warn!(
                    "client: rejected grip checkpoint for {tank}: target exact field is unavailable"
                );
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
            match assembler.push(chunk, expected_elements_per_side) {
                Ok(Some(checkpoint)) => {
                    if let Err(error) = pending.stage(checkpoint) {
                        warn!("client: rejected completed grip checkpoint for {tank}: {error:?}");
                    }
                }
                Ok(None) => {}
                Err(error) => warn!("client: rejected grip checkpoint chunk: {error:?}"),
            }
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
) -> Option<(&'a TrackGripEffect, &'a Rotation)> {
    Some((
        effect_history.get(producing_tick)?,
        rotation_history.get(producing_tick)?,
    ))
}

fn request_resync(
    tank: Entity,
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
    sender.send::<GripRequestChannel>(GripResyncRequest { tank, epoch });
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
) {
    let Some(gear) = gear else { return };
    let now = timeline.tick();
    for (tank, anchor, effect_history, rotation_history, mass, inertia) in &mut tanks {
        if !anchor.is_changed() {
            continue;
        }
        pending.observe_anchor_epoch(tank, anchor.rest_epoch);
        let Some((predicted, rotation)) =
            historical_anchor_state(effect_history, rotation_history, anchor.producing_tick)
        else {
            continue;
        };
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
        if effect_mismatch || (digest_mismatch && (epoch_changed || persistent_digest)) {
            // A digest is only request evidence. The exact checkpoint arrival—not this branch—is
            // what can request forced rollback.
            request_resync(tank, anchor.rest_epoch, now, &mut limiter, &mut senders);
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
    mut limiter: ResMut<GripRequestLimiter>,
    mut senders: Query<&mut MessageSender<GripResyncRequest>, With<Client>>,
) {
    let Some((checkpoint_tank, checkpoint_epoch, state_entering_tick)) =
        pending.checkpoint.as_ref().map(|checkpoint| {
            (
                checkpoint.tank,
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
    let retained = histories
        .get(checkpoint_tank)
        .ok()
        .and_then(|history| history.get(target))
        .is_some();
    let age = now - target;
    let stale = age < 0 || age > i32::from(manager.rollback_policy.max_rollback_ticks) || !retained;
    if stale {
        if request_resync(
            checkpoint_tank,
            checkpoint_epoch,
            now,
            &mut limiter,
            &mut senders,
        ) {
            pending.checkpoint = None;
        }
        return;
    }
    // A different subsystem already owns this frame's forced rollback. Let it finish, then retry
    // this checkpoint; claiming success here would wedge installation on the other selected tick.
    pending.rollback_requested = claim_checkpoint_rollback(state_metadata, target);
}

pub(super) fn install_checkpoint_after_history_restore(
    managers: Query<&PredictionManager>,
    mut fields: Query<(
        &mut TrackGripElements,
        &mut PredictionHistory<TrackGripElements>,
    )>,
    mut pending: ResMut<PendingGripCorrection>,
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
