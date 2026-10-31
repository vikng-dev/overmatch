//! The game's damage-model view: the surface facts an impact and a persistent mark carry.
//!
//! These compact values travel with the resolved surface, so a delayed client effect and a late
//! joiner's decal use the same substance response as the authority. Every hard substance authors
//! them with provenance. Quantisation is DERIVED: linear colour and metallic in units of 1/255,
//! roughness gain in units of 1/64. No substance name or body kind selects a response in logic.

use crate::provenance::Sourced;
use serde::{Deserialize, Serialize};

#[derive(
    bevy::prelude::Component,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
)]
pub struct SurfaceFeedback {
    pub exposed: [u8; 3],
    pub powder: [u8; 3],
    pub metallic: u8,
    pub roughness_gain: u8,
    pub sparks: bool,
}

impl SurfaceFeedback {
    #[cfg(test)]
    pub(crate) const fn fixture() -> Self {
        // FIXTURE DATA: a sparking metallic surface; no shipped substance reads it.
        Self {
            exposed: [128, 130, 135],
            powder: [120, 120, 120],
            metallic: 255,
            roughness_gain: 64,
            sparks: true,
        }
    }

    pub(crate) fn exposed_linear(self) -> [f32; 3] {
        self.exposed.map(|c| f32::from(c) / 255.0)
    }
    pub(crate) fn powder_linear(self) -> [f32; 3] {
        self.powder.map(|c| f32::from(c) / 255.0)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct File {
    exposed_linear_rgb: Sourced<(f32, f32, f32)>,
    powder_linear_rgb: Sourced<(f32, f32, f32)>,
    metallic: Sourced<f32>,
    roughness_gain: Sourced<f32>,
    sparks: Sourced<bool>,
}

impl File {
    pub(super) fn resolve(self, name: &str) -> Result<SurfaceFeedback, String> {
        let unit = |field: &str, value: f32| {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(format!(
                    "substance {name:?}: feedback.{field} must be finite in 0..=1 (got {value}) — author a linear surface response"
                ));
            }
            Ok((value * 255.0).round() as u8)
        };
        let rgb = |field: &str, (r, g, b)| -> Result<[u8; 3], String> {
            Ok([unit(field, r)?, unit(field, g)?, unit(field, b)?])
        };
        let gain = self.roughness_gain.0;
        if !gain.is_finite() || gain < 1.0 / 64.0 || gain > 255.0 / 64.0 {
            return Err(format!(
                "substance {name:?}: feedback.roughness_gain must be in 1/64..=255/64 — author a representable surface response"
            ));
        }
        Ok(SurfaceFeedback {
            exposed: rgb("exposed_linear_rgb", self.exposed_linear_rgb.0)?,
            powder: rgb("powder_linear_rgb", self.powder_linear_rgb.0)?,
            metallic: unit("metallic", self.metallic.0)?,
            roughness_gain: (gain * 64.0).round() as u8,
            sparks: self.sparks.0,
        })
    }
}
