//! The capture's shot lever, in the game's damage model: on the first frame at or after app second
//! `s`, a shipped round in one of its fuze modes is fired at a real tree or at the ground beside it,
//! and the capture's frames are timed from the round's burst. The burst outside a tank opens no pen
//! cam, so the view draws it over the scene: the flight path cyan, a quarter of the fragment cone's
//! directions orange, the burst point a white cross.
//!
//! `OVERMATCH_CAPTURE_SHOT=<round key>,<mode index>,tree|ground@<s>`, with `OVERMATCH_CAPTURE` set;
//! the offline root alone mounts it. A shot that has not burst [`GIVE_UP_S`] after its cue ends
//! the run with a failure code.
use crate::ballistics::{
    ArmourMark, FireShell, PenetrationMarks, RetainSpentShells, ShellPath, SpallMarks,
};
use crate::tank::WeaponRound;
use bevy::prelude::*;

/// App seconds past the cue after which a shot with no burst fails the run (ASSUMED: a 0.1 s delay
/// and the scene's load leave orders of magnitude of slack).
const GIVE_UP_S: f32 = 30.0;

/// The app second the shot burst at, once it has: the capture's frames count from it.
#[derive(Resource)]
pub(super) struct ShotReady(pub(super) Option<f32>);

#[derive(Resource)]
struct Cue {
    round: crate::rounds::Round,
    tree: bool,
    at: f32,
    fired: bool,
    camera: Option<Transform>,
    logged: bool,
}

pub(crate) fn offline_shot_plugin(app: &mut App) {
    if std::env::var_os("OVERMATCH_CAPTURE").is_none() {
        return;
    }
    let Ok(raw) = std::env::var("OVERMATCH_CAPTURE_SHOT") else {
        return;
    };
    let parse = || {
        let (spec, at) = raw.split_once('@')?;
        let fields: Vec<_> = spec.split(',').collect();
        let [key, mode, target] = fields[..] else {
            return None;
        };
        let round = WeaponRound::resolve(key)
            .ok()?
            .in_mode(mode.parse().ok()?)?;
        let at: f32 = at.parse().ok()?;
        if !at.is_finite() || at < 0.0 || !matches!(target, "tree" | "ground") {
            return None;
        }
        Some(Cue {
            round,
            tree: target == "tree",
            at,
            fired: false,
            camera: None,
            logged: false,
        })
    };
    let Some(cue) = parse() else {
        warn!("capture: invalid shot cue {raw:?}");
        return;
    };
    info!("capture: game's damage model shot {raw}");
    app.insert_resource(ShotReady(None))
        .insert_resource(cue)
        .insert_resource(RetainSpentShells(true))
        .insert_resource(crate::camera::CameraCaptive(true))
        .add_systems(Update, fire_on_cue)
        .add_systems(
            PostUpdate,
            show_shot.in_set(crate::camera::CaptiveCameraPlaced),
        );
}

fn fire_on_cue(
    time: Res<Time>,
    mut cue: ResMut<Cue>,
    mut exit: MessageWriter<bevy::app::AppExit>,
    trees: Query<(&crate::scatter::MapInstance, &Transform), With<crate::trees::TreeBody>>,
    grid: Option<Res<crate::terrain_grid::HeightGrid>>,
    mut commands: Commands,
) {
    if !cue.logged && time.elapsed_secs() > cue.at + GIVE_UP_S {
        error!("capture: the shot did not burst within {GIVE_UP_S} s of its cue");
        exit.write(bevy::app::AppExit::error());
        return;
    }
    if cue.fired || time.elapsed_secs() < cue.at {
        return;
    }
    let Some(grid) = grid else {
        return;
    };
    // ASSUMED capture geometry: the lowest map identity supplies a reproducible real tree.
    let Some((_, transform)) = trees.iter().min_by(|a, b| a.0.id.cmp(&b.0.id)) else {
        return;
    };
    let target = if cue.tree {
        transform.translation + Vec3::Y * 2.5
    } else {
        let p = transform.translation + Vec3::X * 10.0;
        Vec3::new(p.x, grid.height_at(p.x, p.z), p.z)
    };
    let origin = if cue.tree {
        target + Vec3::Z * 10.0
    } else {
        target + Vec3::new(0.0, 10.0, 2.0)
    };
    let direction = Dir3::new(target - origin).unwrap();
    commands.trigger(FireShell::of_round(
        &cue.round,
        origin,
        direction,
        crate::spec::FireMechanism::Single,
        true,
        None,
        None,
    ));
    cue.fired = true;
    cue.camera = Some(
        Transform::from_translation(target + Vec3::new(12.0, 7.0, 12.0))
            .looking_at(target, Vec3::Y),
    );
    info!(
        "capture: game's damage model shot origin={origin:?} target={target:?} mode={}",
        cue.round.fuze_mode
    );
}

fn show_shot(
    time: Res<Time>,
    mut ready: ResMut<ShotReady>,
    mut cue: ResMut<Cue>,
    shots: Query<(&PenetrationMarks, &SpallMarks, &ShellPath)>,
    mut cameras: Query<
        &mut Transform,
        (
            With<crate::view::PlayerView>,
            Without<crate::tank::Controlled>,
        ),
    >,
    mut gizmos: Gizmos,
) {
    for (marks, fields, path) in &shots {
        let Some((at, after)) = marks.log.iter().find_map(|mark| match mark {
            ArmourMark::Burst { at, after, .. } => Some((*at, *after)),
            _ => None,
        }) else {
            continue;
        };
        if !cue.logged {
            info!(
                "capture: game's damage model burst at={at:?} after={after} field={}",
                fields.burst.is_some()
            );
            cue.logged = true;
            ready.0 = Some(time.elapsed_secs());
            cue.camera = Some(
                Transform::from_translation(at + Vec3::new(12.0, 7.0, 12.0))
                    .looking_at(at, Vec3::Y),
            );
        }
        for points in path.points.windows(2) {
            gizmos.line(points[0], points[1], Color::srgb(0.1, 0.9, 1.0));
        }
        if let Some(field) = &fields.burst {
            for ray in field.rays.iter().step_by(4) {
                if let Ok(axis) = Dir3::new(ray.end - field.origin) {
                    gizmos.line(
                        field.origin,
                        field.origin + axis * 4.0,
                        Color::srgb(1.0, 0.45, 0.05),
                    );
                }
            }
        }
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            gizmos.line(at - axis * 0.3, at + axis * 0.3, Color::WHITE);
        }
    }
    if let Some(pose) = cue.camera {
        for mut camera in &mut cameras {
            *camera = pose;
        }
    }
}
