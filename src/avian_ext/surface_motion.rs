//! Finite-surface Coulomb rows under Avian's graph-colored body ownership. The seam depends only
//! on Bevy and Avian; each mechanical owner supplies its speed and constrained/free mobility.
//! The contact design and observations live in `track-contact.md`.

use avian3d::dynamics::solver::{
    contact::{ContactConstraint, SurfaceFrictionStage},
    solver_body::{SolverBody, SolverBodyInertia, SurfaceCoordinate, SurfaceData, SurfaceScratch},
};
use bevy::math::{DVec2, Vec2, Vec3};

/// An owner re-solves from one interval's start and current contact totals. `push` reports a
/// changed mechanical face; free mobility stays fixed over the interval and bounds each
/// face's constrained mobility from above.
pub(crate) trait SurfaceModel: core::any::Any + Send + Sync + core::fmt::Debug {
    fn clone_model(&self) -> Box<dyn SurfaceModel>;
    fn speed(&self, index: usize) -> f64;
    fn mobility(&self, index: usize) -> f64;
    fn free_mobility(&self, index: usize) -> f64;
    /// Warm application precedes all solve-stage reads, so its intermediate totals need no solve.
    fn warm_push(&mut self, index: usize, impulse: f64);
    fn push(&mut self, index: usize, impulse: f64) -> bool;
}

#[derive(Debug)]
struct Scratch(Box<dyn SurfaceModel>);
impl SurfaceData for Scratch {
    fn clone_box(&self) -> Box<dyn SurfaceData> {
        Box::new(Self(self.0.clone_model()))
    }
}

pub(crate) fn register() {
    avian3d::dynamics::solver::contact::SURFACE_FRICTION.get_or_init(|| solve);
}

pub(crate) fn install(model: impl SurfaceModel, body: &mut SolverBody) {
    body.surface_scratch = Some(SurfaceScratch(Box::new(Scratch(Box::new(model)))));
}

pub(crate) fn surface(body: &SolverBody) -> &dyn SurfaceModel {
    let data: &dyn core::any::Any = body
        .surface_scratch
        .as_ref()
        .expect("a surface owner prepares scratch before contact solving")
        .0
        .as_ref();
    &*data
        .downcast_ref::<Scratch>()
        .expect("the registered seam owns its scratch wrapper")
        .0
}
pub(crate) fn surface_mut(body: &mut SolverBody) -> &mut dyn SurfaceModel {
    let data: &mut dyn core::any::Any = body
        .surface_scratch
        .as_mut()
        .expect("a surface owner prepares scratch before contact solving")
        .0
        .as_mut();
    &mut *data
        .downcast_mut::<Scratch>()
        .expect("the registered seam owns its scratch wrapper")
        .0
}

/// Applied tangential momentum, shared by the load ledger and the headless contact account.
pub(crate) fn tangent_impulse(
    constraint: &ContactConstraint,
    slot: usize,
    previous: Vec2,
    unapplied: f32,
) -> Vec2 {
    let now = constraint.points[slot]
        .tangent_part
        .as_ref()
        .map_or(Vec2::ZERO, |p| p.impulse);
    if constraint.surface_coordinates.iter().any(Option::is_some) {
        now
    } else {
        now - unapplied * previous
    }
}

fn exchange_pair(
    bodies: (&mut SolverBody, &mut SolverBody),
    coordinates: [Option<SurfaceCoordinate>; 2],
    impulse: Vec3,
    applied: &mut [f64; 2],
) -> bool {
    let mut changed = false;
    for (side, (body, coordinate)) in [(bodies.0, coordinates[0]), (bodies.1, coordinates[1])]
        .into_iter()
        .enumerate()
    {
        if let Some(c) = coordinate {
            let next = f64::from(c.direction.dot(impulse));
            changed |= surface_mut(body).push(c.index, next - applied[side]);
            applied[side] = next;
        }
    }
    changed
}

fn product(w: [f64; 3], p: DVec2) -> DVec2 {
    DVec2::new(w[0] * p.x + w[1] * p.y, w[1] * p.x + w[2] * p.y)
}

struct ContactMetric {
    rigid: [f64; 3],
    gradient: DVec2,
    tangents: [Vec3; 2],
    previous: DVec2,
    coordinates: [Option<SurfaceCoordinate>; 2],
}

impl ContactMetric {
    fn evaluate(
        &self,
        bodies: (&SolverBody, &SolverBody),
        p: DVec2,
        free: bool,
    ) -> (DVec2, [f64; 3]) {
        let mut metric = self.rigid;
        let mut gradient = self.gradient + product(self.rigid, p - self.previous);
        for (body, c) in [
            (bodies.0, self.coordinates[0]),
            (bodies.1, self.coordinates[1]),
        ] {
            if let Some(c) = c {
                let state = surface(body);
                let q = DVec2::new(
                    f64::from(c.direction.dot(self.tangents[0])),
                    f64::from(c.direction.dot(self.tangents[1])),
                );
                gradient += q * state.speed(c.index);
                let inverse = if free {
                    state.free_mobility(c.index)
                } else {
                    state.mobility(c.index)
                };
                metric[0] += inverse * q.x * q.x;
                metric[1] += inverse * q.x * q.y;
                metric[2] += inverse * q.y * q.y;
            }
        }
        (gradient, metric)
    }

    fn minimum(&self, bodies: (&mut SolverBody, &mut SolverBody), radius: f64) -> DVec2 {
        let (one, two) = bodies;
        let mut p = self.previous;
        let mut evaluated = self.evaluate((one, two), p, false);
        let mut applied = [0.0; 2];
        // Resolution: a visit's work is bounded rather than converged per point: one current-face
        // step and at most one corrective one. The bare-inertia exit below took at most 0.012 % of
        // any measured line's visits (MEASURED, `track-contact.md`).
        for _ in 0..2 {
            let (gradient, metric) = evaluated;
            // Resolution: candidates share the rigid solver's f32 impulse storage.
            let next = disk_minimum(metric, gradient - product(metric, p), radius)
                .as_vec2()
                .as_dvec2();
            let delta = next.as_vec2() - self.previous.as_vec2();
            let impulse = self.tangents[0] * delta.x + self.tangents[1] * delta.y;
            if !exchange_pair((one, two), self.coordinates, impulse, &mut applied) {
                return next;
            }
            p = next;
            evaluated = self.evaluate((one, two), p, false);
        }
        // Drop this point's entire warm impulse before measuring slip. Zero impulse is the
        // work reference; a warm base can already contain numerical energy.
        exchange_pair(
            (one, two),
            self.coordinates,
            -self.tangents[0] * self.previous.x as f32 - self.tangents[1] * self.previous.y as f32,
            &mut applied,
        );
        let (gradient, free) = self.evaluate((one, two), DVec2::ZERO, true);
        let next = disk_minimum(free, gradient, radius);
        let delta = next.as_vec2() - self.previous.as_vec2();
        exchange_pair(
            (one, two),
            self.coordinates,
            self.tangents[0] * delta.x + self.tangents[1] * delta.y,
            &mut applied,
        );
        next
    }
}

/// The minimizer of `p' W p / 2 + g' p` on the Coulomb disk. Diagonalizing the positive
/// semidefinite metric retains zero-mobility directions without dividing by zero. A boundary
/// minimizer has `p = -(W + lambda I)^-1 g`; its norm decreases monotonically with lambda.
fn disk_minimum(w: [f64; 3], g: DVec2, radius: f64) -> DVec2 {
    assert!(
        radius.is_finite() && radius >= 0.0,
        "a contact supplies a finite nonnegative friction budget"
    );
    assert!(
        w.iter().all(|v| v.is_finite()) && g.is_finite(),
        "finite inertias and contact velocities produce a finite friction metric and gradient"
    );
    if radius == 0.0 {
        return DVec2::ZERO;
    }
    let [a, b, d] = w;
    let axes = if b == 0.0 {
        if a >= d {
            [DVec2::X, DVec2::Y]
        } else {
            [DVec2::Y, DVec2::NEG_X]
        }
    } else {
        let angle = 0.5 * (2.0 * b).atan2(a - d);
        let (s, c) = angle.sin_cos();
        [DVec2::new(c, s), DVec2::new(-s, c)]
    };
    let large = ((a + d) * 0.5 + ((a - d) * 0.5).hypot(b)).max(0.0);
    // A sum of inertial quadratic forms is PSD; cancellation in its determinant may be negative.
    let small = if large == 0.0 {
        0.0
    } else {
        ((a * d - b * b) / large).max(0.0)
    };
    let eigen = [large, small];
    // Resolution: projection of a zero-mobility mode can retain a few f64 dot-product ulps.
    // It cannot warrant a force in a mode whose work gradient is numerically zero.
    let projected = [0, 1].map(|i| {
        let value = axes[i].dot(g);
        if eigen[i] == 0.0 && value.abs() <= 16.0 * f64::EPSILON * g.length() {
            0.0
        } else {
            value
        }
    });
    let evaluate = |lambda: f64| -> Option<DVec2> {
        let mut p = DVec2::ZERO;
        for i in 0..2 {
            let denominator = eigen[i] + lambda;
            if denominator == 0.0 {
                if projected[i] != 0.0 {
                    return None;
                }
            } else {
                p -= axes[i] * (projected[i] / denominator);
            }
        }
        Some(p)
    };
    if let Some(p) = evaluate(0.0)
        && p.length() <= radius
    {
        return p;
    }
    let (mut lo, mut hi) = (0.0, g.length() / radius);
    // Resolution: 64 bisections exceed f64's mantissa width; the final point stays on the
    // admissible side of the disk, with no speed or physical-force threshold.
    for _ in 0..64 {
        let middle = (lo + hi) * 0.5;
        match evaluate(middle) {
            Some(p) if p.length() <= radius => hi = middle,
            _ => lo = middle,
        }
    }
    evaluate(hi).expect("positive friction regularization resolves every inertial mode")
}

/// Both passes share one accumulated impulse and interval budget. Fresh belt contacts need
/// friction before integration: the cleared and warm-started slab witnesses require no travel
/// under a holding brake. Relaxation alone holds final speed after allowing that travel.
pub fn solve(
    constraint: &mut ContactConstraint,
    body1: &mut SolverBody,
    body2: &mut SolverBody,
    inertia1: &SolverBodyInertia,
    inertia2: &SolverBodyInertia,
    stage: SurfaceFrictionStage,
) {
    let tangents = constraint.tangent_directions();
    let coordinates = constraint.surface_coordinates;
    let inv_mass = inertia1.effective_inv_mass() + inertia2.effective_inv_mass();
    let i1 = inertia1.effective_inv_angular_inertia();
    let i2 = inertia2.effective_inv_angular_inertia();
    for point in &mut constraint.points {
        let Some(part) = &mut point.tangent_part else {
            continue;
        }; // Frictionless contact.
        if let SurfaceFrictionStage::WarmStart(coefficient) = stage {
            let p = tangents[0] * part.impulse.x + tangents[1] * part.impulse.y;
            let p = p * coefficient;
            for (body, coordinate) in [(&mut *body1, coordinates[0]), (&mut *body2, coordinates[1])]
            {
                if let Some(c) = coordinate {
                    surface_mut(body).warm_push(c.index, f64::from(c.direction.dot(p)));
                }
            }
            part.impulse *= coefficient;
            continue;
        }
        let [r1, r2] = [point.anchor1, point.anchor2];
        let velocity =
            body2.velocity_at_point(r2) - body1.velocity_at_point(r1) + constraint.tangent_velocity;
        let cross1 = tangents.map(|t| r1.cross(t));
        let cross2 = tangents.map(|t| r2.cross(t));
        let mass = |a: usize, b: usize| {
            f64::from(
                tangents[a].dot(inv_mass * tangents[b])
                    + cross1[a].dot(i1 * cross1[b])
                    + cross2[a].dot(i2 * cross2[b]),
            )
        };
        let metric = ContactMetric {
            rigid: [mass(0, 0), mass(0, 1), mass(1, 1)],
            gradient: DVec2::new(
                f64::from(velocity.dot(tangents[0])),
                f64::from(velocity.dot(tangents[1])),
            ),
            tangents,
            previous: part.impulse.as_dvec2(),
            coordinates,
        };
        let next = metric
            .minimum(
                (body1, body2),
                f64::from(constraint.friction * point.normal_part.impulse),
            )
            .as_vec2();
        let delta = next - part.impulse;
        part.impulse = next;
        let p = tangents[0] * delta.x + tangents[1] * delta.y;
        body1.linear_velocity -= p * inertia1.effective_inv_mass();
        body1.angular_velocity -= i1 * r1.cross(p);
        body2.linear_velocity += p * inertia2.effective_inv_mass();
        body2.angular_velocity += i2 * r2.cross(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::surface_motion::{Coordinates, free_fixture, surface};
    use avian3d::collision::contact_types::ContactId;
    use avian3d::dynamics::solver::{
        contact::{ContactConstraintPoint, ContactNormalPart, ContactTangentPart},
        softness_parameters::SoftnessCoefficients,
        solver_body::SolverBodyIndex,
    };
    use avian3d::{math::SymmetricTensor, prelude::LockedAxes};

    fn belt_body(velocity: f32, inverse_inertia: f32) -> SolverBody {
        let mut body = SolverBody::default();
        free_fixture([velocity, 0.0], inverse_inertia.recip()).install(&mut body);
        body
    }
    fn point_mass(mass: f32) -> SolverBodyInertia {
        SolverBodyInertia::new(
            1.0 / mass,
            SymmetricTensor::ZERO,
            LockedAxes::default(),
            0,
            true,
        )
    }
    fn relax(
        row: &mut ContactConstraint,
        one: &mut SolverBody,
        two: &mut SolverBody,
        i1: &SolverBodyInertia,
        i2: &SolverBodyInertia,
    ) {
        row.solve::<false>(
            one,
            two,
            i1,
            i2,
            1.0 / 384.0,
            avian3d::dynamics::solver::SolverConfig::default().max_overlap_solve_speed,
        );
    }

    /// Bodies and finite surfaces rely on passive friction never creating kinetic energy.
    /// Fixture: arbitrary SPD tangent metrics, slips and capacities, including a pinned mode.
    #[test]
    fn coulomb_disk_minimizes_the_inertial_work() {
        for w in [
            [1.0, 0.0, 1.0],
            [0.00001, 0.000002, 0.000004],
            [1.0, 0.0, 0.0],
        ] {
            for g in [DVec2::ZERO, DVec2::new(1.68, 0.0), DVec2::new(-3.0, 2.0)] {
                for radius in [0.0, 2.0, 1_000_000.0] {
                    let p = disk_minimum(w, g, radius);
                    let work = g.dot(p)
                        + 0.5 * (w[0] * p.x * p.x + 2.0 * w[1] * p.x * p.y + w[2] * p.y * p.y);
                    // Resolution: relative f64 disk-bound roundoff.
                    assert!(p.is_finite() && p.length() <= radius * (1.0 + 1e-12));
                    // Resolution: f64 cancellation in the quadratic work.
                    assert!(
                        work <= 1e-9,
                        "a passive friction exchange cannot supply energy: {work}"
                    );
                    if radius > 0.0 {
                        let gradient =
                            DVec2::new(w[0] * p.x + w[1] * p.y, w[1] * p.x + w[2] * p.y) + g;
                        // Resolution: distinguish an interior point from the disk boundary in f64.
                        let lambda = if p.length() < radius * (1.0 - 1e-10) {
                            0.0
                        } else {
                            -gradient.dot(p) / (radius * radius)
                        };
                        // Resolution: KKT residual after f64 eigendecomposition and bisection.
                        assert!(
                            lambda >= -1e-9
                                && (gradient + lambda * p).length() <= 1e-8 * (1.0 + g.length())
                        );
                    }
                }
            }
        }
    }
    // Fixture: a loaded plane contact at the body's centre, with a prior tangential impulse.
    fn contact(mass: f32) -> ContactConstraint {
        register();
        let softness = SoftnessCoefficients {
            bias: 0.0,
            mass_scale: 1.0,
            impulse_scale: 0.0,
        };
        ContactConstraint {
            surface_coordinates: [
                Some(SurfaceCoordinate {
                    index: 0,
                    direction: -Vec3::Z,
                }),
                None,
            ],
            body_index1: SolverBodyIndex(0),
            body_index2: SolverBodyIndex::INVALID,
            relative_dominance: 0,
            friction: 0.7,
            restitution: 0.0,
            tangent_velocity: Vec3::ZERO,
            normal: Vec3::Y,
            tangent1: Vec3::Z,
            contact_id: ContactId(0),
            manifold_index: 0,
            points: vec![ContactConstraintPoint {
                normal_part: ContactNormalPart {
                    impulse: 0.0,
                    total_impulse: 0.0,
                    effective_mass: mass,
                    softness,
                    compliant: false,
                    spring: softness,
                },
                tangent_part: Some(ContactTangentPart::generate(
                    Vec3::splat(1.0 / mass),
                    &SymmetricTensor::ZERO,
                    &SymmetricTensor::ZERO,
                    Vec3::ZERO,
                    Vec3::ZERO,
                    [Vec3::Z, Vec3::NEG_X],
                    Some(Vec2::new(17.0, 0.0)),
                )),
                anchor1: Vec3::ZERO,
                anchor2: Vec3::ZERO,
                normal_speed: 0.0,
                initial_separation: 0.0,
            }],
        }
    }

    /// The native constraint must exchange an impulse with both the rigid body and its finite
    /// surface during the same solve. Ratios vary across the cases; a prescribed-speed row with
    /// delayed billing creates kinetic energy when the rigid body is heavier than the surface.
    #[test]
    fn native_contact_friction_cannot_create_energy_in_finite_surfaces() {
        // Fixture: independent masses, a loaded contact's normal budget, slip and warm impulse.
        for mass in [10.0_f32, 10_000.0, 100_000_000.0] {
            for belt in [1.0_f32, 4_750.0, 1_000_000.0] {
                for coefficient in [0.0, 0.5, 1.0] {
                    let inertia = point_mass(mass);
                    let mut body = belt_body(1.68, 1.0 / belt);
                    let mut fixed = SolverBody::DUMMY;
                    let mut constraint = contact(mass);
                    let initial =
                        0.5 * f64::from(belt) * f64::from(surface(&body).speed(0) as f32).powi(2);
                    constraint.warm_start(
                        &mut body,
                        &mut fixed,
                        &inertia,
                        &SolverBodyInertia::DUMMY,
                        coefficient,
                    );
                    // Fixture: a normal press with no normal velocity; only friction is exercised.
                    constraint.points[0].normal_part.impulse = 1_000_000.0;
                    relax(
                        &mut constraint,
                        &mut body,
                        &mut fixed,
                        &inertia,
                        &SolverBodyInertia::DUMMY,
                    );
                    let final_energy = 0.5
                        * f64::from(mass)
                        * body.linear_velocity.as_dvec3().length_squared()
                        + 0.5 * f64::from(belt) * f64::from(surface(&body).speed(0) as f32).powi(2);
                    let budget = constraint.friction * constraint.points[0].normal_part.impulse;
                    let expected = (1.68 / (1.0 / mass + 1.0 / belt)).min(budget);
                    let delivered = constraint.points[0].tangent_part.as_ref().unwrap().impulse;
                    // Resolution: f32 warm-impulse cancellation and final velocity assembly.
                    assert!(
                        (delivered.x - expected).abs() <= 1e-5 * expected.max(1.0),
                        "the contact must use both inertias in the same solve"
                    );
                    // Resolution: f32 impulse and velocity assembly relative to the initial energy.
                    assert!(
                        final_energy <= initial * (1.0 + 1e-5),
                        "passive contact added energy: {final_energy} > {initial}"
                    );
                }
            }
        }
    }
    /// Belt-on-belt contacts rely on both owners receiving the same solved impulse immediately.
    #[test]
    fn a_contact_updates_both_finite_surfaces() {
        // Fixture: unequal bodies/surface inertias, opposite motion and a prescribed surface offset.
        let mut constraint = contact(10_000.0);
        constraint.tangent_velocity = -Vec3::Z * 0.4;
        constraint.surface_coordinates[1] = Some(SurfaceCoordinate {
            index: 0,
            direction: Vec3::Z,
        });
        constraint.body_index2 = SolverBodyIndex(1);
        constraint.points[0].normal_part.impulse = 1_000_000.0;
        constraint.points[0].tangent_part.as_mut().unwrap().impulse = Vec2::ZERO;
        let mut one = belt_body(1.68, 1.0 / 4750.0);
        let mut two = belt_body(-2.0, 1.0 / 2000.0);
        let i1 = point_mass(10_000.0);
        let i2 = point_mass(3000.0);
        let expected =
            (1.68 + 2.0 + 0.4) / (1.0 / 10_000.0 + 1.0 / 3000.0 + 1.0 / 4750.0 + 1.0 / 2000.0);
        relax(&mut constraint, &mut one, &mut two, &i1, &i2);
        // Resolution: f32 exchange at each owner, relative to its fixture speed.
        assert!((surface(&one).speed(0) as f32 - (1.68 - expected / 4750.0)).abs() < 1e-6);
        assert!((surface(&two).speed(0) as f32 - (-2.0 + expected / 2000.0)).abs() < 1e-6);
    }
    /// Both bodies rely on equal/opposite linear momentum and torque at the actual anchor.
    /// Fixture: unequal point masses and diagonal angular inertias, with off-centre contacts.
    #[test]
    fn off_centre_contact_exchanges_each_bodys_linear_and_angular_momentum() {
        let mut row = contact(10_000.0);
        row.body_index2 = SolverBodyIndex(1);
        row.surface_coordinates[1] = Some(SurfaceCoordinate {
            index: 0,
            direction: Vec3::Z,
        });
        row.points[0].anchor1 = Vec3::new(0.4, -0.8, 0.0);
        row.points[0].anchor2 = Vec3::new(-0.3, 0.6, 0.0);
        // DERIVED: coincident contact witnesses have zero separation despite different COM anchors.
        row.points[0].initial_separation =
            -(row.points[0].anchor2 - row.points[0].anchor1).dot(row.normal);

        row.points[0].normal_part.impulse = 1_000_000.0;
        row.points[0].tangent_part.as_mut().unwrap().impulse = Vec2::ZERO;
        let mut one = belt_body(1.68, 1.0 / 4750.0);
        let mut two = belt_body(-2.0, 1.0 / 2000.0);
        let angular = [
            Vec3::new(2000.0, 3000.0, 4000.0),
            Vec3::new(900.0, 1300.0, 1700.0),
        ];
        let i1 = SolverBodyInertia::new(
            1.0 / 10_000.0,
            SymmetricTensor::from_diagonal(angular[0].recip()),
            LockedAxes::default(),
            0,
            true,
        );
        let i2 = SolverBodyInertia::new(
            1.0 / 3000.0,
            SymmetricTensor::from_diagonal(angular[1].recip()),
            LockedAxes::default(),
            0,
            true,
        );
        relax(&mut row, &mut one, &mut two, &i1, &i2);
        let applied = row.points[0].tangent_part.as_ref().unwrap().impulse;
        let [t1, t2] = row.tangent_directions();
        let p = t1 * applied.x + t2 * applied.y;
        assert!(p.length() > 0.0, "the fixture must exchange momentum");
        // Resolution: f32 velocity and torque updates, relative to exchanged momentum.
        let close = |actual: Vec3, expected: Vec3| {
            assert!(
                (actual - expected).length() <= 2e-6 * (1.0 + p.length() + expected.length()),
                "{actual:?} != {expected:?}"
            )
        };
        close(one.linear_velocity * 10_000.0, -p);
        close(two.linear_velocity * 3000.0, p);
        close(
            angular[0] * one.angular_velocity,
            -row.points[0].anchor1.cross(p),
        );
        close(
            angular[1] * two.angular_velocity,
            row.points[0].anchor2.cross(p),
        );
        let belt1 = 4750.0 * (surface(&one).speed(0) - 1.68_f32 as f64);
        let belt2 = 2000.0 * (surface(&two).speed(0) + 2.0);
        // Resolution: the contact's world impulse and its direction dot product are f32.
        assert!((belt1 + f64::from(p.z)).abs() <= 2e-6 * (1.0 + f64::from(p.length())));
        assert!((belt2 - f64::from(p.z)).abs() <= 2e-6 * (1.0 + f64::from(p.length())));
    }

    /// Solver contacts must hold the hull while a brake has capacity and release at the actual
    /// sliding capacity when it breaks. Fixture: class drivetrains, no powered supply in the
    /// game's damage model, a moving box and a loaded plane with ample friction.
    #[test]
    fn contact_sees_the_bounded_brake_in_the_same_interval() {
        use crate::track::drive_fixture::{class_drivetrains, construction};
        use crate::track::transmission::{DriveState, DriveSupply, DriveTick, TransmissionState};
        for (fp, tp) in class_drivetrains() {
            let fp = construction(&fp);
            for h in [1.0 / 192.0, 1.0 / 768.0] {
                let sliding = tp.brake_capacity_n * h;
                let capacity = sliding * tp.brake_static_factor;
                for held in [true, false] {
                    let mass = 1000.0; // Fixture box mass, kg.
                    let speed = if held { 0.25 } else { 2.0 } * capacity / mass;
                    let state = DriveState {
                        transmission: TransmissionState {
                            park: true,
                            mean_hold: false,
                            omega_e: 0.0,
                            ..TransmissionState::from_spec(&tp)
                        },
                        speeds: [0.0; 2],
                        phases: [0.0; 2],
                    };
                    let supply = DriveSupply {
                        control_authority: false,
                        task_rate: 0.0,
                        engine_in_action: false,
                        transmission_condition: 0.0,
                    };
                    let mut tick = DriveTick::contact_only(
                        &fp,
                        &tp,
                        state,
                        crate::track::drive::DriveTarget {
                            supply,
                            intent: crate::track::drive::DriveIntent::Axes(
                                crate::track::drive::DriveAxes::default(),
                            ),
                        },
                        h,
                    );
                    let mut body = SolverBody {
                        linear_velocity: Vec3::NEG_Z * speed,
                        ..Default::default()
                    };
                    tick.prepare_contact(&fp, &tp, h, false).install(&mut body);
                    let mut fixed = SolverBody::DUMMY;
                    let mut row = contact(mass);
                    row.points[0].normal_part.impulse = 4.0 * mass * speed / row.friction;
                    row.points[0].tangent_part.as_mut().unwrap().impulse = Vec2::ZERO;
                    relax(
                        &mut row,
                        &mut body,
                        &mut fixed,
                        &point_mass(mass),
                        &SolverBodyInertia::DUMMY,
                    );
                    let expected = if held {
                        mass * speed
                    } else {
                        (speed + sliding / fp.belt_inertia) / (1.0 / mass + 1.0 / fp.belt_inertia)
                    };
                    let delivered = -row.points[0].tangent_part.as_ref().unwrap().impulse.x;
                    // Resolution: f32 rigid impulse and velocity assembly.
                    assert!(
                        (delivered - expected).abs() <= 4e-6 * expected,
                        "{delivered} != {expected}, held={held}"
                    );
                    tick.finish_contact(&tp, surface(&body), h);
                    assert!(
                        !tick.report(1).locked,
                        "inactive clutch rows cannot report lock to the Driver"
                    );
                    assert_eq!(tick.report(1).held[0], held);
                    assert_eq!(tick.report(1).brakes_slide, !held);
                }
            }
        }
    }

    /// Even an unloaded speculative point must reject a corrupt velocity for its owner.
    #[test]
    #[should_panic(expected = "finite inertias and contact velocities")]
    fn zero_friction_budget_still_checks_finite_inputs() {
        disk_minimum([1.0, 0.0, 1.0], DVec2::new(f64::NAN, 0.0), 0.0);
    }
    /// Fixture: an owner whose every push reports a changed face, so each visit takes the
    /// contact's second exit, the bare-inertia step from zero impulse.
    #[derive(Clone, Debug)]
    struct EveryPushChangesFace(Coordinates);
    impl SurfaceModel for EveryPushChangesFace {
        fn clone_model(&self) -> Box<dyn SurfaceModel> {
            Box::new(self.clone())
        }
        fn speed(&self, index: usize) -> f64 {
            self.0.speed(index)
        }
        fn mobility(&self, index: usize) -> f64 {
            self.0.mobility(index)
        }
        fn free_mobility(&self, index: usize) -> f64 {
            self.0.free_mobility(index)
        }
        fn warm_push(&mut self, index: usize, impulse: f64) {
            self.0.warm_push(index, impulse);
        }
        fn push(&mut self, index: usize, impulse: f64) -> bool {
            self.0.push(index, impulse);
            true
        }
    }

    /// The game's contact physics: a numerical warm start cannot leave an unpowered drivetrain
    /// and rigid body more energy than a frictionless visit from the same state, and none from
    /// rest, including bounded clutch faces. Each cell runs the shipped exits and again with
    /// every visit forced to the second, whose work reference is zero impulse.
    #[test]
    fn bounded_drivetrain_warm_start_cannot_exceed_frictionless_energy() {
        use crate::track::drive_fixture::{class_drivetrains, construction};
        use crate::track::transmission::{
            DriveState, DriveSupply, DriveTick, LaunchState, TransmissionState,
        };
        let coordinates = |body: &SolverBody| -> Coordinates {
            let model: &dyn core::any::Any = super::surface(body);
            model
                .downcast_ref::<Coordinates>()
                .or_else(|| model.downcast_ref::<EveryPushChangesFace>().map(|f| &f.0))
                .expect("the witness installs drivetrain coordinates")
                .clone()
        };
        for (class, (fp, tp)) in class_drivetrains().into_iter().enumerate() {
            let fp = construction(&fp);
            let h = 1.0 / 384.0; // Fixture interval, s.
            for mass in [1000.0_f32, 100_000.0, 100_000_000.0] {
                for scale in [1.0, 4.0, 16.0, 64.0, 256.0] {
                    let state = DriveState {
                        transmission: TransmissionState {
                            omega_e: 0.0,
                            park: false,
                            mean_hold: false,
                            launch: LaunchState::Locked,
                            ..TransmissionState::from_spec(&tp)
                        },
                        speeds: [0.0; 2],
                        phases: [0.0; 2],
                    };
                    let mut tick = DriveTick::contact_only(
                        &fp,
                        &tp,
                        state,
                        crate::track::drive::DriveTarget {
                            supply: DriveSupply {
                                control_authority: false,
                                task_rate: 0.0,
                                engine_in_action: false,
                                transmission_condition: 1.0,
                            },
                            intent: crate::track::drive::DriveIntent::Axes(
                                crate::track::drive::DriveAxes::default(),
                            ),
                        },
                        h,
                    );
                    let k = tp.ratio(false, tick.state.transmission.gear) / tp.sprocket_radius;
                    // DERIVED fixture impulse scale from both finite coupling budgets.
                    let warm = scale * h * (tp.clutch_capacity * k + 2.0 * tp.steer_capacity_n);
                    for velocity in [-1.68, 0.0, 1.68] {
                        // Fixture hull motion, m/s.
                        for forced in [false, true] {
                            let mut body = SolverBody::default();
                            let prepared = tick.prepare_contact(&fp, &tp, h, false);
                            assert_eq!(
                                prepared.kinetic_energy(),
                                0.0,
                                "the fixture drivetrain must be passive and initially at rest"
                            );
                            if forced {
                                install(EveryPushChangesFace(prepared), &mut body);
                            } else {
                                prepared.install(&mut body);
                            }
                            body.linear_velocity = Vec3::Z * velocity;
                            let energy = |body: &SolverBody| {
                                0.5 * f64::from(mass)
                                    * body.linear_velocity.as_dvec3().length_squared()
                                    + coordinates(body).kinetic_energy()
                            };
                            let mut reference = body.clone();
                            let mut fixed = SolverBody::DUMMY;
                            let mut frictionless = contact(mass);
                            frictionless.friction = 0.0;
                            frictionless.points[0]
                                .tangent_part
                                .as_mut()
                                .unwrap()
                                .impulse = Vec2::ZERO;
                            relax(
                                &mut frictionless,
                                &mut reference,
                                &mut fixed,
                                &point_mass(mass),
                                &SolverBodyInertia::DUMMY,
                            );
                            let baseline = energy(&reference);
                            let mut row = contact(mass);
                            row.points[0].tangent_part.as_mut().unwrap().impulse =
                                Vec2::new(warm, 0.0);
                            row.warm_start(
                                &mut body,
                                &mut fixed,
                                &point_mass(mass),
                                &SolverBodyInertia::DUMMY,
                                1.0,
                            );
                            row.points[0].normal_part.impulse = 4.0 * warm / row.friction;
                            relax(
                                &mut row,
                                &mut body,
                                &mut fixed,
                                &point_mass(mass),
                                &SolverBodyInertia::DUMMY,
                            );
                            // DERIVED: 64 f32 ulps of warm cancellation plus moving momentum,
                            // priced through the free mobility and the initial velocity.
                            let dp = 64.0
                                * f64::from(f32::EPSILON)
                                * (f64::from(warm) + f64::from(mass * velocity).abs());
                            let rounding = f64::from(velocity).abs() * dp
                                + 0.5 * dp * dp * f64::from(1.0 / mass + 1.0 / fp.belt_inertia);
                            let after = energy(&body);
                            println!(
                                "PASSIVE_WARM class={class} mass={mass} warm={warm} v={velocity} forced={forced} energy={after} frictionless={baseline} allowance={rounding} J"
                            );
                            assert!(
                                after <= baseline + rounding,
                                "contact energy {after} > frictionless {baseline} + {rounding} J, class={class}, mass={mass}, warm={warm}, v={velocity}, forced={forced}"
                            );
                        }
                    }
                }
            }
        }
    }
}
