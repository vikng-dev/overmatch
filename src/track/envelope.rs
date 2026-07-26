//! Contact-envelope calibration — the ONE derivation both the sandbox and the game run.
//!
//! The envelope model: the support spring's free length sits at the DROOP ("green") envelope
//! — the suspension's maximum reach, `free_travel` past the rest outer face — and ground
//! intruding into the green→rest band is what carries the hull. The spring rate is NOT
//! authored: it is whatever makes the authored rest pose the flat-ground equilibrium, i.e.
//! `k = weight / A₀`, where `A₀` is the law's own engage-weighted penetration area with the
//! hull parked at its rest height. `A₀` is measured by running [`contact_side`] once at unit
//! stiffness against analytic flat ground, so the calibration shares every branch (columns,
//! engage ramp, profile clipping, directional probes) with the law it calibrates — the
//! equilibrium "green compresses to exactly orange" holds by construction, not by tuning.
//!
//! Damping derives from the ride mode's `2ζ√(k_total·m)`, spread over the rest contact
//! length (`A₀ / travel`, the engaged band's mean-value length).
//!
//! The travel is PER-POSITION, not uniform: [`wheel_travel_knots`] builds the profile (0 at
//! the unsprung sprocket/idler, full droop at every road wheel, linear tapers between), and
//! the law additionally gates it to ground-facing stations — the return run shares the
//! belly's z but has no suspension behind it (see `TravelField` in `forces`).

use bevy::math::{Vec2, Vec3};

use super::forces::{ForceParams, SideInput, SideState, TravelField, contact_side};
use super::oracle::TerrainOracle;

/// The free-travel profile knots for a rest circle set in the rig convention
/// (`circles[0]` = drive sprocket, middle = sprung road wheels, last = idler): 0 at the
/// unsprung ends, `travel` at every road wheel, sorted ascending by z — the piecewise-linear
/// [`TravelField`] then tapers the approach runs and leaves the wrap arcs unsprung.
pub(crate) fn wheel_travel_knots(circles: &[(Vec2, f32)], travel: f32) -> Vec<(f32, f32)> {
    let last = circles.len().saturating_sub(1);
    let mut knots: Vec<(f32, f32)> = circles
        .iter()
        .enumerate()
        .map(|(i, (c, _))| (c.x, if i == 0 || i == last { 0.0 } else { travel }))
        .collect();
    knots.sort_by(|a, b| a.0.total_cmp(&b.0));
    knots
}

/// The derived law: what the calibration hands back for [`ForceParams`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct EnvelopeCalibration {
    /// Rest outer face → green envelope (m): the caller's chain-clamped droop, floored at
    /// [`MIN_ENVELOPE_TRAVEL`].
    pub free_travel: f32,
    /// `weight / A₀` (N/m per metre of contacting belt).
    pub stiffness_per_m: f32,
    /// `2ζ√(k_total·m) · travel / A₀` (N·s/m per metre).
    pub damping_per_m: f32,
}

/// Floor on the envelope travel (m): an infeasible link count clamps the droop to zero, and
/// a zero-travel envelope has no area to calibrate against. The floor keeps the law finite
/// and obviously wrong-feeling (a near-rigid pad) instead of dividing by zero — the link
/// window warning already names the real problem.
pub(crate) const MIN_ENVELOPE_TRAVEL: f32 = 0.01;

/// Flat ground at y = 0 with the oracle's DIRECTIONAL-cast semantics (depth measured along
/// the probe ray, exactly what `BlockField` reports on a flat top face) — the calibration
/// must see the same depths the live field will, or the tilted approach runs would carry a
/// different share of the weight than they were calibrated for.
pub(crate) struct FlatRest;

impl TerrainOracle for FlatRest {
    fn depth_along(&self, station: Vec3, out: Vec3, reach: f32) -> f32 {
        if out.y >= -1e-4 {
            return -reach;
        }
        (station.y / out.y).min(reach)
    }
}

/// One side's loop for calibration: the CLOSED pin-line polyline the force law will actually
/// ride (last point == first), its signed track-plane x, and its free-travel profile knots
/// (see [`wheel_travel_knots`]). A vehicle has exactly two — callers with per-side measured
/// loops pass two different polylines; a caller with one shared loop passes it twice at
/// ± plane_x.
pub(crate) struct EnvelopeSide<'a> {
    pub loop_pts: &'a [Vec2],
    pub plane_x: f32,
    pub knots: &'a [(f32, f32)],
    /// This side's collocation columns ([`SideInput::columns`] — signed hull-x offsets from
    /// the side's pin plane): the calibration must probe the exact columns the live law
    /// rides. On the flat calibration ground the offsets are load-invariant (every column
    /// sees the same depth), but the SHARES weight the per-column engage ramps.
    pub columns: [(f32, f32); 3],
}

/// Calibrate the envelope law. `free_travel` is the caller's chain-clamped droop
/// (`RigGeom::droop_travel(..).effective`); each side's `columns` plus
/// `engage_depth`/`face_offset`/`probe_reach` must be EXACTLY what the live
/// [`ForceParams`]/[`SideInput`] will carry, `count` the material link count — the whole
/// point is that the calibration pass and the live law are the same computation.
pub(crate) fn calibrate(
    sides: &[EnvelopeSide<'_>; 2],
    count: usize,
    face_offset: f32,
    engage_depth: f32,
    probe_reach: f32,
    weight_n: f32,
    free_travel: f32,
    ride_frequency: f32,
    damping_ratio: f32,
) -> EnvelopeCalibration {
    let free_travel = free_travel.max(MIN_ENVELOPE_TRAVEL);
    let probe = ForceParams {
        face_offset,
        free_travel,
        support_stiffness_per_m: 1.0,
        support_damping_per_m: 0.0,
        engage_depth,
        probe_reach,
        mu: 1.0,
        slip_saturation: 1.0,
        max_speed: 1.0,
        engine_power: 0.0,
        engine_force: 0.0,
        governor_gain: 0.0,
        inertia: 1.0,
        grip_stiffness: 0.0,
    };
    // Rest height over flat ground: the loop's pin belly sits `face_offset` above y = 0, so
    // the outer face exactly kisses the surface — the same datum `RigGeom::hull_rest_y`
    // derives for the sandbox spawn.
    let belly_y = sides
        .iter()
        .flat_map(|s| s.loop_pts.iter())
        .map(|p| p.y)
        .fold(f32::INFINITY, f32::min);
    let affine =
        bevy::math::Affine3A::from_translation(Vec3::new(0.0, -belly_y + face_offset, 0.0));
    let mut area = 0.0;
    // Calibration runs support-only (`grip_stiffness: 0.0` above skips the element regime),
    // but the law owns the slab shape — pass a correctly sized scratch field.
    let mut scratch = super::forces::GripElements::for_links(count);
    for side in sides {
        let input = SideInput {
            loop_pts: side.loop_pts,
            count,
            plane_x: side.plane_x,
            columns: side.columns,
            command: 0.0,
            travel: TravelField { knots: side.knots },
        };
        let (report, _) = contact_side(
            &input,
            SideState::default(),
            affine,
            1.0 / 64.0,
            &probe,
            &FlatRest,
            |_| Vec3::ZERO,
            &mut scratch,
        );
        area += report.contacts.iter().map(|c| c.load_elastic).sum::<f32>();
    }
    debug_assert!(
        area > 0.0,
        "the rest pose must intrude into its own droop envelope"
    );
    let area = area.max(1e-6);
    let mass = weight_n / super::derive::G;
    let c_total = super::derive::damping_coefficient(mass, ride_frequency, damping_ratio);
    EnvelopeCalibration {
        free_travel,
        stiffness_per_m: weight_n / area,
        damping_per_m: c_total * free_travel / area,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rectangular pin loop whose belly run is exactly 2 m at `bottom_y`, closed.
    fn rect_loop(bottom_y: f32) -> Vec<Vec2> {
        vec![
            Vec2::new(-1.0, bottom_y),
            Vec2::new(1.0, bottom_y),
            Vec2::new(1.0, bottom_y + 0.5),
            Vec2::new(-1.0, bottom_y + 0.5),
            Vec2::new(-1.0, bottom_y),
        ]
    }

    /// The calibration's defining property, in closed form: on a flat 2 m belly the area at
    /// travel `d` is `d · 2 m` per side, so `k = W / (sides · d · 2)` — and a re-run of the
    /// live law at that `k` carries exactly `W`. Mass-independent of everything but the
    /// authored frequency, exactly like a real spring rate.
    #[test]
    fn the_calibrated_stiffness_makes_the_rest_pose_carry_the_weight() {
        let loop_pts = rect_loop(0.0);
        let weight = 559_170.0_f32; // 57 t
        let travel = 0.1726;
        // Constant profile across the whole belly (end-clamped): the uniform closed form.
        let knots = [(-1.0, travel), (1.0, travel)];
        let columns = [(-0.35, 1.0 / 6.0), (0.0, 2.0 / 3.0), (0.35, 1.0 / 6.0)];
        let sides = [
            EnvelopeSide {
                loop_pts: &loop_pts,
                plane_x: -1.5,
                knots: &knots,
                columns,
            },
            EnvelopeSide {
                loop_pts: &loop_pts,
                plane_x: 1.5,
                knots: &knots,
                columns,
            },
        ];
        let cal = calibrate(&sides, 25, 0.025, 0.002, 0.5, weight, travel, 1.2, 0.35);
        let expected_k = weight / (2.0 * travel * 2.0);
        assert!(
            (cal.stiffness_per_m - expected_k).abs() / expected_k < 1e-3,
            "k {} vs closed-form {}",
            cal.stiffness_per_m,
            expected_k,
        );
        assert_eq!(cal.free_travel, travel);
        assert!(cal.damping_per_m > 0.0);
    }

    /// The degenerate guard: zero droop (infeasible link count) floors at
    /// [`MIN_ENVELOPE_TRAVEL`] instead of dividing by zero.
    #[test]
    fn a_zero_travel_request_floors_instead_of_exploding() {
        let loop_pts = rect_loop(0.0);
        // Real callers floor the travel BEFORE building knots — mirror that here.
        let knots = [(-1.0, MIN_ENVELOPE_TRAVEL), (1.0, MIN_ENVELOPE_TRAVEL)];
        let columns = [(-0.1, 0.25), (0.0, 0.5), (0.1, 0.25)];
        let sides = [-1.5_f32, 1.5].map(|plane_x| EnvelopeSide {
            loop_pts: &loop_pts,
            plane_x,
            knots: &knots,
            columns,
        });
        let cal = calibrate(&sides, 25, 0.025, 0.002, 0.5, 100_000.0, 0.0, 1.2, 0.35);
        assert_eq!(cal.free_travel, MIN_ENVELOPE_TRAVEL);
        assert!(cal.stiffness_per_m.is_finite() && cal.stiffness_per_m > 0.0);
    }
}
