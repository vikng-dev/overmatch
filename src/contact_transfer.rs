//! What the physics solver moved through each contact between a standing structure and a dynamic
//! body, one physics step at a time: the reading a body's contact load is taken from in the game's
//! environment destruction model (game state only; a contact never doses a tank). The model it
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
//! Avian builds a constraint only where one body is dynamic, and a structure is static, so the
//! other body of every record is dynamic. `ContactPoint::normal_impulse` is not this reading:
//! Avian adds the whole accumulated impulse into it on every solve call, twice per substep, and
//! keeps the friction impulse only as the last substep's, in a basis it does not store. The
//! vendored crate is not patched for any of it.
//!
//! WHAT IT HOLDS. The last physics step's points, in Avian's constraint order, until the next step
//! replaces them: a system in the next tick's `FixedUpdate` reads the step that ended the tick
//! before. Filled on the authority only, where the destruction laws run; a replica's stays empty.

use avian3d::dynamics::solver::SolverConfig;
use avian3d::dynamics::solver::constraint_graph::ConstraintGraph;
use avian3d::dynamics::solver::contact::ContactConstraintPoint;
use avian3d::dynamics::solver::schedule::{SolverSystems, SubstepSchedule, SubstepSolverSystems};
use avian3d::prelude::{ContactGraph, PhysicsSchedule};
use bevy::prelude::*;

use crate::ClientReplica;

/// A body whose construction the destruction laws judge: a building's root, a tree's. Its
/// contacts with dynamic bodies are read into [`ContactTransfers`]. Required by the components
/// that make a body a structure, never inserted by hand.
#[derive(Component, Debug, Default, Clone, Copy)]
pub(crate) struct StructureBody;

/// One contact point between a structure and a dynamic body, over one physics step.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(
    dead_code,
    reason = "no law reads a transfer yet; the contacts-into-buildings task of #235 is its reader"
)]
pub(crate) struct ContactTransfer {
    /// The structure's body.
    pub(crate) structure: Entity,
    /// The structure's collider the point lies on.
    pub(crate) structure_collider: Entity,
    /// The dynamic body.
    pub(crate) body: Entity,
    /// The dynamic body's collider the point lies on.
    pub(crate) body_collider: Entity,
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

/// Every structure contact's transfer over the last physics step (module doc).
#[derive(Resource, Debug, Default)]
pub(crate) struct ContactTransfers {
    points: Vec<ContactTransfer>,
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
    /// The last step's points, in Avian's constraint order.
    #[allow(
        dead_code,
        reason = "no law reads a transfer yet; the contacts-into-buildings task of #235 is its reader"
    )]
    pub(crate) fn points(&self) -> &[ContactTransfer] {
        &self.points
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

/// Before the substeps: a record for every point of every constraint with a structure on one side.
fn keep(
    graph: Res<ConstraintGraph>,
    contacts: Res<ContactGraph>,
    structures: Query<(), With<StructureBody>>,
    mut transfers: ResMut<ContactTransfers>,
) {
    let ContactTransfers { points, kept, last } = &mut *transfers;
    points.clear();
    kept.clear();
    last.clear();
    for (color, colored) in graph.colors.iter().enumerate() {
        for (index, constraint) in colored.contact_constraints.iter().enumerate() {
            let structure_first = match (
                structures.contains(constraint.body1),
                structures.contains(constraint.body2),
            ) {
                (true, false) => true,
                (false, true) => false,
                (false, false) => continue,
                (true, true) => panic!(
                    "contact_transfer: a constraint joins two structures ({:?}, {:?}); a structure \
                     is static, and Avian builds no constraint between static bodies",
                    constraint.body1, constraint.body2
                ),
            };
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
            kept.push(Kept {
                color,
                index,
                first: points.len(),
                sign,
            });
            for (point, contact) in constraint.points.iter().zip(&manifold.points) {
                points.push(ContactTransfer {
                    structure,
                    structure_collider,
                    body,
                    body_collider,
                    point: contact.point,
                    normal: sign * constraint.normal,
                    impulse: Vec3::ZERO,
                    effective_mass: point.normal_part.effective_mass,
                    closing_speed: -point.normal_speed,
                });
                last.push((point.normal_part.impulse, friction(point)));
            }
        }
    }
}

/// After a substep's relax pass: the momentum the substep moved through every kept point.
fn add_substep(
    graph: Res<ConstraintGraph>,
    config: Res<SolverConfig>,
    mut transfers: ResMut<ContactTransfers>,
) {
    let unapplied = 1.0 - config.warm_start_coefficient;
    let ContactTransfers { points, kept, last } = &mut *transfers;
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
    let ContactTransfers { points, kept, last } = &mut *transfers;
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
        Physics, PhysicsPlugins, Restitution, RigidBody, SleepingDisabled,
    };
    use bevy::time::TimeUpdateStrategy;

    use super::*;

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
        for point in app.world().resource::<ContactTransfers>().points() {
            if point.body == body {
                assert_eq!(point.structure, structure, "the point names its structure");
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
