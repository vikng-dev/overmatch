//! Required settings ranges, defaults, graphics presets and page appearance.
//! Parsed once before the settings feature mounts; player-owned video.ron remains a sparse diff.

use super::preset::Look;
use super::*;
use crate::provenance::{Sourced, from_shipped_str};
use bevy::prelude::Color;
use serde::Deserialize;
use std::sync::LazyLock;

const FILE: &str = "assets/ui/settings.ron";
const DATA: &str = include_str!("../../assets/ui/settings.ron");
static LOOK: LazyLock<SettingsLook> =
    LazyLock::new(|| SettingsLook::parse(DATA).unwrap_or_else(|error| panic!("{error}")));

pub(super) fn get() -> &'static SettingsLook {
    &LOOK
}
pub(super) fn page() -> &'static PageLook {
    &get().page
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SettingsLook {
    /// First cascade far bound, metres, explicitly replacing the builder's character-scale 10.0.
    ///
    /// Bevy's geometric split is `first · (maximum/first)^(i/(n−1))`: at fixed count, raising `first`
    /// flattens the ladder instead of spending its crispness within a room-sized near field.
    ///
    /// DERIVED screen pixels per shadow texel at each cascade's near bound (its worst point),
    /// at 16:9 / 45° vertical / 1964 px tall:
    ///
    /// | config | worst px per texel |
    /// |---|---|
    /// | 4 casc, first 10, 150 m, 2048 (old default) | 6.08 |
    /// | 3 casc, first 10, 1000 m, 4096 | 12.23 |
    /// | 3 casc, first 40, 1000 m, 4096 | 6.12 |
    /// | 3 casc, first 10, 350 m, 4096 | 7.24 |
    /// | **3 casc, first 40, 350 m, 4096 (default)** | **3.64** |
    ///
    /// DERIVED: at 1000 m, 40 m restores crispness to within 0.7% of the old default at 6.7× its reach;
    /// at the shipped 350 m, it is 1.7× crisper at 2.3× the reach. The near texel grows 0.83 → 1.66 cm,
    /// still under two centimetres on a 3 m tank. A short envelope alone does not fix a bad split:
    /// 350 m at first=10 still costs 7.24 px/texel versus 3.64 at first=40.
    ///
    /// Coupled to `super::ShadowCascades` through the exponent: recompute the table if the count changes.
    pub(super) shadow_first_cascade_far_bound_m: Sourced<f32>,
    pub(super) bloom_mip_dimensions: Sourced<[u32; 2]>,
    pub(super) track_print_ranges_m: Sourced<[f32; 4]>,
    pub(super) shadow_cascade_overlap: Sourced<f32>,
    pub(super) pixel_budget: Ladder<f32>,
    pub(super) frame_cap: Ladder<u16>,
    pub(super) ui_scale: Ladder<u16>,
    pub(super) defaults: Defaults,
    pub(super) presets: [Sourced<Look>; 4],
    pub(super) page: PageLook,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Ladder<T> {
    pub(super) min: Sourced<T>,
    pub(super) max: Sourced<T>,
    pub(super) step: Sourced<T>,
}

impl Ladder<f32> {
    pub(super) fn stops(&self) -> u16 {
        1 + ((self.max.0 - self.min.0) / self.step.0).round() as u16
    }
}
impl Ladder<u16> {
    /// Nearest admitted stop, with upper ties. Admission bounds the result to u16;
    /// the rounding sum can exceed it even when both endpoints fit.
    pub(super) fn nearest_stop(&self, value: u16) -> u16 {
        let offset = u32::from(value.clamp(self.min.0, self.max.0) - self.min.0);
        ((offset + u32::from(self.step.0) / 2) / u32::from(self.step.0)) as u16
    }

    pub(super) fn stops(&self) -> u16 {
        1 + (self.max.0 - self.min.0) / self.step.0
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Defaults {
    pub(super) shadow_cascades: Sourced<ShadowCascades>,
    pub(super) shadow_distance: Sourced<ShadowDistance>,
    pub(super) shadow_resolution: Sourced<ShadowResolution>,
    pub(super) msaa: Sourced<MsaaLevel>,
    pub(super) bloom: Sourced<BloomLevel>,
    pub(super) render_scale: Sourced<RenderScaleLevel>,
    pub(super) vsync: Sourced<VsyncMode>,
    pub(super) window_mode: Sourced<WindowModeSetting>,
    pub(super) debug_panel: Sourced<DebugPanel>,
    pub(super) pen_cam_corner: Sourced<PenCamCorner>,
    pub(super) kill_cam: Sourced<KillCam>,
    pub(super) reduce_motion: Sourced<ReduceMotion>,
    pub(super) track_prints: Sourced<TrackPrintRelief>,
    pub(super) track_print_range: Sourced<TrackPrintRange>,
    pub(super) display: Sourced<DisplaySelection>,
    pub(super) lod_pixel_budget: Sourced<PixelBudget>,
    pub(super) frame_cap: Sourced<FrameCap>,
    pub(super) ui_scale: Sourced<UiScalePercent>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PageLook {
    pub(super) track_width_px: Sourced<f32>,
    pub(super) track_hit_height_px: Sourced<f32>,
    pub(super) track_bar_height_px: Sourced<f32>,
    pub(super) stepper_value_well_px: Sourced<f32>,
    /// MEASURED with Barlow Condensed SemiBold: display names need more room than scalar rungs;
    /// the well lets long names overhang the arrows rather than reflowing a row.
    pub(super) display_value_well_px: Sourced<f32>,
    pub(super) slider_value_well_px: Sourced<f32>,
    pub(super) tab_gap_px: Sourced<f32>,
    pub(super) tab_marker_height_px: Sourced<f32>,
    /// The hint wrapper reserves authored line heights; Text itself may grow beyond the slot.
    /// Bevy 0.19 text measurement resolves height as known.or(preferred.or(min)), so putting a
    /// minimum height on Text prevents overlong content from measuring taller.
    /// Source: bevy_ui-0.19.1 `measurement::resolve_axis`; MEASURED on the headless page.
    pub(super) hint_font_px: Sourced<f32>,
    pub(super) hint_line_height_factor: Sourced<f32>,
    pub(super) hint_lines: Sourced<f32>,
    pub(super) card_width_px: Sourced<f32>,
    pub(super) card_padding_px: Sourced<f32>,
    /// Shared by card children and panel rows so the scrolling list has no extra seam.
    pub(super) card_row_gap_px: Sourced<f32>,
    /// Leave visible scrim and clipped rows as scrolling affordances; `scroll_rows` owns reveal.
    pub(super) card_max_height_percent: Sourced<f32>,
    /// LINE-unit wheel notches move a readable row-sized distance; Pixel reports stay unchanged.
    pub(super) wheel_line_px: Sourced<f32>,
    pub(super) card_radius_px: Sourced<f32>,
    pub(super) title_font_px: Sourced<f32>,
    pub(super) title_bottom_margin_px: Sourced<f32>,
    pub(super) hint_top_margin_px: Sourced<f32>,
    pub(super) legend_font_px: Sourced<f32>,
    pub(super) legend_top_margin_px: Sourced<f32>,
    pub(super) row_padding_x_px: Sourced<f32>,
    pub(super) row_padding_y_px: Sourced<f32>,
    pub(super) row_radius_px: Sourced<f32>,
    pub(super) tab_row_gap_px: Sourced<f32>,
    pub(super) control_font_px: Sourced<f32>,
    pub(super) control_gap_px: Sourced<f32>,
    pub(super) track_radius_px: Sourced<f32>,
    pub(super) arrow_padding_x_px: Sourced<f32>,
    pub(super) arrow_padding_y_px: Sourced<f32>,
    row_bg_selected: Sourced<[f32; 4]>,
    row_bg: Sourced<[f32; 4]>,
    slider_track_bg: Sourced<[f32; 4]>,
    slider_fill_color: Sourced<[f32; 4]>,
    slider_fill_disabled: Sourced<[f32; 4]>,
}

impl PageLook {
    pub(super) fn hint_line_height_px(&self) -> f32 {
        self.hint_font_px.0 * self.hint_line_height_factor.0
    }
    pub(super) fn hint_slot_height_px(&self) -> f32 {
        self.hint_line_height_px() * self.hint_lines.0
    }
    pub(super) fn row_bg_selected(&self) -> Color {
        let [r, g, b, a] = self.row_bg_selected.0;
        Color::srgba(r, g, b, a)
    }
    pub(super) fn row_bg(&self) -> Color {
        let [r, g, b, a] = self.row_bg.0;
        Color::srgba(r, g, b, a)
    }
    pub(super) fn slider_track_bg(&self) -> Color {
        let [r, g, b, a] = self.slider_track_bg.0;
        Color::srgba(r, g, b, a)
    }
    pub(super) fn slider_fill_color(&self) -> Color {
        let [r, g, b, a] = self.slider_fill_color.0;
        Color::srgba(r, g, b, a)
    }
    pub(super) fn slider_fill_disabled(&self) -> Color {
        let [r, g, b, a] = self.slider_fill_disabled.0;
        Color::srgba(r, g, b, a)
    }
}

impl SettingsLook {
    fn parse(text: &str) -> Result<Self, String> {
        let data: Self = from_shipped_str(FILE, text)?;
        data.validate()?;
        Ok(data)
    }
    fn validate(&self) -> Result<(), String> {
        let first = self.shadow_first_cascade_far_bound_m.0;
        let near = CascadeShadowConfigBuilder::default().minimum_distance;
        if !first.is_finite()
            || first <= near
            || first
                >= ShadowDistance::ORDER
                    .into_iter()
                    .filter_map(ShadowDistance::distance_m)
                    .reduce(f32::min)
                    .expect("the shadow ladder offers a casting distance")
        {
            return Err(refuse(
                "shadow_first_cascade_far_bound_m",
                "author a finite split between the near clip and the shortest shadow distance",
            ));
        }
        if !self.shadow_cascade_overlap.0.is_finite()
            || !(0.0..1.0).contains(&self.shadow_cascade_overlap.0)
        {
            return Err(refuse(
                "shadow_cascade_overlap",
                "author a finite overlap in [0, 1)",
            ));
        }
        // DERIVED: Bevy 0.19.1 bloom::prepare_bloom_textures makes ilog2(dimension) - 1
        // mips; compute_blend_factor divides by mip_count - 1, requiring at least two.
        if self
            .bloom_mip_dimensions
            .0
            .iter()
            .any(|v| !v.is_power_of_two() || *v < 8)
            || self.bloom_mip_dimensions.0[0] > self.bloom_mip_dimensions.0[1]
        {
            return Err(refuse(
                "bloom_mip_dimensions",
                "author power-of-two dimensions of at least 8 in ascending quality order so Bevy bloom has at least two mips",
            ));
        }
        if self
            .track_print_ranges_m
            .0
            .iter()
            .any(|v| !v.is_finite() || *v <= 0.0)
            || self.track_print_ranges_m.0.windows(2).any(|w| w[0] >= w[1])
        {
            return Err(refuse(
                "track_print_ranges_m",
                "author finite positive distances in ascending quality order",
            ));
        }
        let ladder = &self.pixel_budget;
        let steps = (ladder.max.0 - ladder.min.0) / ladder.step.0;
        if !ladder.min.0.is_finite()
            || !ladder.max.0.is_finite()
            || !ladder.step.0.is_finite()
            || ladder.min.0 <= 0.0
            || ladder.max.0 <= ladder.min.0
            || ladder.step.0 <= 0.0
            || !steps.is_finite()
            || steps < 1.0
            || steps >= f32::from(u16::MAX)
            || steps != steps.round()
        {
            return Err(refuse(
                "pixel_budget",
                "author finite positive min < max and a positive step dividing the span into representable stops",
            ));
        }
        for (field, ladder, extra_stop) in [
            ("frame_cap", &self.frame_cap, 1_u32),
            ("ui_scale", &self.ui_scale, 0_u32),
        ] {
            if ladder.min.0 == 0
                || ladder.max.0 <= ladder.min.0
                || ladder.step.0 == 0
                || (ladder.max.0 - ladder.min.0) % ladder.step.0 != 0
                || 1 + u32::from(ladder.max.0 - ladder.min.0) / u32::from(ladder.step.0)
                    + extra_stop
                    > u32::from(u16::MAX)
            {
                return Err(refuse(
                    field,
                    "author positive min < max and a positive step dividing the span into representable stops",
                ));
            }
        }
        if !(self.pixel_budget.min.0..=self.pixel_budget.max.0)
            .contains(&self.defaults.lod_pixel_budget.0.0)
            || !(self.ui_scale.min.0..=self.ui_scale.max.0).contains(&self.defaults.ui_scale.0.0)
            || (self.defaults.frame_cap.0.0 != 0
                && !(self.frame_cap.min.0..=self.frame_cap.max.0)
                    .contains(&self.defaults.frame_cap.0.0))
        {
            return Err(refuse(
                "defaults",
                "author scalar defaults inside their offered ranges, or zero for an uncapped frame rate",
            ));
        }
        for preset in &self.presets {
            if !(self.pixel_budget.min.0..=self.pixel_budget.max.0)
                .contains(&preset.0.lod_pixel_budget.0)
            {
                return Err(refuse(
                    "presets.lod_pixel_budget",
                    "author a finite budget inside the offered range",
                ));
            }
        }
        self.page.validate()
    }
}

fn refuse(field: &str, fix: &str) -> String {
    format!("{FILE}: {field}: {fix}")
}

impl PageLook {
    fn validate(&self) -> Result<(), String> {
        let page = self;
        for (field, value, positive) in [
            ("page.track_width_px", page.track_width_px.0, true),
            ("page.track_hit_height_px", page.track_hit_height_px.0, true),
            ("page.track_bar_height_px", page.track_bar_height_px.0, true),
            (
                "page.stepper_value_well_px",
                page.stepper_value_well_px.0,
                true,
            ),
            (
                "page.display_value_well_px",
                page.display_value_well_px.0,
                true,
            ),
            (
                "page.slider_value_well_px",
                page.slider_value_well_px.0,
                true,
            ),
            ("page.tab_gap_px", page.tab_gap_px.0, false),
            (
                "page.tab_marker_height_px",
                page.tab_marker_height_px.0,
                true,
            ),
            ("page.hint_font_px", page.hint_font_px.0, true),
            (
                "page.hint_line_height_factor",
                page.hint_line_height_factor.0,
                true,
            ),
            ("page.hint_lines", page.hint_lines.0, true),
            ("page.card_width_px", page.card_width_px.0, true),
            ("page.card_padding_px", page.card_padding_px.0, false),
            ("page.card_row_gap_px", page.card_row_gap_px.0, false),
            (
                "page.card_max_height_percent",
                page.card_max_height_percent.0,
                true,
            ),
            ("page.wheel_line_px", page.wheel_line_px.0, true),
            ("page.card_radius_px", page.card_radius_px.0, false),
            ("page.title_font_px", page.title_font_px.0, true),
            (
                "page.title_bottom_margin_px",
                page.title_bottom_margin_px.0,
                false,
            ),
            ("page.hint_top_margin_px", page.hint_top_margin_px.0, false),
            ("page.legend_font_px", page.legend_font_px.0, true),
            (
                "page.legend_top_margin_px",
                page.legend_top_margin_px.0,
                false,
            ),
            ("page.row_padding_x_px", page.row_padding_x_px.0, false),
            ("page.row_padding_y_px", page.row_padding_y_px.0, false),
            ("page.row_radius_px", page.row_radius_px.0, false),
            ("page.tab_row_gap_px", page.tab_row_gap_px.0, false),
            ("page.control_font_px", page.control_font_px.0, true),
            ("page.control_gap_px", page.control_gap_px.0, false),
            ("page.track_radius_px", page.track_radius_px.0, false),
            ("page.arrow_padding_x_px", page.arrow_padding_x_px.0, false),
            ("page.arrow_padding_y_px", page.arrow_padding_y_px.0, false),
        ] {
            if !value.is_finite() || value < 0.0 || (positive && value == 0.0) {
                return Err(refuse(
                    field,
                    "author a finite size, positive for drawn extents and nonnegative for spacing",
                ));
            }
        }
        if page.card_width_px.0 <= 2.0 * page.card_padding_px.0 {
            return Err(refuse(
                "page.card_width_px",
                "author a card wider than its two padding edges",
            ));
        }
        if page.card_max_height_percent.0 > 100.0 {
            return Err(refuse(
                "page.card_max_height_percent",
                "author a percentage no greater than the viewport",
            ));
        }
        if page.track_bar_height_px.0 > page.track_hit_height_px.0 {
            return Err(refuse(
                "page.track_bar_height_px",
                "keep the drawn bar inside its clickable height",
            ));
        }
        for (field, channels) in [
            ("page.row_bg_selected", page.row_bg_selected.0),
            ("page.row_bg", page.row_bg.0),
            ("page.slider_track_bg", page.slider_track_bg.0),
            ("page.slider_fill_color", page.slider_fill_color.0),
            ("page.slider_fill_disabled", page.slider_fill_disabled.0),
        ] {
            if channels
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            {
                return Err(refuse(field, "author finite sRGBA channels in [0, 1]"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded() -> SettingsLook {
        from_shipped_str(FILE, DATA).unwrap()
    }
    fn refused(data: SettingsLook, field: &str) {
        let error = data.validate().unwrap_err();
        assert!(error.contains(FILE) && error.contains(field), "{error}");
    }

    /// The feature refuses unusable authored settings before it can construct a renderer or page.
    /// Probes mutate typed fields, so valid taste changes cannot make a mutation a no-op.
    #[test]
    fn settings_data_is_required_and_checked_before_play() {
        SettingsLook::parse(DATA).unwrap();
        let error = SettingsLook::parse("()").unwrap_err();
        assert!(
            error.contains(FILE) && error.contains("shadow_first_cascade_far_bound_m"),
            "{error}"
        );
        let error = SettingsLook::parse(&DATA.replace("Taste(", "Fixture(")).unwrap_err();
        assert!(
            error.contains(FILE) && error.contains("Fixture provenance is test-only"),
            "{error}"
        );
        let mut data = decoded();
        data.shadow_first_cascade_far_bound_m.0 = f32::NAN;
        refused(data, "shadow_first_cascade_far_bound_m");
        let mut data = decoded();
        data.shadow_cascade_overlap.0 = 1.0;
        refused(data, "shadow_cascade_overlap");
        let mut data = decoded();
        data.pixel_budget.step.0 = 0.0;
        refused(data, "pixel_budget");
        let mut data = decoded();
        data.frame_cap.step.0 = 0;
        refused(data, "frame_cap");
        let mut data = decoded();
        data.ui_scale.max.0 = data.ui_scale.min.0;
        refused(data, "ui_scale");
        let mut data = decoded();
        data.defaults.lod_pixel_budget.0.0 = f32::NAN;
        refused(data, "defaults");
        let mut data = decoded();
        data.presets[0].0.lod_pixel_budget.0 = f32::NAN;
        refused(data, "presets.lod_pixel_budget");
        let mut data = decoded();
        data.page.card_width_px.0 = 0.0;
        refused(data, "page.card_width_px");
        let mut data = decoded();
        data.page.slider_fill_color.0[0] = 2.0;
        refused(data, "page.slider_fill_color");
        let mut data = decoded();
        data.bloom_mip_dimensions.0[0] = 0;
        refused(data, "bloom_mip_dimensions");
        let mut data = decoded();
        data.track_print_ranges_m.0[0] = f32::INFINITY;
        refused(data, "track_print_ranges_m");
    }

    /// Bloom's final blend needs a nonzero maximum mip index, even on LOW.
    #[test]
    fn bloom_admission_requires_a_calculable_blend() {
        // Fixture: Bevy 0.19.1 uses ilog2(dimension).max(2) - 1 mips;
        // its final blend divides mip zero by mip_count - 1.
        for dimension in [1, 2, 4] {
            let mut data = decoded();
            data.bloom_mip_dimensions.0 = [dimension, 512];
            refused(data, "bloom_mip_dimensions");
        }
        let mut data = decoded();
        data.bloom_mip_dimensions.0 = [8, 512];
        data.validate().unwrap();
    }

    /// Slider rounding must preserve every admitted ladder, including full-width endpoints.
    #[test]
    fn integer_stops_round_without_overflow() {
        let mut data = decoded();
        // Fixture: the rounding sum exceeds u16 while the ladder and its defaults are valid.
        for ladder in [&mut data.frame_cap, &mut data.ui_scale] {
            ladder.min.0 = 1;
            ladder.max.0 = u16::MAX;
            ladder.step.0 = u16::MAX - 1;
        }
        data.validate().unwrap();
        for ladder in [&data.frame_cap, &data.ui_scale] {
            for (value, stop) in [(1, 0), (32767, 0), (32768, 1), (65535, 1)] {
                assert_eq!(ladder.nearest_stop(value), stop);
            }
        }
    }

    /// Valid retuning changes no schema or range invariant. This also exercises the parsed type
    /// without touching the static owner used by concurrent page tests.
    #[test]
    fn valid_settings_tastes_can_be_retuned() {
        let mut data = decoded();
        data.page.card_width_px.0 += 1.0;
        data.page.card_padding_px.0 = 0.0;
        data.shadow_cascade_overlap.0 = 0.0;
        data.validate().unwrap();
    }
}
