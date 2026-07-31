//! Per-render-pass cost recorder (`SPIKE_RENDER_COST=<path>`): an env-gated JSONL log of bevy's
//! built-in render-pass diagnostics, one row per span name per sample.
//!
//! Bevy's `RenderDiagnosticsPlugin` is auto-mounted only under `bevy/trace_tracy` (vendored
//! bevy_render-0.19.0/src/lib.rs:379-380); this module mounts it explicitly when the env var is
//! set, so pass timings are available WITHOUT paying for a tracy build. The plugin's recorder
//! publishes into `DiagnosticsStore` under paths of the shape `render/<span…>/elapsed_cpu` and
//! `render/<span…>/elapsed_gpu` (vendored bevy_render-0.19.0/src/diagnostic/internal.rs,
//! `diagnostic_path`); once per [`SAMPLE_PERIOD_S`] a sampler averages each diagnostic's history
//! and appends `{t, name, cpu_ms, gpu_ms}` rows through the same [`JsonlSink`] discipline the
//! `SPIKE_COST_TRACE` recorder uses. `scripts/render/analyze.py` consumes the rows.
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

use bevy::diagnostic::DiagnosticsStore;
use bevy::prelude::*;
use bevy::render::diagnostic::RenderDiagnosticsPlugin;
use serde_json::json;

use crate::trace::{JsonlSink, role_path};

/// Seconds between emitted samples. One row set per second is plenty: each diagnostic already
/// carries bevy's default 120-measurement history, so the written `average` smooths ~2 s of
/// frames at 60 fps and the file stays small over a long capture.
const SAMPLE_PERIOD_S: f64 = 1.0;

/// Open render-cost sink plus the last-emit clock, present only when recording is armed.
#[derive(Resource)]
struct RenderCost {
    sink: JsonlSink,
    /// `Time<Real>` seconds at the last emitted sample.
    last_emit: f64,
}

/// Windowed client: mount the render diagnostics recorder and the JSONL sampler when
/// `SPIKE_RENDER_COST` is set.
pub fn client_plugin(app: &mut App) {
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
    });
    app.add_systems(Update, sample);
}

/// Once per [`SAMPLE_PERIOD_S`]: fold the `render/*` diagnostics into one row per span name.
fn sample(mut cost: ResMut<RenderCost>, time: Res<Time<Real>>, store: Res<DiagnosticsStore>) {
    let t = time.elapsed_secs_f64();
    if t - cost.last_emit < SAMPLE_PERIOD_S {
        return;
    }
    cost.last_emit = t;
    // BTreeMap for a stable row order run-to-run; span count is ~tens, so per-second allocation
    // cost is noise.
    let mut spans: std::collections::BTreeMap<String, (Option<f64>, Option<f64>)> =
        std::collections::BTreeMap::new();
    for diagnostic in store.iter() {
        let Some(rest) = diagnostic.path().as_str().strip_prefix("render/") else {
            continue;
        };
        let slot = if let Some(name) = rest.strip_suffix("/elapsed_cpu") {
            &mut spans.entry(name.to_owned()).or_default().0
        } else if let Some(name) = rest.strip_suffix("/elapsed_gpu") {
            &mut spans.entry(name.to_owned()).or_default().1
        } else {
            // Pipeline-statistics and asset-diagnostic paths also live under `render/`;
            // this recorder is about time.
            continue;
        };
        *slot = diagnostic.average();
    }
    for (name, (cpu_ms, gpu_ms)) in &spans {
        cost.sink
            .write(&json!({ "t": t, "name": name, "cpu_ms": cpu_ms, "gpu_ms": gpu_ms }));
    }
}
