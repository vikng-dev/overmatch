//! The pen cam's potency bands: a field's local intensity at a point, and the band that intensity
//! falls in ([`Band`]). Pure functions of the record's field (a spall cone or a burst's) and the
//! shipped data: the field's rate (`assets/ballistics/spall_cone.ron`), the crew pool and face
//! (`assets/ballistics/box_classes.ron`) and the sim's HP rounding. The law, its thresholds and its
//! promise are design doc §5, "Potency colour" (`.agents/docs/design/armor-penetration-and-damage.md`).
//!
//! Units: a direction's strength `s` in reference-mm; the rate `ρ` in HP points per reference-mm;
//! the emission `e = ρ s / Ω` in HP points per steradian, `Ω` the solid angle of the field's
//! support (a cap, a ring's band or the whole sphere); the intensity `I = e / r²` and the
//! thresholds in HP points per m²; radii in metres. Nothing here reads a display gain, a level or a
//! fade: the band is the dose law's, never the look's.

use std::f32::consts::TAU;

use bevy::prelude::*;

use super::timeline;
use crate::ballistics::box_classes::BoxClasses;
use crate::ballistics::spall_cone;

/// The field's rate and the band thresholds, as the shipped data states them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Potency {
    /// `ρ`, HP points per reference-mm of field dose.
    pub rate: f32,
    /// The floor and mid thresholds, HP points per m² ([`Band`]).
    pub floor: f32,
    pub mid: f32,
}

/// Where an intensity falls, for a crew station presenting its face wholly at it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Band {
    /// Below the floor: the dose its face takes rounds to no HP point.
    Nothing,
    /// From the floor up to mid: its pool is dosed, and usually not emptied; from `mid − floor` up
    /// its face's total rounds to the whole pool.
    Reduced,
    /// At or above mid: its HP pool is emptied.
    Knockout,
}

impl Band {
    /// The band's colour, in the field's teal family: the field teal for knockout, the deep teal
    /// for reduced, none for nothing.
    pub fn colour(self) -> Option<Color> {
        match self {
            Band::Nothing => None,
            Band::Reduced => Some(timeline::FIELD_DIM),
            Band::Knockout => Some(timeline::FIELD),
        }
    }
}

impl Potency {
    /// The shipped rate and thresholds. Panics on a malformed data file, as its parse does.
    pub fn shipped() -> Self {
        Self::of(
            BoxClasses::shipped(),
            spall_cone::hp_per_reference_mm() as f32,
        )
    }

    /// The thresholds `classes` part the bands at, and the field rate `rate`.
    pub fn of(classes: &BoxClasses, rate: f32) -> Self {
        let thresholds = classes.potency_thresholds();
        Self {
            rate,
            floor: thresholds.floor,
            mid: thresholds.mid,
        }
    }

    /// The emission `e = ρ s / Ω` (HP points per steradian) of a direction of strength `strength`
    /// (reference-mm) in a field whose support lies from `inner` to `half_angle` (rad) off its
    /// axis, `Ω = Ω(half_angle) − Ω(inner)` ([`solid_angle`]; a cap's and the sphere's `inner` is
    /// 0, whose solid angle is exactly 0): each of the field's `N` directions stands for `Ω / N` of
    /// its support and deposits `s / N`, so `e` is its dose per steradian.
    pub fn emission(&self, strength: f32, inner: f32, half_angle: f32) -> f32 {
        self.rate * strength / (solid_angle(half_angle) - solid_angle(inner))
    }

    /// The band intensity `intensity` (HP points per m²) falls in.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the look's shader parts the bands at runtime (`pen_cam_look.wgsl`, `step_over`); this is the law it applies, held by the tests"
        )
    )]
    pub fn band(&self, intensity: f32) -> Band {
        if intensity >= self.mid {
            Band::Knockout
        } else if intensity >= self.floor {
            Band::Reduced
        } else {
            Band::Nothing
        }
    }

    /// The lane `pen_cam_look.wgsl` parts the bands from: the floor (x) and mid (y) thresholds.
    pub fn lanes(&self) -> Vec4 {
        Vec4::new(self.floor, self.mid, 0.0, 0.0)
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
        reason = "the look's shader takes the intensity at runtime (`pen_cam_look.wgsl`, `field_at`); this is the law the sim's dose is checked against (`the_pen_cam_intensity_times_a_face_is_its_dose`)"
    )
)]
pub fn intensity(emission: f32, r: f32) -> f32 {
    emission / (r * r)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture classes: 30 HP set against 0.25 m², the classes apart from the shipped ones.
    fn fixture_classes() -> BoxClasses {
        BoxClasses::from_ron(
            r#"(crew_hp: (30, Tuned("fixture")), crew_face_m2: (0.25, Tuned("fixture")),
                ammo: (200, Tuned("fixture")), hard: (300, Tuned("fixture")))"#,
        )
        .unwrap()
    }

    /// The bands part at the classes' thresholds, never the look's own: mid the crew pool over its
    /// face, the floor the sim's least counted share of an HP point over the same face (fixture:
    /// 120 and 2 HP points per m²; shipped: 150 and 2.5, DERIVED), the rate the field's
    /// calibration. The band promise relies on it; a threshold authored in the look (the shipped
    /// 150 hard-coded), a floor dropped to zero, or one read as a whole HP point fails here.
    #[test]
    fn the_thresholds_read_the_classes() {
        let fixture = Potency::of(&fixture_classes(), 70.0);
        assert_eq!((fixture.floor, fixture.mid), (2.0, 120.0));
        let shipped = Potency::shipped();
        let classes = BoxClasses::shipped();
        assert_eq!(shipped.mid, classes.potency_thresholds().mid);
        assert_eq!(shipped.floor, classes.potency_thresholds().floor);
        let share = crate::ballistics::least_counted_hp() as f32 / classes.crew_hp() as f32;
        assert!((shipped.floor / shipped.mid - share).abs() < 1e-6 * share);
        assert!(shipped.floor > 0.0 && shipped.floor < shipped.mid);
        assert_eq!(shipped.rate, spall_cone::hp_per_reference_mm() as f32);
        assert_eq!(
            shipped.lanes(),
            Vec4::new(shipped.floor, shipped.mid, 0.0, 0.0)
        );
    }

    /// Three bands, each from its threshold up: nothing below the floor, reduced from the floor
    /// to mid, knockout at mid and above, over the shipped thresholds and the fixture's. Nothing
    /// takes no colour; the two bands take the field's two teals, apart in luminance so they part
    /// in every colour vision type, and neither is on the state scale's red. A dropped floor, a
    /// band boundary off its threshold, or one colour for both bands fails here.
    #[test]
    fn three_bands_part_at_the_thresholds() {
        for potency in [Potency::shipped(), Potency::of(&fixture_classes(), 70.0)] {
            let (floor, mid) = (potency.floor, potency.mid);
            for (intensity, want) in [
                (0.0, Band::Nothing),
                (floor * 0.999, Band::Nothing),
                (floor, Band::Reduced),
                (mid * 0.999, Band::Reduced),
                (mid, Band::Knockout),
                (mid * 1.0e3, Band::Knockout),
            ] {
                assert_eq!(
                    potency.band(intensity),
                    want,
                    "{potency:?}: I = {intensity}"
                );
            }
        }
        assert_eq!(Band::Nothing.colour(), None);
        let luminance = |c: Color| {
            LinearRgba::from(c)
                .to_vec3()
                .dot(Vec3::new(0.2126, 0.7152, 0.0722))
        };
        let (reduced, knockout) = (
            Band::Reduced.colour().unwrap(),
            Band::Knockout.colour().unwrap(),
        );
        assert!(luminance(knockout) > 2.0 * luminance(reduced));
        for colour in [reduced, knockout] {
            let c = LinearRgba::from(colour).to_vec3();
            assert!(c.z > c.x && c.y > c.x, "{c}");
        }
    }
}
