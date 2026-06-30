//! Mouse aiming: a screen-center ray drives the turret/gun servos, with RMB free-look, plus
//! the HUD (center reticle, green bore dot, amber aim-point dot). The committed
//! aim point is stored in the hull's local frame, so it rides with the tank (WW2: no gun
//! stabilization). Storing it in world space instead would be the modern-stabilization split.

use avian3d::prelude::SpatialQuery;
use bevy::ecs::lifecycle::Add;
use bevy::prelude::*;

use crate::camera::GunnerCameraPlaced;
use crate::damage::{Capability, ControlledTank};
use crate::sight::in_third_person;
use crate::state::GameplaySet;
use crate::tank::{Controlled, Gun, Hull, Muzzle, Rig, ServoCommand, Turret};
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
        // Attach AimPoint the moment the rig binds the Turret marker.
        .add_observer(attach_aim_point)
        // Third-person aiming only — in gunner view `sight::drive_gunner_aim` commands the same
        // servos instead (one writer per mode).
        .add_systems(Update, aim.run_if(in_third_person).in_set(GameplaySet))
        // HUD markers reproject through the camera, so they run after the camera's pose is final
        // for the frame — after propagation and after the gunner camera places itself — or they
        // lag/jitter against the rendered view (worst at the gunner optic's high zoom).
        .add_systems(
            PostUpdate,
            (update_bore_indicator, update_aim_indicator)
                .in_set(GameplaySet)
                .after(TransformSystems::Propagate)
                .after(GunnerCameraPlaced),
        );
}

/// Reactively give the turret its `AimPoint` the moment the rig binds the `Turret` marker.
fn attach_aim_point(add: On<Add, Turret>, mut commands: Commands) {
    commands.entity(add.entity).insert(AimPoint(None));
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
    spatial: SpatialQuery,
    camera_query: Single<(&Camera, &GlobalTransform)>,
    window: Single<&Window>,
    controlled: ControlledTank,
    hull: Query<&GlobalTransform, With<Hull>>,
    mut turret: Query<
        (&GlobalTransform, &mut ServoCommand, &mut AimPoint),
        (With<Turret>, Without<Gun>),
    >,
    mut gun: Query<(&GlobalTransform, &mut ServoCommand), (With<Gun>, Without<Turret>)>,
) {
    // Hold RMB to free-look: the camera still pans, but we stop committing aim, so the gun
    // and the locked aim point hold their hull-relative pose.
    if mouse.pressed(MouseButton::Right) {
        return;
    }

    // The controlled tank only — its rig handles address its own turret/gun/hull.
    let Some(rig) = controlled.rig() else {
        return;
    };
    if !controlled.available(Capability::Traverse) {
        return;
    }

    let (camera, cam_transform) = *camera_query;
    let Ok(ray) = camera.viewport_to_world(cam_transform, window.size() / 2.0) else {
        return;
    };

    // Aim at the ground hit, or a far fallback when nothing is struck (sky / above horizon).
    let point = ray.get_point(ground_distance(&spatial, ray, MAX_RANGE));

    // Computed in the hull's local frame so aim stays correct wherever the tank sits/turns.
    let Ok(hull) = hull.get(rig.hull) else {
        return;
    };
    let to_local = hull.affine().inverse();

    // Turret yaw + stash the committed point in hull-local space (rides with the hull).
    if let Ok((turret_transform, mut command, mut aim_point)) = turret.get_mut(rig.turret) {
        let dir = to_local.transform_vector3(point - turret_transform.translation());
        command.target = (-dir.x).atan2(-dir.z);
        aim_point.0 = Some(to_local.transform_point3(point));
    }

    if let Ok((gun_transform, mut command)) = gun.get_mut(rig.gun) {
        let dir = to_local.transform_vector3(point - gun_transform.translation());
        let horizontal = (dir.x * dir.x + dir.z * dir.z).sqrt();
        command.target = dir.y.atan2(horizontal);
    }
}

/// Project `world_point` to the screen and place a HUD dot there (its top-left offset by
/// `half_size` to centre the dot), hiding it when the point is off-screen or behind the camera.
fn place_indicator(
    node: &mut Node,
    visibility: &mut Visibility,
    camera: &Camera,
    cam_transform: &GlobalTransform,
    world_point: Vec3,
    half_size: f32,
) {
    match camera.world_to_viewport(cam_transform, world_point) {
        Ok(screen) => {
            node.left = Val::Px(screen.x - half_size);
            node.top = Val::Px(screen.y - half_size);
            *visibility = Visibility::Visible;
        }
        Err(_) => *visibility = Visibility::Hidden,
    }
}

fn update_bore_indicator(
    spatial: SpatialQuery,
    camera_query: Single<(&Camera, &GlobalTransform)>,
    controlled: Query<&Rig, With<Controlled>>,
    muzzle: Query<&GlobalTransform, With<Muzzle>>,
    mut indicator: Query<(&mut Node, &mut Visibility), With<BoreIndicator>>,
) {
    let (camera, cam_transform) = *camera_query;
    let Ok(rig) = controlled.single() else {
        return;
    };
    let Ok(muzzle) = muzzle.get(rig.muzzle) else {
        return;
    };
    let Ok((mut node, mut visibility)) = indicator.single_mut() else {
        return;
    };

    // Where the barrel is actually pointing, capped exactly like the aim picker.
    let ray = Ray3d::new(muzzle.translation(), muzzle.forward());
    let point = ray.get_point(ground_distance(&spatial, ray, MAX_RANGE));

    place_indicator(
        &mut node,
        &mut visibility,
        camera,
        cam_transform,
        point,
        2.0,
    );
}

fn update_aim_indicator(
    mouse: Res<ButtonInput<MouseButton>>,
    camera_query: Single<(&Camera, &GlobalTransform)>,
    controlled: Query<&Rig, With<Controlled>>,
    hull: Query<&GlobalTransform, With<Hull>>,
    aim_point: Query<&AimPoint, With<Turret>>,
    mut indicator: Query<(&mut Node, &mut Visibility), With<AimIndicator>>,
) {
    let (camera, cam_transform) = *camera_query;
    let Ok((mut node, mut visibility)) = indicator.single_mut() else {
        return;
    };

    // Shown only during free-look (RMB held) — otherwise it coincides with the center reticle.
    if !mouse.pressed(MouseButton::Right) {
        *visibility = Visibility::Hidden;
        return;
    }

    let Ok(rig) = controlled.single() else {
        *visibility = Visibility::Hidden;
        return;
    };
    let Ok(hull) = hull.get(rig.hull) else {
        return;
    };
    let Ok(aim_point) = aim_point.get(rig.turret) else {
        return;
    };

    // No committed aim yet (before first aim, or free-look from frame one).
    let Some(local) = aim_point.0 else {
        *visibility = Visibility::Hidden;
        return;
    };

    // Hull-local -> world, so the dot rides with the hull (unstabilized WW2 behaviour).
    let world = hull.affine().transform_point3(local);

    place_indicator(
        &mut node,
        &mut visibility,
        camera,
        cam_transform,
        world,
        3.0,
    );
}
