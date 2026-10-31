//! `OVERMATCH_TRACK_PRINTS_DEMO=<x>,<z>,<heading°>[,<steer>]`, a dev capture of the track prints
//! (cosmetic terrain marks) on the offline route. At [`PLACE_AT_S`] the player's tank is set down at
//! world XZ `(x, z)`, its travel heading `heading°` from `+Z` toward `+X`; once settled it drives
//! straight, stops, pivots on the spot at `steer` (default 1, right), stops, and drives out of the
//! pivot. The camera is then held low, about 10 m off, first on the straight drive's track and then
//! on the pivot, each at the three relief levels in turn ([`crate::settings::TrackPrintRelief`],
//! forced through [`super::ReliefOverride`]) for `OVERMATCH_CAPTURE` stills. Every phase boundary is
//! logged with its app time.
//!
//! `OVERMATCH_TRACK_PRINTS_DEMO_HOLD=drive|pivot` replaces the views' level cycle: the camera holds
//! that one view for good and nothing overrides the player's relief setting, the scene of the frame
//! cost measurement (`scripts/perf/run-track-prints-cost.sh`). The hold's start is logged on the
//! `Time<Real>` clock the frame stream (`crate::frame_cost`) stamps its rows on.
//!
//! The three spots on the shipped map, with a hidden window (fixture data: stretches where one
//! pack dominates the mask, the same spots `OVERMATCH_START` names):
//!
//! ```text
//! SPIKE_SIM_WINDOWED=1 OVERMATCH_TRACK_PRINTS_DEMO=599.9,-599.9,195 \
//!   OVERMATCH_CAPTURE=<dir>/bay1-track-prints-s2-mud.png \
//!   OVERMATCH_CAPTURE_AT=38.5,40.5,42.5,44.5,46.5,48.5 \
//!   OVERMATCH_CAPTURE_UI=0 OVERMATCH_CAPTURE_GIZMOS=0 target/debug/overmatch --offline
//! OVERMATCH_TRACK_PRINTS_DEMO=302.5,542.7,315,-1   (sand, same otherwise)
//! OVERMATCH_TRACK_PRINTS_DEMO=70,-20,90            (the base pack)
//! ```
//!
//! Mounted only by the offline composition (`run_offline`): nothing it writes leaves the process.
//! Compiled into dev builds, and into an optimized one only under the `track_prints_demo` feature,
//! which no shipped build enables.

use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
use bevy::prelude::*;

use crate::camera::{CameraCaptive, CaptiveCameraPlaced};
use crate::command::TankCommand;
use crate::settings::{Settings, TrackPrintRelief};
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

    /// The phase shown at app second `t`: the scripted one, with `hold` standing in for every view.
    fn shown(t: f32, hold: Option<Phase>) -> Self {
        match (Phase::at(t), hold) {
            (Phase::DriveView(_) | Phase::PivotView(_), Some(held)) => held,
            (scripted, _) => scripted,
        }
    }

    /// The relief level the shown phase forces: none while a view is held, so the player's own
    /// setting is what the material draws.
    fn forced_relief(self, hold: Option<Phase>) -> Option<TrackPrintRelief> {
        if hold.is_some() { None } else { self.relief() }
    }
}

#[derive(Resource)]
struct Demo {
    at: Vec2,
    /// Travel heading, rad from `+Z` toward `+X`.
    heading: f32,
    /// The pivot's steer.
    pivot_steer: f32,
    /// The view held for good in place of the level cycle, at the player's own relief setting.
    hold: Option<Phase>,
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
    let hold = match crate::env_value("OVERMATCH_TRACK_PRINTS_DEMO_HOLD").as_deref() {
        None => None,
        Some("drive") => Some(Phase::DriveView(0)),
        Some("pivot") => Some(Phase::PivotView(0)),
        Some(other) => {
            warn!("track prints demo: want a hold of drive or pivot, got {other:?} — not armed");
            return;
        }
    };
    info!(
        "track prints demo: armed at ({x}, {z}) heading {heading}°, set down at {PLACE_AT_S} s, \
         hold {hold:?}"
    );
    app.insert_resource(Demo {
        at: Vec2::new(x, z),
        heading: heading.to_radians(),
        pivot_steer,
        hold,
        phase: Phase::Wait,
        drive_from: None,
        pivot: None,
        exit_from: None,
        exit_to: None,
    })
    .add_systems(Update, (place, follow, report_hold).chain())
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
/// each view's relief level unless a view is held.
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
    let now = Phase::shown(time.elapsed_secs(), demo.hold);
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
        Phase::DriveView(_) | Phase::PivotView(_) => *captive = CameraCaptive(true),
        _ => {}
    }
    if let Some(relief) = now.forced_relief(demo.hold) {
        commands.insert_resource(ReliefOverride(relief));
    }
    info!(
        "track prints demo: {now:?} from t={:.2}s, tank at {at:.2}, {} stamps logged",
        time.elapsed_secs(),
        prints.map_or(0, |prints| prints.stores.log.len()),
    );
    demo.phase = now;
}

/// Log, once, the held view's start on the frame stream's clock, the relief and range the player's
/// settings give it, and the stamps laid by then: the frame-cost runner's proof of its scene.
fn report_hold(
    demo: Res<Demo>,
    real: Res<Time<Real>>,
    settings: Option<Res<Settings>>,
    prints: Option<Res<super::TrackPrints>>,
    mut reported: Local<bool>,
) {
    if *reported || demo.hold != Some(demo.phase) {
        return;
    }
    let settings = settings.map(|settings| *settings).unwrap_or_default();
    info!(
        "track prints demo: holding {:?} from real t={:.2}s at the player's relief {:?}, range \
         {:?}, {} stamps logged",
        demo.phase,
        real.elapsed_secs_f64(),
        settings.track_prints,
        settings.track_print_range,
        prints.map_or(0, |prints| prints.stores.log.len()),
    );
    *reported = true;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The frame-cost runner measures the player's relief setting on a scene that must not change
    /// under it: with a view held, every second after the run shows that one view and no level is
    /// forced, while the run that lays the prints is the unheld script's, second for second.
    #[test]
    fn a_held_view_stands_for_good_and_forces_no_level() {
        let held = Phase::PivotView(0);
        let mut views = 0;
        for tenth in 0..3_000 {
            let t = tenth as f32 / 10.0;
            let scripted = Phase::at(t);
            let shown = Phase::shown(t, Some(held));
            assert_eq!(shown.forced_relief(Some(held)), None, "t={t}");
            match scripted {
                Phase::DriveView(_) | Phase::PivotView(_) => {
                    assert_eq!(shown, held, "t={t}");
                    views += 1;
                }
                _ => assert_eq!(shown, scripted, "t={t}"),
            }
        }
        assert!(views > 0, "the script never reached its views");
        assert_eq!(
            Phase::at(300.0).forced_relief(None),
            TrackPrintRelief::ORDER.last().copied(),
            "unheld, the last view keeps the last level",
        );
    }
}
