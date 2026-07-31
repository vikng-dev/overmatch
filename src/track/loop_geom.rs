//! Closed-loop winding and outward normals in the side plane — the one place that knows which
//! way a belt loop is wound and which side of a segment is OUT.
//!
//! Two consumers walk belt loops with different closure conventions: the shadow ribbon
//! (`super::shadow_proxy::ribbon_mesh`) gets CYCLIC joint lists (last→first edge implied), the
//! sandbox grip overlay (`track_sandbox::suspension_viz`) gets `route::build_route` polylines
//! whose last point REPEATS the first. Both used to carry their own copy of this math; a sign
//! drift between the copies would turn the shadow tube inside-out or draw grip stations inward —
//! silently, because nothing fails.

use bevy::prelude::*;

/// Sign of the loop's winding from its signed area: `+1` counter-clockwise in the side plane,
/// `-1` clockwise. Read from the polyline rather than assumed, because the wrap is free to emit
/// either winding — "outward" is a property of the loop, not of a segment.
///
/// Accepts BOTH closure conventions without being told which it got: the walk wraps `last → first`,
/// and when the last point repeats the first, that wrap edge is degenerate and contributes zero
/// area, so an explicitly-closed list sums the same edges its `windows(2)` walk would.
pub(crate) fn loop_winding(pts: &[Vec2]) -> f32 {
    let n = pts.len();
    let twice_area: f32 = (0..n)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum();
    if twice_area >= 0.0 { 1.0 } else { -1.0 }
}

/// Unit outward normal of the segment `a -> b` for a loop of the given winding. For a CCW loop the
/// outward normal is the right-hand normal `(t.y, -t.x)`; a CW loop flips it.
pub(crate) fn outward_normal(a: Vec2, b: Vec2, winding: f32) -> Vec2 {
    let t = (b - a).normalize_or_zero();
    Vec2::new(t.y, -t.x) * winding
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A CCW unit square in the side plane, WITHOUT a repeated closing point (the cyclic
    /// convention the shadow ribbon feeds).
    fn square_ccw_cyclic() -> Vec<Vec2> {
        vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
        ]
    }

    /// The same square WITH the explicit repeated closing point (the `route::build_route`
    /// convention the sandbox overlay feeds).
    fn square_ccw_closed() -> Vec<Vec2> {
        let mut pts = square_ccw_cyclic();
        pts.push(pts[0]);
        pts
    }

    /// The load-bearing property of the consolidation: both closure conventions, in both
    /// traversal directions, agree on the winding sign.
    #[test]
    fn winding_sign_follows_the_traversal_under_both_closure_conventions() {
        for make in [square_ccw_cyclic, square_ccw_closed] {
            let ccw = make();
            let mut cw = ccw.clone();
            cw.reverse();
            assert_eq!(loop_winding(&ccw), 1.0);
            assert_eq!(loop_winding(&cw), -1.0);
        }
    }

    /// The property every consumer rests on: the normals must point AWAY from the loop, in either
    /// winding and under either closure convention. Get this backwards and the shadow tube hugs
    /// the wheels instead of the ground, or the grip columns draw inside the belt.
    #[test]
    fn outward_normal_points_out_of_the_loop_in_both_windings() {
        for make in [square_ccw_cyclic, square_ccw_closed] {
            let ccw = make();
            let mut cw = ccw.clone();
            cw.reverse();
            for pts in [ccw, cw] {
                let winding = loop_winding(&pts);
                let centre = Vec2::new(0.5, 0.5);
                for w in pts.windows(2) {
                    if w[0] == w[1] {
                        continue; // the degenerate closing edge has no defined normal
                    }
                    let out = outward_normal(w[0], w[1], winding);
                    let mid = (w[0] + w[1]) * 0.5;
                    assert!(
                        out.dot(mid - centre) > 0.0,
                        "normal {out} on segment {}->{} points inward",
                        w[0],
                        w[1],
                    );
                }
            }
        }
    }
}
