//! The base look: the whole tank as one hueless translucent grey surface one layer deep, its
//! components solid in the untouched grey one layer deep among themselves, the struck ones on the
//! state scale, and the record's fields (its spall cones and a burst's) as one translucent volume
//! in their potency bands, the volume alone: a surface a field meets keeps its own look. It is the
//! tank rendering of every x-ray view: the corner, the kill cam's lesson and the x-ray. The
//! band law, the fields' drawing and the draw order are design doc §5, "Potency colour" and "Draw
//! order" (`.agents/docs/design/armor-penetration-and-damage.md`; [`super::potency`],
//! [`super::field`]). [`LookMaterial`] (`pen_cam_look.wgsl`) draws every element; [`hull`],
//! [`component`] and [`volume`] choose them, and the stage spawns what they return.
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

use bevy::camera::{Hdr, ImageRenderTarget, RenderTarget};
use bevy::core_pipeline::tonemapping::{DebandDither, Tonemapping};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, CompareFunction, Face, ShaderType, TextureFormat,
};
use bevy::shader::ShaderRef;
use bevy::window::{PrimaryWindow, WindowRef};

use super::field::{MAP_FIELDS, Map};
use super::material::Blend;
use super::plan::{BoxPlan, Cone};
use super::potency::{Band, Potency};
use super::stage::{Stage, StageCamera};
use super::timeline::{self, Condition, CornerKind};
use super::view::Tier;
use super::{Corner, Surface, WorldViewer};
use crate::render_policy::CameraProfile;

/// The fields' volume: its opacity at most, and each band's extinction length (record m), the
/// stretch of it along an eye ray that takes `1 − 1/e` of that opacity: short in the knockout band,
/// so its core reads dense, long in the reduced band, so its wide shell reads faint. TUNED, as is
/// every figure below that is not marked otherwise.
const VOLUME_OPACITY: f32 = 0.55;
const KNOCKOUT_EXTINCTION_M: f32 = 0.6;
const REDUCED_EXTINCTION_M: f32 = 3.0;
/// The volume's march: the steps across the widest field's ball, which set the samples' spacing
/// ([`spacing`]) and bound their count along a stretch of an eye ray, and the halvings that place
/// a band's edge between two samples, to `2⁻⁸` of a step (DERIVED). `pen_cam_look.wgsl` declares
/// both ([`march`]).
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
/// The most fields the look draws: the atlas's tiles. `pen_cam_look.wgsl`'s arrays hold this many
/// fields' lanes and [`pack`] drops what lies past it, so every record's fields must fit it
/// (`every_record_fits_the_look`).
const FIELDS: usize = MAP_FIELDS;
/// One field's lanes ([`pack`]).
const PRINT_LANES: usize = 5;

/// The uniform block `pen_cam_look.wgsl` reads; its lanes are documented there.
#[derive(ShaderType, Debug, Clone, Copy, Default)]
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
    pub bands: Vec4,
    pub knockout: Vec4,
    pub reduced: Vec4,
    pub volume: Vec4,
    pub fields: Vec4,
    pub prints: [Vec4; PRINT_LANES * FIELDS],
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
    /// The record's field maps ([`super::field::atlas`]), which the volume reads; `None` on every
    /// other element.
    #[texture(3)]
    #[sampler(4)]
    pub field_map: Option<Handle<Image>>,
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
            field_map: None,
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

    /// The volume's fields, `prints`, in its field lanes ([`pack`]), and each band's extinction
    /// length at their scale: every print of a record shares its hull's. The colour and opacity
    /// lanes, set when the volume is made, are untouched.
    pub fn set_fields(&mut self, prints: &[FieldPrint], potency: &Potency) {
        let (lanes, count) = pack(prints, potency);
        self.params.prints = lanes;
        self.params.fields.x = count as f32;
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
/// map's frame (the unit axis, `u` and `v` across it, [`Map`]), its support's half-angle and inner
/// edge (rad), its map's zoom, how far its front has run (m, negative before it is thrown), the
/// world's metres per record metre, and its map's longest reach (record m).
#[derive(Clone, Copy, Debug)]
pub struct FieldPrint {
    pub origin: Vec3,
    pub axis: Vec3,
    pub u: Vec3,
    pub v: Vec3,
    pub half_angle: f32,
    pub inner: f32,
    pub zoom: f32,
    pub front: f32,
    pub scale: f32,
    pub longest: f32,
}

/// The first [`FIELDS`] of `prints` in the fields' lanes, [`PRINT_LANES`] a field, and how many:
/// the origin (world) and half-angle; the unit axis (world) and the front (m); `u` and the inner
/// edge; `v` and the map's zoom; and the emission a reference-mm of strength gives, `ρ / Ω`
/// ([`Potency::emission`], `Ω` its support's solid angle) in world units (squared metres, whose
/// intensity falls as the square of a world distance), the world's metres per record metre (the
/// map holds record metres) and the longest reach (world m). Field `k` reads its map from the
/// atlas's tile `k` ([`super::field::tile`]).
fn pack(prints: &[FieldPrint], potency: &Potency) -> ([Vec4; PRINT_LANES * FIELDS], usize) {
    let mut lanes = [Vec4::ZERO; PRINT_LANES * FIELDS];
    for (print, lane) in prints.iter().zip(lanes.chunks_mut(PRINT_LANES)) {
        lane[0] = print.origin.extend(print.half_angle);
        lane[1] = print.axis.extend(print.front);
        lane[2] = print.u.extend(print.inner);
        lane[3] = print.v.extend(print.zoom);
        let emission = potency.emission(1.0, print.inner, print.half_angle);
        lane[4] = Vec4::new(
            emission * print.scale * print.scale,
            print.scale,
            print.longest * print.scale,
            0.0,
        );
    }
    (lanes, prints.len().min(FIELDS))
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

/// The fields' summed intensity (HP points per m²) at world point `p`, as the volume samples it
/// (`pen_cam_look.wgsl`, `intensity` and `field_at`): each of `prints`, whose maps are `maps`'
/// alike, adds `ρ s / (Ω r²)` where `p` lies on its support, within its front and within its
/// map's reach along its direction, `s` the map's strength there; at its origin, once thrown, it
/// is unbounded (`1e30`). Doses add, so the bands classify the sum, never a field's own. This is
/// the sum the shader mirrors, held by the tests.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the look's shader sums the fields at runtime (`pen_cam_look.wgsl`, `intensity`); this is the sum it mirrors, held by the tests"
    )
)]
pub fn intensity(prints: &[FieldPrint], maps: &[Map], potency: &Potency, p: Vec3) -> f32 {
    let (lanes, count) = pack(prints, potency);
    let mut sum = 0.0;
    for (lane, map) in lanes.chunks(PRINT_LANES).zip(maps).take(count) {
        let d = p - lane[0].truncate();
        let r = d.length();
        if r < 1e-5 {
            sum += if lane[1].w > 0.0 { 1e30 } else { 0.0 };
            continue;
        }
        let dir = d / r;
        let c = dir.dot(lane[1].truncate()).clamp(-1.0, 1.0);
        if c > lane[2].w.cos() || c < lane[0].w.cos() {
            continue;
        }
        let local = Vec3::new(dir.dot(lane[2].truncate()), dir.dot(lane[3].truncate()), c);
        let here = map.sample(map.u * local.x + map.v * local.y + map.axis * local.z);
        if r > (here.x * lane[4].y).min(lane[1].w) {
            continue;
        }
        sum += here.y * lane[4].x / (r * r);
    }
    sum
}

/// The march's spacing (m) over fields `prints`, as `pen_cam_look.wgsl`'s `spacing` takes it: the
/// widest field's ball (its radius the nearer of its front and its longest reach, as [`stretch`]
/// bounds it) across in [`VOLUME_STEPS`] steps. Every stretch is sampled at least this finely, so
/// a short stretch takes few samples and a long one no coarser ones.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the look's shader spaces its march at runtime (`pen_cam_look.wgsl`, `spacing`); this is the spacing it mirrors, held by the tests"
    )
)]
pub fn spacing(prints: &[FieldPrint]) -> f32 {
    let widest = prints
        .iter()
        .take(FIELDS)
        .map(|print| print.front.min(print.longest * print.scale))
        .fold(0.0, f32::max);
    2.0 * widest / VOLUME_STEPS as f32
}

/// The volume's march over one stretch of an eye ray, from `far` back to `near` (m along it), as
/// `pen_cam_look.wgsl`'s `march` and `step_over` take it: equal steps no longer than `space`
/// ([`spacing`]), [`VOLUME_STEPS`] at most, the band (`potency`'s, [`Potency::band`]) of
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
/// reach on its map in `maps`, which the volume is drawn on; `None` where no field reaches.
pub fn field_bounds(cones: &[Cone], maps: &[Map]) -> Option<(Vec3, Vec3)> {
    cones
        .iter()
        .zip(maps)
        .take(FIELDS)
        .filter(|(_, map)| map.longest > 0.0)
        .map(|(cone, map)| (cone.origin - map.longest, cone.origin + map.longest))
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

/// How a tank mesh of tier `tier` is drawn in the base look.
pub fn hull(tier: Tier, masks: &MaskSet) -> HullDraw {
    let layer = Layer::of(tier);
    let material = match layer {
        Layer::Body => base(masks),
        Layer::Scene => component_base(masks),
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
/// edge colour, tested against the part mask (`mode.z = 1`). A struck component's fill covers it.
fn component_base(masks: &MaskSet) -> LookMaterial {
    let mut material = base(masks);
    let grey = LinearRgba::from(timeline::STATE_UNTOUCHED);
    material.params.color = Vec4::new(grey.red, grey.green, grey.blue, BASE_COMPONENT_ALPHA);
    material.params.shade.x = BASE_COMPONENT_FACE_SHARE;
    material.params.mode.z = 1.0;
    material.blend = Blend::Opaque;
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
/// once the shot has dosed it, drawn under a [`timeline::STATE_RED`] outline [`STATE_OUTLINE_PX`]
/// wide; `None` where it is not drawn. The shooter's frames (the glass frame and the outside view)
/// draw a component this shot took HP from, and no other, so no earlier damage reaches the
/// shooter; the victim's x-ray draws every component with any of its pool gone.
pub fn component(b: &BoxPlan, kind: CornerKind, masks: &MaskSet) -> Option<LookMaterial> {
    let settled = condition(b, 1.0)?;
    let drawn = match kind {
        CornerKind::Glass | CornerKind::Damaged => b.struck(),
        CornerKind::Victim => true,
        CornerKind::Slice | CornerKind::Ricochet => false,
    };
    drawn.then(|| state(settled, masks))
}

/// The record's fields as one volume, drawn on the back faces of a box about every field's reach
/// ([`field_bounds`]), reading their maps from `atlas`. From each fragment the eye ray's stretches
/// inside the fields ([`stretch`], merged where they overlap) are marched back toward the eye
/// ([`march`]), each sample the fields' summed intensity ([`intensity`]); each band takes its
/// colour at an opacity that climbs with its length along the ray over its extinction length, the
/// nearer laid over the farther, the whole at most [`VOLUME_OPACITY`]. The fields' lanes are set
/// each frame ([`LookMaterial::set_fields`]). Straight alpha, sorted after every other translucent
/// surface, drawn whatever stands in front and writing no depth ([`LookKey::depth`]). In a frame
/// that draws the body behind ([`body_behind`]) the march ends at its pixel's nearest scene
/// surface in `masks`' part mask (`volume.w = 1`), so a component or the round in front of a field
/// hides it; in the outside view it is drawn over the tank whole. `cut` by the slice plane (unit
/// normal toward the discarded side, a point on it) it stops at the plane.
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
    m.field_map = Some(atlas);
    m.set_cut(cut);
    m
}

#[cfg(test)]
mod tests {
    use bevy::camera::CameraProjection;

    use super::super::field::tests::{band, cap, fixture_field, sphere};
    use super::super::field::{MAP_SIDE, MAP_TEXELS};
    use super::super::potency::solid_angle;
    use super::*;
    use crate::fingerprint::{FNV_OFFSET_BASIS, fnv1a_64};

    /// Every record's fields fit the look: none throws more than [`FIELDS`], its spall cones and
    /// its burst's field together, past which [`pack`] drops a field and the atlas holds no map
    /// for it, so the volume does not draw it.
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
    /// the map's side and texels, the atlas's tiles, the volume's march samples and halvings, the
    /// arrays' lengths, and [`LookParams`]' lanes in their order. A count that drifts reads the
    /// wrong field or texel without a word, or marches a volume short of its far side; a lane
    /// added, dropped or moved on one side alone reads every lane after it from its neighbour's
    /// slot.
    #[test]
    fn the_shader_declares_the_lane_counts() {
        let shader = shader("pen_cam_look.wgsl");
        for k in 0..FIELDS {
            let tile = super::super::field::tile(k);
            assert_eq!(
                tile,
                UVec2::new(k as u32 % 2, k as u32 / 2) * MAP_TEXELS as u32
            );
        }
        let atlas = super::super::field::atlas(&[]);
        assert_eq!(atlas.size(), UVec2::splat(2 * MAP_TEXELS as u32));
        for declared in [
            format!("const FIELDS: i32 = {FIELDS};"),
            format!("const PRINT_LANES: i32 = {PRINT_LANES};"),
            format!("const MAP_SIDE: f32 = {:.1};", MAP_SIDE as f32),
            format!("const MAP_TEXELS: f32 = {:.1};", MAP_TEXELS as f32),
            format!("const VOLUME_STEPS: i32 = {VOLUME_STEPS};"),
            format!("const VOLUME_REFINE: i32 = {VOLUME_REFINE};"),
            format!("prints: array<vec4<f32>, {}>,", PRINT_LANES * FIELDS),
            format!("var lo = array<f32, {FIELDS}>("),
            format!("var hi = array<f32, {FIELDS}>("),
            "let tile = vec2(f32(f % 2), f32(f / 2)) * MAP_TEXELS;".to_owned(),
            "let uv = (tile + g + 0.5) / (2.0 * MAP_TEXELS);".to_owned(),
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
            zoom: 0.3,
            front: 3.0,
            scale: 2.0,
            longest: 1.5,
        }
    }

    /// The colour reads no gain: the lanes the volume colours from carry the field's frame, its
    /// emission per reference-mm `ρ / Ω` in world units (squared metres, whose intensity falls as
    /// the square of a world distance), its scale and longest reach, the shipped thresholds and
    /// each band's colour from the band law, and fading the volume or a base surface moves its fade
    /// lane alone; the volume's extinction lengths scale with the stage. The colour promise relies
    /// on it: a display gain, level or fade in a colouring lane fails here.
    #[test]
    fn the_colour_reads_no_gain() {
        let potency = Potency::shipped();
        let masks = MaskSet::default();
        let atlas = Handle::<Image>::default();
        let print = fixture_print();
        let mut field = volume(&potency, &masks, CornerKind::Glass, None, atlas.clone());
        assert_eq!(field.field_map.as_ref(), Some(&atlas));
        field.set_fields(std::slice::from_ref(&print), &potency);
        let params = field.params;
        assert_eq!(params.fields.x, 1.0);
        let lane = &params.prints[..PRINT_LANES];
        assert_eq!(lane[0], print.origin.extend(print.half_angle));
        assert_eq!(lane[1], print.axis.extend(print.front));
        assert_eq!(lane[2], print.u.extend(print.inner));
        assert_eq!(lane[3], print.v.extend(print.zoom));
        assert_eq!(
            lane[4].x,
            4.0 * potency.emission(1.0, print.inner, print.half_angle)
        );
        assert_eq!(
            lane[4].x,
            4.0 * potency.rate / solid_angle(print.half_angle)
        );
        assert_eq!(lane[4].y, print.scale);
        assert_eq!(lane[4].z, print.longest * print.scale);
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

        for material in [&base(&masks), &field] {
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
            component_base(&masks),
            hull_mask(&masks),
            state(Condition::Dead, &masks),
        ] {
            assert_eq!(LookKey::from(&m).depth(), None);
        }
        assert_eq!(base(&masks).params.mode.z, 0.0);
        assert_eq!(component_base(&masks).params.mode.z, 1.0);
    }

    /// The print of `map`'s field from `origin` with its front at `front` (world m), `scale` world
    /// metres per record metre.
    fn print_of(map: &Map, origin: Vec3, front: f32, scale: f32) -> FieldPrint {
        FieldPrint {
            origin,
            axis: map.axis,
            u: map.u,
            v: map.v,
            half_angle: map.half_angle,
            inner: map.inner,
            zoom: map.zoom,
            front,
            scale,
            longest: map.longest,
        }
    }

    /// The march lays each band along an eye ray over the stretch its iso-surface cuts from the
    /// ray. Fixture: one field over the whole sphere, every direction reaching 2 m at 5 ref-mm, so
    /// its intensity falls as `1/r²` out to its reach, and a fixture potency whose mid and floor
    /// it crosses at 1 m and 1.6 m: the knockout band is the ball of radius 1 m, the reduced band
    /// the shell out to 1.6 m, nothing the shell out to the reach, and a ray `d` off the origin
    /// crosses each over its chord `2√(r² − d²)`, at the record's size and, every length doubled,
    /// at twice it. The march is bounded by the field's stretch ([`stretch`]). A band's opacity
    /// climbs with its length, so the volume shows the dose law only where these lengths are the
    /// chords: a march whose crossings are not halved (`VOLUME_REFINE = 0`) is off by up to half a
    /// step, an intensity that does not fall as the square of a world distance moves the stage's
    /// iso-radii, a stretch short of the reach drops the shell's far end, and samples spaced wider
    /// than [`spacing`] (one step a stretch) step over the knockout ball whole. The rays cross the
    /// ball from its centre to near its rim, so their stretches take from all [`VOLUME_STEPS`]
    /// steps down to a few. Tolerance: each band edge to `2⁻⁹` of the spacing, a step's bound
    /// (DERIVED from [`VOLUME_REFINE`]), two edges to a band and the shell two bands, and the map's
    /// rounding.
    #[test]
    fn a_band_runs_its_iso_surfaces_chord() {
        let map = Map::of(&sphere(|_, _| 2.0), None);
        let strength = map.sample(Vec3::X).y;
        assert!((strength - 5.0).abs() < 1e-4 && (map.longest - 2.0).abs() < 1e-4);
        let (r_mid, r_floor, reach) = (1.0_f32, 1.6_f32, 2.0_f32);
        let rate = 1.0;
        let k = rate * strength / solid_angle(map.half_angle);
        let potency = Potency {
            rate,
            floor: k / (r_floor * r_floor),
            mid: k / (r_mid * r_mid),
        };
        let maps = std::slice::from_ref(&map);
        let dir = Vec3::new(1.0, 0.2, -0.4).normalize();
        let across = dir.any_orthonormal_vector();
        for scale in [1.0, 2.0] {
            let print = print_of(&map, Vec3::new(0.5, -1.0, 2.0), 10.0 * scale, scale);
            let prints = std::slice::from_ref(&print);
            let space = spacing(prints);
            assert_eq!(space, 2.0 * map.longest * scale / VOLUME_STEPS as f32);
            for d in [0.0, 0.35, 0.9, 1.3, 1.75] {
                let eye = print.origin - dir * 6.0 * scale + across * d * scale;
                let (near, far) = stretch(&print, eye, dir).expect("the ray crosses the field");
                let mut lengths = [0.0_f32; 3];
                march(
                    near,
                    far,
                    space,
                    &potency,
                    |t| intensity(prints, maps, &potency, eye + dir * t),
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

    /// Fields' doses add: where two fields overlap, the band is their summed intensity's, never
    /// either field's own nor the larger of the two ([`intensity`], `pen_cam_look.wgsl`'s
    /// `intensity`). The merged volume relies on it. Fixture: two 25° caps of one strength from
    /// origins 0.2 m apart, their axes parallel, read at a point 1 m along the axis between them,
    /// where each lies 5.7° off its own axis; a fixture potency whose mid lies 1.6 times above
    /// either field's intensity there and whose floor at half it.
    #[test]
    fn doses_add_where_fields_overlap() {
        let map = Map::of(&cap(|_, _| 3.0, |_| 1.0), None);
        let maps = [map.clone(), map.clone()];
        let point = map.axis;
        let prints = [
            print_of(&map, map.u * 0.1, 5.0, 1.0),
            print_of(&map, -map.u * 0.1, 5.0, 1.0),
        ];
        let mut potency = Potency {
            rate: 1.0,
            floor: 0.0,
            mid: 0.0,
        };
        let one = intensity(&prints[..1], &maps, &potency, point);
        let other = intensity(&prints[1..], &maps[1..], &potency, point);
        assert!(one > 0.0 && (one - other).abs() <= 1e-4 * one);
        potency.floor = 0.5 * one;
        potency.mid = 1.6 * one;
        let both = intensity(&prints, &maps, &potency, point);
        assert!((both - (one + other)).abs() <= 1e-5 * both);
        assert_eq!(potency.band(one), Band::Reduced);
        assert_eq!(potency.band(other), Band::Reduced);
        assert_eq!(potency.band(both), Band::Knockout);
    }

    /// A field's stretch along an eye ray holds every point the field gives an intensity: the
    /// volume marches only its fields' stretches, so a point outside them is never sampled and its
    /// band never drawn. Fixture: a 25° cap, a 120° cap (wider than the hemisphere, which no cone
    /// bounds), a ring's band and the sphere, each direction's reach one of 0.8, 1.3 and 1.8 m, at
    /// twice the record's size, their fronts mid-run and past every reach; eye rays from eyes
    /// about each origin (on its axis both ways and across it) toward a grid of points about it,
    /// read every 5 mm. A stretch that takes the cone's far nappe for its own, or bounds a cap
    /// wider than the hemisphere by its cone, holds a point out, and that point's band goes
    /// undrawn.
    #[test]
    fn the_stretch_holds_every_point_of_its_field() {
        let reach = |k: usize, _: Vec3| [0.8, 1.3, 1.8][k % 3];
        let fields = [
            cap(reach, |_| 1.0),
            fixture_field(120.0_f32.to_radians(), 0.0, 128, reach, |_| 1.0),
            band(reach),
            sphere(reach),
        ];
        let potency = Potency::shipped();
        let scale = 2.0;
        for cone in &fields {
            let map = Map::of(cone, None);
            let maps = std::slice::from_ref(&map);
            let mut met = 0;
            for front in [1.2 * scale, 10.0] {
                let print = print_of(&map, Vec3::new(-0.3, 0.7, 0.2), front, scale);
                let prints = std::slice::from_ref(&print);
                let eyes = [
                    map.axis,
                    -map.axis,
                    map.u,
                    -map.v,
                    (map.axis + map.u).normalize(),
                ];
                for eye in eyes.map(|e| print.origin + e * 5.0 * scale) {
                    for (i, j) in (0..25).map(|n| (n % 5, n / 5)) {
                        let at = print.origin
                            + (map.u * (i as f32 - 2.0) + map.v * (j as f32 - 2.0)) * 0.7 * scale;
                        let dir = (at - eye).normalize();
                        let span = stretch(&print, eye, dir);
                        for n in 0..2400 {
                            let t = n as f32 * 0.005 * scale;
                            if intensity(prints, maps, &potency, eye + dir * t) <= 0.0 {
                                continue;
                            }
                            let held =
                                span.is_some_and(|(lo, hi)| t >= lo - 1e-3 && t <= hi + 1e-3);
                            assert!(
                                held,
                                "half-angle {}, front {front}: t = {t} off {span:?}",
                                map.half_angle
                            );
                            met += 1;
                        }
                    }
                }
            }
            assert!(
                met > 1000,
                "half-angle {}: {met} points met",
                map.half_angle
            );
        }
    }

    /// The item the module scope is pinned under: everything outside a function (imports,
    /// bindings, structs, constants, entry-point attributes).
    const MODULE_SCOPE: &str = "module scope";

    /// Every top-level item of the look's and the slice's shaders, `(file, item, digest)`: the
    /// FNV-1a fold of the item's text with comments and whitespace removed. MEASURED from the
    /// shaders the shipped frames were captured with.
    const SHADER_PINS: [(&str, &str, u64); 25] = [
        ("pen_cam_look.wgsl", "mode", 0xd884_58c8_42c2_833e),
        ("pen_cam_look.wgsl", "vertex", 0x0c5f_0c8e_ce2c_65f2),
        ("pen_cam_look.wgsl", "mask_at", 0x1d10_776f_dc3c_0e87),
        ("pen_cam_look.wgsl", "orthographic", 0xd848_4040_5cc7_bf5b),
        ("pen_cam_look.wgsl", "eye_depth", 0x959c_ebcc_5e07_ea9e),
        ("pen_cam_look.wgsl", "eye_ray", 0x8fbf_c6b6_664e_ae7b),
        ("pen_cam_look.wgsl", "sign_not_zero", 0x1679_03a6_bf5e_8900),
        ("pen_cam_look.wgsl", "octahedral", 0xee8d_db25_da07_f4ea),
        ("pen_cam_look.wgsl", "map_at", 0x7248_45da_66a6_a60d),
        ("pen_cam_look.wgsl", "field_at", 0x70fb_21e1_5b50_2ec7),
        ("pen_cam_look.wgsl", "intensity", 0xa1d0_858a_a0d0_eea1),
        ("pen_cam_look.wgsl", "stretch", 0xf54b_5e9e_e1d2_8521),
        ("pen_cam_look.wgsl", "over", 0xbcd5_9d02_0fb0_72c6),
        ("pen_cam_look.wgsl", "crossing", 0xf3eb_38f6_6b3c_7759),
        ("pen_cam_look.wgsl", "step_over", 0xa701_5de6_1500_b34c),
        ("pen_cam_look.wgsl", "spacing", 0x2965_9a2f_86a2_a75f),
        ("pen_cam_look.wgsl", "march", 0xec61_59f5_827e_b60c),
        ("pen_cam_look.wgsl", "volume", 0xf205_578b_73d1_2d09),
        ("pen_cam_look.wgsl", "lines", 0x5560_efe8_d460_738c),
        ("pen_cam_look.wgsl", "fragment", 0xe3bf_ec3e_a4b8_f368),
        ("pen_cam_look.wgsl", MODULE_SCOPE, 0xa8b5_b572_9df8_d338),
        (
            "pen_cam_slice.wgsl",
            "behind_the_scene",
            0xd02c_1b1f_4fea_44f0,
        ),
        ("pen_cam_slice.wgsl", "bayer", 0x101e_1ea8_a796_5e70),
        ("pen_cam_slice.wgsl", "fragment", 0x8758_36dd_1da3_fde7),
        ("pen_cam_slice.wgsl", MODULE_SCOPE, 0xbbfa_4d63_379f_a666),
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
        for file in ["pen_cam_look.wgsl", "pen_cam_slice.wgsl"] {
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
             mirrors are read against the new WGSL and pass (`field::tests` for `octahedral` and \
             `map_at` (`Map::sample`), \
             `a_band_runs_its_iso_surfaces_chord` and `doses_add_where_fields_overlap` for \
             `field_at`, `intensity`, `crossing`, `step_over`, `spacing` and `march`, \
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
    /// a direction's emission is finite and positive, its map ([`Map::of`]) is finite with a
    /// positive zoom, and every lane [`LookMaterial::set_fields`] packs is finite. The shader's
    /// volume takes its bands from those lanes; a solid angle that rounds to zero makes the
    /// emission infinite and every band of the field undefined, and a zoom of zero reads every
    /// direction off the map. Fixture strength and reach.
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
                .map(|(direction, weight)| Ray {
                    direction: direction.into(),
                    reach,
                    weight,
                })
                .collect();
            let cone = Cone::new(
                Vec3::ZERO,
                Vec3::NEG_Z,
                descriptor.half_angle(),
                descriptor.inner_angle(),
                strength,
                super::super::field::tests::AT_ORIGIN,
                rays,
            );
            let emission = potency.emission(strength, cone.inner, cone.half_angle);
            assert!(
                emission.is_finite() && emission > 0.0,
                "λ = {ratio}: emission {emission}"
            );
            let map = Map::of(&cone, None);
            assert!(
                map.zoom.is_finite() && map.zoom > 0.0,
                "λ = {ratio}: zoom {}",
                map.zoom
            );
            assert!(
                map.longest.is_finite() && map.texels.iter().all(|t| t.is_finite()),
                "λ = {ratio}: a texel is not finite"
            );
            let print = print_of(&map, cone.origin, cone.max_reach(), 2.0);
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
}
