//! The pen cam's potency bands: a field's local intensity at a point, and the band that intensity
//! falls in ([`Band`]). Pure functions of the record's field (a spall cone or a burst's), the
//! shipped data (the energy rate every dose converts at, `assets/ballistics/energy_dose.ron`; the
//! crew pool, `assets/ballistics/box_classes.ron`), the display's whole point
//! (`ballistics::least_shown_hp`), and the struck tank's crew face.
//! The law, its thresholds and its promise are design doc §5, "Potency colour"
//! (`.agents/docs/design/armor-penetration-and-damage.md`).
//!
//! The crew face is the struck tank's own: the mean face of its crew boxes at rest, read from its
//! sim geometry ([`TankShapes::crew_face`]), the one place a crew box's size is stated; no data
//! file authors a face and nothing corrects one. So a potency belongs to a tank, not to the game:
//! a plan carries its struck tank's ([`Potency::of_tank`], `Plan::potency`), the look parts its
//! bands at the thresholds the plan hands it, and a plan whose tank has no crew station has no
//! potency and draws no field.
//!
//! Units: a direction's strength `s` in joules, the record's own; the rate `ρ` in HP points per
//! joule, the one energy rate's inverse, so the bands read the strength the sim's dose reads;
//! the emission `e = ρ s / Ω` in HP points per steradian, `Ω` the solid angle of the field's
//! support (a cap, a ring's band or the whole sphere); the intensity `I = e / r²` and the
//! thresholds in HP points per m²; radii in metres. Nothing here reads a display gain, a level or a
//! fade: the band is the dose law's, never the look's.

use std::f32::consts::TAU;

use bevy::prelude::*;

use super::timeline;
use crate::ballistics::box_classes::BoxClasses;
use crate::ballistics::energy_dose;
use crate::damage_cam::shapes::TankShapes;

/// The rate a field's strength converts at and a tank's band thresholds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Potency {
    /// `ρ`, HP points per joule of field dose: `1 / joules_per_hp`, the rate the sim converts every
    /// field deposit at.
    pub rate: f32,
    /// The floor and mid thresholds, HP points per m² ([`Band`]).
    pub floor: f32,
    pub mid: f32,
}

/// Where an intensity falls, for a crew station presenting its face wholly at it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Band {
    /// Below the floor: a full pool keeps the dose its face takes and shows no lost point.
    Nothing,
    /// From the floor up to mid: a full pool shows a lost point and is not emptied, short of the
    /// last half pool unit below mid, which converts to the whole pool
    /// (`ballistics::pool_units_of`).
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
    /// The potency of the tank `shapes` holds: the shipped crew pool and energy rate over its crew
    /// face ([`TankShapes::crew_face`]); `None` for a tank with no crew station, whose bands would
    /// promise nothing. Panics on a malformed data file, as its parse does.
    pub fn of_tank(shapes: &TankShapes) -> Option<Self> {
        shapes.crew_face().map(Self::of_crew_face)
    }

    /// The shipped crew pool and energy rate over crew stations of mean face `crew_face` (m²,
    /// positive and finite).
    pub fn of_crew_face(crew_face: f32) -> Self {
        Self::of(BoxClasses::shipped(), crew_face, hp_per_joule())
    }

    /// The thresholds `classes` part the bands at over crew stations of mean face `crew_face` (m²),
    /// and the field rate `rate`.
    pub(crate) fn of(classes: &BoxClasses, crew_face: f32, rate: f32) -> Self {
        let thresholds = classes.potency_thresholds(crew_face);
        Self {
            rate,
            floor: thresholds.floor,
            mid: thresholds.mid,
        }
    }

    /// The bound tank's potency (`tank::TANK_SPEC_PATH`, the fixture tank), the one every plan of
    /// its records carries: for the tests that read the shipped bands.
    #[cfg(test)]
    pub fn bound() -> Self {
        static BOUND: std::sync::LazyLock<Potency> = std::sync::LazyLock::new(|| {
            let asset = crate::damage_cam::shapes::TankAsset::load(crate::tank::TANK_SPEC_PATH)
                .expect("the bound tank loads");
            Potency::of_tank(&asset.shapes(&Default::default())).expect("the bound tank has crew")
        });
        *BOUND
    }

    /// The emission `e = ρ s / Ω` (HP points per steradian) of a direction of strength `strength`
    /// (J) in a field whose support lies from `inner` to `half_angle` (rad) off its
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
            reason = "the look's shader parts the bands at runtime (`pen_cam_look.wgsl`, `strongest` and `footprint`); this is the law it applies, held by the tests"
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

/// `ρ`, the HP points one joule of field dose is worth: the inverse of the one energy rate
/// (`energy_dose::joules_per_hp`), in f64 and cast once.
pub(crate) fn hp_per_joule() -> f32 {
    (1.0 / energy_dose::joules_per_hp()) as f32
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

/// A band's edge as the painting draws it: how far value `x` is past `edge`, `smoothstep` over
/// `width` centred on it (`width` the value's change across a pixel, so the edge is one pixel wide
/// at every zoom), a hard step where the value does not change. `pen_cam_look.wgsl` (`soft_step`)
/// evaluates the same; this is the curve it mirrors, held by the tests.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the look's shader takes the edge at runtime (`pen_cam_look.wgsl`, `soft_step`); this is the curve it mirrors, held by the tests"
    )
)]
pub fn soft_step(edge: f32, width: f32, x: f32) -> f32 {
    let half = 0.5 * width.max(1e-6);
    let t = ((x - (edge - half)) / (2.0 * half)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture potency: a 30 HP crew pool, apart from the shipped one, over a 0.25 m² crew face,
    /// so 120 and 3.998 HP points per m² (DERIVED).
    fn fixture() -> Potency {
        let classes = BoxClasses::from_ron(r#"(crew_hp: (30, Tuned("fixture")))"#).unwrap();
        Potency::of(&classes, 0.25, 70.0)
    }

    /// The bands part at the crew pool and the least dose a full pool shows a lost point at, each
    /// over the struck tank's crew face as its sim geometry has it, never at the look's own
    /// numbers: fixture 120 and 3.998 HP points per m² over 0.25 m²; the bound tank's over its crew
    /// face (0.2033 m², MEASURED: 147.5 and 4.92, DERIVED), the rate the energy rate's inverse; and a
    /// tank with no crew station has no bands. The band promise relies on it; a threshold authored
    /// in the look or over a face held in data, a floor dropped to zero, one read as half a point,
    /// or bands for a tank with no crew station fails here.
    #[test]
    fn the_thresholds_read_the_crew_pool_over_the_tanks_crew_face() {
        let fixture = fixture();
        assert_eq!(fixture.mid, 120.0);
        assert!((fixture.floor - 3.998).abs() < 1e-6, "{}", fixture.floor);
        let asset = crate::damage_cam::shapes::TankAsset::load(crate::tank::TANK_SPEC_PATH)
            .expect("the bound tank loads");
        let face = asset.crew_face().expect("the bound tank has crew");
        let bound = Potency::bound();
        let classes = BoxClasses::shipped();
        assert_eq!(bound.mid, classes.potency_thresholds(face).mid);
        assert_eq!(bound.floor, classes.potency_thresholds(face).floor);
        assert_eq!(bound.mid, classes.crew_hp() as f32 / face);
        let share = crate::ballistics::least_shown_hp() as f32 / classes.crew_hp() as f32;
        assert!((bound.floor / bound.mid - share).abs() < 1e-6 * share);
        assert!(bound.floor > 0.0 && bound.floor < bound.mid);
        assert_eq!(bound.rate, hp_per_joule());
        assert_eq!(bound.rate, (1.0 / energy_dose::joules_per_hp()) as f32);
        assert_eq!(bound.lanes(), Vec4::new(bound.floor, bound.mid, 0.0, 0.0));
        assert_eq!(Potency::of_tank(&TankShapes::default()), None);
    }

    /// Three bands, each from its threshold up: nothing below the floor, reduced from the floor
    /// to mid, knockout at mid and above, over the bound tank's thresholds and the fixture's.
    /// Nothing takes no colour; the two bands take the field's two teals, apart in luminance so
    /// they part in every colour vision type, and neither is on the state scale's red. A dropped
    /// floor, a band boundary off its threshold, or one colour for both bands fails here.
    #[test]
    fn three_bands_part_at_the_thresholds() {
        for potency in [Potency::bound(), fixture()] {
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

    /// A band's edge on a painted surface is a smoothstep one pixel wide, centred on the
    /// threshold: half way at the threshold, nothing a half-width below, whole a half-width above,
    /// monotone between, and a hard step where the value is flat across the pixel. Every zoom
    /// draws the edge crisp and unaliased; a cut (a step function), an edge off its threshold or
    /// one wider than the pixel fails here. Tolerance at the threshold: the curve's slope there,
    /// 1.5 per width, times the f32 rounding of `edge ∓ width / 2`, two ulps of `edge` (DERIVED).
    #[test]
    fn a_band_edge_is_one_pixel_wide() {
        for (edge, width) in [(0.0_f32, 1.0_f32), (7.2, 0.05), (-3.0, 4.0)] {
            let rounding = 1.5 * 2.0 * f32::EPSILON * edge.abs().max(1.0) / width;
            assert!((soft_step(edge, width, edge) - 0.5).abs() <= rounding + 1e-6);
            assert_eq!(soft_step(edge, width, edge - 0.5 * width), 0.0);
            assert_eq!(soft_step(edge, width, edge + 0.5 * width), 1.0);
            let mut last = 0.0;
            for k in 0..=100 {
                let x = edge - 0.5 * width + width * k as f32 / 100.0;
                let here = soft_step(edge, width, x);
                assert!(here >= last - 1e-6, "{edge} {width}: {x}");
                last = here;
            }
            let inside = soft_step(edge, width, edge + 0.25 * width);
            assert!(inside > 0.5 && inside < 1.0, "{inside}");
        }
        assert_eq!(soft_step(1.0, 0.0, 1.0 + 1e-3), 1.0);
        assert_eq!(soft_step(1.0, 0.0, 1.0 - 1e-3), 0.0);
    }
}
