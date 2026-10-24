//! The pen cam's potency colour: a spall field's local intensity at a point, and the colour that
//! intensity takes on the field's ramp ([`Ramp`]). Pure functions of the record's cone and the
//! shipped data: the field's rate (`assets/ballistics/spall_cone.ron`) and the thresholds
//! (`assets/ballistics/box_classes.ron`). The law, its anchors and its promise are design doc §5,
//! "Potency colour" (`.agents/docs/design/armor-penetration-and-damage.md`).
//!
//! Units: a direction's strength `s` in reference-mm; the rate `ρ` in HP points per reference-mm;
//! the emission `e = ρ s / Ω_c` in HP points per steradian, `Ω_c` the cone's solid angle; the
//! intensity `I = e / r²` and the thresholds in HP points per m²; radii in metres; colours linear
//! RGB. Nothing here reads a display gain, a level or a fade: the colour is the dose law's, never
//! the look's.

use std::f32::consts::TAU;

use bevy::prelude::*;

use super::timeline;
use crate::ballistics::box_classes::BoxClasses;
use crate::ballistics::spall_cone;

/// The ramp's lowest anchor, in octaves of intensity below the mid threshold: an intensity this
/// far below it or farther takes the deep teal. TUNED.
const RAMP_DIM_OCTAVES: f32 = 3.0;
/// The volume's rim ([`Rim`]): a march sample's weight is zero at and below the first `u`, the
/// ramp's dim end, and full at and above the second, the mid threshold. TUNED.
const RIM_OCTAVES: (f32, f32) = (-RAMP_DIM_OCTAVES, 0.0);

/// The field's rate and the thresholds, as the shipped data states them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Potency {
    /// `ρ`, HP points per reference-mm of field dose.
    pub rate: f32,
    /// The bright and mid thresholds, HP points per m².
    pub bright: f32,
    pub mid: f32,
}

impl Potency {
    /// The shipped rate and thresholds. Panics on a malformed data file, as its parse does.
    pub fn shipped() -> Self {
        let thresholds = BoxClasses::shipped().potency_thresholds();
        Self {
            rate: spall_cone::hp_per_reference_mm() as f32,
            bright: thresholds.bright,
            mid: thresholds.mid,
        }
    }

    /// The emission `e = ρ s / Ω_c` (HP points per steradian) of a direction of strength
    /// `strength` (reference-mm) in a cone of half-angle `half_angle` (rad): each of the cone's `N`
    /// directions stands for `Ω_c / N` of its solid angle and deposits `s / N`, so `e` is its
    /// dose per steradian.
    pub fn emission(&self, strength: f32, half_angle: f32) -> f32 {
        self.rate * strength / solid_angle(half_angle)
    }

    /// The potency colour's ramp, anchored at these thresholds.
    pub fn ramp(&self) -> Ramp {
        Ramp::new(
            self.mid,
            self.bright,
            [timeline::FIELD_DIM, timeline::FIELD, timeline::FIELD_BRIGHT],
        )
    }

    /// The volume's rim weight, about the mid threshold.
    pub fn rim(&self) -> Rim {
        Rim {
            mid: self.mid,
            zero: RIM_OCTAVES.0,
            full: RIM_OCTAVES.1,
        }
    }
}

/// The solid angle `Ω_c = 2π (1 − cos α) = 4π sin²(α/2)` (sr) of a cone of half-angle
/// `half_angle` (rad), in the second form: the first cancels, and in f32 it is zero below about
/// 2.4·10⁻⁴ rad (DERIVED: `α²/2` under half an ulp of 1, `2⁻²⁵`), while the field law admits
/// cones down to about 10⁻⁸ rad (`spall_cone::FieldDescriptor`).
pub fn solid_angle(half_angle: f32) -> f32 {
    let half = (0.5 * half_angle).sin();
    2.0 * TAU * half * half
}

/// The intensity `e / r²` (HP points per m²) at distance `r` (m) along a direction of emission
/// `emission` (HP points per steradian), inside the direction's reach.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the look's shader takes the intensity at runtime (`pen_cam_look.wgsl`, `volume` and `footprint`); this is the law it mirrors, held by the tests"
    )
)]
pub fn intensity(emission: f32, r: f32) -> f32 {
    emission / (r * r)
}

/// The potency colour: one function of intensity alone. Its parameter is `u = log2(I / T_mid)`,
/// the intensity in octaves about the mid threshold, which the inverse square makes linear in
/// `log r`, so the colour runs evenly along a direction. Three anchors: the deep teal
/// ([`timeline::FIELD_DIM`]) at `u = −`[`RAMP_DIM_OCTAVES`] and below, the field teal
/// ([`timeline::FIELD`]) exactly at the mid threshold (`u = 0`), the near-white
/// ([`timeline::FIELD_BRIGHT`]) exactly at the bright threshold (`u = log2(T_bright / T_mid)`) and
/// above. Between them each channel follows a monotone cubic through the anchors, level at the two
/// ends and at the middle anchor the harmonic mean of its two segments' slopes (zero where they
/// differ in sign), so the colour is smooth (C¹) and, where the anchors climb channel by channel
/// as the shipped ones do, never falls as the intensity rises. `pen_cam_look.wgsl` (`ramp`)
/// evaluates the same curve from [`Ramp::lanes`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ramp {
    /// The mid threshold (HP points per m²), where `u = 0`.
    mid: f32,
    /// Each anchor's `u`, ascending.
    at: [f32; 3],
    /// Each anchor's colour, linear RGB.
    colours: [Vec3; 3],
    /// Each channel's slope (per unit `u`) at the middle anchor.
    slope: Vec3,
}

impl Ramp {
    /// The ramp anchored at the mid threshold `mid` and the bright threshold `bright` (HP points
    /// per m², `0 < mid ≤ bright`), its anchors' colours `colours` (deep, mid, bright).
    pub fn new(mid: f32, bright: f32, colours: [Color; 3]) -> Self {
        let colours = colours.map(|c| LinearRgba::from(c).to_vec3());
        let at = [-RAMP_DIM_OCTAVES, 0.0, (bright / mid).log2().max(0.0)];
        let secant = |k: usize| (colours[k + 1] - colours[k]) / (at[k + 1] - at[k]).max(1e-6);
        let (low, high) = (secant(0), secant(1));
        let slope = Vec3::from_array(std::array::from_fn(|i| {
            let (a, b) = (low[i], high[i]);
            if a * b > 0.0 {
                2.0 * a * b / (a + b)
            } else {
                0.0
            }
        }));
        Self {
            mid,
            at,
            colours,
            slope,
        }
    }

    /// The colour (linear RGB) at intensity `intensity` (HP points per m²); zero takes the deep
    /// anchor.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the look's shader evaluates the ramp at runtime (`pen_cam_look.wgsl`, `ramp`); this is the curve it mirrors, held by the tests"
        )
    )]
    pub fn colour(&self, intensity: f32) -> Vec3 {
        let u = (intensity.max(0.0) / self.mid).log2();
        if u >= self.at[2] {
            return self.colours[2];
        }
        if u <= self.at[0] {
            return self.colours[0];
        }
        let k = usize::from(u >= self.at[1]);
        let span = self.at[k + 1] - self.at[k];
        let t = (u - self.at[k]) / span;
        // The cubic Hermite between the segment's anchors, its end tangents per unit `t`.
        let slopes = [Vec3::ZERO, self.slope, Vec3::ZERO];
        let (t2, t3) = (t * t, t * t * t);
        self.colours[k] * (2.0 * t3 - 3.0 * t2 + 1.0)
            + slopes[k] * span * (t3 - 2.0 * t2 + t)
            + self.colours[k + 1] * (3.0 * t2 - 2.0 * t3)
            + slopes[k + 1] * span * (t3 - t2)
    }

    /// The lanes `pen_cam_look.wgsl` evaluates the ramp from: each anchor's colour (rgb) and `u`
    /// (w), then the middle anchor's slope (rgb) and the mid threshold (w).
    pub fn lanes(&self) -> [Vec4; 4] {
        [
            self.colours[0].extend(self.at[0]),
            self.colours[1].extend(self.at[1]),
            self.colours[2].extend(self.at[2]),
            self.slope.extend(self.mid),
        ]
    }
}

/// A spall field volume's rim: each sample of the march through a volume counts toward its colour
/// and its opacity alike by a weight of its intensity, `smoothstep(zero, full, u)` in
/// `u = log2(I / T_mid)`: zero at and below `zero`, full at and above `full`, a cubic between, so
/// the volume fades out where its intensity falls toward the ramp's dim end instead of ending on
/// its reach surface. `pen_cam_look.wgsl` (`weight`) evaluates the same curve from [`Rim::lanes`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rim {
    /// The mid threshold (HP points per m²), where `u = 0`.
    mid: f32,
    /// The `u` at and below which a sample weighs nothing, and the `u` at and above which it
    /// weighs fully.
    zero: f32,
    full: f32,
}

impl Rim {
    /// A sample's weight at intensity `intensity` (HP points per m²); zero weighs nothing.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the look's shader weighs the march at runtime (`pen_cam_look.wgsl`, `weight`); this is the curve it mirrors, held by the tests"
        )
    )]
    pub fn weight(&self, intensity: f32) -> f32 {
        let u = (intensity.max(0.0) / self.mid).log2();
        let t = ((u - self.zero) / (self.full - self.zero)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// The lane `pen_cam_look.wgsl` weighs the march from: `zero` (x) and `full` (y); the mid
    /// threshold is the ramp's lane.
    pub fn lanes(&self) -> Vec4 {
        Vec4::new(self.zero, self.full, 0.0, 0.0)
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::super::plan::{Plan, tests::records};
    use super::*;
    use crate::damage_cam::shapes::TankShapes;

    /// Every recorded scenario, planned without the tank's shapes (a cone needs none).
    pub(in super::super) fn plans() -> Vec<(String, Plan)> {
        records()
            .into_iter()
            .map(|(name, report)| {
                let plan = Plan::new(&report, &TankShapes::default());
                (name, plan)
            })
            .collect()
    }

    fn linear(c: Color) -> Vec3 {
        LinearRgba::from(c).to_vec3()
    }

    /// The look reads the shipped classes and rate, never its own: bright is the hard class, mid
    /// the crew pool over the face it is set against, the rate the field's calibration. The colour
    /// promise relies on it; a threshold or rate authored in the look breaks it.
    #[test]
    fn the_thresholds_read_the_shipped_classes() {
        use crate::ballistics::box_classes::BoxClass;
        let classes = BoxClasses::shipped();
        let potency = Potency::shipped();
        assert_eq!(
            potency.bright,
            classes.hp_per_square_metre(BoxClass::Hard) as f32
        );
        assert_eq!(potency.mid, classes.potency_thresholds().mid);
        assert!(potency.mid > 0.0 && potency.mid <= potency.bright);
        assert_eq!(potency.rate, spall_cone::hp_per_reference_mm() as f32);
    }

    /// The ramp is anchored at the thresholds it is built from: the field teal exactly at mid, the
    /// near-white exactly at bright and above, the deep teal from `RAMP_DIM_OCTAVES` below mid
    /// down to zero; over the shipped thresholds and over a fixture pair apart from them. "White
    /// is bright, teal is mid" relies on it; a constant colour, or an anchor at a hard-coded
    /// threshold, fails here.
    #[test]
    fn the_ramp_is_anchored_at_the_thresholds() {
        let fixture = Potency {
            rate: 70.0,
            bright: 410.0,
            mid: 95.0,
        };
        for potency in [Potency::shipped(), fixture] {
            let ramp = potency.ramp();
            let deep = potency.mid * (-RAMP_DIM_OCTAVES).exp2();
            for (intensity, want) in [
                (potency.mid, timeline::FIELD),
                (potency.bright, timeline::FIELD_BRIGHT),
                (potency.bright * 7.0, timeline::FIELD_BRIGHT),
                (deep, timeline::FIELD_DIM),
                (deep * 0.1, timeline::FIELD_DIM),
                (0.0, timeline::FIELD_DIM),
            ] {
                let got = ramp.colour(intensity);
                assert!(
                    got.distance(linear(want)) < 1e-5,
                    "{potency:?}: I = {intensity}: {got} against {want:?}"
                );
            }
            for intensity in [potency.mid * 0.5, (potency.mid * potency.bright).sqrt()] {
                let got = ramp.colour(intensity);
                for anchor in [timeline::FIELD_DIM, timeline::FIELD, timeline::FIELD_BRIGHT] {
                    assert!(got.distance(linear(anchor)) > 1e-3, "I = {intensity}");
                }
            }
        }
    }

    /// The ramp never falls as the intensity rises and never jumps: over a fine sweep in `u` from
    /// past the deep anchor to past the bright one, no channel falls, the luminance climbs from
    /// anchor to anchor, and no step changes any channel by more than a bound on its slope. The
    /// colour reads as potency in one direction only and as a gradient, never as bands; a band
    /// (step) mutant, a reversed anchor or an overshooting tangent fails here.
    #[test]
    fn the_ramp_is_smooth_and_monotone() {
        let potency = Potency::shipped();
        let ramp = potency.ramp();
        let luminance = |c: Vec3| c.dot(Vec3::new(0.2126, 0.7152, 0.0722));
        let steps = 4000;
        let (from, to) = (-RAMP_DIM_OCTAVES - 1.0, ramp.at[2] + 1.0);
        let du = (to - from) / steps as f32;
        // Steepest secant between neighbouring anchors, per unit u; a monotone cubic's slope stays
        // within three times it (Fritsch–Carlson).
        let steepest = (0..2)
            .map(|k| {
                ((ramp.colours[k + 1] - ramp.colours[k]) / (ramp.at[k + 1] - ramp.at[k]))
                    .max_element()
            })
            .fold(0.0, f32::max);
        let mut previous = ramp.colour(potency.mid * from.exp2());
        for k in 1..=steps {
            let u = from + du * k as f32;
            let here = ramp.colour(potency.mid * u.exp2());
            let change = here - previous;
            assert!(
                change.min_element() >= -1e-6,
                "u = {u}: {previous} to {here}"
            );
            assert!(
                change.max_element() <= 3.0 * steepest * du + 1e-5,
                "u = {u}: a jump of {change}"
            );
            previous = here;
        }
        let [deep, mid, bright] = [timeline::FIELD_DIM, timeline::FIELD, timeline::FIELD_BRIGHT]
            .map(|c| luminance(linear(c)));
        assert!(deep < mid && mid < bright, "{deep} {mid} {bright}");
    }

    /// The lanes the shader evaluates the ramp from are the ramp's: its anchors' colours and `u`,
    /// the middle slope and the mid threshold, nothing display-side. The shader's ramp parts from
    /// the law's if a lane is packed out of order.
    #[test]
    fn the_ramp_lanes_are_its_anchors() {
        let potency = Potency::shipped();
        let ramp = potency.ramp();
        let lanes = ramp.lanes();
        for (k, colour) in [timeline::FIELD_DIM, timeline::FIELD, timeline::FIELD_BRIGHT]
            .into_iter()
            .enumerate()
        {
            assert_eq!(lanes[k].truncate(), linear(colour));
        }
        assert_eq!(lanes[1].w, 0.0);
        assert_eq!(lanes[2].w, (potency.bright / potency.mid).log2());
        assert_eq!(lanes[3].w, potency.mid);
    }

    /// The rim weighs nothing at and below the ramp's dim end and fully at and above the mid
    /// threshold, rising smoothly between: over a fine sweep in `u` the weight never falls and no
    /// step climbs faster than the cubic's steepest slope, `1.5` over the span, and a quarter of
    /// the way it is the cubic's `5/32` (DERIVED, `t²(3 − 2t)` at `t = 1/4`, as WGSL's
    /// `smoothstep` is). The volume's core, at mid and above, keeps its whole opacity, and its
    /// outer stretch fades to nothing exactly where the colour reaches the deep teal; a weight
    /// anchored apart from the ramp, a step, a weight that falls, or a curve other than the
    /// shader's fails here. Its lane carries the anchors alone.
    #[test]
    fn the_rim_fades_from_the_dim_end_to_mid() {
        let potency = Potency::shipped();
        let (ramp, rim) = (potency.ramp(), potency.rim());
        assert_eq!((rim.zero, rim.full), (ramp.at[0], ramp.at[1]));
        assert_eq!(rim.mid, potency.mid);
        assert_eq!(rim.lanes(), Vec4::new(ramp.at[0], ramp.at[1], 0.0, 0.0));
        let at = |u: f32| rim.weight(potency.mid * u.exp2());
        for u in [rim.zero, rim.zero - 0.5, rim.zero - 9.0] {
            assert_eq!(at(u), 0.0, "u = {u}");
        }
        assert_eq!(rim.weight(0.0), 0.0);
        for u in [rim.full, rim.full + 0.5, ramp.at[2] + 9.0] {
            assert_eq!(at(u), 1.0, "u = {u}");
        }
        let steps = 4000;
        let (from, to) = (rim.zero - 1.0, rim.full + 1.0);
        let du = (to - from) / steps as f32;
        let steepest = 1.5 / (rim.full - rim.zero);
        let mut previous = at(from);
        for k in 1..=steps {
            let u = from + du * k as f32;
            let here = at(u);
            assert!(here >= previous - 1e-6, "u = {u}: {previous} to {here}");
            assert!(
                here - previous <= steepest * du + 1e-5,
                "u = {u}: a jump of {}",
                here - previous
            );
            previous = here;
        }
        let between = at(0.5 * (rim.zero + rim.full));
        assert!(between > 0.0 && between < 1.0, "{between}");
        let quarter = at(rim.zero + 0.25 * (rim.full - rim.zero));
        assert!((quarter - 5.0 / 32.0).abs() < 1e-5, "{quarter}");
    }
}
