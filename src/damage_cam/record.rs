//! The sim-side fill: a [`PenetrationReport`] read off a shell the authority marched.
//!
//! Two reads bracket the shot. [`StruckTank::capture`] takes the struck tank's components and their
//! HP, and every servo's angle, before it; [`report`] reads the shell's armour log ([`crate::ballistics::ArmourMark`]), its
//! spall cones and the components' HP after it, and states it all in the struck hull's frame.
//! Nothing here decides anything the march did not: the verdict is [`ShotVerdict::of_path`] over
//! the logged path, plate classes come from the volumes' components, the round's end is the last
//! crossing or bounce the log has on the struck tank, and every dose is one the march recorded.
//! The outcome is the struck tank's alone: first contact, plates, bounces, cones and their
//! fragments' hits are read only off marks and volumes the tank owns, and the path stops one leg
//! past the struck tank's last mark. The record carries no geometry: it names each component, and
//! its shape stays the tank asset's. The one shape the recorder reads is the struck tank's entry
//! face where its first contact lies inside a crossing another tank's volume opened, from its own
//! colliders ([`entry_face_normal`]), since the march marks no face normal there.
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
use crate::ballistics::{
    ArmourMark, BallisticVolume, ComponentHealth, CrossingEnd, ExternalArmour, PenetrationMarks,
    ShellPath, SpallMarks, TRANSIT_K, VolumeCrossing, entry_face_normal,
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

/// One component of the struck tank, as it stood before the shot.
struct BoxAtShot {
    entity: Entity,
    state: ComponentState,
}

/// The struck tank before the shot: its hull frame, its servos' angles and its components.
pub struct StruckTank {
    tank: Entity,
    /// World → hull-local.
    to_hull: Affine3A,
    /// World → hull-local, rotation only: for directions.
    to_hull_rotation: Quat,
    /// Every servo's angle (degrees), by node name.
    servo_deg: BTreeMap<String, f32>,
    boxes: Vec<BoxAtShot>,
}

impl StruckTank {
    /// The tank's hull frame, every servo's angle and every component it owns, by name. `None` for
    /// an entity with no rig.
    pub fn capture(world: &mut World, tank: Entity) -> Option<Self> {
        let hull = world.get::<Rig>(tank)?.hull;
        let (_, hull_rotation, _) = world
            .get::<GlobalTransform>(hull)?
            .to_scale_rotation_translation();
        let to_hull = world.get::<GlobalTransform>(hull)?.affine().inverse();
        let to_hull_rotation = hull_rotation.inverse();
        let mut owned = world.query::<(Entity, &VolumeOf, &ComponentHealth)>();
        let mut boxes: Vec<(Entity, f32)> = owned
            .iter(world)
            .filter(|(_, owner, _)| owner.tank() == tank)
            .map(|(entity, _, hp)| (entity, hp.current))
            .collect();
        boxes.sort_by_key(|(entity, _)| name_of(world, *entity));
        let boxes = boxes
            .into_iter()
            .filter_map(|(entity, hp)| {
                let max = world.get::<ComponentHealth>(entity)?.max;
                Some(BoxAtShot {
                    entity,
                    state: ComponentState {
                        name: name_of(world, entity),
                        role: role_of(world, entity),
                        count: max,
                        hp,
                    },
                })
            })
            .collect();
        Some(Self {
            tank,
            to_hull,
            to_hull_rotation,
            servo_deg: servo_angles(world, tank),
            boxes,
        })
    }

    fn point(&self, world: Vec3) -> [f32; 3] {
        self.to_hull.transform_point3(world).to_array()
    }

    fn direction(&self, world: Vec3) -> [f32; 3] {
        (self.to_hull_rotation * world).to_array()
    }
}

/// The shot's facts the march does not carry: who fired, from where, what was fired, and what was
/// struck.
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
fn servo_angles(world: &mut World, tank: Entity) -> BTreeMap<String, f32> {
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
/// order, each followed by [`PathStep::Dosed`] when it dosed one of the tank's components. A
/// component's dose is the record's: a crossing's line through it, a bounce's shock off it, a
/// spall cone direction entering it. A cone is read with the crossing that threw it, whichever
/// tank that crossing touched.
pub fn path_steps(world: &World, shell: Entity, tank: Option<Entity>) -> Vec<PathStep> {
    let Some(marks) = world.get::<PenetrationMarks>(shell) else {
        return Vec::new();
    };
    let cones = world
        .get::<SpallMarks>(shell)
        .map_or(&[][..], |marks| &marks.cones[..]);
    let dosed = |volume: Entity, dose: f32| {
        dose > 0.0
            && class_of(world, volume) == VolumeClass::Box
            && tank.is_none_or(|tank| owned_by(world, volume, tank))
    };
    let mut thrown = cones.iter();
    let mut steps = Vec::new();
    for mark in &marks.log {
        let (step, dosed_here) = match mark {
            ArmourMark::Bounce { struck, shock, .. } => {
                (Some(PathStep::Bounce), dosed(*struck, *shock))
            }
            ArmourMark::Crossing {
                entrance,
                incoming,
                volumes,
                end,
                cones,
                ..
            } => {
                let field = thrown.by_ref().take(*cones).any(|cone| {
                    cone.rays
                        .iter()
                        .flat_map(|ray| &ray.doses)
                        .any(|&(volume, dose)| dosed(volume, dose))
                });
                let line = volumes
                    .iter()
                    .any(|crossed| dosed(crossed.volume, crossed.cost * TRANSIT_K));
                let step = share(world, *entrance, *incoming, volumes, end, tank).map(|share| {
                    PathStep::Crossing {
                        perforated_armour: share.perforated_armour,
                        rested: matches!(share.end, ShareEnd::Rest { .. }),
                    }
                });
                (step, line || field)
            }
            ArmourMark::Failed => (Some(PathStep::Failed), false),
        };
        // A crossing has a step for the tank only where one of its volumes is the tank's.
        if let Some(step) = step
            && tank.is_none_or(|tank| touches(world, mark, tank))
        {
            steps.push(step);
        }
        if dosed_here {
            steps.push(PathStep::Dosed);
        }
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
    /// The tank's first volume along the line.
    first: VolumeCrossing,
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
    let first = **mine
        .iter()
        .min_by(|a, b| along(a.entry).total_cmp(&along(b.entry)))?;
    let entry = if whole { entrance } else { first.entry };
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
        first,
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

/// One timed node of the path, WORLD.
struct Node {
    point: Vec3,
    t: f32,
}

/// The report of `shell`'s flight against `struck`, or `None` when it opens no pen cam
/// ([`ShotVerdict::of_path`]).
pub fn report(
    world: &mut World,
    struck: &StruckTank,
    shell: Entity,
    facts: &ShotFacts,
) -> Option<PenetrationReport> {
    let steps = path_steps(world, shell, Some(struck.tank));
    let verdict = ShotVerdict::of_path(&steps)?;
    let marks: Vec<ArmourMark> = world.get::<PenetrationMarks>(shell)?.log.clone();
    // Where the struck tank's first contact is a volume entered inside a crossing another tank's
    // volume opened, its face is read off its own colliders: the march marks no face normal there.
    let entered_inside = marks
        .iter()
        .find(|mark| touches(world, mark, struck.tank))
        .and_then(|mark| match mark {
            ArmourMark::Crossing {
                entrance,
                incoming,
                volumes,
                end,
                ..
            } => share(world, *entrance, *incoming, volumes, end, Some(struck.tank))
                .filter(|share| share.entry.distance(*entrance) > 1.0e-6)
                .map(|share| (share.first, *incoming)),
            _ => None,
        });
    let own_face = entered_inside.map(|(first, incoming)| {
        let normal = entry_face_normal(
            world,
            first.volume,
            first.entry,
            first.end,
            incoming,
            facts.caliber_mm / 1000.0,
        );
        if normal.is_none() {
            warn!(
                "pen cam record: no entering face of {} at {}; its impact keeps the crossing's normal",
                first.volume, first.entry
            );
        }
        normal
    });
    let world: &World = world;
    let cones = &world.get::<SpallMarks>(shell)?.cones;
    let tail = world.get::<ShellPath>(shell)?.points.clone();
    let owns = |mark: &ArmourMark| touches(world, mark, struck.tank);
    let first_touch = marks.iter().position(owns)?;
    let last_touch = marks.iter().rposition(owns)?;

    // The path's nodes up to the struck tank's last mark, each with the speed of the leg that
    // reaches it; `contact` is the node of the struck tank's first contact.
    let mut legs: Vec<(Vec3, f32)> = Vec::new();
    let mut speed = match marks.first()? {
        ArmourMark::Bounce { speed_in, .. } | ArmourMark::Crossing { speed_in, .. } => *speed_in,
        ArmourMark::Failed => return None,
    };
    legs.push((facts.launch, speed));
    let mut last = facts.launch;
    let mut flying = true;
    let mut contact = 0;
    let mine = |mark: &ArmourMark| match mark {
        ArmourMark::Crossing {
            entrance,
            incoming,
            volumes,
            end,
            ..
        } => share(world, *entrance, *incoming, volumes, end, Some(struck.tank)),
        _ => None,
    };
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
                legs.push((*position, speed));
                speed = *speed_out;
                last = *position;
            }
            ArmourMark::Crossing {
                entrance,
                speed_in,
                end,
                ..
            } => {
                legs.push((*entrance, *speed_in));
                // The struck tank's first contact is its own first volume, which an overlapping
                // tank's volume may precede inside the crossing.
                if index == first_touch
                    && let Some(share) = mine(mark)
                    && share.entry.distance(*entrance) > 1.0e-6
                {
                    contact = legs.len();
                    legs.push((share.entry, *speed_in));
                }
                match *end {
                    CrossingEnd::Rest { at } => {
                        legs.push((at, 0.5 * speed_in));
                        last = at;
                        flying = false;
                    }
                    CrossingEnd::Exit { at, speed: out, .. } => {
                        legs.push((at, 0.5 * (speed_in + out)));
                        speed = out;
                        last = at;
                    }
                }
            }
            ArmourMark::Failed => return None,
        }
    }
    if flying
        && let Some(index) = tail.iter().rposition(|point| *point == last)
        && let Some(next) = tail.get(index + 1)
    {
        legs.push((*next, speed));
    }
    let mut nodes: Vec<Node> = Vec::with_capacity(legs.len());
    let mut t = 0.0;
    for (index, &(point, speed)) in legs.iter().enumerate() {
        if index > 0 {
            let length = point.distance(legs[index - 1].0);
            if speed > 0.0 {
                t += length / speed;
            }
        }
        nodes.push(Node { point, t });
    }
    // The struck tank's first contact is t = 0.
    let contact_t = nodes.get(contact)?.t;
    for node in &mut nodes {
        node.t -= contact_t;
    }
    let time_at = |point: Vec3| time_on(&nodes, point);

    let mut plates = Vec::new();
    let mut ricochets = Vec::new();
    let mut spall = Vec::new();
    let mut line: Vec<(Entity, f32, f32)> = Vec::new();
    let mut field: Vec<(Entity, f32, f32)> = Vec::new();
    // Where the line reached each component, with its time.
    let mut entered: Vec<(Entity, f32, Vec3)> = Vec::new();
    let mut end = None;
    let mut thrown = cones.iter();
    let first = &marks[first_touch];
    let owned = |volume: Entity| owned_by(world, volume, struck.tank);
    for mark in &marks[..=last_touch] {
        if !owns(mark) {
            // Its cones are another tank's; the cone log stays aligned with the crossings.
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
                let t = time_at(*position);
                ricochets.push(Ricochet {
                    t,
                    point: struck.point(*position),
                    normal: struck.direction(*normal),
                    incoming: struck.direction(*incoming),
                    outgoing: struck.direction(*outgoing),
                    volume: name_of(world, *volume),
                });
                if class_of(world, *volume) == VolumeClass::Box {
                    line.push((*volume, *shock, t));
                    entered.push((*volume, t, *position));
                }
                end = Some(RoundEnd::Left {
                    at: struck.point(*position),
                });
            }
            ArmourMark::Crossing {
                volumes,
                overmatched,
                cones: count,
                ..
            } => {
                end = mine(mark).map(|share| match share.end {
                    ShareEnd::Left { at } => RoundEnd::Left {
                        at: struck.point(at),
                    },
                    ShareEnd::Rest { volume } => RoundEnd::Rest {
                        volume: volume.map_or_else(String::new, |volume| name_of(world, volume)),
                    },
                });
                for crossed in volumes.iter().filter(|crossed| owned(crossed.volume)) {
                    let t = time_at(crossed.entry);
                    let class = class_of(world, crossed.volume);
                    if class == VolumeClass::Box {
                        line.push((crossed.volume, crossed.cost * TRANSIT_K, t));
                        entered.push((crossed.volume, t, crossed.entry));
                        continue;
                    }
                    plates.push(Plate {
                        name: name_of(world, crossed.volume),
                        t,
                        entry: struck.point(crossed.entry),
                        exit: crossed.perforated.then(|| struck.point(crossed.end)),
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
                                end: struck.point(ray.end),
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
                        origin: struck.point(cone.origin),
                        axis: struck.direction(Vec3::from(cone.axis)),
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
        // The tank's own entry face: the crossing's entrance where the crossing opened on it.
        ArmourMark::Crossing {
            entrance,
            normal,
            incoming,
            ..
        } => (
            mine(first).map_or(*entrance, |share| share.entry),
            own_face.flatten().unwrap_or(*normal),
            *incoming,
        ),
        ArmourMark::Failed => return None,
    };
    let normal = normal.normalize_or_zero();
    let incidence = (-incoming).dot(normal).clamp(-1.0, 1.0).acos();

    let (doses, mut consequences) = outcomes(world, struck, &line, &field, &entered);
    consequences.sort_by(|a, b| a.t.total_cmp(&b.t));
    // A crossing lists its volumes by identity; the record lists them in path order.
    plates.sort_by(|a, b| a.t.total_cmp(&b.t));

    Some(PenetrationReport {
        synthetic: false,
        vehicle: facts.vehicle.clone(),
        servo_deg: struck.servo_deg.clone(),
        round: facts.round.clone(),
        shooter: facts.shooter.clone(),
        origin: struck.point(facts.launch),
        muzzle: facts.muzzle.map(|muzzle| struck.point(muzzle)),
        projectile: Projectile {
            name: facts.projectile.clone(),
            caliber_mm: facts.caliber_mm,
            mass_kg: facts.mass_kg,
            speed_mps: legs.get(contact).map_or(0.0, |(_, speed)| *speed),
        },
        impact: Impact {
            point: struck.point(impact_point),
            normal: struck.direction(normal),
            incoming: struck.direction(incoming),
            incidence_deg: incidence.to_degrees(),
            verdict,
        },
        plates,
        ricochets,
        path: nodes
            .iter()
            .map(|node| PathPoint {
                t: node.t,
                point: struck.point(node.point),
            })
            .collect(),
        spall,
        end: end?,
        doses,
        consequences,
        components: struck
            .boxes
            .iter()
            .map(|at_shot| at_shot.state.clone())
            .collect(),
    })
}

/// Each component's dose from the shot and, for one the shot disabled, its consequence, timed at the
/// deposit that emptied it. `line` and `field` list `(volume, dose, t)` deposits, `entered` the
/// `(volume, t, point)` where the line reached a component (WORLD).
fn outcomes(
    world: &World,
    struck: &StruckTank,
    line: &[(Entity, f32, f32)],
    field: &[(Entity, f32, f32)],
    entered: &[(Entity, f32, Vec3)],
) -> (Vec<BoxDose>, Vec<Consequence>) {
    let mut doses = Vec::new();
    let mut consequences = Vec::new();
    for at_shot in &struck.boxes {
        let of = |list: &[(Entity, f32, f32)]| -> Vec<(f32, f32)> {
            list.iter()
                .filter(|(volume, ..)| *volume == at_shot.entity)
                .map(|&(_, dose, t)| (t, dose))
                .collect()
        };
        let (from_line, from_field) = (of(line), of(field));
        if from_line.is_empty() && from_field.is_empty() {
            continue;
        }
        let first_entry = entered
            .iter()
            .filter(|(volume, ..)| *volume == at_shot.entity)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|&(_, _, point)| struck.point(point));
        doses.push(BoxDose {
            volume: at_shot.state.name.clone(),
            line: from_line.iter().fold(0.0, |sum, (_, dose)| sum + dose),
            field: from_field.iter().fold(0.0, |sum, (_, dose)| sum + dose),
            entered: first_entry,
        });
        let hp = at_shot.state.hp;
        let after = world
            .get::<ComponentHealth>(at_shot.entity)
            .map_or(hp, |hp| hp.current);
        if hp > 0.0 && after <= 0.0 {
            let mut landed: Vec<(f32, f32)> = from_line.into_iter().chain(from_field).collect();
            landed.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut total = 0.0;
            let t = landed
                .iter()
                .find(|(_, dose)| {
                    total += dose;
                    total >= hp
                })
                .or(landed.last())
                .map_or(0.0, |(t, _)| *t);
            consequences.push(Consequence {
                t,
                volume: at_shot.state.name.clone(),
                state: if world.get::<Ammo>(at_shot.entity).is_some() {
                    ConsequenceState::Cookoff
                } else {
                    ConsequenceState::Lost
                },
                function: world
                    .get::<FunctionRole>(at_shot.entity)
                    .map(|function| format!("{function:?}")),
            });
        }
    }
    (doses, consequences)
}

/// The volume among `volumes` that a crossing ended at rest at `at` came to rest in: the one whose
/// line ends nearest `at`, short of perforating it. Every volume the round stopped inside ends at
/// `at`, so overlapping tanks' volumes each hold it and each tank's own volumes name its own.
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
