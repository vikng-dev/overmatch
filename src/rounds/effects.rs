//! The game's damage-model view: authored flight emissions, resolved with the round registry.
//!
//! Every round states both emission presences with provenance. Numeric fields are finite and in
//! their rendering domains before a shot exists; renderers consume these records without guessing
//! from calibre, ammunition names or viewer distance. Units are SI, colours linear HDR.

use crate::provenance::Sourced;
use serde::Deserialize;

/// The round's flight emissions, present on the shell from complete spawn on every role. View
/// plugins alone consume them. An absent emission is an authored construction fact.
#[derive(bevy::prelude::Component, Clone, Copy, Debug, PartialEq)]
pub(crate) struct FlightEffects {
    pub(crate) ember: Option<EmberEmission>,
    pub(crate) smoke: Option<SmokeEmission>,
}

impl FlightEffects {
    /// Explicitly untraced, smokeless fixture data; never a missing-data default.
    #[cfg(test)]
    pub(crate) const NONE: Self = Self {
        ember: None,
        smoke: None,
    };

    /// FIXTURE DATA for renderer and transport tests. No shipped round reads this constructor.
    #[cfg(test)]
    pub(crate) fn fixture_traced() -> Self {
        Self {
            ember: Some(EmberEmission {
                radius_m: 0.09,
                base_offset_m: 0.35,
                burn_s: 2.0,
                fade_s: 0.4,
                emissive: [10.0, 2.5, 0.5],
            }),
            smoke: Some(SmokeEmission {
                lifetime_s: 2.2,
                width_m: 0.35,
                growth_mps: 1.3,
                lateral_accel_mps2: 0.4,
                rise_accel_mps2: 0.35,
                start_m: 10.0,
                fade_m: 20.0,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EmberEmission {
    pub(crate) radius_m: f32,
    /// Offset along the shell's trailing local +Z, metres.
    pub(crate) base_offset_m: f32,
    pub(crate) burn_s: f32,
    pub(crate) fade_s: f32,
    pub(crate) emissive: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SmokeEmission {
    pub(crate) lifetime_s: f32,
    pub(crate) width_m: f32,
    pub(crate) growth_mps: f32,
    /// Coefficients in displacement = coefficient × age², m/s².
    pub(crate) lateral_accel_mps2: f32,
    pub(crate) rise_accel_mps2: f32,
    /// No smoke ribbon inside this travelled arc from the muzzle, metres.
    pub(crate) start_m: f32,
    /// The ribbon's birth taper after `start_m`, metres.
    pub(crate) fade_m: f32,
}

/// Required presences: a `Sourced<Option<_>>` refuses an omitted field as well as missing
/// provenance. Serde's implicit absent-Option default cannot invent an untraced round here.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct File {
    ember: Sourced<Option<EmberFile>>,
    smoke: Sourced<Option<SmokeFile>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmberFile {
    radius_m: Sourced<f32>,
    base_offset_m: Sourced<f32>,
    burn_s: Sourced<f32>,
    fade_s: Sourced<f32>,
    emissive: Sourced<(f32, f32, f32)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SmokeFile {
    lifetime_s: Sourced<f32>,
    width_m: Sourced<f32>,
    growth_mps: Sourced<f32>,
    lateral_accel_mps2: Sourced<f32>,
    rise_accel_mps2: Sourced<f32>,
    start_m: Sourced<f32>,
    fade_m: Sourced<f32>,
}

fn number(field: &str, value: f32, positive: bool) -> Result<f32, String> {
    if !value.is_finite() || value < 0.0 || (positive && value == 0.0) {
        return Err(format!(
            "effects.{field} must be finite and {} (got {value}) — author a valid emission value",
            if positive { "positive" } else { "nonnegative" }
        ));
    }
    Ok(value)
}

impl File {
    pub(super) fn resolve(self) -> Result<FlightEffects, String> {
        let ember = self.ember.0.map(|file| -> Result<EmberEmission, String> {
            let (r, g, b) = file.emissive.0;
            let emissive = [number("ember.emissive.red", r, false)?,
                number("ember.emissive.green", g, false)?, number("ember.emissive.blue", b, false)?];
            if emissive.iter().all(|&channel| channel == 0.0) {
                return Err("effects.ember.emissive states no emission — author a nonzero colour or explicitly no ember".into());
            }
            Ok(EmberEmission {
                radius_m: number("ember.radius_m", file.radius_m.0, true)?,
                base_offset_m: number("ember.base_offset_m", file.base_offset_m.0, false)?,
                burn_s: number("ember.burn_s", file.burn_s.0, true)?,
                fade_s: number("ember.fade_s", file.fade_s.0, true)?, emissive,
            })
        }).transpose()?;
        let smoke = self
            .smoke
            .0
            .map(|file| -> Result<SmokeEmission, String> {
                Ok(SmokeEmission {
                    lifetime_s: number("smoke.lifetime_s", file.lifetime_s.0, true)?,
                    width_m: number("smoke.width_m", file.width_m.0, true)?,
                    growth_mps: number("smoke.growth_mps", file.growth_mps.0, false)?,
                    lateral_accel_mps2: number(
                        "smoke.lateral_accel_mps2",
                        file.lateral_accel_mps2.0,
                        false,
                    )?,
                    rise_accel_mps2: number(
                        "smoke.rise_accel_mps2",
                        file.rise_accel_mps2.0,
                        false,
                    )?,
                    start_m: number("smoke.start_m", file.start_m.0, false)?,
                    fade_m: number("smoke.fade_m", file.fade_m.0, true)?,
                })
            })
            .transpose()?;
        Ok(FlightEffects { ember, smoke })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flight rendering relies on declared presence and positive lifetime/size divisors. A missing
    /// field, missing provenance or unusable value must refuse while the registry loads.
    #[test]
    fn emission_presence_and_numeric_domains_are_required() {
        let none = r#"(ember: (None, Unknown("fixture absence")), smoke: (None, Unknown("fixture absence")))"#;
        assert_eq!(
            ron::from_str::<File>(none).unwrap().resolve().unwrap(),
            FlightEffects::NONE
        );
        for missing in [
            "()",
            r#"(ember: (None, Unknown("fixture")))"#,
            r#"(ember: None, smoke: (None, Unknown("fixture")))"#,
        ] {
            assert!(ron::from_str::<File>(missing).is_err());
        }
        for (field, value, positive) in [
            ("ember.radius_m", 0.0, true),
            ("ember.fade_s", -1.0, true),
            ("smoke.start_m", f32::NAN, false),
            ("smoke.width_m", f32::INFINITY, true),
        ] {
            assert!(number(field, value, positive).unwrap_err().contains(field));
        }
    }
}
