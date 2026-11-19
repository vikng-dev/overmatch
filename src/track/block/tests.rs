use super::*;

// Fixture: all inline inertias, impulses, capacities, targets and random-generator parameters in
// this module pose block topologies, not a vehicle's authored construction.
fn speed(value: f64, inertia: f64, free_impulse: f64) -> Speed {
    Speed {
        value,
        inertia,
        free_impulse,
    }
}

fn row(jacobian: &[f64], capacity: f64) -> Row {
    Row {
        jacobian: jacobian.to_vec(),
        target: 0.0,
        bounds: Bounds {
            lower: -capacity,
            upper: capacity,
        },
        dynamic: None,
    }
}

/// Cyclic coordinate minimization is independent of the production assignment/elimination
/// solver. It first solves with every eligible static bound, then shrinks only rows whose slip
/// cannot be held. The certificate must reject dynamic-only solvers even when their KKT signs fit.
fn reference_bounds(speeds: &[Speed], rows: &[Row]) -> (Vec<Bounds>, Vec<bool>) {
    // Resolution: reference iteration budget and work-gap allowance for these small f64 fixtures.
    const SWEEPS: usize = 100_000;
    const GAP: f64 = 1e-12;
    let mut dynamic: Vec<_> = rows
        .iter()
        .map(|row| {
            row.dynamic.is_some()
                && row
                    .jacobian
                    .iter()
                    .enumerate()
                    .fold(-row.target, |gap, (i, a)| gap + a * speeds[i].value)
                    != 0.0
        })
        .collect();
    loop {
        let bounds: Vec<_> = rows
            .iter()
            .zip(&dynamic)
            .map(|(row, &sliding)| {
                if sliding {
                    row.dynamic
                        .expect("a sliding reference row has dynamic bounds")
                } else {
                    row.bounds
                }
            })
            .collect();
        let mut velocity: Vec<_> = speeds
            .iter()
            .map(|s| s.value + s.free_impulse / s.inertia)
            .collect();
        let mut impulses = vec![0.0; rows.len()];
        let gradient = |row: &Row, velocity: &[f64]| {
            row.jacobian
                .iter()
                .zip(velocity)
                .fold(-row.target, |sum, (a, v)| sum + a * v)
        };
        let mut converged = false;
        for _ in 0..SWEEPS {
            for (i, row) in rows.iter().enumerate() {
                let curvature: f64 = row
                    .jacobian
                    .iter()
                    .zip(speeds)
                    .map(|(a, s)| a * a / s.inertia)
                    .sum();
                if curvature == 0.0 {
                    continue;
                } // A zero reference row cannot change speeds.
                let next = (impulses[i] - gradient(row, &velocity) / curvature)
                    .clamp(bounds[i].lower, bounds[i].upper);
                for (j, v) in velocity.iter_mut().enumerate() {
                    *v += row.jacobian[j] * (next - impulses[i]) / speeds[j].inertia;
                }
                impulses[i] = next;
            }
            let gap: f64 = rows
                .iter()
                .enumerate()
                .map(|(i, row)| {
                    let g = gradient(row, &velocity);
                    g * (impulses[i]
                        - if g > 0.0 {
                            bounds[i].lower
                        } else {
                            bounds[i].upper
                        })
                })
                .sum();
            if gap <= GAP {
                converged = true;
                break;
            }
        }
        assert!(
            converged,
            "the independent reference must converge before certifying a block"
        );
        let mut broke = false;
        for (i, row) in rows.iter().enumerate() {
            // Resolution: distinguish a reference minimizer's nonzero slip from iteration error.
            if !dynamic[i] && row.dynamic.is_some() && gradient(row, &velocity).abs() > 1e-7 {
                dynamic[i] = true;
                broke = true;
            }
        }
        if !broke {
            return (bounds, dynamic);
        }
    }
}

/// Independent convex optimality certificate: feasible impulses, momentum balance and the
/// variational inequality for every coordinate of the dual box imply its global minimum.
/// No elimination, assignment search, dot product or tolerance is shared with production.
fn certify(speeds: &[Speed], rows: &[Row], solution: &Solution) {
    // Resolution: independent certificate allowance for f64 accumulation and final target snap.
    const CHECK: f64 = 1e-8;
    let (expected_bounds, expected_dynamic) = reference_bounds(speeds, rows);
    for (i, input) in speeds.iter().enumerate() {
        let mut momentum = input.free_impulse;
        for (constraint, outcome) in rows.iter().zip(&solution.rows) {
            momentum += constraint.jacobian[i] * outcome.impulse;
        }
        let actual = input.inertia * (solution.speeds[i] - input.value);
        assert!(
            (actual - momentum).abs() <= CHECK * (1.0 + actual.abs() + momentum.abs()),
            "the drivetrain relies on equal impulse at every connected inertia"
        );
    }
    for (index, (constraint, outcome)) in rows.iter().zip(&solution.rows).enumerate() {
        assert_eq!(
            outcome.dynamic, expected_dynamic[index],
            "static capacity sufficiency is decided independently"
        );
        let bound = expected_bounds[index];
        assert!(outcome.impulse >= bound.lower && outcome.impulse <= bound.upper);
        let mut gradient = -constraint.target;
        for (i, coefficient) in constraint.jacobian.iter().enumerate() {
            gradient += coefficient * solution.speeds[i];
        }
        // In one dimension a linear functional attains its minimum at an endpoint. Checking
        // both endpoints proves gradient * (candidate - impulse) >= 0 throughout the interval.
        for endpoint in [bound.lower, bound.upper] {
            let work = gradient * (endpoint - outcome.impulse);
            assert!(
                work >= -CHECK * (1.0 + gradient.abs() * (bound.upper - bound.lower)),
                "drivers rely on rows satisfying convex optimality, not an iteration budget: {constraint:?} {outcome:?} gradient={gradient}"
            );
        }
        match solution.assignment[index] {
            State::Interior => assert!(gradient.abs() <= CHECK * (1.0 + constraint.target.abs())),
            State::Lower => assert_eq!(outcome.impulse, bound.lower),
            State::Upper => assert_eq!(outcome.impulse, bound.upper),
        }
    }
}

/// The block adapter relies on arbitrary and redundant construction rows having a global optimum.
/// Wrong bound signs, dropped reactions, or treating a dependent stuck set as failure break this.
#[test]
fn random_redundant_blocks_satisfy_an_independent_optimality_certificate() {
    let mut seed = 0x91e3_5137_u64;
    let mut random = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 32) as f64 / u32::MAX as f64
    };
    for case in 0..600 {
        let n = 1 + case % 5;
        let speeds: Vec<_> = (0..n)
            .map(|_| {
                speed(
                    if case % 4 == 0 {
                        0.0
                    } else {
                        4.0 * random() - 2.0
                    },
                    0.5 + 10.0 * random(),
                    8.0 * random() - 4.0,
                )
            })
            .collect();
        let mut rows: Vec<_> = (0..case % 6)
            .map(|_| {
                let mut r = row(
                    &(0..n).map(|_| 2.0 * random() - 1.0).collect::<Vec<_>>(),
                    0.1 + 8.0 * random(),
                );
                r.target = random() - 0.5;
                r.bounds.lower *= random();
                if case % 3 == 0 {
                    r.target = 0.0;
                    r.dynamic = Some(Bounds {
                        lower: 0.4 * r.bounds.lower,
                        upper: 0.4 * r.bounds.upper,
                    });
                }
                r
            })
            .collect();
        if case % 2 == 0 && !rows.is_empty() {
            let mut redundant = rows[0].clone();
            for a in &mut redundant.jacobian {
                *a *= 2.0;
            }
            redundant.target *= 2.0;
            rows.push(redundant);
        }
        if case % 3 == 0 {
            rows.push(row(&vec![0.0; n], 0.0));
        }
        certify(&speeds, &rows, &solve(&speeds, &rows));
        let perturbed: Vec<_> = speeds
            .iter()
            .map(|s| Speed {
                free_impulse: -s.free_impulse,
                ..*s
            })
            .collect();
        certify(&perturbed, &rows, &solve(&perturbed, &rows));
    }
}

/// A parked surface is exactly stationary for solver contacts. A tolerance-based rest snap or
/// retaining static capacity after breakaway violates this contract for the contact adapter.
#[test]
fn a_stop_snaps_to_zero_and_breakaway_uses_dynamic_capacity_in_the_same_step() {
    let mut stop = row(&[1.0], 6.0);
    stop.dynamic = Some(Bounds {
        lower: -2.0,
        upper: 2.0,
    });
    let stops = [stop];
    let stop = &stops[0];
    let held = solve(&[speed(0.0, 3.0, 5.0)], &stops);
    assert_eq!(held.speeds, [0.0]);
    assert_eq!(held.assignment[0], State::Interior);
    assert!(!held.rows[0].dynamic);
    for (initial, impulse) in [(0.0, 7.0), (0.01, 0.0), (-0.01, 0.0), (1.0, 0.0)] {
        let input = speed(initial, 3.0, impulse);
        let result = solve(&[input], &stops);
        certify(&[input], &stops, &result);
        if impulse > stop.bounds.upper {
            let dynamic = stop.dynamic.expect("the fixture has sliding capacity");
            // Resolution: the free step and row response round separately in f64.
            assert!(
                (result.speeds[0] - (impulse + dynamic.lower) / input.inertia).abs()
                    <= 16.0 * f64::EPSILON * result.speeds[0].abs()
            );
            assert!(result.rows[0].dynamic);
        } else {
            assert!(
                result.rows[0].dynamic,
                "any nonzero starting slip is dynamic, however small"
            );
        }
    }
    let moving = speed(4.0, 3.0, 0.0);
    let sliding = solve(&[moving], &stops);
    assert_eq!(
        sliding.speeds[0],
        moving.value - 2.0 / moving.inertia,
        "the player feels deceleration from dynamic impulse over inertia"
    );
}

/// A locked differential transfers unequal external loads between outputs. Independent side
/// clamping would release the overloaded stop although the pair can carry the total demand.
#[test]
fn unequal_loads_share_through_a_differential_and_a_held_mean_is_exact() {
    let speeds = [speed(0.0, 3.0, 5.0), speed(0.0, 7.0, 1.0)];
    let rows = [
        row(&[1.0, -1.0], 10.0),
        row(&[1.0, 0.0], 4.0),
        row(&[0.0, 1.0], 4.0),
    ];
    let solution = solve(&speeds, &rows);
    certify(&speeds, &rows, &solution);
    assert_eq!(solution.speeds, [0.0, 0.0]);
    assert_ne!(solution.rows[0].impulse, 0.0);
    let moving = [speed(1.0, 3.0, 0.0), speed(-0.5, 7.0, 0.0)];
    let mean = [row(&[0.5, 0.5], 10.0)];
    let held = solve(&moving, &mean);
    certify(&moving, &mean, &held);
    assert_eq!(held.speeds[0] + held.speeds[1], 0.0);
}

/// A clutch-and-brake construction has no steering tie: two clutches connect one shaft to its
/// outputs, each with its own stop. Requiring the seed layout or three particular rows breaks it.
#[test]
fn clutch_and_brake_layout_uses_the_same_block() {
    let speeds = [
        speed(4.0, 2.0, 1.0),
        speed(0.0, 5.0, -1.0),
        speed(0.0, 8.0, -2.0),
    ];
    let rows = [
        row(&[1.0, -2.0, 0.0], 6.0),
        row(&[1.0, 0.0, -3.0], 4.0),
        row(&[0.0, 1.0, 0.0], 3.0),
        row(&[0.0, 0.0, 1.0], 2.0),
    ];
    let coupled = solve(&speeds, &rows);
    certify(&speeds, &rows, &coupled);
    let mut released = rows;
    released[0].bounds = Bounds {
        lower: 0.0,
        upper: 0.0,
    };
    let open = solve(&speeds, &released);
    certify(&speeds, &released, &open);
    assert_eq!(
        open.speeds[1], 0.0,
        "an opened side clutch lets that brake hold its output"
    );
    assert!(
        coupled.speeds[1] > open.speeds[1],
        "a loaded clutch can overpower that side's brake"
    );
}

/// Independent axes remain solvable regardless of row units. The drivetrain may mix angular
/// and linear ratios; a block-wide rank threshold would discard the smaller independent row.
#[test]
fn independent_rows_with_different_scales_both_stop_their_speeds() {
    // Fixture: unit inertias with unit free impulses; row units differ by six orders.
    let speeds = [speed(0.0, 1.0, 1.0), speed(0.0, 1.0, 1.0)];
    let rows = [row(&[1.0, 0.0], 10.0), row(&[0.0, 1e-6], 1e7)];
    let solution = solve(&speeds, &rows);
    certify(&speeds, &rows, &solution);
    assert_eq!(solution.speeds, [0.0, 0.0]);
    // Resolution: independently derived cancelling impulses, allowing f64 elimination roundoff.
    for (outcome, expected) in solution.rows.iter().zip([-1.0, -1e6]) {
        assert!((outcome.impulse - expected).abs() <= 1e-10 * expected.abs());
    }
}

/// The drivetrain's row units must not change a resisting impulse's sign or preserve static
/// capacity after breakaway. An absolute row-slip tolerance can accept either defect here.
#[test]
fn row_scaling_preserves_resisting_sign_and_static_breakaway() {
    // Fixture: a small negative free speed exceeds both static and dynamic stopping capacity.
    // Row scales change units only; impulse bounds transform inversely to the coefficients.
    let input = speed(0.0, 1.0, -4e-6);
    let static_capacity = 3e-6;
    let dynamic_capacity = 1e-6;
    for scale in [1.0, 1e-6, 1e6] {
        for static_element in [false, true] {
            let mut constraint = row(&[scale], static_capacity / scale);
            if static_element {
                constraint.dynamic = Some(Bounds {
                    lower: -dynamic_capacity / scale,
                    upper: dynamic_capacity / scale,
                });
            }
            let solution = solve(&[input], &[constraint]);
            let outcome = &solution.rows[0];
            assert_eq!(
                solution.assignment[0],
                State::Upper,
                "the resisting impulse opposes negative slip in every row unit"
            );
            assert_eq!(
                outcome.dynamic, static_element,
                "insufficient static capacity breaks away in the same call at every row scale"
            );
            let capacity = if static_element {
                dynamic_capacity
            } else {
                static_capacity
            };
            assert_eq!(outcome.impulse, capacity / scale);
            // DERIVED: free momentum plus the positive saturated resistance, over inertia.
            let expected = (input.free_impulse + capacity) / input.inertia;
            // Resolution: separate row-unit conversion and speed accumulation round in f64.
            assert!((solution.speeds[0] - expected).abs() <= 16.0 * f64::EPSILON * expected.abs());
        }
    }
}

/// Independently clip the two-impulse box into a polygon, then check the variational inequality
/// at every vertex. A linear functional's minimum is at a vertex, so this certifies the global
/// convex minimum without production assignments, multipliers, elimination or rank decisions.
fn certify_polygon(speeds: &[Speed], rows: &[Row], limits: &[ImpulseLimit], result: &Solution) {
    // Resolution: independent f64 momentum and work certificate allowance.
    const CHECK: f64 = 1e-8;
    let mut polygon = vec![
        [rows[0].bounds.lower, rows[1].bounds.lower],
        [rows[0].bounds.upper, rows[1].bounds.lower],
        [rows[0].bounds.upper, rows[1].bounds.upper],
        [rows[0].bounds.lower, rows[1].bounds.upper],
    ];
    for limit in limits {
        for (sign, bound) in [(1.0, limit.bounds.upper), (-1.0, -limit.bounds.lower)] {
            let value = |p: [f64; 2]| {
                sign * (limit.coefficients[0] * p[0] + limit.coefficients[1] * p[1]) - bound
            };
            let mut clipped = Vec::new();
            for i in 0..polygon.len() {
                let a = polygon[i];
                let b = polygon[(i + 1) % polygon.len()];
                let (va, vb) = (value(a), value(b));
                if va <= 0.0 {
                    clipped.push(a);
                }
                if (va < 0.0 && vb > 0.0) || (va > 0.0 && vb < 0.0) {
                    let t = va / (va - vb);
                    clipped.push([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]);
                }
            }
            polygon = clipped;
        }
    }
    assert!(!polygon.is_empty(), "zero is feasible");
    let impulse = [result.rows[0].impulse, result.rows[1].impulse];
    for (i, row) in rows.iter().enumerate() {
        assert!(impulse[i] >= row.bounds.lower - CHECK && impulse[i] <= row.bounds.upper + CHECK);
    }
    for limit in limits {
        let value = limit.coefficients[0] * impulse[0] + limit.coefficients[1] * impulse[1];
        assert!(value >= limit.bounds.lower - CHECK && value <= limit.bounds.upper + CHECK);
    }
    for (i, speed) in speeds.iter().enumerate() {
        let actual = speed.inertia * (result.speeds[i] - speed.value);
        let expected = speed.free_impulse
            + rows
                .iter()
                .enumerate()
                .map(|(j, r)| r.jacobian[i] * impulse[j])
                .sum::<f64>();
        assert!((actual - expected).abs() <= CHECK * (1.0 + actual.abs() + expected.abs()));
    }
    let gradient: Vec<_> = rows
        .iter()
        .map(|r| {
            r.jacobian
                .iter()
                .zip(&result.speeds)
                .map(|(a, v)| a * v)
                .sum::<f64>()
                - r.target
        })
        .collect();
    for vertex in polygon {
        let work: f64 = (0..2).map(|i| gradient[i] * (vertex[i] - impulse[i])).sum();
        let scale: f64 = (0..2)
            .map(|i| gradient[i].abs() * (vertex[i].abs() + impulse[i].abs()))
            .sum();
        assert!(
            work >= -CHECK * (1.0 + scale),
            "aggregate optimum violates vertex certificate: {work}"
        );
    }
}

/// Callers rely on unused rated capacity remaining available to other paths. A filter on the
/// original box assignments misses interior sliding optima on aggregate faces.
#[test]
fn combination_faces_have_checker_certified_optima_including_interior_sliding() {
    let mut seed = 918713_u64; // Fixture generator seed and coefficients.
    let mut random = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        (seed >> 32) as f64 / u32::MAX as f64
    };
    for case in 0..600 {
        let speeds = [
            speed(4.0 * random() - 2.0, 0.5 + random() * 5.0, random()),
            speed(4.0 * random() - 2.0, 0.5 + random() * 5.0, -random()),
        ];
        let mut rows = [
            row(&[1.0, random()], 0.5 + random() * 4.0),
            row(&[random(), 1.0], 0.5 + random() * 4.0),
        ];
        if case % 3 == 0 {
            rows[1].jacobian = rows[0].jacobian.clone();
        }
        rows[0].target = random() - 0.5;
        rows[1].target = random() - 0.5;
        let mut limits = [
            ImpulseLimit {
                coefficients: vec![1.0, 0.1 + random()],
                bounds: Bounds {
                    lower: -random(),
                    upper: random(),
                },
            },
            ImpulseLimit {
                coefficients: vec![-random(), 1.0],
                bounds: Bounds {
                    lower: -random(),
                    upper: random(),
                },
            },
        ];
        if case % 5 == 0 {
            limits[1].coefficients = limits[0].coefficients.iter().map(|c| -2.0 * c).collect();
        }
        if case % 7 == 0 {
            limits[0].coefficients = vec![1.0, 0.0];
        }
        let result = solve_limited(&speeds, &rows, &limits, &mut WarmStart::default());
        certify_polygon(&speeds, &rows, &limits, &result);
    }
    // Fixture: unrestricted optimum (-1,-1); the aggregate allows only p0+p1 >= -1.
    // The constrained optimum (-0.5,-0.5) has nonzero slip on BOTH interior rows.
    let speeds = [speed(1.0, 1.0, 0.0), speed(1.0, 1.0, 0.0)];
    let rows = [row(&[1.0, 0.0], 2.0), row(&[0.0, 1.0], 2.0)];
    let limits = [ImpulseLimit {
        coefficients: vec![1.0, 1.0],
        bounds: Bounds {
            lower: -1.0,
            upper: 1.0,
        },
    }];
    let result = solve_limited(&speeds, &rows, &limits, &mut WarmStart::default());
    certify_polygon(&speeds, &rows, &limits, &result);
    assert_eq!(result.speeds, [0.5, 0.5]);
}

/// Dependent physical equalities can have unique impulses through a combination constraint.
/// Snapping must retain an independent speed basis, not reject a valid aggregate assignment.
#[test]
fn a_combination_can_resolve_redundant_impulses_without_duplicate_speed_snaps() {
    let speeds = [speed(1.0, 1.0, 0.0)];
    let rows = [row(&[1.0], 2.0), row(&[1.0], 2.0)];
    let limits = [ImpulseLimit {
        coefficients: vec![1.0, -1.0],
        bounds: Bounds {
            lower: 0.0,
            upper: 0.0,
        },
    }];
    let result = solve_limited(&speeds, &rows, &limits, &mut WarmStart::default());
    certify_polygon(&speeds, &rows, &limits, &result);
    assert_eq!(result.speeds, [0.0]);
}

/// A generic caller whose finite inputs overflow the effective mass has broken the numeric
/// precondition. The failure is visible before any NaN can pass feasibility comparisons.
#[test]
#[should_panic(expected = "finite effective-mass system")]
fn overflowing_combination_system_is_a_visible_bug() {
    let speeds = [speed(0.0, 1e-300, 0.0)];
    let rows = [row(&[1e100], 1.0)]; // Fixture finite inputs whose Gram product overflows f64.
    let limits = [ImpulseLimit {
        coefficients: vec![1.0],
        bounds: Bounds {
            lower: -1.0,
            upper: 1.0,
        },
    }];
    solve_limited(&speeds, &rows, &limits, &mut WarmStart::default());
}

/// The same constrained optimum must survive changes of row-impulse units. A rank decision
/// based on unscaled KKT columns would reject the interior aggregate face at these scales.
#[test]
fn combination_optimality_is_invariant_under_row_impulse_units() {
    let speeds = [speed(1.0, 1.0, 0.0), speed(1.0, 1.0, 0.0)];
    for scale in [1e-12, 1e-6, 1.0, 1e6, 1e12] {
        // Fixture unit changes, not physical ratings.
        let rows = [
            row(&[scale, 0.0], 2.0 / scale),
            row(&[0.0, 1.0 / scale], 2.0 * scale),
        ];
        let limits = [ImpulseLimit {
            coefficients: vec![scale, 1.0 / scale],
            bounds: Bounds {
                lower: -1.0,
                upper: 1.0,
            },
        }];
        let result = solve_limited(&speeds, &rows, &limits, &mut WarmStart::default());
        certify_polygon(&speeds, &rows, &limits, &result);
        // Resolution: f64 elimination and inverse changes of units.
        assert!(result.speeds.iter().all(|v| (v - 0.5).abs() < 1e-10));
    }
    // Fixture: a linear zero-inertia objective coordinate is fixed by an aggregate face.
    let mut linear = row(&[0.0], 2.0);
    linear.target = 1.0;
    let rows = [linear, row(&[1.0], 2.0)];
    let limits = [ImpulseLimit {
        coefficients: vec![1.0, 0.0],
        bounds: Bounds {
            lower: 0.0,
            upper: 1.0,
        },
    }];
    let result = solve_limited(&speeds[..1], &rows, &limits, &mut WarmStart::default());
    certify_polygon(&speeds[..1], &rows, &limits, &result);
}

/// A zero-width impulse is fixed, not an equality on physical speed. Inactive connections
/// must deliver EXACTLY no impulse despite a nonzero target gap and an aggregate bound.
#[test]
fn singleton_impulses_do_not_create_a_physical_equality_or_roundoff_force() {
    let speeds = [speed(100.0, 1.0, 0.0), speed(0.0, 2.0, 0.0)];
    let rows = [row(&[1.0, -1.0], 0.0), row(&[0.0, 1.0], 10.0)];
    let limits = [ImpulseLimit {
        coefficients: vec![1.0, 0.0],
        bounds: Bounds {
            lower: 0.0,
            upper: 1.0,
        },
    }];
    let result = solve_limited(&speeds, &rows, &limits, &mut WarmStart::default());
    certify_polygon(&speeds, &rows, &limits, &result);
    assert_eq!(
        result.rows.iter().map(|r| r.impulse).collect::<Vec<_>>(),
        [0.0, 0.0]
    );
    assert_eq!(result.speeds, [100.0, 0.0]);
}

/// Aggregate units change the dual multiplier inversely. Its sign test must scale with the
/// normal, or a wrong lower face can precede the actual upper-face optimum in exact search.
#[test]
fn combination_units_cannot_accept_the_wrong_optimality_sign() {
    for sign in [-1.0, 1.0] {
        let speeds = [speed(sign, 1.0, 0.0), speed(sign, 1.0, 0.0)];
        let rows = [row(&[1.0, 0.0], 2.0), row(&[0.0, 1.0], 2.0)];
        for scale in [1e-12, 1.0, 1e12] {
            // Fixture changes of aggregate units only.
            let limits = [ImpulseLimit {
                coefficients: vec![scale, scale],
                bounds: Bounds {
                    lower: -scale,
                    upper: scale,
                },
            }];
            let result = solve_limited(&speeds, &rows, &limits, &mut WarmStart::default());
            certify_polygon(&speeds, &rows, &limits, &result);
            // Resolution: independently derived symmetric optimum, f64 accumulation allowance.
            assert!(result.speeds.iter().all(|v| (v - sign * 0.5).abs() < 1e-10));
        }
    }
}

/// Independent ADMM reference: split every box and combination into a clamped coordinate.
/// Cholesky of the positive-definite augmented objective shares no assignment or rank search.
fn reference_limited(
    speeds: &[Speed],
    rows: &[Row],
    limits: &[ImpulseLimit],
) -> (Vec<f64>, Vec<bool>) {
    let n = rows.len();
    let mut normals: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| f64::from(i == j)).collect())
        .collect();
    normals.extend(limits.iter().map(|l| l.coefficients.clone()));
    let free: Vec<_> = speeds
        .iter()
        .map(|s| s.value + s.free_impulse / s.inertia)
        .collect();
    let linear: Vec<_> = rows
        .iter()
        .map(|r| {
            r.jacobian
                .iter()
                .zip(&free)
                .map(|(a, v)| a * v)
                .sum::<f64>()
                - r.target
        })
        .collect();
    let mut chol = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..=i {
            let entry: f64 = rows[i]
                .jacobian
                .iter()
                .zip(&rows[j].jacobian)
                .zip(speeds)
                .map(|((a, b), s)| a * b / s.inertia)
                .sum::<f64>()
                + normals.iter().map(|r| r[i] * r[j]).sum::<f64>()
                - (0..j).map(|k| chol[i][k] * chol[j][k]).sum::<f64>();
            chol[i][j] = if i == j {
                entry.sqrt()
            } else {
                entry / chol[j][j]
            };
        }
    }
    let mut dynamic: Vec<_> = rows
        .iter()
        .map(|r| {
            r.dynamic.is_some()
                && r.jacobian
                    .iter()
                    .zip(speeds)
                    .map(|(a, s)| a * s.value)
                    .sum::<f64>()
                    != r.target
        })
        .collect();
    loop {
        let mut bounds: Vec<_> = rows
            .iter()
            .zip(&dynamic)
            .map(|(r, d)| if *d { r.dynamic.unwrap() } else { r.bounds })
            .collect();
        bounds.extend(limits.iter().map(|l| l.bounds));
        let mut p = vec![0.0; n];
        let mut z = vec![0.0; normals.len()];
        let mut dual = z.clone();
        let mut converged = false;
        // Resolution: reference primal/dual residual and iteration watchdog, independent of production.
        for _ in 0..200_000 {
            let mut rhs: Vec<_> = (0..n)
                .map(|i| {
                    -linear[i]
                        + normals
                            .iter()
                            .enumerate()
                            .map(|(j, r)| r[i] * (z[j] - dual[j]))
                            .sum::<f64>()
                })
                .collect();
            for i in 0..n {
                rhs[i] = (rhs[i] - (0..i).map(|j| chol[i][j] * rhs[j]).sum::<f64>()) / chol[i][i];
            }
            for i in (0..n).rev() {
                p[i] = (rhs[i] - (i + 1..n).map(|j| chol[j][i] * p[j]).sum::<f64>()) / chol[i][i];
            }
            let mut residual = 0.0_f64;
            for (j, normal) in normals.iter().enumerate() {
                let value = normal.iter().zip(&p).map(|(a, b)| a * b).sum::<f64>();
                let next = (value + dual[j]).clamp(bounds[j].lower, bounds[j].upper);
                residual = residual.max((next - z[j]).abs()).max((value - next).abs());
                z[j] = next;
                dual[j] += value - next;
            }
            if residual < 1e-11 {
                converged = true;
                break;
            }
        }
        assert!(converged, "independent limited reference must converge");
        let velocity: Vec<_> = (0..speeds.len())
            .map(|j| {
                free[j]
                    + rows
                        .iter()
                        .zip(&p)
                        .map(|(r, p)| r.jacobian[j] * p / speeds[j].inertia)
                        .sum::<f64>()
            })
            .collect();
        let mut broke = false;
        for (i, r) in rows.iter().enumerate() {
            let slip = r
                .jacobian
                .iter()
                .zip(&velocity)
                .map(|(a, v)| a * v)
                .sum::<f64>()
                - r.target;
            // Resolution: iteration error cannot break a reference static equality.
            if !dynamic[i] && r.dynamic.is_some() && slip.abs() > 1e-7 {
                dynamic[i] = true;
                broke = true;
            }
        }
        if !broke {
            return (velocity, dynamic);
        }
    }
}

/// Production callers rely on the constrained speeds and one-way breakaway regime with three
/// speeds and dependent clutch, steering, stop and ceiling rows; two-row polygons cannot prove it.
#[test]
fn n_row_reference_covers_dynamic_restart_and_the_production_topology() {
    for case in 0..48 {
        let moving = case % 3 == 0;
        let speeds = [
            speed(1.3, 1.1, 0.2),
            speed(if moving { 0.2 } else { 0.0 }, 2.0, -1.0),
            speed(0.0, 2.0, -0.7),
        ];
        let k = 1.0 + case as f64 / 12.0;
        let c = if case % 4 == 0 { 0.0 } else { 0.2 };
        let mut rows = vec![
            row(&[1.0, -k / 2.0, -k / 2.0], 1.2),
            row(&[-c, 0.5, -0.5], 2.0),
        ];
        for a in [[0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
            let mut r = row(&a, 0.8);
            r.bounds.lower = 0.0;
            r.dynamic = Some(Bounds {
                lower: 0.0,
                upper: 0.3,
            });
            rows.push(r);
        }
        let mut ceiling = row(&[0.0, 0.5, 0.5], 0.4);
        ceiling.bounds.upper = 0.0;
        ceiling.target = 0.2;
        rows.push(ceiling);
        let limits = [ImpulseLimit {
            coefficients: vec![1.0, -c, 0.0, 0.0, 0.0],
            bounds: Bounds {
                lower: -0.2,
                upper: 0.4,
            },
        }];
        let result = solve_limited(&speeds, &rows, &limits, &mut WarmStart::default());
        let (reference, dynamic) = reference_limited(&speeds, &rows, &limits);
        for (a, b) in result.speeds.iter().zip(reference) {
            assert!((a - b).abs() < 1e-7, "constrained speed differs: {a} {b}");
        }
        assert_eq!(
            result.rows.iter().map(|r| r.dynamic).collect::<Vec<_>>(),
            dynamic
        );
        for (j, s) in speeds.iter().enumerate() {
            let impulse = s.free_impulse
                + rows
                    .iter()
                    .zip(&result.rows)
                    .map(|(r, p)| r.jacobian[j] * p.impulse)
                    .sum::<f64>();
            assert!((s.inertia * (result.speeds[j] - s.value) - impulse).abs() < 1e-8);
        }
        for (r, p) in rows.iter().zip(&result.rows) {
            let bound = if p.dynamic {
                r.dynamic.unwrap()
            } else {
                r.bounds
            };
            assert!(p.impulse >= bound.lower && p.impulse <= bound.upper);
        }
        let total = limits[0]
            .coefficients
            .iter()
            .zip(&result.rows)
            .map(|(a, p)| a * p.impulse)
            .sum::<f64>();
        assert!(total >= limits[0].bounds.lower - 1e-9 && total <= limits[0].bounds.upper + 1e-9);
    }
}

/// The admission gate must distinguish exact redundancy from an independent basis below the
/// runtime rank threshold; otherwise authored ratios can reach the solver's invariant panic.
#[test]
fn near_dependent_construction_is_refused_while_exact_dependencies_are_admitted() {
    for angle in [1e-6, 1e-12, 1e-20] {
        // Fixture: independent rows below the solver's squared-angle resolution.
        assert!(!conditioning_resolvable(
            &[1.0, 1.0, 1.0],
            &[[1.0, 0.0, 0.0], [1.0, angle, 0.0]],
            0
        ));
    }
    assert!(conditioning_resolvable(
        &[1.0, 1.0, 1.0],
        &[[1.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
        0
    ));
}

/// Repeated owner solves rely on reusing the accepted faces, while changed row identities
/// must start without history. Fixture: one saturated row whose optimum is the lower bound.
#[test]
#[cfg(feature = "bitprobe")]
fn accepted_faces_warm_the_next_solve_until_the_layout_changes() {
    let speeds = [Speed {
        value: 4.0,
        inertia: 2.0,
        free_impulse: 0.0,
    }];
    let mut rows = [Row {
        jacobian: vec![1.0],
        target: 0.0,
        bounds: Bounds {
            lower: -1.0,
            upper: 1.0,
        },
        dynamic: None,
    }];
    let mut warm = WarmStart::default();
    let cold = solve_limited(&speeds, &rows, &[], &mut warm);
    let hot = solve_limited(&speeds, &rows, &[], &mut warm);
    assert_eq!(hot.assignments_tried, 1);
    assert_eq!(hot.speeds, cold.speeds);
    // Fixture: a changed row with the same winning face. Reusing history would take one trial,
    // while a fresh Interior seed must reject its over-capacity impulse before finding that face.
    rows[0].jacobian[0] = 2.0;
    let changed = solve_limited(&speeds, &rows, &[], &mut warm);
    let fresh = solve_limited(&speeds, &rows, &[], &mut WarmStart::default());
    assert_eq!(changed.assignments_tried, fresh.assignments_tried);
    assert_eq!(changed.speeds, fresh.speeds);
}

/// Contacts rely on a held stop having zero mobility and a locked shaft reflecting its inertia.
/// The same row at capacity must release that mobility; a timestep is absent from this law.
#[test]
fn constrained_mobility_reflects_inertia_and_releases_exhausted_stops() {
    let speeds = [speed(0.0, 3.0, 0.0), speed(0.0, 7.0, 0.0)];
    let clutch = row(&[1.0, -2.0], 100.0);
    let locked = solve(&speeds, std::slice::from_ref(&clutch));
    let mobility = locked
        .response(&speeds, std::slice::from_ref(&clutch), &[])
        .mobility;
    let expected = 1.0 / (7.0 + 3.0 * 2.0 * 2.0);
    // Resolution: f64 KKT elimination and inertial projection.
    assert!((mobility[1][1] - expected).abs() < 1e-12);
    assert!((mobility[1][0] - 2.0 * expected).abs() < 1e-12);
    let shifted_speeds = [speeds[0], speed(0.0, 7.0, 3.0)];
    let shifted = solve(&shifted_speeds, std::slice::from_ref(&clutch));
    assert!((shifted.speeds[1] - 3.0 * expected).abs() < 1e-12);
    let rows = [clutch, row(&[0.0, 1.0], 4.0)];
    for impulse in [2.0, 8.0] {
        let loaded = [speeds[0], speed(0.0, 7.0, impulse)];
        let solved = solve(&loaded, &rows);
        let mobility = solved.response(&loaded, &rows, &[]).mobility;
        if impulse < 4.0 {
            assert_eq!(mobility[1][1], 0.0);
            assert_eq!(solved.speeds[1], 0.0);
        } else {
            assert!((mobility[1][1] - expected).abs() < 1e-12);
            assert!(solved.speeds[1] > 0.0);
        }
    }
}
