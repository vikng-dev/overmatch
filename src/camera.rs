//! Third-person orbit camera: free-aim look, scroll-to-zoom dolly, ground-collision pull-in.
//! The camera is also the aiming device, so look direction stays the player's — zoom only
//! changes the orbit radius, which slides along the view axis and never moves the aim point.

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;

use crate::state::GameplaySet;
use crate::tank::{Tank, Turret};
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
            orbit_camera
                .in_set(GameplaySet)
                .before(TransformSystems::Propagate),
        );
}

/// Capture the turret's position in the tank root's local frame, once, after the rig binds.
fn capture_turret_pivot(
    mut pivot: ResMut<TurretPivot>,
    tank: Query<&GlobalTransform, With<Tank>>,
    turret: Query<&GlobalTransform, With<Turret>>,
) {
    if pivot.0.is_some() {
        return;
    }
    let (Ok(tank_transform), Ok(turret_transform)) = (tank.single(), turret.single()) else {
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
        OrbitCamera { zoom: 0.0, target_zoom: 0.0 },
    ));
}

fn orbit_camera(
    camera: Single<(&mut Transform, &mut OrbitCamera), With<Camera3d>>,
    tank: Query<&Transform, (With<Tank>, Without<Camera3d>)>,
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

    let (mut transform, mut orbit) = camera.into_inner();
    let (Some(turret_local), Ok(tank_transform)) = (pivot.0, tank.single()) else { return; };

    // Free look: yaw/pitch read back from the current rotation, so no orientation state is
    // stored. Mouse delta is already per-frame — do NOT multiply by dt. Stop pitch just short
    // of vertical, where euler angles hit gimbal lock.
    const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.001;
    let (yaw, pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
    let yaw = yaw - mouse_motion.delta.x * 0.004;
    let pitch = (pitch - mouse_motion.delta.y * 0.003).clamp(-PITCH_LIMIT, PITCH_LIMIT);
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
    let pivot_point = tank_transform.transform_point(turret_local) + Vec3::Y * 2.5;
    let distance = 18.0 + (5.0 - 18.0) * orbit.zoom;
    let back_ray = Ray3d::new(pivot_point, -transform.forward());
    transform.translation = back_ray.get_point(ground_distance(back_ray, distance));
}
