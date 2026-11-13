//! What solver contacts moved between interacting bodies, and implicit belt contacts moved
//! into structures, one physics step at a time: the reading a body's contact load is taken from in
//! the game's environment destruction model (game state only; a contact never doses a tank). The model it
//! serves is `.agents/docs/design/construction-destruction.md`, "Bodies in contact".
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
//!   with the point's warm-start impulse as `I_{−1}`;
//! - after each substep's relax pass ([`SubstepSolverSystems::Relax`]): the substep's momentum;
//! - after restitution ([`SolverSystems::Restitution`]): its increment.
//!
//! Avian builds a constraint only where at least one body is dynamic. Each structure side
//! reads a signed view of the same record; the sweep reads every solver constraint once,
//! independently of [`StructureBody`], through [`ContactTransfers::solver_groups`]. The other side
//! may be fixed ground or another moving body. `ContactPoint::normal_impulse` is not this reading:
//! Avian adds the whole accumulated impulse into it on every solve call, twice per substep, and
//! keeps the friction impulse only as the last substep's, in a basis it does not store. The
//! vendored crate is not patched for any of it.
//!
//! WHAT IT HOLDS. The last physics step's points, in Avian's constraint order, until the next step
//! replaces them, with the step's length and a count of the steps summed. A reader retaining
//! [`ContactTransfers::steps`] reads each step once. The structure check receives the whole
//! transfer divided by that step; hull sweeps own the separate plastic exchange before physics.
//!
//! RESTING CONTACTS. A sleeping body leaves the solver, so a body that comes to rest pressing a
//! static structure (a pile on a wall, a body Avian's `ConstantForce` holds there: neither wakes it) would
//! read nothing. The reading keeps such a pair's points from the last step the solver moved them,
//! once, while Avian holds the pair sleeping and touching, and drops them when it wakes or ends
//! ([`ContactTransfers::resting`]): its press stands on the structure ([`Resting::press`]) and is no
//! new transfer.
//!
//! IMPLICIT CONTACTS. The belt applies support and traction before physics without collider
//! constraints. Fixed construction receives its opposite reaction through [`ContactTransfers::stage`],
//! refreshed by every belt pass. A tank at rest runs no pass (`track::sim::NotResting`), so its
//! last step's points rest on the structure as a sleeping pair's do, held while the tank sleeps.
//!
//! Filled on the authority only, where the destruction laws run; a replica's stays empty.

use std::collections::BTreeMap;

use avian3d::dynamics::solver::SolverConfig;
use avian3d::dynamics::solver::constraint_graph::ConstraintGraph;
use avian3d::dynamics::solver::contact::{ContactConstraint, ContactConstraintPoint};
use avian3d::dynamics::solver::schedule::{SolverSystems, SubstepSchedule, SubstepSolverSystems};
use avian3d::dynamics::solver::solver_body::SolverBodyInertia;
use avian3d::prelude::{
    ComputedCenterOfMass, ContactGraph, PhysicsSchedule, Position, RigidBody, Rotation, Sleeping,
};
use bevy::prelude::*;
use bevy::tasks::{ComputeTaskPool, ParallelSlice, ParallelSliceMut, TaskPool};

use crate::ClientReplica;

/// A body whose construction the destruction laws judge: an anchored or moving chunk group. Its
/// contacts select the structural reporting in [`ContactTransfers::groups`]. Required by the components
/// that make a body a structure, never inserted by hand.
#[derive(Component, Debug, Default, Clone, Copy)]
pub(crate) struct StructureBody;

/// One contact point over one physics step, oriented from `reference` into `other`.
/// In structural reporting, `reference` has [`StructureBody`]; the generic solver stream uses
/// Avian's first body as that reference side, regardless of classification.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ContactTransfer {
    /// The reference body.
    pub(crate) reference: Entity,
    /// The reference body's collider the point lies on.
    pub(crate) reference_collider: Entity,
    /// The other body.
    pub(crate) other: Entity,
    /// The other body's collider, or `None` for an implicit belt contact with no solver collider.
    pub(crate) other_collider: Option<Entity>,
    /// The contact point in world space, as the narrow phase found it at the step's start (m).
    pub(crate) point: Vec3,
    /// The contact normal, unit, from the reference body into the other body.
    pub(crate) normal: Vec3,
    /// The momentum the solver moved into the other body through this point over the step (N·s,
    /// world), normal and friction together; the reference took its opposite.
    pub(crate) impulse: Vec3,
    /// The mass the solver sees along the normal at this point (kg): against a static reference,
    /// the other's own effective mass there, its rotation included.
    pub(crate) effective_mass: f32,
    /// How fast the other closed on the reference along the normal before the solve (m/s);
    /// negative when the two were parting.
    pub(crate) closing_speed: f32,
    /// The solver's dynamic friction coefficient here, combined from both colliders: the most
    /// tangential impulse it moves per unit of normal impulse. An implicit belt contact has no
    /// solver row, so none; its grip is the belt contact law's, which the patches report.
    pub(crate) friction: f32,
}

/// A structural side's allocation-free view of a canonical solver manifold.
#[derive(Clone, Copy)]
pub(crate) struct ContactGroup<'a> {
    points: &'a [ContactTransfer],
    reversed: bool,
}

impl ContactGroup<'_> {
    pub(crate) fn first(&self) -> Option<ContactTransfer> {
        self.iter().next()
    }

    pub(crate) fn iter(&self) -> Self {
        *self
    }
}

impl Iterator for ContactGroup<'_> {
    type Item = ContactTransfer;

    fn next(&mut self) -> Option<Self::Item> {
        let (&point, rest) = self.points.split_first()?;
        self.points = rest;
        Some(if self.reversed {
            ContactTransfer {
                reference: point.other,
                reference_collider: point
                    .other_collider
                    .expect("only solver manifolds have an opposite structural view"),
                other: point.reference,
                other_collider: Some(point.reference_collider),
                normal: -point.normal,
                impulse: -point.impulse,
                ..point
            }
        } else {
            point
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.points.len(), Some(self.points.len()))
    }
}

impl ExactSizeIterator for ContactGroup<'_> {}

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

/// A point whose body sleeps against a static structure, a solver pair's or a tank's belt point
/// (module doc): its transfer over the last step its body moved, and that step's length (s).
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
/// axes. The transfer witness checks point effective masses against this one snapshot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BodyInertia {
    pub(crate) centre: Vec3,
    pub(crate) inverse_mass: Vec3,
    pub(crate) inverse_inertia: Mat3,
}

/// Every solver contact's transfer over the last physics step, and every structural resting contact's
/// (module doc).
#[derive(Resource, Debug, Default)]
pub(crate) struct ContactTransfers {
    points: Vec<ContactTransfer>,
    pending: Vec<AppliedContact>,
    /// Implicit contacts by `(structure collider, source body)`, whatever their normals.
    applied: BTreeMap<(Entity, Entity), Vec<ContactTransfer>>,
    #[cfg(test)]
    inertias: BTreeMap<Entity, BodyInertia>,
    resting: Vec<Resting>,
    /// The last step's length, s.
    dt: f32,
    /// The steps summed.
    steps: u64,
    /// One record per constraint: its graph address, first point slot, and structural sides.
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
    structures: [bool; 2],
}

struct PreparedContact {
    at: Kept,
    #[cfg(test)]
    inertias: [(Entity, BodyInertia); 2],
    points: Vec<ContactTransfer>,
    last: Vec<(f32, Vec2)>,
}

type ContactBodies<'w, 's> = Query<
    'w,
    's,
    (
        Option<&'static SolverBodyInertia>,
        &'static RigidBody,
        &'static Position,
        &'static Rotation,
        &'static ComputedCenterOfMass,
    ),
>;

impl ContactTransfers {
    /// Pre-physics staging order, before the solver reading canonicalizes contact geometry.
    pub(crate) fn staged(&self) -> &[AppliedContact] {
        &self.pending
    }

    /// A reading of one step of `dt` s holding `groups`, each one constraint's points: what a
    /// physics step publishes, for a consumer's test.
    #[cfg(test)]
    pub(crate) fn fixture(groups: Vec<Vec<ContactTransfer>>, dt: f32) -> Self {
        let mut transfers = Self {
            dt,
            steps: 1,
            ..Self::default()
        };
        for points in groups {
            transfers.kept.push(Kept {
                color: 0,
                index: transfers.kept.len(),
                first: transfers.points.len(),
                structures: [true, false],
            });
            transfers.points.extend(points);
        }
        transfers
    }

    /// Stage a fixed structure's opposite force this tick, before physics integrates it.
    pub(crate) fn stage(&mut self, contact: AppliedContact) {
        self.pending.push(contact);
    }

    /// A pre-physics sweep removes occupied matter. The staged belt forces on matter that left,
    /// flagged in `departed` in staging order, cease with it; the caller cancels each returned
    /// force on its source before integration.
    pub(crate) fn cancel_implicit(&mut self, departed: &[bool]) -> Vec<AppliedContact> {
        assert_eq!(
            departed.len(),
            self.pending.len(),
            "a sweep judges every staged contact once"
        );
        let mut flags = departed.iter();
        let (cancelled, retained) = self
            .pending
            .drain(..)
            .partition(|_| *flags.next().expect("one flag per staged contact"));
        self.pending = retained;
        cancelled
    }

    /// Before integration, move staged contacts from retired colliders to the colliders covering
    /// the same standing matter: each `(index, collider)` names one by its staging order.
    pub(crate) fn readdress(&mut self, moves: impl IntoIterator<Item = (usize, Entity)>) {
        for (index, collider) in moves {
            self.pending[index].structure_collider = collider;
        }
    }

    /// The source inertia recorded with this step's contacts, shared across all its points.
    #[cfg(test)]
    pub(crate) fn inertia(&self, body: Entity) -> BodyInertia {
        *self
            .inertias
            .get(&body)
            .expect("every body in this step's contacts has its inertia recorded with them")
    }

    /// The points of every body asleep against a static structure, each once, in the order their
    /// bodies fell asleep.
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

    /// Contact forces for this step, in world coordinates. The solver already exchanged the
    /// impulse; structure checks receive its mean force without another launch budget.
    #[cfg(test)]
    pub(crate) fn presses(&self) -> impl Iterator<Item = (ContactTransfer, Vec3)> {
        self.groups()
            .flatten()
            .map(|point| (point, -point.impulse / self.dt))
    }

    /// Structural contact groups in solver order, then canonical implicit-contact order.
    pub(crate) fn groups(&self) -> impl Iterator<Item = ContactGroup<'_>> {
        self.structural_solver_groups()
            .chain(self.applied.values().map(|points| ContactGroup {
                points,
                reversed: false,
            }))
    }

    fn structural_solver_groups(&self) -> impl Iterator<Item = ContactGroup<'_>> {
        self.solved().flat_map(|(kept, points)| {
            kept.structures
                .into_iter()
                .enumerate()
                .filter_map(move |(side, structural)| {
                    structural.then_some(ContactGroup {
                        points,
                        reversed: side == 1,
                    })
                })
        })
    }

    /// Every solver constraint once, oriented from Avian's first body into its second.
    /// Implicit belt contacts are already applied forces and do not enter this stream.
    pub(crate) fn solver_groups(&self) -> impl Iterator<Item = &[ContactTransfer]> {
        self.solved().map(|(_, points)| points)
    }

    fn solved(&self) -> impl Iterator<Item = (&Kept, &[ContactTransfer])> {
        let ends = self
            .kept
            .iter()
            .skip(1)
            .map(|kept| kept.first)
            .chain([self.points.len()]);
        self.kept
            .iter()
            .zip(ends)
            .map(|(kept, end)| (kept, &self.points[kept.first..end]))
    }
}

/// A point's accumulated friction impulse, in its constraint's tangent basis (N·s); none without
/// friction.
fn friction(point: &ContactConstraintPoint) -> Vec2 {
    point
        .tangent_part
        .as_ref()
        .map_or(Vec2::ZERO, |part| part.impulse)
}

/// Each constraint reads immutable solver state; only collection assigns global point slots.
fn prepare_constraint(
    color: usize,
    index: usize,
    constraint: &ContactConstraint,
    contacts: &ContactGraph,
    structures: &Query<&RigidBody, With<StructureBody>>,
    #[cfg(test)] bodies: &ContactBodies,
) -> PreparedContact {
    let (_, pair) = contacts
        .get_by_id(constraint.contact_id)
        .expect("a contact constraint is built from a pair in the contact graph");
    let manifold = &pair.manifolds[constraint.manifold_index];
    assert_eq!(
        manifold.points.len(),
        constraint.points.len(),
        "a contact constraint holds its manifold's points one for one"
    );
    #[cfg(test)]
    let inertias = [constraint.body1, constraint.body2].map(|body| {
        let (solver, kind, position, rotation, com) = bodies
            .get(body)
            .expect("each contact side is a rigid body with its required pose and centre of mass");
        let inertia = if kind.is_static() {
            &SolverBodyInertia::DUMMY
        } else {
            solver.expect("Avian supplies solver inertia for an active moving contact body")
        };
        (
            body,
            BodyInertia {
                centre: position.0 + rotation.0 * com.0,
                inverse_mass: inertia.effective_inv_mass(),
                inverse_inertia: inertia.effective_inv_angular_inertia().to_mat3(),
            },
        )
    });
    let mut points = Vec::with_capacity(constraint.points.len());
    let mut last = Vec::with_capacity(constraint.points.len());
    for (point, contact) in constraint.points.iter().zip(&manifold.points) {
        points.push(ContactTransfer {
            reference: constraint.body1,
            reference_collider: pair.collider1,
            other: constraint.body2,
            other_collider: Some(pair.collider2),
            point: contact.point,
            normal: constraint.normal,
            impulse: Vec3::ZERO,
            effective_mass: point.normal_part.effective_mass,
            closing_speed: -point.normal_speed,
            friction: constraint.friction,
        });
        last.push((point.normal_part.impulse, friction(point)));
    }
    PreparedContact {
        at: Kept {
            color,
            index,
            first: 0,
            structures: [constraint.body1, constraint.body2].map(|body| structures.contains(body)),
        },
        #[cfg(test)]
        inertias,
        points,
        last,
    }
}

/// What keeps a press at rest: Avian holding its pair asleep and touching, its body frozen
/// (`debris::freeze`), which left Avian's contact graph as it turned static and holds the press
/// it slept with until it thaws, or, for an implicit belt point, its tank asleep.
#[derive(bevy::ecs::system::SystemParam)]
struct Rests<'w, 's> {
    contacts: Res<'w, ContactGraph>,
    frozen: Query<'w, 's, (), With<crate::debris::freeze::Frozen>>,
    sleeping: Query<'w, 's, (), With<Sleeping>>,
    structures: Query<'w, 's, &'static RigidBody, With<StructureBody>>,
}

impl Rests<'_, '_> {
    /// Only static structural receivers consume cached presses, and a frozen piece is a moving
    /// group at rest. Moving groups read awake landings through debris; their sleeping contacts
    /// have no structural load.
    fn receives(&self, reference: Entity) -> bool {
        self.structures
            .get(reference)
            .is_ok_and(RigidBody::is_static)
            && !self.frozen.contains(reference)
    }

    fn hold(&self, transfer: &ContactTransfer) -> bool {
        self.frozen.contains(transfer.other)
            || match transfer.other_collider {
                Some(collider) => self
                    .contacts
                    .get(transfer.reference_collider, collider)
                    .is_some_and(|(edge, pair)| edge.is_sleeping() && pair.is_touching()),
                None => self.sleeping.contains(transfer.other),
            }
    }
}

/// Before the substeps: retain static structural rests and record each solver constraint once.
/// `time` is the physics clock Avian runs its schedule on.
fn keep(
    stages: Option<Res<crate::cost::stages::Stages>>,
    graph: Res<ConstraintGraph>,
    rests: Rests,
    bodies: ContactBodies,
    time: Res<Time>,
    mut transfers: ResMut<ContactTransfers>,
) {
    let _contacts =
        crate::cost::stages::Stages::start(stages.as_deref(), crate::cost::stages::Stage::Contacts);
    let groups: Vec<_> = transfers.groups().collect();
    let newly_resting = groups.par_splat_map(ComputeTaskPool::get(), None, |_, groups| {
        groups
            .iter()
            .filter(|group| {
                rests.receives(
                    group
                        .first()
                        .expect("contact groups are nonempty")
                        .reference,
                )
            })
            .flat_map(|group| group.iter())
            .filter(|point| rests.hold(point))
            .map(|transfer| Resting {
                transfer,
                dt: transfers.dt,
            })
            .collect::<Vec<_>>()
    });
    // A pair that woke or ended, or a tank that woke, drops its rest. Neither a sleeping pair nor
    // a sleeping tank adds a point to the step, so retained and newly resting points are disjoint.
    let retained = transfers
        .resting
        .par_splat_map(ComputeTaskPool::get(), None, |_, chunk| {
            chunk
                .iter()
                .filter(|rest| rests.hold(&rest.transfer))
                .copied()
                .collect::<Vec<_>>()
        });
    let ContactTransfers {
        points,
        #[cfg(test)]
        inertias,
        resting,
        dt,
        steps,
        kept,
        last,
        pending,
        applied,
    } = &mut *transfers;
    resting.clear();
    resting.extend(retained.into_iter().flatten());
    resting.extend(newly_resting.into_iter().flatten());
    if let Some(stages) = stages.as_deref() {
        use crate::cost::stages::Work;
        stages.add(Work::RestingStaticPoints, resting.len() as u64);
        stages.add(Work::RestingMovingPoints, 0);
    }
    points.clear();
    #[cfg(test)]
    inertias.clear();
    kept.clear();
    last.clear();
    *dt = time.delta_secs();
    *steps += 1;
    let constraints: Vec<_> = graph
        .colors
        .iter()
        .enumerate()
        .flat_map(|(color, colored)| {
            colored
                .contact_constraints
                .iter()
                .enumerate()
                .map(move |(index, constraint)| (color, index, constraint))
        })
        .collect();
    let prepared = constraints.par_splat_map(ComputeTaskPool::get(), None, |_, constraints| {
        constraints
            .iter()
            .map(|&(color, index, constraint)| {
                prepare_constraint(
                    color,
                    index,
                    constraint,
                    &rests.contacts,
                    &rests.structures,
                    #[cfg(test)]
                    &bodies,
                )
            })
            .collect::<Vec<_>>()
    });
    // Constraint order owns publication; workers never choose accumulator offsets or map order.
    for mut contact in prepared.into_iter().flatten() {
        contact.at.first = points.len();
        kept.push(contact.at);
        #[cfg(test)]
        for (body, inertia) in contact.inertias {
            inertias.entry(body).or_insert(inertia);
        }
        points.extend(contact.points);
        last.extend(contact.last);
    }
    if let Some(stages) = stages.as_deref() {
        stages.add(crate::cost::stages::Work::ContactRecords, kept.len() as u64);
    }
    applied.clear();
    // Canonical staging order fixes the floating sums.
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
    let step = *dt;
    let prepared = pending.par_splat_map(ComputeTaskPool::get(), None, |_, pending| {
        pending.iter().map(|contact| {
            let (_, kind, ..) = bodies.get(contact.structure)
                .expect("an implicit structural recipient survives until the contact reading");
            assert!(!kind.is_dynamic(), "the belt's implicit contacts land only on fixed construction; a moving group meets the belt's segments in the solver");
            let (solver, _, position, rotation, com) = bodies.get(contact.body).expect(
                "a source that applied an implicit contact remains a dynamic body through this physics step");
            let solver = solver.expect("an implicit contact source is an active moving body");
            let inertia = BodyInertia {
                centre: position.0 + rotation.0 * com.0,
                inverse_mass: solver.effective_inv_mass(),
                inverse_inertia: solver.effective_inv_angular_inertia().to_mat3(),
            };
            let arm = (contact.point - inertia.centre).cross(contact.normal);
            let inverse = contact.normal.dot(inertia.inverse_mass * contact.normal)
                + arm.dot(inertia.inverse_inertia * arm);
            (inertia, ContactTransfer {
                reference: contact.structure,
                reference_collider: contact.structure_collider,
                other: contact.body,
                other_collider: None,
                point: contact.point,
                normal: contact.normal,
                impulse: contact.force * step,
                effective_mass: if inverse > 0.0 { inverse.recip() } else { 0.0 },
                closing_speed: contact.closing_speed,
                friction: 0.0,
            })
        }).collect::<Vec<_>>()
    });
    pending.clear();
    for (_inertia, contact) in prepared.into_iter().flatten() {
        #[cfg(test)]
        inertias.entry(contact.other).or_insert(_inertia);
        applied
            .entry((contact.reference_collider, contact.other))
            .or_default()
            .push(contact);
    }
}

/// Substeps depend on the preceding reading of each point; separate constraints own disjoint
/// accumulator slices. All workers finish before the next solver pass can change its inputs.
fn update_contacts(
    pool: &TaskPool,
    kept: &[Kept],
    points: &mut [ContactTransfer],
    last: &mut [(f32, Vec2)],
    update: impl Fn(&Kept, &mut [ContactTransfer], &mut [(f32, Vec2)]) + Send + Sync,
) {
    let total = points.len();
    let (mut points, mut last) = (points, last);
    let mut slots = Vec::with_capacity(kept.len());
    for (index, at) in kept.iter().enumerate() {
        let end = kept.get(index + 1).map_or(total, |next| next.first);
        let count = end - at.first;
        let (owned_points, remaining_points) = points.split_at_mut(count);
        let (owned_last, remaining_last) = last.split_at_mut(count);
        slots.push((at, owned_points, owned_last));
        (points, last) = (remaining_points, remaining_last);
    }
    slots.par_splat_map_mut(pool, None, |_, slots| {
        for (at, points, last) in slots {
            update(at, points, last);
        }
    });
}

/// After a substep's relax pass: the momentum the substep moved through every kept point.
fn add_substep(
    graph: Res<ConstraintGraph>,
    config: Res<SolverConfig>,
    mut transfers: ResMut<ContactTransfers>,
    stages: Option<Res<crate::cost::stages::Stages>>,
) {
    let _contacts =
        crate::cost::stages::Stages::start(stages.as_deref(), crate::cost::stages::Stage::Contacts);
    let unapplied = 1.0 - config.warm_start_coefficient;
    let ContactTransfers {
        points, kept, last, ..
    } = &mut *transfers;
    update_contacts(
        ComputeTaskPool::get(),
        kept,
        points,
        last,
        |constraint_at, points, last| {
            let constraint =
                &graph.colors[constraint_at.color].contact_constraints[constraint_at.index];
            let [tangent1, tangent2] = constraint.tangent_directions();
            for (slot, point) in constraint.points.iter().enumerate() {
                let (normal_now, friction_now) = (point.normal_part.impulse, friction(point));
                let (normal_before, friction_before) = last[slot];
                let normal = normal_now - unapplied * normal_before;
                let tangent = friction_now - unapplied * friction_before;
                points[slot].impulse +=
                    normal * constraint.normal + tangent.x * tangent1 + tangent.y * tangent2;
                last[slot] = (normal_now, friction_now);
            }
        },
    );
}

/// After restitution: the increment it added along every kept point's normal.
fn add_restitution(
    graph: Res<ConstraintGraph>,
    mut transfers: ResMut<ContactTransfers>,
    stages: Option<Res<crate::cost::stages::Stages>>,
) {
    let _contacts =
        crate::cost::stages::Stages::start(stages.as_deref(), crate::cost::stages::Stage::Contacts);
    let ContactTransfers {
        points, kept, last, ..
    } = &mut *transfers;
    update_contacts(
        ComputeTaskPool::get(),
        kept,
        points,
        last,
        |constraint_at, points, last| {
            let constraint =
                &graph.colors[constraint_at.color].contact_constraints[constraint_at.index];
            for (slot, point) in constraint.points.iter().enumerate() {
                let normal_now = point.normal_part.impulse;
                points[slot].impulse += (normal_now - last[slot].0) * constraint.normal;
                last[slot].0 = normal_now;
            }
        },
    );
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
        Physics, PhysicsPlugins, Restitution, RigidBody, SleepingDisabled,
    };
    use bevy::time::TimeUpdateStrategy;

    use super::*;

    /// A moving structure meets the belt through the solver, so an implicit contact on one is a
    /// bug. Structural readers rely on every implicit recipient being fixed construction.
    /// FIXTURE: two separated cubes, an explicitly staged force on the moving one.
    #[test]
    #[should_panic(expected = "the belt's implicit contacts land only on fixed construction")]
    fn an_implicit_contact_on_a_moving_structure_is_a_bug() {
        let mut app = world(1.0);
        let receiver = cube(&mut app, Vec3::ZERO, Vec3::ZERO);
        app.world_mut().entity_mut(receiver).insert(StructureBody);
        let source = cube(&mut app, Vec3::Y * 5.0, Vec3::ZERO);
        app.update();
        app.world_mut()
            .resource_mut::<ContactTransfers>()
            .stage(AppliedContact {
                structure: receiver,
                structure_collider: receiver,
                body: source,
                point: Vec3::ZERO,
                normal: Vec3::Y,
                force: Vec3::Y * 1000.0,
                closing_speed: 1.0,
            });
        app.update();
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
    /// x = −0.25, as a construction holds its elements (fixture data).
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
        for point in app
            .world()
            .resource::<ContactTransfers>()
            .groups()
            .flatten()
        {
            if point.other == body {
                assert_eq!(point.reference, structure, "the point names its structure");
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

    /// Fixture: frictionless boxes with no restitution or gravity, approaching one broad support
    /// or two separate supports. All dimensions, gaps, speeds and density are fixture data.
    fn landing(size: Vec3, supports: usize, gap: f32, speed: f32) -> (App, Entity) {
        let mut app = world(1.0);
        for support in 0..supports {
            let (x, width) = if supports == 1 {
                (0.0, 4.0)
            } else {
                (-0.75 + support as f32 * 1.5, 0.5)
            };
            app.world_mut().spawn((
                Transform::from_xyz(x, -0.1, 0.0),
                RigidBody::Static,
                Collider::cuboid(width, 0.2, 4.0),
                Friction::new(0.0),
                Restitution::new(0.0),
            ));
        }
        let body = app
            .world_mut()
            .spawn((
                Transform::from_xyz(0.0, size.y / 2.0 + gap, 0.0),
                RigidBody::Dynamic,
                Collider::cuboid(size.x, size.y, size.z),
                ColliderDensity(1000.0),
                Friction::new(0.0),
                Restitution::new(0.0),
                LinearVelocity(Vec3::NEG_Y * speed),
                SleepingDisabled,
            ))
            .id();
        (app, body)
    }

    /// The game's environment destruction casts need unilateral presses even when multiple
    /// points or pairs share a stop, and no force from an unused speculative point. The normal
    /// stop estimate can retain some compression; these bounds are not an exact blow/press split.
    #[test]
    fn solver_press_is_unilateral_for_shared_stops_and_unused_points() {
        use crate::construction::breakage::pushes;
        // DERIVED: a symmetric four-corner thin-box face stop retains at most 3/7 of its
        // normal impulse. Shared stops have only the pointwise [0, transferred compression]
        // bound. A column's solo-stop estimates exceed its shares; an unused point moves zero.
        for (name, size, supports, gap, speed, replay_bound) in [
            (
                "flat four-point landing",
                Vec3::new(2.0, 0.25, 1.0),
                1,
                0.01,
                2.0,
                3.0 / 7.0,
            ),
            (
                "two-contact landing",
                Vec3::new(2.0, 0.25, 1.0),
                2,
                0.01,
                2.0,
                1.0,
            ),
            ("column end-on", Vec3::new(0.5, 2.0, 0.5), 1, 0.01, 2.0, 0.0),
            (
                "separated speculative point",
                Vec3::ONE,
                1,
                0.004,
                0.001,
                0.0,
            ),
        ] {
            let (mut app, body) = landing(size, supports, gap, speed);
            let mut observed = false;
            // Resolution: f32 iterative-solver allowance, relative to fixture incoming momentum.
            let tolerance = 0.01 * size.element_product() * 1000.0 * speed;
            for _ in 0..4 {
                app.update();
                let reading = app.world().resource::<ContactTransfers>();
                let groups: Vec<_> = reading.solver_groups().collect();
                if groups.is_empty() {
                    continue;
                } // Free approach before narrow-phase admission.
                if !observed {
                    assert_eq!(
                        groups.len(),
                        supports,
                        "{name}: every support shares the stop"
                    );
                    assert!(
                        groups.iter().all(|points| points.len() == 4),
                        "{name}: flat manifolds"
                    );
                    observed = true;
                }
                for &point in groups.iter().flat_map(|points| points.iter()) {
                    // Isolate a real solver point so opposing errors cannot cancel in the sum.
                    let single = ContactTransfers::fixture(vec![vec![point]], reading.dt());
                    let normal =
                        pushes(&single, |_| false)[&point.other].dot(point.normal) * reading.dt();
                    assert!(
                        normal >= -tolerance,
                        "{name}: a point cannot pull: {normal}"
                    );
                    assert!(
                        normal <= point.impulse.dot(point.normal).max(0.0) + tolerance,
                        "{name}: a point cannot invent compression"
                    );
                }
                let force = pushes(reading, |_| false)[&body];
                let received: Vec3 = groups
                    .iter()
                    .flat_map(|points| points.iter())
                    .map(|point| {
                        if point.other == body {
                            point.impulse
                        } else {
                            -point.impulse
                        }
                    })
                    .sum();
                println!("MEASURED {name}: impulse={received:?}, press={force:?}");
                assert!(
                    force.length() * reading.dt() <= replay_bound * received.length() + tolerance,
                    "{name}: normal stop estimate exceeds its stated bound: {force:?}"
                );
                if speed < 0.01 {
                    assert_eq!(received, Vec3::ZERO, "the speculative point stays unused");
                    assert_eq!(force, Vec3::ZERO, "an unused contact supplies no drive");
                    assert!(
                        app.world().get::<Position>(body).unwrap().y > size.y / 2.0,
                        "the speculative body remains separated"
                    );
                }
            }
            assert!(
                observed,
                "{name}: the solver must admit the fixture's contacts"
            );
        }
    }

    /// The game's environment destruction casts must retain a sustained press after removing
    /// the approach estimate. Both a four-point manifold and two separate supports transmit the
    /// applied force, independent of warm-start scaling; velocity carries no continuing blow.
    #[test]
    fn solver_press_transmits_a_steady_force() {
        use crate::construction::breakage::pushes;
        // Fixture: a flat slab held against its supports by 5 kN, kept awake as a driven body.
        let force = 5000.0;
        for supports in [1, 2] {
            for warm_start in [1.0, 0.5] {
                let (mut app, body) = landing(Vec3::new(2.0, 0.25, 1.0), supports, 0.0, 0.0);
                app.world_mut()
                    .resource_mut::<SolverConfig>()
                    .warm_start_coefficient = warm_start;
                app.world_mut()
                    .entity_mut(body)
                    .insert(ConstantForce::new(0.0, -force, 0.0));
                for tick in 0..60 {
                    app.update();
                    if tick >= 20 {
                        let reading = app.world().resource::<ContactTransfers>();
                        let press = pushes(reading, |_| false)[&body];
                        // Resolution: 1% relative allowance for iterative solver convergence.
                        assert!(
                            (press - Vec3::Y * force).length() <= 0.01 * force,
                            "{supports} supports, w {warm_start}, tick {tick}: {press:?} against {force}"
                        );
                    }
                }
            }
        }
    }

    /// The game's environment destruction sweep relies on every solver contact, independently
    /// of structural reporting. Missing plain-body contacts or counting both structural sides
    /// twice breaks the equality with the source's actual momentum change.
    #[test]
    fn solver_inputs_do_not_depend_on_structure_markers() {
        for structural in [false, true] {
            let mut app = world(1.0);
            let fixed = wall(&mut app);
            let moving = cube(&mut app, Vec3::new(-1.3, 0.0, 0.0), Vec3::X * 3.0);
            if structural {
                app.world_mut().entity_mut(moving).insert(StructureBody);
            } else {
                app.world_mut().entity_mut(fixed).remove::<StructureBody>();
            }
            app.update();
            let mut observed = false;
            for _ in 0..24 {
                let momentum = |app: &App| {
                    app.world().get::<ComputedMass>(moving).unwrap().value()
                        * app.world().get::<LinearVelocity>(moving).unwrap().0
                };
                let before = momentum(&app);
                app.update();
                let reading = app.world().resource::<ContactTransfers>();
                let received: Vec3 = reading
                    .solver_groups()
                    .flatten()
                    .map(|point| {
                        if point.other == moving {
                            point.impulse
                        } else {
                            assert_eq!(point.reference, moving, "the fixture has one contact pair");
                            -point.impulse
                        }
                    })
                    .sum();
                // Resolution: f32 accumulation against the fixture's initial 3000 N s momentum.
                assert!((momentum(&app) - before - received).length() <= 1e-4 * 3000.0);
                if !structural {
                    assert_eq!(
                        reading.groups().count(),
                        0,
                        "plain bodies have no structural report"
                    );
                    assert!(reading.resting().is_empty());
                }
                observed |= received != Vec3::ZERO;
            }
            assert!(
                observed,
                "the solver stream must include the contact with markers {structural}"
            );
        }
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

    /// The game's environment destruction readers need every structural side's whole manifold,
    /// while the sweep needs each solver constraint once. Reversing a side changes only its
    /// body/collider addresses and impulse/normal signs, never its point order or scalars.
    #[test]
    fn structural_views_match_each_solver_constraint_in_both_body_orders() {
        // Fixture: a cube meets a wall either by approach or by a steady force from rest.
        let mut orders = std::collections::BTreeSet::new();
        for press in [false, true] {
            for markers in [[false, false], [true, false], [false, true], [true, true]] {
                let mut app = world(1.0);
                let (fixed, moving) = if press {
                    let moving = cube(&mut app, Vec3::new(-0.75, 0.0, 0.0), Vec3::ZERO);
                    app.update();
                    let fixed = wall(&mut app);
                    app.world_mut()
                        .entity_mut(moving)
                        .insert(ConstantForce::new(5000.0, 0.0, 0.0));
                    (fixed, moving)
                } else {
                    let fixed = wall(&mut app);
                    let moving = cube(&mut app, Vec3::new(-1.3, 0.0, 0.0), Vec3::X * 3.0);
                    (fixed, moving)
                };
                if !markers[0] {
                    app.world_mut().entity_mut(fixed).remove::<StructureBody>();
                }
                if markers[1] {
                    app.world_mut().entity_mut(moving).insert(StructureBody);
                }
                app.world_mut().entity_mut(moving).insert(SleepingDisabled);
                let mut observed = false;
                for _ in 0..24 {
                    app.update();
                    let reading = app.world().resource::<ContactTransfers>();
                    let graph = app.world().resource::<ConstraintGraph>();
                    let constraints: Vec<_> = graph
                        .colors
                        .iter()
                        .flat_map(|color| &color.contact_constraints)
                        .collect();
                    let canonical: Vec<_> = reading.solver_groups().collect();
                    assert_eq!(
                        canonical.len(),
                        constraints.len(),
                        "the sweep reads each constraint once"
                    );
                    let contacts = app.world().resource::<ContactGraph>();
                    let mut expected = Vec::new();
                    for (points, constraint) in canonical.into_iter().zip(constraints) {
                        assert!(!points.is_empty());
                        assert_eq!(points.len(), constraint.points.len());
                        let (_, pair) = contacts.get_by_id(constraint.contact_id).unwrap();
                        let manifold = &pair.manifolds[constraint.manifold_index];
                        for ((point, solver), contact) in
                            points.iter().zip(&constraint.points).zip(&manifold.points)
                        {
                            assert_eq!(
                                (point.reference, point.other),
                                (constraint.body1, constraint.body2)
                            );
                            assert_eq!(
                                (point.reference_collider, point.other_collider),
                                (pair.collider1, Some(pair.collider2))
                            );
                            assert_eq!(point.point, contact.point);
                            assert_eq!(point.normal, constraint.normal);
                            assert_eq!(point.effective_mass, solver.normal_part.effective_mass);
                            assert_eq!(point.closing_speed, -solver.normal_speed);
                        }
                        orders.insert(constraint.body1 == fixed);
                        observed = true;
                        for reference_first in [true, false] {
                            let reference = if reference_first {
                                constraint.body1
                            } else {
                                constraint.body2
                            };
                            if (reference == fixed && markers[0])
                                || (reference == moving && markers[1])
                            {
                                expected.push(
                                    points
                                        .iter()
                                        .map(|&point| {
                                            if reference_first {
                                                point
                                            } else {
                                                ContactTransfer {
                                                    reference: point.other,
                                                    reference_collider: point
                                                        .other_collider
                                                        .unwrap(),
                                                    other: point.reference,
                                                    other_collider: Some(point.reference_collider),
                                                    normal: -point.normal,
                                                    impulse: -point.impulse,
                                                    ..point
                                                }
                                            }
                                        })
                                        .collect::<Vec<_>>(),
                                );
                            }
                        }
                    }
                    let actual: Vec<_> = reading
                        .groups()
                        .map(|points| points.collect::<Vec<_>>())
                        .collect();
                    assert_eq!(
                        actual, expected,
                        "structural views preserve the canonical manifold"
                    );
                }
                assert!(observed, "each marker combination meets the wall");
            }
        }
        assert_eq!(orders, [false, true].into_iter().collect());
    }
}
