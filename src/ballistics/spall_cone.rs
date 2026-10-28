//! The spall cone's shape — its axis rule and the directions it is sampled by —, the field law
//! that weights those directions and sets the cone's half-angle ([`FieldDescriptor`]), the cone's
//! strength per joule of the energy its round lost across the body it left ([`energy_share`]), and
//! a burst's field data ([`BurstFields`]), read from `assets/ballistics/spall_cone.ron`.
//! The law the shape serves — strength, profile, shadowing, dose — is design doc §5
//! (`.agents/docs/design/armor-penetration-and-damage.md`); the throw that applies it is
//! `throw_spall_cone` in the parent module, and a world may throw another shape ([`SpallLaw`]).
//!
//! # Units
//!
//! A field's strength `S` is energy, in joules: a cone's is [`energy_share`] of the kinetic energy
//! its round lost across the body it left (`walk::SpallSource::strength`); a burst's is its own
//! driven energy, the kinetic energy its explosive gives the wall (`burst::Burst::energy`), times
//! the burst block's factor ([`BurstFields::strength`]), nothing the round lost entering it. Every
//! field deposit is joules too, and turns
//! into HP points at the one energy-to-HP rate every hard pool's line dose converts at
//! (`energy_dose::joules_per_hp`, through `ballistics::energy_units`).
//!
//! # The field law
//!
//! A field is described by a law and one shape number, its ratio `λ`, the forward speed over the
//! greatest sideways one ([`FieldDescriptor`]). Under [`FieldLaw::Ball`] the share of the field's
//! emission inside polar angle `θ` of its axis is `F(μ) = 1 − μ·D^m`, `μ = cos θ`,
//! `D = √(1 − λ²(1 − μ²))`, `m` = [`BALL_EXPONENT`]: the field is brightest on the axis and falls to
//! zero at the rim `α = asin(1/λ)`. A plate exit's `λ` is its rear layer's substance's
//! (`crate::substances::HardSubstance::debris_ratio`), at least 1, so the half-angle is the exit
//! material's. The law also holds below `λ = 1`, where it fills the whole sphere:
//! `F = (1 + λ − μg)(2 + λ − λ² + λμg)/4`, `g = λμ + D`, the `m = 3` law's closed form, uniform at
//! `λ = 0`; no shipped field is thrown there, a plate exit's `λ` being at least 1. Under
//! [`FieldLaw::Ring`], a burst's field (design doc §5a, "The burst"), the field is uniform over the
//! band `θ_c ∓ h` about `θ_c = atan(1/λ)`, its near edge held at the axis.
//!
//! # Directions
//!
//! `N` directions in equal-solid-angle strata over the field's support `[t_lo, t_hi]` in depth
//! `t = 1 − cos θ` — the cap `[0, 1 − cos α]`, the sphere `[0, 2]`, the band: direction `k` sits at
//! `t_k = t_lo + (t_hi − t_lo)(k + ½)/N`, the stratum's midpoint, at golden-angle azimuth
//! `φ_k = k·π(3 − √5)`, and stands for `1/N` of the field's solid angle. Its weight is its stratum's
//! exact share of the emission, `f_k = (N/2)[F(e_{k+1}) − F(e_k)]` over the stratum's edges
//! `e_j = t_lo + (t_hi − t_lo)·j/N`, so a field of strength `S` sends `S·f_k` along it, and the
//! emitted total `(1/N) Σ S·f_k` telescopes to `S/2` at every `N`.
//!
//! # Determinism
//!
//! The azimuths are built once per shape, at parse, in f64 with `libm`'s `cos`/`sin`, and kept in
//! f64: the platform's own `sin`/`cos` are not the same on every shipped target. A cone's `μ_k`,
//! `sin θ_k` and `f_k` are built from its descriptor in f64 with IEEE `+`, `−`, `×`, `/` and `sqrt`
//! alone, each correctly rounded, then cast to f32, the ring's band edges with `libm`'s `cos`/`sin`
//! of its half-width, so a decoder on any target rebuilds the same bits from the same descriptor;
//! its half-angle is `libm`'s `asin`, the ring's `atan2`. Placing a stratum on an axis is IEEE f32
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
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum FieldLaw {
    /// Sideways debris speeds uniform in a ball about the forward speed, the greatest `1/λ` of
    /// it: share inside `θ` `F(μ) = 1 − μ·D^m`, `m` = [`BALL_EXPONENT`], rim `α = asin(1/λ)`;
    /// brightest on the axis, zero at the rim. Below `λ = 1` the whole sphere, under the `m = 3`
    /// law's closed form.
    Ball,
    /// A burst's ring: uniform over the band `half_width` (rad, in `(0, π/2)`) to either side of
    /// `θ_c = atan(1/λ)`, the near edge held at the axis.
    Ring { half_width: f32 },
}

impl FieldLaw {
    /// Every law's name, in declaration order: a law's index here is the variant index its
    /// serialized form carries.
    pub(crate) const NAMES: [&'static str; 2] = ["Ball", "Ring"];
}

/// A field's law and its one shape number: what a record and the wire carry per cone. With the
/// shot vocabulary's [`ConeShape`] it rebuilds the cone's directions and weights
/// ([`ConeShape::spread`]).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct FieldDescriptor {
    pub law: FieldLaw,
    /// `λ`: the debris's forward speed over its greatest sideways speed, finite and not negative.
    /// A plate exit's is at least 1 (`α ≤ 90°`); a burst's is zero at rest.
    pub ratio: f32,
}

/// A law's terms for one descriptor: its support in depth `t = 1 − cos θ` and its share there.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Terms {
    /// The ball at `λ ≥ 1`: `λ²` and the cap's depth `w = 1 − cos α`.
    Cap { l2: f64, w: f64 },
    /// The ball at `λ < 1`, over the whole sphere.
    Sphere { lambda: f64 },
    /// The ring over its band `[lo, hi]`.
    Band { lo: f64, hi: f64 },
}

impl Terms {
    /// `[t_lo, t_hi]`, the support in depth.
    pub(crate) fn support(&self) -> (f64, f64) {
        match *self {
            Terms::Cap { w, .. } => (0.0, w),
            Terms::Sphere { .. } => (0.0, 2.0),
            Terms::Band { lo, hi } => (lo, hi),
        }
    }

    /// The share of the emission between depths `a ≤ b` of the axis. The cap's is the difference
    /// of its share inside `t`, `1 − μ·D^m`, `D² = 1 − λ²·t(2 − t)` (`= 1 − λ²(1 − μ²)`, without
    /// the cancellation), held at zero past the rim, `D^m` `m` IEEE products; the band's the
    /// difference of its share, linear across it. The sphere's is the `m = 3` law's (`exponent`
    /// unread) density `g³/2` integrated over the stratum,
    /// `(b − a)(g_a + g_b)²(g_a² + g_b² + 2c) / (16(D_a + D_b))`, `c = 1 − λ²`,
    /// `D = √(c + λ²μ²)`, `g = λμ + D`, taken behind the plate as `c / (D − λμ)`: every factor is a
    /// sum or product of non-negative terms, where the difference of two nearly saturated shares
    /// rounds below zero just under `λ = 1`.
    fn stratum(&self, a: f64, b: f64, exponent: u32) -> f64 {
        match *self {
            Terms::Cap { l2, .. } => {
                let share = |t: f64| {
                    let d = (1.0 - l2 * (t * (2.0 - t))).max(0.0).sqrt();
                    1.0 - (1.0 - t) * (0..exponent).fold(1.0, |power, _| power * d)
                };
                share(b) - share(a)
            }
            Terms::Sphere { lambda } => {
                let c = (1.0 - lambda) * (1.0 + lambda);
                let reach = |t: f64| {
                    let mu = 1.0 - t;
                    let d = (c + lambda * mu * (lambda * mu)).sqrt();
                    let g = if mu >= 0.0 {
                        lambda * mu + d
                    } else {
                        c / (d - lambda * mu)
                    };
                    (g, d)
                };
                let ((g_a, d_a), (g_b, d_b)) = (reach(a), reach(b));
                let sum = g_a + g_b;
                (b - a) * (sum * sum) * (g_a * g_a + g_b * g_b + 2.0 * c) / (16.0 * (d_a + d_b))
            }
            Terms::Band { lo, hi } => {
                let share = |t: f64| ((t - lo) / (hi - lo)).clamp(0.0, 1.0);
                share(b) - share(a)
            }
        }
    }
}

impl FieldDescriptor {
    /// The ball law at ratio `ratio`.
    pub(crate) const fn ball(ratio: f32) -> Self {
        Self {
            law: FieldLaw::Ball,
            ratio,
        }
    }

    /// The ring law of half-width `half_width` (rad) at ratio `ratio`.
    pub(crate) const fn ring(ratio: f32, half_width: f32) -> Self {
        Self {
            law: FieldLaw::Ring { half_width },
            ratio,
        }
    }

    /// α (rad), the field's rim, the greatest angle off the axis it reaches: the ball's
    /// `asin(1/λ)`, or π below `λ = 1`; the ring's `atan2(1, λ) + h`. `libm`'s in f64 cast to f32,
    /// so a descriptor gives the same bits on every target. Every reader of a field's half-angle
    /// takes it from here.
    pub(crate) fn half_angle(&self) -> f32 {
        let lambda = self.ratio as f64;
        match self.law {
            FieldLaw::Ball if lambda < 1.0 => std::f32::consts::PI,
            FieldLaw::Ball => libm::asin(1.0 / lambda) as f32,
            FieldLaw::Ring { half_width } => (libm::atan2(1.0, lambda) + half_width as f64) as f32,
        }
    }

    /// The field's inner edge (rad), the least angle off the axis it reaches: the ring's near edge
    /// `atan2(1, λ) − h`, held at the axis (0) where the band reaches it as [`Self::terms`] holds
    /// it; 0 for the ball, which reaches the axis. Computed as [`Self::half_angle`] is, so the two
    /// bound one support on every target.
    pub(crate) fn inner_angle(&self) -> f32 {
        match self.law {
            FieldLaw::Ball => 0.0,
            FieldLaw::Ring { half_width } => {
                (libm::atan2(1.0, self.ratio as f64) - half_width as f64).max(0.0) as f32
            }
        }
    }

    /// The law's terms, refusing a descriptor outside its domain: a ratio that is negative or not
    /// finite, a ball so narrow that the rim's `cos α` rounds to 1 (`λ` above about 10⁸, DERIVED),
    /// and a ring whose half-width is outside `(0, π/2)` or whose band rounds away. Names the
    /// violation.
    pub(crate) fn terms(&self) -> Result<Terms, String> {
        let lambda = self.ratio as f64;
        if !lambda.is_finite() || lambda < 0.0 {
            return Err(format!(
                "a field's ratio must be finite and not negative (got {lambda})"
            ));
        }
        match self.law {
            FieldLaw::Ball if lambda < 1.0 => Ok(Terms::Sphere { lambda }),
            FieldLaw::Ball => {
                let l2 = lambda * lambda;
                let w = 1.0 / (l2 * (1.0 + (1.0 - 1.0 / l2).sqrt()));
                if 1.0 - w >= 1.0 {
                    return Err(format!(
                        "a field of ratio {lambda} is narrower than the law resolves: its rim's \
                         cos α rounds to 1"
                    ));
                }
                Ok(Terms::Cap { l2, w })
            }
            FieldLaw::Ring { half_width } => {
                let h = half_width as f64;
                if !(h > 0.0 && h < std::f64::consts::FRAC_PI_2) {
                    return Err(format!(
                        "a ring's half-width must lie in (0, π/2) rad (got {h})"
                    ));
                }
                // cos θ_c and sin θ_c of θ_c = atan(1/λ), and the band's edges `θ_c ∓ h` in depth.
                let r = (1.0 + lambda * lambda).sqrt();
                let (cos_c, sin_c) = (lambda / r, 1.0 / r);
                let (cos_h, sin_h) = (libm::cos(h), libm::sin(h));
                let hi = 1.0 - (cos_c * cos_h - sin_c * sin_h);
                let lo = if sin_c * cos_h > cos_c * sin_h {
                    1.0 - (cos_c * cos_h + sin_c * sin_h)
                } else {
                    0.0
                };
                // Every term is finite: the ratio and half-width are checked above.
                if hi <= lo {
                    return Err(format!(
                        "a ring of ratio {lambda} and half-width {h} rad is narrower than the law \
                         resolves"
                    ));
                }
                Ok(Terms::Band { lo, hi })
            }
        }
    }

    /// Whether the descriptor lies in its law's domain ([`Self::terms`]); names the violation.
    pub(crate) fn check(&self) -> Result<(), String> {
        self.terms().map(|_| ())
    }

    /// The law's weight along a direction at `mu = cos θ` from the axis: the continuous form of a
    /// spread's `f_k`, `(t_hi − t_lo)/2 · (−dF/dμ)` — under the ball's cap
    /// `½·w·(D^m + m·λ²μ²·D^(m−2))`, zero at and past the rim; over its sphere `(λμ + D)³/2`; across
    /// the ring's band ½, zero off it. Zero for a descriptor outside the law's domain.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the law's density, which the spread's tests read; the pen cam draws its mean over a stratum (`pen_cam::look::law`)"
        )
    )]
    pub(crate) fn weight_at(&self, mu: f64) -> f64 {
        let Ok(terms) = self.terms() else {
            return 0.0;
        };
        let t = 1.0 - mu;
        match terms {
            // `D²` is even in `μ`: past the rim it turns positive again behind the plate, so the
            // cap is bounded by its depth, not by the sign of `D²`.
            Terms::Cap { l2, w } => {
                if t.is_nan() || t >= w {
                    return 0.0;
                }
                let d = (1.0 - l2 * (t * (2.0 - t))).max(0.0).sqrt();
                let power = |n: u32| (0..n).fold(1.0, |p, _| p * d);
                let m = BALL_EXPONENT;
                0.5 * w * (power(m) + m as f64 * l2 * mu * mu * power(m - 2))
            }
            Terms::Sphere { lambda } => {
                if !(-1.0..=1.0).contains(&mu) {
                    return 0.0;
                }
                let d = (1.0 - lambda * lambda * (t * (2.0 - t))).sqrt();
                let reach = lambda * mu + d;
                0.5 * reach * reach * reach
            }
            Terms::Band { lo, hi } => {
                if (lo..=hi).contains(&t) {
                    0.5
                } else {
                    0.0
                }
            }
        }
    }
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
    energy_share: Sourced<f32>,
    burst: BurstFile,
}

/// A burst's field data as authored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BurstFile {
    driven_energy_factor: Sourced<f32>,
    ring_half_width_deg: Sourced<f32>,
    ring_directions: Sourced<usize>,
}

/// A burst's field, validated (design doc §5a, "The burst"): its strength per joule of its driven
/// energy, the ring's half-width, and the shape the ring is thrown in, sampled by its own
/// direction count and passing as the spall cone's directions do.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BurstFields {
    /// Joules of strength `S` per joule of the driven energy `½ M v_s²`; finite, positive and
    /// normal.
    driven_energy_factor: f32,
    /// The ring's half-width `h` (rad), in `(0, π/2)`.
    ring_half_width: f32,
    ring: ConeShape,
}

/// The burst fields a world's march throws, in place of the shipped ones: inserted by a world that
/// measures another direction count (the throw-cost probe, `pen_cam::burst_law_cost`).
#[derive(Resource, Clone, Debug)]
pub(crate) struct BurstLaw(pub BurstFields);

impl BurstFields {
    /// Parse and validate a shape file's burst block.
    pub(crate) fn from_ron(text: &str) -> Result<Self, String> {
        let file: ShapeFile = ron::de::from_str(text).map_err(|err| err.to_string())?;
        let ring = file.burst.ring_directions.0;
        Self::from_file(&file, ring)
    }

    /// Validate `file`'s burst block, its ring sampled by `ring` strata: refuses a driven-energy
    /// factor that is not finite, positive and normal, a half-width outside `(0°, 90°)` and a direction
    /// count outside the shape's domain, naming the field.
    fn from_file(file: &ShapeFile, ring: usize) -> Result<Self, String> {
        let burst = &file.burst;
        let factor = burst.driven_energy_factor.0;
        if !factor.is_normal() || factor < 0.0 {
            return Err(format!(
                "burst.driven_energy_factor must be finite, positive and normal (got {factor})"
            ));
        }
        let degrees = burst.ring_half_width_deg.0;
        if !(degrees > 0.0 && degrees < 90.0) {
            return Err(format!(
                "burst.ring_half_width_deg must lie in (0, 90) (got {degrees})"
            ));
        }
        Ok(Self {
            driven_energy_factor: factor,
            ring_half_width: (degrees as f64).to_radians() as f32,
            ring: ConeShape::new(file.axis.0, ring)
                .map_err(|err| format!("burst.ring_directions: {err}"))?,
        })
    }

    /// The shipped burst fields with the ring sampled by `ring` strata in place of its own count:
    /// a probe of how the throw's cost scales with `N`.
    #[cfg(test)]
    pub(crate) fn shipped_at(ring: usize) -> Self {
        let file: ShapeFile = ron::de::from_str(SPALL_CONE_RON).expect("the shipped shape parses");
        Self::from_file(&file, ring)
            .expect("the shipped burst fields validate at any direction count")
    }

    /// The shipped burst fields, parsed once. Panics on a malformed file, as
    /// [`ConeShape::shipped`] does; [`super::sim_plugin`] reads them while the app is built.
    pub(crate) fn shipped() -> &'static Self {
        static FIELDS: std::sync::LazyLock<BurstFields> = std::sync::LazyLock::new(|| {
            BurstFields::from_ron(SPALL_CONE_RON).unwrap_or_else(|err| {
                panic!("spall cone: embedded spall_cone.ron's burst block is unusable: {err}")
            })
        });
        &FIELDS
    }

    /// The strength `S` (J) of a burst whose explosive drives its wall with `energy` J, the
    /// driven energy `½ M v_s²` (`burst::Burst::energy`): `energy × driven_energy_factor`, one f64
    /// product cast once, so the same bits on every target. The field carries its own driven
    /// energy, never a share of what the round lost reaching the burst point.
    pub(crate) fn strength(&self, energy: f32) -> f32 {
        (energy as f64 * self.driven_energy_factor as f64) as f32
    }

    /// The shape every burst field is thrown in, the ring's: a decoder rebuilds a burst field's
    /// directions from it.
    pub(crate) fn shape(&self) -> &ConeShape {
        &self.ring
    }

    /// The field a burst throws at ratio `ratio` (its carried speed over its greatest sideways
    /// speed), the ring at the shipped half-width, and the shape it is thrown in.
    pub(crate) fn field(&self, ratio: f32) -> (FieldDescriptor, &ConeShape) {
        (
            FieldDescriptor::ring(ratio, self.ring_half_width),
            &self.ring,
        )
    }
}

/// The cone's strength per joule of the kinetic energy its round lost across the body it left,
/// parsed once. Panics on a malformed file, as [`ConeShape::shipped`] does; [`super::sim_plugin`]
/// reads it while the app is built.
///
/// A cone's strength `S` (J) is this share of that energy ([`super::walk::SpallSource::strength`]),
/// and the cone emits `S/2` ([`EMITTED_SHARE`]); its deposits convert at the energy rate every hard
/// pool's line dose converts at ([`super::energy_units`]). It sets the potency of every cone at once
/// and nothing else: the shape and the field law both conserve a cone's emitted total.
pub(crate) fn energy_share() -> f32 {
    static SHARE: std::sync::LazyLock<f32> = std::sync::LazyLock::new(|| {
        energy_share_from_ron(SPALL_CONE_RON).unwrap_or_else(|err| {
            panic!("spall cone: embedded spall_cone.ron's energy share is unusable: {err}")
        })
    });
    *SHARE
}

/// Parse a shape file's cone energy share, refusing one that is not finite, positive and normal: a
/// zero share would switch every cone off through the pool quantum gate, and a negative one would
/// throw negative strengths.
fn energy_share_from_ron(text: &str) -> Result<f32, String> {
    let file: ShapeFile = ron::de::from_str(text).map_err(|err| err.to_string())?;
    let share = file.energy_share.0;
    if !share.is_normal() || share < 0.0 {
        return Err(format!(
            "energy_share must be finite, positive and normal (got {share})"
        ));
    }
    Ok(share)
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
    /// `(cos φ_k, sin φ_k)` of each direction's golden-angle azimuth, in stratum order.
    azimuths: Vec<(f64, f64)>,
}

/// One cone's directions and weights: a [`ConeShape`]'s strata under a [`FieldDescriptor`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Spread {
    field: FieldDescriptor,
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
    /// A shape of `directions` strata about an axis chosen by `axis`. Refuses a direction count
    /// outside `1..=MAX_DIRECTIONS`.
    pub(crate) fn new(axis: AxisRule, directions: usize) -> Result<Self, String> {
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
        Ok(Self { axis, azimuths })
    }

    /// Parse and validate a shape file.
    pub(crate) fn from_ron(text: &str) -> Result<Self, String> {
        let file: ShapeFile = ron::de::from_str(text).map_err(|err| err.to_string())?;
        let directions = file.directions.0;
        Self::from_file(file, directions)
    }

    /// Validate `file`'s shape sampled by `directions` strata.
    fn from_file(file: ShapeFile, directions: usize) -> Result<Self, String> {
        Self::new(file.axis.0, directions)
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

    /// This shape's strata under `field`: each direction placed in its stratum of the support
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
        if matches!(terms, Terms::Sphere { .. }) && exponent != BALL_EXPONENT {
            return Err(format!(
                "the ball below a ratio of 1 is the exponent-{BALL_EXPONENT} law's closed form \
                 (got exponent {exponent})"
            ));
        }
        let n = self.azimuths.len() as f64;
        let (lo, hi) = terms.support();
        let width = hi - lo;
        let edge = |k: usize| lo + width * k as f64 / n;
        let strata: Vec<Stratum> = self
            .azimuths
            .iter()
            .enumerate()
            .map(|(k, &(cos_phi, sin_phi))| {
                let t = lo + width * (k as f64 + 0.5) / n;
                let sin_theta = (t * (2.0 - t)).sqrt();
                Stratum {
                    mu: (1.0 - t) as f32,
                    lateral: ((sin_theta * cos_phi) as f32, (sin_theta * sin_phi) as f32),
                    weight: (0.5 * n * terms.stratum(edge(k), edge(k + 1), exponent)) as f32,
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
        Ok(Spread { field, strata })
    }
}

impl Spread {
    /// The descriptor it was built from.
    pub(crate) fn field(&self) -> FieldDescriptor {
        self.field
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

    /// Ratios below 1, where the ball fills the whole sphere: at rest, slow, and just under the
    /// hemisphere. Fixture data.
    const SPHERE_RATIOS: [f32; 5] = [0.0, 0.3, 0.45, 0.9, 0.999];

    /// Ring descriptors `(λ, h°)`: at rest, slow, near the band's edge reaching the axis, past it,
    /// and a wide band. Fixture data.
    const RINGS: [(f32, f32); 5] = [
        (0.0, 10.0),
        (0.45, 10.0),
        (5.6, 10.0),
        (100.0, 10.0),
        (1.0, 45.0),
    ];

    fn ring(ratio: f32, degrees: f32) -> FieldDescriptor {
        FieldDescriptor::ring(ratio, degrees.to_radians())
    }

    /// Every field the law's invariants are checked across: the ball at every ratio, above and
    /// below 1, and every ring.
    fn fields() -> Vec<FieldDescriptor> {
        RATIOS
            .iter()
            .chain(&SPHERE_RATIOS)
            .map(|&ratio| FieldDescriptor::ball(ratio))
            .chain(RINGS.iter().map(|&(ratio, degrees)| ring(ratio, degrees)))
            .collect()
    }

    fn shape(directions: usize) -> ConeShape {
        ConeShape::new(AxisRule::RearNormalBisector, directions).unwrap()
    }

    fn spread(directions: usize, ratio: f32) -> Spread {
        shape(directions)
            .spread(FieldDescriptor::ball(ratio))
            .unwrap()
    }

    fn weights(spread: &Spread) -> Vec<f64> {
        spread.strata().map(|[.., f]| f as f64).collect()
    }

    /// A fixture shape file whose first fields are `fields`, its cone's energy share 0.5.
    fn file(fields: &str) -> String {
        format!(
            r#"({fields}, directions: (24, Resolution("fixture")),
            energy_share: (0.5, Tuned("fixture")), {BURST})"#
        )
    }

    /// A fixture burst block, its driven-energy factor 1.0.
    const BURST: &str = r#"burst: (driven_energy_factor: (1.0, Tuned("fixture")),
        ring_half_width_deg: (10.0, Tuned("fixture")), ring_directions: (24, Tuned("fixture")))"#;

    /// A fixture shape file whose burst block's fields are `fields`, its cone's energy share 0.5.
    fn burst_file(fields: &str) -> String {
        format!(
            r#"(axis: (ExitDirection, Tuned("fixture")), directions: (24, Resolution("fixture")),
            energy_share: (0.5, Tuned("fixture")), burst: ({fields}))"#
        )
    }

    /// A shape outside its domain is refused at parse, and so is a slot the shape does not declare:
    /// the half-angle is the exit material's, the profile the law's and a direction's passage
    /// through what it meets the resolver's, so a file that authors any of them is refused rather
    /// than read past.
    #[test]
    fn a_shape_outside_the_domain_is_refused() {
        assert!(ConeShape::new(AxisRule::ExitDirection, 0).is_err());
        assert!(ConeShape::new(AxisRule::ExitDirection, MAX_DIRECTIONS + 1).is_err());
        assert!(ConeShape::from_ron(&file(r#"axis: (ExitDirection, Tuned("fixture"))"#)).is_ok());
        for undeclared in [
            r#"axis: (ExitDirection, Tuned("fixture")), half_angle_deg: (25.0, Tuned("fixture"))"#,
            r#"axis: (ExitDirection, Tuned("fixture")), profile: (Flat, Tuned("fixture"))"#,
            r#"axis: (ExitDirection, Tuned("fixture")), range: (6.0, Tuned("fixture"))"#,
            r#"axis: (ExitDirection, Tuned("fixture")), passage: (Partial, Tuned("fixture"))"#,
        ] {
            assert!(
                ConeShape::from_ron(&file(undeclared)).is_err(),
                "an unknown field is refused: {undeclared}"
            );
        }
    }

    /// A shape value authored without its provenance is refused at parse, the cone's energy share
    /// and the burst block's values too.
    #[test]
    fn a_shape_value_without_provenance_is_refused() {
        let sourced = file(r#"axis: (ExitDirection, Tuned("fixture"))"#);
        assert!(ConeShape::from_ron(&sourced).is_ok());
        assert!(BurstFields::from_ron(&sourced).is_ok());
        assert_eq!(energy_share_from_ron(&sourced), Ok(0.5));
        for (value, bare) in [
            (
                r#"axis: (ExitDirection, Tuned("fixture"))"#,
                "axis: ExitDirection",
            ),
            (
                r#"directions: (24, Resolution("fixture"))"#,
                "directions: 24",
            ),
            (
                r#"energy_share: (0.5, Tuned("fixture"))"#,
                "energy_share: 0.5",
            ),
            (
                r#"driven_energy_factor: (1.0, Tuned("fixture"))"#,
                "driven_energy_factor: 1.0",
            ),
            (
                r#"ring_half_width_deg: (10.0, Tuned("fixture"))"#,
                "ring_half_width_deg: 10.0",
            ),
            (
                r#"ring_directions: (24, Tuned("fixture"))"#,
                "ring_directions: 24",
            ),
        ] {
            let bare = sourced.replacen(value, bare, 1);
            assert_ne!(bare, sourced, "{value}");
            let error = ConeShape::from_ron(&bare).expect_err(&bare);
            assert!(error.contains("provenance"), "{bare}: {error}");
            let error = energy_share_from_ron(&bare).expect_err(&bare);
            assert!(error.contains("provenance"), "{bare}: {error}");
            let error = BurstFields::from_ron(&bare).expect_err(&bare);
            assert!(error.contains("provenance"), "{bare}: {error}");
        }
    }

    /// The cone's energy share is the file's: a shape file's value is what the parse carries, and
    /// the shipped one is the shipped file's. One that is zero, negative, subnormal or not finite
    /// is refused at parse naming the field: every cone's strength is that share of the energy its
    /// round lost, so a zero would silently switch every cone off through the pool quantum gate and
    /// a negative one throw negative strengths.
    #[test]
    fn the_cone_energy_share_is_read_from_the_file_and_must_be_positive() {
        let with = |share: &str| {
            file(r#"axis: (ExitDirection, Tuned("fixture"))"#).replacen(
                "energy_share: (0.5",
                &format!("energy_share: ({share}"),
                1,
            )
        };
        assert_eq!(energy_share_from_ron(&with("2.5")), Ok(2.5));
        assert_eq!(
            energy_share(),
            energy_share_from_ron(SPALL_CONE_RON).unwrap()
        );
        for share in ["0.0", "-0.5", "1.0e-40", "NaN", "inf"] {
            let error = energy_share_from_ron(&with(share)).expect_err(share);
            assert!(error.contains("energy_share"), "{share}: {error}");
        }
    }

    /// A burst block is read from the file: its count samples the ring's shape, every burst
    /// field's, its half-width the ring's field, its driven-energy factor the strength
    /// `energy × driven_energy_factor`; the shipped one is the shipped file's. A factor that is
    /// zero, negative, subnormal or not finite, a half-width outside `(0°, 90°)` and a direction
    /// count outside `1..=MAX_DIRECTIONS` are refused at parse naming the field: a burst's
    /// strength scales by the factor, and the ring's law refuses such a band at the first throw. A count for
    /// another burst shape is no field of the block: every burst is a ring.
    #[test]
    fn a_burst_block_is_read_and_its_domain_is_refused() {
        let read = BurstFields::from_ron(&burst_file(
            r#"driven_energy_factor: (2.0, Tuned("fixture")),
            ring_half_width_deg: (30.0, Tuned("fixture")), ring_directions: (24, Tuned("fixture"))"#,
        ))
        .unwrap();
        assert_eq!(read.strength(150_000.0), 300_000.0);
        let (ring, shape) = read.field(0.5);
        assert_eq!(
            ring,
            FieldDescriptor::ring(0.5, (30.0_f64).to_radians() as f32)
        );
        assert_eq!(shape.directions_count(), 24);
        assert_eq!(read.shape(), shape);
        assert_eq!(
            BurstFields::shipped(),
            &BurstFields::from_ron(SPALL_CONE_RON).unwrap()
        );
        let sourced = burst_file(
            r#"driven_energy_factor: (1.0, Tuned("fixture")),
            ring_half_width_deg: (10.0, Tuned("fixture")), ring_directions: (24, Tuned("fixture"))"#,
        );
        let ball = sourced.replacen(
            r#"ring_directions: (24, Tuned("fixture"))"#,
            r#"ring_directions: (24, Tuned("fixture")), ball_directions: (32, Tuned("fixture"))"#,
            1,
        );
        assert_ne!(ball, sourced);
        let error = BurstFields::from_ron(&ball).expect_err("a ball's count is no burst datum");
        assert!(error.contains("ball_directions"), "{error}");
        for (value, refused, field) in [
            (
                "(1.0",
                ["(0.0", "(-1.0", "(1.0e-40", "(NaN", "(inf"].as_slice(),
                "driven_energy_factor",
            ),
            (
                "(10.0",
                ["(0.0", "(-5.0", "(90.0", "(NaN"].as_slice(),
                "ring_half_width_deg",
            ),
            ("(24", ["(0", "(65537"].as_slice(), "ring_directions"),
        ] {
            for refused in refused {
                let text = sourced.replacen(
                    &format!("{field}: {value}"),
                    &format!("{field}: {refused}"),
                    1,
                );
                assert_ne!(text, sourced, "{field}");
                let error = BurstFields::from_ron(&text).expect_err(&text);
                assert!(error.contains(field), "{field} {refused}: {error}");
            }
        }
    }

    /// The law's domain is a finite ratio `λ ≥ 0`: a negative or non-finite ratio is refused under
    /// either law, a ball so narrow that the rim's `cos α` rounds to 1 is refused as unresolved
    /// (at `λ = 10⁹`, `1 − cos α ≈ 5·10⁻¹⁹`, DERIVED), and a ring whose half-width is outside
    /// `(0, π/2)` is refused naming it; the ball below `λ = 1`, the `m = 3` law's closed form, is
    /// refused under another exponent rather than spread as that law. Every field the law resolves
    /// spreads at every direction count, the ball below `λ = 1` among them. The throw, the
    /// substance registry, the wire's decoder and the vocabulary's fingerprint rely on the refusal
    /// to keep a field inside the law.
    #[test]
    fn a_field_outside_the_laws_domain_is_refused() {
        let shape = shape(24);
        for field in [
            FieldDescriptor::ball(-2.0),
            FieldDescriptor::ball(f32::NAN),
            FieldDescriptor::ball(f32::INFINITY),
            ring(-0.5, 10.0),
            ring(f32::NAN, 10.0),
        ] {
            let error = shape.spread(field).unwrap_err();
            assert!(
                error.contains("finite and not negative"),
                "{field:?}: {error}"
            );
            assert!(field.check().is_err());
        }
        let error = shape.spread(FieldDescriptor::ball(1.0e9)).unwrap_err();
        assert!(error.contains("rounds to 1"), "{error}");
        for degrees in [0.0, -5.0, 90.0, f32::NAN] {
            let error = shape.spread(ring(0.45, degrees)).unwrap_err();
            assert!(error.contains("half-width"), "h = {degrees}°: {error}");
        }
        let sphere = FieldDescriptor::ball(0.5);
        assert!(shape.spread_under(sphere, BALL_EXPONENT + 1).is_err());
        for field in fields() {
            for directions in [1, 24, 97, MAX_DIRECTIONS] {
                assert!(
                    self::shape(directions).spread(field).is_ok(),
                    "{field:?}, N = {directions}"
                );
            }
        }
    }

    /// CLASS INVARIANT 1, EXACT EMISSION: with nothing in the field, `(1/N) Σ f_k = ½` to float
    /// precision at every `N`, under the ball at every `λ`, above and below 1, and under every
    /// ring, so a field emits `S/2` whatever its width and resolution. The cone's and the burst's
    /// energy shares (their potency) and the throw's HP-quantum gate rely on it.
    /// Sampling the density at each stratum's midpoint in place of the stratum's share fails at
    /// small `N`; a dropped `N/2` fails everywhere.
    #[test]
    fn the_emitted_total_is_half_the_strength_at_every_resolution_and_ratio() {
        for field in fields() {
            for directions in [1, 2, 7, 24, 64, 97, 1024] {
                let spread = shape(directions).spread(field).unwrap();
                let emitted = weights(&spread).iter().sum::<f64>() / directions as f64;
                assert!(
                    (emitted - 0.5).abs() <= EMISSION_TOLERANCE,
                    "{field:?}, N = {directions}: emitted {emitted} of S"
                );
            }
        }
    }

    /// Every direction's weight `f_k` is at least zero at every resolution up to
    /// [`MAX_DIRECTIONS`], under the ball across its domain (at rest, the least positive and least
    /// normal f32 ratios, up to `λ = 1` in decades of `1 − λ`, the largest f32 below 1, 1 itself
    /// and past it) and under rings there, while the field still emits ½. A negative `f_k` throws a
    /// negative strength along its direction, and [`EMISSION_TOLERANCE`]'s bound assumes none. The
    /// sphere's weights as differences of its share inside each stratum edge fail here just below 1
    /// (MEASURED: at `λ = 0.999999` and `N = 256`, 21 weights below zero, the least −1.4e-14).
    #[test]
    fn every_weight_is_non_negative_across_the_ratios_domain() {
        let near_one = (1..=6).map(|decade| 1.0 - 10.0_f32.powi(-decade));
        let ratios: Vec<f32> = [0.0, f32::from_bits(1), f32::MIN_POSITIVE, 0.5]
            .into_iter()
            .chain(near_one)
            .chain([1.0 - f32::EPSILON / 2.0, 1.0, 1.0 + f32::EPSILON, 4.0])
            .collect();
        let fields = ratios.iter().flat_map(|&ratio| {
            [
                FieldDescriptor::ball(ratio),
                ring(ratio, 10.0),
                ring(ratio, 45.0),
            ]
        });
        let shapes: Vec<ConeShape> = [1, 2, 3, 24, 97, 256, 4096, MAX_DIRECTIONS]
            .into_iter()
            .map(shape)
            .collect();
        for field in fields {
            for shape in &shapes {
                let directions = shape.directions_count();
                let f = weights(&shape.spread(field).unwrap());
                let least = f.iter().copied().fold(f64::INFINITY, f64::min);
                assert!(least >= 0.0, "{field:?}, N = {directions}: f_k = {least}");
                let emitted = f.iter().sum::<f64>() / directions as f64;
                assert!(
                    (emitted - 0.5).abs() <= EMISSION_TOLERANCE,
                    "{field:?}, N = {directions}: emitted {emitted} of S"
                );
            }
        }
    }

    /// The ball's density per steradian below `λ = 1`, `(t₊³ − max(t₋, 0)³)/(4π)`, `t± = λμ ± D`
    /// (design doc §5): the share of a uniform ball of sideways speeds about the forward one along
    /// each direction, stated apart from the code's closed form.
    fn sphere_density(ratio: f64, mu: f64) -> f64 {
        let d = (1.0 - ratio * ratio * (1.0 - mu * mu)).sqrt();
        let (near, far) = (ratio * mu - d, ratio * mu + d);
        (far.max(0.0).powi(3) - near.max(0.0).powi(3)) / (4.0 * std::f64::consts::PI)
    }

    /// Below `λ = 1` the ball fills the whole sphere and its weights are its density's: each `f_k`
    /// is `N/2` of `2π p_Ω dμ` integrated over its stratum (a midpoint sum, 4096 steps a stratum,
    /// tolerance 1e-6, as the cap's), the weights never grow from the axis to the back, the field
    /// is uniform at rest (`λ = 0`, every `f_k` ½) and reaches straight back (the last stratum's
    /// midpoint is half a stratum, `1/N` in `μ`, short of `μ = −1`). At `λ → 1⁻` the share inside
    /// each stratum edge meets the cap's `1 − μ⁴`, so the law is continuous across the hemisphere.
    /// The wire's decoder rebuilds any ball descriptor by this law, and its fingerprint folds a
    /// probe below 1 (`damage_cam::wire`'s `FINGERPRINT_FIELDS`).
    #[test]
    fn below_a_ratio_of_one_the_ball_fills_the_sphere() {
        for ratio in [0.3, 0.45, 0.9] {
            let lambda = ratio as f64;
            let f = weights(&spread(64, ratio));
            let steps = 4096;
            for (k, weight) in f.iter().enumerate() {
                let low = 1.0 - 2.0 * (k + 1) as f64 / 64.0;
                let h = (2.0 / 64.0) / steps as f64;
                let share: f64 = (0..steps)
                    .map(|i| {
                        let mu = low + h * (i as f64 + 0.5);
                        2.0 * std::f64::consts::PI * sphere_density(lambda, mu) * h
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
        }
        let rest = spread(64, 0.0);
        assert!(weights(&rest).iter().all(|f| (f - 0.5).abs() < 1.0e-7));
        assert_eq!(rest.field().half_angle(), std::f32::consts::PI);
        let [back, ..] = rest.strata().last().unwrap();
        assert!((back - (-1.0 + 1.0 / 64.0)).abs() < 1.0e-6, "{back}");
        let mut inside = 0.0;
        for (k, weight) in weights(&spread(64, 0.999_999)).iter().enumerate() {
            inside += weight / 32.0;
            let edge = 1.0 - 2.0 * (k + 1) as f64 / 64.0;
            let want = if edge > 0.0 { 1.0 - edge.powi(4) } else { 1.0 };
            assert!(
                (inside - want).abs() < 1.0e-4,
                "edge {k}: {inside} inside, the cap's {want}"
            );
        }
    }

    /// A ring's directions lie in its band, `θ_c ∓ h` about `θ_c = atan(1/λ)`, its near edge held
    /// at the axis, in equal-solid-angle strata spanning it, each weighted ½: the field is uniform
    /// over the band, its rim is `θ_c + h` and its inner edge the near edge. A ring centred
    /// anywhere but `atan(1/λ)`, a band weighted toward one edge, or an inner edge off the near
    /// edge or not held at the axis, fails here; the burst's ring, its knockout radius and the pen
    /// cam's band, which holds nothing off it, rely on it. 1e-5 rad is the f32 tolerance on an
    /// angle.
    #[test]
    fn a_rings_directions_lie_in_its_band() {
        for (ratio, degrees) in RINGS {
            let field = ring(ratio, degrees);
            let spread = shape(64).spread(field).unwrap();
            let centre = (1.0 / ratio as f64).atan() as f32;
            let h = degrees.to_radians();
            let (near, far) = ((centre - h).max(0.0), centre + h);
            assert!((field.half_angle() - far).abs() < 1.0e-5, "{field:?}");
            assert!((field.inner_angle() - near).abs() < 1.0e-5, "{field:?}");
            let axis = Dir3::new(Vec3::new(0.3, -0.2, 0.9)).unwrap();
            for (direction, weight) in spread.directions(axis) {
                let angle = direction.angle_between(Vec3::from(axis));
                assert!(
                    angle >= near - 1.0e-5 && angle <= far + 1.0e-5,
                    "{field:?}: {angle} outside [{near}, {far}]"
                );
                assert!((weight - 0.5).abs() < 1.0e-6, "{field:?}: {weight}");
            }
            let (cos_near, cos_far) = (near.cos(), far.cos());
            let width = (cos_near - cos_far) / 64.0;
            for (k, [mu, ..]) in spread.strata().enumerate() {
                let expected = cos_near - width * (k as f32 + 0.5);
                assert!((mu - expected).abs() < 1.0e-5, "{field:?}: stratum {k}");
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
                    angle <= coarse.field().half_angle() + 1.0e-5,
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
    /// the density at its midpoint to second order; tolerance 1e-4 at 4096 strata), under the ball
    /// below `λ = 1` and the ring too, the ring's zero off its band. The spread is this density
    /// in strata, and the pen cam's law (`pen_cam::look::law`) its mean over a stratum.
    #[test]
    fn the_continuous_weight_is_the_spreads_limit() {
        for field in SPHERE_RATIOS
            .iter()
            .map(|&ratio| FieldDescriptor::ball(ratio))
            .chain(RINGS.iter().map(|&(ratio, degrees)| ring(ratio, degrees)))
        {
            for [mu, .., weight] in shape(4096).spread(field).unwrap().strata().step_by(97) {
                let want = field.weight_at(mu as f64);
                assert!(
                    (weight as f64 - want).abs() < 1.0e-4,
                    "{field:?}, μ = {mu}: {weight} against {want}"
                );
            }
        }
        let band = ring(0.45, 10.0);
        let centre = (1.0_f64 / 0.45).atan();
        assert_eq!(band.weight_at((centre + 0.2).cos()), 0.0);
        assert_eq!(band.weight_at((centre - 0.2).cos()), 0.0);
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
            (spread.field().half_angle() - (1.0 / RATIO).asin()).abs() < 1.0e-6,
            "the rim is asin(1/λ)"
        );
        let cos_alpha = spread.field().half_angle().cos();
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
        let along = ConeShape::new(AxisRule::ExitDirection, 24).unwrap();
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
