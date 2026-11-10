//! Vertical decomposition of the visible upper envelope over a positive column rectangle.
//! `partition` refuses nonpositive dimensions. Integration bands are internal geometry;
//! `super::integrated_regions` owns the contact bound. Scratch belongs to one column and is
//! reused across its halves; the spatial index admits only overlapping regions to exact tests.

use super::{Cell, ClippedFace};
use bevy::math::{DVec2, DVec3};
use bevy::prelude::*;

polygon_ops!(DVec3, f64);

#[derive(Clone, Copy)]
struct Bounds {
    min: DVec2,
    max: DVec2,
}

impl Bounds {
    fn of(points: impl Iterator<Item = DVec3>) -> Self {
        points.fold(
            Self {
                min: DVec2::splat(f64::INFINITY),
                max: DVec2::splat(f64::NEG_INFINITY),
            },
            |b, p| Self {
                min: b.min.min(p.truncate()),
                max: b.max.max(p.truncate()),
            },
        )
    }

    fn overlaps(self, other: Self) -> bool {
        self.min.x <= other.max.x
            && self.max.x >= other.min.x
            && self.min.y <= other.max.y
            && self.max.y >= other.min.y
    }

    fn contains(self, other: Self) -> bool {
        self.min.x <= other.min.x
            && self.max.x >= other.max.x
            && self.min.y <= other.min.y
            && self.max.y >= other.max.y
    }
}

struct Region {
    polygon: Vec<DVec3>,
    planes: Vec<DVec3>,
    depth: DVec3,
    face: usize,
    bounds: Bounds,
}

enum NodeKind {
    Leaf(usize),
    Branch(usize, usize),
}

struct Node {
    bounds: Bounds,
    kind: NodeKind,
}

/// Balanced projected bounds, shared by burial, boundary and crossing discovery.
struct Index(Vec<Node>);

impl Index {
    fn new(regions: &[Region]) -> Self {
        fn build(nodes: &mut Vec<Node>, ids: &mut [usize], regions: &[Region]) -> usize {
            let bounds = ids
                .iter()
                .map(|&i| regions[i].bounds)
                .reduce(|a, b| Bounds {
                    min: a.min.min(b.min),
                    max: a.max.max(b.max),
                })
                .expect("an index node has a region");
            let at = nodes.len();
            nodes.push(Node {
                bounds,
                kind: NodeKind::Leaf(ids[0]),
            });
            if ids.len() > 1 {
                let extent = bounds.max - bounds.min;
                let axis = usize::from(extent.y > extent.x);
                let mid = ids.len() / 2;
                ids.select_nth_unstable_by(mid, |&a, &b| {
                    let centre =
                        |i: usize| regions[i].bounds.min[axis] + regions[i].bounds.max[axis];
                    centre(a).total_cmp(&centre(b)).then(a.cmp(&b))
                });
                let (left, right) = ids.split_at_mut(mid);
                let left = build(nodes, left, regions);
                let right = build(nodes, right, regions);
                nodes[at].kind = NodeKind::Branch(left, right);
            }
            at
        }
        let mut nodes = Vec::with_capacity(regions.len() * 2);
        if !regions.is_empty() {
            build(
                &mut nodes,
                &mut (0..regions.len()).collect::<Vec<_>>(),
                regions,
            );
        }
        Self(nodes)
    }

    fn visit(&self, bounds: Bounds, mut f: impl FnMut(usize) -> bool) {
        fn visit(
            nodes: &[Node],
            at: usize,
            bounds: Bounds,
            f: &mut impl FnMut(usize) -> bool,
        ) -> bool {
            let node = &nodes[at];
            if !node.bounds.overlaps(bounds) {
                return true;
            }
            match node.kind {
                NodeKind::Leaf(i) => f(i),
                NodeKind::Branch(a, b) => visit(nodes, a, bounds, f) && visit(nodes, b, bounds, f),
            }
        }
        if !self.0.is_empty() {
            visit(&self.0, 0, bounds, &mut f);
        }
    }
}

#[derive(Clone, Copy)]
struct Edge {
    a: DVec3,
    b: DVec3,
}

impl Edge {
    fn new(a: DVec3, b: DVec3) -> Self {
        if a.x < b.x || (a.x == b.x && a.y <= b.y) {
            Self { a, b }
        } else {
            Self { a: b, b: a }
        }
    }

    fn point(self, t: f64) -> DVec3 {
        if t == 0.0 {
            self.a
        } else if t == 1.0 {
            self.b
        } else {
            self.a + (self.b - self.a) * t
        }
    }

    fn at(self, x: f64) -> DVec3 {
        self.point((x - self.a.x) / (self.b.x - self.a.x))
    }

    fn bounds(self) -> Bounds {
        Bounds::of([self.a, self.b].into_iter())
    }
}

fn covered(edge: Edge, planes: impl Iterator<Item = DVec3>) -> Option<(f64, f64)> {
    let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
    for plane in planes {
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

/// Parameter interval covered by a convex region strictly above this boundary.
fn hidden(edge: Edge, depth: DVec3, region: &Region) -> Option<(f64, f64)> {
    let delta = region.depth - depth;
    if delta.dot(edge.a) <= 0.0 && delta.dot(edge.b) <= 0.0 {
        return None; // Equality alone does not hide a boundary.
    }
    covered(edge, region.planes.iter().copied().chain([delta]))
}

#[derive(Clone, Copy)]
struct Band {
    key: (usize, usize),
    start: f64,
}

#[derive(Default)]
pub(super) struct Scratch {
    /// Exact region burial/edge-cover tests, after bounds pruning (not index node visits).
    pub(super) tests: usize,
    intervals: Vec<(f64, f64)>,
    next_intervals: Vec<(f64, f64)>,
    edges: Vec<Edge>,
    events: Vec<f64>,
    active: Vec<(usize, f64)>,
    lateral: Vec<(f64, f64)>,
    open: Vec<Band>,
    next: Vec<Band>,
}

fn visible(
    edge: Edge,
    depth: DVec3,
    owners: (usize, usize),
    regions: &[Region],
    index: &Index,
    scratch: &mut Scratch,
) {
    if edge.a == edge.b {
        return;
    }
    scratch.intervals.clear();
    scratch.intervals.push((0.0, 1.0));
    index.visit(edge.bounds(), |i| {
        if i == owners.0 || i == owners.1 {
            return true;
        }
        scratch.tests += 1;
        let Some((lo, hi)) = hidden(edge, depth, &regions[i]) else {
            return true;
        };
        scratch.next_intervals.clear();
        for &(a, b) in &scratch.intervals {
            if hi <= a || lo >= b {
                scratch.next_intervals.push((a, b));
            } else {
                if a < lo {
                    scratch.next_intervals.push((a, lo));
                }
                if hi < b {
                    scratch.next_intervals.push((hi, b));
                }
            }
        }
        std::mem::swap(&mut scratch.intervals, &mut scratch.next_intervals);
        !scratch.intervals.is_empty()
    });
    for &(a, b) in &scratch.intervals {
        scratch.edges.push(Edge::new(edge.point(a), edge.point(b)));
    }
}

fn crossing(polygon: &[DVec3], plane: DVec3) -> Option<Edge> {
    if plane.x == 0.0 && plane.y == 0.0 {
        return None;
    } // Parallel/coincident planes.
    let axis = DVec3::new(-plane.y, plane.x, 0.0);
    let (mut first, mut last): (Option<(f64, DVec3)>, Option<(f64, DVec3)>) = (None, None);
    let mut add = |p| {
        let t = axis.dot(p);
        if first.is_none_or(|(s, _)| t < s) {
            first = Some((t, p));
        }
        if last.is_none_or(|(s, _)| t > s) {
            last = Some((t, p));
        }
    };
    for (&a, &b) in polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .take(polygon.len())
    {
        let (da, db) = (plane.dot(a), plane.dot(b));
        if da == 0.0 {
            add(a);
        }
        if (da < 0.0 && db > 0.0) || (da > 0.0 && db < 0.0) {
            add(a + (b - a) * (da / (da - db)));
        }
    }
    let edge = Edge::new(first?.1, last?.1);
    (edge.a != edge.b).then_some(edge)
}

fn boundaries(regions: &[Region], rectangle: &[DVec3], index: &Index, scratch: &mut Scratch) {
    scratch.edges.clear();
    for (&a, &b) in rectangle
        .iter()
        .zip(rectangle.iter().cycle().skip(1))
        .take(rectangle.len())
    {
        scratch.edges.push(Edge::new(a, b));
    }
    for (i, region) in regions.iter().enumerate() {
        for (&a, &b) in region
            .polygon
            .iter()
            .zip(region.polygon.iter().cycle().skip(1))
            .take(region.polygon.len())
        {
            // The rectangle already carries these boundaries; duplicate mesh edges add no seam.
            if (a.y == b.y && (a.y == rectangle[0].y || a.y == rectangle[2].y))
                || (a.x == b.x && (a.x == rectangle[0].x || a.x == rectangle[2].x))
            {
                continue;
            }
            visible(
                Edge::new(a, b),
                region.depth,
                (i, i),
                regions,
                index,
                scratch,
            );
        }
        index.visit(region.bounds, |j| {
            if j >= i {
                return true;
            }
            let other = &regions[j];
            if let Some(edge) = crossing(&region.polygon, region.depth - other.depth)
                && let Some((lo, hi)) = covered(edge, other.planes.iter().copied())
            {
                // No visibility scan for a point intersection or a crossing outside both faces.
                let edge = Edge::new(edge.point(lo), edge.point(hi));
                if edge.a != edge.b {
                    visible(edge, region.depth, (i, j), regions, index, scratch);
                }
            }
            true
        });
    }
    scratch.edges.sort_unstable_by(|a, b| {
        a.a.x
            .total_cmp(&b.a.x)
            .then(a.a.y.total_cmp(&b.a.y))
            .then(a.b.x.total_cmp(&b.b.x))
            .then(a.b.y.total_cmp(&b.b.y))
    });
    scratch.edges.dedup_by(|a, b| a.a == b.a && a.b == b.b);
}

fn events(edges: &[Edge], events: &mut Vec<f64>) {
    events.clear();
    events.extend(edges.iter().flat_map(|e| [e.a.x, e.b.x]));
    for (i, a) in edges.iter().enumerate() {
        for b in &edges[..i] {
            let (lo, hi) = (a.a.x.max(b.a.x), a.b.x.min(b.b.x));
            if lo >= hi || !a.bounds().overlaps(b.bounds()) {
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

fn remove_buried(regions: Vec<Region>, tests: &mut usize) -> Vec<Region> {
    let index = Index::new(&regions);
    let buried: Vec<_> = regions
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut buried = false;
            index.visit(r.bounds, |j| {
                let other = &regions[j];
                if i == j || !other.bounds.contains(r.bounds) {
                    return true;
                }
                *tests += 1;
                buried = r.polygon.iter().all(|p| {
                    other.planes.iter().all(|plane| plane.dot(*p) >= 0.0)
                        && (other.depth - r.depth).dot(*p) >= 0.0
                }) && (r
                    .polygon
                    .iter()
                    .any(|p| (other.depth - r.depth).dot(*p) > 0.0)
                    || (other.depth == r.depth && j < i));
                !buried
            });
            buried
        })
        .collect();
    regions
        .into_iter()
        .zip(buried)
        .filter_map(|(r, b)| (!b).then_some(r))
        .collect()
}

fn emit(
    key: (usize, usize),
    start: f64,
    end: f64,
    edges: &[Edge],
    base: Vec3,
    regions: &[Region],
    cells: &mut Vec<Cell>,
) {
    let (lower, upper) = (edges[key.0], edges[key.1]);
    let corners = [
        lower.at(start),
        lower.at(end),
        upper.at(end),
        upper.at(start),
    ];
    let (mut moment, mut area) = (DVec3::ZERO, 0.0);
    for pair in corners[1..].windows(2) {
        let (a, b) = (pair[0] - corners[0], pair[1] - corners[0]);
        let cross = a.x * b.y - a.y * b.x;
        area += cross;
        moment += (corners[0] + pair[0] + pair[1]) * cross / 3.0;
    }
    let polygon: Vec<_> = corners.into_iter().map(|p| p.as_vec3()).collect();
    if area > 0.0 && super::signed_area(&polygon) > 0.0 {
        // A convex band's centroid is interior, away from depth ties at its apex.
        let owner = owner(moment / area, base.as_dvec3(), regions);
        cells.push(Cell {
            polygon,
            depth: owner.map_or(base, |i| regions[i].depth.as_vec3()),
            face: owner.map(|i| regions[i].face),
        });
    }
}

pub(super) fn partition(
    a: f32,
    b: f32,
    strip: (f32, f32),
    base: Vec3,
    free: Vec3,
    faces: &[(usize, &ClippedFace)],
    scratch: &mut Scratch,
) -> Vec<Cell> {
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
    let regions = faces
        .iter()
        .enumerate()
        .filter_map(|(i, (_, face))| {
            let depth = (face.depth + free).as_dvec3();
            let planes: Vec<_> = face
                .planes
                .iter()
                .map(|p| p.as_dvec3())
                .chain([depth - base.as_dvec3()])
                .collect();
            let polygon = planes
                .iter()
                .fold(rectangle.to_vec(), |p, &plane| clip(&p, plane));
            (signed_area(&polygon) > 0.0).then(|| Region {
                bounds: Bounds::of(polygon.iter().copied()),
                polygon,
                planes,
                depth,
                face: i,
            })
        })
        .collect();
    let regions = remove_buried(regions, &mut scratch.tests);
    let index = Index::new(&regions);
    boundaries(&regions, &rectangle, &index, scratch);
    events(&scratch.edges, &mut scratch.events);
    scratch.open.clear();
    let mut cells = Vec::new();
    for xs in scratch.events.windows(2) {
        let (x, end) = (xs[0], xs[1]);
        let mid = (x + end) / 2.0;
        scratch.active.clear();
        scratch.active.extend(
            scratch
                .edges
                .iter()
                .enumerate()
                .filter(|(_, e)| e.a.x < e.b.x && e.a.x <= x && e.b.x >= end)
                .map(|(i, e)| (i, e.at(mid).y)),
        );
        scratch
            .active
            .sort_unstable_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        scratch.lateral.clear();
        scratch.lateral.extend(
            scratch
                .edges
                .iter()
                .filter(|e| e.a.x == x && e.b.x == x)
                .map(|e| (e.a.y, e.b.y)),
        );
        scratch.next.clear();
        for pair in scratch.active.windows(2) {
            let [(lower, lo), (upper, hi)] = [pair[0], pair[1]];
            if lo >= hi {
                continue;
            } // Coincident boundaries enclose no area.
            let key = (lower, upper);
            let mut start = x;
            if let Ok(i) = scratch.open.binary_search_by_key(&key, |b| b.key) {
                let band = scratch.open[i];
                let interrupted = scratch.lateral.iter().any(|&(lo, hi)| {
                    lo < scratch.edges[upper].at(x).y && hi > scratch.edges[lower].at(x).y
                });
                if interrupted {
                    emit(
                        key,
                        band.start,
                        x,
                        &scratch.edges,
                        base,
                        &regions,
                        &mut cells,
                    );
                } else {
                    start = band.start;
                }
            }
            scratch.next.push(Band { key, start });
        }
        scratch.next.sort_unstable_by_key(|b| b.key);
        for band in &scratch.open {
            if scratch
                .next
                .binary_search_by_key(&band.key, |b| b.key)
                .is_err()
            {
                emit(
                    band.key,
                    band.start,
                    x,
                    &scratch.edges,
                    base,
                    &regions,
                    &mut cells,
                );
            }
        }
        std::mem::swap(&mut scratch.open, &mut scratch.next);
    }
    for band in &scratch.open {
        emit(
            band.key,
            band.start,
            b.into(),
            &scratch.edges,
            base,
            &regions,
            &mut cells,
        );
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(depth: f64, face: usize) -> Region {
        // Fixture: a unit projected region, independent of any asset or collider type.
        let polygon = vec![
            DVec3::Z,
            DVec3::new(1.0, 0.0, 1.0),
            DVec3::ONE,
            DVec3::new(0.0, 1.0, 1.0),
        ];
        let planes = polygon
            .iter()
            .zip(polygon.iter().cycle().skip(1))
            .take(4)
            .map(|(a, b)| a.cross(*b))
            .collect();
        Region {
            bounds: Bounds::of(polygon.iter().copied()),
            polygon,
            planes,
            depth: DVec3::Z * depth,
            face,
        }
    }

    /// Discovery must remove hidden portions before sweeping: retaining them repeats downstream
    /// integration despite leaving the pointwise envelope unchanged. No burial prefilter is used.
    #[test]
    fn visible_trims_a_partly_hidden_boundary() {
        let regions = vec![square(2.0, 0)];
        let index = Index::new(&regions);
        let mut scratch = Scratch::default();
        visible(
            Edge::new(DVec3::new(-1.0, 0.5, 1.0), DVec3::new(2.0, 0.5, 1.0)),
            DVec3::Z,
            (usize::MAX, usize::MAX),
            &regions,
            &index,
            &mut scratch,
        );
        // DERIVED: the unit square hides [0,1] of [-1,2], leaving two unit intervals.
        assert_eq!(scratch.edges.len(), 2);
        assert_eq!((scratch.edges[0].a.x, scratch.edges[0].b.x), (-1.0, 0.0));
        assert_eq!((scratch.edges[1].a.x, scratch.edges[1].b.x), (1.0, 2.0));
    }

    /// Discovery must discard a fully buried input before generating pair crossings. Testing the
    /// prefilter directly prevents the independent visibility pass from masking its removal.
    #[test]
    fn burial_removes_the_lower_region_before_discovery() {
        let mut tests = 0;
        let regions = remove_buried(vec![square(1.0, 0), square(2.0, 1)], &mut tests);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].face, 1);
    }
}
