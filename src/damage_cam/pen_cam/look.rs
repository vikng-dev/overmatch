//! The base look: the whole tank as one hueless translucent grey surface one layer deep, its
//! components solid in the untouched grey one layer deep among themselves, the struck ones on the
//! state scale, and the record's fields (its spall cones and a burst's) as one translucent volume
//! in their potency bands, painted in the same bands on the components alone, each by its role's
//! rule ([`Footprint`]): a hard component where a field meets it, a crew station, a witness,
//! wherever the volume holds the field. It is the tank rendering of every x-ray view: the corner,
//! the kill cam's lesson and the x-ray. The band law, the fields' drawing and the draw order are
//! design doc §5, "Potency colour" and "Draw order"
//! (`.agents/docs/design/armor-penetration-and-damage.md`; [`super::potency`], [`super::picture`]).
//! A field lights a point by its law times its depth picture: the strength its law gives the
//! point's direction, where the picture holds the point within that direction's reach to the
//! struck tank's hard bodies. A field the record stops at its origin is drawn nowhere.
//! [`LookMaterial`] (`pen_cam_look.wgsl`) draws every element; [`hull`], [`component`] and
//! [`volume`] choose them, and the stage spawns what they return.
//!
//! Draw order, in every frame that draws the body behind ([`body_behind`]: the glass frames and
//! the slice): each tank mesh is on one layer by its tier ([`Layer::of`]), and the round's model
//! is on the scene's ([`Layer::ROUND`]). The body draws behind everything else: a body fragment
//! is discarded wherever the part mask holds a scene surface, and the fields' volume is drawn after
//! it and over it, writing no depth. The scene (the components, their state fills and outlines, the
//! round's model and the fields' volume) is ordered by depth among itself: a component is opaque
//! and depth-tested, as is the round in its own opaque material, and the volume's march ends at its
//! pixel's nearest scene surface in the part mask, so a component or the round in front of a
//! field hides it and a field in front of either blends over it. The volume tests no other depth,
//! the world's included. The round is drawn whole in every frame, the slice's included, and so is
//! its mask copy. The slice keeps the tank's own materials with the same rule
//! (`super::material::SliceMaterial`): a body fragment on the cut's kept side is discarded where
//! the part mask holds a scene surface, and the part mask holds the components' kept side alone
//! and the round whole. The trail and the rings are outside the order: drawn and depth-tested as
//! their own materials are.
//!
//! The masks: two offscreen images drawn from the viewing camera's own pose, lens and size before
//! it: the hull mask (each pixel's nearest body surface) and the part mask (each pixel's nearest
//! scene surface: a component or the round), each a view normal (rgb, 0..1) and an eye depth (a,
//! m; 0 where nothing is). The eye depth has one convention for every reader: the distance from
//! the eye in a perspective view, the distance along the camera's forward from its plane in an
//! orthographic one. The masks store it, and the nearest-surface tests and the volume's march
//! compare against it. A body or component surface draws only where its fragment is its own
//! mask's nearest, and its edges come from that mask. The masks hold the mask copies of the
//! looked-at tank and its round alone, never the battlefield, so over the world the body
//! composites by straight alpha on whatever the viewer drew behind it, under the world's own
//! depth. Each viewer that draws the look has its own set ([`Masks`]), and every material of a
//! look is built on the set of the viewer that draws it.
//!
//! Tiers come from the tank's data alone ([`Tier`]): main armour, external armour, component and
//! detail, by the substance each mesh wears and the tank file's volume entries.

use std::f32::consts::PI;

use bevy::camera::{Hdr, ImageRenderTarget, RenderTarget};
use bevy::core_pipeline::tonemapping::{DebandDither, Tonemapping};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, CompareFunction, Face, ShaderType, TextureFormat,
};
use bevy::shader::ShaderRef;
use bevy::window::{PrimaryWindow, WindowRef};

use super::material::Blend;
use super::picture::{FACE_LANES, PICTURE_FIELDS, Seen};
use super::plan::{BoxPlan, Cone};
use super::potency::{Band, Potency};
use super::stage::{Stage, StageCamera};
use super::timeline::{self, Condition, CornerKind};
use super::view::Tier;
use super::{Corner, Surface, WorldViewer};
use crate::ballistics::spall_cone::{FieldDescriptor, Terms};
use crate::damage_cam::Role;
use crate::render_policy::CameraProfile;

/// The fields' volume: its opacity at most, and each band's extinction length (record m), the
/// stretch of it along an eye ray that takes `1 − 1/e` of that opacity: short in the knockout band,
/// so its core reads dense, long in the reduced band, so its wide shell reads faint. TUNED, as is
/// every figure below that is not marked otherwise.
const VOLUME_OPACITY: f32 = 0.55;
const KNOCKOUT_EXTINCTION_M: f32 = 0.6;
const REDUCED_EXTINCTION_M: f32 = 3.0;
/// The volume's march: the steps across a field's ball, which set its samples' spacing
/// ([`spacing`]) and bound their count along a piece of an eye ray ([`schedule`]), and the halvings
/// that place a band's edge between two samples, to `2⁻⁸` of a step (DERIVED).
/// `pen_cam_look.wgsl` declares both ([`march`]).
const VOLUME_STEPS: usize = 48;
const VOLUME_REFINE: usize = 8;
/// A volume sorts this far (m) nearer the eye than its mesh's centre, so it draws after every
/// other translucent surface, the glass and the outlines included
/// ([`super::material::OUTLINE_SORT_BIAS`]), and its colour reads unwashed by them.
pub const VOLUME_SORT_BIAS: f32 = 1.0e4;
/// A struck component's red outline (physical px, an inverted hull), so a sliver of fill still
/// reads as struck; the light's share of its shading; its fill line's width (px); the dead hatch's
/// period (px) and the share of it in red.
pub const STATE_OUTLINE_PX: f32 = 2.0;
const STATE_CONTRAST: f32 = 0.3;
const STATE_LINE_PX: f32 = 1.5;
const STATE_HATCH_PX: f32 = 7.0;
const STATE_HATCH_SHARE: f32 = 0.3;
/// The base surface: a hueless grey, its opacity at a grazing angle and the share of that kept
/// face-on, the fresnel power between them, the light's share of the shading (the rest is flat),
/// and its edge colour and opacity. It keeps one opacity wherever it draws, and gives way to a
/// struck component as to every other.
const BASE_GREY: Color = Color::srgb_u8(0xA4, 0xA4, 0xA4);
const BASE_ALPHA: f32 = 0.6;
const BASE_FACE_SHARE: f32 = 0.4;
const BASE_FRESNEL_POWER: f32 = 2.0;
const BASE_CONTRAST: f32 = 0.35;
const BASE_EDGE: Color = Color::srgb_u8(0xE6, 0xE6, 0xE6);
const BASE_EDGE_ALPHA: f32 = 0.35;
/// The components' base: the state scale's untouched grey, opaque at every angle, so the layout
/// reads solid in front of the body's glass at the corner's size and at third-person distance.
/// It carries no HP: an unstruck component looks the same whatever its pool holds. It is drawn
/// in the opaque pass and writes depth, so a blended element that is depth-tested and lies behind
/// it (another component's outline, the trail, a ring) stays hidden whatever the blended pass's
/// centroid order. The body's glass never covers it: a body fragment is discarded wherever the
/// part mask holds a scene surface.
const BASE_COMPONENT_ALPHA: f32 = 1.0;
const BASE_COMPONENT_FACE_SHARE: f32 = 1.0;
/// The fields' painting on a component: its opacity, short of opaque so the component's own look,
/// the HP fill under it included, reads through; and how far (m) past each texel's reach a hard
/// component's surface counts as met, past the tolerance ([`super::picture::Picture::lit`]).
const FOOTPRINT_FILL_ALPHA: f32 = 0.6;
const FOOTPRINT_REACH_SLACK_M: f32 = 0.01;
/// The most fields the look draws: the pictures' ([`PICTURE_FIELDS`]). `pen_cam_look.wgsl`'s
/// arrays hold this many fields' lanes and [`pack`] drops what lies past it, so every record's
/// fields must fit it (`every_record_fits_the_look`).
const FIELDS: usize = PICTURE_FIELDS;
/// One field's lanes ([`pack`]): six of its own and its picture's faces'.
const PRINT_LANES: usize = 6 + FACE_LANES;

/// The uniform block `pen_cam_look.wgsl` reads; its lanes are documented there.
#[derive(ShaderType, Debug, Clone, Copy, PartialEq)]
pub struct LookParams {
    pub color: Vec4,
    pub edge: Vec4,
    pub shade: Vec4,
    pub hatch: Vec4,
    pub mode: Vec4,
    pub state: Vec4,
    pub span: Vec4,
    pub rest: Vec4,
    pub plane: Vec4,
    pub foot: Vec4,
    pub bands: Vec4,
    pub knockout: Vec4,
    pub reduced: Vec4,
    pub volume: Vec4,
    pub fields: Vec4,
    pub prints: [Vec4; PRINT_LANES * FIELDS],
}

impl Default for LookParams {
    fn default() -> Self {
        let zero = Vec4::ZERO;
        Self {
            color: zero,
            edge: zero,
            shade: zero,
            hatch: zero,
            mode: zero,
            state: zero,
            span: zero,
            rest: zero,
            plane: zero,
            foot: zero,
            bands: zero,
            knockout: zero,
            reduced: zero,
            volume: zero,
            fields: zero,
            prints: [zero; PRINT_LANES * FIELDS],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LookKey {
    bias: bool,
    volume: bool,
}

impl From<&LookMaterial> for LookKey {
    fn from(material: &LookMaterial) -> Self {
        Self {
            bias: material.bias,
            volume: material.is_volume(),
        }
    }
}

impl LookKey {
    /// The depth test and depth write its pipeline takes, `None` where it keeps the pass's own: a
    /// volume is drawn whatever stands in front and writes no depth, its march bounded by the part
    /// mask instead.
    fn depth(self) -> Option<(CompareFunction, bool)> {
        self.volume.then_some((CompareFunction::Always, false))
    }
}

const MODE_MASK: f32 = 0.0;
const MODE_BASE: f32 = 2.0;
const MODE_STATE: f32 = 3.0;
const MODE_VOLUME: f32 = 4.0;

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[bind_group_data(LookKey)]
pub struct LookMaterial {
    #[uniform(0)]
    pub params: LookParams,
    #[texture(1)]
    pub hull_mask: Option<Handle<Image>>,
    #[texture(2)]
    pub part_mask: Option<Handle<Image>>,
    /// The record's depth pictures ([`super::picture::Pictures::image`]), full floats, which the
    /// volume and the painting load unfiltered; `None` on every other element, and where the
    /// record draws no field.
    #[texture(3, sample_type = "float", filterable = false)]
    pub pictures: Option<Handle<Image>>,
    pub blend: Blend,
    /// The depth bias a copy of a mesh needs to win the tie with the mesh it covers.
    pub bias: bool,
}

impl LookMaterial {
    fn new(mode: f32, color: LinearRgba, alpha: f32, blend: Blend, masks: &MaskSet) -> Self {
        Self {
            params: LookParams {
                color: Vec4::new(color.red, color.green, color.blue, alpha),
                shade: Vec4::new(1.0, 0.0, 1.0, 1.0),
                mode: Vec4::new(mode, 0.0, 0.0, 0.0),
                ..default()
            },
            hull_mask: Some(masks.hull.clone()),
            part_mask: Some(masks.part.clone()),
            pictures: None,
            blend,
            bias: false,
        }
    }

    pub fn set_fade(&mut self, fade: f32) {
        self.params.shade.w = fade;
    }

    /// Whether it draws the fields' volume ([`volume`]).
    pub fn is_volume(&self) -> bool {
        self.params.mode.x == MODE_VOLUME
    }

    /// The fields, `prints`, in its field lanes ([`pack`]), and each band's extinction length at
    /// their scale: every print of a record shares its hull's. Without its pictures (`pictures`) it
    /// draws none. The colour and opacity lanes, set when the material is made, are untouched.
    pub fn set_fields(&mut self, prints: &[FieldPrint], potency: &Potency) {
        let (lanes, count) = pack(prints, potency);
        self.params.prints = lanes;
        self.params.fields.x = if self.pictures.is_some() {
            count as f32
        } else {
            0.0
        };
        let scale = prints.first().map_or(1.0, |print| print.scale);
        self.params.knockout.w = KNOCKOUT_EXTINCTION_M * scale;
        self.params.reduced.w = REDUCED_EXTINCTION_M * scale;
    }

    /// Discard what lies on `normal`'s side of the plane through `point` (world); `None` cuts
    /// nothing.
    pub fn set_cut(&mut self, plane: Option<(Vec3, Vec3)>) {
        match plane {
            Some((normal, point)) => {
                self.params.plane = normal.extend(normal.dot(point));
                self.params.mode.y = 1.0;
            }
            None => self.params.mode.y = 0.0,
        }
    }

    /// A state fill's frame: `up` (world, unit) and its component's low and high heights along it
    /// (world m), `None` where the component has no known shape and its fill covers it whole.
    pub fn set_fill_span(&mut self, up: Vec3, heights: Option<(f32, f32)>) {
        self.params.state = up.extend(self.params.state.w);
        self.params.span = match heights {
            Some((lo, hi)) => Vec4::new(lo, hi, 1.0, self.params.span.w),
            None => Vec4::new(0.0, 0.0, 0.0, self.params.span.w),
        };
    }

    /// A state fill at `condition`: reduced, red from the bottom up to its share over the
    /// untouched grey, a crisp lighter line between them; out of action, whole, charcoal under
    /// a red hatch.
    pub fn set_condition(&mut self, condition: Condition) {
        let (color, line, hatch) = match condition {
            Condition::Damaged(_) => (timeline::STATE_RED, timeline::STATE_LINE, false),
            Condition::Dead => (timeline::STATE_DEAD, timeline::STATE_RED, true),
        };
        let (color, line) = (LinearRgba::from(color), LinearRgba::from(line));
        self.params.color = Vec4::new(color.red, color.green, color.blue, 1.0);
        self.params.edge = Vec4::new(line.red, line.green, line.blue, 1.0);
        self.params.mode.w = hatch as u8 as f32;
        self.params.state.w = condition.fill();
    }

    /// The fields' painting by `rule` ([`Footprint::lane`]) in the shipped bands' thresholds and
    /// colours ([`band_lanes`]); the fields themselves come each frame ([`Self::set_fields`]).
    fn paint(&mut self, rule: Footprint) {
        let [bands, knockout, reduced] = band_lanes(&Potency::shipped());
        self.params.foot = rule.lane();
        self.params.bands = bands;
        self.params.knockout = knockout;
        self.params.reduced = reduced;
    }

    /// Whether it paints the fields ([`Self::paint`]).
    pub fn is_painted(&self) -> bool {
        self.params.foot.x > 0.0
    }
}

impl Material for LookMaterial {
    fn vertex_shader() -> ShaderRef {
        "shaders/pen_cam_look.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "shaders/pen_cam_look.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        match self.blend {
            Blend::Blended => AlphaMode::Blend,
            Blend::Opaque => AlphaMode::Opaque,
        }
    }

    fn depth_bias(&self) -> f32 {
        if self.is_volume() {
            VOLUME_SORT_BIAS
        } else {
            0.0
        }
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &bevy::pbr::MaterialPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        key: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        let key = key.bind_group_data;
        // A volume is drawn from the faces where the eye ray leaves it, the back faces of its
        // outward-wound box ([`field_bounds`]); every other element from both faces.
        descriptor.primitive.cull_mode = key.volume.then_some(Face::Front);
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            if key.bias {
                // Toward the eye under bevy's reverse Z.
                depth.bias.constant = 8;
                depth.bias.slope_scale = 1.0;
            }
            if let Some((compare, write)) = key.depth() {
                depth.depth_compare = Some(compare);
                depth.depth_write_enabled = Some(write);
            }
        }
        Ok(())
    }
}

/// The two mask images one viewer's base look is drawn against. Without a set the handles are
/// `Handle::default()`, the id bevy keeps its shared white image at, whose every pixel reads as a
/// surface 1 m from the eye: a material built on that set draws against no real mask, and a frame
/// that draws the body behind is built again once its viewer's set exists ([`own`]).
#[derive(Clone, Default, Debug, PartialEq)]
pub struct MaskSet {
    pub hull: Handle<Image>,
    pub part: Handle<Image>,
}

/// The mask sets, one per viewer whose frame draws the body behind this frame: the corner's
/// inset camera or, for the kill cam's lesson, the main camera ([`Corner::showing`],
/// [`body_behind`]); and the x-ray's camera, stated by that presentation ([`MaskNeed`]). The
/// corner and the x-ray show at once, each against its own set; the lesson and the x-ray never
/// do (a kill run suspends the x-ray). Every mask camera names its viewer ([`MaskCamera`]);
/// [`own`] makes and unmakes each set, and nothing else despawns its cameras.
#[derive(Resource, Default)]
pub struct Masks {
    sets: Vec<(MaskOwner, MaskSet)>,
}

/// The camera a mask set is drawn for, at that camera's physical viewport size and scale factor,
/// and the set's two cameras.
#[derive(Clone, Copy, Debug, PartialEq)]
struct MaskOwner {
    viewer: Entity,
    size: UVec2,
    scale: f32,
    cameras: [Entity; 2],
}

impl Masks {
    /// The set drawn for `viewer`, if one exists.
    pub fn of(&self, viewer: Entity) -> Option<&MaskSet> {
        self.sets
            .iter()
            .find(|(owner, _)| owner.viewer == viewer)
            .map(|(_, set)| set)
    }

    /// Take `index`'s set away: its cameras despawned and its images removed.
    fn release(&mut self, index: usize, commands: &mut Commands, images: &mut Assets<Image>) {
        let (owner, set) = self.sets.remove(index);
        for camera in owner.cameras {
            commands.entity(camera).despawn();
        }
        images.remove(set.hull.id());
        images.remove(set.part.id());
    }

    /// The cameras the sets are drawn for.
    #[cfg(test)]
    pub(super) fn viewers(&self) -> Vec<Entity> {
        self.sets.iter().map(|(owner, _)| owner.viewer).collect()
    }
}

/// A viewer whose frame draws the body behind outside the corner this frame — the x-ray's
/// camera — stated by that presentation before [`own`] runs, which reads it beside
/// [`Corner`].
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct MaskNeed(pub Option<Entity>);

/// A mask camera, drawing its mask from `viewer`'s pose and lens.
#[derive(Component)]
pub(super) struct MaskCamera {
    pub(super) viewer: Entity,
}

/// Whether frame `kind` draws the tank's body behind its components and fields, and so reads the
/// masks: the glass frames and the slice. The outside views draw the tank as built, the body in
/// front.
pub fn body_behind(kind: CornerKind) -> bool {
    matches!(
        kind,
        CornerKind::Glass | CornerKind::Victim | CornerKind::Slice
    )
}

fn mask_image(images: &mut Assets<Image>, size: UVec2) -> Handle<Image> {
    images.add(Image::new_target_texture(
        size.x,
        size.y,
        TextureFormat::Rgba16Float,
        None,
    ))
}

/// Give a mask set to each camera whose frame draws the body behind this frame ([`body_behind`],
/// [`MaskNeed`]), at the physical viewport size and scale factor it renders at this frame
/// ([`viewport_now`]), and take a set away when its viewer's need ends: the set's cameras are
/// despawned and its images removed. A new viewer, size or scale makes a new set and rebuilds the
/// stage, whose materials hold the images; the x-ray compares its set's images itself. The set's
/// cameras draw just before their viewer. Runs after every system that retargets or resizes a
/// viewer this frame. A viewer whose target cannot be resolved gets no set until it is.
#[expect(
    clippy::too_many_arguments,
    reason = "a system spanning the corner, the x-ray's need, three camera roles, the window, the masks and their images, and the stage"
)]
pub fn own(
    mut commands: Commands,
    corner: Res<Corner>,
    need: Res<MaskNeed>,
    inset: Query<Entity, With<StageCamera>>,
    main: Query<Entity, With<WorldViewer>>,
    cameras: Query<(&Camera, &RenderTarget), Without<MaskCamera>>,
    windows: Query<&Window>,
    primary: Query<Entity, With<PrimaryWindow>>,
    mut masks: ResMut<Masks>,
    mut images: ResMut<Assets<Image>>,
    mut stage: ResMut<Stage>,
) {
    let corners = corner
        .showing
        .filter(|showing| body_behind(showing.kind))
        .and_then(|showing| match showing.surface {
            Surface::Corner => inset.single().ok(),
            Surface::World => main.single().ok(),
        });
    let mut wanted: Vec<(Entity, UVec2, f32, isize)> = Vec::new();
    for viewer in corners.into_iter().chain(need.0) {
        if wanted.iter().any(|(have, ..)| *have == viewer) {
            continue;
        }
        let Ok((camera, target)) = cameras.get(viewer) else {
            continue;
        };
        let Some((size, scale)) =
            viewport_now(camera, target, &windows, primary.single().ok(), &images)
        else {
            continue;
        };
        wanted.push((viewer, size.max(UVec2::ONE), scale, camera.order));
    }
    // A set whose viewer no longer needs one, or needs one at another size or scale, goes.
    let mut index = 0;
    while index < masks.sets.len() {
        let owner = masks.sets[index].0;
        let kept = wanted.iter().any(|&(viewer, size, scale, _)| {
            (viewer, size, scale) == (owner.viewer, owner.size, owner.scale)
        });
        if kept {
            index += 1;
        } else {
            masks.release(index, &mut commands, &mut images);
            stage.invalidate();
        }
    }
    for (viewer, size, scale, order) in wanted {
        if masks.of(viewer).is_some() {
            continue;
        }
        stage.invalidate();
        let set = MaskSet {
            hull: mask_image(&mut images, size),
            part: mask_image(&mut images, size),
        };
        let mut set_cameras = [Entity::PLACEHOLDER; 2];
        for (index, (image, profile)) in [
            (set.hull.clone(), CameraProfile::PenCamHullMask),
            (set.part.clone(), CameraProfile::PenCamPartMask),
        ]
        .into_iter()
        .enumerate()
        {
            set_cameras[index] = commands
                .spawn((
                    Camera3d::default(),
                    Camera {
                        order: order - 2 + index as isize,
                        clear_color: ClearColorConfig::Custom(Color::NONE),
                        ..default()
                    },
                    RenderTarget::Image(ImageRenderTarget {
                        handle: image,
                        scale_factor: scale,
                    }),
                    Hdr,
                    Tonemapping::None,
                    DebandDither::Disabled,
                    Msaa::Off,
                    Projection::Perspective(default()),
                    Transform::IDENTITY,
                    profile,
                    MaskCamera { viewer },
                    crate::render_scale::NativeResolution,
                ))
                .id();
        }
        masks.sets.push((
            MaskOwner {
                viewer,
                size,
                scale,
                cameras: set_cameras,
            },
            set,
        ));
    }
}

/// The physical viewport size and scale factor `camera` renders at this frame, read from the
/// window or image `target` names as it stands now, as bevy's camera system will resolve them
/// later in the frame: a window's physical size and scale factor, an image's size and the
/// target's scale factor, a viewport clamped to the target. The camera's own cached values lag a
/// retarget or resize by a frame; they are read only for a texture-view or colourless target.
fn viewport_now(
    camera: &Camera,
    target: &RenderTarget,
    windows: &Query<&Window>,
    primary: Option<Entity>,
    images: &Assets<Image>,
) -> Option<(UVec2, f32)> {
    let (size, scale) = match target {
        RenderTarget::Window(window) => {
            let entity = match window {
                WindowRef::Primary => primary?,
                WindowRef::Entity(entity) => *entity,
            };
            let window = windows.get(entity).ok()?;
            (window.physical_size(), window.resolution.scale_factor())
        }
        RenderTarget::Image(image) => (images.get(&image.handle)?.size(), image.scale_factor),
        RenderTarget::TextureView(_) | RenderTarget::None { .. } => {
            return Some((
                camera.physical_viewport_size()?,
                camera.target_scaling_factor()?,
            ));
        }
    };
    let size = camera
        .viewport
        .as_ref()
        .map_or(size, |viewport| viewport.physical_size.min(size));
    Some((size, scale))
}

/// Give each mask camera its viewer's lens. Runs before bevy derives the cameras' projection
/// matrices ([`bevy::camera::CameraUpdateSystems`]), so a mask draws with the lens its viewer has
/// this frame; bevy then fits the aspect to the mask's own target, the viewer's size. Every
/// system that writes a viewer's [`Projection`] must run before this one, or the mask draws last
/// frame's lens: the edges are named where it is mounted ([`super::presentation`]).
pub fn follow_lens(
    viewers: Query<&Projection, Without<MaskCamera>>,
    mut followers: Query<(&MaskCamera, &mut Projection)>,
) {
    for (follower, mut lens) in &mut followers {
        if let Ok(projection) = viewers.get(follower.viewer) {
            *lens = projection.clone();
        }
    }
}

/// Pose each mask camera as its viewer, from the viewer's `GlobalTransform`. Runs after transform
/// propagation and before the frusta are taken. Every system that sets a viewer's final pose,
/// its `Transform` before propagation or its `GlobalTransform` after, must run before this one,
/// or the mask draws last frame's pose: the edges are named where it is mounted
/// ([`super::presentation`]).
pub fn follow_pose(
    viewers: Query<&GlobalTransform, Without<MaskCamera>>,
    mut followers: Query<(&MaskCamera, &mut Transform, &mut GlobalTransform)>,
) {
    for (follower, mut pose, mut global) in &mut followers {
        let Ok(viewer) = viewers.get(follower.viewer) else {
            continue;
        };
        if *global != *viewer {
            *global = *viewer;
            *pose = viewer.compute_transform();
        }
    }
}

/// One field (a spall cone or a burst's) where the volume is taken, in world space: its origin, its
/// picture's frame (the unit axis, `u` and `v` across it, [`super::picture::Picture`]), its
/// support's half-angle and inner edge (rad), its picture's pitch (rad a texel) and its faces'
/// lanes ([`super::picture::Picture::lanes`]), how far its front has run (m, negative before it
/// is thrown), the world's metres per record metre, its longest reach (record m), its strength
/// `S` (reference-mm) and its law ([`law_lane`]).
#[derive(Clone, Copy, Debug)]
pub struct FieldPrint {
    pub origin: Vec3,
    pub axis: Vec3,
    pub u: Vec3,
    pub v: Vec3,
    pub half_angle: f32,
    pub inner: f32,
    pub pitch: f32,
    pub faces: [Vec4; FACE_LANES],
    pub front: f32,
    pub scale: f32,
    pub longest: f32,
    pub strength: f32,
    pub law: Vec4,
}

/// The first [`FIELDS`] of `prints` in the fields' lanes, [`PRINT_LANES`] a field, and how many:
/// the origin (world) and half-angle; the unit axis (world) and the front (m); `u` and the inner
/// edge; `v` and the picture's pitch; the emission its strength `S` gives, `ρ S / Ω`
/// ([`Potency::emission`], `Ω` its support's solid angle) in world units (squared metres, whose
/// intensity falls as the square of a world distance), the world's metres per record metre (the
/// picture holds record metres) and the longest reach (world m); its law ([`law_lane`]), whose
/// weight along a direction ([`law`]) times that emission is the direction's; and its picture's
/// faces, each face's crop and place in the record's atlas
/// ([`super::picture::Picture::lanes`]).
fn pack(prints: &[FieldPrint], potency: &Potency) -> ([Vec4; PRINT_LANES * FIELDS], usize) {
    let mut lanes = [Vec4::ZERO; PRINT_LANES * FIELDS];
    for (print, lane) in prints.iter().zip(lanes.chunks_mut(PRINT_LANES)) {
        lane[0] = print.origin.extend(print.half_angle);
        lane[1] = print.axis.extend(print.front);
        lane[2] = print.u.extend(print.inner);
        lane[3] = print.v.extend(print.pitch);
        let emission = potency.emission(print.strength, print.inner, print.half_angle);
        lane[4] = Vec4::new(
            emission * print.scale * print.scale,
            print.scale,
            print.longest * print.scale,
            0.0,
        );
        lane[5] = print.law;
        lane[6..].copy_from_slice(&print.faces);
    }
    (lanes, prints.len().min(FIELDS))
}

/// Field `field`'s law as the look evaluates it ([`law`]), thrown in `strata` strata: the kind of
/// its terms (x: 1 the ball's cap, 2 the ball over the whole sphere, 3 the ring's band; 0 a
/// descriptor outside the law's domain, which lights nothing), their numbers (y, z: the cap's
/// `λ²` and depth `w`, the sphere's `λ`, the band's depths `t_lo` and `t_hi`;
/// `spall_cone::Terms`) and `N` (w).
pub fn law_lane(field: FieldDescriptor, strata: usize) -> Vec4 {
    let n = strata as f32;
    match field.terms() {
        Ok(Terms::Cap { l2, w }) => Vec4::new(1.0, l2 as f32, w as f32, n),
        Ok(Terms::Sphere { lambda }) => Vec4::new(2.0, lambda as f32, 0.0, n),
        Ok(Terms::Band { lo, hi }) => Vec4::new(3.0, lo as f32, hi as f32, n),
        Err(_) => Vec4::ZERO,
    }
}

/// The weight of the law `lane` ([`law_lane`]) along a direction at depth `t = 1 − cos θ` off its
/// axis, in f32 as `pen_cam_look.wgsl`'s `law` takes it: the law's weight averaged over the window
/// one stratum wide (`Δ = (t_hi − t_lo) / N`) centred on `t` and held to the support,
/// `(t_hi − t_lo)/2 · share(a, b)/(b − a)`, `share` the emission's share between depths `a` and
/// `b`. At stratum `k`'s centre the window is the stratum and the weight is the stratum's `f_k`
/// (`spall_cone::ConeShape::spread`), so a record direction's strength `S f_k` is the law's there;
/// between centres it runs continuously. The band's share is linear in depth, so its weight is ½
/// throughout. The support is the caller's to test: a depth off it reads its nearest edge's weight,
/// as a painted edge a pixel past the rim reads it. Zero for no strata and for no law.
pub fn law(lane: Vec4, t: f32) -> f32 {
    let kind = lane.x.round() as i32;
    let (lo, hi) = match kind {
        1 => (0.0, lane.z),
        2 => (0.0, 2.0),
        3 => (lane.y, lane.z),
        _ => return 0.0,
    };
    if lane.w < 1.0 {
        return 0.0;
    }
    if kind == 3 {
        return 0.5;
    }
    let t = t.clamp(lo, hi);
    let half = 0.5 * (hi - lo) / lane.w;
    let (a, b) = ((t - half).max(lo), (t + half).min(hi));
    if b <= a {
        return 0.0;
    }
    let share = if kind == 1 {
        cap_share(lane.y, b) - cap_share(lane.y, a)
    } else {
        sphere_share(lane.y, a, b)
    };
    0.5 * (hi - lo) * share / (b - a)
}

/// The ball's cap's share of the emission inside depth `t`, `λ²` `l2`: `1 − μ D³`,
/// `D² = 1 − λ² t(2 − t)`, held at zero past the rim (`spall_cone::Terms::stratum`).
fn cap_share(l2: f32, t: f32) -> f32 {
    let d = (1.0 - l2 * (t * (2.0 - t))).max(0.0).sqrt();
    1.0 - (1.0 - t) * (d * d * d)
}

/// The ball's share over the whole sphere between depths `a ≤ b`, ratio `lambda` below 1: the
/// density `g³/2` integrated, `(b − a)(g_a + g_b)²(g_a² + g_b² + 2c) / (16(D_a + D_b))`,
/// `c = 1 − λ²`, `D = √(c + λ²μ²)`, `g = λμ + D`, behind the plate `c / (D − λμ)`
/// (`spall_cone::Terms::stratum`).
fn sphere_share(lambda: f32, a: f32, b: f32) -> f32 {
    let c = (1.0 - lambda) * (1.0 + lambda);
    let reach = |t: f32| {
        let mu = 1.0 - t;
        let d = (c + lambda * mu * (lambda * mu)).sqrt();
        let g = if mu >= 0.0 {
            lambda * mu + d
        } else {
            c / (d - lambda * mu)
        };
        (g, d)
    };
    let ((g_a, d_a), (g_b, d_b)) = (reach(a), reach(b));
    let sum = g_a + g_b;
    (b - a) * (sum * sum) * (g_a * g_a + g_b * g_b + 2.0 * c) / (16.0 * (d_a + d_b))
}

/// The depth `t = 1 − cos θ` of unit `direction` off unit `axis`, as `½ |direction − axis|²`, which
/// keeps its digits near the axis where `1 − cos θ` cancels.
pub fn depth(direction: Vec3, axis: Vec3) -> f32 {
    0.5 * (direction - axis).length_squared()
}

/// The lanes the bands are drawn from: the floor and mid thresholds (HP points per m²), and the
/// knockout and reduced bands' colours (linear rgb; w their extinction length, set with the
/// fields, [`LookMaterial::set_fields`]).
fn band_lanes(potency: &Potency) -> [Vec4; 3] {
    let colour = |band: Band| {
        let c = LinearRgba::from(band.colour().unwrap_or(Color::NONE));
        Vec4::new(c.red, c.green, c.blue, 0.0)
    };
    [
        potency.lanes(),
        colour(Band::Knockout),
        colour(Band::Reduced),
    ]
}

/// The fields `prints` on the painted material `handle` ([`LookMaterial::is_painted`]), their
/// pictures read from `atlas` ([`LookMaterial::set_fields`]; `None`, no field). The material is written
/// only where that changes it, so a frame that holds still re-uploads none; one not painted is
/// left as it is.
pub fn paint_fields(
    looks: &mut Assets<LookMaterial>,
    handle: &Handle<LookMaterial>,
    prints: &[FieldPrint],
    atlas: Option<&Handle<Image>>,
) {
    let Some(material) = looks.get(handle).filter(|m| m.is_painted()) else {
        return;
    };
    let mut want = material.clone();
    want.pictures = atlas.cloned();
    want.set_fields(prints, &Potency::shipped());
    if (want.params != material.params || want.pictures != material.pictures)
        && let Some(mut material) = looks.get_mut(handle)
    {
        *material = want;
    }
}

/// The fields' summed intensity (HP points per m²) at world point `p`, as the volume samples it
/// (`pen_cam_look.wgsl`, `intensity` and `field_at`): each of `prints`, seen as `seen` alike,
/// adds `ρ S w / (Ω r²)` where `p` lies on its support and within its front, `S w` the strength
/// its law gives `p`'s direction ([`law`]), lit as its picture lights `p`
/// ([`super::picture::Picture::lit`]: each texel about `p`'s direction tested against its own
/// reach), nothing past its longest reach; at its origin, once thrown, it is unbounded (`1e30`),
/// unless the record ended every direction of it there (its longest reach zero): the record wins,
/// and it adds nothing.
/// Doses add, so the bands classify the sum, never a field's own. This is the sum the shader
/// mirrors, held by the tests.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the look's shader sums the fields at runtime (`pen_cam_look.wgsl`, `intensity`); this is the sum it mirrors, held by the tests"
    )
)]
pub fn intensity(prints: &[FieldPrint], seen: &[Seen], potency: &Potency, p: Vec3) -> f32 {
    let (lanes, count) = pack(prints, potency);
    let mut sum = 0.0;
    for (lane, seen) in lanes.chunks(PRINT_LANES).zip(seen).take(count) {
        let d = p - lane[0].truncate();
        let r = d.length();
        if r < 1e-5 {
            sum += if lane[1].w > 0.0 && lane[4].z > 0.0 {
                1e30
            } else {
                0.0
            };
            continue;
        }
        if r > lane[1].w.min(lane[4].z) {
            continue;
        }
        let dir = d / r;
        let c = dir.dot(lane[1].truncate()).clamp(-1.0, 1.0);
        if c > lane[2].w.cos() || c < lane[0].w.cos() {
            continue;
        }
        let local = Vec3::new(dir.dot(lane[2].truncate()), dir.dot(lane[3].truncate()), c);
        let weight = law(lane[5], depth(dir, lane[1].truncate()));
        let lit = seen.picture.lit(local, r, lane[4].y, 0.0, seen.reach);
        sum += lit * weight * lane[4].x / (r * r);
    }
    sum
}

/// The fields' summed intensity (HP points per m²) a component's surface is painted by at world
/// point `p`, its unit normal `n` (world), under `rule`, as `pen_cam_look.wgsl`'s `footprint`
/// sums it with every edge sharp: each of `prints` (seen as `seen`, as [`intensity`] reads them)
/// that covers `p` adds `ρ S w / (Ω r²)`, `S w` the strength its law gives `p`'s direction
/// ([`law`]), lit as its picture lights `p` within the rule's slack past each texel's reach
/// ([`super::picture::Picture::lit`]). A field covers `p` where `p` lies on its support, within
/// its front and within its longest reach and the rule's slack past it (a picture can reach past
/// the record's longest direction between its directions, and the volume holds nothing there),
/// and under [`Footprint::Hard`] where `n` faces its origin; under [`Footprint::Witness`] whatever
/// the facing, so a witness is painted exactly where the volume holds the field ([`intensity`]). The point takes the band of the sum ([`Potency::band`]). This
/// is the sum the shader mirrors, held by the tests.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the look's shader paints the fields at runtime (`pen_cam_look.wgsl`, `footprint`); this is the sum it mirrors, held by the tests"
    )
)]
pub fn painting(
    prints: &[FieldPrint],
    seen: &[Seen],
    potency: &Potency,
    rule: Footprint,
    p: Vec3,
    n: Vec3,
) -> f32 {
    let (lanes, count) = pack(prints, potency);
    let foot = rule.lane();
    let mut sum = 0.0;
    for (lane, seen) in lanes.chunks(PRINT_LANES).zip(seen).take(count) {
        let d = p - lane[0].truncate();
        let r = d.length().max(1e-5);
        let dir = d / r;
        let c = dir.dot(lane[1].truncate()).clamp(-1.0, 1.0);
        let theta = c.acos();
        let on_rim = lane[0].w >= PI || theta <= lane[0].w;
        let off_hole = lane[2].w <= 0.0 || theta >= lane[2].w;
        let facing = foot.z > 0.5 || n.dot(-dir) > 0.0;
        if r > lane[1].w.min(lane[4].z + foot.y) || !on_rim || !off_hole || !facing {
            continue;
        }
        let local = Vec3::new(dir.dot(lane[2].truncate()), dir.dot(lane[3].truncate()), c);
        let weight = law(lane[5], depth(dir, lane[1].truncate()));
        let lit = seen.picture.lit(local, r, lane[4].y, foot.y, seen.reach);
        sum += lit * weight * lane[4].x / (r * r);
    }
    sum
}

/// Field `print`'s spacing (m), as `pen_cam_look.wgsl`'s `spacing` takes it: its ball (its radius
/// the nearer of its front and its longest reach, as [`stretch`] bounds it) across in
/// [`VOLUME_STEPS`] steps. The march samples a field's stretch at least this finely wherever it
/// lies ([`schedule`]), so each field is drawn at its own size's resolution, whatever other field
/// it lies in.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the look's shader spaces its march at runtime (`pen_cam_look.wgsl`, `spacing`); this is the spacing it mirrors, held by the tests"
    )
)]
pub fn spacing(print: &FieldPrint) -> f32 {
    2.0 * print.front.min(print.longest * print.scale) / VOLUME_STEPS as f32
}

/// The march's pieces along the eye ray `eye + dir t` over fields `prints`, kept to `[near, far]`
/// (m along the ray), as `pen_cam_look.wgsl`'s `volume` takes them: each field's stretch
/// ([`stretch`]) clipped to it, every stretch's ends in order, and between each two adjacent ends
/// that a stretch holds the piece `(near, far, space)`, `space` the finest [`spacing`] of the
/// fields whose stretches hold it; far to near. Every field's ends are samples and its whole
/// stretch is sampled at its own spacing or finer, so a field never falls between two samples of
/// a wider one, and a piece takes at most [`VOLUME_STEPS`] steps: it lies inside the ball of every
/// field that spaces it. This is the schedule the shader mirrors, held by the tests.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the look's shader schedules its march at runtime (`pen_cam_look.wgsl`, `volume`); this is the schedule it mirrors, held by the tests"
    )
)]
pub fn schedule(
    prints: &[FieldPrint],
    eye: Vec3,
    dir: Vec3,
    near: f32,
    far: f32,
) -> Vec<(f32, f32, f32)> {
    let stretches: Vec<(f32, f32, f32)> = prints
        .iter()
        .take(FIELDS)
        .filter_map(|print| {
            let (lo, hi) = stretch(print, eye, dir)?;
            let (lo, hi) = (lo.max(near), hi.min(far));
            (lo < hi).then_some((lo, hi, spacing(print)))
        })
        .collect();
    let mut ends: Vec<f32> = stretches.iter().flat_map(|&(lo, hi, _)| [lo, hi]).collect();
    ends.sort_by(f32::total_cmp);
    ends.windows(2)
        .rev()
        .filter_map(|pair| {
            let (a, b) = (pair[0], pair[1]);
            let mid = 0.5 * (a + b);
            let space = stretches
                .iter()
                .filter(|&&(lo, hi, _)| lo <= mid && mid <= hi)
                .map(|&(.., space)| space)
                .reduce(f32::min)?;
            (a < b).then_some((a, b, space))
        })
        .collect()
}

/// The volume's march over one piece of an eye ray ([`schedule`]), from `far` back to `near` (m
/// along it), as `pen_cam_look.wgsl`'s `march` and `step_over` take it: equal steps no longer than
/// `space` ([`spacing`]), [`VOLUME_STEPS`] at most, the band (`potency`'s, [`Potency::band`]) of
/// `intensity` at each sample, and between two samples on either side of a threshold the
/// crossing placed by [`VOLUME_REFINE`] halvings. `visit` takes
/// each piece, far to near, as its length and band; the shader lays each band's pieces over what
/// lies behind them. A band's length along the ray is so its iso-surface's chord to `2⁻⁸` of a
/// step wherever the ray crosses it once between samples. This is the march the shader mirrors,
/// held by the tests.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the look's shader marches at runtime (`pen_cam_look.wgsl`, `march`); this is the march it mirrors, held by the tests"
    )
)]
pub fn march(
    near: f32,
    far: f32,
    space: f32,
    potency: &Potency,
    intensity: impl Fn(f32) -> f32,
    mut visit: impl FnMut(f32, Band),
) {
    let (floor, mid) = (potency.floor, potency.mid);
    let crossing = |far: f32, near: f32, threshold: f32, far_above: bool| {
        let (mut a, mut b) = (far, near);
        for _ in 0..VOLUME_REFINE {
            let half = 0.5 * (a + b);
            if (intensity(half) >= threshold) == far_above {
                a = half;
            } else {
                b = half;
            }
        }
        0.5 * (a + b)
    };
    let steps = ((far - near) / space.max(1e-6))
        .ceil()
        .clamp(1.0, VOLUME_STEPS as f32) as usize;
    let step = (far - near) / steps as f32;
    let (mut t_far, mut i_far) = (far, intensity(far));
    for k in 1..=steps {
        let t_near = far - k as f32 * step;
        let i_near = intensity(t_near);
        let (f_far, f_near) = (i_far >= floor, i_near >= floor);
        let (m_far, m_near) = (i_far >= mid, i_near >= mid);
        let c_f = if f_far != f_near {
            crossing(t_far, t_near, floor, f_far)
        } else {
            t_near
        };
        let c_m = if m_far != m_near {
            crossing(t_far, t_near, mid, m_far)
        } else {
            t_near
        };
        let points = [t_far, c_f.max(c_m), c_f.min(c_m), t_near];
        for piece in points.windows(2) {
            let (a, b) = (piece[0], piece[1]);
            if a <= b {
                continue;
            }
            let t = 0.5 * (a + b);
            let f_in = if f_far != f_near && t < c_f {
                f_near
            } else {
                f_far
            };
            let m_in = if m_far != m_near && t < c_m {
                m_near
            } else {
                m_far
            };
            let band = if m_in {
                Band::Knockout
            } else if f_in {
                Band::Reduced
            } else {
                Band::Nothing
            };
            visit(a - b, band);
        }
        (t_far, i_far) = (t_near, i_near);
    }
}

/// Field `print`'s stretch of the ray `eye + dir t` (`t ≥ 0`, `dir` unit), as `pen_cam_look.wgsl`'s
/// `stretch` finds it: inside its ball out to its front or its longest reach, the nearer, and for a
/// cap no wider than a hemisphere inside its cone too; `None` where the ray misses it. Every point
/// the field holds on the ray lies within it, so the march, which samples only these stretches,
/// misses none. This is the stretch the shader mirrors, held by the tests.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the look's shader bounds its march at runtime (`pen_cam_look.wgsl`, `stretch`); this is the bound it mirrors, held by the tests"
    )
)]
pub fn stretch(print: &FieldPrint, eye: Vec3, dir: Vec3) -> Option<(f32, f32)> {
    let radius = print.front.min(print.longest * print.scale);
    if radius <= 0.0 {
        return None;
    }
    let oc = eye - print.origin;
    let b = oc.dot(dir);
    let h = b * b - (oc.length_squared() - radius * radius);
    if h <= 0.0 {
        return None;
    }
    let (mut lo, mut hi) = (-b - h.sqrt(), -b + h.sqrt());
    if print.half_angle < std::f32::consts::FRAC_PI_2 && print.inner <= 0.0 {
        let cos2 = print.half_angle.cos().powi(2);
        let (dv, ov) = (dir.dot(print.axis), oc.dot(print.axis));
        // The cone's own nappe: `ov + t dv ≥ 0`.
        if dv.abs() > 1e-6 {
            let t0 = -ov / dv;
            if dv > 0.0 {
                lo = lo.max(t0);
            } else {
                hi = hi.min(t0);
            }
        } else if ov < 0.0 {
            return None;
        }
        // Inside the double cone: `qa t² + qb t + qc ≥ 0`.
        let qa = dv * dv - cos2;
        let qb = 2.0 * (dv * ov - cos2 * b);
        let qc = ov * ov - cos2 * oc.length_squared();
        let disc = qb * qb - 4.0 * qa * qc;
        if qa.abs() < 1e-7 {
            if qb.abs() > 1e-12 {
                let t1 = -qc / qb;
                if qb > 0.0 {
                    lo = lo.max(t1);
                } else {
                    hi = hi.min(t1);
                }
            } else if qc < 0.0 {
                return None;
            }
        } else if qa < 0.0 {
            // Inside between the roots alone.
            if disc <= 0.0 {
                return None;
            }
            let r = disc.sqrt();
            let (t1, t2) = ((-qb + r) / (2.0 * qa), (-qb - r) / (2.0 * qa));
            lo = lo.max(t1.min(t2));
            hi = hi.min(t1.max(t2));
        } else if disc > 0.0 {
            // Inside outside the roots: the ray runs steeper than the cone's side, so the cone's
            // own nappe holds the side of the roots the ray runs into it on.
            let r = disc.sqrt();
            let (t1, t2) = ((-qb - r) / (2.0 * qa), (-qb + r) / (2.0 * qa));
            if dv > 0.0 {
                lo = lo.max(t2);
            } else {
                hi = hi.min(t1);
            }
        }
    }
    let lo = lo.max(0.0);
    (lo < hi).then_some((lo, hi))
}

/// The box (record frame, low and high corners) about every field of `cones` out to its longest
/// reach, which the volume is drawn on; `None` where no field reaches.
pub fn field_bounds(cones: &[Cone]) -> Option<(Vec3, Vec3)> {
    cones
        .iter()
        .take(FIELDS)
        .map(|cone| (cone.origin, cone.max_reach()))
        .filter(|&(_, longest)| longest > 0.0)
        .map(|(origin, longest)| (origin - longest, origin + longest))
        .reduce(|(lo, hi), (a, b)| (lo.min(a), hi.max(b)))
}

/// How a tank mesh is drawn in the base look: its material and its layer.
pub struct HullDraw {
    pub material: LookMaterial,
    pub layer: Layer,
}

/// The side of the draw order a mesh is on, and so the mask its copy is drawn into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Layer {
    /// The body (main armour, external armour, detail): drawn behind the scene, into the hull
    /// mask.
    Body,
    /// A component or the round's model: ordered by depth among themselves and with the fields,
    /// into the part mask.
    Scene,
}

impl Layer {
    /// The layer of the round's model: the one place the round is placed in the draw order.
    pub const ROUND: Self = Self::Scene;

    /// The layer of a mesh of tier `tier`: the one place a tier is placed in the draw order.
    pub fn of(tier: Tier) -> Self {
        match tier {
            Tier::Component => Self::Scene,
            Tier::Armour | Tier::External | Tier::Detail => Self::Body,
        }
    }
}

/// The rule a component's surface takes the fields' painting by (`pen_cam_look.wgsl`,
/// `footprint`): its role's ([`Self::of`]), set in its material when the material is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Footprint {
    /// A hard component stops a field's directions on its surface: painted where a field meets
    /// it, a surface point facing the field's origin, on its support, behind its front and within
    /// each texel's reach along its direction, its tolerance
    /// ([`super::picture::REACH_TOLERANCE_DIAGONALS`]) and [`FOOTPRINT_REACH_SLACK_M`] past it.
    Hard,
    /// A crew station is a witness the field passes through (ADR-0040): painted wherever the
    /// volume holds the field, on any face, with no slack.
    Witness,
}

impl Footprint {
    /// The rule a component of role `role` takes: a crew station's the witness rule, every other
    /// component's the hard one.
    pub fn of(role: Role) -> Self {
        match role {
            Role::Crew => Self::Witness,
            Role::Module | Role::Ammo | Role::Engine => Self::Hard,
        }
    }

    /// The rule a tank mesh of tier `tier` takes: a component's mesh its component's role's,
    /// `role` (`None` where it belongs to no component the record names, and is not painted); a
    /// body mesh none.
    pub fn of_mesh(tier: Tier, role: Option<Role>) -> Option<Self> {
        match Layer::of(tier) {
            Layer::Scene => role.map(Self::of),
            Layer::Body => None,
        }
    }

    /// The `foot` lane (`pen_cam_look.wgsl`): the painting's opacity, how far (m) past each
    /// texel's reach a surface counts as met, and the witness rule (1) or the hard one (0).
    pub fn lane(self) -> Vec4 {
        match self {
            Self::Hard => Vec4::new(FOOTPRINT_FILL_ALPHA, FOOTPRINT_REACH_SLACK_M, 0.0, 0.0),
            Self::Witness => Vec4::new(FOOTPRINT_FILL_ALPHA, 0.0, 1.0, 0.0),
        }
    }
}

/// How a tank mesh of tier `tier` is drawn in the base look; a component's mesh, `role` its
/// component's, painted by its rule ([`Footprint::of_mesh`]).
pub fn hull(tier: Tier, role: Option<Role>, masks: &MaskSet) -> HullDraw {
    let layer = Layer::of(tier);
    let material = match layer {
        Layer::Body => base(masks),
        Layer::Scene => component_base(masks, Footprint::of_mesh(tier, role)),
    };
    HullDraw { material, layer }
}

/// The base look's one material for every body mesh: armour, running gear and detail. It is
/// discarded wherever the part mask holds a scene surface (`mode.z = 0`).
fn base(masks: &MaskSet) -> LookMaterial {
    let edge = LinearRgba::from(BASE_EDGE);
    let mut material = LookMaterial::new(
        MODE_BASE,
        BASE_GREY.into(),
        BASE_ALPHA,
        Blend::Blended,
        masks,
    );
    material.params.shade.x = BASE_FACE_SHARE;
    material.params.shade.y = BASE_CONTRAST;
    material.params.shade.z = BASE_FRESNEL_POWER;
    material.params.edge = Vec4::new(edge.red, edge.green, edge.blue, BASE_EDGE_ALPHA);
    material
}

/// The base look's material for every component mesh: the untouched grey, opaque, in the body's
/// edge colour, tested against the part mask (`mode.z = 1`), painted by `rule` (`None`: not
/// painted). A struck component's fill covers it.
fn component_base(masks: &MaskSet, rule: Option<Footprint>) -> LookMaterial {
    let mut material = base(masks);
    let grey = LinearRgba::from(timeline::STATE_UNTOUCHED);
    material.params.color = Vec4::new(grey.red, grey.green, grey.blue, BASE_COMPONENT_ALPHA);
    material.params.shade.x = BASE_COMPONENT_FACE_SHARE;
    material.params.mode.z = 1.0;
    material.blend = Blend::Opaque;
    if let Some(rule) = rule {
        material.paint(rule);
    }
    material
}

/// The masks' material: view normal and eye depth, drawn into either mask.
pub fn hull_mask(masks: &MaskSet) -> LookMaterial {
    LookMaterial::new(MODE_MASK, LinearRgba::BLACK, 1.0, Blend::Opaque, masks)
}

/// Component `b`'s place on the state scale, the share of its whole pool gone, once `rise` of its
/// fill has run: 0 before the picture reaches it, exactly 1 once [`timeline::BOX_FILL_S`] has
/// passed since. A component this shot took HP from rises from its state before the shot
/// (untouched, or red to [`BoxPlan::pool_lost_before`]) to its state after it
/// ([`BoxPlan::pool_lost`]); one whose HP pool the shot emptied rises red to the top and is out
/// of action once the rise completes. One the shot took no HP from holds its state throughout:
/// out of action from the start when lost before the shot. `None` while untouched.
pub fn condition(b: &BoxPlan, rise: f32) -> Option<Condition> {
    let after = Condition::of(b.lost, b.pool_lost());
    if !b.struck() || rise >= 1.0 {
        return after;
    }
    let before = b.pool_lost_before();
    let share = before + (b.pool_lost() - before) * rise;
    (share > 0.0).then_some(Condition::Damaged(share))
}

/// The state scale's opaque fill at `condition` ([`LookMaterial::set_condition`]). Its frame
/// ([`LookMaterial::set_fill_span`]) and its condition are set each frame; its fade multiplies
/// the level, and is whole wherever it is shown.
fn state(condition: Condition, masks: &MaskSet) -> LookMaterial {
    let mut m = LookMaterial::new(MODE_STATE, LinearRgba::BLACK, 1.0, Blend::Opaque, masks);
    m.bias = true;
    let rest = LinearRgba::from(timeline::STATE_UNTOUCHED);
    m.params.rest = Vec4::new(rest.red, rest.green, rest.blue, 1.0);
    m.params.shade.y = STATE_CONTRAST;
    m.params.span.w = STATE_LINE_PX;
    m.params.hatch.x = STATE_HATCH_PX;
    m.params.hatch.y = STATE_HATCH_SHARE;
    m.set_condition(condition);
    m
}

/// Component `b`'s fill in frame `kind`, in any look: the state scale ([`state`]) at its state
/// once the shot has dosed it, painted by its role's rule ([`Footprint::of`]), drawn under a
/// [`timeline::STATE_RED`] outline [`STATE_OUTLINE_PX`] wide; `None` where it is not drawn. The
/// shooter's frames (the glass frame and the outside view) draw a component this shot took HP
/// from, and no other, so no earlier damage reaches the shooter; the victim's x-ray draws every
/// component with any of its pool gone.
pub fn component(b: &BoxPlan, kind: CornerKind, masks: &MaskSet) -> Option<LookMaterial> {
    let settled = condition(b, 1.0)?;
    let drawn = match kind {
        CornerKind::Glass | CornerKind::Damaged => b.struck(),
        CornerKind::Victim => true,
        CornerKind::Slice | CornerKind::Ricochet => false,
    };
    drawn.then(|| {
        let mut fill = state(settled, masks);
        fill.paint(Footprint::of(b.role));
        fill
    })
}

/// The record's fields as one volume, drawn on the back faces of a box about every field's reach
/// ([`field_bounds`]), reading their pictures from `atlas`. From each fragment the eye ray's stretches
/// inside the fields ([`stretch`]), cut at every stretch's ends ([`schedule`]), are marched back
/// toward the eye ([`march`]), each sample the fields' summed intensity ([`intensity`]); each band
/// takes its colour at an opacity that climbs with its length along the ray over its extinction
/// length, the nearer laid over the farther, the whole at most [`VOLUME_OPACITY`]. The fields'
/// lanes are set each frame ([`LookMaterial::set_fields`]). Straight alpha, sorted after every
/// other translucent surface, drawn whatever stands in front and writing no depth
/// ([`LookKey::depth`]). In a frame that draws the body behind ([`body_behind`]) the march ends at
/// its pixel's nearest scene surface in `masks`' part mask (`volume.w = 1`), so a component or the
/// round in front of a field hides it; in the outside view it is drawn over the tank whole. `cut`
/// by the slice plane (unit normal toward the discarded side, a point on it) it stops at the plane.
pub fn volume(
    potency: &Potency,
    masks: &MaskSet,
    kind: CornerKind,
    cut: Option<(Vec3, Vec3)>,
    atlas: Handle<Image>,
) -> LookMaterial {
    let mut m = LookMaterial::new(MODE_VOLUME, LinearRgba::BLACK, 0.0, Blend::Blended, masks);
    let [bands, knockout, reduced] = band_lanes(potency);
    m.params.bands = bands;
    m.params.knockout = knockout;
    m.params.reduced = reduced;
    m.params.volume.x = VOLUME_OPACITY;
    m.params.volume.w = body_behind(kind) as u8 as f32;
    m.pictures = Some(atlas);
    m.set_cut(cut);
    m
}

#[cfg(test)]
mod tests {
    use bevy::camera::CameraProjection;

    use super::super::picture::tests::{AXIS, band, cap, fixture_field, plate_casters, sphere};
    use super::super::picture::{Casters, Picture, REACH_TOLERANCE_DIAGONALS};
    use super::super::potency::solid_angle;
    use super::*;
    use crate::fingerprint::{FNV_OFFSET_BASIS, fnv1a_64};

    /// A fixture picture's pitch (rad a texel): about a quarter degree. Fixture data.
    const PITCH: f32 = 4.0e-3;

    /// Every record's fields fit the look: none throws more than [`FIELDS`], its spall cones and
    /// its burst's field together, past which [`pack`] drops a field and the atlas holds no
    /// picture for it, so the volume does not draw it.
    #[test]
    fn every_record_fits_the_look() {
        for (name, report) in super::super::plan::tests::records() {
            let fields = report.fields().count();
            assert!(
                fields <= FIELDS,
                "{name} throws {fields} fields, past FIELDS = {FIELDS}"
            );
        }
    }

    /// A pen cam shader, read from the shipped assets.
    fn shader(file: &str) -> String {
        let path = crate::assets::asset_root().join("shaders").join(file);
        std::fs::read_to_string(path).unwrap()
    }

    /// `pen_cam_look.wgsl` declares the look's counts and lanes: the fields and each one's lanes,
    /// where a field's face lanes start and how many a face takes ([`Picture::lanes`]), the
    /// volume's march samples and halvings, the reach tolerance its pictures are read with
    /// ([`Picture::lit`]), the arrays' lengths, and [`LookParams`]' lanes in their order. A count
    /// that drifts reads the wrong field or face without a word, or marches a volume short of its
    /// far side; a tolerance that drifts reads the GPU's shadows apart from the mirror's; a lane
    /// added, dropped or moved on one side alone reads every lane after it from its neighbour's
    /// slot.
    #[test]
    fn the_shader_declares_the_lane_counts() {
        let shader = shader("pen_cam_look.wgsl");
        let faces = PRINT_LANES - FACE_LANES;
        assert_eq!(FACE_LANES, 2 * 6, "two lanes a face of the cube");
        for declared in [
            format!("const FIELDS: i32 = {FIELDS};"),
            format!("const PRINT_LANES: i32 = {PRINT_LANES};"),
            format!("const VOLUME_STEPS: i32 = {VOLUME_STEPS};"),
            format!("const VOLUME_REFINE: i32 = {VOLUME_REFINE};"),
            format!("const REACH_TOLERANCE_DIAGONALS: f32 = {REACH_TOLERANCE_DIAGONALS:?};"),
            format!("prints: array<vec4<f32>, {}>,", PRINT_LANES * FIELDS),
            format!("var lo = array<f32, {FIELDS}>("),
            format!("var hi = array<f32, {FIELDS}>("),
            format!("var ends = array<f32, {}>(", 2 * FIELDS),
            format!("let crop = look.prints[base + {faces} + 2 * on.face];"),
            format!(
                "let place = look.prints[base + {} + 2 * on.face].xy;",
                faces + 1
            ),
            "for (var f = 0; f < FIELDS; f = f + 1)".to_owned(),
        ] {
            assert!(shader.contains(&declared), "{declared}");
        }
        let block = shader
            .split("struct LookParams {")
            .nth(1)
            .and_then(|rest| rest.split("\n}").next())
            .expect("the shader declares LookParams");
        let wgsl: Vec<&str> = block
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with("//"))
            .filter_map(|line| line.split_once(':').map(|(name, _)| name.trim()))
            .collect();
        let debug = format!("{:?}", LookParams::default());
        let rust: Vec<&str> = debug
            .match_indices(": ")
            .map(|(at, _)| {
                let head = &debug[..at];
                let start = head
                    .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .map_or(0, |i| i + 1);
                &head[start..]
            })
            .collect();
        assert_eq!(wgsl, rust);
    }

    /// Fixture: a field's print at twice the record's size, its frame off the world's axes.
    fn fixture_print() -> FieldPrint {
        FieldPrint {
            origin: Vec3::ONE,
            axis: Vec3::NEG_Z,
            u: Vec3::Y,
            v: Vec3::X,
            half_angle: 25.0_f32.to_radians(),
            inner: 0.0,
            pitch: 4.0e-3,
            faces: std::array::from_fn(|k| Vec4::new(-0.5, 0.5, 40.0 + k as f32, 30.0)),
            front: 3.0,
            scale: 2.0,
            longest: 1.5,
            strength: 3.0,
            law: law_lane(FieldDescriptor::ball(1.0 / 25.0_f32.to_radians().sin()), 64),
        }
    }

    /// The colour reads no gain: the lanes the volume colours from carry the field's frame, the
    /// emission `ρ S / Ω` its strength gives in world units (squared metres, whose intensity falls
    /// as the square of a world distance), its scale and longest reach, its law, the shipped
    /// thresholds and
    /// each band's colour from the band law, and fading the volume, a base surface or a painted
    /// component moves its fade lane alone; the volume's extinction lengths scale with the stage. A
    /// painted component reads the same lanes, and no field without its pictures. The colour promise
    /// relies on it: a display gain, level or fade in a colouring lane, or a painting packed apart
    /// from the volume, fails here.
    #[test]
    fn the_colour_reads_no_gain() {
        let potency = Potency::shipped();
        let masks = MaskSet::default();
        let atlas = Handle::<Image>::default();
        let print = fixture_print();
        let mut field = volume(&potency, &masks, CornerKind::Glass, None, atlas.clone());
        assert_eq!(field.pictures.as_ref(), Some(&atlas));
        field.set_fields(std::slice::from_ref(&print), &potency);
        let params = field.params;
        assert_eq!(params.fields.x, 1.0);
        let lane = &params.prints[..PRINT_LANES];
        assert_eq!(lane[0], print.origin.extend(print.half_angle));
        assert_eq!(lane[1], print.axis.extend(print.front));
        assert_eq!(lane[2], print.u.extend(print.inner));
        assert_eq!(lane[3], print.v.extend(print.pitch));
        assert_eq!(
            lane[4].x,
            4.0 * potency.emission(print.strength, print.inner, print.half_angle)
        );
        assert_eq!(
            lane[4].x,
            4.0 * (potency.rate * print.strength / solid_angle(print.half_angle))
        );
        assert_eq!(lane[4].y, print.scale);
        assert_eq!(lane[4].z, print.longest * print.scale);
        assert_eq!(lane[5], print.law);
        assert_eq!(lane[6..], print.faces);
        assert!(
            params.prints[PRINT_LANES..]
                .iter()
                .all(|l| *l == Vec4::ZERO)
        );
        assert_eq!(
            params.bands,
            Vec4::new(potency.floor, potency.mid, 0.0, 0.0)
        );
        let rgb = |colour: Color| LinearRgba::from(colour).to_vec3();
        assert_eq!(params.knockout.truncate(), rgb(timeline::FIELD));
        assert_eq!(params.reduced.truncate(), rgb(timeline::FIELD_DIM));
        assert_eq!(params.knockout.w, KNOCKOUT_EXTINCTION_M * print.scale);
        assert_eq!(params.reduced.w, REDUCED_EXTINCTION_M * print.scale);
        assert_eq!(params.volume.x, VOLUME_OPACITY);

        // A painted component draws the fields' bands from the volume's lanes, and no field
        // without its pictures.
        let mut painted = component_base(&masks, Some(Footprint::Hard));
        painted.set_fields(std::slice::from_ref(&print), &potency);
        assert_eq!(painted.params.fields.x, 0.0);
        painted.pictures = Some(atlas);
        painted.set_fields(std::slice::from_ref(&print), &potency);
        assert_eq!(painted.params.fields.x, 1.0);
        assert_eq!(painted.params.prints, params.prints);
        assert_eq!(painted.params.bands, params.bands);
        assert_eq!(
            painted.params.knockout.truncate(),
            params.knockout.truncate()
        );
        assert_eq!(painted.params.reduced.truncate(), params.reduced.truncate());
        assert_eq!(painted.params.foot.x, FOOTPRINT_FILL_ALPHA);

        for material in [&base(&masks), &field, &painted] {
            let mut faded = material.clone();
            faded.set_fade(0.25);
            let mut want = material.params;
            want.shade.w = 0.25;
            assert_eq!(format!("{:?}", faded.params), format!("{want:?}"));
        }
    }

    /// The look draws the body behind the scene: a body surface is discarded wherever the part
    /// mask holds a scene surface; each layer is one surface deep, a surface beneath its mask's
    /// nearest discarded; a volume's pipeline tests no depth and writes none, while every other
    /// element keeps the pass's own; and in every frame that draws the body behind, the volume's
    /// march reads its pixel's bound from the part mask, in the eye depth the masks store and the
    /// march and the nearest-surface test read. In the outside view, whose components lie hidden in
    /// the tank, the volume is drawn whole. The draw order relies on it: a body that tests no part
    /// mask washes the components in grey, a surface drawn beneath its layer's nearest stacks the
    /// glass, a volume tested against the depth drops the stretch of field in front of a component
    /// and vanishes behind the slice's body, one without the bound draws over a component in front
    /// of it, and a depth read in two conventions bounds an orthographic view's field at the wrong
    /// place. The pipeline keys and lanes are checked here; the shaders' discards, layer test,
    /// bound and eye depth are pinned ([`the_shaders_are_the_ones_the_frames_were_captured_with`])
    /// and the eye depth mirrored ([`the_eye_depth_runs_forward_in_both_projections`]).
    #[test]
    fn the_body_draws_behind_the_scene() {
        let potency = Potency::shipped();
        let masks = MaskSet::default();
        for kind in [
            CornerKind::Glass,
            CornerKind::Victim,
            CornerKind::Slice,
            CornerKind::Damaged,
        ] {
            let m = volume(&potency, &masks, kind, None, Handle::default());
            assert_eq!(
                LookKey::from(&m).depth(),
                Some((CompareFunction::Always, false)),
                "{kind:?}"
            );
            let bounded = if body_behind(kind) { 1.0 } else { 0.0 };
            assert_eq!(m.params.volume.w, bounded, "{kind:?}");
        }
        assert!(body_behind(CornerKind::Glass) && body_behind(CornerKind::Slice));
        assert!(!body_behind(CornerKind::Damaged) && !body_behind(CornerKind::Ricochet));
        for m in [
            base(&masks),
            component_base(&masks, None),
            component_base(&masks, Some(Footprint::Hard)),
            hull_mask(&masks),
            state(Condition::Dead, &masks),
        ] {
            assert_eq!(LookKey::from(&m).depth(), None);
        }
        assert_eq!(base(&masks).params.mode.z, 0.0);
        assert_eq!(component_base(&masks, None).params.mode.z, 1.0);
    }

    /// The print of `cone`, its picture `picture`, from `origin` with its front at `front` (world
    /// m), `scale` world metres per record metre.
    fn print_of(
        cone: &Cone,
        picture: &Picture,
        origin: Vec3,
        front: f32,
        scale: f32,
    ) -> FieldPrint {
        FieldPrint {
            origin,
            axis: picture.axis,
            u: picture.u,
            v: picture.v,
            half_angle: picture.half_angle,
            inner: picture.inner,
            pitch: picture.pitch,
            faces: picture.lanes(),
            front,
            scale,
            longest: picture.longest,
            strength: cone.strength,
            law: law_lane(cone.field, cone.strata),
        }
    }

    /// The march lays each band along an eye ray over the stretch its iso-surface cuts from the
    /// ray. Fixture: one field over the whole sphere, every direction reaching 2 m at 5 ref-mm, so
    /// its intensity falls as `1/r²` out to its reach, and a fixture potency whose mid and floor it
    /// crosses at 1 m and 1.6 m: the knockout band is the ball of radius 1 m, the reduced band the
    /// shell out to 1.6 m, nothing the shell out to the reach, and a ray `d` off the origin crosses
    /// each over its chord `2√(r² − d²)`, at the record's size and, every length doubled, at twice
    /// it. The march is bounded by the field's stretch ([`stretch`], [`schedule`]). A band's
    /// opacity climbs with its length, so the volume shows the dose law only where these lengths
    /// are the chords: a march whose crossings are not halved (`VOLUME_REFINE = 0`) is off by up to
    /// half a step, an intensity that does not fall as the square of a world distance moves the
    /// stage's iso-radii, a stretch short of the reach drops the shell's far end, and samples
    /// spaced wider than [`spacing`] (one step a stretch) step over the knockout ball whole. The
    /// rays cross the ball from its centre to near its rim, so their stretches take from all
    /// [`VOLUME_STEPS`] steps down to a few. Tolerance: each band edge to `2⁻⁹` of the spacing, a
    /// step's bound (DERIVED from [`VOLUME_REFINE`]), two edges to a band and the shell two bands,
    /// and f32 rounding at the reach.
    #[test]
    fn a_band_runs_its_iso_surfaces_chord() {
        let cone = sphere(|_, _| 2.0);
        let picture = Picture::of(&cone, PITCH);
        let seen = [Seen {
            picture: &picture,
            reach: &|_| 2.0,
        }];
        let weight = law(
            law_lane(cone.field, cone.strata),
            depth(Vec3::X, picture.axis),
        );
        let strength = cone.strength * weight;
        assert!((strength - 5.0).abs() < 1e-4 && (picture.longest - 2.0).abs() < 1e-4);
        let (r_mid, r_floor, reach) = (1.0_f32, 1.6_f32, 2.0_f32);
        let rate = 1.0;
        let k = rate * strength / solid_angle(picture.half_angle);
        let potency = Potency {
            rate,
            floor: k / (r_floor * r_floor),
            mid: k / (r_mid * r_mid),
        };
        let dir = Vec3::new(1.0, 0.2, -0.4).normalize();
        let across = dir.any_orthonormal_vector();
        for scale in [1.0, 2.0] {
            let print = print_of(
                &cone,
                &picture,
                Vec3::new(0.5, -1.0, 2.0),
                10.0 * scale,
                scale,
            );
            let prints = std::slice::from_ref(&print);
            let space = spacing(&print);
            assert_eq!(space, 2.0 * picture.longest * scale / VOLUME_STEPS as f32);
            for d in [0.0, 0.35, 0.9, 1.3, 1.75] {
                let eye = print.origin - dir * 6.0 * scale + across * d * scale;
                let (near, far) = stretch(&print, eye, dir).expect("the ray crosses the field");
                assert_eq!(
                    schedule(prints, eye, dir, 0.0, f32::MAX),
                    [(near, far, space)]
                );
                let mut lengths = [0.0_f32; 3];
                march(
                    near,
                    far,
                    space,
                    &potency,
                    |t| intensity(prints, &seen, &potency, eye + dir * t),
                    |length, band| lengths[band as usize] += length,
                );
                let chord = |r: f32| 2.0 * (r * r - d * d).max(0.0).sqrt() * scale;
                let edge = space / 512.0 + 1e-4 * scale;
                let want = [
                    chord(reach) - chord(r_floor),
                    chord(r_floor) - chord(r_mid),
                    chord(r_mid),
                ];
                for (band, (got, want)) in lengths.iter().zip(want).enumerate() {
                    let tolerance = if band == Band::Knockout as usize {
                        2.0 * edge
                    } else {
                        4.0 * edge
                    };
                    assert!(
                        (got - want).abs() <= tolerance,
                        "scale {scale}, d {d}: band {band} runs {got} m, its chord {want} m"
                    );
                }
            }
        }
    }

    /// A field is drawn wherever it lies inside another: the march samples each field's stretch at
    /// the field's own spacing ([`schedule`]), so a small field whose stretch lies between two
    /// samples of a wide one runs its knockout chord along the ray wherever it stands and however
    /// far its front has run; it never pops in as a sample happens to fall on it, as the camera or
    /// its front moves. One spacing across a merged stretch, the widest field's, steps over it
    /// whole. Fixture: two fields over the whole sphere of one strength, a wide one reaching 4 m
    /// and a small one reaching 1 m, its front at 2, 3 and 4 cm, so its ball lies inside the wide
    /// one's; a fixture potency whose mid lies 10 cm from either's origin and whose floor 1 m from
    /// it; an eye ray 1.5 m off the wide field's origin, where it alone draws nothing, through the
    /// small field's origin, set at 16 places within the middle of one of the wide field's own
    /// steps, its ball clear of both ends. The small field's ball lies wholly in knockout, so its
    /// knockout chord is its ball's diameter and nothing else is drawn. Tolerance: each of the
    /// ball's two edges to `2⁻⁹` of the wide field's step (a sample at the ball's end that reads it
    /// inside leaves its edge to the wide piece's halvings) and 0.1 mm for its stretch's ends,
    /// rounded at 6 m from the eye (DERIVED: f32 at 36 m² over a 4 cm radius).
    #[test]
    fn a_small_field_inside_a_wide_one_is_drawn() {
        let (wide_cone, small_cone) = (sphere(|_, _| 4.0), sphere(|_, _| 1.0));
        let (wide_picture, small_picture) = (
            Picture::of(&wide_cone, PITCH),
            Picture::of(&small_cone, PITCH),
        );
        let strength = wide_cone.strength * law(law_lane(wide_cone.field, wide_cone.strata), 1.0);
        let k = strength / solid_angle(std::f32::consts::PI);
        let potency = Potency {
            rate: 1.0,
            floor: k,
            mid: k / (0.1 * 0.1),
        };
        let seen = [
            Seen {
                picture: &wide_picture,
                reach: &|_| 4.0,
            },
            Seen {
                picture: &small_picture,
                reach: &|_| 1.0,
            },
        ];
        let dir = Vec3::new(1.0, 0.2, -0.4).normalize();
        let wide = print_of(
            &wide_cone,
            &wide_picture,
            Vec3::new(0.5, -1.0, 2.0),
            10.0,
            1.0,
        );
        let eye = wide.origin - dir * 6.0 + dir.any_orthonormal_vector() * 1.5;
        let (near, far) = stretch(&wide, eye, dir).expect("the ray crosses the wide field");
        let steps = ((far - near) / spacing(&wide))
            .ceil()
            .min(VOLUME_STEPS as f32);
        let step = (far - near) / steps;
        let middle = far - (0.5 * steps).floor() * step - 0.5 * step;
        let tolerance = 2.0 * step / 512.0 + 2.0 * 1e-4;
        let mut checked = 0;
        for front in [0.02, 0.03, 0.04] {
            let room = 0.5 * step - front;
            assert!(room > 0.02, "the small ball spans a wide step: {step} m");
            for n in 0..16 {
                let at = middle + room * (n as f32 / 7.5 - 1.0);
                let small = print_of(&small_cone, &small_picture, eye + dir * at, front, 1.0);
                let prints = [wide, small];
                let mut lengths = [0.0_f32; 3];
                for (near, far, space) in schedule(&prints, eye, dir, 0.0, f32::MAX) {
                    march(
                        near,
                        far,
                        space,
                        &potency,
                        |t| intensity(&prints, &seen, &potency, eye + dir * t),
                        |length, band| lengths[band as usize] += length,
                    );
                }
                let chord = 2.0 * front;
                let error = (lengths[Band::Knockout as usize] - chord).abs();
                assert!(
                    error <= tolerance && lengths[Band::Reduced as usize] <= tolerance,
                    "front {front}, at {at}: bands run {lengths:?} m, the small ball's chord {chord} m"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 48);
    }

    /// Fields' doses add: where two fields overlap, the band is their summed intensity's, never
    /// either field's own nor the larger of the two ([`intensity`], `pen_cam_look.wgsl`'s
    /// `intensity`). The merged volume relies on it. Fixture: two 25° caps of one strength from
    /// origins 0.2 m apart, their axes parallel, read at a point 1 m along the axis between them,
    /// where each lies 5.7° off its own axis; a fixture potency whose mid lies 1.6 times above
    /// either field's intensity there and whose floor at half it.
    #[test]
    fn doses_add_where_fields_overlap() {
        let cone = cap(|_, _| 3.0);
        let picture = Picture::of(&cone, PITCH);
        let one_seen = Seen {
            picture: &picture,
            reach: &|_| 3.0,
        };
        let seen = [one_seen, one_seen];
        let point = picture.axis;
        let prints = [
            print_of(&cone, &picture, picture.u * 0.1, 5.0, 1.0),
            print_of(&cone, &picture, -picture.u * 0.1, 5.0, 1.0),
        ];
        let mut potency = Potency {
            rate: 1.0,
            floor: 0.0,
            mid: 0.0,
        };
        let one = intensity(&prints[..1], &seen, &potency, point);
        let other = intensity(&prints[1..], &seen[1..], &potency, point);
        assert!(one > 0.0 && (one - other).abs() <= 1e-4 * one);
        potency.floor = 0.5 * one;
        potency.mid = 1.6 * one;
        let both = intensity(&prints, &seen, &potency, point);
        assert!((both - (one + other)).abs() <= 1e-5 * both);
        assert_eq!(potency.band(one), Band::Reduced);
        assert_eq!(potency.band(other), Band::Reduced);
        assert_eq!(potency.band(both), Band::Knockout);
    }

    /// A struck component of role `role`: half its pool taken by the line. Fixture data.
    fn struck_box(role: Role) -> BoxPlan {
        BoxPlan {
            name: "fixture".to_owned(),
            role,
            center: None,
            heights: None,
            count: 10,
            hp: 10,
            line: 1.0,
            field: 0.0,
            lost_hp: 5,
            reached_at: Some(super::super::plan::Reach::Line(0.0)),
            lost: false,
        }
    }

    /// A crew station is painted by the witness rule, and nothing else is: crew takes the witness
    /// rule and every other role the hard one ([`Footprint::of`]), and the rule is the lane of
    /// every material its component is drawn in, its mesh's base and its state fill in each frame
    /// that draws one; a body mesh is never painted, whatever role is named for it, nor a
    /// component's mesh that no component of the record claims. ADR-0040 relies on it: crew under
    /// the hard rule leaves the faces of a station a field passes through bare where they turn from
    /// its origin, and a hard component under the witness rule is painted through its own body.
    #[test]
    fn a_crew_station_is_painted_as_a_witness_and_nothing_else_is() {
        let masks = MaskSet::default();
        for role in [Role::Crew, Role::Module, Role::Ammo, Role::Engine] {
            // A role added to `Role` fails to compile here until it is listed above.
            match role {
                Role::Crew | Role::Module | Role::Ammo | Role::Engine => {}
            }
            let rule = Footprint::of(role);
            assert_eq!(rule == Footprint::Witness, role == Role::Crew, "{role:?}");
            let base = hull(Tier::Component, Some(role), &masks).material;
            assert_eq!(base.params.foot, rule.lane(), "{role:?}");
            for tier in [Tier::Armour, Tier::External, Tier::Detail] {
                assert_eq!(Footprint::of_mesh(tier, Some(role)), None, "{tier:?}");
                let body = hull(tier, Some(role), &masks).material;
                assert!(!body.is_painted(), "{role:?} {tier:?}");
            }
            let b = struck_box(role);
            for kind in [CornerKind::Glass, CornerKind::Victim, CornerKind::Damaged] {
                let fill = component(&b, kind, &masks).expect("a struck component is drawn");
                assert_eq!(fill.params.foot, rule.lane(), "{role:?} {kind:?}");
            }
        }
        assert!(!hull(Tier::Component, None, &masks).material.is_painted());
        assert_eq!(Footprint::Witness.lane().z, 1.0);
        assert_eq!(Footprint::Hard.lane().z, 0.0);
    }

    /// A crew station is painted on any face inside the field: a point the volume holds, on a face
    /// turned from the field's origin, is painted by the witness rule at the volume's own
    /// intensity there ([`intensity`]), and across a sweep through the field and past its reach
    /// the witness rule paints exactly what the volume holds, on faces turned every way; the hard
    /// rule leaves the turned face bare and paints a face toward the origin up to its slack past
    /// the reach, where the witness rule paints nothing. A facing test on crew leaves the back of a
    /// station the field passes through unpainted, and a slack on crew paints past the field;
    /// both fail here. Fixture: a 25° cap, every direction reaching 2 m, its front past the reach.
    #[test]
    fn a_crew_station_is_painted_on_any_face_inside_the_field() {
        let cone = cap(|_, _| 2.0);
        let picture = Picture::of(&cone, PITCH);
        let seen = [Seen {
            picture: &picture,
            reach: &|_| 2.0,
        }];
        let print = print_of(&cone, &picture, Vec3::ZERO, 5.0, 1.0);
        let prints = std::slice::from_ref(&print);
        let potency = Potency::shipped();
        let paint = |rule, p, n| painting(prints, &seen, &potency, rule, p, n);
        let p = picture.axis;
        let held = intensity(prints, &seen, &potency, p);
        assert!(held > 0.0);
        assert_eq!(paint(Footprint::Witness, p, picture.axis), held);
        assert_eq!(paint(Footprint::Hard, p, picture.axis), 0.0);
        assert_eq!(paint(Footprint::Hard, p, -picture.axis), held);
        let past = picture.axis * 2.005;
        assert_eq!(intensity(prints, &seen, &potency, past), 0.0);
        assert_eq!(paint(Footprint::Witness, past, picture.axis), 0.0);
        assert_eq!(paint(Footprint::Witness, past, -picture.axis), 0.0);
        assert!(paint(Footprint::Hard, past, -picture.axis) > 0.0);
        let normals = [
            picture.axis,
            -picture.axis,
            picture.u,
            -picture.u,
            picture.v,
        ];
        let (mut inside, mut outside) = (0, 0);
        for k in 0..60 {
            for j in 0..41 {
                let p =
                    picture.axis * (0.05 + 0.05 * k as f32) + picture.u * (0.05 * j as f32 - 1.0);
                let held = intensity(prints, &seen, &potency, p);
                for n in normals {
                    let got = paint(Footprint::Witness, p, n);
                    assert!(
                        (got - held).abs() <= 1e-6 * held,
                        "{p} {n}: {got} of {held}"
                    );
                }
                if held > 0.0 {
                    inside += 1
                } else {
                    outside += 1
                }
            }
        }
        assert!(inside > 300 && outside > 300, "{inside} {outside}");
    }

    /// A 25° cap from the origin down its axis, its directions ending on a wall 3 m out and on a
    /// plate 1 m out over the cap's `+u` half, `thickness` (m) thick ([`plate_casters`]): its
    /// picture, its print, the front past the wall, and the casters its texels' reaches are taken
    /// from. Fixture data.
    fn plate_field(thickness: f32) -> (Picture, FieldPrint, Casters) {
        let casters = plate_casters(thickness);
        let cone = cap(|_, d| casters.reach(Vec3::ZERO, AXIS, d));
        let picture = Picture::of(&cone, PITCH);
        let print = print_of(&cone, &picture, Vec3::ZERO, 10.0, 1.0);
        (picture, print, casters)
    }

    /// A hard component is painted only where a field meets it: on a plate 3 cm thick that stops
    /// the field, the face toward the origin is painted at every direction that meets it, and the
    /// face behind, past where each of those directions stopped, is bare at every one, those
    /// within a texel of the plate's edge included, where the open texels beside them light the
    /// volume there and the witness rule paints it. A hard component under the witness rule shows
    /// the field on its own far side along its silhouette, and fails here. Fixture:
    /// [`plate_field`]; directions 8°, 14° and 20° off the axis, swept a degree either side of the
    /// plate's edge in steps of a hundredth.
    #[test]
    fn a_hard_face_behind_where_the_field_stopped_is_bare() {
        let thickness = 0.03;
        let (picture, print, casters) = plate_field(thickness);
        let seen = [Seen {
            picture: &picture,
            reach: &|d| casters.reach(Vec3::ZERO, AXIS, d),
        }];
        let prints = std::slice::from_ref(&print);
        let potency = Potency::shipped();
        let paint = |rule, p, n| painting(prints, &seen, &potency, rule, p, n);
        // The plate's faces: toward the origin, and away from it behind.
        let (toward, away) = (-picture.axis, picture.axis);
        let (mut met, mut beside) = (0, 0);
        for theta in [8.0_f32, 14.0, 20.0] {
            let theta = theta.to_radians();
            for k in 0..=200 {
                // Off the edge toward the plate (`+u`) by `off` across the sky.
                let off = (k as f32 - 100.0) * 0.01_f32.to_radians();
                let phi = std::f32::consts::FRAC_PI_2 - off / theta.sin();
                let d = picture.axis * theta.cos()
                    + (picture.u * phi.cos() + picture.v * phi.sin()) * theta.sin();
                let depth = d.dot(picture.axis);
                if d.dot(picture.u) <= 0.0 {
                    continue;
                }
                let front = d * (1.0 / depth);
                let back = d * ((1.0 + thickness) / depth);
                assert!(paint(Footprint::Hard, front, toward) > 0.0, "{d}");
                assert_eq!(paint(Footprint::Hard, back, away), 0.0, "{d}");
                beside += usize::from(paint(Footprint::Witness, back, away) > 0.0);
                met += 1;
            }
        }
        assert!(
            met > 250 && beside > 20,
            "{met} met, {beside} beside the edge"
        );
    }

    /// The painting ends at the field's longest recorded reach, as the volume does, a hard surface
    /// counting as met within the rule's slack past it: where the picture reaches past the
    /// record's longest direction (as it can between the directions), a surface facing the origin
    /// beyond the longest reach is bare under either rule and the volume holds nothing there,
    /// while one within the slack of it is painted under the hard rule. A painting capped at the
    /// front alone draws the field on surfaces the volume leaves empty. Fixture: a 25° cap whose
    /// every direction reached 1 m, its picture reaching 2 m, its front at 5 m; a direction 10°
    /// off the axis.
    #[test]
    fn the_painting_ends_at_the_longest_reach_the_record_gives() {
        let cone = cap(|_, _| 1.0);
        let picture = Picture::of(&cone, PITCH);
        let print = print_of(&cone, &picture, Vec3::ZERO, 5.0, 1.0);
        let seen = [Seen {
            picture: &picture,
            reach: &|_| 2.0,
        }];
        let prints = std::slice::from_ref(&print);
        let potency = Potency::shipped();
        let d =
            picture.axis * 10.0_f32.to_radians().cos() + picture.u * 10.0_f32.to_radians().sin();
        let paint = |rule, r: f32| painting(prints, &seen, &potency, rule, d * r, -d);
        let slack = Footprint::Hard.lane().y;
        assert!(
            slack > 0.0 && slack < 0.25,
            "the fixture reads within the slack: {slack}"
        );
        for r in [1.25, 1.5, 1.9] {
            assert_eq!(paint(Footprint::Hard, r), 0.0, "{r} m, hard");
            assert_eq!(paint(Footprint::Witness, r), 0.0, "{r} m, witness");
            assert_eq!(
                intensity(prints, &seen, &potency, d * r),
                0.0,
                "{r} m, volume"
            );
        }
        assert!(
            paint(Footprint::Hard, 1.0 + 0.5 * slack) > 0.0,
            "within the slack"
        );
        for rule in [Footprint::Hard, Footprint::Witness] {
            assert!(
                paint(rule, 0.9) > 0.0,
                "{rule:?} short of the longest reach"
            );
        }
    }

    /// A crew station behind a hard body is not painted: the field stops on the plate, so a
    /// station half a metre behind it, deep in its shadow, is bare on every face, while one in
    /// the open beside it at the same depth is painted. The witness rule paints what the volume
    /// holds, and the volume holds nothing past a direction's reach; a witness painted through a
    /// shadow fails here. Fixture: [`plate_field`], the plate 10 cm thick; directions 6° to 18°
    /// off the axis toward `+u` and toward `−u`.
    #[test]
    fn a_crew_station_behind_a_hard_body_is_bare() {
        let (picture, print, casters) = plate_field(0.1);
        let seen = [Seen {
            picture: &picture,
            reach: &|d| casters.reach(Vec3::ZERO, AXIS, d),
        }];
        let prints = std::slice::from_ref(&print);
        let potency = Potency::shipped();
        let paint = |p, n| painting(prints, &seen, &potency, Footprint::Witness, p, n);
        let normals = [
            picture.axis,
            -picture.axis,
            picture.u,
            -picture.u,
            picture.v,
        ];
        for k in 0..13 {
            let theta = (6.0 + k as f32).to_radians();
            for (side, shadowed) in [(picture.u, true), (-picture.u, false)] {
                let d = picture.axis * theta.cos() + side * theta.sin();
                let p = d * (1.5 / d.dot(picture.axis));
                for n in normals {
                    let got = paint(p, n);
                    assert_eq!(got == 0.0, shadowed, "{theta} {side} {n}: {got}");
                }
            }
        }
    }

    /// The record wins: a field whose recorded directions all ended at its origin is drawn
    /// nowhere. Its picture holds no face, so neither the volume nor the painting lights a point
    /// about its origin under either rule, the origin included, whatever the casters reach there
    /// ([`Casters::reach`], the pictures' CPU mirror), and the volume's box leaves it out
    /// ([`field_bounds`]). A picture drawn from the casters alone fills the hull about such a
    /// field's origin with field the record never threw, and an origin lit at any front draws a
    /// point of it. Fixture: every such field of every shipped record (one at least), read at its
    /// origin and at 200 directions 2 cm to 1 m out, its front past them.
    #[test]
    fn a_field_the_record_stops_at_its_origin_draws_nothing() {
        let potency = Potency::shipped();
        let golden = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
        let mut stopped = 0;
        for (name, report) in super::super::plan::tests::records() {
            let shapes = super::super::plan::tests::shapes(&report);
            let plan = super::super::plan::Plan::new(&report, &shapes);
            let pictures = plan.pictures();
            for (k, (cone, picture)) in plan.cones.iter().zip(&pictures.fields).enumerate() {
                if !cone.rays.is_empty() {
                    continue;
                }
                let casters = shapes
                    .hard()
                    .and_then(Casters::of)
                    .expect("the shipped records have their hard bodies");
                let reach = |d: Vec3| casters.reach(cone.origin, cone.axis, d);
                let seen = [Seen {
                    picture,
                    reach: &reach,
                }];
                let print = print_of(cone, picture, cone.origin, 10.0, 1.0);
                let prints = std::slice::from_ref(&print);
                assert_eq!(picture.texels(), 0, "{name} field {k}");
                assert_eq!(field_bounds(std::slice::from_ref(cone)), None);
                let at_origin = intensity(prints, &seen, &potency, cone.origin);
                assert_eq!(at_origin, 0.0, "{name} field {k} at its origin");
                for i in 0..200 {
                    let z = 1.0 - 2.0 * (i as f32 + 0.5) / 200.0;
                    let d = Vec2::from_angle(i as f32 * golden).extend(0.0) * (1.0 - z * z).sqrt()
                        + Vec3::Z * z;
                    for r in [0.02, 0.1, 0.4, 1.0] {
                        let p = cone.origin + d * r;
                        assert_eq!(intensity(prints, &seen, &potency, p), 0.0, "{name} {p}");
                        for rule in [Footprint::Hard, Footprint::Witness] {
                            for n in [d, -d] {
                                let painted = painting(prints, &seen, &potency, rule, p, n);
                                assert_eq!(painted, 0.0, "{name} {p} {rule:?}");
                            }
                        }
                    }
                }
                stopped += 1;
            }
        }
        assert!(stopped > 0, "no shipped record stops a field at its origin");
    }

    /// A field's stretch along an eye ray holds every point the field gives an intensity: the
    /// volume marches only its fields' stretches, so a point outside them is never sampled and its
    /// band never drawn. Fixture: a 25° cap; a ring's band of `λ = 1` and half-width 75°, from the
    /// axis out to 120° (DERIVED, `45° ± 75°`, its near edge held at the axis), a cap wider than
    /// the hemisphere, which no cone bounds; a ring's band of `λ = 0.45`; and the sphere, each
    /// direction's reach between 0.8 and 1.8 m, turning with the direction, at twice the record's
    /// size, their fronts mid-run and past every reach; eye rays from eyes
    /// about each origin (on its axis both ways and across it) toward a grid of points about it,
    /// read every 5 mm. A stretch that takes the cone's far nappe for its own, or bounds a cap
    /// wider than the hemisphere by its cone, holds a point out, and that point's band goes
    /// undrawn.
    #[test]
    fn the_stretch_holds_every_point_of_its_field() {
        let turning = |d: Vec3| 1.3 + 0.5 * (7.0 * d.x - 5.0 * d.y).sin();
        let reach = |_: usize, d: Vec3| turning(d);
        let fields = [
            cap(reach),
            fixture_field(
                FieldDescriptor::ring(1.0, 75.0_f32.to_radians()),
                128,
                reach,
            ),
            band(reach),
            sphere(reach),
        ];
        let potency = Potency::shipped();
        let scale = 2.0;
        for cone in &fields {
            let picture = Picture::of(cone, PITCH);
            let seen = [Seen {
                picture: &picture,
                reach: &turning,
            }];
            let mut met = 0;
            for front in [1.2 * scale, 10.0] {
                let print = print_of(cone, &picture, Vec3::new(-0.3, 0.7, 0.2), front, scale);
                let prints = std::slice::from_ref(&print);
                let eyes = [
                    picture.axis,
                    -picture.axis,
                    picture.u,
                    -picture.v,
                    (picture.axis + picture.u).normalize(),
                ];
                for eye in eyes.map(|e| print.origin + e * 5.0 * scale) {
                    for (i, j) in (0..25).map(|n| (n % 5, n / 5)) {
                        let at = print.origin
                            + (picture.u * (i as f32 - 2.0) + picture.v * (j as f32 - 2.0))
                                * 0.7
                                * scale;
                        let dir = (at - eye).normalize();
                        let span = stretch(&print, eye, dir);
                        for n in 0..2400 {
                            let t = n as f32 * 0.005 * scale;
                            if intensity(prints, &seen, &potency, eye + dir * t) <= 0.0 {
                                continue;
                            }
                            let held =
                                span.is_some_and(|(lo, hi)| t >= lo - 1e-3 && t <= hi + 1e-3);
                            assert!(
                                held,
                                "half-angle {}, front {front}: t = {t} off {span:?}",
                                picture.half_angle
                            );
                            met += 1;
                        }
                    }
                }
            }
            assert!(
                met > 1000,
                "half-angle {}: {met} points met",
                picture.half_angle
            );
        }
    }

    /// The item the module scope is pinned under: everything outside a function (imports,
    /// bindings, structs, constants, entry-point attributes).
    const MODULE_SCOPE: &str = "module scope";

    /// Every top-level item of the look's and the slice's shaders, `(file, item, digest)`: the
    /// FNV-1a fold of the item's text with comments and whitespace removed. MEASURED from the
    /// shaders the shipped frames were captured with.
    const SHADER_PINS: [(&str, &str, u64); 35] = [
        ("pen_cam_look.wgsl", "mode", 0xd884_58c8_42c2_833e),
        ("pen_cam_look.wgsl", "vertex", 0x0c5f_0c8e_ce2c_65f2),
        ("pen_cam_look.wgsl", "mask_at", 0x1d10_776f_dc3c_0e87),
        ("pen_cam_look.wgsl", "orthographic", 0xd848_4040_5cc7_bf5b),
        ("pen_cam_look.wgsl", "eye_depth", 0x959c_ebcc_5e07_ea9e),
        ("pen_cam_look.wgsl", "eye_ray", 0x8fbf_c6b6_664e_ae7b),
        ("pen_cam_look.wgsl", "face_of", 0xb6db_f5f2_a04e_8548),
        ("pen_cam_look.wgsl", "picture_at", 0x1dd3_126a_b772_9ffd),
        ("pen_cam_look.wgsl", "cap_share", 0x1bb7_0503_7b18_7581),
        ("pen_cam_look.wgsl", "sphere_reach", 0x1f4b_6ef2_6075_2f53),
        ("pen_cam_look.wgsl", "sphere_share", 0x6b86_7e7c_f36d_7a41),
        ("pen_cam_look.wgsl", "law", 0x2210_7415_8a4f_970d),
        ("pen_cam_look.wgsl", "field_at", 0x70bc_41a5_41b4_d99d),
        ("pen_cam_look.wgsl", "intensity", 0xa1d0_858a_a0d0_eea1),
        ("pen_cam_look.wgsl", "soft_step", 0x05cd_e5fb_8619_f8d0),
        ("pen_cam_look.wgsl", "inside", 0xbe35_1ba6_c6e3_7355),
        ("pen_cam_look.wgsl", "footprint", 0x0feb_6029_c26e_2427),
        ("pen_cam_look.wgsl", "printed", 0xa3ef_69a5_13f1_8034),
        ("pen_cam_look.wgsl", "stretch", 0xf54b_5e9e_e1d2_8521),
        ("pen_cam_look.wgsl", "over", 0xbcd5_9d02_0fb0_72c6),
        ("pen_cam_look.wgsl", "crossing", 0xf3eb_38f6_6b3c_7759),
        ("pen_cam_look.wgsl", "step_over", 0xa701_5de6_1500_b34c),
        ("pen_cam_look.wgsl", "spacing", 0x4525_6005_34ea_30f3),
        ("pen_cam_look.wgsl", "march", 0xec61_59f5_827e_b60c),
        ("pen_cam_look.wgsl", "volume", 0xab58_3d8f_98ae_99bf),
        ("pen_cam_look.wgsl", "lines", 0x5560_efe8_d460_738c),
        ("pen_cam_look.wgsl", "fragment", 0x025b_eb62_7e58_1499),
        ("pen_cam_look.wgsl", MODULE_SCOPE, 0x3dc3_45f5_40a9_8cdd),
        (
            "pen_cam_slice.wgsl",
            "behind_the_scene",
            0xd02c_1b1f_4fea_44f0,
        ),
        ("pen_cam_slice.wgsl", "bayer", 0x101e_1ea8_a796_5e70),
        ("pen_cam_slice.wgsl", "fragment", 0x8758_36dd_1da3_fde7),
        ("pen_cam_slice.wgsl", MODULE_SCOPE, 0xbbfa_4d63_379f_a666),
        ("pen_cam_picture.wgsl", "vertex", 0x5e53_ff03_bd49_2d09),
        ("pen_cam_picture.wgsl", "fragment", 0xc071_0faa_9da4_a075),
        ("pen_cam_picture.wgsl", MODULE_SCOPE, 0xefab_c0df_21d2_4ea4),
    ];

    /// The top-level items of WGSL `source` with comments and whitespace removed: each function by
    /// name, then the module scope. A function is the one that starts a line with `fn `; one
    /// written otherwise stays in the module scope, pinned there. Block comments are not stripped,
    /// so a source holding one is refused.
    fn shader_items(source: &str) -> Vec<(String, String)> {
        assert!(
            !source.contains("/*"),
            "a block comment: strip it before pinning"
        );
        let code: String = source
            .lines()
            .flat_map(|line| ["\n", line.split("//").next().unwrap_or_default()])
            .collect();
        let mut items = Vec::new();
        let mut scope = String::new();
        let mut rest = code.as_str();
        while let Some(start) = rest.find("\nfn ").map(|at| at + 1) {
            scope.push_str(&rest[..start]);
            let open = start + rest[start..].find('{').expect("a function body");
            let mut depth = 0;
            let end = rest[open..]
                .char_indices()
                .find_map(|(at, c)| {
                    depth += match c {
                        '{' => 1,
                        '}' => -1,
                        _ => 0,
                    };
                    (depth == 0).then_some(open + at + 1)
                })
                .expect("a closed function body");
            let name = rest[start + 3..].split('(').next().unwrap_or_default();
            items.push((name.trim().to_owned(), rest[start..end].to_owned()));
            rest = &rest[end..];
        }
        scope.push_str(rest);
        items.push((MODULE_SCOPE.to_owned(), scope));
        items
            .into_iter()
            .map(|(name, text)| (name, text.split_whitespace().collect()))
            .collect()
    }

    /// The look's and the slice's shaders are the ones the Rust mirrors were checked against and
    /// the frames were captured with: every top-level item, each function and the module scope,
    /// folds to its pin with comments and whitespace removed. A comment or a layout change keeps
    /// every pin; any other change moves the pin of the item it touches, whatever text it keeps
    /// elsewhere. The colour, band, eye-depth and draw-order promises rely on it: their Rust side
    /// is tested against the WGSL pinned here, and an unpinned item (`orthographic`, a binding)
    /// breaks a pinned one's frame with its text unchanged.
    #[test]
    fn the_shaders_are_the_ones_the_frames_were_captured_with() {
        let hex = |d: u64| {
            format!(
                "0x{:04x}_{:04x}_{:04x}_{:04x}",
                d >> 48,
                (d >> 32) & 0xffff,
                (d >> 16) & 0xffff,
                d & 0xffff
            )
        };
        let mut moved = Vec::new();
        let mut found = Vec::new();
        for file in [
            "pen_cam_look.wgsl",
            "pen_cam_slice.wgsl",
            "pen_cam_picture.wgsl",
        ] {
            for (item, text) in shader_items(&shader(file)) {
                let digest = fnv1a_64(FNV_OFFSET_BASIS, text.as_bytes());
                let pinned = SHADER_PINS
                    .iter()
                    .find(|&&(f, i, _)| f == file && i == item);
                if pinned.map(|&(_, _, pin)| pin) != Some(digest) {
                    moved.push(format!("(\"{file}\", \"{item}\", {}),", hex(digest)));
                }
                found.push((file, item));
            }
        }
        for (file, item, _) in SHADER_PINS {
            if !found.iter().any(|(f, i)| *f == file && i == item) {
                moved.push(format!("{file} `{item}`: pinned, no longer in the shader"));
            }
        }
        assert!(
            moved.is_empty(),
            "the pen cam's shaders moved; the moved items' pins:\n{}\nRe-pin only after the Rust \
             mirrors are read against the new WGSL and pass (`potency::tests` for `soft_step`, \
             `picture::tests` for `face_of` and `picture_at` (`Picture::lit`) and for \
             `pen_cam_picture.wgsl` (`Casters::reach`, its clip and cull), \
             `the_law_gives_each_record_direction_its_recorded_strength` for `law`, `cap_share`, \
             `sphere_reach` and `sphere_share`, the painting's tests \
             (`painting`) for `footprint`, `inside` and `printed`, \
             `a_band_runs_its_iso_surfaces_chord` and `doses_add_where_fields_overlap` for \
             `field_at`, `intensity`, `crossing`, `step_over`, `spacing` and `march`, \
             `a_small_field_inside_a_wide_one_is_drawn` for `volume`'s schedule (`schedule`), \
             `the_stretch_holds_every_point_of_its_field` for `stretch`, \
             `the_pen_cam_intensity_times_a_face_is_its_dose` for the intensity's law, \
             `the_eye_depth_runs_forward_in_both_projections` for `eye_depth` and `eye_ray`) and \
             a hidden capture of every frame the moved items draw (`OVERMATCH_CAPTURE` over \
             `pen_cam_demo`) shows the change meant.",
            moved.join("\n")
        );
    }

    /// `pen_cam_look.wgsl`'s `orthographic`, `eye_depth` and `eye_ray`, read from a view's
    /// `world_from_view` (the camera's `GlobalTransform`) and `clip_from_view` (its projection's)
    /// as Bevy's view uniform hands them to the shader; WGSL's `m[c]` is glam's column `c`.
    struct Eye {
        world_from_view: Mat4,
        clip_from_view: Mat4,
    }

    impl Eye {
        fn orthographic(&self) -> bool {
            self.clip_from_view.w_axis.w == 1.0
        }

        fn forward(&self) -> Vec3 {
            -self.world_from_view.z_axis.truncate().normalize()
        }

        fn to(&self, p: Vec3) -> Vec3 {
            p - self.world_from_view.w_axis.truncate()
        }

        fn depth(&self, p: Vec3) -> f32 {
            if self.orthographic() {
                self.to(p).dot(self.forward())
            } else {
                self.to(p).length()
            }
        }

        fn ray(&self, p: Vec3) -> Vec3 {
            if self.orthographic() {
                self.forward()
            } else {
                self.to(p).normalize()
            }
        }
    }

    /// The eye depth the masks store and the volume's march reads is a world point's distance from
    /// the eye in a perspective view and its distance along the camera's forward in an orthographic
    /// one, positive in front of the camera in both, and a point `s` back toward the eye along the
    /// eye ray lies `s` shallower: Bevy's orthographic projection is the one whose
    /// `clip_from_view[3][3]` is 1, and a camera looks down its `-Z` column. The mask write, the
    /// nearest-surface test and the volume's bound rely on it: a forward read the wrong way gives
    /// every surface in front of an orthographic camera a negative depth, which the mask write
    /// floors and every test against the masks then passes, and a ray that disagrees with the depth
    /// marches the volume from the wrong place. Fixture: a camera off every axis, points off its
    /// forward.
    #[test]
    fn the_eye_depth_runs_forward_in_both_projections() {
        let camera =
            Transform::from_xyz(3.0, 2.0, -4.0).looking_at(Vec3::new(-1.0, 0.5, 2.0), Vec3::Y);
        let world_from_view = GlobalTransform::from(camera).to_matrix();
        let (forward, right) = (*camera.forward(), *camera.right());
        for (clip_from_view, orthographic) in [
            (
                OrthographicProjection::default_3d().get_clip_from_view(),
                true,
            ),
            (PerspectiveProjection::default().get_clip_from_view(), false),
        ] {
            let eye = Eye {
                world_from_view,
                clip_from_view,
            };
            assert_eq!(eye.orthographic(), orthographic);
            for along in [0.5, 7.0, 40.0] {
                let offset = forward * along + right * 1.5;
                let p = camera.translation + offset;
                let depth = if orthographic { along } else { offset.length() };
                let at = eye.depth(p);
                assert!(
                    (at - depth).abs() < 1e-3,
                    "{orthographic}: {at}, not {depth}"
                );
                for s in [0.25, 0.5 * along] {
                    let nearer = eye.depth(p - eye.ray(p) * s);
                    assert!(
                        (nearer - (depth - s)).abs() < 1e-3,
                        "{orthographic}: {s} nearer than {depth} is {nearer}"
                    );
                }
            }
        }
    }

    /// A field's presentation is finite across the field law's whole domain: over the whole
    /// sphere (a ball at rest, `λ = 0`), over a ring's band (fixture `λ = 0.45`, half-width 10°), at
    /// the hemisphere (`λ = 1`) and at the narrowest cone the law resolves (the largest ratio
    /// `FieldDescriptor::check` admits, about 9.5·10⁷, a half-angle of about 10⁻⁸ rad, DERIVED),
    /// a field's emission is finite and positive, its law's weight ([`law`]) finite and positive
    /// at every stratum's centre, its picture ([`Picture::of`]) finite with a positive pitch, and
    /// every lane
    /// [`LookMaterial::set_fields`] packs is finite. The shader's volume takes its bands from
    /// those lanes; a solid angle that rounds to zero makes the emission infinite and every band
    /// of the field undefined, a law whose window rounds away draws nothing of the field, and a
    /// pitch of zero reads every direction off its picture. Fixture strength and reach.
    #[test]
    fn a_cone_packs_finite_lanes_across_the_law() {
        use super::super::plan::{Cone, Ray};
        use crate::ballistics::spall_cone::{ConeShape, FieldDescriptor};
        let admitted = |bits: u32| FieldDescriptor::ball(f32::from_bits(bits)).check().is_ok();
        let (mut lo, mut hi) = (1.0_f32.to_bits(), 1.0e9_f32.to_bits());
        assert!(admitted(lo) && !admitted(hi));
        // A positive f32's bits order as its value, and the law admits every ratio up to its top.
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if admitted(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let top = f32::from_bits(lo);
        assert!(top > 1.0e7, "the law resolves ratios up to {top}");
        let potency = Potency::shipped();
        let masks = MaskSet::default();
        let (strength, reach) = (10.0, 2.0);
        let fields = [
            FieldDescriptor::ball(0.0),
            FieldDescriptor::ring(0.45, 10.0_f32.to_radians()),
            FieldDescriptor::ball(1.0),
            FieldDescriptor::ball(top),
        ];
        for descriptor in fields {
            let ratio = descriptor.ratio;
            let spread = ConeShape::shipped().spread(descriptor).unwrap();
            let rays = spread
                .directions(Dir3::NEG_Z)
                .map(|(direction, _)| Ray {
                    direction: direction.into(),
                    reach,
                })
                .collect();
            let cone = Cone::new(
                Vec3::ZERO,
                Vec3::NEG_Z,
                descriptor,
                spread.directions_count(),
                strength,
                super::super::picture::tests::AT_ORIGIN,
                rays,
            );
            let emission = potency.emission(strength, cone.inner(), cone.half_angle());
            assert!(
                emission.is_finite() && emission > 0.0,
                "λ = {ratio}: emission {emission}"
            );
            let (lane, n) = (law_lane(descriptor, cone.strata), cone.strata);
            let (lo, hi) = descriptor.terms().unwrap().support();
            for k in 0..n {
                let t = lo + (hi - lo) * (k as f64 + 0.5) / n as f64;
                let weight = law(lane, t as f32);
                assert!(
                    weight.is_finite() && weight > 0.0,
                    "λ = {ratio}: stratum {k} of {n} weighs {weight}"
                );
            }
            let picture = Picture::of(&cone, PITCH);
            assert!(
                picture.pitch.is_finite() && picture.pitch > 0.0,
                "λ = {ratio}: pitch {}",
                picture.pitch
            );
            assert!(
                picture.longest.is_finite()
                    && picture.texels() > 0
                    && picture.lanes().iter().all(|lane| lane.is_finite()),
                "λ = {ratio}: a face is not finite"
            );
            let print = print_of(&cone, &picture, cone.origin, cone.max_reach(), 2.0);
            let mut field = volume(&potency, &masks, CornerKind::Glass, None, Handle::default());
            field.set_fields(std::slice::from_ref(&print), &potency);
            let params = field.params;
            let lanes = params.prints[..PRINT_LANES].iter().chain([
                &params.bands,
                &params.knockout,
                &params.reduced,
                &params.volume,
            ]);
            for (k, lane) in lanes.enumerate() {
                assert!(lane.is_finite(), "λ = {ratio}: lane {k} is {lane}");
            }
        }
    }

    /// The law gives each record direction the strength the record threw it with: over every
    /// field of every shipped record, at each stratum's centre depth `t_k` (the record's own
    /// strata, in f64), the f32 law ([`law`]) times the field's strength `S` is direction `k`'s
    /// recorded strength `S f_k`. The volume and the painting take a direction's strength from the
    /// law alone, as the shader's `law` does, so the bands drawn along a record direction are the
    /// record's dose. A law normalised apart from the spread's `f_k` (a window other than one
    /// stratum wide, the support's half-width `(t_hi − t_lo)/2` dropped) or a cap whose share takes
    /// another power of `D` than the ball law's (`BALL_EXPONENT`) fails here. Tolerance
    /// `S (16 N + 2⁸) 2⁻²⁴` (DERIVED: the f32 share difference holds about ten roundings of
    /// `2⁻²⁴`, and the window's `(t_hi − t_lo) / (2 (b − a)) = N/2` scales them; `2⁻¹⁶` the rest
    /// of the arithmetic). MEASURED worst `2.7·10⁻⁵ S` at `N = 256` and `5.9·10⁻⁶ S` at `N = 64`,
    /// against bounds of `2.6·10⁻⁴ S` and `7.6·10⁻⁵ S`.
    #[test]
    fn the_law_gives_each_record_direction_its_recorded_strength() {
        let mut checked = 0;
        for (name, report) in super::super::plan::tests::records() {
            for field in report.fields() {
                let n = field.fragments.len();
                let lane = law_lane(field.field, n);
                let (lo, hi) = field.field.terms().unwrap().support();
                let tolerance = field.strength * (16.0 * n as f32 + 256.0) * f32::EPSILON / 2.0;
                for (k, fragment) in field.fragments.iter().enumerate() {
                    let t = lo + (hi - lo) * (k as f64 + 0.5) / n as f64;
                    let got = field.strength * law(lane, t as f32);
                    assert!(
                        (got - fragment.strength).abs() <= tolerance,
                        "{name}: direction {k} of {n} at {got} ref-mm, recorded {}",
                        fragment.strength
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 1000, "{checked} directions checked");
    }
}
