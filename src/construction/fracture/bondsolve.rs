//! Equilibrium for the game's environment destruction model (game state only).
//!
//! [`Solve`] holds rigid-chunk translations and rotations between updates. [`Problem`] supplies
//! the current owned graph and applied wrenches; a round applies its sparse stiffness once.
//! Joint patches transmit both force and moment. Four area samples integrate a circular patch's
//! linear displacement field exactly (area and both second moments), without candidate cuts.
//! The construction design's "Structural calculation" owns the model and its limitations.

use bevy::math::{DMat3, DVec3};

/// Resolution: bounded sparse work per active tick; unfinished equilibrium resumes next tick.
/// The budget bounds cost independently of the number of joints that fail together.
pub(crate) const ROUNDS: usize = 32;
/// Resolution: relative preconditioned residual at which further f64 work is rounding noise
/// for the f32 construction data. It is a convergence gate, never a material-strength margin.
const RELATIVE_RESIDUAL: f64 = 1e-8;

/// A force and moment, or a translation and rotation. SI units, with the origin at a node centre.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Wrench {
    pub linear: DVec3,
    pub angular: DVec3,
}

impl Wrench {
    pub(crate) fn at(force: DVec3, arm: DVec3) -> Self {
        Self {
            linear: force,
            angular: arm.cross(force),
        }
    }

    fn dot(self, other: Self) -> f64 {
        self.linear.dot(other.linear) + self.angular.dot(other.angular)
    }

    fn add(&mut self, other: Self, scale: f64) {
        self.linear += other.linear * scale;
        self.angular += other.angular * scale;
    }

    fn point(self, arm: DVec3) -> DVec3 {
        self.linear + self.angular.cross(arm)
    }
}

/// One rigid chunk. The caller fixes buried matter; a free owner has its rigid acceleration
/// removed from the loads, so its nullspace carries no resultant force or moment.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Node {
    pub centre: DVec3,
    pub fixed: bool,
}

/// An area-integrated linear elastic connection. Compliance is the sum of both sides' lengths
/// over their moduli: `kn = A / (la/Ea + lb/Eb)`, and likewise for shear with `G`.
/// A ground connection has only the occupied side's compliance.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Edge {
    pub a: usize,
    pub b: Option<usize>,
    pub centre: DVec3,
    pub normal: DVec3,
    pub area: f64,
    /// Second moment of each in-plane axis divided by area, m².
    pub second_per_area: f64,
    /// Normal and tangential stiffness integrated over the area, N/m.
    pub normal_stiffness: f64,
    pub shear_stiffness: f64,
}

impl Edge {
    fn samples(self) -> [DVec3; 4] {
        let helper = if self.normal.x.abs() < self.normal.y.abs() {
            DVec3::X
        } else {
            DVec3::Y
        };
        let u = self.normal.cross(helper).normalize();
        let v = self.normal.cross(u);
        // Four equal shares at ±sqrt(2 I/A) integrate <x²> = <y²> = I/A.
        let radius = (2.0 * self.second_per_area).sqrt();
        [u, -u, v, -v].map(|axis| self.centre + axis * radius)
    }

    fn traction(self, displacement: DVec3) -> DVec3 {
        let normal = self.normal * self.normal.dot(displacement);
        (normal * self.normal_stiffness + (displacement - normal) * self.shear_stiffness) / 4.0
    }

    /// Force and moment carried away from side a, about the patch centre. The caller orients
    /// the normal toward a when reading tension; the stiffness itself is sign-independent.
    pub(crate) fn resultant(self, nodes: &[Node], answer: &[Wrench]) -> Wrench {
        let mut carried = Wrench::default();
        for point in self.samples() {
            let mut delta = answer[self.a].point(point - nodes[self.a].centre);
            if let Some(b) = self.b {
                delta -= answer[b].point(point - nodes[b].centre);
            }
            carried.add(Wrench::at(self.traction(delta), point - self.centre), 1.0);
        }
        carried
    }

    /// Peak normal traction (tension positive), and an upper bound on tangential traction, Pa.
    /// DERIVED: Navier `N/A + |M_b| r/I`; Saint-Venant circular torsion `|T| r/(2I)`.
    /// Tangential translation and torsion use the triangle bound, conservative for mixed shear.
    pub(crate) fn stress(self, carried: Wrench, toward_a: DVec3) -> (f64, f64) {
        let axial = carried.linear.dot(toward_a);
        let twist = carried.angular.dot(toward_a);
        let bend = carried.angular - toward_a * twist;
        let rim = (4.0 * self.second_per_area).sqrt();
        let inertia = self.area * self.second_per_area;
        (
            axial / self.area + bend.length() * rim / inertia,
            (carried.linear - toward_a * axial).length() / self.area
                + twist.abs() * rim / (2.0 * inertia),
        )
    }
}

pub(crate) struct Problem<'a> {
    pub nodes: &'a [Node],
    pub edges: &'a [Edge],
    pub loads: &'a [Wrench],
}

impl Problem<'_> {
    fn apply(&self, input: &[Wrench], output: &mut [Wrench]) {
        output.fill(Wrench::default());
        for edge in self.edges {
            for point in edge.samples() {
                let arm_a = point - self.nodes[edge.a].centre;
                let mut delta = input[edge.a].point(arm_a);
                if let Some(b) = edge.b {
                    delta -= input[b].point(point - self.nodes[b].centre);
                }
                let force = edge.traction(delta);
                output[edge.a].add(Wrench::at(force, arm_a), 1.0);
                if let Some(b) = edge.b {
                    output[b].add(Wrench::at(force, point - self.nodes[b].centre), -1.0);
                }
            }
        }
        for (node, result) in self.nodes.iter().zip(output) {
            if node.fixed {
                *result = Wrench::default();
            }
        }
    }

    fn diagonal(&self) -> Vec<Wrench> {
        let mut diagonal = vec![Wrench::default(); self.nodes.len()];
        for edge in self.edges {
            for point in edge.samples() {
                for c in [Some(edge.a), edge.b].into_iter().flatten() {
                    let arm = point - self.nodes[c].centre;
                    for (axis, direction) in [DVec3::X, DVec3::Y, DVec3::Z].into_iter().enumerate()
                    {
                        diagonal[c].linear[axis] += direction.dot(edge.traction(direction));
                        let delta = direction.cross(arm);
                        diagonal[c].angular[axis] += delta.dot(edge.traction(delta));
                    }
                }
            }
        }
        diagonal
    }
}

/// A solve still propagating a changed load is game state, not a reason to spend extra rounds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Progress {
    #[default]
    Changed,
    Iterating,
    Equilibrated,
}

/// Warm displacement field and Krylov history. Unchanged problems keep the search direction
/// across ticks; a changed load or topology restarts conjugacy while retaining displacement.
#[derive(Clone, Debug, Default)]
pub(crate) struct Solve {
    pub answer: Vec<Wrench>,
    pub progress: Progress,
    residual: Vec<Wrench>,
    direction: Vec<Wrench>,
    inverse_diagonal: Vec<Wrench>,
    product: Vec<Wrench>,
    rho: f64,
    initial_rho: f64,
}

fn precondition(input: &[Wrench], inverse: &[Wrench]) -> Vec<Wrench> {
    input
        .iter()
        .zip(inverse)
        .map(|(r, d)| Wrench {
            linear: r.linear * d.linear,
            angular: r.angular * d.angular,
        })
        .collect()
}

fn dot(a: &[Wrench], b: &[Wrench]) -> f64 {
    a.iter().zip(b).map(|(&a, &b)| a.dot(b)).sum()
}

impl Solve {
    pub(crate) fn changed(&mut self) {
        self.progress = Progress::Changed;
    }

    fn start(&mut self, problem: &Problem) {
        self.answer.resize(problem.nodes.len(), Wrench::default());
        for (node, answer) in problem.nodes.iter().zip(&mut self.answer) {
            if node.fixed {
                *answer = Wrench::default();
            }
        }
        self.product.resize(problem.nodes.len(), Wrench::default());
        problem.apply(&self.answer, &mut self.product);
        self.residual = problem
            .loads
            .iter()
            .zip(&self.product)
            .zip(problem.nodes)
            .map(|((&load, &internal), node)| {
                if node.fixed {
                    Wrench::default()
                } else {
                    let mut residual = load;
                    residual.add(internal, -1.0);
                    residual
                }
            })
            .collect();
        self.inverse_diagonal = problem
            .diagonal()
            .into_iter()
            .zip(problem.nodes)
            .map(|(d, node)| {
                if node.fixed {
                    return Wrench::default();
                }
                assert!(
                    d.linear.min_element() > 0.0 && d.angular.min_element() > 0.0,
                    "every free degree of freedom belongs to an occupied joint patch"
                );
                Wrench {
                    linear: DVec3::ONE / d.linear,
                    angular: DVec3::ONE / d.angular,
                }
            })
            .collect();
        self.direction = precondition(&self.residual, &self.inverse_diagonal);
        self.rho = dot(&self.residual, &self.direction);
        self.initial_rho = self.rho;
        self.progress = if self.rho == 0.0 {
            Progress::Equilibrated
        } else {
            Progress::Iterating
        };
    }

    /// At most `rounds` sparse products; zero products for an unchanged equilibrated owner.
    /// Returns actual products, an operation count rather than a machine timing.
    pub(crate) fn advance(&mut self, problem: &Problem, rounds: usize) -> usize {
        if self.progress == Progress::Changed {
            self.start(problem);
        }
        let mut performed = 0;
        for _ in 0..rounds {
            if self.progress == Progress::Equilibrated {
                break;
            }
            problem.apply(&self.direction, &mut self.product);
            let denominator = dot(&self.direction, &self.product);
            assert!(
                denominator.is_finite() && denominator > 0.0,
                "balanced applied wrenches give a positive elastic energy outside rigid motion"
            );
            let alpha = self.rho / denominator;
            for i in 0..self.answer.len() {
                self.answer[i].add(self.direction[i], alpha);
                self.residual[i].add(self.product[i], -alpha);
            }
            performed += 1;
            let preconditioned = precondition(&self.residual, &self.inverse_diagonal);
            let rho = dot(&self.residual, &preconditioned);
            assert!(
                rho.is_finite() && rho >= 0.0,
                "elastic residual stays finite"
            );
            if rho <= self.initial_rho * RELATIVE_RESIDUAL.powi(2) {
                self.progress = Progress::Equilibrated;
                break;
            }
            let beta = rho / self.rho;
            for (direction, next) in self.direction.iter_mut().zip(preconditioned) {
                direction.linear = next.linear + direction.linear * beta;
                direction.angular = next.angular + direction.angular * beta;
            }
            self.rho = rho;
        }
        performed
    }
}

/// Sum a free owner's applied wrenches once, then remove its rigid acceleration once per node.
/// `own_inertia` is the caller's chunk inertia, about its centroid; spin contributes centrifugal
/// loading and Euler's gyroscopic term. Uniform gravity cancels exactly from internal demand.
pub(crate) fn accelerating_frame(
    nodes: &[Node],
    masses: &[f64],
    own_inertia: &[DMat3],
    spin: DVec3,
    loads: &mut [Wrench],
) {
    let mass: f64 = masses.iter().sum();
    let centre: DVec3 = nodes
        .iter()
        .zip(masses)
        .map(|(n, m)| n.centre * m)
        .sum::<DVec3>()
        / mass;
    let mut inertia = DMat3::ZERO;
    let mut total = Wrench::default();
    for (((node, &m), own), &load) in nodes.iter().zip(masses).zip(own_inertia).zip(loads.iter()) {
        let r = node.centre - centre;
        inertia += *own + DMat3::from_diagonal(DVec3::splat(m * r.length_squared()))
            - DMat3::from_cols(r * r.x, r * r.y, r * r.z) * m;
        total.add(
            Wrench {
                linear: load.linear,
                angular: load.angular + r.cross(load.linear),
            },
            1.0,
        );
    }
    let inverse = inertia.inverse();
    assert!(
        inverse.is_finite(),
        "occupied free matter has finite positive inertia"
    );
    let angular_acceleration = inverse * (total.angular - spin.cross(inertia * spin));
    for (((node, &m), own), load) in nodes.iter().zip(masses).zip(own_inertia).zip(loads) {
        let r = node.centre - centre;
        load.linear -=
            m * (total.linear / mass + angular_acceleration.cross(r) + spin.cross(spin.cross(r)));
        load.angular -= *own * angular_acceleration + spin.cross(*own * spin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    // Fixture: SI dimensions and elastic facts belong to this fixture, not a shipped material.
    const YOUNG: f64 = 8e9;
    const SHEAR: f64 = 3e9;
    const AREA: f64 = 0.25;
    const MASS: f64 = 10.0;
    const PUSH: f64 = 100.0;
    const TOLERANCE: f64 = 1e-5;

    fn close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() <= TOLERANCE * expected.abs().max(1.0),
            "{actual} != {expected}"
        );
    }

    fn edge(a: usize, b: Option<usize>, centre: DVec3, normal: DVec3, length: f64) -> Edge {
        Edge {
            a,
            b,
            centre,
            normal,
            area: AREA,
            second_per_area: AREA / (4.0 * PI),
            normal_stiffness: YOUNG * AREA / length,
            shear_stiffness: SHEAR * AREA / length,
        }
    }

    fn solve(nodes: &[Node], edges: &[Edge], loads: &[Wrench]) -> Solve {
        let problem = Problem {
            nodes,
            edges,
            loads,
        };
        let mut solve = Solve::default();
        solve.advance(&problem, ROUNDS);
        assert_eq!(solve.progress, Progress::Equilibrated);
        solve
    }

    /// The game's construction base must carry its own weight and the side push's full moment.
    /// A point spring without rotational area stiffness cannot satisfy this beam-bending check.
    #[test]
    fn a_walls_base_reads_weight_and_side_push_as_n_over_a_plus_m_over_s() {
        let height = 3.0; // Fixture wall centre's height above its base, m.
        let nodes = [Node {
            centre: DVec3::Y * height,
            fixed: false,
        }];
        let edges = [edge(0, None, DVec3::ZERO, DVec3::Y, height)];
        let gravity = f64::from(crate::physical_laws::gravity_mps2());
        let force = DVec3::new(PUSH, -MASS * gravity, 0.0);
        let answer = solve(&nodes, &edges, &[Wrench::at(force, DVec3::ZERO)]);
        let carried = edges[0].resultant(&nodes, &answer.answer);
        close(carried.linear.y, -MASS * gravity);
        close(carried.angular.z, -PUSH * height);
        let radius = (AREA / PI).sqrt();
        let section_modulus = AREA * radius / 4.0;
        close(
            edges[0].stress(carried, DVec3::Y).0,
            -MASS * gravity / AREA + PUSH * height / section_modulus,
        );
    }

    /// The game's lintels need both piers to share a centred weight. An arbitrary chosen path
    /// would put the complete weight on one support and fail this equilibrium witness.
    #[test]
    fn a_lintel_shares_its_centred_load_between_two_piers() {
        let half_span = 2.0; // Fixture: support lever arms, m.
        let nodes = [
            Node {
                centre: DVec3::Y,
                fixed: false,
            },
            Node {
                centre: DVec3::new(-half_span, 0.0, 0.0),
                fixed: true,
            },
            Node {
                centre: DVec3::new(half_span, 0.0, 0.0),
                fixed: true,
            },
        ];
        let edges = [
            edge(0, Some(1), DVec3::new(-half_span, 0.5, 0.0), DVec3::Y, 1.0),
            edge(0, Some(2), DVec3::new(half_span, 0.5, 0.0), DVec3::Y, 1.0),
        ];
        let weight = MASS * f64::from(crate::physical_laws::gravity_mps2());
        let answer = solve(
            &nodes,
            &edges,
            &[
                Wrench::at(-DVec3::Y * weight, DVec3::ZERO),
                Wrench::default(),
                Wrench::default(),
            ],
        );
        for e in edges {
            close(e.resultant(&nodes, &answer.answer).linear.y, -weight / 2.0);
        }
    }

    /// Every cut of a cantilever carries exactly the weight beyond it and that weight's moment.
    /// The game's joints must see this independent of stiffness and of the graph's numbering.
    #[test]
    fn a_cantilevers_joints_carry_the_sum_and_moment_of_the_load_beyond_them() {
        let nodes: Vec<_> = (0..4)
            .map(|i| Node {
                centre: DVec3::X * (i as f64 + 0.5),
                fixed: false,
            })
            .collect();
        let edges: Vec<_> = (0..4)
            .map(|i| edge(i, i.checked_sub(1), DVec3::X * i as f64, DVec3::X, 1.0))
            .collect();
        let weight = MASS * f64::from(crate::physical_laws::gravity_mps2());
        let loads = vec![Wrench::at(-DVec3::Y * weight, DVec3::ZERO); nodes.len()];
        let answer = solve(&nodes, &edges, &loads);
        for (i, e) in edges.iter().enumerate() {
            let beyond = nodes.len() - i;
            let moment: f64 = nodes[i..]
                .iter()
                .map(|n| (n.centre.x - e.centre.x) * weight)
                .sum();
            let carried = e.resultant(&nodes, &answer.answer);
            close(carried.linear.y, -(beyond as f64) * weight);
            close(carried.angular.z, -moment);
        }
    }

    /// An area patch has finite translational and rotational stiffness. Rigid translation and
    /// rotation of both sides create no internal force, a requirement for moving game owners.
    #[test]
    fn rigid_motion_makes_no_joint_force() {
        let nodes = [
            Node {
                centre: -DVec3::X,
                fixed: false,
            },
            Node {
                centre: DVec3::X,
                fixed: false,
            },
        ];
        let e = edge(0, Some(1), DVec3::ZERO, DVec3::X, 2.0);
        let spin = DVec3::new(0.1, -0.3, 0.5); // Fixture rigid rotation, rad.
        let answer = nodes.map(|n| Wrench {
            linear: DVec3::ONE + spin.cross(n.centre),
            angular: spin,
        });
        let force = e.resultant(&nodes, &answer);
        close(force.linear.length(), 0.0);
        close(force.angular.length(), 0.0);
    }

    /// Removing a free owner's acceleration must balance force AND moment without multiplying
    /// the source stream by the chunk count. Spin's centrifugal and gyroscopic terms must balance.
    #[test]
    fn moving_frame_balances_each_resultant_in_one_node_pass() {
        let nodes = [
            Node {
                centre: DVec3::ZERO,
                fixed: false,
            },
            Node {
                centre: DVec3::X,
                fixed: false,
            },
            Node {
                centre: DVec3::new(0.0, 2.0, 0.0),
                fixed: false,
            },
        ];
        let masses = [MASS, 2.0 * MASS, 3.0 * MASS];
        let own = [DMat3::IDENTITY; 3]; // Fixture chunk inertia, kg m².
        let mut loads = [
            Wrench::at(DVec3::new(PUSH, -PUSH, PUSH), DVec3::Y),
            Wrench::default(),
            Wrench::at(DVec3::X * PUSH, DVec3::Z),
        ];
        accelerating_frame(&nodes, &masses, &own, DVec3::new(1.0, 2.0, 3.0), &mut loads);
        let resultant = nodes
            .iter()
            .zip(loads)
            .fold(Wrench::default(), |mut total, (n, load)| {
                total.add(
                    Wrench {
                        linear: load.linear,
                        angular: load.angular + n.centre.cross(load.linear),
                    },
                    1.0,
                );
                total
            });
        close(resultant.linear.length(), 0.0);
        close(resultant.angular.length(), 0.0);
    }

    /// The game's tick budget must resume an unfinished Krylov sequence, reach an independently
    /// computed beam reaction, and do no sparse work once unchanged equilibrium is reached.
    #[test]
    fn cold_and_warm_solves_converge_and_an_unchanged_owner_costs_zero_rounds() {
        let nodes: Vec<_> = (0..8)
            .map(|i| Node {
                centre: DVec3::Y * (i as f64 + 0.5),
                fixed: false,
            })
            .collect();
        let edges: Vec<_> = (0..8)
            .map(|i| edge(i, i.checked_sub(1), DVec3::Y * i as f64, DVec3::Y, 1.0))
            .collect();
        let mut loads = vec![Wrench::at(DVec3::X * PUSH, DVec3::ZERO); nodes.len()];
        let mut answer = Solve::default();
        let problem = Problem {
            nodes: &nodes,
            edges: &edges,
            loads: &loads,
        };
        assert_eq!(answer.advance(&problem, 1), 1);
        assert_eq!(answer.progress, Progress::Iterating);
        answer.advance(&problem, ROUNDS);
        assert_eq!(answer.progress, Progress::Equilibrated);
        close(
            edges[0].resultant(&nodes, &answer.answer).linear.x,
            PUSH * nodes.len() as f64,
        );
        assert_eq!(answer.advance(&problem, ROUNDS), 0);
        loads.last_mut().unwrap().linear.x += PUSH;
        answer.changed();
        let problem = Problem {
            nodes: &nodes,
            edges: &edges,
            loads: &loads,
        };
        answer.advance(&problem, ROUNDS);
        assert_eq!(answer.progress, Progress::Equilibrated);
        close(
            edges[0].resultant(&nodes, &answer.answer).linear.x,
            PUSH * (nodes.len() + 1) as f64,
        );
    }
}
