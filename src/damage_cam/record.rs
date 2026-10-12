//! The sim-side fill: a [`PenetrationReport`] read off a shell the authority marched.
//!
//! Two reads bracket the shot. [`StruckTank::capture`] takes the struck tank's boxes and their HP
//! before it; [`report`] reads the shell's armour log ([`crate::ballistics::ArmourMark`]), its
//! spall cones and the boxes' HP after it, and states it all in the struck hull's frame. Nothing
//! here decides anything the march did not: the verdict is [`ShotVerdict::of_path`] over the
//! logged path, plate classes come from the volumes' components, and every dose is one the march
//! recorded.
//!
//! The path's times are DERIVED, not marched: each leg's length over the speed in effect on it
//! (the round's speed between crossings, the mean of its entry and exit speeds inside one, half
//! its entry speed inside the crossing it stops in).

use bevy::math::Affine3A;
use bevy::prelude::*;

use super::{
    BoxDose, Consequence, ConsequenceState, Fragment, FragmentHit, Impact, Internal, PathPoint,
    PathStep, PenetrationReport, Plate, PlateClass, Projectile, Ricochet, Role, ShotVerdict,
    SpallCone,
};
use crate::ballistics::{
    ArmourMark, BallisticVolume, ComponentHealth, CrossingEnd, ExternalArmour, PenetrationMarks,
    ShellPath, SpallMarks, TRANSIT_K,
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

/// One box of the struck tank, as it stood before the shot.
struct BoxAtShot {
    entity: Entity,
    internal: Internal,
    hp: f32,
}

/// The struck tank before the shot: its hull frame and its boxes.
pub struct StruckTank {
    tank: Entity,
    /// World → hull-local.
    to_hull: Affine3A,
    /// World → hull-local, rotation only: for directions.
    to_hull_rotation: Quat,
    boxes: Vec<BoxAtShot>,
}

impl StruckTank {
    /// The tank's hull frame and every box it owns, by name. `None` for an entity with no rig.
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
                let (center, half_extents, rotation) =
                    bounds(world, entity, &to_hull, to_hull_rotation)?;
                let max = world.get::<ComponentHealth>(entity)?.max;
                Some(BoxAtShot {
                    entity,
                    internal: Internal {
                        name: name_of(world, entity),
                        role: role_of(world, entity),
                        center: center.to_array(),
                        half_extents: half_extents.to_array(),
                        rotation: rotation.to_array(),
                        count: max,
                    },
                    hp,
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

/// The shot's facts the march does not carry: where it was fired from, what was fired, and what
/// was struck.
pub struct ShotFacts {
    /// Where the round started (WORLD).
    pub launch: Vec3,
    pub projectile: String,
    pub caliber_mm: f32,
    pub mass_kg: f32,
    /// The struck vehicle's tank file, relative to the asset root.
    pub vehicle: Option<String>,
    pub turret_yaw_deg: f32,
}

/// The armour path `shell` took through `tank` (every volume, for `None`), as the verdict reads
/// it: its bounces off and crossings of the tank's volumes, and every walk that failed closed, in
/// march order.
pub fn path_steps(world: &World, shell: Entity, tank: Option<Entity>) -> Vec<PathStep> {
    let Some(marks) = world.get::<PenetrationMarks>(shell) else {
        return Vec::new();
    };
    marks
        .log
        .iter()
        .filter(|mark| tank.is_none_or(|tank| touches(world, mark, tank)))
        .map(|mark| match mark {
            ArmourMark::Bounce { .. } => PathStep::Bounce,
            ArmourMark::Crossing { volumes, end, .. } => PathStep::Crossing {
                perforated_main: volumes.iter().any(|crossed| {
                    crossed.perforated && class_of(world, crossed.volume) == VolumeClass::Main
                }),
                rested: matches!(end, CrossingEnd::Rest { .. }),
            },
            ArmourMark::Failed => PathStep::Failed,
        })
        .collect()
}

/// Whether a mark belongs to `tank`. A failed walk names no volume and belongs to every tank.
fn touches(world: &World, mark: &ArmourMark, tank: Entity) -> bool {
    let owned = |volume: Entity| {
        world
            .get::<VolumeOf>(volume)
            .is_some_and(|owner| owner.tank() == tank)
    };
    match mark {
        ArmourMark::Bounce { struck, .. } => owned(*struck),
        ArmourMark::Crossing { volumes, .. } => volumes.iter().any(|crossed| owned(crossed.volume)),
        ArmourMark::Failed => true,
    }
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
                }
            }
            ArmourMark::Crossing {
                volumes,
                overmatched,
                cones: count,
                ..
            } => {
                for crossed in volumes {
                    let t = time_at(crossed.entry);
                    let class = class_of(world, crossed.volume);
                    if class == VolumeClass::Box {
                        line.push((crossed.volume, crossed.cost * TRANSIT_K, t));
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

    let mut doses = Vec::new();
    let mut consequences = Vec::new();
    for at_shot in &struck.boxes {
        let of = |list: &[(Entity, f32, f32)]| -> Vec<(f32, f32)> {
            list.iter()
                .filter(|(volume, ..)| *volume == at_shot.entity)
                .map(|&(_, dose, t)| (t, dose))
                .collect()
        };
        let (from_line, from_field) = (of(&line), of(&field));
        if from_line.is_empty() && from_field.is_empty() {
            continue;
        }
        doses.push(BoxDose {
            volume: at_shot.internal.name.clone(),
            line: from_line.iter().fold(0.0, |sum, (_, dose)| sum + dose),
            field: from_field.iter().fold(0.0, |sum, (_, dose)| sum + dose),
        });
        let after = world
            .get::<ComponentHealth>(at_shot.entity)
            .map_or(at_shot.hp, |hp| hp.current);
        if at_shot.hp > 0.0 && after <= 0.0 {
            let mut landed: Vec<(f32, f32)> = from_line.into_iter().chain(from_field).collect();
            landed.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut total = 0.0;
            let t = landed
                .iter()
                .find(|(_, dose)| {
                    total += dose;
                    total >= at_shot.hp
                })
                .or(landed.last())
                .map_or(0.0, |(t, _)| *t);
            consequences.push(Consequence {
                t,
                volume: at_shot.internal.name.clone(),
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
    consequences.sort_by(|a, b| a.t.total_cmp(&b.t));
    // A crossing lists its volumes by identity; the record lists them in path order.
    plates.sort_by(|a, b| a.t.total_cmp(&b.t));

    Some(PenetrationReport {
        synthetic: false,
        vehicle: facts.vehicle.clone(),
        turret_yaw_deg: facts.turret_yaw_deg,
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
        doses,
        consequences,
        internals: struck
            .boxes
            .iter()
            .map(|at_shot| at_shot.internal.clone())
            .collect(),
    })
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

/// A volume's bounds in its own frame — the box around its colliders, its own and its children's —
/// as centre, half extents and orientation in the hull frame. A child collider sits at identity
/// under its volume at unit scale (`tank::spawn`), so its shape is volume-local; a trimesh is
/// bounded by its vertices, any other shape by its own box.
fn bounds(
    world: &World,
    volume: Entity,
    to_hull: &Affine3A,
    to_hull_rotation: Quat,
) -> Option<(Vec3, Vec3, Quat)> {
    use avian3d::parry::shape::TypedShape;
    use avian3d::prelude::{Collider, SimpleCollider};
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    let children = world
        .get::<Children>(volume)
        .map(|children| children.iter());
    for entity in std::iter::once(volume).chain(children.into_iter().flatten()) {
        let Some(collider) = world.get::<Collider>(entity) else {
            continue;
        };
        if let TypedShape::TriMesh(mesh) = collider.shape_scaled().as_typed_shape() {
            for vertex in mesh.vertices() {
                let vertex = Vec3::new(vertex.x, vertex.y, vertex.z);
                lo = lo.min(vertex);
                hi = hi.max(vertex);
            }
        } else {
            let aabb = collider.aabb(Vec3::ZERO, Quat::IDENTITY);
            lo = lo.min(aabb.min);
            hi = hi.max(aabb.max);
        }
    }
    if !lo.is_finite() || !hi.is_finite() {
        return None;
    }
    let node = world.get::<GlobalTransform>(volume)?;
    let (_, rotation, _) = node.to_scale_rotation_translation();
    let center = to_hull.transform_point3(node.transform_point(0.5 * (lo + hi)));
    Some((center, 0.5 * (hi - lo), to_hull_rotation * rotation))
}
