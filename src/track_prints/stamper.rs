//! Grounded shoes of the conformed belt view become stamps.
//!
//! [`contact`] reads one shoe at its drawn pose: whether its sole faces the ground and rests on it,
//! and if so where its datum lies, which way it heads, and the plane its underside makes with the
//! ground under it. The plane is fitted to the sole's four corners against the ground beneath
//! them and lowered until its lowest corner touches: the drawn belt rides the oracle within the
//! view's own filter error, and a print is the shoe's shape, not that error.
//!
//! [`ShoeMemory`] keeps where each shoe was last stamped. A grounded shoe is stamped again only
//! once its sole has moved more than [`MOVE_TEXELS`] of a near-store texel: a shoe that rolls
//! without slipping lies still on the ground until the belt lifts it, so straight driving lays one
//! stamp per shoe per touchdown, one per pitch per side; a pivot or a slip moves every grounded
//! shoe across the ground, and each is stamped again as it goes, so the print smears by the
//! simulation's own slip.

use std::collections::HashMap;

use bevy::math::{Affine3A, Vec2, Vec3};
use bevy::prelude::Entity;

use super::store::NEAR_TEXEL_M;

/// The fraction of a near-store texel a grounded sole must move before it is stamped again.
const MOVE_TEXELS: f32 = 0.5;

/// The least cosine between a shoe's inner axis and world up for its sole to face the ground:
/// 60° of tilt. ASSUMED: steeper than any slope a tank holds, flatter than any sole on the end
/// arcs or the return run.
const FACING_COS: f32 = 0.5;

/// The deepest a sole's lowest corner may sit under the ground and still be read as resting on it,
/// m. ASSUMED: far below the drawn belt's filter error, far above a shoe parked below the world
/// before its first placement.
const SINK_LIMIT_M: f32 = 0.3;

/// One grounded shoe, as a stamp places it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ShoeContact {
    /// World XZ of the datum point: the canonical origin at the underside's lowest height.
    pub(crate) datum: Vec2,
    /// Heading of the canonical `+z` in world XZ, rad from `+Z` toward `+X`.
    pub(crate) yaw: f32,
    /// The underside plane over the ground, `lift + tilt · (x, z)` in canonical `(x, z)`, its
    /// lowest corner at zero.
    pub(crate) lift: f32,
    pub(crate) tilt: Vec2,
    /// The farthest the sole reaches from the datum point, m: how far a turn moves it.
    pub(crate) reach: f32,
}

/// Shoe `pose` (canonical frame to world) with its sole over canonical `(x, z)` from `low` to
/// `high` and its datum at canonical `datum_y`, against `ground` (world XZ to height, `None` off
/// the ground): its contact if its sole faces the ground and its plane comes within `contact_m`
/// of resting on it.
pub(crate) fn contact(
    pose: Affine3A,
    (low, high): (Vec2, Vec2),
    datum_y: f32,
    contact_m: f32,
    ground: impl Fn(Vec2) -> Option<f32>,
) -> Option<ShoeContact> {
    let inner = pose.transform_vector3(Vec3::Y);
    if inner.y < FACING_COS {
        return None;
    }
    let along = pose.transform_vector3(Vec3::Z);
    let heading = Vec2::new(along.x, along.z).try_normalize()?;
    let corners = [
        Vec2::new(low.x, low.y),
        Vec2::new(high.x, low.y),
        Vec2::new(low.x, high.y),
        Vec2::new(high.x, high.y),
    ];
    let mut clearance = [0.0; 4];
    for (slot, corner) in clearance.iter_mut().zip(corners) {
        let world = pose.transform_point3(Vec3::new(corner.x, datum_y, corner.y));
        *slot = world.y - ground(Vec2::new(world.x, world.z))?;
    }
    let least = clearance.iter().copied().fold(f32::INFINITY, f32::min);
    if !(-SINK_LIMIT_M..=contact_m).contains(&least) {
        return None;
    }
    // The least-squares plane through four corners of a rectangle.
    let [c00, c10, c01, c11] = clearance;
    let span = high - low;
    let tilt = Vec2::new(
        (c10 - c00 + c11 - c01) / (2.0 * span.x),
        (c01 - c00 + c11 - c10) / (2.0 * span.y),
    );
    let mid = (low + high) * 0.5;
    let mut lift = 0.25 * (c00 + c10 + c01 + c11) - tilt.dot(mid);
    lift -= corners
        .iter()
        .map(|c| lift + tilt.dot(*c))
        .fold(f32::INFINITY, f32::min);
    let datum = pose.transform_point3(Vec3::new(0.0, datum_y, 0.0));
    Some(ShoeContact {
        datum: Vec2::new(datum.x, datum.z),
        yaw: heading.x.atan2(heading.y),
        lift,
        tilt,
        reach: corners.iter().map(|c| c.length()).fold(0.0, f32::max),
    })
}

/// Where one shoe was last stamped.
#[derive(Clone, Copy, Debug)]
struct Placed {
    datum: Vec2,
    yaw: f32,
}

/// Where each shoe of one tank was last stamped.
#[derive(Default)]
pub(crate) struct ShoeMemory(HashMap<Entity, Placed>);

impl ShoeMemory {
    /// Whether `shoe`, grounded at `contact`, is stamped now: its first contact, or a move of its
    /// sole beyond [`MOVE_TEXELS`] of a texel since its last stamp. Remembered when it is.
    pub(crate) fn observe(&mut self, shoe: Entity, contact: &ShoeContact) -> bool {
        let moved = self.0.get(&shoe).is_none_or(|placed| {
            let turn = (contact.yaw - placed.yaw + std::f32::consts::PI)
                .rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI;
            placed.datum.distance(contact.datum) + turn.abs() * contact.reach
                > MOVE_TEXELS * NEAR_TEXEL_M
        });
        if moved {
            self.0.insert(
                shoe,
                Placed {
                    datum: contact.datum,
                    yaw: contact.yaw,
                },
            );
        }
        moved
    }

    /// Forget every shoe: a teleport or a respawn, after which no stamp is a move from the last.
    pub(crate) fn reset(&mut self) {
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Quat;

    /// The synthetic sole every test drives, fixture geometry: 0.6 m across, 0.16 m along.
    const LOW: Vec2 = Vec2::new(-0.3, -0.08);
    const HIGH: Vec2 = Vec2::new(0.3, 0.08);
    const PITCH: f32 = 0.13;
    const CONTACT_M: f32 = 0.03;

    fn flat(_: Vec2) -> Option<f32> {
        Some(0.0)
    }

    /// A sole lying flat on flat ground at world XZ `at`, heading `yaw`.
    fn lying(at: Vec2, yaw: f32) -> Affine3A {
        Affine3A::from_rotation_translation(Quat::from_rotation_y(yaw), Vec3::new(at.x, 0.0, at.y))
    }

    /// A belt `side_x` from the hull's centre: shoe `k` is laid at `k · PITCH` along the hull's
    /// path, and lies there, still, while the hull's contact patch `[z − 1.8, z + 1.8]` covers it.
    /// Rolling without slip: the ground under a shoe never moves.
    fn patch(hull_z: f32, side_x: f32) -> Vec<(u32, Affine3A)> {
        let first = ((hull_z - 1.8) / PITCH).ceil() as i32;
        let last = ((hull_z + 1.8) / PITCH).floor() as i32;
        (first..=last)
            .map(|k| {
                let id = (k.rem_euclid(97) as u32) + if side_x < 0.0 { 1000 } else { 0 };
                (id, lying(Vec2::new(side_x, k as f32 * PITCH), 0.0))
            })
            .collect()
    }

    /// ROLLING LAYS ONE STAMP PER SHOE PER PITCH. A hull rolls 50 m in 2 cm frames over a belt
    /// whose grounded shoes lie still: every shoe is stamped once when it touches down and never
    /// while it lies there, so each side lays one stamp per pitch travelled, plus the patch it
    /// started on. A rule that stamped every grounded shoe every frame, or measured motion against
    /// the hull rather than the ground, lays hundreds of times more.
    #[test]
    fn rolling_without_slip_lays_one_stamp_per_shoe_per_pitch() {
        let mut memory = ShoeMemory::default();
        let entity = |id: u32| Entity::from_raw_u32(id + 1).expect("a valid index");
        let (travel, step) = (50.0, 0.02);
        let mut stamps = [0_u32; 2];
        for frame in 0..=(travel / step) as u32 {
            let hull_z = frame as f32 * step;
            for (side, x) in [(0, -1.4), (1, 1.4)] {
                for (id, pose) in patch(hull_z, x) {
                    let contact = contact(pose, (LOW, HIGH), 0.0, CONTACT_M, flat)
                        .expect("lies on the ground");
                    if memory.observe(entity(id), &contact) {
                        stamps[side] += 1;
                    }
                }
            }
        }
        let expected = ((travel + 3.6) / PITCH).round() as u32;
        for count in stamps {
            assert!(
                count.abs_diff(expected) <= 2,
                "{count} stamps per side, want one per pitch: {expected}",
            );
        }
    }

    /// A PIVOT STAMPS EVERY GROUNDED SHOE AGAIN. The hull turns on the spot: every grounded shoe
    /// slides across the ground about the hull's centre, so once the turn has carried the slowest
    /// shoe (nearest the centre) half a texel, every one of them has been stamped anew. A rule that
    /// read only the hull's own travel, or the shoe's position without its turn, misses the shoes
    /// near the pivot.
    #[test]
    fn a_pivot_stamps_every_grounded_shoe_again() {
        let mut memory = ShoeMemory::default();
        let entity = |id: u32| Entity::from_raw_u32(id + 1).expect("a valid index");
        let shoes = |turn: f32| {
            let mut out = Vec::new();
            for x in [-1.4, 1.4] {
                for k in -13..=13 {
                    let offset = Vec2::new(x, k as f32 * PITCH);
                    let rotated = Vec2::from_angle(-turn).rotate(offset);
                    out.push(lying(rotated, turn));
                }
            }
            out
        };
        for (id, pose) in shoes(0.0).into_iter().enumerate() {
            let contact = contact(pose, (LOW, HIGH), 0.0, CONTACT_M, flat).expect("grounded");
            assert!(memory.observe(entity(id as u32), &contact), "first contact");
        }
        let mut stamped = vec![false; shoes(0.0).len()];
        let mut turn = 0.0;
        while turn < 0.05 {
            turn += 0.002;
            for (id, pose) in shoes(turn).into_iter().enumerate() {
                let contact = contact(pose, (LOW, HIGH), 0.0, CONTACT_M, flat).expect("grounded");
                stamped[id] |= memory.observe(entity(id as u32), &contact);
            }
        }
        assert!(
            stamped.iter().all(|&s| s),
            "a 0.05 rad pivot moves every sole more than half a texel; unstamped: {:?}",
            stamped
                .iter()
                .enumerate()
                .filter(|(_, s)| !**s)
                .map(|(i, _)| i)
                .collect::<Vec<_>>(),
        );
    }

    /// A sole off the ground, facing up, or parked far below it is no contact; one tilted onto
    /// one edge rests on that edge, its plane rising across it.
    #[test]
    fn only_a_sole_resting_on_the_ground_is_in_contact() {
        let at = Vec2::new(3.0, 4.0);
        let raised = Affine3A::from_translation(Vec3::new(at.x, 0.1, at.y));
        assert!(contact(raised, (LOW, HIGH), 0.0, CONTACT_M, flat).is_none());
        let upside_down = lying(at, 0.0) * Affine3A::from_rotation_z(std::f32::consts::PI);
        assert!(contact(upside_down, (LOW, HIGH), 0.0, CONTACT_M, flat).is_none());
        let parked = Affine3A::from_translation(Vec3::new(0.0, -1000.0, 0.0));
        assert!(contact(parked, (LOW, HIGH), 0.0, CONTACT_M, flat).is_none());

        // Rolled 5° about its long axis: the +x edge down, resting on it.
        let rolled = lying(at, 0.0)
            * Affine3A::from_translation(Vec3::new(0.0, 0.3 * 5_f32.to_radians().sin(), 0.0))
            * Affine3A::from_rotation_z(-5_f32.to_radians());
        let tilted = contact(rolled, (LOW, HIGH), 0.0, CONTACT_M, flat).expect("on one edge");
        assert!(
            (tilted.lift + tilted.tilt.x * HIGH.x).abs() < 1e-3 && tilted.tilt.x < -0.05,
            "the +x edge touches and the plane rises toward −x: {tilted:?}",
        );
    }
}
