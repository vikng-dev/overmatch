//! Pure side-plane route geometry (architecture §1/§2). Belt physics samples the taut guide;
//! the kinematic view shares its builders ([`taut_lower_run`], [`sag_span`], [`arc`], [`resample`]).
//!
//! Ground-contact geometry (lower envelope, tangents, wrap arcs) is bit-identical. Return runs
//! differ deliberately ([`SagClip`]): the sim clips only road wheels to avoid unbudgeted length
//! and phantom element strain; the view clips every circle and absorbs length as uniform strain
//! through `wrap::station_params`. Shared builders do not imply identical return polylines.
//!
//! Hull-local side plane: `(z, y)` `Vec2`s, CCW (lower front→rear, return rear→front), outward
//! normal `(tan.y, −tan.x)`. Circles use pin-line radii (wheel radius + half track thickness),
//! ordered front→rear.

use bevy::math::Vec2;

use super::derive::wrap_joint_angle;

/// Stateless guide polyline and arc-length table, rebuilt from current articulated circles.
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

    // Sim length-reference policy: see `SagClip::RoadWheels`.
    let chord = rear_up.distance(front_up);
    let excess = slack(belt_len, polyline_len(&pts), chord);
    let mut top: Vec<Vec2> = Vec::new();
    sag_span(
        rear_up,
        front_up,
        excess,
        circles,
        SagClip::RoadWheels,
        &mut top,
    );
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

/// Shared lower convex envelope: Graham-style circle scan, external tangents and wrap arcs,
/// walked front→rear. Wheels above neighbours' lower tangents drop out rather than wrap wrong-side.
/// See `.agents/docs/design/track-model/architecture.md` §2 for the two-consumer design.
///
/// Emit into consumer-owned sinks: [`build_route`] dedupes onto an exact `front_up` seed; the wrap
/// keeps raw arc spacing for conform resampling. `front_up` / `rear_up` are the upper external
/// tangent points `external_tangent(rear.., front.., 1.0)`, also used for seeding/closing.
/// Emission includes the front arc's reconstruction of `front_up`, then alternates arcs/tangents
/// through `rear_up`, endpoints included.
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

/// Drape clipping policy for [`sag_span`]. Not taste: clipping adds unbudgeted length,
/// and sim/view consumers have different length constraints.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SagClip {
    /// Road wheels only, by vertical projection of the drape's own uniform samples — the drape may
    /// still sink into the two circles its own endpoints sit on.
    ///
    /// Sim length reference ([`build_route`]): the return carries no ground contact, so dips are
    /// invisible but added length feeds element strain. Road-wheel grazes are mm-scale.
    /// Clipping ends adds MEASURED ~11 mm on the Tiger's MEASURED 12.65 m loop (DERIVED 0.09 %
    /// phantom strain), turning its MEASURED 0.1 mm latched 20° hill hold into a >50 mm slide.
    /// Extra ride vertices also cost length, so faithful riding belongs only to `EveryCircle`.
    RoadWheels,
    /// Drawn-belt policy (`super::wrap`): ride every circle, ends included.
    /// A drape leaves tangency downward into its endpoint's circle, which support promotion
    /// cannot catch (end circles are not candidates). Unclipped shoes sank MEASURED 33 mm into
    /// the shipped Tiger's idler and MEASURED 48 mm into its sprocket across the travel band, visible
    /// with textured links. Riding the upper hemisphere models a slack track leaving a wheel.
    ///
    /// Fixed-sample projection clears vertices only: chords still cut circles and arbitrary
    /// entry angles kink. Insert closed-form [`ride_bracket`] ends and walk the hemisphere at
    /// chords no longer than [`sag_clip_chord`]; [`sag_span`] states the bound.
    /// Only this policy clips sprocket/idler circles, requiring endpoint ownership exemptions;
    /// `RoadWheels` clips every emitted sample, endpoints included.
    ///
    /// `wrap::station_params` absorbs extra drawn length as uniform strain, sampling pins in
    /// material arc length without sprocket phase drift. The sim cannot afford that length.
    EveryCircle,
}

/// Resolution: max inward ride-chord cut (m), shared with the promotion search. DERIVED
/// 1 mm "not a penetration" threshold: clip discretisation must not leave a cut
/// the support search would call a penetration.
pub(crate) const SAG_CLIP_INSET: f32 = 1e-3;

/// Inward depth of a straight chord `c` across radius `r`: `r − √(r² − c²/4)`.
/// All track-view clearance budgets use this measure; [`sag_clip_chord`] is its inverse.
pub fn chord_inset(r: f32, c: f32) -> f32 {
    r - (r * r - 0.25 * c * c).max(0.0).sqrt()
}

/// Longest ride chord whose [`chord_inset`] is [`SAG_CLIP_INSET`]. DERIVED inverse:
/// `inset = r − √(r² − c²/4)` ⇒ `c = 2·√(2·r·d − d²)`.
/// Public contract: `wrap`'s drawn-clearance sweep derives its budget from this chord, keeping
/// asserted clearance and produced spacing tied to the same number.
pub fn sag_clip_chord(r: f32) -> f32 {
    2.0 * (2.0 * r * SAG_CLIP_INSET - SAG_CLIP_INSET * SAG_CLIP_INSET)
        .max(0.0)
        .sqrt()
}

/// Mid-span drape depth (m): `h = √(3·chord·excess/8)` for chord/slack in metres.
/// Owns the shape parameter used by [`sag_span`], `wrap::SlackSpring`'s pendulum rate and tests.
pub fn sag_depth(chord: f32, excess: f32) -> f32 {
    (3.0 * chord * excess / 8.0).sqrt()
}

/// Return-run slack: `belt_len − path_len − chord`, floored at zero. An overlong path runs
/// the return taut; a length deficit must not enter the drape's shape (infeasibility rule).
pub fn slack(belt_len: f32, path_len: f32, chord: f32) -> f32 {
    (belt_len - path_len - chord).max(0.0)
}

/// The drape parabola's ordinate at chord parameter `t`: the chord's own lerp pulled down by
/// `4h·t(1−t)`.
fn sag_y(from: Vec2, to: Vec2, h: f32, t: f32) -> f32 {
    from.lerp(to, t).y - 4.0 * h * t * (1.0 - t)
}

/// Upper hemisphere y at `x`, or `None` at `|x − c.x| ≥ r`. Do not clamp to `c.y`:
/// samples outside the circle's x-range must never be raised by it.
fn hemisphere_y(c: Vec2, r: f32, x: f32) -> Option<f32> {
    let dx = x - c.x;
    (dx.abs() < r).then(|| c.y + (r * r - dx * dx).sqrt())
}

/// Boundary-inclusive [`hemisphere_y`]: `|x − c.x| = r` yields the corner `c.y`.
/// Clamped sqrt absorbs a [`ride_bracket`] end rounding a float or two outside during `t ↔ x`.
/// Strict projection drops that corner below the hemisphere (MEASURED 2.97 mm cut vs 1 mm budget
/// on equal-radius span; `a_bracket_end_on_a_circles_lateral_boundary_still_rides_out`). The
/// caller's `max` leaves parabola samples above `c.y` unchanged.
///
/// Call only within the circle's exact ride bracket: a rejected spatial tolerance band raised
/// a sample DERIVED 5 µm outside by 0.25 m
/// (`a_sample_just_outside_a_circle_is_not_lifted_to_its_corner`).
/// [`SagClip::RoadWheels`] has no brackets and keeps strict [`hemisphere_y`], preserving sim bits.
fn silhouette_y(c: Vec2, r: f32, x: f32) -> f32 {
    let dx = x - c.x;
    c.y + (r * r - dx * dx).max(0.0).sqrt()
}

/// Bracket membership widened by one float per end: exactly [`sag_samples`]' dedup merge radius.
/// Dedup keeps the earlier adjacent abscissa, so exact membership could drop a merged corner.
/// No wider: DERIVED one float is ~1e-7 at hull scale, while the false-lift parabola sample is
/// ~5 µm away, dozens of floats (`a_sample_just_outside_a_circle_is_not_lifted_to_its_corner`).
fn bracket_admits(bracket: Option<(f32, f32)>, t: f32) -> bool {
    bracket.is_some_and(|(t0, t1)| (t0.next_down()..=t1.next_up()).contains(&t))
}

/// Circle membership within [`SPAN_END_ON_CIRCLE`], shared by drape endpoint ownership and
/// the view's wrap-arc chord measurement so their tolerances cannot drift.
pub fn on_circle(p: Vec2, (c, r): (Vec2, f32)) -> bool {
    (p.distance(c) - r).abs() <= SPAN_END_ON_CIRCLE
}

/// Resolution: uniform parabola discretisation per span, before any
/// [`SagClip::EveryCircle`] refinement): `SAG_SEGMENTS + 1` abscissas at `t = i / SAG_SEGMENTS`.
const SAG_SEGMENTS: usize = 16;

/// Resolution: radial endpoint-ownership tolerance for [`sag_span`]. DERIVED two-sided margin:
/// 10 µm is two orders above tangent/promoted-top float noise (~1e-7 m at running-gear radii)
/// and below [`SAG_CLIP_INSET`]. Mis-ownership can leave at most this radial cut at an endpoint.
const SPAN_END_ON_CIRCLE: f32 = 1e-5;

/// Closed-form chord-parameter interval an [`SagClip::EveryCircle`] ride covers on one circle;
/// `None` means clear to within [`SAG_CLIP_INSET`]. With parabola ordinate `P(t)`, the clip's
/// vertical gap is `pen(t) = hemisphere(x(t)) − P(t)`.
/// A point at gap `pen` lies at most `pen` inside the circle:
/// `dist² = r² − 2·pen·√(r²−u²) + pen² ≥ (r − pen)²`.
/// Thus a scalar vertical bound also bounds radial cut.
///
/// The bracket is derived from the OSCULATING PARABOLA of the circle top, `r − dx²/(2r)`, which
/// over-estimates the hemisphere everywhere (`√(1−s) ≤ 1 − s/2`). So
///
/// ```text
///   pen(t) > d   ⟹   P(t) < c.y + √(r² − dx²) − d   ⟹   P(t) + dx(t)²/(2r) < c.y + r − d
/// ```
///
/// The right side is quadratic in `t` (`P` quadratic, `dx` affine), with positive leading
/// coefficient. Solve its single interval by the quadratic formula, intersected with the circle's
/// x-range and the span. This superset of `{pen > d}` has no sampling gaps or bisection.
/// The same scan-vs-closed-form trade applies to [`super::oracle`]: a rejected 8-segment
/// sign-change scan + 24-step bisection can only guarantee a detection bound.
///
/// Set `d = SAG_CLIP_INSET`, not zero: the guarantee is no cut deeper than the support search's
/// 1 mm threshold, not detection of every penetration. Outside the bracket `pen ≤ d`; samples
/// and chords of the convex parabola sit `≥ P`, bounding drawn segments by `d` vertically and
/// hence radially. [`sag_span`] composes this with the bound inside the bracket.
///
/// Zero threshold would refine even taut returns: a tangent of slope `m` dips below the
/// osculating parabola across width `2r(√(1+m²) − 1)`. The `d` term swamps this
/// (`(√(1+m²)−1)² ≪ 2d/r` at running-gear slopes), preserving zero-slack, refinement-free,
/// bit-identical drapes under either policy.
fn ride_bracket(from: Vec2, to: Vec2, h: f32, c: Vec2, r: f32) -> Option<(f32, f32)> {
    let dx = to.x - from.x;
    // The circle's own abscissa range, intersected with the span.
    let (a, b) = ((c.x - r - from.x) / dx, (c.x + r - from.x) / dx);
    let (mut t0, mut t1) = (a.min(b).max(0.0), a.max(b).min(1.0));
    if !t0.is_finite() || !t1.is_finite() || t1 <= t0 {
        return None;
    }
    // `P(t) + dx(t)²/(2r) − (c.y + r − d) < 0`, collected in `t`:
    //   P(t)  = from.y + (Δy − 4h)·t + 4h·t²        dx(t) = (from.x − c.x) + dx·t
    let k = 0.5 / r;
    let e = from.x - c.x;
    let qa = 4.0 * h + k * dx * dx; // > 0: the span is non-degenerate and `r` is bounded below
    let qb = (to.y - from.y) - 4.0 * h + 2.0 * k * e * dx;
    let qc = from.y + k * e * e - (c.y + r - SAG_CLIP_INSET);
    let disc = qb * qb - 4.0 * qa * qc;
    if disc <= 0.0 {
        return None; // the drape stays clear of the circle by more than the ride budget
    }
    let root = disc.sqrt();
    t0 = t0.max((-qb - root) / (2.0 * qa));
    t1 = t1.min((-qb + root) / (2.0 * qa));
    (t1 > t0).then_some((t0, t1))
}

/// Parabolic return span with `excess` metres of slack. Promote an entered road wheel to a
/// support at its top; split there and divide remaining slack by chord share. Subspans approach
/// wheels from above by construction: no side search or iterative solve.
///
/// `circles` is the full front→rear list. Supports are only `circles[1..last]`: sprocket/idler
/// tangencies already own span ends, so promoting them would create zero-length subspans and
/// divide slack by zero. Clipping uses the caller's distinct [`SagClip`] set.
///
/// # What the emitted polyline guarantees about the circles it rides
///
/// [`SagClip::EveryCircle`] bounds every drawn segment's cut by [`SAG_CLIP_INSET`]:
///
/// * inside [`ride_bracket`], walk the hemisphere at chords ≤ [`sag_clip_chord`]. Emitting
///   `max(parabola, hemispheres)` can only raise a segment off any circle;
/// * outside, `pen ≤ SAG_CLIP_INSET`; samples and chords sit above the convex parabola, so
///   vertical and radial cuts stay within that budget.
///
/// Bracket ends are samples, so no segment straddles the two proofs. Other circles' samples only
/// subdivide steps; merging/overlaps cannot lengthen them.
///
/// A penetration running off the span domain clamps its ride to an endpoint, possibly its deepest
/// point. The ownership rule below makes every non-owning circle lift that endpoint, so even an
/// off-domain ride starts on the circle rather than inside it.
///
/// # Who may hand a span endpoint through unclipped
///
/// [`SagClip::EveryCircle`] passes an endpoint verbatim only through the circle it sits on
/// ([`SPAN_END_ON_CIRCLE`]). Test position, not index: promoted subspan ends sit on road wheels.
/// Tangent points/promoted tops must join the preceding run exactly; recomputing
/// `c.y + √(r² − dz²)` could round away a ULP from the exact input.
/// All other circles clip it: a blanket exemption leaves off-domain penetration's deepest point
/// inside a non-owner. Raising this attachment changes the preceding join only when that join
/// was already cutting a circle it did not belong to.
///
/// [`SagClip::RoadWheels`] projects all samples, including endpoints. Its only endpoint-bearing
/// circles are promoted road wheels, whose tops project back to themselves.
pub fn sag_span(
    from: Vec2,
    to: Vec2,
    excess: f32,
    circles: &[(Vec2, f32)],
    clip: SagClip,
    out: &mut Vec<Vec2>,
) {
    sag_span_at(from, to, excess, circles, clip, 0, out);
}

/// Keep promotion recursion depth off [`sag_span`]'s public signature; callers always start at 0.
fn sag_span_at(
    from: Vec2,
    to: Vec2,
    excess: f32,
    circles: &[(Vec2, f32)],
    clip: SagClip,
    depth: usize,
    out: &mut Vec<Vec2>,
) {
    let chord = from.distance(to);
    let h = sag_depth(chord, excess);
    // Support candidates: road wheels only (see above). Also the `RoadWheels` clip set.
    let wheels = circles
        .get(1..circles.len().saturating_sub(1))
        .unwrap_or_default();
    // The deepest wheel the sag would enter, tested at the wheel's own abscissa.
    let mut worst: Option<(Vec2, f32)> = None;
    if depth < 4 {
        for &(c, r) in wheels {
            let (lo, hi) = (from.x.min(to.x), from.x.max(to.x));
            if c.x <= lo || c.x >= hi || (to.x - from.x).abs() < 1e-4 {
                continue;
            }
            let t = (c.x - from.x) / (to.x - from.x);
            let pen = (c.y + r) - sag_y(from, to, h, t);
            if pen > SAG_CLIP_INSET && worst.is_none_or(|(_, w)| pen > w) {
                worst = Some((Vec2::new(c.x, c.y + r), pen));
            }
        }
    }
    if let Some((split, _)) = worst {
        let (l, r) = (from.distance(split), split.distance(to));
        // The detour over the wheel top consumes slack; the remainder splits by chord share.
        let remaining = (excess - (l + r - chord)).max(0.0);
        let (left, right) = (remaining * l / (l + r), remaining * r / (l + r));
        sag_span_at(from, split, left, circles, clip, depth + 1, out);
        sag_span_at(split, to, right, circles, clip, depth + 1, out);
        return;
    }
    let clipped = match clip {
        SagClip::RoadWheels => wheels,
        SagClip::EveryCircle => circles,
    };
    // Share brackets between refinement and corner projection; `silhouette_y` owns why exact
    // membership matters. `RoadWheels` has none: uniform samples and strict projection preserve
    // sim bits. Radius ≤ inset needs no ride (cannot cut deeper), avoiding the bracket's `1/(2r)`.
    let brackets: Vec<Option<(f32, f32)>> = match clip {
        SagClip::RoadWheels => Vec::new(),
        SagClip::EveryCircle => clipped
            .iter()
            .map(|&(c, r)| {
                (r > SAG_CLIP_INSET)
                    .then(|| ride_bracket(from, to, h, c, r))
                    .flatten()
            })
            .collect(),
    };
    let ts = sag_samples(from, to, clipped, &brackets);
    let last = ts.len() - 1;
    for (i, &t) in ts.iter().enumerate() {
        let mut q = Vec2::new(from.lerp(to, t).x, sag_y(from, to, h, t));
        // Capture before any lift: ownership tests the caller's point, not another circle's
        // moved point. `RoadWheels` sets no owner (see `sag_span`).
        let end = (clip == SagClip::EveryCircle && (i == 0 || i == last)).then_some(q);
        // Safety clip (mm-scale grazes near tangency that promotion's point-split leaves) — and,
        // under `EveryCircle`, the whole dip out of a tangent point promotion cannot see.
        for (j, &(c, r)) in clipped.iter().enumerate() {
            // `sag_span` owns the per-circle endpoint exemption.
            if end.is_some_and(|p| on_circle(p, (c, r))) {
                continue;
            }
            // Only bracket members get boundary-inclusive projection (`silhouette_y`);
            // `bracket_admits` matches the dedup radius. Others, including all sim samples,
            // use strict `hemisphere_y` so nearby non-bracket samples cannot be lifted.
            let lifted = if bracket_admits(brackets.get(j).copied().flatten(), t) {
                Some(silhouette_y(c, r, q.x))
            } else {
                hemisphere_y(c, r, q.x)
            };
            if let Some(y) = lifted {
                q.y = q.y.max(y);
            }
        }
        out.push(q);
    }
}

/// Sample `t ∈ [0, 1]`: [`SAG_SEGMENTS`] uniform steps, unchanged for [`SagClip::RoadWheels`].
/// [`SagClip::EveryCircle`] adds each nonempty [`ride_bracket`]'s ends and a hemisphere walk
/// spaced by [`sag_clip_chord`]. Empty brackets are clear within budget and add no samples.
/// Step in angle, not x: equal x steps give unbounded chords near vertical `c.x ± r`.
/// [`wrap_joint_angle`] supplies the chord/angle relation shared with sprocket meshing and bounds
/// work by `π / step`, requiring no cap. Merging circles only subdivides steps, preserving bounds.
/// Dedup bracket ends coincident with uniform samples/span ends; re-pin 0/1 because callers pass
/// endpoints through unclipped.
fn sag_samples(
    from: Vec2,
    to: Vec2,
    clipped: &[(Vec2, f32)],
    brackets: &[Option<(f32, f32)>],
) -> Vec<f32> {
    let uniform = || (0..=SAG_SEGMENTS).map(|i| i as f32 / SAG_SEGMENTS as f32);
    let dx = to.x - from.x;
    // No brackets means sim; x-degenerate spans have no single-valued hemisphere to ride.
    // Both keep uniform samples and the caller's vertical projection.
    if brackets.is_empty() || dx.abs() < 1e-4 {
        return uniform().collect();
    }
    let mut ts: Vec<f32> = uniform().collect();
    for (&(c, r), bracket) in clipped.iter().zip(brackets) {
        // Clear within budget or too small to exceed it; see `sag_span_at`.
        let Some((t0, t1)) = *bracket else {
            continue;
        };
        ts.push(t0);
        ts.push(t1);
        let angle = |t: f32| ((from.x + dx * t - c.x) / r).clamp(-1.0, 1.0).asin();
        let (a0, a1) = (angle(t0), angle(t1));
        let step = wrap_joint_angle(sag_clip_chord(r), r);
        let n = if step > 0.0 {
            ((a1 - a0).abs() / step).ceil().max(1.0) as usize
        } else {
            1
        };
        for k in 1..n {
            let a = a0 + (a1 - a0) * k as f32 / n as f32;
            ts.push((c.x + r * a.sin() - from.x) / dx);
        }
    }
    for t in &mut ts {
        *t = t.clamp(0.0, 1.0);
    }
    ts.sort_by(|a, b| a.total_cmp(b));
    // Merge equal/adjacent floats only: they have no interior geometry (coincident bracket
    // ends/uniform samples). Microns-apart ends are distinct shape; merging drops their ride.
    // Remaining sub-micron segments are cheap: `resample` skips degenerates, `polyline_len` sums.
    ts.dedup_by(|a, b| *a <= b.next_up());
    // A neighbouring release abscissa may win dedup; restore exact endpoints for unclipped joins.
    ts[0] = 0.0;
    *ts.last_mut().expect("the uniform samples are never empty") = 1.0;
    ts
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

/// Resolution: fixed chords per [`arc`]. Cut depth depends on sweep angle: a road wheel's few
/// degrees cost microns; an end circle's CLAIMED ~165° costs a link length.
/// `wrap` clearance and `rig_geom` enclosure budgets derive from this chord via [`chord_inset`].
pub const ARC_SEGMENTS: usize = 10;

/// Points along a circle's arc from `from` to `to` (both on the circle), taking whichever sweep
/// has its midpoint heading toward `toward` — so the belt wraps the *outer* side of the wheel
/// rather than cutting across. Endpoints included.
pub fn arc(center: Vec2, radius: f32, from: Vec2, to: Vec2, toward: Vec2) -> Vec<Vec2> {
    use std::f32::consts::{PI, TAU};
    let a0 = (from - center).to_angle();
    let mut delta = (to - center).to_angle() - a0;
    while delta <= -PI {
        delta += TAU;
    }
    while delta > PI {
        delta -= TAU;
    }
    if Vec2::from_angle(a0 + delta * 0.5).dot(toward) < 0.0 {
        delta -= delta.signum() * TAU;
    }
    (0..=ARC_SEGMENTS)
        .map(|i| center + Vec2::from_angle(a0 + delta * (i as f32 / ARC_SEGMENTS as f32)) * radius)
        .collect()
}

/// Inward displacement cap along `−out` before entry into `circles`, for terrain conform.
/// Returns infinity for a zero direction or no entering intersection (tangencies included);
/// zero for an interior point or a boundary point whose ray enters the circle.
/// The lower-run counterpart to [`ride_bracket`]: drawn belt supports the union of wheels/ground,
/// so conform must not push stations inside wheels. Both leave inter-station polygon cuts to
/// [`chord_inset`] budgets, including the drape's [`SAG_CLIP_INSET`].
/// With `v = point − c`, `|out| = 1`: `|v − out·d|² = r²` gives
/// `d² − 2(v·out)·d + (|v|² − r²) = 0`; use the first non-negative entry root.
pub fn max_admissible_depth(point: Vec2, out: Vec2, circles: &[(Vec2, f32)]) -> f32 {
    if out == Vec2::ZERO {
        return f32::INFINITY;
    }
    let mut cap = f32::INFINITY;
    for &(c, r) in circles {
        let v = point - c;
        let b = v.dot(out);
        let disc = b * b - (v.length_squared() - r * r);
        if disc <= 0.0 {
            continue; // the inward ray misses this circle entirely
        }
        let root = disc.sqrt();
        // The far root is where the ray would LEAVE the circle: behind the station ⇒ no constraint.
        if b + root <= 0.0 {
            continue;
        }
        cap = cap.min((b - root).max(0.0));
    }
    cap
}

pub fn polyline_len(pts: &[Vec2]) -> f32 {
    pts.windows(2).map(|w| w[0].distance(w[1])).sum()
}

/// Uniform arc-length stations at `offset + i·spacing`, not bunched at tangent vertices.
/// An advancing belt-phase offset moves stations with the belt; skip degenerate short segments.
pub fn resample(points: &[Vec2], spacing: f32, offset: f32) -> Vec<Vec2> {
    if points.len() < 2 {
        return points.to_vec();
    }
    let mut out = Vec::new();
    // Arc length since the previous station; spacing − offset puts the first at offset.
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

/// Deepest segment cut into a circle (0 = no entry), the rendered quantity
/// [`SagClip::EveryCircle`] promises to bound. A vertex-only measure misses chord cuts.
/// Shared by route and `wrap` tests: the same measurement on different polylines, not separate
/// maintained copies (precedent: [`super::derive::wrap_joint_angle`]).
#[cfg(test)]
pub fn deepest_inside(pts: &[Vec2], (c, r): (Vec2, f32)) -> f32 {
    pts.windows(2)
        .map(|w| {
            let ab = w[1] - w[0];
            let t = ((c - w[0]).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
            r - c.distance(w[0] + ab * t)
        })
        .fold(0.0, f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    use deepest_inside as deepest;

    /// Drawn-belt consumers require every segment, not just vertices, within [`SAG_CLIP_INSET`].
    /// The shipped fixture's return sweep compares `EveryCircle` with `RoadWheels`: MEASURED
    /// end-circle cuts 6.7–63.1 mm (hair-to-quarter-metre slack), one to two orders beyond budget
    /// and growing with slack; MEASURED ridden cuts 0.54–0.97 mm, below 1 mm at every sweep step.
    #[test]
    fn the_every_circle_ride_bounds_the_chord_not_just_the_vertices() {
        use crate::track::side::Side;
        let rig = crate::track::rig_geom::tiger_rig();
        let circles = rig.rest.get(Side::Right);
        let (front_c, front_r) = circles[0];
        let (rear_c, rear_r) = *circles.last().unwrap();
        let (rear_up, front_up) = external_tangent(rear_c, rear_r, front_c, front_r, 1.0);
        let taut = polyline_len(&build_route(circles, 0.0).pts);

        // Slack from a hair to a quarter metre — the whole range the drawn belt's spring can ask
        // for on this rig, and the range over which the ride bracket moves across the hemisphere.
        for step in 1..=12 {
            let excess = rig.belt_len() * 0.02 * step as f32 / 12.0;
            let mut every = Vec::new();
            sag_span(
                rear_up,
                front_up,
                excess,
                circles,
                SagClip::EveryCircle,
                &mut every,
            );
            let mut roads = Vec::new();
            sag_span(
                rear_up,
                front_up,
                excess,
                circles,
                SagClip::RoadWheels,
                &mut roads,
            );
            let worst = |pts: &[Vec2]| {
                circles
                    .iter()
                    .map(|&c| deepest(pts, c))
                    .fold(0.0_f32, f32::max)
            };
            // Print the construction's widest x-bracket so its working scale is MEASURED.
            let h = sag_depth(rear_up.distance(front_up), excess);
            let widest = circles
                .iter()
                .filter_map(|&(c, r)| ride_bracket(rear_up, front_up, h, c, r))
                .map(|(t0, t1)| (t1 - t0) * (front_up.x - rear_up.x).abs())
                .fold(0.0_f32, f32::max);
            println!(
                "  slack {:.3} m over taut {taut:.3} m: EveryCircle {} pts cuts {:.3} mm, \
                 RoadWheels {} pts cuts {:.1} mm (widest ride bracket {:.1} mm of abscissa)",
                excess,
                every.len(),
                worst(&every) * 1000.0,
                roads.len(),
                worst(&roads) * 1000.0,
                widest * 1000.0,
            );
            for &(c, r) in circles {
                let cut = deepest(&every, (c, r));
                assert!(
                    cut <= SAG_CLIP_INSET,
                    "at {excess:.3} m of slack a drape CHORD cut the circle at {c:?} (r {r:.3}) \
                     {:.3} mm deep, past the {:.3} mm ride budget",
                    cut * 1000.0,
                    SAG_CLIP_INSET * 1000.0,
                );
            }
        }
    }

    /// Equal-circle deep drape crosses both equators, clipping ride brackets at silhouette
    /// corners `(c.x ± r, c.y)`. Strict [`hemisphere_y`] gives `None` at `|dx| ≥ r`: projecting
    /// that corner onto the parabola gives MEASURED −0.25 vs corner 0.0 and a 2.97 mm segment cut
    /// against the 1 mm contract. Circle-top endpoints exempt only their owners; DERIVED
    /// `sag_depth(2, 4/3) = 1` (twice the radius) ensures equator crossing. No road wheels means
    /// promotion cannot mask this ride-only case.
    #[test]
    fn a_bracket_end_on_a_circles_lateral_boundary_still_rides_out() {
        let circles = [(Vec2::new(-2.0, 0.0), 0.5), (Vec2::new(0.0, 0.0), 0.5)];
        let (from, to) = (Vec2::new(0.0, 0.5), Vec2::new(-2.0, 0.5));
        assert_eq!(sag_depth(2.0, 4.0 / 3.0), 1.0, "fixture: the drape is deep");
        let mut pts = Vec::new();
        sag_span(
            from,
            to,
            4.0 / 3.0,
            &circles,
            SagClip::EveryCircle,
            &mut pts,
        );
        for &circle in &circles {
            let cut = deepest_inside(&pts, circle);
            assert!(
                cut <= SAG_CLIP_INSET + 1e-6,
                "a segment cuts {:.3} mm into the circle at {:?} — the boundary corner was \
                 dropped to the parabola again (was 2.97 mm before `silhouette_y`)",
                cut * 1e3,
                circle.0,
            );
        }
    }

    /// Corner admission must match the dedup's one-float merge radius. Dedup keeps the earlier
    /// adjacent abscissa, at most one float from a merged end; farther samples need strict projection.
    #[test]
    fn the_bracket_admits_exactly_the_dedups_merge_radius() {
        let bracket = Some((0.25f32, 0.5f32));
        assert!(bracket_admits(bracket, 0.25));
        assert!(
            bracket_admits(bracket, 0.25f32.next_down()),
            "the dedup survivor"
        );
        assert!(
            !bracket_admits(bracket, 0.25f32.next_down().next_down()),
            "two floats out is beyond anything the dedup can produce"
        );
        assert!(bracket_admits(bracket, 0.5));
        assert!(bracket_admits(bracket, 0.5f32.next_up()));
        assert!(!bracket_admits(bracket, 0.5f32.next_up().next_up()));
        assert!(bracket_admits(bracket, 0.3), "interior points are members");
        assert!(!bracket_admits(None, 0.3), "no bracket admits nothing");
    }

    /// A spatial tolerance band falsely lifts an outside sample by DERIVED 0.25 m to the corner;
    /// exact bracket membership must leave it on the parabola. DERIVED span length 2.00002 m puts
    /// uniform `t = 0.25` ~5 µm outside the right boundary while the bracket reconstructs its corner.
    #[test]
    fn a_sample_just_outside_a_circle_is_not_lifted_to_its_corner() {
        let circles = [(Vec2::new(-2.0, 0.0), 0.5), (Vec2::new(0.0, 0.0), 0.5)];
        let (from, to) = (Vec2::new(0.0, 0.5), Vec2::new(-2.00002, 0.5));
        let excess = 4.0 / 3.0;
        let mut pts = Vec::new();
        sag_span(from, to, excess, &circles, SagClip::EveryCircle, &mut pts);
        let h = sag_depth(from.distance(to), excess);
        for q in &pts {
            // Exclude the legitimate corner lift at `|x − c.x| = r` with a 1 µm margin;
            // the false-lift fixture is DERIVED ~5 µm outside, well beyond it.
            let outside_all = circles.iter().all(|&(c, r)| (q.x - c.x).abs() > r + 1e-6);
            if !outside_all {
                continue;
            }
            let t = (q.x - from.x) / (to.x - from.x);
            let parabola = sag_y(from, to, h, t);
            assert!(
                (q.y - parabola).abs() <= 1e-6,
                "a sample at x = {} (outside every circle's abscissa range) sits {} above its \
                 parabola — the corner lift leaked past the bracket again",
                q.x,
                q.y - parabola,
            );
        }
        for &circle in &circles {
            let cut = deepest_inside(&pts, circle);
            assert!(
                cut <= SAG_CLIP_INSET + 1e-6,
                "cut {cut} exceeds the ride budget"
            );
        }
    }

    /// Ride cuts must stay bounded continuously as a circle withdraws: `ride_bracket` disappears
    /// at [`SAG_CLIP_INSET`], so the ridden→ignored handover must be seamless. The shipped-span
    /// test measures the projection-only counterfactual but has comfortably wide penetrations.
    /// This fixture sweeps a small circle in DERIVED 0.2 mm steps on a tilted span, where slope
    /// shifts peak penetration off the circle top and stresses the vertical bound. It crosses
    /// the promotion threshold: below the MEASURED 1 mm own-abscissa threshold, only riding can
    /// keep the chord out.
    #[test]
    fn a_grazing_circle_is_ridden_however_narrow_its_penetration_is() {
        // A tilted span between two end circles the drape leaves at their tops.
        let (from, to) = (Vec2::new(-1.2, 1.1), Vec2::new(1.2, 0.55));
        let (end_a, end_b) = ((Vec2::new(-1.2, 0.8), 0.3), (Vec2::new(1.2, 0.25), 0.3));
        let (probe_x, probe_r) = (0.35, 0.18);
        let excess = 0.05;

        // Where the drape's own parabola runs over the probe circle's abscissa — the height the
        // circle's TOP is swept through.
        let t = (probe_x - from.x) / (to.x - from.x);
        let h = sag_depth(from.distance(to), excess);
        let sag_y = sag_y(from, to, h, t);

        let mut worst = (0.0_f32, 0.0_f32);
        for step in -100..=100 {
            let poke = step as f32 * 2e-4; // ±20 mm about grazing, in 0.2 mm steps
            let circles = vec![
                end_a,
                (Vec2::new(probe_x, sag_y - probe_r + poke), probe_r),
                end_b,
            ];
            let mut pts = Vec::new();
            sag_span(from, to, excess, &circles, SagClip::EveryCircle, &mut pts);
            for &circle in &circles {
                let cut = deepest(&pts, circle);
                assert!(
                    cut <= SAG_CLIP_INSET,
                    "a drape CHORD cut the circle at {:?} (r {:.3}) {:.3} mm deep with the probe \
                     circle poking {:.2} mm through the parabola — past the {:.3} mm ride budget",
                    circle.0,
                    circle.1,
                    cut * 1000.0,
                    poke * 1000.0,
                    SAG_CLIP_INSET * 1000.0,
                );
                if cut > worst.0 {
                    worst = (cut, poke);
                }
            }
        }
        println!(
            "  grazing sweep ±20 mm in 0.2 mm steps: worst chord cut {:.3} mm (at {:+.2} mm poke), \
             budget {:.3} mm",
            worst.0 * 1000.0,
            worst.1 * 1000.0,
            SAG_CLIP_INSET * 1000.0,
        );
    }

    /// Off-domain penetration can be deepest at a span endpoint: no crossing exists to bisect.
    /// Only its owning circle may exempt it; a blanket exemption leaves it inside a non-owner.
    /// Synthetic fixture (the shipped wrap sweep does not reach it), DERIVED from the span start:
    /// equal, level end circles give exact circle-top endpoints. A rogue centre offset behind
    /// the start (`c.x <= lo`) avoids promotion, while its x-range still covers the start and
    /// its lower centre puts the endpoint inside by `r − |off|`: MEASURED 47.2 mm, 47× budget.
    /// The same endpoint sits on its idler owner; the far endpoint sits on its sprocket owner,
    /// whose penetration runs off the opposite domain end. Both exemptions must survive.
    /// Compute the blanket-exemption counterfactual by restoring the first point: clip decisions
    /// do not affect the abscissa set, so this reconstructs that drawing exactly.
    #[test]
    fn a_span_end_inside_a_circle_that_does_not_own_it_is_still_lifted_out() {
        let r_end = 0.10_f32;
        let sprocket = (Vec2::new(2.0, 0.0), r_end);
        let idler = (Vec2::new(0.0, 0.0), r_end);
        // The same call `wrap` makes: the upper external tangent, idler end first.
        let (from, to) = external_tangent(idler.0, idler.1, sprocket.0, sprocket.1, 1.0);
        let off = Vec2::new(-0.02, -0.07);
        let rogue = (from + off, 0.12);
        let circles = vec![sprocket, rogue, idler];
        let excess = 0.02;

        // Assert fixture preconditions so a geometry edit cannot make the test vacuous.
        let inside = rogue.1 - from.distance(rogue.0);
        assert!(
            (from.distance(idler.0) - idler.1).abs() <= SPAN_END_ON_CIRCLE,
            "the span's start is not ON the idler — there is no owner for the rule to exempt"
        );
        assert!(
            inside > 10.0 * SAG_CLIP_INSET,
            "the span's start is only {:.3} mm inside the rogue wheel — the fixture no longer puts \
             an endpoint deep inside a circle that does not own it",
            inside * 1000.0,
        );
        assert!(
            rogue.0.x <= from.x.min(to.x),
            "the rogue wheel's centre is ON the span, so the support search would PROMOTE it and \
             this would stop being a clip case at all"
        );
        assert!(
            rogue.0.x + rogue.1 > from.x.min(to.x),
            "the rogue wheel no longer reaches the span, so its penetration is not off-domain"
        );

        let mut pts = Vec::new();
        sag_span(from, to, excess, &circles, SagClip::EveryCircle, &mut pts);

        let mut blanket = pts.clone();
        blanket[0] = from;
        println!(
            "  span end {:.2} mm inside a non-owner circle: {} pts, rogue cut {:.3} mm ridden vs \
             {:.2} mm with a blanket end-exemption, budget {:.3} mm",
            inside * 1000.0,
            pts.len(),
            deepest(&pts, rogue) * 1000.0,
            deepest(&blanket, rogue) * 1000.0,
            SAG_CLIP_INSET * 1000.0,
        );
        assert!(
            deepest(&blanket, rogue) > 10.0 * SAG_CLIP_INSET,
            "a blanket end-exemption clears this circle anyway — the case proves nothing about \
             ownership"
        );

        for (what, circle) in [
            ("the rogue wheel", rogue),
            ("the idler", idler),
            ("the sprocket", sprocket),
        ] {
            let cut = deepest(&pts, circle);
            assert!(
                cut <= SAG_CLIP_INSET,
                "the drape CHORD cut {what} {:.3} mm deep, past the {:.3} mm ride budget — a span \
                 end inside a circle that does not own it was handed through unclipped",
                cut * 1000.0,
                SAG_CLIP_INSET * 1000.0,
            );
        }

        // The lifted attachment must pass the same `on_circle` ownership test.
        let lifted = pts[0];
        assert!(
            lifted.y > from.y,
            "the span's start was not lifted at all: {lifted:?} vs {from:?}"
        );
        assert!(
            (lifted.distance(rogue.0) - rogue.1).abs() <= SPAN_END_ON_CIRCLE,
            "the lifted start is not on the rogue wheel's hemisphere: {lifted:?}"
        );
        // The owning far circle must still preserve its exact tangent point.
        let far = *pts.last().expect("the drape is never empty");
        assert!(
            far.distance(to) <= SPAN_END_ON_CIRCLE,
            "the far span end moved {:.4} mm — the circle that OWNS it must still hand it through",
            far.distance(to) * 1000.0,
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
