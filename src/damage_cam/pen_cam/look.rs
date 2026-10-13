//! The glass looks ([`GlassLook`], dev-only): how the see-through hull, the struck components and
//! the spall field are drawn in each. [`LookMaterial`] (`pen_cam_look.wgsl`) draws every look's
//! own elements; [`hull`], [`component`] and [`field`] choose them per look, and the stage spawns
//! what they return.
//!
//! The stacking every look but the baseline removes is cut by two offscreen masks drawn from the
//! corner camera's own pose before it: the hull mask (each pixel's nearest hull surface: view
//! normal and distance) and the part mask (the struck components' colour and distance). The glass
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
use super::timeline::{self, CornerKind};
use super::view::Tier;
use super::{Corner, DoseScale, GlassLook, Settings};
use crate::render_policy::{CameraProfile, VisualScope};

/// Each look's glass colour, linear-converted at use. TUNED, as is every figure below.
const GLASS_GREY: Color = Color::srgb_u8(0x9F, 0xB0, 0xC0);
const HOLO_CYAN: Color = Color::srgb_u8(0x3C, 0xE6, 0xFF);
const LINE_GREY: Color = Color::srgb_u8(0x8C, 0x96, 0xA0);
const SILHOUETTE: Color = Color::srgb_u8(0xD8, 0xE0, 0xE8);
const DITHER_GREY: Color = Color::srgb_u8(0x6A, 0x72, 0x7C);
/// The hologram's HP ramp ends: amber at a scratch, red at a destroyed component.
const HOLO_AMBER: Color = Color::srgb_u8(0xFF, 0xB0, 0x20);
const HOLO_RED: Color = Color::srgb_u8(0xFF, 0x28, 0x10);
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
/// The part outline's width (physical px) in the looks that draw it as an inverted hull.
const OUTLINE_PX: f32 = 3.0;
/// The most struck components the ghost clears around.
const HITS: usize = 8;

/// The uniform block `pen_cam_look.wgsl` reads; its lanes are documented there.
#[derive(ShaderType, Debug, Clone, Copy, Default)]
pub struct LookParams {
    pub color: Vec4,
    pub edge: Vec4,
    pub shade: Vec4,
    pub field: Vec4,
    pub cone: Vec4,
    pub cone2: Vec4,
    pub mode: Vec4,
    pub hits: [Vec4; HITS],
}

/// What a look element's pipeline tests depth against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Depth {
    /// The alpha mode's own test and write.
    Normal,
    /// Passes only behind something nearer: the hidden part of a mesh. Writes nothing.
    Hidden,
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
const MODE_FILL: f32 = 4.0;
const MODE_HIDDEN: f32 = 5.0;
const MODE_DISSOLVE: f32 = 6.0;
const MODE_MARCH: f32 = 7.0;
const MODE_SOFT: f32 = 8.0;
const MODE_LINES: f32 = 10.0;
const MODE_SILHOUETTE: f32 = 11.0;
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

    fn with_edge(mut self, color: LinearRgba, gain: f32, power: f32) -> Self {
        self.params.edge = Vec4::new(color.red, color.green, color.blue, 1.0);
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
                Depth::Hidden => {
                    depth.depth_compare = Some(CompareFunction::Less);
                    depth.depth_write_enabled = Some(false);
                }
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

/// How a hull mesh of tier `tier` is drawn in `look`: its own material (`None` for the baseline,
/// which the stage draws) and whether the hull mask holds it.
pub struct HullDraw {
    pub material: LookMaterial,
    pub masked: bool,
}

/// The hull materials of `look` for each tier, built once per stage.
pub fn hull(look: GlassLook, tier: Tier, masks: &Masks) -> Option<HullDraw> {
    let none = || LookMaterial::new(MODE_NONE, LinearRgba::BLACK, 0.0, Blend::Blended, masks);
    let glass = |color: Color, alpha: f32, blend| {
        LookMaterial::new(MODE_GLASS, color.into(), alpha, blend, masks)
    };
    let draw = |material, masked| Some(HullDraw { material, masked });
    match (look, tier) {
        (GlassLook::Baseline, _) => None,
        // Feature lines: the armour and running gear in the mask, nothing in the picture.
        (GlassLook::Technical, Tier::Armour | Tier::External) => draw(none(), true),
        (GlassLook::Envelope, Tier::Armour) => draw(
            glass(GLASS_GREY, 1.0, Blend::Additive).with_rim(0.04, 0.6, 2.5),
            true,
        ),
        (GlassLook::Ghost, Tier::Armour | Tier::External | Tier::Component) => {
            let alpha = match tier {
                Tier::Armour => GHOST_ARMOUR,
                Tier::External => GHOST_RUNNING_GEAR,
                _ => GHOST_COMPONENT,
            };
            let mut material = glass(GLASS_GREY, alpha, Blend::Blended).with_rim(1.0, 1.5, 3.0);
            material.params.mode.y = 1.0;
            draw(material, true)
        }
        (GlassLook::Cad, Tier::Armour | Tier::External) => {
            let material = LookMaterial::new(
                MODE_DITHER,
                DITHER_GREY.into(),
                DITHER_COVER,
                Blend::Opaque,
                masks,
            );
            draw(material, false)
        }
        (GlassLook::Hologram, Tier::Armour) => {
            let mut material = glass(HOLO_CYAN, 1.0, Blend::Additive).with_rim(0.04, 0.6, 2.0);
            material.params.field.w = SCANLINE_M;
            draw(material, true)
        }
        (GlassLook::Silhouette, Tier::Armour | Tier::External | Tier::Detail) => draw(none(), true),
        _ => draw(none(), false),
    }
}

/// The hull mask's material.
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

/// How component `b` is drawn in `look` and frame `kind`; empty where it is not drawn. `HitMark`
/// goes on the first element.
pub fn component(
    look: GlassLook,
    b: &BoxPlan,
    kind: CornerKind,
    settings: &Settings,
    masks: &Masks,
) -> Vec<PartDraw> {
    if !matches!(kind, CornerKind::Glass | CornerKind::Damaged) || b.dose() <= 0.0 {
        return Vec::new();
    }
    let x = if b.lost { 1.0 } else { share(b, settings) };
    let ramp = timeline::dose_ramp(x);
    let fill = |color: LinearRgba| {
        let mut m = LookMaterial::new(MODE_FILL, color, 1.0, Blend::Opaque, masks);
        m.bias = true;
        m
    };
    let mask = |color: LinearRgba| {
        PartDraw::Mask(LookMaterial::new(
            MODE_PART_MASK,
            color,
            1.0,
            Blend::Opaque,
            masks,
        ))
    };
    match look {
        GlassLook::Baseline => Vec::new(),
        GlassLook::Technical => {
            let mut m = fill(ramp);
            m.params.cone2.w = 1.0;
            m.params.mode.w = b.lost as u8 as f32;
            vec![PartDraw::Look(m), PartDraw::Outline(LinearRgba::BLACK, 2.0)]
        }
        GlassLook::Envelope => vec![
            PartDraw::Look(fill(ramp)),
            PartDraw::Outline(timeline::VOLUME_EDGE.into(), OUTLINE_PX),
        ],
        GlassLook::Ghost => {
            let mut hidden = LookMaterial::new(MODE_HIDDEN, ramp, 0.35, Blend::Blended, masks);
            hidden.depth = Depth::Hidden;
            hidden.bias = true;
            vec![PartDraw::Look(fill(ramp)), PartDraw::Look(hidden)]
        }
        GlassLook::Cad => vec![PartDraw::Flooded(fill(ramp), ramp, CAD_OUTLINE_PX)],
        GlassLook::Hologram => {
            let color = LinearRgba::from(HOLO_AMBER).mix(&LinearRgba::from(HOLO_RED), x);
            let mut m = fill(color).with_edge(color * 4.0, 1.6, 2.0);
            m.params.mode.z = b.lost as u8 as f32;
            vec![PartDraw::Look(m)]
        }
        GlassLook::Silhouette => {
            let mut m = fill(ramp);
            m.params.cone2.w = 1.0;
            vec![PartDraw::Look(m), mask(ramp)]
        }
    }
}

/// One element a cone's field is drawn with in a look: its material and whether it is the field's
/// outer surface (whose intensity the stage leaves alone).
pub enum FieldDraw {
    Glow(GlowMaterial),
    Look(LookMaterial),
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
        GlassLook::Technical => FieldDraw::Look(march(Blend::Blended)),
        GlassLook::Hologram => {
            let mut m = march(Blend::Additive);
            m.params.color.w = 2.0;
            FieldDraw::Look(m)
        }
        GlassLook::Ghost => {
            let mut m = LookMaterial::new(
                MODE_DISSOLVE,
                hue,
                0.55 * (0.4 + 0.6 * strength),
                Blend::Blended,
                masks,
            )
            .with_falloff(r0);
            m.params.cone2.z = DISSOLVE_BAND_M;
            FieldDraw::Look(m)
        }
        GlassLook::Cad => FieldDraw::Glow(
            GlowMaterial::blended(hue, 0.12 * (0.4 + 0.6 * strength))
                .with_edge(timeline::FIELD_EDGE.into(), 1.0, 4.0)
                .with_falloff(r0),
        ),
        GlassLook::Silhouette => FieldDraw::Look(
            LookMaterial::new(
                MODE_SOFT,
                hue,
                0.7 * (0.4 + 0.6 * strength),
                Blend::Blended,
                masks,
            )
            .with_falloff(r0),
        ),
    })
}
