//! Finite-inertia moving surfaces inside Avian's contact solve. The fork supplies scalar
//! coordinates and process-wide dispatch; this seam owns their Coulomb law. A friction impulse
//! changes both rigid bodies and every moving-surface coordinate before the next contact reads
//! them. Avian's graph colors preserve that ownership, including its serial overflow color.

use avian3d::dynamics::solver::{
    contact::{ContactConstraint, SurfaceFrictionStage},
    solver_body::{SolverBody, SolverBodyInertia, SurfaceCoordinate},
};
use bevy::math::{DVec2, Vec2, Vec3};

use crate::track::transmission::{self, PreparedDrive};
use avian3d::dynamics::solver::solver_body::{SurfaceData, SurfaceScratch};

#[derive(Clone, Copy, Debug)]
pub(crate) struct SurfaceSpeed {
    pub(crate) velocity: f32,
    pub(crate) inverse_inertia: f32,
}

#[derive(Clone, Debug)]
struct Evaluation {
    speeds: Vec<f64>,
    potential: f64,
    mobility: Vec<Vec<f64>>,
    solved: Option<crate::track::block::Solution>,
    response: Option<std::sync::Arc<crate::track::block::FaceResponse>>,
}

/// Game-owned mechanical scratch. Every candidate reuses the same interval's free momentum
/// and bounded rows; contact impulses never receive another motor or brake budget.
#[derive(Clone, Debug)]
pub(crate) struct Coordinates {
    pub(crate) initial: Vec<SurfaceSpeed>,
    pub(crate) prepared: Option<PreparedDrive>,
    impulses: Vec<f64>,
    sliding: Vec<Option<crate::track::block::Bounds>>,
    current: Evaluation,
}

impl SurfaceData for Coordinates {
    fn clone_box(&self) -> Box<dyn SurfaceData> {
        Box::new(self.clone())
    }
}

pub(crate) fn register() {
    avian3d::dynamics::solver::contact::SURFACE_FRICTION.get_or_init(|| solve);
}

impl Coordinates {
    pub(crate) fn free(initial: Vec<SurfaceSpeed>) -> Self {
        register();
        assert!(
            initial.iter().all(|s| s.velocity.is_finite()
                && s.inverse_inertia.is_finite()
                && s.inverse_inertia >= 0.0),
            "prepared surface coordinates have finite speed and nonnegative finite mobility"
        );
        let current = Evaluation {
            speeds: initial.iter().map(|s| f64::from(s.velocity)).collect(),
            potential: 0.0,
            mobility: initial
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    initial
                        .iter()
                        .enumerate()
                        .map(|(j, s)| {
                            if i == j {
                                f64::from(s.inverse_inertia)
                            } else {
                                0.0
                            }
                        })
                        .collect()
                })
                .collect(),
            solved: None,
            response: None,
        };
        Self {
            impulses: vec![0.0; initial.len()],
            initial,
            prepared: None,
            sliding: Vec::new(),
            current,
        }
    }

    pub(crate) fn drivetrain(prepared: PreparedDrive) -> Self {
        let initial = prepared.mechanical.speeds[1..]
            .iter()
            .map(|s| SurfaceSpeed {
                velocity: s.value as f32,
                inverse_inertia: (1.0 / s.inertia) as f32,
            })
            .collect();
        let mut state = Self::free(initial);
        state.prepared = Some(prepared);
        let mechanical = &mut state
            .prepared
            .as_mut()
            .expect("a drivetrain prepares its rows")
            .mechanical;
        let starting: Vec<_> = mechanical.speeds.iter().map(|s| s.value).collect();
        // The starting row speed selects its regime. Free motor load alone cannot break a stop
        // before the contact has had its chance to share that load.
        state.sliding = mechanical
            .rows
            .iter_mut()
            .map(|row| {
                let sliding = row.dynamic.take();
                if row.starts_sliding(&starting)
                    && let Some(bounds) = sliding
                {
                    row.bounds = bounds;
                    None
                } else {
                    sliding
                }
            })
            .collect();
        state.current = state.evaluate(&state.impulses);
        state
    }

    fn evaluate(&self, impulses: &[f64]) -> Evaluation {
        assert!(
            impulses.iter().all(|p| p.is_finite()),
            "contact impulses are finite"
        );
        if let Some(prepared) = &self.prepared {
            let mechanical = &prepared.mechanical;
            let change = [
                0.0,
                impulses[0] - self.impulses[0],
                impulses[1] - self.impulses[1],
            ];
            let cached = self
                .current
                .response
                .as_ref()
                .zip(self.current.solved.as_ref())
                .and_then(|(response, base)| {
                    response.apply(base, &mechanical.rows, &mechanical.limits, &change)
                });
            let (solved, response) = if let Some(solved) = cached {
                (
                    solved,
                    std::sync::Arc::clone(
                        self.current
                            .response
                            .as_ref()
                            .expect("a certified cached face has its response"),
                    ),
                )
            } else {
                let mut warm = transmission::WarmStart::default();
                if let Some(solved) = &self.current.solved {
                    warm.remember(&mechanical.rows, &mechanical.limits, solved);
                }
                let solved = mechanical.solve([impulses[0], impulses[1]], &mut warm);
                let response = std::sync::Arc::new(solved.response(
                    &mechanical.speeds,
                    &mechanical.rows,
                    &mechanical.limits,
                ));
                (solved, response)
            };
            Evaluation {
                speeds: solved.speeds[1..].to_vec(),
                potential: mechanical.potential(&solved),
                mobility: Vec::new(),
                solved: Some(solved),
                response: Some(response),
            }
        } else {
            Evaluation {
                speeds: self
                    .initial
                    .iter()
                    .zip(impulses)
                    .map(|(s, p)| f64::from(s.velocity) + f64::from(s.inverse_inertia) * p)
                    .collect(),
                potential: self
                    .initial
                    .iter()
                    .zip(impulses)
                    .map(|(s, p)| {
                        f64::from(s.velocity) * p + 0.5 * f64::from(s.inverse_inertia) * p * p
                    })
                    .sum(),
                mobility: self.current.mobility.clone(),
                solved: None,
                response: None,
            }
        }
    }

    fn trial(
        &self,
        coordinate: SurfaceCoordinate,
        impulse: f64,
    ) -> std::borrow::Cow<'_, Evaluation> {
        if impulse == 0.0 {
            return std::borrow::Cow::Borrowed(&self.current);
        }
        let mut impulses = self.impulses.clone();
        *impulses
            .get_mut(coordinate.index)
            .expect("a contact binds its owner's prepared coordinate") += impulse;
        std::borrow::Cow::Owned(self.evaluate(&impulses))
    }

    fn failed_static<'a>(&'a self, speeds: &'a [f64]) -> impl Iterator<Item = usize> + 'a {
        self.sliding
            .iter()
            .enumerate()
            .filter_map(move |(i, sliding)| {
                let mechanical = self
                    .prepared
                    .as_ref()
                    .expect("static elements have prepared mechanics");
                let row = &mechanical.mechanical.rows[i];
                let slip = row
                    .jacobian
                    .iter()
                    .zip(speeds)
                    .map(|(a, v)| a * v)
                    .sum::<f64>()
                    - row.target;
                (sliding.is_some() && slip.abs() > row.slip_tolerance(speeds)).then_some(i)
            })
    }

    fn release_static(&mut self, speeds: &[f64]) -> bool {
        let failed: Vec<_> = self.failed_static(speeds).collect();
        if failed.is_empty() {
            return false;
        }
        let mechanical = &mut self
            .prepared
            .as_mut()
            .expect("static elements have prepared mechanics")
            .mechanical;
        for i in failed {
            mechanical.rows[i].bounds = self.sliding[i]
                .take()
                .expect("a static element retains its sliding bounds");
        }
        self.current = self.evaluate(&self.impulses);
        true
    }

    fn break_static(&mut self, coordinate: SurfaceCoordinate, impulse: f64) -> bool {
        if !self.sliding.iter().any(Option::is_some) {
            return false;
        } // No static row can change regime.
        let trial = self.trial(coordinate, impulse);
        let speeds = trial
            .solved
            .as_ref()
            .expect("a mechanical trial has a solution")
            .speeds
            .clone();
        drop(trial);
        self.release_static(&speeds)
    }

    fn settled(&self) -> std::borrow::Cow<'_, Self> {
        let Some(solved) = &self.current.solved else {
            return std::borrow::Cow::Borrowed(self);
        }; // No mechanical rows.
        if self.failed_static(&solved.speeds).next().is_none() {
            return std::borrow::Cow::Borrowed(self);
        }
        // An owner with no contact still completes the same static-to-sliding law before readback.
        let mut settled = self.clone();
        loop {
            let speeds = settled
                .current
                .solved
                .as_ref()
                .expect("a mechanical owner has a solution")
                .speeds
                .clone();
            if !settled.release_static(&speeds) {
                break;
            }
        }
        std::borrow::Cow::Owned(settled)
    }

    fn accept_impulses(&mut self, impulses: Vec<f64>) {
        let current = self.evaluate(&impulses);
        self.impulses = impulses;
        self.current = current;
    }

    fn exchange(&mut self, coordinate: SurfaceCoordinate, impulse: Vec3) {
        let _ = self
            .impulses
            .get(coordinate.index)
            .expect("a contact binds its owner's prepared coordinate");
        let p = f64::from(coordinate.direction.dot(impulse));
        assert!(
            p.is_finite(),
            "a surface exchange supplies a finite generalized impulse"
        );
        if p == 0.0 {
            return;
        } // No momentum changed.
        let mut next = self.impulses.clone();
        next[coordinate.index] += p;
        self.accept_impulses(next);
    }

    #[cfg(test)]
    pub(crate) fn speeds(&self) -> [f32; 2] {
        self.current
            .speeds
            .iter()
            .map(|s| *s as f32)
            .collect::<Vec<_>>()
            .try_into()
            .expect("a belt owner prepares exactly two coordinates")
    }

    #[cfg(test)]
    pub(crate) fn contact_impulses(&self) -> [f64; 2] {
        self.impulses
            .as_slice()
            .try_into()
            .expect("a belt owner has two contact impulses")
    }

    pub(crate) fn remember(&self, warm: &mut transmission::WarmStart) {
        let prepared = self
            .prepared
            .as_ref()
            .expect("a mechanical owner prepares its rows");
        warm.remember(
            &prepared.mechanical.rows,
            &prepared.mechanical.limits,
            self.current
                .solved
                .as_ref()
                .expect("prepared rows have a solution"),
        );
    }

    #[cfg(test)]
    pub(crate) fn add_impulses(&mut self, impulses: [f64; 2]) {
        let mut next = self.impulses.clone();
        for (p, add) in next.iter_mut().zip(impulses) {
            *p += add;
        }
        self.accept_impulses(next);
    }

    pub(crate) fn finish(
        &self,
        tp: &transmission::TransmissionParams,
        state: &mut transmission::DriveState,
        h: f32,
    ) -> transmission::TransmissionReport {
        transmission::finish_advance(
            tp,
            state,
            self.prepared
                .as_ref()
                .expect("a dynamic drivetrain prepares its bounded mechanical rows"),
            self.current
                .solved
                .as_ref()
                .expect("prepared rows have a solved response"),
            h,
        )
    }

    #[cfg(feature = "bitprobe")]
    pub(crate) fn work(&self) -> f64 {
        self.initial
            .iter()
            .zip(&self.current.speeds)
            .zip(&self.impulses)
            .map(|((from, to), p)| p * (f64::from(from.velocity) + to) * 0.5)
            .sum()
    }

    pub(crate) fn install(self, body: &mut SolverBody) {
        body.surface_scratch = Some(SurfaceScratch(Box::new(self)));
    }
}

pub(crate) fn surface(body: &SolverBody) -> &Coordinates {
    let data: &dyn core::any::Any = body
        .surface_scratch
        .as_ref()
        .expect("a surface owner prepares scratch before contact solving")
        .0
        .as_ref();
    data.downcast_ref()
        .expect("the registered surface adapter owns this scratch type")
}

pub(crate) fn surface_mut(body: &mut SolverBody) -> &mut Coordinates {
    let data: &mut dyn core::any::Any = body
        .surface_scratch
        .as_mut()
        .expect("a surface owner prepares scratch before contact solving")
        .0
        .as_mut();
    data.downcast_mut()
        .expect("the registered surface adapter owns this scratch type")
}

/// Both adapters finish the same bounded rows and read the same final coordinates. Only a
/// contact-free static-to-sliding transition needs replacement scratch at the shared commit.
pub(crate) fn read_back(
    body: &SolverBody,
    tick: &mut transmission::DriveTick,
    tp: &transmission::TransmissionParams,
    h: f32,
) -> Option<Coordinates> {
    let settled = surface(body).settled();
    tick.finish_contact(tp, &settled, h);
    match settled {
        std::borrow::Cow::Borrowed(_) => None,
        std::borrow::Cow::Owned(state) => Some(state),
    }
}

pub(crate) fn belt_coordinates(speeds: [f32; 2], inverse: f32) -> Coordinates {
    Coordinates::free(
        speeds
            .map(|velocity| SurfaceSpeed {
                velocity,
                inverse_inertia: inverse,
            })
            .into(),
    )
}

#[cfg(test)]
pub(crate) fn solved_speeds(body: &SolverBody) -> [f32; 2] {
    surface(body).speeds()
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
) {
    for (body, coordinate) in [(bodies.0, coordinates[0]), (bodies.1, coordinates[1])] {
        if let Some(c) = coordinate {
            surface_mut(body).exchange(c, impulse);
        }
    }
}

struct ContactMetric<'a> {
    rigid: [f64; 3],
    gradient: DVec2,
    tangents: [Vec3; 2],
    previous: DVec2,
    surfaces: [Option<(&'a Coordinates, SurfaceCoordinate)>; 2],
}

fn product(w: [f64; 3], p: DVec2) -> DVec2 {
    DVec2::new(w[0] * p.x + w[1] * p.y, w[1] * p.x + w[2] * p.y)
}

impl ContactMetric<'_> {
    fn evaluate(&self, p: DVec2) -> (f64, DVec2, [f64; 3]) {
        let mut value = self.gradient.dot(p) + 0.5 * p.dot(product(self.rigid, p));
        let mut gradient = self.gradient + product(self.rigid, p);
        let mut metric = self.rigid;
        for &(state, c) in self.surfaces.iter().flatten() {
            let q = DVec2::new(
                f64::from(c.direction.dot(self.tangents[0])),
                f64::from(c.direction.dot(self.tangents[1])),
            );
            let result = state.trial(c, q.dot(p - self.previous));
            value += result.potential;
            gradient += q * result.speeds[c.index];
            let inverse = result.response.as_ref().map_or_else(
                || result.mobility[c.index][c.index],
                |response| response.mobility[c.index + 1][c.index + 1],
            );
            metric[0] += inverse * q.x * q.x;
            metric[1] += inverse * q.x * q.y;
            metric[2] += inverse * q.y * q.y;
        }
        assert!(
            value.is_finite() && gradient.is_finite(),
            "finite contact and mechanical rows have finite work and slip"
        );
        (value, gradient, metric)
    }

    /// Minimize the contact's Coulomb disk after eliminating the bounded mechanical rows.
    /// Held brakes therefore have zero mobility within capacity; locked clutches reflect crank
    /// inertia. A bound changes the KKT face, not a timestep-dependent brake correction.
    fn minimum(&self, radius: f64) -> DVec2 {
        let mut p = self.previous;
        if p.length() > radius {
            p *= radius / p.length();
        }
        // Resolution: semismooth Newton solves the piecewise-quadratic response to f64 roundoff.
        // The iteration limit detects a broken solver; it never publishes an unconverged result.
        let mut evaluated = self.evaluate(p);
        for _ in 0..64 {
            let (value, gradient, metric) = evaluated;
            let candidate = disk_minimum(metric, gradient - product(metric, p), radius);
            let delta = candidate - p;
            // Resolution: impulses are carried to f32 body velocities after this f64 solve.
            if delta.length() <= 1e-9 * (1.0 + p.length()) {
                return candidate;
            }
            let mut fraction = 1.0;
            loop {
                let next = p + fraction * delta;
                let trial = self.evaluate(next);
                let next_value = trial.0;
                // Resolution: objective evaluation can cancel the crank's much larger energy.
                let rounding = 64.0 * f64::EPSILON * (1.0 + value.abs() + next_value.abs());
                // Resolution: sufficient decrease keeps Newton steps inside a descending face.
                if next_value <= value + 0.0001 * fraction * gradient.dot(delta) + rounding {
                    p = next;
                    evaluated = trial;
                    break;
                }
                fraction *= 0.5;
                assert!(
                    fraction >= f64::EPSILON,
                    "a convex contact Newton step has a descending fraction"
                );
            }
        }
        panic!("the constrained surface contact must converge before exchanging momentum")
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

/// Scalar warm impulses are applied exactly once here. Both normal passes share the same
/// bounded mechanical budget: friction precedes position integration and relaxes afterwards,
/// so a holding brake does not depend on cached friction to prevent travel.
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
            exchange_pair((body1, body2), coordinates, p * coefficient);
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
        let rigid = [mass(0, 0), mass(0, 1), mass(1, 1)];
        let previous = part.impulse.as_dvec2();
        let next = loop {
            let metric = ContactMetric {
                rigid,
                gradient: DVec2::new(
                    f64::from(velocity.dot(tangents[0])),
                    f64::from(velocity.dot(tangents[1])),
                ) - product(rigid, previous),
                tangents,
                previous,
                surfaces: [
                    coordinates[0].map(|c| (surface(body1), c)),
                    coordinates[1].map(|c| (surface(body2), c)),
                ],
            };
            let next = metric
                .minimum(f64::from(constraint.friction * point.normal_part.impulse))
                .as_vec2();
            let p =
                tangents[0] * (next.x - part.impulse.x) + tangents[1] * (next.y - part.impulse.y);
            let mut changed = false;
            for (body, c) in [(&mut *body1, coordinates[0]), (&mut *body2, coordinates[1])] {
                if let Some(c) = c {
                    changed |= surface_mut(body).break_static(c, f64::from(c.direction.dot(p)));
                }
            }
            if !changed {
                break next;
            }
        };
        let delta = next - part.impulse;
        part.impulse = next;
        let p = tangents[0] * delta.x + tangents[1] * delta.y;
        body1.linear_velocity -= p * inertia1.effective_inv_mass();
        body1.angular_velocity -= i1 * r1.cross(p);
        body2.linear_velocity += p * inertia2.effective_inv_mass();
        body2.angular_velocity += i2 * r2.cross(p);
        exchange_pair((body1, body2), coordinates, p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avian3d::collision::contact_types::ContactId;
    use avian3d::dynamics::solver::{
        contact::{ContactConstraintPoint, ContactNormalPart, ContactTangentPart},
        softness_parameters::SoftnessCoefficients,
        solver_body::SolverBodyIndex,
    };
    use avian3d::{math::SymmetricTensor, prelude::LockedAxes};

    fn belt_body(velocity: f32, inverse_inertia: f32) -> SolverBody {
        let mut body = SolverBody::default();
        Coordinates::free(vec![SurfaceSpeed {
            velocity,
            inverse_inertia,
        }])
        .install(&mut body);
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
                    let initial = 0.5
                        * f64::from(belt)
                        * f64::from(surface(&body).current.speeds[0] as f32).powi(2);
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
                    let final_energy =
                        0.5 * f64::from(mass) * body.linear_velocity.as_dvec3().length_squared()
                            + 0.5
                                * f64::from(belt)
                                * f64::from(surface(&body).current.speeds[0] as f32).powi(2);
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
        assert!((surface(&one).current.speeds[0] as f32 - (1.68 - expected / 4750.0)).abs() < 1e-6);
        assert!((surface(&two).current.speeds[0] as f32 - (-2.0 + expected / 2000.0)).abs() < 1e-6);
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
            one.linear_velocity * 10_000.0 + two.linear_velocity * 3000.0,
            Vec3::ZERO,
        );
        close(
            angular[0] * one.angular_velocity,
            -row.points[0].anchor1.cross(p),
        );
        close(
            angular[1] * two.angular_velocity,
            row.points[0].anchor2.cross(p),
        );
        let belt1 = 4750.0 * (surface(&one).current.speeds[0] - 1.68_f32 as f64);
        let belt2 = 2000.0 * (surface(&two).current.speeds[0] + 2.0);
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
}
