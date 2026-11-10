//! OVERMATCH PATCH: the interior contacts between two bodies, removed. `OVERMATCH_PATCH.md`
//! ("Body-pair contact reduction") owns the account: why, what was measured and what reduction
//! does not preserve. This states what the code holds.
//!
//! Contacts between the same two bodies cluster by support normal; a cluster admits normals within
//! about 20 degrees of its seed. Normal forces along one direction hold any load whose line falls
//! inside the projected convex hull of their points, so an interior point adds no support. Each
//! normal keeps the vertices of two such hulls, each to within [`HULL_TOLERANCE`]: that of the
//! points that bear, and that of the points that carried load last step. A loaded point is
//! therefore removed only when other loaded points enclose it, never for one that only nears
//! contact. A kept point stays in its own manifold and collider pair.
//!
//! A group is left whole for the step when any point lands (closes faster than the rest speed),
//! when it spans more than [`MAX_NORMALS`], or when a point it would remove overlaps deeper than
//! the deepest kept point of its normal by more than [`ReductionStep::overlap_slop`].

use core::borrow::BorrowMut;

use bevy::prelude::*;
use smallvec::{SmallVec, smallvec};

use crate::math::Vector2;
use crate::prelude::*;

/// A body whose contacts with another [`ReducibleContacts`] body may be reduced. Every hook-free,
/// constraint-generating pair between two such non-static bodies shares the reduction; a pair with
/// a static side, an unmarked body or a contact-modification hook keeps every manifold.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReducibleContacts;

/// Resolution: distinct support normals one body pair may reduce across. A pair past it stays
/// whole for the step, so the ceiling bounds the reducer's work and never removes a face.
pub const MAX_NORMALS: usize = 12;

/// Resolution: two normals within this cosine (20 degrees) act as one support direction. Wider
/// merges a wedge's two faces; narrower keeps every facet of a fractured seam separate.
const SAME_NORMAL_COS: Scalar = 0.9397;

/// Resolution: metres, in the length unit. A point bears when its separation, less what its
/// closing speed covers in the step, is no more than this, or when it carried load last step. A
/// point dropped as not bearing can overlap at the end of the step only by what the step's change
/// in relative velocity adds beyond this margin (DERIVED: gravity alone adds `g·dt²/2`, 1.2 mm at
/// 64 Hz, inside it), and it bears on the next step. The margin is about one step's closing at
/// Avian's default rest speed, 0.15 m/s over a 64 Hz step being 2.3 mm.
pub const BEARING_GAP: Scalar = 0.002;

/// Resolution: metres, in the length unit. A hull vertex closer than this to the segment between
/// its kept neighbours is removed, so every point that bears lies within this of the kept outline.
/// A vertex that carried load last step leaves only at half of it, so noise about the limit does
/// not reselect it.
pub const HULL_TOLERANCE: Scalar = 0.01;

/// One collider pair's manifolds, taken out for reduction with its neighbours of the same two
/// bodies.
#[derive(Clone, Debug)]
pub struct PairManifolds {
    /// The pair's colliders, lesser entity first: with its points' features, the identity that
    /// orders a selection.
    pub colliders: (Entity, Entity),
    /// True when this pair's bodies are in the opposite order to the group's, so that its
    /// normal points the other way.
    pub reversed: bool,
    /// The manifolds as the narrow phase produced them for this collider pair.
    pub manifolds: Vec<ContactManifold>,
}

/// The step a reduction runs in, read from the engine's own state.
#[derive(Clone, Copy, Debug)]
pub struct ReductionStep {
    /// The physics step, seconds.
    pub delta_secs: Scalar,
    /// [`PhysicsLengthUnit`], which the reduction's own `Resolution` lengths scale by.
    pub length_unit: Scalar,
    /// A removed point overlapping deeper than this beyond the deepest kept point of its normal
    /// leaves the group whole. The caller passes [`NarrowPhaseConfig::contact_tolerance`] in world
    /// units: the separation inside which the narrow phase reports a contact, borrowed as the
    /// overlap reduction may leave uncorrected.
    pub overlap_slop: Scalar,
}

/// What one or more reductions did: the groups and why some stayed whole, and the warm-start
/// selection counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReductionCounts {
    /// Body-pair groups of at least two manifolds offered for reduction.
    pub groups: u32,
    /// Of those, groups left whole because a point landed.
    pub landing_whole: u32,
    /// Groups left whole because they spanned more than [`MAX_NORMALS`].
    pub ceiling_whole: u32,
    /// Groups left whole because a point they would remove overlapped past the slop.
    pub overlap_whole: u32,
    /// Points holding a matched warm start that reduction removed.
    pub warm_removed: u32,
    /// Points kept with no warm start.
    pub cold_kept: u32,
}

impl core::ops::AddAssign for ReductionCounts {
    fn add_assign(&mut self, other: Self) {
        self.groups += other.groups;
        self.landing_whole += other.landing_whole;
        self.ceiling_whole += other.ceiling_whole;
        self.overlap_whole += other.overlap_whole;
        self.warm_removed += other.warm_removed;
        self.cold_kept += other.cold_kept;
    }
}

struct Candidate {
    /// Pair in collider order, manifold in feature order, both features, then position in the
    /// manifold: what tells two equal points apart the same way on every step, where depth and
    /// solver order change. Where features are unknown or repeat, the tie falls to the order the
    /// narrow phase generated, deterministic for the same geometry but not tied to the contact.
    identity: [u32; 5],
    world: Vector,
    depth: Scalar,
    /// Separation at the end of the step if the point keeps its closing speed.
    gap_ahead: Scalar,
    carried: bool,
    normal: usize,
    keep: bool,
}

/// Reduces `pairs`, the touching collider pairs between one pair of bodies, in place, leaving a
/// group whole for the reasons the module names; `rest_speed` is the landing threshold. A manifold
/// left with no points is removed.
pub fn reduce_body_pair<T: BorrowMut<PairManifolds>>(
    pairs: &mut [T],
    rest_speed: Scalar,
    step: ReductionStep,
) -> ReductionCounts {
    let manifolds = pairs.iter().flat_map(|pair| &pair.borrow().manifolds);
    if manifolds.clone().count() < 2 {
        return ReductionCounts::default();
    }
    let mut counts = ReductionCounts {
        groups: 1,
        ..default()
    };
    if manifolds
        .flat_map(|manifold| &manifold.points)
        .any(|point| point.normal_speed < -rest_speed)
    {
        counts.landing_whole = 1;
        return counts;
    }

    pairs.sort_unstable_by_key(|pair| pair.borrow().colliders);
    // Manifolds in pair order, then by their least features, then as generated: the seeding order.
    let mut order: SmallVec<[(usize, usize, (u32, u32)); 16]> = SmallVec::new();
    for (p, pair) in pairs.iter().enumerate() {
        for (m, manifold) in pair.borrow().manifolds.iter().enumerate() {
            let features = manifold
                .points
                .iter()
                .map(|point| (point.feature_id1.0, point.feature_id2.0))
                .min()
                .expect("a manifold holds a point");
            order.push((p, m, features));
        }
    }
    order.sort_unstable_by_key(|&(p, m, features)| (p, features, m));
    let mut normals: SmallVec<[Vector; MAX_NORMALS]> = SmallVec::new();
    // Per manifold in generated order: its cluster and its rank in its pair's seeding order.
    let mut seeded: SmallVec<[SmallVec<[(usize, u32); 2]>; 16]> = pairs
        .iter()
        .map(|pair| smallvec![(0, 0); pair.borrow().manifolds.len()])
        .collect();
    for (rank, &(p, m, _)) in order.iter().enumerate() {
        let pair: &PairManifolds = pairs[p].borrow();
        let normal = pair.manifolds[m].normal;
        let oriented = if pair.reversed { -normal } else { normal };
        let known = normals
            .iter()
            .position(|known| known.dot(oriented) >= SAME_NORMAL_COS);
        let cluster = match known {
            Some(cluster) => cluster,
            None if normals.len() == MAX_NORMALS => {
                counts.ceiling_whole = 1;
                return counts;
            }
            None => {
                normals.push(oriented);
                normals.len() - 1
            }
        };
        seeded[p][m] = (cluster, rank as u32);
    }

    let mut candidates: SmallVec<[Candidate; 32]> = SmallVec::new();
    for (p, pair) in pairs.iter().enumerate() {
        for (m, manifold) in pair.borrow().manifolds.iter().enumerate() {
            let (normal, rank) = seeded[p][m];
            for (index, point) in manifold.points.iter().enumerate() {
                candidates.push(Candidate {
                    identity: [
                        p as u32,
                        rank,
                        point.feature_id1.0,
                        point.feature_id2.0,
                        index as u32,
                    ],
                    world: point.point,
                    depth: point.penetration,
                    gap_ahead: -point.penetration + point.normal_speed.min(0.0) * step.delta_secs,
                    carried: point.warm_start_normal_impulse > 0.0,
                    normal,
                    keep: false,
                });
            }
        }
    }

    let gap = BEARING_GAP * step.length_unit;
    let tolerance = HULL_TOLERANCE * step.length_unit;
    for (index, &normal) in normals.iter().enumerate() {
        select(&mut candidates, index, normal, gap, tolerance);
    }

    // The deepest kept and removed overlap of each normal.
    let mut deepest: SmallVec<[(Scalar, Scalar); MAX_NORMALS]> =
        smallvec![(Scalar::MIN, Scalar::MIN); normals.len()];
    for candidate in &candidates {
        let (kept, removed) = &mut deepest[candidate.normal];
        let side = if candidate.keep { kept } else { removed };
        *side = side.max(candidate.depth);
    }
    // A normal that keeps nothing has no point that bears, so none that overlaps.
    if deepest
        .iter()
        .any(|&(kept, removed)| kept > Scalar::MIN && removed > kept + step.overlap_slop)
    {
        counts.overlap_whole = 1;
        return counts;
    }

    // Candidates were pushed in this same order.
    let mut keep = candidates.iter().map(|candidate| candidate.keep);
    for pair in pairs.iter_mut() {
        let pair: &mut PairManifolds = pair.borrow_mut();
        for manifold in &mut pair.manifolds {
            manifold
                .points
                .retain(|_| keep.next().expect("one candidate per point"));
        }
        pair.manifolds
            .retain(|manifold| !manifold.points.is_empty());
    }
    let count = |test: fn(&Candidate) -> bool| candidates.iter().filter(|c| test(c)).count() as u32;
    counts.warm_removed = count(|c| !c.keep && c.carried);
    counts.cold_kept = count(|c| c.keep && !c.carried);
    counts
}

/// Marks what `normal_index`'s cluster keeps: the vertices of the projected hull of the points
/// that bear, and those of the points that carried load. The second hull is what stops a point
/// that only nears contact from making a loaded vertex interior: the body would drop onto the
/// newcomer, up to [`BEARING_GAP`] away, with the support it stood on gone.
fn select(
    candidates: &mut [Candidate],
    normal_index: usize,
    normal: Vector,
    gap: Scalar,
    tolerance: Scalar,
) {
    let (u, v) = normal.any_orthonormal_pair();
    let at = |candidate: &Candidate| Vector2::new(candidate.world.dot(u), candidate.world.dot(v));
    let mut bearing: SmallVec<[usize; 32]> = (0..candidates.len())
        .filter(|&i| candidates[i].normal == normal_index)
        .filter(|&i| candidates[i].gap_ahead <= gap || candidates[i].carried)
        .collect();
    bearing.sort_unstable_by(|&a, &b| {
        let (first, second) = (&candidates[a], &candidates[b]);
        let (p, q) = (at(first), at(second));
        p.x.total_cmp(&q.x)
            .then(p.y.total_cmp(&q.y))
            .then(second.carried.cmp(&first.carried))
            .then(first.identity.cmp(&second.identity))
    });
    bearing.dedup_by(|a, b| at(&candidates[*a]) == at(&candidates[*b]));

    for i in kept_vertices(candidates, &bearing, at, tolerance) {
        candidates[i].keep = true;
    }
    let all = bearing.len();
    bearing.retain(|i| candidates[*i].carried);
    if (1..all).contains(&bearing.len()) {
        for i in kept_vertices(candidates, &bearing, at, tolerance) {
            candidates[i].keep = true;
        }
    }
}

/// The vertices of the convex hull of `sorted`, ordered by position with no duplicate, less those
/// within `tolerance` of the outline the rest make.
fn kept_vertices(
    candidates: &[Candidate],
    sorted: &[usize],
    at: impl Fn(&Candidate) -> Vector2,
    tolerance: Scalar,
) -> SmallVec<[usize; 16]> {
    let position = |i: usize| at(&candidates[i]);
    if sorted.len() < 3 {
        return sorted.iter().copied().collect();
    }
    // Andrew's monotone chain, strictly convex: a vertex exactly on an edge is interior.
    let mut hull: SmallVec<[usize; 16]> = SmallVec::new();
    let turn = |hull: &[usize], next: usize| {
        let (o, a) = (
            position(hull[hull.len() - 2]),
            position(hull[hull.len() - 1]),
        );
        (a - o).perp_dot(position(next) - o)
    };
    for &i in sorted {
        while hull.len() >= 2 && turn(&hull, i) <= 0.0 {
            hull.pop();
        }
        hull.push(i);
    }
    let lower = hull.len() + 1;
    for &i in sorted.iter().rev().skip(1) {
        while hull.len() >= lower && turn(&hull, i) <= 0.0 {
            hull.pop();
        }
        hull.push(i);
    }
    hull.pop();
    if hull.len() < 3 {
        return hull;
    }

    // Douglas-Peucker around the closed polygon, from two far-apart vertices. Measuring each
    // vertex against the chord of its kept neighbours, rather than popping near-collinear ones as
    // the chain goes, is what bounds the sum of the removals by one tolerance.
    let bonus = |i: usize| {
        if candidates[i].carried {
            tolerance / 2.0
        } else {
            0.0
        }
    };
    let farthest = |from: Vector2, skip: Option<usize>| {
        (0..hull.len())
            .filter(|&k| Some(k) != skip)
            .map(|k| (k, (position(hull[k]) - from).length() + bonus(hull[k])))
            .max_by(|a, b| {
                a.1.total_cmp(&b.1).then(
                    candidates[hull[b.0]]
                        .identity
                        .cmp(&candidates[hull[a.0]].identity),
                )
            })
            .expect("a hull has at least three vertices")
            .0
    };
    let n = hull.len();
    let centre = hull.iter().map(|&i| position(i)).sum::<Vector2>() / n as Scalar;
    let first = farthest(centre, None);
    let second = farthest(position(hull[first]), Some(first));
    let mut kept: SmallVec<[usize; 16]> = smallvec![hull[first], hull[second]];
    let wrap = |from: usize, to: usize| if to > from { to } else { to + n };
    let mut arcs: SmallVec<[(usize, usize); 16]> =
        smallvec![(first, wrap(first, second)), (second, wrap(second, first))];
    while let Some((from, to)) = arcs.pop() {
        let (a, b) = (position(hull[from % n]), position(hull[to % n]));
        // Distance to the kept segment, not its line: a vertex past an end lies off the outline.
        let span = b - a;
        let off_segment = |at: Vector2| {
            let along = ((at - a).dot(span) / span.length_squared()).clamp(0.0, 1.0);
            (at - a - span * along).length()
        };
        let split = ((from + 1)..to)
            .map(|k| {
                let i = hull[k % n];
                (k, off_segment(position(i)) + bonus(i))
            })
            .max_by(|x, y| {
                x.1.total_cmp(&y.1).then(
                    candidates[hull[y.0 % n]]
                        .identity
                        .cmp(&candidates[hull[x.0 % n]].identity),
                )
            });
        if let Some((k, score)) = split
            && score > tolerance
        {
            kept.push(hull[k % n]);
            arcs.push((from, k));
            arcs.push((k, to));
        }
    }
    kept
}
