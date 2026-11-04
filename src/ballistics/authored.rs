//! Build-time admission for the game's damage-model crew-line unit and cosmetic fire cues.
//! `build.rs` generates their constant interfaces; library tests exercise this same admission.

use crate::provenance::{Sourced, from_shipped_str};
use serde::Deserialize;

pub(crate) const CREW_LINE_FILE: &str = "assets/ballistics/crew_line.ron";
pub(crate) const FIRE_CUES_FILE: &str = "assets/vfx/fire_cues.ron";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CrewLine {
    hp_per_reference_mm: Sourced<u32>,
}

pub(crate) fn crew_line_unit(text: &str) -> Result<u64, String> {
    let file: CrewLine = from_shipped_str(CREW_LINE_FILE, text).map_err(|error| {
        format!("{error}; author hp_per_reference_mm as a positive sourced integer")
    })?;
    if file.hp_per_reference_mm.0 == 0 {
        return Err(format!(
            "{CREW_LINE_FILE}: hp_per_reference_mm must be positive; author a nonzero crew-line HP scale"
        ));
    }
    Ok(u64::from(file.hp_per_reference_mm.0))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FireCues {
    stale_fire_ticks: Sourced<u32>,
}

pub(crate) fn fire_cue_age(text: &str) -> Result<u32, String> {
    let file: FireCues = from_shipped_str(FIRE_CUES_FILE, text).map_err(|error| {
        format!("{error}; author stale_fire_ticks as a sourced nonnegative integer")
    })?;
    Ok(file.stale_fire_ticks.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Authored game units and fire-cue tastes must be complete and sourced before any build can
    /// play. A bare scalar, test-only label or zero crew-line scale cannot become a code default.
    #[test]
    fn required_scalar_admission_names_the_file_field_and_repair() {
        assert_eq!(
            crew_line_unit("(hp_per_reference_mm: (7, Unit(\"fixture scale\")))"),
            Ok(7)
        );
        assert_eq!(
            fire_cue_age("(stale_fire_ticks: (0, Taste(\"fixture cues\")))"),
            Ok(0)
        );
        for (field, file, parse) in [
            (
                "hp_per_reference_mm",
                CREW_LINE_FILE,
                crew_line_unit as fn(&str) -> Result<u64, String>,
            ),
            ("stale_fire_ticks", FIRE_CUES_FILE, |text| {
                fire_cue_age(text).map(u64::from)
            }),
        ] {
            for text in [
                "()".to_owned(),
                format!("({field}: 7)"),
                format!("({field}: (7, Fixture(\"inline only\")))"),
                format!("({field}: (7, Taste(\" \")))"),
                format!("({field}: (7, Taste(\"fixture\")), unknown: 1)"),
            ] {
                let error = parse(&text).expect_err(&text);
                assert!(error.contains(file), "{error}");
                assert!(
                    error.contains(field) || error.contains("unknown"),
                    "{error}"
                );
            }
        }
        let error =
            crew_line_unit("(hp_per_reference_mm: (0, Unit(\"fixture scale\")))").unwrap_err();
        assert!(
            error.contains(CREW_LINE_FILE)
                && error.contains("hp_per_reference_mm")
                && error.contains("author"),
            "{error}"
        );
    }
}
