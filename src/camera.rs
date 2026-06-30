//! Third-person orbit camera: free-aim look, scroll-to-zoom dolly, ground-collision pull-in.
//! The camera is also the aiming device, so look direction stays the player's — zoom only
//! changes the orbit radius, which slides along the view axis and never moves the aim point.

use avian3d::prelude::SpatialQuery;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;

use crate::hud::HudCamera;
use crate::sight::{Ranging, in_gunner, in_third_person, superelevation};
use crate::state::GameplaySet;
use crate::tank::{Controlled, Gun, Rig, Tank};
use crate::world::ground_distance;

/// Zoom state on the camera entity. Scroll sets `target_zoom`; `zoom` eases toward it for a
/// smooth dolly. 0 = out (far), 1 = in (near).
#[derive(Component)]
struct OrbitCamera {
    zoom: f32,
    target_zoom: f32,
}

/// When false, the orbit camera holds its current pose instead of following the tank — a debug
/// "detach" used to tell camera-follow jitter apart from physics jitter. Always true in release.
#[derive(Resource)]
pub struct CameraFollow(pub bool);

/// Marks the gunner camera placement, which runs *after* transform propagation (it bolts the camera
/// to the gun's live pose and writes its `GlobalTransform` directly). HUD reprojection orders after
/// this set so markers and the rendered view share one consistent, current camera pose.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GunnerCameraPlaced;

/// The turret-ring pivot, captured once as an offset in the tank root's local frame. The camera
/// orbits `root · this`, so it reads the body's interpolated root `Transform` rather than the
/// turret's (one-frame-stale) `GlobalTransform`. `None` until the rig is bound.
#[derive(Resource, Default)]
struct TurretPivot(Option<Vec3>);

pub fn plugin(app: &mut App) {
    app.insert_resource(CameraFollow(true))
        .init_resource::<TurretPivot>()
        .add_systems(Startup, spawn_camera)
        .add_systems(Update, capture_turret_pivot)
        // Avian's follow-camera guidance: run after physics/interpolation but *before* transform
        // propagation, reading the interpolated `Transform`. Propagation then computes the camera's
        // and the tank's `GlobalTransform` together, so they render consistently — no jitter.
        .add_systems(
            PostUpdate,
            // The orbit camera reads the interpolated root *before* propagation (Avian's follow
            // guidance), so it propagates together with the tank.
            orbit_camera
                .run_if(in_third_person)
                .in_set(GameplaySet)
                .before(TransformSystems::Propagate),
        )
        .add_systems(
            PostUpdate,
            // The gunner camera bolts to the gun's *propagated* pose, so it runs after propagation
            // and writes its own `GlobalTransform` (no extra propagation pass). HUD markers order
            // after `GunnerCameraPlaced` to reproject through this same pose.
            gunner_camera
                .run_if(in_gunner)
                .in_set(GameplaySet)
                .in_set(GunnerCameraPlaced)
                .after(TransformSystems::Propagate),
        );
}

/// Capture the turret's position in the tank root's local frame, once, after the rig binds.
fn capture_turret_pivot(
    mut pivot: ResMut<TurretPivot>,
    controlled: Query<(&GlobalTransform, &Rig), With<Controlled>>,
    turrets: Query<&GlobalTransform>,
) {
    if pivot.0.is_some() {
        return;
    }
    // Captured from the controlled tank's own turret. The Tigers are identical, so the offset holds
    // across a swap; a future asymmetric pair would recompute this per controlled tank.
    let Ok((tank_transform, rig)) = controlled.single() else {
        return;
    };
    let Ok(turret_transform) = turrets.get(rig.turret) else {
        return;
    };
    pivot.0 = Some(
        tank_transform
            .affine()
            .inverse()
            .transform_point3(turret_transform.translation()),
    );
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(10.0, 7.0, -7.0).looking_at(Vec3::new(10.0, 1.0, 5.0), Vec3::Y),
        OrbitCamera {
            zoom: 0.0,
            target_zoom: 0.0,
        },
        // The HUD reprojects world-anchored labels through this camera.
        HudCamera,
    ));
}

fn orbit_camera(
    camera: Single<(&mut Transform, &mut OrbitCamera, &mut Projection), With<Camera3d>>,
    spatial: SpatialQuery,
    tank: Query<&Transform, (With<Tank>, With<Controlled>, Without<Camera3d>)>,
    pivot: Res<TurretPivot>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    mouse_scroll: Res<AccumulatedMouseScroll>,
    follow: Res<CameraFollow>,
    time: Res<Time>,
) {
    // Detached (debug): leave the camera where it is so motion can be judged against a fixed view.
    if !follow.0 {
        return;
    }

    let (mut transform, mut orbit, mut projection) = camera.into_inner();

    // Restore the wide FOV when returning from the gunner optic (which narrows it).
    if let Projection::Perspective(p) = projection.as_mut() {
        p.fov = std::f32::consts::FRAC_PI_4;
    }
    let (Some(turret_local), Ok(tank_transform)) = (pivot.0, tank.single()) else {
        return;
    };

    // Free look: yaw/pitch read back from the current rotation, so no orientation state is
    // stored. Mouse delta is already per-frame — do NOT multiply by dt. Stop pitch just short
    // of vertical, where euler angles hit gimbal lock.
    const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.001;
    const YAW_SENSITIVITY: f32 = 0.004;
    const PITCH_SENSITIVITY: f32 = 0.003;
    let (yaw, pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
    let yaw = yaw - mouse_motion.delta.x * YAW_SENSITIVITY;
    let pitch = (pitch - mouse_motion.delta.y * PITCH_SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);

    // Zoom: scroll sets a target the actual zoom eases toward, so chunky (device-dependent)
    // scroll deltas become a smooth dolly. Both consts are feel knobs.
    const ZOOM_SPEED: f32 = 0.01;
    const ZOOM_GLIDE: f32 = 12.0;
    orbit.target_zoom = (orbit.target_zoom + mouse_scroll.delta.y * ZOOM_SPEED).clamp(0.0, 1.0);
    let ease = (ZOOM_GLIDE * time.delta_secs()).min(1.0);
    orbit.zoom += (orbit.target_zoom - orbit.zoom) * ease;

    // Orbit around the turret ring (root pose × captured offset), lifted a little. The camera sits
    // on the line through the pivot along its view axis; the ground ray pulls it in near terrain.
    const PIVOT_LIFT: f32 = 2.5;
    const ORBIT_FAR: f32 = 18.0;
    const ORBIT_NEAR: f32 = 5.0;
    let pivot_point = tank_transform.transform_point(turret_local) + Vec3::Y * PIVOT_LIFT;
    let distance = ORBIT_FAR + (ORBIT_NEAR - ORBIT_FAR) * orbit.zoom;
    let back_ray = Ray3d::new(pivot_point, -transform.forward());
    transform.translation = back_ray.get_point(ground_distance(&spatial, back_ray, distance));
}

/// Gunner optic (System B): lock the camera to the gun's line of sight. Parked at the **Gun node**
/// (the elevation pivot / mantlet) — the coaxial sight's natural home — and oriented along the
/// SIGHT LINE, the bore pitched DOWN by the current superelevation, so dialing range raises the
/// barrel without tilting the view off-target. The tank is hidden in gunner view (`Visibility` on
/// the root), so parking inside the mantlet clips no own geometry. The camera reads the gun's live
/// pose, so it lags the player's intent at the turret's slew rate (the WT "view follows the gun"
/// feel). Narrow FOV for magnification.
fn gunner_camera(
    camera: Single<(&mut Transform, &mut GlobalTransform, &mut Projection), With<Camera3d>>,
    controlled: Query<&Rig, With<Controlled>>,
    gun: Query<&GlobalTransform, (With<Gun>, Without<Camera3d>)>,
    ranging: Res<Ranging>,
) {
    let Ok(rig) = controlled.single() else {
        return;
    };
    let Ok(gun) = gun.get(rig.gun) else {
        return;
    };
    let (mut transform, mut global_transform, mut projection) = camera.into_inner();

    const GUNNER_FOV: f32 = 0.12; // ~7° vertical → ~6× magnification vs the 45° default

    if let Projection::Perspective(p) = projection.as_mut() {
        p.fov = GUNNER_FOV;
    }

    // The gun's propagated frame: bore = local −Z, hull-up = local +Y, right = local +X. The sight
    // line is the bore pitched DOWN by superelevation about right; up stays hull-up (not world up —
    // a hull-mounted sight rolls *with* the tank, so on a side-slope the view cants with it rather
    // than drifting off the bore). Pitching about `right` keeps (sight_dir, right, up) orthonormal,
    // so `look_to` is exact.
    let rot = gun.rotation();
    let bore = rot * Vec3::NEG_Z;
    let right = rot * Vec3::X;
    let up = rot * Vec3::Y;
    let sight_dir = Quat::from_axis_angle(right, -superelevation(ranging.range)) * bore;

    // Park at the pivot, look along the sight line. No bore-axis offset → no superelevation
    // parallax (the camera sits exactly on the sight line, not 0.6 m off it along the barrel).
    let pose = Transform::from_translation(gun.translation()).looking_to(sight_dir, up);

    // Write both: `Transform` for next frame's bookkeeping, `GlobalTransform` for *this* frame's
    // render and HUD reprojection (propagation already ran). The camera has no parent, so they match.
    *transform = pose;
    *global_transform = GlobalTransform::from(pose);
}
