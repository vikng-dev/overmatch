//! The LEAVE MATCH button, matchmaking mode's exit from a match: a root of the match screen on the
//! action rung (`overlay::Overlay::MATCH_ACTIONS_Z`), shown while the Esc menu or reconnect overlay
//! owns the scrim. A click sends [`LeaveMatch`]; `net::client` disconnects and drops the link.

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
            GlobalZIndex(Overlay::MATCH_ACTIONS_Z),
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

fn can_leave(overlays: &Overlays) -> bool {
    overlay::draws_scrim(overlays, Overlay::Menu)
        || overlay::draws_scrim(overlays, Overlay::ConnectStatus)
}

/// Reconnects offer the same immediate exit as the menu; the fill follows hover.
fn show(
    overlays: Res<Overlays>,
    mut root: Query<&mut Visibility, With<LeaveMatchRoot>>,
    mut buttons: Query<(&Hovered, &mut BackgroundColor), With<LeaveMatchButton>>,
) {
    let shown = can_leave(&overlays);
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
    if click.button == PointerButton::Primary && can_leave(&overlays) {
        leave.write(LeaveMatch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::NormalizedRenderTarget;
    use bevy::picking::{
        backend::HitData,
        pointer::{Location, PointerId},
    };

    /// A disconnected player can leave without waiting or opening a menu. The shipped spawner,
    /// visibility system and click observer must all offer the exit above the reconnect scrim.
    #[test]
    fn reconnect_status_offers_a_visible_clickable_leave_match() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .add_plugins(crate::flow::plugin)
            .insert_resource(UiFonts {
                hud: Handle::default(),
                body: Handle::default(),
            })
            .init_resource::<Overlays>()
            .add_message::<LeaveMatch>()
            .add_plugins(plugin);
        app.world_mut()
            .resource_mut::<NextState<Flow>>()
            .set(Flow::InMatch);
        app.world_mut()
            .resource_mut::<Overlays>()
            .declare(Overlay::ConnectStatus, true);
        app.update();
        let world = app.world_mut();
        let (visibility, z) = world
            .query_filtered::<(&Visibility, &GlobalZIndex), With<LeaveMatchRoot>>()
            .single(world)
            .unwrap();
        assert_eq!(*visibility, Visibility::Visible);
        assert!(z.0 > Overlay::ConnectStatus.zindex());
        let button = world
            .query_filtered::<Entity, With<LeaveMatchButton>>()
            .single(world)
            .unwrap();
        // Fixture pointer: no window or renderer is needed to deliver an observer event.
        world.trigger(Pointer::new(
            PointerId::Mouse,
            Location {
                target: NormalizedRenderTarget::None {
                    width: 1,
                    height: 1,
                },
                position: Vec2::ZERO,
            },
            Click {
                button: PointerButton::Primary,
                hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
                duration: std::time::Duration::ZERO,
                count: 1,
            },
            button,
        ));
        assert_eq!(world.resource::<Messages<LeaveMatch>>().len(), 1);
    }
}
