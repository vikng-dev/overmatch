//! OVERMATCH PATCH: the interior contacts between two bodies, removed.
//!
//! Each collider pair of a compound body yields its own manifolds, so two bodies touching along a
//! seam of many hulls carry a manifold per hull pair and up to four points in each. Normal forces
//! along one support direction hold any load whose line falls inside the convex hull of their
//! points, projected along that direction, so a point inside the hull adds no support its vertices
//! lack. Contacts between the same two bodies are clustered by support normal; each normal keeps
//! the hull vertices of the points that bear, and its deepest point when that lies clear of them.
//! A kept point stays in its own manifold and collider pair, attributed to the chunk it touches.
//!
//! DERIVED: each normal's support polygon is preserved to within [`HULL_TOLERANCE`]. Clusters span
//! up to 20 degrees, so across a cluster of unequal normals this is an approximation.
//!
//! Three things reduction does not preserve. A removed point's share of the solved impulses goes
//! to the points that remain. A removed point loses its warm start, and one reselected later
//! starts cold; a loaded preference only makes that rarer. A body pair with any point landing is
//! left whole for that step, yet its blows can still move, since another pair sharing either body
//! may be reduced and the solve couples them.

use core::borrow::BorrowMut;

use bevy::prelude::*;
use smallvec::{SmallVec, smallvec};

use crate::math::Vector2;
use crate::prelude::*;

/// A body whose contacts with another [`ReducibleContacts`] body may be reduced. Every hook-free,
/// constraint-generating pair between two such dynamic bodies shares the reduction; a pair with a
/// static side, an unmarked body or a contact-modification hook keeps every manifold.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReducibleContacts;

/// Resolution: distinct support normals one body pair may reduce across. A pair past it stays
/// whole for the step, so the ceiling bounds the reducer's work and never removes a face.
pub const MAX_NORMALS: usize = 12;

/// Resolution: two normals within this cosine (20 degrees) act as one support direction. Wider
/// merges a wedge's two faces; narrower keeps every facet of a fractured seam separate.
const SAME_NORMAL_COS: Scalar = 0.9397;

/// Resolution: metres, in the length unit. A point within this of touching bears; two depths
/// within it are the same depth; a point that carried load last step holds its place by this much,
/// so near-equal points do not trade places and drop their warm starts.
pub const SELECTION_TIE: Scalar = 0.002;

/// Resolution: metres, in the length unit. A hull vertex closer than this to the chord between its
/// kept neighbours is removed, so every point that bears lies within this of the kept polygon.
/// A vertex that carried load last step leaves only at half of it, so noise about the limit does
/// not reselect it.
pub const HULL_TOLERANCE: Scalar = 0.01;

/// One collider pair's manifolds, taken out for reduction with its neighbours of the same two
/// bodies.
#[derive(Clone, Debug)]
pub struct PairManifolds {
    /// The pair's colliders, lesser entity first: the identity that orders a selection.
    pub colliders: (Entity, Entity),
    /// True when this pair's bodies are in the opposite order to the group's, so that its
    /// normal points the other way.
    pub reversed: bool,
    /// The manifolds as the narrow phase produced them for this collider pair.
    pub manifolds: Vec<ContactManifold>,
}

/// How much of a selection changed from the last step's, the cost of reselecting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReductionChurn {
    /// Points holding a warm start that reduction removed.
    pub warm_removed: u32,
    /// Points kept with no warm start.
    pub cold_kept: u32,
}

impl core::ops::AddAssign for ReductionChurn {
    fn add_assign(&mut self, other: Self) {
        self.warm_removed += other.warm_removed;
        self.cold_kept += other.cold_kept;
    }
}

struct Candidate {
    /// Pair in collider order, manifold, both features, then position in the manifold: what tells
    /// two equal points apart the same way on every step. Depth and solver order change; this
    /// does not.
    identity: [u32; 5],
    world: Vector,
    depth: Scalar,
    carried: bool,
    normal: usize,
    keep: bool,
}

/// Reduces `pairs`, the touching collider pairs between one pair of bodies, in place. A group with
/// a point closing faster than `rest_speed` (a landing), or past [`MAX_NORMALS`], is left whole.
/// A manifold left with no points is removed.
pub fn reduce_body_pair<T: BorrowMut<PairManifolds>>(
    pairs: &mut [T],
    rest_speed: Scalar,
    length_unit: Scalar,
) -> ReductionChurn {
    let manifolds = pairs.iter().flat_map(|pair| &pair.borrow().manifolds);
    if manifolds.clone().count() < 2
        || manifolds
            .flat_map(|manifold| &manifold.points)
            .any(|point| point.normal_speed < -rest_speed)
    {
        return ReductionChurn::default();
    }

    pairs.sort_unstable_by_key(|pair| pair.borrow().colliders);
    let mut normals: SmallVec<[Vector; MAX_NORMALS]> = SmallVec::new();
    let mut candidates: SmallVec<[Candidate; 32]> = SmallVec::new();
    for (p, pair) in pairs.iter().enumerate() {
        let pair: &PairManifolds = pair.borrow();
        for (m, manifold) in pair.manifolds.iter().enumerate() {
            let oriented = if pair.reversed {
                -manifold.normal
            } else {
                manifold.normal
            };
            let known = normals
                .iter()
                .position(|known| known.dot(oriented) >= SAME_NORMAL_COS);
            let normal = match known {
                Some(normal) => normal,
                None if normals.len() == MAX_NORMALS => return ReductionChurn::default(),
                None => {
                    normals.push(oriented);
                    normals.len() - 1
                }
            };
            for (index, point) in manifold.points.iter().enumerate() {
                candidates.push(Candidate {
                    identity: [
                        p as u32,
                        m as u32,
                        point.feature_id1.0,
                        point.feature_id2.0,
                        index as u32,
                    ],
                    world: point.point,
                    depth: point.penetration,
                    carried: point.warm_start_normal_impulse > 0.0,
                    normal,
                    keep: false,
                });
            }
        }
    }

    let (tie, tolerance) = (SELECTION_TIE * length_unit, HULL_TOLERANCE * length_unit);
    for (index, &normal) in normals.iter().enumerate() {
        select(&mut candidates, index, normal, tie, tolerance);
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
    ReductionChurn {
        warm_removed: count(|c| !c.keep && c.carried),
        cold_kept: count(|c| c.keep && !c.carried),
    }
}

/// Marks what `normal_index`'s cluster keeps: the vertices of the projected hull of the points
/// that bear, then the deepest point if the hull's vertices are all a tie shallower.
fn select(
    candidates: &mut [Candidate],
    normal_index: usize,
    normal: Vector,
    tie: Scalar,
    tolerance: Scalar,
) {
    let (u, v) = normal.any_orthonormal_pair();
    let at = |candidate: &Candidate| Vector2::new(candidate.world.dot(u), candidate.world.dot(v));
    let cluster: SmallVec<[usize; 32]> = (0..candidates.len())
        .filter(|&i| candidates[i].normal == normal_index)
        .collect();
    let mut bearing: SmallVec<[usize; 32]> = cluster
        .iter()
        .copied()
        .filter(|&i| candidates[i].depth >= -tie || candidates[i].carried)
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

    let score = |c: &Candidate| c.depth + if c.carried { tie } else { 0.0 };
    let deepest = cluster
        .iter()
        .copied()
        .max_by(|&a, &b| {
            let (first, second) = (&candidates[a], &candidates[b]);
            score(first)
                .total_cmp(&score(second))
                .then(second.identity.cmp(&first.identity))
        })
        .expect("a cluster holds its seed's points");
    let held = cluster
        .iter()
        .filter(|&&i| candidates[i].keep)
        .map(|&i| candidates[i].depth)
        .fold(Scalar::MIN, Scalar::max);
    if score(&candidates[deepest]) > held + tie {
        candidates[deepest].keep = true;
    }
}

/// The vertices of the convex hull of `sorted`, ordered by position with no duplicate, less those
/// within `tolerance` of the polygon the rest make.
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
        let chord = (b - a).normalize();
        let split = ((from + 1)..to)
            .map(|k| {
                let i = hull[k % n];
                (k, (position(i) - a).perp_dot(chord).abs() + bonus(i))
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
