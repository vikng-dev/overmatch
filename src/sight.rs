//! Gunner sight input, presentation, and aim-point commitment.

use avian3d::prelude::{Position, Rotation, SpatialQuery};
use bevy::camera::visibility::RenderLayers;
use bevy::ecs::system::SystemParam;
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

/// HUD dialed-range readout, hidden outside gunner view.
#[derive(Component)]
struct RangeReadout;

/// The ranging reticle's static horizontal reference line, held on the sight centre. The moving range
/// scale slides behind it; whichever graduation the line crosses is the dialed range.
#[derive(Component)]
struct ReticleLine;

/// One moving range-scale graduation.
#[derive(Component)]
struct RangeScaleTick {
    range: f32,
    major: bool,
}

pub fn plugin(app: &mut App) {
    app.init_resource::<SightMode>()
        .init_resource::<Toast>()
        // A/B harness: the active gunner-view scheme + the two free-look/elastic camera view-states.
        .init_resource::<GunnerScheme>()
        .init_resource::<GunnerFreeAim>()
        .init_resource::<ElasticCam>()
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
                // On the net client this DECLARES `Overlay::ViewDead` (+ its prompt text) and leaves
                // the visibility swap to the shared `overlay::apply_overlay_visibility` reconciler — so
                // its declaration joins the `Declare` phase and its apply reads the fully-reconciled
                // set (the ordering fix). In single-player (no `Overlays`) it still sets visibility
                // itself. `Declare` is unconfigured in single-player, so it imposes nothing there.
                update_view_death_overlay.in_set(overlay::OverlaySet::Declare),
                // After `toggle_sight`, so a refused switch this frame shows its reason.
                update_toast,
                update_range_readout,
            )
                .chain()
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
            // `OverlayNode(ViewDead)` stamps the one-scrim contract's lowest z via its hook (in BOTH
            // single-player and net — the hook always runs): the view-death black sits BELOW the death
            // screen, so whole-crew death (Death latched) can never let this opaque black occlude "YOU
            // DIED" — the spawn-order bug this redesign fixes. On the net client the marker ALSO hands
            // this node's visibility to `overlay::apply_overlay_visibility`, which hard-suppresses it
            // whenever a higher overlay owns the scrim; the z is the belt to that brace.
            overlay::OverlayNode(Overlay::ViewDead),
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
    let Some(local) = committed.get(tank) else {
        *visibility = Visibility::Hidden;
        return;
    };
    let (camera, cam_transform) = *camera;

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

/// Render layer for the controlled tank while the gunner optic is active.
const OPTIC_HIDDEN_LAYER: usize = 1;

/// Return the controlled-optic hidden layer; all other meshes use layer zero.
fn desired_optic_layer(gunner: bool, is_controlled: bool) -> RenderLayers {
    if gunner && is_controlled {
        RenderLayers::layer(OPTIC_HIDDEN_LAYER)
    } else {
        RenderLayers::layer(0)
    }
}

/// Reconcile every tank mesh's render layer from live sight mode, control ownership, and mesh set.
///
/// Invariant: this runs every frame because control and asynchronously-instantiated meshes can
/// change without a sight-mode event. `RenderLayers` does not inherit, so each mesh is written only
/// when its layer differs.
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
            if let Ok(current) = meshes.get(entity)
                && current != Some(&desired)
            {
                commands.entity(entity).insert(desired.clone());
            }
        }
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

/// Resolve optic input into the shared hull-local [`aim::CommittedAim`] and `TankCommand`.
///
/// Invariants: decomposition, clamping, and ray resolution share the gun-mount origin;
/// [`resume_commit`] alone owns zero-input identity; mechanical travel is applied before the
/// circular [`optic_margin`] clamp; and intent remains absolute inside both bounds rather than
/// following the current servo lay.
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

    // Treat non-finite memory as absent so a fresh finite resolve can replace it.
    let committed_point = committed.get(tank).filter(|point| point.is_finite());
    let moved = motion.delta != Vec2::ZERO;

    // Fast path before pose guards so [`resume_commit`]'s preserved point is always re-authored.
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
        command.aim = Some(publish.command_aim);
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
struct FreeAimServos<'w, 's> {
    slots: Query<'w, 's, &'static ServoIndex>,
    specs: Query<'w, 's, &'static ServoSpec>,
    sims: Query<'w, 's, &'static TankSim>,
    tables: Query<'w, 's, &'static RangeTable>,
    ranging: Res<'w, Ranging>,
}

#[allow(clippy::too_many_arguments)]
fn drive_free_aim(
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
                    servos
                        .sims
                        .get(tank)
                        .ok()
                        .zip(servos.slots.get(servo).ok())
                        .and_then(|(sim, slot)| sim.servos.get(slot.0))
                        .map(crate::tank::ServoState::current)
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
        command.aim = Some(resolved);
    }
}

/// Show/hide the black overlay + prompt when the active view's crewman is dead. The prompt tells
/// the player to press Lshift to switch to the other view if its crewman is alive; if both are
/// dead, the prompt says so (the tank is effectively dead — 0 living crew imminent).
///
/// On the NET client this participates in the overlay authority: it runs in
/// [`overlay::OverlaySet::Declare`] and only DECLARES `Overlay::ViewDead` presence (+ refreshes the
/// prompt text), leaving the visibility swap to the shared `overlay::apply_overlay_visibility`
/// reconciler that runs AFTER `Declare`. That split is the ordering fix: the one-scrim decision reads a
/// fully-declared set, so this black is suppressed entirely whenever a higher overlay (the death screen
/// above all, but also the menu / connect screen) owns the scrim — whole-crew death shows "YOU DIED",
/// not this black. In single-player the `Overlays` resource is absent (`Option` is `None`) and this
/// system sets the node's visibility itself, standalone as before: crewman down → black + prompt.
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

    // Refresh the prompt whenever the active crewman is down (identical in both modes); a hidden node's
    // stale text is harmless and re-derived here before it can be shown again.
    if crewman_down {
        let other_available = view_available(&controlled, &views, other_view);
        *text = Text::new(if other_available {
            format!("Crewman down — [Lshift] for {other_label}")
        } else {
            "All view crew down".to_string()
        });
    }

    match overlays {
        // Net client: DECLARE presence only; the shared reconciler owns visibility from the fully-
        // declared set (suppressed under any higher overlay). We must not also write `Visibility` here
        // — that would double-write it and read a not-yet-reconciled set.
        Some(mut overlays) => overlays.declare(Overlay::ViewDead, crewman_down),
        // Single-player: no authority — draw whenever the active crewman is down.
        None => {
            vis.set_if_neq(if crewman_down {
                Visibility::Visible
            } else {
                Visibility::Hidden
            });
        }
    }
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

    /// Regression: only the controlled tank is hidden in the optic.
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

    /// Regression: the continuous reconcile handles mesh attachment and control changes.
    #[test]
    fn reconcile_lays_layers_across_transitions() {
        let mut app = App::new();
        app.init_resource::<SightMode>();
        app.add_systems(Update, reconcile_optic_render_layers);

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
