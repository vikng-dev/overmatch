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
}

/// How a [`LookMaterial`](super::look::LookMaterial) composites.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    /// Straight alpha.
    Blended,
    /// Writes depth: what an inverted-hull outline needs in front of it, and the masks.
    Opaque,
}

/// One flat colour added to what lies behind it, optionally rim-lit by view angle; both faces
/// drawn. Additive is commutative, so it needs no sort order.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct GlowMaterial {
    #[uniform(0)]
    pub params: GlowParams,
}

impl GlowMaterial {
    /// An additive glow of `color` (linear) at `intensity`, without rim.
    pub fn additive(color: LinearRgba, intensity: f32) -> Self {
        Self {
            params: GlowParams {
                color: Vec4::new(color.red, color.green, color.blue, 1.0),
                rim: Vec4::new(intensity, 0.0, 1.0, 0.0),
            },
        }
    }

    /// Intensity `base + gain · (1 − |n·v|)^power`: clear face-on, bright at grazing angles.
    pub fn with_rim(mut self, base: f32, gain: f32, power: f32) -> Self {
        self.params.rim.x = base;
        self.params.rim.y = gain;
        self.params.rim.z = power;
        self
    }

    pub fn set_fade(&mut self, fade: f32) {
        self.params.color.w = fade;
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
        AlphaMode::Add
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
        // Toward the eye under bevy's reverse Z: a glow wins a depth tie with the surface it lies
        // on.
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

/// An outline sorts this far (m) nearer the eye than its mesh's centre: after every surface of the
/// base look's body, whose glass would otherwise wash the band where it lies outside its
/// component, and before every volume ([`super::look::VOLUME_SORT_BIAS`]).
pub const OUTLINE_SORT_BIAS: f32 = 5.0e3;

/// A mesh's silhouette as an inverted hull: the mesh pushed out on screen from its centre by a
/// fixed pixel width with its front faces culled, so its back faces show only as a band around the
/// opaque fill the mesh itself wears, which hides the rest of them by depth. Drawn in the blended
/// pass, writing no depth, sorted by [`OUTLINE_SORT_BIAS`].
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

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }

    fn depth_bias(&self) -> f32 {
        OUTLINE_SORT_BIAS
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
/// extension. A body mesh's (`params.params.w = 1`) reads its viewer's part mask
/// ([`super::look::MaskSet`]) and is discarded on the cut's kept side wherever the mask holds a
/// scene surface, a component or the round: the slice's share of the draw order ([`super::look`]).
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct SliceCut {
    #[uniform(100)]
    pub params: SliceParams,
    #[texture(101)]
    pub part_mask: Option<Handle<Image>>,
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
