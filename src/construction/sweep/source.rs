//! Query-only occupied leaves of one rigid body; no enclosing hull participates in contact.

use super::*;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(crate) struct Hull {
    /// A collider's contact restriction in the source body's frame; absent on ordinary hulls.
    pub(crate) surface: Option<crate::track::stop_contacts::BeltFace>,
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
        let nodes = tree(
            &hulls
                .iter()
                .map(|h| h.hull.aabb(h.position, h.rotation))
                .collect::<Vec<_>>(),
        );
        Self { hulls, nodes }
    }

    /// The body's whole-tick query box: the merged rotation envelopes of its leaves, the same
    /// envelope its traversal reads, swept along its COM travel.
    pub(crate) fn bounds(&self, motion: RigidMotion) -> ColliderAabb {
        let envelope = self.query(motion).envelopes[0];
        envelope.merged(ColliderAabb {
            min: envelope.min + motion.velocity * motion.dt,
            max: envelope.max + motion.velocity * motion.dt,
        })
    }

    /// Leaf `hull`'s rotation-only envelope, which translates linearly with the COM. Slab entry
    /// then supplies a lower time bound, so an inward witness can prune later leaves without
    /// losing the patch.
    fn envelope(&self, hull: usize, motion: RigidMotion) -> ColliderAabb {
        let leaf = motion.leaf(&self.hulls[hull]);
        rotation_bounds(
            leaf.hull.aabb(Vec3::ZERO, Quat::IDENTITY),
            leaf.position,
            leaf.rotation,
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
                envelopes[i] = self.envelope(self.nodes[i].chunk, motion);
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
    /// The cast's one velocity: the COM's velocity at time `lead`, whose straight path reaches
    /// the pose free motion gives the body at the tick's end.
    pub(crate) velocity: Vec3,
    /// What Avian integrates before its contact solve, m/s².
    pub(crate) acceleration: Vec3,
    /// The time into the tick at which the COM moves at `velocity`, s.
    pub(crate) lead: f32,
    pub(crate) centre: Vec3,
    pub(crate) spin: Vec3,
    pub(crate) dt: f32,
}

impl RigidMotion {
    /// The closing speed of a cast hit at cast time `reach`, at a point the spin moves at
    /// `turn`: `None` unless the cast closes along `normal`. The body's own motion, closing,
    /// crosses the cast's gap `d` there at the root `t` of `d = v_n t + ½ a_n t²` it reaches
    /// moving inward, `v_n` the point's closing speed at the tick's start, and closes at
    /// `v_n + a_n t`. A body at rest on matter closes at rest, not at its cast's fall.
    /// DERIVED at equal tick fractions: the continuous chord's lateral error is at most
    /// `|a_perp| dt²/8`; Avian's substep reach adds at most `|a_perp| dt²/(2N)` for `N`
    /// substeps, `a_perp` the acceleration across the cast direction.
    fn closing(self, normal: Vec3, turn: Vec3, reach: f32) -> Option<f32> {
        let a = self.acceleration.dot(normal);
        let v = (self.velocity - self.acceleration * self.lead + turn).dot(normal);
        let cast = v + a * self.lead;
        if cast <= 0.0 {
            return None;
        }
        let gap = cast * reach;
        // DERIVED: at the closing root `v_n + a_n t = √(v_n² + 2 a_n d)`. Resolution: the floor
        // holds f32 rounding where a decelerating body only just reaches the gap.
        let speed = (v * v + 2.0 * a * gap).max(0.0).sqrt();
        // The root in its form free of cancellation for each sign of `v`; a closing cast with
        // `v <= 0` has `a > 0`.
        let time = if v > 0.0 {
            2.0 * gap / (v + speed)
        } else {
            (speed - v) / a
        };
        // Resolution: Avian's `N` substeps carry the body `a dt² / (2N)` past this law by the
        // tick's end, so a gap the cast closes in that span closes at the tick's end.
        Some(if time <= self.dt {
            speed
        } else {
            v + a * self.dt
        })
    }

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

pub(super) enum TargetHit {
    Witness(Witness),
    Unresolved,
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
    /// other leaf: a later inward leaf may still occupy this target's patch. A hit without
    /// geometry conservatively retains this target for the solver instead of judging its cut.
    pub(super) fn inward(
        &self,
        target: &Node,
        hull: &Collider,
        origin: Vec3,
        work: &mut Work,
    ) -> Option<TargetHit> {
        let mut first: Option<Witness> = None;
        let mut stack = vec![(0.0, 0)];
        while let Some((entry, index)) = stack.pop() {
            let limit = first.map_or(self.motion.dt, |hit| hit.reach);
            if entry > limit {
                continue;
            }
            let node = &self.tree.nodes[index];
            work.bounds += 1;
            if let Some((a, b)) = node.children {
                // The earlier entry pops first; `a` wins a tie.
                let mut next = [a, b].map(|i| self.entry(i, target, limit).map(|t| (t, i)));
                if let [Some(a), Some(b)] = next
                    && a.0 > b.0
                {
                    next.swap(0, 1);
                }
                stack.extend(next.into_iter().rev().flatten());
                continue;
            }
            if self.entry(index, target, limit).is_none() {
                continue;
            }
            work.casts += 1;
            let leaf = &self.tree.hulls[node.chunk];
            let motion = self.motion.leaf(leaf);
            let (reach, normal, at) = match motion.hit(hull, origin, work) {
                Some(Hit::Contact { reach, normal, at }) => (reach, normal, at),
                Some(Hit::Unresolved) => return Some(TargetHit::Unresolved),
                None => continue,
            };
            if let Some(surface) = &leaf.surface {
                let rotation =
                    Quat::from_scaled_axis(self.motion.spin * reach) * self.motion.rotation;
                if !surface.accepts_normal(rotation.inverse() * normal) {
                    // An unrepresented surface cannot seed or prune an occupied patch. A
                    // diagonal descent can touch radially before developing lateral contact.
                    // DERIVED: a lateral overlap present at the next tick's start is found by
                    // its initial cast, deferring that check by one tick. SweepContacts keeps
                    // overlapping solver points; recasting here would charge every rejected
                    // leaf to avoid that delay.
                    continue;
                }
            }
            if first.is_some_and(|first| first.reach <= reach) {
                continue;
            }
            let turn = motion
                .spin
                .cross(at - motion.centre - motion.velocity * reach);
            if let Some(speed) = self.motion.closing(normal, turn, reach) {
                first = Some(Witness {
                    reach,
                    normal,
                    at,
                    speed,
                    chunk: target.chunk,
                });
            }
        }
        first.map(TargetHit::Witness)
    }
}
