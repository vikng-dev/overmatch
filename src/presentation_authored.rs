//! Required camera and UI authoring data, admitted by `build.rs` before any build can play.
//! Generated constants preserve the callers' interfaces; this module owns admission only.
use crate::provenance::{Sourced, shipped};
use serde::Deserialize;
use std::fmt::Write;

pub(crate) const CAMERA_FILE: &str = "assets/ui/camera.ron";
pub(crate) const STYLE_FILE: &str = "assets/ui/style.ron";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Camera {
    pivot_lift_m: Sourced<f32>,
    orbit_far_m: Sourced<f32>,
    orbit_near_m: Sourced<f32>,
    lod_budget_px: Sourced<f32>,
    terrain_lod_budget_px: Sourced<f32>,
    terrain_lod_deviation_rungs_m: Sourced<Vec<f32>>,
    killcam_record_wait_s: Sourced<f32>,
    commander_fov_before_bind_rad: Sourced<f32>,
    orbit_yaw_rad_per_pixel: Sourced<f32>,
    orbit_pitch_rad_per_pixel: Sourced<f32>,
    zoom_per_scroll_unit: Sourced<f32>,
    zoom_response_per_s: Sourced<f32>,
    free_fly_rad_per_pixel: Sourced<f32>,
    free_fly_m_per_s: Sourced<f32>,
    initial_eye_m: Sourced<[f32; 3]>,
    initial_look_m: Sourced<[f32; 3]>,
}

fn refuse(file: &str, field: &str, fix: &str) -> String {
    format!("{file}: {field}: {fix}")
}

fn parse_data<'de, T: Deserialize<'de>>(file: &str, text: &'de str) -> Result<T, String> {
    shipped(file, || {
        let mut reader =
            ron::de::Deserializer::from_str(text).map_err(|error| error.to_string())?;
        let data = serde_path_to_error::deserialize(&mut reader).map_err(|error| {
            let field = error.path().to_string();
            format!("{field}: {}", reader.span_error(error.into_inner()))
        })?;
        reader
            .end()
            .map_err(|error| reader.span_error(error).to_string())?;
        Ok::<T, String>(data)
    })
    .map_err(|error| format!("{error}; author every required field with its provenance"))
}

/// Preserve the admitted f32 exactly; an authored taste may coincide with a mathematical constant.
fn literal(value: f32) -> String {
    format!("f32::from_bits({:#010x})", value.to_bits())
}

pub(crate) fn camera_constants(text: &str) -> Result<String, String> {
    let data: Camera = parse_data(CAMERA_FILE, text)?;
    let mut out = String::new();
    for (field, constant, value) in [
        ("pivot_lift_m", "PIVOT_LIFT", data.pivot_lift_m.0),
        ("orbit_far_m", "ORBIT_FAR", data.orbit_far_m.0),
        ("orbit_near_m", "ORBIT_NEAR", data.orbit_near_m.0),
        (
            "lod_budget_px",
            "DEFAULT_LOD_BUDGET_PX",
            data.lod_budget_px.0,
        ),
        (
            "terrain_lod_budget_px",
            "TERRAIN_LOD_BUDGET_PX",
            data.terrain_lod_budget_px.0,
        ),
        (
            "killcam_record_wait_s",
            "KILLCAM_RECORD_WAIT_S",
            data.killcam_record_wait_s.0,
        ),
        (
            "commander_fov_before_bind_rad",
            "COMMANDER_FOV_BEFORE_BIND",
            data.commander_fov_before_bind_rad.0,
        ),
        (
            "orbit_yaw_rad_per_pixel",
            "ORBIT_YAW_SENSITIVITY",
            data.orbit_yaw_rad_per_pixel.0,
        ),
        (
            "orbit_pitch_rad_per_pixel",
            "ORBIT_PITCH_SENSITIVITY",
            data.orbit_pitch_rad_per_pixel.0,
        ),
        (
            "zoom_per_scroll_unit",
            "ORBIT_ZOOM_SPEED",
            data.zoom_per_scroll_unit.0,
        ),
        (
            "zoom_response_per_s",
            "ORBIT_ZOOM_GLIDE",
            data.zoom_response_per_s.0,
        ),
        (
            "free_fly_rad_per_pixel",
            "FREE_FLY_SENSITIVITY",
            data.free_fly_rad_per_pixel.0,
        ),
        (
            "free_fly_m_per_s",
            "FREE_FLY_SPEED",
            data.free_fly_m_per_s.0,
        ),
    ] {
        if !value.is_finite() || value <= 0.0 {
            return Err(refuse(
                CAMERA_FILE,
                field,
                "author a positive finite value in the named unit",
            ));
        }
        if matches!(constant, "FREE_FLY_SENSITIVITY" | "FREE_FLY_SPEED") {
            writeln!(out, r#"#[cfg(feature = "dev_ui")]"#).unwrap();
        }
        writeln!(out, "pub const {constant}: f32 = {};", literal(value)).unwrap();
    }
    if data.orbit_near_m.0 > data.orbit_far_m.0 {
        return Err(refuse(
            CAMERA_FILE,
            "orbit_near_m / orbit_far_m",
            "author near <= far for the zoom interval",
        ));
    }
    let rungs = &data.terrain_lod_deviation_rungs_m.0;
    if rungs.len() < 2
        || rungs[0] != 0.0
        || rungs.iter().any(|rung| !rung.is_finite())
        || rungs.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(refuse(
            CAMERA_FILE,
            "terrain_lod_deviation_rungs_m",
            "author exact rung zero followed by finite strictly increasing deviations in metres",
        ));
    }
    writeln!(
        out,
        "pub const TERRAIN_LOD_LADDER: [f32; {}] = [{}];",
        rungs.len(),
        rungs
            .iter()
            .map(|&rung| literal(rung))
            .collect::<Vec<_>>()
            .join(", ")
    )
    .unwrap();
    if data.commander_fov_before_bind_rad.0 >= std::f32::consts::PI {
        return Err(refuse(
            CAMERA_FILE,
            "commander_fov_before_bind_rad",
            "author a field strictly between zero and pi radians",
        ));
    }
    let span: [f32; 3] =
        std::array::from_fn(|i| data.initial_look_m.0[i] - data.initial_eye_m.0[i]);
    let squared = span.map(|v| v * v).iter().sum::<f32>();
    // A vertical look has no horizontal heading to define the initial camera's right axis.
    if !data
        .initial_eye_m
        .0
        .into_iter()
        .chain(data.initial_look_m.0)
        .all(f32::is_finite)
        || !squared.is_finite()
        || squared <= 0.0
        || (span[0] == 0.0 && span[2] == 0.0)
    {
        return Err(refuse(
            CAMERA_FILE,
            "initial_eye_m / initial_look_m",
            "author finite positions with a finite nonzero span and a horizontal heading",
        ));
    }
    for (constant, values) in [
        ("INITIAL_EYE", data.initial_eye_m.0),
        ("INITIAL_LOOK", data.initial_look_m.0),
    ] {
        writeln!(
            out,
            "const {constant}: [f32; 3] = [{}];",
            values.map(literal).join(", ")
        )
        .unwrap();
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Style {
    overlay_font_px: Sourced<f32>,
    overlay_text_srgb: Sourced<[f32; 3]>,
    text_srgb: Sourced<[f32; 3]>,
    text_dim_srgb: Sourced<[f32; 3]>,
    net_warn_srgb: Sourced<[f32; 3]>,
    net_alarm_srgb: Sourced<[f32; 3]>,
    net_debug_refresh_s: Sourced<f64>,
    net_warn_excess_ticks: Sourced<u32>,
    net_alarm_excess_ticks: Sourced<u32>,
    panel_srgba: Sourced<[f32; 4]>,
    modal_srgba: Sourced<[f32; 4]>,
    modal_card_srgb: Sourced<[f32; 3]>,
    death_tint_srgba: Sourced<[f32; 4]>,
}

pub(crate) fn style_constants(text: &str) -> Result<String, String> {
    let data: Style = parse_data(STYLE_FILE, text)?;
    if !data.overlay_font_px.0.is_finite() || data.overlay_font_px.0 <= 0.0 {
        return Err(refuse(
            STYLE_FILE,
            "overlay_font_px",
            "author a positive finite font size in pixels",
        ));
    }
    let mut out = format!(
        "const OVERLAY_FONT_PX: f32 = {:?};\n",
        data.overlay_font_px.0
    );
    let refresh_ns = std::time::Duration::try_from_secs_f64(data.net_debug_refresh_s.0)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_nanos()).ok())
        .filter(|&nanos| nanos > 0)
        .ok_or_else(|| {
            refuse(
                STYLE_FILE,
                "net_debug_refresh_s",
                "author a positive finite duration representable in u64 nanoseconds",
            )
        })?;
    let (warn, alarm) = (data.net_warn_excess_ticks.0, data.net_alarm_excess_ticks.0);
    if warn == 0 || warn > alarm {
        return Err(refuse(
            STYLE_FILE,
            "net_warn_excess_ticks / net_alarm_excess_ticks",
            "author positive warning ticks <= alarm ticks",
        ));
    }
    writeln!(out, "pub(crate) const NET_DEBUG_REFRESH: std::time::Duration = std::time::Duration::from_nanos({refresh_ns});\npub(crate) const NET_WARN_EXCESS_TICKS: u32 = {warn};\npub(crate) const NET_ALARM_EXCESS_TICKS: u32 = {alarm};").unwrap();
    for (field, constant, channels) in [
        (
            "overlay_text_srgb",
            "OVERLAY_TEXT",
            data.overlay_text_srgb.0.as_slice(),
        ),
        ("text_srgb", "TEXT", data.text_srgb.0.as_slice()),
        ("text_dim_srgb", "TEXT_DIM", data.text_dim_srgb.0.as_slice()),
        ("net_warn_srgb", "NET_WARN", data.net_warn_srgb.0.as_slice()),
        (
            "net_alarm_srgb",
            "NET_ALARM",
            data.net_alarm_srgb.0.as_slice(),
        ),
        ("panel_srgba", "PANEL_BG", data.panel_srgba.0.as_slice()),
        ("modal_srgba", "MODAL_BG", data.modal_srgba.0.as_slice()),
        (
            "modal_card_srgb",
            "MODAL_CARD_BG",
            data.modal_card_srgb.0.as_slice(),
        ),
        (
            "death_tint_srgba",
            "DEATH_TINT",
            data.death_tint_srgba.0.as_slice(),
        ),
    ] {
        if !channels
            .iter()
            .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
        {
            return Err(refuse(
                STYLE_FILE,
                field,
                "author finite sRGB/alpha channels in 0..=1",
            ));
        }
        let constructor = if channels.len() == 3 { "srgb" } else { "srgba" };
        let values = channels
            .iter()
            .map(|&value| literal(value))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            out,
            "pub(crate) const {constant}: Color = Color::{constructor}({values});"
        )
        .unwrap();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Presentation consumers require complete sourced data and representable camera/colour domains.
    /// Mutants supplying missing tastes or accepting poisoned fields fail before a client can play.
    #[test]
    fn camera_and_ui_admission_refuse_missing_unsourced_and_invalid_fields() {
        let camera = include_str!("../assets/ui/camera.ron");
        let style = include_str!("../assets/ui/style.ron");
        for (file, text, parse) in [
            (
                CAMERA_FILE,
                camera,
                camera_constants as fn(&str) -> Result<String, String>,
            ),
            (STYLE_FILE, style, style_constants),
        ] {
            parse(text).unwrap();
            for bad in ["()".to_owned(), text.replace("Taste(", "Fixture(")] {
                let error = parse(&bad).unwrap_err();
                assert!(error.contains(file) && error.contains("author"), "{error}");
            }
        }
        for (field, value) in [
            ("lod_budget_px", "0.0"),
            ("terrain_lod_budget_px", "0.0"),
            ("killcam_record_wait_s", "NaN"),
            ("terrain_lod_deviation_rungs_m", "[]"),
            ("terrain_lod_deviation_rungs_m", "[0.0]"),
            ("terrain_lod_deviation_rungs_m", "[0.1, 0.2]"),
            ("terrain_lod_deviation_rungs_m", "[0.0, NaN]"),
            ("terrain_lod_deviation_rungs_m", "[0.0, 0.1, 0.1]"),
            ("orbit_near_m", "NaN"),
            ("orbit_near_m", "0.0"),
            ("orbit_near_m", "1e30"),
            ("commander_fov_before_bind_rad", "4.0"),
        ] {
            let bad = replace_value(camera, field, value);
            let error = camera_constants(&bad).unwrap_err();
            assert!(error.contains(field) && error.contains("author"), "{error}");
        }
        for (file, field, parse) in [
            (
                CAMERA_FILE,
                "pivot_lift_m",
                camera_constants as fn(&str) -> Result<String, String>,
            ),
            (STYLE_FILE, "overlay_font_px", style_constants),
        ] {
            let error = parse(&format!("({field}: 2.0)")).unwrap_err();
            assert!(
                error.contains(file) && error.contains(field) && error.contains("author"),
                "{error}"
            );
        }
        let error = style_constants(&replace_value(style, "overlay_font_px", "-1.0")).unwrap_err();
        assert!(error.contains("overlay_font_px"));
        let error =
            style_constants(&replace_value(style, "text_srgb", "(NaN, 0.0, 0.0)")).unwrap_err();
        assert!(error.contains("text_srgb"));
        for (field, value) in [
            ("net_debug_refresh_s", "NaN"),
            ("net_debug_refresh_s", "0.0"),
            ("net_debug_refresh_s", "1e100"),
            ("net_warn_excess_ticks", "0"),
        ] {
            let error = style_constants(&replace_value(style, field, value)).unwrap_err();
            assert!(error.contains(field) && error.contains("author"), "{error}");
        }
        // Fixture: reversed bands must be refused independently of the shipped UI tastes.
        let reversed = replace_value(style, "net_alarm_excess_ticks", "1");
        let reversed = replace_value(&reversed, "net_warn_excess_ticks", "2");
        let error = style_constants(&reversed).unwrap_err();
        assert!(
            error.contains("net_warn_excess_ticks") && error.contains("author"),
            "{error}"
        );
    }

    fn replace_value(text: &str, field: &str, value: &str) -> String {
        let prefix = format!("\n    {field}: (");
        let start = text.find(&prefix).unwrap() + prefix.len();
        let rest = &text[start..];
        let end = start
            + rest
                .find(", Taste(")
                .into_iter()
                .chain(rest.find(", StandIn("))
                .min()
                .unwrap();
        format!("{}{value}{}", &text[..start], &text[end..])
    }
}
