//! Shared world-anchored HUD for game and armor sandbox: nameplates floated over tanks, as many as
//! the pool holds.
//!
//! Invariant: labels read simulation state but never write it; the camera it reprojects through is
//! the composition's declared [`PlayerView`], never a camera implementation of its own.

use bevy::prelude::*;

use crate::net::NetBot;

use crate::camera::GunnerCameraPlaced;
use crate::flow::{Flow, FlowScoped, InMatchSystems};
use crate::tank::Tank;
use crate::ui_font::{UiFonts, set_text};
use crate::view::PlayerView;

/// A pooled tank nameplate: reprojected each frame, its text rewritten only when the name changes.
#[derive(Component)]
struct TankNameplate;

pub fn plugin(app: &mut App) {
    crate::flow::require(app, "hud");
    app.add_systems(OnEnter(Flow::InMatch), spawn_labels)
        .add_systems(
            PostUpdate,
            // Reprojection runs after propagation and gunner-camera placement (`GunnerCameraPlaced`,
            // itself after `Propagate`), so labels use the same frame's transforms as the rendered
            // model. Read any earlier, they see the previous frame's tank and camera globals while the
            // mesh renders from this frame's — a skew that is not constant (Avian re-interpolates the
            // overstep and the orbit camera re-tracks every frame), so the labels jitter against the
            // model. Same read-after-Propagate idiom as the gunner reticles (`sight`) and the trace
            // recorder.
            update_tank_nameplates
                .after(TransformSystems::Propagate)
                .after(GunnerCameraPlaced)
                .in_set(InMatchSystems),
        );
}

fn spawn_labels(mut commands: Commands, fonts: Res<UiFonts>) {
    // Nameplate pool, just the display name, reprojected each frame and hidden while unused. Four
    // plates: tanks past them, in query order, go unnamed (TUNED for the duel; a fuller match
    // wants a bigger pool).
    for _ in 0..4 {
        commands.spawn((
            FlowScoped(Flow::InMatch),
            TankNameplate,
            Text::new(""),
            TextFont {
                // SemiBold: a tank identity chip floated over the world — reads as a label.
                font: fonts.hud.clone().into(),
                font_size: FontSize::Px(15.0),
                ..default()
            },
            TextColor(Color::srgb(0.90, 0.96, 1.0)),
            Node {
                position_type: PositionType::Absolute,
                padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
                border_radius: BorderRadius::all(Val::Px(3.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.04, 0.06, 0.08, 0.55)),
            Visibility::Hidden,
        ));
    }
}

/// Float a nameplate over each tank the pool reaches, in query order, reprojected to screen; hide
/// the leftover plates. Anchored a little above the hull so it clears the model. The controlled
/// tank's aggregate status is the fixed corner panel's (`crew_ui::update_status_panel`).
fn update_tank_nameplates(
    camera: Single<(&Camera, &GlobalTransform), With<PlayerView>>,
    tanks: Query<(Entity, &GlobalTransform, Option<&Name>), With<Tank>>,
    // A bot carries the replicated `NetBot` marker (`Name` doesn't ride the wire), so its nameplate
    // is prefixed `[BOT]`.
    bots: Query<(), With<NetBot>>,
    mut labels: Query<(&mut Node, &mut Text, &mut Visibility), With<TankNameplate>>,
    mut scratch: Local<String>,
) {
    let (camera, cam_transform) = *camera;
    let mut tanks = tanks.iter();
    for (mut node, mut text, mut visibility) in &mut labels {
        let Some((entity, transform, name)) = tanks.next() else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        let world_point = transform.translation() + Vec3::Y * 4.4;
        match camera.world_to_viewport(cam_transform, world_point) {
            Ok(screen) => {
                // A tank without a `Name` (the tank's data supplies it) gets no plate rather than
                // an invented label.
                let Some(name) = name else {
                    visibility.set_if_neq(Visibility::Hidden);
                    continue;
                };
                node.left = Val::Px(screen.x + 12.0);
                node.top = Val::Px(screen.y - 20.0);
                scratch.clear();
                if bots.get(entity).is_ok() {
                    scratch.push_str("[BOT] ");
                }
                scratch.push_str(name.as_str());
                set_text(text.reborrow(), &scratch);
                visibility.set_if_neq(Visibility::Visible);
            }
            Err(_) => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::camera::{ComputedCameraValues, RenderTargetInfo};

    use super::*;

    /// Frames in which `Text` was flagged changed, counted by a detector scheduled after the writer.
    #[derive(Resource, Default)]
    struct TextChanges(u32);

    fn count_text_changes(
        changed: Query<(), (With<TankNameplate>, Changed<Text>)>,
        mut counter: ResMut<TextChanges>,
    ) {
        counter.0 += changed.iter().count() as u32;
    }

    /// Invariant: a frame in which no displayed name changes marks no nameplate `Text` changed
    /// (a changed `Text` is re-measured and re-laid-out by `bevy_ui`), while the plate's screen
    /// position still follows the tank every frame. The HUD relies on both: the first is its
    /// layout cost, the second is that a plate never lags the model it labels.
    #[test]
    fn nameplate_text_written_only_on_change_but_position_follows_every_frame() {
        let mut world = World::new();
        world.init_resource::<TextChanges>();
        world.spawn((
            Camera {
                computed: ComputedCameraValues {
                    clip_from_view: Mat4::perspective_infinite_reverse_rh(1.0, 16.0 / 9.0, 0.1),
                    target_info: Some(RenderTargetInfo {
                        physical_size: UVec2::new(1280, 720),
                        scale_factor: 1.0,
                    }),
                    ..default()
                },
                ..default()
            },
            PlayerView,
            GlobalTransform::default(),
        ));
        let tank = world
            .spawn((
                Tank,
                Name::new("Fixture"),
                GlobalTransform::from_xyz(0.0, 0.0, -30.0),
            ))
            .id();
        let plate = world
            .spawn((
                TankNameplate,
                Text::new(""),
                Node::default(),
                Visibility::Hidden,
            ))
            .id();
        let mut schedule = Schedule::default();
        schedule.add_systems((update_tank_nameplates, count_text_changes).chain());

        schedule.run(&mut world);
        assert_eq!(
            world.resource::<TextChanges>().0,
            1,
            "first sight writes the name"
        );
        assert_eq!(world.get::<Text>(plate).unwrap().0, "Fixture");
        let left = world.get::<Node>(plate).unwrap().left;

        world
            .entity_mut(tank)
            .insert(GlobalTransform::from_xyz(5.0, 0.0, -30.0));
        schedule.run(&mut world);
        assert_eq!(
            world.resource::<TextChanges>().0,
            1,
            "an unchanged name must not mark the text changed"
        );
        assert_ne!(
            world.get::<Node>(plate).unwrap().left,
            left,
            "the plate follows the tank"
        );

        world.entity_mut(tank).insert(Name::new("Renamed"));
        schedule.run(&mut world);
        assert_eq!(
            world.resource::<TextChanges>().0,
            2,
            "a new name is written"
        );
        assert_eq!(world.get::<Text>(plate).unwrap().0, "Renamed");
    }

    /// A tank with no `Name` gets no plate: the label comes from the tank's data, never a literal.
    #[test]
    fn unnamed_tank_gets_no_plate() {
        let mut world = World::new();
        world.spawn((
            Camera {
                computed: ComputedCameraValues {
                    clip_from_view: Mat4::perspective_infinite_reverse_rh(1.0, 16.0 / 9.0, 0.1),
                    target_info: Some(RenderTargetInfo {
                        physical_size: UVec2::new(1280, 720),
                        scale_factor: 1.0,
                    }),
                    ..default()
                },
                ..default()
            },
            PlayerView,
            GlobalTransform::default(),
        ));
        world.spawn((Tank, GlobalTransform::from_xyz(0.0, 0.0, -30.0)));
        let plate = world
            .spawn((
                TankNameplate,
                Text::new(""),
                Node::default(),
                Visibility::Visible,
            ))
            .id();
        let mut schedule = Schedule::default();
        schedule.add_systems(update_tank_nameplates);
        schedule.run(&mut world);
        assert_eq!(*world.get::<Visibility>(plate).unwrap(), Visibility::Hidden);
    }
}
