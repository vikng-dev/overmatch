//! The game's damage model: the shell walk's angle laws, read from
//! `assets/ballistics/angle_laws.ron` ([`AngleLaws`]). At a face a round meets from air,
//! [`super::walk::begin`] decides overmatch, then ricochet, then the entry bend from the round's
//! hit-nose factor, its potency at the face and the crossing's factor-weighted thickness over the
//! calibre of what meets the plate. The laws, their inputs and the reference case the provisional
//! coefficients keep are design doc §4a (`.agents/docs/design/armor-penetration-and-damage.md`).

use serde::Deserialize;

use crate::provenance::Sourced;

/// The shipped laws, embedded so they are read while the app is built (ADR-0014: built from data,
/// synchronously).
const ANGLE_LAWS_RON: &str = include_str!("../../assets/ballistics/angle_laws.ron");

/// The file as authored: every value with its provenance ([`crate::provenance`]).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AngleLawsFile {
    ricochet_grip: Sourced<f32>,
    normalization_turn: Sourced<f32>,
    overmatch_ratio: Sourced<f32>,
}

/// The three angle-law coefficients, each finite and positive. Dimensionless.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AngleLaws {
    /// A round that is not overmatched skids off a face when its incidence `θ` from the normal has
    /// `tan θ > ricochet_grip × N × P / t`: `N` its hit-nose factor, `P` its potency at the face
    /// and `t` the crossing's steel-equivalent thickness along the normal, both metres of
    /// reference steel.
    pub ricochet_grip: f32,
    /// A round that bites in straightens toward the normal through
    /// `η × normalization_turn × N × θ × (entrance factor / reference factor)`.
    pub normalization_turn: f32,
    /// Overmatch when the calibre of what meets the plate is at least this × `t`.
    pub overmatch_ratio: f32,
}

impl AngleLaws {
    /// Parse an angle-law file, refusing a coefficient that is not finite and positive: a zero
    /// grip would bounce every oblique round, a zero ratio overmatch every plate.
    pub(crate) fn from_ron(text: &str) -> Result<Self, String> {
        let file: AngleLawsFile = ron::de::from_str(text).map_err(|err| err.to_string())?;
        let laws = Self {
            ricochet_grip: file.ricochet_grip.0,
            normalization_turn: file.normalization_turn.0,
            overmatch_ratio: file.overmatch_ratio.0,
        };
        for (name, value) in [
            ("ricochet_grip", laws.ricochet_grip),
            ("normalization_turn", laws.normalization_turn),
            ("overmatch_ratio", laws.overmatch_ratio),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!("{name} must be finite and positive (got {value})"));
            }
        }
        Ok(laws)
    }

    /// The incidence (rad, from the normal) past which a round that is not overmatched skids off a
    /// face: `atan(ricochet_grip × nose × potency / thickness)`, `nose` the hit-nose factor and
    /// `potency` and `thickness` in metres of reference steel. A face of no steel-equivalent
    /// deflects nothing. `libm`'s arctangent, so the angle's bits are the same on every platform.
    pub fn critical_incidence(&self, nose: f32, potency: f32, thickness: f32) -> f32 {
        if thickness > 0.0 {
            libm::atanf(self.ricochet_grip * nose * potency / thickness)
        } else {
            std::f32::consts::FRAC_PI_2
        }
    }

    /// The shipped laws, parsed once. Panics on a malformed file: a broken authored contract is a
    /// build defect. [`super::sim_plugin`] reads them while the app is built.
    pub(crate) fn shipped() -> &'static Self {
        static LAWS: std::sync::LazyLock<AngleLaws> = std::sync::LazyLock::new(|| {
            AngleLaws::from_ron(ANGLE_LAWS_RON).unwrap_or_else(|err| {
                panic!("angle laws: embedded assets/ballistics/angle_laws.ron is unusable: {err}")
            })
        });
        &LAWS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped file loads, and its provisional coefficients keep the reference case the design
    /// doc names: the 88 mm APCBC hit nose (`N = 7/24`, a 1-crh ogive) against a plate its potency
    /// just matches skids past 70° and straightens through exactly the f32 0.2 of its incidence. A
    /// coefficient re-derived for another reference moves every shot the gates pin.
    #[test]
    fn the_shipped_laws_keep_the_reference_case() {
        let laws = AngleLaws::shipped();
        let nose = crate::spec::NoseShape::Ogive { crh: 1.0 }.factor();
        let critical = laws.critical_incidence(nose, 0.1, 0.1).to_degrees();
        assert!((critical - 70.0).abs() < 0.01, "critical {critical}°");
        assert_eq!(
            laws.normalization_turn * nose,
            0.2,
            "the reference bend keeps its bits"
        );
        assert_eq!(laws.overmatch_ratio, 3.0);
    }

    /// A coefficient authored without its provenance, with an unknown entry, or that is zero or
    /// negative is refused at parse, naming the field.
    #[test]
    fn a_malformed_law_is_refused() {
        let file = |grip: &str| {
            format!(
                r#"(ricochet_grip: ({grip}, Fixture("fixture")),
                    normalization_turn: (0.7, Fixture("fixture")),
                    overmatch_ratio: (3.0, Fixture("fixture")))"#
            )
        };
        assert_eq!(
            AngleLaws::from_ron(&file("9.0")).unwrap().ricochet_grip,
            9.0
        );
        for value in ["0.0", "-1.0"] {
            let error = AngleLaws::from_ron(&file(value)).expect_err(value);
            assert!(error.contains("ricochet_grip"), "{value}: {error}");
        }
        let bare = AngleLaws::from_ron(
            r#"(ricochet_grip: 9.0, normalization_turn: (0.7, Fixture("fixture")),
                overmatch_ratio: (3.0, Fixture("fixture")))"#,
        )
        .expect_err("a bare value is refused");
        assert!(bare.contains("provenance"), "{bare}");
        assert!(
            AngleLaws::from_ron(
                r#"(ricochet_grip: (9.0, Fixture("fixture")),
                    normalization_turn: (0.7, Fixture("fixture")),
                    overmatch_ratio: (3.0, Fixture("fixture")),
                    ricochet_angle: (1.221, Fixture("fixture")))"#
            )
            .is_err(),
            "an unknown entry"
        );
    }
}
