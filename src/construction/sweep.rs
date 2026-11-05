//! The game's environment destruction: occupied hull motion frees chunks before contact solving.
//! `Geometry` is baked at admission; `swept` queries only overlapping tree leaves. The design's
//! "Bodies in contact" owns the law and its limits. No view asset constructs simulation state.

use avian3d::collision::collider::contact_query::time_of_impact;
use avian3d::prelude::*;
use bevy::math::{DMat3, DVec3};
use bevy::prelude::*;

use std::collections::HashSet;

use super::fracture::{Block, Side, State, Structure};
use crate::contact_law::{Contactor, peak_force, peak_speed};

/// The farthest a chunk hull's vertex may lie from its bounds' centre on any axis, m: ADR-0015's
/// 10 m figure on the support coordinates of the only colliders a sweep casts against, each cast
/// run in its target's frame about that centre. Resolution: parry 0.27's GJK shape cast returns
/// its hit short by an error that grows with those coordinates; ADR-0015 MEASURED 0.25 mm at 5 m
/// and 139–172 mm at 500 m, so error ∝ extent^1.37–1.42 (DERIVED from the two), at most
/// 0.25 mm × 2^1.42 ≈ 0.67 mm at this bound.
pub(crate) const CAST_HALF_EXTENT_M: f32 = 10.0;

/// The game's environment destruction: only standing, exposed law chunks belong to the sweep.
pub(crate) fn covered(structure: &Structure, state: &State, chunk: usize) -> bool {
    state.standing(chunk) && !structure.chunks[chunk].buried
}

#[derive(Debug, Clone)]
pub(crate) struct Geometry {
    /// Each chunk's hull about its bounds' centre, `centres[c]`: the frame its casts run in.
    hulls: Vec<Collider>,
    centres: Vec<Vec3>,
    parts: Vec<Part>,
    incident: Vec<Vec<usize>>,
    edges: Vec<(usize, Option<usize>)>,
    tree: Vec<Node>,
}

impl PartialEq for Geometry {
    fn eq(&self, other: &Self) -> bool {
        self.parts == other.parts
            && self.incident == other.incident
            && self.edges == other.edges
            && self.tree == other.tree
            && self.hulls.len() == other.hulls.len()
            && self.hulls.iter().zip(&other.hulls).all(|(a, b)| {
                let points = |hull: &Collider| {
                    hull.shape()
                        .as_convex_polyhedron()
                        .expect("sweep geometry contains convex chunk hulls")
                        .points()
                        .to_vec()
                };
                points(a) == points(b)
            })
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Node {
    min: Vec3,
    max: Vec3,
    children: Option<(usize, usize)>,
    chunk: usize,
}

#[derive(Default, Debug, Clone, Copy)]
pub(crate) struct Work {
    pub(crate) bounds: usize,
    pub(crate) casts: usize,
    /// Incident references examined for patch membership, including internal edges twice.
    pub(crate) joints: usize,
    pub(crate) queries: usize,
}

impl Geometry {
    /// Bake the sweep's chunk hulls, each about its bounds' centre, refusing a chunk whose hull
    /// reaches past [`CAST_HALF_EXTENT_M`] from it: the load gate's refusal, its caller naming the
    /// file.
    pub(crate) fn new(blocks: &[Block], structure: &Structure) -> Result<Self, String> {
        use avian3d::dynamics::rigid_body::mass_properties::bevy_heavy::ComputeMassProperties3d;
        let mut hulls = Vec::with_capacity(blocks.len());
        let mut centres = Vec::with_capacity(blocks.len());
        let mut parts = Vec::with_capacity(blocks.len());
        for (c, (block, chunk)) in blocks.iter().zip(&structure.chunks).enumerate() {
            let centre = (block.min + block.max) / 2.0;
            let reach = ((block.max - block.min) / 2.0).max_element();
            if reach > CAST_HALF_EXTENT_M {
                return Err(format!(
                    "chunk {c} reaches {reach:.2} m from its bounds' centre, past the \
                     {CAST_HALF_EXTENT_M} m within which a sweep's shape cast holds its accuracy \
                     (ADR-0015): split it into chunks at most {} m across",
                    2.0 * CAST_HALF_EXTENT_M
                ));
            }
            let about = |origin: Vec3| {
                Collider::convex_hull(block.points.iter().map(|p| *p - origin).collect())
                    .expect("admitted chunk hulls stay solid about any origin")
            };
            // Occupied physics mass uses the same centroid-centred hull as the departure path.
            let props = about(chunk.centroid).mass_properties(chunk.material.density);
            parts.push(Part {
                mass: f64::from(props.mass),
                centre: (chunk.centroid + props.center_of_mass).as_dvec3(),
                inertia: props.angular_inertia_tensor().to_mat3().as_dmat3(),
            });
            hulls.push(about(centre));
            centres.push(centre);
        }
        let mut geometry = Self {
            hulls,
            centres,
            parts,
            incident: vec![Vec::new(); blocks.len()],
            tree: Vec::new(),
            edges: structure
                .joints
                .iter()
                .map(|j| {
                    (
                        j.a,
                        match j.b {
                            Side::Chunk(b) => Some(b),
                            Side::Ground => None,
                        },
                    )
                })
                .collect(),
        };
        for (j, joint) in structure.joints.iter().enumerate() {
            geometry.incident[joint.a].push(j);
            if let Side::Chunk(b) = joint.b {
                geometry.incident[b].push(j);
            }
        }
        if !blocks.is_empty() {
            geometry.build(blocks, &mut (0..blocks.len()).collect::<Vec<_>>());
        }
        Ok(geometry)
    }

    fn build(&mut self, blocks: &[Block], chunks: &mut [usize]) -> usize {
        let (min, max) = chunks
            .iter()
            .fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(a, b), &c| {
                (a.min(blocks[c].min), b.max(blocks[c].max))
            });
        let index = self.tree.len();
        self.tree.push(Node {
            min,
            max,
            children: None,
            chunk: chunks[0],
        });
        if chunks.len() > 1 {
            let axis = (max - min).max_position();
            chunks.sort_unstable_by(|&a, &b| {
                (blocks[a].min[axis] + blocks[a].max[axis])
                    .total_cmp(&(blocks[b].min[axis] + blocks[b].max[axis]))
                    .then(a.cmp(&b))
            });
            let (left, right) = chunks.split_at_mut(chunks.len() / 2);
            let a = self.build(blocks, left);
            let b = self.build(blocks, right);
            self.tree[index].children = Some((a, b));
        }
        index
    }

    /// Occupied rigid motion over `dt`, in construction coordinates; collider scale is applied.
    /// Avian's pinned convex cast supplies the contact normal; tangent motion transfers no mass.
    pub(crate) fn swept(&self, structure: &Structure, state: &State, motion: Motion<'_>) -> Sweep {
        let Motion {
            velocity,
            centre,
            spin,
            dt,
            ..
        } = motion;
        assert!(
            dt.is_finite() && dt > 0.0,
            "sweep has a positive physics step"
        );
        let mut result = Sweep {
            incoming: velocity.length(),
            ..default()
        };
        if (velocity == Vec3::ZERO && spin == Vec3::ZERO) || self.tree.is_empty() {
            return result;
        }
        let ColliderAabb { min, max } = motion.bounds();
        let mut stack = vec![0];
        while let Some(index) = stack.pop() {
            let node = &self.tree[index];
            result.work.bounds += 1;
            if !node.min.cmple(max).all() || !min.cmple(node.max).all() {
                continue;
            }
            if let Some((a, b)) = node.children {
                stack.extend([b, a]);
                continue;
            }
            let c = node.chunk;
            if !covered(structure, state, c) {
                continue;
            }
            result.work.casts += 1;
            let hit = motion.hit(&self.hulls[c], self.centres[c], &mut result.work);
            let Some((time, normal, at)) = hit else {
                continue;
            };
            let speed = (velocity + spin.cross(at - centre - velocity * time)).dot(normal);
            if speed <= 0.0 {
                continue;
            }
            result.chunks.push(c);
            let witness = Witness {
                time,
                normal,
                at,
                speed,
                chunk: c,
            };
            if result.witness.is_none_or(|first| time < first.time) {
                result.witness = Some(witness);
            }
        }
        result.chunks.sort_unstable();
        result
    }

    pub(crate) fn part(&self, chunk: usize, pose: &Transform) -> Part {
        let part = self.parts[chunk];
        let rotation = DMat3::from_quat(pose.rotation.as_dquat());
        Part {
            centre: pose.translation.as_dvec3() + rotation * part.centre,
            inertia: rotation * part.inertia * rotation.transpose(),
            ..part
        }
    }

    /// Only edges crossing patch membership resist its common motion. Internal seams carry none.
    pub(crate) fn cut(&self, chunks: &[usize]) -> Vec<usize> {
        assert!(
            chunks.windows(2).all(|pair| pair[0] < pair[1]),
            "swept patch identities are strictly ordered and unique"
        );
        let patch: HashSet<_> = chunks.iter().copied().collect();
        // Each cut edge has exactly one endpoint inside; ordered chunks and authored incident
        // lists therefore visit it once, in an order independent of the hash table's iteration.
        chunks
            .iter()
            .flat_map(|&c| &self.incident[c])
            .copied()
            .filter(|&j| {
                let edge = self.edges[j];
                patch.contains(&edge.0) != edge.1.is_some_and(|b| patch.contains(&b))
            })
            .collect()
    }

    /// Break `sweep`'s cut when the hull's stop on its witness chunk, plus its inward `drive`,
    /// exceeds the cut's capacity. The stop is the elastic impact's peak force
    /// ([`peak_force`]) of the hull's effective contact `mass` on that chunk, its side [`Contactor::RIGID`],
    /// so no physics step enters it.
    pub(crate) fn resolve(
        &self,
        structure: &Structure,
        state: &State,
        sweep: &mut Sweep,
        mass: f32,
        drive: Vec3,
    ) {
        let Some(witness) = sweep.witness else {
            return;
        }; // No occupied inward motion.
        sweep.work.joints += sweep
            .chunks
            .iter()
            .map(|&c| self.incident[c].len())
            .sum::<usize>();
        let capacity = self
            .cut(&sweep.chunks)
            .into_iter()
            .map(|j| {
                let joint = &structure.joints[j];
                if state.cracked[j] || state.separated[j] || !state.live(joint) {
                    return 0.0;
                }
                joint.seam().map_or(0.0, |seam| {
                    let opening = witness.normal.dot(joint.normal).abs();
                    let sliding = witness.normal.cross(joint.normal).length();
                    joint.area * (seam.tensile * opening + seam.shear.cohesion * sliding)
                })
            })
            .sum::<f32>();
        let drive = drive.dot(witness.normal).max(0.0);
        let (mass, patch) = (f64::from(mass), Contactor::chunk(structure, witness.chunk));
        let stop = peak_force(mass, f64::from(witness.speed), patch, Contactor::RIGID) as f32;
        let demand = stop + drive;
        sweep.broken = demand > capacity;
        let left = f64::from((capacity - drive).max(0.0));
        sweep.load = Some(PatchLoad {
            drive,
            demand,
            capacity,
            threshold: peak_speed(mass, left, patch, Contactor::RIGID) as f32,
        });
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Motion<'a> {
    pub(crate) hull: &'a Collider,
    pub(crate) position: Vec3,
    pub(crate) rotation: Quat,
    pub(crate) velocity: Vec3,
    pub(crate) centre: Vec3,
    pub(crate) spin: Vec3,
    pub(crate) dt: f32,
}

impl Motion<'_> {
    /// Conservative occupied-motion bounds; rotation uses the furthest AABB corner about the COM.
    pub(crate) fn bounds(&self) -> ColliderAabb {
        let Self {
            hull,
            position,
            rotation,
            velocity,
            centre,
            spin,
            dt,
        } = *self;
        let aabb = hull.aabb(position, rotation);
        let travel = velocity * dt;
        let (min, max) = if spin == Vec3::ZERO {
            (
                aabb.min.min(aabb.min + travel),
                aabb.max.max(aabb.max + travel),
            )
        } else {
            let radius = (aabb.min - centre)
                .abs()
                .max((aabb.max - centre).abs())
                .length();
            // DERIVED: every point's rotational travel is at most R*min(|omega|*dt, 2),
            // bounded by arc length and diameter. Intersect with the whole rotation envelope.
            let margin = Vec3::splat(radius * (spin.length() * dt).min(2.0));
            (
                (aabb.min.min(aabb.min + travel) - margin)
                    .max(centre.min(centre + travel) - Vec3::splat(radius)),
                (aabb.max.max(aabb.max + travel) + margin)
                    .min(centre.max(centre + travel) + Vec3::splat(radius)),
            )
        };
        ColliderAabb { min, max }
    }

    /// The first hit of this motion on chunk `target`, a hull about `origin`, in construction
    /// coordinates. The casts are parry's GJK shape casts (ADR-0015's defect): they run in the
    /// target's frame about `origin`, so its support coordinates stay within the load gate's
    /// [`CAST_HALF_EXTENT_M`], and only chunk hulls are ever targets, never terrain. DERIVED error
    /// at the largest shipped chunk (fir_1's chunk 35, 5.67 m half-extent, MEASURED): with
    /// [`CAST_HALF_EXTENT_M`]'s scaling, 0.25 mm × (5.67 / 5)^1.37–1.42 ≈ 0.30 mm short.
    fn hit(&self, target: &Collider, origin: Vec3, work: &mut Work) -> Option<(f32, Vec3, Vec3)> {
        let Self {
            hull,
            position,
            rotation,
            velocity,
            centre,
            spin,
            dt,
        } = *self;
        let (position, centre) = (position - origin, centre - origin);
        if spin == Vec3::ZERO {
            work.queries += 1;
            time_of_impact(
                hull,
                position,
                rotation,
                velocity,
                target,
                Vec3::ZERO,
                Quat::IDENTITY,
                Vec3::ZERO,
                dt,
            )
            .expect("construction sweeps require supported collider shapes")
            .map(|hit| (hit.time_of_impact, -hit.normal2, hit.point2 + origin))
        } else {
            use avian3d::parry::{
                math::Pose3,
                query::{NonlinearRigidMotion, cast_shapes_nonlinear},
            };
            let moving = NonlinearRigidMotion::new(
                Pose3::from_parts(position, rotation),
                rotation.inverse() * (centre - position),
                velocity,
                spin,
            );
            work.queries += 1;
            cast_shapes_nonlinear(
                &moving,
                hull.shape_scaled().0.as_ref(),
                &NonlinearRigidMotion::identity(),
                target.shape_scaled().0.as_ref(),
                0.0,
                dt,
                true,
            )
            .expect("construction sweeps require supported collider shapes")
            .map(|hit| {
                let at = moving.position_at_time(hit.time_of_impact);
                work.queries += 1;
                let contact = avian3d::parry::query::contact(
                    &at,
                    hull.shape_scaled().0.as_ref(),
                    &Pose3::IDENTITY,
                    target.shape_scaled().0.as_ref(),
                    f32::MAX,
                )
                .expect("construction sweeps require supported contact shapes")
                .expect("bounded convex sweep shapes have a closest contact at their cast pose");
                (
                    hit.time_of_impact,
                    -contact.normal2,
                    contact.point2 + origin,
                )
            })
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Witness {
    pub(crate) time: f32,
    pub(crate) normal: Vec3,
    pub(crate) at: Vec3,
    pub(crate) speed: f32,
    /// The chunk the hull meets first, whose material and size stop it.
    pub(crate) chunk: usize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PatchLoad {
    pub(crate) drive: f32,
    pub(crate) demand: f32,
    pub(crate) capacity: f32,
    pub(crate) threshold: f32,
}

#[derive(Default, Debug)]
pub(crate) struct Sweep {
    pub(crate) chunks: Vec<usize>,
    pub(crate) incoming: f32,
    pub(crate) witness: Option<Witness>,
    pub(crate) broken: bool,
    pub(crate) load: Option<PatchLoad>,
    pub(crate) work: Work,
}

impl Sweep {
    /// A rigid body's several contact hulls define one occupied patch, with no duplicate mass.
    pub(crate) fn merge(&mut self, other: Self) {
        self.chunks.extend(other.chunks);
        self.chunks.sort_unstable();
        self.chunks.dedup();
        self.incoming = self.incoming.max(other.incoming);
        if let Some(witness) = other.witness
            && self.witness.is_none_or(|first| witness.time < first.time)
        {
            self.witness = Some(witness);
        }
        self.work.bounds += other.work.bounds;
        self.work.casts += other.work.casts;
        self.work.joints += other.work.joints;
        self.work.queries += other.work.queries;
    }
}

/// Occupied mass properties in the frame of an exchange (SI).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Part {
    pub(crate) mass: f64,
    pub(crate) centre: DVec3,
    pub(crate) inertia: DMat3,
}

pub(crate) struct Body {
    pub(crate) centre: DVec3,
    pub(crate) inverse_mass: DVec3,
    pub(crate) inverse_inertia: DMat3,
    pub(crate) velocity: DVec3,
    pub(crate) spin: DVec3,
}

#[derive(Debug)]
pub(crate) struct Exchange {
    pub(crate) velocity: DVec3,
    pub(crate) spin: DVec3,
    pub(crate) impulse: DVec3,
    pub(crate) angular_impulse: DVec3,
    pub(crate) dissipated: f64,
}

/// DERIVED: chunks leave in the source's resulting rigid velocity field. Their spatial mass
/// matrix K and the source's inverse response R give (identity + R K) twist_after = twist_before.
/// Solving this fixed six-coordinate system counts the source once, including off-centre torque.
/// Locked axes have zero inverse response; their hosting constraint takes the matching reaction.
pub(crate) fn exchange(body: &Body, parts: &[Part]) -> Exchange {
    assert!(!parts.is_empty(), "an exchange has occupied recipients");
    let mut mass = 0.0;
    let mut first = DVec3::ZERO;
    let mut inertia = DMat3::ZERO;
    for part in parts {
        assert!(
            part.mass.is_finite() && part.mass > 0.0,
            "a swept chunk has finite occupied mass"
        );
        let r = part.centre - body.centre;
        mass += part.mass;
        first += part.mass * r;
        inertia += part.inertia
            + part.mass
                * (DMat3::IDENTITY * r.length_squared()
                    - DMat3::from_cols(r * r.x, r * r.y, r * r.z));
    }
    let skew = DMat3::from_cols(
        DVec3::new(0.0, first.z, -first.y),
        DVec3::new(-first.z, 0.0, first.x),
        DVec3::new(first.y, -first.x, 0.0),
    );
    let response = |v: DVec3, w: DVec3| {
        let p = mass * v - skew * w;
        let l = skew * v + inertia * w;
        (v + body.inverse_mass * p, w + body.inverse_inertia * l)
    };
    let mut matrix = [[0.0; 7]; 6];
    for column in 0..6 {
        let mut unit = DVec3::ZERO;
        unit[column % 3] = 1.0;
        let (v, w) = if column < 3 {
            response(unit, DVec3::ZERO)
        } else {
            response(DVec3::ZERO, unit)
        };
        for row in 0..3 {
            matrix[row][column] = v[row];
            matrix[row + 3][column] = w[row];
        }
    }
    for row in 0..3 {
        matrix[row][6] = body.velocity[row];
        matrix[row + 3][6] = body.spin[row];
    }
    // Partial pivoting is numerical resolution of a six-coordinate positive mass response.
    for column in 0..6 {
        let pivot = (column..6)
            .max_by(|&a, &b| matrix[a][column].abs().total_cmp(&matrix[b][column].abs()))
            .expect("the fixed exchange matrix has a pivot row");
        matrix.swap(column, pivot);
        let divisor = matrix[column][column];
        assert!(
            divisor.is_finite() && divisor != 0.0,
            "positive occupied inertia has an invertible exchange"
        );
        for value in &mut matrix[column][column..] {
            *value /= divisor;
        }
        let pivot = matrix[column];
        for (row, values) in matrix.iter_mut().enumerate() {
            if row == column {
                continue;
            }
            let factor = values[column];
            for (value, pivot) in values[column..].iter_mut().zip(&pivot[column..]) {
                *value -= factor * pivot;
            }
        }
    }
    let velocity = DVec3::new(matrix[0][6], matrix[1][6], matrix[2][6]);
    let spin = DVec3::new(matrix[3][6], matrix[4][6], matrix[5][6]);
    let impulse = mass * velocity - skew * spin;
    let angular_impulse = skew * velocity + inertia * spin;
    let dissipated = 0.5 * (impulse.dot(body.velocity) + angular_impulse.dot(body.spin));
    assert!(
        velocity.is_finite() && spin.is_finite() && dissipated.is_finite() && dissipated >= 0.0,
        "plastic exchange has finite motion and nonnegative dissipation"
    );
    Exchange {
        velocity,
        spin,
        impulse,
        angular_impulse,
        dissipated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::construction::fracture::{Bond, Chunk, Joint, Material};

    /// Fixture: nine anchored chunks on a plane. The middle row has strong internal seams;
    /// their capacity must disappear from the cut when that row moves together.
    fn wall(strength: f32) -> (Structure, Geometry) {
        let mut material = Material::of(
            crate::substances::SubstanceRegistry::shipped_static(),
            "Brick",
        )
        .unwrap()
        .unwrap();
        material.density = 50.0; // Fixture: 0.2 m3 at 50 kg/m3 gives each chunk its stated 10 kg.
        let mut structure = Structure::default();
        let mut blocks = Vec::new();
        for y in -1..=1 {
            for z in -1..=1 {
                let centre = Vec3::new(0.0, y as f32, z as f32);
                let points = [-0.1, 0.1]
                    .into_iter()
                    .flat_map(|x| {
                        [-0.5, 0.5].into_iter().flat_map(move |dy| {
                            [-0.5, 0.5].map(move |dz| centre + Vec3::new(x, dy, dz))
                        })
                    })
                    .collect::<Vec<_>>();
                blocks.push(Block::hull(points).unwrap());
                let c = structure.chunks.len();
                structure.chunks.push(Chunk {
                    volume: 0.2,
                    mass: 10.0,
                    centroid: centre,
                    material,
                    buried: false,
                });
                let mut seam = material.seam;
                seam.tensile = strength;
                seam.shear.cohesion = strength;
                structure.joints.push(Joint {
                    a: c,
                    b: Side::Ground,
                    area: 1.0,
                    centroid: centre,
                    normal: Vec3::X,
                    bond: Bond::Bonded(seam),
                });
            }
        }
        for (a, b) in [(3, 4), (4, 5)] {
            let mut seam = material.seam;
            seam.tensile = 1e6; // Fixture: internal seams dominate every anchor if counted wrongly.
            seam.shear.cohesion = 1e6;
            structure.joints.push(Joint {
                a,
                b: Side::Chunk(b),
                area: 1.0,
                centroid: (structure.chunks[a].centroid + structure.chunks[b].centroid) / 2.0,
                normal: Vec3::Z,
                bond: Bond::Bonded(seam),
            });
        }
        let geometry = Geometry::new(&blocks, &structure).expect("the fixture's chunks are small");
        (structure, geometry)
    }

    /// The game's environment destruction: the hole is occupied swept geometry, independent of
    /// contact manifold points. A centre-ray or AABB-only approximation breaks the oblique case.
    #[test]
    fn a_blocks_patch_is_exactly_the_chunks_intersecting_its_swept_hull() {
        let (structure, geometry) = wall(1.0);
        let state = State::new(&structure);
        let hull = Collider::cuboid(0.4, 0.4, 0.4);
        for (rotation, expected) in [
            (Quat::IDENTITY, vec![4]),
            (Quat::from_rotation_x(std::f32::consts::FRAC_PI_4), vec![4]),
        ] {
            let swept = geometry.swept(
                &structure,
                &state,
                Motion {
                    hull: &hull,
                    position: Vec3::NEG_X,
                    rotation,
                    centre: Vec3::NEG_X,
                    spin: Vec3::ZERO,
                    velocity: Vec3::X * 2.0,
                    dt: 1.0,
                },
            );
            assert_eq!(swept.chunks, expected);
        }
        let diagonal = Collider::cuboid(0.4, 0.2, 2.4);
        let swept = geometry.swept(
            &structure,
            &state,
            Motion {
                hull: &diagonal,
                position: Vec3::NEG_X,
                rotation: Quat::from_rotation_x(std::f32::consts::FRAC_PI_4),
                velocity: Vec3::X * 2.0,
                centre: Vec3::NEG_X,
                spin: Vec3::ZERO,
                dt: 1.0,
            },
        );
        assert_eq!(swept.chunks, vec![1, 2, 3, 4, 5, 6, 7]);
        let broad = Collider::cuboid(0.4, 1.4, 1.4);
        let swept = geometry.swept(
            &structure,
            &state,
            Motion {
                hull: &broad,
                position: Vec3::NEG_X,
                rotation: Quat::IDENTITY,
                centre: Vec3::NEG_X,
                spin: Vec3::ZERO,
                velocity: Vec3::X * 2.0,
                dt: 1.0,
            },
        );
        assert_eq!(swept.chunks, (0..9).collect::<Vec<_>>());
    }

    /// The game's environment destruction: rotating occupied matter sweeps even with a
    /// stationary COM. The contact witness, rather than the chunk centre, owns normal speed.
    #[test]
    fn rotation_sweeps_the_hull_without_translation() {
        let (structure, geometry) = wall(1.0);
        let hull = Collider::cuboid(0.2, 4.0, 0.4);
        let swept = geometry.swept(
            &structure,
            &State::new(&structure),
            Motion {
                hull: &hull,
                position: Vec3::NEG_X,
                rotation: Quat::IDENTITY,
                velocity: Vec3::ZERO,
                centre: Vec3::NEG_X,
                spin: Vec3::Z * std::f32::consts::FRAC_PI_2,
                dt: 1.0,
            },
        );
        assert_eq!(swept.chunks, vec![1, 4]);
    }

    /// The game's environment destruction: bounds must contain the complete occupied path,
    /// while vanishing spin must not admit distant chunks. The server's local query cost relies on both.
    #[test]
    fn angular_travel_bounds_contain_the_path_and_shrink_with_spin() {
        // Fixture: offset COM, rotated long hull, translation and angular travel across full turns.
        let hull = Collider::cuboid(0.2, 4.0, 0.4);
        let motion = Motion {
            hull: &hull,
            position: Vec3::new(-1.0, 0.2, 0.3),
            rotation: Quat::from_rotation_y(0.3),
            centre: Vec3::new(0.1, 0.8, -0.2),
            velocity: Vec3::new(1.0, -0.3, 0.7),
            spin: Vec3::ZERO,
            dt: 0.5,
        };
        let linear = motion.bounds();
        let radius = (linear.max - linear.min).length();
        // Resolution: f32 pose composition and AABB rounding, scaled to the fixture's extent.
        let tolerance = 64.0 * f32::EPSILON * radius;
        let axis = Vec3::new(0.2, 1.0, -0.3).normalize();
        for speed in [0.0, f32::EPSILON, 0.01, 1.0, 6.0, 20.0] {
            let motion = Motion {
                spin: axis * speed,
                ..motion
            };
            let bounds = motion.bounds();
            for step in 0..=128 {
                let time = motion.dt * step as f32 / 128.0;
                let turn = Quat::from_scaled_axis(motion.spin * time);
                let position = motion.centre
                    + motion.velocity * time
                    + turn * (motion.position - motion.centre);
                let at = hull.aabb(position, turn * motion.rotation);
                assert!(
                    at.min.cmpge(bounds.min - tolerance).all()
                        && at.max.cmple(bounds.max + tolerance).all(),
                    "speed={speed}, time={time}"
                );
            }
            if speed == f32::EPSILON {
                let expansion = (bounds.min - linear.min)
                    .abs()
                    .max((bounds.max - linear.max).abs());
                assert!(
                    expansion.cmple(Vec3::splat(tolerance)).all(),
                    "a negligible angular path has only rounding-scale bound expansion"
                );
            }
        }
    }

    fn driven_patch(
        structure: &Structure,
        geometry: &Geometry,
        speed: f32,
        mass: f32,
        drive: f32,
    ) -> Sweep {
        // Fixture: middle row, just outside the wall; normal motion crosses the same three chunks.
        // A one-metre travel window reaches the wall at any speed; the stop reads no step.
        let hull = Collider::cuboid(0.4, 0.4, 2.4);
        let mut swept = geometry.swept(
            structure,
            &State::new(structure),
            Motion {
                hull: &hull,
                position: Vec3::X * -0.31,
                rotation: Quat::IDENTITY,
                centre: Vec3::X * -0.31,
                spin: Vec3::ZERO,
                velocity: Vec3::X * speed,
                dt: 1.0 / speed,
            },
        );
        // Fixture: the planar wall's analytical normal isolates the cut threshold from convex
        // cast normal roundoff; the occupancy and production composition tests retain that query.
        let witness = swept
            .witness
            .as_mut()
            .expect("the driven fixture reaches its wall");
        witness.normal = Vec3::X;
        witness.speed = speed;
        geometry.resolve(
            structure,
            &State::new(structure),
            &mut swept,
            mass,
            Vec3::X * drive,
        );
        swept
    }

    /// The game's environment destruction: occupancy chooses the hole and only boundary joints
    /// resist it. Server carriers rely on their combined plastic exchange preserving momentum.
    #[test]
    fn a_fast_block_frees_its_whole_patch_and_balances_momentum() {
        let (structure, geometry) = wall(16.0);
        let patch = driven_patch(&structure, &geometry, 2.0, 100.0, 0.0);
        assert_eq!(patch.chunks, [3, 4, 5]);
        assert!(
            patch.broken,
            "internal seams cannot hold their own moving patch in place"
        );
        assert_eq!(geometry.cut(&patch.chunks), [3, 4, 5]);
        let body = Body {
            centre: DVec3::X * -0.31,
            inverse_mass: DVec3::splat(1.0 / 100.0),
            inverse_inertia: DMat3::IDENTITY,
            velocity: DVec3::X * 2.0,
            spin: DVec3::ZERO,
        };
        let parts: Vec<_> = patch
            .chunks
            .iter()
            .map(|&c| geometry.part(c, &Transform::IDENTITY))
            .collect();
        let result = exchange(&body, &parts);
        let taken: DVec3 = parts
            .iter()
            .map(|p| p.mass * (result.velocity + result.spin.cross(p.centre - body.centre)))
            .sum();
        // Resolution: f64 roundoff in the fixed-coordinate plastic solve.
        assert!(((body.velocity - result.velocity) * 100.0 - taken).length() < 1e-10);
        println!(
            "MEASURED block: patch={:?}, capacity={}, source speed {} -> {}, momentum={taken:?}",
            patch.chunks,
            patch.load.unwrap().capacity,
            body.velocity.x,
            result.velocity.x
        );
    }

    /// The game's environment destruction: a hull's stopping mass, the chunk it meets and its
    /// inward drive govern the threshold through the impact's peak force. Counting chunk mass or
    /// internal seams, losing either seam mode, or a stop read off a step breaks it.
    #[test]
    fn crawl_drive_and_stronger_material_follow_the_cut_threshold() {
        // Fixture: three 16 N anchors and a 128 kg hull. The middle row's chunks share one
        // material and volume, so whichever the witness names stops the hull alike.
        let (structure, geometry) = wall(16.0);
        let speed_at = |structure: &Structure, force: f64| {
            peak_speed(
                128.0,
                force,
                Contactor::chunk(structure, 4),
                Contactor::RIGID,
            ) as f32
        };
        let bracket = |structure: &Structure, geometry: &Geometry, speed: f32, drive: f32| {
            [(0.9999, false), (1.0001, true)].map(|(scale, broken)| {
                let patch = driven_patch(structure, geometry, speed * scale, 128.0, drive);
                assert_eq!(patch.chunks, [3, 4, 5]);
                assert_eq!(patch.broken, broken, "speed={}", speed * scale);
                patch.load.unwrap().threshold
            })
        };
        // DERIVED: the speed whose peak force is the cut's 48 N.
        let base = speed_at(&structure, 48.0);
        let reported = bracket(&structure, &geometry, base, 0.0);
        // Resolution: one f64-to-f32 rounding of the same inverse.
        assert!(reported.iter().all(|&t| (t / base - 1.0).abs() < 1e-6));
        // DERIVED: the peak force grows as v^(6/5), so doubling both seam modes raises the
        // threshold by 2^(5/6) at unchanged mass.
        let (stronger, stronger_geometry) = wall(32.0);
        let strong = speed_at(&stronger, 96.0);
        assert!((strong / base / 2f32.powf(5.0 / 6.0) - 1.0).abs() < 1e-5);
        bracket(&stronger, &stronger_geometry, strong, 0.0);
        // Fixture: an 8 N drive leaves 40 N for the stop.
        let driven = speed_at(&structure, 40.0);
        bracket(&structure, &geometry, driven, 8.0);
        println!(
            "DERIVED and MEASURED block thresholds: base={base}, double-strength={strong}, driven={driven} m/s"
        );
    }

    /// The game's environment destruction: the load gate refuses a chunk reaching past the cast
    /// bound, naming it, so no sweep casts against a collider whose extent parry's shape cast
    /// misjudges (ADR-0015). Dropping the check lets the stretched chunk through.
    #[test]
    fn the_load_gate_refuses_a_chunk_past_the_cast_bound() {
        let material = Material::of(
            crate::substances::SubstanceRegistry::shipped_static(),
            "Brick",
        )
        .unwrap()
        .unwrap();
        // Fixture: one block reaching twice the bound along x from its centroid.
        let reach = 2.0 * CAST_HALF_EXTENT_M;
        let points = [-reach, reach]
            .into_iter()
            .flat_map(|x| {
                [-0.5, 0.5]
                    .into_iter()
                    .flat_map(move |y| [-0.5, 0.5].map(move |z| Vec3::new(x, y, z)))
            })
            .collect::<Vec<_>>();
        let mut structure = Structure::default();
        structure.chunks.push(Chunk {
            volume: 2.0 * reach,
            mass: 1.0,
            centroid: Vec3::ZERO,
            material,
            buried: false,
        });
        let refusal = Geometry::new(&[Block::hull(points).unwrap()], &structure).unwrap_err();
        assert!(refusal.contains("chunk 0"), "{refusal}");
        // MEASURED: the shipped constructions' largest chunk half-extent.
        let library = crate::construction::ConstructionLibrary::load(
            &crate::map::tests::shipped_assets(),
            ["house_1", "fir_1"],
        );
        for id in ["house_1", "fir_1"] {
            let kind = library.get(id).unwrap();
            let farthest = kind
                .layout
                .blocks
                .iter()
                .map(|block| ((block.max - block.min) / 2.0).max_element())
                .fold(0.0, f32::max);
            println!("MEASURED {id}: largest chunk half-extent {farthest} m");
        }
    }

    /// The game's environment destruction: carriers and source share one plastic velocity.
    /// Independent launches using the whole source momentum would violate this mass account.
    #[test]
    fn the_hull_loses_exactly_the_momentum_the_freed_chunks_take() {
        // Fixture: body 100 kg, three 10 kg chunks, velocity (2,0,3) m/s.
        let mass = 100.0;
        let freed = [10.0, 10.0, 10.0];
        let before = Vec3::new(2.0, 0.0, 3.0);
        let source = Body {
            centre: DVec3::ZERO,
            inverse_mass: DVec3::splat(1.0 / mass),
            inverse_inertia: DMat3::IDENTITY,
            velocity: before.as_dvec3(),
            spin: DVec3::ZERO,
        };
        let parts: Vec<_> = freed
            .iter()
            .map(|&mass| Part {
                mass,
                centre: DVec3::ZERO,
                inertia: DMat3::IDENTITY,
            })
            .collect();
        let result = exchange(&source, &parts);
        let after = result.velocity;
        let taken: DVec3 = parts.iter().map(|p| after * p.mass).sum();
        assert!(((source.velocity - after) * mass - taken).length() < 1e-10);
        let initial = 0.5 * mass * source.velocity.length_squared();
        let final_energy =
            0.5 * (mass + parts.iter().map(|p| p.mass).sum::<f64>()) * after.length_squared();
        assert!((initial - final_energy - result.dissipated).abs() < 1e-10);
    }
}

#[cfg(test)]
mod rigid_exchange_tests {
    use super::*;

    /// The game's environment destruction: an off-centre breach transfers orbital and spin
    /// momentum as well as translation. Server bodies rely on the full occupied inertia tensor.
    #[test]
    fn an_off_centre_sweep_balances_both_momenta_and_energy() {
        // Fixture masses, offsets, inertias and twist in SI.
        let mass = 100.0;
        let inertia = DMat3::from_diagonal(DVec3::new(20.0, 30.0, 40.0));
        let source = Body {
            centre: DVec3::ZERO,
            inverse_mass: DVec3::splat(1.0 / mass),
            inverse_inertia: inertia.inverse(),
            velocity: DVec3::new(2.0, 0.0, 3.0),
            spin: DVec3::new(0.1, -0.2, 0.3),
        };
        let parts = [
            Part {
                mass: 10.0,
                centre: DVec3::new(2.0, 0.5, -1.0),
                inertia: DMat3::IDENTITY,
            },
            Part {
                mass: 15.0,
                centre: DVec3::new(-0.5, 1.0, 1.0),
                inertia: DMat3::IDENTITY * 2.0,
            },
        ];
        let result = exchange(&source, &parts);
        let mut momentum = DVec3::ZERO;
        let mut angular = DVec3::ZERO;
        let mut kinetic = 0.0;
        for part in parts {
            let r = part.centre - source.centre;
            let velocity = result.velocity + result.spin.cross(r);
            let p = part.mass * velocity;
            momentum += p;
            angular += r.cross(p) + part.inertia * result.spin;
            kinetic += 0.5
                * (part.mass * velocity.length_squared()
                    + result.spin.dot(part.inertia * result.spin));
        }
        assert!(((source.velocity - result.velocity) * mass - momentum).length() < 1e-10);
        assert!((inertia * (source.spin - result.spin) - angular).length() < 1e-10);
        let initial = 0.5
            * (mass * source.velocity.length_squared() + source.spin.dot(inertia * source.spin));
        let final_source = 0.5
            * (mass * result.velocity.length_squared() + result.spin.dot(inertia * result.spin));
        assert!((initial - final_source - kinetic - result.dissipated).abs() < 1e-10);
    }
}
