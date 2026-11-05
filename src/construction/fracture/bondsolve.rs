//! The joint law of the game's environment destruction model (game state only).
//!
//! A joint is a compliant patch over its authored contact polygon between two rigid nodes
//! ([`Edge`]): area moments integrate affine displacement, vertices bound traction, and root
//! plates declare equivalent disks. [`super::parts`] assembles these edges between parts and
//! solves them; the construction design's "Parts decide what stands" owns that model.

use bevy::math::{DMat3, DVec3};

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

    pub(crate) fn add(&mut self, other: Self, scale: f64) {
        self.linear += other.linear * scale;
        self.angular += other.angular * scale;
    }

    fn point(self, arm: DVec3) -> DVec3 {
        self.linear + self.angular.cross(arm)
    }
}

/// One rigid node: a part. The caller fixes buried matter; a free owner has its rigid
/// acceleration removed from the loads, so its nullspace carries no resultant force or moment.
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
    /// Integral of r² over the area, divided by area, m²: its squared polar radius of gyration.
    pub polar: f64,
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
        if let super::Bond::AreaShare {
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
    /// Pair friction of a push-only joint (a rest or a cracked seam); `None` for a bond.
    pub push: Option<f64>,
    /// A slipping push-only joint's saturated friction per newton of its compression `N`; its
    /// tangential stiffness is then zero.
    pub friction: Option<Friction>,
}

/// Coulomb friction of a slipping patch, linear in its compression `N`: the carried wrench gains
/// `force N` at the patch's pressure centre and the couple `twist N` about its normal, so the
/// pressure-centre offset adds `(M⊥ × n) × force` from the patch's elastic bending `M⊥`
/// (DERIVED: a force `force N` at `r = (M⊥ × n) / N`). The term is not symmetric.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Friction {
    /// μ cos φ t, t the unit direction of the patch's slip: carried along it.
    pub force: DVec3,
    /// μ sin φ s ρ n, m: the twisting share, about the patch normal.
    pub twist: DVec3,
}

impl Edge {
    fn force(&self, displacement: DVec3) -> DVec3 {
        let normal = self.normal * self.normal.dot(displacement);
        normal * self.normal_stiffness + (displacement - normal) * self.shear_stiffness
    }

    /// The patch's corners, in the construction frame. A declared circular footprint
    /// gives its rim at Resolution 32 sites, which bound the angular discretization cost.
    pub(crate) fn corners(&self) -> Vec<DVec3> {
        match &self.patch.rim {
            Rim::Polygon(points) => points.iter().map(|&r| self.centre + r).collect(),
            Rim::Circle(radius) => {
                const SITES: usize = 32;
                let axis = if self.normal.x.abs() < 0.5 {
                    DVec3::X
                } else {
                    DVec3::Y
                };
                let u = self.normal.cross(axis).normalize();
                let v = self.normal.cross(u);
                (0..SITES)
                    .map(|i| {
                        let angle = std::f64::consts::TAU * i as f64 / SITES as f64;
                        self.centre + radius * (u * angle.cos() + v * angle.sin())
                    })
                    .collect()
            }
        }
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
        let mut carried = Wrench {
            linear: self.force(delta),
            angular: self.couple(rotation),
        };
        if let Some(friction) = self.friction {
            let compression = -carried.linear.dot(self.normal);
            let bend = carried.angular - self.normal * carried.angular.dot(self.normal);
            carried.linear += friction.force * compression;
            carried.angular +=
                friction.twist * compression + bend.cross(self.normal).cross(friction.force);
        } // A sticking or bonded patch carries through its stiffness alone.
        carried
    }

    /// Recoverable opening and sliding work, J, from the same area integrals.
    pub(crate) fn energy(&self, nodes: &[Node], answer: &[Wrench]) -> (f64, f64) {
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

/// Joint `j` of `structure` as an edge between local nodes `a` (holding the joint's chunk `a`)
/// and `b`. A `push` edge is push-only at pair friction with the bulk compliance; a bond is a
/// fixing's slip slopes where the attachment states them, the bulk compliance otherwise. The
/// bulk bearing beside a fixing is not a second edge: the fixing carries the bond's whole
/// stiffness (ASSUMED, the bonded kernel's choice), and its friction joins its shear capacity.
pub(crate) fn joint_edge(
    structure: &super::Structure,
    j: usize,
    a: usize,
    b: Option<usize>,
    push: bool,
) -> Edge {
    let joint = &structure.joints[j];
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
    if let super::Side::Chunk(b) = joint.b {
        let other = compliance(b);
        normal += other.0;
        shear += other.1;
    }
    let patch = structure.geometry.patch(joint, j);
    let area = f64::from(joint.area);
    let fixing = if push {
        None
    } else {
        joint.seam().and_then(|s| s.fixing)
    };
    Edge {
        a,
        b,
        centre: patch.centre,
        normal: joint_normal(structure, joint),
        area,
        patch,
        push: push.then(|| pair_friction(structure, joint)),
        friction: None,
        normal_stiffness: fixing.map_or(area / normal, |f| area * f64::from(f.normal)),
        shear_stiffness: fixing.map_or(area / shear, |f| area * f64::from(f.shear)),
    }
}

/// The face normal toward chunk `a`, independent of the authored normal's sign. A convex
/// chunk's centroid lies on its own side of each face; a part's centre need not.
pub(crate) fn joint_normal(structure: &super::Structure, joint: &super::Joint) -> DVec3 {
    let direction = joint.normal.as_dvec3().normalize();
    if (structure.chunks[joint.a].centroid - joint.patch().0)
        .as_dvec3()
        .dot(direction)
        >= 0.0
    {
        direction
    } else {
        -direction
    }
}

/// The joint's traction limits: unlimited compression in bearing, tension for intact seams,
/// and cohesion plus Coulomb friction. Bulk crushing is a separate material verdict.
pub(crate) struct JointStrength {
    tensile: f64,
    cohesion: f64,
    friction: f64,
}

impl JointStrength {
    pub(crate) fn of(structure: &super::Structure, joint: &super::Joint, push: bool) -> Self {
        let seam = joint.seam().filter(|_| !push);
        Self {
            tensile: seam.map_or(0.0, |s| f64::from(s.tensile)),
            cohesion: seam.map_or(0.0, |s| f64::from(s.shear.cohesion)),
            friction: pair_friction(structure, joint),
        }
    }

    /// Tension and shear limits, Pa, at the face's compressive stress (Pa).
    pub(crate) fn limits(&self, compression: f64) -> [f64; 2] {
        [self.tensile, self.cohesion + self.friction * compression]
    }

    /// Maximum added force along a unit direction, N. `normal` points toward the moving side;
    /// `preload` is its existing normal force (positive in tension), with no tangential preload.
    /// DERIVED: solve `p + F d_n <= A T` and `F |d_t| <= A C + μ max(-p - F d_n, 0)`.
    pub(crate) fn capacity(&self, area: f64, normal: DVec3, direction: DVec3, preload: f64) -> f64 {
        let [tensile, cohesion] = self.limits(0.0).map(|stress| stress * area);
        if preload > tensile {
            return 0.0;
        }
        let opening = direction.dot(normal);
        let sliding = direction.cross(normal).length();
        let mut capacity = if opening > 0.0 {
            (tensile - preload) / opening
        } else {
            f64::INFINITY
        };
        // A shear-limit crossing belongs to either the open or the compressed branch.
        if sliding > 0.0 {
            let force = cohesion / sliding;
            let normal = preload + force * opening;
            if normal > 0.0 || (normal == 0.0 && opening >= 0.0) {
                capacity = capacity.min(force);
            }
        }
        let slope = sliding + self.friction * opening;
        if slope > 0.0 {
            let force = (cohesion - self.friction * preload) / slope;
            let normal = preload + force * opening;
            if force >= 0.0 && (normal < 0.0 || (normal == 0.0 && sliding > 0.0)) {
                capacity = capacity.min(force);
            }
        }
        capacity
    }
}

/// The friction a joint's two faces hold with; a ground joint reads its chunk's own.
pub(crate) fn pair_friction(structure: &super::Structure, joint: &super::Joint) -> f64 {
    let own = structure.chunks[joint.a].material.friction;
    let other = match joint.b {
        super::Side::Chunk(c) => structure.chunks[c].material.friction,
        super::Side::Ground => own,
    };
    f64::from(crate::substances::pair_friction(own, other))
}

#[cfg(test)]
mod directional_strength_tests {
    use super::*;

    /// The game's environment destruction: an unloaded face can enter bearing, and a bond
    /// lifting out of friction still has its tensile strength. The sweep relies on the
    /// branch immediately beyond a limit, rather than equality at a branch's endpoint.
    #[test]
    fn unloaded_bearing_and_exhausted_friction_follow_the_traction_cone() {
        // Fixture: a rest at friction 0.5, and a bond with 16 N tension and no cohesion.
        let rest = JointStrength {
            tensile: 0.0,
            cohesion: 0.0,
            friction: 0.5,
        };
        assert!(
            rest.capacity(
                1.0,
                DVec3::X,
                (-DVec3::X + DVec3::Y * 0.25).normalize(),
                0.0
            )
            .is_infinite()
        );
        assert_eq!(
            rest.capacity(1.0, DVec3::X, (-DVec3::X + DVec3::Y).normalize(), 0.0),
            0.0
        );
        let bond = JointStrength {
            tensile: 16.0,
            ..rest
        };
        assert_eq!(bond.capacity(1.0, DVec3::X, DVec3::X, -8.0), 24.0);
        assert!(rest.capacity(1.0, DVec3::X, (DVec3::X + DVec3::Y).normalize(), -8.0) > 0.0);
    }
}
