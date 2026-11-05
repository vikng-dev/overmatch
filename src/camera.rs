//! The game's damage-model view: camera transform kernels.
//!
//! The game's third-person orbit camera provides free-aim look, scroll-to-zoom dolly, and
//! ground-collision pull-in. The camera is also the aiming device, so look direction stays the
//! player's — zoom only changes the orbit radius, which slides along the view axis and never moves
//! the aim point. [`free_fly_transform`] is the shared pure transform kernel behind the dev
//! sandboxes' distinct ECS adapters.
//!
//! # Who writes the player's camera, and when (`PostUpdate`)
//!
//! One writer of the [`PlayerView`] camera's pose, lens and profile at a time:
//!
//! * the player's own: [`orbit_look`] then `orbit_camera` ([`OrbitCameraSet`]) in third person,
//!   `gunner_camera` ([`GunnerCameraPlaced`]) in the optic;
//! * a captor, while [`CameraCaptive`] holds (the kill cam's lesson and killer beats): its writer
//!   runs in [`CaptiveCameraPlaced`] and all three of the player's stand aside.
//!
//! Every writer's edges, which anything copying the camera (the pen cam's mask cameras) relies on:
//!
//! * a pose written as `Transform` runs after `PhysicsSystems::Writeback` (it reads the tank's
//!   pose) and before `TransformSystems::Propagate`; the gunner's, written as `GlobalTransform`
//!   after propagation, runs before `VisibilitySystems::UpdateFrusta`;
//! * every lens (`Projection`) writer runs before `CameraUpdateSystems`, where bevy derives the
//!   projection matrices;
//! * a profile ([`CameraProfile`](crate::render_policy::CameraProfile)) is resolved into
//!   `RenderLayers` after [`CaptiveCameraPlaced`] (`render_policy`), so a captor's profile draws on
//!   the frame it is set.
//!
//! The copiers order after all three sets: `look::follow_lens` before `CameraUpdateSystems`,
//! `look::follow_pose` before `UpdateFrusta`.

use avian3d::prelude::{PhysicsSystems, SpatialQuery};
use bevy::camera::visibility::VisibilitySystems;
use bevy::camera::{CameraUpdateSystems, Hdr};
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::math::Affine3A;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;

use crate::aim::CommittedAim;
use crate::firecontrol::{RangeTable, Ranging};
use crate::sight::{
    SightMode, hull_local_dir, in_gunner, in_third_person, sight_line, yaw_pitch_of,
};
use crate::spec::ViewKind;
use crate::state::{GameplaySet, PlayerInputSet};
use crate::tank::{
    Controlled, Hull, Rig, Tank, TankViews, ViewNode, rig_world_pose, shortest_angle,
};
use crate::view::PlayerView;
use crate::world::ground_distance;

include!(concat!(env!("OUT_DIR"), "/camera_authored.rs"));

/// Resolution: stay clear of the Euler singularity so yaw retains a defined horizontal axis.
const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.001;

/// Apply one free-fly input frame to `transform`.
///
/// Mouse delta controls yaw/pitch without time scaling. WASD moves on the horizontal heading plane,
/// Shift/Ctrl changes altitude, and translation integrates on real-time `delta_secs`.
#[cfg(feature = "dev_ui")]
pub(crate) fn free_fly_transform(
    transform: &mut Transform,
    keys: &ButtonInput<KeyCode>,
    mouse_delta: Vec2,
    delta_secs: f32,
) {
    let (mut yaw, mut pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
    yaw -= mouse_delta.x * FREE_FLY_SENSITIVITY;
    pitch = (pitch - mouse_delta.y * FREE_FLY_SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);

    // Looking down and pressing W keeps moving forward over the ground, not into it. Near-vertical
    // look leaves no horizontal heading, so `normalize_or_zero` just no-ops that axis.
    let forward = Vec3::from(transform.forward())
        .with_y(0.0)
        .normalize_or_zero();
    let right = Vec3::from(transform.right())
        .with_y(0.0)
        .normalize_or_zero();
    let mut dir = Vec3::ZERO;
    if keys.pressed(KeyCode::KeyW) {
        dir += forward;
    }
    if keys.pressed(KeyCode::KeyS) {
        dir -= forward;
    }
    if keys.pressed(KeyCode::KeyD) {
        dir += right;
    }
    if keys.pressed(KeyCode::KeyA) {
        dir -= right;
    }
    if keys.pressed(KeyCode::ShiftLeft) {
        dir += Vec3::Y;
    }
    if keys.pressed(KeyCode::ControlLeft) {
        dir -= Vec3::Y;
    }
    if dir != Vec3::ZERO {
        transform.translation += dir.normalize() * FREE_FLY_SPEED * delta_secs;
    }
}

/// Zoom state on the camera entity. Scroll sets `target_zoom`; `zoom` eases toward it for a
/// smooth dolly. 0 = out (far), 1 = in (near).
#[derive(Component)]
pub(crate) struct OrbitCamera {
    zoom: f32,
    target_zoom: f32,
}

impl OrbitCamera {
    /// The orbit radius the player's zoom sets: the distance `orbit_camera` places the body from
    /// [`orbit_pivot`] before the ground pulls it in. Zoom stays in 0..=1; required near <= far
    /// makes `ORBIT_FAR` an upper bound, including when the camera sits toward another tank.
    pub(crate) fn radius(&self) -> f32 {
        ORBIT_FAR + (ORBIT_NEAR - ORBIT_FAR) * self.zoom
    }
}

/// Whether a captor holds the player's camera: while it does, the orbit look, the orbit placement
/// and the gunner placement do not run, and the captor's writer in [`CaptiveCameraPlaced`] is the
/// camera's one writer of pose, lens and profile. A captor that lets go leaves the camera as it
/// placed it, and the player's writers resume from that pose.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct CameraCaptive(pub(crate) bool);

/// Whether the player's own writers own the camera ([`CameraCaptive`]).
fn camera_free(captive: Option<Res<CameraCaptive>>) -> bool {
    captive.is_none_or(|captive| !captive.0)
}

/// A captor's placement of the player's camera: after the physics writeback, before transform
/// propagation and before bevy derives the projections ([`CameraCaptive`]).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CaptiveCameraPlaced;

/// Marks the gunner camera placement, which runs *after* transform propagation (it bolts the camera
/// to the gun's live pose and writes its `GlobalTransform` directly). HUD reprojection orders after
/// this set so markers and the rendered view share one consistent, current camera pose.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GunnerCameraPlaced;

/// The third-person orbit camera's system set — an ordering anchor for anything that must read or
/// follow the placed orbit pose. A no-op edge in SP (net layers are net-gated) and on a headless
/// client (no camera to place).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OrbitCameraSet;

/// The turret-ring pivot as an offset in the tank root's local frame. The camera orbits
/// `root · this`, so it reads the body's interpolated root `Transform` rather than the turret's
/// (one-frame-stale) `GlobalTransform`. Computed once from the sim skeleton's local-transform
/// chain for the current controlled entity. Possession changes invalidate the cache; `None`
/// names the gap before spawn or between possessions.
#[derive(Resource, Default)]
pub(crate) struct TurretPivot(Option<(Entity, Vec3)>);

impl TurretPivot {
    /// The offset, once a controlled tank has supplied it.
    pub(crate) fn offset(&self) -> Option<Vec3> {
        self.0.map(|(_, offset)| offset)
    }

    /// A pivot at `offset`, for a test without a tank's skeleton.
    #[cfg(test)]
    pub(crate) fn at(offset: Vec3) -> Self {
        Self(Some((Entity::PLACEHOLDER, offset)))
    }
}

/// The orbit pivot in world space: the turret ring (root pose × captured offset), lifted a little.
/// THE point the camera body is placed from ([`orbit_camera`]) and the optic-exit re-aim aims from
/// ([`reaim_orbit_on_optic_exit`]) — one formula, because the re-aim's collinearity guarantee
/// (pivot, camera body, committed point on one line) holds only if it reconstructs exactly the
/// pivot the body placement will use.
pub(crate) fn orbit_pivot(tank_transform: &Transform, turret_local: Vec3) -> Vec3 {
    tank_transform.transform_point(turret_local) + Vec3::Y * PIVOT_LIFT
}

pub fn plugin(app: &mut App) {
    // The owner of the camera's `MainPassDepth`, mounted by the module that spawns it.
    app.add_plugins(crate::scene_depth::plugin);
    app.init_resource::<TurretPivot>()
        .init_resource::<CameraCaptive>()
        .configure_sets(
            PostUpdate,
            CaptiveCameraPlaced
                .after(PhysicsSystems::Writeback)
                .before(TransformSystems::Propagate)
                .before(CameraUpdateSystems),
        )
        .add_systems(Startup, spawn_camera)
        .add_systems(
            PostUpdate,
            // Ordering invariant: read the writeback pose, then let propagation derive camera and
            // tank globals from that same frame's transforms.
            // It writes the lens too, so it precedes bevy's projection update.
            orbit_camera
                .run_if(in_third_person)
                .run_if(camera_free)
                .in_set(GameplaySet)
                .in_set(OrbitCameraSet)
                .after(PhysicsSystems::Writeback)
                .before(TransformSystems::Propagate)
                .before(CameraUpdateSystems),
        )
        .add_systems(
            PostUpdate,
            // Input rotation is gated separately; placement must consume this frame's rotation.
            orbit_look
                .run_if(in_third_person)
                .run_if(camera_free)
                .in_set(GameplaySet)
                .in_set(PlayerInputSet)
                .before(orbit_camera)
                .before(TransformSystems::Propagate),
        )
        .add_systems(
            PostUpdate,
            // The one gunner-view camera. It reads the gun's *propagated* pose after propagation and
            // writes its own `GlobalTransform` (no extra propagation pass); HUD markers order after
            // `GunnerCameraPlaced` to reproject through this same pose. Its final pose precedes the
            // frusta and its lens the projection update.
            gunner_camera
                .run_if(in_gunner)
                .run_if(camera_free)
                .in_set(GameplaySet)
                .in_set(GunnerCameraPlaced)
                .after(TransformSystems::Propagate)
                .before(CameraUpdateSystems)
                .before(VisibilitySystems::UpdateFrusta),
        );
    mount_pivot_tracking(app);
}

/// All Update handoffs are applied before this chain reads ownership. The exit re-aim consumes
/// the new pivot, then orbit input/placement and any captive camera consume that same frame.
fn mount_pivot_tracking(app: &mut App) {
    app.add_systems(
        PostUpdate,
        (
            capture_turret_pivot,
            reaim_orbit_on_optic_exit
                .run_if(resource_changed::<SightMode>)
                .in_set(GameplaySet),
        )
            .chain()
            .before(orbit_look)
            .before(orbit_camera)
            .before(CaptiveCameraPlaced),
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
    let Some((tank, rig)) = crate::tank::controlled_one(controlled.single()) else {
        pivot.0 = None; // Before spawn or between possessions there is no camera anchor.
        return;
    };
    if pivot.0.is_some_and(|(owner, _)| owner == tank) {
        return;
    }
    let (position, _) = rig_world_pose(
        rig.turret,
        tank,
        Vec3::ZERO,
        Quat::IDENTITY,
        &parents,
        &locals,
    )
    .expect("a controlled rig's turret has its complete spawn-time chain to the root");
    pivot.0 = Some((tank, position));
}

/// The controlled tank's authored FOV; absent before spawn or during a possession gap.
pub fn view_fov(
    views: &Query<Option<&TankViews>, With<Controlled>>,
    kind: ViewKind,
) -> Option<f32> {
    crate::tank::controlled_one(views.single()).map(|views| {
        views
            .expect("a controlled tank carries its spawn-built TankViews")
            .get(kind)
            .fov
    })
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        // Tracer emissive values require HDR and bloom to produce the intended highlight.
        Hdr,
        Bloom::NATURAL,
        // The scene depth the VFX billboards' soft-particle fade and the hull marks' decals read,
        // copied from this camera's main pass rather than drawn by a prepass (`scene_depth`). It
        // requires `DepthPrepass`, without which the DEPTH_PREPASS shader def is absent: every
        // sprite is hard-edged and no hull mark draws.
        crate::scene_depth::MainPassDepth,
        Transform::from_translation(Vec3::from_array(INITIAL_EYE))
            .looking_at(Vec3::from_array(INITIAL_LOOK), Vec3::Y),
        OrbitCamera {
            zoom: 0.0,
            target_zoom: 0.0,
        },
        // WHAT THIS CAMERA DRAWS. The game's one 3D camera starts in the commander view, which sees
        // the world AND the body it is riding; `sight::apply_sight_camera_profile` swaps this one
        // component for the optic's profile and back. That single write is the whole "hide my own
        // tank in the gunner sight" mechanism — see `render_policy`.
        crate::render_policy::CameraProfile::BattlefieldThirdPerson,
        // WHAT THIS CAMERA HEARS. Every spatial emitter pans and attenuates against this one
        // listener; the ear gap is the panning width (`sfx::LISTENER_EAR_GAP`), and bevy's default
        // of 4 m would hard-pan anything closer than that.
        SpatialListener::new(crate::sfx::LISTENER_EAR_GAP),
        // WHAT THE PLAYER LOOKS THROUGH. The one declaration behind every reader of the live view
        // — both LOD ladders, the belt selector, the aim projection, the HUD reprojection — none of
        // which may infer "the player's view" from how many cameras happen to exist (`view`).
        PlayerView,
    ));
}

/// Free look: turn the camera from this frame's mouse delta. The one device-reading half of the
/// orbit camera, split out so it hangs on `PlayerInputSet` — with the cursor released the orbit
/// freezes while `orbit_camera` keeps the body following the tank.
fn orbit_look(
    camera: Single<&mut Transform, With<PlayerView>>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    pivot: Res<TurretPivot>,
    tank: Query<(), (With<Tank>, With<Controlled>)>,
) {
    // Before spawn or between possessions there is no rig to anchor the view. Only mouse-look
    // freezes; placement continues following a controlled tank behind an open menu.
    if pivot.offset().is_none() || crate::tank::controlled_one(tank.single()).is_none() {
        return;
    }
    let mut transform = camera.into_inner();
    transform.rotation = orbit_rotation(transform.rotation, mouse_motion.delta);
}

/// The orbit's look law: `rotation` turned by one frame's mouse delta. Yaw and pitch are read back
/// from the rotation, so no orientation state is stored; the delta is already per-frame, so it is
/// not scaled by dt. Pitch stops just short of vertical, where euler angles hit gimbal lock.
pub(crate) fn orbit_rotation(rotation: Quat, mouse_delta: Vec2) -> Quat {
    let (yaw, pitch, _) = rotation.to_euler(EulerRot::YXZ);
    let yaw = yaw - mouse_delta.x * ORBIT_YAW_SENSITIVITY;
    let pitch = (pitch - mouse_delta.y * ORBIT_PITCH_SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0)
}

/// The orbit's placement law: the camera body on the line through `pivot` along the view axis
/// `forward`, `distance` behind it, pulled in by `ground` — the distance along a ray, capped at the
/// second argument, at which the ray meets the ground.
pub(crate) fn orbit_body(
    pivot: Vec3,
    forward: Dir3,
    distance: f32,
    ground: impl FnOnce(Ray3d, f32) -> f32,
) -> Vec3 {
    let back_ray = Ray3d::new(pivot, -forward);
    back_ray.get_point(ground(back_ray, distance))
}

fn orbit_camera(
    camera: Single<(&mut Transform, &mut OrbitCamera, &mut Projection), With<PlayerView>>,
    spatial: SpatialQuery,
    tank: Query<&Transform, (With<Tank>, With<Controlled>, Without<PlayerView>)>,
    views: Query<Option<&TankViews>, With<Controlled>>,
    pivot: Res<TurretPivot>,
    mouse_scroll: Res<AccumulatedMouseScroll>,
    time: Res<Time>,
) {
    let (mut transform, mut orbit, mut projection) = camera.into_inner();

    // Restore the wide commander-view FOV when returning from the gunner optic (which narrows it).
    if let Projection::Perspective(p) = projection.as_mut() {
        p.fov = view_fov(&views, ViewKind::Commander).unwrap_or(COMMANDER_FOV_BEFORE_BIND);
    }
    let (Some(turret_local), Some(tank_transform)) =
        (pivot.offset(), crate::tank::controlled_one(tank.single()))
    else {
        return;
    };

    // The camera's rotation is set by `orbit_look` (the device-reading half, gated on the cursor and
    // ordered `.before` this) — here we only read it to place the body, so the orbit stays frozen
    // behind the menu while the follow keeps tracking the tank.

    orbit.target_zoom =
        (orbit.target_zoom + mouse_scroll.delta.y * ORBIT_ZOOM_SPEED).clamp(0.0, 1.0);
    // Exponential easing makes the dolly's response independent of frame time.
    let ease = 1.0 - (-ORBIT_ZOOM_GLIDE * time.delta_secs()).exp();
    orbit.zoom += (orbit.target_zoom - orbit.zoom) * ease;

    // Orbit around the shared pivot (`orbit_pivot` — the re-aim reconstructs this same point). The
    // camera sits on the line through the pivot along its view axis; the ground ray pulls it in
    // near terrain.
    let pivot_point = orbit_pivot(tank_transform, turret_local);
    let distance = orbit.radius();
    transform.translation = orbit_body(pivot_point, transform.forward(), distance, |ray, max| {
        ground_distance(&spatial, ray, max)
    });
}

/// Re-aim the orbit camera through the committed point when leaving the gunner view.
///
/// Invariant: the orbit pivot, camera body, and committed point remain collinear. A fresh or newly
/// possessed tank has no keyed [`CommittedAim`], so the transition is a no-op.
fn reaim_orbit_on_optic_exit(
    mode: Res<SightMode>,
    committed: Res<CommittedAim>,
    controlled: Query<(Entity, &Rig), With<Controlled>>,
    tank: Query<&Transform, (With<Tank>, With<Controlled>, Without<PlayerView>)>,
    hull: Query<&GlobalTransform, With<Hull>>,
    pivot: Res<TurretPivot>,
    camera: Single<&mut Transform, With<PlayerView>>,
) {
    // Only the exit direction re-aims; entering the optic needs nothing (`gunner_camera` owns the
    // pose outright while in it).
    if *mode != SightMode::ThirdPerson {
        return;
    }
    let Some((tank_entity, rig)) = crate::tank::controlled_one(controlled.single()) else {
        return;
    };
    let Some(local) = committed.get(tank_entity) else {
        return;
    };
    let (Some(turret_local), Some(tank_transform)) =
        (pivot.offset(), crate::tank::controlled_one(tank.single()))
    else {
        return;
    };
    let hull_transform = hull
        .get(rig.hull)
        .expect("a controlled rig's hull is constructed with its global pose");

    // The target uses the propagated hull pose while the pivot uses the current rendered root pose;
    // this view-only transition can therefore span one render frame while moving.
    let target = hull_transform.affine().transform_point3(local);
    let pivot_point = orbit_pivot(tank_transform, turret_local);
    let span = target - pivot_point;
    assert!(
        span.is_finite(),
        "a committed aim and the rendered pivot have finite poses"
    );
    if span == Vec3::ZERO {
        return; // A commitment exactly at the orbit pivot has no bearing to re-aim toward.
    }
    let direction = Dir3::new(span).expect("the finite, nonzero aim span defines a direction");
    camera.into_inner().look_to(direction, Vec3::Y);
}

/// The optic's look bearing (a WORLD direction): the geometric blend, fraction `k`, from the gun's
/// `sight` line toward the committed `intent` — the two ends of the optic's one blend, which rides
/// at the authored sight blend. `k = 0` is the sight line itself, `k = 1` the intent's bearing from the
/// sight's own origin `eye`; the whole interval is a plain interpolation of the two BEARINGS, with
/// no state, so the view cannot overshoot, wobble, or lag.
///
/// The blend uses hull-local bearings. `sight::drive_gunner_aim` bounds intent in the Gunner
/// yaw's rest frame; the camera applies no second clamp. Yaw uses [`shortest_angle`] so a
/// continuous mount crossing the seam does not wind the view the long way round.
///
/// `intent` is the hull-local committed point; `None` (a tank with no commitment yet) collapses the
/// blend to the sight line at every `k`.
pub(crate) fn blended_look(
    hull: &Affine3A,
    eye: Vec3,
    sight: Vec3,
    intent: Option<Vec3>,
    k: f32,
) -> Vec3 {
    let Some(point) = intent else {
        return sight;
    };
    let to_hull = hull.inverse();
    let (sight_yaw, sight_pitch) = yaw_pitch_of(to_hull.transform_vector3(sight));
    let (intent_yaw, intent_pitch) = yaw_pitch_of(point - to_hull.transform_point3(eye));
    let yaw = sight_yaw + k * shortest_angle(intent_yaw - sight_yaw);
    let pitch = sight_pitch + k * (intent_pitch - sight_pitch);
    hull.transform_vector3(hull_local_dir(yaw, pitch))
}

/// Gunner optic (System B): park the camera on the gun's sight, looking along [`blended_look`].
///
/// Parked at the **Gun node** (the elevation pivot / mantlet) — the coaxial sight's natural home.
/// The camera drops the `ViewSubjectBody` channel in gunner view (`sight`'s
/// `apply_sight_camera_profile`), so parking inside the mantlet clips no own geometry. The FOV is
/// the field the view's authored optic frames (`spec::Optics`) — narrow, against a naked-eye
/// commander view.
///
/// The gun end of the blend is the **sight line**, the bore depressed by the current superelevation
/// (`sight::sight_line`, shared with the ranging reticle and the optic mask so the three cannot
/// drift apart): the aim commit lobs the gun up by that angle for the dialed range, so depressing
/// the view by the same holds the sight picture on the target while the barrel rides above it
/// (dial range → barrel rises, view stays on target). At `k = 0` the camera is welded to that line
/// and lags the player's intent at the mount's slew rate (the WT "view follows the gun" feel); at
/// `k = 1` the camera holds the intent and the bore lags on screen instead.
///
/// Up stays the gun node's own +Y — hull-up carried through the yaw-then-pitch chain rather than
/// world up, so a hull-mounted sight rolls *with* the tank on a side-slope instead of drifting off
/// the bore — and the sight line is orthonormal to it by construction.
#[expect(
    clippy::too_many_arguments,
    reason = "a system spanning the player camera, the controlled rig's gun and hull, the committed aim, the blend and the range tables"
)]
fn gunner_camera(
    camera: Single<(&mut Transform, &mut GlobalTransform, &mut Projection), With<PlayerView>>,
    controlled: Query<(Entity, &Rig), With<Controlled>>,
    views: Query<Option<&TankViews>, With<Controlled>>,
    view_nodes: Query<&ViewNode>,
    gun: Query<&GlobalTransform, Without<PlayerView>>,
    hull: Query<&GlobalTransform, (With<Hull>, Without<PlayerView>)>,
    committed: Res<CommittedAim>,
    ranging: Res<Ranging>,
    tables: Query<&RangeTable>,
) {
    let Some((tank, rig)) = crate::tank::controlled_one(controlled.single()) else {
        return;
    };
    // The VIEW gun (design §6C): the optic must ride the render-smoothed pose — the sim gun's
    // chain steps at tick rate since the sim/view split.
    let gun = gun
        .get(ViewNode::resolve(view_nodes.get(rig.gun).ok(), rig.gun))
        .expect("a controlled rig's gun has its global pose, including before view binding");
    let hull = hull
        .get(rig.hull)
        .expect("a controlled rig's hull has its global pose");
    let (mut transform, mut global_transform, mut projection) = camera.into_inner();

    let theta = tables
        .get(rig.muzzle)
        .expect("the primary muzzle receives its range table at complete spawn")
        .superelevation(ranging.range);
    let rotation = gun.rotation();
    let eye = gun.translation();
    let look = blended_look(
        &hull.affine(),
        eye,
        sight_line(rotation, theta),
        committed.get(tank).inspect(|point| {
            assert!(
                point.is_finite(),
                "the committed aim stores finite hull-local points"
            );
        }),
        crate::sight::look::get().gunner_blend.0,
    );
    place_optic_camera(
        &mut transform,
        &mut global_transform,
        &mut projection,
        eye,
        look,
        rotation * Vec3::Y,
        view_fov(&views, ViewKind::Gunner)
            .expect("a complete controlled rig carries its spawn-built TankViews"),
    );
}

/// Park the (parentless) camera at `eye` looking along world `dir`, writing both `Transform` (for
/// next frame's bookkeeping) and `GlobalTransform` (for *this* frame's render + HUD reprojection —
/// propagation already ran), and setting the perspective FOV. Finite pose inputs and a nonzero
/// direction are invariants of the gun's sight line and the bounded intent blend.
fn place_optic_camera(
    transform: &mut Transform,
    global_transform: &mut GlobalTransform,
    projection: &mut Projection,
    eye: Vec3,
    dir: Vec3,
    up: Vec3,
    fov: f32,
) {
    assert!(
        eye.is_finite() && up.is_finite(),
        "the gun's optic pose is finite"
    );
    let dir =
        Dir3::new(dir).expect("the gun's sight and bounded intent blend define a finite direction");
    if let Projection::Perspective(p) = projection {
        p.fov = fov;
    }
    let pose = Transform::from_translation(eye).looking_to(dir, up);
    *transform = pose;
    *global_transform = GlobalTransform::from(pose);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sight::BLEND_SAMPLES;
    use bevy::ecs::system::RunSystemOnce;

    /// Only a possession gap has no FOV; the camera must read the controlled rig's authored view.
    #[test]
    fn view_fov_tracks_control_and_names_the_tankless_gap() {
        let mut world = World::new();
        let read =
            |views: Query<Option<&TankViews>, With<Controlled>>| view_fov(&views, ViewKind::Gunner);
        assert_eq!(world.run_system_once(read).unwrap(), None);
        // Fixture: an arbitrary admitted optic, independent of the selected tank.
        let fov = crate::spec::Optics::Unaided(60.0).vertical_fov();
        let tank = world
            .spawn((Controlled, crate::tests::tank_views(fov)))
            .id();
        assert_eq!(world.run_system_once(read).unwrap(), Some(fov));
        world.entity_mut(tank).remove::<Controlled>();
        assert_eq!(world.run_system_once(read).unwrap(), None);
    }

    /// A malformed controlled rig must not acquire pre-bind framing while input remains active.
    #[test]
    #[should_panic(expected = "a controlled tank carries its spawn-built TankViews")]
    fn view_fov_refuses_a_controlled_tank_without_views() {
        let mut world = World::new();
        world.spawn(Controlled);
        world
            .run_system_once(|views: Query<Option<&TankViews>, With<Controlled>>| {
                view_fov(&views, ViewKind::Commander)
            })
            .unwrap();
    }

    /// A possession write at the end of Update must reach every camera reader this frame. A
    /// capture in Update could precede that write and retain the previous construction's pivot.
    #[test]
    fn a_late_update_handoff_reaches_the_post_update_pivot() {
        let mut app = App::new();
        app.init_resource::<TurretPivot>()
            .init_resource::<SightMode>()
            .init_resource::<CommittedAim>();
        mount_pivot_tracking(&mut app);
        let mut rig = |height| {
            let root = app
                .world_mut()
                .spawn((
                    Transform::IDENTITY,
                    crate::tests::tank_views(COMMANDER_FOV_BEFORE_BIND),
                ))
                .id();
            let turret = app
                .world_mut()
                .spawn((ChildOf(root), Transform::from_xyz(0.0, height, 0.0)))
                .id();
            app.world_mut().entity_mut(root).insert(Rig {
                hull: root,
                turret,
                gun: turret,
                muzzle: turret,
            });
            root
        };
        let old = rig(1.0); // FIXTURE DATA: different construction offsets, metres.
        let new = rig(5.0);
        app.world_mut().entity_mut(old).insert(Controlled);
        app.update();
        assert_eq!(
            app.world().resource::<TurretPivot>().offset(),
            Some(Vec3::Y)
        );
        app.add_systems(
            Update,
            (move |mut commands: Commands| {
                commands.entity(old).remove::<Controlled>();
                commands.entity(new).insert(Controlled);
            })
            .after(capture_turret_pivot),
        );
        app.update();
        assert_eq!(
            app.world().resource::<TurretPivot>().offset(),
            Some(Vec3::Y * 5.0)
        );
    }

    /// Camera anchoring belongs to the currently possessed construction, and must be cleared in
    /// the tankless gap. Reusing the first tank's offset breaks respawn on a different rig.
    #[test]
    fn the_orbit_pivot_follows_possession_and_clears_between_tanks() {
        let mut app = App::new();
        app.init_resource::<TurretPivot>()
            .add_systems(Update, capture_turret_pivot);
        let root = app
            .world_mut()
            .spawn((
                Controlled,
                Transform::IDENTITY,
                crate::tests::tank_views(COMMANDER_FOV_BEFORE_BIND),
            ))
            .id();
        let turret = app
            .world_mut()
            .spawn((Transform::from_xyz(0.0, 2.0, 0.0), ChildOf(root)))
            .id();
        app.world_mut().entity_mut(root).insert(Rig {
            hull: root,
            turret,
            gun: turret,
            muzzle: turret,
        });
        app.update();
        assert_eq!(
            app.world().resource::<TurretPivot>().offset(),
            Some(Vec3::Y * 2.0)
        );
        app.world_mut().entity_mut(root).remove::<Controlled>();
        app.update();
        assert_eq!(app.world().resource::<TurretPivot>().offset(), None);
        let other = app
            .world_mut()
            .spawn((
                Controlled,
                Transform::IDENTITY,
                crate::tests::tank_views(COMMANDER_FOV_BEFORE_BIND),
            ))
            .id();
        let turret = app
            .world_mut()
            .spawn((Transform::from_xyz(1.0, 4.0, 0.0), ChildOf(other)))
            .id();
        app.world_mut().entity_mut(other).insert(Rig {
            hull: other,
            turret,
            gun: turret,
            muzzle: turret,
        });
        app.update();
        assert_eq!(
            app.world().resource::<TurretPivot>().offset(),
            Some(Vec3::new(1.0, 4.0, 0.0))
        );
        app.world_mut().entity_mut(other).remove::<Controlled>();
        app.world_mut().entity_mut(root).insert(Controlled);
        app.update();
        assert_eq!(
            app.world().resource::<TurretPivot>().offset(),
            Some(Vec3::Y * 2.0)
        );
    }

    /// A duplicate control owner must fail, because treating it as a tankless gap would silently
    /// freeze input and the camera while leaving ambiguous ownership alive.
    #[test]
    #[should_panic(expected = "a player controls at most one tank")]
    fn duplicate_control_is_not_a_tankless_camera_state() {
        let mut app = App::new();
        app.init_resource::<TurretPivot>()
            .add_systems(Update, capture_turret_pivot);
        for _ in 0..2 {
            app.world_mut().spawn((
                Controlled,
                crate::tests::tank_views(COMMANDER_FOV_BEFORE_BIND),
                Rig {
                    hull: Entity::PLACEHOLDER,
                    turret: Entity::PLACEHOLDER,
                    gun: Entity::PLACEHOLDER,
                    muzzle: Entity::PLACEHOLDER,
                },
            ));
        }
        app.update();
    }

    /// The angle between two directions, via `atan2` on the cross/dot pair — never `acos` of a dot
    /// product, which loses most of the f32 mantissa near 1, and these are angles that should be 0.
    fn angle_between(a: Vec3, b: Vec3) -> f32 {
        a.cross(b).length().atan2(a.dot(b))
    }

    /// The placement law: the body sits `distance` behind the pivot along the view axis, unless the
    /// ground is nearer along that ray.
    #[test]
    fn the_orbit_body_sits_behind_the_pivot_unless_the_ground_is_nearer() {
        let pivot = Vec3::new(3.0, 4.0, -2.0);
        let forward = Dir3::new(Vec3::new(0.2, -0.3, -1.0)).unwrap();
        let free = orbit_body(pivot, forward, ORBIT_FAR, |_, max| max);
        assert!((free - (pivot - *forward * ORBIT_FAR)).length() < 1e-4);
        let obstruction = ORBIT_FAR * 0.5; // Fixture: the ground lies halfway to the free camera.
        let pulled = orbit_body(pivot, forward, ORBIT_FAR, |_, max| max.min(obstruction));
        assert!((pulled.distance(pivot) - obstruction).abs() < 1e-4);
    }

    /// The look law turns yaw and pitch by the delta and stops pitch short of vertical.
    #[test]
    fn the_orbit_look_clamps_pitch() {
        let turned = orbit_rotation(Quat::IDENTITY, Vec2::new(-100.0, 0.0));
        let (yaw, pitch, _) = turned.to_euler(EulerRot::YXZ);
        assert!(
            shortest_angle(yaw - 100.0 * ORBIT_YAW_SENSITIVITY).abs() < 1e-5 && pitch.abs() < 1e-5
        );
        let up = orbit_rotation(
            Quat::IDENTITY,
            Vec2::new(0.0, -2.0 * PITCH_LIMIT / ORBIT_PITCH_SENSITIVITY),
        );
        let (_, pitch, _) = up.to_euler(EulerRot::YXZ);
        assert!((pitch - PITCH_LIMIT).abs() < 1e-4);
    }

    /// Angular tolerance (rad) for a law asserted through the yaw/pitch round trip — a hundredth of
    /// a milliradian, well under a pixel at the optic's magnification.
    const EPS: f32 = 1e-5;

    /// A hull standing far from the world origin AND off the level, so a blend that composed the
    /// wrong frame — or took `transform_vector3` where it owed `transform_point3` — moves every
    /// number below instead of cancelling. Fixture data.
    fn hull() -> Affine3A {
        Affine3A::from_rotation_translation(
            Quat::from_euler(EulerRot::YXZ, 0.7, 0.11, 0.23),
            Vec3::new(137.0, 4.0, -512.0),
        )
    }

    /// The gun mount in the hull's frame: the elevation pivot, well above the hull origin, so a
    /// bearing measured from the hull origin instead reads a different angle.
    const MOUNT: Vec3 = Vec3::new(0.0, 2.2171, -1.100);
    /// The superelevation the dialed range asks for (rad), large enough that a look taken off the
    /// raised bore instead of the sight line misses by degrees.
    const LOB: f32 = 0.02;

    /// The three world quantities the blend takes: the gun mount, the gun's sight line, and the
    /// hull-local committed point at `(yaw, pitch)` off that mount, `range` metres out.
    fn sighting(
        turret: f32,
        elevation: f32,
        yaw: f32,
        pitch: f32,
        range: f32,
    ) -> (Vec3, Vec3, Vec3) {
        let hull = hull();
        let gun = hull.matrix3
            * Mat3A::from_quat(Quat::from_rotation_y(turret))
            * Mat3A::from_quat(Quat::from_rotation_x(elevation));
        let sight = sight_line(Quat::from_mat3a(&gun), LOB);
        let point = MOUNT + hull_local_dir(yaw, pitch) * range;
        (hull.transform_point3(MOUNT), sight, point)
    }

    /// **`0` is the gun's sight line, exactly.** The endpoint the collapse has to reproduce:
    /// the camera welded to the line the shell arcs back down onto (what `gunner_camera` looked
    /// along before there was a knob), whatever the intent is doing — near, far, and at the
    /// bound's own reach.
    #[test]
    fn the_gun_end_of_the_blend_is_the_sight_line() {
        for (yaw, pitch, range) in [(0.0, 0.0, 50.0), (0.4, -0.2, 4000.0), (-1.9, 0.3, 120.0)] {
            let (eye, sight, point) = sighting(0.35, 0.08, yaw, pitch, range);
            let look = blended_look(&hull(), eye, sight, Some(point), 0.0);
            let off = angle_between(look, sight);
            assert!(
                off < EPS,
                "0 must look along the gun's sight line, off by {off} rad with the intent at \
                 ({yaw}, {pitch}) {range} m out",
            );
        }
        // And with nothing committed at all, every rung collapses to the same line.
        let (eye, sight, _) = sighting(0.35, 0.08, 0.0, 0.0, 50.0);
        for k in BLEND_SAMPLES {
            assert_eq!(blended_look(&hull(), eye, sight, None, k), sight);
        }
    }

    /// **`1` is the committed intent's bearing, exactly.** The other endpoint: the camera
    /// looking straight at the point the player has commanded (what the lead-optic camera did),
    /// measured from the SIGHT's own origin — a bearing off the hull origin ~2.2 m below would miss
    /// a near aim by most of the optic's radius.
    #[test]
    fn the_intent_end_of_the_blend_is_the_committed_point() {
        for (yaw, pitch, range) in [(0.0, -0.05, 40.0), (0.4, -0.2, 4000.0), (-1.9, 0.3, 120.0)] {
            let (eye, sight, point) = sighting(0.35, 0.08, yaw, pitch, range);
            let look = blended_look(&hull(), eye, sight, Some(point), 1.0);
            let off = angle_between(look, hull().transform_point3(point) - eye);
            assert!(
                off < EPS,
                "1 must look at the committed point, off by {off} rad with it at ({yaw}, \
                 {pitch}) {range} m out",
            );
        }
    }

    /// **ONE `k` moves both axes by the same fraction of their own lead.** The knob is a single
    /// position on the segment between the two bearings, so at every rung the yaw and pitch
    /// bearings have each travelled `k` of the way — a blend that lerped the two bearings as
    /// vectors, or re-derived pitch off the blended yaw, would put the axes at different fractions.
    ///
    /// Measured on the hull-local yaw/pitch the blend itself works in, at an intent led off the
    /// sight line on BOTH axes, so a split in either direction moves a number.
    #[test]
    fn one_knob_carries_both_axes_the_same_fraction() {
        // The intent led well off the sight line on BOTH axes: the gun lays at (0.35, 0.08), so
        // this stands ~0.30 rad out in yaw and ~0.15 in pitch.
        let (eye, sight, point) = sighting(0.35, 0.08, 0.65, -0.09, 900.0);
        let to_hull = hull().inverse();
        let bearing = |k| {
            yaw_pitch_of(to_hull.transform_vector3(blended_look(
                &hull(),
                eye,
                sight,
                Some(point),
                k,
            )))
        };
        let (gun_yaw, gun_pitch) = bearing(0.0);
        let (intent_yaw, intent_pitch) = bearing(1.0);
        let (yaw_lead, pitch_lead) = (
            shortest_angle(intent_yaw - gun_yaw),
            intent_pitch - gun_pitch,
        );
        assert!(
            yaw_lead.abs() > 0.2 && pitch_lead.abs() > 0.1,
            "the fixture must lead on both axes, or this proves nothing",
        );

        for k in BLEND_SAMPLES {
            let (yaw, pitch) = bearing(k);
            let (yaw_fraction, pitch_fraction) = (
                shortest_angle(yaw - gun_yaw) / yaw_lead,
                (pitch - gun_pitch) / pitch_lead,
            );
            assert!(
                (yaw_fraction - k).abs() < EPS && (pitch_fraction - k).abs() < EPS,
                "at k = {k} the yaw travelled {yaw_fraction} of its lead and the pitch \
                 {pitch_fraction} — the one knob has split",
            );
        }
    }

    /// **The blend takes the short way round the yaw wrap.** A continuous turret puts no seam in
    /// the mechanism, but the yaw *coordinate* has one at ±π, and the sight line and the intent can
    /// straddle it. Interpolating the two coordinates naively swings the view through the whole
    /// hull instead of across the four degrees actually between them — which is why the commit
    /// itself uses `shortest_angle`, and why this must too.
    #[test]
    fn the_yaw_blend_crosses_the_wrap_the_short_way() {
        const SPLIT: f32 = 0.02;
        let hull = hull();
        let seam = std::f32::consts::PI;
        // Two bearings 2·SPLIT apart, one either side of the seam.
        let sight = hull.transform_vector3(hull_local_dir(seam - SPLIT, 0.01));
        let point = MOUNT + hull_local_dir(-seam + SPLIT, 0.01) * 500.0;
        let eye = hull.transform_point3(MOUNT);
        let intent = hull.transform_point3(point) - eye;

        let look = blended_look(&hull, eye, sight, Some(point), 0.5);
        let (from_sight, from_intent) = (angle_between(look, sight), angle_between(look, intent));
        assert!(
            (from_sight - SPLIT).abs() < EPS && (from_intent - SPLIT).abs() < EPS,
            "a half blend across the wrap must land halfway — {from_sight} rad from the sight line \
             and {from_intent} rad from the intent, against the {SPLIT} rad each way; a naive lerp \
             lands a whole turn away, on the far side of the hull",
        );
    }
}
