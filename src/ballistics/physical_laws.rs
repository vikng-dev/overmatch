//! The game's damage model: required shared environment and cavity-law data, refused before play.

use crate::provenance::Sourced;
use serde::Deserialize;

const PATH: &str = "assets/ballistics/physical_laws.ron";
const RON: &str = include_str!("../../assets/ballistics/physical_laws.ron");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    gravity_mps2: Sourced<f32>,
    air_density: Sourced<f64>,
    cavity_inertia: Sourced<f64>,
    contact_friction: Sourced<f32>,
    floor_m: Sourced<f32>,
}

#[derive(Debug)]
pub(super) struct PhysicalLaws {
    pub gravity_mps2: f32,
    pub air_density: f64,
    pub cavity_inertia: f64,
    pub contact_friction: f32,
    pub floor_m: f32,
}

impl PhysicalLaws {
    fn from_ron(text: &str) -> Result<Self, String> {
        let file: File = ron::de::from_str(text).map_err(|error| format!("{PATH}: {error}"))?;
        for (field, value) in [
            ("gravity_mps2", f64::from(file.gravity_mps2.0)),
            ("air_density", file.air_density.0),
            ("cavity_inertia", file.cavity_inertia.0),
            ("contact_friction", f64::from(file.contact_friction.0)),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!(
                    "{PATH}: {field}: author a finite positive SI value"
                ));
            }
        }
        if !file.floor_m.0.is_finite() {
            return Err(format!(
                "{PATH}: floor_m: author a finite world boundary in metres"
            ));
        }
        Ok(Self {
            gravity_mps2: file.gravity_mps2.0,
            air_density: file.air_density.0,
            cavity_inertia: file.cavity_inertia.0,
            contact_friction: file.contact_friction.0,
            floor_m: file.floor_m.0,
        })
    }

    pub(super) fn shipped() -> &'static Self {
        static LAWS: std::sync::LazyLock<PhysicalLaws> = std::sync::LazyLock::new(|| {
            crate::provenance::shipped(PATH, || PhysicalLaws::from_ron(RON))
                .unwrap_or_else(|error| panic!("{error}"))
        });
        &LAWS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game's damage model refuses bad physical data before a shell flies. Mutant: unchecked NaN.
    #[test]
    fn shared_physical_data_is_required_and_validated() {
        PhysicalLaws::shipped();
        for (field, good) in [
            ("gravity_mps2", "9.81"),
            ("air_density", "1.225"),
            ("cavity_inertia", "1.0"),
            ("contact_friction", "0.5"),
        ] {
            for bad in ["0.0", "-1.0", "NaN", "inf"] {
                let text = RON.replace(&format!("{field}: ({good},"), &format!("{field}: ({bad},"));
                let error = PhysicalLaws::from_ron(&text).unwrap_err();
                assert!(
                    error.contains(PATH) && error.contains(field) && error.contains("author"),
                    "{error}"
                );
            }
        }
    }
}
