//! OVERMATCH PATCH: the contacts between two bodies, reduced to a bounded set.
//!
//! Each collider pair of a compound body yields its own manifolds, so two bodies touching along a
//! seam of many hulls carry a manifold per hull pair and four points in each. Contacts between
//! the same two bodies are clustered by support normal, and each normal keeps at most
//! [`MAX_POINTS_PER_NORMAL`] points: the deepest and those spanning the widest footprint, each
//! left in its own manifold and collider pair so it keeps its warm start and its attribution to
//! the chunk it touches. A point closing faster than the rest speed is a landing, which a game's
//! contact reader takes per collider, so it always survives.

use core::borrow::BorrowMut;

use bevy::prelude::*;
use smallvec::SmallVec;

use crate::prelude::*;

/// A body whose contacts with another [`ReducibleContacts`] body may be reduced. Every hook-free,
/// constraint-generating pair between two such bodies shares the bounded set; a pair with any
/// other body, static ones included, or with a contact-modification hook keeps every manifold.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReducibleContacts;

/// Resolution: a ceiling on support normals, set high so distinct faces are not traded away. A
/// cap of 4 saved almost nothing (MEASURED, one twelve-house run each: 32.8 against 35.5 points
/// per awake piece-tick). A cluster past it holds only the shallowest points of the pair.
pub const MAX_NORMALS: usize = 12;

/// Resolution: points kept per support normal, the count a convex footprint needs and
/// [`ContactManifold::prune_points`] already keeps per manifold.
pub const MAX_POINTS_PER_NORMAL: usize = 4;

/// Resolution: two normals within this cosine (20 degrees) act as one support direction. Wider
/// merges a wedge's two faces; narrower keeps every facet of a fractured seam separate.
const SAME_NORMAL_COS: Scalar = 0.9397;

/// Resolution: metres, in the length unit. Two depths or reaches within this are the same; a
/// point that carried load last step wins by this much, so a selection does not flip between
/// near-equal points and drop their warm start.
pub const SELECTION_TIE: Scalar = 0.002;

/// One collider pair's manifolds, taken out for reduction with its neighbours of the same two
/// bodies.
#[derive(Clone, Debug)]
pub struct PairManifolds {
    /// True when this pair's bodies are in the opposite order to the group's, so that its
    /// normal points the other way.
    pub reversed: bool,
    /// The manifolds as the narrow phase produced them for this collider pair.
    pub manifolds: Vec<ContactManifold>,
}

struct Candidate {
    /// Position of its manifold in the depth order, for the keep mask.
    manifold: usize,
    point: u32,
    world: Vector,
    depth: Scalar,
    carried: bool,
    normal: usize,
    keep: bool,
}

/// Reduces `pairs`, the touching collider pairs between one pair of bodies, in place. A point
/// closing faster than `rest_speed` always stays; `tie` is [`SELECTION_TIE`] in world length
/// units. A manifold left with no points is removed.
pub fn reduce_body_pair<T: BorrowMut<PairManifolds>>(
    pairs: &mut [T],
    rest_speed: Scalar,
    tie: Scalar,
) {
    if pairs
        .iter()
        .map(|pair| pair.borrow().manifolds.len())
        .sum::<usize>()
        < 2
    {
        return;
    }
    // The deepest manifold seeds the first normal, so a normal dropped past the cap is the
    // shallowest; ties break by pair order, so the result is the same on every run.
    let mut order: SmallVec<[(Scalar, usize, usize); 16]> = SmallVec::new();
    for (p, pair) in pairs.iter().enumerate() {
        for (m, manifold) in pair.borrow().manifolds.iter().enumerate() {
            let depth = manifold
                .points
                .iter()
                .map(|point| point.penetration + carried_bonus(point, tie))
                .fold(Scalar::MIN, Scalar::max);
            order.push((depth, p, m));
        }
    }
    order.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then((a.1, a.2).cmp(&(b.1, b.2))));

    let mut normals: SmallVec<[Vector; MAX_NORMALS]> = SmallVec::new();
    let mut candidates: SmallVec<[Candidate; 32]> = SmallVec::new();
    for (manifold_at, &(_, p, m)) in order.iter().enumerate() {
        let pair: &PairManifolds = pairs[p].borrow();
        let manifold = &pair.manifolds[m];
        let oriented = if pair.reversed {
            -manifold.normal
        } else {
            manifold.normal
        };
        let normal = normals
            .iter()
            .position(|known| known.dot(oriented) >= SAME_NORMAL_COS)
            .or_else(|| {
                (normals.len() < MAX_NORMALS).then(|| {
                    normals.push(oriented);
                    normals.len() - 1
                })
            });
        for (index, point) in manifold.points.iter().enumerate() {
            candidates.push(Candidate {
                manifold: manifold_at,
                point: index as u32,
                world: point.point,
                depth: point.penetration,
                carried: point.warm_start_normal_impulse > 0.0,
                normal: normal.unwrap_or(usize::MAX),
                keep: point.normal_speed < -rest_speed,
            });
        }
    }

    for (index, &normal) in normals.iter().enumerate() {
        select(&mut candidates, index, normal, tie);
    }

    let mut masks: SmallVec<[u32; 16]> = SmallVec::from_elem(0, order.len());
    for candidate in candidates.iter().filter(|candidate| candidate.keep) {
        masks[candidate.manifold] |= 1 << candidate.point;
    }
    for (&(_, p, m), &mask) in order.iter().zip(&masks) {
        let mut bit = 1;
        pairs[p].borrow_mut().manifolds[m].points.retain(|_| {
            let keep = mask & bit != 0;
            bit <<= 1;
            keep
        });
    }
    for pair in pairs.iter_mut() {
        pair.borrow_mut()
            .manifolds
            .retain(|manifold| !manifold.points.is_empty());
    }
}

fn carried_bonus(point: &ContactPoint, bias: Scalar) -> Scalar {
    if point.warm_start_normal_impulse > 0.0 {
        bias
    } else {
        0.0
    }
}

/// Keeps up to [`MAX_POINTS_PER_NORMAL`] of `candidates` on `normal_index`: the deepest, the
/// point farthest from it, then the farthest on each side of the line between those two.
fn select(candidates: &mut [Candidate], normal_index: usize, normal: Vector, tie: Scalar) {
    let pool: SmallVec<[usize; 32]> = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.normal == normal_index && !c.keep)
        .map(|(i, _)| i)
        .collect();
    if pool.len() <= MAX_POINTS_PER_NORMAL {
        pool.iter().for_each(|&i| candidates[i].keep = true);
        return;
    }
    let bonus = |c: &Candidate| if c.carried { tie } else { 0.0 };
    let tangent = |c: &Candidate, from: Vector| {
        let offset = c.world - from;
        offset - normal * offset.dot(normal)
    };
    let best = |skip: &[usize], score: &dyn Fn(&Candidate) -> Option<Scalar>| {
        pool.iter()
            .filter(|i| !skip.contains(i))
            .filter_map(|&i| score(&candidates[i]).map(|value| (i, value)))
            .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
            .map(|(i, _)| i)
    };

    // Among the deepest (within one tie), the point farthest from the footprint's centre, so a
    // flat rest seeds on a corner of the footprint rather than an interior point.
    let centre = pool.iter().map(|&i| candidates[i].world).sum::<Vector>() / pool.len() as Scalar;
    let deepest = pool
        .iter()
        .map(|&i| candidates[i].depth)
        .fold(Scalar::MIN, Scalar::max);
    let first = best(&[], &|c| {
        (c.depth >= deepest - tie).then(|| tangent(c, centre).length() + bonus(c))
    })
    .expect("the deepest point is in its own tie");
    let origin = candidates[first].world;
    let second = best(&[first], &|c| Some(tangent(c, origin).length() + bonus(c)))
        .expect("the pool holds more than one point");
    let axis = tangent(&candidates[second], origin).normalize_or_zero();
    let side = normal.cross(axis);
    let offset = |c: &Candidate| (c.world - origin).dot(side);
    let third = best(&[first, second], &|c| {
        let at = offset(c);
        (at > tie / 2.0).then(|| at + bonus(c))
    });
    let fourth = best(&[first, second], &|c| {
        let at = -offset(c);
        (at > tie / 2.0).then(|| at + bonus(c))
    });
    for kept in [Some(first), Some(second), third, fourth]
        .into_iter()
        .flatten()
    {
        candidates[kept].keep = true;
    }
}
