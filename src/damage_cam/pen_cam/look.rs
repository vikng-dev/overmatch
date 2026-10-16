//! The corner's look ([`GlassLook::Base`]): the whole tank as one hueless grey surface one layer
//! deep, its components a darker grey one layer deep among themselves, the struck ones on the state
//! scale, and the spall field as light, drawn as the law defines it, with its footprint outlined on
//! every surface it met. [`LookMaterial`] (`pen_cam_look.wgsl`) draws
//! every element; [`hull`], [`component`] and [`field`] choose them, and the stage spawns what they
//! return. The kill cam's lesson, drawn on the world where the corner's masks do not reach, keeps
//! the stage's own additive glass ([`GlassLook::Baseline`]).
//!
//! The stacking is cut by two offscreen masks drawn from the corner camera's own pose before it:
//! the hull mask (each pixel's nearest hull surface: view normal and distance) and the part mask
//! (each pixel's nearest component surface). A surface draws only where its fragment is its mask's
//! nearest, and its edges come from that mask.
//!
//! Tiers come from the tank's data alone ([`Tier`]): main armour, external armour, component and
//! detail, by the substance each mesh wears and the tank file's volume entries.

use bevy::camera::{Hdr, ImageRenderTarget, RenderTarget};
use bevy::core_pipeline::tonemapping::{DebandDither, Tonemapping};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, CompareFunction, ShaderType, TextureFormat};
use bevy::shader::ShaderRef;

use super::GlassLook;
use super::material::Blend;
use super::plan::BoxPlan;
use super::stage::StageCamera;
use super::timeline::{self, Condition, CornerKind};
use super::view::Tier;
use crate::render_policy::CameraProfile;

/// The field's brightness per unit of its column density `∫ ds / (r² + ε²)` (m⁻¹) at full level
/// ([`timeline::field_level`]): a display gain. TUNED, as is every figure below.
const FIELD_GAIN: f32 = 1.5;
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
/// and its edge colour and opacity. It keeps one opacity everywhere: a struck component shows
/// through it as every other component does.
const BASE_GREY: Color = Color::srgb_u8(0xA4, 0xA4, 0xA4);
const BASE_ALPHA: f32 = 0.6;
const BASE_FACE_SHARE: f32 = 0.4;
const BASE_FRESNEL_POWER: f32 = 2.0;
const BASE_CONTRAST: f32 = 0.35;
const BASE_EDGE: Color = Color::srgb_u8(0xE6, 0xE6, 0xE6);
const BASE_EDGE_ALPHA: f32 = 0.35;
/// The components' base: the state scale's untouched grey, fuller than the hull.
const BASE_COMPONENT_ALPHA: f32 = 0.55;
const BASE_COMPONENT_FACE_SHARE: f32 = 0.6;
const BASE_COMPONENT_EDGE_ALPHA: f32 = 0.4;
/// The field footprint's outline width (px) and its opacity at full level, in the field's hue.
const FOOTPRINT_LINE_PX: f32 = 2.0;
const FOOTPRINT_ALPHA: f32 = 0.9;
/// The most spall fields a footprint marks, and the most directions of each it reads; the arrays
/// of `pen_cam_look.wgsl` hold these counts. [`set_footprints`] drops what lies past them, so every
/// record's cones and directions must fit them (`every_record_fits_the_footprint`).
const FOOTPRINT_FIELDS: usize = 4;
const FOOTPRINT_RAYS: usize = 256;
/// A surface counts as met by a direction up to this share past its reach, and this far (m): the
/// record ends a direction on the surface that stopped it.
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
    pub foot: Vec4,
    pub foot_color: Vec4,
    pub fields: Vec4,
    pub volume: Vec4,
    pub profile: Vec4,
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
            hatch: Vec4::ZERO,
            mode: Vec4::ZERO,
            state: Vec4::ZERO,
            span: Vec4::ZERO,
            rest: Vec4::ZERO,
            foot: Vec4::ZERO,
            foot_color: Vec4::ZERO,
            fields: Vec4::ZERO,
            volume: Vec4::ZERO,
            profile: Vec4::ZERO,
            prints: [Vec4::ZERO; 3 * FOOTPRINT_FIELDS],
            rays: [Vec4::ZERO; FOOTPRINT_FIELDS * FOOTPRINT_RAYS],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LookKey {
    bias: bool,
    unoccluded: bool,
}

impl From<&LookMaterial> for LookKey {
    fn from(material: &LookMaterial) -> Self {
        Self {
            bias: material.bias,
            unoccluded: material.params.mode.x == MODE_VOLUME,
        }
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
    fn new(mode: f32, color: LinearRgba, alpha: f32, blend: Blend, masks: &Masks) -> Self {
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

    pub fn set_front(&mut self, radius: Option<f32>) {
        self.params.field.x = radius.map_or(-1.0, |r| r.max(0.0));
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
        descriptor.primitive.cull_mode = None;
        let key = key.bind_group_data;
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            if key.bias {
                // Toward the eye under bevy's reverse Z.
                depth.bias.constant = 8;
                depth.bias.slope_scale = 1.0;
            }
            if key.unoccluded {
                // Every entry and exit of the volume must reach the sum, whatever stands in front.
                depth.depth_compare = Some(CompareFunction::Always);
                depth.depth_write_enabled = Some(false);
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

/// Pose the mask cameras as the corner camera; run them only while the base look shows.
pub fn sync(
    stage: Res<super::stage::Stage>,
    main: Query<(&Camera, &Transform, &Projection), (With<StageCamera>, Without<MaskCamera>)>,
    mut followers: Query<
        (&mut Camera, &mut Transform, &mut Projection),
        (With<MaskCamera>, Without<StageCamera>),
    >,
) {
    let Ok((camera, transform, projection)) = main.single() else {
        return;
    };
    let active = camera.is_active && stage.look() == GlassLook::Base;
    for (mut follower, mut pose, mut lens) in &mut followers {
        if follower.is_active != active {
            follower.is_active = active;
        }
        if *pose != *transform {
            *pose = *transform;
        }
        *lens = projection.clone();
    }
}

/// One spall field where a footprint is taken, in world space: its origin, unit axis, half-angle
/// (rad), reach (m, its longest direction's), how far its front has run (m, negative before it is
/// thrown), its display level ([`timeline::field_level`]) and its recorded directions (unit) with
/// each one's reach (m).
#[derive(Clone, Debug)]
pub struct FieldPrint {
    pub origin: Vec3,
    pub axis: Vec3,
    pub half_angle: f32,
    pub reach: f32,
    pub front: f32,
    pub level: f32,
    pub rays: Vec<(Vec3, f32)>,
}

/// Outline `prints` on every surface the base look draws, the hull's and the components'. A
/// surface is inside a field's footprint where it faces the field's origin, lies inside its cone,
/// behind its front and no farther than the reach of the recorded direction nearest it: the field
/// met it there, which says nothing of what stopped. The outline is where the angle from the axis
/// crosses the half-angle, its opacity [`FOOTPRINT_ALPHA`] times the field's level, at most 1, so a
/// weak field's outline is as faint as its body.
pub fn set_footprints(looks: &mut Assets<LookMaterial>, prints: &[FieldPrint]) {
    let mut lanes = [Vec4::ZERO; 3 * FOOTPRINT_FIELDS];
    let mut rays = [Vec4::ZERO; FOOTPRINT_FIELDS * FOOTPRINT_RAYS];
    let (share, metres) = FOOTPRINT_REACH_SLACK;
    let count = prints.len().min(FOOTPRINT_FIELDS);
    for (i, print) in prints.iter().take(count).enumerate() {
        let n = print.rays.len().min(FOOTPRINT_RAYS);
        lanes[3 * i] = print.origin.extend(print.half_angle);
        lanes[3 * i + 1] = print.axis.extend(print.reach);
        lanes[3 * i + 2] = Vec4::new(print.front, n as f32, print.level.clamp(0.0, 1.0), 0.0);
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
        material.params.foot = Vec4::new(FOOTPRINT_LINE_PX, 0.0, 0.0, 0.0);
        material.params.foot_color = Vec4::new(hue.red, hue.green, hue.blue, FOOTPRINT_ALPHA);
        material.params.fields = Vec4::new(count as f32, 0.0, 0.0, 0.0);
        material.params.prints = lanes;
        material.params.rays = rays;
    }
}

/// How a hull mesh is drawn in the base look: its material and the mask that holds it.
pub struct HullDraw {
    pub material: LookMaterial,
    pub mask: MaskLayer,
}

/// The offscreen mask a hull mesh is drawn into, for the nearest-surface test of its own layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaskLayer {
    Hull,
    /// The part mask: every component mesh.
    Part,
}

/// How a hull mesh of tier `tier` is drawn in the base look; the baseline's glass is the stage's
/// own.
pub fn hull(tier: Tier, masks: &Masks) -> HullDraw {
    match tier {
        Tier::Component => HullDraw {
            material: component_base(masks),
            mask: MaskLayer::Part,
        },
        _ => HullDraw {
            material: base(masks),
            mask: MaskLayer::Hull,
        },
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

/// The masks' material: view normal and distance, drawn into either mask.
pub fn hull_mask(masks: &Masks) -> LookMaterial {
    LookMaterial::new(MODE_MASK, LinearRgba::BLACK, 1.0, Blend::Opaque, masks)
}

/// Component `b`'s place on the state scale in frame `kind`, `None` where it is not drawn: the
/// shooter's views (the glass and the outside view) show what this shot did to a component it
/// took HP from, the share of the HP points it had at the shot; the victim's glass shows its own
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
fn state(condition: Condition, masks: &Masks) -> LookMaterial {
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
pub fn component(b: &BoxPlan, kind: CornerKind, masks: &Masks) -> Option<LookMaterial> {
    condition(b, kind).map(|condition| state(condition, masks))
}

/// The field in `look` as the law defines it, `None` in the baseline, which draws the stage's own
/// nested shells: a cone from its origin, density `profile × 1/(r² + ε²)`, cut where its
/// directions stopped (the reach surface the mesh it wears is, wound outward), summed along the
/// eye ray as light. `cap` is the radius ε (m) the density is capped within at the origin,
/// `strength` the cone's on-axis strength (ref-mm), drawn linearly ([`timeline::field_level`]), and
/// `profile` its profile law in the mesh's frame ([`super::field::profile`]). Drawn over everything.
pub fn field(
    look: GlassLook,
    cap: f32,
    strength: f32,
    profile: Vec4,
    masks: &Masks,
) -> Option<LookMaterial> {
    if look == GlassLook::Baseline {
        return None;
    }
    let mut m = LookMaterial::new(
        MODE_VOLUME,
        timeline::FIELD.into(),
        FIELD_GAIN * timeline::field_level(strength),
        Blend::Additive,
        masks,
    );
    m.params.volume.w = cap;
    m.params.profile = profile;
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_draw(tier: Tier) -> HullDraw {
        hull(tier, &Masks::default())
    }

    /// The base look draws every non-component tier with one material in the hull mask, at one
    /// opacity everywhere.
    #[test]
    fn the_base_draws_every_hull_tier_alike() {
        let first = base_draw(Tier::Armour).material.params;
        for tier in [Tier::Armour, Tier::External, Tier::Detail] {
            let draw = base_draw(tier);
            assert_eq!(draw.mask, MaskLayer::Hull);
            assert_eq!(draw.material.blend, Blend::Blended);
            let p = draw.material.params;
            assert_eq!(
                (p.mode, p.color, p.edge, p.shade),
                (first.mode, first.color, first.edge, first.shade)
            );
            assert_eq!(p.mode.x, MODE_BASE);
        }
    }

    /// Every component mesh draws in the base's family, hueless and HP-blind, one layer deep among
    /// the components and outside the hull's layer.
    #[test]
    fn the_base_draws_every_component_grey_in_its_own_layer() {
        let draw = base_draw(Tier::Component);
        assert_eq!(draw.mask, MaskLayer::Part);
        assert_eq!(draw.material.params.mode.x, MODE_BASE);
        let c = draw.material.params.color;
        let grey = LinearRgba::from(timeline::STATE_UNTOUCHED);
        assert_eq!(c.truncate(), Vec3::new(grey.red, grey.green, grey.blue));
    }

    /// A struck component draws on the state fill: its share as the fill height, red when damaged,
    /// hatched when dead.
    #[test]
    fn a_struck_component_draws_the_state_scale() {
        let masks = Masks::default();
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
        assert_eq!(fill.params.span, Vec4::new(1.0, 2.0, 1.0, STATE_LINE_PX));
    }

    /// The footprint outlines the base look's surfaces, hull and components alike, and the state
    /// fills, with every field's frame, level and directions; the field itself carries none.
    #[test]
    fn the_footprint_marks_the_base_looks_surfaces() {
        let masks = Masks::default();
        let mut looks = Assets::<LookMaterial>::default();
        let base = looks.add(base(&masks));
        let component = looks.add(component_base(&masks));
        let fill = looks.add(state(Condition::Damaged(0.5), &masks));
        let shell = looks.add(field(GlassLook::Base, 0.04, 5.0, Vec4::W, &masks).unwrap());
        let print = FieldPrint {
            origin: Vec3::ZERO,
            axis: Vec3::NEG_Z,
            half_angle: 0.5,
            reach: 2.0,
            front: 1.0,
            level: 0.25,
            rays: vec![
                (Vec3::NEG_Z, 2.0),
                (Vec3::new(0.0, 0.4, -0.9).normalize(), 1.5),
            ],
        };
        set_footprints(&mut looks, std::slice::from_ref(&print));
        for handle in [&base, &component, &fill] {
            let p = looks.get(handle).unwrap().params;
            assert_eq!(p.foot.x, FOOTPRINT_LINE_PX);
            assert_eq!(p.fields.x, 1.0);
            assert_eq!(p.prints[0], Vec4::new(0.0, 0.0, 0.0, 0.5));
            assert_eq!(p.prints[2].y, 2.0, "both directions");
            assert_eq!(p.prints[2].z, 0.25, "the field's level");
            assert!(p.rays[1].w > 1.5, "a direction's reach holds its slack");
        }
        assert_eq!(looks.get(&shell).unwrap().params.fields.x, 0.0);
    }

    /// Every record's spall fields fit the footprint whole: no more cones than it marks, no more
    /// directions in any than it reads.
    #[test]
    fn every_record_fits_the_footprint() {
        for record in &super::super::Library::load().records {
            let cones = &record.plan.cones;
            assert!(
                cones.len() <= FOOTPRINT_FIELDS,
                "{}: {} cones",
                record.stem,
                cones.len()
            );
            for cone in cones {
                assert!(
                    cone.rays.len() <= FOOTPRINT_RAYS,
                    "{}: {} directions",
                    record.stem,
                    cone.rays.len()
                );
            }
        }
    }

    /// The field's brightness is linear in its strength through one fixed gain, and the baseline
    /// draws its own.
    #[test]
    fn the_field_is_linear_in_its_strength() {
        let masks = Masks::default();
        let gain = |strength: f32| {
            field(GlassLook::Base, 0.04, strength, Vec4::W, &masks)
                .unwrap()
                .params
                .color
                .w
        };
        assert_eq!(gain(0.0), 0.0);
        assert!((gain(7.0) / gain(1.0) - 7.0).abs() < 1e-5);
        assert!(field(GlassLook::Baseline, 0.04, 1.0, Vec4::W, &masks).is_none());
    }
}
