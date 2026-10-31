//! App state (playing/paused), the shared gameplay system set, cursor/pause handling, and the
//! Esc router ([`EscapeAction`]).

use avian3d::prelude::{Physics, PhysicsTime};
use bevy::ecs::schedule::ScheduleConfigs;
use bevy::ecs::system::ScheduleSystem;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow, WindowFocused};

use crate::damage_cam::pen_cam::game::{KillCamGate, KillCamGateSet};
use crate::ui_font::UiFonts;

/// Top-level app mode. `Loading` gates gameplay until required assets (e.g. the tank's spec
/// sheet) have loaded, so nothing binds against a half-loaded world (ADR-0011). More variants
/// (Menu) slot in here later.
#[derive(States, Debug, Clone, Copy, Default, Eq, PartialEq, Hash)]
pub enum AppState {
    #[default]
    Loading,
    Playing,
    Paused,
}

/// All gameplay systems belong to this set; it runs only while `Playing`. Features add their
/// play-only systems with `.in_set(GameplaySet)` rather than repeating a run condition — so
/// pausing freezes everything from one place. Init/teardown systems stay out of the set.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GameplaySet;

/// Cross-feature fixed-step order: drive forces, weapon impulse, recoil spring, then projectile
/// march. Each phase can change physical state read by the next one.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SimPhase {
    DrivingForces,
    WeaponFire,
    Recoil,
    ProjectileMarch,
}

/// The device-reading player-input systems — free-look mouse-orbit, gunner aim, the Lshift view
/// toggle, the range dial, and the drive/command gather. Grouped so ONE `.run_if(cursor_locked)` in
/// the composition root ([`crate::ClientPlugin`]/[`crate::NetClientPlugin`]) arms them all: the
/// license to consume mouse/gameplay input is `grab_mode == Locked`, and gating the group keeps that
/// rule in one place instead of scattered per system. Esc/menu/pause toggles stay OUT of the set — a
/// gated toggle could never release the cursor. Spans `Update`, `PostUpdate`, and `RunFixedMainLoop`,
/// so the root configures the set in each of those schedules.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlayerInputSet;

/// Run condition: the primary window's cursor is captured (`grab_mode == Locked`). This is the
/// license [`PlayerInputSet`] gates on — release the cursor (menu, pause, alt-tab) and every
/// device-reading system idles, so the tank coasts instead of being driven by a freed cursor.
///
/// Returns `true` when there is NO primary window: the headless scripted harness has no cursor and
/// drives the tank by writing `ActionState` directly (`net::harness::buffer_input`), and it never
/// mounts these device systems anyway (`NetClientPlugin` is skipped unless windowed) — so the gate
/// must never be the thing that stops a windowless run.
pub fn cursor_locked(cursor: Query<&CursorOptions, With<PrimaryWindow>>) -> bool {
    cursor
        .single()
        .map_or(true, |c| c.grab_mode == CursorGrabMode::Locked)
}

/// Where this frame's Esc press goes, decided once by [`route_escape`] ahead of every consumer: a
/// kill cam run playing while the cursor is captured takes it as its skip; otherwise it is the
/// menu's — the pause offline ([`toggle_pause`]), the Esc menu online (`overlay::esc_toggle`). One
/// decision per press, so a press that skips the kill cam opens no menu.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum EscapeAction {
    #[default]
    None,
    /// End the kill cam's run at the death screen.
    SkipKillCam,
    /// Toggle the menu: the pause offline, the Esc menu online.
    Menu,
}

/// Where [`route_escape`] runs, in `Update`: before the kill cam's drive (its `KillCamGateSet`
/// orders after this), before the overlay toggles and before [`toggle_pause`], each of which reads
/// [`EscapeAction`] of the frame it runs in.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct EscapeRouted;

/// The Esc router's resource and system, mounted by the composition that owns Esc: the pause
/// ([`client_plugin`]) offline, the overlay authority (`overlay::plugin`) online.
pub(crate) fn escape_router(app: &mut App) {
    app.init_resource::<EscapeAction>()
        .add_systems(Update, route_escape.in_set(EscapeRouted));
}

/// Decide [`EscapeAction`] from this frame's key, the cursor and the kill cam's gate as the frame
/// entered it (the last frame's drive wrote it; this frame's runs after [`EscapeRouted`]): a run
/// playing while the cursor is captured is skipped; with the cursor released (a menu, a pause) or
/// no run, the press is the menu's. The kill cam applies the skip only past its input dead-zone, so
/// a press inside it does nothing.
fn route_escape(
    keys: Res<ButtonInput<KeyCode>>,
    cursor: Query<&CursorOptions, With<PrimaryWindow>>,
    kill_cam: Option<Res<KillCamGate>>,
    mut action: ResMut<EscapeAction>,
) {
    let want = if !keys.just_pressed(KeyCode::Escape) {
        EscapeAction::None
    } else if kill_cam.is_some_and(|gate| gate.running) && cursor_locked(cursor) {
        EscapeAction::SkipKillCam
    } else {
        EscapeAction::Menu
    };
    action.set_if_neq(want);
}

/// The state machine + gameplay gate — authority-side: every configuration (dedicated server
/// included) drives its sim through `AppState`/`GameplaySet`.
pub fn sim_plugin(app: &mut App) {
    app.init_state::<AppState>()
        // Set configuration is per-schedule, so gate it in every schedule it's used in.
        .configure_sets(Update, GameplaySet.run_if(in_state(AppState::Playing)))
        .configure_sets(FixedUpdate, GameplaySet.run_if(in_state(AppState::Playing)))
        .configure_sets(
            FixedUpdate,
            (
                SimPhase::DrivingForces,
                SimPhase::WeaponFire,
                SimPhase::Recoil,
                SimPhase::ProjectileMarch,
            )
                .chain(),
        )
        .configure_sets(PostUpdate, GameplaySet.run_if(in_state(AppState::Playing)))
        .configure_sets(
            RunFixedMainLoop,
            GameplaySet.run_if(in_state(AppState::Playing)),
        );
}

/// Pause input, cursor capture, and the pause overlay — client-side: a headless server has no
/// Esc key, no cursor, and never pauses. Requires [`sim_plugin`] (the states it drives).
pub fn client_plugin(app: &mut App) {
    escape_router(app);
    // The pause toggle and the focus watcher must run in either state, so they are deliberately not
    // in `GameplaySet`.
    if crate::env_flag("OVERMATCH_NO_FOCUS_PAUSE", false) {
        app.insert_resource(FocusPauseSuppressed);
    }
    app.add_systems(Update, (pause_toggle(), release_on_focus_lost))
        .add_systems(OnEnter(AppState::Playing), (grab_cursor, resume_physics))
        .add_systems(
            OnEnter(AppState::Paused),
            (release_cursor, spawn_pause_overlay, pause_physics),
        );
}

/// Lock, hide, and re-center the cursor — the single grab primitive shared by every path that
/// (re)captures: unpause (`grab_cursor`), the net menu close, and the net alt-tab re-grab. Re-center
/// first so mouse-look resumes owned by this window even if the cursor had wandered out.
pub(crate) fn grab_now(window: &mut Window, cursor: &mut CursorOptions) {
    let center = window.size() / 2.0;
    window.set_cursor_position(Some(center));
    cursor.grab_mode = CursorGrabMode::Locked;
    cursor.visible = false;
}

/// Losing window focus does not pause: an unattended run keeps simulating so it never stops
/// mid-leg. Inserted at build by a capture run (`dev_capture`) and by `OVERMATCH_NO_FOCUS_PAUSE=1`
/// (the perf scripts' measurement windows); nothing removes it.
#[derive(Resource)]
pub(crate) struct FocusPauseSuppressed;

/// Collapse a frame's `WindowFocused` messages to the net result — whether the window ended the
/// frame focused (the last event wins) — or `None` if focus didn't change this frame. Shared by the
/// focus watchers ([`release_on_focus_lost`] here and `overlay::cursor_owner`), which both need
/// only that final state, not the individual events.
pub(crate) fn collapse_focus(focus: &mut MessageReader<WindowFocused>) -> Option<bool> {
    let mut ended_focused = None;
    for event in focus.read() {
        ended_focused = Some(event.focused);
    }
    ended_focused
}

/// Release the cursor and pause when the window loses focus (alt-tab). Writing `grab_mode = None`
/// here — not only via `OnEnter(Paused)` — matches OS reality the instant focus is lost AND arms
/// change detection: winit may already have dropped the grab while bevy still caches `Locked`
/// (bevy #16237/#16238), so an explicit write is needed for the next grab to register. Regaining
/// focus leaves the game Paused (Esc, the existing unpause path, re-grabs via `OnEnter(Playing)`),
/// so this only needs to act on the loss. Not in `GameplaySet`: it must run while paused too.
fn release_on_focus_lost(
    mut focus: MessageReader<WindowFocused>,
    state: Res<State<AppState>>,
    mut next: ResMut<NextState<AppState>>,
    cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
    suppressed: Option<Res<FocusPauseSuppressed>>,
) {
    let Some(false) = collapse_focus(&mut focus) else {
        return;
    };
    // The log line lets a capture record count focus losses.
    if suppressed.is_some() {
        info!("state: focus lost — pause suppressed (unattended run)");
        return;
    }
    let mut cursor = cursor.into_inner();
    cursor.grab_mode = CursorGrabMode::None;
    cursor.visible = true;
    if *state.get() == AppState::Playing {
        next.set(AppState::Paused);
    }
}

/// [`toggle_pause`] in its place: after the Esc router, and after the kill cam's drive, which
/// takes back a press the router gave the menu on the frame a run starts.
fn pause_toggle() -> ScheduleConfigs<ScheduleSystem> {
    toggle_pause
        .after(EscapeRouted)
        .after(KillCamGateSet)
        .into_configs()
}

/// The Esc router and the pause toggle alone, for a composed schedule under test.
#[cfg(test)]
pub(crate) fn mount_pause_bare(app: &mut App) {
    escape_router(app);
    app.add_systems(Update, pause_toggle());
}

/// An Esc press the router gave the menu ([`EscapeAction::Menu`]) flips Playing <-> Paused.
fn toggle_pause(
    action: Res<EscapeAction>,
    state: Res<State<AppState>>,
    mut next: ResMut<NextState<AppState>>,
) {
    if *action == EscapeAction::Menu {
        next.set(match state.get() {
            AppState::Playing => AppState::Paused,
            AppState::Paused => AppState::Playing,
            AppState::Loading => return, // no pausing mid-load
        });
    }
}

/// Lock + hide the cursor on entering Playing — fires on the Loading→Playing transition (once
/// assets are ready) as well as on unpause.
fn grab_cursor(window: Single<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>) {
    let (mut window, mut cursor) = window.into_inner();
    grab_now(&mut window, &mut cursor);
}

fn release_cursor(mut cursor: Single<&mut CursorOptions>) {
    cursor.grab_mode = CursorGrabMode::None;
    cursor.visible = true;
}

/// Freeze/thaw Avian alongside the gameplay set, so pausing stops the physics sim too — the
/// dynamic hull and projectiles hold still instead of falling while the rest is frozen.
fn pause_physics(mut time: ResMut<Time<Physics>>) {
    time.pause();
}

fn resume_physics(mut time: ResMut<Time<Physics>>) {
    time.unpause();
}

/// "PAUSED" overlay. `DespawnOnExit(Paused)` deletes it (children included) on unpause, so
/// there is no teardown system to keep in sync. Shares `ui_font::spawn_overlay` with the net menu,
/// connect, and death overlays. Two deliberate departures from the old inline copy: it now renders
/// at the family standard size (was 80 px, see `OVERLAY_FONT_PX`), and `None` preserves this site's
/// distinguishing lack of a dim backdrop.
fn spawn_pause_overlay(mut commands: Commands, fonts: Res<UiFonts>) {
    crate::ui_font::spawn_overlay(
        &mut commands,
        &fonts.hud,
        DespawnOnExit(AppState::Paused),
        "PAUSED",
        (),
        None,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Press Esc for one frame, then run the frame the state transition it asked for lands in.
    fn press(app: &mut App) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(KeyCode::Escape);
        keys.clear();
        app.update();
    }

    /// Losing focus pauses a played run, unless the run is unattended: a capture or a measurement
    /// window that paused on the first focus change would record a paused scene. Mutant: a
    /// watcher that ignores [`FocusPauseSuppressed`] pauses the second app.
    #[test]
    fn focus_loss_pauses_unless_the_run_is_unattended() {
        for suppressed in [false, true] {
            let mut app = App::new();
            app.add_plugins(bevy::state::app::StatesPlugin)
                .init_state::<AppState>()
                .add_message::<WindowFocused>()
                .add_systems(Update, release_on_focus_lost);
            if suppressed {
                app.insert_resource(FocusPauseSuppressed);
            }
            let window = app
                .world_mut()
                .spawn((Window::default(), PrimaryWindow))
                .id();
            app.world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(AppState::Playing);
            app.update();
            app.world_mut().write_message(WindowFocused {
                window,
                focused: false,
            });
            app.update();
            app.update();
            let want = if suppressed {
                AppState::Playing
            } else {
                AppState::Paused
            };
            assert_eq!(
                *app.world().resource::<State<AppState>>().get(),
                want,
                "suppressed: {suppressed}"
            );
        }
    }

    /// Offline, Esc pauses only when the router gives the press to the menu: while a kill cam run
    /// plays the press is the run's skip and the game keeps playing; with none it pauses, and the
    /// next unpauses.
    #[test]
    fn esc_pauses_only_when_no_kill_cam_run_takes_it() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .init_state::<AppState>()
            .init_resource::<ButtonInput<KeyCode>>();
        mount_pause_bare(&mut app);
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Playing);
        app.update();
        let state = |app: &App| *app.world().resource::<State<AppState>>().get();
        assert_eq!(state(&app), AppState::Playing);

        app.insert_resource(KillCamGate {
            running: true,
            ..default()
        });
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();
        assert_eq!(
            *app.world().resource::<EscapeAction>(),
            EscapeAction::SkipKillCam
        );
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(KeyCode::Escape);
        keys.clear();
        app.update();
        assert_eq!(state(&app), AppState::Playing, "the run took the press");

        app.insert_resource(KillCamGate::default());
        press(&mut app);
        assert_eq!(state(&app), AppState::Paused, "no run: the pause");
        press(&mut app);
        assert_eq!(state(&app), AppState::Playing, "and the unpause");
    }
}
