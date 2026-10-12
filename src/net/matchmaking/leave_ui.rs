//! The LEAVE MATCH button, matchmaking mode's exit from a match: a root of the match screen on the
//! menu's action rung (`overlay::Overlay::MENU_ACTIONS_Z`), shown exactly while the Esc menu owns
//! the scrim. A click sends [`LeaveMatch`]; `net::client` disconnects and drops the link.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;

use super::LeaveMatch;
use crate::flow::{Flow, FlowScoped, InMatchSystems};
use crate::overlay::{self, Overlay, OverlaySet, Overlays};
use crate::ui_font::{TEXT, UiFonts};

const BUTTON: Color = Color::srgb(0.40, 0.17, 0.14);
const BUTTON_HOVER: Color = Color::srgb(0.52, 0.23, 0.18);
const LABEL_PX: f32 = 22.0;

#[derive(Component)]
struct LeaveMatchRoot;

#[derive(Component)]
struct LeaveMatchButton;

pub(super) fn plugin(app: &mut App) {
    crate::flow::require(app, "matchmaking::leave_ui");
    app.add_observer(clicked)
        .add_systems(OnEnter(Flow::InMatch), spawn)
        .add_systems(
            Update,
            show.after(OverlaySet::Toggle).in_set(InMatchSystems),
        );
}

fn spawn(mut commands: Commands, fonts: Res<UiFonts>) {
    commands
        .spawn((
            FlowScoped(Flow::InMatch),
            LeaveMatchRoot,
            GlobalZIndex(Overlay::MENU_ACTIONS_Z),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                bottom: Val::Percent(3.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
            Visibility::Hidden,
        ))
        .with_children(|root| {
            root.spawn((
                LeaveMatchButton,
                Hovered::default(),
                Node {
                    padding: UiRect::axes(Val::Px(26.0), Val::Px(8.0)),
                    border_radius: BorderRadius::all(Val::Px(4.0)),
                    ..default()
                },
                BackgroundColor(BUTTON),
            ))
            .with_child((
                Text::new("LEAVE MATCH"),
                TextFont {
                    font: fonts.hud.clone().into(),
                    font_size: FontSize::Px(LABEL_PX),
                    ..default()
                },
                TextColor(TEXT),
            ));
        });
}

/// Shown while the menu owns the scrim; the fill follows hover.
fn show(
    overlays: Res<Overlays>,
    mut root: Query<&mut Visibility, With<LeaveMatchRoot>>,
    mut buttons: Query<(&Hovered, &mut BackgroundColor), With<LeaveMatchButton>>,
) {
    let shown = overlay::draws_scrim(&overlays, Overlay::Menu);
    for mut visibility in &mut root {
        visibility.set_if_neq(if shown {
            Visibility::Visible
        } else {
            Visibility::Hidden
        });
    }
    for (hovered, mut fill) in &mut buttons {
        fill.set_if_neq(BackgroundColor(if hovered.get() {
            BUTTON_HOVER
        } else {
            BUTTON
        }));
    }
}

fn clicked(
    mut click: On<Pointer<Click>>,
    buttons: Query<(), With<LeaveMatchButton>>,
    overlays: Res<Overlays>,
    mut leave: MessageWriter<LeaveMatch>,
) {
    if buttons.get(click.entity).is_err() {
        return;
    }
    click.propagate(false);
    if click.button == PointerButton::Primary && overlay::draws_scrim(&overlays, Overlay::Menu) {
        leave.write(LeaveMatch);
    }
}
