//! How a run shows its primary window: one input, `OVERMATCH_WINDOW=<mode>`, one [`RunWindow`].
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

    fn parse(value: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.name() == value)
            .ok_or_else(|| {
                format!("{value:?} is not one of headless, hidden, offscreen or visible")
            })
    }

    /// This run's mode: [`VARIABLE`]'s, or `default` when it is unset.
    ///
    /// # Panics
    ///
    /// When the variable holds anything but one of the four names (module doc).
    pub(crate) fn of_run(default: Self) -> Self {
        crate::env_value(VARIABLE)
            .unwrap_or_else(crate::refuse_start)
            .map(|value| {
                Self::parse(&value)
                    .unwrap_or_else(|error| crate::refuse_start(format!("{VARIABLE}: {error}")))
            })
            .unwrap_or(default)
    }
}

/// Windowed roots have one primary window; GPU-less harness compositions have none.
/// The latter is a named absence, while multiple primary windows are a construction bug.
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

/// Apply `mode` on a windowed root (`crate::windowed_runtime`), after `crate::default_plugins` has
/// spawned the primary window and before winit creates it.
///
/// # Panics
///
/// On [`RunWindow::Headless`]: this root draws a window, and only the scripted client has a root
/// without one. On a hidden mode with no single primary window ([`hide`]).
pub(crate) fn mount(app: &mut App, mode: RunWindow) {
    info!("window: {}", mode.name());
    match mode {
        RunWindow::Visible => {}
        RunWindow::Hidden | RunWindow::Offscreen => hide(app),
        RunWindow::Headless => panic!(
            "{VARIABLE}=headless: this root draws a window; only the scripted client \
             (`net::client::run`) runs headless"
        ),
    }
}

/// What a run nobody can see implies, on any root: the window hidden and kept a plain window, and
/// the run unattended.
///
/// # Panics
///
/// When the app holds no single primary window: it is called after the root's runtime
/// (`crate::windowed_runtime`) has spawned the root's one window and before winit creates it, and
/// a window spawned later would be created visible.
pub(crate) fn hide(app: &mut App) {
    hide_window(app);
    pin_window(app);
    crate::unattended::mount(app);
}

/// Create the primary window hidden. `WindowPlugin` has spawned the entity by now and `bevy_winit`
/// reads `visible` when it creates the OS window, which is after every plugin's build. On macOS,
/// `visible` is the only lever that keeps the window from taking focus: bevy creates every winit
/// window invisible and then calls `set_visible(window.visible)`, which is `makeKeyAndOrderFront`
/// whatever `focused` says. winit still makes even an invisible unbundled process the active app
/// at launch, so the process also renounces activation (`platform::revoke_macos_activation`).
fn hide_window(app: &mut App) {
    let mut window = primary_window(app);
    if !window.visible {
        return;
    }
    window.visible = false;
    #[cfg(target_os = "macos")]
    app.add_systems(Startup, crate::platform::revoke_macos_activation);
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
                error.contains("headless, hidden, offscreen or visible"),
                "{error}"
            );
        }
    }

    /// A HIDDEN RUN TAKES NO SCREEN AND NEVER PAUSES, on any root: hidden harness clients and
    /// captures run on a machine its owner is working at. The window a root described visible and
    /// fullscreen is hidden and `Windowed` before winit creates it, settings are told to leave it
    /// so, the run is silent, and a focus loss does not pause it. Mutants: dropping any one of
    /// [`hide_window`], [`pin_window`] or `unattended::mount` from [`hide`] fails an assert, and so
    /// does `mount` leaving a hidden or offscreen mode visible.
    #[test]
    fn a_hidden_run_is_hidden_pinned_silent_and_never_paused() {
        for mode in [RunWindow::Hidden, RunWindow::Offscreen] {
            let mut app = App::new();
            let window = app
                .world_mut()
                .spawn((
                    Window {
                        mode: bevy::window::WindowMode::BorderlessFullscreen(
                            bevy::window::MonitorSelection::Primary,
                        ),
                        ..default()
                    },
                    PrimaryWindow,
                ))
                .id();
            mount(&mut app, mode);
            // What a later `AudioPlugin` build writes. Only `PreStartup` runs: `Startup` holds the
            // macOS activation revocation, which needs the process's main thread.
            app.insert_resource(GlobalVolume::new(Volume::Linear(1.0)));
            app.world_mut().run_schedule(PreStartup);
            let window = app.world().get::<Window>(window).expect("the window");
            assert!(!window.visible, "{mode:?}");
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
        mount(&mut App::new(), RunWindow::Hidden);
    }

    /// A VISIBLE RUN IS LEFT AS THE ROOT DESCRIBED IT: the player's own window and its sound.
    #[test]
    fn a_visible_run_is_left_alone() {
        let mut app = App::new();
        let window = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();
        mount(&mut app, RunWindow::Visible);
        assert!(
            app.world()
                .get::<Window>(window)
                .expect("the window")
                .visible
        );
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
