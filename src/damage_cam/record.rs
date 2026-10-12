//! The sim-side fill: a [`PenetrationReport`] read off a shell the authority marched.
//!
//! Two reads bracket the shot. [`StruckTank::capture`] takes the struck tank's components and their
//! HP before it; [`report`] reads the shell's armour log ([`crate::ballistics::ArmourMark`]), its
//! spall cones and the components' HP after it, and states it all in the struck hull's frame.
//! Nothing here decides anything the march did not: the verdict is [`ShotVerdict::of_path`] over
//! the logged path, plate classes come from the volumes' components, the round's end is the log's
//! last crossing or bounce, and every dose is one the march recorded. No component geometry is
//! read: the record names each component, and its shape stays the tank asset's.
//!
//! The path's times are DERIVED, not marched: each leg's length over the speed in effect on it
//! (the round's speed between crossings, the mean of its entry and exit speeds inside one, half
//! its entry speed inside the crossing it stops in).

use bevy::math::Affine3A;
use bevy::prelude::*;

use super::{
    BoxDose, ComponentState, Consequence, ConsequenceState, Fragment, FragmentHit, Impact,
    PathPoint, PathStep, PenetrationReport, Plate, PlateClass, Projectile, Ricochet, Role,
    RoundEnd, Shooter, ShotVerdict, SpallCone,
};
use crate::ballistics::{
    ArmourMark, BallisticVolume, ComponentHealth, CrossingEnd, ExternalArmour, PenetrationMarks,
    ShellPath, SpallMarks, TRANSIT_K, VolumeCrossing,
};
use crate::damage::{Ammo, CrewStation, FunctionRole, VolumeOf};
use crate::tank::Rig;

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

/// The struck tank before the shot: its hull frame and its components.
pub struct StruckTank {
    tank: Entity,
    /// World → hull-local.
    to_hull: Affine3A,
    /// World → hull-local, rotation only: for directions.
    to_hull_rotation: Quat,
    boxes: Vec<BoxAtShot>,
}

impl StruckTank {
    /// The tank's hull frame and every component it owns, by name. `None` for an entity with no
    /// rig.
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
    pub projectile: String,
    pub caliber_mm: f32,
    pub mass_kg: f32,
    /// The struck vehicle's tank file, relative to the asset root.
    pub vehicle: Option<String>,
    pub turret_yaw_deg: f32,
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
            ArmourMark::Bounce { struck, shock, .. } => (PathStep::Bounce, dosed(*struck, *shock)),
            ArmourMark::Crossing {
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
                let step = PathStep::Crossing {
                    perforated_armour: volumes.iter().any(|crossed| {
                        crossed.perforated && class_of(world, crossed.volume) == VolumeClass::Main
                    }),
                    rested: matches!(end, CrossingEnd::Rest { .. }),
                };
                (step, line || field)
            }
            ArmourMark::Failed => (PathStep::Failed, false),
        };
        if tank.is_none_or(|tank| touches(world, mark, tank)) {
            steps.push(step);
        }
        if dosed_here {
            steps.push(PathStep::Dosed);
        }
    }
    steps
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
    let world: &World = world;
    let cones = &world.get::<SpallMarks>(shell)?.cones;
    let tail = world.get::<ShellPath>(shell)?.points.clone();

    // The path's nodes, each with the speed of the leg that reaches it.
    let mut legs: Vec<(Vec3, f32)> = Vec::new();
    let mut speed = match marks.first()? {
        ArmourMark::Bounce { speed_in, .. } | ArmourMark::Crossing { speed_in, .. } => *speed_in,
        ArmourMark::Failed => return None,
    };
    legs.push((facts.launch, speed));
    let mut last = facts.launch;
    let mut flying = true;
    for mark in &marks {
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
    // First contact is t = 0.
    let contact = nodes.get(1)?.t;
    for node in &mut nodes {
        node.t -= contact;
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
    let first = marks.first()?;
    for mark in &marks {
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
                end: crossing_end,
                ..
            } => {
                end = Some(match *crossing_end {
                    CrossingEnd::Exit { at, .. } => RoundEnd::Left {
                        at: struck.point(at),
                    },
                    CrossingEnd::Rest { at } => RoundEnd::Rest {
                        volume: rested_in(volumes, at)
                            .map_or_else(String::new, |volume| name_of(world, volume)),
                    },
                });
                for crossed in volumes {
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
                            for &(volume, dose) in &ray.doses {
                                field.push((volume, dose, t));
                            }
                            Fragment {
                                end: struck.point(ray.end),
                                strength: ray.strength,
                                hits: ray
                                    .doses
                                    .iter()
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
        ArmourMark::Crossing {
            entrance,
            normal,
            incoming,
            ..
        } => (*entrance, *normal, *incoming),
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
        turret_yaw_deg: facts.turret_yaw_deg,
        shooter: facts.shooter.clone(),
        origin: struck.point(facts.launch),
        projectile: Projectile {
            name: facts.projectile.clone(),
            caliber_mm: facts.caliber_mm,
            mass_kg: facts.mass_kg,
            speed_mps: legs.get(1).map_or(0.0, |(_, speed)| *speed),
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

/// The volume a crossing that ended at rest at `at` came to rest in: the one whose line ends
/// nearest `at`, short of perforating it.
fn rested_in(volumes: &[VolumeCrossing], at: Vec3) -> Option<Entity> {
    volumes
        .iter()
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
