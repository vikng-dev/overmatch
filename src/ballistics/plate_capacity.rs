//! The game's damage model: a thin plate's work relative to bulk cavity resistance.
//! The empirical approximation and the separate skid-pressure limitation are design doc §3b.

use serde::Deserialize;

/// Global StandIn parameters; no round or vehicle owns a fitted capacity number.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlateCapacity {
    shear_slope: f64,
    opening_share: f64,
    return_power: f64,
}

impl PlateCapacity {
    // The area is proportional to calibre squared. A return power below two preserves
    // increasing resistance with calibre at fixed mass; a positive power preserves the bulk join.
    pub(crate) fn validate(&self, path: &str) -> Result<(), String> {
        for (field, valid) in [
            (
                "shear_slope",
                self.shear_slope.is_finite() && self.shear_slope >= 1.0,
            ),
            (
                "opening_share",
                self.opening_share.is_finite()
                    && self.opening_share > 0.0
                    && self.opening_share < 1.0,
            ),
            (
                "return_power",
                self.return_power.is_finite() && self.return_power > 0.0 && self.return_power < 2.0,
            ),
        ] {
            if !valid {
                return Err(format!(
                    "{path}: plate_capacity.{field}: author shear_slope >= 1, opening_share in (0, 1), return_power in (0, 2), all finite"
                ));
            }
        }
        Ok(())
    }

    /// The game's damage model: multiply resisted chord, never travelled distance, by this
    /// share. `x` is covered normal geometric thickness / calibre. Work rises as x² near zero;
    /// every accepted parameter set joins bulk resistance continuously and is neutral at x >= 1.
    pub(crate) fn charge(&self, ratio: f64) -> f64 {
        assert!(
            ratio.is_finite() && ratio >= 0.0,
            "the game's damage model collects finite nonnegative plate thickness"
        );
        if ratio >= 1.0 {
            return 1.0;
        }
        (self.shear_slope * ratio).min(self.opening_share.max(libm::pow(ratio, self.return_power)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game's damage model: plate authors rely on monotone work, a continuous bulk join
    /// and an unchanged bulk law. Mutants: a reversed slope, a jump or applying the thin arm above
    /// one calibre. Fixture parameters vary independently of the shipped empirical fit.
    #[test]
    fn capacity_work_rises_continuously_and_leaves_bulk_unchanged() {
        for shear_slope in [1.0, 1.5, 4.0] {
            for opening_share in [0.1, 0.5, 0.9] {
                for return_power in [0.5, 1.0, 1.5, 1.9] {
                    let capacity = PlateCapacity {
                        shear_slope,
                        opening_share,
                        return_power,
                    };
                    capacity.validate("fixture").unwrap();
                    let mut previous = 0.0;
                    let mut width_normalized = f64::INFINITY;
                    for i in 1..=1000 {
                        let x = f64::from(i) / 1000.0;
                        let work = x * capacity.charge(x);
                        assert!(work > previous && work <= x);
                        previous = work;
                        let normalized = capacity.charge(x) / (x * x);
                        assert!(
                            normalized < width_normalized,
                            "area grows as calibre squared: widening at fixed mass cannot lower the limit"
                        );
                        width_normalized = normalized;
                    }
                    for offset in [1.0e-3, 1.0e-6, 1.0e-9] {
                        assert!(
                            (capacity.charge(1.0 - offset) - 1.0).abs()
                                < (shear_slope + return_power) * offset
                        );
                    }
                    for x in [1.0, 1.001, 2.0, 100.0] {
                        assert_eq!(capacity.charge(x), 1.0);
                    }
                }
            }
        }
    }

    /// The game's damage model: the earliest data gate refuses every parameter that could
    /// reverse work, miss the bulk join or produce nonfinite state. Mutant: skip validation.
    #[test]
    fn capacity_parameters_are_refused_with_their_field_and_fix() {
        for (field, values) in [
            ("shear_slope", vec![0.0, 0.9, f64::NAN, f64::INFINITY]),
            ("opening_share", vec![0.0, 1.0, f64::NAN, f64::INFINITY]),
            ("return_power", vec![0.0, 2.0, f64::NAN, f64::INFINITY]),
        ] {
            for value in values {
                let mut capacity = PlateCapacity {
                    shear_slope: 1.5,
                    opening_share: 0.5,
                    return_power: 1.5,
                };
                match field {
                    "shear_slope" => capacity.shear_slope = value,
                    "opening_share" => capacity.opening_share = value,
                    "return_power" => capacity.return_power = value,
                    _ => unreachable!(),
                }
                let error = capacity.validate("fixture.ron").unwrap_err();
                assert!(
                    error.contains("fixture.ron")
                        && error.contains(field)
                        && error.contains("author")
                );
            }
        }
    }
}
