//! Per-element track-grip convergence: cheap per-tick effect anchors plus rare exact checkpoints.

use avian3d::prelude::{ComputedAngularInertia, ComputedMass, Rotation};
use bevy::prelude::*;
use lightyear::connection::client_of::ClientOf;
use lightyear::prelude::client::Remote;
use lightyear::prelude::*;

use super::protocol::{
    GripCheckpointChannel, GripCheckpointChunk, GripCheckpointEntry, GripRequestChannel,
    GripResyncRequest, NetTrackGripAnchor, ROLLBACK_TRACK_DRIVE, ROLLBACK_VELOCITY,
};
use crate::command::TankCommand;
use crate::track::drive::DriveAxes;
use crate::track::forces::{GRIP_SHEAR_MODULUS_M, GripElements};
use crate::track::sim::{TrackDrive, TrackGear, TrackGripEffect, TrackGripElements, TrackGripWake};

/// DERIVED from the existing 1,100-byte application-payload ceiling: 48 worst-case exact
/// world-`Vec3` entries plus checkpoint metadata encode below it (guarded by a unit test).
pub(crate) const CHECKPOINT_ENTRIES_PER_CHUNK: usize = 48;
/// DERIVED from the current fixed field maximum: 16 chunks × 48 entries covers 768 sparse entries,
/// above the Tiger's 582 elements, while bounding hostile/incompatible assembly allocation.
const MAX_CHECKPOINT_CHUNKS: usize = 16;
/// DERIVED defensive cap: one predicted owner normally needs one assembly, while 16 permits several
/// overlapping epoch/resync responses without allowing an unbounded reliable-message ledger.
const MAX_CHECKPOINT_LEDGERS: usize = 16;
/// DERIVED defensive cap: one owner and two epochs need only a handful of stamps; 32 retains a full
/// 8-second window at the 250 ms request interval before evicting oldest dedupe history.
const MAX_REQUEST_STAMPS: usize = 32;
/// DERIVED defensive budget: one owner normally requests one tank once; two responses let a
/// reconnecting connection repair an overlapping despawn/spawn without permitting unbounded
/// full-field serialization in one render frame.
const MAX_RESYNC_RESPONSES_PER_CONNECTION_PER_FRAME: usize = 2;
/// DERIVED as 125 ms at 64 Hz, matching the element law's contact-loss dwell: a one-tick topology
/// flutter cannot declare a field non-healing.
const REST_STABLE_TICKS: u8 = 8;
/// DERIVED initial Batch-C repair cadence: 256 ticks / 64 Hz = 4 s. Batch D's injected-divergence
/// measurements decide whether this fallback can be removed.
const PERIODIC_CHECKPOINT_TICKS: u32 = 256;
/// DERIVED as 250 ms at 64 Hz, twice the topology/loss dwell. This bounds reliable request pressure
/// per `(tank, epoch)` while still allowing two fresh responses inside Lightyear's rollback window.
const RESYNC_MIN_INTERVAL_TICKS: u32 = 16;
/// DERIVED from the same 125 ms topology dwell: a moving digest gets one complete material-contact
/// grace interval to self-heal before requesting bytes.
const DIGEST_PERSIST_ANCHORS: u8 = 8;
/// DERIVED by reusing the existing gross linear-velocity correction policy after converting force
/// error to one-tick `delta-v`: 1.0 m/s.
const EFFECT_LINEAR_VELOCITY_THRESHOLD_MPS: f32 = ROLLBACK_VELOCITY;
/// DERIVED by reusing the existing gross angular-velocity correction policy after converting torque
/// error through world inverse inertia for one tick: 1.0 rad/s.
const EFFECT_ANGULAR_VELOCITY_THRESHOLD_RADPS: f32 = ROLLBACK_VELOCITY;
/// DERIVED by reusing the existing `TrackDrive` belt-speed correction policy after converting belt
/// reaction through reflected belt inertia for one tick: 0.25 m/s.
const EFFECT_BELT_SPEED_THRESHOLD_MPS: f32 = ROLLBACK_TRACK_DRIVE;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SideRestState {
    field_hash: u64,
    occupancy_hash: u64,
    phase_bits: u64,
    stable_ticks: u8,
    resting: bool,
}

/// Authority-only rest/non-turnover detector. It is netcode bookkeeping, not rollback simulation
/// state; local physics never reads or gates on it.
#[derive(Component, Clone, Copy, Debug, Default)]
pub(super) struct GripRestState {
    initialized: bool,
    epoch: u32,
    wake_generation: u32,
    sides: [SideRestState; 2],
    last_checkpoint_tick: Tick,
}

#[derive(Clone, Debug, PartialEq)]
struct ExactCheckpoint {
    tank: Entity,
    epoch: u32,
    state_entering_tick: Tick,
    field: TrackGripElements,
    hash: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CheckpointKey {
    tank: Entity,
    epoch: u32,
    tick: Tick,
    hash: u64,
}

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

impl From<&ExactCheckpoint> for CheckpointKey {
    fn from(checkpoint: &ExactCheckpoint) -> Self {
        Self {
            tank: checkpoint.tank,
            epoch: checkpoint.epoch,
            tick: checkpoint.state_entering_tick,
            hash: checkpoint.hash,
        }
    }
}

#[derive(Clone, Debug)]
struct PartialCheckpoint {
    key: CheckpointKey,
    elements_per_side: u16,
    chunks: Vec<Option<Vec<GripCheckpointEntry>>>,
}

#[derive(Resource, Default)]
struct CheckpointAssembler {
    partials: Vec<PartialCheckpoint>,
    completed: Vec<CheckpointKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssemblyError {
    BadChunkShape,
    FieldShapeMismatch,
    ConflictingChunk,
    InvalidEntries,
    InvalidStrain,
    HashMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdmissionError {
    Duplicate,
    NoRollbackBaseline,
    NotNewerThanWatermark,
    OlderThanAnchorEpoch,
}

#[derive(Resource, Default)]
struct PendingGripCorrection {
    checkpoint: Option<ExactCheckpoint>,
    rollback_requested: bool,
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
    fn observe_anchor_epoch(&mut self, tank: Entity, epoch: u32) {
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

    fn stage(&mut self, checkpoint: ExactCheckpoint) -> Result<(), AdmissionError> {
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

    fn mark_applied(&mut self, key: CheckpointKey) {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RequestStamp {
    tank: Entity,
    epoch: u32,
    tick: Tick,
}

#[derive(Resource, Default)]
struct GripRequestLimiter {
    stamps: Vec<RequestStamp>,
}

impl GripRequestLimiter {
    fn permit(&mut self, tank: Entity, epoch: u32, now: Tick) -> bool {
        if let Some(stamp) = self
            .stamps
            .iter_mut()
            .find(|stamp| stamp.tank == tank && stamp.epoch == epoch)
        {
            let elapsed = now.0.wrapping_sub(stamp.tick.0);
            if elapsed < RESYNC_MIN_INTERVAL_TICKS {
                return false;
            }
            *stamp = RequestStamp {
                tank,
                epoch,
                tick: now,
            };
            return true;
        }
        self.stamps.push(RequestStamp {
            tank,
            epoch,
            tick: now,
        });
        if self.stamps.len() > MAX_REQUEST_STAMPS {
            self.stamps.remove(0);
        }
        true
    }
}

fn permit_server_resync(
    limiter: &mut GripRequestLimiter,
    responses_this_frame: &mut usize,
    tank: Entity,
    _requested_epoch: u32,
    authoritative_epoch: u32,
    now: Tick,
) -> bool {
    if *responses_this_frame >= MAX_RESYNC_RESPONSES_PER_CONNECTION_PER_FRAME
        || !limiter.permit(tank, authoritative_epoch, now)
    {
        return false;
    }
    *responses_this_frame += 1;
    true
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
struct AnchorWatch {
    digests: Vec<DigestWatch>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct EffectErrors {
    linear_velocity: f32,
    angular_velocity: f32,
    belt_speed: [f32; 2],
}

pub(super) fn install_client(app: &mut App) {
    app.init_resource::<CheckpointAssembler>()
        .init_resource::<PendingGripCorrection>()
        .init_resource::<GripRequestLimiter>()
        .init_resource::<AnchorWatch>()
        .add_observer(reset_client_grip_state)
        // Receivers are cleared in `Last`, including on frames with no fixed tick. Drain them on
        // the render schedule and stage fixed-clock work in resources.
        .add_systems(
            Update,
            (compare_track_grip_anchors, receive_checkpoint_chunks).chain(),
        )
        // A forced request must exist before Lightyear's rollback check consumes it.
        .add_systems(
            PreUpdate,
            request_checkpoint_rollback
                .after(ReplicationSystems::Receive)
                .before(super::watchdog::RollbackWatchdog)
                .before(RollbackSystems::Check),
        )
        // Lightyear has restored all ordinary histories at this seam; replace only the promoted
        // element field before its first replayed DrivingForces pass.
        .add_systems(
            PreUpdate,
            install_checkpoint_after_history_restore
                .after(RollbackSystems::Prepare)
                .before(RollbackSystems::Rollback),
        );
}

fn reset_client_grip_state(
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

pub(super) fn install_server(app: &mut App) {
    app
        // Receivers are cleared in `Last`, so owner requests are drained at render rate.
        .add_systems(Update, answer_resync_requests)
        // `apply_track_forces` completed in FixedUpdate. This captures the end-of-tick field and
        // labels checkpoints as entering the next tick.
        .add_systems(FixedPostUpdate, publish_grip_anchor_and_rest_checkpoints);
}

fn hash_write(hash: &mut u64, bytes: impl IntoIterator<Item = u8>) {
    for byte in bytes {
        *hash ^= u64::from(byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn exact_side_hash(side: &GripElements) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    for (element, (&strain, &dwell)) in side.strain.iter().zip(&side.dwell).enumerate() {
        hash_write(&mut hash, (element as u16).to_le_bytes());
        for axis in strain.to_array() {
            hash_write(&mut hash, axis.to_bits().to_le_bytes());
        }
        hash_write(&mut hash, [dwell]);
    }
    hash
}

fn occupancy_hash(side: &GripElements) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    for (element, &dwell) in side.dwell.iter().enumerate() {
        if dwell != 0 {
            hash_write(&mut hash, (element as u16).to_le_bytes());
        }
    }
    hash
}

fn checkpoint_entries(field: &TrackGripElements) -> Option<(u16, Vec<GripCheckpointEntry>)> {
    let count = field.sides[0].strain.len();
    if count == 0
        || count > usize::from(u16::MAX)
        || field.sides.iter().any(|side| {
            side.strain.len() != count || side.dwell.len() != count || !count.is_multiple_of(3)
        })
    {
        return None;
    }
    let mut entries = Vec::new();
    for (side_index, side) in field.sides.iter().enumerate() {
        for (element, (&strain, &dwell)) in side.strain.iter().zip(&side.dwell).enumerate() {
            if dwell != 0 || strain.to_array().iter().any(|axis| axis.to_bits() != 0) {
                entries.push(GripCheckpointEntry {
                    side: side_index as u8,
                    element: element as u16,
                    strain,
                    contact_generation: dwell,
                });
            }
        }
    }
    Some((count as u16, entries))
}

fn checkpoint_hash(
    epoch: u32,
    state_entering_tick: Tick,
    elements_per_side: u16,
    entries: &[GripCheckpointEntry],
) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    hash_write(&mut hash, epoch.to_le_bytes());
    hash_write(&mut hash, state_entering_tick.0.to_le_bytes());
    hash_write(&mut hash, elements_per_side.to_le_bytes());
    for entry in entries {
        hash_write(&mut hash, [entry.side]);
        hash_write(&mut hash, entry.element.to_le_bytes());
        for axis in entry.strain.to_array() {
            hash_write(&mut hash, axis.to_bits().to_le_bytes());
        }
        hash_write(&mut hash, [entry.contact_generation]);
    }
    hash
}

fn make_checkpoint_chunks(
    tank: Entity,
    epoch: u32,
    state_entering_tick: Tick,
    field: &TrackGripElements,
) -> Option<Vec<GripCheckpointChunk>> {
    let (elements_per_side, entries) = checkpoint_entries(field)?;
    let hash = checkpoint_hash(epoch, state_entering_tick, elements_per_side, &entries);
    let chunk_count = entries.len().max(1).div_ceil(CHECKPOINT_ENTRIES_PER_CHUNK);
    if chunk_count > MAX_CHECKPOINT_CHUNKS || chunk_count > usize::from(u8::MAX) {
        return None;
    }
    let mut chunks = Vec::with_capacity(chunk_count);
    for chunk_index in 0..chunk_count {
        let start = chunk_index * CHECKPOINT_ENTRIES_PER_CHUNK;
        let end = (start + CHECKPOINT_ENTRIES_PER_CHUNK).min(entries.len());
        chunks.push(GripCheckpointChunk {
            tank,
            epoch,
            state_entering_tick,
            elements_per_side,
            chunk_index: chunk_index as u8,
            chunk_count: chunk_count as u8,
            entries: entries.get(start..end).unwrap_or_default().to_vec(),
            checkpoint_hash: hash,
        });
    }
    Some(chunks)
}

fn entries_bit_equal(a: &[GripCheckpointEntry], b: &[GripCheckpointEntry]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.side == b.side
                && a.element == b.element
                && a.contact_generation == b.contact_generation
                && a.strain
                    .to_array()
                    .iter()
                    .zip(b.strain.to_array())
                    .all(|(a, b)| a.to_bits() == b.to_bits())
        })
}

impl CheckpointAssembler {
    fn push(
        &mut self,
        chunk: GripCheckpointChunk,
        expected_elements_per_side: usize,
    ) -> Result<Option<ExactCheckpoint>, AssemblyError> {
        let count = usize::from(chunk.chunk_count);
        let index = usize::from(chunk.chunk_index);
        if count == 0
            || count > MAX_CHECKPOINT_CHUNKS
            || index >= count
            || chunk.entries.len() > CHECKPOINT_ENTRIES_PER_CHUNK
            || chunk.elements_per_side == 0
            || !chunk.elements_per_side.is_multiple_of(3)
        {
            return Err(AssemblyError::BadChunkShape);
        }
        if usize::from(chunk.elements_per_side) != expected_elements_per_side {
            return Err(AssemblyError::FieldShapeMismatch);
        }
        for entry in &chunk.entries {
            if entry.side >= 2 || usize::from(entry.element) >= expected_elements_per_side {
                return Err(AssemblyError::InvalidEntries);
            }
            let axes = entry.strain.to_array();
            if axes
                .iter()
                .any(|axis| !axis.is_finite() || axis.abs() > GRIP_SHEAR_MODULUS_M)
                || entry.strain.length_squared() > GRIP_SHEAR_MODULUS_M * GRIP_SHEAR_MODULUS_M
            {
                return Err(AssemblyError::InvalidStrain);
            }
        }
        let key = CheckpointKey {
            tank: chunk.tank,
            epoch: chunk.epoch,
            tick: chunk.state_entering_tick,
            hash: chunk.checkpoint_hash,
        };
        if self.completed.contains(&key) {
            return Ok(None);
        }
        let partial_index = self
            .partials
            .iter()
            .position(|partial| partial.key == key)
            .unwrap_or_else(|| {
                if self.partials.len() >= MAX_CHECKPOINT_LEDGERS {
                    self.partials.remove(0);
                }
                self.partials.push(PartialCheckpoint {
                    key,
                    elements_per_side: chunk.elements_per_side,
                    chunks: vec![None; count],
                });
                self.partials.len() - 1
            });
        let partial = &mut self.partials[partial_index];
        if partial.elements_per_side != chunk.elements_per_side || partial.chunks.len() != count {
            self.partials.remove(partial_index);
            return Err(AssemblyError::ConflictingChunk);
        }
        if let Some(existing) = &partial.chunks[index] {
            if entries_bit_equal(existing, &chunk.entries) {
                return Ok(None);
            }
            self.partials.remove(partial_index);
            return Err(AssemblyError::ConflictingChunk);
        }
        partial.chunks[index] = Some(chunk.entries);
        if partial.chunks.iter().any(Option::is_none) {
            return Ok(None);
        }

        let partial = self.partials.remove(partial_index);
        let entries: Vec<_> = partial.chunks.into_iter().flatten().flatten().collect();
        let mut previous = None;
        let limit = usize::from(partial.elements_per_side);
        for entry in &entries {
            let id = (entry.side, entry.element);
            if entry.side >= 2
                || usize::from(entry.element) >= limit
                || previous.is_some_and(|previous| previous >= id)
            {
                return Err(AssemblyError::InvalidEntries);
            }
            previous = Some(id);
        }
        let actual_hash = checkpoint_hash(
            partial.key.epoch,
            partial.key.tick,
            partial.elements_per_side,
            &entries,
        );
        if actual_hash != partial.key.hash {
            return Err(AssemblyError::HashMismatch);
        }
        let link_count = limit / 3;
        let mut field = TrackGripElements::for_links(link_count);
        for entry in entries {
            let side = usize::from(entry.side);
            let element = usize::from(entry.element);
            field.sides[side].strain[element] = entry.strain;
            field.sides[side].dwell[element] = entry.contact_generation;
        }
        self.completed.push(partial.key);
        if self.completed.len() > MAX_CHECKPOINT_LEDGERS {
            self.completed.remove(0);
        }
        Ok(Some(ExactCheckpoint {
            tank: partial.key.tank,
            epoch: partial.key.epoch,
            state_entering_tick: partial.key.tick,
            field,
            hash: partial.key.hash,
        }))
    }
}

fn send_checkpoint(
    checkpoint: &[GripCheckpointChunk],
    owner: Entity,
    sender: &mut ServerMultiMessageSender,
) {
    for chunk in checkpoint {
        if let Err(error) =
            sender.send_to_entities::<GripCheckpointChunk, GripCheckpointChannel>(chunk, [owner])
        {
            error!("server: grip checkpoint chunk could not enter transport: {error}");
        }
    }
}

fn update_side_rest(
    side: &mut SideRestState,
    field_hash: u64,
    occupancy_hash: u64,
    phase_bits: u64,
    commanded: bool,
) -> (bool, bool) {
    let unchanged = !commanded
        && side.field_hash == field_hash
        && side.occupancy_hash == occupancy_hash
        && side.phase_bits == phase_bits;
    let woke = side.resting && !unchanged;
    let mut entered = false;
    if woke {
        side.resting = false;
        side.stable_ticks = 0;
    } else if !side.resting {
        if unchanged {
            side.stable_ticks = side.stable_ticks.saturating_add(1);
            if side.stable_ticks >= REST_STABLE_TICKS {
                side.resting = true;
                entered = true;
            }
        } else {
            side.stable_ticks = 0;
        }
    }
    side.field_hash = field_hash;
    side.occupancy_hash = occupancy_hash;
    side.phase_bits = phase_bits;
    (entered, woke)
}

fn advance_rest_epoch(
    rest: &mut GripRestState,
    observations: [(u64, u64, u64, bool); 2],
) -> (bool, bool) {
    let mut entered_rest = false;
    let mut woke = false;
    for (side, (field_hash, occupancy_hash, phase_bits, commanded)) in
        rest.sides.iter_mut().zip(observations)
    {
        let (entered, side_woke) =
            update_side_rest(side, field_hash, occupancy_hash, phase_bits, commanded);
        entered_rest |= entered;
        woke |= side_woke;
    }
    if entered_rest || woke {
        rest.epoch = rest.epoch.wrapping_add(1);
    }
    (entered_rest, woke)
}

fn entering_next_tick(now: Tick) -> Tick {
    Tick(now.0.saturating_add(1))
}

fn checkpoint_rollback_baseline(state_entering_tick: Tick) -> Tick {
    Tick(state_entering_tick.0.saturating_sub(1))
}

fn claim_checkpoint_rollback(state_metadata: &mut StateRollbackMetadata, baseline: Tick) -> bool {
    match state_metadata.forced_rollback_tick() {
        Some(selected) => selected == baseline,
        None => {
            state_metadata.request_forced_rollback(baseline);
            state_metadata.forced_rollback_tick() == Some(baseline)
        }
    }
}

fn consume_impulse_wake(rest: &mut GripRestState, wake: &TrackGripWake) -> bool {
    let generation = wake.generation();
    let changed = rest.wake_generation != generation;
    rest.wake_generation = generation;
    changed
}

#[allow(clippy::too_many_arguments)]
fn publish_grip_anchor_and_rest_checkpoints(
    timeline: Res<LocalTimeline>,
    servers: Query<&Server>,
    mut sender: ServerMultiMessageSender,
    mut tanks: Query<
        (
            Entity,
            &TrackGripElements,
            &TrackGripEffect,
            &TrackDrive,
            &TankCommand,
            &TrackGripWake,
            Option<&ControlledBy>,
            &mut NetTrackGripAnchor,
            &mut GripRestState,
        ),
        Without<Remote>,
    >,
) {
    let now = timeline.tick();
    for (tank, field, effect, drive, command, wake, controlled, mut anchor, mut rest) in &mut tanks
    {
        if !rest.initialized {
            for side_index in 0..2 {
                rest.sides[side_index] = SideRestState {
                    field_hash: exact_side_hash(&field.sides[side_index]),
                    occupancy_hash: occupancy_hash(&field.sides[side_index]),
                    phase_bits: drive.sides[side_index].phase.to_bits(),
                    ..default()
                };
            }
            rest.initialized = true;
            rest.wake_generation = wake.generation();
            rest.last_checkpoint_tick = now;
        }

        let impulsed = consume_impulse_wake(&mut rest, wake);
        let side_commands = DriveAxes {
            throttle: command.throttle,
            steer: command.steer,
        }
        .side_commands();
        let observations = core::array::from_fn(|side_index| {
            (
                exact_side_hash(&field.sides[side_index]),
                occupancy_hash(&field.sides[side_index]),
                drive.sides[side_index].phase.to_bits(),
                impulsed || side_commands[side_index].to_bits() & 0x7fff_ffff != 0,
            )
        });
        let (entered_rest, woke) = advance_rest_epoch(&mut rest, observations);
        if woke {
            // Wake never publishes a checkpoint. Restart the optional moving repair cadence so a
            // long parked interval cannot make wake immediately satisfy the four-second timer.
            rest.last_checkpoint_tick = now;
        }

        anchor.set_if_neq(NetTrackGripAnchor {
            producing_tick: now,
            rest_epoch: rest.epoch,
            traction_force: effect.traction_force,
            traction_torque: effect.traction_torque,
            belt_reaction: effect.belt_reaction,
            field_digest: effect.field_digest,
        });

        let periodic = rest.sides.iter().any(|side| !side.resting)
            && now.0.wrapping_sub(rest.last_checkpoint_tick.0) >= PERIODIC_CHECKPOINT_TICKS;
        if (entered_rest || periodic)
            && let (Some(controlled), Ok(_server)) = (controlled, servers.single())
            && let Some(chunks) =
                make_checkpoint_chunks(tank, rest.epoch, entering_next_tick(now), field)
        {
            send_checkpoint(&chunks, controlled.owner, &mut sender);
            rest.last_checkpoint_tick = now;
        }
    }
}

fn receive_checkpoint_chunks(
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

fn effect_errors(
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

fn historical_anchor_state<'a>(
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
fn compare_track_grip_anchors(
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

fn answer_resync_requests(
    timeline: Res<LocalTimeline>,
    mut receivers: Query<(Entity, &mut MessageReceiver<GripResyncRequest>), With<ClientOf>>,
    tanks: Query<(&TrackGripElements, &GripRestState, &ControlledBy)>,
    mut limiter: Local<GripRequestLimiter>,
    mut sender: ServerMultiMessageSender,
) {
    let now = timeline.tick();
    for (requester, mut receiver) in &mut receivers {
        let mut responses = 0;
        for request in receiver.receive() {
            let Ok((field, rest, controlled)) = tanks.get(request.tank) else {
                continue;
            };
            if controlled.owner != requester
                || !permit_server_resync(
                    &mut limiter,
                    &mut responses,
                    request.tank,
                    request.epoch,
                    rest.epoch,
                    now,
                )
            {
                continue;
            }
            let Some(chunks) =
                make_checkpoint_chunks(request.tank, rest.epoch, entering_next_tick(now), field)
            else {
                continue;
            };
            send_checkpoint(&chunks, requester, &mut sender);
        }
    }
}

fn request_checkpoint_rollback(
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

fn install_checkpoint_after_history_restore(
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

#[cfg(test)]
mod tests {
    use super::*;

    fn populated_field() -> TrackGripElements {
        let mut field = TrackGripElements::for_links(20);
        for side in 0..2 {
            for element in 0..field.sides[side].strain.len() {
                if element % 2 == 0 {
                    field.sides[side].strain[element] = Vec3::new(
                        f32::from_bits(0x3a80_0000 + element as u32),
                        -(element as f32) * 0.0001,
                        0.0,
                    );
                    field.sides[side].dwell[element] = 8;
                }
            }
        }
        field
    }

    fn exact_checkpoint(tank: Entity, epoch: u32, tick: u32, hash: u64) -> ExactCheckpoint {
        ExactCheckpoint {
            tank,
            epoch,
            state_entering_tick: Tick(tick),
            field: TrackGripElements::for_links(1),
            hash,
        }
    }

    #[test]
    fn chunk_assembly_is_atomic_and_idempotent() {
        let tank = Entity::from_raw_u32(7).unwrap();
        let field = populated_field();
        let chunks = make_checkpoint_chunks(tank, 4, Tick(90), &field).unwrap();
        let expected_elements_per_side = field.sides[0].strain.len();
        assert!(chunks.len() > 1);
        let mut assembler = CheckpointAssembler::default();
        assert!(
            assembler
                .push(chunks[1].clone(), expected_elements_per_side)
                .unwrap()
                .is_none()
        );
        assert!(
            assembler
                .push(chunks[1].clone(), expected_elements_per_side)
                .unwrap()
                .is_none()
        );
        let mut completed = None;
        for chunk in chunks.iter().skip(2).chain(chunks.iter().take(1)) {
            completed = assembler
                .push(chunk.clone(), expected_elements_per_side)
                .unwrap()
                .or(completed);
        }
        let checkpoint = completed.expect("only the final missing chunk publishes atomically");
        assert_eq!(checkpoint.field, field);
        for chunk in chunks {
            assert!(
                assembler
                    .push(chunk, expected_elements_per_side)
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn chunk_assembly_rejects_whole_hash_corruption() {
        let tank = Entity::from_raw_u32(8).unwrap();
        let field = populated_field();
        let mut chunks = make_checkpoint_chunks(tank, 5, Tick(91), &field).unwrap();
        chunks[0].entries[0].strain.x = f32::from_bits(chunks[0].entries[0].strain.x.to_bits() ^ 1);
        let mut assembler = CheckpointAssembler::default();
        let mut result = Ok(None);
        for chunk in chunks {
            result = assembler.push(chunk, field.sides[0].strain.len());
        }
        assert_eq!(result, Err(AssemblyError::HashMismatch));
    }

    #[test]
    fn chunk_assembly_rejects_unbounded_shape_before_field_allocation() {
        let epoch = 6;
        let tick = Tick(92);
        let elements_per_side = u16::MAX;
        let chunk = GripCheckpointChunk {
            tank: Entity::from_raw_u32(10).unwrap(),
            epoch,
            state_entering_tick: tick,
            elements_per_side,
            chunk_index: 0,
            chunk_count: 1,
            entries: Vec::new(),
            checkpoint_hash: checkpoint_hash(epoch, tick, elements_per_side, &[]),
        };

        assert_eq!(
            CheckpointAssembler::default().push(chunk, 3),
            Err(AssemblyError::FieldShapeMismatch)
        );
    }

    #[test]
    fn chunk_assembly_rejects_non_finite_strain() {
        let epoch = 7;
        let tick = Tick(93);
        let elements_per_side = 3;
        let entries = vec![GripCheckpointEntry {
            side: 0,
            element: 0,
            strain: Vec3::new(f32::from_bits(0x7fc0_1234), 0.0, 0.0),
            contact_generation: 1,
        }];
        let chunk = GripCheckpointChunk {
            tank: Entity::from_raw_u32(11).unwrap(),
            epoch,
            state_entering_tick: tick,
            elements_per_side,
            chunk_index: 0,
            chunk_count: 1,
            checkpoint_hash: checkpoint_hash(epoch, tick, elements_per_side, &entries),
            entries,
        };

        assert_eq!(
            CheckpointAssembler::default().push(chunk, 3),
            Err(AssemblyError::InvalidStrain)
        );
    }

    #[test]
    fn chunk_assembly_rejects_strain_outside_force_law_range() {
        let epoch = 8;
        let tick = Tick(94);
        let elements_per_side = 3;
        let entries = vec![GripCheckpointEntry {
            side: 0,
            element: 0,
            strain: Vec3::new(f32::from_bits(GRIP_SHEAR_MODULUS_M.to_bits() + 1), 0.0, 0.0),
            contact_generation: 1,
        }];
        let chunk = GripCheckpointChunk {
            tank: Entity::from_raw_u32(19).unwrap(),
            epoch,
            state_entering_tick: tick,
            elements_per_side,
            chunk_index: 0,
            chunk_count: 1,
            checkpoint_hash: checkpoint_hash(epoch, tick, elements_per_side, &entries),
            entries,
        };

        assert_eq!(
            CheckpointAssembler::default().push(chunk, 3),
            Err(AssemblyError::InvalidStrain)
        );
    }

    #[test]
    fn checkpoint_chunk_round_trip_preserves_signed_zero_and_nan_payloads() {
        let chunk = GripCheckpointChunk {
            tank: Entity::from_raw_u32(12).unwrap(),
            epoch: 8,
            state_entering_tick: Tick(94),
            elements_per_side: 3,
            chunk_index: 0,
            chunk_count: 1,
            entries: vec![GripCheckpointEntry {
                side: 1,
                element: 2,
                strain: Vec3::new(
                    f32::from_bits(0x8000_0000),
                    f32::from_bits(0x7fc0_1234),
                    f32::from_bits(0xffc0_5678),
                ),
                contact_generation: 8,
            }],
            checkpoint_hash: 0x1234_5678_9abc_def0,
        };
        let encoded = bincode::serde::encode_to_vec(&chunk, bincode::config::standard()).unwrap();
        let (decoded, consumed) = bincode::serde::decode_from_slice::<GripCheckpointChunk, _>(
            &encoded,
            bincode::config::standard(),
        )
        .unwrap();

        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded.entries.len(), 1);
        for (actual, expected) in decoded.entries[0]
            .strain
            .to_array()
            .iter()
            .zip(chunk.entries[0].strain.to_array())
        {
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
    }

    #[test]
    fn rest_epoch_enters_after_dwell_and_wakes_without_a_checkpoint_transition() {
        let side = SideRestState {
            field_hash: 1,
            occupancy_hash: 2,
            phase_bits: 3,
            ..default()
        };
        let mut rest = GripRestState {
            initialized: true,
            sides: [side; 2],
            ..default()
        };
        let stable = [(1, 2, 3, false); 2];
        for _ in 0..REST_STABLE_TICKS - 1 {
            assert_eq!(advance_rest_epoch(&mut rest, stable), (false, false));
        }
        assert_eq!(advance_rest_epoch(&mut rest, stable), (true, false));
        assert_eq!(rest.epoch, 1, "simultaneous side entry is one epoch");
        assert!(rest.sides.iter().all(|side| side.resting));
        assert_eq!(advance_rest_epoch(&mut rest, stable), (false, false));
        assert_eq!(rest.epoch, 1, "unchanged rest does not heal or bump");

        let wake = [(1, 2, 3, true), (1, 2, 3, false)];
        assert_eq!(advance_rest_epoch(&mut rest, wake), (false, true));
        assert_eq!(rest.epoch, 2, "wake bumps but does not publish rest");
        assert!(!rest.sides[0].resting);
    }

    #[test]
    fn explicit_impulse_generation_wakes_the_rest_epoch_immediately() {
        let side = SideRestState {
            field_hash: 1,
            occupancy_hash: 2,
            phase_bits: 3,
            resting: true,
            ..default()
        };
        let mut rest = GripRestState {
            initialized: true,
            sides: [side; 2],
            ..default()
        };
        let mut wake = TrackGripWake::default();
        assert!(!consume_impulse_wake(&mut rest, &wake));

        wake.record_impulse(Vec3::X);
        let impulsed = consume_impulse_wake(&mut rest, &wake);
        assert!(impulsed);
        assert_eq!(
            advance_rest_epoch(&mut rest, [(1, 2, 3, impulsed); 2]),
            (false, true)
        );
        assert_eq!(rest.epoch, 1);
        assert!(rest.sides.iter().all(|side| !side.resting));
    }

    #[test]
    fn checkpoint_ticks_saturate_at_timeline_boundaries() {
        assert_eq!(entering_next_tick(Tick(100)), Tick(101));
        assert_eq!(entering_next_tick(Tick(u32::MAX)), Tick(u32::MAX));
        assert_eq!(checkpoint_rollback_baseline(Tick(100)), Tick(99));
        assert_eq!(
            checkpoint_rollback_baseline(Tick(u32::MAX)),
            Tick(u32::MAX - 1)
        );
        assert_eq!(checkpoint_rollback_baseline(Tick(0)), Tick(0));
    }

    #[test]
    fn checkpoint_entering_at_tick_zero_has_no_rollback_baseline() {
        let tank = Entity::from_raw_u32(12).unwrap();
        let mut pending = PendingGripCorrection::default();
        assert_eq!(
            pending.stage(exact_checkpoint(tank, 0, 0, 1)),
            Err(AdmissionError::NoRollbackBaseline)
        );
    }

    #[test]
    fn competing_forced_rollback_does_not_claim_the_grip_checkpoint() {
        let mut metadata = StateRollbackMetadata::default();
        metadata.request_forced_rollback(Tick(80));

        assert!(!claim_checkpoint_rollback(&mut metadata, Tick(90)));
        assert_eq!(metadata.forced_rollback_tick(), Some(Tick(80)));
        assert!(claim_checkpoint_rollback(&mut metadata, Tick(80)));

        let mut clear = StateRollbackMetadata::default();
        assert!(claim_checkpoint_rollback(&mut clear, Tick(90)));
        assert_eq!(clear.forced_rollback_tick(), Some(Tick(90)));
    }

    #[test]
    fn checkpoint_watermark_rejects_late_completion_and_old_anchor_epochs() {
        let tank = Entity::from_raw_u32(13).unwrap();
        let mut pending = PendingGripCorrection::default();
        let newer = exact_checkpoint(tank, 4, 100, 2);
        pending.stage(newer.clone()).unwrap();
        pending.mark_applied(CheckpointKey::from(&newer));
        pending.checkpoint = None;

        assert_eq!(
            pending.stage(exact_checkpoint(tank, 4, 99, 1)),
            Err(AdmissionError::NotNewerThanWatermark)
        );
        pending.observe_anchor_epoch(tank, 5);
        assert_eq!(
            pending.stage(exact_checkpoint(tank, 4, 101, 3)),
            Err(AdmissionError::OlderThanAnchorEpoch)
        );
    }

    #[test]
    fn checkpoint_watermark_wraps_epochs_but_orders_ticks_monotonically() {
        let tank = Entity::from_raw_u32(14).unwrap();
        let mut pending = PendingGripCorrection::default();
        pending.observe_anchor_epoch(tank, u32::MAX);
        pending
            .stage(exact_checkpoint(tank, u32::MAX, 100, 1))
            .unwrap();
        pending
            .stage(exact_checkpoint(tank, 0, 101, 2))
            .expect("wrapped epoch is newer than u32::MAX");

        let same_epoch = Entity::from_raw_u32(15).unwrap();
        pending
            .stage(exact_checkpoint(same_epoch, 7, u32::MAX, 3))
            .unwrap();
        assert_eq!(
            pending.stage(exact_checkpoint(same_epoch, 7, 1, 4)),
            Err(AdmissionError::NotNewerThanWatermark)
        );
    }

    #[test]
    fn anchor_metrics_use_the_effect_at_the_producing_tick() {
        let mut effect_history = PredictionHistory::<TrackGripEffect>::default();
        effect_history.add_predicted(
            Tick(10),
            Some(TrackGripEffect {
                traction_force: Vec3::new(64.0, 0.0, 0.0),
                ..default()
            }),
        );
        effect_history.add_predicted(Tick(11), Some(TrackGripEffect::default()));
        let historical_rotation = Rotation(Quat::from_rotation_y(0.75));
        let mut rotation_history = PredictionHistory::<Rotation>::default();
        rotation_history.add_predicted(Tick(10), Some(historical_rotation));
        rotation_history.add_predicted(Tick(11), Some(Rotation::default()));
        let anchor = NetTrackGripAnchor {
            producing_tick: Tick(10),
            traction_force: Vec3::ZERO,
            ..default()
        };
        let (predicted, rotation) =
            historical_anchor_state(&effect_history, &rotation_history, anchor.producing_tick)
                .unwrap();
        assert_eq!(rotation.0.to_array(), historical_rotation.0.to_array());
        let errors = effect_errors(
            &anchor,
            predicted,
            1.0 / 64.0,
            1.0,
            ComputedAngularInertia::INFINITY.inverse(),
            1.0,
        );
        assert_eq!(errors.linear_velocity, 1.0);
    }

    #[test]
    fn resync_limiter_deduplicates_and_rate_limits_per_tank() {
        let tank = Entity::from_raw_u32(9).unwrap();
        let mut limiter = GripRequestLimiter::default();
        assert!(limiter.permit(tank, 2, Tick(100)));
        assert!(!limiter.permit(tank, 2, Tick(100)));
        assert!(limiter.permit(tank, 3, Tick(115)));
        assert!(!limiter.permit(tank, 3, Tick(115)));
        assert!(limiter.permit(tank, 2, Tick(116)));
    }

    #[test]
    fn server_resync_uses_authoritative_epoch_and_connection_frame_budget() {
        let mut limiter = GripRequestLimiter::default();
        let mut responses = 0;
        let tank = Entity::from_raw_u32(16).unwrap();
        assert!(permit_server_resync(
            &mut limiter,
            &mut responses,
            tank,
            1,
            9,
            Tick(100),
        ));
        assert!(
            !permit_server_resync(&mut limiter, &mut responses, tank, 2, 9, Tick(100),),
            "cycling the client-supplied epoch must not bypass authoritative rate accounting"
        );

        let second = Entity::from_raw_u32(17).unwrap();
        let third = Entity::from_raw_u32(18).unwrap();
        assert!(permit_server_resync(
            &mut limiter,
            &mut responses,
            second,
            0,
            3,
            Tick(100),
        ));
        assert!(!permit_server_resync(
            &mut limiter,
            &mut responses,
            third,
            0,
            3,
            Tick(100),
        ));
    }

    #[test]
    fn worst_case_chunk_stays_below_application_payload_ceiling() {
        let entries = (0..CHECKPOINT_ENTRIES_PER_CHUNK)
            .map(|element| GripCheckpointEntry {
                side: 1,
                element: element as u16,
                strain: Vec3::new(
                    f32::from_bits(u32::MAX),
                    f32::from_bits(u32::MAX - 1),
                    f32::from_bits(u32::MAX - 2),
                ),
                contact_generation: u8::MAX,
            })
            .collect();
        let chunk = GripCheckpointChunk {
            // Lightyear maps entities per recipient before bincode. Force the nine-byte `u64`
            // tier, matching the existing shot-transport mapped-entity size tripwire.
            tank: Entity::from_bits(0x8000_0000_0000_0001),
            epoch: u32::MAX,
            state_entering_tick: Tick(u32::MAX),
            elements_per_side: u16::MAX - 2,
            chunk_index: u8::MAX - 1,
            chunk_count: u8::MAX,
            entries,
            checkpoint_hash: u64::MAX,
        };
        let encoded = bincode::serde::encode_to_vec(&chunk, bincode::config::standard()).unwrap();
        println!(
            "worst mapped 48-entry checkpoint chunk plus message type id: {} B",
            encoded.len() + 4
        );
        assert!(
            encoded.len() + 4 < 1_100,
            "checkpoint chunk plus message type id is {} bytes",
            encoded.len() + 4
        );
    }

    #[test]
    fn checkpoint_bandwidth_reference_for_measured_grounded_range() {
        let encoded_checkpoint = |active_per_side: usize| {
            // MEASURED Tiger data: 97 links × 3 columns = 291 elements per side.
            let mut field = TrackGripElements::for_links(97);
            for side in &mut field.sides {
                for element in 0..active_per_side {
                    side.strain[element] = Vec3::new(0.001, -0.002, 0.003);
                    side.dwell[element] = 8;
                }
            }
            make_checkpoint_chunks(
                Entity::from_bits(0x8000_0000_0000_0001),
                u32::MAX,
                Tick(u32::MAX),
                &field,
            )
            .unwrap()
            .iter()
            .map(|chunk| {
                bincode::serde::encode_to_vec(chunk, bincode::config::standard())
                    .unwrap()
                    .len()
                    + 4
            })
            .sum::<usize>()
        };
        let low = encoded_checkpoint(75);
        let high = encoded_checkpoint(90);
        println!(
            "exact world-Vec3 checkpoint: {low} B at 75 active/side; {high} B at 90 active/side"
        );
        assert!(low < high);
    }
}
