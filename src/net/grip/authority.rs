//! Authority-side rest detection, effect-anchor publication, and exact checkpoint responses.

use bevy::prelude::*;
use lightyear::connection::client_of::ClientOf;
use lightyear::prelude::client::Remote;
use lightyear::prelude::*;

use crate::command::TankCommand;
use crate::net::protocol::{
    GripCheckpointChannel, GripCheckpointChunk, GripResyncRequest, NetTrackGripAnchor,
};
use crate::track::drive::DriveAxes;
use crate::track::forces::GripElements;
use crate::track::sim::{TrackDrive, TrackGripEffect, TrackGripElements, TrackGripWake};

use super::checkpoint::{hash_write, make_checkpoint_chunks};
use super::{
    GripRequestLimiter, MAX_RESYNC_RESPONSES_PER_CONNECTION_PER_FRAME, PERIODIC_CHECKPOINT_TICKS,
    REST_STABLE_TICKS,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct SideRestState {
    pub(super) field_hash: u64,
    pub(super) occupancy_hash: u64,
    pub(super) phase_bits: u64,
    pub(super) stable_ticks: u8,
    pub(super) resting: bool,
}

/// Authority-only rest/non-turnover detector. It is netcode bookkeeping, not rollback simulation
/// state; local physics never reads or gates on it.
#[derive(Component, Clone, Copy, Debug, Default)]
pub(crate) struct GripRestState {
    pub(super) initialized: bool,
    pub(super) epoch: u32,
    pub(super) wake_generation: u32,
    pub(super) sides: [SideRestState; 2],
    pub(super) last_checkpoint_tick: Tick,
}

pub(super) fn exact_side_hash(side: &GripElements) -> u64 {
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

pub(super) fn occupancy_hash(side: &GripElements) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    for (element, &dwell) in side.dwell.iter().enumerate() {
        if dwell != 0 {
            hash_write(&mut hash, (element as u16).to_le_bytes());
        }
    }
    hash
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

pub(super) fn advance_rest_epoch(
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

pub(super) fn entering_next_tick(now: Tick) -> Tick {
    Tick(now.0.saturating_add(1))
}

pub(super) fn consume_impulse_wake(rest: &mut GripRestState, wake: &TrackGripWake) -> bool {
    let generation = wake.generation();
    let changed = rest.wake_generation != generation;
    rest.wake_generation = generation;
    changed
}

#[allow(clippy::too_many_arguments)]
pub(super) fn publish_grip_anchor_and_rest_checkpoints(
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

pub(super) fn permit_server_resync(
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

pub(super) fn answer_resync_requests(
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
