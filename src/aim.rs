//! Mouse aiming: a screen-center ray commits the shared aim point, which every servo then chases
//! (`drive_aim_servos`) — turret, gun, and the hull MG alike. RMB free-look holds the committed
//! point; the HUD shows the center reticle, green bore dot, and amber aim-point dot. The committed
//! aim point is stored in the hull's local frame, so it rides with the tank (WW2: no gun
//! stabilization). Storing it in world space instead would be the modern-stabilization split.
//!
//! The servo drive (`drive_aim_servos`) is mode-agnostic: it reads the one `AimPoint` regardless of
//! who wrote it, so the gunner optic (`sight::drive_gunner_aim`) reuses it by committing the point
//! from its magnified intent instead of commanding the servos itself.

use avian3d::prelude::SpatialQuery;
use bevy::ecs::lifecycle::Add;
use bevy::prelude::*;

use crate::camera::GunnerCameraPlaced;
use crate::damage::ControlledTank;
use crate::firecontrol::{RangeTable, Ranging, lob};
use crate::sight::in_third_person;
use crate::state::GameplaySet;
use crate::tank::{Controlled, Hull, Muzzle, Rig, ServoCommand, ServoRole, TankRoot, Turret};
use crate::world::ground_distance;

/// Maximum engagement range; rays that hit nothing fall back to a point this far out.
const MAX_RANGE: f32 = 10_000.0;

/// The committed aim point in the hull's local frame. `None` until the first commit. The single
/// shared target both view modes write (third-person ray, gunner intent) and `drive_aim_servos`
/// reads — every servo chases this one point. ("Target" is reserved for a designated enemy; this is
/// the commanded ground point.)
#[derive(Component)]
pub(crate) struct AimPoint(pub(crate) Option<Vec3>);

/// The aim-commit phase: the per-mode input systems that write [`AimPoint`] (`commit_aim` in
/// third-person, `sight::drive_gunner_aim` in the optic). `drive_aim_servos` runs `.after` this, so
/// it always reads the point committed this frame regardless of which mode wrote it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AimCommit;

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
        .add_systems(
            Update,
            (
                // Per-mode aim commit: third-person from the screen-center ray; the optic commits
                // from its magnified intent (`sight::drive_gunner_aim`, also in `AimCommit`).
                commit_aim.run_if(in_third_person).in_set(AimCommit),
                // Mode-agnostic: every controlled-tank servo chases the committed point.
                drive_aim_servos.after(AimCommit),
            )
                .in_set(GameplaySet),
        )
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

/// Third-person aim commit: a screen-center ray picks the ground point (or a far fallback) and
/// stores it hull-local on the turret's [`AimPoint`]. RMB free-look stops committing, so the held
/// point (and the servos chasing it) keep their hull-relative pose. The servos themselves are driven
/// by `drive_aim_servos`, shared with the gunner optic.
fn commit_aim(
    mouse: Res<ButtonInput<MouseButton>>,
    spatial: SpatialQuery,
    camera_query: Single<(&Camera, &GlobalTransform)>,
    window: Single<&Window>,
    controlled: ControlledTank,
    hull: Query<&GlobalTransform, With<Hull>>,
    mut aim_point: Query<&mut AimPoint>,
) {
    // Hold RMB to free-look: the camera still pans, but we stop committing aim, so the servos
    // and the locked aim point hold their hull-relative pose.
    if mouse.pressed(MouseButton::Right) {
        return;
    }

    let Some(rig) = controlled.rig() else {
        return;
    };

    let (camera, cam_transform) = *camera_query;
    let Ok(ray) = camera.viewport_to_world(cam_transform, window.size() / 2.0) else {
        return;
    };

    // Aim at the ground hit, or a far fallback when nothing is struck (sky / above horizon).
    let point = ray.get_point(ground_distance(&spatial, ray, MAX_RANGE));

    // Stored in the hull's local frame so aim stays correct wherever the tank sits/turns.
    let Ok(hull) = hull.get(rig.hull) else {
        return;
    };
    // Store the raw committed point — the player's aim *intention*. The superelevation lob is added
    // downstream in `drive_aim_servos`, so this stays the intention (what the amber HUD dot shows) and
    // the green bore dot ends up the superelevation above it.
    if let Ok(mut aim_point) = aim_point.get_mut(rig.turret) {
        aim_point.0 = Some(hull.affine().inverse().transform_point3(point));
    }
}

/// Drive every servo of the controlled tank at the one committed aim point — mode-agnostic, so the
/// same logic serves third-person and the gunner optic. Yaw solves azimuth, Pitch solves elevation,
/// each from its own pose; the hierarchy composes nested mounts, so the turret+gun and the hull MG
/// converge independently with no chain logic here. Whether a mount actually slews is its own gate
/// (`drive_servos`); this just writes the intent. The committed point is the raw aim *intention*;
/// this bridge lobs it up by the main gun's superelevation for the dialed range, so the bore rides
/// above the line of sight while `drive_servos` stays a generic point-chaser. The coax + hull MG ride
/// the gun's lob until per-weapon laying lands.
fn drive_aim_servos(
    controlled: ControlledTank,
    hull: Query<&GlobalTransform, With<Hull>>,
    aim_point: Query<&AimPoint>,
    ranging: Res<Ranging>,
    tables: Query<&RangeTable>,
    mut servos: Query<(&GlobalTransform, &mut ServoCommand, &ServoRole, &TankRoot)>,
) {
    let Some(controlled_entity) = controlled.entity() else {
        return;
    };
    let Some(rig) = controlled.rig() else {
        return;
    };
    let Ok(hull) = hull.get(rig.hull) else {
        return;
    };
    let Ok(&AimPoint(Some(local))) = aim_point.get(rig.turret) else {
        return;
    };

    // Lob the raw intention up by the superelevation here (not at commit), so the stored aim point —
    // and its amber HUD dot — stay the intention, while the bore the servos reach is the lobbed point.
    let theta = tables
        .get(rig.muzzle)
        .map_or(0.0, |table| table.superelevation(ranging.range));
    let hull_affine = hull.affine();
    let point = hull_affine.transform_point3(lob(local, theta));
    let to_local = hull_affine.inverse();
    for (transform, mut command, role, root) in &mut servos {
        if root.0 != controlled_entity {
            continue;
        }
        let dir = to_local.transform_vector3(point - transform.translation());
        command.target = match role {
            ServoRole::Yaw => (-dir.x).atan2(-dir.z),
            ServoRole::Pitch => dir.y.atan2((dir.x * dir.x + dir.z * dir.z).sqrt()),
        };
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
