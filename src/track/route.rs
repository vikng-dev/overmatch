//! The route core (architecture §1/§2): the tagged taut guide route of one track side, as pure
//! geometry over the side-plane circles of its running gear. Single source of truth for "where
//! the track is" — the route render tier draws it, the chain tier solves inside a tube around
//! it, and (phase B) the belt physics samples along it.
//!
//! Conventions (inherited from the sandbox, steps 17–25): everything is in the hull-local SIDE
//! PLANE as (z, y) `Vec2`s; the loop runs CCW in that plane (lower run front→rear, return run
//! rear→front); a segment's OUTWARD normal is `(tan.y, −tan.x)`. Circles are on the PIN line
//! (wheel radius + half track thickness) and are given front→rear.

use bevy::math::Vec2;

/// One sector of the guide route: which primitive a segment lies on. The tags make the route a
/// CHART, not just a polyline — motor membership, bending rest angles, and the tube's inner
/// bound are all sector questions.
#[derive(Clone, Copy, PartialEq)]
pub enum RouteTag {
    /// Wrap arc of circle `k` in the side's front→rear circle list (0 = front drive circle).
    Arc(usize),
    /// Free span: a lower tangent segment or the sagging return run.
    Span,
}

/// The tagged taut guide route, kept as an arc-length table. Built fresh from the CURRENT
/// articulated circles whenever consumed (shape-as-function; no state).
pub struct Route {
    pub pts: Vec<Vec2>,
    /// Cumulative arc length at each vertex; last = total loop length.
    cum: Vec<f32>,
    /// Per-SEGMENT sector tag (`len == pts.len() − 1`).
    tags: Vec<RouteTag>,
}

impl Route {
    pub fn total(&self) -> f32 {
        *self.cum.last().unwrap()
    }

    pub fn wrap(&self, s: f32) -> f32 {
        s.rem_euclid(self.total().max(1e-4))
    }

    /// Segment index containing WRAPPED arc position `s`.
    fn seg(&self, s: f32) -> usize {
        self.cum
            .partition_point(|&c| c <= s)
            .saturating_sub(1)
            .min(self.tags.len() - 1)
    }

    pub fn point(&self, s: f32) -> Vec2 {
        let s = self.wrap(s);
        let i = self.seg(s);
        let len = (self.cum[i + 1] - self.cum[i]).max(1e-9);
        self.pts[i].lerp(self.pts[i + 1], (s - self.cum[i]) / len)
    }

    pub fn tangent(&self, s: f32) -> Vec2 {
        let i = self.seg(self.wrap(s));
        (self.pts[i + 1] - self.pts[i]).normalize_or_zero()
    }

    pub fn tag(&self, s: f32) -> RouteTag {
        self.tags[self.seg(self.wrap(s))]
    }

    /// The route's own turning angle (rad) over one link pitch centred at `s` — the bending
    /// rest angle θ0 at a joint's OWN route coordinate. Discrete chords at the actual neighbour
    /// coordinates (matching a chain's own θ), not point curvature — point sampling at
    /// arc/tangent tessellation seams concentrates curvature into single stations.
    pub fn turning(&self, s: f32, pitch: f32) -> f32 {
        let a = self.point(s - pitch);
        let b = self.point(s);
        let c = self.point(s + pitch);
        let e0 = b - a;
        let e1 = c - b;
        e0.perp_dot(e1).atan2(e0.dot(e1))
    }

    /// Windowed projection of `p` onto the route near `hint`: (s, u) with `u` signed along the
    /// route's OUTWARD normal (positive = outside the loop). Only segments within the window
    /// are candidates — a global nearest-point query could tunnel `s` across overlapping parts
    /// of the loop (top run over belly); the window makes the rebase topology-safe.
    pub fn project(&self, p: Vec2, hint: f32, window: f32) -> (f32, f32) {
        let mut best = (self.wrap(hint), 0.0, f32::INFINITY);
        let mut s0 = hint - window;
        let hi = hint + window;
        while s0 < hi {
            let sw = self.wrap(s0);
            let i = self.seg(sw);
            let a = self.pts[i];
            let b = self.pts[i + 1];
            let ab = b - a;
            let len2 = ab.length_squared();
            if len2 > 1e-12 {
                let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
                let q = a + ab * t;
                let d2 = p.distance_squared(q);
                if d2 < best.2 {
                    let len = len2.sqrt();
                    let tan = ab / len;
                    let out = Vec2::new(tan.y, -tan.x);
                    best = (self.cum[i] + t * len, (p - q).dot(out), d2);
                }
            }
            // Advance to the segment's end (in unwrapped window coordinates).
            s0 += (self.cum[i + 1] - sw).max(1e-6);
        }
        (best.0, best.1)
    }
}

/// Build the tagged guide route from one side's CURRENT circles (front→rear, pin-line radii):
/// the lower convex envelope + external tangents + budgeted top-run sag, with every segment
/// tagged by the primitive it lies on. Closed: last point == first point. `belt_len` is the
/// loop length budget; its excess over the taut perimeter drapes into the return run.
pub fn build_route(circles: &[(Vec2, f32)], belt_len: f32) -> Route {
    fn push(pts: &mut Vec<Vec2>, tags: &mut Vec<RouteTag>, p: Vec2, tag: RouteTag) {
        if pts.last().is_none_or(|l| l.distance_squared(p) > 1e-10) {
            pts.push(p);
            tags.push(tag);
        }
    }

    // Lower convex envelope over the ordered circles (Graham-style scan): a circle whose body
    // stays above its neighbours' lower tangent is not part of the taut run and drops out — a
    // lifted wheel is skipped, never wrapped from the wrong side.
    let mut active: Vec<usize> = vec![0];
    for k in 1..circles.len() {
        while active.len() >= 2 {
            let (p, a) = (active[active.len() - 2], active[active.len() - 1]);
            let (t0, _) =
                external_tangent(circles[p].0, circles[p].1, circles[k].0, circles[k].1, -1.0);
            let n = (t0 - circles[p].0) / circles[p].1;
            if (circles[a].0 - t0).dot(n) + circles[a].1 > 1e-4 {
                break;
            }
            active.pop();
        }
        active.push(k);
    }

    let (front_c, front_r) = circles[0];
    let (rear_c, rear_r) = *circles.last().unwrap();
    let (rear_up, front_up) = external_tangent(rear_c, rear_r, front_c, front_r, 1.0);

    let mut pts: Vec<Vec2> = vec![front_up];
    let mut tags: Vec<RouteTag> = Vec::new();
    let mut cursor = front_up;
    for w in active.windows(2) {
        let (i, j) = (w[0], w[1]);
        let (t0, t1) =
            external_tangent(circles[i].0, circles[i].1, circles[j].0, circles[j].1, -1.0);
        let toward = if i == 0 {
            Vec2::new(-1.0, 0.0) // the front drive circle wraps around its front
        } else {
            Vec2::new(0.0, -1.0) // road wheels wrap under
        };
        for p in arc(circles[i].0, circles[i].1, cursor, t0, toward) {
            push(&mut pts, &mut tags, p, RouteTag::Arc(i));
        }
        push(&mut pts, &mut tags, t1, RouteTag::Span);
        cursor = t1;
    }
    let last = circles.len() - 1;
    for p in arc(rear_c, rear_r, cursor, rear_up, Vec2::new(1.0, 0.0)) {
        push(&mut pts, &mut tags, p, RouteTag::Arc(last));
    }

    // Return run: the leftover belt length as budgeted sag over the road wheels.
    let chord = rear_up.distance(front_up);
    let excess = (belt_len - polyline_len(&pts) - chord).max(0.0);
    let roads = &circles[1..circles.len() - 1];
    let mut top: Vec<Vec2> = Vec::new();
    sag_span(rear_up, front_up, excess, roads, 0, &mut top);
    for p in top {
        push(&mut pts, &mut tags, p, RouteTag::Span);
    }
    let first = pts[0];
    push(&mut pts, &mut tags, first, RouteTag::Span);

    let mut cum = Vec::with_capacity(pts.len());
    let mut s = 0.0;
    cum.push(0.0);
    for w in pts.windows(2) {
        s += w[0].distance(w[1]);
        cum.push(s);
    }
    Route { pts, cum, tags }
}

/// Drape one return-run span with `excess` metres of slack as a parabola — and if the curve
/// dips into a road wheel, PROMOTE that wheel to a support: split the span at the wheel's top
/// and drape each side with its share of the remaining slack (the loose return run riding its
/// wheels, hanging in short spans between them — computed, not solved). Points arrive from
/// above by construction, so which side of a wheel the belt is on is given, never discovered.
pub fn sag_span(
    from: Vec2,
    to: Vec2,
    excess: f32,
    wheels: &[(Vec2, f32)],
    depth: usize,
    out: &mut Vec<Vec2>,
) {
    const SEGMENTS: usize = 16;
    let chord = from.distance(to);
    let h = (3.0 * chord * excess / 8.0).sqrt();
    // The deepest wheel the sag would enter, tested at the wheel's own abscissa.
    let mut worst: Option<(Vec2, f32)> = None;
    if depth < 4 {
        for &(c, r) in wheels {
            let (lo, hi) = (from.x.min(to.x), from.x.max(to.x));
            if c.x <= lo || c.x >= hi || (to.x - from.x).abs() < 1e-4 {
                continue;
            }
            let t = (c.x - from.x) / (to.x - from.x);
            let sag_y = from.lerp(to, t).y - 4.0 * h * t * (1.0 - t);
            let pen = (c.y + r) - sag_y;
            if pen > 1e-3 && worst.is_none_or(|(_, w)| pen > w) {
                worst = Some((Vec2::new(c.x, c.y + r), pen));
            }
        }
    }
    if let Some((split, _)) = worst {
        let (l, r) = (from.distance(split), split.distance(to));
        // The detour over the wheel top consumes slack; the remainder splits by chord share.
        let remaining = (excess - (l + r - chord)).max(0.0);
        sag_span(from, split, remaining * l / (l + r), wheels, depth + 1, out);
        sag_span(split, to, remaining * r / (l + r), wheels, depth + 1, out);
        return;
    }
    for i in 0..=SEGMENTS {
        let t = i as f32 / SEGMENTS as f32;
        let base = from.lerp(to, t);
        let mut q = Vec2::new(base.x, base.y - 4.0 * h * t * (1.0 - t));
        // Safety clip (mm-scale grazes near tangency that promotion's point-split leaves).
        for &(c, r) in wheels {
            let dz = q.x - c.x;
            if dz.abs() < r {
                q.y = q.y.max(c.y + (r * r - dz * dz).sqrt());
            }
        }
        out.push(q);
    }
}

/// The two tangent points of an external tangent line shared by two circles in a plane, on the
/// side selected by `side_sign` (−1 = lower / smaller y, +1 = upper). Returns (point on circle
/// 0, point on circle 1). Assumes neither circle contains the other (true for running gear) —
/// and per the authoring contract, COINCIDENT circles are rejected upstream (one route circle
/// per axle; interleaved discs are visual subtrees, never duplicate circles).
pub fn external_tangent(c0: Vec2, r0: f32, c1: Vec2, r1: f32, side_sign: f32) -> (Vec2, Vec2) {
    let d = c1 - c0;
    let dist = d.length().max(1e-4);
    let dir = d / dist;
    // Unit normal `n` with n·dir = (r0 − r1)/dist; the remaining component is perpendicular.
    // Pick the perpendicular sign so n points to the requested side (its y has `side_sign`).
    let along = ((r0 - r1) / dist).clamp(-1.0, 1.0);
    let perp_mag = (1.0 - along * along).max(0.0).sqrt();
    let perp = Vec2::new(-dir.y, dir.x);
    let perp = if perp.y.signum() == side_sign.signum() {
        perp
    } else {
        -perp
    };
    let n = dir * along + perp * perp_mag;
    (c0 + n * r0, c1 + n * r1)
}

/// Points along a circle's arc from `from` to `to` (both on the circle), taking whichever sweep
/// has its midpoint heading toward `toward` — so the belt wraps the *outer* side of the wheel
/// rather than cutting across. Endpoints included.
pub fn arc(center: Vec2, radius: f32, from: Vec2, to: Vec2, toward: Vec2) -> Vec<Vec2> {
    const SEGMENTS: usize = 10;
    use std::f32::consts::{PI, TAU};
    let a0 = (from - center).to_angle();
    let mut delta = (to - center).to_angle() - a0;
    // Reduce to the shortest signed sweep, then flip to the complement if it faces away.
    while delta <= -PI {
        delta += TAU;
    }
    while delta > PI {
        delta -= TAU;
    }
    if Vec2::from_angle(a0 + delta * 0.5).dot(toward) < 0.0 {
        delta -= delta.signum() * TAU;
    }
    (0..=SEGMENTS)
        .map(|i| center + Vec2::from_angle(a0 + delta * (i as f32 / SEGMENTS as f32)) * radius)
        .collect()
}

/// Total length of a polyline (sum of segment lengths).
pub fn polyline_len(pts: &[Vec2]) -> f32 {
    pts.windows(2).map(|w| w[0].distance(w[1])).sum()
}

/// Resample a polyline at uniform arc-length `spacing`, stations at arc positions
/// `offset + i·spacing` (evenly spread along the loop, not bunched at tangent vertices) — pass
/// an advancing belt phase as `offset` so the stations *travel with the belt*. Standard
/// arc-length walk; degenerate short segments are skipped.
pub fn resample(points: &[Vec2], spacing: f32, offset: f32) -> Vec<Vec2> {
    if points.len() < 2 {
        return points.to_vec();
    }
    let mut out = Vec::new();
    // Arc length remaining until the next station: the first lands at `offset` along the line.
    let mut since = spacing - offset.rem_euclid(spacing);
    if since >= spacing {
        out.push(points[0]); // offset 0: a station at the very start
        since = 0.0;
    }
    for w in points.windows(2) {
        let seg = w[1] - w[0];
        let len = seg.length();
        if len < 1e-6 {
            continue;
        }
        let dir = seg / len;
        let mut pos = 0.0;
        loop {
            let step = spacing - since;
            if pos + step > len {
                since += len - pos;
                break;
            }
            pos += step;
            since = 0.0;
            out.push(w[0] + dir * pos);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gear() -> Vec<(Vec2, f32)> {
        vec![
            (Vec2::new(-2.0, 0.5), 0.3), // front drive
            (Vec2::new(-0.8, 0.0), 0.4),
            (Vec2::new(0.8, 0.0), 0.4),
            (Vec2::new(2.0, 0.5), 0.3), // rear idler
        ]
    }

    fn taut_len(circles: &[(Vec2, f32)]) -> f32 {
        // Generous estimate via a zero-slack build.
        polyline_len(&build_route(circles, 0.0).pts)
    }

    #[test]
    fn route_is_closed_tagged_and_length_budgeted() {
        let circles = gear();
        let belt_len = taut_len(&circles) + 0.2;
        let route = build_route(&circles, belt_len);
        assert_eq!(route.pts.first(), route.pts.last(), "loop must close");
        assert!(route.pts.iter().all(|p| p.x.is_finite() && p.y.is_finite()));
        // Front drive arc and rear idler arc are present; the sag consumed the slack budget
        // (parabolic approximation: within a few percent).
        let has = |t: RouteTag| (0..route.pts.len() - 1).any(|i| route.tags[i] == t);
        assert!(has(RouteTag::Arc(0)) && has(RouteTag::Arc(3)) && has(RouteTag::Span));
        assert!((route.total() - belt_len).abs() < 0.05 * belt_len);
    }

    #[test]
    fn project_roundtrips_points_on_the_route() {
        let circles = gear();
        let route = build_route(&circles, taut_len(&circles) + 0.2);
        for s in [0.5_f32, 2.0, 4.0, 6.0] {
            let q = route.point(s);
            let (s_hat, u) = route.project(q, s, 0.5);
            assert!((route.wrap(s) - s_hat).abs() < 1e-3, "s {s} -> {s_hat}");
            assert!(u.abs() < 1e-3, "u {u}");
        }
    }

    #[test]
    fn lifted_wheel_drops_out_of_the_envelope() {
        let mut circles = gear();
        circles[1].0.y += 0.6; // articulated far above the taut line
        let route = build_route(&circles, taut_len(&circles) + 0.2);
        let wrapped: Vec<usize> = (0..route.pts.len() - 1)
            .filter_map(|i| match route.tags[i] {
                RouteTag::Arc(k) => Some(k),
                RouteTag::Span => None,
            })
            .collect();
        assert!(
            !wrapped.contains(&1),
            "a lifted wheel must not be wrapped from below"
        );
    }
}
