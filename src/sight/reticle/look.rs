//! Required graticule look data, parsed and checked before the client's sight widgets spawn.

use bevy::prelude::{Color, Resource};
use serde::Deserialize;

use crate::provenance::{Sourced, from_shipped_str};

const FILE: &str = "assets/ui/graticule.ron";
const DATA: &str = include_str!("../../../assets/ui/graticule.ron");

#[derive(Resource, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GraticuleLook {
    pub(super) hairline_physical_px: Sourced<f32>,
    pub(super) line_width_ui_px: Sourced<f32>,
    pub(super) major_tick_width_ui_px: Sourced<f32>,
    pub(super) minor_tick_width_ui_px: Sourced<f32>,
    pub(super) label_gap_ui_px: Sourced<f32>,
    pub(super) label_top_ui_px: Sourced<f32>,
    pub(super) label_font_ui_px: Sourced<f32>,
    pub(super) colour_srgba: Sourced<[f32; 4]>,
}

impl GraticuleLook {
    pub(super) fn tick_width(&self, major: bool) -> f32 {
        if major {
            self.major_tick_width_ui_px.0
        } else {
            self.minor_tick_width_ui_px.0
        }
    }

    pub(super) fn colour(&self) -> Color {
        let [r, g, b, a] = self.colour_srgba.0;
        Color::srgba(r, g, b, a)
    }

    pub(super) fn load() -> Self {
        Self::parse(DATA).unwrap_or_else(|error| panic!("{error}"))
    }

    fn parse(text: &str) -> Result<Self, String> {
        let data: Self = from_shipped_str(FILE, text)?;
        for (field, value) in [
            ("hairline_physical_px", data.hairline_physical_px.0),
            ("line_width_ui_px", data.line_width_ui_px.0),
            ("major_tick_width_ui_px", data.major_tick_width_ui_px.0),
            ("minor_tick_width_ui_px", data.minor_tick_width_ui_px.0),
            ("label_font_ui_px", data.label_font_ui_px.0),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!(
                    "{FILE}: {field} must be finite and positive; author a positive size"
                ));
            }
        }
        if !data.label_gap_ui_px.0.is_finite() || data.label_gap_ui_px.0 < 0.0 {
            return Err(format!(
                "{FILE}: label_gap_ui_px must be finite and nonnegative; author a clearance"
            ));
        }
        if !data.label_top_ui_px.0.is_finite() {
            return Err(format!(
                "{FILE}: label_top_ui_px must be finite; author a signed offset"
            ));
        }
        if data
            .colour_srgba
            .0
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Err(format!(
                "{FILE}: colour_srgba must have finite channels in [0, 1]; author valid sRGBA"
            ));
        }
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The client must refuse missing or malformed look data before spawning the sight;
    /// defaults, unchecked extents and test-only provenance would hide an authoring error.
    #[test]
    fn shipped_graticule_data_is_required_and_checked_before_play() {
        // Fixture: a permitted width change must not invalidate unrelated admission probes.
        let retuned = replace_field(DATA, "line_width_ui_px", "97.0");
        for text in [DATA, retuned.as_str()] {
            check_admission(text);
        }
    }

    fn replace_field(text: &str, field: &str, value: &str) -> String {
        text.lines()
            .map(|line| {
                if line.trim_start().starts_with(&format!("{field}:")) {
                    format!("    {field}: ({value}, Taste(\"graticule: admission fixture\")),")
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn check_admission(text: &str) {
        GraticuleLook::parse(text).unwrap();
        for field in [
            "hairline_physical_px",
            "line_width_ui_px",
            "major_tick_width_ui_px",
            "minor_tick_width_ui_px",
            "label_gap_ui_px",
            "label_top_ui_px",
            "label_font_ui_px",
            "colour_srgba",
        ] {
            let missing = text
                .lines()
                .filter(|line| !line.trim_start().starts_with(&format!("{field}:")))
                .collect::<Vec<_>>()
                .join("\n");
            let error = GraticuleLook::parse(&missing).unwrap_err();
            assert!(error.contains(FILE) && error.contains(field), "{error}");
        }
        for (field, value) in [
            ("hairline_physical_px", "0.0"),
            ("line_width_ui_px", "-1.0"),
            ("major_tick_width_ui_px", "inf"),
            ("minor_tick_width_ui_px", "NaN"),
            ("label_gap_ui_px", "-1.0"),
            ("label_top_ui_px", "NaN"),
            ("label_font_ui_px", "0.0"),
            ("colour_srgba", "(0.0, 0.0, 0.0, 2.0)"),
        ] {
            let error = GraticuleLook::parse(&replace_field(text, field, value)).unwrap_err();
            assert!(error.contains(FILE) && error.contains(field), "{error}");
        }
        let error = GraticuleLook::parse(&text.replace("Taste(", "Fixture(")).unwrap_err();
        assert!(
            error.contains(FILE) && error.contains("Fixture provenance is test-only"),
            "{error}"
        );
    }
}
