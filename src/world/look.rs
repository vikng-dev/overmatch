//! Required world lighting tastes. Renderer sampling resolution stays with its algorithm.
use crate::provenance::{Sourced, from_shipped_str};
use serde::Deserialize;
use std::sync::LazyLock;
const FILE: &str = "assets/world/lighting.ron";
const DATA: &str = include_str!("../../assets/world/lighting.ron");
static LIGHTING: LazyLock<Lighting> =
    LazyLock::new(|| Lighting::parse(DATA).unwrap_or_else(|error| panic!("{error}")));
pub(super) fn get() -> &'static Lighting {
    &LIGHTING
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Lighting {
    /// Sun elevation above the horizon, degrees — LOW, and deliberately so. Relief on a normal-mapped
    /// surface is carried by the cosine between the light and the perturbed normal, so a high sun
    /// (the old `(4, 8, 4)` placement was ~55°) flattens ground detail into near-uniform brightness
    /// no matter how good the map is: every ripple faces the light about equally. At a grazing angle
    /// the same bump swings from lit to self-shadowed across a few centimetres, which is what makes
    /// the terrain read as SURFACE instead of a photograph. Also the honest tactical light for a tank
    /// sim — long shadows are cover and a range cue.
    pub(super) sun_elevation_deg: Sourced<f32>,
    /// Sun azimuth, degrees from +X toward +Z.
    pub(super) sun_azimuth_deg: Sourced<f32>,
    /// Key-light intensity, lux. DERIVED: level-ground irradiance is illuminance times the sine
    /// of elevation, so these two tastes jointly determine how bright a flat surface reads.
    pub(super) sun_illuminance_lux: Sourced<f32>,
    /// Sun colour — a restrained golden-hour warm. Subtle on purpose: this is a legibility change, and
    /// a strongly orange key would tint the albedo readings the gunner uses to tell surfaces apart.
    pub(super) sun_srgb: Sourced<[f32; 3]>,
    /// Fill luminance, cd/m². MEASURED: this analytic sky's cosine-weighted upper hemisphere
    /// averages 0.41 of its scale. DERIVED example: a 600 cd/m² fill adds 246 cd/m² against the
    /// roughly 2300 cd/m² diffuse radiance of a 25000 lux key at 17 degrees, about 11 percent.
    /// The fill keeps shadowed faces legible without erasing the low sun's relief. It also gives
    /// metals something to reflect: a fully metallic surface has no diffuse response.
    pub(super) sky_intensity_cd_m2: Sourced<f32>,
    pub(super) sun_debug_distance_m: Sourced<f32>,
    pub(super) zenith_linear: Sourced<[f32; 3]>,
    pub(super) haze_linear: Sourced<[f32; 3]>,
    pub(super) glow_linear: Sourced<[f32; 3]>,
    pub(super) ground_linear: Sourced<[f32; 3]>,
    pub(super) glow_exponent: Sourced<i32>,
    pub(super) glow_strength: Sourced<f32>,
}
impl Lighting {
    fn parse(text: &str) -> Result<Self, String> {
        let data: Self = from_shipped_str(FILE, text)?;
        let refuse = |field: &str, fix: &str| format!("{FILE}: {field}: {fix}");
        if !data.sun_elevation_deg.0.is_finite()
            || !(0.0..90.0).contains(&data.sun_elevation_deg.0)
            || data.sun_elevation_deg.0 == 0.0
        {
            return Err(refuse(
                "sun_elevation_deg",
                "author a finite elevation strictly between the horizon and the zenith",
            ));
        }
        if !data.sun_azimuth_deg.0.is_finite() {
            return Err(refuse(
                "sun_azimuth_deg",
                "author a finite bearing in degrees",
            ));
        }
        for (field, value) in [
            ("sun_illuminance_lux", data.sun_illuminance_lux.0),
            ("sun_debug_distance_m", data.sun_debug_distance_m.0),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(refuse(field, "author a finite positive magnitude"));
            }
        }
        if !data.sky_intensity_cd_m2.0.is_finite() || data.sky_intensity_cd_m2.0 < 0.0 {
            return Err(refuse(
                "sky_intensity_cd_m2",
                "author a finite nonnegative fill intensity",
            ));
        }
        if data.glow_exponent.0 <= 0 {
            return Err(refuse("glow_exponent", "author a positive lobe exponent"));
        }
        if !data.glow_strength.0.is_finite() || !(0.0..=1.0).contains(&data.glow_strength.0) {
            return Err(refuse(
                "glow_strength",
                "author a finite blend strength in [0, 1]",
            ));
        }
        for (field, channels) in [
            ("sun_srgb", data.sun_srgb.0),
            ("zenith_linear", data.zenith_linear.0),
            ("haze_linear", data.haze_linear.0),
            ("glow_linear", data.glow_linear.0),
            ("ground_linear", data.ground_linear.0),
        ] {
            if channels
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            {
                return Err(refuse(field, "author finite channels in [0, 1]"));
            }
        }
        Ok(data)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    /// The world refuses malformed authored light before startup, including test-only provenance.
    #[test]
    fn lighting_is_required_and_checked_before_play() {
        Lighting::parse(DATA).unwrap();
        let error = Lighting::parse("()").unwrap_err();
        assert!(
            error.contains(FILE) && error.contains("sun_elevation_deg"),
            "{error}"
        );
        for (field, value) in [
            ("sun_elevation_deg", "90.0"),
            ("sun_azimuth_deg", "NaN"),
            ("sun_illuminance_lux", "0.0"),
            ("sun_debug_distance_m", "0.0"),
            ("sky_intensity_cd_m2", "-1.0"),
            ("glow_exponent", "0"),
            ("glow_strength", "2.0"),
            ("sun_srgb", "(2.0, 0.0, 0.0)"),
        ] {
            let text = DATA
                .lines()
                .map(|line| {
                    if line.trim_start().starts_with(&format!("{field}:")) {
                        format!("    {field}: ({value}, Taste(\"lighting: admission fixture\")),")
                    } else {
                        line.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let error = Lighting::parse(&text).unwrap_err();
            assert!(error.contains(FILE) && error.contains(field), "{error}");
        }
        let error = Lighting::parse(&DATA.replace("Taste(", "Fixture(")).unwrap_err();
        assert!(
            error.contains(FILE) && error.contains("Fixture provenance is test-only"),
            "{error}"
        );
    }
}
