//! The route core (architecture §1/§2): the tagged taut guide route of one track side, as pure
//! geometry over the side-plane circles of its running gear. Single source of truth for "where
//! the track is" — the kinematic-wrap view fits the drawn belt around it and (phase B) the belt
//! physics samples along it.
//!
//! Conventions (inherited from the sandbox, steps 17–25): everything is in the hull-local SIDE
//! PLANE as (z, y) `Vec2`s; the loop runs CCW in that plane (lower run front→rear, return run
//! rear→front); a segment's OUTWARD normal is `(tan.y, −tan.x)`. Circles are on the PIN line
//! (wheel radius + half track thickness) and are given front→rear.

use bevy::math::Vec2;

/// The taut guide route, kept as a polyline plus its arc-length table. Built fresh from the
/// CURRENT articulated circles whenever consumed (shape-as-function; no state).
pub struct Route {
    pub pts: Vec<Vec2>,
    /// Cumulative arc length at each vertex; last = total loop length.
    cum: Vec<f32>,
}

impl Route {
    pub fn total(&self) -> f32 {
        *self.cum.last().unwrap()
    }
}

/// Build the guide route from one side's CURRENT circles (front→rear, pin-line radii): the lower
/// convex envelope + external tangents + budgeted top-run sag. Closed: last point == first point.
/// `belt_len` is the loop length budget; its excess over the taut perimeter drapes into the return
/// run.
pub fn build_route(circles: &[(Vec2, f32)], belt_len: f32) -> Route {
    fn push(pts: &mut Vec<Vec2>, p: Vec2) {
        if pts.last().is_none_or(|l| l.distance_squared(p) > 1e-10) {
            pts.push(p);
        }
    }

    let (front_c, front_r) = circles[0];
    let (rear_c, rear_r) = *circles.last().unwrap();
    let (rear_up, front_up) = external_tangent(rear_c, rear_r, front_c, front_r, 1.0);

    // The EXACT tangent point seeds the polyline; the walk's own arc reconstruction of it then
    // dedupes away, so pts[0] stays bit-exact `front_up`.
    let mut pts: Vec<Vec2> = vec![front_up];
    taut_lower_run(circles, front_up, rear_up, |p| push(&mut pts, p));

    // Return run: the leftover belt length as budgeted sag over the road wheels.
    let chord = rear_up.distance(front_up);
    let excess = (belt_len - polyline_len(&pts) - chord).max(0.0);
    let roads = &circles[1..circles.len() - 1];
    let mut top: Vec<Vec2> = Vec::new();
    sag_span(rear_up, front_up, excess, roads, 0, &mut top);
    for p in top {
        push(&mut pts, p);
    }
    let first = pts[0];
    push(&mut pts, first);

    let mut cum = Vec::with_capacity(pts.len());
    let mut s = 0.0;
    cum.push(0.0);
    for w in pts.windows(2) {
        s += w[0].distance(w[1]);
        cum.push(s);
    }
    Route { pts, cum }
}

/// The taut LOWER run — THE shared builder of "where the belt's bottom is": the lower convex
/// envelope over the ordered circles (Graham-style scan) chained into tangent segments and wrap
/// arcs, walked front→rear. [`build_route`] (the sim/route consumer) and the kinematic wrap's
/// `raw_belly` (`super::wrap`) both run exactly this; neither re-implements the scan or the walk.
///
/// A circle whose body stays above its neighbours' lower tangent is not part of the taut run and
/// drops out — a lifted wheel is skipped, never wrapped from the wrong side.
///
/// Points are EMITTED through `emit` rather than collected, because the two consumers sink them
/// differently and each sink is bit-load-bearing: `build_route` dedupes near-coincident vertices
/// (its `push`) onto a polyline seeded with the exact `front_up` tangent point, while the wrap
/// keeps every raw arc point (its conform resample counts on the raw spacing). `front_up` /
/// `rear_up` are the upper external tangent points between the LAST circle and the FIRST
/// (`external_tangent(rear.., front.., 1.0)`, computed by the caller, which needs them for its
/// seed/closing anyway): the walk starts its front arc at `front_up` and ends its rear arc at
/// `rear_up`. Emission starts with the front arc's own reconstruction of `front_up` (endpoints
/// included), then alternates arcs and tangent points to `rear_up`.
pub fn taut_lower_run(
    circles: &[(Vec2, f32)],
    front_up: Vec2,
    rear_up: Vec2,
    mut emit: impl FnMut(Vec2),
) {
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

    let (rear_c, rear_r) = *circles.last().unwrap();
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
            emit(p);
        }
        emit(t1);
        cursor = t1;
    }
    for p in arc(rear_c, rear_r, cursor, rear_up, Vec2::new(1.0, 0.0)) {
        emit(p);
    }
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

    /// FNV-1a 64 over the exact little-endian f32 bits of every route vertex and cumulative
    /// arc-length entry — the whole observable output of [`build_route`].
    fn route_bits_hash(route: &Route) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        let mut write = |v: f32| {
            for byte in v.to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        for p in &route.pts {
            write(p.x);
            write(p.y);
        }
        for i in 0..route.pts.len() {
            write(route.cum[i]);
        }
        hash
    }

    /// BIT-IDENTITY PIN for the two wire-adjacent productions of [`build_route`] on the
    /// shipped Tiger rig: the material-length loop that becomes `TrackGear::loop_pts` (sim
    /// force stations — replicated-state adjacent) and the taut (`belt_len = 0`) wrap whose
    /// belly minimum is `RigGeom::hull_rest_y` (the spawn datum). Captured BEFORE the shared
    /// taut-walk extraction and required to hold bit-for-bit after it: any refactor of the
    /// envelope scan / tangent-arc chaining that moves one bit here is a sim change, not a
    /// cleanup. (Asset re-exports legitimately move these values — re-pin from the printed
    /// actuals when the GLB or authored counts change on purpose.)
    #[test]
    fn build_route_is_bit_stable_on_the_shipped_tiger_rig() {
        use crate::track::side::Side;
        let rig = crate::track::rig_geom::tiger_rig();
        let circles = rig.rest.get(Side::Right);

        let loop_route = build_route(circles, rig.belt_len());
        let taut_route = build_route(circles, 0.0);
        let belly_y = taut_route
            .pts
            .iter()
            .map(|p| p.y)
            .fold(f32::INFINITY, f32::min);

        println!(
            "tiger route pin: loop pts {} hash {:#018x} total bits {:#010x} | taut pts {} \
             hash {:#018x} belly bits {:#010x}",
            loop_route.pts.len(),
            route_bits_hash(&loop_route),
            loop_route.total().to_bits(),
            taut_route.pts.len(),
            route_bits_hash(&taut_route),
            belly_y.to_bits(),
        );

        assert_eq!(loop_route.pts.len(), 172, "loop route vertex count moved");
        assert_eq!(
            route_bits_hash(&loop_route),
            0x553a_c4ba_777d_a774,
            "loop route bits moved (TrackGear::loop_pts would change)"
        );
        assert_eq!(
            loop_route.total().to_bits(),
            0x414a_9637,
            "loop length bits moved"
        );
        assert_eq!(taut_route.pts.len(), 60, "taut route vertex count moved");
        assert_eq!(
            route_bits_hash(&taut_route),
            0x9b28_2f46_9bbb_413e,
            "taut route bits moved (hull_rest_y datum would change)"
        );
        assert_eq!(
            belly_y.to_bits(),
            0x3dc5_1704,
            "hull_rest_y source bits moved"
        );
    }

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

    /// Closest approach of the route's LOWER run to a circle's centre, in radii: `1.0` means the
    /// route lies ON the circle (it wraps it from below), `> 1.0` means it clears it entirely.
    /// Only vertices at or below the centre count — the return run passes over the end circles and
    /// says nothing about whether the taut bottom wraps them.
    fn lower_wrap_ratio(route: &Route, (c, r): (Vec2, f32)) -> f32 {
        route
            .pts
            .iter()
            .filter(|p| p.y <= c.y)
            .map(|p| p.distance(c) / r)
            .fold(f32::INFINITY, f32::min)
    }

    #[test]
    fn route_is_closed_wraps_its_end_circles_and_is_length_budgeted() {
        let circles = gear();
        let belt_len = taut_len(&circles) + 0.2;
        let route = build_route(&circles, belt_len);
        assert_eq!(route.pts.first(), route.pts.last(), "loop must close");
        assert!(route.pts.iter().all(|p| p.x.is_finite() && p.y.is_finite()));
        // The front drive circle and the rear idler are always wrapped — the route sits ON them.
        for end in [circles[0], circles[3]] {
            let ratio = lower_wrap_ratio(&route, end);
            assert!(
                (ratio - 1.0).abs() < 1e-3,
                "an end circle must be wrapped, closest approach was {ratio}x its radius"
            );
        }
        // The sag consumed the slack budget (parabolic approximation: within a few percent).
        assert!((route.total() - belt_len).abs() < 0.05 * belt_len);
    }

    #[test]
    fn lifted_wheel_drops_out_of_the_envelope() {
        let mut circles = gear();
        circles[1].0.y += 0.6; // articulated far above the taut line
        let route = build_route(&circles, taut_len(&circles) + 0.2);
        let ratio = lower_wrap_ratio(&route, circles[1]);
        assert!(
            ratio > 1.05,
            "a lifted wheel must not be wrapped from below — the route came within {ratio}x \
             its radius"
        );
    }
}
