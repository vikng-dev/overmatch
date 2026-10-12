//! The spall cone's shape: its half-angle, its axis rule and the directions it is sampled by, read
//! from `assets/ballistics/spall_cone.ron`. The law the shape serves — strength, profile, shadowing,
//! dose — is design doc §5 (`.agents/docs/design/armor-penetration-and-damage.md`); the throw that
//! applies it is `throw_spall_cone` in the parent module.
//!
//! # Directions
//!
//! `N` directions in equal-solid-angle strata: direction `k` sits at `μ_k = cos θ_k =
//! 1 − (1 − cos α)(k + ½)/N`, the stratum's midpoint in `cos θ`, at golden-angle azimuth `φ_k =
//! k·π(3 − √5)`, and stands for `1/N` of the cone's solid angle. Its profile weight is
//! `f_k = (μ_k − cos α)/(1 − cos α)`, so a cone of on-axis strength `S` sends `S·f_k` along it, and
//! the emitted total `(1/N) Σ S·f_k` is `S/2` at every `N` (the midpoint rule is exact on a profile
//! linear in `cos θ`).
//!
//! # Determinism
//!
//! The strata are built once, at parse, in f64 with `libm`'s `cos`/`sin` and IEEE `sqrt`, then cast
//! to f32: the platform's own `sin`/`cos` are not the same on every shipped target. Placing a
//! stratum on an axis is IEEE f32 arithmetic alone (scalar glam, no fused operations), so a cone's
//! directions are the same bits on every target for the same axis.

use bevy::prelude::*;
use serde::Deserialize;

use crate::provenance::Sourced;

/// The shipped shape, embedded so a throw never waits on the asset server (ADR-0014: built from
/// data, synchronously).
const SPALL_CONE_RON: &str = include_str!("../../assets/ballistics/spall_cone.ron");

/// The golden angle `π(3 − √5)` (rad): the azimuth step between successive directions.
const GOLDEN_ANGLE: f64 = 2.399_963_229_728_653;

/// How the cone's axis is chosen at an exit.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AxisRule {
    /// `normalize(n_rear + d_exit)` at a genuine exit, `d_exit` at an internal step.
    RearNormalBisector,
    /// `d_exit` everywhere.
    ExitDirection,
}

/// The file as authored: every value with its provenance ([`crate::provenance`]).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShapeFile {
    half_angle_deg: Sourced<f64>,
    axis: Sourced<AxisRule>,
    directions: Sourced<usize>,
}

/// One direction of the cone in the cone's own frame: `cos θ`, the two lateral components
/// `sin θ cos φ` and `sin θ sin φ`, and its profile weight `f`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Stratum {
    mu: f32,
    lateral: (f32, f32),
    profile: f32,
}

/// The cone's shape, validated.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ConeShape {
    /// α (rad).
    half_angle: f32,
    axis: AxisRule,
    strata: Vec<Stratum>,
}

/// Largest `N` a shape may declare: a bound on the work one throw does, not a law.
const MAX_DIRECTIONS: usize = 65_536;

/// How far a shape's emitted total `(1/N) Σ f_k` may lie from ½ (the midpoint rule's exact value):
/// the rounding of its `f32` profile weights, each within `2^-24` relative of its f64 value, so
/// their mean is within `2^-25` of ½. DERIVED; this is `f32::EPSILON`, four times that.
const EMISSION_TOLERANCE: f64 = f32::EPSILON as f64;

/// The profile `(cos θ − cos α)/(1 − cos α)` at `mu = cos θ`, zero outside the cone.
pub(crate) fn profile(mu: f64, cos_alpha: f64) -> f64 {
    ((mu - cos_alpha) / (1.0 - cos_alpha)).max(0.0)
}

impl ConeShape {
    /// A shape of half-angle `half_angle` (rad) sampled by `directions` strata. Refuses a half-angle
    /// outside `(0, π/2]` or a direction count outside `1..=MAX_DIRECTIONS`, and a shape whose
    /// construction is numerically degenerate: a half-angle so small that `cos α` rounds to 1 (the
    /// profile's denominator `1 − cos α` vanishes), any non-finite stratum, or an emitted total
    /// `(1/N) Σ f_k` farther from ½ than [`EMISSION_TOLERANCE`].
    pub(crate) fn new(half_angle: f64, axis: AxisRule, directions: usize) -> Result<Self, String> {
        if !(half_angle.is_finite()
            && half_angle > 0.0
            && half_angle <= std::f64::consts::FRAC_PI_2)
        {
            return Err(format!(
                "half-angle must be in (0°, 90°] (got {}°)",
                half_angle.to_degrees()
            ));
        }
        if !(1..=MAX_DIRECTIONS).contains(&directions) {
            return Err(format!(
                "directions must be in 1..={MAX_DIRECTIONS} (got {directions})"
            ));
        }
        let cos_alpha = libm::cos(half_angle);
        if 1.0 - cos_alpha <= 0.0 {
            return Err(format!(
                "half-angle {}° is below what the profile can resolve: cos α rounds to 1",
                half_angle.to_degrees()
            ));
        }
        let n = directions as f64;
        let strata = (0..directions)
            .map(|k| {
                let mu = 1.0 - (1.0 - cos_alpha) * (k as f64 + 0.5) / n;
                let sin_theta = (1.0 - mu * mu).max(0.0).sqrt();
                let phi = k as f64 * GOLDEN_ANGLE;
                Stratum {
                    mu: mu as f32,
                    lateral: (
                        (sin_theta * libm::cos(phi)) as f32,
                        (sin_theta * libm::sin(phi)) as f32,
                    ),
                    profile: profile(mu, cos_alpha) as f32,
                }
            })
            .collect::<Vec<Stratum>>();
        if let Some(k) = strata.iter().position(|stratum| {
            ![
                stratum.mu,
                stratum.lateral.0,
                stratum.lateral.1,
                stratum.profile,
            ]
            .iter()
            .all(|value| value.is_finite())
        }) {
            return Err(format!("direction {k} of the cone is not finite"));
        }
        let emitted = strata
            .iter()
            .map(|stratum| stratum.profile as f64)
            .sum::<f64>()
            / n;
        // Every stratum is finite (checked above), so the total is.
        if (emitted - 0.5).abs() > EMISSION_TOLERANCE {
            return Err(format!(
                "the cone emits {emitted} of its on-axis strength, not ½, at half-angle {}°",
                half_angle.to_degrees()
            ));
        }
        Ok(Self {
            half_angle: half_angle as f32,
            axis,
            strata,
        })
    }

    /// Parse and validate a shape file; degrees are converted here, once.
    pub(crate) fn from_ron(text: &str) -> Result<Self, String> {
        let file: ShapeFile = ron::de::from_str(text).map_err(|err| err.to_string())?;
        Self::new(
            file.half_angle_deg.0 * std::f64::consts::PI / 180.0,
            file.axis.0,
            file.directions.0,
        )
    }

    /// The shipped shape, parsed once. Panics on a malformed file: a broken authored contract is a
    /// build defect, as for the substance registry. [`super::sim_plugin`] calls this while the app
    /// is built, so the file is parsed and validated at startup, never at the first throw.
    pub(crate) fn shipped() -> &'static Self {
        static SHAPE: std::sync::LazyLock<ConeShape> = std::sync::LazyLock::new(|| {
            ConeShape::from_ron(SPALL_CONE_RON).unwrap_or_else(|err| {
                panic!("spall cone: embedded spall_cone.ron is unusable: {err}")
            })
        });
        &SHAPE
    }

    /// α (rad).
    pub(crate) fn half_angle(&self) -> f32 {
        self.half_angle
    }

    /// N.
    pub(crate) fn directions_count(&self) -> usize {
        self.strata.len()
    }

    /// The cone's axis for an exit whose round leaves along `direction`, through a rear face of
    /// outward normal `face` (`None` at an internal step).
    pub(crate) fn axis(&self, face: Option<Vec3>, direction: Dir3) -> Dir3 {
        match (self.axis, face) {
            (AxisRule::RearNormalBisector, Some(normal)) => {
                Dir3::new(normal + Vec3::from(direction)).unwrap_or(direction)
            }
            _ => direction,
        }
    }

    /// Every direction of a cone about `axis`, with its profile weight `f_k`, in stratum order.
    pub(crate) fn directions(&self, axis: Dir3) -> impl Iterator<Item = (Dir3, f32)> + '_ {
        let z = Vec3::from(axis);
        let up = if z.y.abs() > 0.99 { Vec3::X } else { Vec3::Y };
        let x = z.cross(up).normalize();
        let y = z.cross(x);
        self.strata.iter().map(move |stratum| {
            let local = z * stratum.mu + x * stratum.lateral.0 + y * stratum.lateral.1;
            (Dir3::new(local).unwrap_or(axis), stratum.profile)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture half-angle (rad): fixture data, not the shipped value.
    const ALPHA: f64 = 0.436;

    fn shape(directions: usize) -> ConeShape {
        ConeShape::new(ALPHA, AxisRule::RearNormalBisector, directions).unwrap()
    }

    /// The shipped file parses and validates.
    #[test]
    fn the_shipped_shape_parses() {
        let shipped = ConeShape::shipped();
        assert!(shipped.directions_count() >= 1);
        assert!(shipped.half_angle() > 0.0);
    }

    /// A shape outside the law's domain is refused at parse, naming the field.
    #[test]
    fn a_shape_outside_the_domain_is_refused() {
        for (half_angle, directions) in [(0.0, 24), (-0.1, 24), (f64::NAN, 24), (1.6, 24)] {
            assert!(ConeShape::new(half_angle, AxisRule::ExitDirection, directions).is_err());
        }
        assert!(ConeShape::new(ALPHA, AxisRule::ExitDirection, 0).is_err());
        let text = r#"(half_angle_deg: (25.0, Tuned("fixture")),
            axis: (ExitDirection, Tuned("fixture")), directions: (24, Resolution("fixture")),
            range: (6.0, Tuned("fixture")))"#;
        assert!(
            ConeShape::from_ron(text).is_err(),
            "an unknown field is refused"
        );
    }

    /// A shape value authored without its provenance is refused at parse.
    #[test]
    fn a_shape_value_without_provenance_is_refused() {
        let sourced = r#"(half_angle_deg: (25.0, Tuned("fixture")),
            axis: (ExitDirection, Tuned("fixture")), directions: (24, Resolution("fixture")))"#;
        assert!(ConeShape::from_ron(sourced).is_ok());
        for bare in [
            r#"(half_angle_deg: 25.0, axis: (ExitDirection, Tuned("fixture")),
                directions: (24, Resolution("fixture")))"#,
            r#"(half_angle_deg: (25.0, Tuned("fixture")), axis: ExitDirection,
                directions: (24, Resolution("fixture")))"#,
            r#"(half_angle_deg: (25.0, Tuned("fixture")), axis: (ExitDirection, Tuned("fixture")),
                directions: 24)"#,
        ] {
            let error = ConeShape::from_ron(bare).expect_err(bare);
            assert!(error.contains("provenance"), "{bare}: {error}");
        }
    }

    /// A half-angle the profile cannot resolve is refused at parse: at `1e-9` rad `cos α` rounds to
    /// 1 and the profile's denominator vanishes (DERIVED). Every half-angle the profile resolves
    /// emits ½ at every direction count.
    #[test]
    fn a_numerically_degenerate_half_angle_is_refused() {
        let error = ConeShape::new(1.0e-9, AxisRule::ExitDirection, 24).unwrap_err();
        assert!(error.contains("cos α rounds to 1"), "{error}");
        for half_angle in [1.0e-3, 0.0175, ALPHA, 1.0, std::f64::consts::FRAC_PI_2] {
            for directions in [1, 24, 97, MAX_DIRECTIONS] {
                assert!(
                    ConeShape::new(half_angle, AxisRule::ExitDirection, directions).is_ok(),
                    "α = {half_angle} rad, N = {directions}"
                );
            }
        }
    }

    /// CLASS INVARIANT 1, EXACT EMISSION: with nothing in the cone, `(1/N) Σ s_k = S/2` to float
    /// precision at every N.
    #[test]
    fn the_emitted_total_is_half_the_on_axis_strength_at_every_resolution() {
        for directions in [1, 7, 24, 97] {
            let shape = shape(directions);
            let total: f64 =
                shape.strata.iter().map(|s| s.profile as f64).sum::<f64>() / directions as f64;
            assert!(
                (total - 0.5).abs() < 1.0e-6,
                "N = {directions}: emitted {total} of S"
            );
        }
    }

    /// CLASS INVARIANT 5, PROFILE SHAPE: the strength is greatest on the axis, does not grow with
    /// angle, and is zero at and past the rim; every direction lies inside the cone.
    #[test]
    fn the_profile_falls_from_the_axis_to_zero_at_the_rim() {
        let cos_alpha = libm::cos(ALPHA);
        assert_eq!(profile(1.0, cos_alpha), 1.0);
        assert_eq!(profile(cos_alpha, cos_alpha), 0.0);
        assert_eq!(profile(libm::cos(ALPHA * 1.5), cos_alpha), 0.0);
        let mut last = f64::INFINITY;
        for step in 0..=100 {
            let theta = ALPHA * step as f64 / 100.0;
            let value = profile(libm::cos(theta), cos_alpha);
            assert!(value <= last, "non-increasing at θ = {theta}");
            last = value;
        }
        let axis = Dir3::new(Vec3::new(0.3, -0.2, 0.9)).unwrap();
        for (direction, weight) in shape(97).directions(axis) {
            let angle = direction.angle_between(Vec3::from(axis));
            assert!(angle <= ALPHA as f32 + 1.0e-5, "{angle} past the rim");
            assert!(weight > 0.0 && weight < 1.0);
        }
    }

    /// The strata are equal in solid angle: their `cos θ` midpoints are evenly spaced and span the
    /// cap from the axis to the rim.
    #[test]
    fn the_strata_are_equal_in_solid_angle() {
        let shape = shape(24);
        let cos_alpha = libm::cos(ALPHA) as f32;
        let width = (1.0 - cos_alpha) / 24.0;
        for (k, stratum) in shape.strata.iter().enumerate() {
            let expected = 1.0 - width * (k as f32 + 0.5);
            assert!((stratum.mu - expected).abs() < 1.0e-6, "stratum {k}");
        }
    }

    /// The axis rule: square on, the bisector is the face normal and the exit direction; oblique,
    /// it bisects them; at an internal step it is the exit direction.
    #[test]
    fn the_axis_bisects_rear_normal_and_exit_direction_at_genuine_exits_only() {
        let shape = shape(24);
        let square = Dir3::Z;
        assert_eq!(shape.axis(Some(Vec3::Z), square), Dir3::Z);

        let normal = Vec3::Z;
        let oblique = Dir3::new(Vec3::new(1.0, 0.0, 1.0)).unwrap();
        let axis = shape.axis(Some(normal), oblique);
        let to_normal = axis.angle_between(normal);
        let to_exit = axis.angle_between(Vec3::from(oblique));
        assert!(
            (to_normal - to_exit).abs() < 1.0e-6,
            "{to_normal} {to_exit}"
        );
        assert!((to_normal + to_exit - std::f32::consts::FRAC_PI_4).abs() < 1.0e-6);

        assert_eq!(shape.axis(None, oblique), oblique);
        let along = ConeShape::new(ALPHA, AxisRule::ExitDirection, 24).unwrap();
        assert_eq!(along.axis(Some(normal), oblique), oblique);
    }

    /// The directions are the same bits on every target: pinned raw IEEE bits of a few directions
    /// for two fixture axes. MEASURED on macOS arm64; a red run on another target means the
    /// generator is not reproducible there.
    #[test]
    fn the_directions_are_pinned_to_the_bit() {
        let shape = shape(24);
        let axes = [Dir3::Z, Dir3::new(Vec3::new(0.3, -0.2, 0.9)).unwrap()];
        let mut bits = Vec::new();
        for axis in axes {
            let directions: Vec<(Dir3, f32)> = shape.directions(axis).collect();
            for k in [0usize, 5, 23] {
                let (direction, weight) = directions[k];
                let v = Vec3::from(direction);
                bits.push([
                    v.x.to_bits(),
                    v.y.to_bits(),
                    v.z.to_bits(),
                    weight.to_bits(),
                ]);
            }
        }
        if std::env::var_os("OVERMATCH_SPALL_CONE_SHOW").is_some() {
            println!("{bits:#010x?}");
        }
        assert_eq!(bits, PINNED_BITS);
    }

    /// `[x, y, z, f_k]` bits of directions 0, 5 and 23 about each fixture axis. MEASURED on macOS
    /// arm64; print them with `OVERMATCH_SPALL_CONE_SHOW=1` and `--nocapture`.
    const PINNED_BITS: [[u32; 4]; 6] = [
        [0xbd7f9aca, 0x00000000, 0x3f7f8045, 0x3f7aaaab],
        [0xbe31f2ae, 0x3de26451, 0x3f7a82fa, 0x3f455555],
        [0xbdbbf055, 0x3ed0da09, 0x3f688cb8, 0x3caaaaab],
        [0x3e7f9cf6, 0xbe52d2ca, 0x3f723a67, 0x3f7aaaab],
        [0x3e14a0a2, 0xbdbfe3e4, 0x3f7c269b, 0x3f455555],
        [0x3e61edc7, 0x3e58d58a, 0x3f73bc66, 0x3caaaaab],
    ];
}
