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
//!   While occluded, the recorder also asks for focus back once a second
//!   ([`raise_when_occluded`]) — parking the window on the primary display moved it off an empty
//!   external desktop and into the middle of the operator's own windows, so it now needs to insist.
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
//!   on every change, by polling winit's own `current_monitor()` (see [`record_monitor`], which
//!   also records why bevy's `OnMonitor` relationship cannot serve here). The park is
//!   best-effort; the ROW is the evidence, and `scripts/perf/analyze.py` fails any stream whose
//!   measurement window is covered by a monitor below 100 000 mHz, is not covered by a monitor row
//!   at all, or changes monitor mid-window.
//!
//! The parking is deliberately NOT a gate here: a client that refuses to measure is a worse
//! instrument than one that measures and states its own display provenance. Being wrong about the
//! panel and being unable to say which panel are the same evidential state, and both fail in the
//! analyzer — which is also why `refresh_mhz` is written as `null` rather than omitted when winit
//! cannot answer (winit's `MonitorHandle::refresh_rate_millihertz` is an `Option`; on macOS it
//! falls back to `CVDisplayLink` because `CGDisplayModeGetRefreshRate` reports 0 for the built-in
//! panel — vendored winit-0.30.13/src/platform_impl/macos/monitor.rs).
//!
//! Rows on CHANGE rather than per frame, polled once a second: the identity of the presenting
//! panel is a step function, and a row per frame would drown the frame rows it is meant to
//! qualify. The startup park itself can produce an early change row (winit creates the window
//! before the first `Startup` run, so a window born on the external panel is recorded there and
//! then again after the move) — which is why the analyzer's gate is scoped to the MEASUREMENT
//! window, exactly like the occlusion gate: pre-warmup churn is provenance, not a failure.
//!
//! # The surface size
//!
//! The refresh gate above passes on a window that is presenting the wrong NUMBER OF PIXELS, and
//! that is the second way this sweep reads as fiction. The two panels have different scale
//! factors: the ARZOPA is 1.0 and the built-in is 2.0, and bevy's default window is 1280x720
//! LOGICAL (vendored bevy_window-0.19.0/src/window.rs, `WindowResolution::default` — 1280x720
//! physical at scale 1.0). Born on the external panel the window is therefore a 1280x720 surface;
//! every dataset this sweep is compared against was captured on the built-in panel, where the same
//! logical window is a 2560x1440 surface. That is FOUR TIMES the shaded pixels. MEASURED: m350-2
//! read 4.19 ms here against 11.83 ms for identical code on the built-in panel — a ladder whose
//! every rung is three times too fast, with a refresh gate that passes.
//!
//! So the recorder also pins and records the surface:
//!
//! * **pins** the window to [`SURFACE_LOGICAL`] logical pixels at startup, and re-asserts it
//!   whenever the scale factor changes ([`hold_surface_pin`]). The logical size is the invariant
//!   worth pinning: it is what every prior capture held fixed, and at the built-in panel's scale
//!   factor it is exactly the 2560x1440 physical surface those captures measured;
//! * **records** `{t, surface_w, surface_h}` rows — bevy's own `Window::physical_size`, which is
//!   what the render app configures the surface to and what the camera reports as
//!   `physical_target_size`, so the recorded number is the one that costs GPU time rather than a
//!   number that merely correlates with it. `scripts/perf/analyze.py` fails any measurement window
//!   not covered by a surface row, covered by one that is not 2560x1440, or that resizes mid-window.

use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowOccluded};
use serde_json::json;

use crate::settings::{PresentCaps, Settings};
use crate::trace::{JsonlSink, role_path};

/// Where the capture window is parked at startup, in GLOBAL physical pixels — see the module doc's
/// presenting-monitor section. Small and positive so the window's top-left, and therefore the bulk
/// of the window, sits on the main display, which owns the origin of this coordinate space.
const PARK_AT: IVec2 = IVec2::new(120, 120);

/// How often the presenting monitor and surface size are re-resolved. Both are step functions
/// driven by human-scale events (a drag, a cable, a resize), so 1 Hz places any change well inside
/// the second of frames it invalidates — and the analyzer rejects the whole condition either way.
const MONITOR_POLL_S: f64 = 1.0;

/// The LOGICAL window size every frame-cost capture is pinned to — see the module doc's
/// surface-size section. bevy's own default, stated explicitly because the default is only reached
/// when nothing else has touched the window, and because the number that matters downstream is the
/// PHYSICAL surface it becomes: 2560x1440 on the built-in panel's scale factor of 2, which is what
/// `scripts/perf/analyze.py` gates on and what every dataset this sweep compares against measured.
const SURFACE_LOGICAL: Vec2 = Vec2::new(1280.0, 720.0);

/// Open frame-cost sink, present only when recording is armed.
#[derive(Resource)]
struct FrameCost {
    sink: JsonlSink,
    /// Latest presentation-occlusion state, as last reported by winit. Kept beside the sink rather
    /// than in a `Local` because two systems need it: the one that RECORDS the transition and the
    /// one that tries to undo it ([`raise_when_occluded`]).
    occluded: bool,
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
    app.insert_resource(FrameCost {
        sink,
        occluded: false,
    });
    app.add_systems(Startup, park_and_pin);
    app.add_systems(
        Update,
        (
            sample,
            record_occlusion,
            // After the recorder, so it acts on this frame's transition rather than last frame's.
            raise_when_occluded,
            record_monitor,
            // Before the recorder, so a re-pin lands in the same frame it is decided rather than
            // leaving one poll's worth of rows claiming the size it was about to correct.
            hold_surface_pin,
            record_surface,
            report_present_mode,
        )
            .chain(),
    );
}

/// Move the primary window onto the primary display and pin its logical size, once, at startup —
/// the two anti-fiction moves of the module doc (presenting monitor, surface size).
///
/// Best-effort by design and silent about failure: whether either landed is not this system's claim
/// to make, it is [`record_monitor`]'s and [`record_surface`]'s, and an operator who drags or
/// resizes the window afterwards is caught by the same rows rather than fought by a re-parking loop.
fn park_and_pin(mut windows: Query<&mut Window, With<PrimaryWindow>>) {
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    window.position = WindowPosition::At(PARK_AT);
    window.resolution.set(SURFACE_LOGICAL.x, SURFACE_LOGICAL.y);
}

/// Re-assert the logical pin when the scale factor changes — the one event that silently breaks it.
///
/// `WindowResolution` stores a PHYSICAL size and a scale factor, and the two are updated by
/// different winit events: `ScaleFactorChanged` writes the factor alone
/// (`react_to_scale_factor_change`, vendored bevy_winit-0.19.0/src/state.rs) while the physical size
/// only follows if a `Resized` arrives. So the moment the startup park carries the window from the
/// scale-1.0 external panel to the scale-2.0 built-in one, a window pinned once at startup can be
/// left claiming 1280x720 physical at scale 2 — half the logical size it was pinned to, a quarter of
/// the pixels every comparable dataset was captured at, and nothing in the frame rows says so.
///
/// Triggered by the scale factor rather than by the size mismatch itself, deliberately: that is the
/// event which invalidates the pin, so the re-assert is bounded by the number of monitor moves
/// instead of becoming a resize request every second against an OS that may be refusing it.
fn hold_surface_pin(
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut pinned_at_scale: Local<Option<f32>>,
    mut next_poll_s: Local<f64>,
    time: Res<Time<Real>>,
) {
    let t = time.elapsed_secs_f64();
    if t < *next_poll_s {
        return;
    }
    *next_poll_s = t + MONITOR_POLL_S;
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    let scale = window.resolution.scale_factor();
    if *pinned_at_scale == Some(scale) {
        return;
    }
    *pinned_at_scale = Some(scale);
    // `set` is logical: it multiplies by the CURRENT scale factor, which is the whole point here.
    // Guarded so a steady run never marks `Window` changed — every write to it costs bevy a pass
    // through `changed_windows`, which talks to the OS.
    let want = (SURFACE_LOGICAL * scale).as_uvec2();
    if window.resolution.physical_size() != want {
        info!(
            "frame_cost: re-pinning surface to {}x{} logical at scale {scale} (was {}x{} physical, \
             wanted {}x{}) at t={t:.3}s",
            SURFACE_LOGICAL.x,
            SURFACE_LOGICAL.y,
            window.resolution.physical_width(),
            window.resolution.physical_height(),
            want.x,
            want.y,
        );
        window.resolution.set(SURFACE_LOGICAL.x, SURFACE_LOGICAL.y);
    }
}

/// Raise the capture window while — and only while — it is OCCLUDED.
///
/// Parking the window on the primary display fixed the refresh problem and created this one: on the
/// external panel the window had an empty desktop to itself, and on the built-in one it lands in the
/// middle of whatever the operator is running. MEASURED after the park landed: the window was
/// covered 0.36 s after creation and stayed covered for the whole condition, so the sweep threw away
/// a 75-second run at the occlusion gate — twice.
///
/// The trigger is occlusion, never a timer and never startup, and that is the whole design. A
/// healthy capture never calls this, so the sweep does not become an app that grabs focus for a
/// minute at a time; an occluded capture is already producing free-run fiction that the analyzer
/// will reject, so at that moment the alternative to taking focus is not politeness, it is a
/// discarded condition. This is the visible frame sweep, the opposite case to the hidden net-capture
/// harness whose whole design is to never steal focus (`net::client`, `SPIKE_SIM_WINDOWED`).
///
/// Bounded to one request per poll, and loud each time: a window that asks and keeps losing leaves a
/// legible trail instead of an invisible fight, and the occlusion rows still fail the run.
fn raise_when_occluded(
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    cost: Res<FrameCost>,
    mut next_poll_s: Local<f64>,
    time: Res<Time<Real>>,
) {
    let t = time.elapsed_secs_f64();
    if !cost.occluded || t < *next_poll_s {
        return;
    }
    *next_poll_s = t + MONITOR_POLL_S;
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    // bevy turns this into `focus_window()` only on the false -> true EDGE (vendored
    // bevy_winit-0.19.0/src/system.rs, `changed_windows`), which is exactly the shape wanted: while
    // another app holds focus the field reads false, so each poll is one genuine request.
    if !window.focused {
        warn!("frame_cost: window occluded at t={t:.3}s — asking for focus back");
        window.focused = true;
    }
}

/// Append a `{t, surface_w, surface_h}` row whenever the physical surface size resolves or changes
/// (see the module doc's surface-size section).
///
/// Reads bevy's `Window::physical_size` rather than winit's drawable, because bevy's number is the
/// one with consequences: the render app configures the surface from it and the camera reports it as
/// `physical_target_size` (which is what `render_scale`'s "the 3D main pass fills the … target" line
/// prints). A row sourced from the OS could agree with the panel while the renderer quietly drew a
/// quarter of it, which is the exact failure this gate exists to catch.
fn record_surface(
    mut cost: ResMut<FrameCost>,
    time: Res<Time<Real>>,
    mut last: Local<Option<UVec2>>,
    mut next_poll_s: Local<f64>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let t = time.elapsed_secs_f64();
    if t < *next_poll_s {
        return;
    }
    *next_poll_s = t + MONITOR_POLL_S;
    let Ok(window) = windows.single() else {
        return;
    };
    let size = window.resolution.physical_size();
    if *last == Some(size) {
        return;
    }
    // A resize AFTER the first observation is the operator-error case (a dragged edge, a monitor
    // move that re-scaled the surface) and is loud; the first observation is just provenance.
    let message = format!(
        "frame_cost: surface {}x{} physical ({}x{} logical at scale {})",
        size.x,
        size.y,
        window.resolution.width(),
        window.resolution.height(),
        window.resolution.scale_factor(),
    );
    if last.is_some() {
        warn!("{message} — CHANGED at t={t:.3}s");
    } else {
        info!("{message} at t={t:.3}s");
    }
    cost.sink.write(&json!({
        "t": t,
        "surface_w": size.x,
        "surface_h": size.y,
    }));
    *last = Some(size);
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
/// Asks winit's `current_monitor()` directly, once a second, rather than reading bevy's `OnMonitor`
/// relationship — which is the same underlying call, but arrives through a change-detection path
/// that a capture never triggers. `changed_windows` is filtered by `Changed<Window>` and runs in
/// `Last` (vendored bevy_winit-0.19.0/src/lib.rs and src/system.rs), so the relationship is
/// evaluated only on frames where something WROTE to the `Window` component. On a hands-off capture
/// that is the startup park and nothing else, and at that instant the freshly created window is not
/// yet on a screen, so `current_monitor()` answers None and no relationship is inserted; the window
/// then never changes again and bevy never asks a second time. MEASURED: zero monitor rows across a
/// whole condition, which the analyzer correctly called invalid. Polling the OS ourselves removes
/// the dependency on being asked at the right moment.
///
/// The window handle lives in a thread-local, so this is a non-send system exactly like
/// `settings::observe_window_mode`. `current_monitor()` is None until the window is mapped, which
/// costs a poll or two at startup and is why the analyzer treats an unresolved measurement window
/// as INVALID rather than as unremarkable.
fn record_monitor(
    _non_send_marker: bevy::ecs::system::NonSendMarker,
    mut cost: ResMut<FrameCost>,
    time: Res<Time<Real>>,
    mut last: Local<Option<MonitorFacts>>,
    mut next_poll_s: Local<f64>,
    windows: Query<Entity, With<PrimaryWindow>>,
) {
    let t = time.elapsed_secs_f64();
    if t < *next_poll_s {
        return;
    }
    *next_poll_s = t + MONITOR_POLL_S;
    let Ok(entity) = windows.single() else {
        return;
    };
    let resolved = bevy::winit::WINIT_WINDOWS.with_borrow(|winit_windows| {
        let winit_window = winit_windows.get_window(entity)?;
        let current = winit_window.current_monitor()?;
        Some(MonitorFacts {
            // The panel's own report, in millihertz — the quantity that paces presentation, as
            // opposed to the present mode the app asked for.
            refresh_mhz: current.refresh_rate_millihertz(),
            primary: winit_window.primary_monitor().as_ref() == Some(&current),
            name: current.name(),
        })
    });
    let Some(facts) = resolved else {
        return;
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
        cost.occluded = transition.occluded;
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
