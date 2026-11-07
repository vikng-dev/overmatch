//! The game's environment destruction and damage model: shared physical data, refused before play.

use crate::provenance::Sourced;
use serde::Deserialize;

const PATH: &str = "assets/ballistics/physical_laws.ron";
const RON: &str = include_str!("../assets/ballistics/physical_laws.ron");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    reflected_impulse: Sourced<SwisdakFit>,
    incident_impulse: Sourced<SwisdakFit>,
    incident_pressure: Sourced<SwisdakFit>,
    reflected_pressure: Sourced<SwisdakFit>,
    tnt_energy_j_per_kg: Sourced<f32>,
    surface_burst_factor: Sourced<f32>,
    gravity_mps2: Sourced<f32>,
    air_density: Sourced<f64>,
    cavity_inertia: Sourced<f64>,
    plate_capacity: Sourced<crate::ballistics::plate_capacity::PlateCapacity>,
    contact_friction: Sourced<f32>,
    server_rubble_min_size_m: Sourced<f32>,
    ground_youngs_modulus_pa: Sourced<f64>,
    ground_poisson_ratio: Sourced<f64>,
}

#[derive(Debug)]
pub(crate) struct PhysicalLaws {
    pub reflected_impulse: SwisdakFit,
    pub incident_impulse: SwisdakFit,
    pub incident_pressure: SwisdakFit,
    pub reflected_pressure: SwisdakFit,
    pub tnt_energy_j_per_kg: f32,
    pub surface_burst_factor: f32,
    pub gravity_mps2: f32,
    pub air_density: f64,
    pub cavity_inertia: f64,
    pub plate_capacity: crate::ballistics::plate_capacity::PlateCapacity,
    pub contact_friction: f32,
    /// The smallest fragment the server keeps as a body: the cube root of its occupied volume, m
    /// (`crate::debris::crumble::server_body`).
    pub server_rubble_min_size_m: f32,
    /// The ground's Young's modulus (Pa) and Poisson's ratio: its side of a landing's contact
    /// stiffness (`crate::debris::crumble`).
    pub ground_youngs_modulus_pa: f64,
    pub ground_poisson_ratio: f64,
}

impl PhysicalLaws {
    fn from_ron(text: &str) -> Result<Self, String> {
        let file: File = ron::de::from_str(text).map_err(|error| format!("{PATH}: {error}"))?;
        for (field, value) in [
            ("gravity_mps2", f64::from(file.gravity_mps2.0)),
            ("tnt_energy_j_per_kg", f64::from(file.tnt_energy_j_per_kg.0)),
            (
                "surface_burst_factor",
                f64::from(file.surface_burst_factor.0),
            ),
            ("air_density", file.air_density.0),
            ("cavity_inertia", file.cavity_inertia.0),
            ("contact_friction", f64::from(file.contact_friction.0)),
            (
                "server_rubble_min_size_m",
                f64::from(file.server_rubble_min_size_m.0),
            ),
            ("ground_youngs_modulus_pa", file.ground_youngs_modulus_pa.0),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!(
                    "{PATH}: {field}: author a finite positive SI value"
                ));
            }
        }
        if !(0.0..0.5).contains(&file.ground_poisson_ratio.0) {
            return Err(format!(
                "{PATH}: ground_poisson_ratio: author an isotropic ratio in [0, 0.5)"
            ));
        }
        file.plate_capacity.0.validate(PATH)?;
        for (field, fit) in [
            ("reflected_impulse", &file.reflected_impulse.0),
            ("incident_impulse", &file.incident_impulse.0),
            ("incident_pressure", &file.incident_pressure.0),
            ("reflected_pressure", &file.reflected_pressure.0),
        ] {
            fit.validate(field)?;
        }
        Ok(Self {
            reflected_impulse: file.reflected_impulse.0,
            incident_impulse: file.incident_impulse.0,
            incident_pressure: file.incident_pressure.0,
            reflected_pressure: file.reflected_pressure.0,
            tnt_energy_j_per_kg: file.tnt_energy_j_per_kg.0,
            surface_burst_factor: file.surface_burst_factor.0,
            gravity_mps2: file.gravity_mps2.0,
            air_density: file.air_density.0,
            cavity_inertia: file.cavity_inertia.0,
            plate_capacity: file.plate_capacity.0,
            contact_friction: file.contact_friction.0,
            server_rubble_min_size_m: file.server_rubble_min_size_m.0,
            ground_youngs_modulus_pa: file.ground_youngs_modulus_pa.0,
            ground_poisson_ratio: file.ground_poisson_ratio.0,
        })
    }

    pub(crate) fn shipped() -> &'static Self {
        static LAWS: std::sync::LazyLock<PhysicalLaws> = std::sync::LazyLock::new(|| {
            crate::provenance::shipped(PATH, || PhysicalLaws::from_ron(RON))
                .unwrap_or_else(|error| panic!("{error}"))
        });
        &LAWS
    }
}

/// A metric field fit: `Z = R / W^(1/3)` (m/kg^(1/3)), evaluated as
/// `exp(sum c[k] (ln Z)^k)`. Impulse is Pa·s/kg^(1/3); pressure is kPa.
/// Admission certifies ordered positive domains and finite positive results. Reads outside
/// the authored domain hold its edge value; the field's walk ends at its impulse fit's edge.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SwisdakFit {
    from: f32,
    pub(crate) branches: Vec<(f32, Vec<f32>)>,
}

impl SwisdakFit {
    fn validate(&self, field: &str) -> Result<(), String> {
        let refused = || {
            format!(
                "{PATH}: {field}: author ordered positive domains, nonempty finite coefficients and finite positive fit values"
            )
        };
        if !self.from.is_finite() || self.from <= 0.0 || self.branches.is_empty() {
            return Err(refused());
        }
        let mut from = self.from;
        for (to, coefficients) in &self.branches {
            if !to.is_finite()
                || *to <= from
                || coefficients.is_empty()
                || coefficients.iter().any(|c| !c.is_finite())
            {
                return Err(refused());
            }
            // Resolution: subdivide logarithmic domains for interval Horner bounds. This
            // limits overestimation from cancellation; admission runs once, never per ray.
            const INTERVALS: usize = 64;
            let start = from.ln();
            let end = to.ln();
            let mut low = start;
            for step in 1..=INTERVALS {
                let high = if step == INTERVALS {
                    end
                } else {
                    start + (end - start) * (step as f32 / INTERVALS as f32)
                };
                let (mut min, mut max) = (0.0, 0.0);
                for c in coefficients.iter().rev() {
                    let products = [min * low, min * high, max * low, max * high];
                    if products.iter().any(|p: &f32| !p.is_finite()) {
                        return Err(refused());
                    }
                    min = products.iter().copied().fold(f32::INFINITY, f32::min) + c;
                    max = products.iter().copied().fold(f32::NEG_INFINITY, f32::max) + c;
                }
                if !min.is_finite()
                    || !max.is_finite()
                    || min.exp() <= 0.0
                    || !max.exp().is_finite()
                {
                    return Err(refused());
                }
                low = high;
            }
            from = *to;
        }
        Ok(())
    }

    /// The last scaled distance a branch covers, m/kg^⅓.
    pub(crate) fn edge(&self) -> f32 {
        self.branches[self.branches.len() - 1].0
    }

    /// The fit's value at scaled distance `z`, in its own unit.
    pub(crate) fn at(&self, z: f32) -> f32 {
        let held = z.clamp(self.from, self.edge());
        let (_, c) = self
            .branches
            .iter()
            .find(|(to, _)| held <= *to)
            .expect("held is within the last branch's edge");
        let l = held.ln();
        c.iter().rev().fold(0.0, |sum, ck| sum * l + ck).exp()
    }
}

/// Uniform environment acceleration, m/s², shared by flight and construction laws.
pub(crate) fn gravity_mps2() -> f32 {
    PhysicalLaws::shipped().gravity_mps2
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Its own numbers, apart from the shipped file's, so re-authoring shipped data cannot turn the
    /// refusal test red.
    const VALID: &str = r#"(
        gravity_mps2: (9.0, Fixture("fixture")),
        tnt_energy_j_per_kg: (4.0e6, Fixture("fixture")),
        surface_burst_factor: (1.5, Fixture("fixture")),
        air_density: (1.5, Fixture("fixture")),
        cavity_inertia: (2.0, Fixture("fixture")),
        plate_capacity: ((shear_slope: 1.5, opening_share: 0.6, return_power: 1.5), Fixture("fixture")),
        contact_friction: (0.25, Fixture("fixture")),
        server_rubble_min_size_m: (20.0, Fixture("fixture")),
        ground_youngs_modulus_pa: (3.0e7, Fixture("fixture")),
        ground_poisson_ratio: (0.25, Fixture("fixture")),
        reflected_impulse: ((from: 1.0, branches: [(2.0, [0.0])]), Fixture("fixture")),
        incident_impulse: ((from: 1.0, branches: [(2.0, [0.0])]), Fixture("fixture")),
        incident_pressure: ((from: 1.0, branches: [(2.0, [0.0])]), Fixture("fixture")),
        reflected_pressure: ((from: 1.0, branches: [(2.0, [0.0])]), Fixture("fixture")),
    )"#;

    /// The game's damage model refuses bad physical data before a shell flies, and the shipped file
    /// passes. Mutant: unchecked NaN.
    #[test]
    fn shared_physical_data_is_required_and_validated() {
        PhysicalLaws::shipped();
        PhysicalLaws::from_ron(VALID).unwrap();
        for (field, good) in [
            ("gravity_mps2", "9.0"),
            ("tnt_energy_j_per_kg", "4.0e6"),
            ("surface_burst_factor", "1.5"),
            ("air_density", "1.5"),
            ("cavity_inertia", "2.0"),
            ("contact_friction", "0.25"),
            ("server_rubble_min_size_m", "20.0"),
            ("ground_youngs_modulus_pa", "3.0e7"),
        ] {
            for bad in ["0.0", "-1.0", "NaN", "inf"] {
                let text =
                    VALID.replace(&format!("{field}: ({good},"), &format!("{field}: ({bad},"));
                let error = PhysicalLaws::from_ron(&text).unwrap_err();
                assert!(
                    error.contains(PATH) && error.contains(field) && error.contains("author"),
                    "{error}"
                );
            }
        }
    }
    /// The game's damage model: the file gate, not first contact, admits the capacity law.
    /// Mutants: omit the required field or bypass its validator. Values belong to VALID.
    #[test]
    fn plate_capacity_is_required_and_checked_at_file_admission() {
        let missing = VALID
            .lines()
            .filter(|line| !line.trim_start().starts_with("plate_capacity:"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            PhysicalLaws::from_ron(&missing)
                .unwrap_err()
                .contains("plate_capacity")
        );
        for (field, good, bad) in [
            ("shear_slope", "1.5", "0.9"),
            ("opening_share", "0.6", "1.0"),
            ("return_power", "1.5", "NaN"),
        ] {
            let text = VALID.replace(&format!("{field}: {good}"), &format!("{field}: {bad}"));
            let error = PhysicalLaws::from_ron(&text).unwrap_err();
            assert!(error.contains(PATH) && error.contains(field) && error.contains("author"));
        }
    }

    /// The game's field laws need ordered positive domains before evaluating logarithms.
    /// Removing any fit's admission check admits malformed authored branches.
    #[test]
    fn field_fit_domains_are_refused_before_play() {
        for field in [
            "reflected_impulse",
            "incident_impulse",
            "incident_pressure",
            "reflected_pressure",
        ] {
            let offset = RON.find(&format!("{field}: ((")).unwrap();
            let start = offset + RON[offset..].find("from: ").unwrap() + "from: ".len();
            let end = start + RON[start..].find(',').unwrap();
            for bad in ["0.0", "-1.0", "NaN", "inf", "1000.0"] {
                let mut text = RON.to_owned();
                text.replace_range(start..end, bad);
                let error = PhysicalLaws::from_ron(&text).unwrap_err();
                assert!(
                    error.contains(PATH) && error.contains(field) && error.contains("author"),
                    "{error}"
                );
            }
        }
    }
    /// Fixture fits exercise every admission obligation, including overflow/underflow between
    /// branch edges; field evaluation must never invent a finite fallback for malformed data.
    #[test]
    fn field_fits_refuse_empty_unordered_and_nonfinite_values() {
        let valid = SwisdakFit {
            from: 1.0,
            branches: vec![(2.0, vec![0.0])],
        };
        valid.validate("fixture").unwrap();
        for branches in [
            Vec::new(),
            vec![(2.0, Vec::new())],
            vec![(2.0, vec![f32::NAN])],
            vec![(2.0, vec![f32::INFINITY])],
            vec![(2.0, vec![f32::MAX])],
            vec![(2.0, vec![-f32::MAX])],
            vec![(2.0, vec![0.0]), (1.5, vec![0.0])],
            // A concave quadratic is finite at both ends but overflows at ln(Z) = 1/2.
            vec![(std::f32::consts::E, vec![0.0, 400.0, -400.0])],
        ] {
            assert!(
                SwisdakFit {
                    from: 1.0,
                    branches
                }
                .validate("fixture")
                .is_err()
            );
        }
    }
}
