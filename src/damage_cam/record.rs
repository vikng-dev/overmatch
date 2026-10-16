//! The sim-side fill: a [`PenetrationReport`] read off a shell the authority marched.
//!
//! Two reads bracket the shot. [`StruckTank::capture`] takes the struck tank's hull frame, its
//! components and their HP, and every servo's angle, before it; [`report`] reads the shell's armour
//! log ([`crate::ballistics::ArmourMark`]), its spall cones and its own per-box tally
//! ([`ShotDose`]) after it, and states it all in the struck hull's frame. In the game the recorder
//! (`super::recorder`) fills the same [`StruckTank`] from the pose each mark was resolved against
//! and reads the logs through `report_from`.
//!
//! A component's HP at the shot is what the shot found in its pool when it first reached it
//! ([`ShotDose::found`]), and one it never reached reads the HP the struck tank's capture holds.
//! What it lost and whether the shot emptied it are the shot's own tally, never a difference of
//! pool HP, so a shot overlapping another in one tick reads only its own loss.
//!
//! Nothing here decides anything the march did not: the verdict is [`ShotVerdict::of_path`] over
//! the logged path, plate classes come from the volumes' components, the round's end is the last
//! crossing or bounce the log has on the struck tank, and every dose is one the march recorded.
//! The outcome is the struck tank's alone: first contact, plates, bounces, cones and their
//! fragments' hits are read only off marks and volumes the tank owns, and the path stops one leg
//! past the struck tank's last mark. No component geometry is
//! read: the record names each component, and its shape stays the tank asset's.
//!
//! The path's times are DERIVED, not marched: each leg's length over the speed in effect on it
//! (the round's speed between crossings, the mean of its entry and exit speeds inside one, half
//! its entry speed inside the crossing it stops in).

use std::collections::BTreeMap;

use bevy::math::Affine3A;
use bevy::prelude::*;

use super::{
    BoxDose, ComponentState, Consequence, ConsequenceState, Fragment, FragmentHit, Impact,
    PathPoint, PathStep, PenetrationReport, Plate, PlateClass, Projectile, Ricochet, Role,
    RoundEnd, Shooter, ShotVerdict, SpallCone,
};
use crate::ballistics::SpallCone as ThrownCone;
use crate::ballistics::{
    ArmourMark, BallisticVolume, ComponentHealth, CrossingEnd, ExternalArmour, PenetrationMarks,
    ShellPath, ShotDose, SpallMarks, TRANSIT_K, VolumeCrossing, dose_units, hp_of_units,
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

/// A hull frame: world → hull-local.
#[derive(Clone, Copy)]
pub(crate) struct HullFrame {
    to_hull: Affine3A,
    /// Rotation only: for directions.
    to_hull_rotation: Quat,
}

impl HullFrame {
    /// The frame of a hull node posed at `position`, `rotation` (WORLD).
    pub(crate) fn posed(position: Vec3, rotation: Quat) -> Self {
        Self {
            to_hull: Affine3A::from_rotation_translation(rotation, position).inverse(),
            to_hull_rotation: rotation.inverse(),
        }
    }

    fn of(global: &GlobalTransform) -> Self {
        let (_, rotation, _) = global.to_scale_rotation_translation();
        Self {
            to_hull: global.affine().inverse(),
            to_hull_rotation: rotation.inverse(),
        }
    }
}

/// The struck tank at the shot: the hull frame each of the shell's marks was resolved in, its
/// servos' angles and its components' HP.
pub struct StruckTank {
    tank: Entity,
    /// `(first mark, frame)`, ascending: a mark is stated in the last frame that starts at or
    /// before it, a point read before the first mark in the first frame.
    frames: Vec<(usize, HullFrame)>,
    /// Every servo's angle (degrees), by node name.
    servo_deg: BTreeMap<String, f32>,
    /// Every component the tank owns and its HP.
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
            .map(|(entity, _, hp)| (entity, hp.current))
            .collect();
        Some(Self {
            tank,
            frames: vec![(0, frame)],
            servo_deg: servo_angles(world, tank),
            boxes,
        })
    }

    /// The tank as a recorder saw it: `frames` by first mark, ascending, never empty.
    pub(crate) fn recorded(
        tank: Entity,
        frames: Vec<(usize, HullFrame)>,
        servo_deg: BTreeMap<String, f32>,
        boxes: Vec<(Entity, u32)>,
    ) -> Self {
        Self {
            tank,
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

/// One shell's logs as the march left them: its armour log, the cones it threw, its path and its
/// per-box tally (`None` where the shell carries none).
pub(crate) struct ShotLog<'a> {
    pub marks: &'a [ArmourMark],
    pub cones: &'a [ThrownCone],
    pub path: &'a [Vec3],
    pub dose: Option<&'a ShotDose>,
}

impl<'a> ShotLog<'a> {
    /// `shell`'s logs, read off its components.
    pub(crate) fn read(world: &'a World, shell: Entity) -> Option<Self> {
        Some(Self {
            marks: &world.get::<PenetrationMarks>(shell)?.log,
            cones: &world.get::<SpallMarks>(shell)?.cones,
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
/// order, then one [`PathStep::Dosed`] when one of the tank's components lost HP to the shot
/// ([`ShotDose::losses`], the shot's per-box tally rounded once).
pub fn path_steps(world: &World, shell: Entity, tank: Option<Entity>) -> Vec<PathStep> {
    let Some(marks) = world.get::<PenetrationMarks>(shell) else {
        return Vec::new();
    };
    steps_of(world, &marks.log, world.get::<ShotDose>(shell), tank)
}

/// [`path_steps`] over a shell's logs.
fn steps_of(
    world: &World,
    marks: &[ArmourMark],
    dose: Option<&ShotDose>,
    tank: Option<Entity>,
) -> Vec<PathStep> {
    let mut steps = Vec::new();
    for mark in marks {
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
        };
        // A crossing has a step for the tank only where one of its volumes is the tank's.
        if let Some(step) = step
            && tank.is_none_or(|tank| touches(world, mark, tank))
        {
            steps.push(step);
        }
    }
    let dosed = dose.is_some_and(|dose| {
        dose.losses().any(|(volume, lost)| {
            lost > 0
                && class_of(world, volume) == VolumeClass::Box
                && tank.is_none_or(|tank| owned_by(world, volume, tank))
        })
    });
    if dosed {
        steps.push(PathStep::Dosed);
    }
    steps
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
        ArmourMark::Crossing { volumes, .. } => volumes.iter().any(|crossed| owned(crossed.volume)),
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

/// The path's timed nodes up to the struck tank's last mark `last_touch`, t = 0 at its first
/// contact, and the round's speed at that contact.
fn timeline(
    world: &World,
    struck: &StruckTank,
    log: &ShotLog,
    facts: &ShotFacts,
    first_touch: usize,
    last_touch: usize,
) -> Option<(Vec<Node>, f32)> {
    let marks = log.marks;
    // Each node with the speed of the leg that reaches it and the mark whose frame states it;
    // `contact` is the node of the struck tank's first contact.
    let mut legs: Vec<(Vec3, f32, usize)> = Vec::new();
    let mut speed = match marks.first()? {
        ArmourMark::Bounce { speed_in, .. } | ArmourMark::Crossing { speed_in, .. } => *speed_in,
        ArmourMark::Failed => return None,
    };
    legs.push((facts.launch, speed, first_touch));
    let mut last = facts.launch;
    let mut flying = true;
    let mut contact = 0;
    for (index, mark) in marks[..=last_touch].iter().enumerate() {
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
        }
    }
    if flying
        && let Some(index) = log.path.iter().rposition(|point| *point == last)
        && let Some(next) = log.path.get(index + 1)
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
    Some((nodes, legs[contact].1))
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
    let steps = steps_of(world, log.marks, log.dose, Some(struck.tank));
    let verdict = ShotVerdict::of_path(&steps)?;
    let marks = log.marks;
    let owns = |mark: &ArmourMark| touches(world, mark, struck.tank);
    let first_touch = marks.iter().position(owns)?;
    let last_touch = marks.iter().rposition(owns)?;

    let (nodes, contact_speed) = timeline(world, struck, log, facts, first_touch, last_touch)?;
    let mine = |mark: &ArmourMark| share_of(world, mark, struck.tank);
    let time_at = |point: Vec3| time_on(&nodes, point);

    let mut plates = Vec::new();
    let mut ricochets = Vec::new();
    let mut spall = Vec::new();
    let mut line: Vec<(Entity, f32, f32)> = Vec::new();
    let mut field: Vec<(Entity, f32, f32)> = Vec::new();
    // Where the line reached each component, with its time and the mark that reached it.
    let mut entered: Vec<(Entity, f32, Vec3, usize)> = Vec::new();
    let mut end = None;
    let mut thrown = log.cones.iter();
    let first = &marks[first_touch];
    let owned = |volume: Entity| owned_by(world, volume, struck.tank);
    for (index, mark) in marks[..=last_touch].iter().enumerate() {
        if !owns(mark) {
            // Its cones are another tank's; the cone log stays aligned with the crossings.
            if let ArmourMark::Crossing { cones: count, .. } = mark {
                thrown.by_ref().take(*count).for_each(drop);
            }
            continue;
        }
        let point = |world: Vec3| struck.point(index, world);
        let direction = |world: Vec3| struck.direction(index, world);
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
                let t = time_at(*position);
                ricochets.push(Ricochet {
                    t,
                    point: point(*position),
                    normal: direction(*normal),
                    incoming: direction(*incoming),
                    outgoing: direction(*outgoing),
                    volume: name_of(world, *volume),
                });
                if class_of(world, *volume) == VolumeClass::Box {
                    line.push((*volume, *shock, t));
                    entered.push((*volume, t, *position, index));
                }
                end = Some(RoundEnd::Left {
                    at: point(*position),
                });
            }
            ArmourMark::Crossing {
                volumes,
                overmatched,
                cones: count,
                ..
            } => {
                end = mine(mark).map(|share| match share.end {
                    ShareEnd::Left { at } => RoundEnd::Left { at: point(at) },
                    ShareEnd::Rest { volume } => RoundEnd::Rest {
                        volume: volume.map_or_else(String::new, |volume| name_of(world, volume)),
                    },
                });
                for crossed in volumes.iter().filter(|crossed| owned(crossed.volume)) {
                    let t = time_at(crossed.entry);
                    let class = class_of(world, crossed.volume);
                    if class == VolumeClass::Box {
                        line.push((crossed.volume, crossed.cost * TRANSIT_K, t));
                        entered.push((crossed.volume, t, crossed.entry, index));
                        continue;
                    }
                    plates.push(Plate {
                        name: name_of(world, crossed.volume),
                        t,
                        entry: point(crossed.entry),
                        exit: crossed.perforated.then(|| point(crossed.end)),
                        chord_mm: crossed.chord * 1000.0,
                        steel_equiv_mm: crossed.cost,
                        cost_mm: crossed.cost,
                        overmatched: *overmatched,
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
                for cone in thrown.by_ref().take(*count) {
                    let t = time_at(cone.origin);
                    let fragments = cone
                        .rays
                        .iter()
                        .map(|ray| {
                            let doses = ray.doses.iter().filter(|(volume, _)| owned(*volume));
                            for &(volume, dose) in doses.clone() {
                                field.push((volume, dose, t));
                            }
                            Fragment {
                                end: point(ray.end),
                                strength: ray.strength,
                                hits: doses
                                    .map(|&(volume, dose)| FragmentHit {
                                        volume: name_of(world, volume),
                                        damage: dose,
                                    })
                                    .collect(),
                            }
                        })
                        .collect();
                    spall.push(SpallCone {
                        t,
                        origin: point(cone.origin),
                        axis: direction(Vec3::from(cone.axis)),
                        half_angle_deg: cone.half_angle.to_degrees(),
                        strength: cone.strength,
                        fragments,
                    });
                }
            }
            ArmourMark::Failed => {}
        }
    }

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
            mine(first).map_or(*entrance, |share| share.entry),
            *normal,
            *incoming,
        ),
        ArmourMark::Failed => return None,
    };
    let normal = normal.normalize_or_zero();
    let incidence = (-incoming).dot(normal).clamp(-1.0, 1.0).acos();

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
        origin: struck.point(first_touch, facts.launch),
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
        end: end?,
        doses,
        consequences,
        components: components.into_iter().map(|(_, state)| state).collect(),
        killed: None,
    })
}

/// The struck tank's components by name, each at its HP at the shot: what the shot found in its
/// pool where it reached it ([`ShotDose::found`]), the tank's captured HP elsewhere.
fn components(
    world: &World,
    struck: &StruckTank,
    dose: Option<&ShotDose>,
) -> Vec<(Entity, ComponentState)> {
    let mut components: Vec<(Entity, ComponentState)> = struck
        .boxes
        .iter()
        .filter_map(|&(entity, captured)| {
            let max = world.get::<ComponentHealth>(entity)?.max;
            Some((
                entity,
                ComponentState {
                    name: name_of(world, entity),
                    role: role_of(world, entity),
                    count: max,
                    hp: dose.and_then(|dose| dose.found(entity)).unwrap_or(captured),
                },
            ))
        })
        .collect();
    components.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    components
}

/// Each component's dose from the shot and, for one the shot emptied, its consequence, timed at the
/// deposit that emptied it. `line` and `field` list `(volume, dose, t)` deposits, `entered` the
/// `(volume, t, point, mark)` where the line reached a component (WORLD). The HP lost and the
/// emptying are the shot's own tally's ([`ShotDose`]).
fn outcomes(
    world: &World,
    struck: &StruckTank,
    components: &[(Entity, ComponentState)],
    dose: Option<&ShotDose>,
    line: &[(Entity, f32, f32)],
    field: &[(Entity, f32, f32)],
    entered: &[(Entity, f32, Vec3, usize)],
) -> (Vec<BoxDose>, Vec<Consequence>) {
    let mut doses = Vec::new();
    let mut consequences = Vec::new();
    for (entity, state) in components {
        let of = |list: &[(Entity, f32, f32)]| -> Vec<(f32, f32)> {
            list.iter()
                .filter(|(volume, ..)| volume == entity)
                .map(|&(_, dose, t)| (t, dose))
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
        let lost = dose.map_or(0, |dose| {
            dose.losses()
                .find(|(volume, _)| volume == entity)
                .map_or(0, |(_, lost)| lost)
        });
        doses.push(BoxDose {
            volume: state.name.clone(),
            line: from_line.iter().fold(0.0, |sum, (_, dose)| sum + dose),
            field: from_field.iter().fold(0.0, |sum, (_, dose)| sum + dose),
            lost,
            entered: first_entry,
        });
        if dose.is_some_and(|dose| dose.emptied().contains(entity)) {
            let hp = state.hp;
            let mut landed: Vec<(f32, f32)> = from_line.into_iter().chain(from_field).collect();
            landed.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut total = 0;
            let t = landed
                .iter()
                .find(|(_, dose)| {
                    total += dose_units(*dose);
                    hp_of_units(total) >= hp
                })
                .or(landed.last())
                .map_or(0.0, |(t, _)| *t);
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
                    perforated: !deep,
                },
                VolumeCrossing {
                    volume: plate_b,
                    entry: entrance + incoming * 0.15,
                    end: rest,
                    chord: 0.05,
                    cost: 50.0,
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
