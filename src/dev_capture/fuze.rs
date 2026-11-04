//! The capture's shot lever, in the game's damage model: on the first frame at or after app second
//! `at`, a shipped round in one of its fuze modes is fired at a real construction or at the ground beside
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
    aim: ShotAim,
    at: f32,
    fired: bool,
    camera: Option<Transform>,
    logged: bool,
}

pub(crate) fn offline_shot_plugin(app: &mut App) {
    let Some(shot) = super::plan().and_then(|plan| plan.shot.as_ref()) else {
        return;
    };
    let shot = shot.clone();
    info!("capture: game's damage model shot {shot:?}");
    app.insert_resource(ShotReady(None))
        .insert_resource(RetainSpentShells(true))
        .insert_resource(crate::camera::CameraCaptive(true))
        .add_systems(
            PostStartup,
            move |constructions: Query<
                (&crate::scatter::MapInstance, &Transform),
                With<crate::construction::Body>,
            >,
                  grid: Option<Res<crate::terrain_grid::HeightGrid>>,
                  mut commands: Commands| {
                let aim = bind_target(shot.target, grid.as_deref(), constructions.iter())
                    .unwrap_or_else(|error| super::plan::refuse(error));
                commands.insert_resource(Cue {
                    key: shot.key.clone(),
                    round: shot.round,
                    aim,
                    at: shot.at,
                    fired: false,
                    camera: None,
                    logged: false,
                });
            },
        )
        .add_systems(Update, fire_on_cue)
        .add_systems(
            PostUpdate,
            show_shot.in_set(crate::camera::CaptiveCameraPlaced),
        );
}

/// The game's damage-model capture retains its startup-admitted firing pose through the cue.
#[derive(Clone, Copy)]
struct ShotAim {
    target: Vec3,
    origin: Vec3,
    direction: Dir3,
}

fn bind_target<'a>(
    target_kind: super::plan::ShotTarget,
    grid: Option<&crate::terrain_grid::HeightGrid>,
    constructions: impl Iterator<Item = (&'a crate::scatter::MapInstance, &'a Transform)>,
) -> Result<ShotAim, super::plan::PlanError> {
    let construction = target_kind == super::plan::ShotTarget::Construction;
    let grid = grid.ok_or_else(|| {
        super::plan::PlanError::Field(
            "shot: the scene supplies no HeightGrid; choose a terrain scene".into(),
        )
    })?;
    // Stable placement identity breaks distance ties independently of ECS iteration order.
    let (_, transform) = constructions
        .min_by(|a, b| {
            let reach = |transform: &Transform| {
                Vec2::new(transform.translation.x, transform.translation.z).length_squared()
            };
            reach(a.1)
                .total_cmp(&reach(b.1))
                .then(a.0.ordinal.cmp(&b.0.ordinal))
        })
        .ok_or_else(|| {
            super::plan::PlanError::Field(
                "shot: the scene has no construction to aim at; choose a map with a construction"
                    .into(),
            )
        })?;
    // Fixture geometry: the construction 2.5 m up from 10 m off, or the ground 10 m beside it from
    // 10 m above and 2 m back.
    let target = if construction {
        transform.translation + Vec3::Y * 2.5
    } else {
        let p = transform.translation + Vec3::X * 10.0;
        Vec3::new(p.x, grid.height_at(p.x, p.z), p.z)
    };
    let origin = if construction {
        target + Vec3::Z * 10.0
    } else {
        target + Vec3::new(0.0, 10.0, 2.0)
    };
    let direction = Dir3::new(target - origin)
        .expect("capture staging gives a nonzero finite firing direction");
    Ok(ShotAim {
        target,
        origin,
        direction,
    })
}

fn fire_on_cue(
    time: Res<Time>,
    mut cue: ResMut<Cue>,
    mut exit: MessageWriter<bevy::app::AppExit>,
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
    let ShotAim {
        target,
        origin,
        direction,
    } = cue.aim;
    commands.trigger(FireShell::of_round(
        &cue.round,
        origin,
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
    let Some((tank, mut loads)) = crate::tank::controlled_one(tanks.single_mut()) else {
        return; // A cue can run before possession; there is no local HUD selection to update.
    };
    for (weapon, slot, root) in weapons {
        if root.0 != tank {
            continue;
        }
        let Some(index) = weapon.rounds.iter().position(|round| round.key == cue.key) else {
            continue; // This weapon does not chamber the independently cued round.
        };
        let index = u8::try_from(index).expect("WeaponSpec admission bounds round indices to u8");
        let load = loads
            .weapons
            .get_mut(slot.0)
            .expect("complete tank spawn builds one load entry per weapon slot");
        load.selected = index;
        for _ in weapon.rounds[usize::from(index)].fuze_modes() {
            if load.fuze_mode(&weapon.rounds, index) == Some(cue.round.fuze_mode) {
                break;
            }
            load.toggle_fuze(&weapon.rounds, index);
        }
        assert_eq!(
            load.fuze_mode(&weapon.rounds, index),
            Some(cue.round.fuze_mode),
            "the game's damage-model cue and its matching HUD round resolve the same registry modes"
        );
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
        if let Some(field) = &fields.burst {
            let spread = BurstFields::shipped().shape().spread(field.field)
                .expect("the game's damage-model capture reads a field created from the shipped field registry");
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

#[cfg(test)]
mod tests {
    use super::super::plan::{PlanError, ShotTarget};
    use super::*;
    use crate::scatter::MapInstance;
    use crate::terrain_grid::{FIXTURE_EXTENT, HeightGrid};

    /// The game's damage-model shot cue must refuse an incompatible scene at startup, even
    /// when firing is scheduled far later; both target kinds admit a complete scene.
    #[test]
    fn shot_target_admission_requires_scene_geometry() {
        let identity = MapInstance {
            id: "fixture".into(),
            ordinal: 0,
        };
        let pose = Transform::from_xyz(3.0, 0.0, 4.0);
        let grid = HeightGrid::new(vec![0.0; 4].into(), 2, FIXTURE_EXTENT);
        for target in [ShotTarget::Construction, ShotTarget::Ground] {
            let missing_grid = bind_target(target, None, std::iter::once((&identity, &pose)));
            assert!(
                matches!(&missing_grid, Err(PlanError::Field(error)) if error.contains("HeightGrid") && error.contains("choose a terrain scene"))
            );
            let missing_construction = bind_target(target, Some(&grid), std::iter::empty());
            assert!(
                matches!(&missing_construction, Err(PlanError::Field(error)) if error.contains("no construction") && error.contains("choose a map"))
            );
            assert!(bind_target(target, Some(&grid), std::iter::once((&identity, &pose))).is_ok());
        }
    }
}
