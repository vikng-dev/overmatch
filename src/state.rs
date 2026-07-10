//! App state (playing/paused), the shared gameplay system set, and cursor/pause handling.

use avian3d::prelude::{Physics, PhysicsTime};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow, WindowFocused};

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

/// The state machine + gameplay gate — authority-side: every configuration (dedicated server
/// included) drives its sim through `AppState`/`GameplaySet`.
pub fn sim_plugin(app: &mut App) {
    app.init_state::<AppState>()
        // Set configuration is per-schedule, so gate it in every schedule it's used in.
        .configure_sets(Update, GameplaySet.run_if(in_state(AppState::Playing)))
        .configure_sets(FixedUpdate, GameplaySet.run_if(in_state(AppState::Playing)))
        .configure_sets(PostUpdate, GameplaySet.run_if(in_state(AppState::Playing)))
        .configure_sets(
            RunFixedMainLoop,
            GameplaySet.run_if(in_state(AppState::Playing)),
        );
}

/// Pause input, cursor capture, and the pause overlay — client-side: a headless server has no
/// Esc key, no cursor, and never pauses. Requires [`sim_plugin`] (the states it drives).
pub fn client_plugin(app: &mut App) {
    // The pause toggle and the focus watcher must run in either state, so they are deliberately not
    // in `GameplaySet`.
    app.add_systems(Update, (toggle_pause, release_on_focus_lost))
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
) {
    // Collapse the frame's focus events to whether we ended unfocused (the last event wins).
    let mut ended_focused = None;
    for event in focus.read() {
        ended_focused = Some(event.focused);
    }
    let Some(false) = ended_focused else {
        return;
    };
    let mut cursor = cursor.into_inner();
    cursor.grab_mode = CursorGrabMode::None;
    cursor.visible = true;
    if *state.get() == AppState::Playing {
        next.set(AppState::Paused);
    }
}

/// Esc flips Playing <-> Paused.
fn toggle_pause(
    keys: Res<ButtonInput<KeyCode>>,
    state: Res<State<AppState>>,
    mut next: ResMut<NextState<AppState>>,
) {
    if keys.just_pressed(KeyCode::Escape) {
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
/// there is no teardown system to keep in sync.
fn spawn_pause_overlay(mut commands: Commands) {
    commands
        .spawn((
            DespawnOnExit(AppState::Paused),
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("PAUSED"),
                TextFont {
                    font_size: FontSize::Px(80.0),
                    ..default()
                },
                TextColor(Color::WHITE),
            ));
        });
}
