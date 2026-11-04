//! The game's damage-model view: gunner sight input and aim-point commitment.
//!
//! The presentation half — every HUD widget that draws the sight picture — lives behind
//! [`reticle`], which this module mounts and otherwise touches only to raise a [`Toast`].

pub(crate) mod look;
mod reticle;

use avian3d::prelude::{Position, Rotation, SpatialQuery};
use bevy::ecs::system::SystemParam;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::math::Affine3A;
use bevy::prelude::*;

use crate::aim::{AimCommit, CommittedAim, MAX_RANGE, aim_distance};
use crate::camera::view_fov;
use crate::command::{AimIntent, TankCommand, TankDeviceSet, gather_commands};
use crate::damage::{ControlledTank, VolumeOf};
use crate::firecontrol::{RangeTable, Ranging};
use crate::flow::MatchResourceExt;
use crate::render_policy::{CameraProfile, VisualScope};
use crate::spec::ViewKind;
use crate::state::{GameplaySet, PlayerInputSet};
use crate::tank::{
    Controlled, GunnerYaw, RemoteServos, ServoIndex, ServoRest, ServoSpec, Tank, TankServos,
    TankViews, rig_world_pose, shortest_angle,
};
use crate::view::PlayerView;

use reticle::Toast;

/// Whether the controlled tank's `kind` view is usable — its authored `requires` met (a knocked-out
/// gunner closes the optic, a knocked-out commander closes third-person). No controlled tank
/// before spawn or between possessions means neither view is available.
fn view_available(
    controlled: &ControlledTank,
    views: &Query<&TankViews, With<Controlled>>,
    kind: ViewKind,
) -> bool {
    crate::tank::controlled_one(views.single())
        .is_some_and(|views| controlled.meets(&views.get(kind).requires))
}

/// Which view the player is in. Default is the third-person commander view.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum SightMode {
    #[default]
    ThirdPerson,
    Gunner,
}

/// Ordering anchor for systems reacting to a sight-mode change in the same frame.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SightToggled;

/// Run condition: the gunner optic is active AND the gunner is alive (otherwise the view is dark
/// and the player gets a prompt to switch).
pub fn in_gunner(mode: Res<SightMode>) -> bool {
    *mode == SightMode::Gunner
}

/// Run condition: the free third-person view is active AND the commander is alive.
pub fn in_third_person(mode: Res<SightMode>) -> bool {
    *mode == SightMode::ThirdPerson
}

/// Fixture: endpoints and interior blend fractions for the optic's geometric invariants.
#[cfg(test)]
pub(crate) const BLEND_SAMPLES: [f32; 6] = [0.0, 0.35, 0.5, 0.65, 0.8, 1.0];

/// Hull-local unit direction for a `(yaw, pitch)` sight bearing — the shared decomposition
/// [`GunnerIntent`] uses, exposed for the camera-placement systems (`camera.rs`). Yaw is about hull
/// up (0 = forward, +left), pitch is elevation.
pub fn hull_local_dir(yaw: f32, pitch: f32) -> Vec3 {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    Vec3::new(-sy * cp, sp, -cy * cp)
}

/// Inverse of [`hull_local_dir`]: recover `(yaw, pitch)` from a hull-local direction.
pub fn yaw_pitch_of(dir: Vec3) -> (f32, f32) {
    (
        (-dir.x).atan2(-dir.z),
        dir.y.atan2((dir.x * dir.x + dir.z * dir.z).sqrt()),
    )
}

/// Per-frame yaw/pitch working form of the committed aim point in the Gunner yaw rest frame.
///
/// Invariant: decompose `point - mount`, never the hull origin, to keep the optic and servo target
/// at the same parallax origin. [`CommittedAim`] remains the sole persistent state.
#[derive(Clone, Copy)]
struct GunnerIntent {
    yaw: f32,
    pitch: f32,
}

impl GunnerIntent {
    /// Intent as a unit direction in the Gunner yaw rest frame.
    fn local_dir(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(-sy * cp, sp, -cy * cp)
    }

    /// Inverse of [`local_dir`](Self::local_dir); callers provide the mount-relative direction.
    fn from_local_dir(dir: Vec3) -> Self {
        Self {
            yaw: (-dir.x).atan2(-dir.z),
            pitch: dir.y.atan2((dir.x * dir.x + dir.z * dir.z).sqrt()),
        }
    }
}

pub fn plugin(app: &mut App) {
    app.init_match_resource::<SightMode>()
        // Every HUD widget the sight draws, and the systems that keep them in step. The presentation
        // half orders itself against `toggle_sight` below.
        .add_plugins(reticle::plugin)
        .add_systems(
            Update,
            // Only the Lshift view toggle is player input (gated on the cursor); the overlay, toast,
            // and range readout are presentation and keep updating with the cursor free.
            toggle_sight
                .in_set(PlayerInputSet)
                .in_set(TankDeviceSet)
                .in_set(SightToggled)
                .in_set(GameplaySet),
        )
        // Commit the commanded aim from the magnified mouse intent. In `BeforeFixedMainLoop` (with
        // `gather_commands`), NOT `Update`: the fixed loop runs its sim ticks *before* `Update`, so
        // an aim written in `Update` is one render frame stale by the time the sim consumes it —
        // +16.7 ms at 60 Hz of avoidable input latency. This reads only the mouse motion (ready in
        // `PreUpdate`), the last tick's servo angles, and the tick-truth physics pose for its
        // mount-origin resolve (`rig_world_pose` from `Position`/`Rotation` — never the camera or a
        // render-rate `GlobalTransform`), so it moves cleanly out of `Update`. `.after(gather_commands)`
        // pins the order — both write `TankCommand` (disjoint fields: `gather_commands` the
        // drive/range fields, this one `aim`) — and puts the aim commit after this frame's fresh
        // `Ranging` has reached the command. Still in `AimCommit` so `aim::drive_aim_servos` (fixed
        // clock) reads whatever intention stands at each tick.
        .add_systems(
            RunFixedMainLoop,
            // The one gunner commit — the sole author of `CommittedAim` while the optic is up (the
            // single-writer invariant).
            drive_gunner_aim
                .run_if(in_gunner)
                .after(gather_commands)
                .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
                .in_set(AimCommit)
                .in_set(PlayerInputSet)
                .in_set(TankDeviceSet)
                .in_set(GameplaySet),
        )
        // Declare the two view facts `render_policy` resolves: whose body the camera is riding, and
        // which channels the camera draws. Continuous derived render state, no `run_if`/ordering
        // edge (see each system's doc comment — event-driven was the original defect). Both are
        // `set_if_neq` writes on ONE component per tank root and one per camera, so an
        // unconditional schedule costs a handful of reads in steady state.
        .add_systems(
            Update,
            (mark_view_subject_body, apply_sight_camera_profile).in_set(GameplaySet),
        );
}

/// Toggle only to a view with a live crewman.
///
/// Invariant: both modes share [`aim::CommittedAim`], so switching modes never performs an aim handoff.
fn toggle_sight(
    keys: Res<ButtonInput<KeyCode>>,
    mut mode: ResMut<SightMode>,
    controlled: ControlledTank,
    views: Query<&TankViews, With<Controlled>>,
    mut toast: ResMut<Toast>,
) {
    if !keys.just_pressed(KeyCode::ShiftLeft) {
        return;
    }
    if controlled.rig().is_none() {
        return;
    }
    *mode = match *mode {
        SightMode::ThirdPerson => {
            if !view_available(&controlled, &views, ViewKind::Gunner) {
                toast.show(format!("{} unavailable", ViewKind::Gunner.label()));
                return;
            }
            SightMode::Gunner
        }
        SightMode::Gunner => {
            if !view_available(&controlled, &views, ViewKind::Commander) {
                toast.show(format!("{} unavailable", ViewKind::Commander.label()));
                return;
            }
            SightMode::ThirdPerson
        }
    };
}

/// Declare WHOSE BODY the local camera is riding, so the optic can drop it.
///
/// One [`VisualScope`] write per tank ROOT — never per mesh: `render_policy` supplies
/// application-level inheritance, so the whole body (hull, turret, ~194 track shoes, every glb leaf
/// that has not even loaded yet) follows this one component.
///
/// Runs every frame with no `run_if` and no ordering edge, for the reason the per-mesh sweep it
/// replaces did: control can move between tanks with NO sight-mode event (a multiplayer respawn),
/// and a tank can appear at any time. `set_if_neq` makes the steady state a read of one component
/// per tank and nothing downstream — `render_policy` only re-resolves the subtree on a CHANGE.
fn mark_view_subject_body(
    controlled: Query<Entity, With<Controlled>>,
    mut tanks: Query<(Entity, &mut VisualScope), With<Tank>>,
) {
    let subject = crate::tank::controlled_one(controlled.single());
    for (tank, mut scope) in &mut tanks {
        scope.set_if_neq(if Some(tank) == subject {
            VisualScope::VIEW_SUBJECT_BODY
        } else {
            VisualScope::WORLD_SOLID
        });
    }
}

/// Point every camera of the player's view at the channel set the current view wants. A camera
/// with another profile (the pen cam's inset and masks) keeps its own.
///
/// This is the entire "hide the player's own tank in the gunner optic" mechanism now: ONE
/// component per camera, O(1) in the size of the world. The optic profile drops
/// `ViewSubjectBody` and keeps drawing everything else — it is not a "hide" flag and it is not a
/// second camera.
fn apply_sight_camera_profile(
    mode: Res<SightMode>,
    mut cameras: Query<&mut CameraProfile, With<Camera3d>>,
) {
    let want = match *mode {
        SightMode::Gunner => CameraProfile::BattlefieldOptic,
        SightMode::ThirdPerson => CameraProfile::BattlefieldThirdPerson,
    };
    for mut profile in &mut cameras {
        if matches!(
            *profile,
            CameraProfile::BattlefieldOptic | CameraProfile::BattlefieldThirdPerson
        ) {
            profile.set_if_neq(want);
        }
    }
}

/// Viewport aspect (width / height) before the camera's render target has one — square, the aspect
/// at which the two half-extents coincide.
const OPTIC_ASPECT_FALLBACK: f32 = 1.0;

/// The cursor's angular reach off the gun's sight line for vertical field `fov` over a viewport of
/// `aspect` (width / height): ONE angle, equal at every bearing.
///
/// The authored fraction of the LARGER axis' half-extent, taken through the projection —
/// `atan(f · tan(half-field))`. In height units that half-extent is `max(aspect, 1)` times the
/// vertical one, and the drawn rim (`reticle`) spans the same fraction of the same axis.
pub(crate) fn optic_margin(fov: f32, aspect: f32) -> f32 {
    (look::get().optic_span_fraction.0 * aspect.max(1.0) * (fov / 2.0).tan()).atan()
}

/// Clamp `value` to a servo's authored travel `limits` (radians); a `None` (continuous) mount passes
/// through untouched.
fn clamp_to_travel(value: f32, limits: Option<(f32, f32)>) -> f32 {
    match limits {
        Some((min, max)) => value.clamp(min, max),
        None => value,
    }
}

/// Convert the elevation servo's lay limits into the sight-line window (`sight = lay − lob`).
fn sight_pitch_limits(lay_limits: Option<(f32, f32)>, lob: f32) -> Option<(f32, f32)> {
    lay_limits.map(|(min, max)| (min - lob, max - lob))
}

/// Move mechanical yaw windows into the sight frame modulo a turn: a servo angle is unwrapped,
/// while the sight's decomposition is a principal angle.
fn sight_yaw_limits(limits: Option<(f32, f32)>, current: f32, servo: f32) -> Option<(f32, f32)> {
    let offset = shortest_angle(current - servo);
    limits.map(|(min, max)| (min + offset, max + offset))
}

/// The gun's SIGHT LINE as a world direction: its bore (the gun node's −Z) depressed by the
/// superelevation `theta` about that node's own right axis (+X), undoing the lob
/// `aim::drive_aim_servos` laid on for the dialed range. This is the line the shell arcs back down
/// onto, so it — not the raised bore — is the optic's centre (`camera::gunner_camera`) and the
/// ranging reticle's zero mark (`reticle`); both take it from here so the two cannot drift apart.
///
/// Pitching about the node's right axis keeps the sight line, that right axis and the node's up
/// mutually orthonormal, so a caller may use the same frame's up unchanged.
///
/// `firecontrol::lob` is the inverse, about `v × Y` instead of the trunnion: the two axes coincide
/// in the hull's frame, where the turret's yaw keeps `v × Y` on the trunnion, and diverge in world
/// space under hull roll — so that form belongs where the aim is solved (hull-local) and this one
/// here, where the barrel's physical pitch axis is what the view and the reticle must follow.
pub(crate) fn sight_line(gun_rotation: Quat, theta: f32) -> Vec3 {
    Quat::from_axis_angle(gun_rotation * Vec3::X, -theta) * (gun_rotation * Vec3::NEG_Z)
}

/// Values published by `drive_gunner_aim` for this frame.
struct AimPublish {
    /// Command aim re-authored every optic frame.
    command_aim: Vec3,
    /// Updated committed point; `None` preserves existing memory.
    store: Option<Vec3>,
}

/// Preserve an existing committed point until the optic receives mouse motion.
///
/// Invariant: zero input is identity across view origins. Mouse motion, or no prior commitment,
/// publishes and stores `resolved`.
fn resume_commit(committed_point: Option<Vec3>, moved: bool, resolved: Vec3) -> AimPublish {
    match committed_point {
        Some(point) if !moved => AimPublish {
            command_aim: point,
            store: None,
        },
        _ => AimPublish {
            command_aim: resolved,
            store: Some(resolved),
        },
    }
}

/// Read access to a tank's live servo lays: the [`ServoIndex`] slot each servo entity holds, plus
/// the two root-resident integrators those slots address.
#[derive(SystemParam)]
pub(crate) struct LiveServos<'w, 's> {
    slots: Query<'w, 's, &'static ServoIndex>,
    yaws: Query<'w, 's, &'static GunnerYaw>,
    rests: Query<'w, 's, &'static ServoRest>,
    states: Query<'w, 's, &'static TankServos>,
    remote: Query<'w, 's, &'static RemoteServos>,
}

impl LiveServos<'_, '_> {
    /// A `servo` entity's live parent-local lay under `tank`. Same preference the mechanism itself
    /// integrates through (`tank::servo`): the client-local [`RemoteServos`] when the tank carries
    /// one — every replica, the own hull included — else the authoritative [`TankServos`] snapshot.
    /// The optic clamps intent against the gun's live lay, so it must read the integrator that is
    /// moving it.
    fn angle(&self, tank: Entity, servo: Entity) -> f32 {
        let slot = self
            .slots
            .get(servo)
            .expect("every declared rig servo receives its index at complete spawn");
        let state = if let Ok(servos) = self.remote.get(tank) {
            servos
                .0
                .get(slot.0)
                .expect("the replica's complete servo vector covers every declared slot")
        } else {
            self.states
                .get(tank)
                .expect("a controlled rig has either replica or authoritative servo state")
                .states
                .get(slot.0)
                .expect("the authority's complete servo vector covers every declared slot")
        };
        let angle = state.current();
        assert!(
            angle.is_finite(),
            "the live servo integrator holds finite angles"
        );
        angle
    }
}

/// Resolve optic input into the shared hull-local [`aim::CommittedAim`] and `TankCommand`.
///
/// The optic is a hull-anchored view — camera, working intent and gun all ride the hull — so
/// its intention travels hull-local, unchanged across the delivery gap (ADR-0038).
///
/// Invariants: decomposition, clamping, and ray resolution share the gun-mount origin;
/// [`resume_commit`] alone owns zero-input identity; mechanical travel is applied before the
/// circular [`optic_margin`] clamp; and intent remains absolute inside both bounds rather than
/// following the current servo lay.
#[expect(
    clippy::too_many_arguments,
    reason = "a system spanning optic input, the controlled tank and its servos, ranging and aim-ray geometry"
)]
pub(crate) fn drive_gunner_aim(
    motion: Res<AccumulatedMouseMotion>,
    spatial: SpatialQuery,
    mut committed: ResMut<CommittedAim>,
    controlled: ControlledTank,
    views: Query<&TankViews, With<Controlled>>,
    cameras: Query<&Camera, With<PlayerView>>,
    servos: LiveServos,
    servo_specs: Query<&ServoSpec>,
    ranging: Res<Ranging>,
    tables: Query<&RangeTable>,
    poses: Query<(&Position, &Rotation)>,
    parents: Query<&ChildOf>,
    locals: Query<&Transform>,
    volumes: Query<&VolumeOf>,
    mut tank_commands: Query<&mut TankCommand>,
) {
    let (Some(tank), Some(rig)) = (controlled.entity(), controlled.rig()) else {
        return;
    };

    let committed_point = committed.get(tank).inspect(|point| {
        assert!(
            point.is_finite(),
            "the committed aim stores finite hull-local points"
        );
    });
    let moved = motion.delta != Vec2::ZERO;

    // Fast path before pose guards so [`resume_commit`]'s preserved point is always re-authored.
    if let Some(point) = committed_point
        && !moved
    {
        tank_commands
            .get_mut(tank)
            .expect("Tank requires its command component in the same construction flush")
            .aim = Some(AimIntent::HullLocal(point));
        return;
    }

    // The field the view's authored optic frames (`spec::Optics`) and the viewport it is framed on
    // together set the cursor's reach. A square viewport covers an unmeasured render target.
    let fov = view_fov(&views, ViewKind::Gunner)
        .expect("a complete controlled rig carries its spawn-built TankViews");
    let aspect = cameras
        .single()
        .ok()
        .and_then(Camera::logical_viewport_size)
        .filter(|size| size.y > 0.0)
        .map_or(OPTIC_ASPECT_FALLBACK, |size| size.x / size.y);
    let margin = optic_margin(fov, aspect);

    // Scaling mouse gain with the field preserves comparable screen travel across optics.
    let sensitivity = look::get().mouse_sensitivity_per_fov.0 * fov;

    // The margin below clamps intent against the gun's live lay — see [`LiveServos::angle`].
    let yaw = servos
        .yaws
        .get(tank)
        .expect("a complete player rig carries its separately resolved Gunner yaw handle")
        .0;
    let yaw_angle = servos.angle(tank, yaw);
    let pitch_angle = servos.angle(tank, rig.gun);

    // Superelevation for the dialed range; the gun's live pitch carries it, so the sight line (which
    // the intent tracks) is the gun's lay minus the lob. One binding for that scalar: the seed and
    // the circular clamp below must measure from the same line, or the intent settles off the mark
    // it was clamped against.
    let theta = tables
        .get(rig.muzzle)
        .expect("the primary muzzle receives its range table at complete spawn")
        .superelevation(ranging.range);

    // The sight's origin: the gun mount (elevation pivot), from the SAME physics-truth chain
    // `aim::drive_aim_servos` lays from (`rig_world_pose`, never `GlobalTransform`), so the
    // decomposition below, the clamps against the live lay, and the servo convergence all measure
    // their angles from one origin. The hull frame anchors the committed point's local form.
    let (root_position, root_rotation) = poses
        .get(tank)
        .expect("a controlled tank is constructed with its authoritative or replicated pose");
    let (hull_position, hull_rotation) = rig_world_pose(
        rig.hull,
        tank,
        root_position.0,
        root_rotation.0,
        &parents,
        &locals,
    )
    .expect("the controlled rig's hull remains in its spawn-built pose chain");
    let (mount_world, _) = rig_world_pose(
        rig.gun,
        tank,
        root_position.0,
        root_rotation.0,
        &parents,
        &locals,
    )
    .expect("the controlled rig's gun mount remains in its spawn-built pose chain");
    let hull_affine = Affine3A::from_rotation_translation(hull_rotation, hull_position);
    let mount_local = hull_affine.inverse().transform_point3(mount_world);
    assert!(
        mount_world.is_finite() && mount_local.is_finite(),
        "a complete rig has finite hull and gun-mount poses"
    );

    // Work in the yaw's angle-zero frame. Upstream articulation turns this whole frame once;
    // its turn must not be counted again as this servo's parent-local command.
    let (_, yaw_frame) = crate::tank::servo_aim_frame(
        yaw,
        tank,
        root_position.0,
        root_rotation.0,
        servos
            .rests
            .get(yaw)
            .expect("a declared Gunner yaw has its spawn-built rest"),
        &parents,
        &locals,
    );
    let (_, pitch_frame) = crate::tank::servo_aim_frame(
        rig.gun,
        tank,
        root_position.0,
        root_rotation.0,
        servos
            .rests
            .get(rig.gun)
            .expect("a declared Gunner pitch has its spawn-built rest"),
        &parents,
        &locals,
    );
    let current = GunnerIntent::from_local_dir(
        yaw_frame.inverse() * pitch_frame * Quat::from_rotation_x(pitch_angle) * Vec3::NEG_Z,
    );
    let (t_current, g_current) = (current.yaw, current.pitch);
    let sight_now = g_current - theta;
    let hull_to_aim = yaw_frame.inverse() * hull_rotation;

    // Resume the one committed intention into yaw/pitch — the shared `CommittedAim`, whether it was
    // set by the commander commit (`aim::commit_aim`) or by this system's own last resolve. The
    // bearing is `point − mount`, from the sight's origin (see `GunnerIntent`) — decomposing the
    // raw point would measure from the hull origin ~2.2 m below and snap the aim by the mount
    // parallax on the first input. When this tank has NO commitment yet (fresh spawn, or a
    // possession change — the entity-keyed `get` reads `None`), seed from the gun's CURRENT lay
    // instead. This single rule replaces the old seed-on-entry `toggle_sight` did: an active
    // commander aim is simply continued, only an absent commitment falls back to the lay. Seed from
    // the sight line (lay − lob), not the raised bore, or the view jumps θ on handover.
    let mut intent = committed_point
        .map(|point| GunnerIntent::from_local_dir(hull_to_aim * (point - mount_local)))
        .unwrap_or(GunnerIntent {
            yaw: t_current,
            pitch: sight_now,
        });

    intent.yaw -= motion.delta.x * sensitivity;
    intent.pitch -= motion.delta.y * sensitivity;

    // Mechanical windows are parent-local lay limits. Express them in the working frame and
    // lower pitch by the lob before applying the optic margin.
    let pitch_limits = sight_pitch_limits(
        servo_specs
            .get(rig.gun)
            .expect("the rig gun is a declared pitch servo at spawn")
            .travel_limits(),
        theta - (g_current - pitch_angle),
    );
    let yaw_limits = sight_yaw_limits(
        servo_specs
            .get(yaw)
            .expect("the Gunner yaw handle names a declared Yaw servo at spawn")
            .travel_limits(),
        t_current,
        yaw_angle,
    );
    intent.pitch = clamp_to_travel(intent.pitch, pitch_limits);
    intent.yaw = clamp_to_travel(intent.yaw, yaw_limits);

    // Bound 2 — the circular optic margin. Lead as a 2D angular vector from the gun chain's current
    // *sight line* (lay − lob). Yaw uses shortest-angle difference so continuous traverse doesn't
    // wind up. `drive_servos` steps on the fixed clock, so `current` here is the latest tick's
    // integrated angle — the clamp chases the sim truth, ≤1 tick behind the render pose the optic
    // shows. Preserve direction, cap magnitude at the margin; within it the intent is untouched
    // (see the doc comment — re-pinning would make the target recede with the gun). The
    // interpolation stays inside the travel window (both endpoints are, and scale ∈ [0, 1]).
    let yaw_offset = shortest_angle(intent.yaw - t_current);
    let pitch_offset = intent.pitch - sight_now;
    let reach = yaw_offset.hypot(pitch_offset);
    if reach > margin {
        let scale = margin / reach;
        intent.yaw = t_current + yaw_offset * scale;
        intent.pitch = sight_now + pitch_offset * scale;
    }

    // Resolve the (possibly moved) sight line against the world: a ray from the mount along the
    // intent bearing, hitting whatever a shell would meet — terrain or another tank's armor, own
    // tank excluded (the ray starts inside the mantlet volume) — with the shared far fallback in
    // the sky. `point = mount + dir·t` is the committed hull-local form; decomposing it next frame
    // (`point − mount`) recovers these exact angles, so the resolve round-trips and the intent
    // never drifts. Raw sight-line point, hull-local so it rides with the tank (unstabilized);
    // `drive_aim_servos` lobs it by the superelevation, raising the bore above the line of sight,
    // so this stays the intention.
    let dir_local = intent.local_dir();
    let dir_world = Dir3::new(yaw_frame * dir_local)
        .expect("finite rig rotation and bounded intent define a nonzero world direction");
    let distance = aim_distance(
        &spatial,
        Ray3d::new(mount_world, dir_world),
        *MAX_RANGE,
        tank,
        &volumes,
        &parents,
    );
    let resolved = mount_local + hull_to_aim.inverse() * dir_local * distance;

    // Publish. [`resume_commit`] is the full decision (its no-motion arm was short-circuited at the
    // top of the system, before the pose work); reaching here means the OWNING transition — mouse
    // input, or a fresh tank with no commitment to preserve — so the resolved point is published
    // AND re-stored, and the commander finds the optic's aim — a real point on the world — on a
    // later mode switch. Between the fast path and this, SOMETHING writes `command.aim` every
    // healthy frame (the recirculation invariant for the optic: never fall silent).
    let publish = resume_commit(committed_point, moved, resolved);
    if let Some(point) = publish.store {
        committed.set(tank, point);
    }
    tank_commands
        .get_mut(tank)
        .expect("Tank requires its command component in the same construction flush")
        .aim = Some(AimIntent::HullLocal(publish.command_aim));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A multi-turn mechanical window must not shift by a turn when the view wraps its yaw.
    /// The gunner's clamp must retain an intent still inside the authored travel window.
    #[test]
    fn wrapped_sight_yaw_preserves_the_mechanical_window() {
        let limits = (-200.0_f32.to_radians(), 200.0_f32.to_radians()); // Fixture travel.
        let window = sight_yaw_limits(
            Some(limits),
            -170.0_f32.to_radians(),
            190.0_f32.to_radians(),
        )
        .unwrap();
        assert!((window.0 - limits.0).abs() < 1e-5 && (window.1 - limits.1).abs() < 1e-5);
        let intent = 179.0_f32.to_radians(); // Fixture intent within the authored window.
        assert_eq!(clamp_to_travel(intent, Some(window)), intent);
    }

    /// Optic input must use the separately declared yaw's live angle and travel, whether the
    /// structural assembly has no servo or a different one. Offset sight origins must round-trip.
    #[test]
    fn optic_input_uses_the_gunner_yaw_instead_of_the_assembly() {
        use crate::tank::{Rig, ServoRole, ServoState};
        use avian3d::collider_tree::ColliderTrees;
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.init_resource::<AccumulatedMouseMotion>();
        world.init_resource::<CommittedAim>();
        world.init_resource::<Ranging>();
        world.init_resource::<ColliderTrees>();
        // Fixture bearings, offset sight and range. This probes input/clamping, not integration.
        let (bearing, offset) = (0.7, Vec3::new(1.0, 2.0, 3.0));
        let tank = world
            .spawn((
                Tank,
                Controlled,
                Position(Vec3::ZERO),
                Rotation::default(),
                TankServos {
                    states: vec![
                        ServoState::test_new(-1.0, -1.0, 0.0),
                        ServoState::test_new(bearing, bearing, 0.0),
                        ServoState::default(),
                    ],
                },
            ))
            .id();
        // Fixture authored optic: the complete rig always carries both view records at spawn.
        let optics: crate::spec::Optics = ron::from_str("Unaided(60.0)").unwrap();
        let fov = optics.vertical_fov();
        let view = || crate::tank::ViewConfig {
            fov,
            requires: default(),
        };
        world.entity_mut(tank).insert(TankViews {
            gunner: view(),
            commander: view(),
        });
        let hull = world.spawn((Transform::IDENTITY, ChildOf(tank))).id();
        let assembly = world.spawn((Transform::IDENTITY, ChildOf(hull))).id();
        let yaw = world
            .spawn((
                Transform::from_translation(offset).with_rotation(Quat::from_rotation_y(bearing)),
                ChildOf(assembly),
                ServoIndex(1),
                ServoRest(Quat::IDENTITY),
                ServoSpec::test_continuous(ServoRole::Yaw, 1.0),
            ))
            .id();
        let gun = world
            .spawn((
                Transform::IDENTITY,
                ChildOf(yaw),
                ServoIndex(2),
                ServoRest(Quat::IDENTITY),
                ServoSpec::test_limited(ServoRole::Pitch, 1.0, -30.0, 30.0),
            ))
            .id();
        let muzzle = world
            .spawn((
                Transform::IDENTITY,
                ChildOf(gun),
                RangeTable::test_fixed(0.0),
            ))
            .id();
        world.entity_mut(tank).insert((
            Rig {
                hull,
                turret: assembly,
                gun,
                muzzle,
            },
            GunnerYaw(yaw),
        ));
        for assembly_has_servo in [false, true] {
            if assembly_has_servo {
                world.get_mut::<Transform>(assembly).unwrap().rotation =
                    Quat::from_rotation_y(-1.0);
                world.entity_mut(assembly).insert((
                    ServoIndex(0),
                    ServoSpec::test_limited(ServoRole::Yaw, 1.0, -90.0, 0.0),
                ));
            }
            world.insert_resource(CommittedAim::default());
            world.run_system_once(drive_gunner_aim).unwrap();
            let point = world.resource::<CommittedAim>().get(tank).unwrap();
            let base = if assembly_has_servo { -1.0 } else { 0.0 };
            let mount = Quat::from_rotation_y(base) * offset;
            let dir = point - mount;
            let actual = (-dir.x).atan2(-dir.z);
            assert!(
                (actual - (base + bearing)).abs() < 1e-5,
                "{actual} versus {}",
                base + bearing
            );
            // A large input must stop at this optic's margin about its own yaw, not the assembly's.
            world.resource_mut::<AccumulatedMouseMotion>().delta =
                Vec2::new(-1.0 / (look::get().mouse_sensitivity_per_fov.0 * fov), 0.0);
            world.run_system_once(drive_gunner_aim).unwrap();
            let dir = world.resource::<CommittedAim>().get(tank).unwrap() - mount;
            let actual = (-dir.x).atan2(-dir.z);
            let expected = base + bearing + optic_margin(fov, OPTIC_ASPECT_FALLBACK);
            assert!(
                (actual - expected).abs() < 1e-5,
                "{actual} versus {expected}"
            );
            world.resource_mut::<AccumulatedMouseMotion>().delta = Vec2::ZERO;
        }
    }

    /// Optic intent follows the replica's live lay. A malformed replica vector must fail rather
    /// than silently borrowing the authority vector's different angle for the same slot.
    #[test]
    #[should_panic(expected = "replica's complete servo vector")]
    fn an_incomplete_replica_servo_vector_cannot_read_authority_state() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        let tank = app
            .world_mut()
            .spawn((TankServos::for_count(1), RemoteServos(Vec::new())))
            .id();
        let servo = app.world_mut().spawn(ServoIndex(0)).id();
        app.world_mut()
            .run_system_once(move |live: LiveServos| live.angle(tank, servo))
            .expect("the fixture supplies every system resource");
    }

    use crate::render_policy;
    use crate::spec::Optics;

    /// The game's shape in miniature: one camera, the sun, two tanks with meshes at two depths, and
    /// both sight systems plus the `render_policy` resolver wired exactly as the client mounts them.
    struct Views {
        app: App,
        camera: Entity,
        sun: Entity,
    }

    impl Views {
        fn new() -> Self {
            let mut app = App::new();
            app.init_resource::<SightMode>();
            app.add_plugins(render_policy::plugin);
            app.add_systems(Update, (mark_view_subject_body, apply_sight_camera_profile));
            let camera = app
                .world_mut()
                .spawn((Camera3d::default(), CameraProfile::BattlefieldThirdPerson))
                .id();
            let sun = app
                .world_mut()
                .spawn(render_policy::LightProfile::BattlefieldSun)
                .id();
            Self { app, camera, sun }
        }

        fn tank(&mut self, controlled: bool) -> Entity {
            let mut tank = self.app.world_mut().spawn((Tank, VisualScope::WORLD_SOLID));
            if controlled {
                tank.insert(Controlled);
            }
            tank.id()
        }

        fn mesh(&mut self, parent: Entity) -> Entity {
            self.app
                .world_mut()
                .spawn((Mesh3d(Handle::default()), ChildOf(parent)))
                .id()
        }

        fn set_mode(&mut self, mode: SightMode) {
            *self.app.world_mut().resource_mut::<SightMode>() = mode;
        }

        /// Does the player SEE this mesh right now? The law, not the bit.
        fn drawn(&self, mesh: Entity) -> bool {
            render_policy::reaches(self.app.world(), self.camera, mesh)
        }

        fn lit_by_sun(&self, mesh: Entity) -> bool {
            render_policy::reaches(self.app.world(), self.sun, mesh)
        }
    }

    /// Regression, carried over from the per-frame layer sweep this replaces: the five transitions
    /// that each cost a bug once. Phase 3 (a mesh attaching asynchronously while the optic is up)
    /// and phase 4 (control moving between tanks with NO `SightMode` write — the multiplayer
    /// respawn) are the two that a one-shot, event-driven stamp gets wrong.
    ///
    /// Stated as "is it drawn", never as a layer number: the guarantees are about what the player
    /// sees, and they must survive a renumbering of the channels.
    #[test]
    fn the_view_subject_follows_control_and_late_meshes() {
        let mut views = Views::new();
        let tank_a = views.tank(true);
        let a_direct = views.mesh(tank_a);
        let a_subnode = views.app.world_mut().spawn(ChildOf(tank_a)).id();
        let a_nested = views.mesh(a_subnode);
        let tank_b = views.tank(false);
        let b_mesh = views.mesh(tank_b);

        // 1. Third person: everything is drawn, both tanks.
        views.app.update();
        for mesh in [a_direct, a_nested, b_mesh] {
            assert!(views.drawn(mesh), "third person: mesh {mesh:?} is drawn");
        }

        // 2. Gunner: the controlled tank's body drops out; the opponent's does not.
        views.set_mode(SightMode::Gunner);
        views.app.update();
        assert!(!views.drawn(a_direct), "gunner: own direct mesh hidden");
        assert!(!views.drawn(a_nested), "gunner: own nested mesh hidden");
        assert!(views.drawn(b_mesh), "gunner: opponent stays visible");

        // 3. A NEW mesh attaches under the controlled tank WHILE in the optic — the async glb
        // arrival the one-shot stamp missed.
        let a_late = views.mesh(tank_a);
        views.app.update();
        assert!(
            !views.drawn(a_late),
            "a mesh attached while the optic is up inherits its tank's scope"
        );

        // 4. Move `Controlled` to the opponent with NO `SightMode` write — the multiplayer respawn.
        views
            .app
            .world_mut()
            .entity_mut(tank_a)
            .remove::<Controlled>();
        views.app.world_mut().entity_mut(tank_b).insert(Controlled);
        views.app.update();
        for mesh in [a_direct, a_nested, a_late] {
            assert!(
                views.drawn(mesh),
                "respawn: the stepped-out tank's mesh {mesh:?} is world geometry again"
            );
        }
        assert!(
            !views.drawn(b_mesh),
            "respawn: the newly controlled tank drops out with no SightMode change (the bug)"
        );

        // 5. Back to third person: everything drawn again.
        views.set_mode(SightMode::ThirdPerson);
        views.app.update();
        for mesh in [a_direct, a_nested, a_late, b_mesh] {
            assert!(views.drawn(mesh), "back to third person: {mesh:?} is drawn");
        }
    }

    /// The controlled tank must keep CASTING while the player is inside its optic. Hiding a body
    /// from one camera and removing it from the sun are two different decisions, and the mechanism
    /// this replaces could not tell them apart — anything it hid stopped casting, which is why the
    /// track ribbon needed an exemption and an alpha trick to survive.
    ///
    /// Asserted in the SHAPE the game builds: an ordinary hull mesh and a shadow-proxy ribbon under
    /// the same controlled tank, across the mode transition, because the proxy's whole failure mode
    /// is being invisible while it fails.
    #[test]
    fn the_view_subject_and_its_shadow_proxy_keep_casting_in_the_gunner_optic() {
        let mut views = Views::new();
        let tank = views.tank(true);
        let hull = views.mesh(tank);
        let ribbon = views
            .app
            .world_mut()
            .spawn((
                Mesh3d(Handle::default()),
                VisualScope::SHADOW_PROXY,
                ChildOf(tank),
            ))
            .id();

        for mode in [SightMode::ThirdPerson, SightMode::Gunner] {
            views.set_mode(mode);
            views.app.update();
            assert!(
                views.lit_by_sun(hull) && render_policy::casts_shadow(views.app.world(), hull),
                "the player's own hull casts in every view"
            );
            assert!(
                views.lit_by_sun(ribbon) && render_policy::casts_shadow(views.app.world(), ribbon),
                "the track ribbon casts in every view — off the sun's mask the controlled tank's \
                 tracks lose their shadow for as long as the sight is up"
            );
            assert!(
                !views.drawn(ribbon),
                "and no camera ever draws it, in either view"
            );
        }
        assert!(
            !views.drawn(hull),
            "the hull is still dropped by the optic — the shadow guarantee is not a blanket"
        );
    }

    /// **The bound is a CIRCLE of the authored optic span of the LARGER axis' half-extent, taken
    /// through the projection.** Stated in the tangents, where the shape lives, so a bound that
    /// went back to being linear in the angle — or that took the vertical axis when the horizontal
    /// one is larger — moves a number here.
    #[test]
    fn the_deflection_bound_is_the_projected_fraction_of_the_larger_axis() {
        let fov = Optics::Magnified {
            magnification: 2.5,
            field_deg: 25.0,
        }
        .vertical_fov();
        let half = (fov / 2.0).tan();
        for aspect in [16.0 / 9.0, 21.0 / 9.0, 4.0 / 3.0, 0.5625, 1.0] {
            let margin = optic_margin(fov, aspect);
            let larger = aspect.max(1.0);
            assert!(
                (margin.tan() - look::get().optic_span_fraction.0 * half * larger).abs() < 1e-7,
                "the bound at aspect {aspect} does not follow the authored larger-axis fraction",
            );
        }
    }

    /// **The deflection bound is DERIVED from the field the optic frames, never a stored angle and
    /// never its magnification.** A sight framing a wider field gives the cursor a wider reach; a
    /// bound that carried its own constant would sit still through all of it.
    #[test]
    fn the_deflection_bound_follows_the_field() {
        let bound = |magnification: f32, field_deg: f32| {
            optic_margin(
                Optics::Magnified {
                    magnification,
                    field_deg,
                }
                .vertical_fov(),
                16.0 / 9.0,
            )
        };
        let mut widest = 0.0_f32;
        for field_deg in [6.25_f32, 12.5, 25.0, 50.0] {
            let margin = bound(2.5, field_deg);
            assert!(
                margin > widest,
                "a {field_deg}° field must reach further than every narrower one",
            );
            widest = margin;
            // And the instrument's power is not a term in it.
            assert_eq!(margin, bound(12.0, field_deg));
        }
    }

    /// The yaw/pitch ↔ hull-local direction conversion round-trips: decomposing `local_dir`'s output
    /// recovers the original angles. This is the bridge that lets the optic resume the shared
    /// `aim::CommittedAim` (a point) into its yaw/pitch working form and republish it — it must be
    /// lossless over the reachable aim window, and scale-invariant (a committed far point decodes to
    /// the same bearing as its unit direction).
    #[test]
    fn intent_dir_round_trips() {
        // Sample the reachable window: yaw all the way round, pitch within ±80° (well inside the
        // atan2 branch where the decomposition inverts, |pitch| < 90°).
        for yaw_deg in [-170.0, -90.0, -30.0, 0.0, 45.0, 120.0, 179.0_f32] {
            for pitch_deg in [-80.0, -15.0, 0.0, 10.0, 60.0_f32] {
                let intent = GunnerIntent {
                    yaw: yaw_deg.to_radians(),
                    pitch: pitch_deg.to_radians(),
                };
                let dir = intent.local_dir();
                let back = GunnerIntent::from_local_dir(dir);
                assert!(
                    (shortest_angle(back.yaw - intent.yaw)).abs() < 1e-5,
                    "yaw round-trip at ({yaw_deg}, {pitch_deg})"
                );
                assert!(
                    (back.pitch - intent.pitch).abs() < 1e-5,
                    "pitch round-trip at ({yaw_deg}, {pitch_deg})"
                );
                // Scale-invariant: a far committed point decodes to the same angles as the unit dir.
                let far = GunnerIntent::from_local_dir(dir * 10_000.0);
                assert!((shortest_angle(far.yaw - intent.yaw)).abs() < 1e-5);
                assert!((far.pitch - intent.pitch).abs() < 1e-5);
            }
        }
    }

    /// Zero-input identity: resuming an existing commitment with NO mouse motion re-authors that
    /// ORIGINAL point verbatim and re-stores NOTHING, so a mode switch is identity on
    /// `aim::CommittedAim` and on the gun's lay — even when this frame's re-resolve would land
    /// somewhere else (the optic resolves from the mount, third person from the camera: different
    /// origins can see different geometry). Actual mouse input (or a fresh tank with no commitment)
    /// publishes AND re-stores the fresh resolve.
    #[test]
    fn zero_input_resume_is_identity() {
        // A commitment inherited from third person (a floor point ~50 m out)...
        let inherited = Vec3::new(0.0, -2.0, -50.0);
        // ...and what the optic's own resolve found this frame — deliberately different (e.g. a
        // crest between the mount and the inherited point occludes the lower ray).
        let resolved = Vec3::new(0.0, -1.0, -30.0);

        // No motion, existing commitment: re-author the original point, store nothing (identity).
        let held = resume_commit(Some(inherited), false, resolved);
        assert_eq!(
            held.command_aim, inherited,
            "zero input re-authors the ORIGINAL committed point — the gun does not move"
        );
        assert_eq!(
            held.store, None,
            "zero input leaves CommittedAim untouched (identity)"
        );

        // Player moved the mouse: the optic takes ownership of its own resolve and re-stores it.
        let moved = resume_commit(Some(inherited), true, resolved);
        assert_eq!(moved.command_aim, resolved);
        assert_eq!(moved.store, Some(resolved));

        // Fresh tank (no commitment): nothing to preserve, so the resolve seeded from the gun's lay
        // must be published AND stored to establish the commitment — even with zero input
        // (recirculation).
        let fresh = resume_commit(None, false, resolved);
        assert_eq!(fresh.command_aim, resolved);
        assert_eq!(fresh.store, Some(resolved));
    }

    /// Superelevation shifts the reachable sight-line pitch window down from the lay limits.
    #[test]
    fn superelevation_shifts_pitch_window() {
        let (min, max) = (-8.0_f32.to_radians(), 15.0_f32.to_radians());
        let theta = 0.01_f32;
        let clamped = clamp_to_travel(max, sight_pitch_limits(Some((min, max)), theta));

        assert!((clamped - (max - theta)).abs() < 1e-6);
        assert!((clamped + theta - max).abs() < 1e-6);
    }
}
