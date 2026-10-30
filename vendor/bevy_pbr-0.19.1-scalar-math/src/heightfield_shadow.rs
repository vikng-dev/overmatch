//! OVERMATCH PATCH: a heightfield's shadow toward one directional light, baked once and sampled by
//! every lit fragment (see `OVERMATCH_PATCH.md`).
//!
//! The application inserts [`HeightfieldShadow`]: a world-anchored texture over a square of world
//! XZ, one texel per sample point, that records for each point the occluder its ray toward the
//! light meets. `fetch_directional_shadow` (`render/shadows.wgsl`) multiplies the light's cascade
//! shadow by the field's visibility at the fragment, for the light whose direction matches the
//! field's and only inside that light's cascade envelope. Geometry whose shadow the field carries
//! should not also cast into the cascades; that is the application's call.
//!
//! With no resource, or before its image is on the GPU, the binding holds a fallback texture and
//! the uniform says the field is absent: the shader returns full visibility.

use bevy_app::{App, Plugin};
use bevy_asset::Handle;
use bevy_ecs::{
    change_detection::DetectChanges,
    resource::Resource,
    schedule::IntoScheduleConfigs,
    system::{Commands, Res, ResMut},
    world::{FromWorld, World},
};
use bevy_image::Image;
use bevy_math::{Vec2, Vec3, Vec4};
use bevy_render::{
    Extract, ExtractSchedule, GpuResourceAppExt, Render, RenderApp, RenderSystems,
    render_asset::RenderAssets,
    render_resource::{ShaderType, UniformBuffer},
    renderer::{RenderDevice, RenderQueue},
    texture::GpuImage,
};

/// A heightfield's occlusion toward one directional light.
///
/// `image` is `Rg32Float`, `size × size` texels; texel `(i, j)` sits at world XZ
/// `origin + (i, j) · texel_m`. Its R is the height (m) of the occluder the texel's ray toward the
/// light meets, its G the horizontal distance (m) from the texel to that occluder along the light.
/// A negative G records no occluder. The shader extrapolates each of a fragment's four texels'
/// occluder to the fragment along the light and interpolates how far the fragment lies below it
/// and how far away the occluder is; the light's own disc then sets the penumbra.
#[derive(Resource, Clone, Debug)]
pub struct HeightfieldShadow {
    pub image: Handle<Image>,
    /// World XZ of texel `(0, 0)`.
    pub origin: Vec2,
    /// World metres between neighbouring texels.
    pub texel_m: f32,
    /// Unit vector toward the light the field was baked for. The field shadows the directional
    /// light whose `direction_to_light` matches it, and no other.
    pub direction_to_light: Vec3,
    /// The light's angular radius (rad): a fragment whose occluder's edge stands this angle above
    /// the light's centre is fully shadowed, as far below it fully lit, half lit on the line.
    pub angular_radius: f32,
    /// The least half-width (m) of the penumbra in depth, near an occluder's foot where the disc's
    /// own would vanish.
    pub min_penumbra_m: f32,
}

/// The field's uniform, as `mesh_view_types::HeightfieldShadow` reads it.
#[derive(Clone, Copy, Default, ShaderType)]
pub struct GpuHeightfieldShadow {
    /// xy: world XZ of texel (0, 0); z: texels per metre; w: metres per texel.
    map: Vec4,
    /// xy: the unit horizontal direction toward the light; z: the light's rise per horizontal
    /// metre; w: 1 when the field is bound, 0 when absent.
    ray: Vec4,
    /// xyz: the direction toward the light the field was baked for; w: unused.
    light: Vec4,
    /// x: the light's angular radius (rad); y: the least penumbra half-width (m).
    law: Vec4,
}

impl GpuHeightfieldShadow {
    /// The uniform for `field`, bound. Public so an application can check the shader's receiver
    /// law against its own fields headlessly.
    pub fn from_field(field: &HeightfieldShadow) -> Self {
        let dir = field.direction_to_light.normalize();
        let horizontal = Vec2::new(dir.x, dir.z);
        let run = horizontal.length();
        Self {
            map: Vec4::new(
                field.origin.x,
                field.origin.y,
                field.texel_m.recip(),
                field.texel_m,
            ),
            ray: Vec4::new(horizontal.x / run, horizontal.y / run, dir.y / run, 1.0),
            light: dir.extend(0.0),
            law: Vec4::new(field.angular_radius, field.min_penumbra_m, 0.0, 0.0),
        }
    }
}

/// The render world's copy of the main world's [`HeightfieldShadow`], present while it is.
#[derive(Resource, Clone)]
pub struct ExtractedHeightfieldShadow(pub HeightfieldShadow);

/// The field's uniform buffer. Written once at startup (absent) so the view bind group always has
/// a binding, then whenever the field or its image's readiness changes.
#[derive(Resource)]
pub struct HeightfieldShadowUniform {
    pub buffer: UniformBuffer<GpuHeightfieldShadow>,
    bound: bool,
}

impl FromWorld for HeightfieldShadowUniform {
    fn from_world(world: &mut World) -> Self {
        let mut buffer = UniformBuffer::from(GpuHeightfieldShadow::default());
        buffer.set_label(Some("heightfield_shadow_uniform"));
        buffer.write_buffer(
            world.resource::<RenderDevice>(),
            world.resource::<RenderQueue>(),
        );
        Self {
            buffer,
            bound: false,
        }
    }
}

pub struct HeightfieldShadowPlugin;

impl Plugin for HeightfieldShadowPlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_gpu_resource::<HeightfieldShadowUniform>()
            .add_systems(ExtractSchedule, extract_heightfield_shadow)
            .add_systems(
                Render,
                prepare_heightfield_shadow.in_set(RenderSystems::PrepareResources),
            );
    }
}

fn extract_heightfield_shadow(
    mut commands: Commands,
    field: Extract<Option<Res<HeightfieldShadow>>>,
    extracted: Option<Res<ExtractedHeightfieldShadow>>,
) {
    match (field.as_ref(), extracted) {
        (Some(field), extracted) if field.is_changed() || extracted.is_none() => {
            commands.insert_resource(ExtractedHeightfieldShadow((**field).clone()));
        }
        (None, Some(_)) => commands.remove_resource::<ExtractedHeightfieldShadow>(),
        _ => {}
    }
}

/// The field is bound once its image is on the GPU; the bind group reads the same readiness.
fn prepare_heightfield_shadow(
    field: Option<Res<ExtractedHeightfieldShadow>>,
    images: Res<RenderAssets<GpuImage>>,
    mut uniform: ResMut<HeightfieldShadowUniform>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    let ready = field
        .as_ref()
        .filter(|field| images.get(&field.0.image).is_some());
    let changed = field.as_ref().is_some_and(|field| field.is_changed());
    if ready.is_some() == uniform.bound && !changed {
        return;
    }
    uniform.bound = ready.is_some();
    uniform.buffer.set(
        ready
            .map(|field| GpuHeightfieldShadow::from_field(&field.0))
            .unwrap_or_default(),
    );
    uniform.buffer.write_buffer(&device, &queue);
}
