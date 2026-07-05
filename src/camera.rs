//! Third-person orbit camera: free-aim look, scroll-to-zoom dolly, ground-collision pull-in.
//! The camera is also the aiming device, so look direction stays the player's — zoom only
//! changes the orbit radius, which slides along the view axis and never moves the aim point.

use avian3d::prelude::SpatialQuery;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;

use crate::firecontrol::{RangeTable, Ranging};
use crate::hud::HudCamera;
use crate::sight::{in_gunner, in_third_person};
use crate::spec::ViewKind;
use crate::state::GameplaySet;
use crate::tank::{Controlled, Gun, Rig, Tank, TankViews, rig_world_pose};
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

/// The turret-ring pivot as an offset in the tank root's local frame. The camera orbits
/// `root · this`, so it reads the body's interpolated root `Transform` rather than the turret's
/// (one-frame-stale) `GlobalTransform`. Computed once from the sim skeleton's local-transform
/// chain — spawn-complete data, available the first frame (`None` only before any tank exists).
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

/// Compute the turret's position in the tank root's local frame, once, from the sim skeleton's
/// local transforms (`rig_world_pose` with an identity root = the root-relative offset). The
/// chain's translations are static — the turret's own yaw doesn't move its pivot — so this is a
/// constant, derived from spawn-complete data rather than captured from a live `GlobalTransform`
/// (the lazy bind-time capture the sim/view split retired).
fn capture_turret_pivot(
    mut pivot: ResMut<TurretPivot>,
    controlled: Query<(Entity, &Rig), With<Controlled>>,
    parents: Query<&ChildOf>,
    locals: Query<&Transform>,
) {
    if pivot.0.is_some() {
        return;
    }
    // Computed from the controlled tank's own turret. The Tigers are identical, so the offset holds
    // across a swap; a future asymmetric pair would recompute this per controlled tank.
    let Ok((tank, rig)) = controlled.single() else {
        return;
    };
    let Some((position, _)) =
        rig_world_pose(rig.turret, tank, Vec3::ZERO, Quat::IDENTITY, &parents, &locals)
    else {
        return;
    };
    pivot.0 = Some(position);
}

/// The controlled tank's authored FOV for `kind`, or `fallback` before the rig binds.
fn view_fov(views: &Query<&TankViews, With<Controlled>>, kind: ViewKind, fallback: f32) -> f32 {
    views
        .single()
        .ok()
        .and_then(|v| v.0.get(&kind))
        .map(|config| config.fov)
        .unwrap_or(fallback)
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
    views: Query<&TankViews, With<Controlled>>,
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

    // Restore the wide commander-view FOV when returning from the gunner optic (which narrows it).
    if let Projection::Perspective(p) = projection.as_mut() {
        p.fov = view_fov(&views, ViewKind::Commander, std::f32::consts::FRAC_PI_4);
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
/// **sight line**, the bore depressed by the current superelevation: the aim commit lobs the gun up
/// by that angle for the dialed range, so depressing the view by the same holds the reticle on the
/// target while the barrel rides above it (dial range → barrel rises, view stays on target). The tank
/// is hidden in gunner view (`Visibility` on the root), so parking inside the mantlet clips no own
/// geometry. The camera reads the gun's live pose, so it lags the player's intent at the turret's slew
/// rate (the WT "view follows the gun" feel). Narrow FOV for magnification.
fn gunner_camera(
    camera: Single<(&mut Transform, &mut GlobalTransform, &mut Projection), With<Camera3d>>,
    controlled: Query<&Rig, With<Controlled>>,
    views: Query<&TankViews, With<Controlled>>,
    gun: Query<&GlobalTransform, (With<Gun>, Without<Camera3d>)>,
    ranging: Res<Ranging>,
    tables: Query<&RangeTable>,
) {
    let Ok(rig) = controlled.single() else {
        return;
    };
    let Ok(gun) = gun.get(rig.gun) else {
        return;
    };
    let (mut transform, mut global_transform, mut projection) = camera.into_inner();

    // The optic's magnification is the gunner view's authored FOV (Tiger ~0.12 rad ≈ 6× vs the 45°
    // commander view). Fallback covers the pre-bind frame before `TankViews` lands.
    if let Projection::Perspective(p) = projection.as_mut() {
        p.fov = view_fov(&views, ViewKind::Gunner, 0.12);
    }

    // The gun's propagated frame: bore = local −Z, right = local +X, hull-up = local +Y. The sight
    // line is the bore depressed by the superelevation about the gun's right axis — exactly undoing
    // the lob the aim commit applied, so the reticle holds the target while the barrel sits raised.
    // Pitching about `right` keeps (sight_dir, right, up) orthonormal; up stays hull-up (not world up
    // — a hull-mounted sight rolls *with* the tank rather than drifting off the bore on a side-slope).
    let theta = tables
        .get(rig.muzzle)
        .map_or(0.0, |table| table.superelevation(ranging.range));
    let rot = gun.rotation();
    let bore = rot * Vec3::NEG_Z;
    let right = rot * Vec3::X;
    let up = rot * Vec3::Y;
    let sight_dir = Quat::from_axis_angle(right, -theta) * bore;

    // Park at the pivot, look along the sight line.
    let pose = Transform::from_translation(gun.translation()).looking_to(sight_dir, up);

    // Write both: `Transform` for next frame's bookkeeping, `GlobalTransform` for *this* frame's
    // render and HUD reprojection (propagation already ran). The camera has no parent, so they match.
    *transform = pose;
    *global_transform = GlobalTransform::from(pose);
}
