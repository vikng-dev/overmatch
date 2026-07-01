//! The gunner's sight — System B (coaxial, player-solved ranging). See
//! `.agents/docs/design/gunner-sight.md`.
//!
//! Lshift toggles between the free third-person "commander" view (the orbit camera + `aim.rs`) and
//! a zoomed gunner optic locked to the gun's line of sight. In gunner view the camera shows the
//! gun's *reality* (the bore), and aiming is **world-space position control**: mouse deltas move a
//! committed hull-local aim direction (`GunnerIntent`), which is published as the shared `AimPoint`
//! that every servo chases at its authored slew rate — so the view lags and settles (dead-stop on
//! release, *not* rate control), and the hull MG traverses alongside the gun. Pure line of sight:
//! superelevation (the ballistic lob for range) is a firing-side concern deferred to its own slice.

use bevy::camera::visibility::RenderLayers;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;

use crate::aim::{AimCommit, AimPoint};
use crate::camera::GunnerCameraPlaced;
use crate::damage::ControlledTank;
use crate::firecontrol::{RangeTable, Ranging};
use crate::spec::ViewKind;
use crate::state::GameplaySet;
use crate::tank::{
    Controlled, Gun, Hull, Rig, ServoState, Tank, TankViews, Turret, shortest_angle,
};

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

/// Run condition: the gunner optic is active AND the gunner is alive (otherwise the view is dark
/// and the player gets a prompt to switch).
pub fn in_gunner(mode: Res<SightMode>) -> bool {
    *mode == SightMode::Gunner
}

/// Run condition: the free third-person view is active AND the commander is alive.
pub fn in_third_person(mode: Res<SightMode>) -> bool {
    *mode == SightMode::ThirdPerson
}

/// The committed gunner aim direction in the hull's local frame (radians): the *intent* the gun
/// chases. Mouse deltas move it (position control); it is NOT the gun's live lay, which lags.
#[derive(Resource, Default)]
struct GunnerIntent {
    yaw: f32,
    pitch: f32,
}

impl GunnerIntent {
    /// The intent as a direction in the hull's local frame. Inverse of the yaw/pitch decomposition
    /// `aim.rs` uses (`yaw = atan2(-x, -z)`, `pitch = atan2(y, |xz|)`), so the reticle agrees with
    /// what the servos are commanded toward.
    fn local_dir(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(-sy * cp, sp, -cy * cp)
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
        .init_resource::<GunnerIntent>()
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
                toggle_sight,
                // Commits the shared `AimPoint` from the magnified intent (in `AimCommit`, so
                // `aim::drive_aim_servos` reads it after — same as third-person).
                drive_gunner_aim.run_if(in_gunner).in_set(AimCommit),
                update_view_death_overlay,
                // After `toggle_sight`, so a refused switch this frame shows its reason.
                update_toast,
                update_range_readout,
            )
                .chain()
                .in_set(GameplaySet),
        )
        // React to a view-mode change by re-laying the controlled tank's render layer (hidden from
        // the optic / shown otherwise). After `toggle_sight` so it sees this frame's mode.
        .add_systems(
            Update,
            sync_optic_render_layer
                .run_if(resource_changed::<SightMode>)
                .after(toggle_sight)
                .in_set(GameplaySet),
        )
        // The intent cursor reprojects through the gunner camera, so it runs after the camera's pose
        // is final for the frame. Both inputs are render-rate — `intent` (mouse, Update) and the
        // camera pose (which reads the gun's `GlobalTransform`, driven by `drive_servos` in Update)
        // — so the reprojection is clean by construction, no aliasing.
        .add_systems(
            PostUpdate,
            (update_intent_reticle, update_ranging_reticle)
                .in_set(GameplaySet)
                .after(TransformSystems::Propagate)
                .after(GunnerCameraPlaced),
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
fn spawn_view_death_overlay(mut commands: Commands) {
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
            Visibility::Hidden,
        ))
        .with_children(|parent| {
            parent.spawn((
                ViewDeathText,
                Text::new(""),
                TextFont {
                    font_size: FontSize::Px(20.0),
                    ..default()
                },
                TextColor(Color::srgb(0.9, 0.4, 0.3)),
            ));
        });
}

/// The refusal-toast text node: a centred banner in the upper third, hidden until a refused switch
/// raises it. Its own entity carries both `Text` and `Visibility`, so `update_toast` writes one query.
fn spawn_toast(mut commands: Commands) {
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
                    font_size: FontSize::Px(22.0),
                    ..default()
                },
                TextColor(Color::srgb(1.0, 0.75, 0.3)),
                Visibility::Hidden,
            ));
        });
}

/// The dialed-range readout, parked bottom-left; populated/shown only in the optic.
fn spawn_range_readout(mut commands: Commands) {
    commands.spawn((
        RangeReadout,
        Text::new(""),
        TextFont {
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
fn spawn_ranging_reticle(mut commands: Commands) {
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

/// Place the intent cursor at the reprojection of the committed intent direction. A *direction*
/// projects to one screen pixel regardless of distance along the ray (perspective), so the point is
/// `cam_pos + dir` — the constant does no work, it's just to give `world_to_viewport` a point. As
/// the gun (and so the camera/sight line) catches up, this drifts back to screen centre; hidden
/// outside gunner view.
///
/// Both inputs are render-rate — `intent` (mouse, `Update`) and the gunner camera's pose (which
/// reads the gun's `GlobalTransform`, driven by `drive_servos` in `Update`) — so the reprojection
/// is a pure function of two same-clock values: no aliasing.
fn update_intent_reticle(
    mode: Res<SightMode>,
    intent: Res<GunnerIntent>,
    camera: Single<(&Camera, &GlobalTransform)>,
    controlled: Query<&Rig, With<Controlled>>,
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
    let Ok(rig) = controlled.single() else {
        *visibility = Visibility::Hidden;
        return;
    };
    let Ok(hull) = hull.get(rig.hull) else {
        return;
    };
    let (camera, cam_transform) = *camera;

    // Intent direction in world space, as a point one unit along it from the camera.
    let dir = hull.rotation() * intent.local_dir();
    let point = cam_transform.translation() + dir;

    match camera.world_to_viewport(cam_transform, point) {
        Ok(screen) => {
            node.left = Val::Px(screen.x - 4.0);
            node.top = Val::Px(screen.y - 4.0);
            *visibility = Visibility::Visible;
        }
        Err(_) => *visibility = Visibility::Hidden,
    }
}

/// Lshift flips the view — but only if the target view's crewman is alive. Entering gunner view
/// seeds the intent from the gun's *current* lay (not its commanded target — seeding from `target`
/// yanks the intent ahead of the gun by however far it was still slewing, and the lead clamp then
/// snaps it back → a jump on handover). The sight line is the bore (pure LOS). The controlled tank's
/// own mesh is hidden from the optic by `sync_optic_render_layer` (reacting to the mode change), so
/// the camera parked inside the mantlet sees through its own geometry.
fn toggle_sight(
    keys: Res<ButtonInput<KeyCode>>,
    mut mode: ResMut<SightMode>,
    mut intent: ResMut<GunnerIntent>,
    controlled: ControlledTank,
    views: Query<&TankViews, With<Controlled>>,
    turret: Query<&ServoState, (With<Turret>, Without<Gun>)>,
    gun: Query<&ServoState, (With<Gun>, Without<Turret>)>,
    ranging: Res<Ranging>,
    tables: Query<&RangeTable>,
    mut toast: ResMut<Toast>,
) {
    if !keys.just_pressed(KeyCode::ShiftLeft) {
        return;
    }
    let Some((turret_entity, gun_entity, muzzle_entity)) =
        controlled.rig().map(|r| (r.turret, r.gun, r.muzzle))
    else {
        return;
    };
    *mode = match *mode {
        SightMode::ThirdPerson => {
            // Only switch to gunner optic if the gunner view is usable (gunner alive) — otherwise
            // toast the reason, since the switch silently does nothing.
            if !view_available(&controlled, &views, ViewKind::Gunner) {
                toast.show(format!("{} unavailable", ViewKind::Gunner.label()));
                return;
            }
            if let (Ok(t), Ok(g)) = (turret.get(turret_entity), gun.get(gun_entity)) {
                // The gun's live pitch carries the superelevation lob; seed the intent from the sight
                // line (bore − lob), not the raised bore, or the view jumps θ on handover.
                let theta = tables
                    .get(muzzle_entity)
                    .map_or(0.0, |table| table.superelevation(ranging.range));
                intent.yaw = t.current();
                intent.pitch = g.current() - theta;
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

/// Hide the controlled tank from its own gunner optic via **render layers, not `Visibility`**. While
/// in the optic, the controlled tank's render meshes move to [`OPTIC_HIDDEN_LAYER`]; otherwise they
/// sit on the default layer 0 the camera draws. Render layers are per-camera and, unlike
/// `Visibility`, are not co-owned by Avian's debug renderer (`PhysicsDebugPlugin` rewrites mesh
/// `Visibility` when gizmos are toggled), so a debug toggle can no longer defeat the hide. Runs only
/// when the view mode changes; `RenderLayers` does not inherit, so it is set on each render mesh.
fn sync_optic_render_layer(
    mode: Res<SightMode>,
    controlled: Query<Entity, With<Controlled>>,
    tanks: Query<Entity, With<Tank>>,
    children: Query<&Children>,
    meshes: Query<(), With<Mesh3d>>,
    mut commands: Commands,
) {
    let controlled_tank = controlled.single().ok();
    for tank in &tanks {
        let hidden = *mode == SightMode::Gunner && Some(tank) == controlled_tank;
        let layer = if hidden {
            RenderLayers::layer(OPTIC_HIDDEN_LAYER)
        } else {
            RenderLayers::layer(0)
        };
        for entity in children.iter_descendants(tank) {
            if meshes.contains(entity) {
                commands.entity(entity).insert(layer.clone());
            }
        }
    }
}

/// World-space position-control aiming. Mouse deltas accumulate into the committed hull-local
/// intent, which is published as the shared `AimPoint` (a far point along the intent's line of
/// sight) — so `aim::drive_aim_servos` chases it with *every* servo, the gun and the hull MG alike,
/// at their own slew rates. No servo command is written here; this only moves the aim point.
///
/// The intent is clamped to a circular **margin** — it may lead the gun's *current* lay by at most
/// `LEAD_MARGIN` of *angular* distance — so the cursor can't run off-screen ahead of the slow
/// turret: pegged at the margin means "slewing at max," near centre means "caught up." The clamp is
/// circular (not per-axis) so diagonal lead feels uniform — a square clamp let you lead ~√2·margin
/// on the diagonal — and yaw is wrapped (`shortest_angle`) so continuous traverse past ±π doesn't
/// yank the intent across the wrap. This is the on-screen-cursor bound, distinct from the gun's
/// mechanical travel limits, which `drive_servos` still enforces. The gun chain (`Turret`/`Gun`)
/// is the lead reference; the hull MG simply rides the same point.
fn drive_gunner_aim(
    motion: Res<AccumulatedMouseMotion>,
    mut intent: ResMut<GunnerIntent>,
    controlled: ControlledTank,
    turret: Query<&ServoState, (With<Turret>, Without<Gun>)>,
    gun: Query<&ServoState, (With<Gun>, Without<Turret>)>,
    ranging: Res<Ranging>,
    tables: Query<&RangeTable>,
    mut aim_point: Query<&mut AimPoint>,
) {
    let Some(rig) = controlled.rig() else {
        return;
    };

    // Radians of commanded aim per mouse count. Low because the optic is magnified — a small angle
    // is a big screen move at the gunner FOV. (Future refinement: scale with the zoom FOV.)
    const SENSITIVITY: f32 = 0.0005;
    // Max angular distance the intent may lead the gun's live lay (rad, ~2.3°) — keeps the cursor
    // inside the optic.
    const LEAD_MARGIN: f32 = 0.04;
    // Distance to the published aim point: far enough that all mounts aim essentially parallel
    // (boresighted along the intent), since there's no committed convergence range yet.
    const AIM_RANGE: f32 = 10_000.0;

    intent.yaw -= motion.delta.x * SENSITIVITY;
    intent.pitch -= motion.delta.y * SENSITIVITY;

    let Ok(t_state) = turret.get(rig.turret) else {
        return;
    };
    let Ok(g_state) = gun.get(rig.gun) else {
        return;
    };

    // Superelevation for the dialed range; the gun's live pitch carries it, so the sight line (which
    // the intent tracks) is the gun's lay minus the lob.
    let theta = tables
        .get(rig.muzzle)
        .map_or(0.0, |table| table.superelevation(ranging.range));

    // Lead as a 2D angular vector from the gun chain's current *sight line* (lay − lob). Yaw uses
    // shortest-angle difference so continuous traverse doesn't wind up. `drive_servos` runs after
    // `GameplaySet`, so `current` is the prior frame's integrated angle — clamp and chase share one
    // clock.
    let yaw_offset = shortest_angle(intent.yaw - t_state.current());
    let sight_now = g_state.current() - theta;
    let pitch_offset = intent.pitch - sight_now;

    // Circular clamp: preserve direction, cap magnitude. Within the margin the intent is left
    // untouched (absolute, hull-local) so the gun genuinely catches up as it slews — re-pinning to
    // `current + offset` each frame would make the target recede with the gun (never arrives).
    let len = (yaw_offset * yaw_offset + pitch_offset * pitch_offset).sqrt();
    if len > LEAD_MARGIN {
        let scale = LEAD_MARGIN / len;
        intent.yaw = t_state.current() + yaw_offset * scale;
        intent.pitch = sight_now + pitch_offset * scale;
    }

    // Publish the raw sight-line intent as the shared aim point: a far point (mounts aim ~parallel),
    // hull-local so it rides with the tank (unstabilized). `drive_aim_servos` lobs it by the
    // superelevation, raising the bore above the line of sight; this stays the intention.
    if let Ok(mut aim_point) = aim_point.get_mut(rig.turret) {
        aim_point.0 = Some(intent.local_dir() * AIM_RANGE);
    }
}

/// Show/hide the black overlay + prompt when the active view's crewman is dead. The prompt tells
/// the player to press Lshift to switch to the other view if its crewman is alive; if both are
/// dead, the prompt says so (the tank is effectively dead — 0 living crew imminent).
fn update_view_death_overlay(
    mode: Res<SightMode>,
    controlled: ControlledTank,
    views: Query<&TankViews, With<Controlled>>,
    mut overlay: Query<&mut Visibility, With<ViewDeathOverlay>>,
    mut label: Query<&mut Text, With<ViewDeathText>>,
) {
    if controlled.entity().is_none() {
        return;
    }
    // The overlay's `Visibility` lives on the full-screen node; its prompt `Text` on the child.
    let (Ok(mut vis), Ok(mut text)) = (overlay.single_mut(), label.single_mut()) else {
        return;
    };

    let (active_view, other_view, other_label) = match *mode {
        SightMode::ThirdPerson => (ViewKind::Commander, ViewKind::Gunner, "gunner optic"),
        SightMode::Gunner => (ViewKind::Gunner, ViewKind::Commander, "third-person"),
    };

    if view_available(&controlled, &views, active_view) {
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
