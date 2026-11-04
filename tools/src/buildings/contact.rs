//! The joint graph: every face two chunks share, and every chunk face the ground holds.
//!
//! Generated from the cut ("exact" bonds, NVIDIA Blast's term): two chunks touch where a face of
//! one and a face of the other lie on one plane with opposite normals and overlap with positive
//! area. The overlap is the joint: its area, its centroid and the first chunk's outward normal.
//! The search reads geometry only, so it finds joints across element boundaries (a pier and its
//! lintel, a wall's corner, a roof board on a gable) exactly as inside one.

use glam::{DVec2, DVec3};

use super::geom::{Face, Polyhedron, area, centroid, clip, plane_basis};

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
pub fn joints(chunks: &[Polyhedron]) -> Vec<Joint> {
    let faces: Vec<Vec<Face>> = chunks.iter().map(Polyhedron::faces).collect();
    let bounds: Vec<(DVec3, DVec3)> = chunks.iter().map(Polyhedron::bounds).collect();
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
pub fn anchors(chunks: &[Polyhedron], foundation_y: f64) -> Vec<Anchor> {
    let mut out = Vec::new();
    for (chunk, solid) in chunks.iter().enumerate() {
        for face in solid.faces() {
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

#[cfg(test)]
mod tests {
    use super::super::geom::tests::cuboid;
    use super::*;

    /// A JOINT IS THE OVERLAP CALCULATED BY HAND: box `[0,2]×[0,1]×[0,1]` under box
    /// `[1,3]×[1,2]×[0.5,1.5]` share the plane `y = 1` over `x ∈ [1,2]`, `z ∈ [0.5,1]`: one joint of
    /// area 0.5 at (1.5, 1, 0.75), its normal +Y from the lower box into the upper. The lower box's
    /// foot on `y = 0` is its anchor, area 2 at (1, 0, 0.5). Expectations DERIVED by hand; a
    /// search that summed both faces, read the centroid off one face, or flipped the normal fails.
    #[test]
    fn a_joint_is_the_overlap_calculated_by_hand() {
        let boxes = [
            cuboid(DVec3::ZERO, DVec3::new(2.0, 1.0, 1.0)),
            cuboid(DVec3::new(1.0, 1.0, 0.5), DVec3::new(3.0, 2.0, 1.5)),
        ];
        let found = joints(&boxes);
        assert_eq!(found.len(), 1);
        let joint = &found[0];
        assert_eq!((joint.a, joint.b, joint.normal), (0, 1, DVec3::Y));
        assert!((joint.area - 0.5).abs() < 1e-14);
        assert!((joint.centroid - DVec3::new(1.5, 1.0, 0.75)).length() < 1e-14);
        let feet = anchors(&boxes, 0.0);
        assert_eq!(feet.len(), 1);
        assert_eq!(feet[0].chunk, 0);
        assert!((feet[0].area - 2.0).abs() < 1e-14);
        assert!((feet[0].centroid - DVec3::new(1.0, 0.0, 0.5)).length() < 1e-14);
    }

    /// CELLS CUT FROM ONE SOLID ACCOUNT FOR ALL OF IT: a box split by oblique planes into cells,
    /// every cell split by every plane, keeps its volume, and each cell's surface is exactly its
    /// joints plus its faces on the box's own boundary. Each joint's centroid lies on a face of
    /// both its cells. The bakers and the laws rely on every interface being found once with its
    /// full area: a lost interface, a doubled one, or one whose area or centroid differs from the
    /// faces it joins fails the surface balance or the centroid check. Fixture data.
    #[test]
    fn cells_cut_from_one_solid_account_for_all_of_it() {
        let (lo, hi) = (DVec3::new(-1.0, 0.0, -0.5), DVec3::new(1.5, 2.0, 0.5));
        let whole = cuboid(lo, hi);
        let mut cells = vec![whole.clone()];
        for (n, d) in [
            (DVec3::new(1.0, 0.2, 0.1), 0.3),
            (DVec3::new(-0.3, 1.0, 0.4), 1.1),
            (DVec3::new(0.1, -0.2, 1.0), 0.05),
            (DVec3::new(0.7, 0.7, 0.0), 0.9),
        ] {
            cells = cells
                .iter()
                .flat_map(|cell| {
                    let (a, b) = cell.split(n, d);
                    a.into_iter().chain(b)
                })
                .collect();
        }
        assert!(cells.len() >= 10, "{} cells", cells.len());
        let volume: f64 = cells.iter().map(Polyhedron::volume).sum();
        assert!((volume - whole.volume()).abs() < 1e-12);
        let found = joints(&cells);
        let on_box = |face: &Face| {
            whole.faces().iter().any(|b| {
                b.normal.dot(face.normal) > 1.0 - 1e-12 && (b.offset - face.offset).abs() < 1e-9
            })
        };
        for (k, cell) in cells.iter().enumerate() {
            cell.certify(1e-9).unwrap();
            let faces = cell.faces();
            let surface: f64 = faces.iter().map(Face::area).sum();
            let boundary: f64 = faces.iter().filter(|f| on_box(f)).map(Face::area).sum();
            let joined: f64 = found
                .iter()
                .filter(|joint| joint.a == k || joint.b == k)
                .map(|joint| joint.area)
                .sum();
            assert!(
                (boundary + joined - surface).abs() < 1e-9,
                "cell {k}: {boundary} + {joined} against {surface}"
            );
        }
        for joint in &found {
            for (cell, out) in [(joint.a, joint.normal), (joint.b, -joint.normal)] {
                assert!(
                    cells[cell].faces().iter().any(|face| {
                        face.normal.dot(out) > 1.0 - 1e-9
                            && (face.normal.dot(joint.centroid) - face.offset).abs() < 1e-9
                    }),
                    "{joint:?} off cell {cell}'s face"
                );
            }
        }
    }
}
