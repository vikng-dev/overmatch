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
    AnchorWatch, PendingGripCorrection, compare_track_grip_anchors,
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
    AdmissionError, EffectErrors, checkpoint_rollback_baseline, claim_checkpoint_rollback,
    effect_errors, historical_anchor_state,
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
        // `apply_track_forces` completed in FixedUpdate. This captures the end-of-tick field and
        // labels checkpoints as entering the next tick.
        .add_systems(FixedPostUpdate, publish_grip_anchor_and_rest_checkpoints);
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

#[cfg(test)]
#[path = "grip_battery.rs"]
mod battery;
