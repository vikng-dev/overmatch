//! Terrain load observed by the Driver, in N along the engaged ladder. No velocity, contact
//! force or crank state enters this observation. See architecture §5a
//! for the measurement record and the accepted Driver-law design for its use.

use bevy::math::Vec3;

use super::transmission::{TransmissionParams, TransmissionState};

/// Numerical force deadband: observer and crank arithmetic have different scales.
#[derive(bevy::prelude::Resource, Clone, Copy)]
pub(crate) struct DriverResolution {
    pub(crate) observer_weight_share: f32,
    pub(crate) crank_roundoff_share: f32,
}

impl DriverResolution {
    /// L is homogeneous in weight; cancellation in the crank acceleration instead follows
    /// J*omega/dt reflected through the gearbox. Neither term is a vehicle-specific Newton floor.
    pub(crate) fn margin_n(&self, weight_n: f32, tp: &TransmissionParams) -> f32 {
        self.observer_weight_share * weight_n
            + self.crank_roundoff_share * tp.roundoff_force_scale()
    }

    pub(crate) fn load() -> Self {
        static DATA: std::sync::LazyLock<DriverResolution> = std::sync::LazyLock::new(|| {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Source {
                observer_weight_share: crate::provenance::Labelled<f32>,
                crank_roundoff_share: crate::provenance::Labelled<f32>,
            }
            const PATH: &str = "assets/driving/driver.ron";
            let source: Source = crate::provenance::from_shipped_str(
                PATH,
                include_str!("../../assets/driving/driver.ron"),
            )
            .unwrap_or_else(|error| panic!("{error}"));
            for (field, number) in [
                ("observer_weight_share", &source.observer_weight_share),
                ("crank_roundoff_share", &source.crank_roundoff_share),
            ] {
                assert!(
                    matches!(number.label, crate::provenance::Provenance::Resolution(_))
                        && number.value.is_finite()
                        && number.value > 0.0,
                    "{PATH}: {field} must be finite, positive and Resolution; remeasure the observer and same-instant crank residual separately"
                );
            }
            DriverResolution {
                observer_weight_share: source.observer_weight_share.value,
                crank_roundoff_share: source.crank_roundoff_share.value,
            }
        });
        *DATA
    }
}

/// Geometry and material facts for a support patch, collected before the normal-velocity
/// damper can unload it. `length_weight` is patch length times the collocation-column share;
/// it weights uniform nominal pressure, never spring or damper force.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SupportFact {
    pub point: Vec3,
    pub half_span: Vec3,
    pub normal: Vec3,
    pub length_weight: f32,
    pub mu: f32,
    pub rolling_coefficient: f32,
}

/// Separated terms keep the observation auditable beside the reaction-derived demand.
#[derive(Clone, Copy, Debug, Default)]
pub struct PerceivedLoad {
    pub grade_n: f32,
    pub rolling_n: f32,
    pub turn_n: f32,
    /// Uniform-pressure resisting moment (N m), before choosing a gear-specific radius.
    pub turn_moment_nm: f32,
    pub grip_n: f32,
}

impl PerceivedLoad {
    pub fn total_n(self) -> f32 {
        self.grade_n + self.rolling_n + self.turn_n
    }
}

/// Current detent's declared travel radius; straight travel has none. At rest this prices
/// impending travel under that detent, independently of the instantaneous belt speeds.
pub fn turn_radius(tp: &TransmissionParams, state: &TransmissionState) -> Option<f32> {
    let (tight, wide) = tp.steering_radii(state.reverse, state.gear);
    match state.steer_step {
        0 => None,
        1 => Some(wide),
        2 => Some(tight),
        _ => panic!("the transmission admits only straight, wide and tight detents"),
    }
}

/// Both driving adapters observe the same ladder frame and nominal construction weight.
pub(crate) fn observe_rig<'a>(
    contacts: impl Iterator<Item = &'a SupportFact> + Clone,
    forward: Vec3,
    fp: &super::forces::ForceParams,
    tp: &TransmissionParams,
    state: &TransmissionState,
) -> PerceivedLoad {
    let weight = 2.0
        * fp.grip
            .expect("a drivetrain requires grip construction")
            .side_weight;
    observe(
        contacts,
        forward * if state.reverse { -1.0 } else { 1.0 },
        weight,
        turn_radius(tp, state),
    )
}

/// `L = W sin(theta) + c_rr W cos(theta) + mu W cos(theta) length/(4 radius)`.
/// The uniform-pressure footprint model weights patches by their geometric length and column
/// share; actual spring/damper loads never weight the observation. Forward is projected into
/// the support plane, so hull pitch alone cannot manufacture a grade. The footprint's span is
/// measured along that tangent. An airborne rig (no upward support) observes no terrain load.
/// Coefficients and normals are the same facts the contact law read, with no second oracle query.
pub fn observe<'a>(
    contacts: impl Iterator<Item = &'a SupportFact> + Clone,
    forward: Vec3,
    weight: f32,
    radius: Option<f32>,
) -> PerceivedLoad {
    let contacts = contacts.filter(|contact| contact.normal.y > 0.0);
    let count = contacts.clone().count();
    if count == 0 {
        return PerceivedLoad::default(); // Airborne: there is no support to perceive.
    }
    let length_weight = contacts.clone().map(|c| c.length_weight).sum::<f32>();
    let normal = (contacts
        .clone()
        .map(|c| c.normal * c.length_weight)
        .sum::<Vec3>()
        / length_weight)
        .normalize();
    let tangent = forward - normal * forward.dot(normal);
    if tangent.length_squared() == 0.0 {
        return PerceivedLoad::default(); // Nose perpendicular to support: no travel axis on it.
    }
    let tangent = tangent.normalize();
    let rolling = contacts
        .clone()
        .map(|c| c.rolling_coefficient * c.length_weight)
        .sum::<f32>()
        / length_weight;
    let mu = contacts
        .clone()
        .map(|c| c.mu * c.length_weight)
        .sum::<f32>()
        / length_weight;
    let (min, max) = contacts
        .flat_map(|c| {
            [
                (c.point - c.half_span).dot(tangent),
                (c.point + c.half_span).dot(tangent),
            ]
        })
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), point| {
            (min.min(point), max.max(point))
        });
    PerceivedLoad {
        grade_n: weight * tangent.y,
        rolling_n: rolling * weight * normal.y,
        // The exact factor 1/4 is the uniform-pressure resisting-moment integral.
        turn_n: radius.map_or(0.0, |r| mu * weight * normal.y * (max - min) / (4.0 * r)),
        turn_moment_nm: mu * weight * normal.y * (max - min) / 4.0,
        grip_n: mu * weight * normal.y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Quat;

    // Fixture: two endpoints of a uniform contact patch.
    fn patch(angle: f32, rolling: f32) -> [SupportFact; 2] {
        let rot = Quat::from_rotation_x(angle);
        [-2.0, 2.0].map(|z| SupportFact {
            point: rot * Vec3::new(0.0, 0.0, z),
            normal: rot * Vec3::Y,
            half_span: Vec3::ZERO,
            length_weight: 1.0,
            mu: 0.7,
            rolling_coefficient: rolling,
        })
    }

    /// The observer's relative resolution must cover actual numerical observation error at
    /// independent weights; an arbitrarily small positive share must fail this witness.
    #[test]
    fn observer_resolution_bounds_independent_weights() {
        let resolution = DriverResolution::load();
        for mass in [20_000.0, 60_000.0] {
            // Fixture: independent nominal masses.
            let weight = mass * super::super::derive::G;
            for degrees in [10.0_f32, 20.0, 30.0] {
                // Fixture: support planes.
                let angle = degrees.to_radians();
                let contacts = patch(angle, 0.12);
                let actual = observe(contacts.iter(), Vec3::NEG_Z, weight, None).total_n();
                let expected = weight * (angle.sin() + 0.12 * angle.cos());
                assert!((actual - expected).abs() < resolution.observer_weight_share * weight);
            }
        }
    }

    /// The gear-law observer must price gravity and rolling from support geometry at rest as
    /// well as in motion. A load/reaction-based observer or a reversed grade sign fails.
    #[test]
    fn perceived_load_is_weight_times_grade_and_ground_coefficient() {
        // Fixture: course inclines, two coefficients, and nominal weight; no asset expectation.
        let weight = 560_000.0;
        for degrees in [0.0_f32, 10.0, 20.0, 30.0] {
            let angle = degrees.to_radians();
            for rolling in [0.02, 0.12] {
                let contacts = patch(angle, rolling);
                for direction in [-1.0, 1.0] {
                    let actual = observe(contacts.iter(), direction * Vec3::NEG_Z, weight, None);
                    let expected = weight * (direction * angle.sin() + rolling * angle.cos());
                    assert!((actual.total_n() - expected).abs() < weight * 1e-6);
                }
            }
        }
    }

    /// A mixed footprint must price its own geometric shares, never whichever patch happens
    /// to report first. The Driver relies on a continuous load across a ground boundary.
    #[test]
    fn perceived_ground_load_weights_the_supported_footprint() {
        // Fixture: unequal footprint shares and coefficients, with a separately varied weight.
        let mut contacts = patch(0.0, 0.02);
        contacts[1].rolling_coefficient = 0.12;
        contacts[1].length_weight = 3.0;
        for weight in [280_000.0, 560_000.0, 1_120_000.0] {
            let actual = observe(contacts.iter(), Vec3::NEG_Z, weight, None);
            let expected = weight * (0.02 + 3.0 * 0.12) / 4.0;
            assert!((actual.rolling_n - expected).abs() < weight * 1e-6);
        }
    }

    /// Hull pitch on a planar support must not become a slope observation; the Driver reads
    /// the ground. A hull-up-based observer fails when recoil or ride pitches the hull.
    #[test]
    fn perceived_load_reads_support_pitch_instead_of_hull_pitch() {
        // Fixture: level support with a pitching hull, constant heading and weight.
        let contacts = patch(0.0, 0.05);
        let before = observe(contacts.iter(), Vec3::NEG_Z, 560_000.0, Some(10.0)).total_n();
        for pitch in [-0.2, 0.0, 0.2] {
            let forward = Quat::from_rotation_x(pitch) * Vec3::NEG_Z;
            assert_eq!(
                observe(contacts.iter(), forward, 560_000.0, Some(10.0)).total_n(),
                before
            );
        }
    }

    /// The Driver prices the whole patch and normal pressure on a slope, not centroid span
    /// alone or weight along gravity. Fixture: a single finite patch on a pitched plane.
    #[test]
    fn turn_load_reads_half_span_and_normal_pressure() {
        let rot = Quat::from_rotation_x(30.0_f32.to_radians());
        let contact = SupportFact {
            point: Vec3::ZERO,
            half_span: rot * Vec3::Z * 2.0,
            normal: rot * Vec3::Y,
            length_weight: 4.0,
            mu: 0.7,
            rolling_coefficient: 0.05,
        };
        let actual = observe([contact].iter(), rot * Vec3::NEG_Z, 560_000.0, Some(10.0));
        let expected = 0.7 * 560_000.0 * contact.normal.y * 4.0 / 40.0;
        assert!((actual.turn_n - expected).abs() < 0.1);
    }

    /// Rolling turns cost the uniform-pressure resisting moment divided by radius; the gear
    /// law relies on tighter turns costing more even before lateral shear has developed.
    #[test]
    fn perceived_turn_load_scales_with_weight_friction_span_and_inverse_radius() {
        let mut contacts = patch(0.0, 0.05);
        // Fixture: fixed weight and span, independently varied mu and radius; 1/4 is exact.
        for scale in [0.5, 1.0, 2.0] {
            contacts[0].mu = 0.7 * scale;
            contacts[1].mu = 0.7 * scale;
            let actual = observe(contacts.iter(), Vec3::NEG_Z, 560_000.0, Some(10.0));
            assert!((actual.turn_n - 0.7 * scale * 560_000.0 * 4.0 / 40.0).abs() < 0.1);
            let wider = observe(contacts.iter(), Vec3::NEG_Z, 560_000.0, Some(20.0));
            assert_eq!(wider.turn_n * 2.0, actual.turn_n);
        }
        assert_eq!(
            observe(contacts.iter(), Vec3::NEG_Z, 560_000.0, None).turn_n,
            0.0
        );
        assert_eq!(
            observe([].iter(), Vec3::NEG_Z, 560_000.0, Some(10.0)).total_n(),
            0.0
        );
    }
}
