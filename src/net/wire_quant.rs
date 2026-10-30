//! The quantised wire forms of the state every client receives for every tank, every tick
//! (#185): fewer bytes than postcard's raw floats, each with a stated step, range, out-of-range
//! rule and error bound. The components keep their own types on both ends; only the bytes
//! between them change. [`super::protocol::plugin`] registers each codec through replicon's
//! custom rule functions (`*_fns`), and lightyear's interpolation history deserializes through
//! the same functions, so an interpolated component samples the decoded values.
//!
//! | Component | Wire | Step | Range | Out of range | Error bound (DERIVED) |
//! |---|---|---|---|---|---|
//! | `Rotation` | 6 B smallest-three | √2/(2¹⁵−1) per component | every unit quaternion | — (non-finite or zero: the identity) | ≤ 1.50·10⁻⁴ rad, so ≤ 0.15 mm per metre from the root |
//!
//! The extrapolator (`super::extrapolate`) spends the certified ε_vis = 12.08 mm on its
//! constant-velocity projection from the newest sample; a decoded sample differs from the
//! authority's by the bounds above.
//!
//! Every byte layout here is pinned by the wire-types hash (`protocol`'s `WIRE_CODEC_SOURCES`):
//! this file's code, comments stripped, folds into it whole.

use avian3d::prelude::Rotation;
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
