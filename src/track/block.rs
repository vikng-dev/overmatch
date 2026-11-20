//! Bounded linear rows over independent inertias. `solve_limited` accepts free impulses, returning speeds
//! and row impulses in the caller's units. Rows sharing a speed or an impulse limit solve
//! together; this local dependency requires assignment trials to be serial.
//!
//! With fixed bounds the dual objective is `p·(A u - target) + p·W p/2`, where
//! `u = speed + free_impulse/inertia` and `W = A M^-1 A^T`. Optional limits bound linear
//! combinations of impulses. Convex optimality gives unique speeds even when impulses are
//! nonunique. Static bounds may only shrink to dynamic
//! bounds within a call. The drivetrain design owns the mechanism narrative.

use smallvec::{SmallVec, smallvec};

/// Resolution: eight inline entries keep small KKT blocks and their augmented rows on the
/// stack. Larger constructions spill to the heap; this is a storage choice, never a row limit.
const INLINE_CAPACITY: usize = 8;
pub(crate) type Inline<T> = SmallVec<[T; INLINE_CAPACITY]>;

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
    pub jacobian: Inline<f64>,
    pub target: f64,
    pub bounds: Bounds,
    pub dynamic: Option<Bounds>,
}

impl Row {
    pub(crate) fn starts_sliding(&self, velocity: &[f64]) -> bool {
        dot(&self.jacobian, velocity) != self.target
    }

    pub(crate) fn slip_tolerance(&self, velocity: &[f64]) -> f64 {
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

#[derive(Clone, Debug)]
pub(crate) struct RowResult {
    pub impulse: f64,
    /// The row exhausted static capacity during this call, or began sliding.
    pub dynamic: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Solution {
    pub speeds: Inline<f64>,
    pub rows: Inline<RowResult>,
    /// Deterministic work across every static-to-dynamic stage.
    #[cfg(feature = "bitprobe")]
    pub assignments_tried: usize,
    assignment: Inline<State>,
    /// One shadow price per limit, zero while the limit is interior.
    multipliers: Inline<f64>,
    /// Independent zero-multiplier equalities whose targets the speeds satisfy exactly.
    held: Inline<usize>,
}

/// Affine response while a bounded block stays on one KKT face.
#[derive(Clone, Debug)]
pub(crate) struct FaceResponse {
    pub(crate) mobility: Inline<Inline<f64>>,
    /// Per free-impulse column: interior row impulses, then active limit multipliers.
    sensitivity: Inline<Inline<f64>>,
    interior: Inline<usize>,
    active: Inline<usize>,
    /// A row broke static capacity within this interval. A search restarts it static and may
    /// hold it again, which this sliding face cannot certify.
    broke: bool,
}

struct FaceSystem {
    interior: Inline<usize>,
    active: Inline<usize>,
    matrix: Inline<Inline<f64>>,
}

fn face_system(
    speeds: &[Speed],
    rows: &[Row],
    limits: &[ImpulseLimit],
    states: &[State],
) -> FaceSystem {
    let interior: Inline<_> = (0..rows.len())
        .filter(|&i| states[i] == State::Interior)
        .collect();
    let active: Inline<_> = (0..limits.len())
        .filter(|&i| states[rows.len() + i] != State::Interior)
        .collect();
    let nf = interior.len();
    let size = nf + active.len();
    let mut matrix: Inline<Inline<f64>> = smallvec![smallvec![0.0; size + 1]; size];
    for (i, &ri) in interior.iter().enumerate() {
        for (j, &rj) in interior.iter().enumerate() {
            matrix[i][j] = rows[ri]
                .jacobian
                .iter()
                .zip(&rows[rj].jacobian)
                .zip(speeds)
                .map(|((a, b), s)| a * b / s.inertia)
                .sum();
        }
        for (j, &li) in active.iter().enumerate() {
            matrix[i][nf + j] = limits[li].coefficients[ri];
            matrix[nf + j][i] = limits[li].coefficients[ri];
        }
    }
    FaceSystem {
        interior,
        active,
        matrix,
    }
}

impl Solution {
    pub(crate) fn same_face(&self, other: &Self) -> bool {
        self.assignment == other.assignment
            && self
                .rows
                .iter()
                .zip(&other.rows)
                .all(|(a, b)| a.dynamic == b.dynamic)
    }

    pub(crate) fn response(
        &self,
        speeds: &[Speed],
        rows: &[Row],
        limits: &[ImpulseLimit],
    ) -> FaceResponse {
        let FaceSystem {
            interior,
            active,
            matrix,
        } = face_system(speeds, rows, limits, &self.assignment);
        let nf = interior.len();
        let size = matrix.len();
        let mut mobility = Inline::with_capacity(speeds.len());
        let mut sensitivity = Inline::with_capacity(speeds.len());
        for column in 0..speeds.len() {
            let mut matrix = matrix.clone();
            for (i, &ri) in interior.iter().enumerate() {
                matrix[i][size] = -rows[ri].jacobian[column] / speeds[column].inertia;
            }
            let answer = eliminate_bordered(&mut matrix, nf)
                .expect("the accepted KKT face has an independent response basis");
            mobility.push(
                speeds
                    .iter()
                    .enumerate()
                    .map(|(j, s)| {
                        let free = if j == column { 1.0 } else { 0.0 };
                        let response = (free
                            + interior
                                .iter()
                                .zip(&answer)
                                .map(|(&i, p)| rows[i].jacobian[j] * p)
                                .sum::<f64>())
                            / s.inertia;
                        // Resolution: projection onto a held coordinate retains f64 cancellation ulps.
                        if response.abs()
                            <= RESOLUTION / (s.inertia * speeds[column].inertia).sqrt()
                        {
                            0.0
                        } else {
                            response
                        }
                    })
                    .collect(),
            );
            sensitivity.push(answer);
        }
        let start: Inline<_> = speeds.iter().map(|s| s.value).collect();
        FaceResponse {
            mobility,
            sensitivity,
            interior,
            active,
            broke: rows
                .iter()
                .zip(&self.rows)
                .any(|(row, result)| result.dynamic && !row.starts_sliding(&start)),
        }
    }

    /// This face's solution after `delta` more free impulse on speed `column`, or `None` when the
    /// face cannot certify it and the caller searches. On one face, impulses, multipliers and
    /// speeds are affine in free impulse; a search trial's feasibility, optimality and breakaway
    /// tests accept the step, and held equalities substitute their targets exactly again.
    pub(crate) fn step(
        &self,
        response: &FaceResponse,
        rows: &[Row],
        limits: &[ImpulseLimit],
        column: usize,
        delta: f64,
    ) -> Option<Self> {
        if response.broke {
            return None;
        }
        let rate = &response.sensitivity[column];
        let nf = response.interior.len();
        let mut impulses: Inline<_> = self.rows.iter().map(|r| r.impulse).collect();
        for (&i, d) in response.interior.iter().zip(rate) {
            impulses[i] += d * delta;
        }
        let mut multipliers = self.multipliers.clone();
        for (&j, d) in response.active.iter().zip(&rate[nf..]) {
            multipliers[j] += d * delta;
        }
        let mut speeds: Inline<_> = self
            .speeds
            .iter()
            .zip(&response.mobility[column])
            .map(|(v, m)| v + m * delta)
            .collect();
        let bounds: Inline<_> = rows
            .iter()
            .zip(&self.rows)
            .map(|(row, result)| match row.dynamic {
                Some(sliding) if result.dynamic => sliding,
                _ => row.bounds,
            })
            .collect();
        if !check_face(
            rows,
            &bounds,
            limits,
            &self.assignment,
            &speeds,
            &mut impulses,
            &multipliers,
        ) || physical_equalities(rows, limits, &self.assignment, &multipliers)
            != physical_equalities(rows, limits, &self.assignment, &self.multipliers)
        {
            return None;
        }
        snap(&mut speeds, rows, &self.held);
        if rows.iter().zip(&self.rows).any(|(row, result)| {
            !result.dynamic
                && row.dynamic.is_some()
                && (dot(&row.jacobian, &speeds) - row.target).abs() > row.slip_tolerance(&speeds)
        }) {
            return None; // A static row breaks away; the search applies its sliding bounds.
        }
        Some(Self {
            speeds,
            rows: impulses
                .into_iter()
                .zip(&self.rows)
                .map(|(impulse, result)| RowResult {
                    impulse,
                    dynamic: result.dynamic,
                })
                .collect(),
            #[cfg(feature = "bitprobe")]
            assignments_tried: 0,
            assignment: self.assignment.clone(),
            multipliers,
            held: self.held.clone(),
        })
    }
}

fn physical_equalities(
    rows: &[Row],
    limits: &[ImpulseLimit],
    states: &[State],
    multipliers: &[f64],
) -> Inline<usize> {
    (0..rows.len())
        .filter(|&i| {
            states[i] == State::Interior
                && limits
                    .iter()
                    .zip(multipliers)
                    .map(|(limit, mu)| limit.coefficients[i] * mu)
                    .sum::<f64>()
                    == 0.0
        })
        .collect()
}

fn check_face(
    rows: &[Row],
    bounds: &[Bounds],
    limits: &[ImpulseLimit],
    states: &[State],
    velocity: &[f64],
    impulses: &mut [f64],
    multipliers: &[f64],
) -> bool {
    assert!(
        velocity
            .iter()
            .chain(impulses.iter())
            .chain(multipliers)
            .all(|v| v.is_finite()),
        "a finite KKT face produces finite speeds, impulses and multipliers"
    );
    for (i, row) in rows.iter().enumerate() {
        let bound = bounds[i];
        let allowance = RESOLUTION * bound.lower.abs().max(bound.upper.abs());
        let correction: f64 = limits
            .iter()
            .zip(multipliers)
            .map(|(l, mu)| l.coefficients[i] * mu)
            .sum();
        let gradient = dot(&row.jacobian, velocity) - row.target + correction;
        let tolerance = row.slip_tolerance(velocity) + RESOLUTION * correction.abs();
        if impulses[i] < bound.lower - allowance
            || impulses[i] > bound.upper + allowance
            || match states[i] {
                State::Interior => gradient.abs() > tolerance,
                State::Lower => {
                    (impulses[i] - bound.lower).abs() > allowance
                        || (bound.lower != bound.upper && gradient < -tolerance)
                }
                State::Upper => {
                    (impulses[i] - bound.upper).abs() > allowance
                        || (bound.lower != bound.upper && gradient > tolerance)
                }
            }
        {
            return false;
        }
        impulses[i] = impulses[i].clamp(bound.lower, bound.upper);
    }
    for (j, limit) in limits.iter().enumerate() {
        let value = dot(&limit.coefficients, impulses);
        let tolerance = RESOLUTION
            * (limit.bounds.lower.abs().max(limit.bounds.upper.abs())
                + limit
                    .coefficients
                    .iter()
                    .zip(impulses.iter())
                    .map(|(c, p)| (c * p).abs())
                    .sum::<f64>());
        if value < limit.bounds.lower - tolerance || value > limit.bounds.upper + tolerance {
            return false;
        }
        let active_bound = match states[rows.len() + j] {
            State::Interior => None,
            State::Lower => Some(limit.bounds.lower),
            State::Upper => Some(limit.bounds.upper),
        };
        if let Some(bound) = active_bound {
            if (value - bound).abs() > tolerance {
                return false;
            }
            // Row residual allowances map to multiplier units through this normal's coefficients.
            let allowance = (0..rows.len())
                .filter(|&i| states[i] == State::Interior && limit.coefficients[i] != 0.0)
                .map(|i| rows[i].slip_tolerance(velocity) / limit.coefficients[i].abs())
                .reduce(f64::min)
                .expect("an independent active normal has a free coefficient");
            if limit.bounds.lower != limit.bounds.upper
                && match states[rows.len() + j] {
                    State::Lower => multipliers[j] > allowance,
                    State::Upper => multipliers[j] < -allowance,
                    State::Interior => unreachable!("an active limit has a face"),
                }
            {
                return false;
            }
        }
    }
    true
}

/// The accepted KKT faces remain a valid search seed while row identities stay unchanged.
/// Bounds and targets may move; each trial still checks feasibility and optimality afresh.
#[derive(Clone, Debug, Default)]
pub(crate) struct WarmStart {
    layout: Inline<(Inline<f64>, bool)>,
    limits: Inline<Inline<f64>>,
    assignment: Inline<State>,
}

impl WarmStart {
    fn seed(&mut self, rows: &[Row], limits: &[ImpulseLimit]) -> Inline<State> {
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
            self.assignment = smallvec![State::Interior; rows.len() + limits.len()];
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
    pub coefficients: Inline<f64>,
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
    let start: Inline<_> = speeds.iter().map(|s| s.value).collect();
    let free: Inline<_> = speeds
        .iter()
        .map(|s| s.value + s.free_impulse / s.inertia)
        .collect();
    assert!(
        free.iter().all(|v| v.is_finite()),
        "finite block impulses over admitted inertias produce finite free speeds"
    );
    let mut bounds = Inline::with_capacity(rows.len());
    let mut dynamic: Inline<bool> = smallvec![false; rows.len()];
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
            dynamic[i] = row.starts_sliding(&start);
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
                search.neighbours(&seed, &mut Inline::with_capacity(rows.len()), distance)
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
        states: &mut Inline<State>,
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
        let FaceSystem {
            interior,
            active,
            mut matrix,
        } = face_system(self.speeds, self.rows, self.limits, states);
        let mut impulses: Inline<_> = (0..n)
            .map(|i| match states[i] {
                State::Interior => 0.0,
                State::Lower => self.bounds[i].lower,
                State::Upper => self.bounds[i].upper,
            })
            .collect();
        let velocity_for = |impulses: &[f64]| -> Inline<f64> {
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
        for (i, &ri) in interior.iter().enumerate() {
            matrix[i][size] = self.rows[ri].target - dot(&self.rows[ri].jacobian, &fixed);
        }
        for (j, &li) in active.iter().enumerate() {
            let limit = &self.limits[li];
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
        let mut multipliers: Inline<f64> = smallvec![0.0; self.limits.len()];
        for (&j, &mu) in active.iter().zip(&answer[nf..]) {
            multipliers[j] = mu;
        }
        if !check_face(
            self.rows,
            self.bounds,
            self.limits,
            states,
            &velocity,
            &mut impulses,
            &multipliers,
        ) {
            return None;
        }
        let snapped = physical_equalities(self.rows, self.limits, states, &multipliers);
        // Only zero-multiplier physical equalities can be snapped. Aggregate-constrained
        // interior rows may slide; their KKT equality includes the limit's shadow price.
        let mut independent = Inline::new();
        for &i in &snapped {
            independent.push(i);
            let mut gram: Inline<Inline<f64>> = independent
                .iter()
                .map(|&a| {
                    let mut line: Inline<_> = independent
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
            assignment: states.iter().copied().collect(),
            speeds: velocity,
            rows: impulses
                .into_iter()
                .map(|impulse| RowResult {
                    impulse,
                    dynamic: false,
                })
                .collect(),
            multipliers,
            held: independent,
        })
    }
}

/// Symmetric equilibration preserves row-unit invariance in the indefinite KKT system.
/// Free impulses use their inertial norms; multipliers use their constraint's norm in those
/// coordinates. A zero-inertia objective coordinate must be fixed by an active combination.
fn eliminate_bordered(matrix: &mut [Inline<f64>], interior: usize) -> Option<Inline<f64>> {
    let n = matrix.len();
    let mut norms: Inline<f64> = smallvec![0.0; n];
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
    let mut answer = smallvec![0.0; n];
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
        let free: Inline<_> = (0..rows.len()).filter(|i| mask & (1 << i) != 0).collect();
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
            let mut matrix: Inline<Inline<f64>> = smallvec![smallvec![0.0; size + 1]; size];
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
    let mut equations: Inline<_> = stuck
        .iter()
        .map(|&i| {
            let mut line = rows[i].jacobian.clone();
            line.push(rows[i].target);
            line
        })
        .collect();
    let mut pivots = Inline::with_capacity(stuck.len());
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
