//! How hard two bodies stop each other on impact, in the game's environment destruction model
//! (game state only): the elastic contact law a landing (`debris::crumble`) and a hull's ram
//! (`construction::sweep`) both read, from each side's material stiffness and curvature (Hertz;
//! K. L. Johnson, *Contact Mechanics*, Cambridge 1985, §11.4).

use crate::construction::fracture::Structure;
use crate::physical_laws::PhysicalLaws;

/// One side of a contact: its plane-strain compliance `(1 − ν²) / E` (1/Pa, zero where rigid) and
/// its radius of curvature (m, `None` where flat).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Contactor {
    pub(crate) compliance: f64,
    pub(crate) radius: Option<f64>,
}

impl Contactor {
    /// A flat face of a material of Young's modulus `young` (Pa) and Poisson's ratio `poisson`.
    pub(crate) fn face(young: f64, poisson: f64) -> Self {
        Self {
            compliance: (1.0 - poisson * poisson) / young,
            radius: None,
        }
    }

    /// Chunk `c`: its material's face (`ν = E / 2G − 1`, isotropic, from its two moduli) curved to
    /// the radius of the sphere of its volume (a StandIn for the contact patch's own curvature,
    /// which the narrow phase does not report).
    pub(crate) fn chunk(structure: &Structure, c: usize) -> Self {
        let chunk = &structure.chunks[c];
        let (young, shear) = (chunk.material.young, chunk.material.shear_modulus);
        Self {
            radius: Some((3.0 * f64::from(chunk.volume) / (4.0 * std::f64::consts::PI)).cbrt()),
            ..Self::face(young, young / (2.0 * shear) - 1.0)
        }
    }

    /// The ground, flat, with its data ([`PhysicalLaws::ground_youngs_modulus_pa`]).
    pub(crate) fn ground() -> Self {
        let laws = PhysicalLaws::shipped();
        Self::face(laws.ground_youngs_modulus_pa, laws.ground_poisson_ratio)
    }

    /// A body the game states no stiffness for (a hull): rigid and flat, so the chunk's own
    /// compliance governs. StandIn for the body's material stiffness, which no body data carries.
    pub(crate) const RIGID: Self = Self {
        compliance: 0.0,
        radius: None,
    };
}

/// `E*` (Pa) and `√R` (√m) of the contact between `a` and `b` (Hertz; K. L. Johnson, *Contact
/// Mechanics*, Cambridge 1985, §11.4): `1/E* = Σ (1 − ν²)/E`, `1/R = Σ 1/R`.
fn hertz(a: Contactor, b: Contactor) -> (f64, f64) {
    let curvature: f64 = [a.radius, b.radius]
        .iter()
        .flatten()
        .map(|r| r.recip())
        .sum();
    assert!(
        curvature > 0.0,
        "a contact's chunk side is curved: two flat sides make no Hertz contact"
    );
    (
        (a.compliance + b.compliance).recip(),
        curvature.recip().sqrt(),
    )
}

/// The peak force (N) of an elastic impact closing at `speed` m/s with effective mass `mass` kg
/// between `a` and `b` ([`hertz`]): maximum approach `δ = (15 m v² / (16 E* √R))^(2/5)` and peak
/// force `P = (4/3) E* √R δ^(3/2)`. The contact's own stiffness sets how hard it stops, so no
/// solver step enters it.
pub(crate) fn peak_force(mass: f64, speed: f64, a: Contactor, b: Contactor) -> f64 {
    let (stiffness, root) = hertz(a, b);
    let approach = (15.0 * mass * speed * speed / (16.0 * stiffness * root)).powf(0.4);
    4.0 / 3.0 * stiffness * root * approach.powf(1.5)
}

/// The closing speed (m/s) at which [`peak_force`] reaches `force` N, its inverse:
/// `δ = (3P / (4 E* √R))^(2/3)` and `v = √(16 E* √R δ^(5/2) / (15 m))`.
pub(crate) fn peak_speed(mass: f64, force: f64, a: Contactor, b: Contactor) -> f64 {
    let (stiffness, root) = hertz(a, b);
    let approach = (3.0 * force / (4.0 * stiffness * root)).powf(2.0 / 3.0);
    (16.0 * stiffness * root * approach.powf(2.5) / (15.0 * mass)).sqrt()
}
