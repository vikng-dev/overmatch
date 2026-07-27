//! Render scale: draw the 3D world at a fraction of the window, present it at full size.
//!
//! Ships — NOT `dev_tools`-gated. This is the player-facing [`RenderScaleLevel`] row of the settings
//! page made real; `settings::apply_settings` is the ONE writer of the [`RenderScale`] resource this
//! module reads, exactly as it is the one writer of `Msaa` and the shadow config.
//!
//! # Why this shape (research brief §3.2 route (c) / §5.4 "Route A")
//!
//! The knob this replaces set `WindowResolution::set_scale_factor_override`, which is a WINDOW-SIZE
//! knob wearing a render-scale costume: `bevy_winit` turns it into a resize request, macOS ignores
//! resize requests for a window in a fullscreen Space, and the only thing that actually moved in
//! windowed mode was the logical canvas — so the UI grew by `1/fraction` while the drawable, and
//! therefore the GPU cost, stayed put or moved for the wrong reason.
//!
//! Route A instead uses bevy 0.19's `MainPassResolutionOverride`, the primitive that exists as DLSS
//! scaffolding and is honoured by the prepass, the opaque/transparent main passes, the deferred pass
//! and OIT. The camera still targets the WINDOW at its real size, which is the property that makes
//! the blast radius the render app alone:
//!
//! * `aim`/`sight`/`hud` reproject through `world_to_viewport` / `viewport_to_world` against the
//!   same window-logical viewport as before — the override changes neither `Camera::viewport` nor
//!   the window, so their math is untouched BY CONSTRUCTION, not by a matching fix-up.
//! * `overlay`, `settings::ui` and every other `bevy_ui` node lay out against the window as before.
//! * The `vfx` materials do not read `View::viewport`, so nothing in `assets/shaders/vfx_*.wgsl`
//!   needs to learn about this.
//!
//! # The near-miss upstream plugin, and why this is not it
//!
//! bevy 0.19 ships `bevy::core_pipeline::fullscreen_material::FullscreenMaterialPlugin<T>` — a
//! generic "run a fragment shader over a fullscreen triangle in `Core3d`" plugin, with a
//! `FullscreenMaterial` trait for the shader path and the schedule position. It is close enough that
//! the next reader will ask why it is not used, so the answer is recorded here rather than
//! re-derived. **Both blockers were read out of the vendored 0.19.0 source, not inferred:**
//!
//! * **Its sampler is NEAREST, which is the one thing this module exists to avoid.**
//!   `init_pipeline` builds it with `render_device.create_sampler(&SamplerDescriptor::default())`
//!   (`bevy_core_pipeline-0.19.0/src/fullscreen_material.rs:151`), and wgpu-types 29's `FilterMode`
//!   derives `Default` with `#[default] Nearest` (`wgpu-types-29.0.4/src/texture.rs:727`). The
//!   sampler is a private field of `FullscreenMaterialPipeline<T>` with no hook to replace it, so a
//!   render-scale upscale through it would be chunky point magnification. (Its bind-group LAYOUT is
//!   already `filterable: true` / `SamplerBindingType::Filtering`, so only the sampler instance is
//!   wrong — which is exactly the kind of near-miss that costs an afternoon to discover.)
//! * **Its third binding is the MATERIAL's uniform, not the view's.** The layout is
//!   `(texture_2d, sampler, uniform_buffer::<T>(true))` (same file, line 147) and the bind group
//!   fills that slot from `ComponentUniforms<T>` (line 257). The shader therefore cannot read
//!   `View::main_pass_viewport` — the field bevy itself writes the resolution override into
//!   (`bevy_render-0.19.0/src/view/mod.rs:1035-1038`) and the field `MainPassResolutionOverride`'s
//!   own doc tells shaders to use. Passing the sub-rect as material data instead would reintroduce
//!   precisely the disagreement this module's scissor-consistency argument rules out: the stretch
//!   would be whatever the material said last, not what the main pass was actually scissored to.
//!
//! Its default schedule slot (`Core3dSystems::PostProcess`, before tonemapping) is also not where
//! this belongs, but that one is overridable through `FullscreenMaterial::schedule_configs` and is
//! not a reason on its own.
//!
//! # The pipeline position, and what it buys
//!
//! bevy 0.19's 3D pipeline runs `Prepass -> MainPass -> EarlyPostProcess -> PostProcess`, with the
//! UI pass after `PostProcess` and bloom + tonemapping inside it. Putting the upscale in
//! [`Core3dSystems::EarlyPostProcess`] therefore means the reduced-resolution region ends exactly at
//! the end of the 3D main pass: bloom, tonemapping and the whole UI run at native resolution on a
//! full-size image. That is Apple's prescribed architecture for a Mac game verbatim ("Render 2D UI
//! that matches the view's backing size and render 3D in a different render target… then upscale it
//! to the final drawable using a custom render pass").
//!
//! # Honest limits (all from the brief; none of these are bugs to be filed)
//!
//! * **No VRAM saving.** The `ViewTarget` main textures, the depth texture and the prepass textures
//!   are all still allocated at the full physical target size. Render scale saves SHADING, not
//!   memory.
//! * **Bloom and tonemapping do not scale.** They run in `PostProcess`, after this node, on the
//!   full-size image. If bloom ever dominates a frame, that is the brief's Route B conversation, not
//!   something to fix here.
//! * **The UI does not scale**, by design — that is the entire point, and the reason UI scale is a
//!   separate row.
//! * **The MSAA resolve stays full-res.** The sampled attachment is allocated at full size and the
//!   resolve covers it whatever the main pass was scissored to, so MSAA keeps costing what it cost.
//!   At scale < 1 the sensible pairing is 2x or off, which the settings row's hint says out loud.
//! * **50% and 100% are the honest snap points on macOS.** WindowServer quadruples pixels with no
//!   interpolation at exactly half the panel resolution; the intermediate rungs go through this
//!   node's bilinear filter and will look softer than their pixel count suggests.
//!
//! # Zero cost at 100%
//!
//! At `scale == 1.0` [`prepare_render_scale`] removes both the override and the per-view pipeline
//! marker, and the node's `ViewQuery` then fails to match, so [`render_scale_upscale`] is skipped
//! entirely — no bind group, no render pass, not even an empty one. The default is 100%, so a player
//! who never opens the settings page renders precisely what the game rendered before this module
//! existed.
//!
//! # What is proven, and what still needs a pair of eyes
//!
//! Verified 2026-07-27 on an Apple M4 / macOS 26.5 / Metal, `--offline`, with a scratch
//! `video.ron` at 75%: the file reaches [`RenderScale`] (0.75) in the RENDER world,
//! [`prepare_render_scale`] runs every frame in `RenderSystems::Prepare` with no schedule cycle and
//! no skipped system param, and the upscale pipeline — shader, `#import`s, filtering bind-group
//! layout, `Rgba16Float` target — compiles and links on this Metal device (reported `Ok` on frame
//! 3). The override math, the settings row, the ladder stepping and the persistence round-trip are
//! all pinned by tests.
//!
//! What could NOT be exercised from an automated session, and is therefore the human checklist:
//! macOS hands a background window no drawable at all (MEASURED: `ExtractedWindows` reports a
//! correct 2560x1440 physical size with `swap_chain_texture_view: None`), so no `ViewTarget` is ever
//! created, the override is never inserted and the upscale pass never runs. Everything from the
//! override onward — the picture, the seam-free edges, and the A/B showing the MAIN PASS span
//! falling while the bloom/UI spans hold (`cargo tracy`) — needs a foreground session.

use bevy::camera::{Camera3d, MainPassResolutionOverride};
use bevy::core_pipeline::{Core3d, Core3dSystems, FullscreenShader};
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::diagnostic::RecordDiagnostics;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_resource::binding_types::{sampler, texture_2d, uniform_buffer};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedRenderPipelineId,
    ColorTargetState, ColorWrites, FilterMode, FragmentState, LoadOp, MultisampleState, Operations,
    PipelineCache, RenderPassColorAttachment, RenderPassDescriptor, RenderPipelineDescriptor,
    Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages, SpecializedRenderPipeline,
    SpecializedRenderPipelines, StoreOp, TextureFormat, TextureSampleType,
};
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::view::{
    ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms, prepare_view_targets,
    prepare_view_uniforms,
};
use bevy::render::{GpuResourceAppExt, Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;

/// The upscale fragment shader, loaded through the ordinary asset server like the `vfx_*` materials
/// (it ships in `assets/shaders/`, which `scripts/package-macos.sh` copies into the bundle).
const SHADER_PATH: &str = "shaders/render_scale_upscale.wgsl";

/// The floor a scale is clamped to before it reaches the GPU. The settings ladder bottoms out at
/// 50%; this is a wider guard so a hand-edited `video.ron` cannot ask for a one-pixel main pass
/// (or, with a negative number, for an `as_uvec2` saturation surprise).
const MIN_SCALE: f32 = 0.25;

/// The fraction of the window the 3D main pass is rendered at. `1.0` = native, and `1.0` is the
/// default, so this module is inert until a player moves the row.
///
/// Main-world resource, extracted to the render world every frame. **Machine-local config: never
/// replicated, never part of a command, never read by the sim** — it is a view-layer number in the
/// ADR-0014 sense, and two players at different render scales must see the same battle.
#[derive(Resource, Clone, Copy, PartialEq, Debug, ExtractResource)]
pub(crate) struct RenderScale(pub(crate) f32);

impl Default for RenderScale {
    fn default() -> Self {
        Self(1.0)
    }
}

impl RenderScale {
    /// The main-pass size for a window whose physical target is `full`, or `None` when the main pass
    /// should simply run at `full` (scale 1.0, or a degenerate target).
    ///
    /// `None` is the "remove the override" answer, and it is deliberately NOT `Some(full)`: a
    /// present-but-equal override still costs the upscale pass, and `MainPassResolutionOverride`
    /// documents that it must be SMALLER than the viewport.
    pub(crate) fn main_pass_size(self, full: UVec2) -> Option<UVec2> {
        let scale = self.0.clamp(MIN_SCALE, 1.0);
        if scale >= 1.0 || full.min_element() == 0 {
            return None;
        }
        // At least one texel in each axis, and never larger than the target it is a fraction of.
        Some(
            (full.as_vec2() * scale)
                .as_uvec2()
                .max(UVec2::ONE)
                .min(full),
        )
    }
}

/// The resource + the render-app half. Mounted BY `settings::plugin`, not beside it: `apply_settings`
/// is the one writer of [`RenderScale`], so the writer mounts the owner and a root cannot compose a
/// settings page whose render-scale row reaches nothing. Never the headless server (which has no
/// render app at all — the `get_sub_app_mut` below would simply find nothing, but the server never
/// composes this).
pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<RenderScale>()
        .add_plugins(ExtractResourcePlugin::<RenderScale>::default());

    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .allow_ambiguous_resource::<SpecializedRenderPipelines<RenderScaleUpscalePipeline>>()
        .init_gpu_resource::<SpecializedRenderPipelines<RenderScaleUpscalePipeline>>()
        .add_systems(RenderStartup, init_upscale_pipeline)
        .add_systems(
            Render,
            prepare_render_scale
                .in_set(RenderSystems::Prepare)
                // After the view targets exist, so `ViewTarget::main_texture_format` can key the
                // pipeline; BEFORE `prepare_view_uniforms`, which is what reads the override into
                // `View::main_pass_viewport` — the value the upscale shader stretches from. Without
                // that second edge the uniform would lag the override by a frame on the press that
                // changes the scale, and the first frame of every change would be mis-stretched.
                .after(prepare_view_targets)
                .before(prepare_view_uniforms)
                // Same reason bevy's own `prepare_view_upscaling_pipelines` says it: touching
                // `PipelineCache` is ambiguous with everything else that touches it, and the
                // ordering that matters is expressed above.
                .ambiguous_with_all(),
        )
        .add_systems(
            Core3d,
            render_scale_upscale.in_set(Core3dSystems::EarlyPostProcess),
        );
}

/// The upscale pass's shared GPU state. Forked from `bevy_core_pipeline`'s `BlitPipeline` with two
/// deliberate differences, which are the whole reason it is a fork rather than a reuse:
///
/// * the texture binding is `filterable: true` and the sampler binding is
///   [`SamplerBindingType::Filtering`] (bevy's blit is `NonFiltering` / `filterable: false`, because
///   a 1:1 blit never interpolates) — without both, a linear sampler is a validation error;
/// * the sampler itself is linear rather than `SamplerDescriptor::default()`'s nearest, which is
///   what makes this an upscale instead of a chunky point-magnification.
#[derive(Resource)]
struct RenderScaleUpscalePipeline {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    fullscreen_shader: FullscreenShader,
    fragment_shader: Handle<Shader>,
}

/// Everything the upscale pipeline specializes on. The main texture's format is `Rgba16Float` under
/// `Hdr` (which `camera::spawn_camera` sets) and an 8-bit format without it, so it cannot be baked
/// in — and it is the only thing that varies, since the destination is always a single-sampled main
/// texture and the draw always replaces rather than blends.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct RenderScaleUpscaleKey {
    target_format: TextureFormat,
}

impl SpecializedRenderPipeline for RenderScaleUpscalePipeline {
    type Key = RenderScaleUpscaleKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("render_scale_upscale_pipeline".into()),
            layout: vec![self.layout.clone()],
            vertex: self.fullscreen_shader.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.fragment_shader.clone(),
                entry_point: Some("fragment".into()),
                targets: vec![Some(ColorTargetState {
                    format: key.target_format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            // The post-process source and destination are the single-sampled main textures; the
            // MSAA resolve has already happened by the time this runs.
            multisample: MultisampleState {
                count: 1,
                ..default()
            },
            ..default()
        }
    }
}

/// Which pipeline this view's upscale should use. Inserted and removed ATOMICALLY with
/// [`MainPassResolutionOverride`], which is what makes "the override is set" and "there is a working
/// upscale to undo it" the same condition — a scaled main pass with no upscale would present the
/// world in the top-left corner of the window.
#[derive(Component, PartialEq)]
struct RenderScaleUpscaleId(CachedRenderPipelineId);

fn init_upscale_pipeline(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    fullscreen_shader: Res<FullscreenShader>,
    asset_server: Res<AssetServer>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "render_scale_upscale_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                // Dynamic offset: one `ViewUniform` slab holds every view, indexed by the view's
                // own `ViewUniformOffset`.
                uniform_buffer::<ViewUniform>(true),
            ),
        ),
    );
    // Clamp-to-edge (the `SamplerDescriptor` default) plus linear mag/min. The shader's half-texel
    // clamp is what actually keeps taps inside the rendered region — the address mode only covers
    // the outer edge of the FULL texture, which is not where the seam is.
    let linear = render_device.create_sampler(&SamplerDescriptor {
        label: Some("render_scale_upscale_sampler"),
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        // `mipmap_filter` stays at the default: the main texture has exactly one mip level.
        ..default()
    });

    commands.insert_resource(RenderScaleUpscalePipeline {
        layout,
        sampler: linear,
        fullscreen_shader: fullscreen_shader.clone(),
        fragment_shader: asset_server.load(SHADER_PATH),
    });
}

/// Reconcile each 3D view's [`MainPassResolutionOverride`] to the extracted [`RenderScale`].
///
/// Three reasons to carry no override, all of which must also REMOVE a stale one (render-world view
/// entities are synced from the main world and persist across frames, so a component left behind
/// would outlive the condition that put it there):
///
/// * the scale is 1.0 — the shipped default, and the zero-cost path;
/// * the camera has an explicit `Camera::viewport` — then the main pass is already scissored to a
///   sub-rect, and this node's full-texture stretch would be wrong. The game's 3D camera never sets
///   one (`camera::spawn_camera`), so this is a guard against a future letterbox/split-screen
///   feature silently rendering garbage, not a case we have;
/// * the pipeline is not compiled yet. This one is not hypothetical: MEASURED 2026-07-27 on an
///   Apple M4 / macOS 26.5 / Metal, the upscale pipeline reports `ShaderNotLoaded` on frames 1 and 2
///   and reaches `Ok` on frame 3, because the fragment shader is an ordinary path asset and the load
///   is async (`block_on_render_pipeline` blocks on COMPILATION, and there is nothing to compile
///   until the asset arrives). Inserting the override during those frames would put the world in the
///   corner of the window; gating on `get_render_pipeline` makes that unreachable, and covers a
///   device-loss re-init for free.
///
/// Each transition is logged ONCE (`announced`, a render-world `Local`), because the fact this
/// module exists to deliver — "the main pass is now 1920x1080 inside a 2560x1440 window" — is
/// otherwise invisible from a log, and a per-frame line would be unreadable — a profiling number is
/// only worth having if it is attributable to a configuration after the fact.
///
/// # The system is unconditional, but the WRITES are edges
///
/// It deliberately carries no `run_if(resource_changed::<RenderScale>)`: the answer depends on the
/// window's physical size (a resize with the row untouched must re-derive the override) and on
/// whether the pipeline has finished compiling (the async load above), neither of which is a change
/// to the resource. So it runs every frame — and then writes NOTHING unless the desired state
/// differs from the components already on the view. Without that second half, the SHIPPED DEFAULT
/// path queued a `try_remove` for two components that were never there, on every view, on every
/// frame; and a scaled path re-inserted identical components just as often.
fn prepare_render_scale(
    mut commands: Commands,
    scale: Res<RenderScale>,
    mut pipeline_cache: ResMut<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<RenderScaleUpscalePipeline>>,
    upscale: Res<RenderScaleUpscalePipeline>,
    mut announced: Local<Option<Option<UVec2>>>,
    // `Option<&…>` rather than `Has<…>`: presence alone cannot tell a window RESIZE (same override
    // component, different value) from a steady frame, and re-inserting on every frame is the cost
    // being removed. Reading the values makes both edges exact.
    views: Query<
        (
            Entity,
            &ExtractedCamera,
            &ViewTarget,
            Option<&MainPassResolutionOverride>,
            Option<&RenderScaleUpscaleId>,
        ),
        With<Camera3d>,
    >,
) {
    for (entity, camera, target, override_now, id_now) in &views {
        let scaled = camera
            .physical_target_size
            .filter(|_| camera.viewport.is_none())
            .and_then(|full| scale.main_pass_size(full));

        // Both components are inserted and removed ATOMICALLY, so "clear" is one condition.
        let clear = |commands: &mut Commands| {
            if override_now.is_some() || id_now.is_some() {
                commands
                    .entity(entity)
                    .try_remove::<(MainPassResolutionOverride, RenderScaleUpscaleId)>();
            }
        };

        let Some(scaled) = scaled else {
            clear(&mut commands);
            announce(&mut announced, None, camera.physical_target_size);
            continue;
        };

        let id = pipelines.specialize(
            &pipeline_cache,
            &upscale,
            RenderScaleUpscaleKey {
                target_format: target.main_texture_format(),
            },
        );
        // Bevy's own upscaling prepare blocks here for the same reason: a frame that queued a
        // pipeline and then rendered nothing with it is a frame that submitted no GPU work.
        pipeline_cache.block_on_render_pipeline(id);
        if pipeline_cache.get_render_pipeline(id).is_none() {
            clear(&mut commands);
            continue;
        }

        // The pipeline id is compared too, not just the size: it moves when the main texture's
        // format does (HDR on/off), and a stale id would draw through the wrong pipeline at an
        // unchanged resolution — the one failure this edge check could otherwise introduce.
        let unchanged = override_now.is_some_and(|current| current.0 == scaled)
            && id_now.is_some_and(|current| current.0 == id);
        if !unchanged {
            commands
                .entity(entity)
                .try_insert((MainPassResolutionOverride(scaled), RenderScaleUpscaleId(id)));
        }
        announce(&mut announced, Some(scaled), camera.physical_target_size);
    }
}

/// Log a change of main-pass size, once per change. `state` is the last thing said, so a steady
/// render scale costs one comparison per view per frame and no formatting at all.
///
/// One latch for all views, because the game has exactly one 3D camera (`camera::spawn_camera`). Two
/// cameras at different target sizes would make this alternate — a cosmetic log defect, deliberately
/// not paid for with a per-entity map until there is a second camera to pay for it.
fn announce(state: &mut Local<Option<Option<UVec2>>>, scaled: Option<UVec2>, full: Option<UVec2>) {
    if **state == Some(scaled) {
        return;
    }
    **state = Some(scaled);
    let full = full.unwrap_or(UVec2::ZERO);
    match scaled {
        Some(scaled) => info!(
            "render scale: 3D main pass {}x{} inside a {}x{} target ({:.0}% of the pixels); \
             UI, bloom and tonemapping stay at {}x{}",
            scaled.x,
            scaled.y,
            full.x,
            full.y,
            100.0 * (scaled.x as f32 * scaled.y as f32).max(1.0)
                / (full.x as f32 * full.y as f32).max(1.0),
            full.x,
            full.y,
        ),
        // Also said on the FIRST observation, even though native is the shipped default: "the render
        // app is on the native path" is the sentence that distinguishes "the feature is off" from
        // "the feature never reached the render app", which is exactly what a session debugging a
        // render-scale complaint needs to know.
        None => info!("render scale: native — the 3D main pass fills the {full} target"),
    }
}

/// The upscale pass itself: stretch the rendered sub-rect across the full main texture.
///
/// `With<MainPassResolutionOverride>` is not decoration — [`ViewQuery`] SKIPS the system when the
/// current view does not match, which is how this node costs literally nothing at scale 1.0. The
/// override's value is not read here: the shader takes it from `View::main_pass_viewport`, so the
/// stretch can never disagree with the scissor the main pass actually used.
fn render_scale_upscale(
    view: ViewQuery<
        (&ViewTarget, &RenderScaleUpscaleId, &ViewUniformOffset),
        With<MainPassResolutionOverride>,
    >,
    pipeline_cache: Res<PipelineCache>,
    upscale: Res<RenderScaleUpscalePipeline>,
    view_uniforms: Res<ViewUniforms>,
    mut ctx: RenderContext,
) {
    let (target, upscale_id, view_offset) = view.into_inner();
    let Some(pipeline) = pipeline_cache.get_render_pipeline(upscale_id.0) else {
        return;
    };
    let Some(view_binding) = view_uniforms.uniforms.binding() else {
        return;
    };

    // Flips the view's main texture: everything downstream reads the upscaled result. This is a
    // commitment — having called it, we MUST write source to destination.
    let post_process = target.post_process_write();

    // Built fresh each frame rather than cached by texture id (the shape bevy's blit node uses):
    // the bind group has three entries whose invalidation conditions differ (the flipping main
    // texture, and the view uniform buffer, which is reallocated when the view count changes), and
    // one `create_bind_group` per frame is not a cost worth a cache-invalidation bug.
    let layout = pipeline_cache.get_bind_group_layout(&upscale.layout);
    let bind_group = ctx.render_device().create_bind_group(
        "render_scale_upscale_bind_group",
        &layout,
        &BindGroupEntries::sequential((
            post_process.source,
            &upscale.sampler,
            view_binding.clone(),
        )),
    );

    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    // Named so it shows up as its own row in `cargo tracy` (and in any `DiagnosticsStore` consumer),
    // which is how the "the main pass got cheaper and this pass is what it cost" A/B is read.
    let time_span = diagnostics.time_span(ctx.command_encoder(), "render_scale_upscale");
    {
        let mut render_pass = ctx
            .command_encoder()
            .begin_render_pass(&RenderPassDescriptor {
                label: Some("render_scale_upscale"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: post_process.destination,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        // Clear rather than Load: the fullscreen triangle covers every texel, and a
                        // clear is the cheap way to start a tile on Apple's tile-based GPUs.
                        load: LoadOp::Clear(LinearRgba::BLACK.into()),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        render_pass.set_pipeline(pipeline);
        render_pass.set_bind_group(0, &bind_group, &[view_offset.offset]);
        render_pass.draw(0..3, 0..1);
    }
    time_span.end(ctx.command_encoder());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped default is native, and native means NO override — the zero-cost path. `Some(full)`
    /// would be a different, worse answer: it still schedules the upscale pass, and
    /// `MainPassResolutionOverride` documents that it must be smaller than the viewport.
    #[test]
    fn native_scale_carries_no_override() {
        assert_eq!(RenderScale::default().0, 1.0);
        assert_eq!(
            RenderScale::default().main_pass_size(UVec2::new(2560, 1440)),
            None,
        );
        // Above 1.0 is not a supersample feature — it clamps to native, i.e. still no override.
        assert_eq!(
            RenderScale(1.5).main_pass_size(UVec2::new(2560, 1440)),
            None,
            "there is no upsample rung; anything >= 1.0 must resolve to the native path"
        );
    }

    /// The override math the whole feature rests on, at the five shipped rungs of a real Retina
    /// target. Note 50% and 100%: those are the macOS integer-scaling snap points, so they must land
    /// on exact halves rather than on a rounding artefact.
    #[test]
    fn the_shipped_ladder_scales_both_axes() {
        let full = UVec2::new(2560, 1440);
        for (scale, want) in [
            (0.85, UVec2::new(2176, 1224)),
            (0.75, UVec2::new(1920, 1080)),
            (0.67, UVec2::new(1715, 964)),
            (0.50, UVec2::new(1280, 720)),
        ] {
            assert_eq!(
                RenderScale(scale).main_pass_size(full),
                Some(want),
                "scale {scale}"
            );
        }
    }

    /// Degenerate targets must not produce a zero-sized or oversized main pass — a zero extent is an
    /// invalid scissor rect, and an override larger than the viewport is explicitly forbidden.
    #[test]
    fn degenerate_targets_stay_legal() {
        assert_eq!(RenderScale(0.5).main_pass_size(UVec2::ZERO), None);
        assert_eq!(RenderScale(0.5).main_pass_size(UVec2::new(1920, 0)), None);
        // One-texel axes round to zero before the clamp; the clamp is what keeps them legal.
        assert_eq!(
            RenderScale(0.5).main_pass_size(UVec2::ONE),
            Some(UVec2::ONE)
        );
        // A hand-edited config asking for something absurd is clamped, not obeyed.
        for absurd in [0.0, -4.0, f32::MIN_POSITIVE] {
            let size = RenderScale(absurd)
                .main_pass_size(UVec2::new(2560, 1440))
                .expect("a below-floor scale still renders, at the floor");
            assert!(size.min_element() > 0, "{absurd} produced {size}");
            assert!(size.x >= 640, "{absurd} fell below the {MIN_SCALE} floor");
        }
    }

    /// Every rung the settings ladder can produce is a legal override for a range of real window
    /// sizes — the cheap guard against a future rung being added to `RenderScaleLevel::ORDER` that
    /// this module cannot express.
    #[test]
    fn every_settings_rung_produces_a_legal_main_pass() {
        use crate::settings::RenderScaleLevel;

        for level in RenderScaleLevel::ORDER {
            for full in [
                UVec2::new(1280, 720),
                UVec2::new(1920, 1080),
                UVec2::new(3456, 2160),
            ] {
                let Some(size) = RenderScale(level.fraction()).main_pass_size(full) else {
                    assert_eq!(
                        level,
                        RenderScaleLevel::default(),
                        "{level:?} must scale — only the default rung is the native path"
                    );
                    continue;
                };
                assert!(size.min_element() > 0, "{level:?} at {full}");
                assert!(
                    size.x <= full.x && size.y <= full.y,
                    "{level:?} at {full} produced {size}, which is not SMALLER than the viewport"
                );
            }
        }
    }
}
