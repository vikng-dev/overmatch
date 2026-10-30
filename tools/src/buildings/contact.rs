//! The joint graph: every face two chunks share, and every chunk face the ground holds.
//!
//! Generated from the cut ("exact" bonds, NVIDIA Blast's term): two chunks touch where a face of
//! one and a face of the other lie on one plane with opposite normals and overlap with positive
//! area. The overlap is the joint: its area, its centroid and the first chunk's outward normal.
//! The search reads geometry only, so it finds joints across element boundaries (a pier and its
//! lintel, a wall's corner, a roof board on a gable) exactly as inside one.

use glam::{DVec2, DVec3};

use super::geom::{Face, Prism, area, centroid, clip, plane_basis};

/// Two faces oppose when their normals' dot product is within this of −1.
const OPPOSED: f64 = 1.0e-9;

/// Two opposed faces are coplanar when their plane offsets cancel within this, metres.
const COPLANAR_M: f64 = 1.0e-6;

/// The least overlap (m²) that is a joint: a square millimetre, far above the clip's rounding and
/// far below any chunk face.
const MIN_AREA_M2: f64 = 1.0e-6;

/// A joint between chunks `a < b`: the shared face's area, centroid and normal from `a` into `b`.
#[derive(Debug, Clone, PartialEq)]
pub struct Joint {
    pub a: usize,
    pub b: usize,
    pub area: f64,
    pub centroid: DVec3,
    pub normal: DVec3,
}

/// A chunk face on the foundation plane.
#[derive(Debug, Clone, PartialEq)]
pub struct Anchor {
    pub chunk: usize,
    pub area: f64,
    pub centroid: DVec3,
}

/// `face`'s corners in the plane basis `(e1, e2)`.
fn flat(face: &Face, (e1, e2): (DVec3, DVec3)) -> Vec<DVec2> {
    face.corners
        .iter()
        .map(|p| DVec2::new(p.dot(e1), p.dot(e2)))
        .collect()
}

/// The overlap of `a`'s face and `b`'s opposed coplanar face, as `(area, centroid)`.
fn overlap(fa: &Face, fb: &Face) -> Option<(f64, DVec3)> {
    let basis = plane_basis(fa.normal);
    let mut shared = flat(fa, basis);
    // `b`'s corners run counter-clockwise about its own normal, `−n`: clockwise here.
    let mut other = flat(fb, basis);
    other.reverse();
    let n = other.len();
    for k in 0..n {
        let (p, q) = (other[k], other[(k + 1) % n]);
        let edge = q - p;
        // Inside lies left of each counter-clockwise edge: `(edge.y, −edge.x) · x ≤ … · p`.
        let normal = DVec2::new(edge.y, -edge.x);
        shared = clip(&shared, normal, normal.dot(p));
        if shared.is_empty() {
            return None;
        }
    }
    let size = area(&shared);
    (size > MIN_AREA_M2).then(|| {
        let c = centroid(&shared);
        (size, basis.0 * c.x + basis.1 * c.y + fa.normal * fa.offset)
    })
}

/// Every joint between `chunks`, ordered by `(a, b)`. Pairs whose bounds overlap are swept along
/// `x`; each such pair's faces are compared pairwise.
pub fn joints(chunks: &[Prism]) -> Vec<Joint> {
    let faces: Vec<Vec<Face>> = chunks.iter().map(Prism::faces).collect();
    let bounds: Vec<(DVec3, DVec3)> = chunks.iter().map(Prism::bounds).collect();
    let mut order: Vec<usize> = (0..chunks.len()).collect();
    order.sort_by(|&i, &j| bounds[i].0.x.total_cmp(&bounds[j].0.x).then(i.cmp(&j)));
    let slack = DVec3::splat(COPLANAR_M);
    let mut out = Vec::new();
    for (k, &i) in order.iter().enumerate() {
        for &j in &order[k + 1..] {
            if bounds[j].0.x > bounds[i].1.x + COPLANAR_M {
                break;
            }
            let (lo, hi) = (bounds[i].0.max(bounds[j].0), bounds[i].1.min(bounds[j].1));
            if (lo - slack).cmpgt(hi).any() {
                continue;
            }
            let (a, b) = (i.min(j), i.max(j));
            let mut total = 0.0;
            let mut weighted = DVec3::ZERO;
            let mut normal = None;
            for fa in &faces[a] {
                for fb in &faces[b] {
                    if fa.normal.dot(fb.normal) > -1.0 + OPPOSED
                        || (fa.offset + fb.offset).abs() > COPLANAR_M
                    {
                        continue;
                    }
                    if let Some((size, at)) = overlap(fa, fb) {
                        total += size;
                        weighted += at * size;
                        normal.get_or_insert(fa.normal);
                    }
                }
            }
            if let Some(normal) = normal {
                out.push(Joint {
                    a,
                    b,
                    area: total,
                    centroid: weighted / total,
                    normal,
                });
            }
        }
    }
    out.sort_by_key(|joint| (joint.a, joint.b));
    out
}

/// Every chunk face on the plane `y = foundation_y` facing down: the ground's hold, by chunk.
pub fn anchors(chunks: &[Prism], foundation_y: f64) -> Vec<Anchor> {
    let mut out = Vec::new();
    for (chunk, prism) in chunks.iter().enumerate() {
        for face in prism.faces() {
            if face.normal.dot(DVec3::NEG_Y) < 1.0 - OPPOSED
                || (face.offset + foundation_y).abs() > COPLANAR_M
            {
                continue;
            }
            let basis = plane_basis(face.normal);
            let polygon = flat(&face, basis);
            let size = area(&polygon);
            if size > MIN_AREA_M2 {
                let c = centroid(&polygon);
                out.push(Anchor {
                    chunk,
                    area: size,
                    centroid: basis.0 * c.x + basis.1 * c.y + face.normal * face.offset,
                });
            }
        }
    }
    out
}
