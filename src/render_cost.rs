//! Per-render-pass cost recorder (`SPIKE_RENDER_COST=<path>`): an env-gated JSONL log of bevy's
//! built-in render-pass diagnostics, fresh measurements only, one row per span name per sample
//! window.
//!
//! Bevy's `RenderDiagnosticsPlugin` is auto-mounted only under `bevy/trace_tracy` (vendored
//! bevy_render-0.19.0/src/lib.rs:379-380); this module mounts it explicitly when the env var is
//! set, so pass timings are available WITHOUT paying for a tracy build. The plugin's recorder
//! publishes into `DiagnosticsStore` under paths of the shape `render/<span…>/elapsed_cpu` and
//! `render/<span…>/elapsed_gpu` (vendored bevy_render-0.19.0/src/diagnostic/internal.rs,
//! `diagnostic_path`).
//!
//! Freshness discipline — the schema's whole point: bevy's `Diagnostic` keeps a rolling
//! 120-measurement history and the store RETAINS paths that stopped producing, so reading
//! `Diagnostic::average` once per second would (a) overlap successive ~2 s windows and smooth
//! real single-frame spikes away, and (b) keep re-emitting a dead pass's last value forever,
//! indistinguishable from a live pass. Instead, once per [`SAMPLE_PERIOD_S`] the sampler walks
//! each diagnostic's timestamped history (`measurements` on the vendored
//! bevy_diagnostic-0.19.0/src/diagnostic.rs `Diagnostic`) and consumes ONLY entries newer than
//! the newest timestamp it already emitted for that path. Each row is
//! `{t, name, cpu_ms, gpu_ms}` where `cpu_ms`/`gpu_ms` are ARRAYS of the window's raw per-frame
//! millisecond values in measurement order (null where that kind produced nothing fresh): raw
//! values so downstream percentiles are over real observations, arrays so the file stays at
//! ~tens of rows per second instead of thousands. A span with zero fresh measurements in a
//! window emits NOTHING — silence in the file means the pass did not run, never "still echoing
//! old news". `scripts/render/analyze.py` consumes the rows.
//!
//! Coverage honesty:
//! * the built-in span sites (~28 across bevy_core_pipeline/bevy_pbr/bevy_render 0.19 — counted
//!   2026-07-31) cover the main 2d/3d passes, prepass/deferred, bloom, tonemapping, upscaling,
//!   mesh preprocessing etc, but NOT the shadow pass: bevy's shadow node carries only a tracing
//!   `info_span!` (vendored bevy_pbr `ShadowPassNode`), so `cargo tracy` stays the shadow-cost
//!   instrument;
//! * `elapsed_gpu` is real on Vulkan/DX12 only. On macOS bevy no-ops encoder timestamps (vendored
//!   bevy_render-0.19.0/src/diagnostic/internal.rs:744-760, citing bevy#22257) and the client
//!   additionally disables `TIMESTAMP_QUERY` there (see `client_render_plugin` for the Metal
//!   crash it retires), so macOS rows carry `cpu_ms` and a null `gpu_ms`.
//!
//! Off (zero cost — nothing registered) unless the env var is set. Client-only: the headless
//! server has no render app.
//!
//! # Two frame-cost ablation instruments, each off unless its env var is set
//!
//! * **`SPIKE_PHASE_COUNTS=1`**: every [`PHASE_COUNT_PERIOD_S`], one `phase_census:` log line per
//!   view per 3D phase (opaque, alpha mask, transparent, both prepasses, shadow), and one per phase
//!   with its `indirect_draws`: the indirect draw commands batched for it this frame, over all its
//!   views. Bevy batches binned phases into multidraw sets, whose bins it keeps private, so the
//!   per-view lines count sets, and the per-phase total is the draw count: Metal has no native
//!   multi-draw-indirect, and wgpu-hal encodes one indirect draw per command on the CPU (vendored
//!   wgpu-hal-29.0.4/src/metal/command.rs, `draw_indexed_indirect`). Sorted phases draw one item
//!   per command. Read after the phases are batched, in `PrepareBindGroups`.
//! * **`SPIKE_ABLATE=<feature>[,<feature>…]`**: render features removed, to measure what each costs
//!   by difference. `bloom` and `depth_prepass` remove those components from every `Camera3d`;
//!   `depth_copy` removes `scene_depth::MainPassDepth`, so the camera's scene depth is drawn by
//!   bevy's own depth prepass again instead of copied from its main pass (the A/B for that copy;
//!   the picture is meant to stay the same, and it is chosen before the first frame renders);
//!   `env_light` stops `world` from filtering the sky's environment light and giving it to views;
//!   `ui` hides every UI root, which removes the UI's extraction and drawing but not its layout. An
//!   unknown name panics at boot, so a typo cannot read as a free feature. Never part of a shipped
//!   measurement: the picture changes.
//!   `inset_half`, `inset_masks`, `inset_bloom` price the pen cam corner's parts while it shows
//!   (`scripts/perf/run-pencam-attribution.sh`): its target at half size (the mask views follow,
//!   keyed by size), no mask cameras, no bloom on its camera.
//!   `terrain_macro` skips the terrain shader's per-pixel macro tint and `terrain_layers` draws the
//!   base pack only (skips the painted-layer loop), both through shader defs on the terrain
//!   material (`terrain_blend::TerrainAblate`); the picture changes (flatter ground).
//!   `terrain-casters`, `tree-casters`, `house-casters`, `tank-casters` suppress only casting
//!   through render policy; receivers and main-view geometry retain their roles.
//!   `terrain-field` selects the raster terrain shadow instead of the baked one
//!   (`terrain_shadow`): no field is baked and the terrain casts into the cascades again. Without
//!   it the terrain casts nothing, so `terrain-casters` changes nothing on the heightmap world.
//! * `SPIKE_SHADOW_FILTER=gaussian|hardware|temporal`, `SPIKE_SHADOW_SPLITS=<metres,...>` and
//!   `SPIKE_SHADOW_OVERLAP=<proportion>` independently override stock shadow controls at boot.
//!   Explicit splits select count and envelope; overlap and filter leave both untouched.

use std::any::TypeId;
use std::collections::{BTreeMap, HashMap};

use bevy::camera::visibility::VisibilitySystems;
use bevy::core_pipeline::core_3d::{AlphaMask3d, Opaque3d, Transparent3d};
use bevy::core_pipeline::prepass::{AlphaMask3dPrepass, DepthPrepass, Opaque3dPrepass};
use bevy::diagnostic::DiagnosticsStore;
use bevy::light::{CascadeShadowConfig, ShadowFilteringMethod};
use bevy::pbr::Shadow;
use bevy::platform::time::Instant;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::render::batching::gpu_preprocessing::IndirectParametersBuffers;
use bevy::render::diagnostic::RenderDiagnosticsPlugin;
use bevy::render::render_phase::{
    BinnedPhaseItem, BinnedRenderPhase, SortedPhaseItem, ViewBinnedRenderPhases,
    ViewSortedRenderPhases,
};
use bevy::render::view::RetainedViewEntity;
use bevy::render::{Render, RenderApp, RenderSystems};
use serde_json::json;

use crate::render_policy::CameraProfile;
use crate::scene_depth::MainPassDepth;
use crate::trace::{JsonlSink, role_path};

/// Seconds between `phase_census:` reports: the census is a step function of the scene, so a few
/// lines per run are enough and the log stays readable.
const PHASE_COUNT_PERIOD_S: f64 = 5.0;

/// Render features removed by `SPIKE_ABLATE` (module doc). Read by `world::light_battlefield_views`
/// for `env_light` and by `world::spawn_environment` for `terrain_field`; the other fields act here.
#[derive(Resource, Clone, Copy, Default, Debug)]
pub(crate) struct Ablate {
    bloom: bool,
    pub(crate) env_light: bool,
    depth_prepass: bool,
    depth_copy: bool,
    ui: bool,
    pub(crate) terrain_field: bool,
    terrain_casters: bool,
    tree_casters: bool,
    house_casters: bool,
    tank_casters: bool,
    /// The pen cam inset's target at half size, its masks with it.
    pub(crate) inset_half: bool,
    /// No mask cameras for the pen cam inset.
    pub(crate) inset_masks: bool,
    inset_bloom: bool,
    /// The terrain shader's macro tint skipped (`TERRAIN_NO_MACRO`).
    terrain_macro: bool,
    /// The terrain shader's painted-layer loop skipped (`TERRAIN_BASE_ONLY`).
    terrain_layers: bool,
}

impl Ablate {
    /// The terrain material's shader-def levers; read when `world` builds the material.
    pub(crate) fn terrain_ablate(self) -> crate::terrain_blend::TerrainAblate {
        crate::terrain_blend::TerrainAblate {
            macro_tint: self.terrain_macro,
            base_only: self.terrain_layers,
        }
    }

    /// A comma-separated feature list; `Err` names the first unknown feature.
    pub(crate) fn parse(list: &str) -> Result<Self, String> {
        let mut ablate = Self::default();
        for name in list
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            match name {
                "bloom" => ablate.bloom = true,
                "env_light" => ablate.env_light = true,
                "depth_prepass" => ablate.depth_prepass = true,
                "depth_copy" => ablate.depth_copy = true,
                "ui" => ablate.ui = true,
                "terrain-field" => ablate.terrain_field = true,
                "terrain-casters" => ablate.terrain_casters = true,
                "tree-casters" => ablate.tree_casters = true,
                "house-casters" => ablate.house_casters = true,
                "tank-casters" => ablate.tank_casters = true,
                "inset_half" => ablate.inset_half = true,
                "inset_masks" => ablate.inset_masks = true,
                "inset_bloom" => ablate.inset_bloom = true,
                "terrain_macro" => ablate.terrain_macro = true,
                "terrain_layers" => ablate.terrain_layers = true,
                unknown => return Err(unknown.to_owned()),
            }
        }
        Ok(ablate)
    }
}

/// View-only classification, inherited by mesh descendants. Tank ownership is read from the
/// existing tank root/part markers, including detached assemblies and track proxies.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) enum CasterClass {
    Terrain,
    Tree,
    House,
    Tank,
}

impl Ablate {
    pub(crate) fn suppresses_caster(self, class: CasterClass) -> bool {
        match class {
            CasterClass::Terrain => self.terrain_casters,
            CasterClass::Tree => self.tree_casters,
            CasterClass::House => self.house_casters,
            CasterClass::Tank => self.tank_casters,
        }
    }
}

/// Independent, boot-only stock shadow instruments; never persisted as player settings.
#[derive(Resource, Default, Debug)]
pub(crate) struct ShadowControls {
    filter: Option<ShadowFilteringMethod>,
    splits: Option<Vec<f32>>,
    overlap: Option<f32>,
}

impl ShadowControls {
    fn parse(filter: Option<&str>, splits: Option<&str>, overlap: Option<&str>) -> Self {
        let filter = filter.map(|value| match value {
            "gaussian" => ShadowFilteringMethod::Gaussian,
            "hardware" => ShadowFilteringMethod::Hardware2x2,
            "temporal" => ShadowFilteringMethod::Temporal,
            _ => panic!("SPIKE_SHADOW_FILTER: expected gaussian, hardware or temporal"),
        });
        let splits = splits.map(|value| {
            let bounds: Vec<f32> = value
                .split(',')
                .map(|v| {
                    v.trim()
                        .parse()
                        .expect("SPIKE_SHADOW_SPLITS: expected comma-separated metres")
                })
                .collect();
            // DERIVED stock minimum distance is 0.1 m; the shader supports at most four cascades.
            assert!(
                !bounds.is_empty()
                    && bounds.len() <= 4
                    && bounds.iter().all(|v| v.is_finite() && *v > 0.1)
                    && bounds.windows(2).all(|w| w[0] < w[1]),
                "SPIKE_SHADOW_SPLITS: expected increasing finite bounds above 0.1 m, at most four"
            );
            bounds
        });
        let overlap = overlap.map(|value| {
            let value: f32 = value
                .parse()
                .expect("SPIKE_SHADOW_OVERLAP: expected a proportion");
            assert!(
                value.is_finite() && (0.0..1.0).contains(&value),
                "SPIKE_SHADOW_OVERLAP: expected a finite proportion in [0,1)"
            );
            value
        });
        Self {
            filter,
            splits,
            overlap,
        }
    }

    /// Explicit bounds select their own count and envelope; absent fields retain the stock config.
    pub(crate) fn configure_cascades(&self, config: &mut CascadeShadowConfig) {
        if let Some(bounds) = &self.splits {
            config.bounds.clone_from(bounds);
        }
        if let Some(overlap) = self.overlap {
            config.overlap_proportion = overlap;
        }
    }
}

fn shadow_controls(app: &mut App) {
    let filter = crate::env_value("SPIKE_SHADOW_FILTER");
    let splits = crate::env_value("SPIKE_SHADOW_SPLITS");
    let overlap = crate::env_value("SPIKE_SHADOW_OVERLAP");
    if filter.is_none() && splits.is_none() && overlap.is_none() {
        return;
    }
    let controls = ShadowControls::parse(filter.as_deref(), splits.as_deref(), overlap.as_deref());
    info!("render_cost: shadow controls {controls:?}");
    app.insert_resource(controls)
        .add_systems(Update, apply_shadow_filter);
}

fn apply_shadow_filter(
    mut commands: Commands,
    controls: Res<ShadowControls>,
    cameras: Query<
        (
            Entity,
            Option<&crate::render_policy::CameraProfile>,
            Option<&ShadowFilteringMethod>,
        ),
        With<Camera3d>,
    >,
) {
    let Some(filter) = controls.filter else {
        return;
    };
    for (entity, profile, current) in &cameras {
        if profile.is_none_or(|p| p.draws_battlefield()) && current != Some(&filter) {
            commands.entity(entity).insert(filter);
            info!("render_cost: shadow filter applied {entity} {filter:?}");
        }
    }
}

/// Mount the two ablation instruments of the module doc, each only when its env var is set.
fn ablation_instruments(app: &mut App) {
    if let Some(list) = crate::env_value("SPIKE_ABLATE") {
        let ablate = Ablate::parse(&list).unwrap_or_else(|unknown| {
            panic!(
                "SPIKE_ABLATE: unknown feature {unknown:?} (known: bloom, env_light, \
                 depth_prepass, depth_copy, ui, terrain-field, terrain-casters, tree-casters, \
                 house-casters, tank-casters, inset_half, inset_masks, inset_bloom, terrain_macro, \
                 terrain_layers)"
            )
        });
        info!("render_cost: ablating {ablate:?}");
        app.insert_resource(ablate)
            .add_systems(Update, ablate_camera_features)
            .add_systems(
                PostUpdate,
                ablate_ui.before(VisibilitySystems::VisibilityPropagate),
            );
    }
    if crate::env_flag("SPIKE_PHASE_COUNTS", false)
        && let Some(render_app) = app.get_sub_app_mut(RenderApp)
    {
        render_app.add_systems(
            Render,
            log_phase_census.in_set(RenderSystems::PrepareBindGroups),
        );
    }
}

/// `depth_copy` relies on running in the first `Update`, after the `Startup` that spawns the camera
/// and before the first extraction: the render world must never see the marker leave a camera it
/// has drawn (`scene_depth`, "Chosen at boot").
fn ablate_camera_features(
    mut commands: Commands,
    ablate: Res<Ablate>,
    cameras: Query<
        (
            Entity,
            Has<Bloom>,
            Has<DepthPrepass>,
            Has<MainPassDepth>,
            Option<&CameraProfile>,
        ),
        With<Camera3d>,
    >,
) {
    for (camera, bloom, depth_prepass, depth_copy, profile) in &cameras {
        if ablate.bloom && bloom {
            commands.entity(camera).remove::<Bloom>();
        }
        if ablate.inset_bloom && bloom && matches!(profile, Some(CameraProfile::PenCamInset)) {
            commands.entity(camera).remove::<Bloom>();
            info!("render_cost: inset_bloom: bloom removed from the pen cam inset camera {camera}");
        }
        if ablate.depth_prepass && depth_prepass {
            commands.entity(camera).remove::<DepthPrepass>();
        }
        if ablate.depth_copy && depth_copy {
            commands.entity(camera).remove::<MainPassDepth>();
        }
    }
}

/// After every `Update` writer of a UI root's `Visibility` and before it propagates.
fn ablate_ui(
    ablate: Res<Ablate>,
    mut roots: Query<&mut Visibility, (With<Node>, Without<ChildOf>)>,
) {
    if !ablate.ui {
        return;
    }
    for mut visibility in &mut roots {
        visibility.set_if_neq(Visibility::Hidden);
    }
}

/// Render world: the `phase_census:` lines of the module doc.
fn log_phase_census(
    mut next_s: Local<Option<Instant>>,
    (opaque, alpha_mask, opaque_prepass, alpha_mask_prepass, shadow): (
        Option<Res<ViewBinnedRenderPhases<Opaque3d>>>,
        Option<Res<ViewBinnedRenderPhases<AlphaMask3d>>>,
        Option<Res<ViewBinnedRenderPhases<Opaque3dPrepass>>>,
        Option<Res<ViewBinnedRenderPhases<AlphaMask3dPrepass>>>,
        Option<Res<ViewBinnedRenderPhases<Shadow>>>,
    ),
    transparent: Option<Res<ViewSortedRenderPhases<Transparent3d>>>,
    indirect: Option<Res<IndirectParametersBuffers>>,
) {
    let now = Instant::now();
    if next_s.is_some_and(|next| now < next) {
        return;
    }
    *next_s = Some(now + core::time::Duration::from_secs_f64(PHASE_COUNT_PERIOD_S));
    log_binned("opaque", opaque.as_deref());
    log_binned("alpha_mask", alpha_mask.as_deref());
    log_sorted("transparent", transparent.as_deref());
    log_binned("opaque_prepass", opaque_prepass.as_deref());
    log_binned("alpha_mask_prepass", alpha_mask_prepass.as_deref());
    log_binned("shadow", shadow.as_deref());
    // Bevy keeps per-view batch_sets and multidraw bins private; the public indirect buffers
    // aggregate all views. No cheap per-cascade command count exists without renderer changes.
    info!(
        "phase_census: shadow per_cascade_indirect_draws=unavailable (private batch_sets/bins; view subview index identifies cascade; draws counts sets)"
    );
    let indirect = indirect.as_deref();
    log_indirect::<Opaque3d>("opaque", indirect);
    log_indirect::<AlphaMask3d>("alpha_mask", indirect);
    log_indirect::<Opaque3dPrepass>("opaque_prepass", indirect);
    log_indirect::<AlphaMask3dPrepass>("alpha_mask_prepass", indirect);
    log_indirect::<Shadow>("shadow", indirect);
}

/// One phase's indirect draw commands this frame, all views together (module doc).
fn log_indirect<P: 'static>(phase: &str, buffers: Option<&IndirectParametersBuffers>) {
    let Some(buffers) = buffers.and_then(|buffers| buffers.buffers.get(&TypeId::of::<P>())) else {
        return;
    };
    info!(
        "phase_census: {phase} all_views indirect_draws={} (indexed {}, non-indexed {})",
        buffers.indexed.batch_count() + buffers.non_indexed.batch_count(),
        buffers.indexed.batch_count(),
        buffers.non_indexed.batch_count(),
    );
}

/// `light/camera/subview` for a shadow view, `camera/-/0` for a camera's own view.
fn view_label(view: &RetainedViewEntity) -> String {
    format!(
        "{}/{}/{}",
        view.main_entity.id(),
        view.auxiliary_entity.id(),
        view.subview_index
    )
}

fn log_binned<BPI: BinnedPhaseItem>(phase: &str, phases: Option<&ViewBinnedRenderPhases<BPI>>) {
    let Some(phases) = phases else {
        return;
    };
    let mut lines: Vec<String> = phases
        .0
        .iter()
        .map(|(view, binned)| census_line(phase, view, binned))
        .collect();
    lines.sort();
    for line in lines {
        info!("{line}");
    }
}

fn census_line<BPI: BinnedPhaseItem>(
    phase: &str,
    view: &RetainedViewEntity,
    binned: &BinnedRenderPhase<BPI>,
) -> String {
    let bins = binned.batchable_meshes.len();
    let instances: usize = binned
        .batchable_meshes
        .values()
        .map(|bin| bin.entities().len())
        .sum();
    let unbatchable: usize = binned
        .unbatchable_meshes
        .values()
        .map(|set| set.entities.len())
        .sum();
    let non_mesh: usize = binned
        .non_mesh_items
        .values()
        .map(|set| set.entities.len())
        .sum();
    let multidraw_sets = binned.multidrawable_meshes.len();
    format!(
        "phase_census: {phase} view={} draws={} bins={bins} instances={instances} \
         unbatchable={unbatchable} non_mesh={non_mesh} multidraw_sets={multidraw_sets}",
        view_label(view),
        bins + unbatchable + non_mesh + multidraw_sets,
    )
}

fn log_sorted<SPI: SortedPhaseItem>(phase: &str, phases: Option<&ViewSortedRenderPhases<SPI>>) {
    let Some(phases) = phases else {
        return;
    };
    let mut lines: Vec<String> = phases
        .0
        .iter()
        .map(|(view, sorted)| {
            format!(
                "phase_census: {phase} view={} draws={}",
                view_label(view),
                sorted.items.len()
            )
        })
        .collect();
    lines.sort();
    for line in lines {
        info!("{line}");
    }
}

/// Seconds between sample windows. Freshness math sets the ceiling, not file size: bevy retains
/// only the newest 120 measurements per diagnostic, so the window must stay under 120 frames or
/// measurements fall off the back of the history unread. 0.5 s keeps the window inside the cap
/// up to 240 fps; row volume is one row per ACTIVE span per window.
const SAMPLE_PERIOD_S: f64 = 0.5;

/// Span name → the window's fresh raw values, `(cpu_ms, gpu_ms)`.
type FreshSpans = BTreeMap<String, (Vec<f64>, Vec<f64>)>;

/// Open render-cost sink plus the sampler's state, present only when recording is armed.
#[derive(Resource)]
struct RenderCost {
    sink: JsonlSink,
    /// `Time<Real>` seconds at the last emitted sample window.
    last_emit: f64,
    /// Newest measurement timestamp already emitted, per full diagnostic path
    /// (`…/elapsed_cpu` and `…/elapsed_gpu` are tracked independently).
    seen: HashMap<String, Instant>,
}

/// Windowed client: mount the render diagnostics recorder and the JSONL sampler when
/// `SPIKE_RENDER_COST` is set, and each ablation instrument of the module doc when its own is.
pub fn client_plugin(app: &mut App) {
    ablation_instruments(app);
    shadow_controls(app);
    let Some(path) = crate::env_value("SPIKE_RENDER_COST") else {
        return;
    };
    let resolved = role_path(&path, "client");
    let sink = match JsonlSink::create(&resolved) {
        Ok(sink) => sink,
        Err(err) => {
            error!("render_cost: cannot open {}: {err}", resolved.display());
            return;
        }
    };
    // A `bevy/trace_tracy` build has already mounted the plugin via `RenderPlugin` (the vendored
    // cfg cited in the module doc), and plugins are unique by default — adding it again would
    // panic, so the tracy + SPIKE_RENDER_COST combination has to check first.
    if !app.is_plugin_added::<RenderDiagnosticsPlugin>() {
        app.add_plugins(RenderDiagnosticsPlugin);
    }
    info!("render_cost: recording rows to {}", resolved.display());
    app.insert_resource(RenderCost {
        sink,
        last_emit: 0.0,
        seen: HashMap::new(),
    });
    app.add_systems(Update, sample);
}

/// Once per [`SAMPLE_PERIOD_S`]: emit each span's fresh measurements; stale spans stay silent.
fn sample(mut cost: ResMut<RenderCost>, time: Res<Time<Real>>, store: Res<DiagnosticsStore>) {
    let t = time.elapsed_secs_f64();
    if t - cost.last_emit < SAMPLE_PERIOD_S {
        return;
    }
    let cost = &mut *cost;
    cost.last_emit = t;
    for (name, (cpu_ms, gpu_ms)) in fresh_spans(&store, &mut cost.seen) {
        let cpu_ms = (!cpu_ms.is_empty()).then_some(cpu_ms);
        let gpu_ms = (!gpu_ms.is_empty()).then_some(gpu_ms);
        cost.sink
            .write(&json!({ "t": t, "name": name, "cpu_ms": cpu_ms, "gpu_ms": gpu_ms }));
    }
}

/// Fresh raw values per span name, in measurement order (oldest first).
///
/// "Fresh" means strictly newer than the newest timestamp already consumed for that diagnostic
/// path; `seen` advances as a side effect. A diagnostic whose history holds nothing new
/// contributes nothing, so a pass that ran once or stopped mid-capture goes silent instead of
/// replaying its last value forever.
fn fresh_spans(store: &DiagnosticsStore, seen: &mut HashMap<String, Instant>) -> FreshSpans {
    // BTreeMap for a stable row order run-to-run; span count is ~tens, so per-window allocation
    // cost is noise.
    let mut spans = FreshSpans::new();
    for diagnostic in store.iter() {
        let path = diagnostic.path().as_str();
        let Some(rest) = path.strip_prefix("render/") else {
            continue;
        };
        let (name, is_cpu) = if let Some(name) = rest.strip_suffix("/elapsed_cpu") {
            (name, true)
        } else if let Some(name) = rest.strip_suffix("/elapsed_gpu") {
            (name, false)
        } else {
            // Pipeline-statistics and asset-diagnostic paths also live under `render/`;
            // this recorder is about time.
            continue;
        };
        let last = seen.get(path).copied();
        let fresh: Vec<f64> = diagnostic
            .measurements()
            .filter(|m| last.is_none_or(|newest| m.time > newest))
            .map(|m| m.value)
            .collect();
        if fresh.is_empty() {
            continue;
        }
        if let Some(newest) = diagnostic.measurement() {
            seen.insert(path.to_owned(), newest.time);
        }
        let slot = spans.entry(name.to_owned()).or_default();
        if is_cpu {
            slot.0 = fresh;
        } else {
            slot.1 = fresh;
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use bevy::diagnostic::{Diagnostic, DiagnosticMeasurement, DiagnosticPath};

    use super::*;

    /// Attribution users rely on each exclusion preserving main-view visibility and receiving,
    /// including late meshes, cast-only proxies and tank parts detached from their root.
    #[test]
    fn caster_classes_are_independent_and_late_meshes_keep_the_policy() {
        use crate::render_policy::{VisualScope, plugin};
        use bevy::light::{NotShadowCaster, NotShadowReceiver};
        for (lever, excluded_class) in [
            ("terrain-casters", 0),
            ("tree-casters", 1),
            ("house-casters", 2),
            ("tank-casters", 3),
        ] {
            let mut app = App::new();
            app.add_plugins(plugin)
                .insert_resource(Ablate::parse(lever).unwrap());
            let roots: Vec<_> = [CasterClass::Terrain, CasterClass::Tree, CasterClass::House]
                .into_iter()
                .map(|class| app.world_mut().spawn(class).id())
                .collect();
            let tank = app.world_mut().spawn(crate::tank::Tank).id();
            let part = app.world_mut().spawn(crate::tank::TankRoot(tank)).id();
            let roots: Vec<_> = roots.into_iter().chain([part]).collect();
            app.update();
            let meshes: Vec<_> = roots
                .iter()
                .map(|root| {
                    app.world_mut()
                        .spawn((
                            Mesh3d(Handle::default()),
                            ChildOf(*root),
                            Visibility::Inherited,
                        ))
                        .id()
                })
                .collect();
            let proxy = app
                .world_mut()
                .spawn((
                    Mesh3d(Handle::default()),
                    ChildOf(part),
                    VisualScope::SHADOW_PROXY,
                ))
                .id();
            app.update();
            for (class, mesh) in meshes.into_iter().enumerate() {
                let entity = app.world().entity(mesh);
                assert_eq!(
                    entity.contains::<NotShadowCaster>(),
                    class == excluded_class
                );
                assert!(!entity.contains::<NotShadowReceiver>());
                assert_eq!(*entity.get::<Visibility>().unwrap(), Visibility::Inherited);
            }
            assert_eq!(
                app.world().entity(proxy).contains::<NotShadowCaster>(),
                excluded_class == 3
            );
        }
    }

    /// Pricing conditions must alter only their named stock control, leaving the other axes fixed.
    /// ASSUMED fixture bounds and overlap are test inputs, not product recommendations.
    #[test]
    fn shadow_controls_are_independent() {
        let base = CascadeShadowConfig::default();
        let mut config = base.clone();
        ShadowControls::parse(Some("hardware"), None, None).configure_cascades(&mut config);
        assert_eq!(config.bounds, base.bounds);
        assert_eq!(config.overlap_proportion, base.overlap_proportion);
        ShadowControls::parse(None, Some("60,180,350"), None).configure_cascades(&mut config);
        assert_eq!(config.bounds, vec![60.0, 180.0, 350.0]);
        assert_eq!(config.overlap_proportion, base.overlap_proportion);
        ShadowControls::parse(None, None, Some("0.05")).configure_cascades(&mut config);
        assert_eq!(config.bounds, vec![60.0, 180.0, 350.0]);
        assert_eq!(config.overlap_proportion, 0.05);
        assert_eq!(config.minimum_distance, base.minimum_distance);
        for (name, filter) in [
            ("gaussian", ShadowFilteringMethod::Gaussian),
            ("hardware", ShadowFilteringMethod::Hardware2x2),
            ("temporal", ShadowFilteringMethod::Temporal),
        ] {
            assert_eq!(
                ShadowControls::parse(Some(name), None, None).filter,
                Some(filter)
            );
        }
    }

    /// Bad conditions must fail before a battery can silently price a different experiment.
    #[test]
    fn invalid_shadow_controls_are_rejected() {
        for splits in ["", "350,40", "0,350", "NaN,350", "1,2,3,4,5"] {
            assert!(
                std::panic::catch_unwind(|| ShadowControls::parse(None, Some(splits), None))
                    .is_err()
            );
        }
        for overlap in ["NaN", "-0.1", "1"] {
            assert!(
                std::panic::catch_unwind(|| ShadowControls::parse(None, None, Some(overlap)))
                    .is_err()
            );
        }
        assert!(
            std::panic::catch_unwind(|| ShadowControls::parse(Some("typo"), None, None)).is_err()
        );
    }

    /// One measurement per simulated frame, 16 ms apart — real histories are never same-instant.
    fn diagnostic_with(path: &'static str, base: Instant, values: &[f64]) -> Diagnostic {
        let mut diagnostic = Diagnostic::new(DiagnosticPath::const_new(path));
        for (i, value) in values.iter().enumerate() {
            diagnostic.add_measurement(DiagnosticMeasurement {
                time: base + Duration::from_millis(16 * (i as u64 + 1)),
                value: *value,
            });
        }
        diagnostic
    }

    /// An ablation names exactly the features it removes, and a misspelt one is an error: a feature
    /// list that silently dropped an unknown name would price that feature at zero, and the night
    /// battery reads every `SPIKE_ABLATE` condition as the cost of the named feature.
    #[test]
    fn ablation_list_parses_known_features_and_rejects_unknown_ones() {
        let ablate = Ablate::parse("bloom, env_light,,ui").expect("known features");
        assert!(ablate.bloom && ablate.env_light && ablate.ui && !ablate.depth_prepass);
        assert!(!ablate.depth_copy);
        let ablate = Ablate::parse("inset_half,inset_masks,inset_bloom").expect("inset levers");
        assert!(ablate.inset_half && ablate.inset_masks && ablate.inset_bloom && !ablate.bloom);
        let terrain = Ablate::parse("terrain_macro")
            .expect("terrain lever")
            .terrain_ablate();
        assert!(terrain.macro_tint && !terrain.base_only);
        let terrain = Ablate::parse("terrain_layers")
            .expect("terrain lever")
            .terrain_ablate();
        assert!(terrain.base_only && !terrain.macro_tint);
        let ablate = Ablate::parse("depth_copy").expect("known feature");
        assert!(ablate.depth_copy && !ablate.depth_prepass);
        assert_eq!(
            Ablate::parse("bloom,envlight").unwrap_err(),
            "envlight",
            "a misspelt feature must be named, not ignored"
        );
    }

    #[test]
    fn stale_history_is_not_re_emitted() {
        let mut store = DiagnosticsStore::default();
        store.add(diagnostic_with(
            "render/main_opaque_pass_3d/elapsed_cpu",
            Instant::now(),
            &[1.25, 0.75],
        ));
        let mut seen = HashMap::new();

        let first = fresh_spans(&store, &mut seen);
        assert_eq!(first["main_opaque_pass_3d"].0, vec![1.25, 0.75]);

        // No new measurements between ticks: bevy RETAINS the whole history (and the replaced
        // `Diagnostic::average` kept answering from it) — the second window must emit nothing.
        let second = fresh_spans(&store, &mut seen);
        assert!(second.is_empty(), "stale diagnostic re-emitted: {second:?}");
    }

    #[test]
    fn single_spike_survives_at_full_magnitude() {
        // 59 quiet frames around one 50 ms spike: the replaced `Diagnostic::average` would have
        // reported ~1 ms and the spike would never reach the file.
        let mut values = vec![0.2; 59];
        values.insert(30, 50.0);
        let mut store = DiagnosticsStore::default();
        store.add(diagnostic_with(
            "render/main_opaque_pass_3d/elapsed_cpu",
            Instant::now(),
            &values,
        ));

        let mut seen = HashMap::new();
        let rows = fresh_spans(&store, &mut seen);
        let cpu = &rows["main_opaque_pass_3d"].0;
        assert_eq!(cpu.len(), 60, "every raw measurement must be emitted");
        assert_eq!(
            cpu.iter().copied().fold(f64::MIN, f64::max),
            50.0,
            "the spike must survive at full magnitude, not averaged away"
        );
    }

    #[test]
    fn kind_with_no_fresh_measurements_stays_empty() {
        const CPU: &str = "render/bloom/elapsed_cpu";
        let base = Instant::now();
        let mut store = DiagnosticsStore::default();
        store.add(diagnostic_with(CPU, base, &[0.5]));
        store.add(diagnostic_with("render/bloom/elapsed_gpu", base, &[0.4]));
        let mut seen = HashMap::new();
        fresh_spans(&store, &mut seen);

        // Only the CPU side advances before the next window (the macOS shape: GPU timestamps
        // no-op) — the GPU slot must come back empty, not echo its stale 0.4.
        store
            .get_mut(&DiagnosticPath::const_new(CPU))
            .expect("diagnostic just added")
            .add_measurement(DiagnosticMeasurement {
                time: base + Duration::from_secs(1),
                value: 0.6,
            });
        let rows = fresh_spans(&store, &mut seen);
        assert_eq!(rows["bloom"].0, vec![0.6]);
        assert!(rows["bloom"].1.is_empty(), "stale GPU kind must stay empty");
    }
}
