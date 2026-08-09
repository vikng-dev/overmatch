//! The §13.6 embedding certificate: no shell passes through itself.
//!
//! Closure and outward winding say a shell has an inside; EMBEDDING says the ray that enters it
//! leaves it. Without this the walk's alternation law is the only thing standing between a
//! self-intersecting plate and a free crossing, and alternation is a one-dimensional witness: a
//! ray whose crossings happen to alternate resolves, and a fan whose claims disagree about
//! direction is a tangent that carries no evidence at all. So embedding is certified over the
//! SURFACE, at the bake, once — never inferred per ray.
//!
//! # The law
//!
//! For every unordered pair of triangles of one shell whose closed AABBs overlap, with `I` the
//! exact closed-set intersection and `F` the feature the two DECLARE they share (their common
//! welded ids: none, one vertex, one edge — three is the same face twice and is refused):
//!
//! > `I ⊆ F`
//!
//! Both triangles contain `F`, so in practice the condition is `I = F`. No adjacent pair is
//! blanket-exempt: the legal exemption is the declared feature itself, and an intersection that
//! extends one millimetre or one ULP past it is a self-intersection.
//!
//! # No epsilon
//!
//! Every sign here is exact. The float path decides only where its own proven roundoff interval
//! excludes zero ([`ORIENT3D_SLACK`]); everything else escalates to
//! [`crate::exact`], where a certified coordinate is an integer multiple of `2^-87` and a
//! determinant is an integer. Coplanarity, collinearity, feature contact and tangency are
//! exact-zero and nothing else.
//!
//! # Both branches end on a line
//!
//! * **Planes that meet in a line.** `A ∩ B = (A ∩ L) ∩ (B ∩ L)`, and each side is the closed
//!   interval that triangle occupies on `L`.
//! * **One plane.** Projected onto a coordinate plane, the two triangles' interiors are disjoint
//!   iff some edge line of one weakly separates them (the separating-axis theorem). No such line
//!   is a positive-area coplanar overlap and is refused; the line that does separate CONTAINS the
//!   intersection, so the same interval comparison finishes the job.
//!
//! The parameter along that line is a coordinate the line's direction moves along, so it orders
//! points on it exactly, and the endpoints are ratios of exact integers.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::exact::{Int, Ratio};

/// The power of two a [`super::CERTIFIED_RANGE`] coordinate is an integer multiple of: the
/// smallest certified magnitude is `2^-64`, whose `f32` significand ends at `2^-87`.
const SHIFT: i32 = 87;

/// Bound on the f64 evaluation of [`orient3d`], as a multiple of its permanent.
///
/// The differences, the six products and the five sums together lose at most `8.3 · 2^-53`
/// permanents; this is `16 · 2^-53`. `f64::MIN_POSITIVE` covers gradual underflow, where the
/// relative model stops holding — the products of two subnormal-scale coordinates round by
/// `2^-1075` a piece, eleven orders below it.
///
/// A bound on the rounding, NOT a tolerance: the exact answer is always inside it, so a sign
/// decided outside it is the exact sign, and a decision inside it is retaken in [`crate::exact`].
const ORIENT3D_SLACK: f64 = 8.0 * f64::EPSILON;

/// One triangle, with the welded ids it names its corners by and the closed AABB the broad phase
/// sorts on.
struct Face {
    /// Index in the primitive's own triangle order — what a defect is reported by.
    index: usize,
    corners: [u32; 3],
    low: [f32; 3],
    high: [f32; 3],
}

/// Certify that no shell passes through itself.
///
/// `triangles` are welded corners in mesh order, `shells` the dense shell id of each, and
/// `vertices` the canonical position of each welded id — the manifold gate's own three products,
/// so this measures exactly the surface the gate proved closed.
pub(super) fn certify_embedding(
    triangles: &[[u32; 3]],
    shells: &[u32],
    vertices: &[[f32; 3]],
) -> Result<(), String> {
    let mut by_shell: BTreeMap<u32, Vec<Face>> = BTreeMap::new();
    for (index, (corners, &shell)) in triangles.iter().zip(shells).enumerate() {
        let position = corners.map(|corner| vertices[corner as usize]);
        // A triangle whose three distinct welded corners are collinear has no plane, so no
        // predicate below has anything to decide against.
        if plane_normal(&position.map(exact_point))
            .iter()
            .all(Int::is_zero)
        {
            return Err(format!(
                "triangle {index} encloses zero exact area — its three welded corners are \
                 collinear, so it has no plane and no side"
            ));
        }
        let mut low = position[0];
        let mut high = position[0];
        for corner in &position[1..] {
            for axis in 0..3 {
                low[axis] = low[axis].min(corner[axis]);
                high[axis] = high[axis].max(corner[axis]);
            }
        }
        by_shell.entry(shell).or_default().push(Face {
            index,
            corners: *corners,
            low,
            high,
        });
    }

    for faces in by_shell.values_mut() {
        // Sweep and prune along the axis the shell is longest on: sorted by the low edge, a face
        // can only meet the ones whose low edge is still behind its own high edge.
        let axis = longest_axis(faces);
        faces.sort_by(|a, b| {
            a.low[axis]
                .partial_cmp(&b.low[axis])
                .unwrap_or(Ordering::Equal)
                .then(a.index.cmp(&b.index))
        });
        for (slot, face) in faces.iter().enumerate() {
            for other in &faces[slot + 1..] {
                if other.low[axis] > face.high[axis] {
                    break;
                }
                if (0..3).any(|k| face.low[k] > other.high[k] || other.low[k] > face.high[k]) {
                    continue;
                }
                verdict(face, other, vertices).map_err(|what| {
                    format!("triangles {} and {} {what}", face.index, other.index)
                })?;
            }
        }
    }
    Ok(())
}

/// The coordinate axis the shell spreads furthest along, by its faces' low edges.
fn longest_axis(faces: &[Face]) -> usize {
    let mut spread = [0.0f32; 3];
    for (axis, reach) in spread.iter_mut().enumerate() {
        let (mut low, mut high) = (f32::INFINITY, f32::NEG_INFINITY);
        for face in faces {
            low = low.min(face.low[axis]);
            high = high.max(face.high[axis]);
        }
        *reach = high - low;
    }
    (0..3)
        .max_by(|a, b| spread[*a].total_cmp(&spread[*b]))
        .unwrap_or(0)
}

/// Whether one pair of same-shell triangles meets only inside what it declares it shares.
fn verdict(a: &Face, b: &Face, vertices: &[[f32; 3]]) -> Result<(), String> {
    let shared: Vec<u32> = a
        .corners
        .iter()
        .copied()
        .filter(|corner| b.corners.contains(corner))
        .collect();
    if shared.len() == 3 {
        return Err("name the same three welded corners — one face, exported twice".into());
    }
    let (pa, pb) = (
        a.corners.map(|corner| vertices[corner as usize]),
        b.corners.map(|corner| vertices[corner as usize]),
    );
    if float_accepts(&pa, &pb, &shared, &a.corners, &b.corners) {
        return Ok(());
    }
    exact_verdict(&pa, &pb, &shared, vertices)
}

/// The cases the f64 filter can settle on its own, each a sufficient condition for `I ⊆ F`.
///
/// * Every corner of one triangle strictly off one side of the other's plane: `I = ∅`.
/// * A DECLARED EDGE whose two odd corners are off each other's planes: the planes are distinct
///   and both contain the edge's line, so they meet in exactly that line, each triangle meets it
///   in exactly its own edge, and `I` is that edge.
/// * A DECLARED VERTEX with the other triangle's two odd corners strictly one side: that triangle
///   meets the first one's plane at the shared vertex alone, so `I` is that vertex.
///
/// Returning `false` decides nothing; it sends the pair to the exact predicate.
fn float_accepts(
    pa: &[[f32; 3]; 3],
    pb: &[[f32; 3]; 3],
    shared: &[u32],
    ca: &[u32; 3],
    cb: &[u32; 3],
) -> bool {
    if strictly_one_side(pa, pb.iter()) || strictly_one_side(pb, pa.iter()) {
        return true;
    }
    let odd = |corners: &[u32; 3], points: &[[f32; 3]; 3]| -> Vec<[f32; 3]> {
        corners
            .iter()
            .zip(points)
            .filter(|(corner, _)| !shared.contains(corner))
            .map(|(_, point)| *point)
            .collect()
    };
    match shared.len() {
        2 => odd(cb, pb)
            .first()
            .and_then(|point| orient3d(pa[0], pa[1], pa[2], *point))
            .is_some_and(|sign| sign != 0),
        1 => strictly_one_side(pa, odd(cb, pb).iter()) || strictly_one_side(pb, odd(ca, pa).iter()),
        _ => false,
    }
}

/// Whether every point is provably off ONE side of the triangle's plane.
fn strictly_one_side<'p>(
    plane: &[[f32; 3]; 3],
    points: impl Iterator<Item = &'p [f32; 3]>,
) -> bool {
    let mut wanted = 0;
    for point in points {
        match orient3d(plane[0], plane[1], plane[2], *point) {
            Some(sign) if sign != 0 && (wanted == 0 || wanted == sign) => wanted = sign,
            _ => return false,
        }
    }
    wanted != 0
}

/// `((b − a) × (c − a)) · (d − a)` in f64, or `None` where its own roundoff reaches zero.
fn orient3d(a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]) -> Option<i32> {
    let at = |p: [f32; 3]| [p[0] as f64, p[1] as f64, p[2] as f64];
    let (a, b, c, d) = (at(a), at(b), at(c), at(d));
    let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let e3 = [d[0] - a[0], d[1] - a[1], d[2] - a[2]];
    let mut determinant = 0.0;
    let mut permanent = 0.0;
    for (axis, span) in e3.iter().enumerate() {
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        let (left, right) = (e1[u] * e2[v], e1[v] * e2[u]);
        determinant += (left - right) * span;
        permanent += (left.abs() + right.abs()) * span.abs();
    }
    let slack = ORIENT3D_SLACK * permanent + f64::MIN_POSITIVE;
    if determinant > slack {
        return Some(1);
    }
    if determinant < -slack {
        return Some(-1);
    }
    None
}

/// One certified coordinate triple as exact integers at [`SHIFT`].
fn exact_point(position: [f32; 3]) -> [Int; 3] {
    position.map(|value| Int::from_f32_scaled(value, SHIFT))
}

fn difference(a: &[Int; 3], b: &[Int; 3]) -> [Int; 3] {
    [a[0].sub(b[0]), a[1].sub(b[1]), a[2].sub(b[2])]
}

fn cross(a: &[Int; 3], b: &[Int; 3]) -> [Int; 3] {
    [
        a[1].mul(b[2]).sub(a[2].mul(b[1])),
        a[2].mul(b[0]).sub(a[0].mul(b[2])),
        a[0].mul(b[1]).sub(a[1].mul(b[0])),
    ]
}

fn dot(a: &[Int; 3], b: &[Int; 3]) -> Int {
    a[0].mul(b[0]).add(a[1].mul(b[1])).add(a[2].mul(b[2]))
}

fn plane_normal(points: &[[Int; 3]; 3]) -> [Int; 3] {
    cross(
        &difference(&points[1], &points[0]),
        &difference(&points[2], &points[0]),
    )
}

/// The exact law, for the pairs the filter could not settle.
fn exact_verdict(
    pa: &[[f32; 3]; 3],
    pb: &[[f32; 3]; 3],
    shared: &[u32],
    vertices: &[[f32; 3]],
) -> Result<(), String> {
    let (ea, eb) = (pa.map(exact_point), pb.map(exact_point));
    let (na, nb) = (plane_normal(&ea), plane_normal(&eb));
    // Where each triangle's corners sit relative to the OTHER's plane. Exact zero is on it, and
    // is the only thing that is.
    let signs = |points: &[[Int; 3]; 3], normal: &[Int; 3], origin: &[Int; 3]| -> [Int; 3] {
        [0, 1, 2].map(|corner| dot(normal, &difference(&points[corner], origin)))
    };
    let side_a = signs(&ea, &nb, &eb[0]);
    let side_b = signs(&eb, &na, &ea[0]);

    let intersection = if side_a.iter().all(Int::is_zero) {
        coplanar_intersection(&ea, &eb, &na)?
    } else {
        let direction = cross(&na, &nb);
        let Some(axis) = (0..3).find(|k| !direction[*k].is_zero()) else {
            // Parallel and distinct: the triangles share no point at all, and the empty set is
            // inside every feature.
            return Ok(());
        };
        let parameter = |points: &[[Int; 3]; 3]| [0, 1, 2].map(|corner| points[corner][axis]);
        overlap(
            clip(&parameter(&ea), &side_a),
            clip(&parameter(&eb), &side_b),
        )
        .map(|interval| (interval, axis))
    };

    let Some((interval, axis)) = intersection else {
        return Ok(());
    };
    match feature_interval(shared, vertices, axis) {
        Some((low, high)) if interval.0 >= low && interval.1 <= high => Ok(()),
        _ => Err(format!(
            "meet outside the {} they declare — every point two triangles of one shell share must \
             lie in their common welded feature, or the shell passes through itself",
            match shared.len() {
                0 => "nothing".to_string(),
                1 => format!("welded vertex {}", shared[0]),
                _ => format!("welded edge {}—{}", shared[0], shared[1]),
            }
        )),
    }
}

/// The intersection of two triangles that share a plane, as an interval on the line that carries
/// it — or an error, because a positive-area coplanar overlap is a self-intersection whatever the
/// two triangles declare.
///
/// The separating-axis theorem over the six projected edge lines: no weakly separating line means
/// the interiors meet, and the line that does separate contains every point the two share.
fn coplanar_intersection(
    ea: &[[Int; 3]; 3],
    eb: &[[Int; 3]; 3],
    na: &[Int; 3],
) -> Result<Option<((Ratio, Ratio), usize)>, String> {
    // Project along an axis the plane's normal is exactly nonzero on, so the projection is
    // injective on the plane and the two triangles stay non-degenerate.
    let dropped = (0..3)
        .find(|k| !na[*k].is_zero())
        .ok_or("share a plane with no normal")?;
    let kept: [usize; 2] = match dropped {
        0 => [1, 2],
        1 => [0, 2],
        _ => [0, 1],
    };
    let flatten = |points: &[[Int; 3]; 3]| [0, 1, 2].map(|corner| kept.map(|k| points[corner][k]));
    let (fa, fb) = (flatten(ea), flatten(eb));

    let orient = |p: &[Int; 2], q: &[Int; 2], r: &[Int; 2]| -> Int {
        q[0].sub(p[0])
            .mul(r[1].sub(p[1]))
            .sub(q[1].sub(p[1]).mul(r[0].sub(p[0])))
    };
    for edge in [(&fa, 0), (&fa, 1), (&fa, 2), (&fb, 0), (&fb, 1), (&fb, 2)] {
        let (points, corner) = edge;
        let (p, q) = (&points[corner], &points[(corner + 1) % 3]);
        let side = |triangle: &[[Int; 2]; 3]| [0, 1, 2].map(|c| orient(p, q, &triangle[c]));
        let (left, right) = (side(&fa), side(&fb));
        let one_side = |signs: &[Int; 3], want: i32| signs.iter().all(|s| s.signum() * want <= 0);
        if !((one_side(&left, 1) && one_side(&right, -1))
            || (one_side(&left, -1) && one_side(&right, 1)))
        {
            continue;
        }
        // Every shared point is on this line, so the parameter is whichever coordinate the line
        // moves along.
        let along = usize::from(q[0].sub(p[0]).is_zero());
        let parameter = |t: &[[Int; 2]; 3]| [0, 1, 2].map(|c| t[c][along]);
        return Ok(
            overlap(clip(&parameter(&fa), &left), clip(&parameter(&fb), &right))
                .map(|interval| (interval, kept[along])),
        );
    }
    Err(
        "overlap on a positive area of one plane — two coplanar faces of one shell may touch only \
         along the welded feature they share"
            .into(),
    )
}

/// The closed interval a triangle occupies on the clipping line, in the caller's parameter.
///
/// `signs` are the exact orientations of the three corners against that line's plane (or, in two
/// dimensions, against the line): a corner at exact zero IS an endpoint, and an edge whose ends
/// disagree in sign crosses at `(−P_i·s_j + P_j·s_i) / (s_i − s_j)`. All three strictly one side
/// is the empty intersection.
fn clip(parameter: &[Int; 3], signs: &[Int; 3]) -> Option<(Ratio, Ratio)> {
    let mut ends: Vec<Ratio> = Vec::new();
    for corner in 0..3 {
        if signs[corner].is_zero() {
            ends.push(Ratio::whole(parameter[corner]));
        }
    }
    for (i, j) in [(0, 1), (1, 2), (2, 0)] {
        if signs[i].signum() * signs[j].signum() >= 0 {
            continue;
        }
        ends.push(Ratio::new(
            parameter[i]
                .mul(signs[j].negated())
                .add(parameter[j].mul(signs[i])),
            signs[i].sub(signs[j]),
        ));
    }
    let low = ends.iter().copied().min()?;
    let high = ends.into_iter().max()?;
    Some((low, high))
}

/// Two closed intervals' overlap, or `None` where they do not reach each other.
fn overlap(a: Option<(Ratio, Ratio)>, b: Option<(Ratio, Ratio)>) -> Option<(Ratio, Ratio)> {
    let ((a_low, a_high), (b_low, b_high)) = (a?, b?);
    let (low, high) = (a_low.max(b_low), a_high.min(b_high));
    (low <= high).then_some((low, high))
}

/// The declared feature, as an interval in the same parameter the intersection was measured in.
fn feature_interval(shared: &[u32], vertices: &[[f32; 3]], axis: usize) -> Option<(Ratio, Ratio)> {
    let mut ends: Vec<Ratio> = shared
        .iter()
        .map(|&corner| Ratio::whole(Int::from_f32_scaled(vertices[corner as usize][axis], SHIFT)))
        .collect();
    ends.sort();
    Some((*ends.first()?, *ends.last()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Weld a triangle list by exact position, the way the gate does, and certify it as one shell.
    fn certify(faces: &[[[f32; 3]; 3]]) -> Result<(), String> {
        let mut vertices: Vec<[f32; 3]> = Vec::new();
        let mut triangles: Vec<[u32; 3]> = Vec::new();
        for face in faces {
            triangles.push(
                face.map(|point| match vertices.iter().position(|v| *v == point) {
                    Some(id) => id as u32,
                    None => {
                        vertices.push(point);
                        (vertices.len() - 1) as u32
                    }
                }),
            );
        }
        certify_embedding(&triangles, &vec![0; triangles.len()], &vertices)
    }

    const UNIT: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];

    /// THE DECLARED FEATURE IS THE WHOLE EXEMPTION, and meeting exactly on it is legal.
    ///
    /// A shared vertex across two planes, a shared vertex inside one plane, a shared edge, and the
    /// case a Möller interval test gets wrong: an edge of one triangle LIES in the other's plane
    /// and still reaches its closed area only at the vertex the two declare.
    #[test]
    fn a_pair_that_meets_exactly_on_what_it_declares_passes() {
        certify(&[UNIT, [[0.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]])
            .expect("one shared vertex, two planes");
        certify(&[UNIT, [[0.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, -1.0, 0.0]]])
            .expect("one shared vertex, one plane");
        certify(&[UNIT, [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]])
            .expect("one shared edge");
        certify(&[UNIT, [[0.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]]])
            .expect("an edge lying in the other plane, touching only at the shared vertex");
    }

    /// TWO TRIANGLES THAT SHARE NOTHING MUST SHARE NO POINT.
    #[test]
    fn a_contact_with_no_declared_feature_is_refused() {
        let piercing = [[0.25, 0.25, -1.0], [0.25, 0.25, 1.0], [2.0, 2.0, 0.0]];
        let err = certify(&[UNIT, piercing]).expect_err("a pierced face is a self-intersection");
        assert!(err.contains("nothing they declare"), "{err}");
    }

    /// AN INTERSECTION THAT EXTENDS PAST THE FEATURE IS A SELF-INTERSECTION, however small the
    /// extension and however legal the contact at the feature itself.
    #[test]
    fn an_intersection_reaching_past_the_feature_is_refused() {
        let through = [[0.0, 0.0, 0.0], [0.5, 0.5, -1.0], [0.5, 0.5, 1.0]];
        let err = certify(&[UNIT, through]).expect_err("a shared vertex does not license a chord");
        assert!(err.contains("welded vertex"), "{err}");
    }

    /// THE SAME THREE WELDED CORNERS TWICE IS ONE FACE, EXPORTED TWICE.
    #[test]
    fn a_duplicated_face_is_refused() {
        let err = certify(&[UNIT, [UNIT[0], UNIT[2], UNIT[1]]])
            .expect_err("two faces on three corners are one face");
        assert!(err.contains("exported twice"), "{err}");
    }

    /// A POSITIVE-AREA COPLANAR OVERLAP IS FORBIDDEN — there is no feature it could be inside.
    #[test]
    fn a_coplanar_overlap_of_positive_area_is_refused() {
        let inside = [[0.1, 0.1, 0.0], [0.9, 0.1, 0.0], [0.1, 0.9, 0.0]];
        let err = certify(&[UNIT, inside]).expect_err("one plate laid over another is an overlap");
        assert!(err.contains("positive area"), "{err}");
    }

    /// A triangle whose three DISTINCT welded corners are collinear has no plane, so nothing below
    /// can decide a side against it.
    #[test]
    fn a_collinear_triangle_is_refused_before_any_pair_is_tested() {
        let err = certify(&[[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]]])
            .expect_err("three points on a line are not a triangle");
        assert!(err.contains("collinear"), "{err}");
    }

    /// SHELLS ARE INDEPENDENT: two surfaces of one primitive may pass through each other, and this
    /// gate is about a shell's own embedding.
    #[test]
    fn the_gate_is_per_shell() {
        let through = [[0.0, 0.0, 0.0], [0.5, 0.5, -1.0], [0.5, 0.5, 1.0]];
        let mut vertices: Vec<[f32; 3]> = Vec::new();
        let mut triangles: Vec<[u32; 3]> = Vec::new();
        for face in [UNIT, through] {
            triangles.push(
                face.map(|point| match vertices.iter().position(|v| *v == point) {
                    Some(id) => id as u32,
                    None => {
                        vertices.push(point);
                        (vertices.len() - 1) as u32
                    }
                }),
            );
        }
        certify_embedding(&triangles, &[0, 1], &vertices)
            .expect("two shells of one primitive are two certificates");
    }
}
