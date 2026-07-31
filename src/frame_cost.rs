//! Per-frame wall-clock cost recorder (`SPIKE_FRAME_COST=<path>`): an env-gated JSONL log, one row
//! per frame, `{t, frame_ms}` — plus the two validity signals the frame-budget sweep gates on
//! (occlusion transitions and the effective present mode, below).
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
//!
//! # Validity signals (the anti-fiction half)
//!
//! A hidden or fully-occluded window's present returns `SurfaceError::Occluded`, nothing ever
//! vsync-blocks, and the frame loop FREE-RUNS — `Time<Real>` rows keep flowing, every row-count
//! gate passes, and the numbers are fiction. The rows alone cannot distinguish that state, so this
//! recorder emits two extra signals for `scripts/perf/run-frame-sweep.sh` and
//! `scripts/perf/analyze.py` to gate on:
//!
//! * **Occlusion transitions** — bevy delivers winit's occlusion notifications as the
//!   `WindowOccluded` message (vendored bevy_window-0.19.0/src/event.rs: fires when a window is
//!   minimized, hidden, or fully covered, carrying `occluded: bool`). Each transition is written
//!   into the SAME stream as an `{t, occluded}` row (same clock as the frames it invalidates) and
//!   logged with the stable token `frame_cost: presentation occluded=`. The analyzer fails any
//!   stream whose occluded interval overlaps the post-warmup measurement window.
//!
//!   Scope, stated so nobody over-trusts it: this catches a window that becomes covered or
//!   minimized DURING a run, which is the operator-error case. It is not the lever against a
//!   window that was never shown at all — winit reports occlusion CHANGES, and a window created
//!   invisible need never report one. That case is closed upstream, in the runner: a real sweep
//!   refuses to start with the hidden-capture env exported and scrubs it from the child anyway.
//! * **The effective present mode** — `settings::normalize_vsync` can resolve an unsupported OFF
//!   back to ON, and a failed capability probe leaves `AutoNoVsync` free to negotiate down to
//!   `Fifo`; either way the frames become display-paced while the injected `video.ron` still says
//!   "Off". Once the probe answers ([`PresentCaps::answered`]), one loud line states the
//!   post-probe truth: `frame_cost: effective present mode Immediate, frame cap off` is the only
//!   spelling the sweep's real mode accepts as proof the run was uncapped.

use bevy::prelude::*;
use bevy::window::WindowOccluded;
use serde_json::json;

use crate::settings::{PresentCaps, Settings};
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
    app.add_systems(Update, (sample, record_occlusion, report_present_mode));
}

/// Append this frame's `{t, frame_ms}` row.
fn sample(mut cost: ResMut<FrameCost>, time: Res<Time<Real>>) {
    cost.sink.write(&json!({
        "t": time.elapsed_secs_f64(),
        "frame_ms": time.delta_secs_f64() * 1000.0,
    }));
}

/// Append an `{t, occluded}` row — and a loud log line — for every presentation-occlusion
/// transition (see the module doc's validity-signals section).
///
/// `warn!` rather than `info!`, deliberately: an occluded window mid-capture means the measurement
/// is already lost, and the operator should see that the moment it happens rather than when the
/// analyzer rejects the stream. The timestamp is the same `Time<Real>` clock as the frame rows, so
/// the analyzer can place the occluded interval against the frames it invalidates.
fn record_occlusion(
    mut cost: ResMut<FrameCost>,
    time: Res<Time<Real>>,
    mut transitions: MessageReader<WindowOccluded>,
) {
    for transition in transitions.read() {
        let t = time.elapsed_secs_f64();
        warn!(
            "frame_cost: presentation occluded={} at t={t:.3}s",
            transition.occluded
        );
        cost.sink
            .write(&json!({ "t": t, "occluded": transition.occluded }));
    }
}

/// State the EFFECTIVE present mode and frame-cap once the capability probe answers — the sweep's
/// proof that a run was truly uncapped, rather than silently normalized or negotiated back to a
/// display-paced mode (see the module doc's validity-signals section).
///
/// A `Local` latch rather than a run condition: the probe answers once and never changes again
/// (`settings::probe` writes its channel exactly once), so the line is emitted exactly once.
/// The resources are `Option` so a root that mounts this recorder without the settings plugin
/// (tests) simply never reports rather than failing to schedule.
fn report_present_mode(
    mut reported: Local<bool>,
    settings: Option<Res<Settings>>,
    caps: Option<Res<PresentCaps>>,
) {
    if *reported {
        return;
    }
    let (Some(settings), Some(caps)) = (settings, caps) else {
        return;
    };
    if !caps.answered() {
        return;
    }
    let cap = if settings.frame_limit_period(*caps).is_some() {
        "on"
    } else {
        "off"
    };
    info!(
        "frame_cost: effective present mode {:?}, frame cap {cap}",
        settings.present_mode(*caps)
    );
    *reported = true;
}
