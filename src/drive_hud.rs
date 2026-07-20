//! Shared controlled-tank drive HUD for the offline and predicted-network clients.
//!
//! This is view state only: it reads rollback/predicted components in `Update`, owns one local F3
//! toggle, and writes only UI [`Text`] / [`Visibility`]. It never writes a tank component or enters a
//! fixed schedule.

use avian3d::prelude::{LinearVelocity, Rotation};
use bevy::prelude::*;

use crate::tank::Controlled;
use crate::track::sim::{TankTransmission, TrackDrive, TrackGear, TransmissionFeelTest};
use crate::track::transmission::{
    DriveReadout, SchedulerState, TransmissionMode, TransmissionParams, TransmissionState, readout,
};
use crate::ui_font::UiFonts;

/// The normal-play drive row. One instance is mounted by this plugin in both client roots.
#[derive(Component)]
struct StandardDriveHudText;

/// The detailed drive panel controlled by [`DriveDebugVisible`].
#[derive(Component)]
struct DriveDebugHudText;

/// View-local F3 latch. It is deliberately neither a component on the tank nor replicated state.
#[derive(Resource, Default)]
struct DriveDebugVisible(bool);

/// The HUD refresh set lets the offline-only `T` mode cycle commit before this view reads it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DriveHudUpdate;

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<DriveDebugVisible>()
        .add_systems(Startup, spawn_drive_hud)
        .add_systems(
            Update,
            (toggle_drive_debug, update_drive_hud.in_set(DriveHudUpdate)).chain(),
        );
}

fn spawn_drive_hud(mut commands: Commands, fonts: Res<UiFonts>) {
    // Bottom-left, one line above `crew_ui`'s crew row (bottom 10 px). This is ordinary HUD, not an
    // overlay: it owns no scrim and carries no explicit z-index, so the overlay authority's explicit
    // positive `GlobalZIndex` layers still cover it.
    commands.spawn((
        StandardDriveHudText,
        Text::new(""),
        TextFont {
            font: fonts.body.clone().into(),
            font_size: FontSize::Px(15.0),
            ..default()
        },
        TextColor(Color::srgb(0.92, 0.94, 0.78)),
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(34.0),
            left: Val::Px(10.0),
            ..default()
        },
        Visibility::Hidden,
    ));

    // The diagnostic block keeps the old offline HUD's top-right placement and bare-text treatment.
    // Like the standard row it is ordinary non-interactive HUD, never a second overlay/scrim.
    commands.spawn((
        DriveDebugHudText,
        Text::new(""),
        TextFont {
            font: fonts.body.clone().into(),
            font_size: FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::srgb(0.8, 0.75, 0.5)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(10.0),
            right: Val::Px(12.0),
            ..default()
        },
        Visibility::Hidden,
    ));
}

/// Pure toggle transition used by the input system and the offline HUD regression tests.
pub(crate) const fn debug_visible_after_f3(current: bool, f3_just_pressed: bool) -> bool {
    if f3_just_pressed { !current } else { current }
}

fn toggle_drive_debug(keys: Res<ButtonInput<KeyCode>>, mut visible: ResMut<DriveDebugVisible>) {
    visible.0 = debug_visible_after_f3(visible.0, keys.just_pressed(KeyCode::F3));
}

/// Compact normal-play row. The two suffix markers are independent: `P` is the parking-brake latch
/// and `*` is the active hill-hold latch, hence `F1P*` when both are true. A shift window has no
/// standard-row marker; its scheduler detail belongs to the F3 panel.
///
/// Gear occupies four characters (the Tiger's `F1P*` maximum), rpm stays at one decimal in thousands,
/// and speed is an integer km/h in a three-character field. A Governor/spec-less vehicle passes no
/// readout and therefore renders only speed — no placeholder noise.
pub(crate) fn standard_drive_row(
    transmission: Option<(&TransmissionState, &DriveReadout)>,
    ground_speed_mps: f32,
) -> String {
    let speed = format!("Speed {:>3.0} km/h", ground_speed_mps * 3.6);
    let Some((state, operating)) = transmission else {
        return speed;
    };

    let mut gear = operating.gear_label.clone();
    if state.park {
        gear.push('P');
    }
    if state.hill_hold {
        gear.push('*');
    }
    let rpm = format!("{:>3.1}k", (operating.rpm / 1_000.0).max(0.0));
    format!("Gear {gear:<4}  RPM {rpm}  {speed}")
}

/// Horizontal world-space velocity magnitude. Vertical motion and hull facing are intentionally
/// excluded: the normal HUD promises ground speed, not signed forward speed.
pub(crate) fn horizontal_ground_speed(velocity: Vec3) -> f32 {
    Vec3::new(velocity.x, 0.0, velocity.z).length()
}

/// Render scheduler truth for the drive-debug panel. Grade targets live on whichever ladder the
/// state currently engages, so reverse shifts read `R4->R2`, not a hard-coded forward label.
pub(crate) fn scheduler_hud_line(st: &TransmissionState) -> String {
    match st.scheduler {
        SchedulerState::Normal => "sched NORMAL       ".to_string(),
        SchedulerState::GradeShift { from, to } => {
            let ladder = if st.reverse { 'R' } else { 'F' };
            format!("sched GRADE {ladder}{from}->{ladder}{to}")
        }
        SchedulerState::HillHold => "sched HILL HOLD    ".to_string(),
        SchedulerState::GradeLimit => "sched GRADE LIMIT  ".to_string(),
    }
}

/// Fixed-radius steering visibility from the live detent state and the source radius table retained
/// by [`TransmissionParams`]. Hybrid stays blank: its continuous command target is internal to the
/// solve, while this line promises an authored gear/detent radius.
pub(crate) fn steering_hud_line(
    mode: TransmissionMode,
    st: &TransmissionState,
    authored_radii: Option<&[(f32, f32)]>,
) -> String {
    if mode != TransmissionMode::FixedRadii || st.steer_step == 0 {
        return String::new();
    }
    let Some(radii) = authored_radii.filter(|radii| !radii.is_empty()) else {
        return String::new();
    };
    let gear = usize::from(st.gear).clamp(1, radii.len()) - 1;
    let (tight, wide) = radii[gear];
    let (detent, radius) = if st.steer_step == 1 {
        ("I", wide)
    } else {
        ("II", tight)
    };
    format!("STEER {detent} R~{radius:.0}m")
}

fn selected_mode(
    feel: Option<&TransmissionFeelTest>,
    gear: Option<&TrackGear>,
) -> Option<TransmissionMode> {
    feel.map(|feel| feel.0)
        .or_else(|| gear.map(TrackGear::mode))
}

fn active_transmission(
    mode: Option<TransmissionMode>,
    gear: Option<&TrackGear>,
) -> Option<&TransmissionParams> {
    if mode == Some(TransmissionMode::Governor) {
        return None;
    }
    gear.and_then(TrackGear::trans)
}

/// Rebuild both shared HUD surfaces from the controlled tank's predicted/tick-truth components.
/// The standard row is always visible once a controlled body exists; detailed formatting is skipped
/// entirely while F3 is closed.
fn update_drive_hud(
    feel: Option<Res<TransmissionFeelTest>>,
    gear: Option<Res<TrackGear>>,
    debug_visible: Res<DriveDebugVisible>,
    controlled: Query<
        (&TrackDrive, &TankTransmission, &LinearVelocity, &Rotation),
        With<Controlled>,
    >,
    mut standard_label: Query<
        (&mut Text, &mut Visibility),
        (With<StandardDriveHudText>, Without<DriveDebugHudText>),
    >,
    mut debug_label: Query<
        (&mut Text, &mut Visibility),
        (With<DriveDebugHudText>, Without<StandardDriveHudText>),
    >,
) {
    let Ok((mut standard_text, mut standard_visibility)) = standard_label.single_mut() else {
        return;
    };
    let Ok((mut debug_text, mut debug_visibility)) = debug_label.single_mut() else {
        return;
    };
    let Some((drive, transmission, velocity, rotation)) = controlled.iter().next() else {
        standard_text.0.clear();
        debug_text.0.clear();
        *standard_visibility = Visibility::Hidden;
        *debug_visibility = Visibility::Hidden;
        return;
    };

    let gear = gear.as_deref();
    let mode = selected_mode(feel.as_deref(), gear);
    let params = active_transmission(mode, gear);
    let operating = params.map(|params| readout(&transmission.0, params));
    let standard_operating = operating
        .as_ref()
        .map(|operating| (&transmission.0, operating));
    let ground_speed = horizontal_ground_speed(velocity.0);
    standard_text.0 = standard_drive_row(standard_operating, ground_speed);
    *standard_visibility = Visibility::Visible;

    if !debug_visible.0 {
        *debug_visibility = Visibility::Hidden;
        return;
    }
    *debug_visibility = Visibility::Visible;

    let mode_line = match mode {
        Some(mode) if feel.is_some() => format!("trans [T]: {}", mode.label()),
        Some(mode) => format!("trans: {}", mode.label()),
        None => "trans: --".to_string(),
    };
    let scheduler_line = if params.is_some() {
        scheduler_hud_line(&transmission.0)
    } else {
        "sched --           ".to_string()
    };

    // The debug panel preserves the former offline diagnostics: signed hull-forward speed, per-belt
    // speed/slip, shaped commands, and the authored L600 steering detent/radius.
    let forward = rotation.0 * Vec3::NEG_Z;
    let forward_horizontal = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
    let horizontal = Vec3::new(velocity.0.x, 0.0, velocity.0.z);
    let projected = horizontal.dot(forward_horizontal);
    let signed_ground = horizontal.length() * projected.signum();
    let speeds = [drive.sides[0].speed, drive.sides[1].speed];
    let steering = steering_hud_line(
        mode.unwrap_or(TransmissionMode::Governor),
        &transmission.0,
        params.map(|params| params.steer_radii_m.as_slice()),
    );

    debug_text.0 = format!(
        "{mode_line}\n{scheduler_line}\n\
         hull {signed_ground:+5.2} m/s ({:+6.1} km/h)\n\
         belt L {:+5.2} R {:+5.2} | slip L {:+5.2} R {:+5.2}\n\
         cmd thr {:+5.2} steer {:+5.2} | {steering:<15}",
        signed_ground * 3.6,
        speeds[0],
        speeds[1],
        speeds[0] - projected,
        speeds[1] - projected,
        drive.throttle,
        drive.steer,
    );
}
