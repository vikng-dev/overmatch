//! The pen cam's potency bands: a spall field's local intensity at a point, the band that
//! intensity falls in, and each band's surface along a direction. Pure functions of the record's
//! cone and the shipped data: the field's rate (`assets/ballistics/spall_cone.ron`) and the
//! thresholds (`assets/ballistics/box_classes.ron`). The law, its promise and its calibration are
//! design doc §5, "Potency bands" (`.agents/docs/design/armor-penetration-and-damage.md`).
//!
//! Units: a direction's strength `s` in reference-mm; the rate `ρ` in HP points per reference-mm;
//! the emission `e = ρ s / Ω_c` in HP points per steradian, `Ω_c` the cone's solid angle; the
//! intensity `I = e / r²` and the thresholds in HP points per m²; radii in metres. Nothing here
//! reads a display gain, a level or a fade: the bands are the dose law's, never the look's.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "no drawing reads the law yet: only its tests and the resolver's do"
    )
)]

use std::f32::consts::TAU;

use crate::ballistics::box_classes::BoxClasses;
use crate::ballistics::spall_cone;

/// Where a point lies in a field, by its local intensity, ascending.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Band {
    /// Outside the field: past its reach, outside its cone, or at zero intensity.
    None,
    /// Above zero, below the mid threshold: a graze.
    Dim,
    /// At or above the mid threshold: a crew station's face wholly here is knocked out.
    Mid,
    /// At or above the bright threshold: a hard box's face wholly here has its HP pool emptied.
    Bright,
}

impl Band {
    /// The bands a field draws as surfaces, outermost first: the reach surface, then the mid and
    /// bright iso-surfaces inside it.
    pub const DRAWN: [Band; 3] = [Band::Dim, Band::Mid, Band::Bright];
}

/// The field's rate and the bands' thresholds, as the shipped data states them.
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
        let thresholds = BoxClasses::shipped().band_thresholds();
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

    /// The band of `intensity` (HP points per m²): an intensity at a threshold is in the higher
    /// band, zero is in none.
    pub fn band(&self, intensity: f32) -> Band {
        if intensity >= self.bright {
            Band::Bright
        } else if intensity >= self.mid {
            Band::Mid
        } else if intensity > 0.0 {
            Band::Dim
        } else {
            Band::None
        }
    }

    /// The distance (m) along a direction of emission `emission` and reach `reach` (m) out to
    /// which the field lies in `band` or a brighter one: the reach itself for the dim band, the
    /// band's radius clipped at the reach for the others, 0 for none. A band never passes its
    /// direction's reach, so a shadowed direction's bands end where it was stopped.
    pub fn shell_radius(&self, band: Band, reach: f32, emission: f32) -> f32 {
        let threshold = match band {
            Band::None => return 0.0,
            Band::Dim => return reach,
            Band::Mid => self.mid,
            Band::Bright => self.bright,
        };
        reach.min(band_radius(emission, threshold))
    }
}

/// The solid angle `Ω_c = 2π (1 − cos α)` (sr) of a cone of half-angle `half_angle` (rad).
pub fn solid_angle(half_angle: f32) -> f32 {
    TAU * (1.0 - half_angle.cos())
}

/// The intensity `e / r²` (HP points per m²) at distance `r` (m) along a direction of emission
/// `emission` (HP points per steradian), inside the direction's reach.
pub fn intensity(emission: f32, r: f32) -> f32 {
    emission / (r * r)
}

/// The distance `sqrt(e / T)` (m) at which a direction of emission `emission` falls to
/// `threshold` (HP points per m²), before any reach clips it.
pub fn band_radius(emission: f32, threshold: f32) -> f32 {
    (emission / threshold).max(0.0).sqrt()
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

    /// The look reads the shipped classes and rate, never its own: bright is the hard class, mid
    /// the crew pool over the face it is set against, the rate the field's calibration. The colour
    /// promise relies on it; a threshold or rate authored in the look breaks it.
    #[test]
    fn the_bands_read_the_shipped_classes() {
        use crate::ballistics::box_classes::BoxClass;
        let classes = BoxClasses::shipped();
        let potency = Potency::shipped();
        assert_eq!(
            potency.bright,
            classes.hp_per_square_metre(BoxClass::Hard) as f32
        );
        assert_eq!(potency.mid, classes.band_thresholds().mid);
        assert!(potency.mid > 0.0 && potency.mid <= potency.bright);
        assert_eq!(potency.rate, spall_cone::hp_per_reference_mm() as f32);
    }

    /// An intensity at a threshold is in the higher band, one just below it in the lower, zero
    /// in none and any intensity above zero at least dim. The footprint's colour and the shells'
    /// radii both rely on the same comparisons.
    #[test]
    fn a_band_starts_at_its_threshold() {
        let potency = Potency {
            rate: 70.0,
            bright: 300.0,
            mid: 150.0,
        };
        assert_eq!(potency.band(300.0), Band::Bright);
        assert_eq!(potency.band(300.0_f32.next_down()), Band::Mid);
        assert_eq!(potency.band(150.0), Band::Mid);
        assert_eq!(potency.band(150.0_f32.next_down()), Band::Dim);
        assert_eq!(potency.band(f32::MIN_POSITIVE), Band::Dim);
        assert_eq!(potency.band(0.0), Band::None);
        for threshold in [potency.mid, potency.bright] {
            let r = band_radius(1234.0, threshold);
            let at = intensity(1234.0, r);
            assert!((at / threshold - 1.0).abs() < 1e-5, "{at} at {r} m");
        }
    }

    /// A band's surface never passes its direction's reach, and where the reach is the shorter it
    /// is the band's surface: over every direction of every record. The shadow dent in every shell
    /// relies on it, and some recorded direction ends inside a band radius, so a dropped clip
    /// fails here.
    #[test]
    fn a_band_never_passes_its_reach() {
        let potency = Potency::shipped();
        let mut clipped = 0;
        for (stem, plan) in plans() {
            for cone in &plan.cones {
                for ray in &cone.rays {
                    let e = potency.emission(ray.weight * cone.strength, cone.half_angle);
                    for band in [Band::Mid, Band::Bright] {
                        let threshold = if band == Band::Mid {
                            potency.mid
                        } else {
                            potency.bright
                        };
                        let radius = potency.shell_radius(band, ray.reach, e);
                        assert!(
                            radius <= ray.reach,
                            "{stem}: {band:?} {radius} past {}",
                            ray.reach
                        );
                        if ray.reach < band_radius(e, threshold) {
                            assert_eq!(radius, ray.reach, "{stem}: {band:?}");
                            clipped += 1;
                        }
                    }
                }
            }
        }
        assert!(
            clipped > 0,
            "a recorded direction ends inside a band radius"
        );
    }

    /// The band surfaces nest along every direction of every record: bright inside mid inside the
    /// reach. The shells are drawn as nested surfaces in one hue's luminance ladder, and read so.
    #[test]
    fn the_band_surfaces_nest() {
        let potency = Potency::shipped();
        let mut directions = 0;
        for (stem, plan) in plans() {
            for cone in &plan.cones {
                for ray in &cone.rays {
                    let e = potency.emission(ray.weight * cone.strength, cone.half_angle);
                    let [dim, mid, bright] =
                        Band::DRAWN.map(|band| potency.shell_radius(band, ray.reach, e));
                    assert!(
                        bright <= mid && mid <= dim && dim == ray.reach,
                        "{stem}: {bright} {mid} {dim}"
                    );
                    directions += 1;
                }
            }
        }
        assert!(directions > 0, "a record throws a cone");
    }
}
