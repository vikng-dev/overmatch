//! The energy-to-HP rate of the game's damage model, read from `assets/ballistics/energy_dose.ron`:
//! the joules of energy a round loses in a hard pool per HP point that pool loses
//! ([`joules_per_hp`]). One rate for every hard pool, a tank's modules and a tree's trunk alike;
//! [`super::energy_units`] converts a line dose with it. Crew (witnesses) and fields convert at
//! their own rates. Design doc §6.

use serde::Deserialize;

use crate::provenance::Sourced;

/// The shipped rate, embedded so it is read while the app is built (ADR-0014: built from data,
/// synchronously).
const ENERGY_DOSE_RON: &str = include_str!("../../assets/ballistics/energy_dose.ron");

/// The file as authored: every value with its provenance ([`crate::provenance`]).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RateFile {
    joules_per_hp: Sourced<f64>,
}

/// Parse a rate file, refusing a rate that is not finite, positive and normal: a line dose divides
/// by it.
fn from_ron(text: &str) -> Result<f64, String> {
    let file: RateFile = ron::de::from_str(text).map_err(|err| err.to_string())?;
    let rate = file.joules_per_hp.0;
    if !rate.is_normal() || rate < 0.0 {
        return Err(format!(
            "joules_per_hp must be finite, positive and normal (got {rate})"
        ));
    }
    Ok(rate)
}

/// Joules of energy lost per HP point, parsed once. Panics on a malformed file: a broken authored
/// contract is a build defect. [`super::sim_plugin`] reads it while the app is built.
pub(crate) fn joules_per_hp() -> f64 {
    static RATE: std::sync::LazyLock<f64> = std::sync::LazyLock::new(|| {
        from_ron(ENERGY_DOSE_RON).unwrap_or_else(|err| {
            panic!("energy dose: embedded energy_dose.ron is unusable: {err}")
        })
    });
    *RATE
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rate authored without its provenance, with an unknown entry, or that is zero, negative or
    /// subnormal is refused at parse, naming the field; a sourced positive rate loads as written.
    /// Every line dose divides by it, so a zero would poison every hard pool's total.
    #[test]
    fn a_malformed_rate_is_refused() {
        assert_eq!(
            from_ron(r#"(joules_per_hp: (2500.0, Tuned("fixture")))"#),
            Ok(2500.0)
        );
        let bare = from_ron("(joules_per_hp: 2500.0)").expect_err("a bare value is refused");
        assert!(bare.contains("provenance"), "{bare}");
        assert!(
            from_ron(r#"(joules_per_hp: (2500.0, Tuned("fixture")), hp: (1, Tuned("fixture")))"#)
                .is_err(),
            "an unknown entry"
        );
        for value in ["0.0", "-2500.0", "1e-310"] {
            let text = format!(r#"(joules_per_hp: ({value}, Tuned("fixture")))"#);
            let error = from_ron(&text).expect_err(value);
            assert!(error.contains("joules_per_hp"), "{value}: {error}");
        }
    }
}
