//! Shared tank-state HUD: world-anchored readouts both the game and the armor sandbox mount. The
//! systems reproject through whichever camera carries [`HudCamera`], so each binary tags its own
//! world camera (the game's third-person/gunner camera; the sandbox's free-fly camera) and the
//! shared code depends on neither binary's camera marker. The state shown is read straight from the
//! damage model (capability effectiveness, crew, component HP, knock-out), so the HUD stays a pure
//! *view* — it owns no gameplay state, and the same numbers can later drive a designed player HUD,
//! voice, and VFX (this is the diagnostic seam).

use bevy::prelude::*;

use crate::ballistics::{ComponentHealth, ComponentVolume};
use crate::damage::{
    Ammo, Capability, CookedOff, CrewStation, Crewman, Dead, FunctionRole, TankCapabilities,
    TankKnockedOut, TankVolumes, VolumeFacets, capability_effectiveness,
};
use crate::tank::Tank;

/// The camera the HUD reprojects world points through. Each binary tags its own world camera with
/// this — the game's player camera, the sandbox's free-fly camera — so the shared systems don't
/// depend on either binary's camera marker (the sandbox has three `Camera3d`s; the game has one).
#[derive(Component)]
pub struct HudCamera;

/// A pooled label floated over a damaged component each frame, showing its HP; hidden while unused.
#[derive(Component)]
struct ComponentHpLabel;

/// A pooled aggregate readout floated over each tank: terminal state, living crew, cookoff, disabled
/// module functions, and per-capability effectiveness. Reassigned to a live tank each frame.
#[derive(Component)]
struct TankStatusLabel;

pub fn plugin(app: &mut App) {
    app.add_systems(Startup, spawn_labels).add_systems(
        Update,
        (update_component_hp_labels, update_tank_status_labels),
    );
}

fn spawn_labels(mut commands: Commands) {
    // Pool of HP labels floated over damaged components each frame; hidden while unused.
    for _ in 0..12 {
        commands.spawn((
            ComponentHpLabel,
            Text::new(""),
            TextFont {
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
    // Aggregate tank-state labels: living crew, cookoff, knockout, disabled functions, capabilities.
    for _ in 0..4 {
        commands.spawn((
            TankStatusLabel,
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(14.0),
                ..default()
            },
            TextColor(Color::srgb(0.88, 0.95, 1.0)),
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            Visibility::Hidden,
        ));
    }
}

/// Float an HP readout over each *damaged* component (current < max), reprojected to screen; hide
/// the leftover labels. Lets you watch transit damage and spall chip components down (red at 0).
fn update_component_hp_labels(
    camera: Single<(&Camera, &GlobalTransform), With<HudCamera>>,
    components: Query<
        (
            &GlobalTransform,
            &ComponentHealth,
            Option<&CrewStation>,
            Option<&Ammo>,
            Option<&FunctionRole>,
            Option<&Name>,
        ),
        With<ComponentVolume>,
    >,
    mut labels: Query<
        (&mut Node, &mut Text, &mut Visibility, &mut TextColor),
        With<ComponentHpLabel>,
    >,
) {
    let (camera, cam_transform) = *camera;
    let mut damaged = components
        .iter()
        .filter(|(_, hp, _, _, _, _)| hp.current < hp.max);
    for (mut node, mut text, mut visibility, mut color) in &mut labels {
        let Some((transform, hp, crew, ammo, function, name)) = damaged.next() else {
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
        name.map(|name| name.as_str().replace("Ballistic_", ""))
            .unwrap_or_else(|| "Ammo".to_string())
    } else {
        name.map(|name| name.as_str().replace("Ballistic_", ""))
            .unwrap_or_else(|| "Component".to_string())
    }
}

/// Float an aggregate status readout over each tank: terminal state, living crew, cookoff, and
/// currently-disabled module functions, plus per-capability effectiveness. Deliberately diagnostic:
/// the final game can turn the same state into a designed HUD, voice, and VFX later.
fn update_tank_status_labels(
    camera: Single<(&Camera, &GlobalTransform), With<HudCamera>>,
    tanks: Query<
        (
            &GlobalTransform,
            Option<&Name>,
            Option<&TankKnockedOut>,
            Option<&TankVolumes>,
            Option<&TankCapabilities>,
        ),
        With<Tank>,
    >,
    volumes: Query<(
        Option<&CrewStation>,
        Option<&Crewman>,
        Option<&Dead>,
        Option<&FunctionRole>,
        Option<&ComponentHealth>,
        Option<&Ammo>,
        Option<&CookedOff>,
        Option<&Name>,
    )>,
    capability_volumes: Query<VolumeFacets>,
    mut labels: Query<
        (&mut Node, &mut Text, &mut Visibility, &mut TextColor),
        With<TankStatusLabel>,
    >,
) {
    let (camera, cam_transform) = *camera;
    let mut tanks = tanks.iter();
    for (mut node, mut text, mut visibility, mut color) in &mut labels {
        let Some((transform, name, knocked_out, tank_volumes, tank_caps)) = tanks.next() else {
            *visibility = Visibility::Hidden;
            continue;
        };

        let mut crew_total = 0;
        let mut crew_living = 0;
        let mut dead_crew: Vec<&'static str> = Vec::new();
        let mut cooked_off: Vec<String> = Vec::new();
        let mut disabled: Vec<&'static str> = Vec::new();

        if let Some(tank_volumes) = tank_volumes {
            for volume in tank_volumes.iter() {
                let Ok((crew, _crewman, dead, function, hp, ammo, cooked, volume_name)) =
                    volumes.get(volume)
                else {
                    continue;
                };
                if let Some(station) = crew {
                    crew_total += 1;
                    if dead.is_some() || hp.is_some_and(|hp| hp.current <= 0.0) {
                        dead_crew.push(station.label());
                    } else {
                        crew_living += 1;
                    }
                }
                if ammo.is_some() && cooked.is_some() {
                    cooked_off.push(
                        volume_name
                            .map(|name| name.as_str().to_string())
                            .unwrap_or_else(|| "ammo".to_string()),
                    );
                }
                if let (Some(function), Some(hp)) = (function, hp)
                    && hp.current <= 0.0
                {
                    disabled.push(function.label());
                }
            }
        }

        // Capability readout: each capability as a live effectiveness percentage. Composed from its
        // requirement groups against the live world (design §7b) — a backfilled capability reads
        // (e.g.) 60%, not just on/off.
        let caps = [
            Capability::Drive,
            Capability::Traverse,
            Capability::Fire,
            Capability::Load,
            Capability::GunnerSight,
            Capability::CommanderView,
        ];
        let cap_line = caps
            .iter()
            .map(|cap| {
                let eff =
                    capability_effectiveness(tank_volumes, tank_caps, *cap, &capability_volumes);
                format!("{} {}%", cap.label(), (eff * 100.0).round() as i32)
            })
            .collect::<Vec<_>>()
            .join("   ");

        let world_point = transform.translation() + Vec3::Y * 4.4;
        match camera.world_to_viewport(cam_transform, world_point) {
            Ok(screen) => {
                node.left = Val::Px(screen.x + 12.0);
                node.top = Val::Px(screen.y - 56.0);
                let title = name.map(|name| name.as_str()).unwrap_or("Tiger I");
                let state = knocked_out
                    .map(|state| format!("KNOCKED OUT ({})", state.reason.label()))
                    .unwrap_or_else(|| "ALIVE".to_string());
                let dead = if dead_crew.is_empty() {
                    "-".to_string()
                } else {
                    dead_crew.join(", ")
                };
                let cookoff = if cooked_off.is_empty() {
                    "no".to_string()
                } else {
                    cooked_off.join(", ")
                };
                let has_disabled = !disabled.is_empty();
                let disabled = if disabled.is_empty() {
                    "-".to_string()
                } else {
                    disabled.join(", ")
                };
                *text = Text::new(format!(
                    "{title}\n{state}\nCrew {crew_living}/{crew_total}\nDead: {dead}\nCookoff: {cookoff}\nDisabled: {disabled}\nCapabilities: {cap_line}",
                ));
                *color = TextColor(if knocked_out.is_some() {
                    Color::srgb(1.0, 0.35, 0.25)
                } else if !dead_crew.is_empty() || has_disabled {
                    Color::srgb(1.0, 0.85, 0.35)
                } else {
                    Color::srgb(0.85, 0.95, 1.0)
                });
                *visibility = Visibility::Visible;
            }
            Err(_) => *visibility = Visibility::Hidden,
        }
    }
}
