//! Per-frame wall-clock cost recorder (`SPIKE_FRAME_COST=<path>`): an env-gated JSONL log, one row
//! per frame, `{t, frame_ms}`.
//!
//! RAW deltas, deliberately never averaged in-process: the frame-budget sweep's product signal is
//! tail stutter (p95/p99/worst frame), which any smoothing destroys — percentiles are
//! `scripts/perf/analyze.py`'s job. The measured quantity is exactly what bevy's own frame-time
//! diagnostic measures — the `Time<Real>` delta (vendored
//! bevy_diagnostic-0.19.0/src/frame_time_diagnostics_plugin.rs, `diagnostic_system`) — read
//! directly instead of through `FrameTimeDiagnosticsPlugin`, whose `Diagnostic` history exists to
//! smooth (`with_smoothing_factor` in its `build`) and adds nothing over the raw delta here.
//!
//! Rows go through the same [`JsonlSink`]/[`role_path`] discipline as the other `SPIKE_*`
//! recorders. The first frame's delta is 0 by construction (`Time<Real>` has no previous update);
//! it lands inside the warmup window every analysis discards, so it is written rather than
//! special-cased. Off (zero cost — nothing registered) unless the env var is set. Windowed roots
//! only: the headless server presents no frames.

use bevy::prelude::*;
use serde_json::json;

use crate::trace::{JsonlSink, role_path};

/// Open frame-cost sink, present only when recording is armed.
#[derive(Resource)]
struct FrameCost {
    sink: JsonlSink,
}

/// Mount the per-frame recorder when `SPIKE_FRAME_COST` is set.
pub fn client_plugin(app: &mut App) {
    let Some(path) = crate::env_value("SPIKE_FRAME_COST") else {
        return;
    };
    let resolved = role_path(&path, "client");
    let sink = match JsonlSink::create(&resolved) {
        Ok(sink) => sink,
        Err(err) => {
            error!("frame_cost: cannot open {}: {err}", resolved.display());
            return;
        }
    };
    info!("frame_cost: recording rows to {}", resolved.display());
    app.insert_resource(FrameCost { sink });
    app.add_systems(Update, sample);
}

/// Append this frame's `{t, frame_ms}` row.
fn sample(mut cost: ResMut<FrameCost>, time: Res<Time<Real>>) {
    cost.sink.write(&json!({
        "t": time.elapsed_secs_f64(),
        "frame_ms": time.delta_secs_f64() * 1000.0,
    }));
}
