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
use crate::ui_font::UiFonts;
use crate::view::PlayerView;

/// A pooled tank nameplate, reassigned and reprojected each frame.
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
) {
    let (camera, cam_transform) = *camera;
    let mut tanks = tanks.iter();
    for (mut node, mut text, mut visibility) in &mut labels {
        let Some((entity, transform, name)) = tanks.next() else {
            *visibility = Visibility::Hidden;
            continue;
        };
        let world_point = transform.translation() + Vec3::Y * 4.4;
        match camera.world_to_viewport(cam_transform, world_point) {
            Ok(screen) => {
                node.left = Val::Px(screen.x + 12.0);
                node.top = Val::Px(screen.y - 20.0);
                let label = name.map(|name| name.as_str()).unwrap_or("Tiger I");
                let label = if bots.get(entity).is_ok() {
                    format!("[BOT] {label}")
                } else {
                    label.to_string()
                };
                *text = Text::new(label);
                *visibility = Visibility::Visible;
            }
            Err(_) => *visibility = Visibility::Hidden,
        }
    }
}
