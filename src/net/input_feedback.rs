//! Display-only input feedback for the game's fire control and damage model (game state only).

use super::input_arrival::ClientInput;
use super::timing::Timing;
use crate::command::{Bindings, TankDeviceSet};
use crate::flow::{Flow, FlowScoped};
use crate::state::{GameplaySet, PlayerInputSet};
use crate::ui_font::UiFonts;
use bevy::audio::{PlaybackSettings, Volume};
use bevy::prelude::*;

#[derive(Component)]
struct LinkWarning;
#[derive(Component)]
struct RefusedMark;
#[derive(Resource, Default)]
struct Feedback {
    seen: u64,
    until: f64,
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<Feedback>()
        .init_resource::<Timing>()
        .init_resource::<ClientInput>()
        .add_systems(OnEnter(Flow::InMatch), spawn)
        .add_systems(
            Update,
            trigger_click
                .in_set(TankDeviceSet)
                .in_set(PlayerInputSet)
                .in_set(GameplaySet),
        )
        .add_systems(PostUpdate, update.after(super::input_arrival::send_inputs));
}
fn spawn(mut commands: Commands, fonts: Res<UiFonts>) {
    commands.spawn((
        FlowScoped(Flow::InMatch),
        LinkWarning,
        Text::new("NO LINK"),
        TextFont {
            font: fonts.hud.clone().into(),
            ..default()
        },
        Visibility::Hidden,
        Node {
            position_type: PositionType::Absolute,
            top: Val::ZERO,
            justify_self: JustifySelf::Center,
            ..default()
        },
    ));
    commands
        .spawn((
            FlowScoped(Flow::InMatch),
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .with_children(|parent| {
            parent.spawn((
                RefusedMark,
                Text::new("!"),
                TextFont {
                    font: fonts.hud.clone().into(),
                    ..default()
                },
                Visibility::Hidden,
            ));
        });
}
/// The primary binding's immediate click belongs to sfx::play_fire_click. This is its second route.
fn trigger_click(
    bindings: Res<Bindings>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    clips: Res<crate::sfx::SfxClips>,
    mut commands: Commands,
) {
    if bindings.fire_secondary.just_pressed(&keys, &mouse) {
        commands.spawn((
            AudioPlayer::new(clips.click.clone()),
            PlaybackSettings::DESPAWN.with_volume(Volume::Linear(crate::sfx::click_volume())),
        ));
    }
}
fn update(
    state: Res<ClientInput>,
    time: Res<Time<Real>>,
    timing: Res<Timing>,
    clips: Res<crate::sfx::SfxClips>,
    mut feedback: ResMut<Feedback>,
    mut marks: Query<
        (&mut Visibility, Has<LinkWarning>),
        Or<(With<LinkWarning>, With<RefusedMark>)>,
    >,
    mut commands: Commands,
) {
    if state.refused > feedback.seen {
        feedback.until = time.elapsed_secs_f64() + timing.refused_cue_s;
        commands.spawn((
            AudioPlayer::new(clips.click.clone()),
            PlaybackSettings::DESPAWN
                .with_speed(timing.refused_pitch)
                .with_volume(Volume::Linear(crate::sfx::click_volume())),
        ));
    }
    feedback.seen = state.refused;
    for (mut visible, warning) in &mut marks {
        *visible = if if warning {
            state.warning
        } else {
            time.elapsed_secs_f64() < feedback.until
        } {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}
