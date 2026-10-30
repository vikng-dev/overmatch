//! The bake's geometry: convex polygons and right prisms in f64, with only the operations IEEE 754
//! fixes on every platform (add, multiply, divide, square root, round). No trigonometry, so a bake
//! is the same bits everywhere.

use glam::{DVec2, DVec3};

/// Two vertices nearer than this (m) are one: the clip's own rounding, far below any feature.
const WELD_M: f64 = 1.0e-9;

/// `polygon` (convex, counter-clockwise) cut to the half-plane `normal · p ≤ offset`. Keeps the
/// winding; empty when nothing of it lies inside.
pub fn clip(polygon: &[DVec2], normal: DVec2, offset: f64) -> Vec<DVec2> {
    let n = polygon.len();
    let mut out: Vec<DVec2> = Vec::with_capacity(n + 1);
    for k in 0..n {
        let (p, q) = (polygon[k], polygon[(k + 1) % n]);
        let (dp, dq) = (normal.dot(p) - offset, normal.dot(q) - offset);
        if dp <= 0.0 {
            out.push(p);
        }
        if (dp < 0.0 && dq > 0.0) || (dp > 0.0 && dq < 0.0) {
            out.push(p + (q - p) * (dp / (dp - dq)));
        }
    }
    let mut welded: Vec<DVec2> = Vec::with_capacity(out.len());
    for p in out {
        if welded.last().is_none_or(|last| last.distance(p) > WELD_M) {
            welded.push(p);
        }
    }
    while welded.len() > 1 && welded[0].distance(welded[welded.len() - 1]) <= WELD_M {
        welded.pop();
    }
    if welded.len() < 3 { Vec::new() } else { welded }
}

/// The signed area, positive counter-clockwise.
pub fn area(polygon: &[DVec2]) -> f64 {
    let n = polygon.len();
    0.5 * (0..n)
        .map(|k| polygon[k].perp_dot(polygon[(k + 1) % n]))
        .sum::<f64>()
}

/// The area centroid of a polygon of nonzero area.
pub fn centroid(polygon: &[DVec2]) -> DVec2 {
    let n = polygon.len();
    let mut sum = DVec2::ZERO;
    for k in 0..n {
        let (p, q) = (polygon[k], polygon[(k + 1) % n]);
        sum += (p + q) * p.perp_dot(q);
    }
    sum / (6.0 * area(polygon))
}

/// The polygon's extent along `u` on the line `v = y`: `None` where the line misses it.
pub fn span_at(polygon: &[DVec2], y: f64) -> Option<(f64, f64)> {
    let n = polygon.len();
    let mut span: Option<(f64, f64)> = None;
    let mut take = |u: f64| {
        span = Some(span.map_or((u, u), |(lo, hi)| (lo.min(u), hi.max(u))));
    };
    for k in 0..n {
        let (p, q) = (polygon[k], polygon[(k + 1) % n]);
        if p.y == y {
            take(p.x);
        }
        if (p.y < y && q.y > y) || (p.y > y && q.y < y) {
            take(p.x + (q.x - p.x) * ((y - p.y) / (q.y - p.y)));
        }
    }
    span
}

/// A frame: origin and two orthonormal in-plane axes; the third axis is `u × v`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub origin: DVec3,
    pub u: DVec3,
    pub v: DVec3,
}

impl Frame {
    pub fn w(&self) -> DVec3 {
        self.u.cross(self.v)
    }

    pub fn point(&self, p: DVec2, d: f64) -> DVec3 {
        self.origin + self.u * p.x + self.v * p.y + self.w() * d
    }
}

/// A right prism: a convex profile (counter-clockwise about `w`) in `frame`, extruded over
/// `[near, far]` along `w`.
#[derive(Debug, Clone, PartialEq)]
pub struct Prism {
    pub frame: Frame,
    pub profile: Vec<DVec2>,
    pub near: f64,
    pub far: f64,
}

/// One planar face: its outward unit normal, its plane offset `normal · p`, and its corners
/// counter-clockwise about the normal.
#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    pub normal: DVec3,
    pub offset: f64,
    pub corners: Vec<DVec3>,
}

impl Prism {
    pub fn volume(&self) -> f64 {
        area(&self.profile) * (self.far - self.near)
    }

    pub fn centroid(&self) -> DVec3 {
        self.frame
            .point(centroid(&self.profile), 0.5 * (self.near + self.far))
    }

    /// The corners: the profile at `near`, then at `far`.
    pub fn corners(&self) -> Vec<DVec3> {
        [self.near, self.far]
            .iter()
            .flat_map(|&d| self.profile.iter().map(move |&p| self.frame.point(p, d)))
            .collect()
    }

    /// The far cap, the near cap, then one side per profile edge.
    pub fn faces(&self) -> Vec<Face> {
        let n = self.profile.len();
        let w = self.frame.w();
        let face = |normal: DVec3, corners: Vec<DVec3>| Face {
            normal,
            offset: normal.dot(corners[0]),
            corners,
        };
        let mut faces = vec![
            face(
                w,
                self.profile
                    .iter()
                    .map(|&p| self.frame.point(p, self.far))
                    .collect(),
            ),
            face(
                -w,
                self.profile
                    .iter()
                    .rev()
                    .map(|&p| self.frame.point(p, self.near))
                    .collect(),
            ),
        ];
        for k in 0..n {
            let (p, q) = (self.profile[k], self.profile[(k + 1) % n]);
            let out = DVec2::new(q.y - p.y, p.x - q.x).normalize();
            faces.push(face(
                (self.frame.u * out.x + self.frame.v * out.y).normalize(),
                vec![
                    self.frame.point(p, self.near),
                    self.frame.point(q, self.near),
                    self.frame.point(q, self.far),
                    self.frame.point(p, self.far),
                ],
            ));
        }
        faces
    }

    /// The axis-aligned bounds, `(min, max)`.
    pub fn bounds(&self) -> (DVec3, DVec3) {
        self.corners().iter().fold(
            (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
            |(lo, hi), &p| (lo.min(p), hi.max(p)),
        )
    }
}

/// An orthonormal basis `(e1, e2)` of the plane with unit normal `n`, `e1 × e2 = n`: the one
/// projection both faces of a contact are read in.
pub fn plane_basis(n: DVec3) -> (DVec3, DVec3) {
    let axis = if n.x.abs() < 0.9 { DVec3::X } else { DVec3::Y };
    let e1 = n.cross(axis).normalize();
    (e1, n.cross(e1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<DVec2> {
        vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(1.0, 0.0),
            DVec2::new(1.0, 1.0),
            DVec2::new(0.0, 1.0),
        ]
    }

    /// A clip keeps the side the half-plane names and the winding: a unit square cut at `x ≤ 0.25`
    /// keeps a quarter, counter-clockwise; cut by a plane it lies wholly outside of, nothing.
    #[test]
    fn a_clip_keeps_the_named_side_and_the_winding() {
        let kept = clip(&square(), DVec2::X, 0.25);
        assert!((area(&kept) - 0.25).abs() < 1e-12, "{kept:?}");
        assert!(clip(&square(), DVec2::X, -1.0).is_empty());
        assert_eq!(clip(&square(), DVec2::X, 2.0), square());
    }

    /// Every face of a prism points out of it and lies on its own plane: a mutant that winds a
    /// side face inward hands the contact search faces that never oppose a neighbour's.
    #[test]
    fn every_prism_face_points_out_and_lies_on_its_plane() {
        let prism = Prism {
            frame: Frame {
                origin: DVec3::new(1.0, 2.0, 3.0),
                u: DVec3::new(0.8, -0.6, 0.0),
                v: DVec3::new(0.6, 0.8, 0.0),
            },
            profile: square(),
            near: -0.5,
            far: 0.25,
        };
        let centre = prism.centroid();
        assert!((prism.volume() - 0.75).abs() < 1e-12);
        for face in prism.faces() {
            assert!(face.normal.dot(face.corners[0] - centre) > 0.0);
            for &corner in &face.corners {
                assert!((face.normal.dot(corner) - face.offset).abs() < 1e-12);
            }
            let (e1, e2) = plane_basis(face.normal);
            let flat: Vec<DVec2> = face
                .corners
                .iter()
                .map(|p| DVec2::new(p.dot(e1), p.dot(e2)))
                .collect();
            assert!(area(&flat) > 0.0, "counter-clockwise about the normal");
        }
    }
}
