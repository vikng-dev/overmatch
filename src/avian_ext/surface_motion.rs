//! Finite-inertia moving surfaces inside Avian's contact solve. The fork supplies scalar
//! coordinates and a per-constraint hook; this seam owns their Coulomb law. A friction impulse
//! changes both rigid bodies and every moving-surface coordinate before the next contact reads
//! them. Avian's graph colors preserve that ownership, including its serial overflow color.

use avian3d::dynamics::solver::{
    contact::{ContactConstraint, SurfaceFrictionStage},
    solver_body::{SolverBody, SolverBodyInertia, SurfaceCoordinate},
};
use bevy::math::{DVec2, Vec2, Vec3};

fn coordinate(body: &SolverBody, coordinate: SurfaceCoordinate) -> (f32, f32) {
    let state = body
        .surface_speeds
        .get(coordinate.index)
        .expect("a surface constraint addresses its owner's prepared scalar coordinate");
    assert!(
        state.velocity.is_finite()
            && state.inverse_inertia.is_finite()
            && state.inverse_inertia >= 0.0,
        "a prepared surface coordinate has finite velocity and nonnegative finite mobility"
    );
    (state.velocity, state.inverse_inertia)
}

fn exchange(body: &mut SolverBody, coordinate: SurfaceCoordinate, impulse: Vec3) {
    let state = body
        .surface_speeds
        .get_mut(coordinate.index)
        .expect("a surface constraint addresses its owner's prepared scalar coordinate");
    let p = coordinate.direction.dot(impulse);
    assert!(
        state.velocity.is_finite()
            && state.inverse_inertia.is_finite()
            && state.inverse_inertia >= 0.0
            && p.is_finite(),
        "prepared surface coordinates and solver impulses are finite, with nonnegative mobility"
    );
    let before = state.velocity;
    state.velocity += state.inverse_inertia * p;
    assert!(
        state.velocity.is_finite(),
        "a finite contact impulse produces a finite surface velocity"
    );
    state.work += f64::from(p) * (f64::from(before) + f64::from(state.velocity)) * 0.5;
}

/// The minimizer of `p' W p / 2 + g' p` on the Coulomb disk. Diagonalizing the positive
/// semidefinite metric retains zero-mobility directions without dividing by zero. A boundary
/// minimizer has `p = -(W + lambda I)^-1 g`; its norm decreases monotonically with lambda.
fn disk_minimum(w: [f64; 3], g: DVec2, radius: f64) -> DVec2 {
    assert!(
        radius.is_finite() && radius >= 0.0,
        "a contact supplies a finite nonnegative friction budget"
    );
    if radius == 0.0 {
        return DVec2::ZERO;
    }
    assert!(
        w.iter().all(|v| v.is_finite()) && g.is_finite(),
        "finite inertias and contact velocities produce a finite friction metric and gradient"
    );
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

/// Called by Avian under the same body ownership as its ordinary contact row. Warm start has
/// already applied the rigid-body impulse; its scalar counterpart is applied exactly once here.
/// The hook's accumulator holds the impulse actually applied, including warm-start scaling.
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
            if let Some(c) = coordinates[0] {
                exchange(body1, c, p * coefficient);
            }
            if let Some(c) = coordinates[1] {
                exchange(body2, c, p * coefficient);
            }
            part.impulse *= coefficient;
            continue;
        }
        let [r1, r2] = [point.anchor1, point.anchor2];
        let mut velocity =
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
        let mut w = [mass(0, 0), mass(0, 1), mass(1, 1)];
        for (body, c) in [(&*body1, coordinates[0]), (&*body2, coordinates[1])] {
            if let Some(c) = c {
                let (speed, inverse) = coordinate(body, c);
                velocity += c.direction * speed;
                let q = Vec2::new(c.direction.dot(tangents[0]), c.direction.dot(tangents[1]))
                    .as_dvec2();
                w[0] += f64::from(inverse) * q.x * q.x;
                w[1] += f64::from(inverse) * q.x * q.y;
                w[2] += f64::from(inverse) * q.y * q.y;
            }
        }
        let before = part.impulse.as_dvec2();
        let slip = DVec2::new(
            f64::from(velocity.dot(tangents[0])),
            f64::from(velocity.dot(tangents[1])),
        );
        let g = slip
            - DVec2::new(
                w[0] * before.x + w[1] * before.y,
                w[1] * before.x + w[2] * before.y,
            );
        let next = disk_minimum(
            w,
            g,
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
        if let Some(c) = coordinates[0] {
            exchange(body1, c, p);
        }
        if let Some(c) = coordinates[1] {
            exchange(body2, c, p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avian3d::collision::contact_types::ContactId;
    use avian3d::dynamics::solver::{
        contact::{
            ContactConstraintPoint, ContactNormalPart, ContactTangentPart, SurfaceFrictionHook,
        },
        softness_parameters::SoftnessCoefficients,
        solver_body::{SolverBodyIndex, SurfaceSpeed},
    };
    use avian3d::{math::SymmetricTensor, prelude::LockedAxes};

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
                    assert!(p.is_finite() && p.length() <= radius * (1.0 + 1e-12));
                    assert!(
                        work <= 1e-9,
                        "a passive friction exchange cannot supply energy: {work}"
                    );
                    if radius > 0.0 {
                        let gradient =
                            DVec2::new(w[0] * p.x + w[1] * p.y, w[1] * p.x + w[2] * p.y) + g;
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
            surface_hook: Some(SurfaceFrictionHook(solve)),
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
                    let inertia = SolverBodyInertia::new(
                        1.0 / mass,
                        SymmetricTensor::ZERO,
                        LockedAxes::default(),
                        0,
                        true,
                    );
                    let mut body = SolverBody {
                        surface_speeds: vec![SurfaceSpeed {
                            velocity: 1.68,
                            inverse_inertia: 1.0 / belt,
                            work: 0.0,
                        }],
                        ..Default::default()
                    };
                    let mut fixed = SolverBody::DUMMY;
                    let mut constraint = contact(mass);
                    let initial =
                        0.5 * f64::from(belt) * f64::from(body.surface_speeds[0].velocity).powi(2);
                    constraint.warm_start(
                        &mut body,
                        &mut fixed,
                        &inertia,
                        &SolverBodyInertia::DUMMY,
                        coefficient,
                    );
                    // Fixture: a normal press with no normal velocity; only friction is exercised.
                    constraint.points[0].normal_part.impulse = 1_000_000.0;
                    constraint.solve::<false>(
                        &mut body,
                        &mut fixed,
                        &inertia,
                        &SolverBodyInertia::DUMMY,
                        1.0 / 384.0,
                        avian3d::dynamics::solver::SolverConfig::default().max_overlap_solve_speed,
                    );
                    let final_energy =
                        0.5 * f64::from(mass) * body.linear_velocity.as_dvec3().length_squared()
                            + 0.5
                                * f64::from(belt)
                                * f64::from(body.surface_speeds[0].velocity).powi(2);
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
        let mut one = SolverBody {
            surface_speeds: vec![SurfaceSpeed {
                velocity: 1.68,
                inverse_inertia: 1.0 / 4750.0,
                work: 0.0,
            }],
            ..Default::default()
        };
        let mut two = SolverBody {
            surface_speeds: vec![SurfaceSpeed {
                velocity: -2.0,
                inverse_inertia: 1.0 / 2000.0,
                work: 0.0,
            }],
            ..Default::default()
        };
        let i1 = SolverBodyInertia::new(
            1.0 / 10_000.0,
            SymmetricTensor::ZERO,
            LockedAxes::default(),
            0,
            true,
        );
        let i2 = SolverBodyInertia::new(
            1.0 / 3000.0,
            SymmetricTensor::ZERO,
            LockedAxes::default(),
            0,
            true,
        );
        let expected =
            (1.68 + 2.0 + 0.4) / (1.0 / 10_000.0 + 1.0 / 3000.0 + 1.0 / 4750.0 + 1.0 / 2000.0);
        constraint.solve::<false>(
            &mut one,
            &mut two,
            &i1,
            &i2,
            1.0 / 384.0,
            avian3d::dynamics::solver::SolverConfig::default().max_overlap_solve_speed,
        );
        // Resolution: f32 exchange at each owner, relative to its fixture speed.
        assert!((one.surface_speeds[0].velocity - (1.68 - expected / 4750.0)).abs() < 1e-6);
        assert!((two.surface_speeds[0].velocity - (-2.0 + expected / 2000.0)).abs() < 1e-6);
    }
    /// A serialized custom row must be rebound before use; otherwise it must not fall back to
    /// an infinite-inertia prescribed surface with the finite coordinates silently ignored.
    #[test]
    #[should_panic(expected = "surface coordinates require their friction hook")]
    fn a_custom_surface_without_its_hook_is_a_bug() {
        let mut row = contact(1000.0); // Fixture body mass, kg.
        row.surface_hook = None;
        let mut one = SolverBody::DUMMY;
        let mut two = SolverBody::DUMMY;
        row.warm_start(
            &mut one,
            &mut two,
            &SolverBodyInertia::DUMMY,
            &SolverBodyInertia::DUMMY,
            1.0,
        );
    }
}
