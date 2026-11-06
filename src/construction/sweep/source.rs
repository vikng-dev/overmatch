//! Query-only occupied leaves of one rigid body; no enclosing hull participates in contact.

use super::*;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(crate) struct Hull {
    pub(crate) hull: Collider,
    pub(crate) position: Vec3,
    pub(crate) rotation: Quat,
}

/// Immutable geometry prepared with a free group's spawn/split, independently of ECS insertion.
#[derive(Component, Clone, Debug)]
#[component(immutable)]
pub(crate) struct SourceShape(pub(crate) Arc<SourceTree>);

#[derive(Debug)]
pub(crate) struct SourceTree {
    hulls: Vec<Hull>,
    nodes: Vec<Node>,
}

impl SourceTree {
    #[cfg(test)]
    pub(crate) fn hulls(&self) -> &[Hull] {
        &self.hulls
    }

    pub(crate) fn new(hulls: Vec<Hull>) -> Self {
        assert!(
            !hulls.is_empty(),
            "a sweep source has occupied contact hulls"
        );
        let bounds: Vec<_> = hulls
            .iter()
            .map(|h| h.hull.aabb(h.position, h.rotation))
            .collect();
        let mut tree = Self {
            hulls,
            nodes: Vec::new(),
        };
        tree.build(&bounds, &mut (0..tree.hulls.len()).collect::<Vec<_>>());
        tree
    }

    fn build(&mut self, bounds: &[ColliderAabb], leaves: &mut [usize]) -> usize {
        let bound = leaves
            .iter()
            .fold(ColliderAabb::INVALID, |a, &i| a.merged(bounds[i]));
        let index = self.nodes.len();
        self.nodes.push(Node {
            min: bound.min,
            max: bound.max,
            children: None,
            chunk: leaves[0],
        });
        if leaves.len() > 1 {
            let axis = bound.size().max_position();
            leaves.sort_unstable_by(|&a, &b| {
                bounds[a].center()[axis]
                    .total_cmp(&bounds[b].center()[axis])
                    .then(a.cmp(&b))
            });
            let (a, b) = leaves.split_at_mut(leaves.len() / 2);
            let a = self.build(bounds, a);
            let b = self.build(bounds, b);
            self.nodes[index].children = Some((a, b));
        }
        index
    }

    pub(crate) fn bounds(&self, motion: RigidMotion) -> ColliderAabb {
        let envelope = self.envelope(0, motion);
        envelope.merged(ColliderAabb {
            min: envelope.min + motion.velocity * motion.dt,
            max: envelope.max + motion.velocity * motion.dt,
        })
    }

    /// Rotation-only envelopes translate linearly with the COM. Slab entry then supplies a
    /// lower time bound, so an inward witness can prune later leaves without losing the patch.
    fn envelope(&self, index: usize, motion: RigidMotion) -> ColliderAabb {
        let node = &self.nodes[index];
        if node.children.is_none() {
            return Motion {
                velocity: Vec3::ZERO,
                ..motion.leaf(&self.hulls[node.chunk])
            }
            .bounds();
        }
        rotation_bounds(
            ColliderAabb {
                min: node.min,
                max: node.max,
            },
            motion.position,
            motion.rotation,
            motion.centre,
            motion.spin * motion.dt,
        )
    }

    pub(super) fn query(&self, motion: RigidMotion) -> QueryTree<'_> {
        let mut envelopes = vec![ColliderAabb::INVALID; self.nodes.len()];
        let mut starts = envelopes.clone();
        let mut radii = vec![0.0_f32; self.nodes.len()];
        for i in (0..self.nodes.len()).rev() {
            if let Some((a, b)) = self.nodes[i].children {
                envelopes[i] = envelopes[a].merged(envelopes[b]);
                starts[i] = starts[a].merged(starts[b]);
                radii[i] = radii[a].max(radii[b]);
            } else {
                envelopes[i] = self.envelope(i, motion);
                let leaf = motion.leaf(&self.hulls[self.nodes[i].chunk]);
                starts[i] = leaf.hull.aabb(leaf.position, leaf.rotation);
                radii[i] = (starts[i].min - motion.centre)
                    .abs()
                    .max((starts[i].max - motion.centre).abs())
                    .length();
            }
        }
        QueryTree {
            tree: self,
            motion,
            envelopes,
            starts,
            radii,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct RigidMotion {
    pub(crate) position: Vec3,
    pub(crate) rotation: Quat,
    pub(crate) velocity: Vec3,
    pub(crate) centre: Vec3,
    pub(crate) spin: Vec3,
    pub(crate) dt: f32,
}

impl RigidMotion {
    fn leaf(self, hull: &Hull) -> Motion<'_> {
        Motion {
            hull: &hull.hull,
            position: self.position + self.rotation * hull.position,
            rotation: self.rotation * hull.rotation,
            velocity: self.velocity,
            centre: self.centre,
            spin: self.spin,
            dt: self.dt,
        }
    }
}

pub(super) struct QueryTree<'a> {
    tree: &'a SourceTree,
    motion: RigidMotion,
    envelopes: Vec<ColliderAabb>,
    starts: Vec<ColliderAabb>,
    radii: Vec<f32>,
}

impl QueryTree<'_> {
    pub(super) fn entry(&self, source: usize, target: &Node, limit: f32) -> Option<f32> {
        let bound = self.envelopes[source];
        let mut enter: f32 = 0.0;
        let mut exit = limit;
        for axis in 0..3 {
            let v = self.motion.velocity[axis];
            let lo = target.min[axis] - bound.max[axis];
            let hi = target.max[axis] - bound.min[axis];
            if v == 0.0 {
                if lo > 0.0 || hi < 0.0 {
                    return None;
                }
            } else {
                let a = lo / v;
                let b = hi / v;
                enter = enter.max(a.min(b));
                exit = exit.min(a.max(b));
                if enter > exit {
                    return None;
                }
            }
        }
        if self.motion.spin == Vec3::ZERO {
            return Some(enter);
        }
        // DERIVED: |v_i| + |omega cross e_i|*R bounds any occupied point's axis speed.
        // Unlike a full-tick envelope, this orders rotation and lets an inward hit prune later leaves.
        let start = self.starts[source];
        let omega = self.motion.spin;
        let speed = self.motion.velocity.abs()
            + Vec3::new(
                omega.y.hypot(omega.z),
                omega.z.hypot(omega.x),
                omega.x.hypot(omega.y),
            ) * self.radii[source];
        for axis in 0..3 {
            // Resolution: f32 contact/pose rounding, consistent with `rotation_bounds`.
            let pad = MOTION_ROUNDING_SCALE * self.radii[source];
            let gap =
                (target.min[axis] - start.max[axis]).max(start.min[axis] - target.max[axis]) - pad;
            if gap > 0.0 {
                if speed[axis] == 0.0 {
                    return None;
                }
                enter = enter.max(gap / speed[axis]);
            }
        }
        (enter <= exit).then_some(enter)
    }

    /// First *inward* leaf, not first geometric touch. Tangent/outward hits cannot prune any
    /// other leaf: a later inward leaf may still occupy this target's patch.
    pub(super) fn inward(
        &self,
        target: &Node,
        hull: &Collider,
        origin: Vec3,
        work: &mut Work,
    ) -> Option<Witness> {
        let mut first: Option<Witness> = None;
        let mut stack = vec![(0.0, 0)];
        while let Some((entry, index)) = stack.pop() {
            let limit = first.map_or(self.motion.dt, |hit| hit.time);
            if entry > limit {
                continue;
            }
            let node = &self.tree.nodes[index];
            work.bounds += 1;
            if let Some((a, b)) = node.children {
                let a = self.entry(a, target, limit).map(|t| (t, a));
                let b = self.entry(b, target, limit).map(|t| (t, b));
                match (a, b) {
                    (Some(a), Some(b)) => {
                        if a.0 <= b.0 {
                            stack.extend([b, a]);
                        } else {
                            stack.extend([a, b]);
                        }
                    }
                    (Some(a), None) | (None, Some(a)) => stack.push(a),
                    (None, None) => {}
                }
                continue;
            }
            if self.entry(index, target, limit).is_none() {
                continue;
            }
            work.casts += 1;
            let motion = self.motion.leaf(&self.tree.hulls[node.chunk]);
            let Some((time, normal, at)) = motion.hit(hull, origin, work) else {
                continue;
            };
            let speed = (motion.velocity
                + motion
                    .spin
                    .cross(at - motion.centre - motion.velocity * time))
            .dot(normal);
            if speed > 0.0 && first.is_none_or(|first| time < first.time) {
                first = Some(Witness {
                    time,
                    normal,
                    at,
                    speed,
                    chunk: target.chunk,
                });
            }
        }
        first
    }
}
