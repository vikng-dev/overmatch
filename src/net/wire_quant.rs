//! The quantised wire forms of the state every client receives for every tank, every tick
//! (#185): fewer bytes than postcard's raw floats, each with a stated step, range, out-of-range
//! rule and error bound. The components keep their own types on both ends; only the bytes
//! between them change. [`super::protocol::plugin`] registers each codec through replicon's
//! custom rule functions (`*_fns`), and lightyear's interpolation history deserializes through
//! the same functions, so an interpolated component samples the decoded values.
//!
//! | Component | Wire | Step | Range | Out of range | Error bound (DERIVED) |
//! |---|---|---|---|---|---|
//! | `Position` | 8 B: 3 × 21-bit fixed point | 2⁻¹⁰ m ≈ 0.98 mm | ±1024 m per axis | 13 B: a marker byte and the three f32s, exact | ≤ 0.49 mm per axis, ≤ 0.85 mm |
//! | `Rotation` | 6 B smallest-three | √2/(2¹⁵−1) per component | every unit quaternion | — (non-finite or zero: the identity) | ≤ 1.50·10⁻⁴ rad, so ≤ 0.15 mm per metre from the root |
//! | `LinearVelocity` | 6 B: 3 × i16 | 2⁻⁸ m/s ≈ 3.9 mm/s | ±128 m/s per axis | clamped to the range; NaN reads 0 | ≤ 1.95 mm/s per axis, ≤ 3.4 mm/s |
//! | `AngularVelocity` | 6 B: 3 × i16 | 2⁻¹⁰ rad/s ≈ 0.98 mrad/s | ±32 rad/s per axis | clamped to the range; NaN reads 0 | ≤ 0.49 mrad/s per axis, ≤ 0.85 mrad/s |
//! | `TrackDrive` belt speed, per side | 2 B: i16 | 2⁻¹⁰ m/s ≈ 0.98 mm/s | ±32 m/s | clamped to the range; NaN reads 0 | ≤ 0.49 mm/s |
//! | `TrackDrive` belt phase, per side | 3 B: u24, wrapped | 2⁻¹⁴ m ≈ 0.061 mm | modulo `BELT_PHASE_WRAP_M` = 1024 m | — (wrapped by design; non-finite sends 0) | ≤ 0.031 mm, modulo the wrap |
//!
//! Every step is a power of two, so every code decodes to an exact f32. Kalinovo spans ±750 m
//! (`assets/maps/kalinovo/level.json`), inside the packed position range; a larger map's far
//! ground costs the 13-byte form, never a wrong position.
//!
//! Against the extrapolator (`super::extrapolate`): its horizon g* ≈ 52.3 ms spends the certified
//! ε_vis = 12.08 mm on the constant-velocity projection from the newest sample, which these codecs
//! deliver within the bounds above. The velocities move the projection by at most 3.4 mm/s × g* ≈
//! 0.18 mm plus 0.85 mrad/s × g* ≈ 0.044 mm per metre from the root; the sample itself sits within
//! 0.85 mm plus 0.15 mm per metre of the authority's, gap or no gap. At 5 m from the root (ASSUMED,
//! a hull's far corner) the four add up to ≤ 2.0 mm, a sixth of ε_vis, which the horizon's
//! derivation does not subtract.
//!
//! The belt state feeds the track view, which draws a phase carried at the belt speed and eased
//! onto the replicated phase (`track::view`'s `presented_phase`): its errors sit three orders
//! under a link pitch. The view and the belt interpolator take phase differences modulo the wrap.
//!
//! Every byte layout here is pinned by the wire-types hash (`protocol`'s `WIRE_CODEC_SOURCES`):
//! this file's code, comments stripped, folds into it whole.

use crate::track::sim::{BELT_PHASE_WRAP_M, TrackDrive, TrackDriveSide};
use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use bevy_replicon::bytes::Bytes;
use bevy_replicon::prelude::RuleFns;
use bevy_replicon::shared::replication::registry::ctx::{SerializeCtx, WriteCtx};

/// Bits per smallest-three component.
const ROTATION_BITS: u32 = 15;

/// The largest code of a smallest-three component.
const ROTATION_CODES: f64 = ((1u32 << ROTATION_BITS) - 1) as f64;

/// A rotation in 6 bytes: the index of the quaternion's largest-magnitude component (2 bits), the
/// other three in index order (15 bits each, uniform over `[−1/√2, 1/√2]`, the most any of them
/// can hold beside a larger one: `c² ≤ ½(c² + c_max²) ≤ ½`), and the largest one's sign (1 bit),
/// little-endian. The largest is recovered as `√(1 − Σc²)`; carrying its sign keeps the decoded
/// quaternion in the authority's hemisphere, so consecutive samples never flip between `q` and
/// `−q`.
///
/// Error (DERIVED): each component is off by at most half a step, `δ = √2/(2·(2¹⁵−1)) ≈
/// 2.16·10⁻⁵`. The recovered component `w` (|w| ≥ ½, the largest of four unit components) is off
/// by `|Σc·δc|/|w| ≤ δ·√3·√(3/4)/½ = 3δ`, so the decoded quaternion is within `√(3δ² + 9δ²) =
/// √12·δ` of the true one and the rotation between them is at most `2·√12·δ ≈ 1.50·10⁻⁴ rad`.
pub(super) fn write_rotation(rotation: Quat, message: &mut Vec<u8>) {
    let q = if rotation.is_finite() && rotation.length_squared() > 0.0 {
        rotation.normalize()
    } else {
        Quat::IDENTITY
    };
    let c = q.to_array().map(f64::from);
    let largest = (0..4)
        .max_by(|&a, &b| c[a].abs().total_cmp(&c[b].abs()))
        .expect("four components");
    let mut packed = largest as u64;
    let mut shift = 2;
    for (index, &component) in c.iter().enumerate() {
        if index == largest {
            continue;
        }
        let unit = (component * std::f64::consts::SQRT_2 + 1.0) * 0.5;
        let code = (unit * ROTATION_CODES).round().clamp(0.0, ROTATION_CODES) as u64;
        packed |= code << shift;
        shift += ROTATION_BITS;
    }
    if c[largest] < 0.0 {
        packed |= 1 << shift;
    }
    message.extend_from_slice(&packed.to_le_bytes()[..6]);
}

/// The unit quaternion [`write_rotation`] wrote; consumes its 6 bytes.
pub(super) fn read_rotation(message: &mut Bytes) -> Result<Quat> {
    let bytes: [u8; 6] = take(message)?;
    let mut le = [0; 8];
    le[..6].copy_from_slice(&bytes);
    let packed = u64::from_le_bytes(le);
    let largest = (packed & 0b11) as usize;
    let mut c = [0.0f64; 4];
    let mut shift = 2;
    let mut sum = 0.0;
    for (index, component) in c.iter_mut().enumerate() {
        if index == largest {
            continue;
        }
        let code = (packed >> shift) & ((1 << ROTATION_BITS) - 1);
        shift += ROTATION_BITS;
        *component = (code as f64 / ROTATION_CODES * 2.0 - 1.0) / std::f64::consts::SQRT_2;
        sum += *component * *component;
    }
    let magnitude = (1.0 - sum).max(0.0).sqrt();
    c[largest] = if (packed >> shift) & 1 == 1 {
        -magnitude
    } else {
        magnitude
    };
    Ok(Quat::from_array(c.map(|component| component as f32)).normalize())
}

/// [`Rotation`] on the wire as [`write_rotation`].
pub(super) fn rotation_fns() -> RuleFns<Rotation> {
    RuleFns::new(
        |_: &mut SerializeCtx, rotation: &Rotation, message: &mut Vec<u8>| {
            write_rotation(rotation.0, message);
            Ok(())
        },
        |_: &mut WriteCtx, message: &mut Bytes| read_rotation(message).map(Rotation),
    )
}

/// Position steps per metre: a step of 2⁻¹⁰ m.
const POSITION_STEPS_PER_M: f32 = 1024.0;

/// Bits per packed position axis, two's complement: ±2²⁰ steps, ±1024 m.
const POSITION_BITS: u32 = 21;

/// The first byte of the full-precision form. Bit 0 is the form flag: the packed form keeps it
/// clear (its fields start at bit 1).
const POSITION_FULL: u8 = 1;

/// A position in 8 bytes when every axis is inside ±1024 m: a clear flag bit, then x, y and z as
/// 21-bit two's-complement counts of 2⁻¹⁰ m, little-endian. Error: half a step per axis, ≤ 2⁻¹¹ m
/// ≈ 0.49 mm, ≤ √3·2⁻¹¹ m ≈ 0.85 mm in space. Any axis outside, or not finite: [`POSITION_FULL`]
/// and the three f32s, 13 bytes and exact.
pub(super) fn write_position(position: Vec3, message: &mut Vec<u8>) {
    let half = 1i64 << (POSITION_BITS - 1);
    let mut packed = 0u64;
    for (axis, value) in position.to_array().into_iter().enumerate() {
        let steps = (value * POSITION_STEPS_PER_M).round();
        // Fails for NaN too.
        if !(steps >= -(half as f32) && steps < half as f32) {
            message.push(POSITION_FULL);
            for value in position.to_array() {
                message.extend_from_slice(&value.to_le_bytes());
            }
            return;
        }
        let code = (steps as i64 + half) as u64;
        packed |= code << (1 + axis as u32 * POSITION_BITS);
    }
    message.extend_from_slice(&packed.to_le_bytes());
}

/// The position [`write_position`] wrote, in either form; consumes its 8 or 13 bytes.
pub(super) fn read_position(message: &mut Bytes) -> Result<Vec3> {
    let [first] = take::<1>(message)?;
    if first & 1 == POSITION_FULL {
        let bytes: [u8; 12] = take(message)?;
        let axis =
            |index: usize| f32::from_le_bytes(std::array::from_fn(|byte| bytes[4 * index + byte]));
        return Ok(Vec3::new(axis(0), axis(1), axis(2)));
    }
    let rest: [u8; 7] = take(message)?;
    let packed = u64::from_le_bytes(std::array::from_fn(|byte| {
        if byte == 0 { first } else { rest[byte - 1] }
    }));
    let half = 1i64 << (POSITION_BITS - 1);
    let axis = |index: u32| {
        let code = (packed >> (1 + index * POSITION_BITS)) & ((1 << POSITION_BITS) - 1);
        (code as i64 - half) as f32 / POSITION_STEPS_PER_M
    };
    Ok(Vec3::new(axis(0), axis(1), axis(2)))
}

/// [`Position`] on the wire as [`write_position`].
pub(super) fn position_fns() -> RuleFns<Position> {
    RuleFns::new(
        |_: &mut SerializeCtx, position: &Position, message: &mut Vec<u8>| {
            write_position(position.0, message);
            Ok(())
        },
        |_: &mut WriteCtx, message: &mut Bytes| read_position(message).map(Position),
    )
}

/// Linear velocity steps per m/s: a step of 2⁻⁸ m/s, so an i16 holds ±128 m/s.
pub(super) const LINEAR_VELOCITY_STEPS: f32 = 256.0;

/// Angular velocity steps per rad/s: a step of 2⁻¹⁰ rad/s, so an i16 holds ±32 rad/s.
pub(super) const ANGULAR_VELOCITY_STEPS: f32 = 1024.0;

/// A vector in 6 bytes: each axis an i16 count of `1/steps`, little-endian. Error: half a step per
/// axis, `√3/(2·steps)` in space. An axis outside the i16 range saturates at its bound (Rust's
/// float-to-int cast), and NaN reads 0. Postcard would write each i16 as a zigzag varint of up to
/// 3 bytes; the fixed layout is 2.
pub(super) fn write_i16x3(value: Vec3, steps: f32, message: &mut Vec<u8>) {
    for axis in value.to_array() {
        message.extend_from_slice(&i16_code(axis, steps));
    }
}

/// The vector [`write_i16x3`] wrote at the same `steps`; consumes its 6 bytes.
pub(super) fn read_i16x3(message: &mut Bytes, steps: f32) -> Result<Vec3> {
    let bytes: [u8; 6] = take(message)?;
    let axis = |index: usize| {
        f32::from(i16::from_le_bytes([bytes[2 * index], bytes[2 * index + 1]])) / steps
    };
    Ok(Vec3::new(axis(0), axis(1), axis(2)))
}

/// [`LinearVelocity`] on the wire as [`write_i16x3`] at [`LINEAR_VELOCITY_STEPS`].
pub(super) fn linear_velocity_fns() -> RuleFns<LinearVelocity> {
    RuleFns::new(
        |_: &mut SerializeCtx, velocity: &LinearVelocity, message: &mut Vec<u8>| {
            write_i16x3(velocity.0, LINEAR_VELOCITY_STEPS, message);
            Ok(())
        },
        |_: &mut WriteCtx, message: &mut Bytes| {
            read_i16x3(message, LINEAR_VELOCITY_STEPS).map(LinearVelocity)
        },
    )
}

/// [`AngularVelocity`] on the wire as [`write_i16x3`] at [`ANGULAR_VELOCITY_STEPS`].
pub(super) fn angular_velocity_fns() -> RuleFns<AngularVelocity> {
    RuleFns::new(
        |_: &mut SerializeCtx, velocity: &AngularVelocity, message: &mut Vec<u8>| {
            write_i16x3(velocity.0, ANGULAR_VELOCITY_STEPS, message);
            Ok(())
        },
        |_: &mut WriteCtx, message: &mut Bytes| {
            read_i16x3(message, ANGULAR_VELOCITY_STEPS).map(AngularVelocity)
        },
    )
}

/// Belt speed steps per m/s: a step of 2⁻¹⁰ m/s, so an i16 holds ±32 m/s.
pub(super) const BELT_SPEED_STEPS: f32 = 1024.0;

/// Belt phase steps per metre: a step of 2⁻¹⁴ m. With [`BELT_PHASE_WRAP_M`] = 2¹⁰ m, the wrap is
/// exactly 2²⁴ steps, so a u24 holds one wrap.
const BELT_PHASE_STEPS_PER_M: f64 = 16_384.0;

/// The phase's code count: one wrap of steps, exactly the u24's range.
const BELT_PHASE_CODES: f64 = BELT_PHASE_WRAP_M * BELT_PHASE_STEPS_PER_M;
const _: () = assert!(BELT_PHASE_CODES == (1u32 << 24) as f64);

/// The belt state in 10 bytes: per side, left then right, the speed as an i16 count of 2⁻¹⁰ m/s
/// (±32 m/s, clamped beyond, NaN 0; error ≤ 2⁻¹¹ m/s ≈ 0.49 mm/s) and the phase as a u24 count of
/// 2⁻¹⁴ m modulo [`BELT_PHASE_WRAP_M`] (error ≤ 2⁻¹⁵ m ≈ 0.031 mm modulo the wrap; non-finite
/// sends 0), little-endian. A decoded phase lies in `[0, BELT_PHASE_WRAP_M)`.
pub(super) fn write_track_drive(drive: &TrackDrive, message: &mut Vec<u8>) {
    for side in drive.sides {
        message.extend_from_slice(&i16_code(side.speed, BELT_SPEED_STEPS));
        let code = if side.phase.is_finite() {
            (side.phase * BELT_PHASE_STEPS_PER_M)
                .round()
                .rem_euclid(BELT_PHASE_CODES) as u32
        } else {
            0
        };
        message.extend_from_slice(&code.to_le_bytes()[..3]);
    }
}

/// The belt state [`write_track_drive`] wrote; consumes its 10 bytes.
pub(super) fn read_track_drive(message: &mut Bytes) -> Result<TrackDrive> {
    let mut sides = [TrackDriveSide::default(); 2];
    for side in &mut sides {
        let [s0, s1, p0, p1, p2] = take::<5>(message)?;
        side.speed = f32::from(i16::from_le_bytes([s0, s1])) / BELT_SPEED_STEPS;
        side.phase = f64::from(u32::from_le_bytes([p0, p1, p2, 0])) / BELT_PHASE_STEPS_PER_M;
    }
    Ok(TrackDrive { sides })
}

/// [`TrackDrive`] on the wire as [`write_track_drive`].
pub(super) fn track_drive_fns() -> RuleFns<TrackDrive> {
    RuleFns::new(
        |_: &mut SerializeCtx, drive: &TrackDrive, message: &mut Vec<u8>| {
            write_track_drive(drive, message);
            Ok(())
        },
        |_: &mut WriteCtx, message: &mut Bytes| read_track_drive(message),
    )
}

/// `value` as an i16 count of `1/steps`, little-endian: rounded, saturating at the i16 bounds
/// (Rust's float-to-int cast), NaN 0.
fn i16_code(value: f32, steps: f32) -> [u8; 2] {
    ((value * steps).round() as i16).to_le_bytes()
}

/// The next `N` raw bytes, or an error naming the shortfall: a truncated message is refused, never
/// read past.
fn take<const N: usize>(message: &mut Bytes) -> Result<[u8; N]> {
    if message.len() < N {
        return Err(format!("wire_quant: {N} bytes expected, {} left", message.len()).into());
    }
    let head = message.split_to(N);
    Ok(std::array::from_fn(|index| head[index]))
}

#[cfg(test)]
mod tests;
