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
        match outcome.state {
            State::Stuck => assert!(gradient.abs() <= CHECK * (1.0 + constraint.target.abs())),
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
    assert_eq!(held.rows[0].state, State::Stuck);
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
                outcome.state,
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
