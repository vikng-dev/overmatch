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

use std::collections::{BTreeMap, HashMap};

use bevy::diagnostic::DiagnosticsStore;
use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy::render::diagnostic::RenderDiagnosticsPlugin;
use serde_json::json;

use crate::trace::{JsonlSink, role_path};

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
