//! Bounded linear rows over independent inertias. `solve` accepts free impulses, returning speeds
//! and row impulses in the caller's units. Rows sharing a speed solve together; this local dependency requires assignment trials to be serial.
//!
//! With fixed bounds the dual objective is `p·(A u - target) + p·W p/2`, where
//! `u = speed + free_impulse/inertia` and `W = A M^-1 A^T`. Its box optimality conditions
//! give unique speeds even when impulses are nonunique. Static bounds may only shrink to dynamic
//! bounds within a call. The drivetrain design owns the mechanism narrative.

/// Resolution: relative roundoff allowance for rank, impulse bounds and row signs in f64.
/// It resolves elimination error, never a physical rest-speed band.
const RESOLUTION: f64 = 1e-11;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Speed {
    pub value: f64,
    pub inertia: f64,
    pub free_impulse: f64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Bounds {
    pub lower: f64,
    pub upper: f64,
}

/// Positive impulse adds `jacobian[i] * impulse / inertia[i]` to speed i.
/// `dynamic` supplies sliding bounds for a static element; static bounds are available only
/// when its starting row speed equals `target` exactly. Nonstatic elements use `bounds` alone.
#[derive(Clone, Debug)]
pub(crate) struct Row {
    pub jacobian: Vec<f64>,
    pub target: f64,
    pub bounds: Bounds,
    pub dynamic: Option<Bounds>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Stuck,
    Lower,
    Upper,
}

#[derive(Debug)]
pub(crate) struct RowResult {
    pub impulse: f64,
    pub state: State,
    /// The row exhausted static capacity during this call, or began sliding.
    pub dynamic: bool,
}

#[derive(Debug)]
pub(crate) struct Solution {
    pub speeds: Vec<f64>,
    pub rows: Vec<RowResult>,
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}

pub(crate) fn solve(speeds: &[Speed], rows: &[Row]) -> Solution {
    assert!(
        speeds.iter().all(|s| s.value.is_finite()
            && s.inertia.is_finite()
            && s.inertia > 0.0
            && s.free_impulse.is_finite()),
        "block speeds have finite state and positive finite inertia"
    );
    let start: Vec<_> = speeds.iter().map(|s| s.value).collect();
    let free: Vec<_> = speeds
        .iter()
        .map(|s| s.value + s.free_impulse / s.inertia)
        .collect();
    assert!(
        free.iter().all(|v| v.is_finite()),
        "finite block impulses over admitted inertias produce finite free speeds"
    );
    let mut bounds = Vec::with_capacity(rows.len());
    let mut dynamic = vec![false; rows.len()];
    for (i, row) in rows.iter().enumerate() {
        assert!(
            row.jacobian.len() == speeds.len()
                && row.jacobian.iter().all(|a| a.is_finite())
                && row.target.is_finite(),
            "a block row supplies one finite coefficient per speed and a finite target"
        );
        assert!(
            row.bounds.lower.is_finite()
                && row.bounds.upper.is_finite()
                && row.bounds.lower <= row.bounds.upper,
            "a block row has finite ordered impulse bounds"
        );
        if let Some(sliding) = row.dynamic {
            assert!(
                sliding.lower.is_finite()
                    && sliding.upper.is_finite()
                    && row.bounds.lower <= sliding.lower
                    && sliding.lower <= sliding.upper
                    && sliding.upper <= row.bounds.upper,
                "dynamic bounds are a finite ordered subset of static bounds"
            );
            dynamic[i] = dot(&row.jacobian, &start) != row.target;
        }
        bounds.push(if dynamic[i] {
            row.dynamic
                .expect("only a static element has a sliding regime")
        } else {
            row.bounds
        });
    }
    let mut seed = vec![State::Stuck; rows.len()];
    loop {
        let search = Search {
            speeds,
            rows,
            bounds: &bounds,
            free: &free,
        };
        let mut accepted = None;
        for distance in 0..=rows.len() {
            accepted = search.neighbours(&seed, &mut Vec::with_capacity(rows.len()), distance);
            if accepted.is_some() {
                break;
            }
        }
        let mut result = accepted
            .expect("a finite convex box problem has an optimum with independent interior rows");
        let mut broke = false;
        for (i, row) in rows.iter().enumerate() {
            if !dynamic[i]
                && let Some(sliding) = row.dynamic
                && result.rows[i].state != State::Stuck
                && (dot(&row.jacobian, &result.speeds) - row.target).abs()
                    > RESOLUTION * (1.0 + row.target.abs())
            {
                bounds[i] = sliding;
                dynamic[i] = true;
                broke = true;
            }
            result.rows[i].dynamic = dynamic[i];
        }
        if !broke {
            return result;
        }
        seed = result.rows.iter().map(|r| r.state).collect();
    }
}

struct Search<'a> {
    speeds: &'a [Speed],
    rows: &'a [Row],
    bounds: &'a [Bounds],
    free: &'a [f64],
}

impl Search<'_> {
    // Increasing Hamming distance visits the accepted assignment before any of its neighbours.
    fn neighbours(
        &self,
        seed: &[State],
        states: &mut Vec<State>,
        distance: usize,
    ) -> Option<Solution> {
        let i = states.len();
        if i == seed.len() {
            return if distance == 0 {
                self.trial(states)
            } else {
                None
            };
        }
        for state in [seed[i], State::Stuck, State::Lower, State::Upper]
            .into_iter()
            .enumerate()
            .filter_map(|(at, state)| (at == 0 || state != seed[i]).then_some(state))
        {
            let changed = usize::from(state != seed[i]);
            if changed <= distance && distance - changed < seed.len() - i {
                states.push(state);
                let found = self.neighbours(seed, states, distance - changed);
                states
                    .pop()
                    .expect("the recursive trial just pushed its row");
                if found.is_some() {
                    return found;
                }
            }
        }
        None // No feasible assignment at this distance in this subtree.
    }

    fn trial(&self, states: &[State]) -> Option<Solution> {
        let stuck: Vec<_> = states
            .iter()
            .enumerate()
            .filter_map(|(i, s)| (*s == State::Stuck).then_some(i))
            .collect();
        let mut impulses: Vec<_> = states
            .iter()
            .zip(self.bounds)
            .map(|(state, bound)| match state {
                State::Stuck => 0.0,
                State::Lower => bound.lower,
                State::Upper => bound.upper,
            })
            .collect();
        let mut velocity = self.free.to_vec();
        for (row, p) in self.rows.iter().zip(&impulses) {
            for ((v, a), s) in velocity.iter_mut().zip(&row.jacobian).zip(self.speeds) {
                *v += a * p / s.inertia;
            }
        }
        let mut matrix: Vec<Vec<f64>> = stuck
            .iter()
            .map(|&i| {
                let mut line: Vec<_> = stuck
                    .iter()
                    .map(|&j| {
                        self.rows[i]
                            .jacobian
                            .iter()
                            .zip(&self.rows[j].jacobian)
                            .zip(self.speeds)
                            .map(|((a, b), s)| a * b / s.inertia)
                            .sum()
                    })
                    .collect();
                line.push(self.rows[i].target - dot(&self.rows[i].jacobian, &velocity));
                line
            })
            .collect();
        assert!(
            matrix.iter().flatten().all(|v| v.is_finite()),
            "admitted row coefficients and inertias produce a finite effective-mass system"
        );
        // A dependent stuck set has no unique impulse; a vertex assignment represents it instead.
        let solved = eliminate(&mut matrix)?;
        for (&row, p) in stuck.iter().zip(solved) {
            impulses[row] = p;
            for ((v, a), s) in velocity
                .iter_mut()
                .zip(&self.rows[row].jacobian)
                .zip(self.speeds)
            {
                *v += a * p / s.inertia;
            }
        }
        for (i, row) in self.rows.iter().enumerate() {
            let p = impulses[i];
            let bound = self.bounds[i];
            let allowance = RESOLUTION * (1.0 + bound.lower.abs().max(bound.upper.abs()));
            let slip = dot(&row.jacobian, &velocity) - row.target;
            let tolerance = RESOLUTION
                * (1.0
                    + row.target.abs()
                    + row
                        .jacobian
                        .iter()
                        .zip(&velocity)
                        .map(|(a, v)| (a * v).abs())
                        .sum::<f64>());
            if p < bound.lower - allowance
                || p > bound.upper + allowance
                || match states[i] {
                    State::Stuck => slip.abs() > tolerance,
                    State::Lower => bound.lower != bound.upper && slip < -tolerance,
                    State::Upper => bound.lower != bound.upper && slip > tolerance,
                }
            {
                return None;
            } // This assignment violates the box optimality conditions.
            impulses[i] = p.clamp(bound.lower, bound.upper);
        }
        snap(&mut velocity, self.rows, &stuck);
        assert!(
            velocity.iter().chain(&impulses).all(|v| v.is_finite()),
            "an accepted finite independent block produces finite speeds and impulses"
        );
        Some(Solution {
            speeds: velocity,
            rows: impulses
                .into_iter()
                .zip(states)
                .map(|(impulse, &state)| RowResult {
                    impulse,
                    state,
                    dynamic: false,
                })
                .collect(),
        })
    }
}

/// Gaussian elimination with row pivoting. `None` means a dependent stuck set, not a failed solve.
fn eliminate(matrix: &mut [Vec<f64>]) -> Option<Vec<f64>> {
    let n = matrix.len();
    // Normalize each row by its inertial norm on both axes: a change of row units then
    // leaves the rank test unchanged. The normalized Gram matrix has unit diagonal.
    let norms: Vec<_> = (0..n).map(|i| matrix[i][i].sqrt()).collect();
    if norms.contains(&0.0) {
        return None; // A zero row cannot be part of an independent stuck set.
    }
    for (i, line) in matrix.iter_mut().enumerate() {
        for j in 0..n {
            line[j] /= norms[i] * norms[j];
        }
        line[n] /= norms[i];
    }
    for col in 0..n {
        let pivot = (col..n)
            .max_by(|&a, &b| matrix[a][col].abs().total_cmp(&matrix[b][col].abs()))
            .expect("an unfinished square system has a pivot candidate");
        if matrix[pivot][col].abs() <= RESOLUTION {
            return None;
        }
        matrix.swap(col, pivot);
        for row in col + 1..n {
            let factor = matrix[row][col] / matrix[col][col];
            let (before, after) = matrix.split_at_mut(row);
            for (value, pivot) in after[0][col..=n].iter_mut().zip(&before[col][col..=n]) {
                *value -= factor * pivot;
            }
        }
    }
    let mut answer = vec![0.0; n];
    for i in (0..n).rev() {
        answer[i] = (matrix[i][n] - dot(&matrix[i][i + 1..n], &answer[i + 1..n])) / matrix[i][i];
    }
    for (value, norm) in answer.iter_mut().zip(norms) {
        *value /= norm;
    }
    Some(answer)
}

/// Eliminate speed coordinates, then substitute exact targets. In particular a coordinate stop
/// writes 0.0 directly and an equal-weight zero mean writes one speed as the other's negative.
fn snap(velocity: &mut [f64], rows: &[Row], stuck: &[usize]) {
    let n = velocity.len();
    let mut equations: Vec<_> = stuck
        .iter()
        .map(|&i| {
            let mut line = rows[i].jacobian.clone();
            line.push(rows[i].target);
            line
        })
        .collect();
    let mut pivots = Vec::with_capacity(stuck.len());
    for col in 0..n {
        let at = pivots.len();
        if at == equations.len() {
            break;
        }
        let pivot = (at..equations.len())
            .max_by(|&a, &b| equations[a][col].abs().total_cmp(&equations[b][col].abs()))
            .expect("an independent unfinished row has a coordinate pivot");
        if equations[pivot][col] == 0.0 {
            continue;
        } // This coordinate is free.
        equations.swap(at, pivot);
        for row in at + 1..equations.len() {
            let factor = equations[row][col] / equations[at][col];
            let (before, after) = equations.split_at_mut(row);
            for (value, pivot) in after[0][col..=n].iter_mut().zip(&before[at][col..=n]) {
                *value -= factor * pivot;
            }
        }
        pivots.push(col);
    }
    assert_eq!(
        pivots.len(),
        equations.len(),
        "accepted stuck rows are independent"
    );
    for (i, &col) in pivots.iter().enumerate().rev() {
        velocity[col] = (equations[i][n] - dot(&equations[i][col + 1..n], &velocity[col + 1..n]))
            / equations[i][col];
    }
}

#[cfg(test)]
mod tests;
