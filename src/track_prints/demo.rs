//! `OVERMATCH_TRACK_PRINTS_DEMO=<x>,<z>,<heading°>[,<steer>]`, a dev capture of the track prints
//! (cosmetic terrain marks) on the offline route. At [`PLACE_AT_S`] the player's tank is set down at
//! world XZ `(x, z)`, its travel heading `heading°` from `+Z` toward `+X`; once settled it drives
//! straight, stops, pivots on the spot at `steer` (default 1, right), stops, and drives out of the
//! pivot. The camera is then held low, about 10 m off, first on the straight drive's track and then
//! on the pivot, each at the three relief levels in turn ([`crate::settings::TrackPrintRelief`],
//! forced through [`super::ReliefOverride`]) for `OVERMATCH_CAPTURE` stills. Every phase boundary is
//! logged with its app time.
//!
//! The three spots on the shipped map, with a hidden window (fixture data: stretches where one
//! pack dominates the mask, the same spots `OVERMATCH_START` names):
//!
//! ```text
//! OVERMATCH_OFFLINE=1 SPIKE_SIM_WINDOWED=1 OVERMATCH_TRACK_PRINTS_DEMO=599.9,-599.9,195 \
//!   OVERMATCH_CAPTURE=<dir>/bay1-track-prints-s2-mud.png \
//!   OVERMATCH_CAPTURE_AT=38.5,40.5,42.5,44.5,46.5,48.5 \
//!   OVERMATCH_CAPTURE_UI=0 OVERMATCH_CAPTURE_GIZMOS=0 target/debug/overmatch
//! OVERMATCH_TRACK_PRINTS_DEMO=302.5,542.7,315,-1   (sand, same otherwise)
//! OVERMATCH_TRACK_PRINTS_DEMO=70,-20,90            (the base pack)
//! ```
//!
//! Mounted only by the offline composition (`run_offline`): nothing it writes leaves the process.

use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
use bevy::prelude::*;

use crate::camera::{CameraCaptive, CaptiveCameraPlaced};
use crate::command::TankCommand;
use crate::settings::TrackPrintRelief;
use crate::tank::{Controlled, Tank};
use crate::view::PlayerView;

use super::ReliefOverride;

/// When the tank is set down, app seconds: the duel has spawned by then (TUNED).
const PLACE_AT_S: f32 = 10.0;
/// The phases after it, s each (TUNED). A stop is long enough for the hull to coast to rest,
/// which is when the box takes a pivot rather than a turn.
const SETTLE_S: f32 = 3.0;
const DRIVE_S: f32 = 6.0;
const STOP_S: f32 = 3.0;
const PIVOT_S: f32 = 7.0;
const EXIT_S: f32 = 4.0;
const REST_S: f32 = 1.0;
/// How long each view holds each relief level, s: the capture lands 1.5 s into it, after the
/// material has taken the level.
const HOLD_S: f32 = 2.0;
/// The speed a stop brakes down to, m/s: above the box's reversal speed, so the brake never turns
/// into reverse (TUNED).
const BRAKE_ABOVE_MPS: f32 = 0.6;
/// The two low views: slant distance, m, and elevation, degrees. ASSUMED: about 10 m off, low
/// enough that a rut's far wall and its shadow read.
const DRIVE_VIEW: (f32, f32) = (10.0, 12.0);
const PIVOT_VIEW: (f32, f32) = (10.0, 14.0);
/// How far the camera stands off the incoming track's line, degrees, away from the exit side.
const VIEW_ASIDE_DEG: f32 = 30.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Wait,
    Settle,
    Drive,
    Stop,
    Pivot,
    Halt,
    Exit,
    Rest,
    /// The straight drive's track, at relief level `.0`.
    DriveView(usize),
    /// The pivot, at relief level `.0`.
    PivotView(usize),
}

impl Phase {
    fn at(t: f32) -> Self {
        let mut end = PLACE_AT_S;
        for (phase, length) in [
            (Phase::Wait, 0.0),
            (Phase::Settle, SETTLE_S),
            (Phase::Drive, DRIVE_S),
            (Phase::Stop, STOP_S),
            (Phase::Pivot, PIVOT_S),
            (Phase::Halt, STOP_S),
            (Phase::Exit, EXIT_S),
            (Phase::Rest, REST_S),
        ] {
            end += length;
            if t < end {
                return phase;
            }
        }
        let levels = TrackPrintRelief::ORDER.len();
        let hold = ((t - end) / HOLD_S) as usize;
        if hold < levels {
            Phase::DriveView(hold)
        } else {
            Phase::PivotView((hold - levels).min(levels - 1))
        }
    }

    /// The driver's `(throttle, steer)`, pivoting at `pivot`.
    fn command(self, pivot: f32) -> (f32, f32) {
        match self {
            Phase::Drive | Phase::Exit => (1.0, 0.0),
            Phase::Pivot => (0.0, pivot),
            _ => (0.0, 0.0),
        }
    }

    /// The relief level a view phase forces.
    fn relief(self) -> Option<TrackPrintRelief> {
        match self {
            Phase::DriveView(level) | Phase::PivotView(level) => {
                TrackPrintRelief::ORDER.get(level).copied()
            }
            _ => None,
        }
    }
}

#[derive(Resource)]
struct Demo {
    at: Vec2,
    /// Travel heading, rad from `+Z` toward `+X`.
    heading: f32,
    /// The pivot's steer.
    pivot_steer: f32,
    phase: Phase,
    /// The tank's ground position at the start of the drive, the pivot and the exit.
    drive_from: Option<Vec3>,
    pivot: Option<Vec3>,
    exit_from: Option<Vec3>,
    exit_to: Option<Vec3>,
}

pub(crate) fn offline_plugin(app: &mut App) {
    let Ok(spec) = std::env::var("OVERMATCH_TRACK_PRINTS_DEMO") else {
        return;
    };
    let parts: Vec<f32> = spec
        .split(',')
        .filter_map(|part| part.trim().parse().ok())
        .collect();
    let (x, z, heading, pivot_steer) = match parts[..] {
        [x, z, heading] => (x, z, heading, 1.0),
        [x, z, heading, steer] => (x, z, heading, steer),
        _ => {
            warn!("track prints demo: want <x>,<z>,<heading°>[,<steer>], got {spec:?} — not armed");
            return;
        }
    };
    info!("track prints demo: armed at ({x}, {z}) heading {heading}°, set down at {PLACE_AT_S} s");
    app.insert_resource(Demo {
        at: Vec2::new(x, z),
        heading: heading.to_radians(),
        pivot_steer,
        phase: Phase::Wait,
        drive_from: None,
        pivot: None,
        exit_from: None,
        exit_to: None,
    })
    .add_systems(Update, (place, follow).chain())
    .add_systems(
        RunFixedMainLoop,
        drive
            .after(crate::command::gather_commands)
            .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop),
    )
    .add_systems(PostUpdate, hold_camera.in_set(CaptiveCameraPlaced));
}

/// Set the player's tank down on the demo's spot, once.
fn place(
    time: Res<Time>,
    mut demo: ResMut<Demo>,
    grid: Option<Res<crate::terrain_grid::HeightGrid>>,
    mut tank: Query<
        (
            &mut Position,
            &mut Rotation,
            &mut LinearVelocity,
            &mut AngularVelocity,
        ),
        (With<Tank>, With<Controlled>),
    >,
) {
    if demo.phase != Phase::Wait || time.elapsed_secs() < PLACE_AT_S {
        return;
    }
    let Ok((mut position, mut rotation, mut linear, mut angular)) = tank.single_mut() else {
        warn_once!("track prints demo: no player tank at {PLACE_AT_S} s");
        return;
    };
    // The hull faces its own −Z: a turn of `heading + π` about Y points it along the heading.
    position.0 = crate::terrain_grid::spawn_pos(grid.as_deref(), demo.at);
    rotation.0 = Quat::from_rotation_y(demo.heading + std::f32::consts::PI);
    linear.0 = Vec3::ZERO;
    angular.0 = Vec3::ZERO;
    demo.phase = Phase::Settle;
    info!(
        "track prints demo: set down at {:.1} at t={:.2}s",
        position.0,
        time.elapsed_secs()
    );
}

/// Track the phase, note where the tank stands at each boundary the camera aims from, and force
/// each view's relief level.
fn follow(
    time: Res<Time>,
    mut demo: ResMut<Demo>,
    tank: Query<&GlobalTransform, (With<Tank>, With<Controlled>)>,
    mut captive: ResMut<CameraCaptive>,
    prints: Option<Res<super::TrackPrints>>,
    mut commands: Commands,
) {
    if demo.phase == Phase::Wait {
        return;
    }
    let now = Phase::at(time.elapsed_secs());
    if now == demo.phase {
        return;
    }
    let Ok(tank) = tank.single() else {
        return;
    };
    let at = tank.translation();
    match now {
        Phase::Drive => demo.drive_from = Some(at),
        Phase::Pivot => demo.pivot = Some(at),
        Phase::Exit => demo.exit_from = Some(at),
        Phase::Rest => demo.exit_to = Some(at),
        Phase::DriveView(0) => *captive = CameraCaptive(true),
        _ => {}
    }
    if let Some(relief) = now.relief() {
        commands.insert_resource(ReliefOverride(relief));
    }
    info!(
        "track prints demo: {now:?} from t={:.2}s, tank at {at:.2}, {} stamps logged",
        time.elapsed_secs(),
        prints.map_or(0, |prints| prints.stores.log.len()),
    );
    demo.phase = now;
}

/// Drive the player's tank through the phase's command, over whatever the devices wrote. A stop
/// holds the opposite throttle, the service brake, until the hull is near rest: released, the box
/// coasts.
fn drive(
    demo: Res<Demo>,
    mut tanks: Query<(&mut TankCommand, &LinearVelocity, &Rotation), With<Controlled>>,
) {
    let (throttle, steer) = demo.phase.command(demo.pivot_steer);
    for (mut command, velocity, rotation) in &mut tanks {
        // The hull faces its own −Z.
        let ahead = velocity.0.dot(rotation.0 * Vec3::NEG_Z);
        // Every phase that does not drive brakes, so the tank holds still under the views.
        command.throttle = match demo.phase {
            Phase::Drive | Phase::Exit | Phase::Pivot => throttle,
            _ if ahead.abs() > BRAKE_ABOVE_MPS => -ahead.signum(),
            _ => throttle,
        };
        command.steer = steer;
    }
}

/// Hold the player's camera low on the straight drive's track, then on the pivot.
fn hold_camera(
    demo: Res<Demo>,
    grid: Option<Res<crate::terrain_grid::HeightGrid>>,
    mut camera: Query<&mut Transform, With<PlayerView>>,
) {
    let (Some(from), Some(pivot), Some(exit_from), Some(exit_to), Ok(mut camera)) = (
        demo.drive_from,
        demo.pivot,
        demo.exit_from,
        demo.exit_to,
        camera.single_mut(),
    ) else {
        return;
    };
    let flat = |v: Vec3| Vec2::new(v.x, v.z).normalize_or_zero();
    let incoming = flat(pivot - from);
    let outgoing = flat(exit_to - exit_from);
    // The side the exit leaves by; the camera stands on the other.
    let exit_side = (outgoing - incoming * outgoing.dot(incoming)).normalize_or_zero();
    let ((slant, elevation), target, back) = match demo.phase {
        // Across the drive's track from the exit side's far flank, looking at the middle of the run.
        Phase::DriveView(_) => {
            let middle = from.lerp(pivot, 0.5);
            let aside = VIEW_ASIDE_DEG.to_radians();
            (
                DRIVE_VIEW,
                middle,
                -exit_side * aside.cos() - incoming * aside.sin(),
            )
        }
        // Behind the pivot along the incoming track, turned away from the side the exit leaves by.
        Phase::PivotView(_) => {
            let aside = VIEW_ASIDE_DEG.to_radians();
            (
                PIVOT_VIEW,
                pivot,
                -incoming * aside.cos() - exit_side * aside.sin(),
            )
        }
        _ => return,
    };
    let elevation = elevation.to_radians();
    let ground_y = grid.map_or(0.0, |grid| grid.height_at(target.x, target.z));
    let ground = Vec3::new(target.x, ground_y, target.z);
    let eye = ground
        + Vec3::new(back.x, 0.0, back.y) * slant * elevation.cos()
        + Vec3::Y * slant * elevation.sin();
    *camera = Transform::from_translation(eye).looking_at(ground, Vec3::Y);
}
