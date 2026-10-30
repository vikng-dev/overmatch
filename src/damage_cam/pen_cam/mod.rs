//! The pen cam: a hit explained in a corner inset, top-right over the game, one frame per verdict
//! — the see-through hull for a penetration, the plate slice for a non-pen, the outside view for a
//! ricochet or for damage — opening 0.2 s after the impact and closing on its own. Specification:
//! `.agents/scratch/damage-cam-2026-10-08/demo/ux-spec.md` (§3–§5) under the owner record
//! `…/product/in-battle/owner-direction-2026-10-10.md` (four verdicts).
//!
//! The presentation (this module's [`Corner`], [`Exhibits`], [`Knobs`], and the stage, look, plan,
//! timeline and corner HUD under it) draws whatever a driver puts in [`Corner`]: a record on show,
//! its frame and its clock. Two drivers write it:
//!
//! * [`game`], the corner slot in play: the shooter's corner, each explanation of the local
//!   player's own hit once its world impact is drawn, by the delivery store's rule. Beside it the
//!   x-ray, the player's own tank dressed in the look on the battlefield under the latest
//!   record that struck it, is its own presentation (`game::xray`): no corner, no stage copy.
//! * [`demo`], the design bench over the recorded scenarios in `assets/dev/pen_cam/`: its own
//!   battlefield, virtual-time control, keys and the kill cam's lesson.
//!
//! A view over the record: nothing here marches a shell or reads the sim. Every position the shot
//! reached is the record's (hull-local, the stage's frame); the tank asset, posed by the record's
//! servo angles ([`super::shapes`]), supplies only each component's shape and the whole tank's
//! bounds, which the camera frames. Every timing comes from [`timeline`] and the record's own
//! places, and the record's few derived quantities are named in [`plan`].

mod demo;
pub mod game;
mod hud;
mod killcam;
mod look;
mod material;
mod picture;
mod plan;
pub(crate) mod potency;
mod stage;
mod timeline;
mod view;
mod world;

use std::collections::BTreeMap;
use std::sync::Arc;

use bevy::camera::CameraUpdateSystems;
use bevy::camera::visibility::VisibilitySystems;
use bevy::ecs::schedule::ScheduleConfigs;
use bevy::ecs::system::ScheduleSystem;
use bevy::prelude::*;

pub use self::demo::demo_plugin;
use self::plan::Plan;
use self::timeline::CornerKind;
use crate::camera::{CaptiveCameraPlaced, GunnerCameraPlaced, OrbitCameraSet};

/// The corner as it stands this frame.
#[derive(Resource, Default, Clone, PartialEq)]
pub struct Corner {
    /// The record on show, if the corner is open.
    pub showing: Option<Showing>,
    /// The turn before it while the crossfade runs: its opacity.
    pub fading: Option<f32>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Showing {
    /// Index into [`Exhibits`].
    pub record: usize,
    /// Which turn of the driver's: a new turn rebuilds the stage even for the same record.
    pub turn: usize,
    pub kind: CornerKind,
    /// Corner seconds since the turn opened, holds excluded.
    pub c: f32,
    pub opacity: f32,
    pub scale: f32,
    /// Fraction of the turn still to play, for the progress hairline.
    pub remaining: f32,
    /// Where the record is drawn.
    pub surface: Surface,
}

/// Where a [`Showing`] is drawn. The x-ray is on neither: it is not a showing of the corner
/// but a dressing of the live tank.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Surface {
    /// The corner inset.
    Corner,
    /// On the battlefield, over the struck tank itself: the kill cam's lesson.
    World,
}

/// One record as the stage draws it: its plan, and the struck tank's servo angles (degrees, by
/// node name) the stage view is posed by.
#[derive(Clone)]
pub struct Exhibit {
    pub plan: Arc<Plan>,
    pub servo_deg: BTreeMap<String, f32>,
}

/// The records a driver can show; [`Showing::record`] indexes them.
#[derive(Resource, Default)]
pub struct Exhibits(pub Vec<Exhibit>);

/// How the presentation draws: the player's reduce-motion setting, and the bench's switches, which
/// the game leaves at their defaults.
#[derive(Resource, Clone, PartialEq, Debug)]
pub struct Knobs {
    /// No orbit or drift, and the record's events land with short fades.
    pub reduce_motion: bool,
    /// The slice frame's camera is orthographic.
    pub slice_ortho: bool,
    /// The corner fills the window.
    pub expanded: bool,
    /// Each field's axis drawn as a line.
    pub axis_line: bool,
    /// The viewer's drag on the corner camera (yaw, pitch; rad).
    pub orbit: Vec2,
}

impl Default for Knobs {
    fn default() -> Self {
        Self {
            reduce_motion: false,
            slice_ortho: true,
            expanded: false,
            axis_line: false,
            orbit: Vec2::ZERO,
        }
    }
}

/// Every entity the corner spawns for its own drawing: the inset camera, its lights, the shell, the
/// stage view, the corner's UI, the mask cameras. A driver that closes the corner for good despawns
/// them all ([`game`]).
#[derive(Component, Default, Clone, Copy)]
pub struct CornerRig;

/// The camera that draws the battlefield a [`Surface::World`] record is drawn on.
#[derive(Component, Default, Clone, Copy)]
pub struct WorldViewer;

/// The materials the stage draws with, and the render world's pass that draws the fields' depth
/// pictures they read.
pub(super) fn materials(app: &mut App) {
    app.add_plugins((
        MaterialPlugin::<material::GlowMaterial>::default(),
        MaterialPlugin::<material::OutlineMaterial>::default(),
        MaterialPlugin::<material::SliceMaterial>::default(),
        MaterialPlugin::<look::LookMaterial>::default(),
        MaterialPlugin::<look::BeltLookMaterial>::default(),
    ));
    picture::render(app);
}

/// The presentation's resources, the depth pictures' jobs and the mask cameras' following, which a
/// driver mounts once.
///
/// The mask cameras copy their viewer's lens and pose ([`look::follow_lens`], [`look::follow_pose`]),
/// so they run after every writer of either. The corner's viewer, the inset camera, is written
/// only by [`stage::animate`] in `Update`, which every `PostUpdate` system follows. The kill cam's
/// viewer is the battlefield camera: in the demo its writers run in `Update`; in the game the
/// orbit camera writes its pose and lens in [`OrbitCameraSet`], the gunner camera its global pose
/// and lens in [`GunnerCameraPlaced`] after propagation, and the kill cam its captive pose, lens
/// and profile in [`CaptiveCameraPlaced`], so all three edges are named here (`crate::camera`
/// states the writers' own).
pub(super) fn presentation(app: &mut App) {
    app.init_resource::<Corner>()
        .init_resource::<Exhibits>()
        .init_resource::<Knobs>()
        .init_resource::<look::Masks>()
        .init_resource::<look::MaskNeed>()
        .init_resource::<picture::PictureDetail>()
        .add_systems(
            PostUpdate,
            (
                look::follow_lens
                    .after(OrbitCameraSet)
                    .after(GunnerCameraPlaced)
                    .after(CaptiveCameraPlaced)
                    .before(CameraUpdateSystems),
                look::follow_pose
                    .after(TransformSystems::Propagate)
                    .after(OrbitCameraSet)
                    .after(GunnerCameraPlaced)
                    .after(CaptiveCameraPlaced)
                    .before(VisibilitySystems::UpdateFrusta),
            ),
        );
    picture::jobs(app);
    stage::plugin(app);
}

/// The corner's drawing after its masks, in order, from [`Corner`] as its driver left it this
/// frame: the stage, its animation and the corner's frame. A driver runs the corner's size and
/// images (`hud::layout`) first, then [`look::own`], which gives every viewer that draws the base
/// look this frame its masks — the corner's inset and the x-ray's camera alike — then the
/// detail each view asks of the pictures ([`hud::detail`]), and then this; the game gates the
/// corner's halves on its stage view, the x-ray draws beside them.
pub(super) fn present() -> ScheduleConfigs<ScheduleSystem> {
    (stage::rebuild, stage::animate, hud::animate)
        .chain()
        .into_configs()
}
