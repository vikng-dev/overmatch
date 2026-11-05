//! What solver contacts and implicit belt contacts moved between a standing structure and a
//! dynamic body, one physics step at a time: the reading a body's contact load is taken from in
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
//! replaces them, with the step's length and a count of the steps summed. A reader retaining
//! [`ContactTransfers::steps`] reads each step once. The structure check receives the whole
//! transfer divided by that step; hull sweeps own the separate plastic exchange before physics.
//!
//! RESTING CONTACTS. A sleeping body leaves the solver, so a body that comes to rest pressing a
//! structure (a pile on a wall, a body Avian's `ConstantForce` holds there: neither wakes it) would
//! read nothing. The reading keeps such a pair's points from the last step the solver moved them,
//! once, while Avian holds the pair sleeping and touching, and drops them when it wakes or ends
//! ([`ContactTransfers::resting`]): its press stands on the structure ([`Resting::press`]) and is no
//! new transfer.
//!
//! IMPLICIT CONTACTS. The belt applies support and traction before physics without collider
//! constraints. Fixed construction receives its opposite reaction through [`ContactTransfers::stage`];
//! a moving structure receives the impulse it actually took through [`ContactTransfers::stage_on_moving`],
//! with its tick-start inertia.
//! These contacts are refreshed by the belt each tick and never enter the solver's sleeping-pair
//! cache.
//!
//! Filled on the authority only, where the destruction laws run; a replica's stays empty.

use std::collections::BTreeMap;

use avian3d::dynamics::solver::SolverConfig;
use avian3d::dynamics::solver::constraint_graph::ConstraintGraph;
use avian3d::dynamics::solver::contact::{ContactConstraint, ContactConstraintPoint};
use avian3d::dynamics::solver::schedule::{SolverSystems, SubstepSchedule, SubstepSolverSystems};
use avian3d::dynamics::solver::solver_body::SolverBodyInertia;
use avian3d::prelude::{
    ComputedCenterOfMass, ContactGraph, PhysicsSchedule, Position, RigidBody, Rotation,
};
use bevy::prelude::*;
use bevy::tasks::{ComputeTaskPool, ParallelSlice, ParallelSliceMut, TaskPool};

use crate::ClientReplica;

/// A body whose construction the destruction laws judge: an anchored or moving chunk group. Its
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
/// axes. The transfer witness checks point effective masses against this one snapshot.
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
    /// Implicit contacts by `(structure collider, source body)`, whatever their normals.
    applied: BTreeMap<(Entity, Entity), Vec<ContactTransfer>>,
    inertias: BTreeMap<Entity, BodyInertia>,
    /// Moving structures under implicit contacts, by entity, with their inertia at staging: those
    /// staged for the next step, and this step's.
    staged_receivers: BTreeMap<Entity, BodyInertia>,
    receivers: BTreeMap<Entity, BodyInertia>,
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

struct PreparedContact {
    at: Kept,
    body: Entity,
    inertia: BodyInertia,
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
    #[cfg(test)]
    pub(crate) fn staged(&self) -> &[AppliedContact] {
        &self.pending
    }

    /// Stage a fixed structure's opposite force this tick, before physics integrates it.
    /// Dynamic receivers must use Self::stage_on_moving with their tick-start inertia.
    pub(crate) fn stage(&mut self, contact: AppliedContact) {
        self.pending.push(contact);
    }

    /// Stage the impulse a moving structure actually took, `receiver` its inertia then: its
    /// receiver provenance is checked when the physics step publishes its transfers.
    pub(crate) fn stage_on_moving(&mut self, contact: AppliedContact, receiver: BodyInertia) {
        self.staged_receivers.insert(contact.structure, receiver);
        self.pending.push(contact);
    }

    /// A pre-physics sweep removes occupied surfaces. Their staged belt forces cease with
    /// those colliders; the caller cancels each returned force on its source before integration.
    pub(crate) fn cancel_implicit(
        &mut self,
        retired: &std::collections::BTreeSet<Entity>,
    ) -> Vec<AppliedContact> {
        let (cancelled, retained) = self
            .pending
            .drain(..)
            .partition(|contact| retired.contains(&contact.structure_collider));
        self.pending = retained;
        cancelled
    }

    /// The source inertia recorded with this step's contacts, shared across all its points.
    #[cfg(test)]
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

    /// Contact forces for this step, in world coordinates. The solver already exchanged the
    /// impulse; structure checks receive its mean force without another launch budget.
    #[cfg(test)]
    pub(crate) fn presses(&self) -> impl Iterator<Item = (&ContactTransfer, Vec3)> {
        self.groups()
            .flatten()
            .map(|point| (point, -point.impulse / self.dt))
    }

    /// Raw contact groups in solver order, then canonical implicit-contact order. Instrumentation
    /// can inspect metadata without evaluating the mass, blow and press laws.
    pub(crate) fn groups(&self) -> impl Iterator<Item = &[ContactTransfer]> {
        let ends = self
            .kept
            .iter()
            .skip(1)
            .map(|kept| kept.first)
            .chain([self.points.len()]);
        self.kept
            .iter()
            .zip(ends)
            .map(|(kept, end)| &self.points[kept.first..end])
            .chain(self.applied.values().map(Vec::as_slice))
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
    structures: &Query<(), With<StructureBody>>,
    bodies: &ContactBodies,
) -> Vec<PreparedContact> {
    let mut prepared = Vec::new();
    for structure_first in [true, false] {
        if !structures.contains(if structure_first {
            constraint.body1
        } else {
            constraint.body2
        }) {
            continue; // Neither side need be a construction.
        }
        let (_, pair) = contacts
            .get_by_id(constraint.contact_id)
            .expect("a contact constraint is built from a pair in the contact graph");
        let manifold = &pair.manifolds[constraint.manifold_index];
        assert_eq!(
            manifold.points.len(),
            constraint.points.len(),
            "a contact constraint holds its manifold's points one for one"
        );
        let (sign, structure, structure_collider, body, body_collider) = if structure_first {
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
        let (solver, kind, position, rotation, com) = bodies
            .get(body)
            .expect("each contact side is a rigid body with its required pose and centre of mass");
        let inertia = if kind.is_static() {
            &SolverBodyInertia::DUMMY
        } else {
            solver.expect("Avian supplies solver inertia for an active moving contact body")
        };
        let inertia = BodyInertia {
            centre: position.0 + rotation.0 * com.0,
            inverse_mass: inertia.effective_inv_mass(),
            inverse_inertia: inertia.effective_inv_angular_inertia().to_mat3(),
        };
        let mut points = Vec::with_capacity(constraint.points.len());
        let mut last = Vec::with_capacity(constraint.points.len());
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
            });
            last.push((point.normal_part.impulse, friction(point)));
        }

        prepared.push(PreparedContact {
            at: Kept {
                color,
                index,
                first: 0,
                sign,
            },
            body,
            inertia,
            points,
            last,
        });
    }
    prepared
}

/// Before the substeps: the resting pairs brought up to date, then a record for every point of
/// every constraint with a structure on one side. `time` is the physics clock Avian runs its
/// schedule on.
fn keep(
    stages: Option<Res<crate::cost::stages::Stages>>,
    graph: Res<ConstraintGraph>,
    contacts: Res<ContactGraph>,
    structures: Query<(), With<StructureBody>>,
    bodies: ContactBodies,
    time: Res<Time>,
    mut transfers: ResMut<ContactTransfers>,
) {
    let _contacts =
        crate::cost::stages::Stages::start(stages.as_deref(), crate::cost::stages::Stage::Contacts);
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
        staged_receivers,
        receivers,
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
    let retained = resting.par_splat_map(ComputeTaskPool::get(), None, |_, rests| {
        rests
            .iter()
            .filter(|rest| asleep(&rest.transfer))
            .copied()
            .collect::<Vec<_>>()
    });
    let newly_resting = points.par_splat_map(ComputeTaskPool::get(), None, |_, points| {
        points
            .iter()
            .filter(|point| asleep(point))
            .map(|&transfer| Resting { transfer, dt: *dt })
            .collect::<Vec<_>>()
    });
    resting.clear();
    resting.extend(retained.into_iter().flatten());
    resting.extend(newly_resting.into_iter().flatten());
    points.clear();
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
            .flat_map(|&(color, index, constraint)| {
                prepare_constraint(color, index, constraint, &contacts, &structures, &bodies)
            })
            .collect::<Vec<_>>()
    });
    // Constraint order owns publication; workers never choose accumulator offsets or map order.
    for mut contact in prepared.into_iter().flatten() {
        contact.at.first = points.len();
        kept.push(contact.at);
        inertias.entry(contact.body).or_insert(contact.inertia);
        points.extend(contact.points);
        last.extend(contact.last);
    }
    applied.clear();
    *receivers = std::mem::take(staged_receivers);
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
            assert_eq!(kind.is_dynamic(), receivers.contains_key(&contact.structure),
                "a dynamic implicit receiver requires its staged inertia; fixed construction has none");
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
                structure: contact.structure,
                structure_collider: contact.structure_collider,
                body: contact.body,
                body_collider: None,
                point: contact.point,
                normal: contact.normal,
                impulse: contact.force * step,
                effective_mass: if inverse > 0.0 { inverse.recip() } else { 0.0 },
                closing_speed: contact.closing_speed,
            })
        }).collect::<Vec<_>>()
    });
    pending.clear();
    for (inertia, contact) in prepared.into_iter().flatten() {
        inertias.entry(contact.body).or_insert(inertia);
        applied
            .entry((contact.structure_collider, contact.body))
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
                points[slot].impulse += constraint_at.sign
                    * (normal * constraint.normal + tangent.x * tangent1 + tangent.y * tangent2);
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
                points[slot].impulse +=
                    constraint_at.sign * (normal_now - last[slot].0) * constraint.normal;
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

    /// A dynamic implicit recipient must carry the inertia used to size its pair budget.
    /// Structural readers rely on missing receiver state being a bug rather than a fixed-body
    /// budget. FIXTURE: two separated cubes, an explicitly staged force on the moving one.
    #[test]
    #[should_panic(expected = "a dynamic implicit receiver requires its staged inertia")]
    fn a_moving_implicit_receiver_cannot_silently_use_a_fixed_budget() {
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
                        .groups()
                        .flatten()
                        .any(|p| p.structure == moving && p.body == fixed)
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
}
