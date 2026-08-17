//! Gunner sight input and aim-point commitment.
//!
//! The presentation half — every HUD widget that draws the sight picture — lives behind
//! [`reticle`], which this module mounts and otherwise touches only to raise a [`Toast`].

mod reticle;

use avian3d::prelude::{Position, Rotation, SpatialQuery};
use bevy::ecs::system::SystemParam;
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

/// **Which gunner-view *handling* scheme is active** — an A/B harness knob, orthogonal to
/// [`SightMode`] and consulted ONLY while `SightMode::Gunner`. All four schemes resolve to the same
/// shared [`aim::CommittedAim`] point and drive the gun through the same authority-side servo path
/// (`aim::drive_aim_servos`) — the gun *command* machinery is identical and untouched. They differ
/// purely in the CLIENT/VIEW layer: where the camera sits and how it tracks the gun, and how the
/// mouse maps to that one committed point. Cycle live with `V` to compare feel. Default is the
/// shipped baseline (`BoundOptic`).
#[derive(Resource, Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum GunnerScheme {
    /// **A** — camera bolted rigidly to the gun's sight line; mouse = bounded deflection inside the
    /// ~3° optic circle. The reticle leads within the circle and drifts back as the gun catches up.
    /// The one rigid body. (`camera::gunner_camera` + [`drive_gunner_aim`].)
    #[default]
    BoundOptic,
    /// **B** — free-look camera at the mount; mouse points a free reticle (screen centre), the gun
    /// chases it at servo rate with no circle. Two visible bodies: your look (instant) and the gun
    /// (lagging). WoT "camera dictates intent". (`camera::free_aim_camera` + [`drive_free_aim`].)
    FreeReticle,
    /// **C** — the camera's look *damps* toward the mouse and the gun trails the camera; the gun-bore
    /// is the swimming reticle you settle on target. Camera and gun glide relative to one another —
    /// the War Thunder Realistic feel. (`camera::free_aim_camera` + [`drive_free_aim`], damped.)
    DecoupledOptic,
    /// **D** *(novel)* — A's exact aiming (same bounded commit), but the camera is an underdamped
    /// elastic spring toward the aim intent instead of a rigid bolt: the view whips ahead and settles
    /// while the gun grinds underneath, so three inertias (mouse → camera → gun) are legible. Pure
    /// view-juice — the spring never touches the gun command. (`camera::elastic_bore_camera` +
    /// [`drive_gunner_aim`].)
    ElasticBore,
    /// **E** *(camera follows the orange intent dot)* — A's exact bounded aiming, but the camera locks
    /// to the *committed intent* (the orange lead cursor) instead of the gun. The view is crisp and
    /// responsive — the orange dot sits at screen centre — while the gun bore (green) lags *behind* it,
    /// visibly catching up within the same 3° circle. The instant-camera sibling of D (no spring) and
    /// the inverse of A (which welds the camera to the gun and lets the dot lead a laggy view). Camera
    /// only — the commit is unchanged. (`camera::lead_optic_camera` + [`drive_gunner_aim`].)
    LeadOptic,
}

impl GunnerScheme {
    /// On-screen name for the switch toast, so a playtester can name what they are feeling.
    fn label(self) -> &'static str {
        match self {
            GunnerScheme::BoundOptic => "A — Bound Optic",
            GunnerScheme::FreeReticle => "B — Free Reticle",
            GunnerScheme::DecoupledOptic => "C — Decoupled Optic",
            GunnerScheme::ElasticBore => "D — Elastic Bore",
            GunnerScheme::LeadOptic => "E — Lead Optic",
        }
    }

    /// The cycle order for the `V` hotkey: A → B → C → D → E → A.
    fn next(self) -> Self {
        match self {
            GunnerScheme::BoundOptic => GunnerScheme::FreeReticle,
            GunnerScheme::FreeReticle => GunnerScheme::DecoupledOptic,
            GunnerScheme::DecoupledOptic => GunnerScheme::ElasticBore,
            GunnerScheme::ElasticBore => GunnerScheme::LeadOptic,
            GunnerScheme::LeadOptic => GunnerScheme::BoundOptic,
        }
    }

    /// A, D and E share the bounded-deflection commit ([`drive_gunner_aim`]); the gun is commanded
    /// identically (they differ only in where the camera rides — the gun, a spring, or the intent).
    pub fn bounded_commit(self) -> bool {
        matches!(
            self,
            GunnerScheme::BoundOptic | GunnerScheme::ElasticBore | GunnerScheme::LeadOptic
        )
    }

    /// B and C share the mouse-driven look camera + screen-centre commit ([`drive_free_aim`]).
    pub fn free_look(self) -> bool {
        matches!(
            self,
            GunnerScheme::FreeReticle | GunnerScheme::DecoupledOptic
        )
    }
}

/// Run condition: gunner optic active AND scheme A (the rigid bolt camera).
pub fn in_gunner_bound(mode: Res<SightMode>, scheme: Res<GunnerScheme>) -> bool {
    *mode == SightMode::Gunner && *scheme == GunnerScheme::BoundOptic
}

/// Run condition: gunner optic active AND scheme D (the elastic-spring camera).
pub fn in_gunner_elastic(mode: Res<SightMode>, scheme: Res<GunnerScheme>) -> bool {
    *mode == SightMode::Gunner && *scheme == GunnerScheme::ElasticBore
}

/// Run condition: gunner optic active AND scheme E (camera locked to the intent/orange dot).
pub fn in_gunner_lead(mode: Res<SightMode>, scheme: Res<GunnerScheme>) -> bool {
    *mode == SightMode::Gunner && *scheme == GunnerScheme::LeadOptic
}

/// Run condition: gunner optic active AND a free-look scheme (B or C) — one camera + one commit,
/// parameterized by the scheme.
pub fn in_gunner_free_look(mode: Res<SightMode>, scheme: Res<GunnerScheme>) -> bool {
    *mode == SightMode::Gunner && scheme.free_look()
}

/// Run condition: gunner optic active AND a bounded-deflection scheme (A or D) — both drive
/// [`drive_gunner_aim`]. Mutually exclusive with [`in_gunner_free_look`], so exactly one gunner
/// commit runs per frame and the single-writer invariant on [`aim::CommittedAim`] holds.
pub fn in_gunner_bounded_commit(mode: Res<SightMode>, scheme: Res<GunnerScheme>) -> bool {
    *mode == SightMode::Gunner && scheme.bounded_commit()
}

/// View-layer camera-look state for the free-look schemes (B/C).
///
/// This is the *camera's* aim, NOT the gun's: [`aim::CommittedAim`] remains the sole gun-command
/// memory (the "no second aim memory" invariant). `target_*` is the raw mouse-integrated intent;
/// `yaw`/`pitch` is the look the camera actually shows — equal to the target for B (instant), and an
/// eased follow of it for C (damped). Both are hull-local (they ride the tank), decomposed exactly
/// as [`GunnerIntent`]. `seeded` is cleared on scheme/mode entry ([`invalidate_gunner_view_state`])
/// so the first frame reseeds from the current committed aim and the view never jumps.
#[derive(Resource, Default)]
pub struct GunnerFreeAim {
    pub target_yaw: f32,
    pub target_pitch: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub seeded: bool,
}

/// View-layer spring state for the elastic-bore camera (D): the camera's current look (`yaw`/`pitch`,
/// hull-local) and its angular velocity, integrated as an underdamped harmonic oscillator toward the
/// committed aim (intent) direction in `camera::elastic_bore_camera`. Like [`GunnerFreeAim`] this is
/// camera-only — it never feeds the gun command — and reseeds on entry.
#[derive(Resource, Default)]
pub struct ElasticCam {
    pub yaw: f32,
    pub pitch: f32,
    pub vel_yaw: f32,
    pub vel_pitch: f32,
    pub seeded: bool,
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
        // A/B harness: the active gunner-view scheme + the two free-look/elastic camera view-states.
        .init_resource::<GunnerScheme>()
        .init_resource::<GunnerFreeAim>()
        .init_resource::<ElasticCam>()
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
        // A/B harness: cycle the gunner-view scheme (`V`), then reseed the camera view-state on any
        // scheme/mode change so the switch never snaps the view.
        .add_systems(
            Update,
            cycle_gunner_scheme
                .in_set(PlayerInputSet)
                .in_set(GameplaySet),
        )
        .add_systems(
            Update,
            invalidate_gunner_view_state
                .run_if(gunner_view_context_changed)
                .after(cycle_gunner_scheme)
                .after(toggle_sight)
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
            // The active gunner commit (A/B harness): the bounded-deflection commit for schemes A+D,
            // or the free-look commit for schemes B+C. Their run conditions are mutually exclusive, so
            // exactly one authors `CommittedAim` per frame (the single-writer invariant).
            (
                drive_gunner_aim.run_if(in_gunner_bounded_commit),
                drive_free_aim.run_if(in_gunner_free_look),
            )
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
/// Invariant: sight input and the optic overlay share this constant.
pub const OPTIC_RADIUS_FRACTION: f32 = 0.9;

/// Angular cursor radius for vertical FOV `fov`.
fn optic_margin(fov: f32) -> f32 {
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

    // The optic's authored vertical FOV (per-tank) sets both the magnification and the cursor's
    // reach — the margin is a fixed fraction of the half-FOV, so the travel circle IS the drawn
    // optic rim. Fallback mirrors `camera.rs` for the pre-bind frame before `TankViews` lands.
    let fov = view_fov(&views, ViewKind::Gunner, GUNNER_FOV_FALLBACK);
    let margin = optic_margin(fov);

    // Radians of commanded aim per mouse count, scaled with the optic FOV so the screen-space cursor
    // feel — and the count of mouse-counts to cross the optic — is magnification-invariant (a
    // narrower optic magnifies, so the same screen move is a smaller angle). Anchored so the
    // reference 0.12 rad optic keeps its tuned 0.0005 (this retires the old "scale with the zoom
    // FOV" note); with one authored gunner FOV today it is a no-op, and correct the moment a second
    // optic exists.
    const SENSITIVITY_AT_REF: f32 = 0.0005;
    const REF_FOV: f32 = 0.12;
    let sensitivity = SENSITIVITY_AT_REF * (fov / REF_FOV);

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
    // the intent tracks) is the gun's lay minus the lob.
    let theta = tables
        .get(rig.muzzle)
        .map_or(0.0, |table| table.superelevation(ranging.range));

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
            pitch: g_current - theta,
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
    let sight_now = g_current - theta;
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

/// Scheme B's wide "camera dictates intent" FOV (radians) — a gunnery view, not the magnified optic.
/// Shared with `camera::free_aim_camera` so the sensitivity (here) and the magnification (there) agree.
pub(crate) const FREE_RETICLE_FOV: f32 = 0.6;

/// Scheme C's look-ease rate (1/s): higher tracks the mouse more tightly (less decoupled), lower
/// glides more. The gun chases this eased look, so it is also the aim lag. Feel knob, tuned in playtest.
const DECOUPLED_LOOK_GLIDE: f32 = 16.0;

/// Live A/B switch: cycle the gunner-view scheme with `V` and name it on-screen.
///
/// The scheme only *matters* in gunner view, but cycling is allowed from anywhere so a playtester can
/// pre-pick; the toast is the feedback. The camera/commit swap itself is handled by the per-scheme run
/// conditions — this only flips the resource, and the change reseeds the view state
/// ([`invalidate_gunner_view_state`]).
fn cycle_gunner_scheme(
    keys: Res<ButtonInput<KeyCode>>,
    mut scheme: ResMut<GunnerScheme>,
    mut toast: ResMut<Toast>,
) {
    if !keys.just_pressed(KeyCode::KeyV) {
        return;
    }
    *scheme = scheme.next();
    toast.show(format!("Sight: {}", scheme.label()));
}

/// Run condition: the gunner-view context (scheme or sight mode) changed this frame.
fn gunner_view_context_changed(mode: Res<SightMode>, scheme: Res<GunnerScheme>) -> bool {
    mode.is_changed() || scheme.is_changed()
}

/// Clear the free-look / elastic view-state seed whenever the scheme or sight mode changes, so the
/// next frame reseeds the camera look from the current committed aim — the view continues from
/// wherever the outgoing scheme was aimed instead of snapping. The gun command itself is already
/// continuous through the shared [`aim::CommittedAim`]; this only keeps the *view* seamless.
fn invalidate_gunner_view_state(mut free: ResMut<GunnerFreeAim>, mut elastic: ResMut<ElasticCam>) {
    free.seeded = false;
    elastic.seeded = false;
}

/// Commit for the free-look schemes (B/C): the mouse drives the *camera's* look, and the gun chases
/// wherever the camera centre points. Writes the shared [`aim::CommittedAim`] every frame (the
/// recirculation invariant) and the camera's look into [`GunnerFreeAim`] for `camera::free_aim_camera`
/// to read this same frame. B is instant (look = target); C damps the look toward the target so the
/// camera — and thus the gun that chases it — glides. No optic circle: the only bound is the gun's
/// mechanical travel, so an out-of-reach look just leaves the gun chasing at its limit (the WoWS
/// "aiming blockade"). Runs in the same `BeforeFixedMainLoop` slot as [`drive_gunner_aim`], mutually
/// exclusive with it by run condition, so exactly one gunner commit authors `CommittedAim` per frame.
/// The servo + ranging context [`drive_free_aim`] needs, bundled into one [`SystemParam`] so the
/// system stays under Bevy's 16-argument limit (it mirrors the fields [`drive_gunner_aim`] takes
/// loose).
#[derive(SystemParam)]
pub(crate) struct FreeAimServos<'w, 's> {
    slots: Query<'w, 's, &'static ServoIndex>,
    specs: Query<'w, 's, &'static ServoSpec>,
    states: Query<'w, 's, &'static TankServos>,
    remote: Query<'w, 's, &'static RemoteServos>,
    tables: Query<'w, 's, &'static RangeTable>,
    ranging: Res<'w, Ranging>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn drive_free_aim(
    motion: Res<AccumulatedMouseMotion>,
    time: Res<Time>,
    scheme: Res<GunnerScheme>,
    mut free: ResMut<GunnerFreeAim>,
    spatial: SpatialQuery,
    mut committed: ResMut<CommittedAim>,
    controlled: ControlledTank,
    views: Query<&TankViews, With<Controlled>>,
    servos: FreeAimServos,
    poses: Query<(&Position, &Rotation)>,
    parents: Query<&ChildOf>,
    locals: Query<&Transform>,
    volumes: Query<&VolumeOf>,
    mut tank_commands: Query<&mut TankCommand>,
) {
    let (Some(tank), Some(rig)) = (controlled.entity(), controlled.rig()) else {
        return;
    };

    // Optic FOV drives both the magnification (camera) and the mouse sensitivity (a
    // magnification-invariant screen feel). B is a wide gunnery view; C the authored magnified optic.
    let fov = if *scheme == GunnerScheme::FreeReticle {
        FREE_RETICLE_FOV
    } else {
        view_fov(&views, ViewKind::Gunner, GUNNER_FOV_FALLBACK)
    };
    const SENSITIVITY_AT_REF: f32 = 0.0005;
    const REF_FOV: f32 = 0.12;
    let sensitivity = SENSITIVITY_AT_REF * (fov / REF_FOV);

    // Tick-truth mount + hull pose (the SAME `rig_world_pose` chain `aim::drive_aim_servos` lays from,
    // never a render `GlobalTransform`), so the resolve, the store, and the servo convergence all
    // measure from one origin.
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
    if !(mount_world.is_finite() && mount_local.is_finite()) {
        return;
    }

    let theta = servos
        .tables
        .get(rig.muzzle)
        .map_or(0.0, |table| table.superelevation(servos.ranging.range));

    // Seed the look on entry (scheme/mode change cleared `seeded`): continue from the current
    // committed aim so the camera does not jump; a fresh tank with no commitment seeds from the gun's
    // current lay (sight line = lay − lob).
    if !free.seeded {
        let (yaw, pitch) = match committed.get(tank).filter(|point| point.is_finite()) {
            Some(point) => yaw_pitch_of(point - mount_local),
            None => {
                let angle = |servo| {
                    servos.slots.get(servo).ok().and_then(|slot| {
                        live_servo_angle(tank, slot, &servos.states, &servos.remote)
                    })
                };
                (
                    angle(rig.turret).unwrap_or(0.0),
                    angle(rig.gun).unwrap_or(0.0) - theta,
                )
            }
        };
        free.target_yaw = yaw;
        free.target_pitch = pitch;
        free.yaw = yaw;
        free.pitch = pitch;
        free.seeded = true;
    }

    // Integrate the raw target from the mouse (absolute intent, position control).
    free.target_yaw -= motion.delta.x * sensitivity;
    free.target_pitch -= motion.delta.y * sensitivity;

    // Bound 1 — mechanical travel only (no optic circle for free-look). Pitch tracks the sight line =
    // lay − θ, so shift the elevation window down by the lob; a limited turret clamps yaw, a
    // continuous one passes through.
    let pitch_limits = sight_pitch_limits(
        servos
            .specs
            .get(rig.gun)
            .ok()
            .and_then(ServoSpec::travel_limits),
        theta,
    );
    let yaw_limits = servos
        .specs
        .get(rig.turret)
        .ok()
        .and_then(ServoSpec::travel_limits);
    free.target_pitch = clamp_to_travel(free.target_pitch, pitch_limits);
    free.target_yaw = clamp_to_travel(free.target_yaw, yaw_limits);

    // The look the camera shows: B snaps to the target; C eases toward it. The gun chases this eased
    // look, so C's camera lag becomes aim lag — the decoupled glide.
    if *scheme == GunnerScheme::DecoupledOptic {
        let ease = 1.0 - (-DECOUPLED_LOOK_GLIDE * time.delta_secs()).exp();
        free.yaw += shortest_angle(free.target_yaw - free.yaw) * ease;
        free.pitch += (free.target_pitch - free.pitch) * ease;
    } else {
        free.yaw = free.target_yaw;
        free.pitch = free.target_pitch;
    }

    // Resolve the look ray from the mount → world hit (or far fallback), store hull-local as the
    // shared committed aim, and re-author the command every frame (recirculation).
    let dir_local = hull_local_dir(free.yaw, free.pitch);
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
    if !resolved.is_finite() {
        return;
    }
    committed.set(tank, resolved);
    if let Ok(mut command) = tank_commands.get_mut(tank) {
        command.aim = Some(AimIntent::HullLocal(resolved));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::render_policy;

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

    /// The margin is pinned at the Tiger's authored 0.12 rad optic (≈0.054 rad) and scales with FOV
    /// so the cursor's travel circle and the drawn rim stay one radius.
    #[test]
    fn margin_is_fraction_of_half_fov() {
        assert!((optic_margin(0.12) - 0.054).abs() < 1e-6);
        // Scales with the authored FOV: a wider optic gets a proportionally wider reach.
        assert!((optic_margin(0.24) - 2.0 * optic_margin(0.12)).abs() < 1e-9);
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
