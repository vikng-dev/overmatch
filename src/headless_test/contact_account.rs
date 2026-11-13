//! Test-time instrument for the game's tank/world contact migration (game state only).
//! Run `contact_account_snag --ignored --nocapture` for one JSON record per fixed tick.
//! Forces are on the tank, in N; moments are about its COM, in N m. `normal_n` is the
//! scalar normal load; `normal[1]` is vertical support. Tangent includes solver friction
//! or belt traction; rolling is separate. Rigid points use the side of their body-local x.
//! The sweep's plastic mass exchange has no surface normal and stays body-owned in `exchange`.
//! This observes motion; its trajectory is deliberately not a pass condition.

use avian3d::dynamics::solver::SolverConfig;
use avian3d::dynamics::solver::constraint_graph::ConstraintGraph;
use avian3d::dynamics::solver::contact::ContactConstraintPoint;
use avian3d::dynamics::solver::schedule::{SolverSystems, SubstepSchedule, SubstepSolverSystems};
use avian3d::prelude::*;
use serde::Serialize;

use super::*;
use crate::construction::breakage::SweepAccount;
use crate::track::bump_stop::{BumpStop, SidePlate};
use crate::track::sim::TrackContacts;

const PATHS: [&str; 5] = ["terrain", "collider", "slice", "stop", "hull"];

#[derive(Clone, Copy, Default, Serialize)]
struct Load {
    normal_n: f32,
    normal: [f32; 3],
    tangent: [f32; 3],
    rolling: [f32; 3],
    moment: [f32; 3],
}

impl Load {
    fn add(&mut self, normal_n: f32, normal: Vec3, tangent: Vec3, rolling: Vec3, lever: Vec3) {
        self.normal_n += normal_n;
        self.normal = (Vec3::from_array(self.normal) + normal).to_array();
        self.tangent = (Vec3::from_array(self.tangent) + tangent).to_array();
        self.rolling = (Vec3::from_array(self.rolling) + rolling).to_array();
        self.moment =
            (Vec3::from_array(self.moment) + lever.cross(normal + tangent + rolling)).to_array();
    }

    fn force(&self) -> Vec3 {
        Vec3::from_array(self.normal)
            + Vec3::from_array(self.tangent)
            + Vec3::from_array(self.rolling)
    }
}

struct SolverPoint {
    color: usize,
    constraint: usize,
    slot: usize,
    side: usize,
    path: usize,
    sign: f32,
    lever: Vec3,
    last: (f32, Vec2),
}

#[derive(Resource)]
struct ContactAccount {
    body: Entity,
    dt: f32,
    forward: Vec3,
    sides: [[Load; 5]; 2],
    exchange: Vec3,
    exchange_moment: Vec3,
    points: Vec<SolverPoint>,
}

impl ContactAccount {
    fn force(&self) -> Vec3 {
        self.sides.iter().flatten().map(Load::force).sum::<Vec3>() + self.exchange
    }
}

fn mount(app: &mut App, body: Entity) {
    app.insert_resource(ContactAccount {
        body,
        dt: 0.0,
        forward: Vec3::ZERO,
        sides: [[Load::default(); 5]; 2],
        exchange: Vec3::ZERO,
        exchange_moment: Vec3::ZERO,
        points: Vec::new(),
    })
    .add_systems(
        PhysicsSchedule,
        (
            prepare.in_set(SolverSystems::PreSubstep),
            restitution
                .after(SolverSystems::Restitution)
                .before(SolverSystems::Finalize),
        ),
    )
    .add_systems(
        SubstepSchedule,
        substep
            .after(SubstepSolverSystems::Relax)
            .before(SubstepSolverSystems::Damping),
    );
}

fn friction(point: &ContactConstraintPoint) -> Vec2 {
    // No tangent part means a frictionless constraint.
    point
        .tangent_part
        .as_ref()
        .map_or(Vec2::ZERO, |p| p.impulse)
}

fn side(rotation: Quat, offset: Vec3) -> usize {
    usize::from((rotation.inverse() * offset).x >= 0.0)
}

fn prepare(
    mut account: ResMut<ContactAccount>,
    time: Res<Time>,
    graph: Res<ConstraintGraph>,
    contacts: Res<ContactGraph>,
    bodies: Query<(&Position, &Rotation, &ComputedCenterOfMass, &TrackContacts)>,
    colliders: Query<(Has<SidePlate>, Has<BumpStop>)>,
    sweep: Res<SweepAccount>,
) {
    account.dt = time.delta_secs();
    account.sides = [[Load::default(); 5]; 2];
    account.exchange = Vec3::ZERO;
    account.exchange_moment = Vec3::ZERO;
    account.points.clear();
    let (position, rotation, com, belt) = bodies
        .get(account.body)
        .expect("the observed body retains its pose, COM and belt contacts");
    let centre = position.0 + rotation.0 * com.0;
    account.forward = rotation.0 * Vec3::NEG_Z;
    for (si, belt) in belt.0.iter().enumerate() {
        for contact in belt {
            account.sides[si][usize::from(contact.collider.is_some())].add(
                contact.load,
                contact.normal * contact.load,
                contact.traction,
                contact.rolling_force,
                contact.point - centre,
            );
        }
    }
    for cancelled in &sweep.cancelled_belts {
        if cancelled.body == account.body {
            let si = side(rotation.0, cancelled.point - position.0);
            let load = -cancelled.force.dot(cancelled.normal);
            let normal = cancelled.normal * load;
            let original = belt.0[si]
                .iter()
                .find(|contact| {
                    contact.collider == Some(cancelled.structure_collider)
                        && contact.point == cancelled.point
                })
                .expect("a cancelled belt force retains its emitted contact telemetry");
            account.sides[si][1].add(
                load,
                normal,
                -cancelled.force - normal + original.rolling_force,
                -original.rolling_force,
                cancelled.point - centre,
            );
        }
    }
    for impulse in &sweep.source_impulses {
        if impulse.body != account.body {
            continue; // Other bodies have independent accounts.
        }
        let dt = account.dt;
        account.exchange += impulse.linear / dt;
        account.exchange_moment += impulse.angular / dt;
    }
    for (color, constraints) in graph.colors.iter().enumerate() {
        for (ci, constraint) in constraints.contact_constraints.iter().enumerate() {
            let (sign, collider) = if constraint.body1 == account.body {
                let (_, pair) = contacts.get_by_id(constraint.contact_id).unwrap();
                (-1.0, pair.collider1)
            } else if constraint.body2 == account.body {
                let (_, pair) = contacts.get_by_id(constraint.contact_id).unwrap();
                (1.0, pair.collider2)
            } else {
                continue; // This constraint transfers no momentum to the observed body.
            };
            let (slice, stop) = colliders
                .get(collider)
                .expect("a solver collider still exists");
            let path = if slice {
                2
            } else if stop {
                3
            } else {
                4
            };
            let (_, pair) = contacts
                .get_by_id(constraint.contact_id)
                .expect("a solver constraint retains its contact graph pair");
            let manifold = &pair.manifolds[constraint.manifold_index];
            assert_eq!(constraint.points.len(), manifold.points.len());
            for (slot, (point, geometry)) in
                constraint.points.iter().zip(&manifold.points).enumerate()
            {
                account.points.push(SolverPoint {
                    color,
                    constraint: ci,
                    slot,
                    side: side(rotation.0, geometry.point - position.0),
                    path,
                    sign,
                    lever: geometry.point - centre,
                    last: (point.normal_part.impulse, friction(point)),
                });
            }
        }
    }
}

fn substep(
    graph: Res<ConstraintGraph>,
    config: Res<SolverConfig>,
    mut account: ResMut<ContactAccount>,
) {
    read_solver(
        &graph,
        &mut account,
        1.0 - config.warm_start_coefficient,
        false,
    );
}

fn restitution(graph: Res<ConstraintGraph>, mut account: ResMut<ContactAccount>) {
    read_solver(&graph, &mut account, 1.0, true);
}

fn read_solver(
    graph: &ConstraintGraph,
    account: &mut ContactAccount,
    unapplied: f32,
    restitution: bool,
) {
    // Avian applies I_now - (1-w) I_previous per substep. Restitution adds only its normal
    // increment. ContactPoint's stored normal_impulse counts solver passes, not momentum.
    let ContactAccount {
        dt, sides, points, ..
    } = account;
    for at in points {
        let constraint = &graph.colors[at.color].contact_constraints[at.constraint];
        let point = &constraint.points[at.slot];
        let normal_now = point.normal_part.impulse;
        let tangent_now = friction(point);
        let normal = (normal_now - unapplied * at.last.0) / *dt;
        let tangent = if restitution {
            Vec2::ZERO
        } else {
            (tangent_now - unapplied * at.last.1) / *dt
        };
        let [t1, t2] = constraint.tangent_directions();
        sides[at.side][at.path].add(
            normal,
            at.sign * constraint.normal * normal,
            at.sign * (t1 * tangent.x + t2 * tangent.y),
            Vec3::ZERO,
            at.lever,
        );
        at.last = (normal_now, tangent_now);
    }
}

/// The migration's instrument must close the tank's linear momentum account. A missed contact,
/// wrong sign, or twice-counted warm start would misidentify the path carrying the vehicle. A
/// tick the tank rests through (asleep before and after it) is not integrated and has no
/// account: its momentum must not change.
/// Fixture: shipped composition, flat world, resting then straight and pivot commands.
#[test]
fn contact_account_closes_momentum() {
    let (mut app, tank) = booted_offline_sim();
    mount(&mut app, tank);
    let mass = app.world().get::<ComputedMass>(tank).unwrap().value();
    let gravity = app.world().resource::<Gravity>().0;
    let resting = |app: &App| app.world().get::<Sleeping>(tank).is_some();
    let mut max_error = 0.0_f32;
    for tick in 0..256 {
        let before = app.world().get::<LinearVelocity>(tank).unwrap().0;
        let rested = resting(&app);
        drive_tick(
            &mut app,
            tank,
            if tick < 64 { 0.0 } else { 1.0 },
            if tick < 128 { 0.0 } else { 1.0 },
        );
        let after = app.world().get::<LinearVelocity>(tank).unwrap().0;
        if rested && resting(&app) {
            assert_eq!(
                after, before,
                "tick {tick}: a resting tank's momentum changed"
            );
            continue;
        }
        let account = app.world().resource::<ContactAccount>();
        let observed = (after - before) * mass;
        let accounted = (account.force() + gravity * mass) * account.dt;
        let error = (observed - accounted).length();
        max_error = max_error.max(error);
        // Resolution: f32 integration and force accumulation, relative to one tick's weight.
        assert!(
            error < gravity.length() * mass * account.dt * 0.001,
            "tick {tick}: momentum {observed}, account {accounted}, error {error} N s"
        );
    }
    println!("MEASURED contact account maximum momentum residual {max_error} N s");
}

/// Observation only: the owner's foundation-crossing stimulus on the shipped map. Fixture
/// coordinates and input times identify the reproduction; no emergent motion is asserted.
#[test]
#[cfg(feature = "bitprobe")]
#[ignore = "operator contact account; prints one record per tick, no trajectory gate"]
fn contact_account_snag() {
    let grid = crate::terrain_grid::tests::shipped_grid();
    let at = crate::terrain_grid::spawn_pos(Some(&grid), Vec2::new(-641.434, 153.384));
    let mut app = booted_sim_on(Some(grid));
    let tank = app
        .world_mut()
        .query_filtered::<Entity, With<Controlled>>()
        .single(app.world())
        .unwrap();
    // Fixture heading uses the operator lever's convention: heading from +Z, hull front -Z.
    let rotation = Quat::from_rotation_y(176.08_f32.to_radians() + std::f32::consts::PI);
    super::drive_load::place(&mut app, tank, at, rotation, 0.0, 1);
    mount(&mut app, tank);
    start_fixed_clock(&mut app);
    let gravity = app.world().resource::<Gravity>().0;
    println!(
        "CONTACT_ACCOUNT_SCHEMA {}",
        serde_json::json!({"paths": PATHS, "sides": ["left", "right"], "units": "N, N m, world XYZ", "tick_hz": FIXED_TICKS_PER_SECOND, "gravity": gravity.to_array()})
    );
    // Fixture: enough reversing time to return from the braking run-out to the foundation.
    for tick in 0..60 * FIXED_TICKS_PER_SECOND {
        let throttle = if tick < 2 * FIXED_TICKS_PER_SECOND {
            0.0
        } else if tick < 24 * FIXED_TICKS_PER_SECOND {
            1.0
        } else {
            -1.0
        };
        let before = app.world().get::<LinearVelocity>(tank).unwrap().0;
        drive_tick(&mut app, tank, throttle, 0.0);
        let a = app.world().resource::<ContactAccount>();
        let pose = app.world().get::<Position>(tank).unwrap().0;
        let velocity = app.world().get::<LinearVelocity>(tank).unwrap().0;
        let mass = app.world().get::<ComputedMass>(tank).unwrap().value();
        let residual = (velocity - before) * mass - (a.force() + gravity * mass) * a.dt;
        println!(
            "CONTACT_ACCOUNT {}",
            serde_json::json!({
                "tick": tick, "position": pose.to_array(), "forward": a.forward.to_array(),
                "velocity": velocity.to_array(), "momentum_residual": residual.to_array(),
                "spin": app.world().get::<AngularVelocity>(tank).unwrap().0.to_array(),
                "mass": mass,
                "sides": a.sides, "exchange": a.exchange.to_array(), "exchange_moment": a.exchange_moment.to_array(),
            })
        );
    }
}
