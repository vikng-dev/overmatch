//! Third-person orbit camera: free-aim look, scroll-to-zoom dolly, ground-collision pull-in.
//! The camera is also the aiming device, so look direction stays the player's — zoom only
//! changes the orbit radius, which slides along the view axis and never moves the aim point.

use avian3d::prelude::{PhysicsSystems, SpatialQuery};
use bevy::camera::Hdr;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;

use crate::aim::CommittedAim;
use crate::firecontrol::{RangeTable, Ranging};
use crate::hud::HudCamera;
use crate::sight::{
    ElasticCam, FREE_RETICLE_FOV, GunnerFreeAim, GunnerScheme, SightMode, SightToggled,
    hull_local_dir, in_gunner_bound, in_gunner_elastic, in_gunner_free_look, in_gunner_lead,
    in_third_person, yaw_pitch_of,
};
use crate::spec::ViewKind;
use crate::state::{GameplaySet, PlayerInputSet};
use crate::tank::{
    Controlled, Hull, Rig, Tank, TankViews, ViewNode, rig_world_pose, shortest_angle,
};
use crate::world::ground_distance;

/// Zoom state on the camera entity. Scroll sets `target_zoom`; `zoom` eases toward it for a
/// smooth dolly. 0 = out (far), 1 = in (near).
#[derive(Component)]
struct OrbitCamera {
    zoom: f32,
    target_zoom: f32,
}

/// Debug switch that freezes the orbit camera's follow transform.
#[derive(Resource)]
pub struct CameraFollow(pub bool);

/// Marks the gunner camera placement, which runs *after* transform propagation (it bolts the camera
/// to the gun's live pose and writes its `GlobalTransform` directly). HUD reprojection orders after
/// this set so markers and the rendered view share one consistent, current camera pose.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GunnerCameraPlaced;

/// The view-layer hit-kick's ordering anchor. `net::hit_feel::apply_camera_kick` displaces the
/// camera's rendered `GlobalTransform` by a decaying recoil offset when the player is hit, and it must
/// run AFTER both camera placements have set that pose ([`orbit_camera`] before `Propagate`,
/// [`gunner_camera`] after it in [`GunnerCameraPlaced`]) — so the kick anchors `.after(GunnerCameraPlaced)`.
/// Systems that reproject through the camera (the gunner reticles in `sight`) order `.after` this set
/// so they ride the kicked pose and the whole sight picture jolts as one. **Empty in single-player and
/// on a headless client** (the kick is net-client-only, mounted by `NetClientPlugin`), where every
/// `.after(CameraKickApplied)` edge is a harmless no-op — the same vacuous-anchor idiom as
/// [`OrbitCameraSet`].
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CameraKickApplied;

/// The third-person orbit camera's system set — an ordering anchor. The MP render-error layer
/// (`net::render_error`) offsets the predicted root's `Transform` between `PhysicsSystems::Writeback`
/// and `TransformSystems::Propagate`; ordering it `.before(OrbitCameraSet)` there makes the camera
/// orbit the offset (rendered) pose rather than the pre-offset one, so the whole view moves as one.
/// A no-op edge in SP (the layer is net-gated) and on a headless client (no camera to place).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OrbitCameraSet;

/// The turret-ring pivot as an offset in the tank root's local frame. The camera orbits
/// `root · this`, so it reads the body's interpolated root `Transform` rather than the turret's
/// (one-frame-stale) `GlobalTransform`. Computed once from the sim skeleton's local-transform
/// chain — spawn-complete data, available the first frame (`None` only before any tank exists).
#[derive(Resource, Default)]
struct TurretPivot(Option<Vec3>);

/// Height of the orbit pivot above the turret ring.
const PIVOT_LIFT: f32 = 2.5;

/// The orbit pivot in world space: the turret ring (root pose × captured offset), lifted a little.
/// THE point the camera body is placed from ([`orbit_camera`]) and the optic-exit re-aim aims from
/// ([`reaim_orbit_on_optic_exit`]) — one formula, because the re-aim's collinearity guarantee
/// (pivot, camera body, committed point on one line) holds only if it reconstructs exactly the
/// pivot the body placement will use.
fn orbit_pivot(tank_transform: &Transform, turret_local: Vec3) -> Vec3 {
    tank_transform.transform_point(turret_local) + Vec3::Y * PIVOT_LIFT
}

pub fn plugin(app: &mut App) {
    app.insert_resource(CameraFollow(true))
        .init_resource::<TurretPivot>()
        .add_systems(Startup, spawn_camera)
        .add_systems(Update, capture_turret_pivot)
        .add_systems(
            PostUpdate,
            // Ordering invariant: read the writeback pose, then let propagation derive camera and
            // tank globals from that same frame's transforms.
            orbit_camera
                .run_if(in_third_person)
                .in_set(GameplaySet)
                .in_set(OrbitCameraSet)
                .after(PhysicsSystems::Writeback)
                .before(TransformSystems::Propagate),
        )
        .add_systems(
            PostUpdate,
            // Input rotation is gated separately; placement must consume this frame's rotation.
            orbit_look
                .run_if(in_third_person)
                .in_set(GameplaySet)
                .in_set(PlayerInputSet)
                .before(orbit_camera)
                .before(TransformSystems::Propagate),
        )
        .add_systems(
            PostUpdate,
            // The three gunner-view cameras (A/B harness), one per scheme family, gated by mutually
            // exclusive run conditions so exactly one places the camera. Each bolts to the gun's
            // *propagated* pose (or the mount) after propagation and writes its own `GlobalTransform`
            // (no extra propagation pass). HUD markers order after `GunnerCameraPlaced` to reproject
            // through this same pose.
            //   A  — `gunner_camera`: rigid bolt to the gun sight line.
            //   B/C — `free_aim_camera`: mouse-driven look at the mount, gun trails.
            //   D  — `elastic_bore_camera`: elastic spring toward the aim intent.
            //   E  — `lead_optic_camera`: locked to the intent/orange dot, gun lags behind centre.
            (
                gunner_camera.run_if(in_gunner_bound),
                free_aim_camera.run_if(in_gunner_free_look),
                elastic_bore_camera.run_if(in_gunner_elastic),
                lead_optic_camera.run_if(in_gunner_lead),
            )
                .in_set(GameplaySet)
                .in_set(GunnerCameraPlaced)
                .after(TransformSystems::Propagate),
        )
        .add_systems(
            Update,
            // React to leaving the optic by re-aiming the orbit camera at the committed point.
            // `.after(SightToggled)` so the flip is consumed the SAME frame; the mode filter runs
            // inside (a change to Gunner must not fire it a frame — or a session — later).
            reaim_orbit_on_optic_exit
                .run_if(resource_changed::<SightMode>)
                .after(SightToggled)
                .in_set(GameplaySet),
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
    let Some((position, _)) = rig_world_pose(
        rig.turret,
        tank,
        Vec3::ZERO,
        Quat::IDENTITY,
        &parents,
        &locals,
    ) else {
        return;
    };
    pivot.0 = Some(position);
}

/// The gunner optic's fallback vertical FOV (radians) for the pre-bind frame before `TankViews`
/// lands — mirrors the Tiger's authored `0.12`. Shared (rather than a bare literal at each call
/// site) so the camera's magnification and the sight's cursor-travel margin agree on the same
/// pre-bind value.
pub const GUNNER_FOV_FALLBACK: f32 = 0.12;

/// The controlled tank's authored FOV for `kind`, or `fallback` before the rig binds.
pub fn view_fov(views: &Query<&TankViews, With<Controlled>>, kind: ViewKind, fallback: f32) -> f32 {
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
        // Tracer emissive values require HDR and bloom to produce the intended highlight.
        Hdr,
        Bloom::NATURAL,
        Transform::from_xyz(10.0, 7.0, -7.0).looking_at(Vec3::new(10.0, 1.0, 5.0), Vec3::Y),
        OrbitCamera {
            zoom: 0.0,
            target_zoom: 0.0,
        },
        // The HUD reprojects world-anchored labels through this camera.
        HudCamera,
    ));
}

/// Free look: turn the camera from this frame's mouse delta. The one device-reading half of the
/// orbit camera, split out so it hangs on `PlayerInputSet` — with the cursor released the orbit
/// freezes while `orbit_camera` keeps the body following the tank.
fn orbit_look(
    camera: Single<&mut Transform, With<Camera3d>>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    follow: Res<CameraFollow>,
    pivot: Res<TurretPivot>,
    tank: Query<(), (With<Tank>, With<Controlled>)>,
) {
    // Detached (debug): leave the camera where it is so motion can be judged against a fixed view.
    if !follow.0 {
        return;
    }
    // Freeze mouse-look when there is no controlled tank/pivot to orbit — the tankless death→respawn
    // gap, or CONNECTING. Without a body to re-anchor to, `orbit_camera` can't reposition the camera,
    // so the locked cursor's motion here would re-point it at nothing and the player would respawn
    // facing a random direction. This is the guard the wave-4 orbit split (62da9bd) dropped: it
    // mirrors the `(pivot.0, tank.single())` check `orbit_camera` uses to place the body, so ONLY the
    // mouse-delta rotation gates — `orbit_camera`'s follow half keeps tracking the tank behind the menu.
    if pivot.0.is_none() || tank.is_empty() {
        return;
    }
    let mut transform = camera.into_inner();
    // Free look: yaw/pitch read back from the current rotation, so no orientation state is stored.
    // Mouse delta is already per-frame — do NOT multiply by dt. Stop pitch just short of vertical,
    // where euler angles hit gimbal lock.
    const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.001;
    const YAW_SENSITIVITY: f32 = 0.004;
    const PITCH_SENSITIVITY: f32 = 0.003;
    let (yaw, pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
    let yaw = yaw - mouse_motion.delta.x * YAW_SENSITIVITY;
    let pitch = (pitch - mouse_motion.delta.y * PITCH_SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
}

fn orbit_camera(
    camera: Single<(&mut Transform, &mut OrbitCamera, &mut Projection), With<Camera3d>>,
    spatial: SpatialQuery,
    tank: Query<&Transform, (With<Tank>, With<Controlled>, Without<Camera3d>)>,
    views: Query<&TankViews, With<Controlled>>,
    pivot: Res<TurretPivot>,
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

    // The camera's rotation is set by `orbit_look` (the device-reading half, gated on the cursor and
    // ordered `.before` this) — here we only read it to place the body, so the orbit stays frozen
    // behind the menu while the follow keeps tracking the tank.

    // Zoom: scroll sets a target the actual zoom eases toward, so chunky (device-dependent)
    // scroll deltas become a smooth dolly. Both consts are feel knobs.
    const ZOOM_SPEED: f32 = 0.01;
    const ZOOM_GLIDE: f32 = 12.0;
    orbit.target_zoom = (orbit.target_zoom + mouse_scroll.delta.y * ZOOM_SPEED).clamp(0.0, 1.0);
    // Exponential easing makes the dolly's response independent of frame time.
    let ease = 1.0 - (-ZOOM_GLIDE * time.delta_secs()).exp();
    orbit.zoom += (orbit.target_zoom - orbit.zoom) * ease;

    // Orbit around the shared pivot (`orbit_pivot` — the re-aim reconstructs this same point). The
    // camera sits on the line through the pivot along its view axis; the ground ray pulls it in
    // near terrain.
    const ORBIT_FAR: f32 = 18.0;
    const ORBIT_NEAR: f32 = 5.0;
    let pivot_point = orbit_pivot(tank_transform, turret_local);
    let distance = ORBIT_FAR + (ORBIT_NEAR - ORBIT_FAR) * orbit.zoom;
    let back_ray = Ray3d::new(pivot_point, -transform.forward());
    transform.translation = back_ray.get_point(ground_distance(&spatial, back_ray, distance));
}

/// Re-aim the orbit camera through the committed point when leaving the gunner view.
///
/// Invariant: the orbit pivot, camera body, and committed point remain collinear. A fresh or newly
/// possessed tank has no keyed [`CommittedAim`], so the transition is a no-op.
fn reaim_orbit_on_optic_exit(
    mode: Res<SightMode>,
    committed: Res<CommittedAim>,
    controlled: Query<(Entity, &Rig), With<Controlled>>,
    tank: Query<&Transform, (With<Tank>, With<Controlled>, Without<Camera3d>)>,
    hull: Query<&GlobalTransform, With<Hull>>,
    pivot: Res<TurretPivot>,
    camera: Single<&mut Transform, With<Camera3d>>,
) {
    // Only the exit direction re-aims; entering the optic needs nothing (`gunner_camera` owns the
    // pose outright while in it).
    if *mode != SightMode::ThirdPerson {
        return;
    }
    let Ok((tank_entity, rig)) = controlled.single() else {
        return;
    };
    let Some(local) = committed.get(tank_entity) else {
        return;
    };
    let (Some(turret_local), Ok(tank_transform)) = (pivot.0, tank.single()) else {
        return;
    };
    let Ok(hull_transform) = hull.get(rig.hull) else {
        return;
    };

    // The target uses the propagated hull pose while the pivot uses the current rendered root pose;
    // this view-only transition can therefore span one render frame while moving.
    let target = hull_transform.affine().transform_point3(local);
    let pivot_point = orbit_pivot(tank_transform, turret_local);
    // Fallible: a zero/non-finite span (a poisoned pose on the toggle frame) must not NaN the
    // camera rotation — keep the current direction instead.
    let Ok(direction) = Dir3::new(target - pivot_point) else {
        return;
    };
    camera.into_inner().look_to(direction, Vec3::Y);
}

/// Gunner optic (System B): lock the camera to the gun's line of sight. Parked at the **Gun node**
/// (the elevation pivot / mantlet) — the coaxial sight's natural home — and oriented along the
/// **sight line**, the bore depressed by the current superelevation: the aim commit lobs the gun up
/// by that angle for the dialed range, so depressing the view by the same holds the reticle on the
/// target while the barrel rides above it (dial range → barrel rises, view stays on target). The
/// controlled tank's meshes are moved off this camera's render layer in gunner view (`sight`'s
/// `reconcile_optic_render_layers`), so parking inside the mantlet clips no own geometry. The camera
/// reads the gun's live pose, so it lags the player's intent at the turret's slew
/// rate (the WT "view follows the gun" feel). Narrow FOV for magnification.
fn gunner_camera(
    camera: Single<(&mut Transform, &mut GlobalTransform, &mut Projection), With<Camera3d>>,
    controlled: Query<&Rig, With<Controlled>>,
    views: Query<&TankViews, With<Controlled>>,
    view_nodes: Query<&ViewNode>,
    gun: Query<&GlobalTransform, Without<Camera3d>>,
    ranging: Res<Ranging>,
    tables: Query<&RangeTable>,
) {
    let Ok(rig) = controlled.single() else {
        return;
    };
    // The VIEW gun (design §6C): the optic must ride the render-smoothed pose — the sim gun's
    // chain steps at tick rate since the sim/view split.
    let Ok(gun) = gun.get(ViewNode::resolve(view_nodes.get(rig.gun).ok(), rig.gun)) else {
        return;
    };
    let (mut transform, mut global_transform, mut projection) = camera.into_inner();

    // The optic's magnification is the gunner view's authored FOV (Tiger ~0.12 rad ≈ 6× vs the 45°
    // commander view). Fallback covers the pre-bind frame before `TankViews` lands.
    if let Projection::Perspective(p) = projection.as_mut() {
        p.fov = view_fov(&views, ViewKind::Gunner, GUNNER_FOV_FALLBACK);
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

/// Park the (parentless) camera at `eye` looking along world `dir`, writing both `Transform` (for next
/// frame's bookkeeping) and `GlobalTransform` (for *this* frame's render + HUD reprojection —
/// propagation already ran), and setting the perspective FOV. Mirrors [`gunner_camera`]'s direct-write
/// and is shared by the free-look (B/C) and elastic (D) gunner cameras. A non-finite/zero `dir` is a
/// no-op that keeps the last good pose (a poisoned pose frame must not NaN the camera).
fn place_optic_camera(
    transform: &mut Transform,
    global_transform: &mut GlobalTransform,
    projection: &mut Projection,
    eye: Vec3,
    dir: Vec3,
    up: Vec3,
    fov: f32,
) {
    let Ok(dir) = Dir3::new(dir) else {
        return;
    };
    if let Projection::Perspective(p) = projection {
        p.fov = fov;
    }
    let pose = Transform::from_translation(eye).looking_to(dir, up);
    *transform = pose;
    *global_transform = GlobalTransform::from(pose);
}

/// Free-look gunner camera (schemes B and C): park at the gun mount and look along the camera's own
/// aim ([`GunnerFreeAim`], driven by the mouse in `sight::drive_free_aim`) — NOT bolted to the gun.
/// The gun trails the look, so the barrel bore drifts off-centre (the `aim` bore dot shows it). B is a
/// wide "camera dictates intent" view; C is the magnified optic whose look damps toward the mouse.
fn free_aim_camera(
    camera: Single<(&mut Transform, &mut GlobalTransform, &mut Projection), With<Camera3d>>,
    controlled: Query<&Rig, With<Controlled>>,
    views: Query<&TankViews, With<Controlled>>,
    view_nodes: Query<&ViewNode>,
    gun: Query<&GlobalTransform, Without<Camera3d>>,
    hull: Query<&GlobalTransform, (With<Hull>, Without<Camera3d>)>,
    scheme: Res<GunnerScheme>,
    free: Res<GunnerFreeAim>,
) {
    // Until the commit reseeds the look on entry (`sight::drive_free_aim`, next `BeforeFixedMainLoop`),
    // hold the previous pose rather than snap to a default-zero look for one frame.
    if !free.seeded {
        return;
    }
    let Ok(rig) = controlled.single() else {
        return;
    };
    let Ok(gun) = gun.get(ViewNode::resolve(view_nodes.get(rig.gun).ok(), rig.gun)) else {
        return;
    };
    let Ok(hull) = hull.get(rig.hull) else {
        return;
    };
    let (mut transform, mut global_transform, mut projection) = camera.into_inner();

    // B is a wide gunnery view; C the authored (magnified) optic FOV — matched to the sensitivity in
    // `drive_free_aim` so the screen feel agrees.
    let fov = if *scheme == GunnerScheme::FreeReticle {
        FREE_RETICLE_FOV
    } else {
        view_fov(&views, ViewKind::Gunner, GUNNER_FOV_FALLBACK)
    };

    // The look is hull-local (it rides the tank, like the optic); up stays hull-up so the horizon
    // rolls *with* the tank on a side-slope rather than drifting off the bore.
    let hull_rot = hull.rotation();
    let look = hull_rot * hull_local_dir(free.yaw, free.pitch);
    let up = hull_rot * Vec3::Y;
    place_optic_camera(
        &mut transform,
        &mut global_transform,
        &mut projection,
        gun.translation(),
        look,
        up,
        fov,
    );
}

/// Elastic-bore feel knobs. `FREQ` is the oscillator's natural angular frequency (rad/s — higher =
/// stiffer, faster settle); `DAMPING` is its ratio (< 1 = underdamped, so the view overshoots the
/// intent and settles — the whole point of the scheme). Tuned in playtest.
const ELASTIC_FREQ: f32 = 11.0;
const ELASTIC_DAMPING: f32 = 0.62;

/// One semi-implicit Euler step of a damped harmonic oscillator dragging `pos` (velocity `vel`) toward
/// `target`. `wrap` uses shortest-angle error for the continuous-traverse yaw axis. dt is clamped so a
/// frame hitch can't explode the spring.
fn integrate_spring(pos: &mut f32, vel: &mut f32, target: f32, wrap: bool, dt: f32) {
    let dt = dt.min(1.0 / 30.0);
    let error = if wrap {
        shortest_angle(*pos - target)
    } else {
        *pos - target
    };
    let accel = -2.0 * ELASTIC_DAMPING * ELASTIC_FREQ * *vel - ELASTIC_FREQ * ELASTIC_FREQ * error;
    *vel += accel * dt;
    *pos += *vel * dt;
}

/// Elastic-bore gunner camera (scheme D): park at the gun mount and look along a spring that chases the
/// committed-aim (intent) direction as an underdamped oscillator — the view whips ahead of the trailing
/// gun and settles, giving the *camera* its own mass. Aiming is unchanged from scheme A
/// (`sight::drive_gunner_aim` still owns the commit); this only changes how the camera rides.
fn elastic_bore_camera(
    camera: Single<(&mut Transform, &mut GlobalTransform, &mut Projection), With<Camera3d>>,
    controlled: Query<(Entity, &Rig), With<Controlled>>,
    views: Query<&TankViews, With<Controlled>>,
    view_nodes: Query<&ViewNode>,
    gun: Query<&GlobalTransform, Without<Camera3d>>,
    hull: Query<&GlobalTransform, (With<Hull>, Without<Camera3d>)>,
    committed: Res<CommittedAim>,
    mut elastic: ResMut<ElasticCam>,
    time: Res<Time>,
) {
    let Ok((tank, rig)) = controlled.single() else {
        return;
    };
    let Ok(gun) = gun.get(ViewNode::resolve(view_nodes.get(rig.gun).ok(), rig.gun)) else {
        return;
    };
    let Ok(hull) = hull.get(rig.hull) else {
        return;
    };
    let (mut transform, mut global_transform, mut projection) = camera.into_inner();

    let hull_affine = hull.affine();
    let eye = gun.translation();
    let mount_local = hull_affine.inverse().transform_point3(eye);

    // Target look = the committed-aim (intent) bearing from the mount, hull-local; fall back to the
    // gun's own bore before the first commit exists.
    let target = match committed.get(tank).filter(|point| point.is_finite()) {
        Some(point) => yaw_pitch_of(point - mount_local),
        None => yaw_pitch_of(
            hull_affine
                .inverse()
                .transform_vector3(gun.rotation() * Vec3::NEG_Z),
        ),
    };

    if !elastic.seeded {
        // Seed on entry so the view continues from the current aim (no spring snap).
        elastic.yaw = target.0;
        elastic.pitch = target.1;
        elastic.vel_yaw = 0.0;
        elastic.vel_pitch = 0.0;
        elastic.seeded = true;
    } else {
        let dt = time.delta_secs();
        // Reborrow the inner struct once so the two axes are disjoint field borrows — `&mut elastic.x`
        // twice in one call would each deref the `ResMut` and conflict.
        let e = &mut *elastic;
        integrate_spring(&mut e.yaw, &mut e.vel_yaw, target.0, true, dt);
        integrate_spring(&mut e.pitch, &mut e.vel_pitch, target.1, false, dt);
    }

    let hull_rot = hull.rotation();
    let look = hull_rot * hull_local_dir(elastic.yaw, elastic.pitch);
    let up = hull_rot * Vec3::Y;
    let fov = view_fov(&views, ViewKind::Gunner, GUNNER_FOV_FALLBACK);
    place_optic_camera(
        &mut transform,
        &mut global_transform,
        &mut projection,
        eye,
        look,
        up,
        fov,
    );
}

/// Lead-optic gunner camera (scheme E): park at the gun mount and look straight at the committed-aim
/// (intent / orange-dot) point — the camera locks to where you are commanding, so the orange cursor
/// sits at screen centre and the gun bore (green) lags *behind* it, catching up within the bounded
/// circle. Stateless and instant (the no-spring sibling of [`elastic_bore_camera`]); aiming is
/// unchanged from scheme A (`sight::drive_gunner_aim` owns the commit).
fn lead_optic_camera(
    camera: Single<(&mut Transform, &mut GlobalTransform, &mut Projection), With<Camera3d>>,
    controlled: Query<(Entity, &Rig), With<Controlled>>,
    views: Query<&TankViews, With<Controlled>>,
    view_nodes: Query<&ViewNode>,
    gun: Query<&GlobalTransform, Without<Camera3d>>,
    hull: Query<&GlobalTransform, (With<Hull>, Without<Camera3d>)>,
    committed: Res<CommittedAim>,
) {
    let Ok((tank, rig)) = controlled.single() else {
        return;
    };
    let Ok(gun) = gun.get(ViewNode::resolve(view_nodes.get(rig.gun).ok(), rig.gun)) else {
        return;
    };
    let Ok(hull) = hull.get(rig.hull) else {
        return;
    };
    let (mut transform, mut global_transform, mut projection) = camera.into_inner();

    let eye = gun.translation();
    // Look at the committed intent point (the orange dot), hull-local → world; before the first commit
    // exists, fall back to the gun's own bore.
    let dir = match committed.get(tank).filter(|point| point.is_finite()) {
        Some(point) => hull.affine().transform_point3(point) - eye,
        None => gun.rotation() * Vec3::NEG_Z,
    };
    let up = hull.rotation() * Vec3::Y;
    let fov = view_fov(&views, ViewKind::Gunner, GUNNER_FOV_FALLBACK);
    place_optic_camera(
        &mut transform,
        &mut global_transform,
        &mut projection,
        eye,
        dir,
        up,
        fov,
    );
}
