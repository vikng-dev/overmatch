//! The pen cam stage's two materials: [`GlowMaterial`], the unlit diagram material every record
//! element wears (`pen_cam_glow.wgsl`), and [`SliceMaterial`], the tank's own materials with the
//! eye's side of the slice plane dissolved away (`pen_cam_slice.wgsl`).

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
}

/// One flat colour, optionally rim-lit by view angle, vertex-coloured, clipped by a plane or
/// hatched; both faces drawn. Additive materials are commutative, so they need no sort order.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct GlowMaterial {
    #[uniform(0)]
    pub params: GlowParams,
    pub additive: bool,
}

impl GlowMaterial {
    /// An additive glow of `color` (linear) at `intensity`, without rim.
    pub fn additive(color: LinearRgba, intensity: f32) -> Self {
        Self {
            params: GlowParams {
                color: Vec4::new(color.red, color.green, color.blue, 1.0),
                rim: Vec4::new(intensity, 0.0, 1.0, 0.0),
                plane: Vec4::ZERO,
                flags: Vec4::new(0.0, 0.0, 1.0, 1.0),
            },
            additive: true,
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
            },
            additive: false,
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

    pub fn with_hatch(mut self) -> Self {
        self.params.flags.y = 1.0;
        self
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
    fn fragment_shader() -> ShaderRef {
        "shaders/pen_cam_glow.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        if self.additive {
            AlphaMode::Add
        } else {
            AlphaMode::Blend
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
