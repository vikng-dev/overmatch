//! Vertical decomposition of the visible upper envelope over a positive column rectangle.
//! `partition` refuses nonpositive dimensions. Hidden edges never enter the sweep.
//! Work to discover visibility depends on the input faces; integration cells depend only on
//! visible finite edges. With E edges and X proper crossings, at most 1 + 4E + 4X bands close:
//! each endpoint changes at most two neighbouring bands, and each crossing at most four.

use super::{Cell, ClippedFace};
use bevy::math::DVec3;
use bevy::prelude::*;
use std::collections::BTreeMap;

struct Region {
    polygon: Vec<DVec3>,
    planes: Vec<DVec3>,
    depth: DVec3,
    face: usize,
}

#[derive(Clone, Copy)]
struct Edge {
    a: DVec3,
    b: DVec3,
}

impl Edge {
    fn new(a: DVec3, b: DVec3) -> Self {
        if a.x <= b.x {
            Self { a, b }
        } else {
            Self { a: b, b: a }
        }
    }

    fn at(self, x: f64) -> DVec3 {
        self.a + (self.b - self.a) * ((x - self.a.x) / (self.b.x - self.a.x))
    }
}

fn clip(polygon: &[DVec3], plane: DVec3) -> Vec<DVec3> {
    let Some(&last) = polygon.last() else {
        return Vec::new();
    }; // Empty intersection.
    let mut result = Vec::new();
    let (mut a, mut da) = (last, plane.dot(last));
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

fn positive_area(polygon: &[DVec3]) -> bool {
    polygon.len() >= 3
        && polygon[1..]
            .windows(2)
            .map(|e| {
                let a = e[0] - polygon[0];
                let b = e[1] - polygon[0];
                a.x * b.y - a.y * b.x
            })
            .sum::<f64>()
            > 0.0
}

/// Parameter interval of a finite edge covered by a convex region and deeper than its plane.
fn hidden(edge: Edge, depth: DVec3, region: &Region) -> Option<(f64, f64)> {
    let delta = region.depth - depth;
    let (da, db) = (delta.dot(edge.a), delta.dot(edge.b));
    if da <= 0.0 && db <= 0.0 {
        return None;
    } // Equality alone does not hide a boundary.
    let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
    for plane in region.planes.iter().copied().chain([delta]) {
        let (a, b) = (plane.dot(edge.a), plane.dot(edge.b));
        if a < 0.0 && b < 0.0 {
            return None;
        }
        if a < 0.0 {
            lo = lo.max(a / (a - b));
        }
        if b < 0.0 {
            hi = hi.min(a / (a - b));
        }
        if lo >= hi {
            return None;
        }
    }
    Some((lo, hi))
}

fn visible(
    edge: Edge,
    depth: DVec3,
    owners: (usize, usize),
    regions: &[Region],
    edges: &mut Vec<Edge>,
) {
    if edge.a == edge.b {
        return;
    }
    let mut intervals = vec![(0.0, 1.0)];
    for (index, region) in regions.iter().enumerate() {
        if index == owners.0 || index == owners.1 {
            continue;
        }
        let Some((lo, hi)) = hidden(edge, depth, region) else {
            continue;
        }; // No higher cover.
        let mut next = Vec::new();
        for (a, b) in intervals {
            if hi <= a || lo >= b {
                next.push((a, b));
                continue;
            }
            if a < lo {
                next.push((a, lo));
            }
            if hi < b {
                next.push((hi, b));
            }
        }
        intervals = next;
        if intervals.is_empty() {
            return;
        }
    }
    for (a, b) in intervals {
        edges.push(Edge::new(
            edge.a + (edge.b - edge.a) * a,
            edge.a + (edge.b - edge.a) * b,
        ));
    }
}

fn crossing(polygon: &[DVec3], plane: DVec3) -> Option<Edge> {
    let mut points = Vec::new();
    for (&a, &b) in polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .take(polygon.len())
    {
        let (da, db) = (plane.dot(a), plane.dot(b));
        if da == 0.0 {
            points.push(a);
        }
        if (da < 0.0 && db > 0.0) || (da > 0.0 && db < 0.0) {
            points.push(a + (b - a) * (da / (da - db)));
        }
    }
    // Parallel/coincident planes have no depth crossing line.
    if points.len() < 2 || (plane.x == 0.0 && plane.y == 0.0) {
        return None;
    }
    let axis = DVec3::new(-plane.y, plane.x, 0.0);
    points.sort_by(|a, b| axis.dot(*a).total_cmp(&axis.dot(*b)));
    Some(Edge::new(
        points[0],
        *points.last().expect("a crossing has endpoints"),
    ))
}

fn boundaries(regions: &[Region], rectangle: &[DVec3]) -> Vec<Edge> {
    let mut edges: Vec<_> = rectangle
        .iter()
        .zip(rectangle.iter().cycle().skip(1))
        .take(rectangle.len())
        .map(|(&a, &b)| Edge::new(a, b))
        .collect();
    for (i, region) in regions.iter().enumerate() {
        for (&a, &b) in region
            .polygon
            .iter()
            .zip(region.polygon.iter().cycle().skip(1))
            .take(region.polygon.len())
        {
            visible(Edge::new(a, b), region.depth, (i, i), regions, &mut edges);
        }
        for (j, other) in regions[..i].iter().enumerate() {
            let polygon = other
                .planes
                .iter()
                .fold(region.polygon.clone(), |p, &plane| clip(&p, plane));
            if !positive_area(&polygon) {
                continue;
            } // Disjoint projected faces.
            if let Some(edge) = crossing(&polygon, region.depth - other.depth) {
                visible(edge, region.depth, (i, j), regions, &mut edges);
            }
        }
    }
    edges
}

fn events(edges: &[Edge]) -> Vec<f64> {
    let mut events: Vec<_> = edges.iter().flat_map(|e| [e.a.x, e.b.x]).collect();
    for (i, a) in edges.iter().enumerate() {
        for b in &edges[..i] {
            let (lo, hi) = (a.a.x.max(b.a.x), a.b.x.min(b.b.x));
            if lo >= hi {
                continue;
            }
            let (dlo, dhi) = (a.at(lo).y - b.at(lo).y, a.at(hi).y - b.at(hi).y);
            if (dlo < 0.0 && dhi > 0.0) || (dlo > 0.0 && dhi < 0.0) {
                events.push(lo + (hi - lo) * (dlo / (dlo - dhi)));
            }
        }
    }
    events.sort_by(f64::total_cmp);
    events.dedup();
    events
}

fn owner(point: DVec3, base: DVec3, regions: &[Region]) -> Option<usize> {
    let mut best = base.dot(point);
    let mut owner = None;
    for (i, region) in regions.iter().enumerate() {
        let depth = region.depth.dot(point);
        if depth > best && region.planes.iter().all(|p| p.dot(point) >= 0.0) {
            best = depth;
            owner = Some(i);
        }
    }
    owner
}

pub(super) fn partition<'a>(
    a: f32,
    b: f32,
    strip: (f32, f32),
    base: Vec3,
    free: Vec3,
    faces: &[(usize, &'a ClippedFace)],
) -> Vec<Cell<'a>> {
    assert!(
        a < b && strip.0 < strip.1,
        "a support half has positive length and width"
    );
    let rectangle = [
        DVec3::new(a.into(), strip.0.into(), 1.0),
        DVec3::new(b.into(), strip.0.into(), 1.0),
        DVec3::new(b.into(), strip.1.into(), 1.0),
        DVec3::new(a.into(), strip.1.into(), 1.0),
    ];
    let base64 = base.as_dvec3();
    let regions: Vec<_> = faces
        .iter()
        .enumerate()
        .filter_map(|(i, (_, face))| {
            let depth = (face.depth + free).as_dvec3();
            let planes: Vec<_> = face
                .planes
                .iter()
                .map(|p| p.as_dvec3())
                .chain([depth - base64])
                .collect();
            let polygon = planes
                .iter()
                .fold(rectangle.to_vec(), |p, &plane| clip(&p, plane));
            positive_area(&polygon).then_some(Region {
                polygon,
                planes,
                depth,
                face: i,
            })
        })
        .collect();
    // A single higher face can certify burial without constructing any boundary intersections.
    let buried: Vec<_> = regions
        .iter()
        .enumerate()
        .map(|(i, r)| {
            regions.iter().enumerate().any(|(j, other)| {
                i != j
                    && r.polygon.iter().all(|p| {
                        other.planes.iter().all(|plane| plane.dot(*p) >= 0.0)
                            && (other.depth - r.depth).dot(*p) >= 0.0
                    })
                    && (r
                        .polygon
                        .iter()
                        .any(|p| (other.depth - r.depth).dot(*p) > 0.0)
                        || (other.depth == r.depth && j < i))
            })
        })
        .collect();
    let regions: Vec<_> = regions
        .into_iter()
        .zip(buried)
        .filter_map(|(r, buried)| (!buried).then_some(r))
        .collect();
    let edges = boundaries(&regions, &rectangle);
    let events = events(&edges);
    let mut open: BTreeMap<(usize, usize), f64> = BTreeMap::new();
    let mut cells = Vec::new();
    let emit = |key: (usize, usize), start: f64, end: f64, cells: &mut Vec<Cell<'a>>| {
        let (lower, upper) = (edges[key.0], edges[key.1]);
        let corners = [
            lower.at(start),
            lower.at(end),
            upper.at(end),
            upper.at(start),
        ];
        // A convex band's area centroid lies inside it, away from depth ties at its apex.
        let mut moment = DVec3::ZERO;
        let mut area = 0.0;
        for pair in corners[1..].windows(2) {
            let (a, b) = (pair[0] - corners[0], pair[1] - corners[0]);
            let cross = a.x * b.y - a.y * b.x;
            area += cross;
            moment += (corners[0] + pair[0] + pair[1]) * cross / 3.0;
        }
        let polygon: Vec<_> = corners.into_iter().map(|p| p.as_vec3()).collect();
        if area > 0.0 && super::signed_area(&polygon) > 0.0 {
            let owner = owner(moment / area, base64, &regions);
            cells.push(Cell {
                polygon,
                depth: owner.map_or(base, |i| regions[i].depth.as_vec3()),
                face: owner.map(|i| faces[regions[i].face]),
            });
        }
    };
    for xs in events.windows(2) {
        let (x, end) = (xs[0], xs[1]);
        let mid = (x + end) / 2.0;
        let mut active: Vec<_> = edges
            .iter()
            .enumerate()
            .filter(|(_, e)| e.a.x < e.b.x && e.a.x <= x && e.b.x >= end)
            .map(|(i, e)| (i, e.at(mid).y))
            .collect();
        active.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        let mut next = BTreeMap::new();
        for pair in active.windows(2) {
            let [(lower, lo), (upper, hi)] = [pair[0], pair[1]];
            if lo >= hi {
                continue;
            } // Coincident boundaries enclose no area.
            let key = (lower, upper);
            // A lateral boundary changes ownership without changing the band's side keys.
            let interrupted = edges.iter().any(|e| {
                e.a.x == x
                    && e.b.x == x
                    && e.a.y.min(e.b.y) < edges[upper].at(x).y
                    && e.a.y.max(e.b.y) > edges[lower].at(x).y
            });
            let band = if interrupted {
                if let Some(start) = open.remove(&key) {
                    emit(key, start, x, &mut cells);
                }
                x
            } else {
                open.remove(&key).unwrap_or(x)
            };
            next.insert(key, band);
        }
        for (key, start) in open {
            emit(key, start, x, &mut cells);
        }
        open = next;
    }
    for (key, start) in open {
        emit(key, start, b.into(), &mut cells);
    }
    cells
}
