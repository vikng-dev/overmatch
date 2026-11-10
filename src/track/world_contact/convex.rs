//! Convex entry faces and their exact projected coverage in a belt element's frame.
//! Coordinates are `(along, lateral, 1)`; depth planes map them to metres inward from the
//! resting face. Each triangle supplies its own plane; only faces reached from the probe origin
//! enter the support envelope.
//! Each visible owner triangle and the field have at most one positive-pressure patch per
//! column half: at most F+1 patches for F admitted faces. Area, pressure integral and first
//! moment sum over that owner's entire visible region, including disconnected parts. Engagement
//! uses that region's maximum depth. Each point still uses its highest covering triangle's plane.
//! The field wins equal depths; solid ties use snapshot collider order then canonical geometry,
//! independently of face insertion order.
//! See `.agents/scratch/belt-freeze.md` for the captured freeze and representation choice.

use super::{SupportPatch, sample_halves};
use crate::cost::stages::{Detail, Stages, Work};
use crate::track::rubble::{RubbleBodies, Slab};
use bevy::prelude::*;

macro_rules! polygon_ops {
    ($vector:ty, $scalar:ty) => {
        /// Replace `result` with the nonnegative side of an affine half-plane. Homogeneous
        /// footprint coordinates keep clipping, depth comparison and surface interpolation in the
        /// same frame.
        fn clip_into(polygon: &[$vector], plane: $vector, result: &mut Vec<$vector>) {
            result.clear();
            let Some(&last) = polygon.last() else {
                return; // An empty intersection remains empty.
            };
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
        }

        fn signed_area(polygon: &[$vector]) -> $scalar {
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
                .sum::<$scalar>()
                / 2.0
        }
    };
}

polygon_ops!(Vec3, f32);

fn clip(polygon: &[Vec3], plane: Vec3) -> Vec<Vec3> {
    let mut result = Vec::with_capacity(polygon.len() + 1);
    clip_into(polygon, plane, &mut result);
    result
}

mod envelope;

/// Partition buffers for one side's columns, reused across columns and halves.
#[derive(Default)]
pub(crate) struct PartitionScratch {
    envelope: envelope::Scratch,
    cells: Vec<Cell>,
    clipped: Vec<Vec3>,
}

#[cfg(test)]
fn partition(
    a: f32,
    b: f32,
    strip: (f32, f32),
    base: Vec3,
    free: Vec3,
    faces: &[(usize, &ClippedFace)],
) -> Vec<Cell> {
    let mut cells = Vec::new();
    envelope::partition(
        (a, b),
        strip,
        base,
        free,
        faces,
        &mut envelope::Scratch::default(),
        &mut cells,
    );
    let regions = owners(&cells, faces.len(), strip.1 - strip.0);
    assert!(
        regions.len() <= faces.len() + 1,
        "patches exceed the module's per-half owner bound"
    );
    cells
}

#[cfg(test)]
fn owners(cells: &[Cell], faces: usize, width: f32) -> Vec<(Option<usize>, Integral, f32, f32)> {
    integrated_regions(cells, faces, width, &mut Vec::new(), &mut 0)
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ClippedFace {
    pub(crate) polygon: Vec<Vec3>,
    pub(crate) depth: Vec3,
    pub(crate) normal: Vec3,
    planes: Vec<Vec3>,
}

impl ClippedFace {
    /// A rigid shoe rests on the cross-width maximum at each along coordinate. On a convex
    /// face that envelope follows one polygon chain; each edge supplies one linear interval.
    /// Its shadow spans the shoe width only where that interval reaches the probe envelope.
    pub(crate) fn across_shoe(self, slab: &Slab) -> Vec<Self> {
        self.polygon
            .iter()
            .zip(self.polygon.iter().cycle().skip(1))
            .take(self.polygon.len())
            .filter_map(|(a, b)| {
                let along = b.x - a.x;
                // CCW polygons traverse the upper chain toward decreasing along coordinates.
                // Vertical edges have no length; a flat depth may use either chain.
                if along == 0.0
                    || if self.depth.y < 0.0 {
                        along < 0.0
                    } else {
                        along > 0.0
                    }
                {
                    return None;
                }
                let lateral_slope = (b.y - a.y) / along;
                let depth = Vec3::new(
                    self.depth.x + self.depth.y * lateral_slope,
                    0.0,
                    self.depth.z + self.depth.y * (a.y - lateral_slope * a.x),
                );
                let (start, end) = (a.x.min(b.x), a.x.max(b.x));
                let envelope = [Vec3::Z * slab.reach - depth, depth + Vec3::Z * slab.travel];
                let polygon = envelope.iter().fold(
                    vec![
                        Vec3::new(start, slab.lateral.0, 1.0),
                        Vec3::new(end, slab.lateral.0, 1.0),
                        Vec3::new(end, slab.lateral.1, 1.0),
                        Vec3::new(start, slab.lateral.1, 1.0),
                    ],
                    |polygon, plane| clip(&polygon, *plane),
                );
                (signed_area(&polygon) != 0.0).then_some(Self {
                    polygon,
                    depth,
                    normal: self.normal,
                    planes: [Vec3::new(1.0, 0.0, -start), Vec3::new(-1.0, 0.0, end)]
                        .into_iter()
                        .chain(envelope)
                        .collect(),
                })
            })
            .collect()
    }
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
    faces: impl Iterator<Item = (usize, &'a ClippedFace)>,
    strip: (f32, f32),
    len: f32,
    pen: [f32; 3],
    travel: [f32; 3],
) -> Vec<(usize, &'a ClippedFace)> {
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
    pub(super) collider: Entity,
    pub(super) normal: Vec3,
    pub(super) point: Vec3,
}

#[derive(Clone)]
pub(super) struct Integral {
    pub(super) area: f32,
    pub(super) length: f32,
    pub(super) centroid: Vec3,
    pub(super) max_depth: f32,
    /// The field region's visible polygons, for curved-field slicing. A face region holds none:
    /// its patch keeps its own plane and is never sliced (`super::terrain_surface_patches`).
    polygons: Vec<Vec<Vec3>>,
    depth: Vec3,
    width: f32,
}

impl Integral {
    fn rebase(&mut self, start: f32) {
        for p in self.polygons.iter_mut().flatten() {
            p.x -= start;
        }
        self.depth.z += self.depth.x * start;
        self.centroid.x -= start;
    }

    fn add(&mut self, other: Self) {
        let area = self.area + other.area;
        self.centroid = (self.centroid * self.area + other.centroid * other.area) / area;
        self.area = area;
        self.length += other.length;
        self.max_depth = self.max_depth.max(other.max_depth);
        self.polygons.extend(other.polygons);
    }

    pub(super) fn slice(&self, start: f32, len: f32) -> Option<Self> {
        assert!(
            !self.polygons.is_empty(),
            "only a field region is sliced; a face patch keeps its own plane"
        );
        let mut result: Option<Self> = None;
        for polygon in &self.polygons {
            let polygon = clip(
                &clip(polygon, Vec3::new(1.0, 0.0, -start)),
                Vec3::new(-1.0, 0.0, start + len),
            );
            if let Some(part) = integrate_field(&polygon, self.depth, self.width) {
                match &mut result {
                    Some(result) => result.add(part),
                    None => result = Some(part),
                }
            } // This polygon may have no positive pressure in the slice.
        }
        let mut result = result?;
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
        polygons: Vec::new(),
        depth,
        width,
    })
}

/// [`integrate`], keeping the polygon for slicing.
fn integrate_field(polygon: &[Vec3], depth: Vec3, width: f32) -> Option<Integral> {
    integrate(polygon, depth, width).map(|mut integral| {
        integral.polygons.push(polygon.to_vec());
        integral
    })
}

/// One sweep band: a quadrilateral between two boundaries, owned by a face or the field.
#[derive(Clone, Copy)]
struct Cell {
    polygon: [Vec3; 4],
    depth: Vec3,
    face: Option<usize>,
}

/// Group decomposition bands before sampling support: a face's disconnected visible parts
/// share exact integrals and one engagement maximum.
/// `polygon` is a reused clipping buffer; `allocs` counts the field polygons kept per band.
fn integrated_regions(
    cells: &[Cell],
    faces: usize,
    width: f32,
    polygon: &mut Vec<Vec3>,
    allocs: &mut usize,
) -> Vec<(Option<usize>, Integral, f32, f32)> {
    let mut regions: Vec<Option<(Integral, f32, f32)>> =
        std::iter::repeat_with(|| None).take(faces + 1).collect();
    for cell in cells {
        clip_into(&cell.polygon, cell.depth, polygon);
        let integral = match cell.face {
            Some(_) => integrate(polygon, cell.depth, width),
            None => integrate_field(polygon, cell.depth, width),
        };
        let Some(integral) = integral else {
            continue; // No positive pressure remains in this band.
        };
        *allocs += usize::from(cell.face.is_none());
        let lo = polygon.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
        let hi = polygon
            .iter()
            .map(|p| p.x)
            .fold(f32::NEG_INFINITY, f32::max);
        let slot = &mut regions[cell.face.map_or(0, |i| i + 1)];
        match slot {
            Some((sum, start, end)) => {
                sum.add(integral);
                *start = start.min(lo);
                *end = end.max(hi);
            }
            None => *slot = Some((integral, lo, hi)),
        }
    }
    regions
        .into_iter()
        .enumerate()
        .filter_map(|(i, r)| r.map(|(integral, lo, hi)| (i.checked_sub(1), integral, lo, hi)))
        .collect()
}

/// Integrate the entry faces' upper envelope over each field half-profile. Each band owns the deepest
/// surface; clipping at face edges and depth crossings prevents both holes and double support.
pub(super) fn patches(
    query: &super::PatchQuery,
    faces: &[(usize, &ClippedFace)],
    pieces: &RubbleBodies,
    scratch: &mut PartitionScratch,
) -> Vec<SupportPatch> {
    let super::PatchQuery {
        slab,
        strip,
        field_offset,
        pen,
        travel,
        ..
    } = *query;
    let mut result = Vec::new();
    for half in 0..2 {
        let (a, b) = (
            half as f32 * slab.len / 2.0,
            (half + 1) as f32 * slab.len / 2.0,
        );
        let free = half_plane(slab.len, travel, half);
        let bracket = Stages::detail(pieces.stages.as_ref(), Detail::BeltPartition);
        envelope::partition(
            (a, b),
            strip,
            half_plane(slab.len, pen, half),
            free,
            faces,
            &mut scratch.envelope,
            &mut scratch.cells,
        );
        drop(bracket);
        pieces.count(Work::BeltPartitionCells, scratch.cells.len());
        let regions = integrated_regions(
            &scratch.cells,
            faces.len(),
            slab.lateral.1 - slab.lateral.0,
            &mut scratch.clipped,
            &mut scratch.envelope.counts.allocs,
        );
        pieces.count(Work::BeltPartitionPatches, regions.len());
        for (owner, mut integral, lo, hi) in regions {
            let depth = owner.map_or(half_plane(slab.len, pen, half), |i| faces[i].1.depth + free);
            let face = owner.map(|i| {
                let (piece, face) = faces[i];
                ContactFace {
                    collider: pieces.collider(piece),
                    normal: face.normal,
                    // The projected pressure centroid on the face's plane. Reintegrating world
                    // triangles would square a sliver's area in a vector norm, which can underflow.
                    point: slab.origin
                        + slab.axis * integral.centroid.x
                        + slab.lat * integral.centroid.y
                        + slab.out * (slab.face - face.depth.dot(integral.centroid)),
                }
            });
            integral.rebase(lo);
            result.push(SupportPatch {
                origin: slab.origin + slab.axis * lo + slab.lat * field_offset,
                len: hi - lo,
                weight: 1.0,
                pen: [lo, (lo + hi) / 2.0, hi].map(|x| depth.dot(Vec3::new(x, field_offset, 1.0))),
                travel: [lo, (lo + hi) / 2.0, hi].map(|x| sample_halves(x, slab.len, travel)),
                face,
                integral: Some(integral),
            });
        }
    }
    let counts = std::mem::take(&mut scratch.envelope.counts);
    for (work, count) in [
        (Work::BeltDiscoveryTests, counts.tests),
        (Work::BeltPartitionEdges, counts.edges),
        (Work::BeltSweepIntervals, counts.intervals),
        (Work::BeltSweepPairs, counts.pairs),
        (Work::BeltSweepEdgeVisits, counts.edge_visits),
        (Work::BeltOwnerTests, counts.owner_tests),
        (Work::BeltPartitionAllocs, counts.allocs),
    ] {
        pieces.count(work, count);
    }
    result
}

#[cfg(test)]
mod tests {

    use super::*;

    // Fixture: affine surfaces over the unit footprint, in homogeneous coordinates.
    fn unit_face(depth: Vec3) -> ClippedFace {
        let polygon = vec![
            Vec3::Z,
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::ONE,
            Vec3::new(0.0, 1.0, 1.0),
        ];
        let planes = polygon
            .iter()
            .zip(polygon.iter().cycle().skip(1))
            .take(4)
            .map(|(a, b)| a.cross(*b))
            .collect();
        ClippedFace {
            polygon,
            planes,
            depth,
            normal: Vec3::Y,
        }
    }

    // Keep a fixture's polygon and constraints consistent, as the production constructor does.
    fn restrict(face: &mut ClippedFace, plane: Vec3) {
        face.polygon = clip(&face.polygon, plane);
        face.planes.push(plane);
    }

    /// Suspension reads the envelope's own pressure moments. Removing the interior oblique
    /// crossing assigns one plane across a region where the other is higher. On one collider
    /// the faces stand for a fanned, non-planar outline folded over itself: a mutant skipping
    /// every crossing within a collider, not just across a shared edge, fails that case.
    #[test]
    fn interior_oblique_crossing_has_analytic_moments() {
        for pieces in [(0, 1), (0, 0)] {
            let faces = [
                unit_face(Vec3::new(1.0, 1.0, 1.0)),
                unit_face(Vec3::new(-1.0, -1.0, 2.0)),
            ];
            for refs in [
                vec![(pieces.0, &faces[0]), (pieces.1, &faces[1])],
                vec![(pieces.1, &faces[1]), (pieces.0, &faces[0])],
            ] {
                let cells = partition(0.0, 1.0, (0.0, 1.0), Vec3::ZERO, Vec3::ZERO, &refs);
                let (area, pressure, moment) = moments(&cells, 1.0);
                // DERIVED: integrate 1+x+y over the square, then add 1-2x-2y over x+y<1/2.
                // Resolution: unit-scale f32 integration roundoff.
                assert!((area - 1.0).abs() < 1e-6);
                assert!((pressure - 49.0 / 24.0).abs() < 1e-6);
                assert!(
                    (moment - Vec3::new(209.0 / 192.0, 209.0 / 192.0, 49.0 / 24.0)).length() < 1e-6
                );
            }
        }
    }

    /// Loose-piece shadows must replace each other inside a half. Sampling must not carry
    /// an owner chosen on their tie boundary across the other shadow's visible region. Shadows
    /// of one loose piece overlap too: a mutant skipping crossings within a collider loses theirs.
    #[test]
    fn crossing_shoe_shadows_close_lateral_bands_in_both_orders() {
        // Fixture: dyadic and oblique f32 crossings, with exact and non-dyadic shoe edges.
        for (intercept, pieces) in [2.0, 1.74, 1.26]
            .into_iter()
            .flat_map(|intercept| [(intercept, (0, 1)), (intercept, (0, 0))])
        {
            for strip in [(0.0, 1.0), (-0.34611118, 0.3786031)] {
                let (mut slab, _, _) = partition_fixture(false);
                slab.len = 2.0;
                slab.lateral = strip;
                slab.reach = 4.0;
                let faces: Vec<_> = [
                    unit_face(Vec3::new(1.0, 0.0, 1.0)),
                    unit_face(Vec3::new(-1.0, 0.0, intercept)),
                ]
                .into_iter()
                .flat_map(|f| f.across_shoe(&slab))
                .collect();
                let crossing = (intercept - 1.0) / 2.0;
                for refs in [
                    vec![(pieces.0, &faces[0]), (pieces.1, &faces[1])],
                    vec![(pieces.1, &faces[1]), (pieces.0, &faces[0])],
                ] {
                    let cells = partition(0.0, 1.0, strip, Vec3::ZERO, Vec3::ZERO, &refs);
                    let (area, pressure, moment) = moments(&cells, strip.1 - strip.0);
                    // DERIVED: integrate 1+x, then add 2(c-x) on [0,c].
                    let expected = 1.5 + crossing * crossing;
                    let mx = 5.0 / 6.0 + crossing * crossing * crossing / 3.0;
                    // Resolution: unit-scale f32 integration roundoff.
                    assert!((area - 1.0).abs() < 1e-6);
                    assert!((pressure - expected).abs() < 1e-6);
                    assert!(
                        (moment - Vec3::new(mx, expected * (strip.0 + strip.1) / 2.0, expected))
                            .length()
                            < 1e-6
                    );
                    for (x, owner) in [(crossing / 2.0, 1), ((1.0 + crossing) / 2.0, 0)] {
                        let p = Vec3::new(x, (strip.0 + strip.1) / 2.0, 1.0);
                        let cell = cells
                            .iter()
                            .find(|c| contains_point(&c.polygon, p, 0.0))
                            .unwrap();
                        assert!(std::ptr::eq(refs[cell.face.unwrap()].1, &faces[owner]));
                    }
                }
            }
        }
    }

    /// A solid tied with the field contributes no raised support. Choosing the solid on equality
    /// would make the reaction recipient depend on an otherwise invisible coincident face.
    #[test]
    fn field_wins_an_equal_depth_tie() {
        let face = unit_face(Vec3::Z);
        let cells = partition(0.0, 1.0, (0.0, 1.0), Vec3::Z, Vec3::ZERO, &[(0, &face)]);
        assert!(cells.iter().all(|c| c.face.is_none()));
    }

    /// A triangulated surface supplies the shared edge twice. Contact sampling must still
    /// visit each visible triangle once; duplicated clipping planes cannot multiply contacts.
    #[test]
    fn shared_triangle_edges_do_not_multiply_patches() {
        // Fixture: two flat triangles tiling the unit square along x+y=1.
        let mut faces = [unit_face(Vec3::Z), unit_face(Vec3::Z)];
        restrict(&mut faces[0], Vec3::new(-1.0, -1.0, 1.0));
        restrict(&mut faces[1], Vec3::new(1.0, 1.0, -1.0));
        for duplicate in [false, true] {
            if duplicate {
                for f in &mut faces {
                    f.planes.extend(f.planes.clone());
                }
            }
            for refs in [
                vec![(0, &faces[0]), (0, &faces[1])],
                vec![(0, &faces[1]), (0, &faces[0])],
            ] {
                let cells = partition(0.0, 1.0, (0.0, 1.0), Vec3::ZERO, Vec3::ZERO, &refs);
                let regions = owners(&cells, refs.len(), 1.0);
                // DERIVED: both triangles have positive visible area; no field remains.
                assert_eq!(regions.len(), 2);
                let pressure: f32 = regions.iter().map(|(_, i, _, _)| i.area).sum();
                let moment: Vec3 = regions.iter().map(|(_, i, _, _)| i.centroid * i.area).sum();
                // Resolution: unit-scale f32 moment accumulation.
                assert!((pressure - 1.0).abs() < 1e-6);
                assert!((moment - Vec3::new(0.5, 0.5, 1.0)).length() < 1e-6);
            }
        }
    }

    /// The same coplanar solids must expose the same owner regions under input reversal.
    /// Snapshot collider and geometry order break solid ties; the field retains priority.
    #[test]
    fn coplanar_nested_faces_keep_their_owner_under_reversal() {
        // Fixture: an inner face covers the lower-left quarter, on the same or an earlier collider.
        // Its canonical geometry also sorts before the outer square.
        let outer = unit_face(Vec3::Z);
        let mut inner = unit_face(Vec3::Z);
        for plane in [Vec3::new(-1.0, 0.0, 0.5), Vec3::new(0.0, -1.0, 0.5)] {
            restrict(&mut inner, plane);
        }
        for outer_piece in [0, 1] {
            for refs in [
                vec![(0, &inner), (outer_piece, &outer)],
                vec![(outer_piece, &outer), (0, &inner)],
            ] {
                let cells = partition(0.0, 1.0, (0.0, 1.0), Vec3::ZERO, Vec3::ZERO, &refs);
                let regions = owners(&cells, refs.len(), 1.0);
                // DERIVED: the inner face owns 1/4; the outer face owns the remaining 3/4.
                assert_eq!(regions.len(), 2);
                for (owner, integral, _, _) in regions {
                    let is_inner = std::ptr::eq(refs[owner.unwrap()].1, &inner);
                    let expected = if is_inner { 0.25 } else { 0.75 };
                    // Resolution: unit-scale f32 area accumulation.
                    assert!((integral.length - expected).abs() < 1e-6);
                }
            }
        }
    }

    /// Sampling and engagement operate once per visible face region. A partly hidden face
    /// and its cover each contribute one patch, regardless of decomposition or input order.
    #[test]
    fn partly_hidden_face_has_one_patch_for_each_visible_owner() {
        let lower = unit_face(Vec3::Z);
        let mut upper = unit_face(Vec3::Z * 2.0);
        restrict(&mut upper, Vec3::new(1.0, 0.0, -0.5));
        for refs in [
            vec![(0, &lower), (1, &upper)],
            vec![(1, &upper), (0, &lower)],
        ] {
            let cells = partition(0.0, 1.0, (0.0, 1.0), Vec3::ZERO, Vec3::ZERO, &refs);
            let regions = owners(&cells, refs.len(), 1.0);
            // DERIVED: each plane wins one half-square; the field is completely covered.
            assert_eq!(regions.len(), 2);
            let pressure: f32 = regions.iter().map(|(_, i, _, _)| i.area).sum();
            let moment: Vec3 = regions.iter().map(|(_, i, _, _)| i.centroid * i.area).sum();
            // Resolution: unit-scale f32 moment accumulation.
            assert!((pressure - 1.5).abs() < 1e-6);
            assert!((moment - Vec3::new(0.875, 0.75, 1.5)).length() < 1e-6);
            for (owner, i, _, _) in regions {
                assert_eq!(i.max_depth, refs[owner.unwrap()].1.depth.z);
            }
        }
    }

    /// Curved-field quadrature must slice a disconnected owner region without filling its gap.
    /// Keeping only one polygon or replacing the union with a bounding box loses this invariant.
    #[test]
    fn disconnected_region_slicing_preserves_pressure_and_moment() {
        let face = unit_face(Vec3::new(1.0, 0.0, 1.0));
        let mut integral = integrate_field(
            &clip(&face.polygon, Vec3::new(-1.0, 0.0, 0.25)),
            face.depth,
            1.0,
        )
        .unwrap();
        integral.add(
            integrate_field(
                &clip(&face.polygon, Vec3::new(1.0, 0.0, -0.75)),
                face.depth,
                1.0,
            )
            .unwrap(),
        );
        assert_eq!(integral.max_depth, 2.0);
        let left = integral.slice(0.0, 0.5).unwrap();
        let right = integral.slice(0.5, 0.5).unwrap();
        // DERIVED: integral of 1+x on [0,1/4] union [3/4,1] is 3/4;
        // integral of x(1+x) is 43/96. Slicing rebases the right centroid by 1/2.
        // Resolution: unit-scale f32 integration roundoff.
        assert!((left.area + right.area - 0.75).abs() < 1e-6);
        assert!(
            (left.centroid.x * left.area + (right.centroid.x + 0.5) * right.area - 43.0 / 96.0)
                .abs()
                < 1e-6
        );
        assert!((left.length + right.length - 0.5).abs() < 1e-6);
    }

    /// Suspension's integration regions depend on the upper envelope, never on hidden solids.
    /// A mutant retaining an overwritten cell's seams produces more than one rectangle per half.
    #[test]
    fn buried_faces_cannot_fragment_the_upper_envelope() {
        fn face(polygon: Vec<Vec3>, depth: f32) -> ClippedFace {
            let planes = polygon
                .iter()
                .zip(polygon.iter().cycle().skip(1))
                .take(polygon.len())
                .map(|(a, b)| a.cross(*b))
                .collect();
            ClippedFace {
                polygon,
                depth: Vec3::Z * depth,
                normal: Vec3::Y,
                planes,
            }
        }
        // Fixture: a flat rectangular upper face covers many smaller triangular solids.
        let upper = face(
            vec![
                Vec3::Z,
                Vec3::new(2.0, 0.0, 1.0),
                Vec3::new(2.0, 1.0, 1.0),
                Vec3::new(0.0, 1.0, 1.0),
            ],
            0.5,
        );
        let mut faces: Vec<_> = (0..32)
            .map(|i| {
                let x = 0.05 + (i % 8) as f32 * 0.23;
                let y = 0.05 + (i / 8) as f32 * 0.22;
                face(
                    vec![
                        Vec3::new(x, y, 1.0),
                        Vec3::new(x + 0.16, y, 1.0),
                        Vec3::new(x + 0.08, y + 0.15, 1.0),
                    ],
                    0.25,
                )
            })
            .collect();
        faces.push(upper);
        for reverse in [false, true] {
            if reverse {
                faces.reverse();
            }
            let refs: Vec<_> = faces.iter().map(|face| (0, face)).collect();
            let cells: Vec<_> = (0..2)
                .flat_map(|half| {
                    partition(
                        half as f32,
                        (half + 1) as f32,
                        (0.0, 1.0),
                        Vec3::ZERO,
                        Vec3::ZERO,
                        &refs,
                    )
                })
                .collect();
            // DERIVED: a rectangular upper envelope has one integration region per half.
            assert_eq!(cells.len(), 2, "buried boundaries became integration cells");
            let (area, pressure, moment) = moments(&cells, 1.0);
            // Resolution: f32 area and moment accumulation roundoff on this unit-scale plane.
            assert!((area - 2.0).abs() < 1e-6);
            assert!((pressure - 1.0).abs() < 1e-6);
            assert!((moment - Vec3::new(1.0, 0.5, 1.0)).length() < 1e-6);
        }
    }

    /// One column's slab, shoe strip, field and travel profiles, and admitted faces.
    struct Column {
        slab: Slab,
        strip: (f32, f32),
        pen: [f32; 3],
        travel: [f32; 3],
        faces: Vec<(usize, ClippedFace)>,
    }

    /// The first threshold capture in the 30-tank rest-3 rehearsal: one tank's running gear
    /// under another's belt, rebuilt through the production collider constructor.
    fn captured_column() -> Column {
        use crate::track::bump_stop::{TrackColliders, spawn_replica_track_colliders};
        let spec: crate::spec::TankSpec = crate::provenance::from_shipped_str(
            crate::tank::TANK_SPEC_PATH,
            &std::fs::read_to_string(crate::assets::asset_root().join(crate::tank::TANK_SPEC_PATH))
                .unwrap(),
        )
        .unwrap();
        let glb = crate::assets::asset_root().join(crate::tank::TANK_SIM_GLB_PATH);
        let geometry = crate::bake::extract_tank_geometry(
            &glb,
            &spec,
            &crate::substances::SubstanceRegistry::shipped(),
        )
        .unwrap();
        let blueprint =
            crate::bake::TankBlueprint::new(crate::tank::TANK_SPEC_PATH.into(), geometry, spec);
        let shapes = TrackColliders::of_blueprint(&blueprint, &glb);
        let mut world = World::new();
        let body = world.spawn_empty().id();
        spawn_replica_track_colliders(&mut world.commands(), body, &shapes);
        world.flush();
        // Fixture: first threshold capture in the 30-tank rest-3 rehearsal. Coordinates and
        // profiles are captured f32 values; shapes come from the production data constructor.
        let slab = Slab {
            origin: Vec3::new(31.904072, 33.64055, -0.7934673),
            axis: Vec3::new(-0.9512535, -0.02233869, -0.30760053),
            lat: Vec3::new(-0.30487394, 0.21876416, 0.9269272),
            out: Vec3::new(-0.046576746, -0.9755218, 0.21491349),
            len: 0.2636633,
            lateral: (-0.34611118, 0.3786031),
            face: 0.02463995,
            reach: 0.5,
            travel: 0.17256261,
        };
        let strip = (-0.22532547, 0.2578174);
        let pen = [-0.5257142, -0.5189536, -0.5121931];
        let travel = [0.17256261; 3];
        let position = Vec3::new(30.766592, 32.89622, 2.4966617);
        let rotation = Quat::from_xyzw(-0.044458035, -0.41336578, -0.019702647, 0.90926576);
        let mut faces = Vec::new();
        for (index, collider) in world
            .query::<&avian3d::prelude::Collider>()
            .iter(&world)
            .enumerate()
        {
            let triangles: Vec<_> = triangles(collider, rotation)
                .into_iter()
                .map(|tri| tri.map(|p| p + position))
                .collect();
            let clipped = clip_faces(&triangles, &slab);
            faces.extend(clipped.into_iter().map(|face| (index, face)));
        }
        let faces = raised_faces(
            faces.iter().map(|(piece, face)| (*piece, face)),
            strip,
            slab.len,
            pen,
            travel,
        )
        .into_iter()
        .map(|(piece, face)| (piece, face.clone()))
        .collect();
        Column {
            slab,
            strip,
            pen,
            travel,
            faces,
        }
    }

    /// Suspension must integrate the captured upper surface independently of face order.
    /// A mutant retaining hidden crossings or assigning a remote plane breaks coverage or moments.
    #[test]
    fn captured_running_gear_envelope() {
        let Column {
            slab,
            strip,
            pen,
            travel,
            faces,
        } = captured_column();
        let faces: Vec<_> = faces.iter().map(|(piece, face)| (*piece, face)).collect();
        let width = slab.lateral.1 - slab.lateral.0;
        let mut expected_patches = None;
        let mut expected: Option<(f32, f32, Vec3)> = None;
        for ordered in [faces.clone(), faces.iter().rev().copied().collect()] {
            let mut patches = 0;
            let cells: Vec<_> = (0..2)
                .flat_map(|half| {
                    let cells = partition(
                        half as f32 * slab.len / 2.0,
                        (half + 1) as f32 * slab.len / 2.0,
                        strip,
                        half_plane(slab.len, pen, half),
                        half_plane(slab.len, travel, half),
                        &ordered,
                    );
                    patches += owners(&cells, ordered.len(), width).len();
                    cells
                })
                .collect();
            if let Some(expected) = expected_patches {
                assert_eq!(
                    patches, expected,
                    "visible owner count depends on face order"
                );
            } else {
                expected_patches = Some(patches);
            }
            let actual = moments(&cells, width);
            // Resolution: accumulated f32 polygon and pressure-moment roundoff in this column.
            if let Some((area, pressure, moment)) = expected {
                assert!((actual.0 - area).abs() < 1e-5);
                assert!((actual.1 - pressure).abs() < 1e-5);
                assert!((actual.2 - moment).length() < 1e-5);
            } else {
                expected = Some(actual);
            }
            let coverage: f32 = cells.iter().map(|c| signed_area(&c.polygon)).sum();
            assert!((coverage - slab.len * (strip.1 - strip.0)).abs() < 1e-5);
            // Fixture: interior samples avoid the authored half boundary and shared edge ties.
            for x in 0..13 {
                for y in 0..11 {
                    let p = Vec3::new(
                        (x as f32 + 0.37) * slab.len / 13.0,
                        strip.0 + (y as f32 + 0.29) * (strip.1 - strip.0) / 11.0,
                        1.0,
                    );
                    let half = usize::from(p.x > slab.len / 2.0);
                    let free = half_plane(slab.len, travel, half);
                    let expected = ordered
                        .iter()
                        .filter(|(_, f)| f.planes.iter().all(|plane| plane.dot(p) >= 0.0))
                        .map(|(_, f)| (f.depth + free).dot(p))
                        .fold(half_plane(slab.len, pen, half).dot(p), f32::max);
                    let actual = cells
                        .iter()
                        .filter(|c| contains_point(&c.polygon, p, 1e-6))
                        .map(|c| c.depth.dot(p))
                        .fold(f32::NEG_INFINITY, f32::max);
                    // Resolution: f32 plane evaluation and projected shared-edge roundoff.
                    assert!(
                        (actual - expected).abs() < 2e-4,
                        "{p:?}: {actual} != {expected}"
                    );
                }
            }
        }
    }

    fn triangles(collider: &avian3d::prelude::Collider, rotation: Quat) -> Vec<[Vec3; 3]> {
        let shape = collider.shape_scaled();
        let (points, indices) = if let Some(cuboid) = shape.as_cuboid() {
            cuboid.to_trimesh()
        } else {
            shape.as_convex_polyhedron().unwrap().to_trimesh()
        };
        indices
            .into_iter()
            .map(|tri| tri.map(|i| rotation * points[i as usize]))
            .collect()
    }

    // Fixture: a rotated box and a convex ring-sampled sphere, both inside a broad column.
    fn partition_fixture(many: bool) -> (Slab, avian3d::prelude::Collider, Quat) {
        use avian3d::prelude::Collider;
        let collider = if many {
            let mut points = vec![Vec3::Z, -Vec3::Z];
            for ring in 1..8 {
                let elevation = std::f32::consts::PI * ring as f32 / 8.0;
                for sector in 0..16 {
                    let angle = std::f32::consts::TAU * sector as f32 / 16.0;
                    points.push(Vec3::new(
                        elevation.sin() * angle.cos(),
                        elevation.sin() * angle.sin(),
                        elevation.cos(),
                    ));
                }
            }
            Collider::convex_hull(points).unwrap()
        } else {
            Collider::cuboid(1.0, 1.0, 1.0)
        };
        let rotation = Quat::from_rotation_x(0.31) * Quat::from_rotation_y(0.47);
        (
            Slab {
                origin: Vec3::new(-2.0, 0.0, 0.0),
                axis: Vec3::X,
                lat: Vec3::Y,
                out: -Vec3::Z,
                len: 4.0,
                lateral: (-2.0, 2.0),
                face: 0.0,
                reach: 2.0,
                travel: 2.0,
            },
            collider,
            rotation,
        )
    }

    fn triangulate(boundary: &[Vec<Vec3>], offset: usize) -> Vec<[Vec3; 3]> {
        boundary
            .iter()
            .flat_map(|face| {
                let mut points = face.clone();
                let count = points.len();
                points.rotate_left(offset % count);
                points[1..]
                    .windows(2)
                    .map(|edge| [points[0], edge[0], edge[1]])
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn column_cells(slab: &Slab, faces: &[ClippedFace]) -> Vec<Cell> {
        let refs: Vec<_> = faces.iter().map(|f| (0, f)).collect();
        (0..2)
            .flat_map(|half| {
                partition(
                    half as f32 * slab.len / 2.0,
                    (half + 1) as f32 * slab.len / 2.0,
                    slab.lateral,
                    Vec3::ZERO,
                    Vec3::Z,
                    &refs,
                )
            })
            .collect()
    }

    fn moments(cells: &[Cell], width: f32) -> (f32, f32, Vec3) {
        cells
            .iter()
            .filter_map(|c| integrate(&clip(&c.polygon, c.depth), c.depth, width))
            .fold((0.0, 0.0, Vec3::ZERO), |(area, pressure, moment), i| {
                (
                    area + i.length,
                    pressure + i.area,
                    moment + i.centroid * i.area,
                )
            })
    }

    /// A face admitted for the left half must not fragment the right half. Belt contacts rely
    /// on remote edge lines leaving the uncovered field cell whole.
    #[test]
    fn a_disjoint_face_keeps_the_cell_whole() {
        // Fixture: a triangle confined to along < 1; its horizontal edge crosses the other half.
        let (mut slab, _, _) = partition_fixture(false);
        slab.len = 2.0;
        slab.lateral = (0.0, 1.0);
        let face = [
            Vec3::new(-2.0, 0.25, 0.5),
            Vec3::new(-1.6, 0.25, 0.5),
            Vec3::new(-1.8, 0.75, 0.5),
        ];
        let faces = clip_faces(&[face], &slab);
        let refs: Vec<_> = faces.iter().map(|f| (0, f)).collect();
        let cells = partition(1.0, 2.0, (0.0, 1.0), Vec3::ZERO, Vec3::Z, &refs);
        assert_eq!(cells.len(), 1);
    }

    /// Suspension depth belongs to each triangle, including adjacent facets within Parry's
    /// face-merge tolerance. A shared normal would move the second facet's interior sample.
    #[test]
    fn each_triangle_owns_its_depth_plane() {
        let (slab, _, _) = partition_fixture(false);
        // Fixture: adjacent facets one degree apart, rotated away from the probe axes.
        let tilt = 1.0_f32.to_radians().tan() / 2.0_f32.sqrt();
        let rotation = Quat::from_rotation_x(0.13) * Quat::from_rotation_y(0.21);
        let boundary = [
            [
                Vec3::new(-1.0, -1.0, 0.0),
                Vec3::new(1.0, -1.0, 0.0),
                Vec3::new(-1.0, 1.0, 0.0),
            ],
            [
                Vec3::new(1.0, -1.0, 0.0),
                Vec3::new(1.0, 1.0, -2.0 * tilt),
                Vec3::new(-1.0, 1.0, 0.0),
            ],
        ]
        .map(|tri| tri.map(|p| rotation * p));
        let faces = clip_faces(&boundary, &slab);
        assert_eq!(faces.len(), boundary.len());
        for (triangle, face) in boundary.iter().zip(&faces) {
            let expected = triangle.iter().sum::<Vec3>() / 3.0;
            let relative = expected - slab.origin;
            let p = Vec3::new(relative.dot(slab.axis), relative.dot(slab.lat), 1.0);
            let actual = slab.origin
                + slab.axis * p.x
                + slab.lat * p.y
                + slab.out * (slab.face - face.depth.dot(p));
            // Resolution: unit-scale f32 projection roundoff; much smaller than the facet offset.
            assert!(
                (actual - expected).length() < 1e-6,
                "{actual} left triangle interior {expected}"
            );
        }
    }

    /// The field and overlapping solid planes must exchange ownership only at their depth
    /// crossings, independently of order and fan diagonals; the suspension reads that envelope.
    #[test]
    fn crossing_faces_preserve_the_positive_depth_envelope() {
        let (slab, _, _) = partition_fixture(false);
        // Fixture: planes with free depths along-2.5 and 1.5-along across the 4 m column.
        // DERIVED: two right triangular profiles, each base/height 1.5; covered length 3,
        // pressure 2 * 1.5^2 / 2 = 2.25, symmetric pressure centroid at along=2, lateral=0.
        let boundary: Vec<_> = [-1.0, 1.0]
            .into_iter()
            .map(|slope| {
                [(-2.0, -2.0), (2.0, -2.0), (2.0, 2.0), (-2.0, 2.0)]
                    .map(|(x, y)| Vec3::new(x, y, slope * x - 1.5))
                    .to_vec()
            })
            .collect();
        for boundary in [triangulate(&boundary, 0), triangulate(&boundary, 1)] {
            let mut faces = clip_faces(&boundary, &slab);
            for reverse in [false, true] {
                if reverse {
                    faces.reverse();
                }
                let cells = column_cells(&slab, &faces);
                let (area, pressure, moment) = moments(&cells, 4.0);
                // Resolution: f32 clipping and moment accumulation roundoff for this fixture.
                let epsilon = 1e-5;
                assert!((area - 3.0).abs() < epsilon);
                assert!((pressure - 2.25).abs() < epsilon);
                assert!((moment - Vec3::new(4.5, 0.0, 2.25)).length() < epsilon);
            }
        }
    }

    // Clipping can leave nearly collapsed edges with unreliable half-plane directions. A finite
    // polygon winding check tests coverage independently, with f64 evaluation of its f32 vertices.
    fn contains_point(polygon: &[Vec3], p: Vec3, epsilon: f32) -> bool {
        let (px, py) = (f64::from(p.x), f64::from(p.y));
        let epsilon = f64::from(epsilon);
        let mut inside = false;
        for (a, b) in polygon
            .iter()
            .zip(polygon.iter().cycle().skip(1))
            .take(polygon.len())
        {
            let (ax, ay) = (f64::from(a.x), f64::from(a.y));
            let (bx, by) = (f64::from(b.x), f64::from(b.y));
            let (dx, dy) = (bx - ax, by - ay);
            let length_squared = dx * dx + dy * dy;
            if length_squared > 0.0 {
                let t = (((px - ax) * dx + (py - ay) * dy) / length_squared).clamp(0.0, 1.0);
                let (x, y) = (px - ax - t * dx, py - ay - t * dy);
                if x * x + y * y <= epsilon * epsilon {
                    return true;
                }
            }
            if (ay > py) != (by > py) && px < ax + (py - ay) * dx / dy {
                inside = !inside;
            }
        }
        inside
    }

    fn assert_depth_envelope(slab: &Slab, faces: &[ClippedFace], cells: &[Cell]) {
        let width = slab.lateral.1 - slab.lateral.0;
        // Resolution: point containment allows f32 shared-edge roundoff in footprint metres.
        let distance_epsilon = 1e-6 * slab.len.max(width);
        // Fixture: offset grid samples avoid shared surface edges and half boundaries.
        for x in 0..13 {
            for y in 0..11 {
                let p = Vec3::new(
                    (x as f32 + 0.37) * slab.len / 13.0,
                    slab.lateral.0 + (y as f32 + 0.29) * width / 11.0,
                    1.0,
                );
                let expected = faces
                    .iter()
                    .filter(|f| f.planes.iter().all(|plane| plane.dot(p) >= 0.0))
                    .map(|f| (f.depth + Vec3::Z).dot(p))
                    .fold(0.0, f32::max);
                let actual = cells
                    .iter()
                    .filter(|c| contains_point(&c.polygon, p, distance_epsilon))
                    .map(|c| c.depth.dot(p))
                    .fold(f32::NEG_INFINITY, f32::max);
                // Resolution: depth-plane evaluation roundoff for the unit-scale fixture.
                assert!(
                    (actual - expected).abs() < 2e-4,
                    "{p:?}: {actual} != {expected}"
                );
            }
        }
    }

    /// Suspension relies on the same covered area and depth envelope regardless of face order
    /// or triangle vertex order. Integrals and interior samples detect holes, double coverage,
    /// and assigning a remote face's depth to uncovered ground.
    #[test]
    fn convex_partition_preserves_coverage_and_depth_under_order() {
        for many in [false, true] {
            let (slab, collider, rotation) = partition_fixture(many);
            let boundary = triangles(&collider, rotation);
            let canonical = clip_faces(&boundary, &slab);
            let width = slab.lateral.1 - slab.lateral.0;
            let expected = canonical
                .iter()
                .filter_map(|f| integrate(&f.polygon, f.depth + Vec3::Z, width))
                .fold((0.0, 0.0, Vec3::ZERO), |(area, pressure, moment), i| {
                    (
                        area + i.length,
                        pressure + i.area,
                        moment + i.centroid * i.area,
                    )
                });
            for boundary in [
                boundary.clone(),
                boundary.iter().map(|[a, b, c]| [*b, *c, *a]).collect(),
            ] {
                let mut faces = clip_faces(&boundary, &slab);
                for reverse in [false, true] {
                    if reverse {
                        faces.reverse();
                    }
                    let cells = column_cells(&slab, &faces);
                    let (area, pressure, moment) = moments(&cells, width);
                    // Resolution: accumulated f32 clipping/integration roundoff, relative to
                    // the column's area and depth moments, not a permitted geometric gap.
                    let epsilon = 2e-4;
                    assert!((area - expected.0).abs() < epsilon);
                    assert!((pressure - expected.1).abs() < epsilon);
                    assert!((moment - expected.2).length() < epsilon);
                    let total: f32 = cells.iter().map(|c| signed_area(&c.polygon).abs()).sum();
                    assert!((total - slab.len * width).abs() < epsilon);
                    assert_depth_envelope(&slab, &canonical, &cells);
                }
            }
        }
    }

    /// The ground partition must keep ground exposed where a loose piece is lower. Fixture:
    /// projected triangle (0,0), (1,0), (1,1), depth = lateral. DERIVED: its cross-width
    /// maximum is along, so ground at depth 1/2 is exposed over half the shoe's length.
    #[test]
    fn a_tilted_loose_faces_shadow_keeps_lower_ground_exposed() {
        let slab = Slab {
            origin: Vec3::ZERO,
            axis: Vec3::X,
            lat: Vec3::Y,
            out: -Vec3::Z,
            len: 1.0,
            lateral: (0.0, 1.0),
            face: 0.0,
            reach: 2.0,
            travel: 0.0,
        };
        let face = ClippedFace {
            polygon: vec![Vec3::Z, Vec3::new(1.0, 0.0, 1.0), Vec3::ONE],
            depth: Vec3::Y,
            normal: Vec3::Z,
            planes: Vec::new(),
        };
        let shadow: Vec<_> = face.across_shoe(&slab).into_iter().collect();
        let above_ground: f32 = shadow
            .iter()
            .map(|face| signed_area(&clip(&face.polygon, face.depth - Vec3::Z * 0.5)))
            .sum();
        assert!(
            (above_ground - 0.5).abs() < 1e-6,
            "the piece suppressed ground over {above_ground} of the unit shoe"
        );
    }

    /// The contact law relies on pressure and its lever arm belonging only to covered area.
    /// Fixture: unit right triangle, depth = along coordinate. DERIVED barycentric integrals:
    /// area 1/2, pressure 1/6, pressure centroid (1/2, 1/4), not the bounding box's centre.
    #[test]
    fn triangular_pressure_has_its_own_coverage_and_centroid() {
        let polygon = [Vec3::Z, Vec3::new(1.0, 0.0, 1.0), Vec3::new(0.0, 1.0, 1.0)];
        let integral = integrate_field(&polygon, Vec3::X, 1.0).unwrap();
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

    /// Fixture: 512 shipped construction chunks, smallest first, rotated and scattered under one
    /// shoe column as separate solid colliders, so their entry faces cover each other densely.
    fn chunk_pile() -> Column {
        fn files(root: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(root).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    files(&path, out);
                } else if path.to_string_lossy().ends_with(".construction.ron") {
                    out.push(path);
                }
            }
        }
        let mut paths = Vec::new();
        files(
            &crate::assets::asset_root().join("constructions"),
            &mut paths,
        );
        paths.sort();
        let mut chunks = Vec::new();
        for path in paths {
            let text = std::fs::read_to_string(path).unwrap();
            chunks.extend(
                crate::construction::schema::ConstructionAsset::from_ron(&text)
                    .unwrap()
                    .chunks,
            );
        }
        chunks.sort_by(|a, b| a.volume_m3.total_cmp(&b.volume_m3));
        // Fixture: a captured shoe's length and width, its probe reach and free travel.
        let (length, width) = (0.13042593, 0.7247143);
        let slab = Slab {
            origin: Vec3::ZERO,
            axis: Vec3::X,
            lat: Vec3::Y,
            out: Vec3::NEG_Z,
            len: length,
            lateral: (-width / 2.0, width / 2.0),
            face: 0.0,
            reach: 0.5,
            travel: 0.17256261,
        };
        let mut faces = Vec::new();
        for (i, chunk) in chunks.into_iter().take(512).enumerate() {
            let t = i as f32;
            let rotation = Quat::from_euler(EulerRot::XYZ, 0.71 * t, 1.13 * t, 2.39 * t);
            let centre = Vec3::from_array(chunk.centroid);
            let collider = avian3d::prelude::Collider::convex_hull(
                chunk
                    .hull
                    .into_iter()
                    .map(|p| Vec3::from_array(p) - centre)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            let at = Vec3::new(
                length * (0.1 + 0.8 * (0.618 * t).fract()),
                width * ((0.414 * t).fract() - 0.5),
                0.05 + 0.04 * (i % 3) as f32,
            );
            let triangles: Vec<_> = triangles(&collider, rotation)
                .into_iter()
                .map(|tri| tri.map(|p| p + at))
                .collect();
            faces.extend(clip_faces(&triangles, &slab).into_iter().map(|f| (i, f)));
        }
        Column {
            strip: slab.lateral,
            pen: [0.0; 3],
            travel: [slab.travel; 3],
            slab,
            faces,
        }
    }

    /// Fixture: a box whose top corners on one diagonal rise 5 mm over its 1 m width. DERIVED:
    /// the top's two hull triangles differ by about 0.8 degrees, inside Parry's 1.5 degree face
    /// merge, so Parry stores one non-planar outline and fans it.
    fn fanned_hull() -> Column {
        let points: Vec<_> = [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)]
            .into_iter()
            .flat_map(|(x, y)| {
                let top = if x == y { 0.505 } else { 0.5 };
                [Vec3::new(x, y, -0.5), Vec3::new(x, y, top)]
            })
            .collect();
        let collider = avian3d::prelude::Collider::convex_hull(points).unwrap();
        let hull = collider.shape_scaled();
        assert_eq!(
            hull.as_convex_polyhedron().unwrap().faces().len(),
            6,
            "the raised top merges into one Parry face"
        );
        let (slab, _, rotation) = partition_fixture(false);
        let faces = clip_faces(&triangles(&collider, rotation), &slab)
            .into_iter()
            .map(|f| (0, f))
            .collect();
        Column {
            strip: slab.lateral,
            pen: [0.0; 3],
            travel: [1.0; 3],
            slab,
            faces,
        }
    }

    /// Each half's patches: half, owner, pressure integral, contact length, first moment of
    /// pressure and maximum depth.
    fn column_patches(
        column: &Column,
        mut scratch: envelope::Scratch,
    ) -> Vec<(usize, Option<usize>, f32, f32, Vec3, f32)> {
        let refs: Vec<_> = column
            .faces
            .iter()
            .map(|(piece, face)| (*piece, face))
            .collect();
        let len = column.slab.len;
        let width = column.slab.lateral.1 - column.slab.lateral.0;
        (0..2)
            .flat_map(|half| {
                let mut cells = Vec::new();
                envelope::partition(
                    (half as f32 * len / 2.0, (half + 1) as f32 * len / 2.0),
                    column.strip,
                    half_plane(len, column.pen, half),
                    half_plane(len, column.travel, half),
                    &refs,
                    &mut scratch,
                    &mut cells,
                );
                owners(&cells, refs.len(), width)
                    .into_iter()
                    .map(move |(owner, i, _, _)| {
                        (
                            half,
                            owner,
                            i.area,
                            i.length,
                            i.centroid * i.area,
                            i.max_depth,
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn fixtures() -> Vec<(&'static str, Column)> {
        let solid = |many| {
            let (slab, collider, rotation) = partition_fixture(many);
            let faces = clip_faces(&triangles(&collider, rotation), &slab)
                .into_iter()
                .map(|f| (0, f))
                .collect();
            Column {
                strip: slab.lateral,
                pen: [0.0; 3],
                travel: [1.0; 3],
                slab,
                faces,
            }
        };
        vec![
            ("captured column", captured_column()),
            ("chunk pile", chunk_pile()),
            ("fanned hull", fanned_hull()),
            ("rotated box", solid(false)),
            ("ring hull", solid(true)),
        ]
    }

    /// Faces across a shared edge are never crossed (`envelope`). Suspension must read the same
    /// owners and moments as when every pair is crossed. Fixtures: the captured tank-on-tank
    /// column, the 512-chunk pile and its shoe shadows, a fanned non-planar hull, the rotated
    /// box and the ring hull. A mutant that also skips a pair sharing no edge, or matches a line
    /// by its points rather than its orientation, changes an owner or a moment here.
    #[test]
    fn skipping_shared_edge_crossings_keeps_every_owner_and_moment() {
        let mut fixtures = fixtures();
        let pile = &fixtures[1].1;
        let shadows = Column {
            slab: Slab { ..pile.slab },
            faces: pile
                .faces
                .iter()
                .flat_map(|(piece, face)| {
                    face.clone()
                        .across_shoe(&pile.slab)
                        .into_iter()
                        .map(|face| (*piece, face))
                })
                .collect(),
            ..*pile
        };
        fixtures.push(("pile shadows", shadows));
        for (name, column) in fixtures {
            let [skipped, crossed] = [
                envelope::Scratch::default(),
                envelope::Scratch::crossing_shared_edges(),
            ]
            .map(|scratch| column_patches(&column, scratch));
            assert!(!crossed.is_empty(), "{name}: the fixture has support");
            assert_eq!(skipped.len(), crossed.len(), "{name}: patch count");
            for (a, b) in skipped.iter().zip(&crossed) {
                assert_eq!((a.0, a.1), (b.0, b.1), "{name}: owner");
                // Resolution: f32 accumulation over a column's bands, far below any patch.
                let epsilon = 1e-6;
                assert!((a.2 - b.2).abs() < epsilon, "{name}: pressure {a:?} {b:?}");
                assert!((a.3 - b.3).abs() < epsilon, "{name}: length {a:?} {b:?}");
                assert!((a.4 - b.4).length() < epsilon, "{name}: moment {a:?} {b:?}");
                assert!((a.5 - b.5).abs() < epsilon, "{name}: depth {a:?} {b:?}");
            }
        }
    }
}
