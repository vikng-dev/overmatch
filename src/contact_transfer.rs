//! What solver contacts and implicit belt contacts moved between a standing structure and a
//! dynamic body, one physics step at a time: the reading a body's contact load is taken from in
//! the game's environment destruction model (game state only; a contact never doses a tank). The model it
//! serves is `.agents/docs/design/wall-destruction.md`, "Bodies in contact".
//!
//! THE READING. Avian's contact solver runs sequential impulses over substeps. Each substep warm
//! starts every contact point with its accumulated impulse scaled by
//! `SolverConfig::warm_start_coefficient` (`w`), then a biased pass and a relaxing pass each add a
//! clamped increment to that accumulator; after the substeps a restitution pass may add one more,
//! along the normal. So the momentum one substep moves through a point is its post-relax
//! accumulated impulse `I_s` less the share of the previous one warm starting did not reapply,
//! `I_s − (1 − w) I_{s−1}`, normal and friction alike, and restitution moves its increment.
//! [`ContactTransfers`] sums exactly that, from Avian's public solver state (`ConstraintGraph`):
//! - before the substeps ([`SolverSystems::PreSubstep`]): one record per point of every constraint
//!   between a [`StructureBody`] and another body, with the point's warm-start impulse as `I_{−1}`;
//! - after each substep's relax pass ([`SubstepSolverSystems::Relax`]): the substep's momentum;
//! - after restitution ([`SolverSystems::Restitution`]): its increment.
//!
//! Avian builds a constraint only where at least one body is dynamic. Each structure side
//! gets its own signed reading, including two structures touching each other; the other side
//! may be fixed ground or another moving body. `ContactPoint::normal_impulse` is not this reading:
//! Avian adds the whole accumulated impulse into it on every solve call, twice per substep, and
//! keeps the friction impulse only as the last substep's, in a basis it does not store. The
//! vendored crate is not patched for any of it.
//!
//! WHAT IT HOLDS. The last physics step's points, in Avian's constraint order, until the next step
//! replaces them, with the step's length and a count of the steps summed, so a reader that keeps
//! the count it last read reads each step once ([`ContactTransfers::steps`]). Each contact splits at
//! its body's rest speed into a blow and a press ([`ContactTransfers::split`]).
//!
//! RESTING CONTACTS. A sleeping body leaves the solver, so a body that comes to rest pressing a
//! structure (a pile on a wall, a body Avian's `ConstantForce` holds there: neither wakes it) would
//! read nothing. The reading keeps such a pair's points from the last step the solver moved them,
//! once, while Avian holds the pair sleeping and touching, and drops them when it wakes or ends
//! ([`ContactTransfers::resting`]): its press stands on the structure ([`Resting::press`]) and is no
//! new transfer.
//!
//! IMPLICIT CONTACTS. The belt applies support and traction before physics without collider
//! constraints. Its equal-and-opposite structure reaction is staged by [`ContactTransfers::stage`]
//! and published with that physics step. Every patch one source body presses on one collider
//! shares one closing-momentum budget per co-normal manifold at its load centroid: neither
//! splitting a shoe footprint nor each link's own rounding of its normal can multiply a blow.
//! These contacts are refreshed by the belt each tick and never enter the solver's sleeping-pair
//! cache.
//!
//! Filled on the authority only, where the destruction laws run; a replica's stays empty.

use std::collections::BTreeMap;

use avian3d::dynamics::solver::SolverConfig;
use avian3d::dynamics::solver::constraint_graph::ConstraintGraph;
use avian3d::dynamics::solver::contact::ContactConstraintPoint;
use avian3d::dynamics::solver::schedule::{SolverSystems, SubstepSchedule, SubstepSolverSystems};
use avian3d::dynamics::solver::solver_body::SolverBodyInertia;
use avian3d::prelude::{
    ComputedCenterOfMass, ContactGraph, PhysicsLengthUnit, PhysicsSchedule, Position, RigidBody,
    Rotation, SleepThreshold,
};
use bevy::prelude::*;

use crate::ClientReplica;

/// A body whose construction the destruction laws judge: an anchored or moving chunk group,
/// or a tree. Its
/// contacts with dynamic bodies are read into [`ContactTransfers`]. Required by the components
/// that make a body a structure, never inserted by hand.
#[derive(Component, Debug, Default, Clone, Copy)]
pub(crate) struct StructureBody;

/// One contact point between a structure and a dynamic body, over one physics step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ContactTransfer {
    /// The structure's body.
    pub(crate) structure: Entity,
    /// The structure's collider the point lies on.
    pub(crate) structure_collider: Entity,
    /// The dynamic body.
    pub(crate) body: Entity,
    /// The source collider, or `None` for an implicit belt contact with no solver collider.
    pub(crate) body_collider: Option<Entity>,
    /// The contact point in world space, as the narrow phase found it at the step's start (m).
    pub(crate) point: Vec3,
    /// The contact normal, unit, from the structure into the body.
    pub(crate) normal: Vec3,
    /// The momentum the solver moved into the body through this point over the step (N·s,
    /// world), normal and friction together; the structure took its opposite.
    pub(crate) impulse: Vec3,
    /// The mass the solver sees along the normal at this point (kg): against a static structure,
    /// the body's own effective mass there, its rotation included.
    pub(crate) effective_mass: f32,
    /// How fast the body closed on the structure along the normal before the solve (m/s);
    /// negative when the two were parting.
    pub(crate) closing_speed: f32,
    /// The body's rest speed (m/s): Avian's own, under which the library lets the body sleep
    /// (its `SleepThreshold::linear`, in `PhysicsLengthUnit`s).
    pub(crate) rest_speed: f32,
}

/// An implicit contact force applied before physics. Its momentum is the force integrated over
/// that physics step, and the structure takes its opposite. The normal points into the source.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AppliedContact {
    pub(crate) structure: Entity,
    pub(crate) structure_collider: Entity,
    pub(crate) body: Entity,
    pub(crate) point: Vec3,
    pub(crate) normal: Vec3,
    pub(crate) force: Vec3,
    pub(crate) closing_speed: f32,
}

/// A point of a pair whose body sleeps against a structure (module doc): its transfer over the
/// last step the solver moved it, and that step's length (s).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Resting {
    pub(crate) transfer: ContactTransfer,
    pub(crate) dt: f32,
}

impl Resting {
    /// The force it holds on the structure (N): the whole of its last transfer over that step.
    /// Its body sleeps, closing on nothing, so none of it is a blow.
    pub(crate) fn press(&self) -> Vec3 {
        -self.transfer.impulse / self.dt
    }
}

/// A contact source's solver inertia at the step's start, in world terms, including locked
/// axes. Shared by all its contacts: launch must count their combined momentum and torque once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BodyInertia {
    pub(crate) centre: Vec3,
    pub(crate) inverse_mass: Vec3,
    pub(crate) inverse_inertia: Mat3,
}

/// Every structure contact's transfer over the last physics step, and every resting contact's
/// (module doc).
#[derive(Resource, Debug, Default)]
pub(crate) struct ContactTransfers {
    points: Vec<ContactTransfer>,
    pending: Vec<AppliedContact>,
    /// Implicit manifolds by `(structure collider, source body)`, each with one normal.
    applied: BTreeMap<(Entity, Entity), Vec<Vec<ContactTransfer>>>,
    inertias: BTreeMap<Entity, BodyInertia>,
    resting: Vec<Resting>,
    /// The last step's length, s.
    dt: f32,
    /// The steps summed.
    steps: u64,
    /// The constraints kept this step: where each lies in Avian's constraint graph, its first
    /// point's index in `points`, and `+1` when the dynamic body is the constraint's second body
    /// (the one Avian applies `+impulse` to), `−1` when it is the first.
    kept: Vec<Kept>,
    /// Per point, beside `points`: the accumulated normal and friction impulse at the last
    /// reading (N·s, the friction in the constraint's tangent basis).
    last: Vec<(f32, Vec2)>,
}

#[derive(Debug, Clone, Copy)]
struct Kept {
    color: usize,
    index: usize,
    first: usize,
    sign: f32,
}

impl ContactTransfers {
    /// Stage a force the source actually applies this tick, before physics integrates it.
    pub(crate) fn stage(&mut self, contact: AppliedContact) {
        self.pending.push(contact);
    }

    /// The source inertia recorded with this step's contacts, shared across all its points.
    pub(crate) fn inertia(&self, body: Entity) -> BodyInertia {
        *self
            .inertias
            .get(&body)
            .expect("every body in this step's contacts has its inertia recorded with them")
    }

    /// The points of every pair whose body sleeps against a structure, each pair once, in the
    /// order their bodies fell asleep.
    pub(crate) fn resting(&self) -> &[Resting] {
        &self.resting
    }

    /// The last step's length, s: what its points' blows and presses were moved within.
    pub(crate) fn dt(&self) -> f32 {
        self.dt
    }

    /// How many steps the reading has summed, the last step's points the latest.
    pub(crate) fn steps(&self) -> u64 {
        self.steps
    }

    /// The last step's points, each with the blow and the press the structure took through it,
    /// in Avian's constraint order: each constraint's transfer split at its body's rest speed (the
    /// wall design, `.agents/docs/design/wall-destruction.md`, "Bodies in contact"). A
    /// constraint's blow is the momentum its body's closing delivered (N·s): its points' `m_b v_n`
    /// summed over those closing faster than the rest speed, `m_b` each point's effective mass,
    /// and never more than the solver moved along the normal; it falls on the points in shares of
    /// what the solver moved through each along the normal. A point's press is the rest of its
    /// transfer, held through the step (N), friction included. The split is the constraint's, not
    /// the point's: the solver's split of one contact's momentum among its points is its own
    /// (MEASURED: a cube striking a wall square on at 3 m/s, 106, 67, 112 and 67 N·s through its
    /// four corners, each corner's closing momentum 88.6 N·s).
    ///
    /// The fourth item is the contact's source mass (kg), the same on each of its points: the
    /// body's effective mass along the normal at the blow's centroid, its rotation and locked
    /// axes included. A plastic share is linear in the blow, so points that each take their share
    /// of one contact's blow against this mass take the contact's share once; against each
    /// point's own effective mass (a cube's corner holds a quarter of the cube's along its face's
    /// normal, DERIVED) the points of one contact would each spend the receiver's whole mass and
    /// hand it up to their count times the exchange. A contact whose solver moved nothing along
    /// its normal has no blow and no blow centroid; its points' mean stands for it, and no law
    /// reads a source mass without a blow.
    pub(crate) fn split(&self) -> impl Iterator<Item = (&ContactTransfer, Vec3, Vec3, f32)> {
        let ends = self
            .kept
            .iter()
            .skip(1)
            .map(|kept| kept.first)
            .chain([self.points.len()]);
        self.kept
            .iter()
            .zip(ends)
            .flat_map(move |(kept, end)| self.split_points(&self.points[kept.first..end]))
            .chain(self.applied.values().flat_map(|manifolds| {
                manifolds
                    .iter()
                    .flat_map(|points| self.split_points(points))
            }))
    }

    /// Solver points carry the constraint's effective masses. Implicit patches on one collider
    /// share the source mass at their centroid, so neither subdividing a footprint nor a link's
    /// rounded normal spends that mass repeatedly.
    fn split_points<'a>(
        &'a self,
        points: &'a [ContactTransfer],
    ) -> impl Iterator<Item = (&'a ContactTransfer, Vec3, Vec3, f32)> + 'a {
        let first = points
            .first()
            .expect("every retained contact has at least one point");
        let along = |point: &ContactTransfer| point.impulse.dot(point.normal);
        let moved: f32 = points.iter().map(along).sum();
        let mass = self.contact_mass(points, moved);
        let closing = if first.body_collider.is_some() {
            points
                .iter()
                .filter(|point| point.closing_speed > point.rest_speed)
                .map(|point| point.effective_mass * point.closing_speed)
                .sum()
        } else if moved > 0.0 {
            let speed = points
                .iter()
                .map(|p| p.closing_speed * along(p))
                .sum::<f32>()
                / moved;
            if speed > first.rest_speed {
                mass * speed
            } else {
                0.0
            }
        } else {
            0.0 // No normal momentum was moved, so there is no blow.
        };
        let blow = closing.min(moved);
        points.iter().map(move |point| {
            let blow = if moved > 0.0 {
                point.normal * (blow * along(point) / moved)
            } else {
                Vec3::ZERO
            };
            (point, -blow, (blow - point.impulse) / self.dt, mass)
        })
    }

    /// One contact's source mass ([`Self::split`]), kg: its body's effective mass along the
    /// contact's normal at the centroid of what the solver moved through its points along it
    /// (`moved`, N·s), their mean where it moved nothing.
    fn contact_mass(&self, points: &[ContactTransfer], moved: f32) -> f32 {
        let first = points
            .first()
            .expect("Avian builds a contact constraint only for a manifold with points");
        let at = if moved > 0.0 {
            points
                .iter()
                .map(|point| point.point * point.impulse.dot(point.normal))
                .sum::<Vec3>()
                / moved
        } else {
            points.iter().map(|point| point.point).sum::<Vec3>() / points.len() as f32
        };
        let inertia = self.inertia(first.body);
        let normal = points
            .iter()
            .map(|point| {
                point.normal
                    * if moved > 0.0 {
                        point.impulse.dot(point.normal)
                    } else {
                        1.0
                    }
            })
            .sum::<Vec3>()
            .normalize();
        let arm = (at - inertia.centre).cross(normal);
        let inverse =
            normal.dot(inertia.inverse_mass * normal) + arm.dot(inertia.inverse_inertia * arm);
        assert!(
            inverse.is_finite() && inverse >= 0.0,
            "a contact's solver inverse effective mass is finite and nonnegative"
        );
        // Locked axes can prevent all motion along this normal even on a dynamic body.
        // Its effective mass is infinite, and Avian's zero effective mass moves no blow.
        inverse.recip()
    }
}

/// ASSUMED numerical envelope: pose composition may round a shared direction by up to
/// 32 float epsilons. This merges roundoff alone; distinct footprint directions retain their
/// own closing momentum and cannot turn a resting face's press into another face's blow.
fn same_normal(a: Vec3, b: Vec3) -> bool {
    a.dot(b) > 0.0 && a.cross(b).length_squared() <= (32.0 * f32::EPSILON).powi(2)
}

/// A point's accumulated friction impulse, in its constraint's tangent basis (N·s); none without
/// friction.
fn friction(point: &ContactConstraintPoint) -> Vec2 {
    point
        .tangent_part
        .as_ref()
        .map_or(Vec2::ZERO, |part| part.impulse)
}

/// Before the substeps: the resting pairs brought up to date, then a record for every point of
/// every constraint with a structure on one side. `time` is the physics clock Avian runs its
/// schedule on.
fn keep(
    graph: Res<ConstraintGraph>,
    contacts: Res<ContactGraph>,
    structures: Query<(), With<StructureBody>>,
    bodies: Query<(
        Option<&SleepThreshold>,
        Option<&SolverBodyInertia>,
        &RigidBody,
        &Position,
        &Rotation,
        &ComputedCenterOfMass,
    )>,
    length_unit: Res<PhysicsLengthUnit>,
    time: Res<Time>,
    mut transfers: ResMut<ContactTransfers>,
) {
    let ContactTransfers {
        points,
        inertias,
        resting,
        dt,
        steps,
        kept,
        last,
        pending,
        applied,
    } = &mut *transfers;
    let asleep = |transfer: &ContactTransfer| {
        contacts
            .get(
                transfer.structure_collider,
                transfer
                    .body_collider
                    .expect("only solver contacts enter the sleeping-pair cache"),
            )
            .is_some_and(|(edge, pair)| edge.is_sleeping() && pair.is_touching())
    };
    // A pair that woke or ended drops its rest; one whose body slept after the last step rests
    // with that step's transfer. A sleeping pair builds no constraint, so no pair is both.
    resting.retain(|rest| asleep(&rest.transfer));
    resting.extend(
        points
            .iter()
            .filter(|point| asleep(point))
            .map(|&transfer| Resting { transfer, dt: *dt }),
    );
    points.clear();
    inertias.clear();
    kept.clear();
    last.clear();
    *dt = time.delta_secs();
    *steps += 1;
    for (color, colored) in graph.colors.iter().enumerate() {
        for (index, constraint) in colored.contact_constraints.iter().enumerate() {
            let sides: Vec<_> = [true, false]
                .into_iter()
                .filter(|&first| {
                    structures.contains(if first {
                        constraint.body1
                    } else {
                        constraint.body2
                    })
                })
                .collect();
            for structure_first in sides {
                let (_, pair) = contacts
                    .get_by_id(constraint.contact_id)
                    .expect("a contact constraint is built from a pair in the contact graph");
                let manifold = &pair.manifolds[constraint.manifold_index];
                assert_eq!(
                    manifold.points.len(),
                    constraint.points.len(),
                    "a contact constraint holds its manifold's points one for one"
                );
                let (sign, structure, structure_collider, body, body_collider) = if structure_first
                {
                    (
                        1.0,
                        constraint.body1,
                        pair.collider1,
                        constraint.body2,
                        pair.collider2,
                    )
                } else {
                    (
                        -1.0,
                        constraint.body2,
                        pair.collider2,
                        constraint.body1,
                        pair.collider1,
                    )
                };
                kept.push(Kept {
                    color,
                    index,
                    first: points.len(),
                    sign,
                });
                let (rest, solver, kind, position, rotation, com) = bodies.get(body).expect(
                    "each contact side is a rigid body with its required pose and centre of mass",
                );
                let inertia = if kind.is_static() {
                    &SolverBodyInertia::DUMMY
                } else {
                    solver.expect("Avian supplies solver inertia for an active moving contact body")
                };
                let rest_speed = rest
                    .or_else(|| {
                        bodies
                            .get(structure)
                            .expect("the other contact side is a rigid body")
                            .0
                    })
                    .expect("a solver contact has at least one moving side with a sleep threshold")
                    .linear
                    * length_unit.0;
                inertias.entry(body).or_insert_with(|| BodyInertia {
                    centre: position.0 + rotation.0 * com.0,
                    inverse_mass: inertia.effective_inv_mass(),
                    inverse_inertia: inertia.effective_inv_angular_inertia().to_mat3(),
                });
                for (point, contact) in constraint.points.iter().zip(&manifold.points) {
                    points.push(ContactTransfer {
                        structure,
                        structure_collider,
                        body,
                        body_collider: Some(body_collider),
                        point: contact.point,
                        normal: sign * constraint.normal,
                        impulse: Vec3::ZERO,
                        effective_mass: point.normal_part.effective_mass,
                        closing_speed: -point.normal_speed,
                        rest_speed,
                    });
                    last.push((point.normal_part.impulse, friction(point)));
                }
            }
        }
    }
    applied.clear();
    // Canonical staging order fixes both the clustering representative and floating sums.
    pending.sort_by(|a, b| {
        let key = |c: &AppliedContact| {
            (
                c.structure_collider,
                c.body,
                c.normal.to_array().map(f32::to_bits),
                c.point.to_array().map(f32::to_bits),
                c.force.to_array().map(f32::to_bits),
                c.closing_speed.to_bits(),
            )
        };
        key(a).cmp(&key(b))
    });
    for contact in pending.drain(..) {
        let (rest, solver, _, position, rotation, com) = bodies.get(contact.body).expect(
            "a source that applied an implicit contact remains a dynamic body through this physics step",
        );
        let solver = solver.expect("an implicit contact source is an active moving body");
        let rest = rest.expect("a moving implicit contact source has its sleep threshold");
        let inertia = *inertias.entry(contact.body).or_insert_with(|| BodyInertia {
            centre: position.0 + rotation.0 * com.0,
            inverse_mass: solver.effective_inv_mass(),
            inverse_inertia: solver.effective_inv_angular_inertia().to_mat3(),
        });
        let arm = (contact.point - inertia.centre).cross(contact.normal);
        let inverse = contact.normal.dot(inertia.inverse_mass * contact.normal)
            + arm.dot(inertia.inverse_inertia * arm);
        let manifolds = applied
            .entry((contact.structure_collider, contact.body))
            .or_default();
        let slot = manifolds
            .iter()
            .position(|points| same_normal(points[0].normal, contact.normal));
        let slot = slot.unwrap_or_else(|| {
            manifolds.push(Vec::new());
            manifolds.len() - 1
        });
        manifolds[slot].push(ContactTransfer {
            structure: contact.structure,
            structure_collider: contact.structure_collider,
            body: contact.body,
            body_collider: None,
            point: contact.point,
            normal: contact.normal,
            impulse: contact.force * *dt,
            effective_mass: if inverse > 0.0 { inverse.recip() } else { 0.0 },
            closing_speed: contact.closing_speed,
            rest_speed: rest.linear * length_unit.0,
        });
    }
}

/// After a substep's relax pass: the momentum the substep moved through every kept point.
fn add_substep(
    graph: Res<ConstraintGraph>,
    config: Res<SolverConfig>,
    mut transfers: ResMut<ContactTransfers>,
) {
    let unapplied = 1.0 - config.warm_start_coefficient;
    let ContactTransfers {
        points, kept, last, ..
    } = &mut *transfers;
    for constraint_at in kept.iter() {
        let constraint =
            &graph.colors[constraint_at.color].contact_constraints[constraint_at.index];
        let [tangent1, tangent2] = constraint.tangent_directions();
        for (offset, point) in constraint.points.iter().enumerate() {
            let slot = constraint_at.first + offset;
            let (normal_now, friction_now) = (point.normal_part.impulse, friction(point));
            let (normal_before, friction_before) = last[slot];
            let normal = normal_now - unapplied * normal_before;
            let tangent = friction_now - unapplied * friction_before;
            points[slot].impulse += constraint_at.sign
                * (normal * constraint.normal + tangent.x * tangent1 + tangent.y * tangent2);
            last[slot] = (normal_now, friction_now);
        }
    }
}

/// After restitution: the increment it added along every kept point's normal.
fn add_restitution(graph: Res<ConstraintGraph>, mut transfers: ResMut<ContactTransfers>) {
    let ContactTransfers {
        points, kept, last, ..
    } = &mut *transfers;
    for constraint_at in kept.iter() {
        let constraint =
            &graph.colors[constraint_at.color].contact_constraints[constraint_at.index];
        for (offset, point) in constraint.points.iter().enumerate() {
            let slot = constraint_at.first + offset;
            let normal_now = point.normal_part.impulse;
            points[slot].impulse +=
                constraint_at.sign * (normal_now - last[slot].0) * constraint.normal;
            last[slot].0 = normal_now;
        }
    }
}

/// Mount the reading in Avian's solver.
pub(crate) fn sim_plugin(app: &mut App) {
    app.init_resource::<ContactTransfers>()
        .add_systems(
            PhysicsSchedule,
            (
                keep.in_set(SolverSystems::PreSubstep)
                    .run_if(not(resource_exists::<ClientReplica>)),
                add_restitution
                    .after(SolverSystems::Restitution)
                    .before(SolverSystems::Finalize),
            ),
        )
        .add_systems(
            SubstepSchedule,
            add_substep
                .after(SubstepSolverSystems::Relax)
                .before(SubstepSolverSystems::Damping),
        );
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use avian3d::prelude::{
        Collider, ColliderDensity, ComputedMass, ConstantForce, Friction, Gravity, LinearVelocity,
        LockedAxes, Physics, PhysicsPlugins, Restitution, RigidBody, SleepingDisabled,
    };
    use bevy::time::TimeUpdateStrategy;

    use super::*;

    /// Coplanar implicit patches share one closing-momentum budget at their centroid. Structural
    /// laws rely on a subdivided footprint producing the same blow and press as one patch.
    /// FIXTURE DATA: source mass 10 kg, closing speed 1 m/s, impulse 20 N·s over 0.1 s.
    #[test]
    fn implicit_patch_subdivision_cannot_multiply_the_source_momentum() {
        let mut world = World::new();
        let body = world.spawn_empty().id();
        let structure = world.spawn_empty().id();
        let collider = world.spawn_empty().id();
        for copies in [1, 4] {
            let mut transfers = ContactTransfers {
                dt: 0.1,
                ..default()
            };
            transfers.inertias.insert(
                body,
                BodyInertia {
                    centre: Vec3::ZERO,
                    inverse_mass: Vec3::splat(0.1),
                    inverse_inertia: Mat3::IDENTITY,
                },
            );
            let points = (0..copies)
                .map(|_| ContactTransfer {
                    structure,
                    structure_collider: collider,
                    body,
                    body_collider: None,
                    point: Vec3::ZERO,
                    normal: Vec3::Y,
                    impulse: Vec3::Y * (20.0 / copies as f32),
                    effective_mass: 10.0,
                    closing_speed: 1.0,
                    rest_speed: 0.15,
                })
                .collect();
            transfers.applied.insert((collider, body), vec![points]);
            let (mut blow, mut press) = (Vec3::ZERO, Vec3::ZERO);
            for (_, struck, held, mass) in transfers.split() {
                assert_eq!(mass, 10.0);
                blow += struck;
                press += held;
            }
            assert_eq!(blow, Vec3::NEG_Y * 10.0);
            assert_eq!(press, Vec3::NEG_Y * 100.0);
        }
    }

    /// A source pressing one collider through several links delivers at most its own closing
    /// momentum as a blow, however each link's normal rounds: a straight belt run's links meet a
    /// wall along normals equal in law but posed link by link. Wall and trunk laws rely on the
    /// blow being the source's, never one budget per link. FIXTURE DATA: the 1000 kg cube clear
    /// of the wall, closing at 1 m/s, four 128 kN patches through its centre along normals one
    /// ulp apart.
    #[test]
    fn implicit_patches_with_rounded_normals_share_the_source_momentum() {
        let mut app = world(1.0);
        let structure = wall(&mut app);
        let collider = app
            .world_mut()
            .query_filtered::<(Entity, &ChildOf), With<Collider>>()
            .iter(app.world())
            .find(|(_, parent)| parent.parent() == structure)
            .map(|(collider, _)| collider)
            .expect("the wall has its collider");
        let centre = Vec3::new(3.0, 0.0, 0.0);
        let body = cube(&mut app, centre, Vec3::ZERO);
        app.update();
        let normals: Vec<Vec3> = (0..4)
            .map(|ulp| Vec3::new(0.6, f32::from_bits(0.8f32.to_bits() + ulp), 0.0))
            .collect();
        for &normal in &normals {
            app.world_mut()
                .resource_mut::<ContactTransfers>()
                .stage(AppliedContact {
                    structure,
                    structure_collider: collider,
                    body,
                    point: centre,
                    normal,
                    force: normal * 128_000.0,
                    closing_speed: 1.0,
                });
        }
        app.update();
        let transfers = app.world().resource::<ContactTransfers>();
        let (mut blow, mut seen) = (Vec3::ZERO, 0);
        for (point, struck, _, mass) in transfers.split().filter(|(point, ..)| point.body == body) {
            assert_eq!(point.body_collider, None);
            assert!(
                (mass - 1000.0).abs() < 0.1,
                "the cube's mass at its centre: {mass}"
            );
            blow += struck;
            seen += 1;
        }
        assert_eq!(seen, normals.len(), "every staged patch is read");
        let delivered = -blow.dot(normals[0]);
        assert!(
            delivered > 0.0 && delivered <= 1000.0 * (1.0 + 1e-4),
            "the blow {delivered} N·s exceeds the source's closing momentum 1000 N·s"
        );
    }

    /// Distinct contact directions have distinct closing budgets. Environment construction
    /// relies on a resting top-face press staying a press while another face is approached;
    /// staging order must not choose the source mass. FIXTURE DATA: a cube and two orthogonal
    /// patches at the same off-centre point, one resting and one closing.
    #[test]
    fn implicit_unequal_normals_preserve_resting_press_and_staging_order() {
        let run = |reverse: bool| {
            let mut app = world(1.0);
            let structure = wall(&mut app);
            let collider = app
                .world_mut()
                .query_filtered::<(Entity, &ChildOf), With<Collider>>()
                .iter(app.world())
                .find(|(_, p)| p.parent() == structure)
                .unwrap()
                .0;
            let centre = Vec3::new(3.0, 0.0, 0.0);
            let body = cube(&mut app, centre, Vec3::ZERO);
            app.update();
            let mut patches = [(Vec3::Y, 0.0), (Vec3::X, 1.0)];
            if reverse {
                patches.reverse();
            }
            for (normal, closing_speed) in patches {
                app.world_mut()
                    .resource_mut::<ContactTransfers>()
                    .stage(AppliedContact {
                        structure,
                        structure_collider: collider,
                        body,
                        point: centre + Vec3::new(0.0, 1.0, 0.5),
                        normal,
                        force: normal * 128_000.0,
                        closing_speed,
                    });
            }
            app.update();
            let transfers = app.world().resource::<ContactTransfers>();
            let mut result = [0; 8];
            for (point, blow, _, mass) in transfers.split().filter(|(p, ..)| p.body == body) {
                let slot = if point.normal == Vec3::Y { 0 } else { 4 };
                if slot == 0 {
                    assert_eq!(blow, Vec3::ZERO, "a resting face only presses");
                }
                result[slot..slot + 3].copy_from_slice(&blow.to_array().map(f32::to_bits));
                result[slot + 3] = mass.to_bits();
            }
            result
        };
        assert_eq!(
            run(false),
            run(true),
            "contact order cannot select a normal or mass"
        );
    }

    /// Avian at the game's fixed 64 Hz tick, one tick per update, no gravity, with the reading
    /// mounted and the solver's warm start scaled by `warm_start` (fixture data).
    fn world(warm_start: f32) -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            // Avian's collider cache reads `AssetEvent<Mesh>`.
            AssetPlugin::default(),
            PhysicsPlugins::default(),
            sim_plugin,
        ))
        .init_asset::<Mesh>()
        .insert_resource(Gravity::ZERO)
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
            15_625,
        )));
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        app.world_mut()
            .resource_mut::<SolverConfig>()
            .warm_start_coefficient = warm_start;
        // The first updates start the clocks; every update after them steps physics once.
        for _ in 0..3 {
            app.update();
        }
        app
    }

    /// A structure: a static root holding a 0.5 m thick wall whose face toward −x stands at
    /// x = −0.25, as a building holds its elements (fixture data).
    fn wall(app: &mut App) -> Entity {
        let root = app
            .world_mut()
            .spawn((Transform::IDENTITY, RigidBody::Static, StructureBody))
            .id();
        app.world_mut().spawn((
            ChildOf(root),
            Transform::IDENTITY,
            Collider::cuboid(0.5, 4.0, 4.0),
            Friction::new(0.6),
            Restitution::new(0.5),
        ));
        root
    }

    /// A 1 m, 1000 kg dynamic cube centred at `at`, moving at `velocity` (fixture data).
    fn cube(app: &mut App, at: Vec3, velocity: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                Transform::from_translation(at),
                RigidBody::Dynamic,
                Collider::cuboid(1.0, 1.0, 1.0),
                ColliderDensity(1000.0),
                Friction::new(0.6),
                Restitution::new(0.5),
                LinearVelocity(velocity),
            ))
            .id()
    }

    /// One step: the body's momentum change, the transfer the reading summed into it, the step's
    /// length, and whether the body's touching pair with `structure` names the structure first
    /// (none out of contact); panics unless the update stepped physics exactly once and every
    /// point on the body names `structure`.
    fn step(app: &mut App, body: Entity, structure: Entity) -> (Vec3, Vec3, f32, Option<bool>) {
        let momentum = |app: &App| {
            let world = app.world();
            world.get::<ComputedMass>(body).expect("a body").value()
                * world.get::<LinearVelocity>(body).expect("a body").0
        };
        let (before, clock) = (
            momentum(app),
            app.world().resource::<Time<Physics>>().elapsed(),
        );
        app.update();
        let time = app.world().resource::<Time<Physics>>();
        let dt = time.delta_secs();
        assert_eq!(
            time.elapsed() - clock,
            time.delta(),
            "the update steps physics once"
        );
        let mut transfer = Vec3::ZERO;
        for point in &app.world().resource::<ContactTransfers>().points {
            if point.body == body {
                assert_eq!(point.structure, structure, "the point names its structure");
                let inertia = app.world().resource::<ContactTransfers>().inertia(body);
                let arm = (point.point - inertia.centre).cross(point.normal);
                let inverse_mass = point.normal.dot(inertia.inverse_mass * point.normal)
                    + arm.dot(inertia.inverse_inertia * arm);
                assert!(
                    (inverse_mass.recip() - point.effective_mass).abs()
                        <= 2e-5 * point.effective_mass,
                    "the source snapshot reproduces the solver's point effective mass"
                );
                transfer += point.impulse;
            }
        }
        let structure_first = app
            .world()
            .resource::<ContactGraph>()
            .iter_active_touching()
            .find(|pair| pair.body1 == Some(body) || pair.body2 == Some(body))
            .map(|pair| pair.body1 == Some(structure));
        (momentum(app) - before, transfer, dt, structure_first)
    }

    /// Both sides of a resting joint receive the solver's one transfer with opposite signs.
    /// Moving chunk groups rely on this against fixed ground and against another structure.
    /// ASSUMED fixture: the existing cube hits the existing wall along its normal.
    #[test]
    fn both_structure_sides_receive_their_contact_transfer() {
        let mut app = world(1.0);
        let fixed = wall(&mut app);
        let moving = cube(&mut app, Vec3::new(-0.76, 0.0, 0.0), Vec3::X * 3.0);
        app.world_mut().entity_mut(moving).insert(StructureBody);
        let mut observed = false;
        for _ in 0..8 {
            app.update();
            let reading = app.world().resource::<ContactTransfers>();
            let on = |structure| {
                reading
                    .points
                    .iter()
                    .filter(|p| p.structure == structure)
                    .map(|p| -p.impulse)
                    .sum::<Vec3>()
            };
            let a = on(fixed);
            let b = on(moving);
            if a.length() > 0.0 {
                assert!(b.length() > 0.0);
                assert!((a + b).length() < 1e-4 * a.length());
                assert!(
                    reading
                        .split()
                        .any(|(p, ..)| p.structure == moving && p.body == fixed)
                );
                observed = true;
            }
        }
        assert!(observed, "the moving group met the fixed wall");
    }

    /// THE READING IS THE MOMENTUM THE SOLVER MOVED (the game's environment destruction model):
    /// in every step, the transfer summed into a body that touches only a structure equals the
    /// body's momentum change less its applied force's `F·dt`, normal and friction together, with
    /// the warm start whole or halved; and a body pressed on the structure by a steady force reads
    /// `F·dt` a step. Every structure law that takes a body's contact load relies on it: the
    /// strength limit reads the press, the energy limit and the give-back the blow. A reading of
    /// `ContactPoint::normal_impulse` (2.000 × `F·dt` under this press at the whole warm start,
    /// 4.000 × at the halved one, MEASURED where the reading holds 1.000 ×), of the normal part alone
    /// (the oblique strike's friction), without restitution's increment (the strike's rebound),
    /// without the warm start's unapplied share (the halved warm start) or with the body's side of
    /// the pair taken wrong (the strike's pair names the body first, the press's the structure)
    /// fails here. Fixture numbers are fixture data.
    #[test]
    fn the_reading_is_the_momentum_the_solver_moved() {
        for warm_start in [1.0, 0.5] {
            // An oblique strike at 4 m/s into the face, 1.5 m/s along it: friction and the
            // rebound both act.
            let mut app = world(warm_start);
            let structure = wall(&mut app);
            let striker = cube(
                &mut app,
                Vec3::new(-1.3, 0.0, 0.0),
                Vec3::new(4.0, 0.0, 1.5),
            );
            app.update();
            let (mut normal, mut along) = (0.0_f32, 0.0_f32);
            let mut orders = Vec::new();
            for tick in 0..24 {
                let (change, transfer, _, structure_first) = step(&mut app, striker, structure);
                assert!(
                    (change - transfer).length() <= 1e-4 * 4000.0,
                    "w {warm_start}, strike tick {tick}: momentum change {change} against the \
                     reading {transfer}"
                );
                normal -= transfer.x;
                along -= transfer.z;
                orders.extend(structure_first);
            }
            assert!(
                !orders.is_empty() && orders.iter().all(|first| !first),
                "w {warm_start}: the strike's pair names the moving body first: {orders:?}"
            );
            assert!(
                normal > 1.05 * 4000.0,
                "w {warm_start}: the striker rebounds, {normal} N·s against 4000 N·s closing"
            );
            assert!(
                along > 0.1 * 1500.0,
                "w {warm_start}: friction acts along the face, {along} N·s"
            );

            // A press: a cube at rest, the wall spawned against its face, then 5 kN into it. The
            // cube never sleeps, as a body its engine drives: Avian lets a body under a
            // `ConstantForce` sleep, and a sleeping body leaves the solver.
            let mut app = world(warm_start);
            let pressed = cube(&mut app, Vec3::new(-0.75, 0.0, 0.0), Vec3::ZERO);
            app.world_mut().entity_mut(pressed).insert(SleepingDisabled);
            app.update();
            let structure = wall(&mut app);
            app.world_mut()
                .entity_mut(pressed)
                .insert(ConstantForce::new(5000.0, 0.0, 0.0));
            let mut orders = Vec::new();
            for tick in 0..60 {
                let (change, transfer, dt, structure_first) = step(&mut app, pressed, structure);
                let applied = Vec3::X * 5000.0 * dt;
                assert!(
                    (change - applied - transfer).length() <= 1e-4 * applied.length(),
                    "w {warm_start}, press tick {tick}: momentum change {change} against the \
                     reading {transfer} and F·dt {applied}"
                );
                if tick >= 20 {
                    assert!(
                        (transfer + applied).length() <= 0.005 * applied.length(),
                        "w {warm_start}, press tick {tick}: the reading {transfer} against F·dt \
                         {applied}"
                    );
                }
                orders.extend(structure_first);
            }
            assert!(
                !orders.is_empty() && orders.iter().all(|first| *first),
                "w {warm_start}: the press's pair names the structure first: {orders:?}"
            );
        }
    }

    /// Every step's points on `body` split ([`ContactTransfers::split`]) over `ticks` updates: per
    /// step, the blow and press summed and the transfer the structure took (`−impulse`).
    fn splits(app: &mut App, body: Entity, ticks: usize) -> Vec<(Vec3, Vec3, Vec3)> {
        (0..ticks)
            .map(|_| {
                app.update();
                let transfers = app.world().resource::<ContactTransfers>();
                let mut sums = (Vec3::ZERO, Vec3::ZERO, Vec3::ZERO);
                for (point, blow, press, _) in transfers.split().filter(|(p, ..)| p.body == body) {
                    sums.0 += blow;
                    sums.1 += press;
                    sums.2 -= point.impulse;
                }
                sums
            })
            .collect()
    }

    /// A CONTACT IS A BLOW ABOVE ITS BODY'S OWN REST SPEED AND A PRESS BELOW IT (the game's
    /// environment destruction model): a 1000 kg cube striking the structure square on at 3 m/s
    /// delivers a blow of its closing momentum, 3000 N·s along the normal into the structure
    /// within 1 % (DERIVED: a cube's corner point has a quarter of its effective mass along a face
    /// normal, so the four points' `m_b v_n` sum to `m v`), though restitution moves half as much
    /// again; each step's blow and press over the step make up its whole transfer. The same
    /// strike by a cube whose `SleepThreshold` puts its rest speed above 3 m/s only presses, and a
    /// cube a steady 5 kN holds on the structure presses 5 kN within 0.5 % with no blow. The
    /// destruction laws rely on the split (the wall design, "Bodies in contact"): the strength
    /// limit reads the press, the energy limit and the give-back the blow. A split at a fixed
    /// rest speed (the strike above its body's own), at the whole normal transfer (4500 N·s), or
    /// one that drops a striking step's press fails here. Fixture numbers are fixture data.
    #[test]
    fn a_contact_is_a_blow_above_its_bodys_rest_speed_and_a_press_below_it() {
        let structure_way = Vec3::X;
        for slow_to_rest in [false, true] {
            let mut app = world(1.0);
            wall(&mut app);
            let striker = cube(&mut app, Vec3::new(-1.3, 0.0, 0.0), Vec3::X * 3.0);
            if slow_to_rest {
                app.world_mut().entity_mut(striker).insert(SleepThreshold {
                    linear: 4.0,
                    angular: 0.15,
                });
            }
            let steps = splits(&mut app, striker, 30);
            let dt = app.world().resource::<ContactTransfers>().dt;
            let blow: Vec3 = steps.iter().map(|(blow, ..)| *blow).sum();
            let normal: f32 = steps
                .iter()
                .map(|(.., taken)| taken.dot(structure_way))
                .sum();
            assert!(
                normal > 1.4 * 3000.0,
                "rest speed raised {slow_to_rest}: restitution moves more than the closing \
                 momentum, {normal} N·s"
            );
            for (tick, (blow, press, taken)) in steps.iter().enumerate() {
                assert!(
                    (*blow + *press * dt - *taken).length() <= 1e-3,
                    "tick {tick}: blow {blow} and press {press} make up the transfer {taken}"
                );
            }
            if slow_to_rest {
                assert_eq!(
                    blow,
                    Vec3::ZERO,
                    "a strike under its body's rest speed presses"
                );
            } else {
                assert!(
                    (blow - structure_way * 3000.0).length() <= 0.01 * 3000.0,
                    "the strike's blow {blow} against its closing momentum 3000 N·s"
                );
            }
        }

        let mut app = world(1.0);
        let pressed = cube(&mut app, Vec3::new(-0.75, 0.0, 0.0), Vec3::ZERO);
        app.world_mut().entity_mut(pressed).insert(SleepingDisabled);
        app.update();
        wall(&mut app);
        app.world_mut()
            .entity_mut(pressed)
            .insert(ConstantForce::new(5000.0, 0.0, 0.0));
        for (tick, (blow, press, _)) in splits(&mut app, pressed, 40).iter().enumerate().skip(20) {
            assert_eq!(
                *blow,
                Vec3::ZERO,
                "press tick {tick}: a held body delivers no blow"
            );
            assert!(
                (*press - structure_way * 5000.0).length() <= 0.005 * 5000.0,
                "press tick {tick}: the press {press} against 5 kN"
            );
        }
    }

    /// A CONTACT'S POINTS CARRY ONE SOURCE MASS, THE BODY'S FOR THEIR COMBINED BLOW (the game's
    /// environment destruction model): a 1000 kg cube striking the structure square on at 3 m/s
    /// gives every point of its contact the cube's whole mass within 2 %, though each corner's own
    /// effective mass along the face's normal is a quarter of it (DERIVED for a cube's corner;
    /// the solver's uneven split among the corners moves the blow's centroid off the face's
    /// centre by a little, so not exactly). The section, rest and launch laws rely on it: each
    /// point's plastic share of one blow against one receiver sums to the contact's share once.
    /// Passing each point's own effective mass reads 250 kg and fails, as does a source mass that
    /// differs between the points of one contact. Fixture numbers are fixture data.
    #[test]
    fn a_contacts_points_carry_its_bodys_mass_for_their_combined_blow() {
        let mut app = world(1.0);
        wall(&mut app);
        let striker = cube(&mut app, Vec3::new(-1.3, 0.0, 0.0), Vec3::X * 3.0);
        let mut struck = false;
        for _ in 0..30 {
            app.update();
            let transfers = app.world().resource::<ContactTransfers>();
            let points: Vec<(f32, Vec3, f32)> = transfers
                .split()
                .filter(|(point, ..)| point.body == striker)
                .map(|(point, blow, _, mass)| (point.effective_mass, blow, mass))
                .collect();
            if points.iter().all(|(_, blow, _)| *blow == Vec3::ZERO) {
                continue;
            }
            struck = true;
            assert_eq!(points.len(), 4, "a face on a face meets at four corners");
            for &(own, _, mass) in &points {
                assert!(
                    (own - 250.0).abs() <= 0.01 * 250.0,
                    "a corner's own effective mass {own} kg is a quarter of the cube's"
                );
                assert!(
                    (mass - 1000.0).abs() <= 0.02 * 1000.0,
                    "the contact's source mass {mass} kg against the cube's 1000 kg"
                );
                assert_eq!(mass, points[0].2, "one source mass for the contact");
            }
            println!(
                "MEASURED square-on contact source mass {:.2} kg, corner effective mass {:.2} kg",
                points[0].2, points[0].0
            );
        }
        assert!(struck, "the fixture cube strikes the structure");
    }
    /// A LOCKED BODY'S ZERO TRANSFER STILL HAS A CONTACT (the game's environment destruction
    /// model). The contact reader must accept Avian's zero inverse response: the infinite source
    /// mass carries no blow. Panicking on that constraint would stop the authority's law pass.
    #[test]
    fn a_locked_body_can_touch_without_delivering_a_blow() {
        let mut app = world(1.0);
        wall(&mut app);
        let body = cube(&mut app, Vec3::new(-0.7, 0.0, 0.0), Vec3::ZERO);
        app.world_mut()
            .entity_mut(body)
            .insert(LockedAxes::ALL_LOCKED);
        let mut seen = false;
        for _ in 0..5 {
            app.update();
            let transfers = app.world().resource::<ContactTransfers>();
            for (_, blow, press, mass) in transfers.split().filter(|(p, ..)| p.body == body) {
                seen = true;
                assert_eq!(blow, Vec3::ZERO);
                assert_eq!(press, Vec3::ZERO);
                assert_eq!(mass, f32::INFINITY);
            }
        }
        assert!(seen, "Avian keeps a constraint for the locked dynamic body");
    }
}
