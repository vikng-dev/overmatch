//! Bounded linear rows over independent inertias. `solve_limited` accepts free impulses, returning speeds
//! and row impulses in the caller's units. Rows sharing a speed or an impulse limit solve
//! together; this local dependency requires assignment trials to be serial.
//!
//! With fixed bounds the dual objective is `p·(A u - target) + p·W p/2`, where
//! `u = speed + free_impulse/inertia` and `W = A M^-1 A^T`. Optional limits bound linear
//! combinations of impulses. Convex optimality gives unique speeds even when impulses are
//! nonunique. Static bounds may only shrink to dynamic
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

impl Row {
    pub(super) fn slip_tolerance(&self, velocity: &[f64]) -> f64 {
        // Resolution: coefficient magnitudes scale the unit-speed roundoff floor into row
        // units. Target and evaluation magnitudes cover cancellation without a rest-speed band.
        RESOLUTION
            * (self.target.abs()
                + self
                    .jacobian
                    .iter()
                    .zip(velocity)
                    .map(|(a, v)| a.abs() * (1.0 + v.abs()))
                    .sum::<f64>())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// An interior box impulse; aggregate multipliers may leave physical slip nonzero.
    Interior,
    Lower,
    Upper,
}

#[derive(Debug)]
pub(crate) struct RowResult {
    pub impulse: f64,
    /// The row exhausted static capacity during this call, or began sliding.
    pub dynamic: bool,
}

#[derive(Debug)]
pub(crate) struct Solution {
    pub speeds: Vec<f64>,
    pub rows: Vec<RowResult>,
    /// Deterministic work across every static-to-dynamic stage.
    #[cfg(feature = "bitprobe")]
    pub assignments_tried: usize,
    assignment: Vec<State>,
}

/// The accepted KKT faces remain a valid search seed while row identities stay unchanged.
/// Bounds and targets may move; each trial still checks feasibility and optimality afresh.
#[derive(Default)]
pub(crate) struct WarmStart {
    layout: Vec<(Vec<f64>, bool)>,
    limits: Vec<Vec<f64>>,
    assignment: Vec<State>,
}

impl WarmStart {
    fn seed(&mut self, rows: &[Row], limits: &[ImpulseLimit]) -> Vec<State> {
        let matches = self.layout.len() == rows.len()
            && self.limits.len() == limits.len()
            && self
                .layout
                .iter()
                .zip(rows)
                .all(|((jacobian, dynamic), row)| {
                    *jacobian == row.jacobian && *dynamic == row.dynamic.is_some()
                })
            && self
                .limits
                .iter()
                .zip(limits)
                .all(|(a, b)| *a == b.coefficients);
        if !matches {
            self.layout = rows
                .iter()
                .map(|r| (r.jacobian.clone(), r.dynamic.is_some()))
                .collect();
            self.limits = limits.iter().map(|l| l.coefficients.clone()).collect();
            self.assignment = vec![State::Interior; rows.len() + limits.len()];
        }
        self.assignment.clone()
    }
}

#[cfg(feature = "bitprobe")]
thread_local! {
    static ASSIGNMENT_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Per-worker diagnostic work; a mechanical tick drains it around its candidate and final solves.
#[cfg(feature = "bitprobe")]
pub(super) fn take_assignment_work() -> usize {
    ASSIGNMENT_WORK.with(|work| work.replace(0))
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}

/// A bound on a linear combination of row impulses. Bounds must admit zero, as must every
/// row when combinations are supplied, so the feasible polytope is nonempty in every regime.
#[derive(Clone, Debug)]
pub(crate) struct ImpulseLimit {
    pub coefficients: Vec<f64>,
    pub bounds: Bounds,
}

#[cfg(test)]
fn solve(speeds: &[Speed], rows: &[Row]) -> Solution {
    solve_limited(speeds, rows, &[], &mut WarmStart::default())
}

/// Enumerate box faces and active combination faces. The bordered KKT system includes the
/// combination's multiplier: interior impulses need not have zero physical slip at that face.
/// A compact nonempty polytope has an optimum; choose an extreme point of its minimizer face
/// and an independent normal-cone basis to obtain a nonsingular assignment.
pub(crate) fn solve_limited(
    speeds: &[Speed],
    rows: &[Row],
    limits: &[ImpulseLimit],
    warm: &mut WarmStart,
) -> Solution {
    for limit in limits {
        assert!(
            limit.coefficients.len() == rows.len()
                && limit.coefficients.iter().all(|c| c.is_finite())
                && limit.bounds.lower.is_finite()
                && limit.bounds.upper.is_finite()
                && limit.bounds.lower <= 0.0
                && limit.bounds.upper >= 0.0
                && rows.iter().all(|r| r.bounds.lower <= 0.0
                    && r.bounds.upper >= 0.0
                    && r.dynamic.is_none_or(|b| b.lower <= 0.0 && b.upper >= 0.0)),
            "combination limits and their rows must be finite and admit zero in every regime"
        );
    }
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
    let mut seed = warm.seed(rows, limits);
    for (i, bound) in bounds.iter().enumerate() {
        if bound.lower == bound.upper {
            seed[i] = State::Lower;
        }
    }
    #[cfg(feature = "bitprobe")]
    let mut assignments_tried = 0;
    loop {
        let search = Search {
            #[cfg(feature = "bitprobe")]
            assignments_tried: std::cell::Cell::new(0),
            speeds,
            rows,
            bounds: &bounds,
            limits,
            free: &free,
        };
        let mut result = (0..=seed.len())
            .find_map(|distance| {
                search.neighbours(&seed, &mut Vec::with_capacity(rows.len()), distance)
            })
            .expect("the loaded metric must leave an optimal independent KKT basis above the rank threshold; admission certifies only unloaded conditioning");
        #[cfg(feature = "bitprobe")]
        {
            assignments_tried += search.assignments_tried.get();
            result.assignments_tried = assignments_tried;
        }
        let mut broke = false;
        for (i, row) in rows.iter().enumerate() {
            if !dynamic[i]
                && let Some(sliding) = row.dynamic
                && (dot(&row.jacobian, &result.speeds) - row.target).abs()
                    > row.slip_tolerance(&result.speeds)
            {
                bounds[i] = sliding;
                dynamic[i] = true;
                broke = true;
            }
            result.rows[i].dynamic = dynamic[i];
        }
        if !broke {
            #[cfg(feature = "bitprobe")]
            ASSIGNMENT_WORK.with(|work| work.set(work.get() + assignments_tried));
            warm.assignment.clone_from(&result.assignment);
            return result;
        }
        seed = result.assignment;
    }
}

struct Search<'a> {
    #[cfg(feature = "bitprobe")]
    assignments_tried: std::cell::Cell<usize>,
    speeds: &'a [Speed],
    rows: &'a [Row],
    bounds: &'a [Bounds],
    free: &'a [f64],
    limits: &'a [ImpulseLimit],
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
        for state in [seed[i], State::Interior, State::Lower, State::Upper]
            .into_iter()
            .enumerate()
            .filter_map(|(at, state)| (at == 0 || state != seed[i]).then_some(state))
        {
            // A singleton impulse is a fixed coordinate, never an interior stationarity
            // equation. Its physical row can have arbitrary slip (an open coupling).
            if i < self.bounds.len()
                && self.bounds[i].lower == self.bounds[i].upper
                && state != State::Lower
            {
                continue;
            }
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
        #[cfg(feature = "bitprobe")]
        self.assignments_tried.set(self.assignments_tried.get() + 1);
        let n = self.rows.len();
        let interior: Vec<_> = (0..n).filter(|&i| states[i] == State::Interior).collect();
        let active: Vec<_> = (0..self.limits.len())
            .filter(|&i| states[n + i] != State::Interior)
            .collect();
        let mut impulses: Vec<_> = (0..n)
            .map(|i| match states[i] {
                State::Interior => 0.0,
                State::Lower => self.bounds[i].lower,
                State::Upper => self.bounds[i].upper,
            })
            .collect();
        let velocity_for = |impulses: &[f64]| -> Vec<f64> {
            self.speeds
                .iter()
                .enumerate()
                .map(|(j, speed)| {
                    self.free[j]
                        + self
                            .rows
                            .iter()
                            .zip(impulses)
                            .map(|(r, p)| r.jacobian[j] * p / speed.inertia)
                            .sum::<f64>()
                })
                .collect()
        };
        let fixed = velocity_for(&impulses);
        let nf = interior.len();
        let size = nf + active.len();
        let mut matrix = vec![vec![0.0; size + 1]; size];
        for (i, &ri) in interior.iter().enumerate() {
            for (j, &rj) in interior.iter().enumerate() {
                matrix[i][j] = self.rows[ri]
                    .jacobian
                    .iter()
                    .zip(&self.rows[rj].jacobian)
                    .zip(self.speeds)
                    .map(|((a, b), s)| a * b / s.inertia)
                    .sum();
            }
            matrix[i][size] = self.rows[ri].target - dot(&self.rows[ri].jacobian, &fixed);
        }
        for (j, &li) in active.iter().enumerate() {
            let limit = &self.limits[li];
            for (i, &ri) in interior.iter().enumerate() {
                matrix[i][nf + j] = limit.coefficients[ri];
                matrix[nf + j][i] = limit.coefficients[ri];
            }
            matrix[nf + j][size] = match states[n + li] {
                State::Lower => limit.bounds.lower,
                State::Upper => limit.bounds.upper,
                State::Interior => unreachable!("only active faces enter the bordered system"),
            } - dot(&limit.coefficients, &impulses);
        }
        assert!(
            matrix.iter().flatten().all(|v| v.is_finite()),
            "admitted row coefficients and inertias produce a finite effective-mass system"
        );
        let answer = eliminate_bordered(&mut matrix, nf)?;
        assert!(
            answer.iter().all(|v| v.is_finite()),
            "an independent finite bordered system produces finite impulses and multipliers"
        );
        for (&i, &p) in interior.iter().zip(&answer) {
            impulses[i] = p;
        }
        let mut velocity = velocity_for(&impulses);
        let multipliers = &answer[nf..];
        let mut snapped = Vec::new();
        for i in 0..n {
            let bound = self.bounds[i];
            let allowance = RESOLUTION * bound.lower.abs().max(bound.upper.abs());
            let correction: f64 = active
                .iter()
                .zip(multipliers)
                .map(|(&j, mu)| self.limits[j].coefficients[i] * mu)
                .sum();
            let slip = dot(&self.rows[i].jacobian, &velocity) - self.rows[i].target;
            let gradient = slip + correction;
            let tolerance = self.rows[i].slip_tolerance(&velocity) + RESOLUTION * correction.abs();
            if impulses[i] < bound.lower - allowance
                || impulses[i] > bound.upper + allowance
                || match states[i] {
                    State::Interior => gradient.abs() > tolerance,
                    State::Lower => bound.lower != bound.upper && gradient < -tolerance,
                    State::Upper => bound.lower != bound.upper && gradient > tolerance,
                }
            {
                return None;
            }
            impulses[i] = impulses[i].clamp(bound.lower, bound.upper);
            if states[i] == State::Interior && correction == 0.0 {
                snapped.push(i);
            }
        }
        for (j, limit) in self.limits.iter().enumerate() {
            let value = dot(&limit.coefficients, &impulses);
            let tolerance = RESOLUTION
                * (limit.bounds.lower.abs().max(limit.bounds.upper.abs())
                    + limit
                        .coefficients
                        .iter()
                        .zip(&impulses)
                        .map(|(c, p)| (c * p).abs())
                        .sum::<f64>());
            if value < limit.bounds.lower - tolerance || value > limit.bounds.upper + tolerance {
                return None;
            }
            if let Some(at) = active.iter().position(|&i| i == j) {
                let mu = multipliers[at];
                // Dividing a row residual allowance by this normal's coefficient gives
                // multiplier units. Rescaling an aggregate must not change its sign test.
                let allowance = interior
                    .iter()
                    .filter(|&&i| limit.coefficients[i] != 0.0)
                    .map(|&i| self.rows[i].slip_tolerance(&velocity) / limit.coefficients[i].abs())
                    .reduce(f64::min)
                    .expect("an independent active normal has a free coefficient");
                if limit.bounds.lower != limit.bounds.upper
                    && match states[n + j] {
                        State::Lower => mu > allowance,
                        State::Upper => mu < -allowance,
                        State::Interior => unreachable!("an active limit has a face"),
                    }
                {
                    return None;
                }
            }
        }
        // Only zero-multiplier physical equalities can be snapped. Aggregate-constrained
        // interior rows may slide; their KKT equality includes the limit's shadow price.
        let mut independent = Vec::new();
        for i in snapped {
            independent.push(i);
            let mut gram: Vec<Vec<f64>> = independent
                .iter()
                .map(|&a| {
                    let mut line: Vec<_> = independent
                        .iter()
                        .map(|&b| {
                            self.rows[a]
                                .jacobian
                                .iter()
                                .zip(&self.rows[b].jacobian)
                                .zip(self.speeds)
                                .map(|((a, b), s)| a * b / s.inertia)
                                .sum()
                        })
                        .collect();
                    line.push(0.0);
                    line
                })
                .collect();
            if eliminate_bordered(&mut gram, independent.len()).is_none() {
                independent.pop();
            }
        }
        snap(&mut velocity, self.rows, &independent);
        assert!(
            velocity.iter().chain(&impulses).all(|v| v.is_finite()),
            "an accepted finite independent block produces finite speeds and impulses"
        );
        Some(Solution {
            #[cfg(feature = "bitprobe")]
            assignments_tried: 0,
            assignment: states.to_vec(),
            speeds: velocity,
            rows: impulses
                .into_iter()
                .map(|impulse| RowResult {
                    impulse,
                    dynamic: false,
                })
                .collect(),
        })
    }
}

/// Symmetric equilibration preserves row-unit invariance in the indefinite KKT system.
/// Free impulses use their inertial norms; multipliers use their constraint's norm in those
/// coordinates. A zero-inertia objective coordinate must be fixed by an active combination.
fn eliminate_bordered(matrix: &mut [Vec<f64>], interior: usize) -> Option<Vec<f64>> {
    let n = matrix.len();
    let mut norms = vec![0.0; n];
    for i in 0..interior {
        norms[i] = matrix[i][i].sqrt();
        if norms[i] == 0.0 {
            norms[i] = matrix[interior..]
                .iter()
                .map(|line| line[i].abs())
                .fold(0.0, f64::max);
        }
        if norms[i] == 0.0 {
            return None;
        }
    }
    for i in interior..n {
        norms[i] = matrix[i][..interior]
            .iter()
            .zip(&norms[..interior])
            .fold(0.0_f64, |norm, (c, scale)| norm.hypot(c / scale));
        if norms[i] == 0.0 {
            return None;
        }
    }
    assert!(
        norms.iter().all(|norm| norm.is_finite()),
        "finite independent rows have representable inertial and combination norms"
    );
    for (i, line) in matrix.iter_mut().enumerate() {
        for j in 0..n {
            line[j] = line[j] / norms[i] / norms[j];
        }
        line[n] /= norms[i];
    }
    for col in 0..n {
        let pivot =
            (col..n).max_by(|&a, &b| matrix[a][col].abs().total_cmp(&matrix[b][col].abs()))?;
        if matrix[pivot][col].abs() <= RESOLUTION {
            return None;
        }
        matrix.swap(col, pivot);
        for row in col + 1..n {
            let factor = matrix[row][col] / matrix[col][col];
            let (before, after) = matrix.split_at_mut(row);
            for (v, p) in after[0][col..=n].iter_mut().zip(&before[col][col..=n]) {
                *v -= factor * p;
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

/// A three-speed construction's limit bounds impulse on one of those speeds. Its normal is
/// already a column of the row matrix, so it cannot resolve a dependent impulse basis. Direct
/// minors identify physical independence without squaring small angles into Gram cancellation.
pub(super) fn conditioning_resolvable(
    inertias: &[f64; 3],
    rows: &[[f64; 3]],
    limited_coordinate: usize,
) -> bool {
    for mask in 1usize..(1 << rows.len()) {
        let free: Vec<_> = (0..rows.len()).filter(|i| mask & (1 << i) != 0).collect();
        let nf = free.len();
        let independent = match free.as_slice() {
            [a] => rows[*a].iter().any(|x| *x != 0.0),
            [a, b] => (0..3).any(|i| {
                (i + 1..3).any(|j| rows[*a][i] * rows[*b][j] != rows[*a][j] * rows[*b][i])
            }),
            [a, b, c] => {
                let (a, b, c) = (rows[*a], rows[*b], rows[*c]);
                a[0] * (b[1] * c[2] - b[2] * c[1])
                    + a[1] * (b[2] * c[0] - b[0] * c[2])
                    + a[2] * (b[0] * c[1] - b[1] * c[0])
                    != 0.0
            }
            _ => false, // More rows than speeds are dependent.
        };
        if !independent {
            continue;
        }
        for active in [false, true] {
            if active && free.iter().all(|&i| rows[i][limited_coordinate] == 0.0) {
                continue;
            }
            let size = nf + usize::from(active);
            let mut matrix = vec![vec![0.0; size + 1]; size];
            for (i, &a) in free.iter().enumerate() {
                for (j, &b) in free.iter().enumerate() {
                    matrix[i][j] = rows[a]
                        .iter()
                        .zip(&rows[b])
                        .zip(inertias)
                        .map(|((x, y), m)| x * y / m)
                        .sum();
                }
                if active {
                    matrix[i][nf] = rows[a][limited_coordinate];
                    matrix[nf][i] = rows[a][limited_coordinate];
                }
            }
            if eliminate_bordered(&mut matrix, nf).is_none() {
                return false;
            }
        }
    }
    true
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
