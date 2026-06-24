//! Mouse aiming: a screen-center ray drives the turret/gun servos, with RMB free-look, plus
//! the HUD (center reticle, green bore dot, amber aim-point dot). The committed
//! aim point is stored in the hull's local frame, so it rides with the tank (WW2: no gun
//! stabilization). Storing it in world space instead would be the modern-stabilization split.

use bevy::prelude::*;

use crate::state::GameplaySet;
use crate::tank::{Gun, Hull, Muzzle, Servo, Turret};
use crate::world::ground_distance;

/// Maximum engagement range; rays that hit nothing fall back to a point this far out.
const MAX_RANGE: f32 = 10_000.0;

/// The committed aim point in the hull's local frame. `None` until the first commit.
/// ("Target" is reserved for a designated enemy; this is the commanded ground point.)
#[derive(Component)]
struct AimPoint(Option<Vec3>);

/// HUD: where the barrel is actually pointing (lags the reticle) — the gun's reality.
#[derive(Component)]
struct BoreIndicator;

/// HUD: the committed aim point — where the gun is *commanded* to point. Shown only during
/// free-look, since otherwise it sits exactly under the center reticle.
#[derive(Component)]
struct AimIndicator;

pub fn plugin(app: &mut App) {
    app.add_systems(Startup, spawn_hud)
        // Attach AimPoint to the turret once the rig binds it (init, so ungated).
        .add_systems(Update, attach_aim_point)
        .add_systems(
            Update,
            (aim, update_bore_indicator, update_aim_indicator).in_set(GameplaySet),
        );
}

/// Reactively give the turret its `AimPoint` when the tank rig binds the `Turret` marker.
fn attach_aim_point(turrets: Query<Entity, Added<Turret>>, mut commands: Commands) {
    for entity in &turrets {
        commands.entity(entity).insert(AimPoint(None));
    }
}

fn spawn_hud(mut commands: Commands) {
    // Center reticle: a small white dot held at screen center by flexbox.
    commands
        .spawn(Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        })
        .with_children(|parent| {
            parent.spawn((
                Node {
                    width: Val::Px(6.0),
                    height: Val::Px(6.0),
                    border_radius: BorderRadius::MAX,
                    ..default()
                },
                BackgroundColor(Color::WHITE),
            ));
        });

    // Green: actual bore. Amber: commanded aim (free-look only). Both hidden until shown.
    commands.spawn((
        BoreIndicator,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Px(4.0),
            height: Val::Px(4.0),
            border_radius: BorderRadius::MAX,
            ..default()
        },
        BackgroundColor(Color::srgba(0.3, 0.9, 0.4, 0.6)),
        Visibility::Hidden,
    ));
    commands.spawn((
        AimIndicator,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Px(6.0),
            height: Val::Px(6.0),
            border_radius: BorderRadius::MAX,
            ..default()
        },
        BackgroundColor(Color::srgba(1.0, 0.7, 0.1, 0.7)),
        Visibility::Hidden,
    ));
}

fn aim(
    mouse: Res<ButtonInput<MouseButton>>,
    camera_query: Single<(&Camera, &GlobalTransform)>,
    window: Single<&Window>,
    hull: Query<&GlobalTransform, With<Hull>>,
    mut turret: Query<(&GlobalTransform, &mut Servo, &mut AimPoint), (With<Turret>, Without<Gun>)>,
    mut gun: Query<(&GlobalTransform, &mut Servo), (With<Gun>, Without<Turret>)>,
) {
    // Hold RMB to free-look: the camera still pans, but we stop committing aim, so the gun
    // and the locked aim point hold their hull-relative pose.
    if mouse.pressed(MouseButton::Right) {
        return;
    }

    let (camera, cam_transform) = *camera_query;
    let Ok(ray) = camera.viewport_to_world(cam_transform, window.size() / 2.0) else { return; };

    // Aim at the ground hit, or a far fallback when nothing is struck (sky / above horizon).
    let point = ray.get_point(ground_distance(ray, MAX_RANGE));

    // Computed in the hull's local frame so aim stays correct wherever the tank sits/turns.
    let Ok(hull) = hull.single() else { return; };
    let to_local = hull.affine().inverse();

    // Turret yaw + stash the committed point in hull-local space (rides with the hull).
    if let Ok((turret_transform, mut servo, mut aim_point)) = turret.single_mut() {
        let dir = to_local.transform_vector3(point - turret_transform.translation());
        servo.target = (-dir.x).atan2(-dir.z);
        aim_point.0 = Some(to_local.transform_point3(point));
    }

    // Gun pitch.
    if let Ok((gun_transform, mut servo)) = gun.single_mut() {
        let dir = to_local.transform_vector3(point - gun_transform.translation());
        let horizontal = (dir.x * dir.x + dir.z * dir.z).sqrt();
        servo.target = dir.y.atan2(horizontal);
    }
}

fn update_bore_indicator(
    camera_query: Single<(&Camera, &GlobalTransform)>,
    muzzle: Query<&GlobalTransform, With<Muzzle>>,
    mut indicator: Query<(&mut Node, &mut Visibility), With<BoreIndicator>>,
) {
    let (camera, cam_transform) = *camera_query;
    let Ok(muzzle) = muzzle.single() else { return; };
    let Ok((mut node, mut visibility)) = indicator.single_mut() else { return; };

    // Where the barrel is actually pointing, capped exactly like the aim picker.
    let ray = Ray3d::new(muzzle.translation(), muzzle.forward());
    let point = ray.get_point(ground_distance(ray, MAX_RANGE));

    match camera.world_to_viewport(cam_transform, point) {
        Ok(screen) => {
            node.left = Val::Px(screen.x - 2.0);
            node.top = Val::Px(screen.y - 2.0);
            *visibility = Visibility::Visible;
        }
        Err(_) => *visibility = Visibility::Hidden,
    }
}

fn update_aim_indicator(
    mouse: Res<ButtonInput<MouseButton>>,
    camera_query: Single<(&Camera, &GlobalTransform)>,
    hull: Query<&GlobalTransform, With<Hull>>,
    aim_point: Query<&AimPoint, With<Turret>>,
    mut indicator: Query<(&mut Node, &mut Visibility), With<AimIndicator>>,
) {
    let (camera, cam_transform) = *camera_query;
    let Ok((mut node, mut visibility)) = indicator.single_mut() else { return; };

    // Shown only during free-look (RMB held) — otherwise it coincides with the center reticle.
    if !mouse.pressed(MouseButton::Right) {
        *visibility = Visibility::Hidden;
        return;
    }

    let Ok(hull) = hull.single() else { return; };
    let Ok(aim_point) = aim_point.single() else { return; };

    // No committed aim yet (before first aim, or free-look from frame one).
    let Some(local) = aim_point.0 else {
        *visibility = Visibility::Hidden;
        return;
    };

    // Hull-local -> world, so the dot rides with the hull (unstabilized WW2 behaviour).
    let world = hull.affine().transform_point3(local);

    match camera.world_to_viewport(cam_transform, world) {
        Ok(screen) => {
            node.left = Val::Px(screen.x - 3.0);
            node.top = Val::Px(screen.y - 3.0);
            *visibility = Visibility::Visible;
        }
        Err(_) => *visibility = Visibility::Hidden,
    }
}
