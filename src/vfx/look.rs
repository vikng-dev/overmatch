//! Required presentation data for the game's damage-model view; game state only.
//! Admission rejects absent fields, nonfinite values and test-only provenance before play.

use crate::provenance::{Sourced, from_shipped_str};
use bevy::math::{Vec3, Vec4};
use serde::{Deserialize, de::Error as _};
use std::sync::OnceLock;

const FILE: &str = "assets/vfx/presentation.ron";
const DATA: &str = include_str!("../../assets/vfx/presentation.ron");

trait Finite {
    fn finite(&self) -> bool;
}
impl Finite for f32 {
    fn finite(&self) -> bool {
        self.is_finite()
    }
}
impl Finite for u32 {
    fn finite(&self) -> bool {
        true
    }
}
impl<T: Finite> Finite for (T, T) {
    fn finite(&self) -> bool {
        self.0.finite() && self.1.finite()
    }
}
impl Finite for Vec3 {
    fn finite(&self) -> bool {
        self.is_finite()
    }
}
impl Finite for Vec4 {
    fn finite(&self) -> bool {
        self.is_finite()
    }
}

fn finite<T: Finite>(value: &T) -> Result<(), &'static str> {
    if value.finite() {
        Ok(())
    } else {
        Err("finite")
    }
}
fn nonnegative(value: &f32) -> Result<(), &'static str> {
    finite(value)?;
    if *value >= 0.0 {
        Ok(())
    } else {
        Err("nonnegative")
    }
}
fn positive_vec3(value: &Vec3) -> Result<(), &'static str> {
    positive(&value.x)?;
    positive(&value.y)?;
    positive(&value.z)
}
fn dominant_fraction(value: &f32) -> Result<(), &'static str> {
    finite(value)?;
    if *value > 0.5 && *value <= 1.0 {
        Ok(())
    } else {
        Err("in (0.5,1] so opposite unit directions cannot cancel")
    }
}
fn positive(value: &f32) -> Result<(), &'static str> {
    finite(value)?;
    if *value > 0.0 {
        Ok(())
    } else {
        Err("positive")
    }
}
fn fraction(value: &f32) -> Result<(), &'static str> {
    finite(value)?;
    if (0.0..=1.0).contains(value) {
        Ok(())
    } else {
        Err("in [0,1]")
    }
}
fn fraction_vec3(value: &Vec3) -> Result<(), &'static str> {
    value.to_array().iter().try_for_each(fraction)
}
fn positive_pair(value: &(f32, f32)) -> Result<(), &'static str> {
    positive(&value.0)?;
    positive(&value.1)
}
fn sampled_pair(value: &(f32, f32)) -> Result<(), &'static str> {
    positive_pair(value)?;
    if value.0 <= value.1 {
        Ok(())
    } else {
        Err("ordered from low to high")
    }
}
fn count_pair(value: &(u32, u32)) -> Result<(), &'static str> {
    if value.0 > 0 && value.0 <= value.1 {
        Ok(())
    } else {
        Err("a positive ordered count range")
    }
}

// The macro keeps each field's type, required provenance and admission check together.
macro_rules! look_table {
    ($vis:vis $name:ident { $($field:ident: $ty:ty => $check:ident,)* }) => {
        #[derive(Debug)]
        $vis struct $name { $($vis $field: $ty,)* }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Fields { $($field: Sourced<$ty>,)* }
                let fields = Fields::deserialize(deserializer)?;
                $(if let Err(expected) = $check(&fields.$field.0) {
                    return Err(D::Error::custom(format!("{}.{} must be {expected}; author a valid presentation value", stringify!($name), stringify!($field))));
                })*
                Ok(Self { $($field: fields.$field.0,)* })
            }
        }
    };
}

look_table! { pub(super) ImpactLook {
    debris_gravity: Vec3 => finite,
    dust_lifetime: (f32, f32) => sampled_pair,
    dust_size: (f32, f32) => positive_pair,
    dust_normal_push: f32 => finite,
    dust_rise: f32 => finite,
    dust_alpha: f32 => fraction,
    dust_frame_rate: f32 => nonnegative,
    dust_spin_max: f32 => nonnegative,
    ping_lifetime: f32 => positive,
    ping_size: f32 => positive,
    ping_glow: f32 => finite,
    spark_count: (u32, u32) => count_pair,
    spark_speed: (f32, f32) => sampled_pair,
    spark_lifetime: (f32, f32) => sampled_pair,
    spark_stretch: (f32, f32) => sampled_pair,
    spark_width_ratio: f32 => positive,
    spark_glow: f32 => finite,
    spark_spread: (f32, f32) => finite,
    ejecta_count: (u32, u32) => count_pair,
    ejecta_speed: (f32, f32) => sampled_pair,
    ejecta_lifetime: (f32, f32) => sampled_pair,
    ejecta_up_bias: f32 => dominant_fraction,
    ejecta_stretch: (f32, f32) => sampled_pair,
    ejecta_width_ratio: f32 => positive,
    splash_plume_count: (u32, u32) => count_pair,
    splash_plume_size: (f32, f32) => positive_pair,
    splash_plume_aspect: Vec3 => positive_vec3,
    splash_plume_rise: f32 => finite,
    splash_plume_lifetime: f32 => positive,
    splash_ring_size: (f32, f32) => positive_pair,
    splash_ring_lifetime: f32 => positive,
    splash_ring_alpha: f32 => fraction,
    splash_ring_sharpness: f32 => positive,
    splash_cloud_size: (f32, f32) => positive_pair,
    splash_cloud_lifetime: f32 => positive,
    splash_cloud_alpha: f32 => fraction,
    splash_cloud_rise: f32 => finite,
    ground_mark_size: f32 => positive,
    ground_mark_lifetime: f32 => positive,
    scar_fade: Vec4 => finite,
    scar_end_scale: f32 => finite,
    scar_shade: f32 => positive,
    burst_flash_scale: f32 => positive,
    burst_flash_lifetime: f32 => positive,
    burst_flash_end_scale: f32 => finite,
    armor_flash_size: f32 => positive,
    armor_flash_lifetime: f32 => positive,
    armor_flash_glow: f32 => finite,
    armor_spark_count: (u32, u32) => count_pair,
    armor_spark_speed: (f32, f32) => sampled_pair,
    armor_spark_lifetime: (f32, f32) => sampled_pair,
    armor_spark_stretch: (f32, f32) => sampled_pair,
    armor_spark_width_ratio: f32 => positive,
    armor_spark_glow: f32 => finite,
    armor_spark_spread: (f32, f32) => finite,
    armor_spark_deflect_bias: f32 => dominant_fraction,
    spall_puff_size: (f32, f32) => positive_pair,
    spall_puff_lifetime: (f32, f32) => sampled_pair,
    spall_puff_alpha: f32 => fraction,
    spall_puff_rise: f32 => finite,
    flame_lick_size: (f32, f32) => positive_pair,
    flame_lick_lifetime: f32 => positive,
    flame_lick_glow: f32 => finite,
    flame_lick_rise: f32 => finite,
    dust_sharpness: f32 => positive,
    ejecta_sharpness: f32 => positive,
    ejecta_spread: (f32, f32) => finite,
    spark_sharpness: f32 => positive,
    splash_plume_sharpness: f32 => positive,
    splash_cloud_sharpness: f32 => positive,
    impact_reference_energy_j: f32 => positive,
    impact_size_exponent: f32 => positive,
    flash_sharpness: f32 => positive,
    powder_luma: (f32, f32) => finite,
    spark_green: (f32, f32) => finite,
    spark_blue: (f32, f32) => finite,
    spark_heat: (f32, f32) => finite,
    hot_green: (f32, f32) => finite,
    hot_blue: (f32, f32) => finite,
    hot_heat: (f32, f32) => finite,
    surface_clearance_m: f32 => positive,
    wide_quad_clearance_m: f32 => positive,
    ping_end_scale: f32 => finite,
    spark_end_scale: f32 => finite,
    ejecta_end_scale: f32 => finite,
    plume_alpha: f32 => fraction,
    plume_roll_rad: f32 => nonnegative,
    cloud_lift_m: f32 => finite,
    flash_end_scale: f32 => finite,
    spall_push_mps: f32 => finite,
    flame_push_mps: f32 => finite,
    spall_sharpness: f32 => positive,
    soft_particle_depth: f32 => finite,
    powder_erosion_end: f32 => fraction,
    scar_erosion_end: f32 => fraction,
    streak_erosion_end: f32 => fraction,
    flame_erosion_end: f32 => fraction,
}}
pub(super) fn impact() -> &'static ImpactLook {
    &load().impact
}

look_table! { pub(super) MuzzleLook {
    blast_erosion: f32 => fraction,
    smoke_frame_rate: f32 => nonnegative,
    smoke_spin_max: f32 => nonnegative,
    ground_dust_frame_rate: f32 => nonnegative,
    ground_dust_spin_max: f32 => nonnegative,
    ground_dust_erosion_end: f32 => fraction,
    far_full_dressing: f32 => positive,
    shadow_read_window_s: f32 => finite,
    flash_floor: f32 => finite,
    blast_floor: (f32, f32) => finite,
    blast_hot: f32 => finite,
    blast_cool: f32 => finite,
    flash_red: (f32, f32) => finite,
    flash_green: (f32, f32) => finite,
    flash_blue: (f32, f32) => finite,
    flash_heat: (f32, f32) => finite,
    blast_luma: (f32, f32) => finite,
    blast_green: (f32, f32) => finite,
    blast_blue: (f32, f32) => finite,
    smoke_luma: (f32, f32) => finite,
    smoke_red: f32 => finite,
    smoke_green: (f32, f32) => finite,
    smoke_warmth: f32 => finite,
    smoke_blue: f32 => finite,
    smoke_cooling: f32 => finite,
    soil_shade: Vec4 => finite,
    flame_origin_fraction: f32 => finite,
}}
pub(super) fn muzzle() -> &'static MuzzleLook {
    &load().muzzle
}

look_table! { pub(super) TrailLook {
    luma: (f32, f32) => finite,
    red: f32 => finite,
    green: (f32, f32) => finite,
    blue: (f32, f32) => finite,
    warmth: f32 => finite,
    heat: (f32, f32) => finite,
    shape: Vec4 => finite,
    glow: Vec4 => finite,
    lateral_variation: (f32, f32) => finite,
    noise_blend: f32 => fraction,
    noise_cells: (u32, u32) => count_pair,
}}
pub(super) fn trail() -> &'static TrailLook {
    &load().trail
}

look_table! { pub(super) MarksLook {
    depth_fade_m: f32 => positive,
    hole_core: f32 => finite,
    dent_bowl: f32 => finite,
    scratch_width: f32 => finite,
    hole_depth: f32 => finite,
    dent_depth: f32 => finite,
    scratch_depth: f32 => finite,
    core: Vec3 => finite,
    paint_edge: Vec3 => finite,
    exposed_luma: f32 => finite,
    exposed_grain: f32 => finite,
    exposed_metallic: f32 => finite,
    exposed_roughness: f32 => positive,
    roughness_grain: f32 => positive,
    grain_relief: f32 => finite,
    paint_roughness: f32 => positive,
    hole_distortion: f32 => finite,
    hole_edge_frequency: f32 => finite,
    hole_lip: f32 => finite,
    hole_ring_radius: f32 => finite,
    hole_ring_distortion: f32 => finite,
    hole_ring_frequency: f32 => finite,
    hole_ring_offset: f32 => finite,
    grain_frequency: f32 => finite,
    hole_wall_luma: f32 => finite,
    hole_wall_fade: f32 => finite,
    hole_ring_feather: f32 => positive,
    hole_lip_inner_luma: f32 => fraction,
    dent_bowl_luma: f32 => fraction,
    dent_flake_rho: (f32, f32) => finite,
    core_metallic: f32 => finite,
    core_roughness: f32 => positive,
    hole_ring_luma: f32 => finite,
    hole_ring_grain: f32 => finite,
    hole_ring_metallic: f32 => finite,
    hole_ring_roughness: f32 => positive,
    hole_roughness_grain: f32 => positive,
    hole_lip_depth: f32 => finite,
    dent_distortion: f32 => finite,
    dent_edge_frequency: f32 => finite,
    dent_rim_height: f32 => finite,
    dent_rim_radius: f32 => finite,
    dent_rim_width: f32 => finite,
    dent_bare_radius: f32 => finite,
    dent_bare_distortion: f32 => finite,
    dent_bare_frequency: f32 => finite,
    dent_bare_offset: f32 => finite,
    dent_flake_frequency: f32 => finite,
    dent_flake_threshold: f32 => finite,
    dent_crack_outer: f32 => finite,
    dent_crack_distortion: f32 => finite,
    dent_crack_frequency: f32 => finite,
    dent_fine_crack_frequency: f32 => finite,
    dent_crack_sharpness: f32 => positive,
    dent_fine_crack_sharpness: f32 => positive,
    dent_crack_floor: f32 => finite,
    dent_crack_alpha: f32 => fraction,
    dent_bowl_outer: f32 => finite,
    dent_bowl_alpha: f32 => fraction,
    dent_bowl_fade: f32 => finite,
    dent_bowl_roughness: f32 => positive,
    scratch_start: f32 => finite,
    scratch_end: f32 => finite,
    scratch_taper: f32 => finite,
    scratch_edge_frequency: f32 => finite,
    scratch_edge_distortion: f32 => finite,
    scratch_streak_x: f32 => finite,
    scratch_streak_y: f32 => finite,
    scratch_depth_taper: f32 => finite,
    scratch_luma: f32 => finite,
    scratch_grain: f32 => finite,
    scratch_alpha_edge: f32 => finite,
    scratch_metallic: f32 => finite,
    scratch_roughness: f32 => positive,
    scratch_roughness_grain: f32 => positive,
    scratch_lip: f32 => finite,
    scratch_lip_alpha: f32 => fraction,
    scratch_lip_height: f32 => finite,
    scratch_chip_reach: f32 => finite,
    scratch_chip_frequency: f32 => finite,
    scratch_chip_threshold: f32 => finite,
    chip_edge: f32 => finite,
    chip_alpha: f32 => fraction,
    chip_height: f32 => finite,
    chip_reach: f32 => finite,
    chip_frequency: f32 => finite,
    chip_threshold: f32 => finite,
    noise_turn_rad: f32 => finite,
    noise_lacunarity: f32 => finite,
    dent_crack_inner: f32 => finite,
    dent_crack_ring_frequency: f32 => finite,
    dent_crack_fade: f32 => finite,
    scratch_taper_power: f32 => finite,
    hole_wall_gain: f32 => finite,
}}
pub(super) fn marks() -> &'static MarksLook {
    &load().marks
}

look_table! { pub(crate) DiagramLook {
    lead_m: f32 => nonnegative,
    run_out_m: f32 => nonnegative,
    reach_calibres: f32 => positive,
    line_floor: f32 => fraction,
    field_note_px: f32 => positive,
}}
pub(crate) fn diagram() -> &'static DiagramLook {
    &load().diagram
}

look_table! { pub(crate) BuildingCracks {
    half_width_m: f32 => positive,
    standoff_m: f32 => positive,
    color_srgb: Vec3 => fraction_vec3,
}}
pub(crate) fn building_cracks() -> &'static BuildingCracks {
    &load().building_cracks
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct VfxLook {
    impact: ImpactLook,
    muzzle: MuzzleLook,
    trail: TrailLook,
    marks: MarksLook,
    diagram: DiagramLook,
    building_cracks: BuildingCracks,
}

pub(super) fn load() -> &'static VfxLook {
    static LOOK: OnceLock<VfxLook> = OnceLock::new();
    LOOK.get_or_init(|| parse(DATA).unwrap_or_else(|error| panic!("{error}")))
}

fn parse(text: &str) -> Result<VfxLook, String> {
    let look: VfxLook = from_shipped_str(FILE, text)?;

    // These shape radii define a finite decal footprint and nonzero parallax normalization.
    for (field, value) in [
        ("marks.hole_core", look.marks.hole_core),
        ("marks.dent_bowl", look.marks.dent_bowl),
        ("marks.scratch_width", look.marks.scratch_width),
        ("marks.hole_depth", look.marks.hole_depth),
        ("marks.dent_depth", look.marks.dent_depth),
        ("marks.scratch_depth", look.marks.scratch_depth),
    ] {
        if value <= 0.0 || value >= 1.0 {
            return Err(format!(
                "{FILE}: {field} must be in (0,1); author a shape inside its decal square"
            ));
        }
    }
    Ok(look)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Keep provenance valid so a value edit reaches its field's domain gate, independently of
    // the shipped number's spelling. The presentation table declares one field per line.
    fn with_value(field: &str, value: &str) -> String {
        let prefix = format!("{field}:");
        DATA.lines()
            .map(|line| {
                if line.trim_start().starts_with(&prefix) {
                    format!("{field}: ({value}, Taste(\"admission fixture\")),")
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Effect consumers require complete authored values in their declared domains before play.
    #[test]
    fn presentation_is_required_and_checked_before_play() {
        parse(DATA).unwrap();
        for field in [
            "lead_m",
            "run_out_m",
            "reach_calibres",
            "line_floor",
            "field_note_px",
            "half_width_m",
            "standoff_m",
            "color_srgb",
            "dust_size",
        ] {
            let missing = DATA
                .lines()
                .filter(|line| !line.trim_start().starts_with(&format!("{field}:")))
                .collect::<Vec<_>>()
                .join("\n");
            let error = parse(&missing).unwrap_err();
            assert!(error.contains(FILE) && error.contains(field), "{error}");
        }
        for (field, value) in [
            ("ejecta_up_bias", "0.5"),
            ("splash_plume_aspect", "(0.0, 1.0, 1.0)"),
            ("dust_alpha", "NaN"),
            ("half_width_m", "0.0"),
            ("standoff_m", "0.0"),
        ] {
            let error = parse(&with_value(field, value)).unwrap_err();
            assert!(error.contains(FILE) && error.contains(field), "{error}");
        }
        let error = parse(&DATA.replace("Taste(", "Fixture(")).unwrap_err();
        assert!(
            error.contains(FILE) && error.contains("Fixture provenance is test-only"),
            "{error}"
        );
        let cracks = &parse(DATA).unwrap().building_cracks;
        let rgb = cracks.color_srgb.to_array();
        for invalid in [f32::NAN, -0.1, 1.1] {
            for channel in 0..3 {
                let mut color = rgb;
                color[channel] = invalid;
                let value = format!("({}, {}, {})", color[0], color[1], color[2]);
                let error = parse(&with_value("color_srgb", &value)).unwrap_err();
                assert!(
                    error.contains(FILE) && error.contains("color_srgb"),
                    "{error}"
                );
            }
        }
    }
}
