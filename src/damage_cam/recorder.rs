//! The game's damage model: pen cam recordings on the authority (the server and offline play,
//! never a net client), one [`PenetrationReport`] per main-gun round ([`FireMechanism::Single`]) and encounter
//! of its path with a tank alive at the strike, emitted as a [`RecordedShot`]. Automatic fire
//! records nothing. A husk opens no new encounter; the knockout shot keeps its strike pose.
//!
//! An encounter runs from the round's first mark on a tank until its record is finished; a round
//! that strikes the tank again after that, say off another tank's armour, opens the next encounter,
//! whose record reads only the marks and settles from its own first mark on. A record is built the
//! tick its encounter finishes, before any later mark exists, so it ends where the next begins: a
//! shell's encounters with one tank partition its marks and its loss there, no dose in two records.
//! A dose whose mark touches no volume of the tank (another tank's spall reaching it) opens no
//! encounter, and lands in a record only while one is open.
//!
//! A recorded shell is born with its [`ShotRecording`]. After each march step the shell notes every
//! tank its new marks touch ([`MarchRecorder::note`]): an encounter opened has the tank's
//! components' pools read there. Runtime marks carry their immutable contact-time hull frames;
//! [`pose_struck`] fills frames for standalone static fixtures and reads servo angles before
//! damage consequences can detach a turret. A shell that despawns hands its
//! logs to the [`ShotRecorder`] ([`keep_spent`]), so a record never depends on how long the shell
//! lives.
//!
//! The record's pool figures are the shot's own ([`crate::ballistics::ShotDose`], see
//! [`super::record`]). A tank's knockout is the shot's when its own deposition emptied the box that
//! decided it, in the same tick ([`attribute_kills`]): for a cookoff the first ammunition box
//! emptied that tick, for crew loss the pool that takes the in-action count below two, in deposition order across every shell
//! (automatic fire included, which can empty pools though it is not recorded). The same decision, for
//! every knocked-out tank, is the authority's [`TankKnockout`] fact, which the match score credits.
//!
//! An encounter's record is finished ([`finish_records`]) once the shell ended, or is clear of the
//! tank's colliders with no fuze that tank armed still running: a burst wherever the delay ends is
//! the encounter's. A tank gone by then (despawned, or respawned as a new entity) gets no record.
//! Every new encounter is posed before damage consequences can detach a turret; an encounter
//! already open keeps that pose after the knockout.

use std::collections::BTreeMap;

use avian3d::prelude::{
    ColliderAabb, ColliderTransform, Position, RigidBody, RigidBodyColliders, Rotation,
};
use bevy::ecs::system::{SystemParam, SystemState};
use bevy::prelude::*;

use super::PenetrationReport;
use super::record::{self, HullFrame, ShotFacts, ShotLog, StruckTank};
use super::wire::TankLife;
use crate::ballistics::{
    ArmourMark, BurstField, ComponentHealth, FireShell, PenetrationMarks, Projectile as Flying,
    ShellPath, ShotDose, SpallCone, SpallMarks,
};
use crate::damage::{
    Ammo, CrewStation, DamageConsequences, Dead, KnockoutCause, KnockoutReason, TankKnockedOut,
    TankKnockout, TankVolumes, VolumeOf,
};
use crate::spec::{FireMechanism, TankSpecHandle};
use crate::state::{GameplaySet, SimPhase};
use crate::tank::{Rig, Weapon, rig_world_pose};
use crate::{ClientReplica, CombatantId, CombatantLife, ShotClock, ShotId};

/// How far (m) a shell must be outside the union of a struck tank's collider bounds before its
/// record is finished: DERIVED to cover the tank's travel in a tick (~0.17 m at 11 m/s and 64 Hz)
/// and the round's own radius, so a shell that ended a tick between two of the tank's plates is
/// still inside.
const CLEAR_MARGIN: f32 = 0.5;

/// A finished record: the tank the round struck and the life of its combatant it is (`None` for a
/// tank no match spawned), its shot identity when keyed, which of the round's encounters with the tank it is (0 the
/// first), the [`ShotClock`] ticks the encounter opened and finished on (0 where no clock runs), the
/// authority outcome a replica draws as the encounter's impact ([`sanctioned_outcome`]), the struck
/// hull's pose (WORLD) when the encounter opened, and the report.
#[derive(Message)]
pub(crate) struct RecordedShot {
    pub life: Option<TankLife>,
    pub shot: Option<ShotId>,
    pub encounter: u32,
    pub contact_tick: u32,
    pub completion_tick: u32,
    pub outcome: Option<u32>,
    pub hull: Option<(Vec3, Quat)>,
    pub report: PenetrationReport,
}

/// The authority's recording state.
#[derive(Resource, Default)]
pub(crate) struct ShotRecorder {
    /// This tick's emptied pools, in deposition order.
    emptied: Vec<Emptied>,
    /// Recordings whose shell despawned with an encounter unfinished, with its logs.
    spent: Vec<Spent>,
    /// Encounters not yet finished, live or spent.
    open: usize,
    /// An encounter has a frame this tick's [`pose_struck`] has not filled.
    unposed: bool,
    /// The game's damage model: first contacts with husks since the authority started, at most
    /// one per round and tank, including automatic fire. Later marks cannot reclassify a strike
    /// that began while the tank was alive.
    husk_strikes: u64,
}

impl ShotRecorder {
    /// The husk strikes counted since the authority started; the server's stats line reports its
    /// rise per period.
    pub(crate) fn husk_strikes(&self) -> u64 {
        self.husk_strikes
    }
}

#[cfg(test)]
impl ShotRecorder {
    /// Nothing logged, held or open.
    pub(crate) fn is_idle(&self) -> bool {
        self.emptied.is_empty() && self.spent.is_empty() && self.open == 0 && !self.unposed
    }
}

/// One pool a shell's deposition emptied: the shell, its network identity (`None` for an unkeyed
/// round), and the box.
struct Emptied {
    shell: Entity,
    shot: Option<ShotId>,
    volume: Entity,
}

/// A recording whose shell despawned, and the shell's logs.
struct Spent {
    shell: Entity,
    recording: ShotRecording,
    marks: Vec<ArmourMark>,
    cones: Vec<SpallCone>,
    burst: Option<BurstField>,
    path: Vec<Vec3>,
    dose: ShotDose,
}

/// A recorded round: the facts the march does not carry and every encounter of its path with a tank.
///
/// Invariant: born with the shell, on the authority only. See ADR-0014.
#[derive(Component, Default)]
pub(crate) struct ShotRecording {
    facts: ShotFacts,
    shot: Option<ShotId>,
    struck: Vec<Struck>,
}

/// One encounter of a recorded round's path with a tank.
struct Struck {
    tank: Entity,
    /// Which of the round's encounters with the tank, 0 the first.
    encounter: u32,
    /// The [`ShotClock`] tick it opened on.
    contact_tick: u32,
    /// The encounter's first mark.
    from: usize,
    /// Its components and the pool units each held when the encounter opened.
    boxes: Vec<(Entity, u32)>,
    /// `(first mark, hull frame)` per sampled contact; standalone static fixtures are unposed
    /// until the post-march recorder reads their frame.
    frames: Vec<(usize, Option<HullFrame>)>,
    /// `None` until [`pose_struck`] reads the live tank's servos.
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
                launch: fire.birth,
                muzzle: fire.shooter.map(|_| fire.origin),
                projectile: weapon.map_or_else(String::new, |weapon| weapon.name.clone()),
                caliber_mm: fire.caliber * 1000.0,
                mass_kg: fire.mass,
                round: weapon
                    .zip(fire.shooter)
                    .and_then(|(weapon, source)| weapon.rounds.get(usize::from(source.round)))
                    .map_or_else(String::new, |round| round.key.clone()),
                vehicle: None,
            },
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
    clock: Option<Res<'w, ShotClock>>,
    tanks: Query<'w, 's, &'static TankVolumes>,
    knocked_out: Query<'w, 's, (), With<TankKnockedOut>>,
}

impl MarchRecorder<'_, '_> {
    /// After `shell`'s march step: log the pools its tally `dose` emptied from `emptied_from` on,
    /// and note each tank its marks touch from the `(marks, reached)` cursors on in its
    /// `recording`, in the tank's open encounter or a new one. A crossing touches a tank when it
    /// reaches it ([`PenetrationMarks::reached`]); every other mark when it is logged.
    #[expect(
        clippy::too_many_arguments,
        reason = "one march step's evidence: the shell and its shot identity, its recording, the mark and dose cursors, and the ownership and health reads"
    )]
    pub(crate) fn note(
        &mut self,
        shell: Entity,
        contacted: &mut Vec<Entity>,
        shot: Option<ShotId>,
        recording: Option<Mut<ShotRecording>>,
        marks: &PenetrationMarks,
        (marks_from, reached_from): (usize, usize),
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
                .map(|&volume| Emptied {
                    shell,
                    shot,
                    volume,
                }),
        );
        let owner = |volume: Entity| owners.get(volume).ok().map(VolumeOf::tank);
        let touched = |mark: &ArmourMark| -> Vec<Entity> {
            let mut touched: Vec<Entity> = match mark {
                ArmourMark::Bounce { struck, .. } => owner(*struck).into_iter().collect(),
                // A crossing touches each tank when it reaches it (`PenetrationMarks::reached`).
                ArmourMark::Crossing { .. } => Vec::new(),
                ArmourMark::Wound { volumes, .. } => volumes
                    .iter()
                    .filter_map(|crossed| owner(crossed.volume))
                    .collect(),
                ArmourMark::Burst { tank, .. } => tank.iter().copied().collect(),
            };
            touched.sort();
            touched.dedup();
            touched
        };
        let mut events: Vec<(usize, Entity)> = marks.reached[reached_from..].to_vec();
        for (index, mark) in marks.log.iter().enumerate().skip(marks_from) {
            events.extend(touched(mark).into_iter().map(|tank| (index, tank)));
        }
        events.sort_by_key(|&(index, _)| index);
        for &(_, tank) in &events {
            if !contacted.contains(&tank) {
                contacted.push(tank);
                recorder.husk_strikes += u64::from(self.knocked_out.contains(tank));
            }
        }
        let Some(mut recording) = recording else {
            return;
        };
        let tick = self.clock.as_ref().map_or(0, |clock| clock.0);
        for (index, tank) in events {
            let latest = recording
                .struck
                .iter()
                .rposition(|struck| struck.tank == tank);
            let at = match latest {
                Some(at) if !recording.struck[at].done => at,
                _ => {
                    // The game's damage model: a husk has no new encounter. This read is
                    // before this tick's knockout consequences, so the knockout shot records.
                    if self.knocked_out.contains(tank) {
                        continue;
                    }
                    let encounter = latest.map_or(0, |at| recording.struck[at].encounter + 1);
                    let boxes = self
                        .tanks
                        .get(tank)
                        .into_iter()
                        .flat_map(|volumes| volumes.iter())
                        .filter_map(|volume| Some((volume, health.get(volume).ok()?.held())))
                        .collect();
                    recording.struck.push(Struck {
                        tank,
                        encounter,
                        contact_tick: tick,
                        from: index,
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
            if let Some(frame) = marks.frames.get(&(index, tank)) {
                struck.frames.push((index, Some(*frame)));
                recorder.unposed |= struck.servo_deg.is_none();
            } else if !matches!(struck.frames.last(), Some((_, None))) {
                struck.frames.push((index, None));
                recorder.unposed = true;
            }
        }
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<ShotRecorder>()
        .add_message::<RecordedShot>()
        .add_observer(keep_spent)
        .add_systems(
            FixedPostUpdate,
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
                    .in_set(SimPhase::RecordedConsequences),
            )
                .in_set(GameplaySet)
                .run_if(not(resource_exists::<ClientReplica>)),
        );
}

/// Hand a despawning recorded shell's recording and logs to the recorder while it has an encounter
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
        burst: spall.burst.take(),
        path: std::mem::take(&mut path.points),
        dose: std::mem::take(&mut *dose),
    });
}

/// The game's damage model: fill standalone static frames and read each struck tank's servo
/// angles the first time. Runtime contact frames are already carried by immutable playback.
fn pose_struck(world: &mut World) {
    world.resource_mut::<ShotRecorder>().unposed = false;
    let mut live = world.query::<&ShotRecording>();
    let mut tanks = BTreeMap::<Entity, bool>::new();
    for struck in live
        .iter(world)
        .chain(
            world
                .resource::<ShotRecorder>()
                .spent
                .iter()
                .map(|spent| &spent.recording),
        )
        .flat_map(|recording| &recording.struck)
    {
        if struck.servo_deg.is_none() || matches!(struck.frames.last(), Some((_, None))) {
            *tanks.entry(struck.tank).or_default() |= struck.servo_deg.is_none();
        }
    }
    let posed: Vec<(Entity, Option<HullFrame>, Option<BTreeMap<String, f32>>)> = tanks
        .into_iter()
        .map(|(tank, needs_angles)| {
            let frame = hull_frame(world, tank);
            // The game's damage model captures angles once at the living strike. A later frame
            // placement must not read a mechanism that may have detached after that strike.
            let angles = if needs_angles {
                record::servo_angles(world, tank)
            } else {
                None
            };
            (tank, frame, angles)
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
        if let Some(servo_deg) = servo_deg {
            struck.servo_deg.get_or_insert_with(|| servo_deg.clone());
        } // Captured angles stay in place; a despawned tank supplies no new pose.
    };
    let mut live = world.query::<&mut ShotRecording>();
    for mut recording in live.iter_mut(world) {
        recording.struck.iter_mut().for_each(pose);
    }
    for spent in &mut world.resource_mut::<ShotRecorder>().spent {
        spent.recording.struck.iter_mut().for_each(pose);
    }
}

/// The game's damage model: the hull frame of `tank` at completed collider placement, before
/// consequences can detach its parts. The body pose is read back from a composed child collider,
/// or from the body itself when its volumes are bodies of their own.
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

/// Decide each tank knocked out this tick: the shell whose deposition emptied the box that decided
/// it. Raise [`TankKnockout`] for every one, with that shell's round or none, and attribute the knockout
/// to the shell's open encounter with the tank (the one this tick's marks are in); then clear the
/// tick's emptied log.
fn attribute_kills(
    knocked: Query<(Entity, &TankKnockedOut), Added<TankKnockedOut>>,
    facets: Query<(&VolumeOf, Has<Ammo>, Has<CrewStation>, Has<Dead>)>,
    mut recordings: Query<&mut ShotRecording>,
    mut recorder: ResMut<ShotRecorder>,
    mut commands: Commands,
) {
    let recorder = &mut *recorder;
    for (tank, knocked_out) in &knocked {
        let reason = knocked_out.reason;
        let decides = |emptied: &&Emptied| {
            facets
                .get(emptied.volume)
                .is_ok_and(|(owner, ammo, crew, _)| {
                    owner.tank() == tank
                        && match reason {
                            KnockoutReason::Cookoff => ammo,
                            KnockoutReason::CrewLoss => crew,
                        }
                })
        };
        let killer = match reason {
            KnockoutReason::Cookoff => recorder.emptied.iter().find(decides),
            // The emptying that left fewer than two crew members in action
            // (`damage::knockout_from_counts`): with `left` in action after this tick's emptied
            // crew pools, the one that took the count from two to one.
            KnockoutReason::CrewLoss => {
                let emptied: Vec<&Emptied> = recorder.emptied.iter().filter(decides).collect();
                let left = facets
                    .iter()
                    .filter(|&(owner, _, crew, dead)| owner.tank() == tank && crew && !dead)
                    .count();
                (emptied.len() + left)
                    .checked_sub(2)
                    .and_then(|at| emptied.get(at))
                    .copied()
            }
        };
        commands.trigger(TankKnockout {
            tank,
            cause: match killer.and_then(|killer| killer.shot) {
                Some(shot) => KnockoutCause::Round(shot),
                None => KnockoutCause::NoKeyedRound,
            },
        });
        let Some(&Emptied { shell, .. }) = killer else {
            continue;
        };
        let struck = match recordings.get_mut(shell) {
            Ok(recording) => recording.into_inner().struck.iter_mut(),
            Err(_) => match recorder.spent.iter_mut().find(|spent| spent.shell == shell) {
                Some(spent) => spent.recording.struck.iter_mut(),
                None => continue,
            },
        };
        if let Some(struck) = struck.rev().find(|struck| struck.tank == tank) {
            struck.killed = Some(reason);
        }
    }
    recorder.emptied.clear();
}

/// Finish every encounter whose shell ended, or is clear of the tank with no fuze that tank armed
/// still running, and emit its record.
fn finish_records(world: &mut World) {
    let spent = std::mem::take(&mut world.resource_mut::<ShotRecorder>().spent);
    let mut finished: Vec<(Entity, usize)> = Vec::new();
    let mut records = Vec::new();
    let mut live = world.query::<(Entity, &ShotRecording, Option<&Flying>, &Transform)>();
    for (shell, recording, flying, at) in live.iter(world) {
        for (index, struck) in recording.struck.iter().enumerate() {
            let open = flying.is_some_and(|round| {
                round.armed_by(struck.tank) || !clear_of(world, struck.tank, at.translation)
            });
            if struck.done || open {
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
            burst: spent.burst.as_ref(),
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

/// The record of `recording`'s round in encounter `struck`, its logs `log`; `None` where the tank
/// is gone, a frame was never posed, or the encounter opens no pen cam.
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
    let hull = frames.first().map(|(_, frame)| frame.pose());
    let servo_deg = struck
        .servo_deg
        .as_ref()
        .expect("the game's damage model poses every live struck tank before finishing its record")
        .clone();
    let tank = StruckTank::recorded(
        struck.tank,
        struck.from,
        frames,
        servo_deg,
        struck.boxes.clone(),
    );
    let mut report = record::report_from(world, &tank, log, &recording.facts)?;
    report.killed = struck.killed;
    if report.vehicle.is_none() {
        report.vehicle = world
            .get::<TankSpecHandle>(struck.tank)
            .and_then(|spec| spec.0.path())
            .map(|path| path.path().to_string_lossy().into_owned());
    }
    let life = world
        .get::<CombatantId>(struck.tank)
        .zip(world.get::<CombatantLife>(struck.tank))
        .map(|(&combatant, &life)| TankLife { combatant, life });
    Some(RecordedShot {
        life,
        shot: recording.shot,
        encounter: struck.encounter,
        contact_tick: struck.contact_tick,
        completion_tick: world.get_resource::<ShotClock>().map_or(0, |clock| clock.0),
        outcome: sanctioned_outcome(log.marks, struck.from),
        hull,
        report,
    })
}

/// The game's damage model: the explanation's first-receipt ordinal for the encounter whose
/// first mark is `marks[from]`. Bounces take their sequence and the initial crossing takes the
/// count of preceding bounces (`ShellTerminal::after_bounces`). Later crossings render from
/// public paths, but this explanation receipt mapper returns `None` for those encounters.
///
/// [`ShellRicochet::sequence`]: crate::ballistics::ShellRicochet
/// [`ShellTerminal::after_bounces`]: crate::ballistics::ShellTerminal
fn sanctioned_outcome(marks: &[ArmourMark], from: usize) -> Option<u32> {
    let before = marks.get(..from)?;
    if !before
        .iter()
        .all(|mark| matches!(mark, ArmourMark::Bounce { .. } | ArmourMark::Wound { .. }))
    {
        return None;
    }
    // A witness mark is no outcome the authority sends: the ordinal counts bounces, and the outcome
    // drawn is the encounter's first hard mark.
    let ordinal = before
        .iter()
        .filter(|mark| matches!(mark, ArmourMark::Bounce { .. }))
        .count();
    match marks
        .get(from..)?
        .iter()
        .find(|mark| !matches!(mark, ArmourMark::Wound { .. }))?
    {
        ArmourMark::Bounce { .. } | ArmourMark::Crossing { .. } => u32::try_from(ordinal).ok(),
        ArmourMark::Wound { .. } | ArmourMark::Burst { .. } => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The game's damage model: the server reports rounds, not march ticks. A first contact
    /// while alive is never reclassified by later witness/reach marks after knockout.
    #[test]
    fn husk_statistics_count_first_contact_once_per_round_and_tank() {
        for husk_at_contact in [true, false] {
            let mut world = World::new();
            world.init_resource::<ShotRecorder>();
            let tank = world.spawn_empty().id();
            let shell = world.spawn_empty().id();
            if husk_at_contact {
                world.entity_mut(tank).insert(TankKnockedOut {
                    reason: KnockoutReason::CrewLoss,
                });
            }
            let mut marks = PenetrationMarks::default();
            let dose = ShotDose::default();
            let mut contacted = Vec::new();
            let mut state =
                SystemState::<(MarchRecorder, Query<&VolumeOf>, Query<&mut ComponentHealth>)>::new(
                    &mut world,
                );
            for step in 0..3 {
                marks.reached.push((step, tank));
                let (mut recorder, owners, health) = state.get_mut(&mut world).unwrap();
                recorder.note(
                    shell,
                    &mut contacted,
                    None,
                    None,
                    &marks,
                    (0, step),
                    &dose,
                    0,
                    &owners,
                    &health,
                );
                world.entity_mut(tank).insert(TankKnockedOut {
                    reason: KnockoutReason::CrewLoss,
                });
            }
            assert_eq!(
                world.resource::<ShotRecorder>().husk_strikes(),
                u64::from(husk_at_contact),
                "one round's later marks neither recount a husk nor turn its living strike into one"
            );
        }
    }

    /// The game's damage model: an encounter opened while the target lived keeps its strike
    /// angles when later static-frame placement runs after knockout. The record still needs a
    /// hull frame, but the detached mechanism cannot supply new angles. Fixture pose data.
    #[test]
    fn an_open_encounter_keeps_its_pose_after_knockout() {
        let mut world = World::new();
        world.init_resource::<ShotRecorder>();
        let tank = world
            .spawn((
                Transform::default(),
                GlobalTransform::default(),
                Position(Vec3::ZERO),
                Rotation::IDENTITY,
                crate::tank::TankServos::for_count(1),
                TankKnockedOut {
                    reason: KnockoutReason::Cookoff,
                },
            ))
            .id();
        world.entity_mut(tank).insert(Rig {
            hull: tank,
            turret: tank,
            gun: tank,
            muzzle: tank,
        });
        let angles = BTreeMap::from([("fixture servo".to_owned(), 42.0)]);
        let shell = world
            .spawn(ShotRecording {
                facts: ShotFacts::default(),
                shot: None,
                struck: vec![Struck {
                    tank,
                    encounter: 0,
                    contact_tick: 0,
                    from: 0,
                    boxes: Vec::new(),
                    frames: vec![
                        (0, Some(HullFrame::posed(Vec3::ZERO, Quat::IDENTITY))),
                        (1, None),
                    ],
                    servo_deg: Some(angles.clone()),
                    killed: Some(KnockoutReason::Cookoff),
                    done: false,
                }],
            })
            .id();
        assert!(
            StruckTank::capture(&mut world, tank).is_none(),
            "a husk opens no new capture"
        );
        pose_struck(&mut world);
        let struck = &world.get::<ShotRecording>(shell).unwrap().struck[0];
        assert_eq!(struck.servo_deg.as_ref(), Some(&angles));
        assert!(
            struck.frames[1].1.is_some(),
            "the later hull frame is filled"
        );
    }
}
