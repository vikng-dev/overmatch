//! Convex entry faces and their exact projected coverage in a belt element's frame.
//! Coordinates are `(along, lateral, 1)`; depth planes map them to metres inward from the
//! resting face. Only faces reached from the probe origin enter the support envelope.

use super::{SupportPatch, sample_halves};
use crate::track::rubble::{RubbleBodies, Slab};
use bevy::prelude::*;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ClippedFace {
    pub(crate) polygon: Vec<Vec3>,
    pub(crate) depth: Vec3,
    pub(crate) normal: Vec3,
    planes: Vec<Vec3>,
}

/// Keep the nonnegative side of an affine half-plane. Homogeneous footprint coordinates keep
/// clipping, depth comparison and surface interpolation in the same frame.
fn clip(polygon: &[Vec3], plane: Vec3) -> Vec<Vec3> {
    let Some(&last) = polygon.last() else {
        return Vec::new(); // An empty intersection remains empty.
    };
    let mut result = Vec::with_capacity(polygon.len() + 1);
    let mut a = last;
    let mut da = plane.dot(a);
    for &b in polygon {
        let db = plane.dot(b);
        if (da < 0.0 && db >= 0.0) || (da >= 0.0 && db < 0.0) {
            result.push(a + (b - a) * (da / (da - db)));
        }
        if db >= 0.0 {
            result.push(b);
        }
        (a, da) = (b, db);
    }
    result
}

fn signed_area(polygon: &[Vec3]) -> f32 {
    if polygon.len() < 3 {
        return 0.0;
    } // A point or edge has no projected area.
    polygon[1..]
        .windows(2)
        .map(|edge| {
            let a = edge[0] - polygon[0];
            let b = edge[1] - polygon[0];
            a.x * b.y - a.y * b.x
        })
        .sum::<f32>()
        / 2.0
}

pub(crate) fn clip_faces(triangles: &[[Vec3; 3]], slab: &Slab) -> Vec<ClippedFace> {
    let mut faces = Vec::new();
    for &[a, b, c] in triangles {
        let cross = (b - a).cross(c - a);
        let radial = cross.dot(slab.out);
        if radial >= 0.0 {
            continue; // Back faces and grazing faces cannot be entered along this probe direction.
        }
        let normal = cross.normalize();
        let depth = Vec3::new(
            cross.dot(slab.axis) / radial,
            cross.dot(slab.lat) / radial,
            slab.face - cross.dot(a - slab.origin) / radial,
        );
        assert!(
            normal.is_finite() && depth.is_finite(),
            "a finite convex entry face must define a finite unit normal and depth plane"
        );
        let mut polygon: Vec<_> = [a, b, c]
            .map(|p| {
                let r = p - slab.origin;
                Vec3::new(r.dot(slab.axis), r.dot(slab.lat), 1.0)
            })
            .into();
        if signed_area(&polygon) < 0.0 {
            polygon.reverse();
        }
        // Original triangle edges remain valid half-planes even when clipping creates nearly
        // coincident vertices. Reconstructing edges from that polygon can cut away real area.
        let mut planes: Vec<_> = polygon
            .iter()
            .zip(polygon.iter().cycle().skip(1))
            .take(polygon.len())
            .map(|(a, b)| a.cross(*b))
            .collect();
        let envelope = [Vec3::Z * slab.reach - depth, depth + Vec3::Z * slab.travel];
        planes.extend(envelope);
        for plane in [
            Vec3::X,
            Vec3::new(-1.0, 0.0, slab.len),
            Vec3::new(0.0, 1.0, -slab.lateral.0),
            Vec3::new(0.0, -1.0, slab.lateral.1),
            envelope[0],
            envelope[1],
        ] {
            polygon = clip(&polygon, plane);
        }
        let area = signed_area(&polygon);
        if area == 0.0 {
            continue; // A touching edge carries no projected support area.
        }
        if area < 0.0 {
            polygon.reverse();
        }
        faces.push(ClippedFace {
            polygon,
            depth,
            normal,
            planes,
        });
    }
    faces
}

fn half_plane(len: f32, values: [f32; 3], half: usize) -> Vec3 {
    let slope = (values[half + 1] - values[half]) / (len / 2.0);
    Vec3::new(slope, 0.0, values[half] - slope * half as f32 * len / 2.0)
}

fn in_cell(polygon: &[Vec3], strip: (f32, f32), u: (f32, f32)) -> Vec<Vec3> {
    let mut polygon = polygon.to_vec();
    for plane in [
        Vec3::new(1.0, 0.0, -u.0),
        Vec3::new(-1.0, 0.0, u.1),
        Vec3::new(0.0, 1.0, -strip.0),
        Vec3::new(0.0, -1.0, strip.1),
    ] {
        polygon = clip(&polygon, plane);
    }
    polygon
}

/// Ignore solids wholly below the sampled field so they cannot repartition its quadrature.
pub(super) fn raised_faces<'a>(
    faces: impl Iterator<Item = (Option<usize>, &'a ClippedFace)>,
    strip: (f32, f32),
    len: f32,
    pen: [f32; 3],
    travel: [f32; 3],
) -> Vec<(Option<usize>, &'a ClippedFace)> {
    faces
        .filter(|(_, face)| {
            (0..2).any(|half| {
                let polygon = in_cell(
                    &face.polygon,
                    strip,
                    (half as f32 * len / 2.0, (half + 1) as f32 * len / 2.0),
                );
                let depth = face.depth + half_plane(len, travel, half);
                let raised = clip(&clip(&polygon, depth), depth - half_plane(len, pen, half));
                signed_area(&raised).abs() > 0.0
                    && raised.iter().any(|p| {
                        depth.dot(*p) > 0.0 && (depth - half_plane(len, pen, half)).dot(*p) > 0.0
                    })
            })
        })
        .collect()
}

#[derive(Clone, Copy)]
pub(super) struct ContactFace {
    pub(super) collider: Option<Entity>,
    pub(super) normal: Vec3,
    pub(super) point: Vec3,
}

#[derive(Clone)]
pub(super) struct Integral {
    pub(super) area: f32,
    pub(super) length: f32,
    pub(super) centroid: Vec3,
    pub(super) max_depth: f32,
    polygon: Vec<Vec3>,
    depth: Vec3,
    width: f32,
}

impl Integral {
    fn rebase(&mut self, start: f32) {
        for p in &mut self.polygon {
            p.x -= start;
        }
        self.depth.z += self.depth.x * start;
        self.centroid.x -= start;
    }

    pub(super) fn slice(&self, start: f32, len: f32) -> Option<Self> {
        let polygon = clip(
            &clip(&self.polygon, Vec3::new(1.0, 0.0, -start)),
            Vec3::new(-1.0, 0.0, start + len),
        );
        let mut result = integrate(&polygon, self.depth, self.width)?;
        result.rebase(start);
        Some(result)
    }
}

/// Integrate a linear penetration over a convex polygon. Barycentric moments give exact
/// pressure and its centroid; dividing by shoe width retains the law's per-metre units.
fn integrate(polygon: &[Vec3], depth: Vec3, width: f32) -> Option<Integral> {
    if polygon.len() < 3 {
        return None;
    }
    let mut area = 0.0;
    let mut volume = 0.0;
    let mut moment = Vec3::ZERO;
    let max_depth = polygon.iter().map(|p| depth.dot(*p)).fold(0.0, f32::max);
    for edge in polygon[1..].windows(2) {
        let [a, b, c] = [polygon[0], edge[0], edge[1]];
        let da = depth.dot(a).max(0.0);
        let db = depth.dot(b).max(0.0);
        let dc = depth.dot(c).max(0.0);
        let triangle = ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)).abs() / 2.0;
        let sum = da + db + dc;
        area += triangle;
        volume += triangle * sum / 3.0;
        moment += triangle / 12.0 * ((a + b + c) * sum + a * da + b * db + c * dc);
    }
    assert!(
        area.is_finite() && volume.is_finite() && moment.is_finite(),
        "finite clipped geometry must produce finite pressure moments"
    );
    (volume > 0.0).then(|| Integral {
        area: volume / width,
        length: area / width,
        centroid: moment / volume,
        max_depth,
        polygon: polygon.to_vec(),
        depth,
        width,
    })
}

struct Cell<'a> {
    polygon: Vec<Vec3>,
    depth: Vec3,
    face: Option<(Option<usize>, &'a ClippedFace)>,
}

/// Overlay convex entry faces onto each field half-profile. Each disjoint cell owns the deepest
/// surface; clipping at face edges and depth crossings prevents both holes and double support.
pub(super) fn patches(
    slab: &Slab,
    strip: (f32, f32),
    field_offset: f32,
    pen: [f32; 3],
    travel: [f32; 3],
    faces: &[(Option<usize>, &ClippedFace)],
    pieces: &RubbleBodies,
) -> Vec<SupportPatch> {
    let mut result = Vec::new();
    for half in 0..2 {
        let (a, b) = (
            half as f32 * slab.len / 2.0,
            (half + 1) as f32 * slab.len / 2.0,
        );
        let free = half_plane(slab.len, travel, half);
        let mut cells = vec![Cell {
            polygon: vec![
                Vec3::new(a, strip.0, 1.0),
                Vec3::new(b, strip.0, 1.0),
                Vec3::new(b, strip.1, 1.0),
                Vec3::new(a, strip.1, 1.0),
            ],
            depth: half_plane(slab.len, pen, half),
            face: None,
        }];
        for &(piece, face) in faces {
            let depth = face.depth + free;
            let mut next = Vec::new();
            for cell in cells {
                let mut remaining = cell.polygon;
                for plane in face.planes.iter().copied().chain([depth - cell.depth]) {
                    if plane == Vec3::ZERO {
                        continue;
                    } // Coincident planes have one owner, never two.
                    let outside = clip(&remaining, -plane);
                    if signed_area(&outside).abs() > 0.0 {
                        next.push(Cell {
                            polygon: outside,
                            depth: cell.depth,
                            face: cell.face,
                        });
                    }
                    remaining = clip(&remaining, plane);
                    if remaining.is_empty() {
                        break;
                    }
                }
                if signed_area(&remaining).abs() > 0.0 {
                    next.push(Cell {
                        polygon: remaining,
                        depth,
                        face: Some((piece, face)),
                    });
                }
            }
            cells = next;
        }
        for cell in cells {
            let polygon = clip(&cell.polygon, cell.depth);
            let Some(mut integral) =
                integrate(&polygon, cell.depth, slab.lateral.1 - slab.lateral.0)
            else {
                continue; // No positive covered pressure remains in this cell.
            };
            let lo = polygon.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
            let hi = polygon
                .iter()
                .map(|p| p.x)
                .fold(f32::NEG_INFINITY, f32::max);
            let face = cell.face.map(|(piece, face)| ContactFace {
                collider: piece.map(|piece| pieces.collider(piece)),
                normal: face.normal,
                // Use the projected pressure integral directly. Reintegrating world triangles
                // squares a tiny sliver's area in the vector norm and can underflow to zero.
                point: slab.origin
                    + slab.axis * integral.centroid.x
                    + slab.lat * integral.centroid.y
                    + slab.out * (slab.face - face.depth.dot(integral.centroid)),
            });
            integral.rebase(lo);
            result.push(SupportPatch {
                origin: slab.origin + slab.axis * lo + slab.lat * field_offset,
                len: hi - lo,
                weight: 1.0,
                pen: [lo, (lo + hi) / 2.0, hi]
                    .map(|x| cell.depth.dot(Vec3::new(x, field_offset, 1.0))),
                travel: [lo, (lo + hi) / 2.0, hi].map(|x| sample_halves(x, slab.len, travel)),
                face,
                integral: Some(integral),
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The contact law relies on pressure and its lever arm belonging only to covered area.
    /// Fixture: unit right triangle, depth = along coordinate. DERIVED barycentric integrals:
    /// area 1/2, pressure 1/6, pressure centroid (1/2, 1/4), not the bounding box's centre.
    #[test]
    fn triangular_pressure_has_its_own_coverage_and_centroid() {
        let polygon = [Vec3::Z, Vec3::new(1.0, 0.0, 1.0), Vec3::new(0.0, 1.0, 1.0)];
        let integral = integrate(&polygon, Vec3::X, 1.0).unwrap();
        assert!((integral.length - 0.5).abs() < 1e-6);
        assert!((integral.area - 1.0 / 6.0).abs() < 1e-6);
        assert!((integral.centroid - Vec3::new(0.5, 0.25, 1.0)).length() < 1e-6);
        assert!(in_cell(&polygon, (0.75, 1.0), (0.75, 1.0)).is_empty());
        let left = integral.slice(0.0, 0.5).unwrap();
        let right = integral.slice(0.5, 0.5).unwrap();
        assert!((left.area + right.area - integral.area).abs() < 1e-6);
        assert!((left.length + right.length - integral.length).abs() < 1e-6);
    }

    /// Suspension must read the struck plane, not the link's orientation or a remote vertex.
    /// Fixture: a level triangular face beneath an inclined element, with finite probe reach.
    #[test]
    fn clipped_faces_keep_surface_normals_and_the_probe_envelope() {
        let mut slab = Slab {
            origin: Vec3::ZERO,
            axis: Vec3::new(0.0, 0.6, 0.8),
            lat: Vec3::X,
            out: Vec3::new(0.0, -0.8, 0.6),
            len: 0.2,
            lateral: (-0.1, 0.1),
            face: 0.025,
            reach: 0.5,
            travel: 0.1,
        };
        let triangles = [[
            Vec3::new(-1.0, 0.0, -1.0),
            Vec3::new(-1.0, 0.0, 4.0),
            Vec3::new(4.0, 0.0, -1.0),
        ]];
        let faces = clip_faces(&triangles, &slab);
        assert!(!faces.is_empty());
        for face in faces {
            assert_eq!(face.normal, Vec3::Y);
            for p in face.polygon {
                let depth = face.depth.dot(p);
                let world =
                    slab.origin + slab.axis * p.x + slab.lat * p.y + slab.out * (slab.face - depth);
                assert!(world.y.abs() < 1e-6, "point left its actual face: {world}");
                assert!(p.x >= 0.0 && p.x <= slab.len);
                assert!(p.y >= slab.lateral.0 && p.y <= slab.lateral.1);
                assert!(depth <= slab.reach + 1e-6 && depth >= -slab.travel - 1e-6);
            }
        }
        slab.origin.y = -2.0;
        assert!(
            clip_faces(&triangles, &slab).is_empty(),
            "surface behind the probe origin"
        );
        slab.origin.y = 2.0;
        assert!(
            clip_faces(&triangles, &slab).is_empty(),
            "surface beyond free travel"
        );
    }
}
