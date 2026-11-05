//! The game's environment destruction: occupied hull motion frees chunks before contact solving.
//! `Geometry` is baked at admission; `swept` queries only overlapping tree leaves. The design's
//! "Bodies in contact" owns the law and its limits. No view asset constructs simulation state.

use avian3d::collision::collider::contact_query::time_of_impact;
use avian3d::prelude::*;
use bevy::math::{DMat3, DVec3};
use bevy::prelude::*;

use super::fracture::{Block, Side, State, Structure};

#[derive(Debug, Clone)]
pub(crate) struct Geometry {
    hulls: Vec<Collider>,
    parts: Vec<Part>,
    incident: Vec<Vec<usize>>,
    tree: Vec<Node>,
}

impl PartialEq for Geometry {
    fn eq(&self, other: &Self) -> bool {
        self.parts == other.parts
            && self.incident == other.incident
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
    pub(crate) joints: usize,
    pub(crate) queries: usize,
}

impl Geometry {
    pub(crate) fn new(blocks: &[Block], structure: &Structure) -> Self {
        use avian3d::dynamics::rigid_body::mass_properties::bevy_heavy::ComputeMassProperties3d;
        let hulls: Vec<_> = blocks
            .iter()
            .map(|b| {
                Collider::convex_hull(b.points.clone())
                    .expect("admitted chunks have solid convex hulls")
            })
            .collect();
        let parts = blocks
            .iter()
            .zip(&structure.chunks)
            .map(|(block, chunk)| {
                // Occupied physics mass uses the same centred hull as the departure path.
                let hull = Collider::convex_hull(
                    block.points.iter().map(|p| *p - chunk.centroid).collect(),
                )
                .expect("admitted chunk hulls remain solid about their centroids");
                let props = hull.mass_properties(chunk.material.density);
                Part {
                    mass: f64::from(props.mass),
                    centre: (chunk.centroid + props.center_of_mass).as_dvec3(),
                    inertia: props.angular_inertia_tensor().to_mat3().as_dmat3(),
                }
            })
            .collect();
        let mut geometry = Self {
            hulls,
            parts,
            incident: vec![Vec::new(); blocks.len()],
            tree: Vec::new(),
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
        geometry
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

    /// Translation over `dt`, in construction coordinates. Collider scale is already applied.
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
            if !state.standing(c) || structure.chunks[c].buried {
                continue;
            }
            result.work.casts += 1;
            let hit = motion.hit(&self.hulls[c], &mut result.work);
            let Some((time, normal, at)) = hit else {
                continue;
            };
            let speed = (velocity + spin.cross(at - centre - velocity * time)).dot(normal);
            if speed <= 0.0 {
                continue;
            }
            result.contacts += 1;
            let direction = normal;
            let capacity: f32 = self.incident[c]
                .iter()
                .map(|&j| {
                    result.work.joints += 1;
                    let joint = &structure.joints[j];
                    if state.cracked[j] || state.separated[j] || !state.live(joint) {
                        return 0.0;
                    }
                    joint.seam().map_or(0.0, |seam| {
                        let opening = direction.dot(joint.normal).abs();
                        let sliding = direction.cross(joint.normal).length();
                        joint.area * (seam.tensile * opening + seam.shear.cohesion * sliding)
                    })
                })
                .sum();
            let force = self.parts[c].mass as f32 * speed / dt;
            result.samples.push((c, speed, force, capacity));
            if force > capacity {
                result.chunks.push(c);
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

    pub(crate) fn joints(&self, chunk: usize) -> &[usize] {
        &self.incident[chunk]
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
            (
                centre.min(centre + travel) - Vec3::splat(radius),
                centre.max(centre + travel) + Vec3::splat(radius),
            )
        };
        ColliderAabb { min, max }
    }

    fn hit(&self, target: &Collider, work: &mut Work) -> Option<(f32, Vec3, Vec3)> {
        let Self {
            hull,
            position,
            rotation,
            velocity,
            centre,
            spin,
            dt,
        } = *self;
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
            .map(|hit| (hit.time_of_impact, -hit.normal2, hit.point2))
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
                (hit.time_of_impact, -contact.normal2, contact.point2)
            })
        }
    }
}

#[derive(Default, Debug)]
pub(crate) struct Sweep {
    pub(crate) chunks: Vec<usize>,
    pub(crate) contacts: usize,
    pub(crate) incoming: f32,
    pub(crate) samples: Vec<(usize, f32, f32, f32)>,
    pub(crate) work: Work,
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

    /// Fixture: nine unit-face chunks on a plane, independent equal-capacity anchors. Rows with
    /// the same geometry can carry different authored strengths without any special case in a law.
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
        let geometry = Geometry::new(&blocks, &structure);
        (structure, geometry)
    }

    /// The game's environment destruction: the hole is occupied swept geometry, independent of
    /// contact manifold points. A centre-ray or AABB-only approximation breaks the oblique case.
    #[test]
    fn a_block_frees_exactly_the_chunks_intersecting_its_swept_hull() {
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

    /// The game's environment destruction: demand is m*delta-v/dt against intact capacity, not
    /// a speed threshold or a contact impulse cap. Presses that cannot free matter stay for parts.
    #[test]
    fn slow_motion_and_stronger_joints_hold_by_the_same_force_comparison() {
        let (mut structure, geometry) = wall(10.0);
        let hull = Collider::cuboid(0.4, 0.4, 0.4);
        let sweep = |structure: &Structure, speed| {
            geometry.swept(
                structure,
                &State::new(structure),
                Motion {
                    hull: &hull,
                    position: Vec3::X * -0.31,
                    rotation: Quat::IDENTITY,
                    centre: Vec3::X * -0.31,
                    spin: Vec3::ZERO,
                    velocity: Vec3::X * speed,
                    dt: 1.0,
                },
            )
        };
        // Fixture: 10 kg * 0.1 m/s / 1 s = 1 N; fast is 100 N, capacity 10 N.
        assert!(sweep(&structure, 0.1).chunks.is_empty());
        assert_eq!(sweep(&structure, 10.0).chunks, vec![4]);
        if let Bond::Bonded(ref mut seam) = structure.joints[4].bond {
            seam.tensile = 1000.0;
        }
        assert!(sweep(&structure, 10.0).chunks.is_empty());
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
