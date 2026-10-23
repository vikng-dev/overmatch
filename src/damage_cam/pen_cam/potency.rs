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
}

/// The solid angle `Ω_c = 2π (1 − cos α)` (sr) of a cone of half-angle `half_angle` (rad).
pub fn solid_angle(half_angle: f32) -> f32 {
    TAU * (1.0 - half_angle.cos())
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
}
