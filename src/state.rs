//! App state (playing/paused), the shared gameplay system set, and cursor/pause handling.

use avian3d::prelude::{Physics, PhysicsTime};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions};

/// Top-level app mode. More variants (Loading, Menu) slot in here later.
#[derive(States, Debug, Clone, Copy, Default, Eq, PartialEq, Hash)]
pub enum AppState {
    #[default]
    Playing,
    Paused,
}

/// All gameplay systems belong to this set; it runs only while `Playing`. Features add their
/// play-only systems with `.in_set(GameplaySet)` rather than repeating a run condition — so
/// pausing freezes everything from one place. Init/teardown systems stay out of the set.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GameplaySet;

pub fn plugin(app: &mut App) {
    app.init_state::<AppState>()
        // Set configuration is per-schedule, so gate it in every schedule it's used in.
        .configure_sets(Update, GameplaySet.run_if(in_state(AppState::Playing)))
        .configure_sets(FixedUpdate, GameplaySet.run_if(in_state(AppState::Playing)))
        .configure_sets(PostUpdate, GameplaySet.run_if(in_state(AppState::Playing)))
        // The pause toggle must run in either state, so it is deliberately not in the set.
        .add_systems(Update, toggle_pause)
        .add_systems(OnEnter(AppState::Playing), (grab_cursor, resume_physics))
        .add_systems(
            OnEnter(AppState::Paused),
            (release_cursor, spawn_pause_overlay, pause_physics),
        );
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
        });
    }
}

/// Lock + hide the cursor on entering Playing. The initial state transition fires this at
/// startup, so it doubles as the startup grab as well as the unpause grab.
fn grab_cursor(mut cursor: Single<&mut CursorOptions>) {
    cursor.grab_mode = CursorGrabMode::Locked;
    cursor.visible = false;
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
                TextFont { font_size: FontSize::Px(80.0), ..default() },
                TextColor(Color::WHITE),
            ));
        });
}
