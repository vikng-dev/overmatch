//! Required inspection-course construction and presentation choices; no physical law is selected here.

use crate::provenance::{Sourced, from_shipped_str};
use serde::Deserialize;

const FILE: &str = "assets/dev/track_sandbox.ron";
const DATA: &str = include_str!("../../assets/dev/track_sandbox.ron");

pub(super) fn scene() -> &'static Scene {
    static SCENE: std::sync::OnceLock<Scene> = std::sync::OnceLock::new();
    SCENE.get_or_init(|| Scene::parse(DATA).unwrap_or_else(|error| panic!("{error}")))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Trench {
    pub(super) forward_m: Sourced<f32>,
    pub(super) width_m: Sourced<f32>,
    pub(super) label: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Scene {
    pub(super) trenches: Vec<Trench>,
    pub(super) washboards: Sourced<Vec<(f32, f32, usize, f32)>>,
    pub(super) lane_near_m: Sourced<f32>,
    pub(super) lane_far_m: Sourced<f32>,
    pub(super) lane_width_m: Sourced<f32>,
    pub(super) obstacle_width_m: Sourced<f32>,
    pub(super) trench_floor_y_m: Sourced<f32>,
    pub(super) slab_thickness_m: Sourced<f32>,
    pub(super) slope_pad_deg: Sourced<f32>,
    pub(super) slope_pad_center_m: Sourced<[f32; 3]>,
    pub(super) slope_pad_size_m: Sourced<f32>,
    pub(super) slope_pad_thickness_m: Sourced<f32>,
    pub(super) runway_center_m: Sourced<[f32; 3]>,
    pub(super) runway_size_m: Sourced<[f32; 2]>,
    pub(super) runway_spawn_m: Sourced<[f32; 3]>,
    pub(super) contact_probe_m: Sourced<f32>,
    pub(super) slip_saturation_mps: Sourced<f32>,
    pub(super) camera_eye_m: Sourced<[f32; 3]>,
    pub(super) camera_target_m: Sourced<[f32; 3]>,
    pub(super) light_lux: Sourced<f32>,
    pub(super) light_eye_m: Sourced<[f32; 3]>,
    pub(super) ground_srgb: Sourced<[f32; 3]>,
    pub(super) obstacle_srgb: Sourced<[f32; 3]>,
    pub(super) step_height_m: Sourced<f32>,
    pub(super) step_z_m: Sourced<f32>,
    pub(super) step_depth_m: Sourced<f32>,
    pub(super) ramp_run_m: Sourced<f32>,
    pub(super) ramp_thickness_m: Sourced<f32>,
    pub(super) ramp_deg: Sourced<f32>,
    pub(super) ramp_entry_y_m: Sourced<f32>,
    pub(super) ramp_z_m: Sourced<f32>,
    pub(super) washboard_thickness_divisor: Sourced<f32>,
    pub(super) hub_radius_m: Sourced<f32>,
    pub(super) dot_min_radius_m: Sourced<f32>,
    pub(super) dot_max_growth_m: Sourced<f32>,
    pub(super) normal_stub_m: Sourced<f32>,
    pub(super) slip_green_fall: Sourced<f32>,
    pub(super) slip_blue: Sourced<f32>,
    pub(super) traction_min_n_squared: Sourced<f32>,
    pub(super) force_n_per_m: Sourced<f32>,
    pub(super) hub_srgb: Sourced<[f32; 3]>,
    pub(super) drive_hub_srgb: Sourced<[f32; 3]>,
    pub(super) belt_srgb: Sourced<[f32; 3]>,
    pub(super) belt_outer_srgb: Sourced<[f32; 3]>,
    pub(super) normal_srgb: Sourced<[f32; 3]>,
    pub(super) support_force_srgb: Sourced<[f32; 3]>,
    pub(super) traction_force_srgb: Sourced<[f32; 3]>,
    pub(super) cast_srgb: Sourced<[f32; 3]>,
    pub(super) reference_srgb: Sourced<[f32; 3]>,
    pub(super) rubble_construction: Sourced<String>,
    pub(super) rubble_pile_at_m: Sourced<[f32; 3]>,
    pub(super) rubble_wall_at_m: Sourced<[f32; 2]>,
    pub(super) rubble_spot_ahead_m: Sourced<f32>,
    pub(super) rubble_pile_pieces: Sourced<usize>,
    pub(super) rubble_pile_row: Sourced<PileRow>,
    pub(super) rubble_pile_taper: Sourced<f32>,
    pub(super) rubble_pile_yaw_rad: Sourced<f32>,
    pub(super) rubble_piece_srgb: Sourced<[f32; 3]>,
    pub(super) rubble_wall_srgb: Sourced<[f32; 3]>,
}

/// A square drop layer whose nonzero capacity fits the spawning index type.
#[derive(Deserialize)]
#[serde(try_from = "usize")]
pub(super) struct PileRow {
    width: usize,
    capacity: usize,
}

impl TryFrom<usize> for PileRow {
    type Error = String;

    fn try_from(width: usize) -> Result<Self, Self::Error> {
        let capacity = width.checked_mul(width).filter(|&n| n > 0).ok_or_else(|| {
            refuse(
                "rubble_pile_row",
                "author a positive row width whose square fits usize",
            )
        })?;
        Ok(Self { width, capacity })
    }
}

impl PileRow {
    pub(super) fn width(&self) -> usize {
        self.width
    }

    pub(super) fn capacity(&self) -> usize {
        self.capacity
    }
}

impl Scene {
    fn parse(text: &str) -> Result<Self, String> {
        let data: Self = from_shipped_str(FILE, text)?;
        data.validate()?;
        Ok(data)
    }

    fn validate(&self) -> Result<(), String> {
        if self.rubble_construction.0.trim().is_empty() {
            return Err(refuse(
                "rubble_construction",
                "name the construction asset to cut into the yard",
            ));
        }
        for (field, value) in [
            ("lane_width_m", self.lane_width_m.0),
            ("obstacle_width_m", self.obstacle_width_m.0),
            ("slab_thickness_m", self.slab_thickness_m.0),
            ("slope_pad_size_m", self.slope_pad_size_m.0),
            ("slope_pad_thickness_m", self.slope_pad_thickness_m.0),
            ("contact_probe_m", self.contact_probe_m.0),
            ("slip_saturation_mps", self.slip_saturation_mps.0),
            ("light_lux", self.light_lux.0),
            ("step_height_m", self.step_height_m.0),
            ("step_depth_m", self.step_depth_m.0),
            ("ramp_run_m", self.ramp_run_m.0),
            ("ramp_thickness_m", self.ramp_thickness_m.0),
            (
                "washboard_thickness_divisor",
                self.washboard_thickness_divisor.0,
            ),
            ("hub_radius_m", self.hub_radius_m.0),
            ("dot_min_radius_m", self.dot_min_radius_m.0),
            ("dot_max_growth_m", self.dot_max_growth_m.0),
            ("normal_stub_m", self.normal_stub_m.0),
            ("force_n_per_m", self.force_n_per_m.0),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(refuse(field, "author a finite positive value"));
            }
        }
        if self.rubble_pile_pieces.0 == 0 {
            return Err(refuse("rubble_pile_pieces", "author at least one piece"));
        }
        let layers = self
            .rubble_pile_pieces
            .0
            .div_ceil(self.rubble_pile_row.0.capacity());
        if !(0.0..1.0 / layers as f32).contains(&self.rubble_pile_taper.0) {
            return Err(refuse(
                "rubble_pile_taper",
                "author a nonnegative taper that leaves the top layer a positive width",
            ));
        }
        for (field, value) in [
            ("rubble_spot_ahead_m", self.rubble_spot_ahead_m.0),
            ("rubble_pile_yaw_rad", self.rubble_pile_yaw_rad.0),
            ("lane_near_m", self.lane_near_m.0),
            ("lane_far_m", self.lane_far_m.0),
            ("trench_floor_y_m", self.trench_floor_y_m.0),
            ("step_z_m", self.step_z_m.0),
            ("ramp_entry_y_m", self.ramp_entry_y_m.0),
            ("ramp_z_m", self.ramp_z_m.0),
            ("traction_min_n_squared", self.traction_min_n_squared.0),
        ] {
            if !value.is_finite() {
                return Err(refuse(field, "author a finite value"));
            }
        }
        for (field, value) in [
            ("slope_pad_deg", self.slope_pad_deg.0),
            ("ramp_deg", self.ramp_deg.0),
        ] {
            if !value.is_finite() || value.abs() >= 90.0 {
                return Err(refuse(
                    field,
                    "author a finite incline strictly between -90 and 90 degrees",
                ));
            }
        }
        if self.lane_near_m.0 <= self.lane_far_m.0 {
            return Err(refuse(
                "lane_near_m / lane_far_m",
                "put the near end above the far end on Z",
            ));
        }
        if self.washboard_thickness_divisor.0 < 1.0 {
            return Err(refuse(
                "washboard_thickness_divisor",
                "author at least one so bumps fit their periods",
            ));
        }
        if self.traction_min_n_squared.0 < 0.0 {
            return Err(refuse(
                "traction_min_n_squared",
                "author a nonnegative squared force",
            ));
        }
        for (field, value) in [
            ("slip_green_fall", self.slip_green_fall.0),
            ("slip_blue", self.slip_blue.0),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(refuse(field, "author a finite colour fraction in [0, 1]"));
            }
        }
        for (field, value) in [
            ("slope_pad_center_m", self.slope_pad_center_m.0),
            ("runway_center_m", self.runway_center_m.0),
            ("runway_spawn_m", self.runway_spawn_m.0),
            ("camera_eye_m", self.camera_eye_m.0),
            ("camera_target_m", self.camera_target_m.0),
            ("light_eye_m", self.light_eye_m.0),
            ("rubble_pile_at_m", self.rubble_pile_at_m.0),
        ] {
            if value.iter().any(|v| !v.is_finite()) {
                return Err(refuse(field, "author finite coordinates"));
            }
        }
        if self.camera_eye_m.0 == self.camera_target_m.0 || self.light_eye_m.0 == [0.0; 3] {
            return Err(refuse(
                "camera_eye_m / light_eye_m",
                "separate each eye from its look target",
            ));
        }
        if self
            .runway_size_m
            .0
            .iter()
            .any(|v| !v.is_finite() || *v <= 0.0)
        {
            return Err(refuse(
                "runway_size_m",
                "author finite positive X/Z lengths",
            ));
        }
        if self.rubble_wall_at_m.0.iter().any(|v| !v.is_finite()) {
            return Err(refuse("rubble_wall_at_m", "author finite coordinates"));
        }
        self.validate_colours()?;
        let mut previous = self.lane_near_m.0;
        for (index, trench) in self.trenches.iter().enumerate() {
            let (z, width) = (trench.forward_m.0, trench.width_m.0);
            let (near, far) = (-z + width / 2.0, -z - width / 2.0);
            if !z.is_finite()
                || !width.is_finite()
                || width <= 0.0
                || near >= previous
                || far <= self.lane_far_m.0
            {
                return Err(refuse(
                    &format!("trenches[{index}]"),
                    "author finite positive widths, ordered nonoverlapping gaps inside the lane",
                ));
            }
            if trench.label.trim().is_empty() {
                return Err(refuse(
                    &format!("trenches[{index}].label"),
                    "name the reset witness",
                ));
            }
            previous = far;
        }
        for (index, &(start, period, count, height)) in self.washboards.0.iter().enumerate() {
            if !start.is_finite()
                || !period.is_finite()
                || period <= 0.0
                || count == 0
                || !height.is_finite()
                || height <= 0.0
            {
                return Err(refuse(
                    &format!("washboards[{index}]"),
                    "author a finite start, positive period/height and nonzero bump count",
                ));
            }
        }
        Ok(())
    }
    fn validate_colours(&self) -> Result<(), String> {
        for (field, value) in [
            ("ground_srgb", self.ground_srgb.0),
            ("obstacle_srgb", self.obstacle_srgb.0),
            ("hub_srgb", self.hub_srgb.0),
            ("drive_hub_srgb", self.drive_hub_srgb.0),
            ("belt_srgb", self.belt_srgb.0),
            ("belt_outer_srgb", self.belt_outer_srgb.0),
            ("normal_srgb", self.normal_srgb.0),
            ("support_force_srgb", self.support_force_srgb.0),
            ("traction_force_srgb", self.traction_force_srgb.0),
            ("cast_srgb", self.cast_srgb.0),
            ("reference_srgb", self.reference_srgb.0),
            ("rubble_piece_srgb", self.rubble_piece_srgb.0),
            ("rubble_wall_srgb", self.rubble_wall_srgb.0),
        ] {
            if value
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            {
                return Err(refuse(field, "author finite sRGB channels in [0, 1]"));
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

    /// The yard's source is an authored choice; an absent or blank key cannot invent a default.
    #[test]
    fn rubble_requires_an_authored_construction() {
        let original = DATA
            .lines()
            .find(|line| line.trim_start().starts_with("rubble_construction:"))
            .unwrap();
        // Fixture: an omitted choice and an explicitly blank choice.
        for replacement in [
            "",
            "rubble_construction: (\"\", Taste(\"Fixture absent construction\")),",
        ] {
            let text = DATA.replace(original, replacement);
            let error = Scene::parse(&text)
                .err()
                .expect("a source construction is required");
            assert!(
                error.contains(FILE) && error.contains("rubble_construction"),
                "{error}"
            );
        }
    }

    /// Spawned walls require finite positions; checking only the pile leaves physics invalid.
    #[test]
    fn wall_coordinates_are_refused_before_spawn() {
        // Fixture: each nonfinite coordinate, in either authored axis.
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for axis in 0..2 {
                let mut data = Scene::parse(DATA).expect("the shipped course is admitted");
                data.rubble_wall_at_m.0[axis] = bad;
                let error = data
                    .validate()
                    .expect_err("nonfinite wall position is refused");
                for required in [FILE, "rubble_wall_at_m", "finite coordinates"] {
                    assert!(error.contains(required), "{error}");
                }
            }
        }
    }

    /// Spawning divides by a layer's capacity; authored overflow or zero must fail at load.
    #[test]
    fn unrepresentable_drop_layers_are_refused_at_load() {
        let original = DATA
            .lines()
            .find(|line| line.trim_start().starts_with("rubble_pile_row:"))
            .unwrap();
        // Fixture: a zero divisor and an overflowing authored row width.
        for width in [0, usize::MAX] {
            let text = DATA.replace(
                original,
                &format!("rubble_pile_row: ({width}, Taste(\"Fixture invalid row\")),"),
            );
            let error = Scene::parse(&text)
                .err()
                .expect("invalid layer capacity is refused");
            for required in [FILE, "rubble_pile_row", "positive row width", "square fits"] {
                assert!(error.contains(required), "{error}");
            }
        }
    }

    /// The dimension gate names its field and repair, independent of authored course tastes.
    #[test]
    fn dimension_gate_refuses_invalid_width_with_its_fix() {
        let mut data = Scene::parse(DATA).expect("the shipped course is admitted");
        data.lane_width_m.0 = -1.0; // Fixture: invalid construction width.
        let error = data
            .validate()
            .expect_err("negative course width is refused before spawn");
        for required in [FILE, "lane_width_m", "finite positive"] {
            assert!(error.contains(required), "{error}");
        }
    }
}
