//! Equilibrium for the game's environment destruction model (game state only).
//!
//! [`Solve`] holds rigid-chunk translations and rotations between updates. [`Problem`] supplies
//! the current owned graph and applied wrenches; a round applies its sparse stiffness once.
//! Joint patches transmit force and moment over the authored contact polygon. Area moments
//! integrate affine displacement; vertices bound traction. Root plates declare equivalent disks.
//! The construction design's "Structural calculation" owns the model and its limitations.

use bevy::math::{DMat3, DVec3};

/// Resolution: bounded sparse work per active tick; unfinished equilibrium resumes next tick.
/// The budget bounds cost independently of the number of joints that fail together.
pub(crate) const ROUNDS: usize = 32;
/// Resolution: relative preconditioned residual at which further f64 work is rounding noise
/// for the f32 construction data. It is a convergence gate, never a material-strength margin.
const RELATIVE_RESIDUAL: f64 = 1e-8;
/// Resolution: the frozen elastic solve is tighter than the outer contact gate, so an
/// inner residual cannot consume the whole nonlinear tolerance and stall redistribution.
const LINEAR_RESIDUAL: f64 = RELATIVE_RESIDUAL * 0.01;
/// Resolution: bound shared pre-play preparation to eight rank budgets. Finite precision can
/// lose conjugacy and revisit directions; this permits that work without unbounded asset loading.
const INITIAL_RANK_PASSES: usize = 8;

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
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Node {
    pub centre: DVec3,
    pub fixed: bool,
}

/// Area moments and boundary of one authored contact face, in the construction frame.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Patch {
    pub centre: DVec3,
    /// Integral of the normal-displacement rotation quadratic, divided by area, m².
    bending: DMat3,
    polar: f64,
    rim: Rim,
}

#[derive(Clone, Debug, PartialEq)]
enum Rim {
    Circle(f64),
    Polygon(std::sync::Arc<[DVec3]>),
}

fn outer(v: DVec3) -> DMat3 {
    DMat3::from_cols(v * v.x, v * v.y, v * v.z)
}

impl Patch {
    /// Equivalent circular footprints are explicit root-plate data; analytical fixtures also
    /// use disks. Authored chunk contacts use their actual polygon through `Self::face`.
    fn circle(centre: DVec3, normal: DVec3, area: f64) -> Self {
        let second = area / (4.0 * std::f64::consts::PI);
        Self {
            centre,
            bending: (DMat3::IDENTITY - outer(normal)) * second,
            polar: 2.0 * second,
            rim: Rim::Circle((area / std::f64::consts::PI).sqrt()),
        }
    }

    /// Integrate a convex face by triangle fans. The load gate checks the baked area and
    /// centre against the hull overlap; disagreement beyond boundary rounding requires rebaking.
    pub(crate) fn face(joint: &super::Joint, face: &[bevy::math::Vec3]) -> Result<Self, String> {
        if let Bond::AreaShare {
            footprint_area,
            footprint_centroid,
            ..
        } = joint.bond
        {
            return Ok(Self::circle(
                footprint_centroid.as_dvec3(),
                joint.normal.as_dvec3().normalize(),
                f64::from(footprint_area),
            ));
        }
        if face.len() < 3 {
            return Err(
                "centroid/normal: no shared contact polygon; rebake the interface on both hulls"
                    .into(),
            );
        }
        let normal = joint.normal.as_dvec3().normalize();
        let origin = face[0].as_dvec3();
        let mut area = 0.0;
        let mut first = DVec3::ZERO;
        for pair in face[1..].windows(2) {
            let (p, q) = (pair[0].as_dvec3() - origin, pair[1].as_dvec3() - origin);
            let triangle = normal.dot(p.cross(q)).abs() / 2.0;
            area += triangle;
            first += (p + q) * (triangle / 3.0);
        }
        let perimeter: f64 = face
            .iter()
            .zip(face.iter().cycle().skip(1))
            .map(|(p, q)| f64::from(p.distance(*q)))
            .sum();
        let tolerance = f64::from(super::ON_PLANE);
        if !area.is_finite()
            || area <= 0.0
            || (area - f64::from(joint.area)).abs() > perimeter * tolerance
        {
            return Err(format!(
                "area: baked {} m², hull overlap {area} m²; rebake the interface",
                joint.area
            ));
        }
        let centre = origin + first / area;
        if centre.distance(joint.centroid.as_dvec3()) > tolerance {
            return Err(format!(
                "centroid: baked {:?}, hull overlap {centre}; rebake the interface",
                joint.centroid
            ));
        }
        let corners: Vec<_> = face.iter().map(|p| p.as_dvec3() - centre).collect();
        let mut second = DMat3::ZERO;
        for (p, q) in corners.iter().zip(corners.iter().cycle().skip(1)) {
            let triangle = normal.dot(p.cross(*q)).abs() / 2.0;
            second += (outer(*p) + outer(*q) + outer(*p + *q)) * (triangle / 12.0);
        }
        second /= area;
        let polar = second.x_axis.x + second.y_axis.y + second.z_axis.z;
        let bending = (DMat3::IDENTITY - outer(normal)) * polar - second;
        if !(bending + outer(normal)).determinant().is_finite()
            || (bending + outer(normal)).determinant() <= 0.0
        {
            return Err(
                "area moments: degenerate contact polygon; rebake a nondegenerate interface".into(),
            );
        }
        Ok(Self {
            centre,
            bending,
            polar,
            rim: Rim::Polygon(corners.into()),
        })
    }

    pub(crate) fn scaled(&self, scale: f64) -> Self {
        Self {
            centre: self.centre * scale,
            bending: self.bending * scale.powi(2),
            polar: self.polar * scale.powi(2),
            rim: match &self.rim {
                Rim::Circle(r) => Rim::Circle(r * scale),
                Rim::Polygon(points) => Rim::Polygon(points.iter().map(|p| *p * scale).collect()),
            },
        }
    }
}

/// Authored faces in joint order. Circular test fixtures explicitly declare their geometry;
/// a production graph cannot silently substitute a disk for a missing face.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Geometry {
    Faces(Vec<Patch>),
    #[cfg(test)]
    CircularFixture,
}

impl Default for Geometry {
    fn default() -> Self {
        Self::Faces(Vec::new())
    }
}

impl Geometry {
    fn patch(&self, _joint: &super::Joint, index: usize) -> Patch {
        match self {
            Self::Faces(patches) => patches[index].clone(),
            #[cfg(test)]
            Self::CircularFixture => {
                let (centre, area) = _joint.patch();
                Patch::circle(
                    centre.as_dvec3(),
                    _joint.normal.as_dvec3().normalize(),
                    f64::from(area),
                )
            }
        }
    }
}

/// An area-integrated linear elastic connection. Compliance is the sum of both sides' lengths
/// over their moduli: `kn = A / (la/Ea + lb/Eb)`, and likewise for shear with `G`.
/// A ground connection has only the occupied side's compliance.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Edge {
    pub a: usize,
    pub b: Option<usize>,
    pub centre: DVec3,
    pub normal: DVec3,
    pub area: f64,
    pub patch: Patch,
    /// Normal and tangential stiffness integrated over the area, N/m.
    pub normal_stiffness: f64,
    pub shear_stiffness: f64,
    /// Pair friction for a one-way contact; bonded patches have no contact projection.
    pub contact: Option<f64>,
    /// Occupied faces bear through bulk compression/friction in parallel with fixing springs.
    pub bearing: Option<Bearing>,
    /// A saturated Coulomb site's tangent keeps transverse stiffness, with none along slip.
    pub slip: Option<DVec3>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Bearing {
    normal: f64,
    shear: f64,
    friction: f64,
}

impl Edge {
    fn force(&self, displacement: DVec3) -> DVec3 {
        let normal = self.normal * self.normal.dot(displacement);
        let mut shear = displacement - normal;
        if let Some(direction) = self.slip {
            shear -= direction * direction.dot(shear);
        }
        normal * self.normal_stiffness + shear * self.shear_stiffness
    }

    fn couple(&self, rotation: DVec3) -> DVec3 {
        let twist = self.normal * self.normal.dot(rotation);
        self.patch.bending * rotation * self.normal_stiffness
            + twist * (self.shear_stiffness * self.patch.polar)
    }

    /// Integrate the affine displacement over the authored area and its tensor of second moments.
    pub(crate) fn resultant(&self, nodes: &[Node], answer: &[Wrench]) -> Wrench {
        let mut delta = answer[self.a].point(self.centre - nodes[self.a].centre);
        let mut rotation = answer[self.a].angular;
        if let Some(b) = self.b {
            delta -= answer[b].point(self.centre - nodes[b].centre);
            rotation -= answer[b].angular;
        }
        Wrench {
            linear: self.force(delta),
            angular: self.couple(rotation),
        }
    }

    /// Recoverable opening and sliding work, J, from the same area integrals.
    fn energy(&self, nodes: &[Node], answer: &[Wrench]) -> (f64, f64) {
        let mut delta = answer[self.a].point(self.centre - nodes[self.a].centre);
        let mut rotation = answer[self.a].angular;
        if let Some(b) = self.b {
            delta -= answer[b].point(self.centre - nodes[b].centre);
            rotation -= answer[b].angular;
        }
        let normal = self.normal.dot(delta);
        let twist = self.normal.dot(rotation);
        (
            self.normal_stiffness * (normal * normal + rotation.dot(self.patch.bending * rotation))
                / 2.0,
            self.shear_stiffness
                * ((delta - self.normal * normal).length_squared()
                    + self.patch.polar * twist * twist)
                / 2.0,
        )
    }

    /// Peak normal and tangential traction, Pa, of the affine spring field. A convex polygon's
    /// extrema lie at vertices. A declared circular footprint has the analytical rim extrema.
    pub(crate) fn stress(&self, carried: Wrench, toward_a: DVec3) -> (f64, f64) {
        let axial = carried.linear.dot(toward_a);
        let twist = carried.angular.dot(toward_a);
        let bend = carried.angular - toward_a * twist;
        let force = (carried.linear - toward_a * axial) / self.area;
        let gradient =
            toward_a.cross((self.patch.bending + outer(toward_a)).inverse() * bend) / self.area;
        match &self.patch.rim {
            Rim::Circle(radius) => (
                axial / self.area + gradient.length() * radius,
                force.length() + twist.abs() * radius / (self.area * self.patch.polar),
            ),
            Rim::Polygon(corners) => {
                corners
                    .iter()
                    .fold((f64::NEG_INFINITY, 0.0_f64), |(tension, shear), &r| {
                        (
                            tension.max(axial / self.area + gradient.dot(r)),
                            shear.max(
                                (force
                                    + toward_a.cross(r) * (twist / (self.area * self.patch.polar)))
                                    .length(),
                            ),
                        )
                    })
            }
        }
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
            let carried = edge.resultant(self.nodes, input);
            output[edge.a].add(
                Wrench {
                    linear: carried.linear,
                    angular: carried.angular
                        + (edge.centre - self.nodes[edge.a].centre).cross(carried.linear),
                },
                1.0,
            );
            if let Some(b) = edge.b {
                output[b].add(
                    Wrench {
                        linear: carried.linear,
                        angular: carried.angular
                            + (edge.centre - self.nodes[b].centre).cross(carried.linear),
                    },
                    -1.0,
                );
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
        for base in self.edges {
            let bearing = base.contact_edge().filter(|_| base.contact.is_none());
            for edge in std::iter::once(base).chain(bearing.as_ref()) {
                for c in [Some(edge.a), edge.b].into_iter().flatten() {
                    let arm = edge.centre - self.nodes[c].centre;
                    for (axis, direction) in [DVec3::X, DVec3::Y, DVec3::Z].into_iter().enumerate()
                    {
                        diagonal[c].linear[axis] += direction.dot(edge.force(direction));
                        let delta = direction.cross(arm);
                        diagonal[c].angular[axis] +=
                            delta.dot(edge.force(delta)) + direction.dot(edge.couple(direction));
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
    /// Applied work remains in a contact mechanism with no elastic resistance.
    Mechanism,
}

/// Warm displacement field and Krylov history. Unchanged problems keep the search direction
/// across ticks; a changed load or topology restarts conjugacy while retaining displacement.
#[derive(Clone, Debug, Default, PartialEq)]
struct Krylov {
    pub answer: Vec<Wrench>,
    pub progress: Progress,
    pub joint_visits: usize,
    residual: Vec<Wrench>,
    direction: Vec<Wrench>,
    inverse_diagonal: Vec<Wrench>,
    product: Vec<Wrench>,
    preconditioned: Vec<Wrench>,
    rho: f64,
    initial_rho: f64,
}

fn precondition(input: &[Wrench], inverse: &[Wrench], output: &mut [Wrench]) {
    for ((r, d), out) in input.iter().zip(inverse).zip(output) {
        *out = Wrench {
            linear: r.linear * d.linear,
            angular: r.angular * d.angular,
        };
    }
}

fn dot(a: &[Wrench], b: &[Wrench]) -> f64 {
    a.iter().zip(b).map(|(&a, &b)| a.dot(b)).sum()
}

impl Krylov {
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
        self.joint_visits += 2 * problem.edges.len();
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
                let inverse = |v: f64| if v > 0.0 { v.recip() } else { 1.0 };
                // A contact mechanism can have a free coordinate. Its residual must reach the
                // Krylov breakdown gate rather than disappearing in a zero preconditioner.
                Wrench {
                    linear: DVec3::new(
                        inverse(d.linear.x),
                        inverse(d.linear.y),
                        inverse(d.linear.z),
                    ),
                    angular: DVec3::new(
                        inverse(d.angular.x),
                        inverse(d.angular.y),
                        inverse(d.angular.z),
                    ),
                }
            })
            .collect();
        self.direction
            .resize(problem.nodes.len(), Wrench::default());
        self.preconditioned
            .resize(problem.nodes.len(), Wrench::default());
        precondition(&self.residual, &self.inverse_diagonal, &mut self.direction);
        self.rho = dot(&self.residual, &self.direction);
        let mut load = problem.loads.to_vec();
        for (node, load) in problem.nodes.iter().zip(&mut load) {
            if node.fixed {
                *load = Wrench::default();
            }
        }
        let mut scaled_load = load.clone();
        precondition(&load, &self.inverse_diagonal, &mut scaled_load);
        self.initial_rho = self.rho.max(dot(&load, &scaled_load));
        self.progress = if self.rho <= self.initial_rho * LINEAR_RESIDUAL.powi(2) {
            Progress::Equilibrated
        } else {
            Progress::Iterating
        };
    }

    /// At most `rounds` sparse products; zero products for an unchanged equilibrated owner.
    /// Returns actual products, an operation count rather than a machine timing.
    pub(crate) fn advance(&mut self, problem: &Problem, rounds: usize) -> usize {
        self.joint_visits = 0;
        if self.progress == Progress::Changed {
            self.start(problem);
        }
        let mut performed = 0;
        for _ in 0..rounds {
            if matches!(self.progress, Progress::Equilibrated | Progress::Mechanism) {
                break;
            }
            problem.apply(&self.direction, &mut self.product);
            self.joint_visits += problem.edges.len();
            let denominator = dot(&self.direction, &self.product);
            assert!(
                denominator.is_finite(),
                "elastic search energy stays finite"
            );
            // Resolution: f64 loses a null direction below this scaled roundoff bound.
            let scale = self
                .inverse_diagonal
                .iter()
                .flat_map(|d| [d.linear.max_element(), d.angular.max_element()])
                .fold(0.0_f64, f64::max);
            if denominator <= 64.0 * f64::EPSILON * dot(&self.direction, &self.direction) / scale {
                self.progress = Progress::Mechanism;
                break;
            }
            let alpha = self.rho / denominator;
            for i in 0..self.answer.len() {
                self.answer[i].add(self.direction[i], alpha);
                self.residual[i].add(self.product[i], -alpha);
            }
            performed += 1;
            precondition(
                &self.residual,
                &self.inverse_diagonal,
                &mut self.preconditioned,
            );
            let rho = dot(&self.residual, &self.preconditioned);
            assert!(
                rho.is_finite() && rho >= 0.0,
                "elastic residual stays finite"
            );
            if rho <= self.initial_rho * LINEAR_RESIDUAL.powi(2) {
                self.progress = Progress::Equilibrated;
                break;
            }
            let beta = rho / self.rho;
            for (direction, next) in self.direction.iter_mut().zip(&self.preconditioned) {
                direction.linear = next.linear + direction.linear * beta;
                direction.angular = next.angular + direction.angular * beta;
            }
            self.rho = rho;
        }
        performed
    }
}

/// Contact quadrature integrates every affine full-face spring exactly. Positive weights at
/// perimeter vertices also retain the authored support polygon when pressure localizes there.
impl Edge {
    fn contact_edge(&self) -> Option<Self> {
        if self.contact.is_some() {
            return Some(self.clone());
        }
        self.bearing.map(|b| Self {
            normal_stiffness: b.normal,
            shear_stiffness: b.shear,
            contact: Some(b.friction),
            bearing: None,
            slip: None,
            ..self.clone()
        })
    }

    fn samples(&self) -> Vec<(DVec3, f64)> {
        match &self.patch.rim {
            Rim::Polygon(points) => {
                let mut samples = Vec::with_capacity(4 * points.len());
                for (p, q) in points.iter().zip(points.iter().cycle().skip(1)) {
                    let fraction = self.normal.dot(p.cross(*q)).abs() / (2.0 * self.area);
                    for (r, weight) in [
                        (DVec3::ZERO, 1.0 / 12.0),
                        (*p, 1.0 / 12.0),
                        (*q, 1.0 / 12.0),
                        ((*p + *q) / 3.0, 3.0 / 4.0),
                    ] {
                        samples.push((self.centre + r, fraction * weight));
                    }
                }
                let sum: f64 = samples.iter().map(|(_, w)| *w).sum();
                for (_, weight) in &mut samples {
                    *weight /= sum;
                }
                samples
            }
            Rim::Circle(radius) => {
                // Resolution: 32 rim sites bound the angular footprint discretization cost.
                const SITES: usize = 32;
                let axis = if self.normal.x.abs() < 0.5 {
                    DVec3::X
                } else {
                    DVec3::Y
                };
                let u = self.normal.cross(axis).normalize();
                let v = self.normal.cross(u);
                let mut samples = vec![(self.centre, 0.5)];
                samples.extend((0..SITES).map(|i| {
                    let angle = std::f64::consts::TAU * i as f64 / SITES as f64;
                    (
                        self.centre + radius * (u * angle.cos() + v * angle.sin()),
                        0.5 / SITES as f64,
                    )
                }));
                samples
            }
        }
    }

    fn relative(&self, nodes: &[Node], answer: &[Wrench], point: DVec3) -> DVec3 {
        let mut delta = answer[self.a].point(point - nodes[self.a].centre);
        if let Some(b) = self.b {
            delta -= answer[b].point(point - nodes[b].centre);
        }
        delta
    }

    fn contact_force(
        &self,
        nodes: &[Node],
        answer: &[Wrench],
        point: DVec3,
        weight: f64,
    ) -> (DVec3, bool, bool) {
        let mu = self.contact.expect("only contacts project traction");
        let delta = self.relative(nodes, answer, point);
        let opening = self.normal.dot(delta);
        let normal = self.normal_stiffness * opening.min(0.0) * weight;
        let trial = (delta - self.normal * opening) * (self.shear_stiffness * weight);
        let limit = -mu * normal;
        let sliding = trial.length() > limit;
        let shear = if sliding {
            trial * (limit / trial.length())
        } else {
            trial
        };
        (self.normal * normal + shear, opening <= 0.0, sliding)
    }
}

/// Active-set contact and the warm elastic Krylov sequence share one bounded round budget.
/// A closed cracked seam has the same contact law as an authored rest. The active set freezes
/// compression and slip directions during a linear solve, then tests the actual projected
/// traction before declaring equilibrium. A saturated site has no tangent stiffness along
/// slip; transverse stiffness and the affine force term retain changes in slip direction.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Solve {
    pub answer: Vec<Wrench>,
    pub progress: Progress,
    pub joint_visits: usize,
    linear: Krylov,
    tangent: Vec<Edge>,
    adjusted: Vec<Wrench>,
    anchor: Vec<Wrench>,
    metric: Vec<Wrench>,
    initialized: bool,
}

impl Solve {
    pub(crate) fn changed(&mut self) {
        self.progress = Progress::Changed;
        self.initialized = false;
    }

    fn linearize(&mut self, problem: &Problem) {
        self.anchor.clone_from(&self.answer);
        self.tangent.clear();
        self.adjusted = problem.loads.to_vec();
        for edge in problem.edges {
            if edge.contact.is_none() {
                self.tangent.push(Edge {
                    bearing: None,
                    slip: None,
                    ..edge.clone()
                });
            }
            let Some(edge) = edge.contact_edge() else {
                continue;
            }; // An intrinsic cohesive seam has no separate contact spring.
            for (point, weight) in edge.samples() {
                let (force, closed, sliding) =
                    edge.contact_force(problem.nodes, &self.answer, point, weight);
                if !closed {
                    continue;
                } // An open contact site transmits no force.
                let delta = edge.relative(problem.nodes, &self.answer, point);
                let tangent_delta = delta - edge.normal * edge.normal.dot(delta);
                let shear = force - edge.normal * edge.normal.dot(force);
                let spring = Edge {
                    centre: point,
                    area: edge.area * weight,
                    patch: Patch::circle(point, edge.normal, 0.0),
                    normal_stiffness: edge.normal_stiffness * weight,
                    shear_stiffness: if sliding {
                        shear.length() / tangent_delta.length()
                    } else {
                        edge.shear_stiffness * weight
                    },
                    contact: None,
                    bearing: None,
                    slip: sliding.then(|| tangent_delta.normalize()),
                    ..edge.clone()
                };
                // The affine term makes the frozen tangent equal the current projected
                // force, including when the slip direction turns between active sets.
                let bias = force - spring.force(delta);
                self.adjusted[edge.a]
                    .add(Wrench::at(bias, point - problem.nodes[edge.a].centre), -1.0);
                if let Some(b) = edge.b {
                    self.adjusted[b].add(Wrench::at(bias, point - problem.nodes[b].centre), 1.0);
                }
                self.tangent.push(spring);
            }
        }
        self.linear.answer.clone_from(&self.answer);
        self.linear.changed();
        self.initialized = true;
    }

    fn projected_residual(&self, problem: &Problem) -> Vec<Wrench> {
        let mut residual = problem.loads.to_vec();
        for edge in problem.edges {
            if edge.contact.is_none() {
                let force = edge.resultant(problem.nodes, &self.answer);
                residual[edge.a].add(
                    Wrench {
                        linear: force.linear,
                        angular: force.angular
                            + (edge.centre - problem.nodes[edge.a].centre).cross(force.linear),
                    },
                    -1.0,
                );
                if let Some(b) = edge.b {
                    residual[b].add(
                        Wrench {
                            linear: force.linear,
                            angular: force.angular
                                + (edge.centre - problem.nodes[b].centre).cross(force.linear),
                        },
                        1.0,
                    );
                }
            }
            if let Some(edge) = edge.contact_edge() {
                for (point, weight) in edge.samples() {
                    let (force, _, _) =
                        edge.contact_force(problem.nodes, &self.answer, point, weight);
                    residual[edge.a].add(
                        Wrench::at(force, point - problem.nodes[edge.a].centre),
                        -1.0,
                    );
                    if let Some(b) = edge.b {
                        residual[b].add(Wrench::at(force, point - problem.nodes[b].centre), 1.0);
                    }
                }
            }
        }
        for (node, residual) in problem.nodes.iter().zip(&mut residual) {
            if node.fixed {
                *residual = Wrench::default();
            }
        }
        residual
    }

    pub(crate) fn advance(&mut self, problem: &Problem, rounds: usize) -> usize {
        self.joint_visits = 0;
        if matches!(self.progress, Progress::Equilibrated | Progress::Mechanism) {
            return 0;
        }
        self.answer.resize(problem.nodes.len(), Wrench::default());
        if !self.initialized {
            // The convergence norm keeps the original elastic scale. An inactive contact's
            // zero tangent cannot turn roundoff force into an order-one weighted residual.
            self.metric = problem
                .diagonal()
                .into_iter()
                .zip(problem.nodes)
                .map(|(d, node)| {
                    if node.fixed {
                        return Wrench::default();
                    }
                    let inverse = |v: f64| if v > 0.0 { v.recip() } else { 1.0 };
                    Wrench {
                        linear: DVec3::new(
                            inverse(d.linear.x),
                            inverse(d.linear.y),
                            inverse(d.linear.z),
                        ),
                        angular: DVec3::new(
                            inverse(d.angular.x),
                            inverse(d.angular.y),
                            inverse(d.angular.z),
                        ),
                    }
                })
                .collect();
            self.linearize(problem);
        }
        let mut performed = 0;
        for _ in 0..rounds {
            let n = self.linear.advance(
                &Problem {
                    nodes: problem.nodes,
                    edges: &self.tangent,
                    loads: &self.adjusted,
                },
                1,
            );
            performed += n;
            self.joint_visits += self.linear.joint_visits;
            self.answer.clone_from(&self.linear.answer);
            if self.linear.progress == Progress::Mechanism {
                assert!(
                    problem
                        .edges
                        .iter()
                        .any(|e| e.contact.is_some() || e.bearing.is_some()),
                    "a bonded component has balanced loads outside its rigid nullspace"
                );
                // An inactive face can intercept the proposed motion. Activate that face
                // before calling the direction an escape mechanism.
                let mut entry = f64::INFINITY;
                for edge in problem.edges.iter().filter_map(Edge::contact_edge) {
                    for (point, _) in edge.samples() {
                        let gap = edge
                            .relative(problem.nodes, &self.answer, point)
                            .dot(edge.normal);
                        let speed = edge
                            .relative(problem.nodes, &self.linear.direction, point)
                            .dot(edge.normal);
                        if gap > 0.0 && speed < 0.0 {
                            entry = entry.min(gap / -speed);
                        }
                    }
                }
                if entry.is_finite() {
                    // Resolution: cross the active boundary by f64 roundoff, never by a
                    // geometric gap or material-strength allowance.
                    for (answer, direction) in self.answer.iter_mut().zip(&self.linear.direction) {
                        answer.add(*direction, entry * (1.0 + 64.0 * f64::EPSILON));
                    }
                    self.linearize(problem);
                    self.progress = Progress::Iterating;
                    continue;
                }
                self.progress = Progress::Mechanism;
                break;
            }
            if self.linear.progress == Progress::Equilibrated {
                let residual = self.projected_residual(problem);
                self.joint_visits += problem.edges.len();
                let mut scaled = residual.clone();
                precondition(&residual, &self.metric, &mut scaled);
                let mut load = problem.loads.to_vec();
                for (node, load) in problem.nodes.iter().zip(&mut load) {
                    if node.fixed {
                        *load = Wrench::default();
                    }
                }
                let mut scaled_load = load.clone();
                precondition(&load, &self.metric, &mut scaled_load);
                if dot(&residual, &scaled) <= dot(&load, &scaled_load) * RELATIVE_RESIDUAL.powi(2) {
                    self.progress = Progress::Equilibrated;
                    break;
                }
                // Resolution: eight halvings bound globalization work for changing contact
                // sets. Accept the least projected residual, never a historical displacement.
                const BACKTRACKS: usize = 8;
                let candidate = self.answer.clone();
                let mut best = dot(&residual, &scaled);
                let mut accepted = candidate.clone();
                for half in 1..=BACKTRACKS {
                    let fraction = 0.5_f64.powi(half as i32);
                    for ((out, &a), &b) in self.answer.iter_mut().zip(&self.anchor).zip(&candidate)
                    {
                        *out = a;
                        out.add(b, fraction);
                        out.add(a, -fraction);
                    }
                    let r = self.projected_residual(problem);
                    let mut z = r.clone();
                    precondition(&r, &self.metric, &mut z);
                    let merit = dot(&r, &z);
                    if merit < best {
                        best = merit;
                        accepted.clone_from(&self.answer);
                    }
                }
                self.answer = accepted;
                self.linearize(problem);
                self.joint_visits += (BACKTRACKS + 1) * problem.edges.len();
            }
            self.progress = Progress::Iterating;
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
            patch: Patch::circle(centre, normal, AREA),
            contact: None,
            bearing: None,
            slip: None,
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

    fn contact_solve(nodes: &[Node], edges: &[Edge], loads: &[Wrench]) -> Solve {
        let problem = Problem {
            nodes,
            edges,
            loads,
        };
        let mut answer = Solve::default();
        for _ in 0..256 {
            answer.advance(&problem, ROUNDS);
            if matches!(
                answer.progress,
                Progress::Equilibrated | Progress::Mechanism
            ) {
                break;
            }
        }
        answer
    }

    /// A game's board must use compression reactions on both supports, including a partly
    /// unloaded support. The sum and moment are independently fixed by statics.
    #[test]
    fn a_resting_board_redistributes_pressure_between_two_supports() {
        // Fixture: supports at x=+-2 m; weight at x=1 m gives reactions W/4 and 3W/4.
        let nodes = [Node {
            centre: DVec3::new(1.0, 1.0, 0.0),
            fixed: false,
        }];
        let mut edges = [-2.0, 2.0].map(|x| edge(0, None, DVec3::X * x, DVec3::Y, 1.0));
        for e in &mut edges {
            e.contact = Some(0.5);
            // Fixture point bearings: no authored couple, so statics fixes each reaction.
            e.patch = Patch::circle(e.centre, e.normal, 0.0);
        }
        let load = [Wrench::at(-DVec3::Y * PUSH, DVec3::ZERO)];
        let answer = contact_solve(&nodes, &edges, &load);
        assert_eq!(answer.progress, Progress::Equilibrated, "{answer:?}");
        let residual = answer.projected_residual(&Problem {
            nodes: &nodes,
            edges: &edges,
            loads: &load,
        });
        close(residual[0].linear.length(), 0.0);
        close(residual[0].angular.length(), 0.0);
        for (e, share) in edges.iter().zip([0.25, 0.75]) {
            let mut reaction = DVec3::ZERO;
            for (point, weight) in e.samples() {
                let (force, _, _) = e.contact_force(&nodes, &answer.answer, point, weight);
                assert!(force.y <= 0.0, "a support cannot pull its board");
                reaction += force;
            }
            close(reaction.y, -PUSH * share);
        }
    }

    /// The game's static support ends at the actual footprint; no tensile contact may hold a
    /// plank whose centre of mass lies beyond it. Fixture COM is twice the support radius.
    #[test]
    fn an_overhanging_plank_has_a_tipping_mechanism() {
        let radius = (AREA / PI).sqrt();
        let nodes = [Node {
            centre: DVec3::new(2.0 * radius, 1.0, 0.0),
            fixed: false,
        }];
        let mut edge = edge(0, None, DVec3::ZERO, DVec3::Y, 1.0);
        edge.contact = Some(0.5);
        let answer = contact_solve(
            &nodes,
            &[edge],
            &[Wrench::at(-DVec3::Y * PUSH, DVec3::ZERO)],
        );
        assert_eq!(answer.progress, Progress::Mechanism, "{answer:?}");
    }

    /// Coulomb's friction angle alone separates a holding block from a sliding one in the
    /// game's environment law. Fixture slopes straddle tan(theta)=mu with load through contact.
    #[test]
    fn slope_contacts_hold_below_and_slide_above_their_friction_angle() {
        let mu = 0.5; // Fixture pair friction.
        for ratio in [0.9, 1.1] {
            let normal = DVec3::new(mu * ratio, 1.0, 0.0).normalize();
            let nodes = [Node {
                centre: DVec3::ZERO,
                fixed: false,
            }];
            let mut edge = edge(0, None, DVec3::ZERO, normal, 1.0);
            edge.contact = Some(mu);
            let answer = contact_solve(
                &nodes,
                &[edge],
                &[Wrench::at(-DVec3::Y * PUSH, DVec3::ZERO)],
            );
            assert_eq!(
                answer.progress,
                if ratio < 1.0 {
                    Progress::Equilibrated
                } else {
                    Progress::Mechanism
                },
                "ratio {ratio}: {answer:?}"
            );
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

use super::{Bond, Cause, Failure, Load, Side, State, Structure};
use std::collections::HashMap;

/// One owner's persistent graph and equilibrium state. Source addresses stay in the ledger;
/// all numerical arrays contain only this owner's standing matter.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Assessment {
    solve: Solve,
    members: Vec<usize>,
    local: HashMap<usize, usize>,
    nodes: Vec<Node>,
    solve_nodes: Vec<Node>,
    solve_edges: Vec<Edge>,
    edges: Vec<Edge>,
    joints: Vec<usize>,
    free_groups: Vec<Vec<usize>>,
    masses: Vec<f64>,
    own_inertia: Vec<DMat3>,
    loads: Vec<Wrench>,
    topology_changed: bool,
    initialized: bool,
    last_spin: bevy::math::Vec3,
    impulsive: bool,
    /// Compiled gravity overloads await the first authoritative failure stage.
    initial_verdict: Vec<Failure>,
    pub(crate) support: Vec<Vec<usize>>,
}

impl Assessment {
    #[cfg(test)]
    pub(crate) fn applied_force(&self) -> DVec3 {
        self.loads[self.nodes.len()..]
            .iter()
            .map(|l| l.linear)
            .sum()
    }

    pub(crate) fn needs_tick(&self) -> bool {
        !self.initialized
            || !self.initial_verdict.is_empty()
            || self.topology_changed
            || self.impulsive
            || self.solve.progress != Progress::Equilibrated
    }

    pub(crate) fn spin_changed(&self, spin: bevy::math::Vec3) -> bool {
        self.last_spin != spin
    }

    /// Compile initial gravity from construction data once per admitted scale. Instances share
    /// this immutable warm state until their own first change; no view asset or async load enters
    /// sim construction. Existing overloads are committed by the authority, never during loading.
    pub(crate) fn at_rest(structure: &Structure) -> Result<Self, String> {
        Self::prepare_gravity(structure, None)
    }

    /// Reuse only an intact displacement guess, never cracks, contact separation or verdicts.
    /// DERIVED bulk scaling: force scales as s³, translation as s² and rotation as s. Fixing
    /// compliance can break that homogeneity; every target scale still solves and passes its
    /// own equilibrium gate before admission.
    pub(crate) fn at_rest_scaled(
        structure: &Structure,
        unit: &Self,
        scale: f64,
    ) -> Result<Self, String> {
        Self::prepare_gravity(structure, Some((unit, scale)))
    }

    fn prepare_gravity(structure: &Structure, guess: Option<(&Self, f64)>) -> Result<Self, String> {
        let state = State::new(structure);
        let mut assessment = Self::default();
        assessment.rebuild(structure, &state);
        if let Some((unit, scale)) = guess {
            assert_eq!(
                assessment.members, unit.members,
                "scales of an admitted construction retain source addresses"
            );
            assessment.solve.answer = unit
                .solve
                .answer
                .iter()
                .map(|w| Wrench {
                    linear: w.linear * scale.powi(2),
                    angular: w.angular * scale,
                })
                .collect();
        }
        assessment.loads = assessment.gravity();
        assessment.loads.extend(assessment.loads.clone());
        // Six rigid degrees per chunk set the exact rank; finite-precision CG can revisit them.
        let budget = INITIAL_RANK_PASSES * 6 * assessment.nodes.len();
        for _ in 0..budget.div_ceil(ROUNDS).max(1) {
            assessment.solve.advance(
                &Problem {
                    nodes: &assessment.solve_nodes,
                    edges: &assessment.solve_edges,
                    loads: &assessment.loads,
                },
                ROUNDS,
            );
            if matches!(
                assessment.solve.progress,
                Progress::Equilibrated | Progress::Mechanism
            ) {
                break;
            }
        }
        if !matches!(
            assessment.solve.progress,
            Progress::Equilibrated | Progress::Mechanism
        ) {
            let problem = Problem {
                nodes: &assessment.solve_nodes,
                edges: &assessment.solve_edges,
                loads: &assessment.loads,
            };
            let residual = assessment.solve.projected_residual(&problem);
            let norm = dot(&residual, &residual).sqrt();
            let mut scaled = residual.clone();
            precondition(&residual, &assessment.solve.metric, &mut scaled);
            let mut scaled_load = assessment.loads.clone();
            precondition(
                &assessment.loads,
                &assessment.solve.metric,
                &mut scaled_load,
            );
            let relative = (dot(&residual, &scaled) / dot(&assessment.loads, &scaled_load)).sqrt();
            return Err(format!(
                "joints: initial gravity equilibrium did not converge within {budget} rounds (projected residual {norm}, relative {relative}, state {:?}); review joint areas and material elastic compliance",
                assessment.solve.progress
            ));
        }
        assessment.initial_verdict = assessment.failures(structure);
        assessment.support = super::loose_groups(structure, &state);
        // A disconnected initial component must be released even when its internal stress is zero.
        if !assessment.support.is_empty() {
            assessment.topology_changed = true;
        }
        Ok(assessment)
    }

    fn gravity(&self) -> Vec<Wrench> {
        let mut gravity: Vec<_> = self
            .masses
            .iter()
            .map(|&mass| {
                Wrench::at(
                    -DVec3::Y * (f64::from(crate::physical_laws::gravity_mps2()) * mass),
                    DVec3::ZERO,
                )
            })
            .collect();
        for group in &self.free_groups {
            for &c in group {
                gravity[c] = Wrench::default();
            }
        }
        gravity
    }

    fn rebuild(&mut self, structure: &Structure, state: &State) {
        let previous: HashMap<_, _> = self
            .members
            .iter()
            .enumerate()
            .filter_map(|(i, &c)| {
                self.solve
                    .answer
                    .get(i)
                    .zip(self.solve.answer.get(self.members.len() + i))
                    .map(|(&steady, &pulse)| (c, (steady, pulse)))
            })
            .collect();
        self.members = state
            .members
            .iter()
            .copied()
            .filter(|&c| state.standing(c))
            .collect();
        self.local = self
            .members
            .iter()
            .enumerate()
            .map(|(i, &c)| (c, i))
            .collect();
        self.nodes = self
            .members
            .iter()
            .map(|&c| Node {
                centre: structure.chunks[c].centroid.as_dvec3(),
                fixed: structure.chunks[c].buried,
            })
            .collect();
        self.masses = self
            .members
            .iter()
            .map(|&c| f64::from(structure.chunks[c].mass))
            .collect();
        self.own_inertia = self
            .members
            .iter()
            .map(|&c| DMat3::from_diagonal(DVec3::splat(structure.chunks[c].intrinsic_inertia())))
            .collect();
        self.edges.clear();
        self.joints.clear();
        for &j in &state.joints {
            let joint = &structure.joints[j];
            if state.separated[j] || !state.live(joint) {
                continue;
            } // Open or departed matter has no bond.
            let a = self.local[&joint.a];
            let b = match joint.b {
                Side::Chunk(b) => Some(self.local[&b]),
                Side::Ground => None,
            };
            let compliance = |c: usize| {
                let chunk = &structure.chunks[c];
                // DERIVED representative half-length of the equal-volume cube. Compliance
                // apportions the bulk, rather than treating a baked cut as a material flaw.
                let length = f64::from(chunk.volume).cbrt() / 2.0;
                (
                    length / chunk.material.young,
                    length / chunk.material.shear_modulus,
                )
            };
            let (mut normal, mut shear) = compliance(joint.a);
            if let Side::Chunk(b) = joint.b {
                let other = compliance(b);
                normal += other.0;
                shear += other.1;
            }
            let patch = structure.geometry.patch(joint, j);
            let area = f64::from(joint.area);
            // The f32 source direction is normalized again in the arithmetic precision of
            // the contact law; projection requires an orthonormal normal/shear split.
            let direction = joint.normal.as_dvec3().normalize();
            let toward_a = if (self.nodes[a].centre - patch.centre).dot(direction) >= 0.0 {
                direction
            } else {
                -direction
            };
            let other_friction = match joint.b {
                Side::Chunk(c) => structure.chunks[c].material.friction,
                Side::Ground => structure.chunks[joint.a].material.friction,
            };
            let friction = f64::from(crate::substances::pair_friction(
                structure.chunks[joint.a].material.friction,
                other_friction,
            ));
            let contact = state.cracked[j] || matches!(joint.bond, Bond::Resting);
            let fixing = if contact {
                None
            } else {
                joint.seam().and_then(|s| s.fixing)
            };
            self.edges.push(Edge {
                a,
                b,
                centre: patch.centre,
                normal: toward_a,
                area,
                patch,
                contact: contact.then_some(friction),
                slip: None,
                bearing: fixing.map(|_| Bearing {
                    normal: area / normal,
                    shear: area / shear,
                    friction,
                }),
                normal_stiffness: fixing.map_or(area / normal, |f| area * f64::from(f.normal)),
                shear_stiffness: fixing.map_or(area / shear, |f| area * f64::from(f.shear)),
            });
            self.joints.push(j);
        }
        // An isolated chunk has no internal stress degrees of freedom; connectivity releases it.
        let mut incident = vec![false; self.nodes.len()];
        for edge in &self.edges {
            incident[edge.a] = true;
            if let Some(b) = edge.b {
                incident[b] = true;
            }
        }
        for (node, incident) in self.nodes.iter_mut().zip(incident) {
            node.fixed |= !incident;
        }
        let mut neighbours = vec![Vec::new(); self.nodes.len()];
        let mut anchored: Vec<_> = self.nodes.iter().map(|n| n.fixed).collect();
        for e in &self.edges {
            if let Some(b) = e.b {
                neighbours[e.a].push(b);
                neighbours[b].push(e.a);
            } else {
                anchored[e.a] = true;
            }
        }
        self.free_groups.clear();
        let mut seen = vec![false; self.nodes.len()];
        for start in 0..self.nodes.len() {
            if seen[start] {
                continue;
            }
            let mut stack = vec![start];
            let mut group = Vec::new();
            let mut held = false;
            while let Some(c) = stack.pop() {
                if seen[c] {
                    continue;
                }
                seen[c] = true;
                held |= anchored[c];
                group.push(c);
                stack.extend(neighbours[c].iter().copied().filter(|&n| !seen[n]));
            }
            if !held {
                self.free_groups.push(group);
            }
        }
        // A free component's six rigid modes are gauge freedom, not supports. Fixing one
        // node's coordinate frame removes that nullspace after its resultant was balanced.
        for group in &self.free_groups {
            self.nodes[group[0]].fixed = true;
        }
        let n = self.nodes.len();
        self.solve_nodes = self.nodes.iter().chain(&self.nodes).copied().collect();
        self.solve_edges = self.edges.clone();
        self.solve_edges.extend(self.edges.iter().map(|e| Edge {
            a: e.a + n,
            b: e.b.map(|b| b + n),
            ..e.clone()
        }));
        // A newly owned node has no previous displacement; existing nodes retain both columns.
        let warm: Vec<_> = self
            .members
            .iter()
            .map(|c| previous.get(c).copied().unwrap_or_default())
            .collect();
        self.solve = Solve {
            answer: warm
                .iter()
                .map(|v| v.0)
                .chain(warm.iter().map(|v| v.1))
                .collect(),
            ..Solve::default()
        };
        self.loads.clear();
        self.topology_changed = false;
        self.initialized = true;
    }

    /// One bounded equilibrium solve with sustained and pulse load columns. A pulse enters
    /// as J/step, capped by its stated peak, and must add G_f A of elastic work to an overloaded
    /// patch. Force-controlled drivers supply continuing separation work. All qualifying joints
    /// fail together only after equilibrium; the next tick remaps topology and redistributes.
    /// This quasi-static pulse approximation does not resolve waves or cohesive softening.
    pub(crate) fn advance(
        &mut self,
        structure: &Structure,
        state: &mut State,
        sources: &[Load],
        spin: bevy::math::Vec3,
        step: f32,
        changed: bool,
    ) -> (Vec<Failure>, usize) {
        assert!(
            step.is_finite() && step > 0.0,
            "the structure solve has a positive numerical tick"
        );
        self.initial_verdict.clear();
        self.last_spin = spin;
        let membership_changed = !self.members.iter().copied().eq(state
            .members
            .iter()
            .copied()
            .filter(|&c| state.standing(c)));
        let edges_changed = !self.joints.iter().copied().eq(state
            .joints
            .iter()
            .copied()
            .filter(|&j| !state.separated[j] && state.live(&structure.joints[j])));
        let topology = !self.initialized
            || self.topology_changed
            || changed
            || membership_changed
            || edges_changed;
        if topology {
            self.rebuild(structure, state);
        }
        let n = self.nodes.len();
        let mut steady = vec![Wrench::default(); n];
        let mut pulse = vec![Wrench::default(); n];
        self.impulsive = false;
        for source in sources {
            let Some(&c) = self.local.get(&source.chunk) else {
                continue;
            }; // Removal consumed this carrier.
            let mut force = source.blow.as_dvec3() / f64::from(step);
            if let Some(peak) = source.peak {
                let magnitude = force.length();
                if magnitude > f64::from(peak.length()) {
                    force *= f64::from(peak.length()) / magnitude;
                }
            }
            let arm = source.at.as_dvec3() - self.nodes[c].centre;
            steady[c].add(Wrench::at(source.press.as_dvec3(), arm), 1.0);
            pulse[c].add(Wrench::at(force, arm), 1.0);
            self.impulsive |= source.blow != bevy::math::Vec3::ZERO;
        }
        for group in &self.free_groups {
            let nodes: Vec<_> = group.iter().map(|&c| self.nodes[c]).collect();
            let masses: Vec<_> = group.iter().map(|&c| self.masses[c]).collect();
            let own: Vec<_> = group.iter().map(|&c| self.own_inertia[c]).collect();
            let mut framed: Vec<_> = group.iter().map(|&c| steady[c]).collect();
            let mut impulses: Vec<_> = group.iter().map(|&c| pulse[c]).collect();
            accelerating_frame(&nodes, &masses, &own, spin.as_dvec3(), &mut framed);
            accelerating_frame(&nodes, &masses, &own, DVec3::ZERO, &mut impulses);
            for ((&c, load), impulse) in group.iter().zip(framed).zip(impulses) {
                steady[c] = load;
                pulse[c] = impulse;
            }
        }
        for (load, gravity) in steady.iter_mut().zip(self.gravity()) {
            load.add(gravity, 1.0);
        }
        // Contact is nonlinear: solve sustained and TOTAL loads, never superpose two
        // independently projected reactions. Their energy difference identifies pulse work.
        let total: Vec<_> = steady
            .iter()
            .zip(pulse)
            .map(|(&s, p)| {
                let mut sum = s;
                sum.add(p, 1.0);
                sum
            })
            .collect();
        let loads: Vec<_> = steady.into_iter().chain(total).collect();
        if topology || loads != self.loads {
            self.solve.changed();
        }
        self.loads = loads;
        let rounds = self.solve.advance(
            &Problem {
                nodes: &self.solve_nodes,
                edges: &self.solve_edges,
                loads: &self.loads,
            },
            ROUNDS,
        );
        let failures = self.failures(structure);
        let failed: HashMap<_, _> = failures.iter().map(|f| (f.joint, f)).collect();
        for (&j, edge) in self.joints.iter().zip(&self.edges) {
            let Some(failure) = failed.get(&j) else {
                continue;
            }; // This joint retained cohesion.
            state.cracked[failure.joint] = true;
            let total = &self.solve.answer[n..];
            if self.solve.progress == Progress::Mechanism
                || edge.samples().iter().all(|(point, _)| {
                    edge.relative(&self.nodes, total, *point).dot(edge.normal) > 0.0
                })
            {
                state.separated[failure.joint] = true;
            }
        }
        self.topology_changed = !failures.is_empty();
        if topology || self.topology_changed {
            self.support = super::loose_groups(structure, state);
        }
        (failures, rounds)
    }
    fn failures(&self, structure: &Structure) -> Vec<Failure> {
        if matches!(self.solve.progress, Progress::Changed | Progress::Iterating) {
            return Vec::new();
        } // Unfinished equilibrium resumes next tick; its trial stresses cannot change topology.
        let n = self.nodes.len();
        let steady_answer = &self.solve.answer[..n];
        let combined = &self.solve.answer[n..];
        let directions = [
            &self.solve.linear.direction[..n.min(self.solve.linear.direction.len())],
            &self.solve.linear.direction[n.min(self.solve.linear.direction.len())..],
        ];
        let motion_at = |edge: &Edge, direction: &[Wrench]| -> f64 {
            edge.samples()
                .iter()
                .map(|(point, _)| edge.relative(&self.nodes, direction, *point).length())
                .fold(0.0_f64, f64::max)
        };
        let reference = if self.solve.progress == Progress::Mechanism {
            self.edges
                .iter()
                .filter(|e| e.contact.is_some())
                .flat_map(|e| directions.map(|d| motion_at(e, d)))
                .fold(0.0_f64, f64::max)
        } else {
            0.0
        };
        let mut failures = Vec::new();
        for (&j, edge) in self.joints.iter().zip(&self.edges) {
            let joint = &structure.joints[j];
            if edge.contact.is_some() {
                if self.solve.progress == Progress::Mechanism {
                    let direction =
                        if motion_at(edge, directions[1]) >= motion_at(edge, directions[0]) {
                            directions[1]
                        } else {
                            directions[0]
                        };
                    let motion = edge.relative(&self.nodes, direction, edge.centre);
                    let spin = direction[edge.a].angular
                        - edge.b.map_or(DVec3::ZERO, |b| direction[b].angular);
                    if motion_at(edge, direction) > reference * RELATIVE_RESIDUAL {
                        let bending = spin - edge.normal * spin.dot(edge.normal);
                        let rotation_motion = bending.length() * edge.patch.polar.sqrt();
                        let cause = if rotation_motion > reference * RELATIVE_RESIDUAL {
                            Cause::Tip
                        } else if motion.dot(edge.normal) > reference * RELATIVE_RESIDUAL {
                            Cause::LiftForce
                        } else {
                            Cause::Slide
                        };
                        failures.push(Failure {
                            joint: j,
                            cause,
                            // A mechanism has no local force threshold; its cause records
                            // the missing equilibrium, not an arbitrary displacement scale.
                            demand: 0.0,
                            capacity: 0.0,
                            work: 0.0,
                        });
                    }
                }
                continue; // Admissible contacts never undergo cohesive fracture.
            }
            if self.solve.progress == Progress::Mechanism {
                continue;
            }
            let carried = edge.resultant(&self.nodes, combined);
            let sustained = edge.resultant(&self.nodes, steady_answer);
            let normal = if (self.nodes[edge.a].centre - edge.centre).dot(edge.normal) >= 0.0 {
                edge.normal
            } else {
                -edge.normal
            };
            let (tension, shear) = edge.stress(carried, normal);
            let (steady_tension, steady_shear) = edge.stress(sustained, normal);
            let total_energy = edge.energy(&self.nodes, combined);
            let steady_energy = edge.energy(&self.nodes, steady_answer);
            let compression = -carried.linear.dot(normal);
            let other_friction = match joint.b {
                Side::Chunk(b) => structure.chunks[b].material.friction,
                Side::Ground => structure.chunks[joint.a].material.friction,
            };
            let friction = if edge.bearing.is_some() {
                0.0
            } else {
                f64::from(crate::substances::pair_friction(
                    structure.chunks[joint.a].material.friction,
                    other_friction,
                ))
            };
            let failure = match joint.bond {
                Bond::Resting => {
                    unreachable!("resting contacts are projected before cohesive failure")
                }
                Bond::Bonded(seam) | Bond::AreaShare { seam, .. } => {
                    let cohesion = f64::from(seam.shear.cohesion);
                    let energy = f64::from(seam.shear.energy);
                    let opening_work = edge.area * f64::from(seam.fracture_energy);
                    let sliding_work = edge.area * energy;
                    let static_shear_capacity =
                        cohesion + friction * (-sustained.linear.dot(normal) / edge.area).max(0.0);
                    if steady_tension > f64::from(seam.tensile) {
                        Some((Cause::Tension, steady_tension, f64::from(seam.tensile), 0.0))
                    } else if steady_shear > static_shear_capacity {
                        Some((Cause::Shear, steady_shear, static_shear_capacity, 0.0))
                    } else if tension > f64::from(seam.tensile)
                        && total_energy.0 - steady_energy.0 >= opening_work
                    {
                        Some((
                            Cause::Tension,
                            tension,
                            f64::from(seam.tensile),
                            opening_work,
                        ))
                    } else if shear > cohesion + friction * (compression / edge.area).max(0.0)
                        && total_energy.1 - steady_energy.1 >= sliding_work
                    {
                        Some((
                            Cause::Shear,
                            shear,
                            cohesion + friction * (compression / edge.area).max(0.0),
                            sliding_work,
                        ))
                    } else {
                        None
                    }
                }
            };
            if let Some((cause, demand, capacity, cost)) = failure {
                failures.push(Failure {
                    joint: j,
                    cause,
                    demand: demand as f32,
                    capacity: capacity as f32,
                    work: cost as f32,
                });
            }
        }
        failures
    }
}

#[cfg(test)]
mod gates {
    use super::super::{Chunk, Material};
    use super::*;
    use bevy::math::Vec3;
    use std::f64::consts::PI;

    // Fixture: geometry, mass and tick duration isolate the constitutive limits from shipped assets.
    const STEP: f32 = 1.0 / 64.0;
    const AREA: f32 = 1.0;
    const MASS: f32 = 100.0;

    fn material() -> Material {
        Material::of(
            crate::substances::SubstanceRegistry::shipped_static(),
            "Brick",
        )
        .unwrap()
        .unwrap()
    }

    fn resting(height: f32) -> Structure {
        let material = material();
        Structure {
            geometry: crate::construction::fracture::bondsolve::Geometry::CircularFixture,
            chunks: [-height, height]
                .map(|h| Chunk {
                    volume: AREA,
                    mass: MASS,
                    centroid: Vec3::Y * h,
                    material,
                    buried: h < 0.0,
                })
                .to_vec(),
            joints: vec![super::super::Joint {
                a: 1,
                b: Side::Chunk(0),
                area: AREA,
                centroid: Vec3::ZERO,
                normal: Vec3::Y,
                bond: Bond::Resting,
            }],
        }
    }

    fn press(chunk: usize, at: Vec3, force: Vec3) -> Load {
        Load {
            chunk,
            at,
            press: force,
            blow: Vec3::ZERO,
            peak: None,
            energy: 0.0,
            crushed: 0.0,
            origin: super::super::Origin::Field,
        }
    }

    /// A warm guess can carry obsolete force after a load or topology change. The authority
    /// must preserve cohesion while equilibrium is unfinished, even when that guess overloads
    /// a joint. The hanging chain's actual maximum tension is its total weight over area.
    #[test]
    fn an_unfinished_warm_solve_cannot_commit_a_crack() {
        const CHUNKS: usize = 3 * ROUNDS; // Fixture: axial rank exceeds the per-tick round budget.
        let material = material();
        let structure = Structure {
            geometry: Geometry::CircularFixture,
            chunks: (0..CHUNKS)
                .map(|i| Chunk {
                    volume: AREA,
                    mass: MASS,
                    centroid: -Vec3::Y * (i as f32 + 0.5),
                    material,
                    buried: false,
                })
                .collect(),
            joints: (0..CHUNKS)
                .map(|i| super::super::Joint {
                    a: i,
                    b: if i == 0 {
                        Side::Ground
                    } else {
                        Side::Chunk(i - 1)
                    },
                    area: AREA,
                    centroid: -Vec3::Y * i as f32,
                    normal: Vec3::Y,
                    bond: Bond::Bonded(material.seam),
                })
                .collect(),
        };
        let actual_tension = CHUNKS as f32 * MASS * crate::physical_laws::gravity_mps2() / AREA;
        assert!(actual_tension < material.seam.tensile);
        let mut state = State::new(&structure);
        let mut assessment = Assessment::default();
        assessment.rebuild(&structure, &state);
        // Fixture: an obsolete axial stretch, deliberately beyond the bond's strength.
        for column in assessment.solve.answer.chunks_mut(CHUNKS) {
            for (i, w) in column.iter_mut().enumerate() {
                w.linear = -DVec3::Y * (i as f64 + 1.0);
            }
        }
        let (failures, rounds) =
            assessment.advance(&structure, &mut state, &[], Vec3::ZERO, STEP, true);
        assert_eq!(rounds, ROUNDS);
        assert_eq!(assessment.solve.progress, Progress::Iterating);
        assert!(
            failures.is_empty(),
            "unfinished trial reactions cannot commit {} cracks",
            failures.len()
        );
        assert!(state.cracked.iter().all(|&cracked| !cracked));
        assert!(assessment.needs_tick());
        for _ in 0..CHUNKS {
            let (failures, _) =
                assessment.advance(&structure, &mut state, &[], Vec3::ZERO, STEP, false);
            assert!(failures.is_empty());
            if !assessment.needs_tick() {
                break;
            }
        }
        assert_eq!(assessment.solve.progress, Progress::Equilibrated);
        assert!(state.cracked.iter().all(|&cracked| !cracked));
    }

    /// Contact pressure is still support after cohesion is lost. The game's record and loose
    /// groups must distinguish that closed crack from subsequent separation under lift.
    #[test]
    fn a_closed_crack_keeps_compression_contact_until_it_separates() {
        let mut structure = resting(0.5);
        structure.joints[0].bond = Bond::Bonded(material().seam);
        let mut state = State::new(&structure);
        state.cracked[0] = true; // Fixture: cohesion lost, both occupied faces still touch.
        let mut assessment = Assessment::default();
        let (failures, _) = assessment.advance(&structure, &mut state, &[], Vec3::ZERO, STEP, true);
        assert!(failures.is_empty());
        assert!(assessment.support.is_empty());
        assert!(!state.separated[0]);
        let lift = press(
            1,
            structure.chunks[1].centroid,
            Vec3::Y * (1.01 * MASS * crate::physical_laws::gravity_mps2()),
        );
        for _ in 0..256 {
            assessment.advance(&structure, &mut state, &[lift], Vec3::ZERO, STEP, false);
            if state.separated[0] {
                break;
            }
        }
        assert!(state.separated[0]);
        assert_eq!(assessment.support, vec![vec![1]]);
    }

    /// Unit source normals rounded to f32 are not unit in f64. Without renormalizing at the
    /// solve edge, a holding contact leaks normal traction into shear and stalls convergence.
    #[test]
    fn contact_projection_uses_a_unit_normal_in_solver_precision() {
        let mut structure = resting(0.5);
        structure.joints[0].normal = Vec3::new(0.3, 1.0, 0.0).normalize(); // Fixture sub-friction slope.
        let assessment = Assessment::at_rest(&structure).unwrap();
        assert_eq!(assessment.solve.progress, Progress::Equilibrated);
        assert!(assessment.initial_verdict.is_empty());
        assert!((assessment.edges[0].normal.length_squared() - 1.0).abs() < 8.0 * f64::EPSILON);
    }

    /// A fixing's slip slope belongs to its attachment data. Changing bulk elasticity must
    /// change closed-face bearing without stiffening the fixing or changing its strength.
    #[test]
    fn fixing_slip_and_bulk_bearing_have_distinct_owners() {
        let mut structure = resting(0.5);
        let fixing = super::super::Fixing {
            normal: 1e6,
            shear: 2e6,
        }; // Fixture interface slopes, N/m³.
        structure.joints[0].bond = Bond::Bonded(super::super::Seam {
            fixing: Some(fixing),
            ..material().seam
        });
        let mut first = Assessment::default();
        first.rebuild(&structure, &State::new(&structure));
        let e = &first.edges[0];
        assert_eq!(
            e.normal_stiffness,
            f64::from(AREA) * f64::from(fixing.normal)
        );
        assert_eq!(e.shear_stiffness, f64::from(AREA) * f64::from(fixing.shear));
        let bearing = e.bearing.unwrap();
        for chunk in &mut structure.chunks {
            chunk.material.young *= 2.0; // Fixture perturbation of the bulk owner alone.
            chunk.material.shear_modulus *= 2.0;
        }
        let mut second = Assessment::default();
        second.rebuild(&structure, &State::new(&structure));
        let changed = &second.edges[0];
        assert_eq!(changed.normal_stiffness, e.normal_stiffness);
        assert_eq!(changed.shear_stiffness, e.shear_stiffness);
        assert_eq!(changed.bearing.unwrap().normal, 2.0 * bearing.normal);
        assert_eq!(changed.bearing.unwrap().shear, 2.0 * bearing.shear);
        assert_eq!(
            structure.joints[0].seam().unwrap().tensile,
            material().seam.tensile
        );
    }

    /// The game's resting joints must slide at mu*N and tip at M=N*r, using the pair's friction
    /// and their own bearing lever. A tension-capable rest, or a no-bending point joint, fails.
    #[test]
    fn a_beam_on_a_wall_slides_or_tips_at_its_friction_and_bearing_lever() {
        let mu = crate::substances::pair_friction(material().friction, material().friction);
        let radius = (f64::from(AREA) / PI).sqrt() as f32;
        let weight = MASS * crate::physical_laws::gravity_mps2();
        for (height, limit, cause) in [
            (radius / (2.0 * mu), mu * weight, Cause::Slide),
            (2.0 * radius / mu, weight * mu / 2.0, Cause::Tip),
        ] {
            for reversed in [false, true] {
                let mut structure = resting(height);
                if reversed {
                    structure.joints[0].a = 0;
                    structure.joints[0].b = Side::Chunk(1);
                }
                for ratio in [0.99, 1.01] {
                    // Fixture: straddle the independently derived limit.
                    let mut state = State::new(&structure);
                    let mut assessment = Assessment::default();
                    let mut failures = Vec::new();
                    for _ in 0..256 {
                        let (batch, _) = assessment.advance(
                            &structure,
                            &mut state,
                            &[press(
                                1,
                                structure.chunks[1].centroid,
                                Vec3::X * (limit * ratio),
                            )],
                            Vec3::ZERO,
                            STEP,
                            false,
                        );
                        failures.extend(batch);
                        if !failures.is_empty() || !assessment.needs_tick() {
                            break;
                        }
                    }
                    assert_eq!(
                        failures.len(),
                        usize::from(ratio > 1.0),
                        "{cause:?}, reversed {reversed}, ratio {ratio}: {failures:?}, progress {:?}",
                        assessment.solve.progress
                    );
                    if let Some(failure) = failures.first() {
                        assert_eq!(failure.cause, cause);
                    }
                }
            }
        }
    }

    /// A cantilever's circular root carries M=m*g*lever; its section modulus is A*r/4.
    /// The game's release path must straddle that independently derived bending threshold.
    #[test]
    fn a_cantilever_past_its_derived_limit_falls_and_one_within_it_stands() {
        let material = material();
        let lever = 2.0_f32; // Fixture cantilever COM lever, m.
        let radius = (AREA / std::f32::consts::PI).sqrt();
        let limit = material.seam.tensile * AREA * radius
            / (4.0 * crate::physical_laws::gravity_mps2() * lever);
        assert!(
            limit * crate::physical_laws::gravity_mps2() / AREA < material.seam.shear.cohesion,
            "bending is the fixture's first limit"
        );
        for ratio in [0.98, 1.02] {
            let structure = Structure {
                geometry: Geometry::CircularFixture,
                chunks: vec![Chunk {
                    volume: 1.0,
                    mass: limit * ratio,
                    centroid: Vec3::X * lever,
                    material,
                    buried: false,
                }],
                joints: vec![super::super::Joint {
                    a: 0,
                    b: Side::Ground,
                    area: AREA,
                    centroid: Vec3::ZERO,
                    normal: Vec3::X,
                    bond: Bond::Bonded(material.seam),
                }],
            };
            let mut live = crate::construction::breakage::Live::from_structure(structure, 1);
            let first = crate::construction::breakage::run_laws(
                &[0],
                &mut live,
                &[],
                Vec::new(),
                Vec3::ZERO,
                STEP,
            );
            assert_eq!(live.state.cracked[0], ratio > 1.0);
            let mut groups = first.groups.len();
            for _ in 0..256 {
                if groups > 0 {
                    break;
                }
                groups += crate::construction::breakage::run_laws(
                    &[0],
                    &mut live,
                    &[],
                    Vec::new(),
                    Vec3::ZERO,
                    STEP,
                )
                .groups
                .len();
                if groups > 0 || !live.assessment.needs_tick() {
                    break;
                }
            }
            assert_eq!(groups, usize::from(ratio > 1.0));
        }
    }

    /// Two identical overhead attachments share the panel weight. Direct tension reaches
    /// sigma at total mass 2*A*sigma/g, independently of stiffness and the numerical step.
    #[test]
    fn a_hanging_panel_holds_up_to_its_derived_limit() {
        let material = material();
        let limit = 2.0 * AREA * material.seam.tensile / crate::physical_laws::gravity_mps2();
        for ratio in [0.98, 1.02] {
            let structure = Structure {
                geometry: Geometry::CircularFixture,
                chunks: vec![Chunk {
                    volume: 1.0,
                    mass: limit * ratio,
                    centroid: Vec3::NEG_Y,
                    material,
                    buried: false,
                }],
                joints: [-1.0, 1.0]
                    .map(|x| super::super::Joint {
                        a: 0,
                        b: Side::Ground,
                        area: AREA,
                        centroid: Vec3::X * x,
                        normal: Vec3::Y,
                        bond: Bond::Bonded(material.seam),
                    })
                    .to_vec(),
            };
            let mut live = crate::construction::breakage::Live::from_structure(structure, 1);
            let first = crate::construction::breakage::run_laws(
                &[0],
                &mut live,
                &[],
                Vec::new(),
                Vec3::ZERO,
                STEP,
            );
            assert_eq!(live.state.cracked, vec![ratio > 1.0; 2]);
            let mut groups = first.groups.len();
            for _ in 0..256 {
                if groups > 0 {
                    break;
                }
                groups += crate::construction::breakage::run_laws(
                    &[0],
                    &mut live,
                    &[],
                    Vec::new(),
                    Vec3::ZERO,
                    STEP,
                )
                .groups
                .len();
                if groups > 0 || !live.assessment.needs_tick() {
                    break;
                }
            }
            assert_eq!(groups, usize::from(ratio > 1.0));
        }
    }

    /// The game's finite pulse must beat both traction strength and G_f*A, while a sustained
    /// driver can supply continuing separation work. This derives the one-patch limits without
    /// using the solver's answer or a former run as the expected value.
    #[test]
    fn a_blow_pays_the_patches_fracture_work_and_a_press_is_force_controlled() {
        let material = material();
        let height = 0.5; // Fixture representative half-length, m, of a unit-volume chunk.
        let structure = Structure {
            geometry: crate::construction::fracture::bondsolve::Geometry::CircularFixture,
            chunks: vec![Chunk {
                volume: 1.0,
                mass: MASS,
                centroid: Vec3::Y * height,
                material,
                buried: false,
            }],
            joints: vec![super::super::Joint {
                a: 0,
                b: Side::Ground,
                area: AREA,
                centroid: Vec3::ZERO,
                normal: Vec3::Y,
                bond: Bond::Bonded(material.seam),
            }],
        };
        let weight = f64::from(MASS * crate::physical_laws::gravity_mps2());
        let stiffness = material.young * f64::from(AREA) / f64::from(height);
        let work = f64::from(material.seam.fracture_energy * AREA);
        let strength = f64::from(material.seam.tensile * AREA);
        // DERIVED: U(F-J) - U(-mg) = ((F-mg)^2-(mg)^2)/(2k).
        let energy_limit = weight + (weight * weight + 2.0 * stiffness * work).sqrt();
        let limit = energy_limit.max(weight + strength);
        for ratio in [0.99, 1.01] {
            let mut source = press(0, structure.chunks[0].centroid, Vec3::ZERO);
            source.blow = Vec3::Y * (limit * ratio * f64::from(STEP)) as f32;
            let mut state = State::new(&structure);
            let (failures, _) = Assessment::default().advance(
                &structure,
                &mut state,
                &[source],
                Vec3::ZERO,
                STEP,
                false,
            );
            assert_eq!(
                failures.len(),
                usize::from(ratio > 1.0),
                "ratio {ratio}: {failures:?}"
            );
            if let Some(failure) = failures.first() {
                assert!((f64::from(failure.work) - work).abs() < work * 1e-5);
            }
        }
        let force = (weight + strength) * 1.01; // Fixture: strength exceeded, regardless of pulse work.
        let (failures, _) = Assessment::default().advance(
            &structure,
            &mut State::new(&structure),
            &[press(
                0,
                structure.chunks[0].centroid,
                Vec3::Y * force as f32,
            )],
            Vec3::ZERO,
            STEP,
            false,
        );
        assert_eq!(failures.len(), 1);
        assert_eq!(
            failures[0].work, 0.0,
            "the maintained driver supplies separation work"
        );
    }

    /// A running-bond wall admits a staircase displacement jump. The game's solve must recover
    /// force on exactly the seams crossing that jump, with no coplanar-cut enumeration. Applied
    /// wrenches are an independent constant-translation patch calculation at seam centres.
    #[test]
    fn a_running_bond_wall_can_crack_along_a_staircase_of_seams() {
        // Fixture: three offset courses, four unit bricks per course, unit depth and height.
        let mut nodes = Vec::new();
        let mut shifted = Vec::new();
        for row in 0..3 {
            for col in 0..4 {
                let x = col as f64 + 0.5 + (row % 2) as f64 * 0.5;
                let moving = x > 1.0 + row as f64 * 0.5;
                nodes.push(Node {
                    centre: DVec3::new(x, row as f64 + 0.5, 0.0),
                    fixed: !moving,
                });
                shifted.push(moving);
            }
        }
        let mut edges = Vec::new();
        for a in 0..nodes.len() {
            for b in a + 1..nodes.len() {
                let d = nodes[b].centre - nodes[a].centre;
                let area = if d.y == 0.0 && d.x == 1.0 {
                    1.0
                } else if d.y == 1.0 && d.x.abs() < 1.0 {
                    1.0 - d.x.abs()
                } else {
                    continue;
                };
                let normal = if d.y == 0.0 { DVec3::X } else { DVec3::Y };
                edges.push(Edge {
                    a,
                    b: Some(b),
                    centre: (nodes[a].centre + nodes[b].centre) / 2.0,
                    normal,
                    area,
                    patch: Patch::circle((nodes[a].centre + nodes[b].centre) / 2.0, normal, area),
                    contact: None,
                    bearing: None,
                    slip: None,
                    normal_stiffness: 8e9 * area,
                    shear_stiffness: 3e9 * area,
                });
            }
        }
        let displacement = DVec3::X * 0.001; // Fixture translation jump, m.
        let mut loads = vec![Wrench::default(); nodes.len()];
        let mut expected = Vec::new();
        for (j, e) in edges.iter().enumerate() {
            let b = e.b.unwrap();
            if shifted[e.a] == shifted[b] {
                continue;
            }
            expected.push(j);
            let delta = displacement * (f64::from(shifted[e.a]) - f64::from(shifted[b]));
            let normal = e.normal * e.normal.dot(delta);
            let force = normal * e.normal_stiffness + (delta - normal) * e.shear_stiffness;
            loads[e.a].add(Wrench::at(force, e.centre - nodes[e.a].centre), 1.0);
            loads[b].add(Wrench::at(force, e.centre - nodes[b].centre), -1.0);
        }
        let mut solve = Solve::default();
        let problem = Problem {
            nodes: &nodes,
            edges: &edges,
            loads: &loads,
        };
        for _ in 0..32 {
            solve.advance(&problem, ROUNDS);
            if solve.progress == Progress::Equilibrated {
                break;
            }
        }
        assert_eq!(solve.progress, Progress::Equilibrated);
        let strength = 1e6; // Fixture: below E*jump and G*jump, above numerical residuals.
        let cracked: Vec<_> = edges
            .iter()
            .enumerate()
            .filter_map(|(j, e)| {
                let stress = e.stress(e.resultant(&nodes, &solve.answer), -e.normal);
                (stress.0 > strength || stress.1 > strength).then_some(j)
            })
            .collect();
        assert_eq!(cracked, expected);
        assert!(cracked.iter().any(|&j| edges[j].normal == DVec3::X));
        assert!(cracked.iter().any(|&j| edges[j].normal == DVec3::Y));
    }

    /// Thin faces retain both section moduli. A circular replacement
    /// changes bending stiffness and strength in opposite directions along these two axes.
    /// Fixture: a 2 m by 0.1 m face, with pure couples about each in-plane axis.
    #[test]
    fn rectangular_faces_read_their_own_second_moments() {
        let (width, thickness) = (2.0_f32, 0.1_f32);
        let joint = super::super::Joint {
            a: 0,
            b: Side::Ground,
            area: width * thickness,
            centroid: Vec3::ZERO,
            normal: Vec3::Y,
            bond: Bond::Resting,
        };
        let corners = [(-1.0, -1.0), (-1.0, 1.0), (1.0, 1.0), (1.0, -1.0)]
            .map(|(x, z)| Vec3::new(x * width / 2.0, 0.0, z * thickness / 2.0));
        let patch = Patch::face(&joint, &corners).unwrap();
        let edge = Edge {
            a: 0,
            b: None,
            centre: DVec3::ZERO,
            normal: DVec3::Y,
            area: f64::from(joint.area),
            patch,
            contact: None,
            bearing: None,
            slip: None,
            normal_stiffness: material().young,
            shear_stiffness: material().shear_modulus,
        };
        // Fixture: unit couple. I = b*h³/12 and S = b*h²/6, independently of the solver.
        for (axis, breadth, depth) in [(DVec3::X, width, thickness), (DVec3::Z, thickness, width)] {
            let second = f64::from(depth).powi(2) / 12.0;
            let modulus = f64::from(breadth) * f64::from(depth).powi(2) / 6.0;
            let close = |actual: f64, expected: f64| {
                assert!(
                    (actual / expected - 1.0).abs() < 1e-6,
                    "{actual} vs {expected}"
                )
            };
            close(edge.couple(axis).dot(axis), edge.normal_stiffness * second);
            close(
                edge.stress(
                    Wrench {
                        linear: DVec3::ZERO,
                        angular: axis,
                    },
                    DVec3::Y,
                )
                .0,
                modulus.recip(),
            );
            let scaled = edge.patch.scaled(2.0);
            close(scaled.polar, edge.patch.polar * 4.0);
        }
    }

    /// Sparse work grows with joint count at fixed round count, including on the shipped graph.
    /// An equilibrated owner performs no sparse products; no wall-clock timing is asserted.
    #[test]
    fn house_and_doubled_house_use_linear_joint_visits_and_idle_is_zero() {
        let kind = crate::construction::tests::shipped_house();
        let structure = kind.layout.structure(&kind.asset, 1.0);
        let mut one = Assessment::default();
        let (_, rounds) = one.advance(
            &structure,
            &mut State::new(&structure),
            &[],
            Vec3::ZERO,
            STEP,
            false,
        );
        let base_visits = one.solve.joint_visits;
        let problem = Problem {
            nodes: &one.solve_nodes,
            edges: &one.solve_edges,
            loads: &one.loads,
        };
        let mut single = Solve::default();
        single.advance(&problem, 0);
        single.advance(&problem, 1);
        println!(
            "MEASURED one sparse round: {} joint visits",
            single.joint_visits
        );

        let mut total_rounds = rounds;
        for _ in 0..one.nodes.len() * 6 / ROUNDS + 1 {
            if one.solve.progress == Progress::Equilibrated {
                break;
            }
            total_rounds += one.solve.advance(
                &Problem {
                    nodes: &one.solve_nodes,
                    edges: &one.solve_edges,
                    loads: &one.loads,
                },
                ROUNDS,
            );
        }
        println!(
            "MEASURED bounded cold observation: {total_rounds} rounds, state {:?}, {} current joint overloads",
            one.solve.progress,
            one.failures(&structure).len()
        );
        let mut doubled = structure.clone();
        if let Geometry::Faces(patches) = &mut doubled.geometry {
            patches.extend(patches.clone().into_iter().map(|mut p| {
                p.centre += DVec3::X * 100.0;
                p
            }));
        }
        let n = structure.chunks.len();
        doubled
            .chunks
            .extend(structure.chunks.iter().map(|&c| super::super::Chunk {
                centroid: c.centroid + Vec3::X * 100.0,
                ..c
            }));
        doubled
            .joints
            .extend(structure.joints.iter().map(|&j| super::super::Joint {
                a: j.a + n,
                b: match j.b {
                    Side::Chunk(b) => Side::Chunk(b + n),
                    Side::Ground => Side::Ground,
                },
                centroid: j.centroid + Vec3::X * 100.0,
                bond: match j.bond {
                    Bond::AreaShare {
                        seam,
                        footprint_area,
                        footprint_centroid,
                    } => Bond::AreaShare {
                        seam,
                        footprint_area,
                        footprint_centroid: footprint_centroid + Vec3::X * 100.0,
                    },
                    bond => bond,
                },
                ..j
            }));
        let mut two = Assessment::default();
        let (_, doubled_rounds) = two.advance(
            &doubled,
            &mut State::new(&doubled),
            &[],
            Vec3::ZERO,
            STEP,
            false,
        );
        assert_eq!(rounds, doubled_rounds);
        assert_eq!(two.solve.joint_visits, 2 * base_visits);
        println!(
            "MEASURED sparse visits: house {base_visits}, doubled {}, rounds {rounds}",
            two.solve.joint_visits
        );
        let fixture = resting(0.5);
        let mut idle = Assessment::default();
        let mut state = State::new(&fixture);
        idle.advance(&fixture, &mut state, &[], Vec3::ZERO, STEP, false);
        let (failures, rounds) = idle.advance(&fixture, &mut state, &[], Vec3::ZERO, STEP, false);
        assert!(failures.is_empty());
        assert_eq!(rounds, 0);
        assert_eq!(idle.solve.joint_visits, 0);
    }
    /// Observation tool: lists current gravity demands; no shipped outcome is a pass condition.
    #[test]
    #[ignore = "gravity census, observational"]
    fn gravity_census() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        let mut ids: Vec<_> = std::fs::read_dir(root.join("constructions"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|id| {
                root.join(format!("constructions/{id}/{id}.construction.ron"))
                    .exists()
            })
            .collect();
        ids.sort();
        let library =
            crate::construction::ConstructionLibrary::load(&root, ids.iter().map(String::as_str));
        for id in &ids {
            let kind = library.get(id).unwrap();
            let structure = kind.layout.structure(&kind.asset, 1.0);
            let mut assessment = Assessment::at_rest(&structure).unwrap();
            println!(
                "CENSUS {id}: chunks={} joints={} initial={}",
                structure.chunks.len(),
                structure.joints.len(),
                assessment.initial_verdict.len()
            );
            for f in &assessment.initial_verdict {
                let j = &structure.joints[f.joint];
                let label = |c: usize| {
                    let c = &kind.asset.chunks[kind.layout.chunks[c] as usize];
                    format!(
                        "{}:{}",
                        kind.asset.elements[usize::from(c.element)].name,
                        kind.asset.materials[usize::from(c.material)]
                    )
                };
                let other = match j.b {
                    Side::Ground => "ground".to_owned(),
                    Side::Chunk(c) => label(c),
                };
                let bond = if f.joint < kind.asset.joints.len() {
                    match kind.asset.joints[f.joint].bond {
                        crate::construction::schema::Bond::Seam => "intrinsic",
                        crate::construction::schema::Bond::Attached(a) => {
                            &kind.asset.attachments[usize::from(a)]
                        }
                    }
                } else {
                    "anchor"
                };
                println!(
                    "JOINT,{id},{},{},{},{},{:?},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}",
                    f.joint,
                    label(j.a),
                    other,
                    bond,
                    f.cause,
                    f.demand,
                    f.capacity,
                    j.area,
                    j.centroid.x,
                    j.centroid.y,
                    j.centroid.z
                );
            }
            let mut state = State::new(&structure);
            for tick in 0..128 {
                let (failures, rounds) =
                    assessment.advance(&structure, &mut state, &[], Vec3::ZERO, STEP, false);
                let departed = observe_departures(&assessment, &mut state);
                println!(
                    "TICK {id} {tick} cracks={} fallen={} rounds={rounds} progress={:?}",
                    failures.len(),
                    departed,
                    assessment.solve.progress
                );
                if !assessment.needs_tick() {
                    break;
                }
            }
        }
    }
    fn observed_relative_residual(a: &Assessment) -> f64 {
        let problem = Problem {
            nodes: &a.solve_nodes,
            edges: &a.solve_edges,
            loads: &a.loads,
        };
        let r = a.solve.projected_residual(&problem);
        let mut z = r.clone();
        precondition(&r, &a.solve.metric, &mut z);
        let mut load = a.loads.clone();
        for (node, load) in a.solve_nodes.iter().zip(&mut load) {
            if node.fixed {
                *load = Wrench::default();
            }
        }
        let mut scaled_load = load.clone();
        precondition(&load, &a.solve.metric, &mut scaled_load);
        let numerator = dot(&r, &z);
        let denominator = dot(&load, &scaled_load);
        if denominator == 0.0 {
            assert_eq!(numerator, 0.0, "an unloaded equilibrium has no residual");
            0.0
        } else {
            (numerator / denominator).sqrt()
        }
    }

    fn observed_cohesive_uses(a: &Assessment, structure: &Structure) -> Vec<(usize, f64, f64)> {
        a.joints
            .iter()
            .zip(&a.edges)
            .filter_map(|(&j, e)| {
                let seam = structure.joints[j].seam()?;
                let force = e.resultant(&a.nodes, &a.solve.answer[..a.nodes.len()]);
                let (tension, shear) = e.stress(force, e.normal);
                let other = match structure.joints[j].b {
                    Side::Chunk(b) => structure.chunks[b].material.friction,
                    Side::Ground => structure.chunks[structure.joints[j].a].material.friction,
                };
                let friction = if e.bearing.is_some() {
                    0.0
                } else {
                    f64::from(crate::substances::pair_friction(
                        structure.chunks[structure.joints[j].a].material.friction,
                        other,
                    ))
                };
                let capacity = f64::from(seam.shear.cohesion)
                    + friction * (-force.linear.dot(e.normal) / e.area).max(0.0);
                Some((j, tension / f64::from(seam.tensile), shear / capacity))
            })
            .collect()
    }

    /// Law-only observations remove the same loose groups as the standing authority. Physical
    /// landings are deliberately outside this instrument.
    fn observe_departures(assessment: &Assessment, state: &mut State) -> usize {
        let fallen: Vec<_> = assessment
            .support
            .iter()
            .flatten()
            .copied()
            .filter(|&c| state.standing(c))
            .collect();
        for &c in &fallen {
            state.fate[c] = super::super::Fate::Fallen;
        }
        fallen.len()
    }

    /// Observation of every admitted scale and numerical stage, including the residual of the
    /// exact field used for judgement. Authored outcomes are observations, never assertions.
    #[test]
    #[ignore = "placed-scale gravity census, observational"]
    fn gravity_placed_scales() {
        use std::collections::{BTreeMap, BTreeSet};

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        let manifest = crate::map::load(&root);
        let mut rows = BTreeMap::<String, Vec<(f32, Vec<(usize, usize)>, bool)>>::new();
        let mut printed = BTreeSet::new();
        for instance in &manifest.instances {
            let crate::map::Prototype::Construction { asset } =
                &manifest.prototypes[&instance.prototype]
            else {
                continue;
            };
            let kind = manifest.constructions.get(asset).unwrap();
            if printed.insert(asset) {
                let structure = kind.structure(1.0);
                let a = kind.equilibrium(1.0);
                for (j, tension, shear) in observed_cohesive_uses(&a, &structure) {
                    let joint = &structure.joints[j];
                    let label = |c: usize| {
                        let c = &kind.asset.chunks[kind.layout.chunks[c] as usize];
                        format!(
                            "{}:{}",
                            kind.asset.elements[usize::from(c.element)].name,
                            kind.asset.materials[usize::from(c.material)]
                        )
                    };
                    let other = match joint.b {
                        Side::Ground => "ground".to_owned(),
                        Side::Chunk(c) => label(c),
                    };
                    let bond = if j < kind.asset.joints.len() {
                        match kind.asset.joints[j].bond {
                            crate::construction::schema::Bond::Seam => "intrinsic",
                            crate::construction::schema::Bond::Attached(k) => {
                                &kind.asset.attachments[usize::from(k)]
                            }
                        }
                    } else {
                        "anchor"
                    };
                    println!(
                        "UNITUSE,{asset},{j},{},{other},{bond},{tension:.9},{shear:.9},{:.6},{:.6},{:.6}",
                        label(joint.a),
                        joint.centroid.x,
                        joint.centroid.y,
                        joint.centroid.z
                    );
                }
            }
            let scale = instance.scale[0];
            let traced = scale
                == manifest
                    .instances
                    .iter()
                    .filter(|i| i.prototype == instance.prototype)
                    .map(|i| i.scale[0])
                    .fold(0.0_f32, f32::max);
            let structure = kind.structure(scale);
            let mut assessment = (*kind.equilibrium(scale)).clone();
            let initial = assessment.initial_verdict.len();
            println!(
                "ADMITTED,{},{asset},{scale:.9},{initial},{:?},{:.9e}",
                instance.id,
                assessment.solve.progress,
                observed_relative_residual(&assessment)
            );
            let mut ranked = observed_cohesive_uses(&assessment, &structure);
            ranked.sort_by(|a, b| b.1.max(b.2).total_cmp(&a.1.max(a.2)));
            for &(j, t, s) in ranked.iter().take(16) {
                println!("PEAK,{},{scale:.9},{j},{t:.9},{s:.9}", instance.id);
            }
            let trace: BTreeSet<_> = ranked.iter().take(8).map(|r| r.0).collect();
            let mut state = State::new(&structure);
            let mut stages = Vec::new();
            let mut ticks = 0;
            for tick in 0..128 {
                let (failures, rounds) =
                    assessment.advance(&structure, &mut state, &[], Vec3::ZERO, STEP, false);
                let departed = observe_departures(&assessment, &mut state);
                println!(
                    "JUDGE,{},{scale:.9},{tick},{},{},{:?},{:.9e},{rounds}",
                    instance.id,
                    failures.len(),
                    departed,
                    assessment.solve.progress,
                    observed_relative_residual(&assessment)
                );
                for f in &failures {
                    println!(
                        "FAIL,{},{tick},{},{:?},{:.6},{:.6}",
                        instance.id, f.joint, f.cause, f.demand, f.capacity
                    );
                }
                if traced {
                    for (&j, edge) in assessment
                        .joints
                        .iter()
                        .zip(&assessment.edges)
                        .filter(|(j, _)| trace.contains(j))
                    {
                        let answer = &assessment.solve.answer[..assessment.nodes.len()];
                        let mut force = if edge.contact.is_none() {
                            edge.resultant(&assessment.nodes, answer).linear
                        } else {
                            DVec3::ZERO
                        };
                        let (mut closed, mut sliding) = (0, 0);
                        if let Some(contact) = edge.contact_edge() {
                            for (point, weight) in contact.samples() {
                                let (f, c, s) =
                                    contact.contact_force(&assessment.nodes, answer, point, weight);
                                force += f;
                                closed += usize::from(c);
                                sliding += usize::from(s);
                            }
                        }
                        // Resultants in N; site counts describe this iterate, not a settled state.
                        println!(
                            "TRACE,{},{tick},{j},{},{},{},{closed},{sliding},{:.9},{:.9}",
                            instance.id,
                            state.cracked[j],
                            state.separated[j],
                            edge.contact.is_some(),
                            force.dot(edge.normal),
                            (force - edge.normal * force.dot(edge.normal)).length()
                        );
                    }
                }
                stages.push((failures.len(), departed));
                ticks = tick;
                if !assessment.needs_tick() {
                    break;
                }
            }
            println!(
                "PLACEDROW,{},{scale:.9},{initial},{},{},{},{}",
                instance.id,
                state.cracked.iter().filter(|&&v| v).count(),
                state
                    .fate
                    .iter()
                    .filter(|&&f| f == super::super::Fate::Fallen)
                    .count(),
                ticks,
                assessment.needs_tick()
            );
            rows.entry(asset.clone())
                .or_default()
                .push((scale, stages, assessment.needs_tick()));
            if traced {
                // Resolution: bound the largest placement's continuation to 2,048 ticks,
                // without changing its 128-tick row or imposing a physical crack-count oracle.
                for tick in 128_usize..2048 {
                    if !assessment.needs_tick() {
                        break;
                    }
                    let (failures, rounds) =
                        assessment.advance(&structure, &mut state, &[], Vec3::ZERO, STEP, false);
                    let departed = observe_departures(&assessment, &mut state);
                    if !failures.is_empty()
                        || tick.is_power_of_two()
                        || !assessment.needs_tick()
                        || tick == 2047
                    {
                        println!(
                            "CONTINUE,{},{tick},{},{},{:?},{:.9e},{rounds}",
                            instance.id,
                            failures.len(),
                            departed,
                            assessment.solve.progress,
                            observed_relative_residual(&assessment)
                        );
                    }
                }
            }
        }
        for (id, rows) in rows {
            let min_scale = rows.iter().map(|r| r.0).fold(f32::INFINITY, f32::min);
            let max_scale = rows.iter().map(|r| r.0).fold(0.0_f32, f32::max);
            let unsettled = rows.iter().filter(|r| r.2).count();
            println!(
                "PLACED {id}: instances={} scales={min_scale}..{max_scale} unfinished_after_128={unsettled}",
                rows.len()
            );
            for tick in 0..8 {
                let counts: Vec<_> = rows
                    .iter()
                    .map(|r| r.1.get(tick).copied().unwrap_or((0, 0)))
                    .collect();
                println!(
                    "STAGE {id} {tick}: cracks={}..{} fallen={}..{}",
                    counts.iter().map(|c| c.0).min().unwrap(),
                    counts.iter().map(|c| c.0).max().unwrap(),
                    counts.iter().map(|c| c.1).min().unwrap(),
                    counts.iter().map(|c| c.1).max().unwrap()
                );
            }
        }
    }
}
