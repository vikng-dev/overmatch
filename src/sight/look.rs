//! Required sight and aim-HUD appearance, shared by cursor reach and the drawn optic.

use std::sync::LazyLock;

use bevy::prelude::Color;
use serde::Deserialize;

use crate::provenance::{Sourced, from_shipped_str};

const FILE: &str = "assets/ui/sight.ron";
const DATA: &str = include_str!("../../assets/ui/sight.ron");
static LOOK: LazyLock<SightLook> =
    LazyLock::new(|| SightLook::parse(DATA).unwrap_or_else(|error| panic!("{error}")));

pub(crate) fn get() -> &'static SightLook {
    &LOOK
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SightLook {
    /// View-only bearing blend from gun sight line (zero) to committed intent (one), on both axes.
    /// Zero lets intent lead and return as the gun catches up; one holds intent at glass centre
    /// while the bore lags. The blend is instantaneous and stateless, with no damping or overshoot;
    /// aiming and servo commands are independent of it.
    pub(crate) gunner_blend: Sourced<f32>,
    pub(crate) mouse_sensitivity_per_fov: Sourced<f32>,
    /// Diameter / larger viewport axis, also cursor reach / projected half-extent.
    pub(crate) optic_span_fraction: Sourced<f32>,
    /// Broad surround blur, as a fraction of radius; distinct edges keep smoothstep defined.
    pub(crate) optic_feather_fraction: Sourced<f32>,
    pub(crate) optic_surround_linear_rgba: Sourced<[f32; 4]>,
    view_surround_srgba: Sourced<[f32; 4]>,
    pub(crate) toast_seconds: Sourced<f32>,
    pub(crate) toast_top_percent: Sourced<f32>,
    pub(crate) range_inset_ui_px: Sourced<f32>,
    pub(crate) intent: DotLook,
    pub(crate) centre: DotLook,
    pub(crate) bore: DotLook,
    pub(crate) aim: DotLook,
    pub(crate) view_prompt: LabelLook,
    pub(crate) toast: LabelLook,
    pub(crate) range: LabelLook,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DotLook {
    pub(crate) diameter_ui_px: Sourced<f32>,
    colour_srgba: Sourced<[f32; 4]>,
}

impl DotLook {
    pub(crate) fn colour(&self) -> Color {
        colour(self.colour_srgba.0)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LabelLook {
    pub(crate) font_ui_px: Sourced<f32>,
    colour_srgba: Sourced<[f32; 4]>,
}

impl LabelLook {
    pub(crate) fn colour(&self) -> Color {
        colour(self.colour_srgba.0)
    }
}

fn colour([r, g, b, a]: [f32; 4]) -> Color {
    Color::srgba(r, g, b, a)
}

impl SightLook {
    pub(crate) fn view_surround(&self) -> Color {
        colour(self.view_surround_srgba.0)
    }

    fn parse(text: &str) -> Result<Self, String> {
        let data: Self = from_shipped_str(FILE, text)?;
        data.validate()?;
        Ok(data)
    }

    fn validate(&self) -> Result<(), String> {
        if !self.gunner_blend.0.is_finite() || !(0.0..=1.0).contains(&self.gunner_blend.0) {
            return Err(refuse("gunner_blend", "author a finite fraction in [0, 1]"));
        }
        let span = self.optic_span_fraction.0;
        if !span.is_finite() || span <= 0.0 || span > 1.0 {
            return Err(refuse(
                "optic_span_fraction",
                "author a finite fraction in (0, 1]",
            ));
        }
        let radius = span / 2.0;
        let feather = radius * self.optic_feather_fraction.0;
        if !feather.is_finite()
            || feather <= 0.0
            || feather >= radius
            || radius - feather >= radius
            || radius + feather <= radius
        {
            return Err(refuse(
                "optic_feather_fraction",
                "author a fraction whose radius +/- feather are distinct and leave the centre clear",
            ));
        }
        for (field, value) in [
            ("toast_seconds", self.toast_seconds.0),
            (
                "mouse_sensitivity_per_fov",
                self.mouse_sensitivity_per_fov.0,
            ),
            ("intent.diameter_ui_px", self.intent.diameter_ui_px.0),
            ("centre.diameter_ui_px", self.centre.diameter_ui_px.0),
            ("bore.diameter_ui_px", self.bore.diameter_ui_px.0),
            ("aim.diameter_ui_px", self.aim.diameter_ui_px.0),
            ("view_prompt.font_ui_px", self.view_prompt.font_ui_px.0),
            ("toast.font_ui_px", self.toast.font_ui_px.0),
            ("range.font_ui_px", self.range.font_ui_px.0),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(refuse(field, "author a finite positive value"));
            }
        }
        if !self.toast_top_percent.0.is_finite()
            || !(0.0..=100.0).contains(&self.toast_top_percent.0)
        {
            return Err(refuse(
                "toast_top_percent",
                "author a finite viewport percentage in [0, 100]",
            ));
        }
        if !self.range_inset_ui_px.0.is_finite() || self.range_inset_ui_px.0 < 0.0 {
            return Err(refuse(
                "range_inset_ui_px",
                "author a finite nonnegative UI inset",
            ));
        }
        for (field, channels) in [
            ("view_surround_srgba", self.view_surround_srgba.0),
            (
                "optic_surround_linear_rgba",
                self.optic_surround_linear_rgba.0,
            ),
            ("intent.colour_srgba", self.intent.colour_srgba.0),
            ("centre.colour_srgba", self.centre.colour_srgba.0),
            ("bore.colour_srgba", self.bore.colour_srgba.0),
            ("aim.colour_srgba", self.aim.colour_srgba.0),
            ("view_prompt.colour_srgba", self.view_prompt.colour_srgba.0),
            ("toast.colour_srgba", self.toast.colour_srgba.0),
            ("range.colour_srgba", self.range.colour_srgba.0),
        ] {
            if channels
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            {
                return Err(refuse(field, "author finite RGBA channels in [0, 1]"));
            }
        }
        Ok(())
    }
}

fn refuse(field: &str, fix: &str) -> String {
    format!("{FILE}: {field}: {fix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Presentation admission refuses missing, malformed and test-only authored values before play.
    #[test]
    fn sight_appearance_is_required_and_checked() {
        SightLook::parse(DATA).unwrap();
        for field in [
            "gunner_blend",
            "mouse_sensitivity_per_fov",
            "optic_span_fraction",
            "optic_feather_fraction",
            "optic_surround_linear_rgba",
            "view_surround_srgba",
            "toast_seconds",
            "toast_top_percent",
            "range_inset_ui_px",
            "intent",
            "centre",
            "bore",
            "aim",
            "view_prompt",
            "toast",
            "range",
        ] {
            let missing = DATA
                .lines()
                .filter(|line| !line.trim_start().starts_with(&format!("{field}:")))
                .collect::<Vec<_>>()
                .join("\n");
            let error = SightLook::parse(&missing).unwrap_err();
            assert!(error.contains(FILE) && error.contains(field), "{error}");
        }
        for (field, value) in [
            ("gunner_blend", "2.0"),
            ("mouse_sensitivity_per_fov", "0.0"),
            ("optic_span_fraction", "0.0"),
            ("optic_feather_fraction", "0.0"),
            ("toast_seconds", "NaN"),
            ("toast_top_percent", "101.0"),
            ("range_inset_ui_px", "-1.0"),
        ] {
            let malformed = DATA
                .lines()
                .map(|line| {
                    if line.trim_start().starts_with(&format!("{field}:")) {
                        format!("    {field}: ({value}, Taste(\"sight: admission probe\")),")
                    } else {
                        line.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let error = SightLook::parse(&malformed).unwrap_err();
            assert!(error.contains(FILE) && error.contains(field), "{error}");
        }
        for field in [
            "intent",
            "centre",
            "bore",
            "aim",
            "view_prompt",
            "toast",
            "range",
            "optic_surround_linear_rgba",
            "view_surround_srgba",
        ] {
            let mut data = SightLook::parse(DATA).unwrap();
            match field {
                "intent" => data.intent.diameter_ui_px.0 = 0.0,
                "centre" => data.centre.diameter_ui_px.0 = f32::NAN,
                "bore" => data.bore.colour_srgba.0[0] = 2.0,
                "aim" => data.aim.colour_srgba.0[0] = f32::NAN,
                "view_prompt" => data.view_prompt.font_ui_px.0 = 0.0,
                "toast" => data.toast.colour_srgba.0[0] = -1.0,
                "range" => data.range.font_ui_px.0 = f32::INFINITY,
                "view_surround_srgba" => data.view_surround_srgba.0[0] = -1.0,
                "optic_surround_linear_rgba" => data.optic_surround_linear_rgba.0[0] = 2.0,
                _ => unreachable!(),
            }
            let error = data.validate().unwrap_err();
            assert!(error.contains(FILE) && error.contains(field), "{error}");
        }
        let error = SightLook::parse(&DATA.replace("Taste(", "Fixture(")).unwrap_err();
        assert!(
            error.contains(FILE) && error.contains("Fixture provenance is test-only"),
            "{error}"
        );
    }
}
