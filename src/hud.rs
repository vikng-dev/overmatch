//! Shared world-anchored HUD for game and armor sandbox.
//!
//! Invariant: labels read simulation state but never write it; each composition supplies a
//! [`HudCamera`] rather than coupling shared HUD systems to a camera implementation.

use bevy::prelude::*;

use crate::net::NetBot;

use crate::ballistics::{ComponentHealth, ComponentVolume};
use crate::damage::{Ammo, CrewStation, FunctionRole};
use crate::tank::{Tank, ViewNode};
use crate::ui_font::UiFonts;

/// The camera the HUD reprojects world points through. Each binary tags its own world camera with
/// this — the game's player camera, the sandbox's free-fly camera — so the shared systems don't
/// depend on either binary's camera marker (the sandbox has three `Camera3d`s; the game has one).
#[derive(Component)]
pub struct HudCamera;

/// A pooled label floated over a damaged component each frame, showing its HP; hidden while unused.
#[derive(Component)]
struct ComponentHpLabel;

/// A pooled tank nameplate, reassigned and reprojected each frame.
#[derive(Component)]
struct TankNameplate;

pub fn plugin(app: &mut App) {
    app.add_systems(Startup, spawn_labels)
        .add_systems(Update, (update_component_hp_labels, update_tank_nameplates));
}

fn spawn_labels(mut commands: Commands, fonts: Res<UiFonts>) {
    // Pool of HP labels floated over damaged components each frame; hidden while unused.
    for _ in 0..12 {
        commands.spawn((
            ComponentHpLabel,
            Text::new(""),
            TextFont {
                // Regular: a small, dense numeric readout (13px).
                font: fonts.body.clone().into(),
                font_size: FontSize::Px(13.0),
                ..default()
            },
            TextColor(Color::srgb(1.0, 0.8, 0.3)),
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            Visibility::Hidden,
        ));
    }
    // Nameplate pool floated over each tank — just the display name. Reprojected each frame; hidden
    // while unused. Four covers the current SP duel + a bit of headroom, same as the old pool.
    for _ in 0..4 {
        commands.spawn((
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

/// Float an HP readout over each *damaged* component (current < max), reprojected to screen; hide
/// the leftover labels. Lets you watch transit damage and spall chip components down (red at 0).
/// Anchored at the volume's VIEW node where one is attached ([`ViewNode::resolve`]): a
/// turret-resident component's sim pose steps at tick rate since the sim/view split, and the
/// label must track the smoothly-rendered model, not the stepped skeleton.
fn update_component_hp_labels(
    camera: Single<(&Camera, &GlobalTransform), With<HudCamera>>,
    components: Query<
        (
            Entity,
            Option<&ViewNode>,
            &ComponentHealth,
            Option<&CrewStation>,
            Option<&Ammo>,
            Option<&FunctionRole>,
            Option<&Name>,
        ),
        With<ComponentVolume>,
    >,
    transforms: Query<&GlobalTransform>,
    mut labels: Query<
        (&mut Node, &mut Text, &mut Visibility, &mut TextColor),
        With<ComponentHpLabel>,
    >,
) {
    let (camera, cam_transform) = *camera;
    let mut damaged = components
        .iter()
        .filter(|(_, _, hp, _, _, _, _)| hp.current < hp.max);
    for (mut node, mut text, mut visibility, mut color) in &mut labels {
        let Some((entity, view, hp, crew, ammo, function, name)) = damaged.next() else {
            *visibility = Visibility::Hidden;
            continue;
        };
        let Ok(transform) = transforms.get(ViewNode::resolve(view, entity)) else {
            *visibility = Visibility::Hidden;
            continue;
        };
        match camera.world_to_viewport(cam_transform, transform.translation()) {
            Ok(screen) => {
                node.left = Val::Px(screen.x + 8.0);
                node.top = Val::Px(screen.y - 8.0);
                *text = Text::new(format!(
                    "{}\n{:.1}/{:.0} hp",
                    volume_label(crew, ammo, function, name),
                    hp.current,
                    hp.max
                ));
                *color = TextColor(if hp.current <= 0.0 {
                    Color::srgb(1.0, 0.3, 0.2)
                } else {
                    Color::srgb(1.0, 0.8, 0.3)
                });
                *visibility = Visibility::Visible;
            }
            Err(_) => *visibility = Visibility::Hidden,
        }
    }
}

fn volume_label(
    crew: Option<&CrewStation>,
    ammo: Option<&Ammo>,
    function: Option<&FunctionRole>,
    name: Option<&Name>,
) -> String {
    if let Some(crew) = crew {
        crew.label().to_string()
    } else if let Some(function) = function {
        function.label().to_string()
    } else if ammo.is_some() {
        name.map(|name| name.as_str().replace("_Ballistic", ""))
            .unwrap_or_else(|| "Ammo".to_string())
    } else {
        name.map(|name| name.as_str().replace("_Ballistic", ""))
            .unwrap_or_else(|| "Component".to_string())
    }
}

/// Float a name nameplate over each tank, reprojected to screen; hide the leftover plates. Anchored
/// a little above the hull so it clears the model. The aggregate status this used to carry now lives
/// in the controlled tank's fixed corner panel (`crew_ui::update_status_panel`).
fn update_tank_nameplates(
    camera: Single<(&Camera, &GlobalTransform), With<HudCamera>>,
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
