//! The scene depth a camera's translucent draws read, copied from that camera's own main pass
//! instead of drawn a second time by a depth prepass. Two readers sample it: the VFX billboards'
//! soft-particle fade (`assets/shaders/vfx_billboard.wgsl`) and the hull marks' `ForwardDecal`
//! (`vfx/hull_marks.rs`), whose fade is also what hides a mark behind a nearer surface. Both draw
//! in `Transparent3d`.
//!
//! # Mechanism (bevy 0.19, `bevy_core_pipeline` 0.19.1 and the vendored `bevy_pbr`)
//!
//! A [`MainPassDepth`] camera keeps `DepthPrepass`: the readers' `DEPTH_PREPASS` shader def, the
//! prepass depth binding in the mesh view layout and the main depth texture's `COPY_SRC` usage
//! (`prepare_core_3d_depth_textures`) all key on it. In the render world:
//!
//! * [`drop_prepass_phases`] removes the view's `Opaque3dPrepass` and `AlphaMask3dPrepass` phases
//!   after extraction and before `RenderSystems::PrepareMeshes`. With neither phase,
//!   `specialize_prepass_material_meshes` and `queue_prepass_material_meshes` skip the view, no
//!   batch is built for it, and `run_prepass_system` returns before its render pass, its depth clear
//!   and its depth copy.
//! * [`provide_depth_texture`] gives the view its `ViewPrepassTextures` back: with no prepass phase
//!   `prepare_prepass_textures` removes them, and the mesh view bind group reads the depth binding
//!   from them (`prepass::get_bindings`).
//! * [`copy_main_pass_depth`] copies the main depth into that texture in `Core3d`, after
//!   `main_opaque_pass_3d` and before the transmissive and transparent passes: the same copy
//!   bevy's prepass makes at the end of its own pass, taken after the main pass has drawn the depth
//!   instead.
//!
//! The main pass clears and writes depth itself (`GreaterEqual`, writes on for opaque and alpha
//! mask, `MeshPipeline::specialize`), so it needs no prepass in front of it.
//!
//! # Invariants
//!
//! * **The copy's texture matches the main depth**: format `CORE_3D_DEPTH_FORMAT`, the target's
//!   physical size, and the view's `Msaa` sample count, as `prepare_core_3d_depth_textures` makes
//!   the main depth. `copy_texture_to_texture` requires equal sample counts, and the readers bind it
//!   multisampled exactly when the view is (`MeshPipelineViewLayoutKey::MULTISAMPLED`), so every
//!   MSAA rung the settings offer (off, 2x, 4x) is served by the same code.
//! * **Depth only.** A camera that also asks for normals, motion vectors, a deferred G-buffer or a
//!   double-buffered depth is left to bevy's own prepass ([`Served`]): those outputs come from the
//!   prepass draw itself.
//! * **Chosen at boot.** The marker must not leave a camera the render world has already drawn with
//!   it. Bevy's prepass phases are retained: they queue only entities that became visible or
//!   changed, so a phase recreated mid-run would miss everything already on screen. The
//!   `SPIKE_ABLATE=depth_copy` lever removes the marker in the first `Update`, before the first
//!   extraction.
//!
//! # What differs from the prepass depth
//!
//! The copy holds everything the main opaque pass drew, which includes opaque materials that opt out
//! of the prepass. The one such material here is the pen cam's `LookMaterial` with an opaque blend,
//! drawn by the main camera in the kill cam's lesson: the billboards and hull marks now fade against
//! it, where the prepass depth left it out.

use bevy::core_pipeline::core_3d::{
    CORE_3D_DEPTH_FORMAT, main_opaque_pass_3d, main_transparent_pass_3d, prepare_prepass_textures,
};
use bevy::core_pipeline::prepass::{
    AlphaMask3dPrepass, DeferredPrepass, DepthPrepass, DepthPrepassDoubleBuffer,
    MotionVectorPrepass, NormalPrepass, Opaque3dPrepass, ViewPrepassTextures,
};
use bevy::core_pipeline::{Core3d, Core3dSystems};
use bevy::ecs::schedule::ScheduleConfigs;
use bevy::ecs::system::ScheduleSystem;
use bevy::image::ToExtents;
use bevy::pbr::main_transmissive_pass_3d;
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::render_phase::ViewBinnedRenderPhases;
use bevy::render::render_resource::{TextureDescriptor, TextureDimension, TextureUsages};
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::texture::{ColorAttachment, TextureCache};
use bevy::render::view::{ExtractedView, Msaa, ViewDepthTexture};
use bevy::render::{Render, RenderApp, RenderSystems};

/// On a camera: its prepass depth texture holds its own main pass's depth, copied after the opaque
/// pass, and no prepass draws (module doc). Requires `DepthPrepass`, the component the readers'
/// bindings key on.
#[derive(Component, Clone, Copy, Default, ExtractComponent)]
#[require(DepthPrepass)]
pub(crate) struct MainPassDepth;

/// The render-world views this module serves: a [`MainPassDepth`] camera whose only prepass output
/// is depth.
type Served = (
    With<MainPassDepth>,
    With<DepthPrepass>,
    Without<NormalPrepass>,
    Without<MotionVectorPrepass>,
    Without<DeferredPrepass>,
    Without<DepthPrepassDoubleBuffer>,
);

/// Mounted by `camera::plugin`, which spawns the marker. Without a render app (headless tests) the
/// marker stays a plain component and nothing is registered.
pub(crate) fn plugin(app: &mut App) {
    if app.get_sub_app(RenderApp).is_none() {
        return;
    }
    app.add_plugins(ExtractComponentPlugin::<MainPassDepth>::default());
    let render_app = app
        .get_sub_app_mut(RenderApp)
        .expect("checked above: the render app exists");
    render_app
        .add_systems(
            Render,
            (
                drop_prepass_phases_after_extraction(),
                // After bevy removes the textures from a view with no prepass phase; the auto sync
                // point on this edge applies that removal before this system's insert.
                provide_depth_texture
                    .after(prepare_prepass_textures)
                    .in_set(RenderSystems::PrepareResources),
            ),
        )
        .add_systems(Core3d, copy_between_main_passes());
}

/// [`drop_prepass_phases`], after the extracted marker and views are applied
/// (`RenderSystems::ExtractCommands`) and before the prepass specializes
/// (`RenderSystems::PrepareMeshes`) and queues.
fn drop_prepass_phases_after_extraction() -> ScheduleConfigs<ScheduleSystem> {
    drop_prepass_phases
        .after(RenderSystems::ExtractCommands)
        .before(RenderSystems::PrepareMeshes)
}

/// [`copy_main_pass_depth`], between the main opaque pass, which writes the depth, and the first
/// pass that can read it.
fn copy_between_main_passes() -> ScheduleConfigs<ScheduleSystem> {
    copy_main_pass_depth
        .after(main_opaque_pass_3d)
        .before(main_transmissive_pass_3d)
        .before(main_transparent_pass_3d)
        .in_set(Core3dSystems::MainPass)
}

fn drop_prepass_phases(
    views: Query<&ExtractedView, Served>,
    mut opaque: ResMut<ViewBinnedRenderPhases<Opaque3dPrepass>>,
    mut alpha_mask: ResMut<ViewBinnedRenderPhases<AlphaMask3dPrepass>>,
) {
    for view in &views {
        opaque.remove(&view.retained_view_entity);
        alpha_mask.remove(&view.retained_view_entity);
    }
}

fn provide_depth_texture(
    mut commands: Commands,
    mut texture_cache: ResMut<TextureCache>,
    render_device: Res<RenderDevice>,
    views: Query<(Entity, &ExtractedCamera, &Msaa), Served>,
) {
    for (entity, camera, msaa) in &views {
        let Some(physical_size) = camera.physical_target_size else {
            continue;
        };
        let size = physical_size.to_extents();
        let texture = texture_cache.get(
            &render_device,
            TextureDescriptor {
                label: Some("main_pass_depth_copy"),
                size,
                mip_level_count: 1,
                sample_count: msaa.samples(),
                dimension: TextureDimension::D2,
                format: CORE_3D_DEPTH_FORMAT,
                // RENDER_ATTACHMENT although nothing draws into it: wgpu requires it of every
                // multisampled texture.
                usage: TextureUsages::COPY_DST
                    | TextureUsages::TEXTURE_BINDING
                    | TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            },
        );
        commands.entity(entity).insert(ViewPrepassTextures {
            depth: Some(ColorAttachment::new(texture, None, None, None)),
            normal: None,
            motion_vectors: None,
            deferred: None,
            deferred_lighting_pass_id: None,
            size,
        });
    }
}

fn copy_main_pass_depth(
    view: ViewQuery<(&ViewDepthTexture, &ViewPrepassTextures), Served>,
    mut ctx: RenderContext,
) {
    let (depth, prepass) = view.into_inner();
    let Some(copy) = &prepass.depth else {
        return;
    };
    ctx.command_encoder().copy_texture_to_texture(
        depth.texture.as_image_copy(),
        copy.texture.texture.as_image_copy(),
        prepass.size,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    use bevy::ecs::system::RunSystemOnce;
    use bevy::render::batching::gpu_preprocessing::GpuPreprocessingMode;
    use bevy::render::render_resource::TextureFormat;
    use bevy::render::sync_world::MainEntity;
    use bevy::render::view::{ColorGrading, RetainedViewEntity};

    fn view_of(main: Entity) -> ExtractedView {
        ExtractedView {
            retained_view_entity: RetainedViewEntity::new(MainEntity::from(main), None, 0),
            clip_from_view: Mat4::IDENTITY,
            world_from_view: GlobalTransform::IDENTITY,
            clip_from_world: None,
            target_format: TextureFormat::Rgba16Float,
            viewport: UVec4::ZERO,
            color_grading: ColorGrading::default(),
            invert_culling: false,
        }
    }

    /// The prepass draws nothing for a served camera: its two prepass phases are gone after
    /// [`drop_prepass_phases`], which is what makes bevy's specialize, queue and `run_prepass_system`
    /// skip the view. A camera without the marker keeps both, so the real prepass still runs for
    /// it (the `SPIKE_ABLATE=depth_copy` arm and any other prepass camera rely on that), and a marked
    /// camera that also asks for normals keeps both (its normals come from the prepass draw).
    #[test]
    fn a_served_camera_has_no_prepass_phases_and_others_keep_theirs() {
        let mut world = World::new();
        let mut opaque = ViewBinnedRenderPhases::<Opaque3dPrepass>::default();
        let mut alpha_mask = ViewBinnedRenderPhases::<AlphaMask3dPrepass>::default();
        let served = world.spawn_empty().id();
        let unmarked = world.spawn_empty().id();
        let with_normals = world.spawn_empty().id();
        for main in [served, unmarked, with_normals] {
            let key = view_of(main).retained_view_entity;
            opaque.prepare_for_new_frame(key, GpuPreprocessingMode::Culling);
            alpha_mask.prepare_for_new_frame(key, GpuPreprocessingMode::Culling);
        }
        world.insert_resource(opaque);
        world.insert_resource(alpha_mask);
        world.spawn((view_of(served), MainPassDepth));
        world.spawn((view_of(unmarked), DepthPrepass));
        world.spawn((view_of(with_normals), MainPassDepth, NormalPrepass));

        world.run_system_once(drop_prepass_phases).unwrap();

        let opaque = world.resource::<ViewBinnedRenderPhases<Opaque3dPrepass>>();
        let alpha_mask = world.resource::<ViewBinnedRenderPhases<AlphaMask3dPrepass>>();
        let key = |main| view_of(main).retained_view_entity;
        assert!(!opaque.contains_key(&key(served)) && !alpha_mask.contains_key(&key(served)));
        for kept in [unmarked, with_normals] {
            assert!(opaque.contains_key(&key(kept)) && alpha_mask.contains_key(&key(kept)));
        }
    }

    /// Whether a `Core3d` schedule holding the main opaque pass and `reader`, plus the copy when
    /// `with_copy`, builds.
    fn builds(reader: fn() -> ScheduleConfigs<ScheduleSystem>, with_copy: bool) -> bool {
        let mut world = World::new();
        let mut schedule = Core3d::base_schedule();
        schedule.add_systems((main_opaque_pass_3d, reader()));
        if with_copy {
            schedule.add_systems(copy_between_main_passes());
        }
        schedule.initialize(&mut world).is_ok()
    }

    /// The copy runs after the main opaque pass has written the depth and before both passes that
    /// can read it, the transmissive and the transparent (where the billboards and hull marks
    /// draw): anywhere else it hands the readers a cleared or stale texture. Shown by contradiction:
    /// a reader ordered BEFORE the opaque pass builds alone and builds beside the copy while the
    /// three are otherwise unordered, but not once the copy's edges join them, which order the
    /// opaque pass before the reader through the copy.
    #[test]
    fn the_copy_runs_after_the_opaque_pass_and_before_every_depth_reader() {
        let readers: [[fn() -> ScheduleConfigs<ScheduleSystem>; 2]; 2] = [
            [
                || main_transmissive_pass_3d.into_configs(),
                || main_transmissive_pass_3d.before(main_opaque_pass_3d),
            ],
            [
                || main_transparent_pass_3d.into_configs(),
                || main_transparent_pass_3d.before(main_opaque_pass_3d),
            ],
        ];
        for [unordered, first] in readers {
            assert!(
                builds(unordered, true),
                "the copy and the passes must build"
            );
            assert!(builds(first, false), "a reader first must build alone");
            assert!(!builds(first, true), "the copy must sit between the passes");
        }
    }
}
