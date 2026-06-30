//! The gunner's sight — System B (coaxial, player-solved ranging). See
//! `.agents/docs/design/gunner-sight.md`.
//!
//! Lshift toggles between the free third-person "commander" view (the orbit camera + `aim.rs`) and
//! a zoomed gunner optic locked to the gun's line of sight. In gunner view the camera shows the
//! gun's *reality* (the sight line), and aiming is **world-space position control**: mouse deltas
//! move a committed hull-local aim direction (`GunnerIntent`); the turret/gun servos chase it at
//! their authored slew rate, so the view lags and settles — dead-stop on release, *not* rate
//! control. Range is dialed by scroll and sets the gun's superelevation above the sight line, so
//! the barrel elevates while the reticle stays on target and the shell arcs back onto it.

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;

use crate::camera::GunnerCameraPlaced;
use crate::damage::{Capability, ControlledTank};
use crate::state::GameplaySet;
use crate::tank::{
    Controlled, Gun, Hull, Rig, ServoCommand, ServoState, Tank, Turret, shortest_angle,
};

/// 88 mm muzzle velocity (m/s) — mirrors `shooting`'s muzzle speed; used for the superelevation
/// solution. (The shells are gravity-only, so the flat-fire formula below is exact for them.)
const MUZZLE_SPEED: f32 = 773.0;

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

/// Dialed range (m), set by scroll in gunner view. Drives superelevation — the Tiger has no
/// rangefinder, so the player estimates and dials it (the WW2 gunnery skill).
#[derive(Resource)]
pub struct Ranging {
    pub range: f32,
}

impl Default for Ranging {
    fn default() -> Self {
        Self { range: 800.0 }
    }
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

/// Gun elevation above the line of sight for a flat-fire, drag-free gravity solution:
/// `θ ≈ g·R / (2·v²)`. The shells are gravity-only (`shooting.rs`), so this is exact for them.
pub fn superelevation(range: f32) -> f32 {
    const G: f32 = 9.81;
    G * range / (2.0 * MUZZLE_SPEED * MUZZLE_SPEED)
}

pub fn plugin(app: &mut App) {
    app.init_resource::<SightMode>()
        .init_resource::<Ranging>()
        .init_resource::<GunnerIntent>()
        .add_systems(Startup, (spawn_intent_reticle, spawn_view_death_overlay))
        .add_systems(
            Update,
            (
                toggle_sight,
                drive_gunner_aim.run_if(in_gunner),
                adjust_range.run_if(in_gunner),
                update_view_death_overlay,
            )
                .chain()
                .in_set(GameplaySet),
        )
        // The intent cursor reprojects through the gunner camera, so it runs after the camera's pose
        // is final for the frame. Both inputs are render-rate — `intent` (mouse, Update) and the
        // camera pose (which reads the gun's `GlobalTransform`, driven by `drive_servos` in Update)
        // — so the reprojection is clean by construction, no aliasing.
        .add_systems(
            PostUpdate,
            update_intent_reticle
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
                Text::new(""),
                TextFont {
                    font_size: FontSize::Px(20.0),
                    ..default()
                },
                TextColor(Color::srgb(0.9, 0.4, 0.3)),
            ));
        });
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
/// snaps it back → a jump on handover). The sight-line pitch is the gun's bore minus the current
/// superelevation. Entering also hides the tank root so the optic (parked at the gun pivot, inside
/// the mantlet) clips no own geometry; leaving restores it.
fn toggle_sight(
    keys: Res<ButtonInput<KeyCode>>,
    mut mode: ResMut<SightMode>,
    mut intent: ResMut<GunnerIntent>,
    ranging: Res<Ranging>,
    controlled: ControlledTank,
    turret: Query<&ServoState, (With<Turret>, Without<Gun>)>,
    gun: Query<&ServoState, (With<Gun>, Without<Turret>)>,
    mut tank_vis: Query<&mut Visibility, With<Tank>>,
) {
    if !keys.just_pressed(KeyCode::ShiftLeft) {
        return;
    }
    let (Some(tank), Some((turret_entity, gun_entity))) = (
        controlled.entity(),
        controlled.rig().map(|r| (r.turret, r.gun)),
    ) else {
        return;
    };
    *mode = match *mode {
        SightMode::ThirdPerson => {
            // Only switch to gunner optic if the gunner is alive.
            if !controlled.available(Capability::GunnerSight) {
                return;
            }
            if let (Ok(t), Ok(g)) = (turret.get(turret_entity), gun.get(gun_entity)) {
                intent.yaw = t.current();
                // Sight-line pitch = bore − superelevation; seeding the bore would re-elevate on top.
                intent.pitch = g.current() - superelevation(ranging.range);
            }
            // Hide only the controlled tank's mesh — the optic sits inside its mantlet.
            if let Ok(mut v) = tank_vis.get_mut(tank) {
                *v = Visibility::Hidden;
            }
            SightMode::Gunner
        }
        SightMode::Gunner => {
            // Only switch to third-person if the commander is alive.
            if !controlled.available(Capability::CommanderView) {
                return;
            }
            if let Ok(mut v) = tank_vis.get_mut(tank) {
                *v = Visibility::Inherited;
            }
            SightMode::ThirdPerson
        }
    };
}

/// World-space position-control aiming. Mouse deltas accumulate into the committed hull-local
/// intent; the turret/gun servos are commanded to chase it. The gun target carries the
/// superelevation on top of the sight-line pitch, so the barrel rides above the line of sight.
///
/// Gated by the Gunner's `Traverse` capability — a dead gunner freezes the servos (the turret/gun
/// hold their last commanded position).
///
/// The intent is clamped to a circular **margin** — it may lead the gun's *current* lay by at most
/// `LEAD_MARGIN` of *angular* distance — so the cursor can't run off-screen ahead of the slow
/// turret: pegged at the margin means "slewing at max," near centre means "caught up." The clamp is
/// circular (not per-axis) so diagonal lead feels uniform — a square clamp let you lead ~√2·margin
/// on the diagonal — and yaw is wrapped (`shortest_angle`) so continuous traverse past ±π doesn't
/// yank the intent across the wrap. This is the on-screen-cursor bound, distinct from the gun's
/// mechanical travel limits, which `drive_servos` still enforces.
fn drive_gunner_aim(
    motion: Res<AccumulatedMouseMotion>,
    ranging: Res<Ranging>,
    mut intent: ResMut<GunnerIntent>,
    controlled: ControlledTank,
    mut turret: Query<(&mut ServoCommand, &ServoState), (With<Turret>, Without<Gun>)>,
    mut gun: Query<(&mut ServoCommand, &ServoState), (With<Gun>, Without<Turret>)>,
) {
    if !controlled.available(Capability::Traverse) {
        return;
    }
    let Some((turret_entity, gun_entity)) = controlled.rig().map(|r| (r.turret, r.gun)) else {
        return;
    };

    // Radians of commanded aim per mouse count. Low because the optic is magnified — a small angle
    // is a big screen move at the gunner FOV. (Future refinement: scale with the zoom FOV.)
    const SENSITIVITY: f32 = 0.0005;
    // Max angular distance the intent may lead the gun's live lay (rad, ~2.3°) — keeps the cursor
    // inside the optic.
    const LEAD_MARGIN: f32 = 0.04;

    intent.yaw -= motion.delta.x * SENSITIVITY;
    intent.pitch -= motion.delta.y * SENSITIVITY;

    let superelevation = superelevation(ranging.range);

    let Ok((mut t_cmd, t_state)) = turret.get_mut(turret_entity) else {
        return;
    };
    let Ok((mut g_cmd, g_state)) = gun.get_mut(gun_entity) else {
        return;
    };

    // Lead as a 2D angular vector from the gun's current sight-line lay. Yaw uses shortest-angle
    // difference so continuous traverse doesn't wind up. `drive_servos` runs in `Update` after this
    // system, so `current` is this frame's live angle — the clamp and the chase share one clock.
    let yaw_offset = shortest_angle(intent.yaw - t_state.current());
    let sight_now = g_state.current() - superelevation;
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

    t_cmd.target = intent.yaw;
    // The gun's live lay carries the superelevation; the sight line is that minus it.
    g_cmd.target = intent.pitch + superelevation;
}

/// Show/hide the black overlay + prompt when the active view's crewman is dead. The prompt tells
/// the player to press Lshift to switch to the other view if its crewman is alive; if both are
/// dead, the prompt says so (the tank is effectively dead — 0 living crew imminent).
fn update_view_death_overlay(
    mode: Res<SightMode>,
    controlled: ControlledTank,
    mut overlay: Query<(&mut Visibility, &mut Text), With<ViewDeathOverlay>>,
) {
    if controlled.entity().is_none() {
        return;
    }
    let Ok((mut vis, mut text)) = overlay.single_mut() else {
        return;
    };

    let (active_cap, other_cap, other_label) = match *mode {
        SightMode::ThirdPerson => (
            Capability::CommanderView,
            Capability::GunnerSight,
            "gunner optic",
        ),
        SightMode::Gunner => (
            Capability::GunnerSight,
            Capability::CommanderView,
            "third-person",
        ),
    };

    let active_available = controlled.available(active_cap);
    if active_available {
        *vis = Visibility::Hidden;
        return;
    }

    let other_available = controlled.available(other_cap);
    *text = Text::new(if other_available {
        format!("Crewman down — [Lshift] for {other_label}")
    } else {
        "All view crew down".to_string()
    });
    *vis = Visibility::Visible;
}

/// Scroll dials the range in gunner view (range, not zoom — the optic's magnification is fixed).
fn adjust_range(scroll: Res<AccumulatedMouseScroll>, mut ranging: ResMut<Ranging>) {
    const RANGE_STEP: f32 = 50.0;
    const RANGE_MIN: f32 = 50.0;
    const RANGE_MAX: f32 = 4000.0;
    ranging.range = (ranging.range + scroll.delta.y * RANGE_STEP).clamp(RANGE_MIN, RANGE_MAX);
}
