//! The base look: the whole tank as one hueless translucent grey surface one layer deep, its
//! components solid in the untouched grey one layer deep among themselves, the struck ones on the
//! state scale, and each spall field as one translucent volume in the potency colour, with its
//! footprint on every surface it met. It is the tank rendering of every x-ray view: the corner,
//! the kill cam's lesson and the x-ray. The colour's law, the volume's rim and the draw order are
//! design doc §5, "Potency colour" and "Draw order"
//! (`.agents/docs/design/armor-penetration-and-damage.md`; [`super::potency`]).
//! [`LookMaterial`] (`pen_cam_look.wgsl`) draws every element; [`hull`], [`component`] and
//! [`volume`] choose them, and the stage spawns what they return.
//!
//! Draw order, in every frame that draws the body behind ([`body_behind`]: the glass frames and
//! the slice): each tank mesh is on one layer by its tier ([`Layer::of`]), and the round's model
//! is on the scene's ([`Layer::ROUND`]). The body draws behind everything else: a body fragment
//! is discarded wherever the part mask holds a scene surface, and every volume is drawn after it
//! and over it, writing no depth. The scene (the components, their state fills and outlines, the
//! round's model and the fields' volumes) is ordered by depth among itself: a component is opaque
//! and depth-tested, as is the round in its own opaque material, and a volume's march ends at its
//! pixel's nearest scene surface in the part mask, so a component or the round in front of a
//! field hides it and a field in front of either blends over it. A volume tests no other depth,
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

use super::field::DISC_SIDE;
use super::material::Blend;
use super::plan::BoxPlan;
use super::potency::Potency;
use super::stage::{Stage, StageCamera};
use super::timeline::{self, Condition, CornerKind};
use super::view::Tier;
use super::{Corner, Surface, WorldViewer};
use crate::render_policy::CameraProfile;

/// A spall field's volume: its opacity at most, and the weighted path through it
/// ([`super::potency::Rim`]), as a share of its cone's longest reach, over which its opacity
/// climbs to `1 − 1/e` of that, so a thin stretch (the silhouette, a sliver beside a shadowed
/// direction) stays faint. TUNED, as is every figure below that is not marked otherwise.
const VOLUME_OPACITY: f32 = 0.55;
const VOLUME_DEPTH_SHARE: f32 = 0.2;
/// The march's samples along an eye ray inside a volume. The step is the cone's longest chord over
/// this count (DERIVED, [`VolumeGeometry::of`]), so the march spans any chord in at most this many
/// steps; `pen_cam_look.wgsl` declares the same count.
const VOLUME_STEPS: usize = 16;
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
/// The field footprint's fill opacity, short of opaque so the HP fill under it, the outcome,
/// reads through.
const FOOTPRINT_FILL_ALPHA: f32 = 0.6;
/// The most spall fields a footprint marks; `pen_cam_look.wgsl`'s arrays hold this many fields'
/// lanes and [`set_footprints`] drops what lies past it, so every record's cones must fit it
/// (`every_record_fits_the_footprint`).
const FOOTPRINT_FIELDS: usize = 4;
/// One field's disc in lanes, two nodes a lane.
const DISC_LANES: usize = DISC_SIDE * DISC_SIDE / 2;
/// A surface counts as met by a direction up to this share past its reach, and this far (m): the
/// record ends a direction on the surface that stopped it, and between directions the disc's
/// reach is interpolated, not the surface's distance.
const FOOTPRINT_REACH_SLACK: (f32, f32) = (1.03, 0.03);

/// The uniform block `pen_cam_look.wgsl` reads; its lanes are documented there.
#[derive(ShaderType, Debug, Clone, Copy)]
pub struct LookParams {
    pub color: Vec4,
    pub edge: Vec4,
    pub shade: Vec4,
    pub field: Vec4,
    pub hatch: Vec4,
    pub mode: Vec4,
    pub state: Vec4,
    pub span: Vec4,
    pub rest: Vec4,
    pub plane: Vec4,
    pub foot: Vec4,
    pub ramp: [Vec4; 4],
    pub volume: Vec4,
    pub rim: Vec4,
    pub fields: Vec4,
    pub prints: [Vec4; 4 * FOOTPRINT_FIELDS],
    pub disc: [Vec4; FOOTPRINT_FIELDS * DISC_LANES],
}

impl Default for LookParams {
    fn default() -> Self {
        Self {
            color: Vec4::ZERO,
            edge: Vec4::ZERO,
            shade: Vec4::ZERO,
            field: Vec4::ZERO,
            hatch: Vec4::ZERO,
            mode: Vec4::ZERO,
            state: Vec4::ZERO,
            span: Vec4::ZERO,
            rest: Vec4::ZERO,
            plane: Vec4::ZERO,
            foot: Vec4::ZERO,
            ramp: [Vec4::ZERO; 4],
            volume: Vec4::ZERO,
            rim: Vec4::ZERO,
            fields: Vec4::ZERO,
            prints: [Vec4::ZERO; 4 * FOOTPRINT_FIELDS],
            disc: [Vec4::ZERO; FOOTPRINT_FIELDS * DISC_LANES],
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
                field: Vec4::new(-1.0, 0.0, 0.0, 0.0),
                mode: Vec4::new(mode, 0.0, 0.0, 0.0),
                ..default()
            },
            hull_mask: Some(masks.hull.clone()),
            part_mask: Some(masks.part.clone()),
            blend,
            bias: false,
        }
    }

    pub fn set_fade(&mut self, fade: f32) {
        self.params.shade.w = fade;
    }

    /// Whether it draws a spall field's volume ([`volume`]).
    pub fn is_volume(&self) -> bool {
        self.params.mode.x == MODE_VOLUME
    }

    /// A volume's field, `print`, in its first field's lanes: the field's frame and front, its
    /// disc's reach and emission ([`pack`]), and the march's step and the opacity's depth from the
    /// disc's longest reach ([`VolumeGeometry::of`]). The colour lanes, set when the volume is
    /// made, are untouched.
    pub fn set_volume(&mut self, print: &FieldPrint, potency: &Potency) {
        pack(
            print,
            potency,
            &mut self.params.prints[..4],
            &mut self.params.disc[..DISC_LANES],
        );
        let geometry = VolumeGeometry::of(print);
        self.params.volume.y = geometry.depth;
        self.params.volume.z = geometry.step;
        self.params.fields.x = 1.0;
    }

    pub fn set_front(&mut self, radius: Option<f32>) {
        self.params.field.x = radius.map_or(-1.0, |r| r.max(0.0));
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
        // outward-wound surface (`field::reach_mesh`); every other element from both faces.
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

/// One spall field where a footprint or a volume is taken, in world space: its origin, its disc's
/// frame (the unit axis, `u` and `v` across it, the half-angle in rad), how far its front has run
/// (m, negative before it is thrown), the world's metres per record metre, and its disc's nodes
/// ([`super::field::Disc::nodes`], record units).
#[derive(Clone, Debug)]
pub struct FieldPrint<'a> {
    pub origin: Vec3,
    pub axis: Vec3,
    pub u: Vec3,
    pub v: Vec3,
    pub half_angle: f32,
    pub front: f32,
    pub scale: f32,
    pub nodes: &'a [Vec2],
}

/// `print` in one field's lanes: four `frame` lanes (origin and half-angle; axis and front; `u`;
/// `v`) and its disc's [`DISC_LANES`] lanes, two nodes a lane, each node's reach and emission
/// `ρ s / Ω_c` ([`Potency::emission`]) in world units: metres for the reach, squared metres for
/// the emission, whose intensity falls as the square of a world distance.
fn pack(print: &FieldPrint, potency: &Potency, frame: &mut [Vec4], disc: &mut [Vec4]) {
    frame[0] = print.origin.extend(print.half_angle);
    frame[1] = print.axis.extend(print.front);
    frame[2] = print.u.extend(0.0);
    frame[3] = print.v.extend(0.0);
    let world = |node: Vec2| {
        let emission = potency.emission(node.y, print.half_angle);
        Vec2::new(node.x * print.scale, emission * print.scale * print.scale)
    };
    for (lane, pair) in disc.iter_mut().zip(print.nodes.chunks(2)) {
        let second = pair.get(1).copied().map_or(Vec2::ZERO, world);
        *lane = world(pair[0]).extend(second.x).extend(second.y);
    }
}

/// The march's step and the opacity's depth of a field's volume, world metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeGeometry {
    /// The longest chord any eye ray can run inside the volume, over [`VOLUME_STEPS`].
    pub step: f32,
    /// [`VOLUME_DEPTH_SHARE`] of the disc's longest reach.
    pub depth: f32,
}

impl VolumeGeometry {
    /// `print`'s, from `R`, the longest reach among its disc's nodes (world m), past which neither
    /// the volume's surface nor its march reads the disc. Every point of the volume lies within
    /// `R` of the origin on a direction at most `α` off the axis, so two of them lie at most
    /// `R · max(1, 2 sin α)` apart (DERIVED: the third side of a triangle whose other two are at
    /// most `R` about an angle at most `2α`, for `α ≤ 90°` as `spall_cone.ron`'s parse holds): the
    /// longest chord.
    pub fn of(print: &FieldPrint) -> Self {
        let reach = print.nodes.iter().map(|node| node.x).fold(0.0, f32::max) * print.scale;
        let chord = reach * (2.0 * print.half_angle.sin()).max(1.0);
        Self {
            step: chord / VOLUME_STEPS as f32,
            depth: VOLUME_DEPTH_SHARE * reach,
        }
    }
}

/// Mark `prints` on every surface the base look drawn against `masks` draws, the hull's and the
/// components': a base or state material is one look's by the mask set it holds, so another
/// look's surfaces (the corner's beside the x-ray's) keep their own footprints. A surface point is
/// in a field where it faces the field's origin, lies inside its cone, behind its front and no
/// farther than its direction's reach on the disc: the field met it there. It is filled in the
/// potency colour of the fields' summed intensity there ([`super::potency::Ramp`]). The disc lanes
/// ([`pack`]) and the ramp lanes carry the law alone: nothing display-side sets the colour.
pub fn set_footprints(
    looks: &mut Assets<LookMaterial>,
    masks: &MaskSet,
    prints: &[FieldPrint],
    potency: &Potency,
) {
    let mut lanes = [Vec4::ZERO; 4 * FOOTPRINT_FIELDS];
    let mut disc = [Vec4::ZERO; FOOTPRINT_FIELDS * DISC_LANES];
    let count = prints.len().min(FOOTPRINT_FIELDS);
    for (i, print) in prints.iter().take(count).enumerate() {
        pack(
            print,
            potency,
            &mut lanes[4 * i..4 * (i + 1)],
            &mut disc[i * DISC_LANES..(i + 1) * DISC_LANES],
        );
    }
    let (share, metres) = FOOTPRINT_REACH_SLACK;
    let foot = Vec4::new(FOOTPRINT_FILL_ALPHA, share, metres, 0.0);
    let ramp = potency.ramp().lanes();
    for (_, material) in looks.iter_mut() {
        let mode = material.params.mode.x;
        if mode != MODE_BASE && mode != MODE_STATE
            || material.hull_mask.as_ref() != Some(&masks.hull)
        {
            continue;
        }
        material.params.foot = foot;
        material.params.ramp = ramp;
        material.params.fields = Vec4::new(count as f32, 0.0, 0.0, 0.0);
        material.params.prints = lanes;
        material.params.disc = disc;
    }
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

/// Component `b`'s place on the state scale in frame `kind`, `None` where it is not drawn: the
/// shooter's frames (the glass frame and the outside view) show what this shot did to a component it
/// took HP from, the share of the HP points it had at the shot; the victim's x-ray shows its own
/// tank's state, the share of each pool gone.
fn condition(b: &BoxPlan, kind: CornerKind) -> Option<Condition> {
    match kind {
        CornerKind::Glass | CornerKind::Damaged if b.struck() => {
            Condition::of(b.lost, timeline::hp_share(b.lost_hp, b.hp))
        }
        CornerKind::Victim => Condition::of(b.lost, b.pool_lost()),
        _ => None,
    }
}

/// The state scale's opaque fill for `condition`: the struck share of the component's height in
/// red from the bottom up, the rest in the untouched grey, a crisp lighter line between them; a
/// dead component charcoal under a red hatch. Its frame is set each frame
/// ([`LookMaterial::set_fill_span`]) and its fade raises the fill.
fn state(condition: Condition, masks: &MaskSet) -> LookMaterial {
    let (color, hatch) = match condition {
        Condition::Damaged(_) => (timeline::STATE_RED, false),
        Condition::Dead => (timeline::STATE_DEAD, true),
    };
    let mut m = LookMaterial::new(MODE_STATE, color.into(), 1.0, Blend::Opaque, masks);
    m.bias = true;
    let line = LinearRgba::from(if hatch {
        timeline::STATE_RED
    } else {
        timeline::STATE_LINE
    });
    let rest = LinearRgba::from(timeline::STATE_UNTOUCHED);
    m.params.edge = Vec4::new(line.red, line.green, line.blue, 1.0);
    m.params.rest = Vec4::new(rest.red, rest.green, rest.blue, 1.0);
    m.params.shade.y = STATE_CONTRAST;
    m.params.state.w = condition.fill();
    m.params.span.w = STATE_LINE_PX;
    m.params.mode.w = hatch as u8 as f32;
    m.params.hatch.x = STATE_HATCH_PX;
    m.params.hatch.y = STATE_HATCH_SHARE;
    m
}

/// Component `b`'s fill in frame `kind`, in any look: the state scale ([`state`]), drawn under a
/// [`timeline::STATE_RED`] outline [`STATE_OUTLINE_PX`] wide; `None` where it is not drawn.
pub fn component(b: &BoxPlan, kind: CornerKind, masks: &MaskSet) -> Option<LookMaterial> {
    condition(b, kind).map(|condition| state(condition, masks))
}

/// A spall field's volume, drawn on its reach surface ([`super::field::reach_mesh`]): from each
/// fragment where the eye ray leaves the volume, the ray is marched back toward the eye through
/// it, each sample weighed by its intensity ([`super::potency::Rim`]), and the pixel takes the
/// weighted mean potency colour of what it crossed ([`super::potency::Ramp`]), at an opacity that
/// climbs with the weighted length crossed to [`VOLUME_OPACITY`], so the volume fades out toward
/// the ramp's dim end; its field's lanes are set each frame ([`LookMaterial::set_volume`]).
/// Straight alpha, sorted after every other
/// translucent surface, drawn whatever stands in front and writing no depth ([`LookKey::depth`]).
/// In a frame that draws the body behind ([`body_behind`]) the march ends at its pixel's nearest
/// scene surface in `masks`' part mask (`volume.w = 1`), so a component or the round in front of
/// the field hides it; in the outside view it is drawn over the tank whole. `cut` by the slice
/// plane (unit normal toward the discarded side, a point on it) it stops at the plane.
pub fn volume(
    potency: &Potency,
    masks: &MaskSet,
    kind: CornerKind,
    cut: Option<(Vec3, Vec3)>,
) -> LookMaterial {
    let mut m = LookMaterial::new(MODE_VOLUME, LinearRgba::BLACK, 0.0, Blend::Blended, masks);
    m.params.ramp = potency.ramp().lanes();
    m.params.rim = potency.rim().lanes();
    m.params.volume.x = VOLUME_OPACITY;
    m.params.volume.w = body_behind(kind) as u8 as f32;
    m.set_cut(cut);
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every record's spall cones fit the footprint: none throws more than [`FOOTPRINT_FIELDS`],
    /// past which [`set_footprints`] drops a field unmarked, and a disc's nodes fill its lanes
    /// exactly, whatever the cone's direction count.
    #[test]
    fn every_record_fits_the_footprint() {
        for (name, report) in super::super::plan::tests::records() {
            let cones = report.spall.len();
            assert!(
                cones <= FOOTPRINT_FIELDS,
                "{name} throws {cones} cones, past FOOTPRINT_FIELDS = {FOOTPRINT_FIELDS}"
            );
        }
        assert_eq!(2 * DISC_LANES, DISC_SIDE * DISC_SIDE);
    }

    /// The look's shader, read from the shipped assets.
    fn shader() -> String {
        let path = crate::assets::asset_root().join("shaders/pen_cam_look.wgsl");
        std::fs::read_to_string(path).unwrap()
    }

    /// `pen_cam_look.wgsl` declares the look's counts and lanes: the disc's side and lanes, the
    /// volume's march samples, the arrays' lengths, the fields its footprint loops over, and
    /// [`LookParams`]' lanes in their order. A count that drifts reads the wrong node, field or
    /// ramp anchor without a word, or marches a volume short of its far side; a lane added,
    /// dropped or moved on one side alone reads every lane after it from its neighbour's slot.
    #[test]
    fn the_shader_declares_the_lane_counts() {
        let shader = shader();
        for declared in [
            format!("const DISC_SIDE: i32 = {DISC_SIDE};"),
            format!("const DISC_LANES: i32 = {DISC_LANES};"),
            format!("const VOLUME_STEPS: i32 = {VOLUME_STEPS};"),
            "ramp: array<vec4<f32>, 4>,".to_owned(),
            format!("prints: array<vec4<f32>, {}>,", 4 * FOOTPRINT_FIELDS),
            format!("disc: array<vec4<f32>, {}>,", FOOTPRINT_FIELDS * DISC_LANES),
            format!("for (var f = 0; f < {FOOTPRINT_FIELDS}; f = f + 1)"),
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

    /// Fixture: two nodes, a stage at twice the record's size.
    fn fixture_print(nodes: &[Vec2]) -> FieldPrint<'_> {
        FieldPrint {
            origin: Vec3::ONE,
            axis: Vec3::NEG_Z,
            u: Vec3::X,
            v: Vec3::Y,
            half_angle: 25.0_f32.to_radians(),
            front: 3.0,
            scale: 2.0,
            nodes,
        }
    }

    /// The colour reads no gain: the lanes a footprint and a volume colour from carry each node's
    /// reach and emission `ρ s / Ω_c` in world units (squared metres for the emission, whose
    /// intensity falls as the square of a world distance), the shipped thresholds and the shipped
    /// ramp, and fading a material moves its fade lane alone. The colour promise relies on it; a
    /// display gain, level or fade in a colouring lane, or a volume packed apart from the
    /// footprint, fails here.
    #[test]
    fn the_colour_reads_no_gain() {
        let potency = Potency::shipped();
        let masks = MaskSet::default();
        let mut looks = Assets::<LookMaterial>::default();
        let handle = looks.add(base(&masks));
        let nodes = [Vec2::new(1.5, 5.0), Vec2::new(0.4, 2.5)];
        let print = fixture_print(&nodes);
        set_footprints(&mut looks, &masks, std::slice::from_ref(&print), &potency);
        let params = looks.get(&handle).unwrap().params;
        let lane = params.disc[0];
        for (k, node) in nodes.iter().enumerate() {
            let (reach, emission) = (lane[2 * k], lane[2 * k + 1]);
            assert_eq!(reach, 2.0 * node.x);
            assert_eq!(emission, 4.0 * potency.emission(node.y, print.half_angle));
            assert_eq!(
                emission,
                4.0 * potency.rate * node.y / super::super::potency::solid_angle(print.half_angle)
            );
        }
        assert_eq!(params.ramp, potency.ramp().lanes());

        let mut field = volume(&potency, &masks, CornerKind::Glass, None);
        assert_eq!(field.params.ramp, potency.ramp().lanes());
        field.set_volume(&print, &potency);
        assert_eq!(field.params.disc[..DISC_LANES], params.disc[..DISC_LANES]);
        assert_eq!(field.params.prints[..4], params.prints[..4]);
        assert_eq!(field.params.ramp, potency.ramp().lanes());
        assert_eq!(field.params.rim, potency.rim().lanes());
        assert_eq!(field.params.volume.x, VOLUME_OPACITY);

        for material in [looks.get(&handle).unwrap(), &field] {
            let mut faded = material.clone();
            faded.set_fade(0.25);
            let mut want = material.params;
            want.shade.w = 0.25;
            assert_eq!(format!("{:?}", faded.params), format!("{want:?}"));
        }
    }

    /// A volume fades at its rim: the shader weighs each march sample by the curve of
    /// `potency::Rim::weight`, from the volume's rim lane about the ramp lane's mid threshold, and
    /// one weighed sample carries both the colour and the length, so the colour is the weighted
    /// mean and the opacity climbs with the weighted length alone. The rim promise relies on it:
    /// an opacity from the plain length keeps the faint outer stretch at full strength, and a
    /// colour weighted apart from the length shifts the mean toward the deep teal the opacity no
    /// longer shows.
    #[test]
    fn the_volume_fades_at_its_rim() {
        let potency = Potency::shipped();
        let masks = MaskSet::default();
        let field = volume(&potency, &masks, CornerKind::Glass, None);
        let rim = potency.rim();
        assert_eq!(field.params.rim, rim.lanes());
        let mid = field.params.ramp[3].w;
        assert_eq!(rim.weight(mid), 1.0);
        assert_eq!(rim.weight(mid * field.params.rim.x.exp2()), 0.0);
        assert!(field.params.rim.x < field.params.rim.y);
        let shader = shader();
        for read in [
            "return smoothstep(look.rim.x, look.rim.y, log2(max(i, 1e-30) / look.ramp[3].w));",
            "let w = weight(i);\n    return vec4(ramp(i) * w, w);",
            "sum = sum + 0.5 * (now + last) * (last_t - t);",
            "let near = select(last, weighed(hi_intensity), hi_intensity >= 0.0);",
            "sum = sum + 0.5 * (near + last) * (last_t - entry);",
            "let a = look.volume.x * (1.0 - exp(-sum.a / look.volume.y)) * fade;",
            "return vec4(sum.rgb / sum.a, clamp(a, 0.0, 1.0));",
        ] {
            assert!(shader.contains(read), "{read}");
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
    /// place.
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
            let m = volume(&potency, &masks, kind, None);
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
        let shader = shader();
        for read in [
            "if mask_at(part_mask, p).a > 0.0 {\n                discard;",
            "if look.volume.w > 0.5 {\n        let bound = mask_at(part_mask, p).a;",
            "let t_exit = eye_depth(exit);",
            "let depth = eye_depth(in.world_position.xyz);",
            "return vec4(vn * 0.5 + 0.5, max(depth, 1e-4));",
            "if nearest > 0.0 && depth > nearest * 1.002 + 0.003 {\n            discard;\n        }",
        ] {
            assert!(shader.contains(read), "{read}");
        }
    }

    /// A volume's march spans its longest chord: [`VOLUME_STEPS`] steps cover the distance between
    /// any two vertices of its reach surface, the volume's bound, for every record's cones at the
    /// record's size and at the stage's. The shader marches at most that many steps back from where
    /// the eye ray leaves; a step from the mean reach, or one that forgets the cone's width, leaves
    /// the near part of a long chord unread and its colour short.
    #[test]
    fn the_march_spans_every_chord() {
        let mut checked = 0;
        for (name, plan) in super::super::potency::tests::plans() {
            for cone in &plan.cones {
                let mesh = super::super::field::reach_mesh(cone);
                let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
                    mesh.attribute(Mesh::ATTRIBUTE_POSITION)
                else {
                    panic!("positions");
                };
                let mut corners: Vec<[u32; 3]> =
                    positions.iter().map(|p| p.map(f32::to_bits)).collect();
                corners.sort_unstable();
                corners.dedup();
                let corners: Vec<Vec3> = corners
                    .iter()
                    .map(|p| Vec3::from(p.map(f32::from_bits)))
                    .collect();
                let chord = corners
                    .iter()
                    .flat_map(|a| corners.iter().map(|b| a.distance(*b)))
                    .fold(0.0, f32::max);
                for scale in [1.0, 2.0] {
                    let print = FieldPrint {
                        origin: cone.origin,
                        axis: cone.disc.axis,
                        u: cone.disc.u,
                        v: cone.disc.v,
                        half_angle: cone.half_angle,
                        front: 0.0,
                        scale,
                        nodes: &cone.disc.nodes,
                    };
                    let span = VOLUME_STEPS as f32 * VolumeGeometry::of(&print).step;
                    assert!(
                        span >= scale * chord * (1.0 - 1e-5),
                        "{name}: {span} m marched against a {} m chord",
                        scale * chord
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 0);
    }

    /// A cone's presentation is finite across the field law's whole domain: at the hemisphere
    /// (`λ = 1`) and at the narrowest cone the law resolves (the largest ratio
    /// `FieldDescriptor::check` admits, about 9.5·10⁷, a half-angle of about 10⁻⁸ rad, DERIVED),
    /// a direction's emission is finite and positive, and every lane [`set_footprints`] and
    /// [`LookMaterial::set_volume`] pack (the field's frame, its disc's reach and emission through
    /// `field::Disc`'s mapping, the march's step and depth) is finite. The shader's footprint and
    /// volume take the potency colour from those lanes; a solid angle that rounds to zero makes
    /// the emission infinite and every colour of the field undefined. Fixture strength and reach.
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
        for ratio in [1.0, top] {
            let spread = ConeShape::shipped()
                .spread(FieldDescriptor::ball(ratio))
                .unwrap();
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
                spread.half_angle(),
                strength,
                0.0,
                rays,
            );
            let emission = potency.emission(strength, cone.half_angle);
            assert!(
                emission.is_finite() && emission > 0.0,
                "λ = {ratio}: emission {emission}"
            );
            let print = FieldPrint {
                origin: cone.origin,
                axis: cone.disc.axis,
                u: cone.disc.u,
                v: cone.disc.v,
                half_angle: cone.disc.half_angle,
                front: cone.max_reach(),
                scale: 2.0,
                nodes: &cone.disc.nodes,
            };
            let mut looks = Assets::<LookMaterial>::default();
            let handle = looks.add(base(&masks));
            set_footprints(&mut looks, &masks, std::slice::from_ref(&print), &potency);
            let mut field = volume(&potency, &masks, CornerKind::Glass, None);
            field.set_volume(&print, &potency);
            for params in [looks.get(&handle).unwrap().params, field.params] {
                let lanes = params.prints[..4]
                    .iter()
                    .chain(&params.disc[..DISC_LANES])
                    .chain([&params.volume]);
                for (k, lane) in lanes.enumerate() {
                    assert!(lane.is_finite(), "λ = {ratio}: lane {k} is {lane}");
                }
            }
        }
    }
}
