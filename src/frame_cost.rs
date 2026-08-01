//! Per-frame wall-clock cost recorder (`SPIKE_FRAME_COST=<path>`): an env-gated JSONL log, one row
//! per frame, `{t, frame_ms}` — plus the three validity signals the frame-budget sweep gates on
//! (occlusion transitions, the presenting monitor, and the effective present mode, below).
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
//! recorder emits extra signals for `scripts/perf/run-frame-sweep.sh` and
//! `scripts/perf/analyze.py` to gate on. The occlusion one is above; the display one has a section
//! of its own, because a window can be perfectly visible and still measure the wrong machine:
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
//! * **The presenting monitor** — see [the section below](#the-presenting-monitor); a 60 Hz panel
//!   paces every frame to a multiple of 16.67 ms while every other gate here still passes.
//! * **The effective present mode** — `settings::normalize_vsync` can resolve an unsupported OFF
//!   back to ON, and a failed capability probe leaves `AutoNoVsync` free to negotiate down to
//!   `Fifo`; either way the frames become display-paced while the injected `video.ron` still says
//!   "Off". Once the probe answers ([`PresentCaps::answered`]), one loud line states the
//!   post-probe truth: `frame_cost: effective present mode Immediate, frame cap off` is the only
//!   spelling the sweep's real mode accepts as proof the run was uncapped.
//!
//! # The presenting monitor
//!
//! A window presented on a 60 Hz panel is paced by macOS to multiples of 16.67 ms whatever present
//! mode the app negotiated: every rung of a shadow ladder quantizes to the same ~60 fps, and the
//! sweep is a fiction that LOOKS clean, because the present-mode line above reports what the APP
//! ASKED FOR, not how the OS paced the surface it got. Measured on this machine, same binary and
//! settings: shadows fully OFF read 16.632 ms on the 60 Hz external panel and 12.057 ms on the
//! built-in 120 Hz ProMotion one. So the recorder does two things:
//!
//! * **parks** the primary window at [`PARK_AT`] once, at startup. The coordinate space is global
//!   physical pixels whose origin is the top-left of the MAIN display (vendored
//!   bevy_winit-0.19.0/src/winit_windows.rs, `winit_window_position`: `WindowPosition::At` is
//!   handed to winit as a `PhysicalPosition` unchanged), so a small positive point lands on the
//!   primary panel by construction whatever else is plugged in;
//! * **records** which monitor is actually presenting, as `{t, monitor, refresh_mhz, primary}`
//!   rows in the same stream and on the same clock as the frames — at first resolution and again
//!   on every change. The park is best-effort; the ROW is the evidence, and
//!   `scripts/perf/analyze.py` fails any stream whose measurement window is covered by a monitor
//!   below 100 000 mHz, is not covered by a monitor row at all, or changes monitor mid-window.
//!
//! The parking is deliberately NOT a gate here: a client that refuses to measure is a worse
//! instrument than one that measures and states its own display provenance. Being wrong about the
//! panel and being unable to say which panel are the same evidential state, and both fail in the
//! analyzer — which is also why `refresh_mhz` is written as `null` rather than omitted when winit
//! cannot answer (vendored bevy_window-0.19.0/src/monitor.rs: `Monitor::refresh_rate_millihertz`
//! is an `Option`).
//!
//! Rows on CHANGE rather than per frame, polled once a second: the identity of the presenting
//! panel is a step function, and a row per frame would drown the frame rows it is meant to
//! qualify. The startup park itself can produce an early change row (winit creates the window
//! before the first `Startup` run, so a window born on the external panel is recorded there and
//! then again after the move) — which is why the analyzer's gate is scoped to the MEASUREMENT
//! window, exactly like the occlusion gate: pre-warmup churn is provenance, not a failure.

use bevy::prelude::*;
use bevy::window::{Monitor, OnMonitor, PrimaryMonitor, PrimaryWindow, WindowOccluded};
use serde_json::json;

use crate::settings::{PresentCaps, Settings};
use crate::trace::{JsonlSink, role_path};

/// Where the capture window is parked at startup, in GLOBAL physical pixels — see the module doc's
/// presenting-monitor section. Small and positive so the window's top-left, and therefore the bulk
/// of the window, sits on the main display, which owns the origin of this coordinate space.
const PARK_AT: IVec2 = IVec2::new(120, 120);

/// How often the presenting monitor is re-resolved. The identity of a panel is a step function
/// driven by human-scale events (a drag, a cable), so 1 Hz places any change well inside the
/// second of frames it invalidates — and the analyzer rejects the whole condition either way.
const MONITOR_POLL_S: f64 = 1.0;

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
    app.add_systems(Startup, park_on_primary);
    app.add_systems(
        Update,
        (
            sample,
            record_occlusion,
            record_monitor,
            report_present_mode,
        ),
    );
}

/// Move the primary window onto the primary display, once, at startup — the anti-60 Hz half of the
/// presenting-monitor signal (see the module doc).
///
/// Best-effort by design and silent about failure: whether the move landed is not this system's
/// claim to make, it is [`record_monitor`]'s, and an operator who drags the window afterwards is
/// caught by the same rows rather than fought by a re-parking loop.
fn park_on_primary(mut windows: Query<&mut Window, With<PrimaryWindow>>) {
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    window.position = WindowPosition::At(PARK_AT);
}

/// The presenting panel's identity, as recorded — the tuple the analyzer gates on, so "changed" is
/// defined by exactly the fields a reader of the stream can see.
#[derive(Clone, PartialEq, Eq)]
struct MonitorFacts {
    name: Option<String>,
    refresh_mhz: Option<u32>,
    primary: bool,
}

/// Append a `{t, monitor, refresh_mhz, primary}` row whenever the window's presenting monitor
/// resolves or changes (see the module doc's presenting-monitor section).
///
/// The monitor is read through the window's `OnMonitor` relationship, which bevy maintains from
/// winit's `current_monitor()` — the OS's own answer to "which panel is this surface on", not an
/// inference from coordinates (vendored bevy_winit-0.19.0/src/system.rs, `changed_windows`). It is
/// absent for the first frames: the relationship is inserted after window creation, so a stream
/// simply has no monitor row until then, and the analyzer treats a measurement window that no row
/// covers as INVALID rather than as unremarkable.
fn record_monitor(
    mut cost: ResMut<FrameCost>,
    time: Res<Time<Real>>,
    mut last: Local<Option<MonitorFacts>>,
    mut next_poll_s: Local<f64>,
    windows: Query<&OnMonitor, With<PrimaryWindow>>,
    monitors: Query<(&Monitor, Has<PrimaryMonitor>)>,
) {
    let t = time.elapsed_secs_f64();
    if t < *next_poll_s {
        return;
    }
    *next_poll_s = t + MONITOR_POLL_S;
    let Ok(on_monitor) = windows.single() else {
        return;
    };
    let Ok((monitor, primary)) = monitors.get(on_monitor.0) else {
        return;
    };
    let facts = MonitorFacts {
        name: monitor.name.clone(),
        refresh_mhz: monitor.refresh_rate_millihertz,
        primary,
    };
    if last.as_ref() == Some(&facts) {
        return;
    }
    // A change AFTER the first resolution is the operator-error case (a dragged window, a display
    // re-arrangement) and is loud; the first resolution is just provenance.
    let name = facts.name.as_deref().unwrap_or("<unnamed>");
    let refresh = facts
        .refresh_mhz
        .map_or_else(|| "unknown".to_string(), |mhz| format!("{mhz} mHz"));
    if last.is_some() {
        warn!(
            "frame_cost: presenting on monitor {name:?} ({refresh}, primary={}) — CHANGED at \
             t={t:.3}s",
            facts.primary
        );
    } else {
        info!(
            "frame_cost: presenting on monitor {name:?} ({refresh}, primary={}) at t={t:.3}s",
            facts.primary
        );
    }
    cost.sink.write(&json!({
        "t": t,
        "monitor": facts.name,
        "refresh_mhz": facts.refresh_mhz,
        "primary": facts.primary,
    }));
    *last = Some(facts);
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
