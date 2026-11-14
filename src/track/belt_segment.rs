//! The running gear's support law against loose pieces and the layers its soft run meets them on,
//! in the game's tank driving physics (game state only).

use avian3d::prelude::CollisionLayers;
use bevy::prelude::*;

use crate::Layer;

/// The calibrated envelope law the soft run carries per metre of belt, the spring channel's own
/// (`ForceParams::support_stiffness_per_m`, `support_damping_per_m`): published by whichever
/// driver calibrated it, the game's `TrackGear` or the sandbox's rig.
#[derive(Resource, Clone, Copy, Debug)]
pub(crate) struct SegmentLaw {
    /// N/m per metre of belt.
    pub(crate) stiffness_per_m: f32,
    /// N·s/m per metre of belt.
    pub(crate) damping_per_m: f32,
}

/// What a run meets: loose pieces only.
pub(crate) fn segment_layers() -> CollisionLayers {
    CollisionLayers::new([Layer::TrackBelt], [Layer::Debris])
}
