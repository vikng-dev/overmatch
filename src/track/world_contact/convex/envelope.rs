//! Vertical decomposition of the visible upper envelope over a positive column rectangle.
//! `partition` refuses nonpositive dimensions. Integration bands are internal geometry;
//! `super::integrated_regions` owns the contact bound. Scratch belongs to one side and is
//! reused across its columns and halves. The spatial index admits only overlapping regions to
//! exact tests, and faces on opposite sides of a shared edge are never crossed
//! ([`Regions::share_edge`]). The sweep visits edges in left-end order: crossing events and each
//! interval's active edges read only the edges spanning it.

use super::{Cell, ClippedFace};
use bevy::math::{DVec2, DVec3};
use bevy::prelude::*;
use std::ops::Range;

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

/// A face clipped to the half and above the field; its polygon and planes live in [`Regions`].
struct Region {
    polygon: Range<usize>,
    planes: Range<usize>,
    depth: DVec3,
    face: usize,
    collider_order: usize,
    bounds: Bounds,
}

/// One half's regions, sharing point and plane buffers across halves and columns.
#[derive(Default)]
struct Regions {
    list: Vec<Region>,
    points: Vec<DVec3>,
    planes: Vec<DVec3>,
}

impl Regions {
    fn polygon(&self, i: usize) -> &[DVec3] {
        &self.points[self.list[i].polygon.clone()]
    }

    fn planes(&self, i: usize) -> &[DVec3] {
        &self.planes[self.list[i].planes.clone()]
    }

    /// Constraints holding one line with opposite signs, bit for bit, put two regions on opposite
    /// sides of it, so their depth crossing lies on boundaries both already carry. Adjacent
    /// triangles cross their shared projected vertices in opposite order; adjacent shoe shadows
    /// meet at one along coordinate. Faces of one collider sharing no edge are still crossed:
    /// Parry fans merged outlines that need not be planar, and a fold can overlap.
    fn share_edge(&self, i: usize, j: usize) -> bool {
        let other = self.planes(j);
        self.planes(i).iter().any(|p| other.contains(&-*p))
    }
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
#[derive(Default)]
struct Index {
    nodes: Vec<Node>,
    ids: Vec<usize>,
}

impl Index {
    fn build(&mut self, regions: &[Region]) {
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
        self.nodes.clear();
        self.ids.clear();
        self.ids.extend(0..regions.len());
        if !regions.is_empty() {
            build(&mut self.nodes, &mut self.ids, regions);
        }
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
        if !self.nodes.is_empty() {
            visit(&self.nodes, 0, bounds, &mut f);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
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
fn hidden(edge: Edge, depth: DVec3, other: DVec3, planes: &[DVec3]) -> Option<(f64, f64)> {
    let delta = other - depth;
    if delta.dot(edge.a) <= 0.0 && delta.dot(edge.b) <= 0.0 {
        return None; // Equality alone does not hide a boundary.
    }
    covered(edge, planes.iter().copied().chain([delta]))
}

#[derive(Clone, Copy)]
struct Band {
    key: (usize, usize),
    start: f64,
}

/// Partition work, drained into the tick's counters by `super::patches`.
#[derive(Default)]
pub(super) struct Counts {
    /// Exact region burial/edge-cover tests, after bounds pruning (not index node visits).
    pub(super) tests: usize,
    /// Distinct boundary edges entering the sweep.
    pub(super) edges: usize,
    pub(super) intervals: usize,
    /// Edge pairs examined for a crossing event.
    pub(super) pairs: usize,
    /// Edges visited to build each interval's active and lateral lists.
    pub(super) edge_visits: usize,
    /// Regions examined to choose band owners.
    pub(super) owner_tests: usize,
    /// Buffers allocated per admitted face and per band; reused buffers are not counted.
    pub(super) allocs: usize,
}

#[derive(Default)]
struct Discovery {
    /// Test reference: cross faces across shared edges too.
    #[cfg(test)]
    cross_shared_edges: bool,
    intervals: Vec<(f64, f64)>,
    next_intervals: Vec<(f64, f64)>,
    edges: Vec<Edge>,
}

#[derive(Default)]
struct Sweep {
    events: Vec<f64>,
    /// Edges, by index, that began at or before the sweep position and end after it.
    live: Vec<usize>,
    active: Vec<(usize, f64)>,
    lateral: Vec<(f64, f64)>,
    open: Vec<Band>,
    next: Vec<Band>,
}

#[derive(Default)]
pub(super) struct Scratch {
    pub(super) counts: Counts,
    regions: Regions,
    index: Index,
    clipped: [Vec<DVec3>; 2],
    buried: Vec<bool>,
    discovery: Discovery,
    sweep: Sweep,
}

#[cfg(test)]
impl Scratch {
    /// The reference that also crosses faces across shared edges.
    pub(super) fn crossing_shared_edges() -> Self {
        let mut scratch = Self::default();
        scratch.discovery.cross_shared_edges = true;
        scratch
    }
}

impl Discovery {
    /// Whether regions `i` and `j` share an edge, so their crossing is skipped.
    fn skips(&self, regions: &Regions, i: usize, j: usize) -> bool {
        #[cfg(test)]
        if self.cross_shared_edges {
            return false;
        }
        regions.share_edge(i, j)
    }
}

fn visible(
    edge: Edge,
    depth: DVec3,
    owners: (usize, usize),
    regions: &Regions,
    index: &Index,
    out: &mut Discovery,
    counts: &mut Counts,
) {
    if edge.a == edge.b {
        return;
    }
    out.intervals.clear();
    out.intervals.push((0.0, 1.0));
    index.visit(edge.bounds(), |i| {
        if i == owners.0 || i == owners.1 {
            return true;
        }
        counts.tests += 1;
        let Some((lo, hi)) = hidden(edge, depth, regions.list[i].depth, regions.planes(i)) else {
            return true;
        };
        out.next_intervals.clear();
        for &(a, b) in &out.intervals {
            if hi <= a || lo >= b {
                out.next_intervals.push((a, b));
            } else {
                if a < lo {
                    out.next_intervals.push((a, lo));
                }
                if hi < b {
                    out.next_intervals.push((hi, b));
                }
            }
        }
        std::mem::swap(&mut out.intervals, &mut out.next_intervals);
        !out.intervals.is_empty()
    });
    for &(a, b) in &out.intervals {
        out.edges.push(Edge::new(edge.point(a), edge.point(b)));
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

fn boundaries(
    regions: &Regions,
    rectangle: &[DVec3; 4],
    index: &Index,
    out: &mut Discovery,
    counts: &mut Counts,
) {
    out.edges.clear();
    for (&a, &b) in rectangle.iter().zip(rectangle.iter().cycle().skip(1)) {
        out.edges.push(Edge::new(a, b));
    }
    for (i, region) in regions.list.iter().enumerate() {
        let polygon = regions.polygon(i);
        for (&a, &b) in polygon
            .iter()
            .zip(polygon.iter().cycle().skip(1))
            .take(polygon.len())
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
                out,
                counts,
            );
        }
        index.visit(region.bounds, |j| {
            if j >= i || out.skips(regions, i, j) {
                return true;
            }
            let other = &regions.list[j];
            if let Some(edge) = crossing(polygon, region.depth - other.depth)
                && let Some((lo, hi)) = covered(edge, regions.planes(j).iter().copied())
            {
                // No visibility scan for a point intersection or a crossing outside both faces.
                let edge = Edge::new(edge.point(lo), edge.point(hi));
                if edge.a != edge.b {
                    visible(edge, region.depth, (i, j), regions, index, out, counts);
                }
            }
            true
        });
    }
    out.edges.sort_unstable_by(|a, b| {
        a.a.x
            .total_cmp(&b.a.x)
            .then(a.a.y.total_cmp(&b.a.y))
            .then(a.b.x.total_cmp(&b.b.x))
            .then(a.b.y.total_cmp(&b.b.y))
    });
    out.edges.dedup_by(|a, b| a.a == b.a && a.b == b.b);
}

/// Every endpoint and every crossing of two edges spanning a common interval. `edges` are in
/// left-end order, so an earlier edge overlaps this one exactly while it ends to the right of
/// this one's left end; an edge ending there overlaps no later edge.
fn events(edges: &[Edge], sweep: &mut Sweep, counts: &mut Counts) {
    sweep.events.clear();
    sweep
        .events
        .extend(edges.iter().flat_map(|e| [e.a.x, e.b.x]));
    sweep.live.clear();
    for (i, a) in edges.iter().enumerate() {
        sweep.live.retain(|&j| edges[j].b.x > a.a.x);
        counts.pairs += sweep.live.len();
        for b in sweep.live.iter().map(|&j| edges[j]) {
            let (lo, hi) = (a.a.x.max(b.a.x), a.b.x.min(b.b.x));
            if lo >= hi || !a.bounds().overlaps(b.bounds()) {
                continue;
            }
            let (dlo, dhi) = (a.at(lo).y - b.at(lo).y, a.at(hi).y - b.at(hi).y);
            if (dlo < 0.0 && dhi > 0.0) || (dlo > 0.0 && dhi < 0.0) {
                sweep.events.push(lo + (hi - lo) * (dlo / (dlo - dhi)));
            }
        }
        if a.a.x < a.b.x {
            sweep.live.push(i);
        }
    }
    sweep.events.sort_by(f64::total_cmp);
    sweep.events.dedup();
}

/// Move the live set to the interval starting at event `x`: the edges that span it, because
/// every endpoint is an event. Returns the edges starting at `x`, which hold its lateral edges.
fn advance(
    edges: &[Edge],
    x: f64,
    next: &mut usize,
    live: &mut Vec<usize>,
    counts: &mut Counts,
) -> Range<usize> {
    let start = *next;
    while *next < edges.len() && edges[*next].a.x <= x {
        *next += 1;
    }
    counts.edge_visits += live.len() + (*next - start);
    live.retain(|&i| edges[i].b.x > x);
    live.extend((start..*next).filter(|&i| edges[i].b.x > x));
    start..*next
}

// Field ties are handled by `owner`; solid ties use snapshot identity and canonical geometry.
fn owner_order(regions: &Regions, a: usize, b: usize) -> std::cmp::Ordering {
    fn point(a: DVec3, b: DVec3) -> std::cmp::Ordering {
        a.x.total_cmp(&b.x)
            .then(a.y.total_cmp(&b.y))
            .then(a.z.total_cmp(&b.z))
    }
    fn first(polygon: &[DVec3]) -> usize {
        polygon
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| point(**a, **b))
            .expect("positive-area regions have vertices")
            .0
    }
    let order = regions.list[a]
        .collider_order
        .cmp(&regions.list[b].collider_order)
        .then(point(regions.list[a].depth, regions.list[b].depth));
    if !order.is_eq() {
        return order;
    }
    let (a, b) = (regions.polygon(a), regions.polygon(b));
    for (a, b) in a
        .iter()
        .cycle()
        .skip(first(a))
        .take(a.len())
        .zip(b.iter().cycle().skip(first(b)).take(b.len()))
    {
        let order = point(*a, *b);
        if !order.is_eq() {
            return order;
        }
    }
    a.len().cmp(&b.len())
}

fn owner(point: DVec3, base: DVec3, regions: &Regions, counts: &mut Counts) -> Option<usize> {
    counts.owner_tests += regions.list.len();
    let mut best = base.dot(point);
    let mut owner = None;
    for (i, region) in regions.list.iter().enumerate() {
        let depth = region.depth.dot(point);
        if (depth > best
            || (depth == best && owner.is_some_and(|j| owner_order(regions, i, j).is_lt())))
            && regions.planes(i).iter().all(|p| p.dot(point) >= 0.0)
        {
            best = depth;
            owner = Some(i);
        }
    }
    owner
}

fn remove_buried(
    regions: &mut Regions,
    index: &mut Index,
    buried: &mut Vec<bool>,
    tests: &mut usize,
) {
    index.build(&regions.list);
    buried.clear();
    for (i, r) in regions.list.iter().enumerate() {
        let mut is_buried = false;
        index.visit(r.bounds, |j| {
            let other = &regions.list[j];
            if i == j || !other.bounds.contains(r.bounds) {
                return true;
            }
            *tests += 1;
            let polygon = regions.polygon(i);
            is_buried = polygon.iter().all(|p| {
                regions.planes(j).iter().all(|plane| plane.dot(*p) >= 0.0)
                    && (other.depth - r.depth).dot(*p) >= 0.0
            }) && (polygon
                .iter()
                .any(|p| (other.depth - r.depth).dot(*p) > 0.0)
                || (other.depth == r.depth && owner_order(regions, j, i).then(j.cmp(&i)).is_lt()));
            !is_buried
        });
        buried.push(is_buried);
    }
    let mut flags = buried.iter();
    regions
        .list
        .retain(|_| !*flags.next().expect("one burial flag per region"));
}

fn emit(
    Band { key, start }: Band,
    end: f64,
    edges: &[Edge],
    base: Vec3,
    regions: &Regions,
    cells: &mut Vec<Cell>,
    counts: &mut Counts,
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
    let polygon = corners.map(|p| p.as_vec3());
    if area > 0.0 && super::signed_area(&polygon) > 0.0 {
        // A convex band's centroid is interior, away from depth ties at its apex.
        let owner = owner(moment / area, base.as_dvec3(), regions, counts);
        cells.push(Cell {
            polygon,
            depth: owner.map_or(base, |i| regions.list[i].depth.as_vec3()),
            face: owner.map(|i| regions.list[i].face),
        });
    }
}

/// Bands of the visible envelope over `along x strip`, replacing `cells`.
pub(super) fn partition(
    (a, b): (f32, f32),
    strip: (f32, f32),
    base: Vec3,
    free: Vec3,
    faces: &[(usize, &ClippedFace)],
    scratch: &mut Scratch,
    cells: &mut Vec<Cell>,
) {
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
    let Scratch {
        counts,
        regions,
        index,
        clipped,
        buried,
        discovery,
        sweep,
    } = scratch;
    regions.list.clear();
    regions.points.clear();
    regions.planes.clear();
    for (i, (collider_order, face)) in faces.iter().enumerate() {
        let depth = (face.depth + free).as_dvec3();
        let planes = regions.planes.len()..regions.planes.len() + face.planes.len() + 1;
        regions
            .planes
            .extend(face.planes.iter().map(|p| p.as_dvec3()));
        regions.planes.push(depth - base.as_dvec3());
        let [polygon, next] = &mut *clipped;
        polygon.clear();
        polygon.extend(rectangle);
        for &plane in &regions.planes[planes.clone()] {
            clip_into(polygon, plane, next);
            std::mem::swap(polygon, next);
        }
        if signed_area(polygon) > 0.0 {
            let start = regions.points.len();
            regions.points.extend_from_slice(polygon);
            regions.list.push(Region {
                polygon: start..regions.points.len(),
                planes,
                depth,
                face: i,
                collider_order: *collider_order,
                bounds: Bounds::of(polygon.iter().copied()),
            });
        } else {
            regions.planes.truncate(planes.start);
        }
    }
    remove_buried(regions, index, buried, &mut counts.tests);
    index.build(&regions.list);
    boundaries(regions, &rectangle, index, discovery, counts);
    let edges = &discovery.edges;
    events(edges, sweep, counts);
    counts.edges += edges.len();
    counts.intervals += sweep.events.len().saturating_sub(1);
    cells.clear();
    sweep.open.clear();
    sweep.live.clear();
    let mut next = 0;
    for xs in sweep.events.windows(2) {
        let (x, end) = (xs[0], xs[1]);
        let mid = (x + end) / 2.0;
        let starting = advance(edges, x, &mut next, &mut sweep.live, counts);
        sweep.active.clear();
        sweep
            .active
            .extend(sweep.live.iter().map(|&i| (i, edges[i].at(mid).y)));
        sweep
            .active
            .sort_unstable_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        sweep.lateral.clear();
        sweep.lateral.extend(
            edges[starting]
                .iter()
                .filter(|e| e.a.x == x && e.b.x == x)
                .map(|e| (e.a.y, e.b.y)),
        );
        sweep.next.clear();
        for pair in sweep.active.windows(2) {
            let [(lower, lo), (upper, hi)] = [pair[0], pair[1]];
            if lo >= hi {
                continue;
            } // Coincident boundaries enclose no area.
            let key = (lower, upper);
            let mut start = x;
            if let Ok(i) = sweep.open.binary_search_by_key(&key, |b| b.key) {
                let band = sweep.open[i];
                let interrupted = sweep
                    .lateral
                    .iter()
                    .any(|&(lo, hi)| lo < edges[upper].at(x).y && hi > edges[lower].at(x).y);
                if interrupted {
                    emit(band, x, edges, base, regions, cells, counts);
                } else {
                    start = band.start;
                }
            }
            sweep.next.push(Band { key, start });
        }
        sweep.next.sort_unstable_by_key(|b| b.key);
        for band in &sweep.open {
            if sweep
                .next
                .binary_search_by_key(&band.key, |b| b.key)
                .is_err()
            {
                emit(*band, x, edges, base, regions, cells, counts);
            }
        }
        std::mem::swap(&mut sweep.open, &mut sweep.next);
    }
    for band in &sweep.open {
        emit(*band, b.into(), edges, base, regions, cells, counts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn squares(depths: &[f64]) -> Regions {
        // Fixture: unit projected regions, independent of any asset or collider type.
        let polygon = [
            DVec3::Z,
            DVec3::new(1.0, 0.0, 1.0),
            DVec3::ONE,
            DVec3::new(0.0, 1.0, 1.0),
        ];
        let mut regions = Regions::default();
        for (face, &depth) in depths.iter().enumerate() {
            let points = regions.points.len()..regions.points.len() + polygon.len();
            regions.points.extend(polygon);
            let planes = regions.planes.len()..regions.planes.len() + polygon.len();
            regions.planes.extend(
                polygon
                    .iter()
                    .zip(polygon.iter().cycle().skip(1))
                    .map(|(a, b)| a.cross(*b)),
            );
            regions.list.push(Region {
                polygon: points,
                planes,
                depth: DVec3::Z * depth,
                face,
                collider_order: face,
                bounds: Bounds::of(polygon.into_iter()),
            });
        }
        regions
    }

    /// Fixture: a deterministic xorshift stream for coordinates.
    struct Stream(u64);

    impl Stream {
        fn unit(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1_u64 << 53) as f64
        }
    }

    /// Discovery must remove hidden portions before sweeping: retaining them repeats downstream
    /// integration despite leaving the pointwise envelope unchanged. No burial prefilter is used.
    #[test]
    fn visible_trims_a_partly_hidden_boundary() {
        let regions = squares(&[2.0, 1.0]);
        let mut index = Index::default();
        index.build(&regions.list);
        let mut out = Discovery::default();
        visible(
            Edge::new(DVec3::new(-1.0, 0.5, 1.0), DVec3::new(2.0, 0.5, 1.0)),
            DVec3::Z,
            (1, 1),
            &regions,
            &index,
            &mut out,
            &mut Counts::default(),
        );
        // DERIVED: the unit square hides [0,1] of [-1,2], leaving two unit intervals.
        assert_eq!(out.edges.len(), 2);
        assert_eq!((out.edges[0].a.x, out.edges[0].b.x), (-1.0, 0.0));
        assert_eq!((out.edges[1].a.x, out.edges[1].b.x), (1.0, 2.0));
    }

    /// Discovery must discard a fully buried input before generating pair crossings. Testing the
    /// prefilter directly prevents the independent visibility pass from masking its removal.
    #[test]
    fn burial_removes_the_lower_region_before_discovery() {
        let mut regions = squares(&[1.0, 2.0]);
        remove_buried(&mut regions, &mut Index::default(), &mut Vec::new(), &mut 0);
        assert_eq!(regions.list.len(), 1);
        assert_eq!(regions.list[0].face, 1);
    }

    /// Burial, hiding and crossing discovery read only the regions the index visits, so every
    /// region whose bounds overlap the query must be visited. A node bound taken from one child,
    /// or a visit that stops after the first branch, fails this full scan.
    #[test]
    fn the_index_visits_every_overlapping_region() {
        let mut stream = Stream(0x2545_f491_4f6c_dd1d);
        let mut bounds = || {
            let (x, y) = (stream.unit(), stream.unit());
            let (w, h) = (stream.unit() * 0.3, stream.unit() * 0.3);
            Bounds {
                min: DVec2::new(x, y),
                max: DVec2::new(x + w, y + h),
            }
        };
        // Fixture: 200 random boxes, including empty-width ones, and 100 random queries.
        let list: Vec<_> = (0..200)
            .map(|i| Region {
                polygon: 0..0,
                planes: 0..0,
                depth: DVec3::ZERO,
                face: i,
                collider_order: i,
                bounds: if i % 7 == 0 {
                    let b = bounds();
                    Bounds {
                        max: DVec2::new(b.min.x, b.max.y),
                        ..b
                    }
                } else {
                    bounds()
                },
            })
            .collect();
        let mut index = Index::default();
        index.build(&list);
        let mut visited_any = false;
        for _ in 0..100 {
            let query = bounds();
            let mut visited = Vec::new();
            index.visit(query, |i| {
                visited.push(i);
                true
            });
            visited.sort_unstable();
            let expected: Vec<_> = (0..list.len())
                .filter(|&i| list[i].bounds.overlaps(query))
                .collect();
            assert_eq!(visited, expected);
            visited_any |= !expected.is_empty();
        }
        assert!(visited_any, "the fixture queries must hit some regions");
    }

    /// The sweep reads each interval's edges from its live set and tests crossings only between
    /// edges spanning a common interval. Both must equal a full scan: a missed crossing leaves
    /// two boundaries swapped inside a band, and a missed edge merges bands across it. Mutants
    /// that keep an edge ending at the interval start, or prune an earlier edge ending exactly at
    /// a later edge's left end too late or too early, fail here.
    #[test]
    fn the_incremental_sweep_matches_a_full_scan() {
        let mut stream = Stream(0x9e37_79b9_7f4a_7c15);
        // Fixture: segments on a coarse grid, so endpoints coincide, lines repeat and some are
        // vertical, mixed with off-grid oblique segments that cross them.
        let mut edges: Vec<_> = (0..400)
            .map(|i| {
                let mut point = || {
                    let (x, y) = (stream.unit(), stream.unit());
                    if i % 3 == 0 {
                        DVec3::new(x, y, 1.0)
                    } else {
                        DVec3::new((x * 9.0).floor() / 8.0, (y * 9.0).floor() / 8.0, 1.0)
                    }
                };
                Edge::new(point(), point())
            })
            .filter(|e| e.a != e.b)
            .collect();
        edges.sort_unstable_by(|a, b| {
            a.a.x
                .total_cmp(&b.a.x)
                .then(a.a.y.total_cmp(&b.a.y))
                .then(a.b.x.total_cmp(&b.b.x))
                .then(a.b.y.total_cmp(&b.b.y))
        });
        edges.dedup_by(|a, b| a.a == b.a && a.b == b.b);
        let mut sweep = Sweep::default();
        let mut counts = Counts::default();
        events(&edges, &mut sweep, &mut counts);
        let mut expected: Vec<f64> = edges.iter().flat_map(|e| [e.a.x, e.b.x]).collect();
        let endpoints = {
            let mut x = expected.clone();
            x.sort_by(f64::total_cmp);
            x.dedup();
            x.len()
        };
        for (i, a) in edges.iter().enumerate() {
            for b in &edges[..i] {
                let (lo, hi) = (a.a.x.max(b.a.x), a.b.x.min(b.b.x));
                if lo >= hi || !a.bounds().overlaps(b.bounds()) {
                    continue;
                }
                let (dlo, dhi) = (a.at(lo).y - b.at(lo).y, a.at(hi).y - b.at(hi).y);
                if (dlo < 0.0 && dhi > 0.0) || (dlo > 0.0 && dhi < 0.0) {
                    expected.push(lo + (hi - lo) * (dlo / (dlo - dhi)));
                }
            }
        }
        expected.sort_by(f64::total_cmp);
        expected.dedup();
        assert_eq!(sweep.events, expected);
        assert!(
            expected.len() > endpoints,
            "the fixture must contain crossings"
        );
        assert!(counts.pairs < edges.len() * (edges.len() - 1) / 2);
        let (mut next, mut live) = (0, Vec::new());
        let mut laterals = 0;
        for xs in sweep.events.windows(2) {
            let (x, end) = (xs[0], xs[1]);
            let starting = advance(&edges, x, &mut next, &mut live, &mut counts);
            let mut actual = live.clone();
            actual.sort_unstable();
            let spanning: Vec<_> = (0..edges.len())
                .filter(|&i| {
                    edges[i].a.x < edges[i].b.x && edges[i].a.x <= x && edges[i].b.x >= end
                })
                .collect();
            assert_eq!(actual, spanning);
            let lateral = |i: &usize| edges[*i].a.x == x && edges[*i].b.x == x;
            let expected: Vec<_> = (0..edges.len()).filter(lateral).collect();
            assert_eq!(starting.filter(lateral).collect::<Vec<_>>(), expected);
            laterals += expected.len();
        }
        assert!(laterals > 0, "the fixture must contain lateral edges");
    }
}
