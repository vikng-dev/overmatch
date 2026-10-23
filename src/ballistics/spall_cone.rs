//! The spall cone's shape — its axis rule, the directions it is sampled by and how a direction
//! passes through what it meets —, the field law that weights those directions and sets the
//! cone's half-angle ([`FieldDescriptor`]), and the field's calibration, its HP points per
//! reference-mm ([`hp_per_reference_mm`]) and the calibre that rate was set against
//! ([`reference_calibre`]), read from `assets/ballistics/spall_cone.ron`.
//! The law the shape serves — strength, profile, shadowing, dose — is design doc §5
//! (`.agents/docs/design/armor-penetration-and-damage.md`); the throw that applies it is
//! `throw_spall_cone` in the parent module, and a world may throw another shape ([`SpallLaw`]).
//!
//! # The field law
//!
//! A cone is described by a law and one shape number, its ratio `λ` ([`FieldDescriptor`]). Under
//! the one law, [`FieldLaw::Ball`], the share of the cone's emission inside polar angle `θ` of its
//! axis is `F(μ) = 1 − μ·D^m`, `μ = cos θ`, `D = √(1 − λ²(1 − μ²))`, `m` = [`BALL_EXPONENT`]: the
//! field is brightest on the axis and falls to zero at the rim `α = asin(1/λ)`. A plate exit's `λ`
//! is its rear layer's substance's (`crate::substances::HardSubstance::debris_ratio`), so the
//! half-angle is the exit material's. The passage stays a data-selectable dial (playtest fork F6):
//! the shipped [`Passage::FirstSolid`] and the kept [`Passage::Partial`].
//!
//! # Directions
//!
//! `N` directions in equal-solid-angle strata: with `w = 1 − cos α`, direction `k` sits at
//! `μ_k = 1 − w(k + ½)/N`, the stratum's midpoint in `cos θ`, at golden-angle azimuth
//! `φ_k = k·π(3 − √5)`, and stands for `1/N` of the cone's solid angle. Its weight is its stratum's
//! exact share of the emission, `f_k = (N/2)[F(e_{k+1}) − F(e_k)]` over the stratum's edges
//! `e_j = 1 − w·j/N`, so a cone of strength `S` sends `S·f_k` along it, and the emitted total
//! `(1/N) Σ S·f_k` telescopes to `S/2` at every `N`.
//!
//! # Determinism
//!
//! The azimuths are built once per shape, at parse, in f64 with `libm`'s `cos`/`sin`, and kept in
//! f64: the platform's own `sin`/`cos` are not the same on every shipped target. A cone's `μ_k`,
//! `sin θ_k` and `f_k` are built from its `λ` in f64 with IEEE `+`, `−`, `×`, `/` and `sqrt` alone,
//! each correctly rounded, then cast to f32, so a decoder on any target rebuilds the same bits from
//! the same descriptor; its half-angle is `libm`'s `asin`. Placing a stratum on an axis is IEEE f32
//! arithmetic alone (scalar glam, no fused operations), so a cone's directions are the same bits on
//! every target for the same axis.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::provenance::Sourced;

/// The shipped shape, embedded so a throw never waits on the asset server (ADR-0014: built from
/// data, synchronously).
const SPALL_CONE_RON: &str = include_str!("../../assets/ballistics/spall_cone.ron");

/// The share of the strength `S` a cone emits in total, `(1/N) Σ f_k`, under every law: a law's
/// stratum shares sum to its whole emission, 1, and `f_k` is `N/2` of a share. Every spread is
/// validated against it.
pub(crate) const EMITTED_SHARE: f32 = 0.5;

/// The ball law's exponent `m`: sideways debris speeds spread evenly through a ball, from zero to
/// the greatest, give `F(μ) = 1 − μ·D³`. A constant of the law, not data: another `m` is another
/// law.
pub(crate) const BALL_EXPONENT: u32 = 3;

/// The golden angle `π(3 − √5)` (rad): the azimuth step between successive directions.
const GOLDEN_ANGLE: f64 = 2.399_963_229_728_653;

/// How a field's strength is spread over its directions, and how wide it is.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldLaw {
    /// Sideways debris speeds uniform in a ball about the forward speed, the greatest `1/λ` of
    /// it: share inside `θ` `F(μ) = 1 − μ·D^m`, `m` = [`BALL_EXPONENT`], rim `α = asin(1/λ)`;
    /// brightest on the axis, zero at the rim.
    Ball,
}

impl FieldLaw {
    /// Every law, in id order.
    pub(crate) const ALL: [FieldLaw; 1] = [FieldLaw::Ball];
}

/// A field's law and its one shape number: what a record and the wire carry per cone. With the
/// shot vocabulary's [`ConeShape`] it rebuilds the cone's directions and weights
/// ([`ConeShape::spread`]).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct FieldDescriptor {
    pub law: FieldLaw,
    /// `λ`: the debris's forward speed over its greatest sideways speed; the cone's half-angle is
    /// `asin(1/λ)`. The representation admits `λ < 1`, a field over the whole sphere; the ball's
    /// domain is `λ ≥ 1`, a plate exit's (`α ≤ 90°`).
    pub ratio: f32,
}

/// The ball law's terms for one ratio: `λ²` and the cap's depth `w = 1 − cos α`.
#[derive(Clone, Copy, Debug)]
struct BallTerms {
    l2: f64,
    w: f64,
}

impl BallTerms {
    /// The share of the emission inside the cone of depth `t = 1 − cos θ` about the axis,
    /// `1 − μ·D^m`, `D² = 1 − λ²·t(2 − t)` (`= 1 − λ²(1 − μ²)`, without the cancellation), held
    /// at zero past the rim. `D^m` is `m` IEEE products.
    fn share(&self, t: f64, exponent: u32) -> f64 {
        let d = (1.0 - self.l2 * (t * (2.0 - t))).max(0.0).sqrt();
        1.0 - (1.0 - t) * (0..exponent).fold(1.0, |power, _| power * d)
    }
}

impl FieldDescriptor {
    /// The ball law at ratio `ratio`.
    pub(crate) fn ball(ratio: f32) -> Self {
        Self {
            law: FieldLaw::Ball,
            ratio,
        }
    }

    /// The law's terms, refusing a descriptor outside its domain: a ratio that is not finite, a
    /// ratio below 1 (a full sphere, which the APHE burst slice adds), and a cone so narrow that
    /// `1 − w`, the rim's `cos α`, rounds to 1 (`λ` above about 10⁸, DERIVED). Names the violation.
    fn terms(&self) -> Result<BallTerms, String> {
        let FieldLaw::Ball = self.law;
        let lambda = self.ratio as f64;
        if !lambda.is_finite() {
            return Err(format!("a field's ratio must be finite (got {lambda})"));
        }
        if lambda < 1.0 {
            return Err(format!(
                "a field's ratio below 1 (got {lambda}) throws over the whole sphere; the APHE \
                 burst slice adds it, and a plate exit's cone has a ratio of at least 1"
            ));
        }
        let l2 = lambda * lambda;
        let w = 1.0 / (l2 * (1.0 + (1.0 - 1.0 / l2).sqrt()));
        if 1.0 - w >= 1.0 {
            return Err(format!(
                "a field of ratio {lambda} is narrower than the law resolves: its rim's cos α \
                 rounds to 1"
            ));
        }
        Ok(BallTerms { l2, w })
    }

    /// Whether the descriptor lies in its law's domain ([`Self::terms`]); names the violation.
    pub(crate) fn check(&self) -> Result<(), String> {
        self.terms().map(|_| ())
    }

    /// The law's weight along a direction at `mu = cos θ` from the axis: the continuous form of a
    /// spread's `f_k`, `½·w·(D^m + m·λ²μ²·D^(m−2))` under the ball, `(w/2)·(−dF/dμ)`; zero at and
    /// past the rim, and for a descriptor outside the law's domain.
    pub(crate) fn weight_at(&self, mu: f64) -> f64 {
        let Ok(terms) = self.terms() else {
            return 0.0;
        };
        // `D²` is even in `μ`: past the rim it turns positive again behind the plate, so the cap is
        // bounded by its depth, not by the sign of `D²`.
        let t = 1.0 - mu;
        if t.is_nan() || t >= terms.w {
            return 0.0;
        }
        let d = (1.0 - terms.l2 * (t * (2.0 - t))).max(0.0).sqrt();
        let power = |n: u32| (0..n).fold(1.0, |p, _| p * d);
        let m = BALL_EXPONENT;
        0.5 * terms.w * (power(m) + m as f64 * terms.l2 * mu * mu * power(m - 2))
    }
}

/// How a direction of the cone passes through the hard bodies it meets. Under either, a witness
/// (ADR-0040) is dosed at the strength entering it and neither spends nor stops the direction.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Passage {
    /// Spent by `chord × factor` through every hard body in turn; each box it enters is dosed at
    /// its strength there, and it goes on until spent.
    Partial,
    /// Stopped by the first hard body it enters: a box there takes its whole strength, armour
    /// none.
    FirstSolid,
}

/// The cone shape a world's march throws, in place of the shipped one: inserted by a world that
/// measures another shape (the throw-cost probe, `pen_cam::spall_law_cost`).
#[derive(Resource, Clone, Debug)]
pub(crate) struct SpallLaw(pub ConeShape);

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
    axis: Sourced<AxisRule>,
    directions: Sourced<usize>,
    passage: Sourced<Passage>,
    hp_per_reference_mm: Sourced<u64>,
    reference_calibre_m: Sourced<f32>,
}

/// The field's calibration as parsed: the two numbers every cone's dose is scaled by.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Calibration {
    /// HP points per reference-mm of field dose; at least 1.
    hp_per_reference_mm: u64,
    /// The calibre (m) the rate was set against; finite, positive and normal.
    reference_calibre: f32,
}

/// The shipped calibration, parsed once. Panics on a malformed file, as [`ConeShape::shipped`]
/// does; [`super::sim_plugin`] reads it while the app is built.
fn shipped_calibration() -> Calibration {
    static CALIBRATION: std::sync::LazyLock<Calibration> = std::sync::LazyLock::new(|| {
        calibration_from_ron(SPALL_CONE_RON).unwrap_or_else(|err| {
            panic!("spall cone: embedded spall_cone.ron's calibration is unusable: {err}")
        })
    });
    *CALIBRATION
}

/// The field's HP points per reference-mm of field dose, where the line converts at
/// [`super::HP_PER_REFERENCE_MM`]. A field deposit of `d` reference-mm enters a box's per-shot dose
/// at `d × this / HP_PER_REFERENCE_MM` ([`super::field_units`]); a cone's strength `S` stays in
/// reference-mm.
pub(crate) fn hp_per_reference_mm() -> u64 {
    shipped_calibration().hp_per_reference_mm
}

/// The calibre `d_ref` (m) the field's rate was set against: a cone's strength goes as the hole's
/// area, `(d / d_ref)²` of a hole of calibre `d` ([`super::walk::SpallSource::strength`]), so a
/// round of this calibre throws its cone unscaled.
pub(crate) fn reference_calibre() -> f32 {
    shipped_calibration().reference_calibre
}

/// Parse a shape file's calibration, refusing a rate of zero (a field that converts to no HP point
/// is no field) and a reference calibre that is not finite, positive and normal (a cone's strength
/// divides by it).
fn calibration_from_ron(text: &str) -> Result<Calibration, String> {
    let file: ShapeFile = ron::de::from_str(text).map_err(|err| err.to_string())?;
    let hp_per_reference_mm = match file.hp_per_reference_mm.0 {
        0 => return Err("hp_per_reference_mm must be at least 1 (got 0)".into()),
        rate => rate,
    };
    let reference_calibre = file.reference_calibre_m.0;
    if !reference_calibre.is_normal() || reference_calibre < 0.0 {
        return Err(format!(
            "reference_calibre_m must be finite, positive and normal (got {reference_calibre})"
        ));
    }
    Ok(Calibration {
        hp_per_reference_mm,
        reference_calibre,
    })
}

/// One direction of a cone in the cone's own frame: `cos θ`, the two lateral components
/// `sin θ cos φ` and `sin θ sin φ`, and its law weight `f`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Stratum {
    mu: f32,
    lateral: (f32, f32),
    weight: f32,
}

/// The cone's shape, validated: everything about a cone but its field law.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ConeShape {
    axis: AxisRule,
    passage: Passage,
    /// `(cos φ_k, sin φ_k)` of each direction's golden-angle azimuth, in stratum order.
    azimuths: Vec<(f64, f64)>,
}

/// One cone's directions and weights: a [`ConeShape`]'s strata under a [`FieldDescriptor`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Spread {
    field: FieldDescriptor,
    /// α (rad).
    half_angle: f32,
    strata: Vec<Stratum>,
}

/// Largest `N` a shape may declare: a bound on the work one throw does, not a law.
const MAX_DIRECTIONS: usize = 65_536;

/// How far a spread's emitted total `(1/N) Σ f_k` may lie from ½. In f64 the shares telescope to
/// ½ within a few f64 ulps; each f32 weight is within `2^-24` relative of its f64 value and every
/// weight is non-negative, so their mean is within `2^-25` of ½. DERIVED; this is `f32::EPSILON`,
/// four times that.
const EMISSION_TOLERANCE: f64 = f32::EPSILON as f64;

impl ConeShape {
    /// A shape of `directions` strata about an axis chosen by `axis`, its directions passing as
    /// `passage`. Refuses a direction count outside `1..=MAX_DIRECTIONS`.
    pub(crate) fn new(axis: AxisRule, directions: usize, passage: Passage) -> Result<Self, String> {
        if !(1..=MAX_DIRECTIONS).contains(&directions) {
            return Err(format!(
                "directions must be in 1..={MAX_DIRECTIONS} (got {directions})"
            ));
        }
        let azimuths = (0..directions)
            .map(|k| {
                let phi = k as f64 * GOLDEN_ANGLE;
                (libm::cos(phi), libm::sin(phi))
            })
            .collect();
        Ok(Self {
            axis,
            passage,
            azimuths,
        })
    }

    /// Parse and validate a shape file.
    pub(crate) fn from_ron(text: &str) -> Result<Self, String> {
        let file: ShapeFile = ron::de::from_str(text).map_err(|err| err.to_string())?;
        let directions = file.directions.0;
        Self::from_file(file, directions)
    }

    /// Validate `file`'s shape sampled by `directions` strata.
    fn from_file(file: ShapeFile, directions: usize) -> Result<Self, String> {
        Self::new(file.axis.0, directions, file.passage.0)
    }

    /// The shipped shape sampled by `directions` strata in place of its own count: a probe of how
    /// the throw's cost scales with `N`.
    #[cfg(test)]
    pub(crate) fn shipped_at(directions: usize) -> Self {
        let file = ron::de::from_str(SPALL_CONE_RON).expect("the shipped shape parses");
        Self::from_file(file, directions)
            .expect("the shipped shape validates at any direction count")
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

    /// N.
    pub(crate) fn directions_count(&self) -> usize {
        self.azimuths.len()
    }

    /// How each direction passes through what it meets.
    pub(crate) fn passage(&self) -> Passage {
        self.passage
    }

    /// This shape under `passage` instead.
    #[cfg(test)]
    pub(crate) fn with_passage(&self, passage: Passage) -> Self {
        Self {
            passage,
            ..self.clone()
        }
    }

    /// Each direction's `(cos φ_k, sin φ_k)`, in stratum order.
    pub(crate) fn azimuths(&self) -> impl Iterator<Item = (f64, f64)> + '_ {
        self.azimuths.iter().copied()
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

    /// The first lateral axis of a cone about the unit axis `z`: `z × up`, normalized, `up` the
    /// frame's `+Y`, or `+X` where `z` lies within ~8° of `±Y`. The second is `z` × this.
    pub(crate) fn lateral_axis(z: Vec3) -> Vec3 {
        let up = if z.y.abs() > 0.99 { Vec3::X } else { Vec3::Y };
        z.cross(up).normalize()
    }

    /// This shape's strata under `field`: each direction placed in its stratum of the cone
    /// `field` bounds, and weighted by its stratum's share of the emission. Refuses a descriptor
    /// outside its law's domain ([`FieldDescriptor::check`]), and a spread whose construction is
    /// numerically degenerate: any non-finite stratum, or an emitted total `(1/N) Σ f_k` farther
    /// from ½ than [`EMISSION_TOLERANCE`].
    pub(crate) fn spread(&self, field: FieldDescriptor) -> Result<Spread, String> {
        self.spread_under(field, BALL_EXPONENT)
    }

    /// [`Self::spread`] with the ball's exponent `exponent` in place of [`BALL_EXPONENT`]: the
    /// shot vocabulary's fingerprint folds the law's strata through it, so its test can show that
    /// the exponent moves the fingerprint.
    pub(crate) fn spread_under(
        &self,
        field: FieldDescriptor,
        exponent: u32,
    ) -> Result<Spread, String> {
        let terms = field.terms()?;
        let n = self.azimuths.len() as f64;
        let share = |edge: usize| terms.share(terms.w * edge as f64 / n, exponent);
        let strata: Vec<Stratum> = self
            .azimuths
            .iter()
            .enumerate()
            .map(|(k, &(cos_phi, sin_phi))| {
                let t = terms.w * (k as f64 + 0.5) / n;
                let sin_theta = (t * (2.0 - t)).sqrt();
                Stratum {
                    mu: (1.0 - t) as f32,
                    lateral: ((sin_theta * cos_phi) as f32, (sin_theta * sin_phi) as f32),
                    weight: (0.5 * n * (share(k + 1) - share(k))) as f32,
                }
            })
            .collect();
        if let Some(k) = strata.iter().position(|stratum| {
            ![
                stratum.mu,
                stratum.lateral.0,
                stratum.lateral.1,
                stratum.weight,
            ]
            .iter()
            .all(|value| value.is_finite())
        }) {
            return Err(format!("direction {k} of the cone is not finite"));
        }
        let emitted = strata
            .iter()
            .map(|stratum| stratum.weight as f64)
            .sum::<f64>()
            / n;
        // Every stratum is finite (checked above), so the total is.
        if (emitted - EMITTED_SHARE as f64).abs() > EMISSION_TOLERANCE {
            return Err(format!(
                "the cone emits {emitted} of its strength, not ½, at ratio {}",
                field.ratio
            ));
        }
        Ok(Spread {
            field,
            half_angle: libm::asin(1.0 / field.ratio as f64) as f32,
            strata,
        })
    }
}

impl Spread {
    /// The descriptor it was built from.
    pub(crate) fn field(&self) -> FieldDescriptor {
        self.field
    }

    /// α (rad): `asin(1/λ)`.
    pub(crate) fn half_angle(&self) -> f32 {
        self.half_angle
    }

    /// N.
    pub(crate) fn directions_count(&self) -> usize {
        self.strata.len()
    }

    /// Every direction of the cone about `axis`, with its weight `f_k`, in stratum order: the
    /// strata placed about `axis` and its [`ConeShape::lateral_axis`].
    pub(crate) fn directions(&self, axis: Dir3) -> impl Iterator<Item = (Dir3, f32)> + '_ {
        self.directions_about(axis, ConeShape::lateral_axis(Vec3::from(axis)))
    }

    /// Every direction of the cone about `axis` whose first lateral axis is `x` (unit, normal to
    /// `axis`), with its weight `f_k`, in stratum order.
    pub(crate) fn directions_about(
        &self,
        axis: Dir3,
        x: Vec3,
    ) -> impl Iterator<Item = (Dir3, f32)> + '_ {
        let z = Vec3::from(axis);
        let y = z.cross(x);
        self.strata.iter().map(move |stratum| {
            let local = z * stratum.mu + x * stratum.lateral.0 + y * stratum.lateral.1;
            (Dir3::new(local).unwrap_or(axis), stratum.weight)
        })
    }

    /// Every direction in the cone's own frame, in stratum order: `cos θ`, the two lateral
    /// components and the weight `f`.
    pub(crate) fn strata(&self) -> impl Iterator<Item = [f32; 4]> + '_ {
        self.strata.iter().map(|stratum| {
            [
                stratum.mu,
                stratum.lateral.0,
                stratum.lateral.1,
                stratum.weight,
            ]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture ratio `λ` (a 24.6° cone, DERIVED): fixture data, not a substance's value.
    const RATIO: f32 = 2.4;

    /// Ratios the law's invariants are checked across: the hemisphere, wide, plate-like and narrow
    /// cones. Fixture data.
    const RATIOS: [f32; 7] = [1.0, 1.05, 1.5, 2.27, 4.0, 100.0, 1.0e4];

    fn shape(directions: usize) -> ConeShape {
        ConeShape::new(AxisRule::RearNormalBisector, directions, Passage::Partial).unwrap()
    }

    fn spread(directions: usize, ratio: f32) -> Spread {
        shape(directions)
            .spread(FieldDescriptor::ball(ratio))
            .unwrap()
    }

    fn weights(spread: &Spread) -> Vec<f64> {
        spread.strata().map(|[.., f]| f as f64).collect()
    }

    /// A fixture shape file whose first fields are `fields`.
    fn file(fields: &str) -> String {
        format!(
            r#"({fields}, directions: (24, Resolution("fixture")),
            passage: (Partial, Tuned("fixture")), hp_per_reference_mm: (70, Tuned("fixture")),
            reference_calibre_m: (0.088, Tuned("fixture")))"#
        )
    }

    /// A shape outside its domain is refused at parse, and so is a slot the shape does not declare:
    /// the half-angle is the exit material's and the profile the law's, so a file that authors
    /// either is refused rather than read past.
    #[test]
    fn a_shape_outside_the_domain_is_refused() {
        assert!(ConeShape::new(AxisRule::ExitDirection, 0, Passage::Partial).is_err());
        assert!(
            ConeShape::new(
                AxisRule::ExitDirection,
                MAX_DIRECTIONS + 1,
                Passage::Partial
            )
            .is_err()
        );
        assert!(ConeShape::from_ron(&file(r#"axis: (ExitDirection, Tuned("fixture"))"#)).is_ok());
        for undeclared in [
            r#"axis: (ExitDirection, Tuned("fixture")), half_angle_deg: (25.0, Tuned("fixture"))"#,
            r#"axis: (ExitDirection, Tuned("fixture")), profile: (Flat, Tuned("fixture"))"#,
            r#"axis: (ExitDirection, Tuned("fixture")), range: (6.0, Tuned("fixture"))"#,
        ] {
            assert!(
                ConeShape::from_ron(&file(undeclared)).is_err(),
                "an unknown field is refused: {undeclared}"
            );
        }
    }

    /// A shape value authored without its provenance is refused at parse, the calibration's too.
    #[test]
    fn a_shape_value_without_provenance_is_refused() {
        let sourced = file(r#"axis: (ExitDirection, Tuned("fixture"))"#);
        assert!(ConeShape::from_ron(&sourced).is_ok());
        assert_eq!(
            calibration_from_ron(&sourced),
            Ok(Calibration {
                hp_per_reference_mm: 70,
                reference_calibre: 0.088,
            })
        );
        for bare in [
            r#"(axis: ExitDirection, directions: (24, Resolution("fixture")),
                passage: (Partial, Tuned("fixture")), hp_per_reference_mm: (70, Tuned("fixture")),
                reference_calibre_m: (0.088, Tuned("fixture")))"#,
            r#"(axis: (ExitDirection, Tuned("fixture")), directions: 24,
                passage: (Partial, Tuned("fixture")), hp_per_reference_mm: (70, Tuned("fixture")),
                reference_calibre_m: (0.088, Tuned("fixture")))"#,
            r#"(axis: (ExitDirection, Tuned("fixture")), directions: (24, Resolution("fixture")),
                passage: FirstSolid, hp_per_reference_mm: (70, Tuned("fixture")),
                reference_calibre_m: (0.088, Tuned("fixture")))"#,
            r#"(axis: (ExitDirection, Tuned("fixture")), directions: (24, Resolution("fixture")),
                passage: (Partial, Tuned("fixture")), hp_per_reference_mm: 70,
                reference_calibre_m: (0.088, Tuned("fixture")))"#,
            r#"(axis: (ExitDirection, Tuned("fixture")), directions: (24, Resolution("fixture")),
                passage: (Partial, Tuned("fixture")), hp_per_reference_mm: (70, Tuned("fixture")),
                reference_calibre_m: 0.088)"#,
        ] {
            let error = ConeShape::from_ron(bare).expect_err(bare);
            assert!(error.contains("provenance"), "{bare}: {error}");
            let error = calibration_from_ron(bare).expect_err(bare);
            assert!(error.contains("provenance"), "{bare}: {error}");
        }
    }

    /// A calibration of zero HP points per reference-mm is refused at parse, naming the field: a
    /// zero rate would silently switch every cone off through the HP quantum gate.
    #[test]
    fn a_zero_field_calibration_is_refused() {
        let zero = r#"(axis: (ExitDirection, Tuned("fixture")),
            directions: (24, Resolution("fixture")), passage: (Partial, Tuned("fixture")),
            hp_per_reference_mm: (0, Tuned("fixture")),
            reference_calibre_m: (0.088, Tuned("fixture")))"#;
        let error = calibration_from_ron(zero).expect_err("a zero rate is refused");
        assert!(error.contains("hp_per_reference_mm"), "{error}");
    }

    /// The reference calibre is the file's: a shape file's value is what the calibration carries,
    /// and the shipped one is the shipped file's. One that is zero, negative, subnormal or not
    /// finite is refused at parse naming the field: every cone's strength divides by it.
    #[test]
    fn the_reference_calibre_is_read_from_the_file_and_must_be_positive() {
        let file = |reference: &str| {
            format!(
                r#"(axis: (ExitDirection, Tuned("fixture")),
                directions: (24, Resolution("fixture")), passage: (Partial, Tuned("fixture")),
                hp_per_reference_mm: (70, Tuned("fixture")),
                reference_calibre_m: ({reference}, Tuned("fixture")))"#
            )
        };
        let read = calibration_from_ron(&file("0.05")).unwrap();
        assert_eq!(read.reference_calibre, 0.05);
        assert_eq!(
            reference_calibre(),
            calibration_from_ron(SPALL_CONE_RON)
                .unwrap()
                .reference_calibre
        );
        for reference in ["0.0", "-0.088", "1.0e-40", "NaN", "inf"] {
            let error = calibration_from_ron(&file(reference)).expect_err(reference);
            assert!(
                error.contains("reference_calibre_m"),
                "{reference}: {error}"
            );
        }
    }

    /// The ball's domain is `λ ≥ 1`: a ratio below 1 is refused naming the APHE burst as the slice
    /// that adds the full sphere, a non-finite ratio is refused, and a ratio so large that the
    /// rim's `cos α` rounds to 1 is refused as unresolved (at `λ = 10⁹`, `1 − cos α ≈ 5·10⁻¹⁹`,
    /// DERIVED). Every ratio the law resolves spreads at every direction count. The throw, the
    /// substance registry and the wire's decoder rely on the refusal to keep a cone inside the law.
    #[test]
    fn a_ratio_outside_the_balls_domain_is_refused() {
        let shape = shape(24);
        for ratio in [0.999, 0.5, 0.0, -2.0] {
            let error = shape.spread(FieldDescriptor::ball(ratio)).unwrap_err();
            assert!(error.contains("APHE burst"), "λ = {ratio}: {error}");
            assert!(FieldDescriptor::ball(ratio).check().is_err());
        }
        for ratio in [f32::NAN, f32::INFINITY] {
            let error = shape.spread(FieldDescriptor::ball(ratio)).unwrap_err();
            assert!(error.contains("finite"), "λ = {ratio}: {error}");
        }
        let error = shape.spread(FieldDescriptor::ball(1.0e9)).unwrap_err();
        assert!(error.contains("rounds to 1"), "{error}");
        for ratio in RATIOS {
            for directions in [1, 24, 97, MAX_DIRECTIONS] {
                assert!(
                    self::shape(directions)
                        .spread(FieldDescriptor::ball(ratio))
                        .is_ok(),
                    "λ = {ratio}, N = {directions}"
                );
            }
        }
    }

    /// CLASS INVARIANT 1, EXACT EMISSION: with nothing in the cone, `(1/N) Σ f_k = ½` to float
    /// precision at every `N` and every `λ ≥ 1`, so a cone emits `S/2` whatever its width and
    /// resolution. The field's calibration (`hp_per_reference_mm`) and the throw's HP-quantum gate
    /// rely on it. Sampling the density at each stratum's midpoint in place of the stratum's share
    /// fails at small `N`; a dropped `N/2` fails everywhere.
    #[test]
    fn the_emitted_total_is_half_the_strength_at_every_resolution_and_ratio() {
        for ratio in RATIOS {
            for directions in [1, 2, 7, 24, 64, 97, 1024] {
                let emitted =
                    weights(&spread(directions, ratio)).iter().sum::<f64>() / directions as f64;
                assert!(
                    (emitted - 0.5).abs() <= EMISSION_TOLERANCE,
                    "λ = {ratio}, N = {directions}: emitted {emitted} of S"
                );
            }
        }
    }

    /// The ball's density per steradian, `p_Ω = D(D² + 3λ²μ²)/(2π)` (m = 3, design doc §5),
    /// stated apart from the code's cumulative share.
    fn ball_density(ratio: f64, mu: f64) -> f64 {
        let d2 = 1.0 - ratio * ratio * (1.0 - mu * mu);
        if d2 <= 0.0 {
            return 0.0;
        }
        let d = d2.sqrt();
        d * (d2 + 3.0 * ratio * ratio * mu * mu) / (2.0 * std::f64::consts::PI)
    }

    /// CLASS INVARIANT 5, PROFILE SHAPE: the weights are the ball's. Each `f_k` is `N/2` of the
    /// ball's density integrated over its stratum (an independent midpoint sum of `2π p_Ω dμ`,
    /// 4096 steps a stratum, within 2.1e-8 of the exact share in f64, MEASURED; the f32 weights
    /// add at most 1.2e-7, DERIVED; tolerance 1e-6); the weights never grow from the axis to the
    /// rim; the rim stratum is a graze (below a tenth of the axis's at 64 strata, and falling as
    /// `N^(−½)`: 0.497–0.500 of it at 256, DERIVED, bound 0.6); and at `λ = 1` the share inside
    /// each stratum edge is `1 − μ⁴`. Every direction lies
    /// inside the cone. A flat profile, the rim-bright shell law (`m = 1`) and a wrong exponent
    /// each fail here; the look's centre-bright field and the graze a box near the rim takes rely
    /// on it.
    #[test]
    fn the_weights_are_the_balls() {
        for ratio in [1.0, 1.5, 2.27, 4.0] {
            let lambda = ratio as f64;
            let coarse = spread(64, ratio);
            let f = weights(&coarse);
            let w = 1.0 - (1.0 - 1.0 / (lambda * lambda)).sqrt();
            let steps = 4096;
            for (k, weight) in f.iter().enumerate() {
                let low = 1.0 - w * (k + 1) as f64 / 64.0;
                let h = (w / 64.0) / steps as f64;
                let share: f64 = (0..steps)
                    .map(|i| {
                        let mu = low + h * (i as f64 + 0.5);
                        2.0 * std::f64::consts::PI * ball_density(lambda, mu) * h
                    })
                    .sum();
                assert!(
                    (weight - 32.0 * share).abs() < 1.0e-6,
                    "λ = {ratio}, stratum {k}: {weight} against {}",
                    32.0 * share
                );
            }
            assert!(
                f.windows(2).all(|pair| pair[1] <= pair[0]),
                "λ = {ratio}: non-increasing {f:?}"
            );
            assert!(
                f[63] < 0.1 * f[0],
                "λ = {ratio}: rim {} axis {}",
                f[63],
                f[0]
            );
            let fine = weights(&spread(256, ratio));
            assert!(
                fine[255] < 0.6 * f[63],
                "λ = {ratio}: the rim stratum falls with N: {} at 256, {} at 64",
                fine[255],
                f[63]
            );
            let axis = Dir3::new(Vec3::new(0.3, -0.2, 0.9)).unwrap();
            for (direction, _) in coarse.directions(axis) {
                let angle = direction.angle_between(Vec3::from(axis));
                assert!(
                    angle <= coarse.half_angle() + 1.0e-5,
                    "{angle} past the rim"
                );
            }
        }
        let mut inside = 0.0;
        for (k, weight) in weights(&spread(64, 1.0)).iter().enumerate() {
            inside += weight / 32.0;
            let edge = 1.0 - (k + 1) as f64 / 64.0;
            let want = 1.0 - edge * edge * edge * edge;
            assert!(
                (inside - want).abs() < 1.0e-6,
                "edge {k}: {inside} inside, 1 − μ⁴ = {want}"
            );
        }
    }

    /// The continuous weight is the spread's at the limit: on the axis it is half the centre
    /// weight `(1 + 3λ²)(1 − cos α)`, it is zero at and past the rim, and at a stratum's midpoint
    /// of a fine spread it is that stratum's weight (DERIVED: a stratum's share over its width is
    /// the density at its midpoint to second order; tolerance 1e-4 at 4096 strata). The pen cam's
    /// drawing of a cone recorded without strengths relies on it.
    #[test]
    fn the_continuous_weight_is_the_spreads_limit() {
        for ratio in [1.0, 2.27, 4.0] {
            let field = FieldDescriptor::ball(ratio);
            let lambda = ratio as f64;
            let cos_alpha = (1.0 - 1.0 / (lambda * lambda)).sqrt();
            let centre = (1.0 + 3.0 * lambda * lambda) * (1.0 - cos_alpha);
            assert!((field.weight_at(1.0) - 0.5 * centre).abs() < 1.0e-12);
            assert_eq!(field.weight_at(cos_alpha - 1.0e-9), 0.0);
            assert_eq!(field.weight_at(-1.0), 0.0);
            for [mu, .., weight] in spread(4096, ratio).strata().step_by(97) {
                let want = field.weight_at(mu as f64);
                assert!(
                    (weight as f64 - want).abs() < 1.0e-4,
                    "λ = {ratio}, μ = {mu}: {weight} against {want}"
                );
            }
        }
    }

    /// The strata are equal in solid angle: their `cos θ` midpoints are evenly spaced and span the
    /// cap from the axis to the rim `asin(1/λ)`.
    #[test]
    fn the_strata_are_equal_in_solid_angle() {
        let spread = spread(24, RATIO);
        assert!(
            (spread.half_angle() - (1.0 / RATIO).asin()).abs() < 1.0e-6,
            "the rim is asin(1/λ)"
        );
        let cos_alpha = spread.half_angle().cos();
        let width = (1.0 - cos_alpha) / 24.0;
        for (k, [mu, ..]) in spread.strata().enumerate() {
            let expected = 1.0 - width * (k as f32 + 0.5);
            assert!((mu - expected).abs() < 1.0e-6, "stratum {k}");
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
        let along = ConeShape::new(AxisRule::ExitDirection, 24, Passage::Partial).unwrap();
        assert_eq!(along.axis(Some(normal), oblique), oblique);
    }

    /// The directions and weights are the same bits on every target: pinned raw IEEE bits of a
    /// few directions for two fixture axes. MEASURED on macOS arm64; a red run on another target
    /// means the generator is not reproducible there.
    #[test]
    fn the_directions_are_pinned_to_the_bit() {
        let spread = spread(24, RATIO);
        let axes = [Dir3::Z, Dir3::new(Vec3::new(0.3, -0.2, 0.9)).unwrap()];
        let mut bits = Vec::new();
        for axis in axes {
            let directions: Vec<(Dir3, f32)> = spread.directions(axis).collect();
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
        [0xbd7c_0403, 0x0000_0000, 0x3f7f_83d6, 0x3f51_733f],
        [0xbe2f_7939, 0x3ddf_3e69, 0x3f7a_aa31, 0x3f30_2361],
        [0xbdb9_6be7, 0x3ece_0de5, 0x3f69_3447, 0x3db1_e40d],
        [0x3e80_3da4, 0xbe52_d5ba, 0x3f72_2b8d, 0x3f51_733f],
        [0x3e17_0fd6, 0xbdc3_392d, 0x3f7c_0537, 0x3f30_2361],
        [0x3e63_916b, 0x3e52_d1ce, 0x3f73_f864, 0x3db1_e40d],
    ];
}
