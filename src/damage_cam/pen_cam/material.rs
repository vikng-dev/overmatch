//! The pen cam stage's materials: [`GlowMaterial`], the unlit diagram material every record element
//! wears (`pen_cam_glow.wgsl`); [`OutlineMaterial`], a struck component's silhouette as an inverted
//! hull (`pen_cam_outline.wgsl`); and [`SliceMaterial`], the
//! tank's own materials with the eye's side of the slice plane dissolved away
//! (`pen_cam_slice.wgsl`).

use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

/// The uniform block `pen_cam_glow.wgsl` reads; its lanes are documented there.
#[derive(ShaderType, Debug, Clone, Copy, Default)]
pub struct GlowParams {
    pub color: Vec4,
    pub rim: Vec4,
    pub plane: Vec4,
    pub flags: Vec4,
    pub field: Vec4,
}

/// How a [`GlowMaterial`] composites.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    /// Commutative, so it needs no sort order.
    Additive,
    /// Straight alpha.
    Blended,
    /// Writes depth: what an inverted-hull outline needs in front of it.
    Opaque,
}

/// One flat colour, optionally rim-lit by view angle, vertex-coloured, clipped by a plane, faded by distance from the mesh's origin or cut off at a radius from
/// it; both faces drawn.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct GlowMaterial {
    #[uniform(0)]
    pub params: GlowParams,
    pub blend: Blend,
}

/// [`GlowParams::field`] with no front (`x < 0`) and no falloff (`y = 0`).
const NO_FIELD: Vec4 = Vec4::new(-1.0, 0.0, 0.0, 0.0);

impl GlowMaterial {
    /// An additive glow of `color` (linear) at `intensity`, without rim.
    pub fn additive(color: LinearRgba, intensity: f32) -> Self {
        Self {
            params: GlowParams {
                color: Vec4::new(color.red, color.green, color.blue, 1.0),
                rim: Vec4::new(intensity, 0.0, 1.0, 0.0),
                plane: Vec4::ZERO,
                flags: Vec4::new(0.0, 0.0, 1.0, 1.0),
                field: NO_FIELD,
            },
            blend: Blend::Additive,
        }
    }

    /// A straight-alpha fill of `color` at `alpha`.
    pub fn blended(color: LinearRgba, alpha: f32) -> Self {
        Self {
            params: GlowParams {
                color: Vec4::new(color.red, color.green, color.blue, alpha),
                rim: Vec4::new(1.0, 0.0, 1.0, 0.0),
                plane: Vec4::ZERO,
                flags: Vec4::new(0.0, 0.0, 0.0, 1.0),
                field: NO_FIELD,
            },
            blend: Blend::Blended,
        }
    }

    /// Intensity `base + gain · (1 − |n·v|)^power`: clear face-on, bright at grazing angles.
    pub fn with_rim(mut self, base: f32, gain: f32, power: f32) -> Self {
        self.params.rim.x = base;
        self.params.rim.y = gain;
        self.params.rim.z = power;
        self
    }

    /// Multiply colour by the mesh's vertex colour and intensity by its alpha.
    pub fn with_vertex_color(mut self) -> Self {
        self.params.rim.w = 1.0;
        self
    }

    /// Intensity falls as `min(1, (r₀ / r)²)` with `r` the distance (m) from the mesh's origin: a
    /// field's fluence from its source, unity inside `r₀`.
    pub fn with_falloff(mut self, r0: f32) -> Self {
        self.params.field.y = r0;
        self
    }

    /// Every vertex farther than `radius` (m) from the mesh's origin is drawn at that radius, along
    /// its own direction: a front grown that far. `None` draws the mesh whole.
    pub fn set_front(&mut self, radius: Option<f32>) {
        self.params.field.x = radius.map_or(NO_FIELD.x, |r| r.max(0.0));
    }

    /// Discard fragments on `normal`'s side of the plane through `point`; `None` clips nothing.
    pub fn set_clip(&mut self, plane: Option<(Vec3, Vec3)>) {
        match plane {
            Some((normal, point)) => {
                self.params.plane = normal.extend(normal.dot(point));
                self.params.flags.x = 1.0;
            }
            None => self.params.flags.x = 0.0,
        }
    }

    pub fn set_fade(&mut self, fade: f32) {
        self.params.flags.w = fade;
    }

    pub fn set_intensity(&mut self, intensity: f32) {
        self.params.rim.x = intensity;
    }
}

impl Material for GlowMaterial {
    fn vertex_shader() -> ShaderRef {
        "shaders/pen_cam_glow.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "shaders/pen_cam_glow.wgsl".into()
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
        _key: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        // A component's tint is a copy of the mesh it covers, at the same depth; the bias (toward
        // the eye under bevy's reverse Z) settles the tie in the tint's favour, opaque tank
        // materials included.
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.bias.constant = 8;
            depth.bias.slope_scale = 1.0;
        }
        Ok(())
    }
}

/// The uniform block `pen_cam_outline.wgsl` reads; its lanes are documented there.
#[derive(ShaderType, Debug, Clone, Copy, Default)]
pub struct OutlineParams {
    pub color: Vec4,
    pub center: Vec4,
}

/// A mesh's silhouette as an inverted hull: the mesh pushed out on screen from its centre by a
/// fixed pixel width with its front faces culled, so its back faces show only as a band around the
/// opaque fill the mesh itself wears.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct OutlineMaterial {
    #[uniform(0)]
    pub params: OutlineParams,
}

impl OutlineMaterial {
    /// An outline of `color` (linear), `width_px` physical pixels wide, pushed out from `center` in
    /// the mesh's own frame.
    pub fn new(color: LinearRgba, width_px: f32, center: Vec3) -> Self {
        Self {
            params: OutlineParams {
                color: Vec4::new(color.red, color.green, color.blue, 1.0),
                center: center.extend(width_px),
            },
        }
    }

    pub fn set_fade(&mut self, fade: f32) {
        self.params.color.w = fade;
    }
}

impl Material for OutlineMaterial {
    fn vertex_shader() -> ShaderRef {
        "shaders/pen_cam_outline.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "shaders/pen_cam_outline.wgsl".into()
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
        _key: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = Some(bevy::render::render_resource::Face::Front);
        Ok(())
    }
}

pub type SliceMaterial = ExtendedMaterial<StandardMaterial, SliceCut>;

/// The slice's half of [`SliceMaterial`]. Bindings start at 100, bevy's convention for a material
/// extension.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct SliceCut {
    #[uniform(100)]
    pub params: SliceParams,
}

/// The uniform block `pen_cam_slice.wgsl` reads; its lanes are documented there.
#[derive(ShaderType, Reflect, Debug, Clone, Copy, Default)]
pub struct SliceParams {
    pub plane: Vec4,
    pub cut_color: Vec4,
    pub params: Vec4,
}

impl MaterialExtension for SliceCut {
    fn fragment_shader() -> ShaderRef {
        "shaders/pen_cam_slice.wgsl".into()
    }
}
