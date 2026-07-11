//! The gunner's sight — System B (coaxial, player-solved ranging). See
//! `.agents/docs/design/gunner-sight.md`.
//!
//! Lshift toggles between the free third-person "commander" view (the orbit camera + `aim.rs`) and
//! a zoomed gunner optic locked to the gun's line of sight. In gunner view the camera shows the
//! gun's *reality* (the bore), and aiming is **world-space position control**: mouse deltas steer
//! the sight line from the gun pivot (worked in yaw/pitch here), which is then RESOLVED against the
//! world — terrain or another tank's armor, a far fallback in the sky — and committed as a
//! hull-local POINT: the shared [`aim::CommittedAim`], the same domain form third person commits,
//! so the two modes never convert between a point and a bare direction (the parallax bug class the
//! 2026-07-10 unification exposed). The point is published as the tank's commanded aim
//! (`TankCommand`) that every servo chases at its authored slew rate — so the view lags and
//! settles (dead-stop on release, *not* rate control), and the hull MG traverses alongside the gun.
//! Pure line of sight: superelevation (the ballistic lob for range) is a firing-side concern
//! deferred to its own slice.

use avian3d::prelude::{Position, Rotation, SpatialQuery};
use bevy::camera::visibility::RenderLayers;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::math::Affine3A;
use bevy::prelude::*;

use crate::aim::{AimCommit, CommittedAim, MAX_RANGE, aim_distance};
use crate::camera::{CameraKickApplied, GUNNER_FOV_FALLBACK, GunnerCameraPlaced, view_fov};
use crate::command::{TankCommand, gather_commands};
use crate::damage::{ControlledTank, VolumeOf};
use crate::firecontrol::{RangeTable, Ranging};
use crate::overlay::{self, Overlay, Overlays};
use crate::spec::ViewKind;
use crate::state::{GameplaySet, PlayerInputSet};
use crate::tank::{
    Controlled, Hull, Rig, ServoIndex, ServoSpec, Tank, TankSim, TankViews, rig_world_pose,
    shortest_angle,
};
use crate::ui_font::UiFonts;

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

/// The Lshift view toggle's ordering anchor: `toggle_sight` flips [`SightMode`] here (`Update`).
/// A system that must react to the flip the SAME frame (the orbit camera's optic-exit re-aim in
/// `camera`) orders `.after` this set — reacting a frame late means one frame of camera and aim
/// commit consuming the stale pre-toggle direction.
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

/// The committed gunner aim as yaw/pitch (radians, hull frame): the working form of the shared
/// [`aim::CommittedAim`] while the optic drives it (position control — mouse deltas move it; it is
/// NOT the gun's live lay, which lags). The angles are the bearing of the committed point **from
/// the gun mount** (`point − mount`, the sight's true origin — where `camera::gunner_camera` parks
/// and where `aim::drive_aim_servos` measures the pitch servo's target), NEVER from the hull-frame
/// origin: the hull origin sits ~2.2 m below the mount at ground level, and a near floor point's
/// bearing differs between the two by the mount parallax (~2.5° at 50 m, ~9° at 15 m — nearly the
/// whole 3.1° optic radius under magnification; the 2026-07-10 "aim snaps on first optic input"
/// regression). Not a resource: the persistent memory is the one `CommittedAim` point, which the
/// optic resumes into yaw/pitch each frame, works the clamps on, and re-resolves — a per-frame
/// scratch value, not a second source of truth to keep in sync.
#[derive(Clone, Copy)]
struct GunnerIntent {
    yaw: f32,
    pitch: f32,
}

impl GunnerIntent {
    /// The intent as a direction in the hull's local frame (unit length). Inverse of the yaw/pitch
    /// decomposition `aim.rs` uses (`yaw = atan2(-x, -z)`, `pitch = atan2(y, |xz|)`), so the
    /// reticle agrees with what the servos are commanded toward.
    fn local_dir(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(-sy * cp, sp, -cy * cp)
    }

    /// Decompose a hull-local aim direction into yaw/pitch — the exact inverse of
    /// [`local_dir`](Self::local_dir), and the SAME decomposition `aim::drive_aim_servos` applies
    /// per servo (`yaw = atan2(-x, -z)`, `pitch = atan2(y, |xz|)`). Scale-invariant (`atan2`
    /// ignores magnitude). The caller resumes the committed point by feeding `point − mount`, so
    /// the bearing is measured from the sight's origin exactly as `drive_aim_servos` measures each
    /// servo's target from its own pose — feeding the raw point would measure from the hull-frame
    /// origin and reintroduce the mount parallax (see [`GunnerIntent`]).
    fn from_hull_local_dir(dir: Vec3) -> Self {
        Self {
            yaw: (-dir.x).atan2(-dir.z),
            pitch: dir.y.atan2((dir.x * dir.x + dir.z * dir.z).sqrt()),
        }
    }
}

/// The on-screen intent cursor — the marker the gun chases. It moves immediately with the mouse
/// (position control) and drifts back to centre as the gun's lay catches up.
#[derive(Component)]
struct IntentReticle;

/// Full-screen black overlay shown when the active view's crewman is dead, plus a center prompt
/// telling the player to switch to the other view. Hidden when the view is alive.
#[derive(Component)]
struct ViewDeathOverlay;

/// The prompt text inside the [`ViewDeathOverlay`] — its own (child) entity, so the overlay's
/// `Visibility` (on the parent) and this `Text` are written separately.
#[derive(Component)]
struct ViewDeathText;

/// Seconds a refusal toast stays up.
const TOAST_SECONDS: f32 = 2.0;

/// A brief on-screen message — used when a view switch is *refused* (the target view's crewman is
/// down), so the silent Lshift no-op gets a reason. Ticks down in `update_toast`.
#[derive(Resource, Default)]
struct Toast {
    message: String,
    remaining: f32,
}

impl Toast {
    fn show(&mut self, message: impl Into<String>) {
        self.message = message.into();
        self.remaining = TOAST_SECONDS;
    }
}

/// The toast's text node (upper-centre); shown while [`Toast::remaining`] > 0.
#[derive(Component)]
struct ToastText;

/// HUD: the dialed range, shown in the optic so the ranging skill is legible — the player needs to
/// read what they've set to estimate and correct it. Hidden outside gunner view.
#[derive(Component)]
struct RangeReadout;

/// The ranging reticle's static horizontal reference line, held on the sight centre. The moving range
/// scale slides behind it; whichever graduation the line crosses is the dialed range.
#[derive(Component)]
struct ReticleLine;

/// One graduation of the moving range scale, tagged with the absolute range it marks. Majors (400 m
/// multiples) carry a number; minors (the 200 m halves) don't. Repositioned each frame to
/// `θ(dialed) − θ(range)` above centre, so the scale rides up with the gun as range is dialed out and
/// the dialed graduation lands on the [`ReticleLine`].
#[derive(Component)]
struct RangeScaleTick {
    range: f32,
    major: bool,
}

pub fn plugin(app: &mut App) {
    app.init_resource::<SightMode>()
        .init_resource::<Toast>()
        .add_systems(
            Startup,
            (
                spawn_intent_reticle,
                spawn_view_death_overlay,
                spawn_toast,
                spawn_range_readout,
                spawn_ranging_reticle,
            ),
        )
        .add_systems(
            Update,
            (
                // Only the Lshift view toggle is player input (gated on the cursor); the overlay,
                // toast, and range readout are presentation and keep updating with the cursor free.
                toggle_sight.in_set(PlayerInputSet).in_set(SightToggled),
                update_view_death_overlay,
                // After `toggle_sight`, so a refused switch this frame shows its reason.
                update_toast,
                update_range_readout,
            )
                .chain()
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
            drive_gunner_aim
                .run_if(in_gunner)
                .after(gather_commands)
                .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
                .in_set(AimCommit)
                .in_set(PlayerInputSet)
                .in_set(GameplaySet),
        )
        // Reconcile the controlled tank's optic render-layer hide every frame — continuous derived
        // render state, no `run_if`/ordering edge (see the system's doc comment for why event-driven
        // was the original defect). Plain `Update`/`GameplaySet`; it only writes on mismatch, so an
        // unconditional schedule costs a read of each mesh's layer in steady state.
        .add_systems(Update, reconcile_optic_render_layers.in_set(GameplaySet))
        // The intent cursor reprojects through the gunner camera, so it runs after the camera's pose
        // is final for the frame. Both inputs are render-rate — `intent` (mouse, Update) and the
        // camera pose (which reads the VIEW gun's `GlobalTransform`, blended by
        // `interpolate_servos` in Update) — so the reprojection is clean by construction, no
        // aliasing.
        .add_systems(
            PostUpdate,
            (update_intent_reticle, update_ranging_reticle)
                .in_set(GameplaySet)
                .after(TransformSystems::Propagate)
                .after(GunnerCameraPlaced)
                // After the hit-kick has displaced the camera's rendered pose, so the reticles
                // reproject through the kicked view and the whole sight picture jolts together on a
                // hit. Vacuous edge in SP/headless (the kick set is net-client-only, empty there).
                .after(CameraKickApplied),
        );
}

fn spawn_intent_reticle(mut commands: Commands) {
    commands.spawn((
        IntentReticle,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Px(8.0),
            height: Val::Px(8.0),
            border_radius: BorderRadius::MAX,
            ..default()
        },
        BackgroundColor(Color::srgba(1.0, 0.7, 0.1, 0.9)),
        Visibility::Hidden,
    ));
}

/// The full-screen black overlay + center prompt, shown when the active view's crewman is dead.
/// The prompt tells the player to press Lshift to switch to the other view (if its crewman is
/// alive). Solid black — "your crewman's eyes are gone" (design §7a, view-death model).
fn spawn_view_death_overlay(mut commands: Commands, fonts: Res<UiFonts>) {
    commands
        .spawn((
            ViewDeathOverlay,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                position_type: PositionType::Absolute,
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::BLACK),
            // The one-scrim contract's lowest z: the view-death black sits BELOW the death screen, so
            // whole-crew death (Death latched) can never let this opaque black occlude "YOU DIED" — the
            // spawn-order bug this redesign fixes. On the net client `update_view_death_overlay` also
            // hard-suppresses it whenever Death is latched; the z is the belt to that braces.
            GlobalZIndex(Overlay::ViewDead.zindex()),
            Visibility::Hidden,
        ))
        .with_children(|parent| {
            parent.spawn((
                ViewDeathText,
                Text::new(""),
                TextFont {
                    // SemiBold: a full-screen crew-death prompt.
                    font: fonts.hud.clone().into(),
                    font_size: FontSize::Px(20.0),
                    ..default()
                },
                TextColor(Color::srgb(0.9, 0.4, 0.3)),
            ));
        });
}

/// The refusal-toast text node: a centred banner in the upper third, hidden until a refused switch
/// raises it. Its own entity carries both `Text` and `Visibility`, so `update_toast` writes one query.
fn spawn_toast(mut commands: Commands, fonts: Res<UiFonts>) {
    commands
        .spawn(Node {
            width: Val::Percent(100.0),
            position_type: PositionType::Absolute,
            top: Val::Percent(30.0),
            justify_content: JustifyContent::Center,
            ..default()
        })
        .with_children(|parent| {
            parent.spawn((
                ToastText,
                Text::new(""),
                TextFont {
                    // SemiBold: a centred refusal banner.
                    font: fonts.hud.clone().into(),
                    font_size: FontSize::Px(22.0),
                    ..default()
                },
                TextColor(Color::srgb(1.0, 0.75, 0.3)),
                Visibility::Hidden,
            ));
        });
}

/// The dialed-range readout, parked bottom-left; populated/shown only in the optic.
fn spawn_range_readout(mut commands: Commands, fonts: Res<UiFonts>) {
    commands.spawn((
        RangeReadout,
        Text::new(""),
        TextFont {
            // SemiBold: an all-caps gunnery readout ("RANGE ... m").
            font: fonts.hud.clone().into(),
            font_size: FontSize::Px(16.0),
            ..default()
        },
        TextColor(Color::srgba(1.0, 0.8, 0.3, 0.9)),
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(24.0),
            left: Val::Px(24.0),
            ..default()
        },
        Visibility::Hidden,
    ));
}

/// Show the dialed range in the optic so the player can read and correct their estimate; hidden in
/// third-person (where scroll is the camera dolly, not ranging).
fn update_range_readout(
    mode: Res<SightMode>,
    ranging: Res<Ranging>,
    mut readout: Query<(&mut Text, &mut Visibility), With<RangeReadout>>,
) {
    let Ok((mut text, mut visibility)) = readout.single_mut() else {
        return;
    };
    if *mode == SightMode::Gunner {
        *text = Text::new(format!("RANGE {} m", ranging.range as i32));
        *visibility = Visibility::Visible;
    } else {
        *visibility = Visibility::Hidden;
    }
}

/// Reticle graticule colour — amber, grouping it with the other gunnery readouts.
const RETICLE_COLOR: Color = Color::srgba(1.0, 0.8, 0.3, 0.85);

/// Spawn the ranging reticle: the static centre line (held on the sight centre via a flex box, the
/// same idiom as the white centre dot) and the pool of range graduations (200 m steps, majors
/// numbered in hundreds of metres). All hidden until shown in the optic.
fn spawn_ranging_reticle(mut commands: Commands, fonts: Res<UiFonts>) {
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
                ReticleLine,
                Node {
                    width: Val::Px(96.0),
                    height: Val::Px(2.0),
                    ..default()
                },
                BackgroundColor(RETICLE_COLOR),
                Visibility::Hidden,
            ));
        });

    let mut range = 200.0_f32;
    while range <= 4000.0 {
        let major = (range as i32) % 400 == 0;
        let width = if major { 24.0 } else { 12.0 };
        let mut tick = commands.spawn((
            RangeScaleTick { range, major },
            Node {
                position_type: PositionType::Absolute,
                width: Val::Px(width),
                height: Val::Px(2.0),
                ..default()
            },
            BackgroundColor(RETICLE_COLOR),
            Visibility::Hidden,
        ));
        if major {
            // Label rides the tick: an absolute child offsets from the tick's own top-left.
            tick.with_children(|parent| {
                parent.spawn((
                    Text::new(format!("{}", (range as i32) / 100)),
                    TextFont {
                        // Regular: a tiny reticle graduation number (12px).
                        font: fonts.body.clone().into(),
                        font_size: FontSize::Px(12.0),
                        ..default()
                    },
                    TextColor(RETICLE_COLOR),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(width + 5.0),
                        top: Val::Px(-7.0),
                        ..default()
                    },
                ));
            });
        }
        range += 200.0;
    }
}

/// Slide the range scale so each graduation sits at `θ(dialed) − θ(range)` above the sight centre: the
/// dialed range lands on the [`ReticleLine`], nearer ranges above it, farther below, the whole scale
/// riding up with the gun as range is dialed out. Reprojected through the gunner camera (after it has
/// placed itself this frame), so it shares the rendered pose; hidden outside the optic. Reads the laid
/// weapon's table — the main gun for now — which is the per-ammo ballistic scale.
fn update_ranging_reticle(
    mode: Res<SightMode>,
    ranging: Res<Ranging>,
    controlled: Query<&Rig, With<Controlled>>,
    tables: Query<&RangeTable>,
    camera: Single<(&Camera, &GlobalTransform)>,
    mut line: Query<&mut Visibility, (With<ReticleLine>, Without<RangeScaleTick>)>,
    mut ticks: Query<(&RangeScaleTick, &mut Node, &mut Visibility), Without<ReticleLine>>,
) {
    let gunner = *mode == SightMode::Gunner;
    if let Ok(mut visibility) = line.single_mut() {
        *visibility = if gunner {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }

    let table = controlled
        .single()
        .ok()
        .and_then(|rig| tables.get(rig.muzzle).ok());
    let (camera, cam_transform) = *camera;
    let rot = cam_transform.rotation();
    let forward = rot * Vec3::NEG_Z;
    let right = rot * Vec3::X;

    for (tick, mut node, mut visibility) in &mut ticks {
        let Some(table) = table.filter(|_| gunner) else {
            *visibility = Visibility::Hidden;
            continue;
        };
        // Angle above centre = θ(dialed) − θ(this mark); rotate the sight line up by it about the
        // camera's right axis (so the scale is screen-vertical regardless of hull roll) and reproject.
        let angle = table.superelevation(ranging.range) - table.superelevation(tick.range);
        let dir = Quat::from_axis_angle(right, angle) * forward;
        match camera.world_to_viewport(cam_transform, cam_transform.translation() + dir) {
            Ok(screen) => {
                let half = if tick.major { 12.0 } else { 6.0 };
                node.left = Val::Px(screen.x - half);
                node.top = Val::Px(screen.y - 1.0);
                *visibility = Visibility::Visible;
            }
            Err(_) => *visibility = Visibility::Hidden,
        }
    }
}

/// Place the intent cursor at the reprojection of the committed aim point — a resolved point on
/// the world in BOTH regimes (a third-person commit resumed on entry, or the optic's own resolve),
/// so its true screen position is exact at any range; no bearing-only shortcut, which would place a
/// near floor aim too high by the mount parallax. As the gun (and so the camera/sight line) catches
/// up, this drifts back to screen centre; hidden outside gunner view.
///
/// Reads the shared [`aim::CommittedAim`] (republished by `drive_gunner_aim` earlier this frame in
/// `BeforeFixedMainLoop`) and the gunner camera's pose (which reads the VIEW gun's `GlobalTransform`,
/// blended by `interpolate_servos` in `Update`) — a pure function of the committed intent and the
/// camera, no aliasing.
fn update_intent_reticle(
    mode: Res<SightMode>,
    committed: Res<CommittedAim>,
    camera: Single<(&Camera, &GlobalTransform)>,
    controlled: Query<(Entity, &Rig), With<Controlled>>,
    hull: Query<&GlobalTransform, With<Hull>>,
    mut reticle: Query<(&mut Node, &mut Visibility), With<IntentReticle>>,
) {
    let Ok((mut node, mut visibility)) = reticle.single_mut() else {
        return;
    };
    if *mode != SightMode::Gunner {
        *visibility = Visibility::Hidden;
        return;
    }
    let Ok((tank, rig)) = controlled.single() else {
        *visibility = Visibility::Hidden;
        return;
    };
    let Ok(hull) = hull.get(rig.hull) else {
        return;
    };
    // The committed intention (the shared `CommittedAim`, republished by `drive_gunner_aim` every
    // gunner frame), hull-local. `None` before the first commit for this tank (possession-keyed).
    let Some(local) = committed.get(tank) else {
        *visibility = Visibility::Hidden;
        return;
    };
    let (camera, cam_transform) = *camera;

    // Reproject the ACTUAL committed world point, hull-local → world. The committed value is always
    // a resolved point (both modes commit points), so this is exact at any range — projecting a
    // bearing instead would place a near floor aim too high by the mount parallax (the regression).
    let point = hull.affine().transform_point3(local);

    match camera.world_to_viewport(cam_transform, point) {
        Ok(screen) => {
            node.left = Val::Px(screen.x - 4.0);
            node.top = Val::Px(screen.y - 4.0);
            *visibility = Visibility::Visible;
        }
        Err(_) => *visibility = Visibility::Hidden,
    }
}

/// Lshift flips the view — but only if the target view's crewman is alive. **No aim handoff:** both
/// modes read and write the one [`aim::CommittedAim`], so a switch preserves the committed intention
/// by construction — the optic RESUMES the commander's committed aim on entry (`drive_gunner_aim`),
/// and third-person re-authors the optic's last-published lay on return (`aim::commit_aim`'s RMB
/// hold). What used to live here — seed-the-intent-from-the-gun's-lay on entry, reseed-the-free-look-
/// hold-from-the-live-aim on exit — is gone: the seeding it did IS what sharing one memory does for
/// free (a fresh, never-committed tank still starts from the gun's current lay, but that one rule now
/// lives in `drive_gunner_aim`, keyed on the absence of a commitment). The controlled tank's own mesh
/// is hidden from the optic by `reconcile_optic_render_layers` (which derives the hide from this mode
/// every frame), so the camera parked inside the mantlet sees through its own geometry.
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
    // Need a controlled rig to have a view to flip into.
    if controlled.rig().is_none() {
        return;
    }
    *mode = match *mode {
        SightMode::ThirdPerson => {
            // Only switch to gunner optic if the gunner view is usable (gunner alive) — otherwise
            // toast the reason, since the switch silently does nothing.
            if !view_available(&controlled, &views, ViewKind::Gunner) {
                toast.show(format!("{} unavailable", ViewKind::Gunner.label()));
                return;
            }
            SightMode::Gunner
        }
        SightMode::Gunner => {
            // Only switch to third-person if the commander view is usable (commander alive).
            if !view_available(&controlled, &views, ViewKind::Commander) {
                toast.show(format!("{} unavailable", ViewKind::Commander.label()));
                return;
            }
            SightMode::ThirdPerson
        }
    };
}

/// The render layer the controlled tank's meshes move to while in the gunner optic. The world,
/// terrain, and other tanks stay on layer 0 (which the camera draws); the controlled tank's own
/// meshes go here, which the camera does not draw — so the optic, parked inside the mantlet, sees
/// through its own geometry while everything else renders normally.
const OPTIC_HIDDEN_LAYER: usize = 1;

/// The render layer a tank mesh belongs on: [`OPTIC_HIDDEN_LAYER`] when it is a mesh of the
/// controlled tank AND the gunner optic is up (hide it from the mount-parked camera), else the
/// default layer 0 every camera draws. Pure so the reconcile invariant — controlled-and-gunner hides,
/// everything else shows — is unit-tested without an app.
fn desired_optic_layer(gunner: bool, is_controlled: bool) -> RenderLayers {
    if gunner && is_controlled {
        RenderLayers::layer(OPTIC_HIDDEN_LAYER)
    } else {
        RenderLayers::layer(0)
    }
}

/// Hide the controlled tank from its own gunner optic via **render layers, not `Visibility`**. While
/// in the optic, the controlled tank's render meshes move to [`OPTIC_HIDDEN_LAYER`]; otherwise they
/// sit on the default layer 0 the camera draws. Render layers are per-camera and, unlike
/// `Visibility`, are not co-owned by Avian's debug renderer (`PhysicsDebugPlugin` rewrites mesh
/// `Visibility` when gizmos are toggled), so a debug toggle can no longer defeat the hide.
///
/// **Runs unconditionally every frame — this hide is CONTINUOUS DERIVED RENDER STATE** of (SightMode
/// × which tank is `Controlled` × the tank's live mesh set), the same shape as Bevy's own per-frame
/// visibility and transform propagation, and derived the same way: recomputed from its inputs rather
/// than event-driven. Event/`resource_changed`-driven was the original defect — it re-laid the layers
/// only on a `SightMode` change, so two input-less mutations of the derived state silently went
/// unhandled: (1) a multiplayer respawn swaps `Controlled` onto a fresh tank (and despawns the old)
/// with no mode change, leaving the new tank's barrel rendering in the mount-parked optic; and (2)
/// the tank's meshes attach ASYNCHRONOUSLY as the glb scene instantiates (default layer 0) — there is
/// no mesh-set-changed event to subscribe, so meshes landing after a one-shot stamp were missed even
/// on first gunner entry. A per-frame reconcile owns all three inputs by construction.
///
/// `RenderLayers` does not inherit, so the layer is written per render mesh, and **only on mismatch**:
/// each mesh's current `Option<&RenderLayers>` is compared against the desired layer and left alone
/// when it already matches, so steady state is reads-only (no per-frame change-detection dirtying) and
/// the unconditional schedule costs one layer read per mesh. Non-controlled tanks' meshes stay on
/// layer 0 in gunner mode, so the optic still sees opponents.
fn reconcile_optic_render_layers(
    mode: Res<SightMode>,
    controlled: Query<Entity, With<Controlled>>,
    tanks: Query<Entity, With<Tank>>,
    children: Query<&Children>,
    meshes: Query<Option<&RenderLayers>, With<Mesh3d>>,
    mut commands: Commands,
) {
    let controlled_tank = controlled.single().ok();
    let gunner = *mode == SightMode::Gunner;
    for tank in &tanks {
        let desired = desired_optic_layer(gunner, Some(tank) == controlled_tank);
        for entity in children.iter_descendants(tank) {
            // `meshes.get` doubles as the `Mesh3d` filter (non-mesh nodes `Err` out) and the current
            // layer read; write only when it differs from the desired layer.
            if let Ok(current) = meshes.get(entity)
                && current != Some(&desired)
            {
                commands.entity(entity).insert(desired.clone());
            }
        }
    }
}

/// The gunner optic's radius as a fraction of the view's **half** vertical FOV — the single shared
/// object that ties the cursor's travel circle to the drawn optic rim. `drive_gunner_aim` uses it
/// for the intent's angular margin (`margin = OPTIC_RADIUS_FRACTION · fov/2`); the circular
/// sight-overlay UI (to come) MUST read this same constant for its rim radius, so the cursor can
/// reach exactly the drawn edge — no more, no less — by construction rather than by two hand-tuned
/// numbers drifting apart. Angular by half-FOV means it maps to a fixed fraction of the viewport's
/// half-height regardless of magnification: the overlay's rim in pixels is `OPTIC_RADIUS_FRACTION ·
/// viewport_height/2`. 0.9 leaves a sliver of margin between the cursor's reach and the hard edge.
pub const OPTIC_RADIUS_FRACTION: f32 = 0.9;

/// The cursor-travel / optic-rim angular radius (radians) for a view of vertical FOV `fov`: a fixed
/// fraction of the half-FOV (see [`OPTIC_RADIUS_FRACTION`]). Pulled out as a pure function so the
/// derivation is unit-testable and the overlay UI can call the identical maths.
fn optic_margin(fov: f32) -> f32 {
    OPTIC_RADIUS_FRACTION * (fov / 2.0)
}

/// Clamp `value` to a servo's authored travel `limits` (radians); a `None` (continuous) mount passes
/// through untouched. Kept pure for the unit test — the caller shifts the pitch window by the lob
/// before calling (sight line = lay − superelevation).
fn clamp_to_travel(value: f32, limits: Option<(f32, f32)>) -> f32 {
    match limits {
        Some((min, max)) => value.clamp(min, max),
        None => value,
    }
}

/// What `drive_gunner_aim` publishes this frame: the value re-authored into `command.aim` every
/// frame (recirculation — never fall silent), and, when the optic OWNS the intention, the point to
/// re-store into [`aim::CommittedAim`] (`None` = leave the committed memory untouched).
struct AimPublish {
    /// Re-authored into `command.aim` every frame in the optic.
    command_aim: Vec3,
    /// `Some` = re-store into `CommittedAim` (the optic's freshly resolved point); `None` = leave
    /// the committed memory exactly as it was (the zero-input-identity invariant).
    store: Option<Vec3>,
}

/// The **zero-input-identity** decision for the optic's aim commit. Both modes commit RESOLVED
/// WORLD POINTS, but they resolve from different origins — third person raycasts from the orbit
/// camera, the optic from the gun mount — so re-resolving an inherited commitment can land on
/// DIFFERENT world geometry (a crest the elevated camera saw over occludes the mount's lower ray),
/// and a mode transition must be identity on the aim. So the resolve is gated on actual input:
///
/// - **No mouse motion this frame, with an existing commitment** (`committed_point = Some`,
///   `!moved`): re-author the ORIGINAL committed point verbatim and store NOTHING. A mode
///   transition with zero input is thus identity on `CommittedAim` and on the gun's lay — the gun
///   keeps chasing exactly the point it chased in third person, floor / horizon / sky alike.
/// - **The player moved the mouse** (`moved`): the optic takes ownership — publish and re-store
///   the point just resolved along the moved sight line. From here the intention is the optic's
///   own resolve, and the commander finds it (a real point on the world) on a later mode switch.
/// - **No commitment yet** (`committed_point = None`: fresh spawn or a possession change): there
///   is nothing to preserve, so the resolve seeded from the gun's lay must be published AND stored
///   to establish the commitment — even with zero input, so the recirculation invariant still
///   writes `command.aim`.
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

/// World-space position-control aiming. The one committed intention (the shared [`aim::CommittedAim`],
/// resumed into yaw/pitch each frame — or seeded from the gun's lay when this tank has none yet)
/// takes this frame's mouse deltas, is clamped, then RESOLVED against the world and re-published as
/// the tank's commanded aim so `aim::drive_aim_servos` chases it with *every* servo, the gun and the
/// hull MG alike, at their own slew rates. No servo command is written here; this only moves the aim
/// intention.
///
/// **One frame convention, one origin.** The resume decomposes `point − mount` (the gun pivot, from
/// the same physics-truth chain `drive_aim_servos` lays from), the clamps compare against the gun's
/// lay measured at that same mount, and the resolve raycasts FROM that mount along the sight line —
/// so the working angles, the servo convergence, and the gunner camera (parked at the pivot) all
/// agree, and no conversion ever moves the gun by the mount↔hull-origin parallax (~2.5° at 50 m —
/// most of the optic's 3.1° radius; the "snaps on first input" regression).
///
/// **Zero-input identity ([`resume_commit`]).** Both modes commit resolved points, but from
/// different origins (orbit camera vs mount), so re-resolving an inherited commitment could land on
/// different geometry (crest occlusion). The resolve is therefore published/re-stored ONLY once the
/// player actually moves the mouse (or on a fresh tank with no commitment to preserve); until then
/// the resume re-authors the ORIGINAL committed point verbatim — the gun does not move, the reticle
/// does not jump, and `CommittedAim` is left untouched. From the first mouse delta the optic OWNS
/// the intention and re-stores its own resolve, so the commander finds a real point on the world
/// already committed on the next mode switch.
///
/// Two bounds shape the intent, in order:
///
/// 1. **Mechanical travel** — the intent is clamped to what the gun chain can actually reach, from
///    the servos' authored travel limits ([`ServoSpec::travel_limits`], the same window
///    `drive_servos` enforces on the lay). The servos limit the *lay* (bore); the intent tracks the
///    *sight line* = lay − lob, so the reachable pitch window is those limits shifted DOWN by the
///    superelevation θ. Without this the cursor could park above the gun's max elevation, the gun
///    would saturate at its stop, and the reticle would peg at the optic rim forever, never settling.
/// 2. **Circular optic margin** — the intent may then lead the gun's *current* sight line by at most
///    `optic_margin(fov)` = [`OPTIC_RADIUS_FRACTION`] · half the authored optic FOV, so the cursor
///    can't run past the optic edge ahead of the slow turret: pegged at the margin means "slewing at
///    max," near centre means "caught up." Deriving the radius from the *authored* per-tank FOV (not
///    a hardcoded angle) makes the travel circle the same object as the drawn optic rim. The clamp is
///    circular (not per-axis) so diagonal lead feels uniform — a square clamp let you lead ~√2·margin
///    on the diagonal — and yaw is wrapped (`shortest_angle`) so continuous traverse past ±π doesn't
///    yank the intent across the wrap.
///
/// Inside both bounds the absolute intent is left untouched (hull-local) so the gun genuinely catches
/// up as it slews — re-pinning to `current + offset` each frame would make the target recede with the
/// gun (never arrives). The gun chain (`Turret`/`Gun`) is the lead reference; the hull MG rides the
/// same point.
fn drive_gunner_aim(
    motion: Res<AccumulatedMouseMotion>,
    spatial: SpatialQuery,
    mut committed: ResMut<CommittedAim>,
    controlled: ControlledTank,
    views: Query<&TankViews, With<Controlled>>,
    servo_slots: Query<&ServoIndex>,
    servo_specs: Query<&ServoSpec>,
    sims: Query<&TankSim>,
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

    // The one committed intention, filtered for finiteness: a poisoned committed value (rollback
    // edge) reads as NO commitment, so the seed-from-lay path below overwrites it with a finite
    // resolve instead of latching the optic dead (a NaN resume would trip the direction guard
    // before every publish, forever, with nothing in gunner mode ever healing the memory).
    let committed_point = committed.get(tank).filter(|point| point.is_finite());
    // Zero motion this frame is exactly `Vec2::ZERO` from `AccumulatedMouseMotion`.
    let moved = motion.delta != Vec2::ZERO;

    // Zero-input identity, taken FIRST — [`resume_commit`]'s no-motion arm, short-circuited before
    // any pose work: with an existing commitment and no input, re-author the ORIGINAL point
    // (recirculation: holding is an act) and touch nothing else. Publishing before the pose fetch
    // keeps the hold alive even on a frame the resolve guards below would skip, and drops the
    // world raycast that `resume_commit` would discard anyway.
    if let Some(point) = committed_point
        && !moved
    {
        if let Ok(mut command) = tank_commands.get_mut(tank) {
            command.aim = Some(point);
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

    // Servo angles live root-resident (`TankSim`), addressed by each node's `ServoIndex`.
    let angle = |servo| {
        sims.get(tank)
            .ok()
            .zip(servo_slots.get(servo).ok())
            .and_then(|(sim, slot)| sim.servos.get(slot.0))
            .map(crate::tank::ServoState::current)
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
    let pitch_limits = servo_specs
        .get(rig.gun)
        .ok()
        .and_then(ServoSpec::travel_limits)
        .map(|(min, max)| (min - theta, max - theta));
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
        command.aim = Some(publish.command_aim);
    }
}

/// Show/hide the black overlay + prompt when the active view's crewman is dead. The prompt tells
/// the player to press Lshift to switch to the other view if its crewman is alive; if both are
/// dead, the prompt says so (the tank is effectively dead — 0 living crew imminent).
///
/// On the NET client this participates in the overlay authority: it declares `Overlay::ViewDead`
/// presence and defers its own visibility to the one-scrim rule — suppressed entirely whenever a
/// higher overlay (the death screen above all, but also the menu / connect screen) owns the scrim, so
/// whole-crew death shows "YOU DIED", not this black. In single-player the `Overlays` resource is
/// absent (`Option` is `None`) and it behaves standalone as before: crewman down → black + prompt.
fn update_view_death_overlay(
    mode: Res<SightMode>,
    controlled: ControlledTank,
    views: Query<&TankViews, With<Controlled>>,
    overlays: Option<ResMut<Overlays>>,
    mut overlay_vis: Query<&mut Visibility, With<ViewDeathOverlay>>,
    mut label: Query<&mut Text, With<ViewDeathText>>,
) {
    let has_controlled = controlled.entity().is_some();
    // The overlay's `Visibility` lives on the full-screen node; its prompt `Text` on the child.
    let (Ok(mut vis), Ok(mut text)) = (overlay_vis.single_mut(), label.single_mut()) else {
        return;
    };

    let (active_view, other_view, other_label) = match *mode {
        SightMode::ThirdPerson => (ViewKind::Commander, ViewKind::Gunner, "gunner optic"),
        SightMode::Gunner => (ViewKind::Gunner, ViewKind::Commander, "third-person"),
    };

    // The active view's crewman is down — the standalone condition for wanting this overlay. Gated on a
    // controlled tank existing (no station to be dead without one).
    let crewman_down = has_controlled && !view_available(&controlled, &views, active_view);

    // Declare presence into the net authority (net client only), then let the one-scrim rule decide
    // whether we actually draw. In single-player there is no authority: draw whenever `crewman_down`.
    let suppressed = if let Some(mut overlays) = overlays {
        overlays.declare(Overlay::ViewDead, crewman_down);
        !overlay::draws_scrim(&overlays, Overlay::ViewDead)
    } else {
        false
    };

    if !crewman_down || suppressed {
        *vis = Visibility::Hidden;
        return;
    }

    let other_available = view_available(&controlled, &views, other_view);
    *text = Text::new(if other_available {
        format!("Crewman down — [Lshift] for {other_label}")
    } else {
        "All view crew down".to_string()
    });
    *vis = Visibility::Visible;
}

/// Tick the refusal toast: show its message while it has time left, then hide it. Set by
/// `toggle_sight` when a view switch is refused (the target view's crewman is down).
fn update_toast(
    time: Res<Time>,
    mut toast: ResMut<Toast>,
    mut label: Query<(&mut Text, &mut Visibility), With<ToastText>>,
) {
    let Ok((mut text, mut visibility)) = label.single_mut() else {
        return;
    };
    if toast.remaining > 0.0 {
        toast.remaining -= time.delta_secs();
        *text = Text::new(toast.message.clone());
        *visibility = Visibility::Visible;
    } else if *visibility != Visibility::Hidden {
        *visibility = Visibility::Hidden;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pure hide invariant: a mesh is hidden from the optic ([`OPTIC_HIDDEN_LAYER`]) only when it
    /// belongs to the controlled tank AND the gunner optic is up; every other combination is layer 0
    /// (the world the camera draws — including opponents, so the optic sees them).
    #[test]
    fn desired_optic_layer_hides_only_controlled_in_gunner() {
        let hidden = RenderLayers::layer(OPTIC_HIDDEN_LAYER);
        let shown = RenderLayers::layer(0);
        assert_eq!(desired_optic_layer(true, true), hidden, "own tank in optic");
        assert_eq!(
            desired_optic_layer(true, false),
            shown,
            "opponent stays visible in optic"
        );
        assert_eq!(
            desired_optic_layer(false, true),
            shown,
            "own tank shown in third person"
        );
        assert_eq!(
            desired_optic_layer(false, false),
            shown,
            "opponent, third person"
        );
    }

    /// The reconcile is CONTINUOUS DERIVED RENDER STATE: it re-lays every mesh's render layer from the
    /// live (`SightMode`, `Controlled`, mesh set) each frame, not on a `SightMode` event. This drives
    /// the whole system through its transitions and asserts each mesh's `RenderLayers`, including the
    /// two cases the old event-driven stamp missed (both fail without the per-frame reconcile):
    /// a mesh attaching asynchronously WHILE in the optic, and `Controlled` moving to a fresh tank
    /// with NO `SightMode` write (the multiplayer respawn that leaves the barrel in the sight).
    #[test]
    fn reconcile_lays_layers_across_transitions() {
        let mut app = App::new();
        app.init_resource::<SightMode>();
        app.add_systems(Update, reconcile_optic_render_layers);

        // Two tanks: A controlled (a direct mesh + a mesh under a non-mesh sub-node, to exercise
        // `iter_descendants`), B an opponent with its own meshes.
        let world = app.world_mut();
        let tank_a = world.spawn((Tank, Controlled)).id();
        let a_direct = world
            .spawn((Mesh3d(Handle::default()), ChildOf(tank_a)))
            .id();
        let a_subnode = world.spawn(ChildOf(tank_a)).id();
        let a_nested = world
            .spawn((Mesh3d(Handle::default()), ChildOf(a_subnode)))
            .id();
        let tank_b = world.spawn((Tank,)).id();
        let b_mesh = world
            .spawn((Mesh3d(Handle::default()), ChildOf(tank_b)))
            .id();

        let hidden = RenderLayers::layer(OPTIC_HIDDEN_LAYER);
        let shown = RenderLayers::layer(0);
        let layer = |app: &App, e: Entity| {
            app.world()
                .entity(e)
                .get::<RenderLayers>()
                .cloned()
                .expect("every mesh carries a RenderLayers after reconcile")
        };

        // 1. Third person: every mesh, both tanks, on layer 0.
        app.update();
        for e in [a_direct, a_nested, b_mesh] {
            assert_eq!(layer(&app, e), shown, "third person: mesh {e:?} on layer 0");
        }

        // 2. Gunner: the controlled tank's meshes hide; the opponent's stay visible.
        *app.world_mut().resource_mut::<SightMode>() = SightMode::Gunner;
        app.update();
        assert_eq!(
            layer(&app, a_direct),
            hidden,
            "gunner: own direct mesh hidden"
        );
        assert_eq!(
            layer(&app, a_nested),
            hidden,
            "gunner: own nested mesh hidden"
        );
        assert_eq!(layer(&app, b_mesh), shown, "gunner: opponent stays visible");

        // 3. A NEW mesh attaches under the controlled tank WHILE in the optic (the async-attach case
        // the one-shot stamp missed) — the next frame lands it hidden.
        let a_late = app
            .world_mut()
            .spawn((Mesh3d(Handle::default()), ChildOf(tank_a)))
            .id();
        app.update();
        assert_eq!(
            layer(&app, a_late),
            hidden,
            "async-attached mesh hides on the next reconcile (old event-driven design misses it)"
        );

        // 4. Move `Controlled` to the opponent with NO `SightMode` write — the multiplayer respawn.
        // Old tank's meshes return to layer 0; the newly controlled tank's meshes hide.
        app.world_mut().entity_mut(tank_a).remove::<Controlled>();
        app.world_mut().entity_mut(tank_b).insert(Controlled);
        app.update();
        for e in [a_direct, a_nested, a_late] {
            assert_eq!(
                layer(&app, e),
                shown,
                "respawn: stepped-out tank's mesh {e:?} back to layer 0"
            );
        }
        assert_eq!(
            layer(&app, b_mesh),
            hidden,
            "respawn: newly controlled tank hides with no SightMode change (the bug)"
        );

        // 5. Back to third person: every mesh on layer 0 again.
        *app.world_mut().resource_mut::<SightMode>() = SightMode::ThirdPerson;
        app.update();
        for e in [a_direct, a_nested, a_late, b_mesh] {
            assert_eq!(
                layer(&app, e),
                shown,
                "back to third person: mesh {e:?} on layer 0"
            );
        }
    }

    /// The margin is a fixed fraction of the half-FOV — the derivation the overlay UI must share, so
    /// the cursor's travel circle and the drawn rim are one radius. Pinned at the Tiger's authored
    /// 0.12 rad optic (≈0.054 rad) and confirmed proportional to the fraction.
    #[test]
    fn margin_is_fraction_of_half_fov() {
        let fov = 0.12_f32;
        assert!((optic_margin(fov) - OPTIC_RADIUS_FRACTION * fov / 2.0).abs() < 1e-9);
        assert!((optic_margin(fov) - 0.054).abs() < 1e-6);
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

    /// Superelevation slides the reachable *sight-line* pitch window down by the lob: the servo
    /// limits bound the lay (= sight line + θ), so a sight line laid at `max − θ` puts the bore
    /// exactly at its elevation stop. As range is dialed out and θ grows, the sight can't be laid as
    /// high — the gun spends more of its travel on the lob.
    #[test]
    fn superelevation_shifts_pitch_window() {
        let (min, max) = (-8.0_f32.to_radians(), 15.0_f32.to_radians());
        let theta = 0.01_f32;
        let shifted = Some((min - theta, max - theta));
        // A sight line just under the shifted ceiling stays; one above it is pulled to `max − θ`.
        let ceiling = max - theta;
        assert!((clamp_to_travel(ceiling + 0.05, shifted) - ceiling).abs() < 1e-6);
        // Lay = clamped sight line + θ never exceeds the mechanical `max`.
        assert!(clamp_to_travel(ceiling + 0.05, shifted) + theta <= max + 1e-6);
    }
}
