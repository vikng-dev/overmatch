//! Gunner sight input and aim-point commitment.
//!
//! The presentation half — every HUD widget that draws the sight picture — lives behind
//! [`reticle`], which this module mounts and otherwise touches only to raise a [`Toast`].

mod reticle;

use avian3d::prelude::{Position, Rotation, SpatialQuery};
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::math::Affine3A;
use bevy::prelude::*;

use crate::aim::{AimCommit, CommittedAim, MAX_RANGE, aim_distance};
use crate::camera::{GUNNER_FOV_FALLBACK, view_fov};
use crate::command::{AimIntent, TankCommand, gather_commands};
use crate::damage::{ControlledTank, VolumeOf};
use crate::firecontrol::{RangeTable, Ranging};
use crate::render_policy::{CameraProfile, VisualScope};
use crate::spec::ViewKind;
use crate::state::{GameplaySet, PlayerInputSet};
use crate::tank::{
    Controlled, RemoteServos, ServoIndex, ServoSpec, ServoState, Tank, TankServos, TankViews,
    rig_world_pose, shortest_angle,
};

use reticle::Toast;

/// Whether the controlled tank's `kind` view is usable — its authored `requires` met (a dead
/// gunner closes the optic, a dead commander closes third-person). A missing view is unusable.
fn view_available(
    controlled: &ControlledTank,
    views: &Query<&TankViews, With<Controlled>>,
    kind: ViewKind,
) -> bool {
    views
        .single()
        .ok()
        .and_then(|v| v.0.get(&kind))
        .is_some_and(|config| controlled.meets(&config.requires))
}

/// Which view the player is in. Default is the third-person commander view.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Default)]
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

/// **Where the optic camera rides between the gun and the player's intent** — one continuous knob,
/// consulted ONLY while [`SightMode::Gunner`], and a pure VIEW parameter: the gun is commanded by
/// [`drive_gunner_aim`] and laid by `aim::drive_aim_servos` at every value, identically.
///
/// `0` welds the look to the gun's SIGHT LINE (lay − superelevation), so the intent cursor leads
/// inside the optic circle and drifts back as the gun catches up. `1` welds it to the committed
/// intent, so the cursor holds the centre of the glass and the gun's bore visibly lags behind it
/// within the same circle. Everything between is a geometric blend of the two bearings — no
/// damping and no spring, so the aperture is instantaneous, stateless, and cannot overshoot.
///
/// Cycle the ladder live with `V` to compare feel.
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
pub struct GunnerBlend(pub f32);

/// The `V` hotkey's rungs, gun-end first. Playtest knob: the endpoints are the two cameras this
/// collapsed, the interior is the hypothesis.
pub(crate) const GUNNER_BLEND_LADDER: [f32; 5] = [0.0, 0.35, 0.5, 0.65, 1.0];

impl Default for GunnerBlend {
    fn default() -> Self {
        Self(GUNNER_BLEND_LADDER[2])
    }
}

impl GunnerBlend {
    /// The next rung, wrapping. A value off the ladder (only reachable by a caller that set one)
    /// resumes at the gun end.
    fn next(self) -> Self {
        let next = GUNNER_BLEND_LADDER
            .iter()
            .position(|&k| k == self.0)
            .map_or(0, |rung| (rung + 1) % GUNNER_BLEND_LADDER.len());
        Self(GUNNER_BLEND_LADDER[next])
    }
}

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

/// Per-frame yaw/pitch working form of the committed aim point.
///
/// Invariant: decompose `point - mount`, never the hull origin, to keep the optic and servo target
/// at the same parallax origin. [`CommittedAim`] remains the sole persistent state.
#[derive(Clone, Copy)]
struct GunnerIntent {
    yaw: f32,
    pitch: f32,
}

impl GunnerIntent {
    /// Intent as a unit direction in hull-local space.
    fn local_dir(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(-sy * cp, sp, -cy * cp)
    }

    /// Inverse of [`local_dir`](Self::local_dir); callers provide the mount-relative direction.
    fn from_hull_local_dir(dir: Vec3) -> Self {
        Self {
            yaw: (-dir.x).atan2(-dir.z),
            pitch: dir.y.atan2((dir.x * dir.x + dir.z * dir.z).sqrt()),
        }
    }
}

pub fn plugin(app: &mut App) {
    app.init_resource::<SightMode>()
        // Where the optic camera rides between the gun and the intent.
        .init_resource::<GunnerBlend>()
        // Every HUD widget the sight draws, and the systems that keep them in step. The presentation
        // half orders itself against `toggle_sight` below.
        .add_plugins(reticle::plugin)
        .add_systems(
            Update,
            // Only the Lshift view toggle is player input (gated on the cursor); the overlay, toast,
            // and range readout are presentation and keep updating with the cursor free.
            toggle_sight
                .in_set(PlayerInputSet)
                .in_set(SightToggled)
                .in_set(GameplaySet),
        )
        // Playtest knob: cycle where the optic camera rides between gun and intent (`V`). The camera
        // reads it fresh every frame and holds no state, so a change needs no reseed.
        .add_systems(
            Update,
            cycle_gunner_blend
                .in_set(PlayerInputSet)
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
    let subject = controlled.single().ok();
    for (tank, mut scope) in &mut tanks {
        scope.set_if_neq(if Some(tank) == subject {
            VisualScope::VIEW_SUBJECT_BODY
        } else {
            VisualScope::WORLD_SOLID
        });
    }
}

/// Point the one 3D camera at the channel set its current view wants.
///
/// This is the entire "hide the player's own tank in the gunner optic" mechanism now: ONE
/// component on ONE entity, O(1) in the size of the world. The optic profile drops
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
        profile.set_if_neq(want);
    }
}

/// Cursor radius as a fraction of half the vertical FOV: `margin = fraction * fov / 2`.
///
/// This is the AIMING bound, in every mask style — no style of surround is an input to it. Only the
/// `Aperture` mask draws its rim on this same angle, and there the drawn glass IS the reachable set;
/// `Framed` sizes its circle off the viewport instead, so it contains the bound without indicating
/// it (`reticle::MaskStyle`).
pub const OPTIC_RADIUS_FRACTION: f32 = 0.9;

/// Angular cursor radius for vertical FOV `fov`.
pub(crate) fn optic_margin(fov: f32) -> f32 {
    OPTIC_RADIUS_FRACTION * (fov / 2.0)
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

/// A servo's live parent-local lay, addressed by its [`ServoIndex`] slot in the tank's root-resident
/// integrator. Same preference the mechanism itself integrates through (`tank::servo`): the
/// client-local [`RemoteServos`] when the tank carries one — every replica, the own hull
/// included — else the authoritative [`TankServos`] snapshot. The
/// optic clamps intent against the gun's live lay, so it must read the integrator that is moving it.
fn live_servo_angle(
    tank: Entity,
    slot: &ServoIndex,
    states: &Query<&TankServos>,
    remote: &Query<&RemoteServos>,
) -> Option<f32> {
    remote
        .get(tank)
        .ok()
        .and_then(|servos| servos.0.get(slot.0))
        .or_else(|| {
            states
                .get(tank)
                .ok()
                .and_then(|servos| servos.states.get(slot.0))
        })
        .map(ServoState::current)
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
pub(crate) fn drive_gunner_aim(
    motion: Res<AccumulatedMouseMotion>,
    spatial: SpatialQuery,
    mut committed: ResMut<CommittedAim>,
    controlled: ControlledTank,
    views: Query<&TankViews, With<Controlled>>,
    servo_slots: Query<&ServoIndex>,
    servo_specs: Query<&ServoSpec>,
    servo_states: Query<&TankServos>,
    remote_servos: Query<&RemoteServos>,
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

    // Treat non-finite memory as absent so a fresh finite resolve can replace it.
    let committed_point = committed.get(tank).filter(|point| point.is_finite());
    let moved = motion.delta != Vec2::ZERO;

    // Fast path before pose guards so [`resume_commit`]'s preserved point is always re-authored.
    if let Some(point) = committed_point
        && !moved
    {
        if let Ok(mut command) = tank_commands.get_mut(tank) {
            command.aim = Some(AimIntent::HullLocal(point));
        }
        return;
    }

    // The field the view's authored optic frames (`spec::Optics`) sets the cursor's reach — the
    // margin is a fixed fraction of the half-FOV, so the travel circle IS the drawn optic rim.
    // Fallback mirrors `camera.rs` for the pre-bind frame before `TankViews` lands.
    let fov = view_fov(&views, ViewKind::Gunner, GUNNER_FOV_FALLBACK);
    let margin = optic_margin(fov);

    // Radians of commanded aim per mouse count, per radian of vertical FOV. What the knob tunes is
    // the cursor's SCREEN travel, so scaling with the field holds the count of mouse-counts needed
    // to cross the optic the same in every instrument (a narrower field puts the same screen move
    // over a smaller angle). TUNED at 0.0005 rad/count against a 0.12 rad field.
    const SENSITIVITY_PER_FOV: f32 = 0.0005 / 0.12;
    let sensitivity = SENSITIVITY_PER_FOV * fov;

    // The margin below clamps intent against the gun's live lay — see [`live_servo_angle`].
    let angle = |servo| {
        servo_slots
            .get(servo)
            .ok()
            .and_then(|slot| live_servo_angle(tank, slot, &servo_states, &remote_servos))
    };
    let Some(t_current) = angle(rig.turret) else {
        return;
    };
    let Some(g_current) = angle(rig.gun) else {
        return;
    };

    // Superelevation for the dialed range; the gun's live pitch carries it, so the sight line (which
    // the intent tracks) is the gun's lay minus the lob. One binding for that scalar: the seed and
    // the circular clamp below must measure from the same line, or the intent settles off the mark
    // it was clamped against.
    let theta = tables
        .get(rig.muzzle)
        .map_or(0.0, |table| table.superelevation(ranging.range));
    let sight_now = g_current - theta;

    // The sight's origin: the gun mount (elevation pivot), from the SAME physics-truth chain
    // `aim::drive_aim_servos` lays from (`rig_world_pose`, never `GlobalTransform`), so the
    // decomposition below, the clamps against the live lay, and the servo convergence all measure
    // their angles from one origin. The hull frame anchors the committed point's local form.
    let Ok((root_position, root_rotation)) = poses.get(tank) else {
        return;
    };
    let Some((hull_position, hull_rotation)) = rig_world_pose(
        rig.hull,
        tank,
        root_position.0,
        root_rotation.0,
        &parents,
        &locals,
    ) else {
        return;
    };
    let Some((mount_world, _)) = rig_world_pose(
        rig.gun,
        tank,
        root_position.0,
        root_rotation.0,
        &parents,
        &locals,
    ) else {
        return;
    };
    let hull_affine = Affine3A::from_rotation_translation(hull_rotation, hull_position);
    let mount_local = hull_affine.inverse().transform_point3(mount_world);
    // NaN discipline for the resolve inputs: a poisoned pose frame must reach neither the raycast
    // nor the store — a non-finite resolve would poison the shared memory itself. Skip the frame
    // (the fast path above has already re-authored a held commitment; a fresh tank skips one seed
    // frame). `mount_local` finite implies the hull affine is too, so `dir_world` below stays
    // finite whenever these pass.
    if !(mount_world.is_finite() && mount_local.is_finite()) {
        return;
    }

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
        .map(|point| GunnerIntent::from_hull_local_dir(point - mount_local))
        .unwrap_or(GunnerIntent {
            yaw: t_current,
            pitch: sight_now,
        });

    intent.yaw -= motion.delta.x * sensitivity;
    intent.pitch -= motion.delta.y * sensitivity;

    // Bound 1 — mechanical travel. The pitch (elevation) servo's limits are on the *lay*; the intent
    // is the *sight line* = lay − θ, so shift the window down by the lob. The Tiger's turret is
    // `Continuous` (yaw passes through); a limited-traverse turret would clamp yaw directly (no lob
    // on yaw). Clamping the absolute intent here — before the circular clamp — guarantees the final
    // intent is reachable, so the reticle always has an angle it can settle onto.
    let pitch_limits = sight_pitch_limits(
        servo_specs
            .get(rig.gun)
            .ok()
            .and_then(ServoSpec::travel_limits),
        theta,
    );
    let yaw_limits = servo_specs
        .get(rig.turret)
        .ok()
        .and_then(ServoSpec::travel_limits);
    intent.pitch = clamp_to_travel(intent.pitch, pitch_limits);
    intent.yaw = clamp_to_travel(intent.yaw, yaw_limits);

    // Bound 2 — circular optic margin. Lead as a 2D angular vector from the gun chain's current
    // *sight line* (lay − lob). Yaw uses shortest-angle difference so continuous traverse doesn't
    // wind up. `drive_servos` steps on the fixed clock, so `current` here is the latest tick's
    // integrated angle — the clamp chases the sim truth, ≤1 tick behind the render pose the optic
    // shows. Preserve direction, cap magnitude at the optic radius; within the margin the intent is
    // untouched (see the doc comment — re-pinning would make the target recede with the gun). The
    // interpolation stays inside the travel window (both endpoints are, and scale ∈ [0, 1]).
    let yaw_offset = shortest_angle(intent.yaw - t_current);
    let pitch_offset = intent.pitch - sight_now;
    let len = (yaw_offset * yaw_offset + pitch_offset * pitch_offset).sqrt();
    if len > margin {
        let scale = margin / len;
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
    // Fallible direction: a NaN-poisoned pose or committed value this frame (rollback edge) must
    // not be resolved and re-stored — that would poison the shared memory itself. Skip the frame,
    // the same idiom as the bore dot and `drive_aim_servos`' non-finite hold.
    let Ok(dir_world) = Dir3::new(hull_rotation * dir_local) else {
        return;
    };
    let distance = aim_distance(
        &spatial,
        Ray3d::new(mount_world, dir_world),
        MAX_RANGE,
        tank,
        &volumes,
        &parents,
    );
    let resolved = mount_local + dir_local * distance;

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
    if let Ok(mut command) = tank_commands.get_mut(tank) {
        command.aim = Some(AimIntent::HullLocal(publish.command_aim));
    }
}

/// Live playtest knob: step [`GunnerBlend`] along its ladder with `V` and name the value on-screen.
///
/// The blend only *matters* in gunner view, but cycling is allowed from anywhere so a playtester can
/// pre-pick; the toast is the feedback. The camera reads the resource fresh each frame and keeps no
/// state of its own, so a change takes effect the same frame with nothing to reseed.
fn cycle_gunner_blend(
    keys: Res<ButtonInput<KeyCode>>,
    mut blend: ResMut<GunnerBlend>,
    mut toast: ResMut<Toast>,
) {
    if !keys.just_pressed(KeyCode::KeyV) {
        return;
    }
    *blend = blend.next();
    toast.show(format!("Optic blend k = {:.2}", blend.0));
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// The `V` ladder is a cycle over the two collapsed endpoints and the interval between them: it
    /// starts on neither end, reaches both, and returns — so a playtester can walk it in one
    /// direction and land back where they began without hunting.
    #[test]
    fn the_blend_ladder_cycles_through_both_endpoints() {
        let start = GunnerBlend::default();
        let mut walk = vec![start.0];
        let mut blend = start;
        for _ in 1..GUNNER_BLEND_LADDER.len() {
            blend = blend.next();
            walk.push(blend.0);
        }
        assert_eq!(
            blend.next(),
            start,
            "the ladder closes on its starting rung"
        );
        walk.sort_by(f32::total_cmp);
        assert_eq!(walk, GUNNER_BLEND_LADDER, "the walk visits every rung once");
        assert!(
            start.0 > 0.0 && start.0 < 1.0,
            "the default must be an interior rung — a knob that ships on one of its own endpoints \
             is the pair of cameras it replaced, not a blend",
        );
    }

    /// The margin is a fraction of the half-FOV and nothing else, so the cursor's travel circle and
    /// the drawn rim stay one radius at any field.
    #[test]
    fn margin_is_fraction_of_half_fov() {
        assert!((optic_margin(0.12) - 0.054).abs() < 1e-6);
        // Scales with the field: a wider one gets a proportionally wider reach.
        assert!((optic_margin(0.24) - 2.0 * optic_margin(0.12)).abs() < 1e-9);
    }

    /// **The deflection bound is DERIVED from the field the optic frames, never a stored angle and
    /// never its magnification.** A sight framing twice the field gives the cursor twice the reach;
    /// a bound that carried its own constant would sit still through all of it.
    #[test]
    fn the_deflection_bound_follows_the_field() {
        let bound = |magnification: f32, field_deg: f32| {
            optic_margin(
                Optics::Magnified {
                    magnification,
                    field_deg,
                }
                .vertical_fov(),
            )
        };
        for field_deg in [6.25_f32, 12.5, 25.0, 50.0] {
            // Proportional, which is the fixed fraction of the half-field carried through.
            assert!(
                (bound(2.5, field_deg) * 25.0 - bound(2.5, 25.0) * field_deg).abs() < 1e-6,
                "the bound over {field_deg}° is not the 25° bound scaled by the field",
            );
            // And the instrument's power is not a term in it.
            assert_eq!(bound(2.5, field_deg), bound(12.0, field_deg));
        }
        // One worked instance, as fixture data: the TZF 9b frames 25°, so the cursor reaches
        // 0.9 × 12.5°.
        assert!((bound(2.5, 25.0).to_degrees() - 11.25).abs() < 1e-3);
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
                let back = GunnerIntent::from_hull_local_dir(dir);
                assert!(
                    (shortest_angle(back.yaw - intent.yaw)).abs() < 1e-5,
                    "yaw round-trip at ({yaw_deg}, {pitch_deg})"
                );
                assert!(
                    (back.pitch - intent.pitch).abs() < 1e-5,
                    "pitch round-trip at ({yaw_deg}, {pitch_deg})"
                );
                // Scale-invariant: a far committed point decodes to the same angles as the unit dir.
                let far = GunnerIntent::from_hull_local_dir(dir * 10_000.0);
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

    /// The resume measures the committed point's bearing from the MOUNT, and the resolve
    /// (`mount + dir · t`) inverts it exactly. Decomposing the raw point instead would measure from
    /// the hull-frame origin — ~2.2 m below the mount at ground level — and a near floor aim's
    /// bearing differs between the two by the mount parallax (~2.5° at 50 m, most of the 3.1° optic
    /// radius under magnification): the "aim snaps much higher on first optic input" regression.
    #[test]
    fn resume_measures_bearing_from_the_mount() {
        // The Tiger's geometry: gun pivot ~2.2 m above the hull-frame origin, floor point 50 m out
        // at ground level (hull origin ≈ ground).
        let mount = Vec3::new(0.0, 2.2171, -1.100);
        let point = Vec3::new(0.0, 0.0, -50.0);

        let intent = GunnerIntent::from_hull_local_dir(point - mount);
        // The true sight line from the mount is depressed ~2.6°; the hull-origin bearing is 0°.
        let expected = (-(mount.y)).atan2((point - mount).xz().length());
        assert!(
            (intent.pitch - expected).abs() < 1e-6,
            "sight-line pitch from the mount, got {}",
            intent.pitch
        );
        assert!(
            intent.pitch < -0.04,
            "a near floor aim depresses the sight line — a ~0 pitch means the decomposition \
             regressed to the hull origin"
        );

        // Resolving along that bearing from the mount lands back on the committed point: the
        // resume↔resolve pair round-trips, so the intent never drifts frame to frame.
        let distance = (point - mount).length();
        let resolved = mount + intent.local_dir() * distance;
        assert!(
            (resolved - point).length() < 1e-4,
            "resolve should invert the resume, got {resolved}"
        );
    }

    /// A continuous mount (turret yaw) passes through; a limited mount clamps to its window. This is
    /// the raw clamp — the caller shifts the pitch window down by the superelevation before calling.
    #[test]
    fn travel_clamp_respects_limits() {
        assert_eq!(clamp_to_travel(3.0, None), 3.0);
        let limits = Some((-8.0_f32.to_radians(), 15.0_f32.to_radians()));
        // Below the floor and above the ceiling saturate; an in-window angle is untouched.
        assert!((clamp_to_travel(-1.0, limits) - (-8.0_f32).to_radians()).abs() < 1e-6);
        assert!((clamp_to_travel(1.0, limits) - 15.0_f32.to_radians()).abs() < 1e-6);
        assert!((clamp_to_travel(0.1, limits) - 0.1).abs() < 1e-9);
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
