//! Camera transform kernels.
//!
//! The game's third-person orbit camera provides free-aim look, scroll-to-zoom dolly, and
//! ground-collision pull-in. The camera is also the aiming device, so look direction stays the
//! player's — zoom only changes the orbit radius, which slides along the view axis and never moves
//! the aim point. [`free_fly_transform`] is the shared pure transform kernel behind the dev
//! sandboxes' distinct ECS adapters.

use avian3d::prelude::{PhysicsSystems, SpatialQuery};
use bevy::camera::Hdr;
use bevy::core_pipeline::prepass::DepthPrepass;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::math::Affine3A;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;

use crate::aim::CommittedAim;
use crate::firecontrol::{RangeTable, Ranging};
use crate::sight::{
    GunnerBlend, SightMode, SightToggled, hull_local_dir, in_gunner, in_third_person, sight_line,
    yaw_pitch_of,
};
use crate::spec::ViewKind;
use crate::state::{GameplaySet, PlayerInputSet};
use crate::tank::{
    Controlled, Hull, Rig, Tank, TankViews, ViewNode, rig_world_pose, shortest_angle,
};
use crate::view::PlayerView;
use crate::world::ground_distance;

/// Apply one free-fly input frame to `transform`.
///
/// Mouse delta controls yaw/pitch without time scaling. WASD moves on the horizontal heading plane,
/// Shift/Ctrl changes altitude, and translation integrates on real-time `delta_secs`.
pub(crate) fn free_fly_transform(
    transform: &mut Transform,
    keys: &ButtonInput<KeyCode>,
    mouse_delta: Vec2,
    delta_secs: f32,
) {
    const SENS: f32 = 0.003;
    const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.001;
    let (mut yaw, mut pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
    yaw -= mouse_delta.x * SENS;
    pitch = (pitch - mouse_delta.y * SENS).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);

    // Looking down and pressing W keeps moving forward over the ground, not into it. Near-vertical
    // look leaves no horizontal heading, so `normalize_or_zero` just no-ops that axis.
    const SPEED: f32 = 12.0;
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
        transform.translation += dir.normalize() * SPEED * delta_secs;
    }
}

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

/// The third-person orbit camera's system set — an ordering anchor for anything that must read or
/// follow the placed orbit pose. A no-op edge in SP (net layers are net-gated) and on a headless
/// client (no camera to place).
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

/// The orbit radius at full zoom-out — how far the camera body can sit from [`orbit_pivot`] along
/// the view axis. A feel knob for [`orbit_camera`], and a BOUND for anything that has to reason
/// about where the camera can actually be relative to the tank it follows: mouse-look is free, so
/// the body can be this far to any side of the pivot, INCLUDING toward whatever is being measured.
/// `terrain_grid`'s far-probe placement test spends it that way — a probe's camera-to-shoe distance
/// is its tank-to-tank distance minus this, worst case.
///
/// A ground ray only ever pulls the body IN (`ground_distance`), so this is a true maximum.
pub(crate) const ORBIT_FAR: f32 = 18.0;

/// The orbit radius at full zoom-in. Paired with [`ORBIT_FAR`] purely as the other end of the
/// dolly; nothing outside [`orbit_camera`] needs it.
const ORBIT_NEAR: f32 = 5.0;

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
            // The one gunner-view camera. It reads the gun's *propagated* pose after propagation and
            // writes its own `GlobalTransform` (no extra propagation pass); HUD markers order after
            // `GunnerCameraPlaced` to reproject through this same pose.
            gunner_camera
                .run_if(in_gunner)
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

/// The vertical FOV (radians) every consumer assumes before it can read a real one — a conservative
/// BOUND, not any vehicle's authored field: narrower than the field any authored optic derives
/// (`spec::Optics`), and deliberately so.
///
/// Two consumers, one reason. The gunner camera and the sight's cursor-travel margin take it for
/// the pre-bind frame before `TankViews` lands, so they agree on one number rather than each
/// carrying a literal. Both LOD ladders seed with it at Startup, before a camera exists
/// (`view::ViewFacts::default`, `world::terrain_lod_view`) — a narrow field demands the finest
/// geometry, so seeding under every authored field makes the first frames over-detailed rather than
/// under-detailed. Widening it would silently coarsen those frames.
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
        // The opaque geometry's depth, one pass ahead of the main pass — the scene depth the VFX
        // billboards read for their soft-particle fade (`assets/shaders/vfx_billboard.wgsl`).
        // Without this component the shader's DEPTH_PREPASS def is absent and every sprite is
        // hard-edged again; the translucent sprites themselves opt out of writing to it.
        DepthPrepass,
        Transform::from_xyz(10.0, 7.0, -7.0).looking_at(Vec3::new(10.0, 1.0, 5.0), Vec3::Y),
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
    camera: Single<(&mut Transform, &mut OrbitCamera, &mut Projection), With<PlayerView>>,
    spatial: SpatialQuery,
    tank: Query<&Transform, (With<Tank>, With<Controlled>, Without<PlayerView>)>,
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

/// The optic's look bearing (a WORLD direction): the geometric blend, fraction `k`, from the gun's
/// `sight` line toward the committed `intent` — the two ends of the one gunner knob
/// ([`GunnerBlend`]). `k = 0` is the sight line itself, `k = 1` the intent's bearing from the
/// sight's own origin `eye`; the whole interval is a plain interpolation of the two BEARINGS, with
/// no state, so the view cannot overshoot, wobble, or lag.
///
/// The blend is taken in the HULL's frame — the frame `sight::drive_gunner_aim` bounds the intent
/// in — so the camera lands on the segment between the two bearings in exactly the space the
/// optic's bound is a circle in, and the drawn glass therefore always contains the intent. Yaw uses
/// [`shortest_angle`], as the commit does: a continuous turret wraps, and a naive lerp would wind
/// the view the long way round.
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
fn gunner_camera(
    camera: Single<(&mut Transform, &mut GlobalTransform, &mut Projection), With<PlayerView>>,
    controlled: Query<(Entity, &Rig), With<Controlled>>,
    views: Query<&TankViews, With<Controlled>>,
    view_nodes: Query<&ViewNode>,
    gun: Query<&GlobalTransform, Without<PlayerView>>,
    hull: Query<&GlobalTransform, (With<Hull>, Without<PlayerView>)>,
    committed: Res<CommittedAim>,
    blend: Res<GunnerBlend>,
    ranging: Res<Ranging>,
    tables: Query<&RangeTable>,
) {
    let Ok((tank, rig)) = controlled.single() else {
        return;
    };
    // The VIEW gun (design §6C): the optic must ride the render-smoothed pose — the sim gun's
    // chain steps at tick rate since the sim/view split.
    let Ok(gun) = gun.get(ViewNode::resolve(view_nodes.get(rig.gun).ok(), rig.gun)) else {
        return;
    };
    let Ok(hull) = hull.get(rig.hull) else {
        return;
    };
    let (mut transform, mut global_transform, mut projection) = camera.into_inner();

    let theta = tables
        .get(rig.muzzle)
        .map_or(0.0, |table| table.superelevation(ranging.range));
    let rotation = gun.rotation();
    let eye = gun.translation();
    let look = blended_look(
        &hull.affine(),
        eye,
        sight_line(rotation, theta),
        committed.get(tank).filter(|point| point.is_finite()),
        blend.0,
    );
    place_optic_camera(
        &mut transform,
        &mut global_transform,
        &mut projection,
        eye,
        look,
        rotation * Vec3::Y,
        view_fov(&views, ViewKind::Gunner, GUNNER_FOV_FALLBACK),
    );
}

/// Park the (parentless) camera at `eye` looking along world `dir`, writing both `Transform` (for
/// next frame's bookkeeping) and `GlobalTransform` (for *this* frame's render + HUD reprojection —
/// propagation already ran), and setting the perspective FOV. A non-finite/zero `dir` is a no-op
/// that keeps the last good pose (a poisoned pose frame must not NaN the camera).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sight::GUNNER_BLEND_LADDER;

    /// The angle between two directions, via `atan2` on the cross/dot pair — never `acos` of a dot
    /// product, which loses most of the f32 mantissa near 1, and these are angles that should be 0.
    fn angle_between(a: Vec3, b: Vec3) -> f32 {
        a.cross(b).length().atan2(a.dot(b))
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
        for k in GUNNER_BLEND_LADDER {
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

        for k in GUNNER_BLEND_LADDER {
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
