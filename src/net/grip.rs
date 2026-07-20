//! Per-element track-grip convergence: cheap per-tick effect anchors plus rare exact checkpoints.

use bevy::prelude::*;
use lightyear::prelude::*;

use super::protocol::{ROLLBACK_TRACK_DRIVE, ROLLBACK_VELOCITY};
use crate::track::forces::GRIP_ELEMENT_LOSS_DWELL_TICKS;

mod authority;
mod checkpoint;
mod client;

pub(super) use authority::GripRestState;
use authority::{answer_resync_requests, publish_grip_anchor_and_rest_checkpoints};
use checkpoint::CheckpointAssembler;
use client::{
    AnchorWatch, DeferredCheckpointChunks, PendingGripCorrection, compare_track_grip_anchors,
    install_checkpoint_after_history_restore, receive_checkpoint_chunks,
    request_checkpoint_rollback, reset_client_grip_state,
};

#[cfg(test)]
use super::protocol::{GripCheckpointChunk, GripCheckpointEntry, NetTrackGripAnchor};
#[cfg(test)]
use crate::track::forces::GRIP_SHEAR_MODULUS_M;
#[cfg(test)]
use crate::track::sim::{TrackGripEffect, TrackGripElements, TrackGripWake};
#[cfg(test)]
use authority::{
    SideRestState, advance_rest_epoch, consume_impulse_wake, entering_next_tick, exact_side_hash,
    occupancy_hash, permit_server_resync,
};
#[cfg(test)]
use avian3d::prelude::{ComputedAngularInertia, Rotation};
#[cfg(test)]
use checkpoint::{
    AssemblyError, CheckpointKey, ExactCheckpoint, checkpoint_hash, make_checkpoint_chunks,
};
#[cfg(test)]
use client::{
    AdmissionError, CheckpointDisposition, EffectErrors, admit_completed_checkpoint,
    checkpoint_rollback_baseline, claim_checkpoint_rollback, effect_errors,
    historical_anchor_state,
};

/// DERIVED from the existing 1,100-byte application-payload ceiling: 48 worst-case exact
/// world-`Vec3` entries plus checkpoint metadata encode below it (guarded by a unit test).
pub(crate) const CHECKPOINT_ENTRIES_PER_CHUNK: usize = 48;
/// DERIVED from the current fixed field maximum: 16 chunks × 48 entries covers 768 sparse entries,
/// above the Tiger's 582 elements, while bounding hostile/incompatible assembly allocation.
const MAX_CHECKPOINT_CHUNKS: usize = 16;
/// DERIVED defensive cap: one predicted owner normally needs one assembly, while 16 permits several
/// overlapping epoch/resync responses without allowing an unbounded reliable-message ledger.
const MAX_CHECKPOINT_LEDGERS: usize = 16;
/// DERIVED from the existing assembly bounds: 16 ledgers × 16 chunks retains every chunk that could
/// otherwise participate in all permitted concurrent assemblies while identity replication catches up.
const MAX_DEFERRED_CHECKPOINT_CHUNKS: usize = MAX_CHECKPOINT_LEDGERS * MAX_CHECKPOINT_CHUNKS;
/// DERIVED as 2 s at the project's MEASURED 64 Hz configuration: enough for ordinary JIP ordering,
/// short enough that a checkpoint whose combatant never becomes unique cannot occupy the queue forever.
const DEFERRED_CHECKPOINT_EXPIRY_TICKS: u32 = 128;
/// DERIVED defensive cap: one owner and two epochs need only a handful of stamps; 32 retains a full
/// 8-second window at the 250 ms request interval before evicting oldest dedupe history.
const MAX_REQUEST_STAMPS: usize = 32;
/// DERIVED defensive budget: one owner normally requests one tank once; two responses let a
/// reconnecting connection repair an overlapping despawn/spawn without permitting unbounded
/// full-field serialization in one render frame.
const MAX_RESYNC_RESPONSES_PER_CONNECTION_PER_FRAME: usize = 2;
/// DERIVED as 125 ms at 64 Hz, matching the element law's contact-loss dwell: a one-tick topology
/// flutter cannot declare a field non-healing.
const REST_STABLE_TICKS: u8 = GRIP_ELEMENT_LOSS_DWELL_TICKS;
/// DERIVED initial Batch-C repair cadence: 256 ticks / 64 Hz = 4 s. Batch D's MEASURED curves
/// concluded that moving fields self-heal; the fallback remains until Phase-4 MP evidence.
const PERIODIC_CHECKPOINT_TICKS: u32 = 256;
/// DERIVED as 250 ms at 64 Hz, twice the topology/loss dwell. This bounds reliable request pressure
/// per `(tank, epoch)` while still allowing two fresh responses inside Lightyear's rollback window.
const RESYNC_MIN_INTERVAL_TICKS: u32 = 16;
/// DERIVED from the same 125 ms topology dwell: a moving digest gets one complete material-contact
/// grace interval to self-heal before requesting bytes.
const DIGEST_PERSIST_ANCHORS: u8 = GRIP_ELEMENT_LOSS_DWELL_TICKS;
/// DERIVED by reusing the existing gross linear-velocity correction policy after converting force
/// error to one-tick `delta-v`: 1.0 m/s.
const EFFECT_LINEAR_VELOCITY_THRESHOLD_MPS: f32 = ROLLBACK_VELOCITY;
/// DERIVED by reusing the existing gross angular-velocity correction policy after converting torque
/// error through world inverse inertia for one tick: 1.0 rad/s.
const EFFECT_ANGULAR_VELOCITY_THRESHOLD_RADPS: f32 = ROLLBACK_VELOCITY;
/// DERIVED by reusing the existing `TrackDrive` belt-speed correction policy after converting belt
/// reaction through reflected belt inertia for one tick: 0.25 m/s.
const EFFECT_BELT_SPEED_THRESHOLD_MPS: f32 = ROLLBACK_TRACK_DRIVE;

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

pub(super) fn install_client(app: &mut App) {
    app.init_resource::<CheckpointAssembler>()
        .init_resource::<DeferredCheckpointChunks>()
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

pub(super) fn install_server(app: &mut App) {
    app
        // Receivers are cleared in `Last`, so owner requests are drained at render rate.
        .add_systems(Update, answer_resync_requests)
        // `apply_track_forces` completed in FixedUpdate. The anchor names this completed tick,
        // matching Lightyear's end-of-tick prediction-history label; checkpoints name the same
        // field as state entering the next tick.
        .add_systems(FixedPostUpdate, publish_grip_anchor_and_rest_checkpoints);
}

#[cfg(test)]
mod tests {
    use super::checkpoint::fields_bit_equal;
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
            combatant: crate::CombatantId(1),
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
        let chunks = make_checkpoint_chunks(crate::CombatantId(7), 4, Tick(90), &field).unwrap();
        let expected_elements_per_side = field.sides[0].strain.len();
        assert!(chunks.len() > 1);
        let mut assembler = CheckpointAssembler::default();
        assert!(
            assembler
                .push(chunks[1].clone(), tank, expected_elements_per_side)
                .unwrap()
                .is_none()
        );
        assert!(
            assembler
                .push(chunks[1].clone(), tank, expected_elements_per_side)
                .unwrap()
                .is_none()
        );
        let mut completed = None;
        for chunk in chunks.iter().skip(2).chain(chunks.iter().take(1)) {
            completed = assembler
                .push(chunk.clone(), tank, expected_elements_per_side)
                .unwrap()
                .or(completed);
        }
        let checkpoint = completed.expect("only the final missing chunk publishes atomically");
        assert_eq!(checkpoint.field, field);
        for chunk in chunks {
            assert!(
                assembler
                    .push(chunk, tank, expected_elements_per_side)
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn chunk_assembly_rejects_whole_hash_corruption() {
        let tank = Entity::from_raw_u32(8).unwrap();
        let field = populated_field();
        let mut chunks =
            make_checkpoint_chunks(crate::CombatantId(8), 5, Tick(91), &field).unwrap();
        chunks[0].entries[0].strain.x = f32::from_bits(chunks[0].entries[0].strain.x.to_bits() ^ 1);
        let mut assembler = CheckpointAssembler::default();
        let mut result = Ok(None);
        for chunk in chunks {
            result = assembler.push(chunk, tank, field.sides[0].strain.len());
        }
        assert_eq!(result, Err(AssemblyError::HashMismatch));
    }

    #[test]
    fn chunk_assembly_rejects_unbounded_shape_before_field_allocation() {
        let epoch = 6;
        let tick = Tick(92);
        let elements_per_side = u16::MAX;
        let chunk = GripCheckpointChunk {
            combatant: crate::CombatantId(10),
            epoch,
            state_entering_tick: tick,
            elements_per_side,
            chunk_index: 0,
            chunk_count: 1,
            entries: Vec::new(),
            checkpoint_hash: checkpoint_hash(epoch, tick, elements_per_side, &[]),
        };

        assert_eq!(
            CheckpointAssembler::default().push(chunk, Entity::from_raw_u32(10).unwrap(), 3,),
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
            combatant: crate::CombatantId(11),
            epoch,
            state_entering_tick: tick,
            elements_per_side,
            chunk_index: 0,
            chunk_count: 1,
            checkpoint_hash: checkpoint_hash(epoch, tick, elements_per_side, &entries),
            entries,
        };

        assert_eq!(
            CheckpointAssembler::default().push(chunk, Entity::from_raw_u32(11).unwrap(), 3,),
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
            strain: Vec3::new(GRIP_SHEAR_MODULUS_M * 1.01, 0.0, 0.0),
            contact_generation: 1,
        }];
        let chunk = GripCheckpointChunk {
            combatant: crate::CombatantId(19),
            epoch,
            state_entering_tick: tick,
            elements_per_side,
            chunk_index: 0,
            chunk_count: 1,
            checkpoint_hash: checkpoint_hash(epoch, tick, elements_per_side, &entries),
            entries,
        };

        assert_eq!(
            CheckpointAssembler::default().push(chunk, Entity::from_raw_u32(19).unwrap(), 3,),
            Err(AssemblyError::InvalidStrain)
        );
    }

    #[test]
    fn chunk_assembly_accepts_a_rounded_producer_saturation() {
        let epoch = 9;
        let tick = Tick(95);
        let elements_per_side = 3;
        // DERIVED witness from the force law's unchanged `j1 *= K / |j1|` expression. Its
        // f32 `length_squared()` is two ULPs above the old zero-tolerance `K * K` bound.
        let strain = Vec3::new(-0.023_136_795, 0.064_153_15, 0.031_209_983);
        assert!(
            strain.length_squared() > GRIP_SHEAR_MODULUS_M * GRIP_SHEAR_MODULUS_M,
            "the regression witness must remain outside the old validator contract"
        );
        let entries = vec![GripCheckpointEntry {
            side: 0,
            element: 0,
            strain,
            contact_generation: 1,
        }];
        let chunk = GripCheckpointChunk {
            combatant: crate::CombatantId(20),
            epoch,
            state_entering_tick: tick,
            elements_per_side,
            chunk_index: 0,
            chunk_count: 1,
            checkpoint_hash: checkpoint_hash(epoch, tick, elements_per_side, &entries),
            entries,
        };

        assert!(
            CheckpointAssembler::default()
                .push(
                    chunk,
                    Entity::from_raw_u32(20).unwrap(),
                    usize::from(elements_per_side),
                )
                .unwrap()
                .is_some(),
            "a value emitted by the producer's saturation expression is valid authority state"
        );
    }

    #[test]
    fn checkpoint_chunk_round_trip_preserves_signed_zero_and_nan_payloads() {
        let chunk = GripCheckpointChunk {
            combatant: crate::CombatantId(12),
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
        let (predicted, rotation) = historical_anchor_state(
            &effect_history,
            &rotation_history,
            anchor.producing_tick,
            Tick(10),
        )
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
    fn changing_field_anchor_waits_for_its_producing_tick_then_matches_tick_for_tick() {
        let field_320 = populated_field();
        let mut field_321 = field_320.clone();
        field_321.sides[0].strain[0].x = 1.0;
        let effect_320 = TrackGripEffect {
            traction_force: Vec3::new(1.0, 2.0, 3.0),
            field_digest: crate::track::sim::coarse_grip_digest(&field_320),
            ..default()
        };
        let effect_321 = TrackGripEffect {
            traction_force: Vec3::new(4.0, 5.0, 6.0),
            field_digest: crate::track::sim::coarse_grip_digest(&field_321),
            ..default()
        };
        let mut effects = PredictionHistory::<TrackGripEffect>::default();
        let mut rotations = PredictionHistory::<Rotation>::default();
        effects.add_predicted(Tick(320), Some(effect_320));
        rotations.add_predicted(Tick(320), Some(Rotation::default()));

        assert!(
            historical_anchor_state(&effects, &rotations, Tick(321), Tick(320)).is_none(),
            "a future changing-effect anchor must not reuse tick 320 through HistoryBuffer's floor lookup"
        );
        let mut watch = AnchorWatch::default();
        assert!(!watch.anchor_was_compared(Entity::PLACEHOLDER, Tick(321)));

        effects.add_predicted(Tick(321), Some(effect_321));
        rotations.add_predicted(Tick(321), Some(Rotation::default()));
        let (predicted, _) = historical_anchor_state(&effects, &rotations, Tick(321), Tick(321))
            .expect("the anchor becomes comparable when its producing tick has run");
        assert_eq!(*predicted, effect_321);
        watch.mark_anchor_compared(Entity::PLACEHOLDER, Tick(321));
        assert!(watch.anchor_was_compared(Entity::PLACEHOLDER, Tick(321)));
    }

    #[test]
    fn bit_identical_checkpoint_is_a_no_op_and_rearms_only_for_new_evidence() {
        let tank = Entity::from_raw_u32(21).unwrap();
        let field = populated_field();
        let digest = crate::track::sim::coarse_grip_digest(&field);
        let checkpoint = ExactCheckpoint {
            tank,
            combatant: crate::CombatantId(21),
            epoch: 4,
            state_entering_tick: Tick(100),
            field: field.clone(),
            hash: 0xfeed,
        };
        let mut pending = PendingGripCorrection::default();
        let mut watch = AnchorWatch::default();
        let mut field_history = PredictionHistory::<TrackGripElements>::default();
        field_history.add_predicted(Tick(99), Some(field.clone()));
        let mut current_field = field.clone();
        current_field.sides[0].strain[0].x = 1.0;
        assert!(fields_bit_equal(
            field_history.get(Tick(99)).unwrap(),
            &checkpoint.field
        ));
        assert!(!fields_bit_equal(&current_field, &checkpoint.field));

        assert_eq!(
            admit_completed_checkpoint(
                &mut pending,
                &mut watch,
                checkpoint,
                &field_history,
                Tick(100),
            ),
            Ok(CheckpointDisposition::NoOp)
        );
        assert!(
            pending.checkpoint.is_none(),
            "the forced-rollback system must have no correction to claim"
        );
        assert!(watch.evidence_spent(tank, 4, digest));
        assert!(
            !watch.evidence_spent(tank, 5, digest),
            "a new epoch is new request evidence"
        );
        watch.spend_evidence(tank, 5, digest);
        assert!(watch.evidence_spent(tank, 5, digest));
        assert!(
            !watch.evidence_spent(tank, 5, digest.wrapping_add(1)),
            "a new digest is new request evidence"
        );
        watch.spend_evidence(tank, 5, digest.wrapping_add(1));
        assert!(watch.evidence_spent(tank, 5, digest.wrapping_add(1)));

        let mut different_bits = field.clone();
        different_bits.sides[0].strain[1].x = -0.0;
        let mut changed_pending = PendingGripCorrection::default();
        assert_eq!(
            admit_completed_checkpoint(
                &mut changed_pending,
                &mut AnchorWatch::default(),
                ExactCheckpoint {
                    tank,
                    combatant: crate::CombatantId(21),
                    epoch: 6,
                    state_entering_tick: Tick(102),
                    field: different_bits,
                    hash: 0xbeef,
                },
                &field_history,
                Tick(102),
            ),
            Ok(CheckpointDisposition::NeedsRollback),
            "signed zero differs on the exact raw-bit contract"
        );
        assert!(changed_pending.checkpoint.is_some());
    }

    #[test]
    fn combatant_address_round_trip_and_unresolvable_chunk_deferral() {
        let combatant = crate::CombatantId(0x0123_4567_89ab_cdef);
        let field = populated_field();
        let chunk = make_checkpoint_chunks(combatant, 5, Tick(101), &field)
            .unwrap()
            .remove(0);
        let encoded = bincode::serde::encode_to_vec(&chunk, bincode::config::standard()).unwrap();
        let (decoded, consumed) = bincode::serde::decode_from_slice::<GripCheckpointChunk, _>(
            &encoded,
            bincode::config::standard(),
        )
        .unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded.combatant, combatant);

        let mut deferred = DeferredCheckpointChunks::default();
        deferred.defer(decoded, Tick(10));
        assert!(
            deferred.take_resolvable(Tick(11), |_| None).is_empty(),
            "an unresolved combatant must retain its exact chunk"
        );
        assert_eq!(deferred.len(), 1);
        let tank = Entity::from_raw_u32(22).unwrap();
        let ready = deferred.take_resolvable(Tick(12), |id| (id == combatant).then_some(tank));
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].0, tank);
        assert_eq!(ready[0].1.combatant, combatant);
        assert_eq!(deferred.len(), 0);

        deferred.defer(ready[0].1.clone(), Tick(20));
        assert!(
            deferred
                .take_resolvable(Tick(20 + DEFERRED_CHECKPOINT_EXPIRY_TICKS + 1), |_| None,)
                .is_empty()
        );
        assert_eq!(deferred.len(), 0, "an unresolved chunk expires boundedly");
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
            combatant: crate::CombatantId(u64::MAX),
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
            "worst stable-id 48-entry checkpoint chunk plus message type id: {} B",
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
                crate::CombatantId(u64::MAX),
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

#[cfg(test)]
#[path = "grip_battery.rs"]
mod battery;
