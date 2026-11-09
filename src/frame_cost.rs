//! Per-frame wall-clock recorder (`SPIKE_FRAME_COST=<path>`): JSONL `{t, frame_ms, drawn}` rows,
//! plus occlusion, monitor and surface rows and an effective-present-mode log for sweep validity.
//!
//! Raw deltas preserve tail stutter (p95/p99/worst frame); `scripts/perf/analyze.py` computes percentiles.
//!
//! `frame_ms` uses [`sample`]'s monotonic `Instant`, not the `Time<Real>` delta read by bevy's
//! diagnostic (vendored bevy_diagnostic-0.19.1, `frame_time_diagnostics_plugin.rs`, `diagnostic_system`).
//! Manual `TimeUpdateStrategy` fixes that delta to the flight step regardless of cost, as in the
//! armour sandbox (vendored bevy_time-0.19.1/src/lib.rs, `time_system`). DERIVED from source, not
//! measured side by side: under the default strategy both read the loop period at different points
//! (`Time<Real>` at the render world's `send_time`, this recorder in `Update`).
//!
//! `t` is `Time<Real>`'s elapsed seconds, shared with pen-cam marks (`scripts/perf/pencam_cost.py`
//! joins them). Under manual time, equal `t` identifies the same flight frame across runs; analyzer
//! warmup and span use those strategy seconds, not wall seconds.
//!
//! Uses [`JsonlSink`]/[`role_path`] like the other `SPIKE_*` recorders. The first `frame_ms` is 0
//! (no previous row): positive warmup discards it; `--warmup-s 0` keeps it. Unset means
//! nothing registered and zero cost. Headless clients record update frames without window or
//! render systems; servers mount no recorder.
//!
//! # Validity signals (the anti-fiction half)
//!
//! A hidden or fully occluded window's present returns `SurfaceError::Occluded`: no vsync block,
//! so free-running rows pass row-count gates without measuring presentation. Visible windows can
//! also measure the wrong display. `scripts/perf/run-frame-sweep.sh` and `scripts/perf/analyze.py`
//! therefore gate on these additional signals:
//!
//! * **Occlusion transitions:** `WindowOccluded` carries winit's `occluded: bool` when minimized,
//!   hidden or fully covered (vendored bevy_window-0.19.0/src/event.rs). `{t, occluded}` rows share
//!   the frames' stream and clock; logs use the stable token `frame_cost: presentation occluded=`.
//!   Any occluded interval overlapping the post-warmup measurement window fails analysis.
//!
//!   [`raise_when_occluded`] requests focus once a second: primary-display parking can put the
//!   window among the operator's windows instead of on an empty external desktop.
//!
//!   This catches covering/minimizing during a run, not an invisible window that never emits an
//!   occlusion change. The real-sweep runner refuses an exported hidden-capture env and scrubs it
//!   from the child.
//! * **The presenting monitor** — see [the section below](#the-presenting-monitor); a 60 Hz panel
//!   paces every frame to a multiple of 16.67 ms while every other gate here still passes.
//! * **Effective present mode:** `settings::normalize_vsync` can turn unsupported OFF into ON;
//!   failed probes let `AutoNoVsync` negotiate `Fifo`. Both pace frames despite `video.ron` saying
//!   "Off". After [`PresentCaps::answered`], real mode accepts only the log spelling
//!   `frame_cost: effective present mode Immediate, frame cap off` as uncapped proof.
//!
//! # The presenting monitor
//!
//! macOS paces a 60 Hz panel in 16.67 ms multiples (DERIVED from refresh), whatever present mode
//! the app requests: shadow-ladder rungs all quantize to ~60 fps despite an uncapped log line.
//! MEASURED on this machine, same binary/settings, shadows OFF: 16.632 ms on the 60 Hz external
//! panel versus 12.057 ms on the built-in 120 Hz ProMotion panel. The recorder therefore:
//!
//! * **parks** the primary window at [`PARK_AT`] once, at startup. The coordinate space is global
//!   physical pixels whose origin is the top-left of the MAIN display (vendored
//!   bevy_winit-0.19.0/src/winit_windows.rs, `winit_window_position`: `WindowPosition::At` is
//!   handed to winit as a `PhysicalPosition` unchanged), so a small positive point lands on the
//!   primary panel by construction whatever else is plugged in;
//! * **records** `{t, monitor, refresh_mhz, primary}` on first resolution and changes, sharing the
//!   frames' stream and clock. [`record_monitor`] polls winit's `current_monitor()` and explains why
//!   `OnMonitor` cannot serve. Parking is best-effort; analysis requires monitor-row coverage,
//!   refresh of at least 100 000 mHz and no monitor change within the measurement window.
//!
//! Keep recording if parking fails, to retain display provenance; unknown and wrong panels both
//! fail analysis. Unknown `refresh_mhz` is explicitly `null`: `MonitorHandle::refresh_rate_millihertz`
//! returns an `Option` (vendored winit-0.30.13/src/platform_impl/macos/monitor.rs). macOS falls back
//! to `CVDisplayLink` because `CGDisplayModeGetRefreshRate` reports 0 for the built-in panel.
//!
//! Poll once a second and emit changes: panel identity is a step function; per-frame monitor rows
//! would drown frame data. winit creates the window before `Startup`, so parking can record the
//! birth panel then a move. Like occlusion, only measurement-window churn fails; warmup is provenance.
//!
//! # The surface size
//!
//! Refresh alone cannot prove pixel count. Bevy's default is 1280x720 logical
//! (vendored bevy_window-0.19.0/src/window.rs, `WindowResolution::default`: 1280x720 physical at
//! scale 1.0). ARZOPA's scale 1.0 gives that surface; built-in scale 2.0 gives the reference
//! datasets' 2560x1440, four times the pixels (DERIVED). MEASURED: m350-2 read 4.19 ms on the
//! external panel versus 11.83 ms for identical code on the built-in: about three times too fast
//! (DERIVED), despite passing refresh.
//! The recorder also:
//!
//! * **pins** the window to [`SURFACE_LOGICAL`] logical pixels at startup, and re-asserts it
//!   whenever the scale factor changes ([`hold_surface_pin`]). The logical size is the invariant
//!   worth pinning: it is what every prior capture held fixed, and at the built-in panel's scale
//!   factor it is exactly the 2560x1440 physical surface those captures measured;
//! * **records** `{t, surface_w, surface_h}` from bevy's `Window::physical_size`, the renderer's
//!   configured surface and camera's `physical_target_size`: the pixels that cost GPU time.
//!   Analysis requires 2560x1440 surface-row coverage with no resize in the measurement window.
//!
//! # Drawn frames: the positive proof
//!
//! An occluded-from-birth window can pass negative gates while never acquiring a swapchain texture
//! or `ViewTarget`; its camera is skipped. MEASURED diagnosis and alternatives:
//! `.agents/scratch/perf-2026-10-29/focus-gap-diagnosis.md`.
//! [`count_drawn`] counts render frames with a `Camera3d` `ViewTarget`; frame rows carry that
//! running `drawn` count via `Arc<AtomicU64>`. Analysis requires advancement on nearly every row.
//! Pipelined rendering puts the count a frame behind, shifting the window without changing its rate.
//!
//! # Offscreen mode (`OVERMATCH_WINDOW=offscreen`)
//!
//! The root hides the window (`crate::run_window`); from its first frame, each window camera targets
//! one image: [`SURFACE_LOGICAL`] × [`OFFSCREEN_SCALE_FACTOR`] = 2560x1440 physical, matching the
//! visible harness. No presentation means no parking, pinning, raising or monitor queries.
//! Surface rows use the 3D camera's target size and `offscreen: true`; analysis skips display and
//! occlusion gates, retaining surface and drawn gates. MEASURED using the reused `OVERMATCH_CAPTURE`
//! retarget: within 0.1 ms p50 of same-binary visible runs at 2 tanks, 0.7 ms at 30
//! (`focus-gap-diagnosis.md`). Offscreen omits the compositor's share.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bevy::camera::{Camera3d, ImageRenderTarget, RenderTarget};
use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy::render::view::{ViewTarget, prepare_view_targets};
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy::ui::IsDefaultUiCamera;
use bevy::window::{PrimaryWindow, WindowOccluded};
use serde_json::json;

use crate::run_window::RunWindow;
use crate::settings::{PresentCaps, Settings};
use crate::trace::{JsonlSink, role_path};

/// Startup park in global physical pixels; the module's presenting-monitor section owns its rationale.
static PARK_AT: std::sync::LazyLock<IVec2> = std::sync::LazyLock::new(|| {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Placement {
        park_at_physical_px: crate::provenance::Sourced<(i32, i32)>,
    }
    let placement: Placement = crate::provenance::from_shipped_str(
        "assets/dev/frame_cost.ron",
        include_str!("../assets/dev/frame_cost.ron"),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let (x, y) = placement.park_at_physical_px.0;
    IVec2::new(x, y)
});

/// Resolution: 1 Hz polling bounds monitor/surface changes from human-scale drags, cables and
/// resizes within a second of affected frames; analysis rejects the whole condition either way.
const MONITOR_POLL_S: f64 = 1.0;

/// Explicit logical pin, independent of prior window edits; the module's surface-size section owns
/// the default-size and physical-surface contract with `scripts/perf/analyze.py`.
const SURFACE_LOGICAL: Vec2 = Vec2::new(1280.0, 720.0);

/// The offscreen image's scale factor: the built-in panel's, so [`SURFACE_LOGICAL`] becomes the same
/// 2560x1440 physical surface the visible harness gates on, and the UI lays out at the same size.
const OFFSCREEN_SCALE_FACTOR: f32 = 2.0;

#[derive(Resource, Clone, Default)]
struct DrawnFrames(Arc<AtomicU64>);

/// Offscreen mode's render target, created on the first frame; the resource's presence is the mode.
#[derive(Resource)]
struct Offscreen {
    target: Option<ImageRenderTarget>,
}

/// Open frame-cost sink, present only when recording is armed.
#[derive(Resource)]
struct FrameCost {
    sink: JsonlSink,
    /// Winit's latest occlusion state; shared rather than `Local` for recording and [`raise_when_occluded`].
    occluded: bool,
    /// When the previous frame row was sampled: the start of the next row's `frame_ms`.
    last_sample: Option<Instant>,
}

/// Mount the per-frame recorder when `SPIKE_FRAME_COST` is set.
///
/// # Panics
///
/// When the run is offscreen without it: nothing else renders into the offscreen image.
pub fn client_plugin(app: &mut App) {
    let window = RunWindow::of_run(RunWindow::Visible);
    let offscreen = window == RunWindow::Offscreen;
    let Some(path) = crate::env_value("SPIKE_FRAME_COST").unwrap_or_else(crate::refuse_start)
    else {
        assert!(
            !offscreen,
            "OVERMATCH_WINDOW=offscreen renders into the frame-cost recorder's image: set \
             SPIKE_FRAME_COST"
        );
        return;
    };
    let resolved = role_path(&path, "client");
    let sink = JsonlSink::create(&resolved).unwrap_or_else(crate::refuse_start);
    info!("frame_cost: recording rows to {}", resolved.display());
    app.insert_resource(FrameCost {
        sink,
        occluded: false,
        last_sample: None,
    });
    let drawn = DrawnFrames::default();
    app.insert_resource(drawn.clone());
    if window == RunWindow::Headless {
        app.add_systems(Update, sample);
        return;
    }
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app.insert_resource(drawn).add_systems(
            Render,
            count_drawn
                .in_set(RenderSystems::Prepare)
                .after(prepare_view_targets),
        );
    }
    if offscreen {
        let size = SURFACE_LOGICAL * OFFSCREEN_SCALE_FACTOR;
        info!(
            "frame_cost: offscreen: hidden window, cameras render into a {}x{} image at scale \
             {OFFSCREEN_SCALE_FACTOR}",
            size.x, size.y,
        );
        app.insert_resource(Offscreen { target: None });
        app.add_systems(
            Update,
            (
                hold_offscreen_target,
                sample,
                record_occlusion,
                record_surface,
                report_present_mode,
            )
                .chain(),
        );
        return;
    }
    app.add_systems(Startup, park_and_pin);
    app.add_systems(
        Update,
        (
            sample,
            record_occlusion,
            // Act on this frame's occlusion transition.
            raise_when_occluded,
            record_monitor,
            // Re-pin before recording, avoiding a poll's worth of stale-size rows.
            hold_surface_pin,
            record_surface,
            report_present_mode,
        )
            .chain(),
    );
}

/// After `prepare_view_targets` inserts/removes `ViewTarget`; the module doc owns the drawn-frame gate.
fn count_drawn(drawn: Res<DrawnFrames>, views: Query<(), (With<Camera3d>, With<ViewTarget>)>) {
    if !views.is_empty() {
        drawn.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// Offscreen mode: point every window-targeting camera at the one offscreen image, every frame, so a
/// camera spawned late (a view switch, an inset) moves on its first frame like the rest.
fn hold_offscreen_target(
    mut commands: Commands,
    mut offscreen: ResMut<Offscreen>,
    mut images: ResMut<Assets<Image>>,
    mut cameras: Query<(Entity, &Camera, &mut RenderTarget, Has<IsDefaultUiCamera>)>,
) {
    let target = offscreen.target.get_or_insert_with(|| {
        crate::dev_capture::offscreen_target(
            &mut images,
            (SURFACE_LOGICAL * OFFSCREEN_SCALE_FACTOR).as_uvec2(),
            OFFSCREEN_SCALE_FACTOR,
        )
    });
    let moved = crate::dev_capture::retarget_to(&mut commands, &mut cameras, target);
    if moved > 0 {
        info!("frame_cost: {moved} camera(s) retargeted to the offscreen image");
    }
}

/// Requires one primary window. [`record_monitor`]/[`record_surface`] report OS placement and
/// operator moves rather than fighting them; see the module's monitor and surface contracts.
fn park_and_pin(mut windows: Query<&mut Window, With<PrimaryWindow>>) {
    let mut window = windows.single_mut().expect(
        "the frame-cost recorder is mounted on a windowed root with exactly one primary window",
    );
    window.position = WindowPosition::At(*PARK_AT);
    window.resolution.set(SURFACE_LOGICAL.x, SURFACE_LOGICAL.y);
}

/// Re-pin on scale changes: `WindowResolution` stores physical size separately from scale.
/// `ScaleFactorChanged` writes only scale (`react_to_scale_factor_change`, vendored
/// bevy_winit-0.19.0/src/state.rs); size follows only on `Resized`. A scale-1.0 → 2.0 park can leave
/// 1280x720 physical at scale 2: half the pinned logical size and a quarter of reference pixels
/// (DERIVED). Trigger on scale, not size mismatch, to bound requests by monitor moves rather than
/// retrying every second against an OS refusing resize.
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
    let mut window = windows.single_mut().expect(
        "the frame-cost recorder is mounted on a windowed root with exactly one primary window",
    );
    let scale = window.resolution.scale_factor();
    if *pinned_at_scale == Some(scale) {
        return;
    }
    *pinned_at_scale = Some(scale);
    // `set` multiplies logical size by current scale. Guard the write: changing `Window` makes
    // bevy's `changed_windows` talk to the OS even in an otherwise steady run.
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

/// Request focus only while occluded, at most once per poll, logging every request. Healthy captures
/// never request focus; occluded ones free-run and fail analysis, so recovery avoids discarding a
/// condition. Occlusion rows still invalidate it, leaving a trail if focus keeps being lost.
///
/// Primary-display parking can put the window among the operator's windows instead of on an empty
/// external desktop. MEASURED: covered 0.36 s after creation through the whole condition; two
/// 75-second runs discarded. This visible sweep differs from the hidden net-capture harness
/// (`OVERMATCH_WINDOW=hidden`), which must never take focus.
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
    let mut window = windows.single_mut().expect(
        "the frame-cost recorder is mounted on a windowed root with exactly one primary window",
    );
    // Only false -> true calls `focus_window()` (vendored bevy_winit-0.19.0/src/system.rs,
    // `changed_windows`); another app holding focus leaves false, allowing a new request per poll.
    if !window.focused {
        warn!("frame_cost: window occluded at t={t:.3}s — asking for focus back");
        window.focused = true;
    }
}

/// Surface-row state; see the module's surface-size contract. `render_scale` prints the same
/// `physical_target_size` as "the 3D main pass fills the … target". Winit's drawable could agree
/// with the panel while bevy rendered a quarter of it, so it cannot supply this evidence.
/// Offscreen rows use [`offscreen_surface`] and carry `offscreen: true`.
#[derive(Default)]
struct SurfaceRecord {
    last: Option<UVec2>,
    pending_since: Option<Instant>,
    next_poll_s: f64,
}

fn record_surface(
    mut cost: ResMut<FrameCost>,
    time: Res<Time<Real>>,
    mut state: Local<SurfaceRecord>,
    windows: Query<&Window, With<PrimaryWindow>>,
    offscreen: Option<Res<Offscreen>>,
    cameras: Query<(&Camera, &RenderTarget), With<Camera3d>>,
) {
    let t = time.elapsed_secs_f64();
    if t < state.next_poll_s {
        return;
    }
    state.next_poll_s = t + MONITOR_POLL_S;
    let resolved = match &offscreen {
        Some(offscreen) => offscreen_surface(offscreen, &cameras),
        None => {
            let window = windows
                .single()
                .expect("the frame-cost recorder's windowed root has exactly one primary window");
            let size = window.resolution.physical_size();
            let message = format!(
                "frame_cost: surface {}x{} physical ({}x{} logical at scale {})",
                size.x,
                size.y,
                window.resolution.width(),
                window.resolution.height(),
                window.resolution.scale_factor(),
            );
            Some((size, message))
        }
    };
    let Some((size, message)) = resolved else {
        // The image and camera target info follow the retarget asynchronously. ASSUMED startup
        // allowance: 30 wall seconds; a recorder with no eventual surface cannot produce evidence.
        let since = state.pending_since.get_or_insert_with(Instant::now);
        assert!(
            since.elapsed() < std::time::Duration::from_secs(30),
            "frame_cost: the offscreen image and full-frame Camera3d target never resolved; fix the root's camera/target setup"
        );
        return;
    };
    state.pending_since = None;
    if state.last == Some(size) {
        return;
    }
    // First observation is provenance; warn on later resizes (dragged edges or monitor rescaling).
    if state.last.is_some() {
        warn!("{message} — CHANGED at t={t:.3}s");
    } else {
        info!("{message} at t={t:.3}s");
    }
    let mut row = json!({
        "t": t,
        "surface_w": size.x,
        "surface_h": size.y,
    });
    if offscreen.is_some() {
        row["offscreen"] = json!(true);
    }
    cost.sink.write(&row);
    state.last = Some(size);
}

/// Offscreen mode's surface: the physical target size of the lowest-order full-frame `Camera3d`
/// rendering into the offscreen image, which is the camera `render_scale` reports on. None until the
/// image exists and the camera's target info has followed the retarget.
fn offscreen_surface(
    offscreen: &Offscreen,
    cameras: &Query<(&Camera, &RenderTarget), With<Camera3d>>,
) -> Option<(UVec2, String)> {
    let target = offscreen.target.as_ref()?;
    let (camera, _) = cameras
        .iter()
        .filter(|(camera, render_target)| {
            camera.viewport.is_none()
                && matches!(render_target, RenderTarget::Image(image) if image == target)
        })
        .min_by_key(|(camera, _)| camera.order)?;
    let size = camera.physical_target_size()?;
    let message = format!(
        "frame_cost: surface {}x{} physical (offscreen image at scale {})",
        size.x, size.y, target.scale_factor,
    );
    Some((size, message))
}

/// Analyzer-visible monitor identity: only recorded fields define a change.
#[derive(Clone, PartialEq, Eq)]
struct MonitorFacts {
    name: Option<String>,
    refresh_mhz: Option<u32>,
    primary: bool,
}

/// Poll winit's `current_monitor()` for the module's presenting-monitor evidence, rather than
/// `OnMonitor`: it uses the same call but only through `changed_windows`, filtered by
/// `Changed<Window>` in `Last` (vendored bevy_winit-0.19.0/src/lib.rs and src/system.rs).
/// A hands-off capture can write `Window` only at startup park, before mapping, get None, then
/// never trigger another query. MEASURED: zero monitor rows for a whole condition, rejected by
/// analysis. Polling avoids depending on query timing.
///
/// The handle is thread-local, requiring non-send like `settings::observe_window_placement`.
/// Mapping can take a poll or two; an unresolved measurement window remains invalid.
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
    let entity = windows.single().expect(
        "the frame-cost recorder is mounted on a windowed root with exactly one primary window",
    );
    let resolved = bevy::winit::WINIT_WINDOWS.with_borrow(|winit_windows| {
        let winit_window = winit_windows.get_window(entity)?;
        let current = winit_window.current_monitor()?;
        Some(MonitorFacts {
            // Panel-reported millihertz pace presentation, independently of the requested mode.
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
    // First resolution is provenance; warn on later changes (window drags or display rearrangement).
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

fn sample(mut cost: ResMut<FrameCost>, time: Res<Time<Real>>, drawn: Res<DrawnFrames>) {
    let now = Instant::now();
    let frame_ms = cost
        .last_sample
        .replace(now)
        .map_or(0.0, |last| now.duration_since(last).as_secs_f64() * 1000.0);
    cost.sink.write(&json!({
        "t": time.elapsed_secs_f64(),
        "frame_ms": frame_ms,
        "drawn": drawn.0.load(Ordering::Relaxed),
    }));
}

/// Warn immediately: mid-capture occlusion invalidates measurement, and the operator should see it
/// before analysis. Rows use the frames' `Time<Real>` clock to locate the invalid interval.
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

/// Post-probe evidence for the module's present-mode gate. A `Local` latch, not a run condition,
/// matches `settings::probe`'s single channel write. Optional resources let roots without the
/// settings plugin (tests) schedule this system without reporting.
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use bevy::time::TimeUpdateStrategy;

    use super::*;

    /// Manual capture flights (e.g. a fixed 1/30 s step) must still report wall-clock `frame_ms`.
    /// Reading `Time<Real>`'s delta would report the step, much longer here than the frame,
    /// invalidating the frame-cost gate. Capture plans choose their step through `step_s`.
    #[test]
    fn frame_time_is_wall_clock_under_a_manual_time_step() {
        const STEP: Duration = Duration::from_secs(10);
        const FRAME: Duration = Duration::from_millis(20);
        let path = std::env::temp_dir().join(format!(
            "overmatch-frame-cost-{}-{}.jsonl",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        {
            let mut app = App::new();
            app.add_plugins(MinimalPlugins)
                .insert_resource(TimeUpdateStrategy::ManualDuration(STEP))
                .insert_resource(FrameCost {
                    sink: JsonlSink::create(&path).expect("open the sink"),
                    occluded: false,
                    last_sample: None,
                })
                .init_resource::<DrawnFrames>()
                .add_systems(Update, sample);
            app.update();
            std::thread::sleep(FRAME);
            app.update();
        }
        let rows = std::fs::read_to_string(&path).expect("read the rows");
        let _ = std::fs::remove_file(&path);
        let frame_ms: Vec<f64> = rows
            .lines()
            .map(|row| {
                serde_json::from_str::<serde_json::Value>(row).expect("a JSON row")["frame_ms"]
                    .as_f64()
                    .expect("a frame_ms")
            })
            .collect();
        let [_, frame_ms] = frame_ms[..] else {
            panic!("two frame rows, got {frame_ms:?}");
        };
        let (frame, step) = (FRAME.as_secs_f64() * 1000.0, STEP.as_secs_f64() * 1000.0);
        assert!(
            (frame..step / 2.0).contains(&frame_ms),
            "frame_ms {frame_ms} is not the {frame} ms slept (the app clock stepped {step} ms)"
        );
    }
}
