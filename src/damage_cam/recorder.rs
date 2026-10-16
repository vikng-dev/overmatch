//! The game's pen cam recorder: on the authority (the server and offline play, never a net
//! client), one [`PenetrationReport`] per main-gun round ([`FireMechanism::Single`]) and tank its
//! path struck, emitted as a [`RecordedShot`]. Automatic fire records nothing.
//!
//! A recorded shell is born with its [`ShotRecording`]. After each march step the shell notes every
//! tank its new marks touch ([`MarchRecorder::note`]): a tank first touched has its components' HP
//! taken there, and each tick that logs marks on it opens a hull frame. [`pose_struck`] fills those
//! frames in the same tick from the pose the march's colliders were placed at, before the damage
//! consequences can detach a turret. A shell that despawns hands its logs to the [`ShotRecorder`]
//! ([`keep_spent`]), so a record never depends on how long the shell lives.
//!
//! The record's HP figures are the shot's own ([`crate::ballistics::ShotDose`], see
//! [`super::record`]). A tank's death is the shot's when its own deposition emptied the box that
//! decided it, in the same tick ([`attribute_kills`]): for a cookoff the first ammunition box
//! emptied that tick, for crew loss the last crew station, in deposition order across every shell
//! (automatic fire included, which can kill though it is not recorded).
//!
//! A struck tank's record is finished ([`finish_records`]) once the shell can add nothing to it:
//! the shell ended, or it is clear of the tank's colliders. A tank gone by then (despawned, or
//! respawned as a new entity) gets no record.

use std::collections::BTreeMap;

use avian3d::prelude::{
    ColliderAabb, ColliderTransform, Position, RigidBody, RigidBodyColliders, Rotation,
};
use bevy::ecs::system::{SystemParam, SystemState};
use bevy::prelude::*;

use super::PenetrationReport;
use super::record::{self, HullFrame, ShotFacts, ShotLog, StruckTank};
use crate::ballistics::{
    ArmourMark, ComponentHealth, FireShell, PenetrationMarks, Projectile as Flying, ShellPath,
    ShotDose, ShotSource, SpallCone, SpallMarks,
};
use crate::damage::{
    Ammo, CrewStation, DamageConsequences, KnockoutReason, TankKnockedOut, TankVolumes, VolumeOf,
};
use crate::spec::FireMechanism;
use crate::state::{GameplaySet, SimPhase};
use crate::tank::{Rig, Weapon, rig_world_pose};
use crate::{ClientReplica, ShotId};

/// How far (m) a shell must be outside the union of a struck tank's collider bounds before its
/// record is finished: DERIVED to cover the tank's travel in a tick (~0.17 m at 11 m/s and 64 Hz)
/// and the round's own radius, so a shell that ended a tick between two of the tank's plates is
/// still inside.
const CLEAR_MARGIN: f32 = 0.5;

/// A finished record: the tank the round struck, who fired it, and the report.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "its consumer is the per-recipient projection, not yet built"
    )
)]
#[derive(Message)]
pub(crate) struct RecordedShot {
    pub tank: Entity,
    pub shooter: Option<ShotSource>,
    pub shot: Option<ShotId>,
    pub report: PenetrationReport,
}

/// The authority's recording state.
#[derive(Resource, Default)]
pub(crate) struct ShotRecorder {
    /// This tick's emptied pools, `(shell, volume)`, in deposition order.
    emptied: Vec<(Entity, Entity)>,
    /// Recordings whose shell despawned, with its logs.
    spent: Vec<Spent>,
    /// Struck tanks not yet finished, live or spent.
    open: usize,
    /// A struck tank has a frame this tick's [`pose_struck`] has not filled.
    unposed: bool,
}

#[cfg(test)]
impl ShotRecorder {
    /// Nothing logged, held or open.
    pub(crate) fn is_idle(&self) -> bool {
        self.emptied.is_empty() && self.spent.is_empty() && self.open == 0 && !self.unposed
    }
}

/// A recording whose shell despawned, and the shell's logs.
struct Spent {
    shell: Entity,
    recording: ShotRecording,
    marks: Vec<ArmourMark>,
    cones: Vec<SpallCone>,
    path: Vec<Vec3>,
    dose: ShotDose,
}

/// A recorded round: the facts the march does not carry and every tank its path has struck.
///
/// Invariant: born with the shell, on the authority only. See ADR-0014.
#[derive(Component, Default)]
pub(crate) struct ShotRecording {
    facts: ShotFacts,
    source: Option<ShotSource>,
    shot: Option<ShotId>,
    struck: Vec<Struck>,
}

/// One tank a recorded round's path struck.
struct Struck {
    tank: Entity,
    /// Its components and their HP when the round first touched it.
    boxes: Vec<(Entity, u32)>,
    /// `(first mark, hull frame)` per tick that logged marks on it; `None` until posed.
    frames: Vec<(usize, Option<HullFrame>)>,
    servo_deg: Option<BTreeMap<String, f32>>,
    killed: Option<KnockoutReason>,
    done: bool,
}

impl ShotRecording {
    /// The recording for `fire`, fired from `weapon` when a tank's weapon fired it.
    pub(crate) fn of(fire: &FireShell, weapon: Option<&Weapon>) -> Self {
        Self {
            facts: ShotFacts {
                shooter: None,
                launch: fire.origin,
                muzzle: fire.shooter.map(|_| fire.origin),
                projectile: weapon.map_or_else(String::new, |weapon| weapon.name.clone()),
                caliber_mm: fire.caliber * 1000.0,
                mass_kg: fire.mass,
                round: weapon.map_or_else(String::new, |weapon| weapon.round.clone()),
                vehicle: None,
            },
            source: fire.shooter,
            shot: fire.shot,
            struck: Vec::new(),
        }
    }

    /// Whether `fire` is a round the recorder records: a main-gun round.
    pub(crate) fn records(fire: &FireShell) -> bool {
        fire.mechanism == FireMechanism::Single
    }
}

/// The march's access to the recorder.
#[derive(SystemParam)]
pub(crate) struct MarchRecorder<'w, 's> {
    recorder: Option<ResMut<'w, ShotRecorder>>,
    tanks: Query<'w, 's, &'static TankVolumes>,
}

impl MarchRecorder<'_, '_> {
    /// After `shell`'s march step: log the pools its tally `dose` emptied from `emptied_from` on,
    /// and note each tank its marks from `marks_from` on touch in its `recording`.
    pub(crate) fn note(
        &mut self,
        shell: Entity,
        recording: Option<Mut<ShotRecording>>,
        marks: &[ArmourMark],
        marks_from: usize,
        dose: &ShotDose,
        emptied_from: usize,
        owners: &Query<&VolumeOf>,
        health: &Query<&mut ComponentHealth>,
    ) {
        let Some(recorder) = self.recorder.as_mut() else {
            return;
        };
        recorder.emptied.extend(
            dose.emptied()[emptied_from..]
                .iter()
                .map(|&volume| (shell, volume)),
        );
        let Some(mut recording) = recording else {
            return;
        };
        let owner = |volume: Entity| owners.get(volume).ok().map(VolumeOf::tank);
        for (index, mark) in marks.iter().enumerate().skip(marks_from) {
            let mut touched: Vec<Entity> = match mark {
                ArmourMark::Bounce { struck, .. } => owner(*struck).into_iter().collect(),
                ArmourMark::Crossing { volumes, .. } => volumes
                    .iter()
                    .filter_map(|crossed| owner(crossed.volume))
                    .collect(),
                ArmourMark::Failed => Vec::new(),
            };
            touched.sort();
            touched.dedup();
            for tank in touched {
                let at = match recording
                    .struck
                    .iter()
                    .position(|struck| struck.tank == tank)
                {
                    Some(at) => at,
                    None => {
                        let boxes = self
                            .tanks
                            .get(tank)
                            .into_iter()
                            .flat_map(|volumes| volumes.iter())
                            .filter_map(|volume| Some((volume, health.get(volume).ok()?.current)))
                            .collect();
                        recording.struck.push(Struck {
                            tank,
                            boxes,
                            frames: Vec::new(),
                            servo_deg: None,
                            killed: None,
                            done: false,
                        });
                        recorder.open += 1;
                        recording.struck.len() - 1
                    }
                };
                let struck = &mut recording.struck[at];
                if !matches!(struck.frames.last(), Some((_, None))) {
                    struck.frames.push((index, None));
                    recorder.unposed = true;
                }
            }
        }
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<ShotRecorder>()
        .add_message::<RecordedShot>()
        .add_observer(keep_spent)
        .add_systems(
            FixedUpdate,
            (
                pose_struck
                    .after(SimPhase::ProjectileMarch)
                    .before(DamageConsequences)
                    .run_if(|recorder: Res<ShotRecorder>| recorder.unposed),
                (
                    attribute_kills,
                    finish_records.run_if(|recorder: Res<ShotRecorder>| recorder.open > 0),
                )
                    .chain()
                    .after(DamageConsequences),
            )
                .in_set(GameplaySet)
                .run_if(not(resource_exists::<ClientReplica>)),
        );
}

/// Hand a despawning recorded shell's recording and logs to the recorder while it has struck tanks
/// to finish.
fn keep_spent(
    remove: On<Remove, ShotRecording>,
    mut shells: Query<(
        &mut ShotRecording,
        &mut PenetrationMarks,
        &mut SpallMarks,
        &mut ShellPath,
        &mut ShotDose,
    )>,
    recorder: Option<ResMut<ShotRecorder>>,
) {
    let (Some(mut recorder), Ok((mut recording, mut marks, mut spall, mut path, mut dose))) =
        (recorder, shells.get_mut(remove.entity))
    else {
        return;
    };
    if recording.struck.iter().all(|struck| struck.done) {
        return;
    }
    recorder.spent.push(Spent {
        shell: remove.entity,
        recording: std::mem::take(&mut *recording),
        marks: std::mem::take(&mut marks.log),
        cones: std::mem::take(&mut spall.cones),
        path: std::mem::take(&mut path.points),
        dose: std::mem::take(&mut *dose),
    });
}

/// Fill every unposed frame with the struck hull's frame as the march resolved this tick's marks,
/// and each struck tank's servo angles the first time.
fn pose_struck(world: &mut World) {
    world.resource_mut::<ShotRecorder>().unposed = false;
    let mut live = world.query::<&ShotRecording>();
    let mut tanks: Vec<Entity> = live
        .iter(world)
        .chain(
            world
                .resource::<ShotRecorder>()
                .spent
                .iter()
                .map(|spent| &spent.recording),
        )
        .flat_map(|recording| &recording.struck)
        .filter(|struck| matches!(struck.frames.last(), Some((_, None))))
        .map(|struck| struck.tank)
        .collect();
    tanks.sort();
    tanks.dedup();
    let posed: Vec<(Entity, Option<HullFrame>, BTreeMap<String, f32>)> = tanks
        .into_iter()
        .map(|tank| {
            (
                tank,
                hull_frame(world, tank),
                record::servo_angles(world, tank),
            )
        })
        .collect();
    let pose = |struck: &mut Struck| {
        let Some((_, frame, servo_deg)) = posed.iter().find(|(tank, ..)| *tank == struck.tank)
        else {
            return;
        };
        if let Some((_, slot @ None)) = struck.frames.last_mut() {
            *slot = *frame;
        }
        struck.servo_deg.get_or_insert_with(|| servo_deg.clone());
    };
    let mut live = world.query::<&mut ShotRecording>();
    for mut recording in live.iter_mut(world) {
        recording.struck.iter_mut().for_each(pose);
    }
    for spent in &mut world.resource_mut::<ShotRecorder>().spent {
        spent.recording.struck.iter_mut().for_each(pose);
    }
}

/// The hull frame of `tank` the march's colliders were placed in this tick. The march casts
/// against each child collider's pose, which the physics step writes from the body's pose at the
/// step's start composed with the collider's [`ColliderTransform`]; that body pose is read back off
/// any child collider, the body's own pose where it has none (a body whose volumes are bodies of
/// their own).
fn hull_frame(world: &mut World, tank: Entity) -> Option<HullFrame> {
    let hull = world.get::<Rig>(tank)?.hull;
    let child = world.get::<RigidBodyColliders>(tank).and_then(|colliders| {
        colliders.iter().find_map(|collider| {
            let entity = world.get_entity(collider).ok()?;
            if entity.contains::<RigidBody>() {
                return None;
            }
            Some((
                entity.get::<Position>()?.0,
                entity.get::<Rotation>()?.0,
                *entity.get::<ColliderTransform>()?,
            ))
        })
    });
    let (position, rotation) = match child {
        Some((position, rotation, local)) => {
            let body = rotation * local.rotation.0.inverse();
            (position - body * local.translation, body)
        }
        None => (
            world.get::<Position>(tank)?.0,
            world.get::<Rotation>(tank)?.0,
        ),
    };
    let mut state = SystemState::<(Query<&ChildOf>, Query<&Transform>)>::new(world);
    let (parents, locals) = state.get(world).ok()?;
    let (at, turn) = rig_world_pose(hull, tank, position, rotation, &parents, &locals)?;
    Some(HullFrame::posed(at, turn))
}

/// Attribute each tank knocked out this tick to the shell whose deposition emptied the box that
/// decided it, and clear the tick's emptied log.
fn attribute_kills(
    knocked: Query<(Entity, &TankKnockedOut), Added<TankKnockedOut>>,
    facets: Query<(&VolumeOf, Has<Ammo>, Has<CrewStation>)>,
    mut recordings: Query<&mut ShotRecording>,
    mut recorder: ResMut<ShotRecorder>,
) {
    let recorder = &mut *recorder;
    for (tank, knocked_out) in &knocked {
        let reason = knocked_out.reason;
        let decides = |&&(_, volume): &&(Entity, Entity)| {
            facets.get(volume).is_ok_and(|(owner, ammo, crew)| {
                owner.tank() == tank
                    && match reason {
                        KnockoutReason::Cookoff => ammo,
                        KnockoutReason::CrewLoss => crew,
                    }
            })
        };
        let killer = match reason {
            KnockoutReason::Cookoff => recorder.emptied.iter().find(decides),
            KnockoutReason::CrewLoss => recorder.emptied.iter().rev().find(decides),
        };
        let Some(&(shell, _)) = killer else {
            continue;
        };
        let struck = match recordings.get_mut(shell) {
            Ok(recording) => recording.into_inner().struck.iter_mut(),
            Err(_) => match recorder.spent.iter_mut().find(|spent| spent.shell == shell) {
                Some(spent) => spent.recording.struck.iter_mut(),
                None => continue,
            },
        };
        for struck in struck.filter(|struck| struck.tank == tank) {
            struck.killed = Some(reason);
        }
    }
    recorder.emptied.clear();
}

/// Finish every struck tank the shell can add nothing more to, and emit its record.
fn finish_records(world: &mut World) {
    let spent = std::mem::take(&mut world.resource_mut::<ShotRecorder>().spent);
    let mut finished: Vec<(Entity, usize)> = Vec::new();
    let mut records = Vec::new();
    let mut live = world.query::<(Entity, &ShotRecording, Has<Flying>, &Transform)>();
    for (shell, recording, flying, at) in live.iter(world) {
        for (index, struck) in recording.struck.iter().enumerate() {
            if struck.done || (flying && !clear_of(world, struck.tank, at.translation)) {
                continue;
            }
            finished.push((shell, index));
            if let Some(log) = ShotLog::read(world, shell) {
                records.extend(recorded(world, recording, struck, &log));
            }
        }
    }
    let mut closed = finished.len();
    for spent in &spent {
        let log = ShotLog {
            marks: &spent.marks,
            cones: &spent.cones,
            path: &spent.path,
            dose: Some(&spent.dose),
        };
        for struck in spent.recording.struck.iter().filter(|struck| !struck.done) {
            closed += 1;
            records.extend(recorded(world, &spent.recording, struck, &log));
        }
    }
    for (shell, index) in finished {
        if let Some(mut recording) = world.get_mut::<ShotRecording>(shell) {
            recording.struck[index].done = true;
        }
    }
    {
        let mut recorder = world.resource_mut::<ShotRecorder>();
        recorder.open = recorder.open.saturating_sub(closed);
    }
    world.write_message_batch(records);
}

/// `struck`'s record of `recording`'s round, its logs `log`; `None` where the tank is gone, a
/// frame was never posed, or the round opens no pen cam.
fn recorded(
    world: &World,
    recording: &ShotRecording,
    struck: &Struck,
    log: &ShotLog,
) -> Option<RecordedShot> {
    world.get_entity(struck.tank).ok()?;
    let frames = struck
        .frames
        .iter()
        .map(|&(mark, frame)| Some((mark, frame?)))
        .collect::<Option<Vec<_>>>()?;
    let tank = StruckTank::recorded(
        struck.tank,
        frames,
        struck.servo_deg.clone().unwrap_or_default(),
        struck.boxes.clone(),
    );
    let mut report = record::report_from(world, &tank, log, &recording.facts)?;
    report.killed = struck.killed;
    Some(RecordedShot {
        tank: struck.tank,
        shooter: recording.source,
        shot: recording.shot,
        report,
    })
}

/// Whether `point` (WORLD) is clear of `tank`'s colliders by [`CLEAR_MARGIN`]; a tank with none
/// is clear of everything.
fn clear_of(world: &World, tank: Entity, point: Vec3) -> bool {
    let colliders = world
        .get::<RigidBodyColliders>(tank)
        .into_iter()
        .flat_map(|colliders| colliders.iter());
    let volumes = world
        .get::<TankVolumes>(tank)
        .into_iter()
        .flat_map(|volumes| volumes.iter());
    let mut bounds: Option<(Vec3, Vec3)> = None;
    for entity in colliders.chain(volumes) {
        if let Some(aabb) = world.get::<ColliderAabb>(entity) {
            bounds = Some(match bounds {
                Some((min, max)) => (min.min(aabb.min), max.max(aabb.max)),
                None => (aabb.min, aabb.max),
            });
        }
    }
    bounds.is_none_or(|(min, max)| {
        point.cmplt(min - CLEAR_MARGIN).any() || point.cmpgt(max + CLEAR_MARGIN).any()
    })
}
