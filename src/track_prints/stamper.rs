//! Grounded shoes of the conformed belt view become stamps.
//!
//! [`contact`] reads one shoe at its drawn pose: whether its sole faces the ground and rests on it,
//! and if so where its datum lies, which way it heads, and the plane its underside makes with the
//! ground under it. The plane is fitted to the sole's four corners against the ground beneath
//! them and lowered until its lowest corner touches: the drawn belt rides the oracle within the
//! view's own filter error, and a print is the shoe's shape, not that error.
//!
//! [`ShoeMemory`] keeps where each shoe was last stamped and how far into its PRESS it is. A
//! grounded shoe is stamped again once its sole has moved more than [`MOVE_TEXELS`] of a
//! near-store texel: a shoe that rolls without slipping lies still on the ground until the belt
//! lifts it, so it is stamped where it touched down and nowhere else; a pivot or a slip moves every
//! grounded shoe across the ground, and each is stamped again as it goes, so the print smears by the
//! simulation's own slip.
//!
//! THE PRESS. A link's print deepens from nothing to all of it while the link travels one pitch
//! after touching down: its travel is its motion relative to the hull (the belt carrying it back as
//! the tank drives on, or round as it pivots) plus its slide over the ground. The press is laid in
//! [`PRESS_STEPS`] re-stamps, each at the next step's share of the pattern, and the stores keep
//! the deepest, so a print only ever deepens. Straight driving therefore lays `PRESS_STEPS` stamps
//! per link per touchdown.

use bevy::math::{Affine3A, Vec2, Vec3};
use bevy::platform::collections::HashMap;
use bevy::prelude::Entity;

use super::store::{NEAR_TEXEL_M, Placement};

/// The fraction of a near-store texel a grounded sole must move before it is stamped again.
const MOVE_TEXELS: f32 = 0.5;

/// The most a sweep's samples are apart, in near-store texels: one. A sole is far longer than a
/// texel, so the samples' union leaves no gap; each texel centre on the path is sampled within half
/// a texel of every point of the shoe that crossed it, the store's own resolution. Half a texel
/// would double a pivot's stamps for no texel the store can hold.
const SWEEP_TEXELS: f32 = 1.0;

/// The re-stamps a link's press is laid in, over one pitch of its travel. ASSUMED: four, the
/// fewest that read as a ramp rather than a pop at the 10 m view.
pub(crate) const PRESS_STEPS: u8 = 4;

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

impl ShoeContact {
    /// This contact as a stamp places it, of shoe stamp `shoe`, read mirrored or not.
    pub(crate) fn placement(&self, shoe: u8, mirrored: bool) -> Placement {
        Placement {
            datum: self.datum,
            yaw: self.yaw,
            lift: self.lift,
            tilt: self.tilt,
            shoe,
            mirrored,
        }
    }

    /// How far the sole moved over the ground from `from`, m: its datum's travel plus the arc
    /// its farthest point swept turning.
    fn motion_from(&self, from: &ShoeContact) -> f32 {
        from.datum.distance(self.datum) + turn(from.yaw, self.yaw).abs() * self.reach
    }
}

/// The shortest signed turn from heading `from` to `to`, rad.
fn turn(from: f32, to: f32) -> f32 {
    (to - from + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
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

/// A continuous grounded movement, sampled at no more than [`SWEEP_TEXELS`] of sole motion, each
/// sample with the press it is laid at. Interpolated contacts retain lift and tilt, and take the
/// shortest turn through the yaw seam.
pub(crate) struct ShoeSweep {
    from: ShoeContact,
    to: ShoeContact,
    turn: f32,
    step: u32,
    steps: u32,
    press: f32,
}

impl Iterator for ShoeSweep {
    type Item = (ShoeContact, f32);

    fn next(&mut self) -> Option<Self::Item> {
        if self.step == self.steps {
            return None;
        }
        self.step += 1;
        let t = self.step as f32 / self.steps as f32;
        Some((
            ShoeContact {
                datum: self.from.datum.lerp(self.to.datum, t),
                yaw: self.from.yaw + self.turn * t,
                lift: self.from.lift + (self.to.lift - self.from.lift) * t,
                tilt: self.from.tilt.lerp(self.to.tilt, t),
                reach: self.to.reach,
            },
            self.press,
        ))
    }
}

/// One continuously grounded shoe: where it was last stamped, where it was last frame, and how
/// far into its press it is.
#[derive(Clone, Copy)]
struct Grounded {
    /// The last stamp's contact, or while nothing has been stamped, the latest one.
    stamped: ShoeContact,
    /// Last frame's contact and hull-local position, the base of this frame's travel.
    frame: ShoeContact,
    hull_local: Vec3,
    /// The link's travel since touchdown, m.
    travel: f32,
    /// The press step last stamped, `0..=PRESS_STEPS`.
    step: u8,
}

/// Every continuously grounded shoe of one tank.
#[derive(Default)]
pub(crate) struct ShoeMemory(HashMap<Entity, Grounded>);

impl ShoeMemory {
    /// The stamps grounded shoe `shoe` lays this frame, at `contact`, its hull-local position
    /// `hull_local`, on a belt of link pitch `pitch` (m): the sweep since its last stamp once its
    /// sole has moved more than [`MOVE_TEXELS`] of a texel, or a stamp where it lies once its press
    /// reaches the next step. Subsampling motion keeps sliding prints continuous even when a frame
    /// moves a shoe farther than its own length. Nothing at touchdown: the press starts at none.
    pub(crate) fn observe(
        &mut self,
        shoe: Entity,
        contact: &ShoeContact,
        hull_local: Vec3,
        pitch: f32,
    ) -> Option<ShoeSweep> {
        let Some(grounded) = self.0.get_mut(&shoe) else {
            self.0.insert(
                shoe,
                Grounded {
                    stamped: *contact,
                    frame: *contact,
                    hull_local,
                    travel: 0.0,
                    step: 0,
                },
            );
            return None;
        };
        grounded.travel +=
            grounded.hull_local.distance(hull_local) + contact.motion_from(&grounded.frame);
        grounded.frame = *contact;
        grounded.hull_local = hull_local;
        let step = ((grounded.travel / pitch.max(1e-3)) * f32::from(PRESS_STEPS))
            .floor()
            .min(f32::from(PRESS_STEPS)) as u8;
        if step == 0 {
            grounded.stamped = *contact;
            return None;
        }
        let from = grounded.stamped;
        let motion = contact.motion_from(&from);
        if step == grounded.step && motion <= MOVE_TEXELS * NEAR_TEXEL_M {
            return None;
        }
        grounded.stamped = *contact;
        grounded.step = step;
        Some(ShoeSweep {
            from,
            to: *contact,
            turn: turn(from.yaw, contact.yaw),
            step: 0,
            steps: (motion / (SWEEP_TEXELS * NEAR_TEXEL_M)).ceil().max(1.0) as u32,
            press: f32::from(step) / f32::from(PRESS_STEPS),
        })
    }

    /// An airborne shoe begins a new contact episode, and a new press, when it next touches
    /// down. Never draw a sweep through the air, or along the return run, between those episodes.
    pub(crate) fn airborne(&mut self, shoe: Entity) {
        self.0.remove(&shoe);
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
    /// Rolling without slip: the ground under a shoe never moves, and the belt carries it back
    /// along the hull. Each shoe with its hull-local position.
    fn patch(hull_z: f32, side_x: f32) -> Vec<(u32, Affine3A, Vec3)> {
        let first = ((hull_z - 1.8) / PITCH).ceil() as i32;
        let last = ((hull_z + 1.8) / PITCH).floor() as i32;
        (first..=last)
            .map(|k| {
                let id = (k.rem_euclid(97) as u32) + if side_x < 0.0 { 1000 } else { 0 };
                let z = k as f32 * PITCH;
                (
                    id,
                    lying(Vec2::new(side_x, z), 0.0),
                    Vec3::new(side_x, 0.0, z - hull_z),
                )
            })
            .collect()
    }

    fn entity(id: u32) -> Entity {
        Entity::from_raw_u32(id + 1).expect("a valid index")
    }

    /// Roll a hull `travel` m in frames of `step` m; per side, the stamps laid, and per contact
    /// episode of each link the presses it was stamped at, in order. Shoes leaving the patch go
    /// airborne, as the belt lifts them; a link id comes round again 97 pitches on.
    fn roll(travel: f32, step: f32) -> ([u32; 2], Vec<Vec<f32>>) {
        let mut memory = ShoeMemory::default();
        let mut stamps = [0_u32; 2];
        let mut presses: HashMap<u32, Vec<f32>> = HashMap::default();
        let mut episodes = Vec::new();
        let mut grounded: Vec<u32> = Vec::new();
        for frame in 0..=(travel / step) as u32 {
            let hull_z = frame as f32 * step;
            let mut now = Vec::new();
            for (side, x) in [(0, -1.4), (1, 1.4)] {
                for (id, pose, local) in patch(hull_z, x) {
                    let contact = contact(pose, (LOW, HIGH), 0.0, CONTACT_M, flat)
                        .expect("lies on the ground");
                    now.push(id);
                    if let Some(sweep) = memory.observe(entity(id), &contact, local, PITCH) {
                        for (_, press) in sweep {
                            stamps[side] += 1;
                            presses.entry(id).or_default().push(press);
                        }
                    }
                }
            }
            for id in grounded.iter().filter(|id| !now.contains(id)) {
                memory.airborne(entity(*id));
                episodes.extend(presses.remove(id));
            }
            grounded = now;
        }
        episodes.extend(presses.into_values());
        (stamps, episodes)
    }

    /// ROLLING LAYS ONE PRESS PER LINK. A hull rolls 50 m in 2 cm frames over a belt whose
    /// grounded shoes lie still: every link is stamped [`PRESS_STEPS`] times where it touched
    /// down, as its press deepens, and never again while it lies there, so each side lays
    /// `PRESS_STEPS` stamps per pitch travelled, plus the patch it started on. A rule that stamped
    /// every grounded shoe every frame, or measured motion against the hull rather than the ground,
    /// lays hundreds of times more.
    #[test]
    fn rolling_without_slip_lays_one_press_per_link() {
        let travel = 50.0;
        let (stamps, _) = roll(travel, 0.02);
        let links = ((travel + 3.6) / PITCH).round() as u32;
        let expected = links * u32::from(PRESS_STEPS);
        for count in stamps {
            assert!(
                count.abs_diff(expected) <= 2 * u32::from(PRESS_STEPS),
                "{count} stamps per side, want {PRESS_STEPS} per link: {expected}",
            );
        }
    }

    /// THE PRESS RAMPS MONOTONICALLY OVER ONE PITCH: every link's stamps rise strictly, step by
    /// step, and the last is the full press, reached once the hull has carried the link one pitch.
    /// A ramp that started full is the pop the press exists to remove; one that could fall would
    /// let the store's MAX hide it; one slower than a pitch leaves the front of a moving track
    /// shallow.
    #[test]
    fn the_press_ramps_monotonically_over_one_pitch() {
        let (_, presses) = roll(10.0, 0.01);
        let full: Vec<_> = presses.iter().filter(|p| p.last() == Some(&1.0)).collect();
        assert!(full.len() > 100, "links reach their full press");
        for ramp in &full {
            assert!(
                ramp[0] < 1.0,
                "a link never prints whole at touchdown: {ramp:?}"
            );
            assert!(
                ramp.windows(2).all(|w| w[1] > w[0]),
                "the press only deepens: {ramp:?}"
            );
            assert_eq!(ramp.len(), usize::from(PRESS_STEPS), "{ramp:?}");
        }

        // One link, lying still while the hull drives on: full press exactly one pitch on.
        let mut memory = ShoeMemory::default();
        let shoe = contact(lying(Vec2::ZERO, 0.0), (LOW, HIGH), 0.0, CONTACT_M, flat).unwrap();
        let mut full_at = None;
        for frame in 0..60 {
            let travelled = frame as f32 * 0.005;
            let local = Vec3::new(0.0, 0.0, -travelled);
            if let Some(sweep) = memory.observe(entity(0), &shoe, local, PITCH)
                && sweep.last().is_some_and(|(_, press)| press == 1.0)
            {
                full_at.get_or_insert(travelled);
            }
        }
        let full_at = full_at.expect("the link reaches its full press");
        assert!(
            (PITCH - 1e-4..PITCH + 0.005 + 1e-4).contains(&full_at),
            "full press at {full_at} m of travel, one pitch is {PITCH} m"
        );
    }

    /// The grounded shoes of a hull pivoting on the spot, turned `turn`, each with its hull-local
    /// position: a rigid patch whose shoes slide over the ground about the hull's centre.
    fn pivot_patch(turn: f32) -> Vec<(Affine3A, Vec3)> {
        let mut out = Vec::new();
        for x in [-1.4, 1.4] {
            for k in -13..=13 {
                let offset = Vec2::new(x, k as f32 * PITCH);
                let rotated = Vec2::from_angle(-turn).rotate(offset);
                out.push((lying(rotated, turn), Vec3::new(offset.x, 0.0, offset.y)));
            }
        }
        out
    }

    /// STAMP COUNTS STAY BOUNDED. At speed a link crosses its whole press in a frame or two, and
    /// lays at most [`PRESS_STEPS`] stamps however fast the belt runs; under a pivot every frame's
    /// stamps per shoe are bounded by its sole's motion over the sweep's spacing. A rule that
    /// re-stamped per frame, or per step crossed rather than per step reached, grows with speed.
    #[test]
    fn stamp_counts_stay_bounded_at_speed_and_under_pivot() {
        // 15 m/s at 60 Hz: a quarter metre, two pitches, a frame.
        let travel = 100.0;
        let (stamps, presses) = roll(travel, 0.25);
        let links = ((travel + 3.6) / PITCH).ceil() as u32 + 2;
        for count in stamps {
            assert!(
                count <= links * u32::from(PRESS_STEPS),
                "{count} stamps per side at speed, {links} links touched down"
            );
        }
        assert!(
            presses.iter().all(|p| p.len() <= usize::from(PRESS_STEPS)),
            "no link is pressed more than {PRESS_STEPS} times"
        );

        let mut memory = ShoeMemory::default();
        let spacing = SWEEP_TEXELS * NEAR_TEXEL_M;
        let mut previous: Vec<ShoeContact> = Vec::new();
        for frame in 0..120 {
            let turn = frame as f32 * 0.018; // ASSUMED 1.08 rad/s at 60 Hz.
            let shoes = pivot_patch(turn);
            let now: Vec<ShoeContact> = shoes
                .iter()
                .map(|(pose, _)| contact(*pose, (LOW, HIGH), 0.0, CONTACT_M, flat).unwrap())
                .collect();
            for (id, ((_, local), contact)) in shoes.into_iter().zip(&now).enumerate() {
                let laid = memory
                    .observe(entity(id as u32), contact, local, PITCH)
                    .map_or(0, Iterator::count);
                if let Some(before) = previous.get(id) {
                    let bound = (contact.motion_from(before) / spacing).ceil() as usize + 1;
                    assert!(
                        laid <= bound,
                        "frame {frame}, shoe {id}: {laid} stamps for one frame's slide"
                    );
                }
            }
            previous = now;
        }
    }

    /// A PIVOT STAMPS EVERY GROUNDED SHOE AGAIN. The hull turns on the spot: every grounded shoe
    /// slides across the ground about the hull's centre, so once the turn has carried the slowest
    /// shoe (nearest the centre) past its first press step, every one of them has been stamped
    /// anew. A rule that read only the hull's own travel, or the shoe's position without its turn,
    /// misses the shoes near the pivot.
    #[test]
    fn a_pivot_stamps_every_grounded_shoe_again() {
        let mut memory = ShoeMemory::default();
        for (id, (pose, local)) in pivot_patch(0.0).into_iter().enumerate() {
            let contact = contact(pose, (LOW, HIGH), 0.0, CONTACT_M, flat).expect("grounded");
            assert!(
                memory
                    .observe(entity(id as u32), &contact, local, PITCH)
                    .is_none(),
                "touchdown prints nothing: the press starts at none"
            );
        }
        let mut stamped = vec![false; pivot_patch(0.0).len()];
        let mut turn = 0.0;
        while turn < 0.05 {
            turn += 0.002;
            for (id, (pose, local)) in pivot_patch(turn).into_iter().enumerate() {
                let contact = contact(pose, (LOW, HIGH), 0.0, CONTACT_M, flat).expect("grounded");
                stamped[id] |= memory
                    .observe(entity(id as u32), &contact, local, PITCH)
                    .is_some();
            }
        }
        assert!(
            stamped.iter().all(|&s| s),
            "a 0.05 rad pivot slides every sole into its press; unstamped: {:?}",
            stamped
                .iter()
                .enumerate()
                .filter(|(_, s)| !**s)
                .map(|(i, _)| i)
                .collect::<Vec<_>>(),
        );
    }

    /// Sliding must cover the whole path between frames for the store rasterizer, every sample
    /// within a texel of the last, far inside the sole's own length. A rule emitting only the final
    /// pose leaves gaps once a frame's slide exceeds the shoe length.
    #[test]
    fn fast_sliding_is_sampled_without_gaps() {
        let shoe = entity(0);
        let mut memory = ShoeMemory::default();
        let start = contact(lying(Vec2::ZERO, 0.0), (LOW, HIGH), 0.0, CONTACT_M, flat).unwrap();
        assert!(memory.observe(shoe, &start, Vec3::ZERO, PITCH).is_none());
        let end = contact(
            lying(Vec2::new(0.0, 0.5), 0.2),
            (LOW, HIGH),
            0.0,
            CONTACT_M,
            flat,
        )
        .unwrap();
        let mut previous = start;
        let mut count = 0;
        for (next, press) in memory.observe(shoe, &end, Vec3::ZERO, PITCH).unwrap() {
            assert!(next.motion_from(&previous) <= SWEEP_TEXELS * NEAR_TEXEL_M + 1e-6);
            assert_eq!(press, 1.0, "half a metre of slide is past a pitch of press");
            previous = next;
            count += 1;
        }
        assert!(count > 2, "the intervening path must be stamped");
        assert_eq!(previous.datum, end.datum);
        assert!((previous.yaw - end.yaw).abs() < 1e-6);
    }

    /// Airborne episodes and root snaps must never connect old ground to the next touchdown, and
    /// each touchdown starts a new press. The print consumer needs exactly one initial pose after
    /// either discontinuity.
    #[test]
    fn airborne_and_reset_contacts_never_sweep_from_old_ground() {
        let shoe = entity(0);
        let mut memory = ShoeMemory::default();
        let at = |z| {
            contact(
                lying(Vec2::new(0.0, z), 0.0),
                (LOW, HIGH),
                0.0,
                CONTACT_M,
                flat,
            )
            .unwrap()
        };
        // Pressed in place: the hull drives a pitch on while the link lies at z = 0.
        assert!(memory.observe(shoe, &at(0.0), Vec3::ZERO, PITCH).is_none());
        let pressed = memory.observe(shoe, &at(0.0), Vec3::Z * -PITCH, PITCH);
        assert_eq!(pressed.map(Iterator::count), Some(1));
        memory.airborne(shoe);
        assert!(
            memory.observe(shoe, &at(2.0), Vec3::ZERO, PITCH).is_none(),
            "a new touchdown, a new press"
        );
        let first = memory
            .observe(shoe, &at(2.0), Vec3::Z * -PITCH, PITCH)
            .expect("pressed")
            .collect::<Vec<_>>();
        assert_eq!(
            first.len(),
            1,
            "stamped where it lies, not swept from z = 0"
        );
        assert_eq!(first[0].0.datum, at(2.0).datum);
        memory.reset();
        assert!(
            memory
                .observe(shoe, &at(-20.0), Vec3::ZERO, PITCH)
                .is_none()
        );
        let after = memory
            .observe(shoe, &at(-20.0), Vec3::Z * -PITCH, PITCH)
            .expect("pressed")
            .count();
        assert_eq!(after, 1);
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
