//! The glass looks ([`GlassLook`], dev-only): how the see-through hull, the struck components and
//! the spall field are drawn in each. [`LookMaterial`] (`pen_cam_look.wgsl`) draws every look's
//! own elements; [`hull`], [`component`] and [`field`] choose them per look, and the stage spawns
//! what they return.
//!
//! The stacking every look but the baseline removes is cut by two offscreen masks drawn from the
//! corner camera's own pose before it: the hull mask (each pixel's nearest hull surface: view
//! normal and distance) and the part mask (the struck components' colour and distance; in the
//! base look, every component mesh's distance, so the components are one layer deep too). The glass
//! draws only where its fragment is the hull mask's nearest surface, so the hull is one layer deep;
//! the full-screen overlays (feature lines, silhouette) read the hull mask directly, and the soft
//! field shell fades against the part mask. The CAD look's constant-pixel outline is
//! `bevy_mod_outline`'s jump flood.
//!
//! Tiers come from the tank's data alone ([`Tier`]): main armour, external armour, component and
//! detail, by the substance each mesh wears and the tank file's volume entries.

use bevy::camera::visibility::NoFrustumCulling;
use bevy::camera::{Hdr, ImageRenderTarget, RenderTarget};
use bevy::core_pipeline::tonemapping::{DebandDither, Tonemapping};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, CompareFunction, Face, ShaderType, TextureFormat,
};
use bevy::shader::ShaderRef;

use super::material::{Blend, GlowMaterial};
use super::plan::{BoxPlan, Cone};
use super::stage::StageCamera;
use super::timeline::{self, Condition, CornerKind};
use super::view::Tier;
use super::{Corner, DoseScale, Footprint, GlassLook, Settings};
use crate::render_policy::{CameraProfile, VisualScope};

/// Each look's glass colour, linear-converted at use. TUNED, as is every figure below.
const GLASS_GREY: Color = Color::srgb_u8(0x9F, 0xB0, 0xC0);
const HOLO_CYAN: Color = Color::srgb_u8(0x3C, 0xE6, 0xFF);
const LINE_GREY: Color = Color::srgb_u8(0x8C, 0x96, 0xA0);
const SILHOUETTE: Color = Color::srgb_u8(0xD8, 0xE0, 0xE8);
const DITHER_GREY: Color = Color::srgb_u8(0x6A, 0x72, 0x7C);
/// The importance ghost's opacity per tier.
const GHOST_ARMOUR: f32 = 0.15;
const GHOST_RUNNING_GEAR: f32 = 0.04;
const GHOST_COMPONENT: f32 = 0.04;
/// The CAD look's dither coverage and outline width (logical px, `bevy_mod_outline`'s unit).
const DITHER_COVER: f32 = 0.25;
const CAD_OUTLINE_PX: f32 = 2.5;
/// The hologram's scanline period (m, world).
const SCANLINE_M: f32 = 0.12;
/// The ray-marched field's optical depth gain per metre of unit density.
const MARCH_GAIN: f32 = 2.5;
/// The dissolving front's band (m) behind the front.
const DISSOLVE_BAND_M: f32 = 0.35;
/// A struck component's red outline (physical px, an inverted hull), so a sliver of fill still
/// reads as struck; the light's share of its shading; its fill line's width (px); the dead hatch's
/// period (px) and the share of it in red.
const STATE_OUTLINE_PX: f32 = 2.0;
const STATE_CONTRAST: f32 = 0.3;
const STATE_LINE_PX: f32 = 1.5;
const STATE_HATCH_PX: f32 = 7.0;
const STATE_HATCH_SHARE: f32 = 0.3;
/// The base look's surface: a hueless grey, its opacity at a grazing angle and the share of that
/// kept face-on, the fresnel power between them, the light's share of the shading (the rest is
/// flat), and its edge colour and opacity. It keeps one opacity everywhere: a struck component
/// shows through it as every other component does.
const BASE_GREY: Color = Color::srgb_u8(0xA4, 0xA4, 0xA4);
const BASE_ALPHA: f32 = 0.6;
const BASE_FACE_SHARE: f32 = 0.4;
const BASE_FRESNEL_POWER: f32 = 2.0;
const BASE_CONTRAST: f32 = 0.35;
const BASE_EDGE: Color = Color::srgb_u8(0xE6, 0xE6, 0xE6);
const BASE_EDGE_ALPHA: f32 = 0.35;
/// The base look's components: the state scale's untouched grey, fuller than the hull, one layer
/// deep among themselves.
const BASE_COMPONENT_ALPHA: f32 = 0.55;
const BASE_COMPONENT_FACE_SHARE: f32 = 0.6;
const BASE_COMPONENT_EDGE_ALPHA: f32 = 0.4;
/// The field footprint's outline width and hatch period (px), the hatched share of the period,
/// and its opacity, in the field's hue.
const FOOTPRINT_LINE_PX: f32 = 2.0;
const FOOTPRINT_HATCH_PX: f32 = 5.0;
const FOOTPRINT_HATCH_SHARE: f32 = 0.4;
const FOOTPRINT_ALPHA: f32 = 0.9;
/// The most spall fields a footprint marks, and the most directions of each it reads.
pub const FOOTPRINT_FIELDS: usize = 4;
pub const FOOTPRINT_RAYS: usize = 32;
/// A surface counts as met by a direction up to this share past its reach, and this far (m): the
/// record ends a direction on the surface that stopped it.
const FOOTPRINT_REACH_SLACK: (f32, f32) = (1.03, 0.03);
/// The most struck components a look clears around.
const HITS: usize = 8;

/// The uniform block `pen_cam_look.wgsl` reads; its lanes are documented there.
#[derive(ShaderType, Debug, Clone, Copy)]
pub struct LookParams {
    pub color: Vec4,
    pub edge: Vec4,
    pub shade: Vec4,
    pub field: Vec4,
    pub cone: Vec4,
    pub cone2: Vec4,
    pub mode: Vec4,
    pub hits: [Vec4; HITS],
    pub state: Vec4,
    pub span: Vec4,
    pub rest: Vec4,
    pub foot: Vec4,
    pub foot_color: Vec4,
    pub prints: [Vec4; 3 * FOOTPRINT_FIELDS],
    pub rays: [Vec4; FOOTPRINT_FIELDS * FOOTPRINT_RAYS],
}

impl Default for LookParams {
    fn default() -> Self {
        Self {
            color: Vec4::ZERO,
            edge: Vec4::ZERO,
            shade: Vec4::ZERO,
            field: Vec4::ZERO,
            cone: Vec4::ZERO,
            cone2: Vec4::ZERO,
            mode: Vec4::ZERO,
            hits: [Vec4::ZERO; HITS],
            state: Vec4::ZERO,
            span: Vec4::ZERO,
            rest: Vec4::ZERO,
            foot: Vec4::ZERO,
            foot_color: Vec4::ZERO,
            prints: [Vec4::ZERO; 3 * FOOTPRINT_FIELDS],
            rays: [Vec4::ZERO; FOOTPRINT_FIELDS * FOOTPRINT_RAYS],
        }
    }
}

/// What a look element's pipeline tests depth against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Depth {
    /// The alpha mode's own test and write.
    Normal,
    /// Always passes and writes nothing: a full-screen overlay.
    Always,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LookKey {
    depth: Depth,
    cull_front: bool,
    bias: bool,
}

impl From<&LookMaterial> for LookKey {
    fn from(material: &LookMaterial) -> Self {
        Self {
            depth: material.depth,
            cull_front: material.cull_front,
            bias: material.bias,
        }
    }
}

const MODE_HULL_MASK: f32 = 0.0;
const MODE_PART_MASK: f32 = 1.0;
const MODE_GLASS: f32 = 2.0;
const MODE_DITHER: f32 = 3.0;
const MODE_DISSOLVE: f32 = 6.0;
const MODE_MARCH: f32 = 7.0;
const MODE_SOFT: f32 = 8.0;
const MODE_BASE: f32 = 9.0;
const MODE_LINES: f32 = 10.0;
const MODE_SILHOUETTE: f32 = 11.0;
const MODE_STATE: f32 = 12.0;
/// A mode the shader draws nothing in.
const MODE_NONE: f32 = 99.0;

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
    pub depth: Depth,
    pub cull_front: bool,
    /// The depth bias a copy of a mesh needs to win the tie with the mesh it covers.
    pub bias: bool,
}

impl LookMaterial {
    fn new(mode: f32, color: LinearRgba, alpha: f32, blend: Blend, masks: &Masks) -> Self {
        Self {
            params: LookParams {
                color: Vec4::new(color.red, color.green, color.blue, alpha),
                shade: Vec4::new(1.0, 0.0, 1.0, 1.0),
                field: Vec4::new(-1.0, 0.0, 0.0, 0.0),
                cone2: Vec4::new(0.0, (blend == Blend::Additive) as u8 as f32, 0.0, 0.0),
                mode: Vec4::new(mode, 0.0, 0.0, 0.0),
                ..default()
            },
            hull_mask: Some(masks.hull.clone()),
            part_mask: Some(masks.part.clone()),
            blend,
            depth: Depth::Normal,
            cull_front: false,
            bias: false,
        }
    }

    fn with_rim(mut self, base: f32, gain: f32, power: f32) -> Self {
        self.params.shade.x = base;
        self.params.shade.y = gain;
        self.params.shade.z = power;
        self
    }

    fn with_falloff(mut self, r0: f32) -> Self {
        self.params.field.y = r0;
        self
    }

    pub fn set_fade(&mut self, fade: f32) {
        self.params.shade.w = fade;
    }

    pub fn set_front(&mut self, radius: Option<f32>) {
        self.params.field.x = radius.map_or(-1.0, |r| r.max(0.0));
    }

    /// A state fill's frame: `up` (world, unit) and its component's low and high heights along it
    /// (world m), `None` where the component has no known shape and its fill covers it whole.
    pub fn set_fill_span(&mut self, up: Vec3, heights: Option<(f32, f32)>) {
        self.params.state = up.extend(self.params.state.w);
        self.params.span = match heights {
            Some((lo, hi)) => Vec4::new(lo, hi, 1.0, STATE_LINE_PX),
            None => Vec4::new(0.0, 0.0, 0.0, STATE_LINE_PX),
        };
    }

    fn uses_hits(&self) -> bool {
        self.params.mode.y > 0.5
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
            Blend::Additive => AlphaMode::Add,
            Blend::Blended => AlphaMode::Blend,
            Blend::Opaque => AlphaMode::Opaque,
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
        descriptor.primitive.cull_mode = key.cull_front.then_some(Face::Front);
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            if key.bias {
                // Toward the eye under bevy's reverse Z, as `GlowMaterial`'s.
                depth.bias.constant = 8;
                depth.bias.slope_scale = 1.0;
            }
            match key.depth {
                Depth::Normal => {}
                Depth::Always => {
                    depth.depth_compare = Some(CompareFunction::Always);
                    depth.depth_write_enabled = Some(false);
                }
            }
        }
        Ok(())
    }
}

/// The two mask images the mask cameras draw, at the corner image's size.
#[derive(Resource, Default)]
pub struct Masks {
    pub hull: Handle<Image>,
    pub part: Handle<Image>,
    size: UVec2,
}

#[derive(Component)]
pub(super) struct MaskCamera;

/// A struck component's fill in a look that clears the glass around it.
#[derive(Component)]
pub(super) struct HitMark;

/// The full-screen overlay quad, a child of the corner camera.
#[derive(Component)]
pub(super) struct Overlay;

fn mask_image(images: &mut Assets<Image>, size: UVec2) -> Handle<Image> {
    images.add(Image::new_target_texture(
        size.x,
        size.y,
        TextureFormat::Rgba16Float,
        None,
    ))
}

pub fn spawn(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let size = UVec2::new(16, 10);
    let masks = Masks {
        hull: mask_image(&mut images, size),
        part: mask_image(&mut images, size),
        size,
    };
    for (order, image, profile) in [
        (-3, masks.hull.clone(), CameraProfile::PenCamHullMask),
        (-2, masks.part.clone(), CameraProfile::PenCamPartMask),
    ] {
        commands.spawn((
            Camera3d::default(),
            Camera {
                order,
                is_active: false,
                clear_color: ClearColorConfig::Custom(Color::NONE),
                ..default()
            },
            RenderTarget::Image(image.into()),
            Hdr,
            Tonemapping::None,
            DebandDither::Disabled,
            Msaa::Off,
            Projection::Perspective(default()),
            Transform::IDENTITY,
            profile,
            MaskCamera,
        ));
    }
    commands.insert_resource(masks);
}

/// Keep the masks at the corner image's size; a new size rebuilds the stage, whose materials hold
/// the mask handles.
pub fn layout(
    corner_size: Option<Res<super::hud::CornerImages>>,
    mut masks: ResMut<Masks>,
    mut images: ResMut<Assets<Image>>,
    mut cameras: Query<(&mut RenderTarget, &CameraProfile), With<MaskCamera>>,
    mut stage: ResMut<super::stage::Stage>,
) {
    let Some(corner_size) = corner_size else {
        return;
    };
    let (size, scale) = corner_size.size_and_scale();
    if size == masks.size || size.x == 0 {
        return;
    }
    masks.size = size;
    masks.hull = mask_image(&mut images, size);
    masks.part = mask_image(&mut images, size);
    for (mut target, profile) in &mut cameras {
        let handle = match profile {
            CameraProfile::PenCamHullMask => masks.hull.clone(),
            _ => masks.part.clone(),
        };
        *target = RenderTarget::Image(ImageRenderTarget {
            handle,
            scale_factor: scale,
        });
    }
    stage.invalidate();
}

/// Pose the mask cameras as the corner camera, run them only while a look that reads them shows,
/// and update the looks' per-frame lanes: the corner clock and the struck components' clearings.
pub fn sync(
    corner: Res<Corner>,
    stage: Res<super::stage::Stage>,
    main: Query<
        (&Camera, &Transform, &Projection, &GlobalTransform),
        (With<StageCamera>, Without<MaskCamera>),
    >,
    mut masks: Query<
        (&mut Camera, &mut Transform, &mut Projection),
        (With<MaskCamera>, Without<StageCamera>),
    >,
    hits: Query<
        (
            &GlobalTransform,
            &bevy::camera::primitives::Aabb,
            &Visibility,
        ),
        With<HitMark>,
    >,
    mut looks: ResMut<Assets<LookMaterial>>,
) {
    let Ok((camera, transform, projection, global)) = main.single() else {
        return;
    };
    let look = stage.look();
    let active = camera.is_active && look != GlassLook::Baseline;
    for (mut mask, mut pose, mut lens) in &mut masks {
        if mask.is_active != active {
            mask.is_active = active;
        }
        if *pose != *transform {
            *pose = *transform;
        }
        *lens = projection.clone();
    }
    if !active {
        return;
    }
    let mut marks = [Vec4::ZERO; HITS];
    let right = global.right();
    for (slot, (mark, aabb, _)) in marks.iter_mut().zip(
        hits.iter()
            .filter(|(_, _, visibility)| **visibility != Visibility::Hidden),
    ) {
        let center = mark.transform_point(Vec3::from(aabb.center));
        let radius = (mark.affine().matrix3 * aabb.half_extents).length();
        let (Some(a), Some(b)) = (
            camera.world_to_ndc(global, center),
            camera.world_to_ndc(global, center + *right * radius),
        ) else {
            continue;
        };
        if a.z < 0.0 || a.z > 1.0 {
            continue;
        }
        *slot = Vec4::new(
            a.x,
            a.y,
            (b.truncate() - a.truncate()).length().max(0.05),
            1.0,
        );
    }
    let c = corner.showing.map_or(0.0, |showing| showing.c);
    for (_, material) in looks.iter_mut() {
        material.params.field.z = c;
        if material.uses_hits() {
            material.params.hits = marks;
        }
    }
}

/// One spall field where a footprint is taken, in world space: its origin, unit axis, half-angle
/// (rad), reach (m, its longest direction's), how far its front has run (m, negative before it is
/// thrown) and its recorded directions (unit) with each one's reach (m).
#[derive(Clone, Debug)]
pub struct FieldPrint {
    pub origin: Vec3,
    pub axis: Vec3,
    pub half_angle: f32,
    pub reach: f32,
    pub front: f32,
    pub rays: Vec<(Vec3, f32)>,
}

/// Mark `prints` on every surface the base look draws, the hull's and the components', in
/// `style`; nothing in any other look or with the footprint off. A surface is marked where it
/// faces a field's origin, inside its cone, behind its front and no farther than the reach of the
/// recorded direction nearest it: the field met it there, which says nothing of what stopped.
pub fn set_footprints(
    looks: &mut Assets<LookMaterial>,
    look: GlassLook,
    style: Footprint,
    prints: &[FieldPrint],
) {
    let style = match (look, style) {
        (GlassLook::Base, Footprint::Outline) => 1.0,
        (GlassLook::Base, Footprint::Hatch) => 2.0,
        _ => 0.0,
    };
    let count = prints.len().min(FOOTPRINT_FIELDS);
    let mut lanes = [Vec4::ZERO; 3 * FOOTPRINT_FIELDS];
    let mut rays = [Vec4::ZERO; FOOTPRINT_FIELDS * FOOTPRINT_RAYS];
    let (share, metres) = FOOTPRINT_REACH_SLACK;
    for (i, print) in prints.iter().take(count).enumerate() {
        let n = print.rays.len().min(FOOTPRINT_RAYS);
        lanes[3 * i] = print.origin.extend(print.half_angle);
        lanes[3 * i + 1] = print.axis.extend(print.reach);
        lanes[3 * i + 2] = Vec4::new(print.front, n as f32, 0.0, 0.0);
        for (j, (direction, reach)) in print.rays.iter().take(n).enumerate() {
            rays[i * FOOTPRINT_RAYS + j] = direction.extend(reach * share + metres);
        }
    }
    let hue = LinearRgba::from(timeline::FIELD);
    for (_, material) in looks.iter_mut() {
        let mode = material.params.mode.x;
        if mode != MODE_BASE && mode != MODE_STATE {
            continue;
        }
        material.params.foot =
            Vec4::new(style, count as f32, FOOTPRINT_LINE_PX, FOOTPRINT_HATCH_PX);
        material.params.foot_color = Vec4::new(hue.red, hue.green, hue.blue, FOOTPRINT_ALPHA);
        material.params.prints = lanes;
        material.params.rays = rays;
        material.params.cone2.z = FOOTPRINT_HATCH_SHARE;
    }
}

/// How a hull mesh is drawn in a look: its material (one that draws nothing where the look drops
/// the tier) and the mask that holds it, if any.
pub struct HullDraw {
    pub material: LookMaterial,
    pub mask: Option<MaskLayer>,
}

/// The offscreen mask a hull mesh is drawn into, for the nearest-surface test of its own layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaskLayer {
    Hull,
    /// The part mask: in the base look, every component mesh.
    Part,
}

/// How a hull mesh of tier `tier` is drawn in `look`; `None` for the baseline, which the stage
/// draws itself.
pub fn hull(look: GlassLook, tier: Tier, masks: &Masks) -> Option<HullDraw> {
    let none = || LookMaterial::new(MODE_NONE, LinearRgba::BLACK, 0.0, Blend::Blended, masks);
    let glass = |color: Color, alpha: f32, blend| {
        LookMaterial::new(MODE_GLASS, color.into(), alpha, blend, masks)
    };
    let draw = |material, mask| Some(HullDraw { material, mask });
    match (look, tier) {
        (GlassLook::Baseline, _) => None,
        // Feature lines: the armour and running gear in the mask, nothing in the picture.
        (GlassLook::Technical, Tier::Armour | Tier::External) => {
            draw(none(), Some(MaskLayer::Hull))
        }
        (GlassLook::Envelope, Tier::Armour) => draw(
            glass(GLASS_GREY, 1.0, Blend::Additive).with_rim(0.04, 0.6, 2.5),
            Some(MaskLayer::Hull),
        ),
        (GlassLook::Ghost, Tier::Armour | Tier::External | Tier::Component) => {
            let alpha = match tier {
                Tier::Armour => GHOST_ARMOUR,
                Tier::External => GHOST_RUNNING_GEAR,
                _ => GHOST_COMPONENT,
            };
            let mut material = glass(GLASS_GREY, alpha, Blend::Blended).with_rim(1.0, 1.5, 3.0);
            material.params.mode.y = 1.0;
            draw(material, Some(MaskLayer::Hull))
        }
        (GlassLook::Cad, Tier::Armour | Tier::External) => {
            let material = LookMaterial::new(
                MODE_DITHER,
                DITHER_GREY.into(),
                DITHER_COVER,
                Blend::Opaque,
                masks,
            );
            draw(material, None)
        }
        (GlassLook::Hologram, Tier::Armour) => {
            let mut material = glass(HOLO_CYAN, 1.0, Blend::Additive).with_rim(0.04, 0.6, 2.0);
            material.params.field.w = SCANLINE_M;
            draw(material, Some(MaskLayer::Hull))
        }
        (GlassLook::Silhouette, Tier::Armour | Tier::External | Tier::Detail) => {
            draw(none(), Some(MaskLayer::Hull))
        }
        (GlassLook::Base, Tier::Armour | Tier::External | Tier::Detail) => {
            draw(base(masks), Some(MaskLayer::Hull))
        }
        (GlassLook::Base, Tier::Component) => draw(component_base(masks), Some(MaskLayer::Part)),
        _ => draw(none(), None),
    }
}

/// The base look's one material for every hull part it draws: armour, running gear and detail.
fn base(masks: &Masks) -> LookMaterial {
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

/// The base look's material for every component mesh: the hull's surface darkened, tested
/// against the part mask. A struck component's fill covers it.
fn component_base(masks: &Masks) -> LookMaterial {
    let mut material = base(masks);
    let grey = LinearRgba::from(timeline::STATE_UNTOUCHED);
    material.params.color = Vec4::new(grey.red, grey.green, grey.blue, BASE_COMPONENT_ALPHA);
    material.params.shade.x = BASE_COMPONENT_FACE_SHARE;
    material.params.edge.w = BASE_COMPONENT_EDGE_ALPHA;
    material.params.mode.z = 1.0;
    material
}

/// The hull mask's material, drawn into either mask.
pub fn hull_mask(masks: &Masks) -> LookMaterial {
    LookMaterial::new(MODE_HULL_MASK, LinearRgba::BLACK, 1.0, Blend::Opaque, masks)
}

/// The full-screen overlay of `look`, if it has one.
pub fn overlay(look: GlassLook, kind: CornerKind, masks: &Masks) -> Option<LookMaterial> {
    let glass = matches!(kind, CornerKind::Glass | CornerKind::Victim);
    let mut material = match look {
        GlassLook::Technical if glass => {
            LookMaterial::new(MODE_LINES, LINE_GREY.into(), 0.9, Blend::Blended, masks)
        }
        GlassLook::Silhouette if glass => LookMaterial::new(
            MODE_SILHOUETTE,
            SILHOUETTE.into(),
            0.95,
            Blend::Blended,
            masks,
        ),
        _ => return None,
    };
    material.depth = Depth::Always;
    Some(material)
}

/// Spawn `look`'s overlay quad under the corner camera.
pub fn spawn_overlay(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    material: Handle<LookMaterial>,
    camera: Entity,
) -> Entity {
    commands
        .spawn((
            Mesh3d(meshes.add(Rectangle::new(2.0, 2.0))),
            MeshMaterial3d(material),
            Transform::from_xyz(0.0, 0.0, -0.05),
            NoFrustumCulling,
            VisualScope::PEN_CAM_STAGE,
            Overlay,
            ChildOf(camera),
        ))
        .id()
}

/// One element a component is drawn with in a look.
pub enum PartDraw {
    Look(LookMaterial),
    /// A fill with a jump-flood outline of a colour and width (logical px) about its silhouette.
    Flooded(LookMaterial, LinearRgba, f32),
    Outline(LinearRgba, f32),
    /// The part mask's copy.
    Mask(LookMaterial),
}

/// The share of its hit points component `b` lost, on the demo's dose scale.
fn share(b: &BoxPlan, settings: &Settings) -> f32 {
    match settings.dose {
        DoseScale::HpShare => timeline::hp_share(b.dose(), b.hp),
        DoseScale::Absolute => timeline::absolute_dose(b.dose()),
    }
}

/// Component `b`'s place on the state scale in frame `kind`, `None` where it is not drawn: the
/// shooter's views (the glass and the outside view) show what this shot did to a component it
/// dosed, the share of the hit points it had at the shot (or the dev absolute scale); the victim's
/// glass shows its own tank's state, the share of each pool gone.
pub fn condition(b: &BoxPlan, kind: CornerKind, settings: &Settings) -> Option<Condition> {
    match kind {
        CornerKind::Glass | CornerKind::Damaged if b.dose() > 0.0 => {
            Condition::of(b.lost, share(b, settings))
        }
        CornerKind::Victim => Condition::of(b.lost, b.pool_lost()),
        _ => None,
    }
}

/// The state scale's opaque fill for `condition`: the struck share of the component's height in
/// red from the bottom up, the rest in the untouched grey, a crisp lighter line between them; a
/// dead component charcoal under a red hatch. Its frame is set each frame
/// ([`LookMaterial::set_fill_span`]) and its fade raises the fill.
pub fn state(condition: Condition, masks: &Masks) -> LookMaterial {
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
    m.params.mode.z = STATE_HATCH_PX;
    m.params.cone2.w = STATE_HATCH_SHARE;
    m
}

/// How component `b` is drawn in `look` and frame `kind`; empty where it is not drawn. Every look
/// draws it on the one state scale ([`state`]) under a red outline; the CAD look's outline is its
/// jump flood, and the silhouette look adds the part mask's copy. The stage marks the first element
/// [`HitMark`].
pub fn component(
    look: GlassLook,
    b: &BoxPlan,
    kind: CornerKind,
    settings: &Settings,
    masks: &Masks,
) -> Vec<PartDraw> {
    condition(b, kind, settings).map_or_else(Vec::new, |condition| draws(look, condition, masks))
}

/// The elements a component in `condition` is drawn with in `look`.
fn draws(look: GlassLook, condition: Condition, masks: &Masks) -> Vec<PartDraw> {
    let fill = state(condition, masks);
    let red = LinearRgba::from(timeline::STATE_RED);
    match look {
        GlassLook::Cad => vec![PartDraw::Flooded(fill, red, CAD_OUTLINE_PX)],
        GlassLook::Silhouette => vec![
            PartDraw::Look(fill),
            PartDraw::Outline(red, STATE_OUTLINE_PX),
            PartDraw::Mask(LookMaterial::new(
                MODE_PART_MASK,
                red,
                1.0,
                Blend::Opaque,
                masks,
            )),
        ],
        _ => vec![
            PartDraw::Look(fill),
            PartDraw::Outline(red, STATE_OUTLINE_PX),
        ],
    }
}

/// The one surface a cone's field is drawn with in a look, in the stage's material or the look's.
pub enum FieldDraw {
    Glow(GlowMaterial),
    Look(Box<LookMaterial>),
}

/// How `cone`'s field is drawn in `look`, `None` for the stage's own nested shells. `strength` is
/// the field's absolute dose in `0..=1` and `r0` its falloff radius (m).
pub fn field(
    look: GlassLook,
    cone: &Cone,
    strength: f32,
    r0: f32,
    masks: &Masks,
) -> Option<FieldDraw> {
    let hue: LinearRgba = timeline::FIELD.into();
    let march = |blend| {
        let mut m = LookMaterial::new(MODE_MARCH, hue, 0.9, blend, masks).with_falloff(r0);
        m.params.cone = cone.axis.extend(cone.half_angle);
        m.params.cone2.x = MARCH_GAIN * (0.4 + 0.6 * strength);
        m.params.cone2.z = cone.rays.iter().map(|ray| ray.reach).fold(0.0, f32::max);
        m.cull_front = true;
        m
    };
    Some(match look {
        GlassLook::Baseline | GlassLook::Envelope => return None,
        GlassLook::Technical => FieldDraw::Look(Box::new(march(Blend::Blended))),
        GlassLook::Hologram => {
            let mut m = march(Blend::Additive);
            m.params.color.w = 2.0;
            FieldDraw::Look(Box::new(m))
        }
        GlassLook::Ghost | GlassLook::Base => {
            let mut m = LookMaterial::new(
                MODE_DISSOLVE,
                hue,
                0.55 * (0.4 + 0.6 * strength),
                Blend::Blended,
                masks,
            )
            .with_falloff(r0);
            m.params.cone2.z = DISSOLVE_BAND_M;
            FieldDraw::Look(Box::new(m))
        }
        GlassLook::Cad => FieldDraw::Glow(
            GlowMaterial::blended(hue, 0.12 * (0.4 + 0.6 * strength))
                .with_edge(timeline::FIELD_EDGE.into(), 1.0, 4.0)
                .with_falloff(r0),
        ),
        GlassLook::Silhouette => FieldDraw::Look(Box::new(
            LookMaterial::new(
                MODE_SOFT,
                hue,
                0.7 * (0.4 + 0.6 * strength),
                Blend::Blended,
                masks,
            )
            .with_falloff(r0),
        )),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_draw(tier: Tier) -> HullDraw {
        hull(GlassLook::Base, tier, &Masks::default()).expect("the base draws every tier")
    }

    /// The base look draws every non-component tier with one material in the hull mask, at one
    /// opacity everywhere: no clearing over the struck components.
    #[test]
    fn the_base_draws_every_hull_tier_alike() {
        let first = base_draw(Tier::Armour).material.params;
        for tier in [Tier::Armour, Tier::External, Tier::Detail] {
            let draw = base_draw(tier);
            assert_eq!(draw.mask, Some(MaskLayer::Hull));
            assert_eq!(draw.material.blend, Blend::Blended);
            assert!(!draw.material.uses_hits());
            let p = draw.material.params;
            assert_eq!(
                (p.mode, p.color, p.edge, p.shade, p.cone2),
                (
                    first.mode,
                    first.color,
                    first.edge,
                    first.shade,
                    first.cone2
                )
            );
            assert_eq!(p.mode.x, MODE_BASE);
        }
    }

    /// Every component mesh draws in the base's family, hueless and HP-blind, one layer deep among
    /// the components and outside the hull's layer.
    #[test]
    fn the_base_draws_every_component_grey_in_its_own_layer() {
        let draw = base_draw(Tier::Component);
        assert_eq!(draw.mask, Some(MaskLayer::Part));
        assert_eq!(draw.material.params.mode.x, MODE_BASE);
        assert!(!draw.material.uses_hits());
        let c = draw.material.params.color;
        assert!(
            c.x == c.y && c.y == c.z,
            "a component's base carries no hue"
        );
        let grey = LinearRgba::from(timeline::STATE_UNTOUCHED);
        assert_eq!(c.truncate(), Vec3::new(grey.red, grey.green, grey.blue));
    }

    /// Every look draws a struck component on the same state fill: its share as the fill height,
    /// red when damaged, hatched when dead, under a red outline.
    #[test]
    fn every_look_draws_the_one_state_scale() {
        let masks = Masks::default();
        let red = LinearRgba::from(timeline::STATE_RED);
        for condition in [Condition::Damaged(0.05), Condition::Dead] {
            let m = state(condition, &masks);
            assert_eq!(m.params.mode.x, MODE_STATE);
            assert_eq!(m.blend, Blend::Opaque);
            assert_eq!(m.params.state.w, condition.fill());
            assert_eq!(m.params.mode.w > 0.5, condition == Condition::Dead);
        }
        let mut fill = state(Condition::Damaged(0.4), &masks);
        fill.set_fill_span(Vec3::Y, Some((1.0, 2.0)));
        assert_eq!(fill.params.state, Vec4::new(0.0, 1.0, 0.0, 0.4));
        assert_eq!(fill.params.span.truncate(), Vec3::new(1.0, 2.0, 1.0));
        for look in GlassLook::ALL {
            let first = |draws: Vec<PartDraw>| match draws.into_iter().next() {
                Some(PartDraw::Look(m)) | Some(PartDraw::Flooded(m, _, _)) => m.params,
                _ => panic!("{look:?} draws no fill first"),
            };
            let (dead, damaged) = (
                state(Condition::Dead, &masks),
                state(Condition::Damaged(0.4), &masks),
            );
            for (want, condition) in [(dead, Condition::Dead), (damaged, Condition::Damaged(0.4))] {
                let draws = draws(look, condition, &masks);
                let outlined = draws.iter().any(|draw| match draw {
                    PartDraw::Outline(color, _) | PartDraw::Flooded(_, color, _) => *color == red,
                    _ => false,
                });
                assert!(outlined, "{look:?}: no red outline");
                let got = first(draws);
                assert_eq!(
                    (got.mode, got.color, got.rest, got.state),
                    (
                        want.params.mode,
                        want.params.color,
                        want.params.rest,
                        want.params.state
                    ),
                    "{look:?}"
                );
            }
        }
    }

    /// The footprint marks only the base look's surfaces, hull and components alike, with every
    /// field's frame and directions; any other look, or the footprint off, marks nothing.
    #[test]
    fn the_footprint_marks_only_the_base_looks_surfaces() {
        let masks = Masks::default();
        let mut looks = Assets::<LookMaterial>::default();
        let base = looks.add(base(&masks));
        let component = looks.add(component_base(&masks));
        let fill = looks.add(state(Condition::Damaged(0.5), &masks));
        let glass = looks.add(
            hull(GlassLook::Ghost, Tier::Armour, &masks)
                .unwrap()
                .material,
        );
        let print = FieldPrint {
            origin: Vec3::ZERO,
            axis: Vec3::NEG_Z,
            half_angle: 0.5,
            reach: 2.0,
            front: 1.0,
            rays: vec![
                (Vec3::NEG_Z, 2.0),
                (Vec3::new(0.0, 0.4, -0.9).normalize(), 1.5),
            ],
        };
        let style = |looks: &Assets<LookMaterial>, handle: &Handle<LookMaterial>| {
            looks.get(handle).unwrap().params.foot.x
        };
        set_footprints(
            &mut looks,
            GlassLook::Base,
            Footprint::Hatch,
            std::slice::from_ref(&print),
        );
        for handle in [&base, &component, &fill] {
            assert_eq!(style(&looks, handle), 2.0);
            let p = looks.get(handle).unwrap().params;
            assert_eq!(p.foot.y, 1.0);
            assert_eq!(p.prints[0], Vec4::new(0.0, 0.0, 0.0, 0.5));
            assert_eq!(p.prints[2].y, 2.0, "both directions");
            assert!(p.rays[1].w > 1.5, "a direction's reach holds its slack");
        }
        assert_eq!(style(&looks, &glass), 0.0);
        set_footprints(
            &mut looks,
            GlassLook::Ghost,
            Footprint::Hatch,
            std::slice::from_ref(&print),
        );
        assert_eq!(style(&looks, &base), 0.0);
        set_footprints(&mut looks, GlassLook::Base, Footprint::Off, &[print]);
        assert_eq!(style(&looks, &fill), 0.0);
    }
}
