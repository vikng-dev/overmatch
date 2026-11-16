//! How a run shows its primary window: `OVERMATCH_WINDOW=<mode>` selects a [`RunWindow`].
//! A value outside the four modes fails the boot naming the variable (ADR-0011).
//!
//! - `visible`: the window on screen, as a player has it. The default on every windowed root, and
//!   the only mode whose frames are presented.
//! - `hidden`: the full presentation stack (a GPU device, the render app, every camera) in a window
//!   created invisible ([`hide`]). Its present returns `SurfaceError::Occluded` every frame, so
//!   nothing vsync-blocks and the frame loop free-runs: plumbing, never a frame time.
//! - `offscreen`: hidden, and every window camera renders into the frame-cost recorder's image
//!   (`frame_cost`, which it needs: `SPIKE_FRAME_COST`). A frame-time measurement that takes nobody's
//!   screen.
//! - `headless`: no window and no GPU. The scripted client's GPU-less root (`net::client::run`)
//!   and its default; every other root has a window and refuses it.
//!
//! A hidden or offscreen run is unattended (`crate::unattended`): nobody can see it, so nobody may
//! hear it. Every windowed root reads the mode through `crate::windowed_runtime`, and the scripted
//! client's GPU-less root reads it itself; each logs `window: <mode>` once, which the frame-cost
//! runners (`scripts/perf/frame-run.zsh`) gate on.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

/// The variable that names the mode.
const VARIABLE: &str = "OVERMATCH_WINDOW";

/// How the run shows its primary window (module doc).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunWindow {
    Headless,
    Hidden,
    Offscreen,
    Visible,
}

/// Operator-selected mode errors, handled visibly at the composition root.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WindowModeError {
    InvalidMode(String),
    HeadlessWindowedRoot,
}

impl std::fmt::Display for WindowModeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidMode(value) => write!(
                f,
                "{VARIABLE}: {value:?} is not one of headless, hidden, offscreen or visible"
            ),
            Self::HeadlessWindowedRoot => write!(
                f,
                "{VARIABLE}=headless: this root requires hidden, offscreen or visible; headless needs a scripted client (OVERMATCH_HARNESS)"
            ),
        }
    }
}

impl std::error::Error for WindowModeError {}

/// A mode admitted for a root that constructs a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowedRun {
    Hidden,
    Offscreen,
    Visible,
}

impl WindowedRun {
    /// Frame captures need an invisible window even when no window-mode override was supplied.
    pub(crate) fn for_capture(self, takes_frames: bool) -> Self {
        match (self, takes_frames) {
            (Self::Visible, true) => Self::Hidden,
            _ => self,
        }
    }

    /// Winit reads focus during plugin construction, before it creates the native window.
    /// Bevy 0.20.1 suppresses macOS launch activation only when every initial window is unfocused.
    pub(crate) fn prepare_window(self, mut window: Window) -> Window {
        if matches!(self, Self::Hidden | Self::Offscreen) {
            window.visible = false;
            window.focused = false;
        }
        window
    }

    fn name(self) -> &'static str {
        match self {
            Self::Hidden => RunWindow::Hidden,
            Self::Offscreen => RunWindow::Offscreen,
            Self::Visible => RunWindow::Visible,
        }
        .name()
    }
}

impl RunWindow {
    const ALL: [Self; 4] = [Self::Headless, Self::Hidden, Self::Offscreen, Self::Visible];

    /// The value [`VARIABLE`] spells this mode with.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Headless => "headless",
            Self::Hidden => "hidden",
            Self::Offscreen => "offscreen",
            Self::Visible => "visible",
        }
    }

    fn parse(value: &str) -> Result<Self, WindowModeError> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.name() == value)
            .ok_or_else(|| WindowModeError::InvalidMode(value.to_owned()))
    }

    /// Parse the operator mode once, visibly refusing malformed input; use `default` when unset.
    pub(crate) fn of_run(default: Self) -> Self {
        crate::env_value(VARIABLE)
            .unwrap_or_else(crate::refuse_start)
            .map(|value| Self::parse(&value).unwrap_or_else(crate::refuse_start))
            .unwrap_or(default)
    }

    pub(crate) fn windowed(self) -> Result<WindowedRun, WindowModeError> {
        match self {
            Self::Headless => Err(WindowModeError::HeadlessWindowedRoot),
            Self::Hidden => Ok(WindowedRun::Hidden),
            Self::Offscreen => Ok(WindowedRun::Offscreen),
            Self::Visible => Ok(WindowedRun::Visible),
        }
    }
}

/// Windowed roots have one primary window; GPU-less harness compositions have none.
/// The latter is a named absence, while multiple primary windows are a construction bug.
#[cfg(feature = "dev_ui")]
pub(crate) fn primary_or_headless<T>(
    result: Result<T, bevy::ecs::query::QuerySingleError>,
) -> Option<T> {
    match result {
        Ok(window) => Some(window),
        Err(bevy::ecs::query::QuerySingleError::NoEntities(_)) => None,
        Err(bevy::ecs::query::QuerySingleError::MultipleEntities(_)) => {
            panic!("a root declares at most one primary window, including its cursor")
        }
    }
}

/// Mount the run policy after its prepared primary window spawns, before winit creates it.
/// Hidden modes require exactly one primary window (`hide`); violating that is a construction bug.
pub(crate) fn mount(app: &mut App, mode: WindowedRun) {
    info!("window: {}", mode.name());
    match mode {
        WindowedRun::Visible => {}
        WindowedRun::Hidden | WindowedRun::Offscreen => hide(app),
    }
}

/// Mount policy for a prepared hidden window: keep it windowed and the run unattended.
///
/// # Panics
///
/// When the app holds no single primary window: it is called after the root's runtime
/// (`crate::windowed_runtime`) has spawned the root's one window and before winit creates it, and
/// a window spawned later would be created visible.
fn hide(app: &mut App) {
    pin_window(app);
    crate::unattended::mount(app);
}

/// Keep the primary window a plain window for the whole run: `Windowed` at creation, so a persisted
/// fullscreen never boots, and [`crate::settings::CaptureWindowPinned`], so settings never
/// re-applies one later. On macOS either would order the window front whatever `visible` says.
fn pin_window(app: &mut App) {
    primary_window(app).mode = bevy::window::WindowMode::Windowed;
    app.insert_resource(crate::settings::CaptureWindowPinned);
}

/// The root's one primary window, as [`hide`] finds it ([`hide`]'s panics).
fn primary_window(app: &mut App) -> Mut<'_, Window> {
    let world = app.world_mut();
    let mut windows = world.query_filtered::<&mut Window, With<PrimaryWindow>>();
    windows.single_mut(world).unwrap_or_else(|error| {
        panic!(
            "a hidden run hides the root's one primary window, which the root's runtime \
             (`crate::windowed_runtime`) spawns before this mount and winit creates after it; \
             found {error}, and a window spawned later would be created visible"
        )
    })
}

#[cfg(test)]
mod tests {
    use bevy::audio::{GlobalVolume, Volume};

    use super::*;

    /// EVERY MODE PARSES FROM ITS OWN NAME, AND NOTHING ELSE DOES: the frame-cost runners write
    /// these names and gate on the `window: <mode>` line they print, and a typo that fell back to
    /// a visible window would take the screen of a machine its owner is working at.
    #[test]
    fn each_mode_parses_from_its_name_and_nothing_else_does() {
        for mode in RunWindow::ALL {
            assert_eq!(RunWindow::parse(mode.name()), Ok(mode));
        }
        for bad in ["", "Hidden", "windowed", "1", "off", " hidden"] {
            let error = RunWindow::parse(bad).expect_err("not a mode");
            assert!(
                error
                    .to_string()
                    .contains("headless, hidden, offscreen or visible"),
                "{error}"
            );
        }
    }

    /// Windowed admission excludes Headless; refusal names supported modes and the scripted root.
    #[test]
    fn headless_is_refused_for_windowed_roots() {
        let error = RunWindow::Headless.windowed().unwrap_err();
        assert_eq!(error, WindowModeError::HeadlessWindowedRoot);
        let message = error.to_string();
        for required in [
            VARIABLE,
            "hidden",
            "offscreen",
            "visible",
            "OVERMATCH_HARNESS",
        ] {
            assert!(message.contains(required), "{message}");
        }
        for mode in [RunWindow::Hidden, RunWindow::Offscreen, RunWindow::Visible] {
            assert_eq!(mode.windowed().unwrap().name(), mode.name());
        }
    }

    /// A HIDDEN RUN TAKES NO SCREEN AND NEVER PAUSES, on any root: hidden harness clients and
    /// captures run on a machine its owner is working at. The window a root described visible and
    /// fullscreen, or a frame capture with no mode override, is hidden and unfocused before
    /// WinitPlugin builds, and `Windowed` before native creation. Settings leave it so, the run
    /// is silent, and a focus loss does not pause it. Mutants: dropping the installer's
    /// `for_capture` or `prepare_window`, [`pin_window`] or `unattended::mount` fails an assert.
    #[test]
    fn a_hidden_run_is_hidden_pinned_silent_and_never_paused() {
        fn before_winit(app: &mut App) {
            let window = primary_window(app);
            assert!(!window.visible, "hidden before WinitPlugin builds");
            assert!(!window.focused, "unfocused before WinitPlugin builds");
        }

        let capture =
            crate::dev_capture::plan::CapturePlan::parse(r#"(frames: (out: "fixture.png"))"#)
                .unwrap();
        for (mode, capture) in [
            (RunWindow::Hidden, None),
            (RunWindow::Offscreen, None),
            (RunWindow::Visible, Some(&capture)),
        ] {
            let mut app = App::new();
            crate::install_windowed_runtime(
                &mut app,
                Window {
                    mode: bevy::window::WindowMode::BorderlessFullscreen(
                        bevy::window::MonitorSelection::Primary,
                    ),
                    ..default()
                },
                mode,
                capture,
                bevy::app::PluginGroupBuilder::start::<MinimalPlugins>()
                    .add(WindowPlugin::default())
                    .add(before_winit),
            );
            // What a later `AudioPlugin` build writes; unattended policy applies in PreStartup.
            app.insert_resource(GlobalVolume::new(Volume::Linear(1.0)));
            app.world_mut().run_schedule(PreStartup);
            let window = primary_window(&mut app);
            assert!(!window.visible, "{mode:?}");
            assert!(!window.focused, "{mode:?}");
            assert_eq!(window.mode, bevy::window::WindowMode::Windowed);
            assert!(
                app.world()
                    .contains_resource::<crate::settings::CaptureWindowPinned>()
            );
            assert!(
                app.world()
                    .contains_resource::<crate::state::FocusPauseSuppressed>()
            );
            assert_eq!(
                app.world().resource::<GlobalVolume>().volume.to_linear(),
                0.0
            );
        }
    }

    /// A HIDDEN RUN WITH NO PRIMARY WINDOW YET REFUSES TO BOOT: a window a root spawned after the
    /// mount would be created visible, taking the screen of a machine its owner is working at, so
    /// mounting out of order is a bug, never a warning. Mutant: a [`hide`] that skips a missing
    /// window returns here.
    #[test]
    #[should_panic(expected = "a window spawned later would be created visible")]
    fn a_hidden_run_with_no_primary_window_refuses_to_boot() {
        mount(&mut App::new(), WindowedRun::Hidden);
    }

    /// A VISIBLE RUN IS LEFT AS THE ROOT DESCRIBED IT: the player's own window and its sound.
    #[test]
    fn a_visible_run_is_left_alone() {
        let mut app = App::new();
        let window = app
            .world_mut()
            .spawn((
                WindowedRun::Visible
                    .for_capture(false)
                    .prepare_window(Window::default()),
                PrimaryWindow,
            ))
            .id();
        mount(&mut app, WindowedRun::Visible);
        let window = app.world().get::<Window>(window).expect("the window");
        assert!(window.visible);
        assert!(window.focused);
        assert!(
            !app.world()
                .contains_resource::<crate::settings::CaptureWindowPinned>()
        );
        assert!(
            !app.world()
                .contains_resource::<crate::state::FocusPauseSuppressed>()
        );
    }
}
