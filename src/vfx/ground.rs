//! The game's damage-model view: the painted ground under an effect, one seam for everything that
//! lifts soil ([`super::impact`], [`super::muzzle`]).

use bevy::camera::Exposure;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::ground_types::GroundTypes;
use crate::track_prints::PrintDepths;

/// The ground-type table and its painted mask.
#[derive(SystemParam)]
pub(super) struct PaintedGround<'w> {
    table: Option<Res<'w, GroundTypes>>,
    mask: Option<Res<'w, PrintDepths>>,
}

impl PaintedGround<'_> {
    /// Both halves, or `None` in a world with no painted terrain: a headless harness, the flat
    /// fixture slab. `track_prints::install` inserts them together, so one alone is a bug.
    pub(super) fn resolved(&self) -> Option<(&GroundTypes, &PrintDepths)> {
        match (self.table.as_deref(), self.mask.as_deref()) {
            (Some(table), Some(mask)) => Some((table, mask)),
            (None, None) => None,
            _ => panic!("drawn terrain installs its ground table and painted mask together"),
        }
    }
}

/// The key light's radiance per unit albedo on level ground: what lit soil of albedo one draws at
/// under this camera.
pub(super) fn soil_light(exposure: Exposure) -> f32 {
    crate::sky::level_ground_irradiance_lux() / std::f32::consts::PI * exposure.exposure()
}

/// The source whose share interval holds `draw`, a point in `[0, Σ share)`: the sources laid end
/// to end in slot order. A draw at or past the total (float rounding at the top end) is the last
/// source with a positive share.
pub(super) fn soil_of<T: Copy>(sources: &[(f32, T); 4], mut draw: f32) -> T {
    for (share, soil) in sources {
        if *share > 0.0 && draw < *share {
            return *soil;
        }
        draw -= share;
    }
    sources
        .iter()
        .rev()
        .find(|(share, _)| *share > 0.0)
        .expect("a ground that lifts soil has a source with a positive share")
        .1
}
