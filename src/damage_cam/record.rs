//! The sim-side fill: a [`PenetrationReport`] read off a shell the authority marched.
//!
//! Two reads bracket the shot. [`StruckTank::capture`] takes the struck tank's hull frame, its
//! components and their HP, and every servo's angle, before it; [`report`] reads the shell's armour
//! log ([`crate::ballistics::ArmourMark`]), its spall cones, its burst's field and its own per-box
//! tally ([`ShotDose`]) after it, and states it all in the struck hull's frame. In the game the
//! recorder (`super::recorder`) fills the same [`StruckTank`] from the pose each mark was resolved
//! against and reads the logs through `report_from`.
//!
//! A report covers one encounter of the shell with the struck tank, its marks and settles from the
//! encounter's first mark on (the whole flight, for a capture). What a component's pool held at the
//! shot is what the encounter's first settle of its pool found there ([`ShotDose::settles`]), and
//! one it never reached reads what the struck tank's capture holds. What it lost, whether the shot
//! emptied it and the deposit that did are the shot's own settles, never a difference of pool
//! readings, so a shot overlapping another reads only its own loss, and another round draining the
//! pool between the shot's marks moves the emptying to the deposit that actually took the last
//! pool unit. Every pool figure is in pool units, as the sim keeps it; the path's
//! [`PathStep::Dosed`] reads the shown points the views read ([`crate::ballistics::shown_lost`]).
//!
//! Nothing here decides anything the march did not: the verdict is [`ShotVerdict::of_path`] over
//! the logged path, plate classes come from the volumes' components, the round's end is the last
//! crossing, bounce or burst the log has on the struck tank, and every dose is one the march
//! recorded. The outcome is the struck tank's alone: first contact, plates, bounces, the burst, the
//! fields and their fragments' hits are read only off marks and volumes the tank owns, and the path
//! stops one leg past the struck tank's last mark. No component geometry is read: the record names
//! each component, and its shape stays the tank asset's.
//!
//! The path's times are DERIVED, not marched: each leg's length over the speed in effect on it
//! (the round's speed between crossings, the mean of its entry and exit speeds inside one, half
//! its entry speed inside the crossing it stops in).

use std::collections::BTreeMap;

use bevy::math::Affine3A;
use bevy::prelude::*;

use super::{
    BoxDose, Burst, ComponentState, Consequence, ConsequenceState, Fragment, FragmentHit, Impact,
    PathPoint, PathStep, PenetrationReport, Plate, PlateClass, Projectile, Ricochet, Role,
    RoundEnd, Shooter, ShotVerdict, SpallCone,
};
use crate::ballistics::SpallCone as ThrownCone;
use crate::ballistics::spall_cone::FieldDescriptor;
use crate::ballistics::{
    ArmourMark, BallisticVolume, BurstField, ComponentHealth, CrossingEnd, ExternalArmour,
    PenetrationMarks, ShellPath, ShotDose, SpallMarks, SpallRay, VolumeCrossing, dose_units,
    energy_units, field_units, hp_before_rounding, pool_units_of,
};
use crate::damage::{Ammo, CrewStation, FunctionRole, VolumeOf};
use crate::tank::{
    Muzzle, RemoteServos, Rig, ServoIndex, TankRoot, TankServos, Weapon, rig_world_pose,
};
use avian3d::prelude::{Position, Rotation};
use bevy::ecs::system::SystemState;

/// What a volume is to the record.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum VolumeClass {
    Main,
    External,
    Box,
}

/// A hull frame: world → hull-local, and the hull node's pose it inverts.
#[derive(Clone, Copy)]
pub(crate) struct HullFrame {
    to_hull: Affine3A,
    /// Rotation only: for directions.
    to_hull_rotation: Quat,
    /// The hull node's position and rotation (WORLD).
    pose: (Vec3, Quat),
}

impl HullFrame {
    /// The frame of a hull node posed at `position`, `rotation` (WORLD).
    pub(crate) fn posed(position: Vec3, rotation: Quat) -> Self {
        Self {
            to_hull: Affine3A::from_rotation_translation(rotation, position).inverse(),
            to_hull_rotation: rotation.inverse(),
            pose: (position, rotation),
        }
    }

    fn of(global: &GlobalTransform) -> Self {
        let (_, rotation, translation) = global.to_scale_rotation_translation();
        Self {
            to_hull: global.affine().inverse(),
            to_hull_rotation: rotation.inverse(),
            pose: (translation, rotation),
        }
    }

    /// The hull node's position and rotation (WORLD).
    pub(crate) fn pose(&self) -> (Vec3, Quat) {
        self.pose
    }
}

/// The struck tank at the shot: the first mark of the shell's encounter with it, the hull frame each
/// of the shell's marks was resolved in, its servos' angles and what its components' pools held.
pub struct StruckTank {
    tank: Entity,
    /// The encounter's first mark: the report reads the shell's marks and settles from it on.
    from: usize,
    /// `(first mark, frame)`, ascending: a mark is stated in the last frame that starts at or
    /// before it, a point read before the first mark in the first frame.
    frames: Vec<(usize, HullFrame)>,
    /// Every servo's angle (degrees), by node name.
    servo_deg: BTreeMap<String, f32>,
    /// Every component the tank owns and the pool units its pool held.
    boxes: Vec<(Entity, u32)>,
}

impl StruckTank {
    /// The tank's hull frame, every servo's angle and every component it owns, by name, read now
    /// for a shot about to be marched. `None` for an entity with no rig.
    pub fn capture(world: &mut World, tank: Entity) -> Option<Self> {
        let hull = world.get::<Rig>(tank)?.hull;
        let frame = HullFrame::of(world.get::<GlobalTransform>(hull)?);
        let mut owned = world.query::<(Entity, &VolumeOf, &ComponentHealth)>();
        let boxes = owned
            .iter(world)
            .filter(|(_, owner, _)| owner.tank() == tank)
            .map(|(entity, _, hp)| (entity, hp.held()))
            .collect();
        Some(Self {
            tank,
            from: 0,
            frames: vec![(0, frame)],
            servo_deg: servo_angles(world, tank),
            boxes,
        })
    }

    /// The tank as a recorder saw it in an encounter from mark `from` on: `frames` by first mark,
    /// ascending, never empty.
    pub(crate) fn recorded(
        tank: Entity,
        from: usize,
        frames: Vec<(usize, HullFrame)>,
        servo_deg: BTreeMap<String, f32>,
        boxes: Vec<(Entity, u32)>,
    ) -> Self {
        Self {
            tank,
            from,
            frames,
            servo_deg,
            boxes,
        }
    }

    fn frame(&self, mark: usize) -> &HullFrame {
        let at = self
            .frames
            .iter()
            .rposition(|(first, _)| *first <= mark)
            .unwrap_or(0);
        &self.frames[at].1
    }

    /// `world` (a point of mark `mark`, WORLD) in the hull frame.
    fn point(&self, mark: usize, world: Vec3) -> [f32; 3] {
        self.frame(mark).to_hull.transform_point3(world).to_array()
    }

    /// `world` (a direction of mark `mark`, WORLD) in the hull frame.
    fn direction(&self, mark: usize, world: Vec3) -> [f32; 3] {
        (self.frame(mark).to_hull_rotation * world).to_array()
    }
}

/// One shell's logs as the march left them: its armour log, the cones it threw, its burst's field,
/// its path and its per-box tally (`None` where the shell carries none).
pub(crate) struct ShotLog<'a> {
    pub marks: &'a [ArmourMark],
    pub cones: &'a [ThrownCone],
    /// The field of the log's one [`ArmourMark::Burst`], where it threw one.
    pub burst: Option<&'a BurstField>,
    pub path: &'a [Vec3],
    pub dose: Option<&'a ShotDose>,
}

impl<'a> ShotLog<'a> {
    /// `shell`'s logs, read off its components.
    pub(crate) fn read(world: &'a World, shell: Entity) -> Option<Self> {
        let spall = world.get::<SpallMarks>(shell)?;
        Some(Self {
            marks: &world.get::<PenetrationMarks>(shell)?.log,
            cones: &spall.cones,
            burst: spall.burst.as_ref(),
            path: &world.get::<ShellPath>(shell)?.points,
            dose: world.get::<ShotDose>(shell),
        })
    }
}

/// The shot's facts the march does not carry: who fired, from where, what was fired, and what was
/// struck.
#[derive(Default)]
pub struct ShotFacts {
    pub shooter: Option<Shooter>,
    /// Where the round started (WORLD).
    pub launch: Vec3,
    /// The shooter's muzzle at fire time (WORLD), [`muzzle_of`].
    pub muzzle: Option<Vec3>,
    pub projectile: String,
    pub caliber_mm: f32,
    pub mass_kg: f32,
    /// The round fired: its key in the round registry.
    pub round: String,
    /// The struck vehicle's tank file, relative to the asset root.
    pub vehicle: Option<String>,
}

/// Where `shooter`'s weapon `weapon` fires from now (WORLD): its muzzle node's tick-truth pose,
/// composed from the shooter's root through its rig's local transforms, the pose `shooting::fire`
/// launches a round from. `None` where the shooter has no such weapon.
pub fn muzzle_of(world: &mut World, shooter: Entity, weapon: &str) -> Option<Vec3> {
    let position = world.get::<Position>(shooter)?.0;
    let rotation = world.get::<Rotation>(shooter)?.0;
    let mut state = SystemState::<(
        Query<(Entity, &Weapon, &TankRoot), With<Muzzle>>,
        Query<&ChildOf>,
        Query<&Transform>,
    )>::new(world);
    let (weapons, parents, locals) = state.get(world).ok()?;
    let (muzzle, ..) = weapons
        .iter()
        .find(|(_, fired, root)| root.0 == shooter && fired.name == weapon)?;
    rig_world_pose(muzzle, shooter, position, rotation, &parents, &locals).map(|(at, _)| at)
}

/// Every servo of `tank` by node name, at the sim's own angle (degrees): the integrator state its
/// node is posed from, a replica's [`RemoteServos`] before the authority's [`TankServos`], the
/// precedence `restore_servo_truth` reads them in.
pub(crate) fn servo_angles(world: &mut World, tank: Entity) -> BTreeMap<String, f32> {
    let remote = world
        .get::<RemoteServos>(tank)
        .map(|servos| servos.0.clone());
    let authority = world
        .get::<TankServos>(tank)
        .map(|servos| servos.states.clone());
    let Some(states) = remote.or(authority) else {
        return BTreeMap::new();
    };
    let mut servos = world.query::<(&Name, &ServoIndex, &TankRoot)>();
    servos
        .iter(world)
        .filter(|(_, _, root)| root.0 == tank)
        .filter_map(|(name, slot, _)| {
            let angle = states.get(slot.0)?.current();
            Some((name.as_str().to_owned(), angle.to_degrees()))
        })
        .collect()
}

/// The path `shell` took through `tank` (every tank, for `None`), as the verdict reads it: its
/// bounces off and crossings of the tank's volumes and every walk that failed closed, in march
/// order, then one [`PathStep::Dosed`] when one of the tank's components shows a lost HP point to
/// the shot ([`shown_lost_from`]).
pub fn path_steps(world: &World, shell: Entity, tank: Option<Entity>) -> Vec<PathStep> {
    let Some(marks) = world.get::<PenetrationMarks>(shell) else {
        return Vec::new();
    };
    steps_of(world, &marks.log, world.get::<ShotDose>(shell), tank, 0)
}

/// [`path_steps`] over a shell's logs from mark `from` on.
fn steps_of(
    world: &World,
    marks: &[ArmourMark],
    dose: Option<&ShotDose>,
    tank: Option<Entity>,
    from: usize,
) -> Vec<PathStep> {
    let mut steps = Vec::new();
    for mark in marks.iter().skip(from) {
        let step = match mark {
            ArmourMark::Bounce { .. } => Some(PathStep::Bounce),
            ArmourMark::Crossing {
                entrance,
                incoming,
                volumes,
                end,
                ..
            } => share(world, *entrance, *incoming, volumes, end, tank).map(|share| {
                PathStep::Crossing {
                    perforated_armour: share.perforated_armour,
                    rested: matches!(share.end, ShareEnd::Rest { .. }),
                }
            }),
            ArmourMark::Failed => Some(PathStep::Failed),
            // A witness's wound is a dose, never a step of the path: the hard world's marks alone
            // decide the verdict, and a dose reaches it as `Dosed` below. So is a burst's field.
            ArmourMark::Wound { .. } | ArmourMark::Burst { .. } => None,
        };
        // A crossing has a step for the tank only where one of its volumes is the tank's.
        if let Some(step) = step
            && tank.is_none_or(|tank| touches(world, mark, tank))
        {
            steps.push(step);
        }
    }
    let dosed = dose.is_some_and(|dose| {
        dose.losses().any(|(volume, _)| {
            shown_lost_from(dose, volume, from) > 0
                && class_of(world, volume) == VolumeClass::Box
                && tank.is_none_or(|tank| owned_by(world, volume, tank))
        })
    });
    if dosed {
        steps.push(PathStep::Dosed);
    }
    steps
}

/// The pool units `volume`'s pool lost to the shot's settles from mark `from` on.
fn lost_from(dose: &ShotDose, volume: Entity, from: usize) -> u32 {
    dose.settles(volume)
        .iter()
        .filter(|settle| settle.mark >= from)
        .map(|settle| settle.lost)
        .sum()
}

/// The shown HP points `volume`'s pool lost to the shot's settles from mark `from` on, as its
/// record shows them: what the first of those settles found ([`components`]'s `held`) less what
/// they took ([`lost_from`], the record's `taken`), through the display rule
/// ([`crate::ballistics::shown_lost`]). The record's own path ([`PenetrationReport::steps`]) reads
/// the same two figures the same way, so both read one verdict.
fn shown_lost_from(dose: &ShotDose, volume: Entity, from: usize) -> u32 {
    let Some(first) = dose
        .settles(volume)
        .iter()
        .find(|settle| settle.mark >= from)
    else {
        return 0;
    };
    crate::ballistics::shown_lost(first.held, lost_from(dose, volume, from))
}

/// One tank's part of a crossing, whose volumes may belong to several tanks where hulls overlap:
/// where the line entered the tank's first volume, how the crossing ended for the tank, and whether
/// it perforated the tank's armour. The tank's own volumes decide all three, so a round that
/// perforates one tank and rests in another inside one crossing has left the first and rests in
/// the second.
struct Share {
    entry: Vec3,
    end: ShareEnd,
    perforated_armour: bool,
}

enum ShareEnd {
    /// It came to rest in one of the tank's volumes (`None`: none is named by the march).
    Rest { volume: Option<Entity> },
    /// It left the tank's volumes at `at` (WORLD).
    Left { at: Vec3 },
}

/// `tank`'s share of a crossing (every volume's, for `None`); `None` when it crossed none of the
/// tank's volumes. A crossing all of whose volumes are the tank's keeps the march's own entrance
/// and end points.
fn share(
    world: &World,
    entrance: Vec3,
    incoming: Vec3,
    volumes: &[VolumeCrossing],
    end: &CrossingEnd,
    tank: Option<Entity>,
) -> Option<Share> {
    let mine: Vec<&VolumeCrossing> = volumes
        .iter()
        .filter(|crossed| tank.is_none_or(|tank| owned_by(world, crossed.volume, tank)))
        .collect();
    if mine.is_empty() {
        return None;
    }
    let whole = mine.len() == volumes.len();
    let along = |point: Vec3| (point - entrance).dot(incoming);
    let entry = if whole {
        entrance
    } else {
        mine.iter()
            .map(|crossed| crossed.entry)
            .min_by(|a, b| along(*a).total_cmp(&along(*b)))?
    };
    let last_out = || {
        mine.iter()
            .map(|crossed| crossed.end)
            .max_by(|a, b| along(*a).total_cmp(&along(*b)))
    };
    let end = match *end {
        CrossingEnd::Rest { at } => match rested_in(mine.iter().copied(), at) {
            Some(rest) => ShareEnd::Rest { volume: Some(rest) },
            None if whole => ShareEnd::Rest { volume: None },
            None => ShareEnd::Left { at: last_out()? },
        },
        CrossingEnd::Exit { at, .. } => ShareEnd::Left {
            at: if whole { at } else { last_out()? },
        },
    };
    Some(Share {
        entry,
        end,
        perforated_armour: mine.iter().any(|crossed| {
            crossed.perforated && class_of(world, crossed.volume) == VolumeClass::Main
        }),
    })
}

/// Whether a mark belongs to `tank`. A failed walk names no volume and belongs to every tank.
fn touches(world: &World, mark: &ArmourMark, tank: Entity) -> bool {
    let owned = |volume: Entity| owned_by(world, volume, tank);
    match mark {
        ArmourMark::Bounce { struck, .. } => owned(*struck),
        ArmourMark::Crossing { volumes, .. } | ArmourMark::Wound { volumes, .. } => {
            volumes.iter().any(|crossed| owned(crossed.volume))
        }
        ArmourMark::Burst { tank: walked, .. } => *walked == Some(tank),
        ArmourMark::Failed => true,
    }
}

fn owned_by(world: &World, volume: Entity, tank: Entity) -> bool {
    world
        .get::<VolumeOf>(volume)
        .is_some_and(|owner| owner.tank() == tank)
}

/// One timed node of the path, WORLD, and the mark whose frame states it.
struct Node {
    point: Vec3,
    t: f32,
    mark: usize,
}

/// `tank`'s share of a crossing `mark`; `None` for any other mark.
fn share_of(world: &World, mark: &ArmourMark, tank: Entity) -> Option<Share> {
    match mark {
        ArmourMark::Crossing {
            entrance,
            incoming,
            volumes,
            end,
            ..
        } => share(world, *entrance, *incoming, volumes, end, Some(tank)),
        _ => None,
    }
}

/// The first mark of `marks[from..=last_touch]` past its last hard one: every wound mark from here
/// on lies beyond the hard world's last point of the encounter, so it extends the path and states
/// the round's end. `from` when the encounter has no hard mark.
fn wound_tail(marks: &[ArmourMark], from: usize, last_touch: usize) -> usize {
    marks[..=last_touch]
        .iter()
        .enumerate()
        .skip(from)
        .rfind(|(_, mark)| !matches!(mark, ArmourMark::Wound { .. }))
        .map_or(from, |(index, _)| index + 1)
}

/// The struck tank's witnesses a wound mark crossed: where the line first entered one and where it
/// last left one (or the leg ended), and whether it left every one.
fn wound_span(
    world: &World,
    volumes: &[VolumeCrossing],
    tank: Entity,
) -> Option<(Vec3, Vec3, bool)> {
    let mut owned = volumes
        .iter()
        .filter(|crossed| owned_by(world, crossed.volume, tank));
    let first = owned.next()?;
    let (mut far, mut left) = (first.end, first.perforated);
    for crossed in owned {
        if crossed.end.distance(first.entry) >= far.distance(first.entry) {
            (far, left) = (crossed.end, crossed.perforated);
        }
    }
    Some((first.entry, far, left))
}

/// The encounter's path, from its start up to the struck tank's last mark `last_touch`.
struct Timeline {
    /// Timed nodes, t = 0 at the struck tank's first contact.
    nodes: Vec<Node>,
    /// Where the path starts (WORLD): the launch, or for a later encounter where the round left
    /// the mark before it.
    start: Vec3,
    /// The round's speed at first contact.
    contact_speed: f32,
}

/// The encounter's [`Timeline`]; `None` where a mark failed or the encounter has no start.
fn timeline(
    world: &World,
    struck: &StruckTank,
    log: &ShotLog,
    facts: &ShotFacts,
    first_touch: usize,
    last_touch: usize,
) -> Option<Timeline> {
    let marks = log.marks;
    // A wound mark moves nothing, so the path starts at the hard mark before the encounter.
    let before = marks[..struck.from]
        .iter()
        .rev()
        .find(|mark| !matches!(mark, ArmourMark::Wound { .. }));
    let start = match before {
        None => facts.launch,
        Some(ArmourMark::Bounce { position, .. }) => *position,
        Some(ArmourMark::Crossing {
            end: CrossingEnd::Exit { at, .. },
            ..
        }) => *at,
        Some(_) => return None,
    };
    // Each node with the speed of the leg that reaches it and the mark whose frame states it;
    // `contact` is the node of the struck tank's first contact.
    let mut legs: Vec<(Vec3, f32, usize)> = Vec::new();
    let mut speed = match marks.get(struck.from)? {
        ArmourMark::Bounce { speed_in, .. } | ArmourMark::Crossing { speed_in, .. } => *speed_in,
        ArmourMark::Wound { speed, .. } => *speed,
        ArmourMark::Failed | ArmourMark::Burst { .. } => return None,
    };
    legs.push((start, speed, first_touch));
    let mut last = start;
    // The farthest point a wound mark past the hard world put on the path, on the straight flight
    // from `last`.
    let mut beyond: Option<Vec3> = None;
    let mut flying = true;
    let mut contact = 0;
    let tail = wound_tail(marks, struck.from, last_touch);
    let span = marks[..=last_touch].iter().enumerate().skip(struck.from);
    for (index, mark) in span {
        if index == first_touch {
            contact = legs.len();
        }
        match mark {
            ArmourMark::Bounce {
                position,
                speed_out,
                ..
            } => {
                legs.push((*position, speed, index));
                speed = *speed_out;
                last = *position;
            }
            ArmourMark::Crossing {
                entrance,
                speed_in,
                end,
                ..
            } => {
                legs.push((*entrance, *speed_in, index));
                // The struck tank's first contact is its own first volume, which an overlapping
                // tank's volume may precede inside the crossing.
                if index == first_touch
                    && let Some(share) = share_of(world, mark, struck.tank)
                    && share.entry.distance(*entrance) > 1.0e-6
                {
                    contact = legs.len();
                    legs.push((share.entry, *speed_in, index));
                }
                match *end {
                    CrossingEnd::Rest { at } => {
                        legs.push((at, 0.5 * speed_in, index));
                        last = at;
                        flying = false;
                    }
                    CrossingEnd::Exit { at, speed: out, .. } => {
                        legs.push((at, 0.5 * (speed_in + out), index));
                        speed = out;
                        last = at;
                    }
                }
            }
            ArmourMark::Failed => return None,
            // The round's flight to the burst point, at the speed of the exit or bounce that last
            // started the burst trigger's delay: the speed the trigger counts that path at, so the
            // burst lies the delay after that exit (a round at rest there waits the mark's
            // `after` on top). The nose piece flies on at the carried speed; a round at rest ends
            // there.
            ArmourMark::Burst {
                at, speed: carried, ..
            } => {
                legs.push((*at, speed, index));
                last = *at;
                flying &= *carried > 0.0;
                if *carried > 0.0 {
                    speed = *carried;
                }
            }
            ArmourMark::Wound { volumes, speed, .. } => {
                if index >= tail
                    && let Some((entry, exit, _)) = wound_span(world, volumes, struck.tank)
                {
                    legs.push((entry, *speed, index));
                    legs.push((exit, *speed, index));
                    beyond = Some(exit);
                }
            }
        }
    }
    // The flight runs on to the next point of the path past everything the encounter reached.
    if flying
        && let Some(index) = log.path.iter().rposition(|point| *point == last)
        && let Some(next) = match beyond {
            None => log.path.get(index + 1),
            Some(reached) => log.path[index + 1..]
                .iter()
                .find(|point| point.distance(last) > reached.distance(last)),
        }
    {
        legs.push((*next, speed, last_touch));
    }
    let mut nodes: Vec<Node> = Vec::with_capacity(legs.len());
    let mut t = 0.0;
    for (index, &(point, speed, mark)) in legs.iter().enumerate() {
        if index > 0 {
            let length = point.distance(legs[index - 1].0);
            if speed > 0.0 {
                t += length / speed;
            }
        }
        nodes.push(Node { point, t, mark });
    }
    // The struck tank's first contact is t = 0.
    let contact_t = nodes.get(contact)?.t;
    for node in &mut nodes {
        node.t -= contact_t;
    }
    Some(Timeline {
        nodes,
        start,
        contact_speed: legs[contact].1,
    })
}

/// The report of `shell`'s flight against `struck`, or `None` when it opens no pen cam
/// ([`ShotVerdict::of_path`]).
pub fn report(
    world: &mut World,
    struck: &StruckTank,
    shell: Entity,
    facts: &ShotFacts,
) -> Option<PenetrationReport> {
    let world: &World = world;
    report_from(world, struck, &ShotLog::read(world, shell)?, facts)
}

/// The report of a shell's flight, read from its logs `log`, against `struck`, or `None` when it
/// opens no pen cam ([`ShotVerdict::of_path`]).
pub(crate) fn report_from(
    world: &World,
    struck: &StruckTank,
    log: &ShotLog,
    facts: &ShotFacts,
) -> Option<PenetrationReport> {
    let steps = steps_of(world, log.marks, log.dose, Some(struck.tank), struck.from);
    let verdict = ShotVerdict::of_path(&steps)?;
    let marks = log.marks;
    let owns = |mark: &ArmourMark| touches(world, mark, struck.tank);
    let first_touch = struck.from + marks.get(struck.from..)?.iter().position(owns)?;
    let last_touch = marks.iter().rposition(owns)?;

    let Timeline {
        nodes,
        start,
        contact_speed,
    } = timeline(world, struck, log, facts, first_touch, last_touch)?;
    let on = Encounter {
        world,
        struck,
        nodes: &nodes,
        tail: wound_tail(marks, struck.from, last_touch),
    };
    let mut ledger = Ledger::new(on);
    let mut thrown = log.cones.iter();
    for (index, mark) in marks[..=last_touch].iter().enumerate() {
        if index < struck.from || !owns(mark) {
            // Its cones are an earlier encounter's or another tank's; the cone log stays aligned
            // with the crossings.
            if let ArmourMark::Crossing { cones: count, .. } = mark {
                thrown.by_ref().take(*count).for_each(drop);
            }
            continue;
        }
        match mark {
            ArmourMark::Bounce {
                position,
                normal,
                incoming,
                outgoing,
                struck: volume,
                shock,
                ..
            } => {
                let directions = [*normal, *incoming, *outgoing];
                ledger.bounce(index, *position, directions, *volume, *shock);
            }
            ArmourMark::Crossing {
                volumes,
                overmatched,
                cones: count,
                ..
            } => {
                let cones = thrown.by_ref().take(*count);
                ledger.crossing(index, on.mine(mark), volumes, *overmatched, cones);
            }
            ArmourMark::Wound { volumes, .. } => ledger.wound(index, volumes),
            ArmourMark::Burst {
                at,
                speed,
                after,
                sideways,
                nose,
                ..
            } => ledger.burst(index, (*at, *speed, *after, *sideways), *nose, log.burst),
            ArmourMark::Failed => {}
        }
    }

    let first = &marks[first_touch];
    let (impact_point, normal, incoming) = match first {
        ArmourMark::Bounce {
            position,
            normal,
            incoming,
            ..
        } => (*position, *normal, *incoming),
        // The march records one entrance face per crossing, so a crossing that spans tanks
        // attributes it to every tank in it: a tank first entered inside the crossing keeps the
        // crossing's normal and incidence, though its impact point is its own entry.
        ArmourMark::Crossing {
            entrance,
            normal,
            incoming,
            ..
        } => (
            on.mine(first).map_or(*entrance, |share| share.entry),
            *normal,
            *incoming,
        ),
        // Met first by the line inside a witness: no face, so the line's own direction stands for
        // the normal.
        ArmourMark::Wound {
            volumes, incoming, ..
        } => (
            volumes
                .iter()
                .find(|crossed| on.owned(crossed.volume))
                .map(|crossed| crossed.entry)?,
            -*incoming,
            *incoming,
        ),
        ArmourMark::Failed | ArmourMark::Burst { .. } => return None,
    };
    let normal = normal.normalize_or_zero();
    let incidence = (-incoming).dot(normal).clamp(-1.0, 1.0).acos();

    let Ledger {
        mut plates,
        ricochets,
        spall,
        burst,
        line,
        field,
        entered,
        end,
        ..
    } = ledger;
    let components = components(world, struck, log.dose);
    let (doses, mut consequences) = outcomes(
        world,
        struck,
        &components,
        log.dose,
        &line,
        &field,
        &entered,
    );
    consequences.sort_by(|a, b| a.t.total_cmp(&b.t));
    // A crossing lists its volumes by identity; the record lists them in path order.
    plates.sort_by(|a, b| a.t.total_cmp(&b.t));

    Some(PenetrationReport {
        synthetic: false,
        vehicle: facts.vehicle.clone(),
        servo_deg: struck.servo_deg.clone(),
        round: facts.round.clone(),
        shooter: facts.shooter.clone(),
        origin: struck.point(first_touch, start),
        muzzle: facts.muzzle.map(|muzzle| struck.point(first_touch, muzzle)),
        projectile: Projectile {
            name: facts.projectile.clone(),
            caliber_mm: facts.caliber_mm,
            mass_kg: facts.mass_kg,
            speed_mps: contact_speed,
        },
        impact: Impact {
            point: struck.point(first_touch, impact_point),
            normal: struck.direction(first_touch, normal),
            incoming: struck.direction(first_touch, incoming),
            incidence_deg: incidence.to_degrees(),
            verdict,
        },
        plates,
        ricochets,
        path: nodes
            .iter()
            .map(|node| PathPoint {
                t: node.t,
                point: struck.point(node.mark, node.point),
            })
            .collect(),
        spall,
        burst,
        end: end?,
        doses,
        consequences,
        components: components.into_iter().map(|(_, state)| state).collect(),
        killed: None,
    })
}

/// One deposit on a component: `(volume, units, t, mark)`, its dose in the dose units the shot's
/// tally took it in (a hard box's line at the energy rate, a witness's chord and a bounce's shock
/// at the reference-mm rate, a field at its own: [`ShotDose`]) and `mark` the armour mark that
/// dealt it.
type Deposit = (Entity, u64, f32, usize);

/// What [`report_from`] reads each mark of an encounter against: the struck tank, the path's timed
/// nodes, and `tail`, the first mark past the encounter's last hard one ([`wound_tail`]).
#[derive(Clone, Copy)]
struct Encounter<'a> {
    world: &'a World,
    struck: &'a StruckTank,
    nodes: &'a [Node],
    tail: usize,
}

impl Encounter<'_> {
    /// Whether the struck tank owns `volume`.
    fn owned(&self, volume: Entity) -> bool {
        owned_by(self.world, volume, self.struck.tank)
    }

    /// The struck tank's share of a crossing `mark` ([`share_of`]).
    fn mine(&self, mark: &ArmourMark) -> Option<Share> {
        share_of(self.world, mark, self.struck.tank)
    }

    /// The path's time at `point` (WORLD), [`time_on`].
    fn time_at(&self, point: Vec3) -> f32 {
        time_on(self.nodes, point)
    }
}

/// The report's parts an encounter's marks add up to, filled by [`report_from`] one mark at a time
/// in path order: points and directions in the hull frame of the mark that states them, times on
/// the encounter's path.
struct Ledger<'a> {
    on: Encounter<'a>,
    plates: Vec<Plate>,
    ricochets: Vec<Ricochet>,
    spall: Vec<SpallCone>,
    burst: Option<Burst>,
    /// The line's deposits on the struck tank's components.
    line: Vec<Deposit>,
    /// The fields' deposits on them: the spall cones' and the burst's.
    field: Vec<Deposit>,
    /// Where the line reached each component (WORLD), with its time and the mark that reached it.
    entered: Vec<(Entity, f32, Vec3, usize)>,
    /// The round's end as the marks so far state it.
    end: Option<RoundEnd>,
}

impl<'a> Ledger<'a> {
    fn new(on: Encounter<'a>) -> Self {
        Self {
            on,
            plates: Vec::new(),
            ricochets: Vec::new(),
            spall: Vec::new(),
            burst: None,
            line: Vec::new(),
            field: Vec::new(),
            entered: Vec::new(),
            end: None,
        }
    }

    /// Mark `index`, a bounce at `position` off `volume`'s face, its `[normal, incoming, outgoing]`
    /// `directions` (WORLD): its ricochet, the `shock` it jarred into a box, and the round leaving
    /// there.
    fn bounce(
        &mut self,
        index: usize,
        position: Vec3,
        directions: [Vec3; 3],
        volume: Entity,
        shock: f32,
    ) {
        let on = self.on;
        let t = on.time_at(position);
        let [normal, incoming, outgoing] =
            directions.map(|direction| on.struck.direction(index, direction));
        self.ricochets.push(Ricochet {
            t,
            point: on.struck.point(index, position),
            normal,
            incoming,
            outgoing,
            volume: name_of(on.world, volume),
        });
        if class_of(on.world, volume) == VolumeClass::Box {
            self.line.push((volume, dose_units(shock), t, index));
            self.entered.push((volume, t, position, index));
        }
        self.end = Some(RoundEnd::Left {
            at: on.struck.point(index, position),
        });
    }

    /// Mark `index`, a crossing of `volumes` whose share for the struck tank is `share`: the round's
    /// end as the share states it, a plate per armour volume the tank owns, a line deposit and an
    /// entry per box, and the spall `cones` it threw with their fragments' deposits.
    fn crossing<'c>(
        &mut self,
        index: usize,
        share: Option<Share>,
        volumes: &[VolumeCrossing],
        overmatched: bool,
        cones: impl Iterator<Item = &'c ThrownCone>,
    ) {
        let on = self.on;
        self.end = share.map(|share| match share.end {
            ShareEnd::Left { at } => RoundEnd::Left {
                at: on.struck.point(index, at),
            },
            ShareEnd::Rest { volume } => RoundEnd::Rest {
                volume: volume.map_or_else(String::new, |volume| name_of(on.world, volume)),
            },
        });
        for crossed in volumes.iter().filter(|crossed| on.owned(crossed.volume)) {
            let t = on.time_at(crossed.entry);
            let class = class_of(on.world, crossed.volume);
            if class == VolumeClass::Box {
                self.line
                    .push((crossed.volume, energy_units(crossed.energy), t, index));
                self.entered.push((crossed.volume, t, crossed.entry, index));
                continue;
            }
            self.plates.push(Plate {
                name: name_of(on.world, crossed.volume),
                t,
                entry: on.struck.point(index, crossed.entry),
                exit: crossed
                    .perforated
                    .then(|| on.struck.point(index, crossed.end)),
                chord_mm: crossed.chord * 1000.0,
                steel_equiv_mm: crossed.cost,
                cost_mm: crossed.cost,
                overmatched,
                class: if class == VolumeClass::External {
                    PlateClass::External
                } else {
                    PlateClass::Main
                },
                factor: if crossed.chord > 0.0 {
                    crossed.cost / crossed.chord
                } else {
                    0.0
                },
            });
        }
        for cone in cones {
            let t = on.time_at(cone.origin);
            let thrown = (cone.origin, cone.axis, cone.field, cone.strength);
            let cone = self.field(index, t, thrown, &cone.rays);
            self.spall.push(cone);
        }
    }

    /// Mark `index`, the round's burst at `at` (WORLD) at the carried `speed`, `after` seconds past
    /// the round reaching `at`, at the greatest sideways speed `sideways`, its nose piece's mass
    /// `nose` and its field `thrown` (`None` where it threw none): the burst with its field's
    /// deposits. It sets no end: the nose piece flies on from a burst in flight, and a round at
    /// rest has ended where it rests.
    fn burst(
        &mut self,
        index: usize,
        (at, speed, after, sideways): (Vec3, f32, f32, f32),
        nose: f32,
        thrown: Option<&BurstField>,
    ) {
        let on = self.on;
        let t = on.time_at(at) + after;
        let field = thrown.map(|burst| {
            let throw = (burst.origin, burst.axis, burst.field, burst.strength);
            self.field(index, t, throw, &burst.rays)
        });
        let point = on.struck.point(index, at);
        self.burst = Some(Burst {
            t,
            point,
            speed_mps: speed,
            sideways_mps: sideways,
            nose_kg: nose,
            field,
        });
    }

    /// The field of mark `index` thrown at `t` from `origin` about `axis` (WORLD) by `field` at
    /// strength `S`, marched by `rays`: its fragments, its deposits on the struck tank's components
    /// added to the ledger's.
    fn field(
        &mut self,
        index: usize,
        t: f32,
        (origin, axis, field, strength): (Vec3, Dir3, FieldDescriptor, f32),
        rays: &[SpallRay],
    ) -> SpallCone {
        let on = self.on;
        let fragments = rays
            .iter()
            .map(|ray| {
                let doses = ray.doses.iter().filter(|(volume, _)| on.owned(*volume));
                for &(volume, dose) in doses.clone() {
                    self.field.push((volume, field_units(dose), t, index));
                }
                Fragment {
                    end: on.struck.point(index, ray.end),
                    strength: ray.strength,
                    hits: doses
                        .map(|&(volume, dose)| FragmentHit {
                            volume: name_of(on.world, volume),
                            damage: dose,
                        })
                        .collect(),
                }
            })
            .collect();
        SpallCone {
            t,
            origin: on.struck.point(index, origin),
            axis: on.struck.direction(index, Vec3::from(axis)),
            field,
            half_angle_deg: field.half_angle().to_degrees(),
            strength,
            fragments,
        }
    }

    /// Mark `index`, the line through the witnesses `volumes`: a line deposit and an entry per
    /// witness the struck tank owns, and past the hard world the round's end.
    fn wound(&mut self, index: usize, volumes: &[VolumeCrossing]) {
        let on = self.on;
        // The witness law's dose as it stands, in reference-mm: the energy rate is the hard
        // world's.
        for crossed in volumes.iter().filter(|crossed| on.owned(crossed.volume)) {
            let t = on.time_at(crossed.entry);
            self.line
                .push((crossed.volume, dose_units(crossed.cost), t, index));
            self.entered.push((crossed.volume, t, crossed.entry, index));
        }
        // Past the hard world, the round ends where it last left a witness, or at rest inside the
        // one its flight ended in.
        if index >= on.tail
            && let Some((_, exit, left)) = wound_span(on.world, volumes, on.struck.tank)
        {
            self.end = Some(if left {
                RoundEnd::Left {
                    at: on.struck.point(index, exit),
                }
            } else {
                RoundEnd::Rest {
                    volume: volumes
                        .iter()
                        .filter(|crossed| on.owned(crossed.volume))
                        .find(|crossed| crossed.end == exit)
                        .map_or_else(String::new, |crossed| name_of(on.world, crossed.volume)),
                }
            });
        }
    }
}

/// The struck tank's components by name, each at what its pool held at the shot: what the
/// encounter's first settle of its pool found there ([`ShotDose::settles`]), the tank's capture
/// elsewhere.
fn components(
    world: &World,
    struck: &StruckTank,
    dose: Option<&ShotDose>,
) -> Vec<(Entity, ComponentState)> {
    let mut components: Vec<(Entity, ComponentState)> = struck
        .boxes
        .iter()
        .filter_map(|&(entity, captured)| {
            let points = world.get::<ComponentHealth>(entity)?.points();
            let found = dose.and_then(|dose| {
                dose.settles(entity)
                    .iter()
                    .find(|settle| settle.mark >= struck.from)
                    .map(|settle| settle.held)
            });
            Some((
                entity,
                ComponentState {
                    name: name_of(world, entity),
                    role: role_of(world, entity),
                    count: points,
                    held: found.unwrap_or(captured),
                },
            ))
        })
        .collect();
    components.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    components
}

/// Each component's dose from the encounter and, for one it emptied, its consequence, timed at the
/// deposit that emptied it. `line` and `field` list the encounter's deposits, `entered` the
/// `(volume, t, point, mark)` where the line reached a component (WORLD). The pool units lost and
/// the emptying are the shot's own settles from the encounter's first mark on
/// ([`ShotDose::settles`]). A mark's deposits, its line and its cones, all land before its one
/// settle, so within a mark the pool has no order; the emptying deposit is the first, in path time,
/// whose running total reaches what the box held at the mark.
fn outcomes(
    world: &World,
    struck: &StruckTank,
    components: &[(Entity, ComponentState)],
    dose: Option<&ShotDose>,
    line: &[Deposit],
    field: &[Deposit],
    entered: &[(Entity, f32, Vec3, usize)],
) -> (Vec<BoxDose>, Vec<Consequence>) {
    let mut doses = Vec::new();
    let mut consequences = Vec::new();
    for (entity, state) in components {
        let of = |list: &[Deposit]| -> Vec<Deposit> {
            list.iter()
                .filter(|(volume, ..)| volume == entity)
                .copied()
                .collect()
        };
        let (from_line, from_field) = (of(line), of(field));
        if from_line.is_empty() && from_field.is_empty() {
            continue;
        }
        let first_entry = entered
            .iter()
            .filter(|(volume, ..)| volume == entity)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|&(_, _, point, mark)| struck.point(mark, point));
        let sum =
            |list: &[Deposit]| hp_before_rounding(list.iter().map(|&(_, units, ..)| units).sum());
        doses.push(BoxDose {
            volume: state.name.clone(),
            line: sum(&from_line),
            field: sum(&from_field),
            taken: dose.map_or(0, |dose| lost_from(dose, *entity, struck.from)),
            entered: first_entry,
        });
        let emptying = dose.and_then(|dose| {
            dose.settles(*entity)
                .iter()
                .find(|settle| settle.mark >= struck.from && settle.emptied())
        });
        if let Some(emptying) = emptying {
            // Each deposit in the units the shot's tally took it in.
            let mut landed: Vec<(u64, f32, usize)> = from_line
                .into_iter()
                .chain(from_field)
                .map(|(_, units, t, mark)| (units, t, mark))
                .filter(|&(.., mark)| mark == emptying.mark)
                .collect();
            landed.sort_by(|a, b| a.1.total_cmp(&b.1));
            let taken_before = pool_units_of(emptying.units_before);
            let mut total = emptying.units_before;
            let t = landed
                .iter()
                .find(|(units, ..)| {
                    total += units;
                    pool_units_of(total) - taken_before >= emptying.held
                })
                .or(landed.last())
                .map_or(0.0, |&(_, t, _)| t);
            consequences.push(Consequence {
                t,
                volume: state.name.clone(),
                state: if world.get::<Ammo>(*entity).is_some() {
                    ConsequenceState::Cookoff
                } else {
                    ConsequenceState::Lost
                },
                function: world
                    .get::<FunctionRole>(*entity)
                    .map(|function| format!("{function:?}")),
            });
        }
    }
    (doses, consequences)
}

/// The volume among `volumes` that a crossing ended at rest at `at` came to rest in: the one whose
/// line ends nearest `at`, short of perforating it. The march ends every unperforated volume's line
/// at the rest point (`resolve::volume_crossings`), so overlapping tanks' volumes each hold it and
/// each tank's own volumes name its own.
fn rested_in<'a>(volumes: impl Iterator<Item = &'a VolumeCrossing>, at: Vec3) -> Option<Entity> {
    volumes
        .filter(|crossed| !crossed.perforated)
        .min_by(|a, b| a.end.distance(at).total_cmp(&b.end.distance(at)))
        .map(|crossed| crossed.volume)
}

/// The time at which the path passes nearest `point`, interpolated along that leg.
fn time_on(nodes: &[Node], point: Vec3) -> f32 {
    let mut best = (f32::INFINITY, nodes.first().map_or(0.0, |node| node.t));
    for pair in nodes.windows(2) {
        let (a, b) = (pair[0].point, pair[1].point);
        let span = b - a;
        let along = span.length_squared();
        let s = if along > 0.0 {
            ((point - a).dot(span) / along).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let distance = point.distance(a + span * s);
        if distance < best.0 {
            best = (distance, pair[0].t + (pair[1].t - pair[0].t) * s);
        }
    }
    best.1
}

fn class_of(world: &World, volume: Entity) -> VolumeClass {
    if world.get::<ComponentHealth>(volume).is_some() {
        VolumeClass::Box
    } else if world.get::<ExternalArmour>(volume).is_some() {
        VolumeClass::External
    } else {
        VolumeClass::Main
    }
}

/// A volume's name: its node's, or for a primitive that is one substance of a mixed node,
/// `node/substance#k`, `k` its place among the node's volume-carrying children.
fn name_of(world: &World, volume: Entity) -> String {
    if let Some(name) = world.get::<Name>(volume) {
        return name.as_str().to_owned();
    }
    let Some(parent) = world.get::<ChildOf>(volume).map(ChildOf::parent) else {
        return format!("{volume}");
    };
    let node = world
        .get::<Name>(parent)
        .map_or_else(|| format!("{parent}"), |name| name.as_str().to_owned());
    let Some(found) = world.get::<BallisticVolume>(volume) else {
        return node;
    };
    let place = world.get::<Children>(parent).map_or(0, |children| {
        children
            .iter()
            .filter(|child| world.get::<BallisticVolume>(*child).is_some())
            .position(|child| child == volume)
            .unwrap_or(0)
    });
    format!("{node}/{}#{place}", found.substance)
}

/// A box's role, from its facets: crew, ammunition, the engine function, or a module.
fn role_of(world: &World, volume: Entity) -> Role {
    if world.get::<CrewStation>(volume).is_some() {
        Role::Crew
    } else if world.get::<Ammo>(volume).is_some() {
        Role::Ammo
    } else if world.get::<FunctionRole>(volume) == Some(&FunctionRole::Engine) {
        Role::Engine
    } else {
        Role::Module
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two overlapping tanks inside one crossing: the round comes to rest in B's plate, which
    /// starts 15 cm along, after perforating A's 10 cm plate, or, where A's plate is `deep`, inside
    /// both. Fixture geometry.
    fn shared_crossing_through(deep: bool) -> (World, Entity, Entity, Entity, Vec3, Vec3) {
        let mut world = World::new();
        let (a, b) = (world.spawn_empty().id(), world.spawn_empty().id());
        let plate_a = world.spawn(VolumeOf(a)).id();
        let plate_b = world.spawn(VolumeOf(b)).id();
        let entrance = Vec3::new(1.0, 1.5, 0.0);
        let incoming = Vec3::Z;
        let rest = entrance + incoming * 0.2;
        let crossing = ArmourMark::Crossing {
            entrance,
            normal: -Vec3::Z,
            incoming,
            speed_in: 800.0,
            overmatched: false,
            volumes: vec![
                VolumeCrossing {
                    volume: plate_a,
                    entry: entrance,
                    end: if deep {
                        rest
                    } else {
                        entrance + incoming * 0.1
                    },
                    chord: if deep { 0.2 } else { 0.1 },
                    cost: 100.0,
                    energy: 300_000.0,
                    perforated: !deep,
                },
                VolumeCrossing {
                    volume: plate_b,
                    entry: entrance + incoming * 0.15,
                    end: rest,
                    chord: 0.05,
                    cost: 50.0,
                    energy: 150_000.0,
                    perforated: false,
                },
            ],
            end: CrossingEnd::Rest { at: rest },
            cones: 0,
        };
        let shell = world
            .spawn(PenetrationMarks {
                log: vec![crossing],
                ..PenetrationMarks::default()
            })
            .id();
        (world, shell, a, b, entrance, rest)
    }

    fn shared_crossing() -> (World, Entity, Entity, Entity, Vec3, Vec3) {
        shared_crossing_through(false)
    }

    /// `tank`'s share of the fixture's one crossing.
    fn share_of(world: &World, shell: Entity, tank: Entity) -> Share {
        let marks = &world.get::<PenetrationMarks>(shell).unwrap().log;
        let ArmourMark::Crossing {
            entrance,
            incoming,
            volumes,
            end,
            ..
        } = &marks[0]
        else {
            unreachable!()
        };
        share(world, *entrance, *incoming, volumes, end, Some(tank)).unwrap()
    }

    /// Each tank reads only its own volumes of a shared crossing: A was perforated and the round
    /// left it; B was not perforated and holds the round; the whole crossing has both.
    #[test]
    fn a_shared_crossing_is_scoped_to_each_tank() {
        let (world, shell, a, b, entrance, _) = shared_crossing();
        let crossing = |perforated_armour, rested| PathStep::Crossing {
            perforated_armour,
            rested,
        };
        assert_eq!(
            path_steps(&world, shell, Some(a)),
            vec![crossing(true, false)]
        );
        assert_eq!(
            path_steps(&world, shell, Some(b)),
            vec![crossing(false, true)]
        );
        assert_eq!(path_steps(&world, shell, None), vec![crossing(true, true)]);

        let (share_a, share_b) = (share_of(&world, shell, a), share_of(&world, shell, b));
        assert!(share_a.entry.distance(entrance) < 1e-6);
        assert!(
            matches!(share_a.end, ShareEnd::Left { at } if at.distance(entrance + Vec3::Z * 0.1) < 1e-6),
            "A is left at its plate's far face"
        );
        assert!(share_b.entry.distance(entrance + Vec3::Z * 0.15) < 1e-6);
        assert!(matches!(share_b.end, ShareEnd::Rest { volume: Some(_) }));
    }

    /// A round that stops inside both tanks' overlapping plates rests in each tank's own plate:
    /// neither tank's record has it leave.
    #[test]
    fn a_rest_shared_by_tanks_is_each_tanks_own() {
        let (world, shell, a, b, ..) = shared_crossing_through(true);
        for tank in [a, b] {
            let ShareEnd::Rest {
                volume: Some(volume),
            } = share_of(&world, shell, tank).end
            else {
                panic!("{tank} holds the round");
            };
            assert!(
                owned_by(&world, volume, tank),
                "{tank} rests in its own plate"
            );
            assert_eq!(
                path_steps(&world, shell, Some(tank)),
                vec![PathStep::Crossing {
                    perforated_armour: false,
                    rested: true,
                }]
            );
        }
    }
}
