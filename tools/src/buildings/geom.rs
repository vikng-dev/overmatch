//! The bake's geometry: convex polygons, right prisms and convex polyhedra in f64, with only the
//! operations IEEE 754 fixes on every platform (add, multiply, divide, square root, round). No
//! trigonometry, so a bake is the same bits everywhere.
//!
//! A [`Polyhedron`] is the one solid every cut produces and every reader after the cut takes
//! (contacts, volume, centroid, the written hull); a [`Prism`] is how a generator states one.

use std::collections::BTreeMap;

use glam::{DVec2, DVec3};

/// Two vertices nearer than this (m) are one, and a corner nearer a cutting plane lies on it: the
/// clip's own rounding, far below any feature (Resolution).
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
    /// The volume in closed form, profile area × depth: the check its cells' moments sum to.
    pub fn volume(&self) -> f64 {
        area(&self.profile) * (self.far - self.near)
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

    /// The prism as a [`Polyhedron`]: the profile at `near` (corners `0..n`), then at `far`
    /// (`n..2n`); its faces in [`Prism::faces`]' order.
    pub fn solid(&self) -> Polyhedron {
        let n = self.profile.len();
        let corners = self.corners();
        let faces = self
            .faces()
            .into_iter()
            .enumerate()
            .map(|(f, face)| Facet {
                normal: face.normal,
                offset: face.offset,
                corners: match f {
                    0 => (n..2 * n).collect(),
                    1 => (0..n).rev().collect(),
                    _ => {
                        let k = f - 2;
                        vec![k, (k + 1) % n, n + (k + 1) % n, n + k]
                    }
                },
            })
            .collect();
        Polyhedron { corners, faces }
    }
}

/// An orthonormal basis `(e1, e2)` of the plane with unit normal `n`, `e1 × e2 = n`: the one
/// projection both faces of a contact are read in.
pub fn plane_basis(n: DVec3) -> (DVec3, DVec3) {
    let axis = if n.x.abs() < 0.9 { DVec3::X } else { DVec3::Y };
    let e1 = n.cross(axis).normalize();
    (e1, n.cross(e1))
}

impl Face {
    /// The face's area, m²: its fan's cross products read along its normal.
    pub fn area(&self) -> f64 {
        let first = self.corners[0];
        0.5 * self
            .corners
            .windows(2)
            .skip(1)
            .map(|pair| (pair[0] - first).cross(pair[1] - first))
            .sum::<DVec3>()
            .dot(self.normal)
    }
}

/// One face of a [`Polyhedron`]: its plane (outward unit normal, offset `normal · p`) and its
/// corners, indices into [`Polyhedron::corners`], counter-clockwise seen from outside.
#[derive(Debug, Clone, PartialEq)]
pub struct Facet {
    pub normal: DVec3,
    pub offset: f64,
    pub corners: Vec<usize>,
}

/// A convex polyhedron (module doc). Closed: every edge one face runs `i → j`, exactly one other
/// runs `j → i`, so two faces sharing an edge share its corners, bit for bit. A face's plane is
/// the plane it was cut on, never refitted to its rounded corners.
#[derive(Debug, Clone, PartialEq)]
pub struct Polyhedron {
    pub corners: Vec<DVec3>,
    pub faces: Vec<Facet>,
}

impl Polyhedron {
    /// The part of the solid with `normal · p ≤ offset` (`normal` nonzero, any length): `None`
    /// when no corner lies strictly inside, the whole solid when none lies strictly outside. A
    /// corner within [`WELD_M`] of the plane lies on it, so no cut leaves a sliver thinner than
    /// that. The cut face is new, on the plane, its outward normal `normal`.
    ///
    /// Each crossing corner is computed once per edge, from its lesser end in coordinate order, so
    /// every solid holding that edge (the faces on either side of it, the two halves of
    /// [`Self::split`], neighbouring cells cut by one plane) meets at the same bits.
    pub fn clip(&self, normal: DVec3, offset: f64) -> Option<Self> {
        let length = normal.length();
        let (n, d) = (normal / length, offset / length);
        let side: Vec<f64> = self
            .corners
            .iter()
            .map(|&p| {
                let s = n.dot(p) - d;
                if s.abs() <= WELD_M { 0.0 } else { s }
            })
            .collect();
        if side.iter().all(|&s| s >= 0.0) {
            return None;
        }
        if side.iter().all(|&s| s <= 0.0) {
            return Some(self.clone());
        }
        let mut corners: Vec<DVec3> = Vec::new();
        let mut kept: Vec<Option<usize>> = vec![None; self.corners.len()];
        for (k, &p) in self.corners.iter().enumerate() {
            if side[k] <= 0.0 {
                kept[k] = Some(corners.len());
                corners.push(p);
            }
        }
        let mut crossings: BTreeMap<(usize, usize), usize> = BTreeMap::new();
        let mut cap: Vec<usize> = Vec::new();
        let mut faces: Vec<Facet> = Vec::new();
        for face in &self.faces {
            if face.corners.iter().all(|&i| side[i] >= 0.0) {
                continue;
            }
            let m = face.corners.len();
            let mut ring = Vec::with_capacity(m + 1);
            for k in 0..m {
                let (i, j) = (face.corners[k], face.corners[(k + 1) % m]);
                if let Some(at) = kept[i] {
                    ring.push(at);
                    if side[i] == 0.0 {
                        cap.push(at);
                    }
                }
                if side[i] * side[j] < 0.0 {
                    let at = *crossings.entry((i.min(j), i.max(j))).or_insert_with(|| {
                        let lesser = |p: DVec3, q: DVec3| {
                            p.to_array()
                                .iter()
                                .zip(q.to_array())
                                .map(|(a, b)| a.total_cmp(&b))
                                .find(|order| order.is_ne())
                                .is_some_and(|order| order.is_lt())
                        };
                        let (a, b) = if lesser(self.corners[j], self.corners[i]) {
                            (j, i)
                        } else {
                            (i, j)
                        };
                        let (pa, pb) = (self.corners[a], self.corners[b]);
                        corners.push(pa + (pb - pa) * (side[a] / (side[a] - side[b])));
                        corners.len() - 1
                    });
                    ring.push(at);
                    cap.push(at);
                }
            }
            faces.push(Facet {
                normal: face.normal,
                offset: face.offset,
                corners: ring,
            });
        }
        cap.sort_unstable();
        cap.dedup();
        faces.push(Facet {
            normal: n,
            offset: d,
            corners: counter_clockwise(&corners, cap, n),
        });
        Some(Self { corners, faces }.compact())
    }

    /// The solid's two parts on either side of the plane `normal · p = offset`: `(≤, ≥)`. Their
    /// cut faces share every corner, bit for bit ([`Self::clip`]).
    pub fn split(&self, normal: DVec3, offset: f64) -> (Option<Self>, Option<Self>) {
        (self.clip(normal, offset), self.clip(-normal, -offset))
    }

    /// The same solid without corners no face names, renumbered in first-use order of the old.
    fn compact(self) -> Self {
        let mut used = vec![false; self.corners.len()];
        for face in &self.faces {
            for &i in &face.corners {
                used[i] = true;
            }
        }
        let mut renumber = vec![usize::MAX; self.corners.len()];
        let mut corners = Vec::new();
        for (k, p) in self.corners.into_iter().enumerate() {
            if used[k] {
                renumber[k] = corners.len();
                corners.push(p);
            }
        }
        let faces = self
            .faces
            .into_iter()
            .map(|face| Facet {
                corners: face.corners.iter().map(|&i| renumber[i]).collect(),
                ..face
            })
            .collect();
        Self { corners, faces }
    }

    /// Every face with its corners as points.
    pub fn faces(&self) -> Vec<Face> {
        self.faces
            .iter()
            .map(|face| Face {
                normal: face.normal,
                offset: face.offset,
                corners: face.corners.iter().map(|&i| self.corners[i]).collect(),
            })
            .collect()
    }

    /// The volume and the volume centroid: the signed tetrahedra from the corners' mean to each
    /// face's fan.
    fn moments(&self) -> (f64, DVec3) {
        let mean = self.corners.iter().sum::<DVec3>() / self.corners.len() as f64;
        let (mut volume, mut moment) = (0.0, DVec3::ZERO);
        for face in self.faces() {
            let first = face.corners[0];
            for pair in face.corners.windows(2).skip(1) {
                let v = (first - mean).dot((pair[0] - mean).cross(pair[1] - mean)) / 6.0;
                volume += v;
                moment += (mean + first + pair[0] + pair[1]) * (v / 4.0);
            }
        }
        (volume, moment / volume)
    }

    /// The volume, m³.
    pub fn volume(&self) -> f64 {
        self.moments().0
    }

    /// The volume centroid.
    pub fn centroid(&self) -> DVec3 {
        self.moments().1
    }

    /// The axis-aligned bounds, `(min, max)`.
    pub fn bounds(&self) -> (DVec3, DVec3) {
        self.corners.iter().fold(
            (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
            |(lo, hi), &p| (lo.min(p), hi.max(p)),
        )
    }

    /// The certificate every baked cell meets, naming the first breach: every corner and plane
    /// finite (a NaN passes every ordered test below, so it is refused first); at least four
    /// faces, each of three corners or more, a unit normal, positive area and its corners on its
    /// plane; closed (each directed edge once, its reverse once); one shell (every face reaches
    /// every other across edges, so no second surface over the same corners doubles the volume);
    /// convex (every corner on or inside every face's plane); positive volume. Plane tolerance
    /// `tolerance` m.
    pub fn certify(&self, tolerance: f64) -> Result<(), String> {
        if let Some(i) = self.corners.iter().position(|p| !p.is_finite()) {
            return Err(format!("corner {i} is not finite"));
        }
        if let Some(f) = self
            .faces
            .iter()
            .position(|face| !(face.normal.is_finite() && face.offset.is_finite()))
        {
            return Err(format!("face {f}'s plane is not finite"));
        }
        if self.faces.len() < 4 {
            return Err(format!("{} faces bound no solid", self.faces.len()));
        }
        let mut edges: BTreeMap<(usize, usize), usize> = BTreeMap::new();
        let mut owner: BTreeMap<(usize, usize), usize> = BTreeMap::new();
        for (f, face) in self.faces().iter().enumerate() {
            let ring = &self.faces[f].corners;
            if ring.len() < 3 || (face.normal.length() - 1.0).abs() > 1.0e-9 {
                return Err(format!(
                    "face {f} has {} corners or no unit normal",
                    ring.len()
                ));
            }
            if face.area() <= tolerance * tolerance {
                return Err(format!(
                    "face {f} has area {} about its normal: reversed or degenerate",
                    face.area()
                ));
            }
            for (k, &p) in face.corners.iter().enumerate() {
                if (face.normal.dot(p) - face.offset).abs() > tolerance {
                    return Err(format!("face {f}'s corner {k} is off its plane"));
                }
            }
            for k in 0..ring.len() {
                let edge = (ring[k], ring[(k + 1) % ring.len()]);
                *edges.entry(edge).or_default() += 1;
                owner.insert(edge, f);
            }
            for (i, &p) in self.corners.iter().enumerate() {
                if face.normal.dot(p) - face.offset > tolerance {
                    return Err(format!("corner {i} lies outside face {f}: not convex"));
                }
            }
        }
        for (&(i, j), &count) in &edges {
            if count != 1 || edges.get(&(j, i)) != Some(&1) {
                return Err(format!("edge {i} → {j} is not shared by exactly two faces"));
            }
        }
        let mut shell = vec![false; self.faces.len()];
        let mut stack = vec![0];
        shell[0] = true;
        while let Some(f) = stack.pop() {
            let ring = &self.faces[f].corners;
            for k in 0..ring.len() {
                let twin = owner[&(ring[(k + 1) % ring.len()], ring[k])];
                if !std::mem::replace(&mut shell[twin], true) {
                    stack.push(twin);
                }
            }
        }
        if let Some(f) = shell.iter().position(|&reached| !reached) {
            return Err(format!(
                "face {f} lies on a second shell, unreached from face 0 across edges"
            ));
        }
        if self.volume() <= 0.0 {
            return Err(format!("volume {} is not positive", self.volume()));
        }
        Ok(())
    }
}

/// `points` (indices into `corners`, on one plane of unit normal `n`, the corners of a convex
/// polygon) ordered counter-clockwise about `n`, by half-plane then cross product about their
/// mean: exact comparisons, no angle.
fn counter_clockwise(corners: &[DVec3], mut points: Vec<usize>, n: DVec3) -> Vec<usize> {
    let (e1, e2) = plane_basis(n);
    let mean = points.iter().map(|&i| corners[i]).sum::<DVec3>() / points.len() as f64;
    let flat = |i: usize| {
        let p = corners[i] - mean;
        DVec2::new(p.dot(e1), p.dot(e2))
    };
    let upper = |p: DVec2| p.y > 0.0 || (p.y == 0.0 && p.x > 0.0);
    points.sort_by(|&a, &b| {
        let (pa, pb) = (flat(a), flat(b));
        upper(pb)
            .cmp(&upper(pa))
            .then_with(|| 0.0_f64.total_cmp(&pa.perp_dot(pb)))
    });
    points
}

#[cfg(test)]
pub(crate) mod tests {
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
        let centre = prism.solid().centroid();
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
        prism
            .solid()
            .certify(1e-12)
            .expect("a prism is a closed convex solid");
    }

    /// FIXTURE DATA: the axis-aligned box `lo..hi` as a prism's solid.
    pub(crate) fn cuboid(lo: DVec3, hi: DVec3) -> Polyhedron {
        let size = hi - lo;
        Prism {
            frame: Frame {
                origin: lo,
                u: DVec3::X,
                v: DVec3::Y,
            },
            profile: vec![
                DVec2::ZERO,
                DVec2::new(size.x, 0.0),
                DVec2::new(size.x, size.y),
                DVec2::new(0.0, size.y),
            ],
            near: 0.0,
            far: size.z,
        }
        .solid()
    }

    /// FIXTURE DATA: a pentagonal prism in a frame off every axis, about `origin`, at `scale`.
    fn oblique(origin: DVec3, scale: f64) -> Polyhedron {
        Prism {
            frame: Frame {
                origin,
                u: DVec3::new(0.8, 0.0, -0.6),
                v: DVec3::new(0.0, 1.0, 0.0),
            },
            profile: [(0.0, 0.0), (1.2, 0.0), (1.5, 0.7), (0.6, 1.3), (-0.2, 0.8)]
                .map(|(x, y)| DVec2::new(x, y) * scale)
                .to_vec(),
            near: -0.4 * scale,
            far: 0.5 * scale,
        }
        .solid()
    }

    /// The cut face is the face a clip adds: the one on the cutting plane.
    fn cut_face(solid: &Polyhedron, n: DVec3, d: f64) -> Face {
        solid
            .faces()
            .into_iter()
            .find(|face| face.normal.dot(n) > 1.0 - 1e-12 && (face.offset - d).abs() < 1e-12)
            .expect("a clip that cut adds a face on its plane")
    }

    /// A CLIP IS THE SOLID CALCULATED BY HAND: the unit cube cut to `x + y + z ≤ 1` is the corner
    /// tetrahedron (volume 1/6, centroid (¼, ¼, ¼), a cut face of area √3/2), and cut to `≤ 1.5`
    /// is the half cube whose cut face is the regular hexagon of side √½ (area 3√3/4). Both are
    /// closed convex solids. A clip that lost volume, kept the wrong side, wound the cut face
    /// inward or dropped a face fails here. Expectations DERIVED by hand, independent of the code.
    #[test]
    fn a_clip_is_the_solid_calculated_by_hand() {
        let cube = cuboid(DVec3::ZERO, DVec3::ONE);
        let n = DVec3::ONE / 3.0_f64.sqrt();
        let corner = cube.clip(DVec3::ONE, 1.0).expect("the corner is inside");
        corner.certify(1e-12).unwrap();
        assert_eq!((corner.corners.len(), corner.faces.len()), (4, 4));
        assert!((corner.volume() - 1.0 / 6.0).abs() < 1e-14);
        assert!((corner.centroid() - DVec3::splat(0.25)).length() < 1e-14);
        let face = cut_face(&corner, n, 1.0 / 3.0_f64.sqrt());
        assert!((face.area() - 3.0_f64.sqrt() / 2.0).abs() < 1e-14);
        let half = cube.clip(DVec3::ONE, 1.5).expect("half the cube is inside");
        half.certify(1e-12).unwrap();
        assert!((half.volume() - 0.5).abs() < 1e-14);
        let face = cut_face(&half, n, 1.5 / 3.0_f64.sqrt());
        assert_eq!(face.corners.len(), 6);
        assert!((face.area() - 3.0 * 3.0_f64.sqrt() / 4.0).abs() < 1e-14);
    }

    /// A SPLIT PARTITIONS: its two parts are closed convex solids, each on its own side of the
    /// plane (no overlap), their volumes and first moments sum to the whole's, and their cut
    /// faces are one polygon, the same corners to the bit, facing opposite ways. The contact
    /// search relies on that shared face to find the interface; a part that kept the other side,
    /// lost a sliver, or computed its crossings apart from its twin fails here.
    #[test]
    fn a_split_partitions_the_solid_and_its_parts_share_one_face() {
        let solid = oblique(DVec3::new(1.0, -2.0, 0.5), 1.0);
        let n = DVec3::new(0.3, 1.0, -0.2);
        let d = n.dot(solid.centroid()) + 0.05;
        let (below, above) = solid.split(n, d);
        let (below, above) = (below.unwrap(), above.unwrap());
        for part in [&below, &above] {
            part.certify(1e-12).unwrap();
        }
        assert!(below.corners.iter().all(|p| n.dot(*p) <= d + 1e-12));
        assert!(above.corners.iter().all(|p| n.dot(*p) >= d - 1e-12));
        let whole = solid.volume();
        assert!((below.volume() + above.volume() - whole).abs() < 1e-12 * whole);
        let moment = below.centroid() * below.volume() + above.centroid() * above.volume();
        assert!((moment - solid.centroid() * whole).length() < 1e-12);
        let unit = n.normalize();
        let (fb, fa) = (
            cut_face(&below, unit, d / n.length()),
            cut_face(&above, -unit, -d / n.length()),
        );
        let mut reversed = fa.corners;
        reversed.reverse();
        let start = reversed
            .iter()
            .position(|p| *p == fb.corners[0])
            .expect("the twin's corners are the same bits");
        reversed.rotate_left(start);
        assert_eq!(fb.corners, reversed);
    }

    /// NEIGHBOURING CELLS CUT BY ONE PLANE MEET AT THE SAME BITS: two unit boxes sharing the face
    /// `x = 1`, stated by prisms that number its corners differently, each cut by one oblique
    /// plane across that face, hold the same corners on it, bit for bit. The cutter's cells join
    /// over exactly shared corners; a crossing taken from whichever end a cell numbered first
    /// drifts by a rounding between neighbours. Fixture data.
    #[test]
    fn neighbouring_cells_cut_by_one_plane_meet_at_the_same_bits() {
        let left = cuboid(DVec3::ZERO, DVec3::ONE);
        let right = Prism {
            frame: Frame {
                origin: DVec3::X,
                u: DVec3::Z,
                v: DVec3::Y,
            },
            profile: vec![
                DVec2::ZERO,
                DVec2::new(1.0, 0.0),
                DVec2::new(1.0, 1.0),
                DVec2::new(0.0, 1.0),
            ],
            near: -1.0,
            far: 0.0,
        }
        .solid();
        for k in 1..40 {
            let t = f64::from(k) / 40.0;
            let n = DVec3::new(0.3 * t, 0.7 - t, 1.0);
            let d = n.dot(DVec3::new(1.0, 1.0, t));
            let shared = |cell: &Polyhedron, out: DVec3, offset: f64| {
                let cell = cell.clip(n, d).unwrap();
                let mut corners: Vec<[u64; 3]> = cut_face(&cell, out, offset)
                    .corners
                    .iter()
                    .map(|p| p.to_array().map(f64::to_bits))
                    .collect();
                corners.sort_unstable();
                corners
            };
            assert_eq!(
                shared(&left, DVec3::X, 1.0),
                shared(&right, DVec3::NEG_X, -1.0),
                "case {k}"
            );
        }
    }

    /// A CUT ON A FACE LEAVES NO SLIVER: a plane within the weld of a face keeps the whole solid
    /// on its side and nothing on the other, so no cell thinner than the clip's rounding exists;
    /// one a micrometre in is a real cut. Mutant: a clip that reads corners a picometre over the
    /// plane as outside writes a zero-thickness cell.
    #[test]
    fn a_cut_on_a_face_leaves_no_sliver() {
        let cube = cuboid(DVec3::ZERO, DVec3::ONE);
        for offset in [1.0 - 1e-12, 1.0, 1.0 + 1e-12] {
            assert_eq!(cube.split(DVec3::Y, offset), (Some(cube.clone()), None));
        }
        let (_, cap) = cube.split(DVec3::Y, 1.0 - 1e-6);
        assert!((cap.expect("a micrometre in cuts").volume() - 1e-6).abs() < 1e-15);
    }

    /// SCALE AND TRANSLATION CARRY THROUGH A CUT: the same cut of a solid scaled by `s` and moved
    /// by `t` has `s³` the volume, `s²` the cut face's area and its centroid at `s·c + t`. A cut
    /// whose crossings or moments depend on where the solid sits fails here. Fixture data.
    #[test]
    fn scale_and_translation_carry_through_a_cut() {
        let (n, d) = (DVec3::new(-0.5, 1.0, 0.25), 0.3);
        let unit = n.normalize();
        let part = oblique(DVec3::ZERO, 1.0).clip(n, d).unwrap();
        let area = cut_face(&part, unit, d / n.length()).area();
        for (s, t) in [
            (2.5, DVec3::new(10.0, -3.0, 7.0)),
            (0.2, DVec3::new(-1.0, 0.5, 2.0)),
        ] {
            let moved = oblique(t, s).clip(n, s * d + n.dot(t)).unwrap();
            moved.certify(1e-12).unwrap();
            assert!((moved.volume() - s.powi(3) * part.volume()).abs() < 1e-12 * moved.volume());
            assert!((moved.centroid() - (part.centroid() * s + t)).length() < 1e-12);
            let face = cut_face(&moved, unit, (s * d + n.dot(t)) / n.length());
            assert!((face.area() - s * s * area).abs() < 1e-12 * face.area());
        }
    }

    /// THE CERTIFICATE REFUSES WHAT NO CELL MAY BE: a face wound inward, a missing face (an open
    /// shell) and a corner pushed off its faces' planes (a dent) each name their breach; the
    /// writer refuses a bake on any of them. Fixture data: the unit cube.
    #[test]
    fn the_certificate_refuses_a_reversed_open_or_dented_solid() {
        let cube = cuboid(DVec3::ZERO, DVec3::ONE);
        cube.certify(1e-9).unwrap();
        let mut reversed = cube.clone();
        reversed.faces[2].corners.reverse();
        assert!(reversed.certify(1e-9).unwrap_err().contains("reversed"));
        let mut open = cube.clone();
        open.faces.pop();
        assert!(
            open.certify(1e-9)
                .unwrap_err()
                .contains("exactly two faces")
        );
        let mut dented = cube;
        dented.corners[0] += DVec3::splat(0.1);
        assert!(dented.certify(1e-9).unwrap_err().contains("off its plane"));
    }

    /// A NAN NEVER CERTIFIES: every ordered test is false on a NaN, so a facet offset, a normal
    /// component or a corner coordinate set to NaN would pass each one. The writer relies on a
    /// certified cell's numbers being real. Mutant: a certificate without the finiteness gate
    /// passes the cube with a NaN offset. Fixture data: the unit cube.
    #[test]
    fn the_certificate_refuses_a_nan_plane_or_corner() {
        let cube = cuboid(DVec3::ZERO, DVec3::ONE);
        let mut offset = cube.clone();
        offset.faces[0].offset = f64::NAN;
        let mut normal = cube.clone();
        normal.faces[3].normal.y = f64::NAN;
        let mut corner = cube;
        corner.corners[5].z = f64::NAN;
        for (case, solid, breach) in [
            ("offset", offset, "plane is not finite"),
            ("normal", normal, "plane is not finite"),
            ("corner", corner, "not finite"),
        ] {
            let err = solid.certify(1e-9).unwrap_err();
            assert!(err.contains(breach), "{case}: {err}");
        }
    }

    /// TWO SHELLS ARE NOT ONE SOLID: a cube's corners and faces repeated under fresh indices close
    /// every edge on its own copy and pass every per-face test, yet their moments sum to twice the
    /// volume, so the writer would bake twice the mass. Mutant: a certificate that checks edges
    /// pairwise but not that one shell holds every face passes it. Fixture data: the unit cube.
    #[test]
    fn the_certificate_refuses_coincident_shells() {
        let cube = cuboid(DVec3::ZERO, DVec3::ONE);
        let mut doubled = cube.clone();
        let n = cube.corners.len();
        doubled.corners.extend(cube.corners.iter().copied());
        doubled.faces.extend(cube.faces.iter().map(|face| Facet {
            corners: face.corners.iter().map(|&i| i + n).collect(),
            ..face.clone()
        }));
        assert!(
            (doubled.volume() - 2.0).abs() < 1e-12,
            "the mass it would bake"
        );
        let err = doubled.certify(1e-9).unwrap_err();
        assert!(err.contains("second shell"), "{err}");
    }
}
