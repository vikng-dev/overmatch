//! Running-gear phase from belt travel and authored contact/tooth datums.
//!
//! Road wheels roll at their declared tread radius; wrapped idlers follow their pin circle.
//! Sprockets turn one tooth per material link and seat the reference tooth declared in the tank
//! sheet at half a tooth beyond the route origin. No view mesh can change those datums.

use bevy::math::Affine3A;
use bevy::prelude::*;

// ---------------------------------------------------------------------------------------------
// The phase law
// ---------------------------------------------------------------------------------------------

/// Belt travel → axle angle for a wheel the belt ROLLS on, wrapped per revolution in `f64` before
/// the `f32` cast so a long session's accumulated travel never erodes the angle's precision.
///
/// The sign is the one place the rotation convention lives, and it is forced: positive phase scrolls
/// the belly toward `+z` (the loop is ordered sprocket → belly → idler), so a wheel's contact point
/// must travel `+z`, and a positive rotation about `+x` moves the bottom of a wheel toward `−z`.
/// Hence negative. The load gate certifies the finite positive radius.
pub(crate) fn spin_angle(travel: f64, radius: f32) -> f32 {
    assert!(
        radius.is_finite() && radius > 0.0,
        "the load gate certifies a finite positive rolling radius"
    );
    let circumference = f64::from(radius) * std::f64::consts::TAU;
    -(travel.rem_euclid(circumference) / f64::from(radius)) as f32
}

/// Where a tooth TIP must sit at belt travel `travel`: the side-plane angle (rad) of the tip that
/// bisects the pin pair straddling the loop's arc-length origin.
///
/// Two terms and no fitting: `origin` (the angle zero phase puts pin 0 at) `+ ½ tooth` (the rule
/// itself) `+` one tooth per pitch of travel (the proven rate lock), wrapped per full REVOLUTION of
/// the sprocket (`teeth × pitch` metres of belt) in `f64`. Increasing, because positive travel
/// scrolls the belt forward around the sprocket's front, i.e. counter-clockwise in the `(z, y)`
/// side plane the angle is measured in.
pub(crate) fn tooth_tip_angle(travel: f64, pitch: f32, teeth: u32, origin: f32) -> f32 {
    let tooth = std::f32::consts::TAU / teeth as f32;
    let per_revolution = f64::from(pitch) * f64::from(teeth);
    let turn = (travel.rem_euclid(per_revolution) / per_revolution * std::f64::consts::TAU) as f32;
    origin + tooth / 2.0 + turn
}

/// Belt travel → sprocket node spin about the hull's lateral axis (rad): **put this mesh's own tooth
/// tip where the rule wants a tip**.
///
/// `mesh_tip` is where the mesh carries a tip at zero spin (the tank sheet); the target
/// is [`tooth_tip_angle`]. A positive spin about the hull's `+x` DECREASES a side-plane angle, so
/// the difference is taken tip-minus-target — the same flip [`spin_angle`] carries.
///
/// The tooth-per-pitch RATE is untouched by the calibration: only the target moves with travel, and
/// it moves by exactly one tooth per pitch, so the spin does too. Any representative of `mesh_tip`
/// modulo one tooth is as good as any other — which is precisely what makes a single measured
/// constant enough to seat all of them. The load gate certifies pitch and tooth count.
pub(crate) fn tooth_angle(travel: f64, pitch: f32, teeth: u32, origin: f32, mesh_tip: f32) -> f32 {
    assert!(
        pitch.is_finite() && pitch > 0.0 && teeth >= 3,
        "the load gate certifies positive pitch and a sprocket pin polygon"
    );
    mesh_tip - tooth_tip_angle(travel, pitch, teeth, origin)
}

/// Reduce an angle to the signed representative nearest zero within one `period` — "how far off is
/// it, and which way", for readouts and for assertions about a periodic quantity.
#[cfg(test)]
pub(crate) fn fold(angle: f32, period: f32) -> f32 {
    (angle + period / 2.0).rem_euclid(period) - period / 2.0
}

// ---------------------------------------------------------------------------------------------
// Posing a bound node
// ---------------------------------------------------------------------------------------------

/// The node's local transform for `dy` metres of travel along `up` and `angle` of spin about `axle`.
///
/// Two things are load-bearing. The spin PRE-multiplies the authored rotation — that applies it in
/// the node's PARENT space, so it is a rotation about the hull's lateral axis whatever the node's
/// own baked orientation is; post-multiplying would spin about the node's own `x`, which a baked
/// 180° Y flip silently reverses (one track's wheels turning backwards). And the translation is
/// untouched by the spin, which is what makes this a rotation about the NODE ORIGIN — the
/// hand-corrected true axle (bake invariant `rotating_nodes_carry_their_own_axle_origin`) — rather
/// than about the model origin. The authored scale is carried, never assumed.
pub(crate) fn gear_transform(
    rest: &Transform,
    up: Vec3,
    axle: Vec3,
    dy: f32,
    angle: f32,
) -> Transform {
    Transform {
        translation: rest.translation + up * dy,
        rotation: Quat::from_axis_angle(axle, angle) * rest.rotation,
        scale: rest.scale,
    }
}

/// A checked parent-space pose for a rigid running-gear node. Parent scale must be uniform:
/// otherwise a hull-axis rotation needs shear, which a local `Transform` cannot represent.
#[derive(Clone, Copy)]
pub(crate) struct GearFrame {
    rest: Transform,
    up: Vec3,
    axle: Vec3,
}

#[derive(Debug)]
pub(crate) struct GearFrameFault;

impl std::fmt::Display for GearFrameFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("running-gear parent axes must be finite, orthogonal and uniformly scaled; bake parent scale/shear into the mesh before export")
    }
}

impl GearFrame {
    pub(crate) fn new(rest: Transform, hull_from_parent: Affine3A) -> Result<Self, GearFrameFault> {
        let axes = [Vec3::X, Vec3::Y, Vec3::Z].map(|axis| hull_from_parent.transform_vector3(axis));
        let scale2 = axes[0].length_squared();
        // ASSUMED f32 representation tolerance for composed uniform parent frames.
        let tolerance = 128.0 * f32::EPSILON * scale2;
        if !scale2.is_finite()
            || scale2 <= 0.0
            || axes
                .iter()
                .any(|axis| !axis.is_finite() || (axis.length_squared() - scale2).abs() > tolerance)
            || axes[0].dot(axes[1]).abs() > tolerance
            || axes[0].dot(axes[2]).abs() > tolerance
            || axes[1].dot(axes[2]).abs() > tolerance
        {
            return Err(GearFrameFault);
        }
        let parent_from_hull = hull_from_parent.inverse();
        let handedness = axes[0].cross(axes[1]).dot(axes[2]).signum();
        let axle = parent_from_hull.transform_vector3(Vec3::X) * handedness;
        Ok(Self {
            rest,
            up: parent_from_hull.transform_vector3(Vec3::Y),
            axle: axle.try_normalize().ok_or(GearFrameFault)?,
        })
    }

    pub(crate) fn rest(&self) -> Transform {
        self.rest
    }

    pub(crate) fn pose(&self, dy: f32, angle: f32) -> Transform {
        gear_transform(&self.rest, self.up, self.axle, dy, angle)
    }
}

/// Compose the authored parent chain before transform propagation. `None` names a node outside
/// this hull; a linked ancestor without its authored transform is a construction bug.
pub(crate) fn hull_from_parent(
    node: Entity,
    hull: Entity,
    parents: &Query<&ChildOf>,
    transforms: &Query<&Transform>,
) -> Option<Affine3A> {
    let mut chain = Vec::new();
    let mut current = parents.get(node).ok()?.parent();
    while current != hull {
        chain.push(current);
        current = parents.get(current).ok()?.parent();
    }
    Some(chain.iter().rev().fold(Affine3A::IDENTITY, |frame, &entity| {
        frame * transforms.get(entity).expect("every glTF ancestor in the hull hierarchy keeps its authored Transform").compute_affine()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Tiger's authored counts, as the export carries them. INPUTS to the pure math below, not
    /// assertions about the file — a re-authoring that changes them must not turn this suite red.
    const TEETH: u32 = 20;
    const PITCH: f32 = 0.130;
    /// A belt origin and a mesh tooth phase that are both deliberately NOT round numbers, so a
    /// calibration term that silently dropped out of the rate math would show up here.
    const ORIGIN: f32 = 1.5199;
    const MESH_TIP: f32 = 0.0163;

    /// The game and sandbox pose wheels in their actual parent frame. The hull-space rim
    /// motion must keep its direction through a flipped node and rotated/scaled parent; a
    /// post-multiplied spin or a parent-local X axis breaks this invariant.
    #[test]
    fn a_bound_wheel_rotates_about_the_hull_axis_in_its_parent_frame() {
        let parent = Transform {
            translation: Vec3::new(0.3, 0.2, -0.4),
            rotation: Quat::from_rotation_z(0.4),
            scale: Vec3::splat(0.8),
        }
        .compute_affine();
        let rest = Transform {
            translation: Vec3::new(1.0, 0.5, 0.0),
            rotation: Quat::from_rotation_y(std::f32::consts::PI),
            scale: Vec3::splat(0.7),
        };
        let frame = GearFrame::new(rest, parent).expect("uniform fixture parent frame");
        let (dy, angle) = (0.045, -0.7); // Fixture travel (m) and spin (rad).
        let point = Vec3::new(0.0, -0.4, 0.0);
        let axle = parent.transform_point3(rest.translation);
        let before = parent.transform_point3(rest.transform_point(point));
        let expected = axle + Quat::from_rotation_x(angle) * (before - axle) + Vec3::Y * dy;
        let after = parent.transform_point3(frame.pose(dy, angle).transform_point(point));
        assert!((after - expected).length() < 1e-5);
        assert!(
            GearFrame::new(
                rest,
                Transform::from_scale(Vec3::new(1.0, 2.0, 1.0)).compute_affine()
            )
            .is_err()
        );
    }

    /// The rolling wheels turn the way a wheel turns: driving forward scrolls the belly toward `+z`,
    /// so the wheel's CONTACT POINT must travel `+z` with it, and the arc it sweeps must equal the
    /// belt travel exactly — that is what "no visible slip on flat ground" means numerically.
    #[test]
    fn a_rolling_wheel_matches_the_ground_it_rolls_on() {
        let radius = 0.41;
        let travel = 0.37;
        let angle = spin_angle(travel as f64, radius);
        assert!(
            angle < 0.0,
            "positive travel must turn the wheel negatively"
        );
        assert!(
            ((-angle * radius) - travel).abs() < 1e-5,
            "the swept arc must equal the belt travel",
        );
        // The bottom of the wheel really does move toward +z under that rotation.
        let bottom = Quat::from_rotation_x(angle) * Vec3::new(0.0, -radius, 0.0);
        assert!(bottom.z > 0.0, "the contact point went the wrong way");
        // And travel that wraps many revolutions still lands on the same angle as the wrapped one.
        let laps = travel as f64 + 7.0 * f64::from(radius) * std::f64::consts::TAU;
        assert!((spin_angle(laps, radius) - angle).abs() < 1e-4);
    }

    /// The composition rule, against the two things the export has actually shipped: a baked 180° Y
    /// rotation and a non-unit scale. Travel must stay hull-vertical, spin must stay about the
    /// HULL's lateral axis (not the node's flipped one), the scale must survive, and the node ORIGIN
    /// — the hand-corrected axle — must not move when only the spin changes.
    #[test]
    fn a_flipped_scaled_node_still_travels_up_and_spins_about_the_axle() {
        let rest = Transform {
            translation: Vec3::new(1.67, 0.51, -1.88),
            rotation: Quat::from_rotation_y(std::f32::consts::PI),
            scale: Vec3::splat(0.8),
        };
        let angle = -0.7;
        let posed = gear_transform(&rest, Vec3::Y, Vec3::X, 0.045, angle);

        assert_eq!(posed.scale, rest.scale, "the authored scale must survive");
        assert_eq!(
            posed.translation,
            rest.translation + Vec3::Y * 0.045,
            "travel is hull-vertical and nothing else",
        );
        // The node origin (the axle) is exactly the translation — spin does not move it.
        let spun_only = gear_transform(&rest, Vec3::Y, Vec3::X, 0.045, angle * 3.0);
        assert_eq!(spun_only.translation, posed.translation);
        // The game's hull-fixed entry point carries the captured REST translation verbatim:
        // overwriting it drags the node to the tank origin at phase zero.
        assert_eq!(
            gear_transform(&rest, Vec3::ZERO, Vec3::X, 0.0, angle).translation,
            rest.translation
        );

        // The visible rotation is a pure `angle` about the hull's +x, composed onto the rest pose —
        // NOT about the node's own (flipped) x. Post-multiplying would give the opposite sense.
        let want = Quat::from_rotation_x(angle) * rest.rotation;
        assert!(posed.rotation.angle_between(want) < 1e-5);
        let wrong = rest.rotation * Quat::from_rotation_x(angle);
        assert!(
            posed.rotation.angle_between(wrong) > 1.0,
            "the flip makes the two conventions genuinely different - that is the point",
        );
        // Concretely: a point on the rim still sweeps the right way in hull space.
        let rim = |t: &Transform| t.transform_point(Vec3::new(0.0, -0.4, 0.0)) - t.translation;
        assert!(
            rim(&posed).z > rim(&rest).z,
            "the bottom of the wheel must move toward +z",
        );
    }

    /// at exactly half a tooth.
    #[test]
    fn a_pin_lands_in_a_gullet_at_every_phase() {
        let tooth = std::f32::consts::TAU / TEETH as f32;
        for links in [
            0.0_f64, 0.25, 0.5, 1.0, 1.5, 7.0, 19.5, 20.0, 41.0, -1.0, -13.75, -400.0, 5_000.0,
        ] {
            let travel = links * f64::from(PITCH);
            // Where the belt puts its pins on the sprocket, from the belt's own facts alone.
            let pin = ORIGIN + (links as f32) * tooth;
            // Where the mesh's teeth end up, from the spin this module writes. A spin of `θ` about
            // the hull's `+x` takes a side-plane angle to `angle − θ`.
            let tip = MESH_TIP - tooth_angle(travel, PITCH, TEETH, ORIGIN, MESH_TIP);
            let offset = fold(tip - pin, tooth);
            assert!(
                (offset.abs() - tooth / 2.0).abs() < 1e-4,
                "at {links} links the nearest tip sits {:.4}° from a pin - it must be exactly \
                 {:.4}° (half a tooth) for the pin to seat in a gullet",
                offset.to_degrees(),
                tooth.to_degrees() / 2.0,
            );
            // Said the other way round, because it is the way you check it on screen: the GULLET
            // (half a tooth off a tip) is where the pin is, to within a rounding.
            let gullet = tip + tooth / 2.0;
            assert!(fold(gullet - pin, tooth).abs() < 1e-4);
        }
    }
}
