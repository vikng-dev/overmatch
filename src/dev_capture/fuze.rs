//! The capture's shot lever, in the game's damage model: on the first frame at or after app second
//! `at`, a shipped round in one of its fuze modes is fired at a real tree or at the ground beside
//! it, and the capture's frames are timed from the round's burst. The burst outside a tank opens no
//! pen cam, so the view draws it over the scene: the flight path cyan, a quarter of the fragment
//! cone's directions orange (4 m each), the burst point a white cross. The controlled tank's weapon
//! that lists the round selects it in that mode, so the HUD's readout names both.
//!
//! The capture plan's `shot` ([`super::plan::Shot`]); the offline root alone mounts it. A shot that
//! has not burst [`GIVE_UP_S`] after its cue ends the run with a failure code.
use crate::ballistics::spall_cone::BurstFields;
use crate::ballistics::{
    ArmourMark, FireShell, PenetrationMarks, RetainSpentShells, ShellPath, SpallMarks,
};
use crate::tank::{Controlled, Muzzle, TankRoot, Weapon, WeaponIndex, WeaponLoads};
use bevy::prelude::*;

/// App seconds past the cue after which a shot with no burst fails the run (ASSUMED: a 0.1 s delay
/// and the scene's load leave orders of magnitude of slack).
const GIVE_UP_S: f32 = 30.0;

/// The app second the shot burst at, once it has: the capture's frames count from it.
#[derive(Resource)]
pub(super) struct ShotReady(pub(super) Option<f32>);

#[derive(Resource)]
struct Cue {
    key: String,
    round: crate::rounds::Round,
    tree: bool,
    at: f32,
    fired: bool,
    camera: Option<Transform>,
    logged: bool,
}

pub(crate) fn offline_shot_plugin(app: &mut App) {
    let Some(shot) = super::plan().and_then(|plan| plan.shot.as_ref()) else {
        return;
    };
    let round = super::resolve_round(&shot.round, shot.mode, "shot");
    let cue = Cue {
        key: shot.round.clone(),
        round,
        tree: shot.target == super::plan::ShotTarget::Tree,
        at: shot.at,
        fired: false,
        camera: None,
        logged: false,
    };
    info!("capture: game's damage model shot {shot:?}");
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

#[expect(
    clippy::too_many_arguments,
    reason = "the cue, its clock and exit, the tree and ground it aims at, the HUD it sets"
)]
fn fire_on_cue(
    time: Res<Time>,
    mut cue: ResMut<Cue>,
    mut exit: MessageWriter<bevy::app::AppExit>,
    trees: Query<(&crate::scatter::MapInstance, &Transform), With<crate::trees::TreeBody>>,
    grid: Option<Res<crate::terrain_grid::HeightGrid>>,
    mut tanks: Query<(Entity, &mut WeaponLoads), With<Controlled>>,
    weapons: Query<(&Weapon, &WeaponIndex, &TankRoot), With<Muzzle>>,
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
    // The standing tree nearest the map's centre, the lowest ordinal on a tie: a real tree the
    // same on every run, with the map on every side of it.
    let Some((_, transform)) = trees.iter().min_by(|a, b| {
        let reach = |transform: &Transform| {
            Vec2::new(transform.translation.x, transform.translation.z).length_squared()
        };
        reach(a.1)
            .total_cmp(&reach(b.1))
            .then(a.0.ordinal.cmp(&b.0.ordinal))
    }) else {
        return;
    };
    // Fixture geometry: the trunk 2.5 m up from 10 m off, or the ground 10 m beside the tree from
    // 10 m above and 2 m back.
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
    cue.camera = Some(viewpoint(target));
    select_on_hud(&cue, &mut tanks, &weapons);
    info!(
        "capture: game's damage model shot origin={origin:?} target={target:?} mode={}",
        cue.round.fuze_mode
    );
}

/// Where the camera stands to see a shot fired along -Z at `at`: 14 m off to its side, 4 m back
/// toward the gun and 6 m up (ASSUMED framing), so the flight path crosses the frame.
fn viewpoint(at: Vec3) -> Transform {
    Transform::from_translation(at + Vec3::new(14.0, 6.0, 4.0)).looking_at(at, Vec3::Y)
}

/// The controlled tank's weapon that lists the cue's round selects it, in the cue's fuze mode, as
/// the crew's round step and fuze toggle would: the next-load rule leaves the chambered round as
/// it is, so the readout names the cue's round as the next one.
fn select_on_hud(
    cue: &Cue,
    tanks: &mut Query<(Entity, &mut WeaponLoads), With<Controlled>>,
    weapons: &Query<(&Weapon, &WeaponIndex, &TankRoot), With<Muzzle>>,
) {
    for (tank, mut loads) in tanks {
        for (weapon, slot, root) in weapons {
            let index = weapon
                .rounds
                .iter()
                .position(|round| round.key == cue.key)
                .and_then(|index| u8::try_from(index).ok());
            let (true, Some(index), Some(load)) =
                (root.0 == tank, index, loads.weapons.get_mut(slot.0))
            else {
                continue;
            };
            load.selected = index;
            for _ in weapon.rounds[usize::from(index)].fuze_modes() {
                if load.fuze_mode(&weapon.rounds, index) == Some(cue.round.fuze_mode) {
                    break;
                }
                load.toggle_fuze(&weapon.rounds, index);
            }
        }
    }
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
            cue.camera = Some(viewpoint(at));
        }
        for points in path.points.windows(2) {
            gizmos.line(points[0], points[1], Color::srgb(0.1, 0.9, 1.0));
        }
        // Every fourth direction the field was thrown along, rebuilt from its descriptor as a
        // decoder does: a ray's recorded end is its origin when it met nothing.
        if let Some(field) = &fields.burst
            && let Ok(spread) = BurstFields::shipped().shape().spread(field.field)
        {
            for (direction, _) in spread.directions(field.axis).step_by(4) {
                gizmos.line(
                    field.origin,
                    field.origin + direction * 4.0,
                    Color::srgb(1.0, 0.45, 0.05),
                );
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
