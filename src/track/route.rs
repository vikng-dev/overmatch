//! The route core (architecture §1/§2): the tagged taut guide route of one track side, as pure
//! geometry over the side-plane circles of its running gear. The SIM's reference for "where the
//! track is" — the belt physics samples along it (phase B), and the kinematic-wrap view fits the
//! drawn belt around the SAME builders ([`taut_lower_run`], [`sag_span`], [`arc`], [`resample`]).
//!
//! The route and the drawn belt are NOT the same polyline, and that is deliberate — see [`SagClip`].
//! Everything that touches GROUND (the lower run: the convex envelope, its tangents and wrap arcs)
//! is shared bit for bit. The RETURN run is not: its drape is pushed out of the road wheels only for
//! the route and out of EVERY circle for the drawn belt, because the clip costs length the route
//! cannot spend (phantom strain in the element law) and the view can (`wrap::station_params` reads
//! the drawn length as a uniform strain). "Single source of truth" is therefore true of the shared
//! BUILDERS and of the ground-contact geometry, and knowingly false of the return run's shape.
//!
//! Conventions (inherited from the sandbox, steps 17–25): everything is in the hull-local SIDE
//! PLANE as (z, y) `Vec2`s; the loop runs CCW in that plane (lower run front→rear, return run
//! rear→front); a segment's OUTWARD normal is `(tan.y, −tan.x)`. Circles are on the PIN line
//! (wheel radius + half track thickness) and are given front→rear.

use bevy::math::Vec2;

use super::derive::wrap_joint_angle;

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

    // Return run: the leftover belt length as budgeted sag over the road wheels. [`SagClip`] is
    // `RoadWheels` because this route is the SIM's length reference — see the variant's own note.
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

/// Which circles a [`sag_span`] drape is pushed back out of, and how faithfully. Not a taste knob:
/// clipping COSTS LENGTH the drape was not budgeted, and the two consumers of a drape can afford
/// wildly different amounts of that.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SagClip {
    /// Road wheels only, by vertical projection of the drape's own uniform samples — the drape may
    /// still sink into the two circles its own endpoints sit on.
    ///
    /// The SIM route's setting ([`build_route`]). Its return run carries no ground contact, so the
    /// dip is invisible there; what is very visible is length. The route polyline IS the material
    /// reference the force stations are laid along, and the wheel clip's grazes are mm-scale by
    /// construction. Clipping the ends adds MEASURED ~11 mm on the Tiger's MEASURED 12.65 m loop —
    /// DERIVED 0.09 % of phantom strain fed straight into the element law, MEASURED as enough to
    /// turn a 0.1 mm latched 20° hill hold into a >50 mm slide (`headless_test`'s
    /// `hill_hold_20_deg_engages_and_pulls_away_tiger` / `slope_park_holds_20_deg_tiger`). Slack the
    /// route did not budget is not free here — and neither is the extra vertex the faithful ride
    /// below would insert, which is why that ride is `EveryCircle`-only rather than universal.
    RoadWheels,
    /// Every circle, ends included, and RIDDEN rather than projected — the DRAWN belt's setting
    /// (`super::wrap`).
    ///
    /// The drape leaves each tangent point heading DOWN, so any slack at all sinks the first
    /// samples straight into the very circle that endpoint sits on, and promotion can never catch
    /// it because that circle is not a support candidate. Unclipped, the shipped Tiger's shoes sank
    /// MEASURED 33 mm into the idler and MEASURED 48 mm into the sprocket across the travel band —
    /// plainly visible now that the links carry a real texture. Riding the end circle's upper
    /// hemisphere back out is what the real track does as it leaves a wheel under slack.
    ///
    /// "Ridden" is the second half of the setting and it is what makes the result a POLYLINE OF THE
    /// HEMISPHERE rather than a shadow of it. Vertical projection of a fixed sample set guarantees
    /// only that the VERTICES clear the circle: the chords between them still cut it, and the
    /// segment that straddles the entry point leaves the arc at an arbitrary angle (a visible
    /// kink). So this variant additionally inserts the CLOSED-FORM ride bracket ([`ride_bracket`])
    /// and walks the arc between its ends at a chord no longer than [`sag_clip_chord`]. See
    /// [`sag_span`] for the bound that buys.
    ///
    /// It is also the ONLY setting whose samples may sit on the two circles the span's own endpoints
    /// belong to, which is why it is also the only one that hands those endpoints through unclipped
    /// (see [`sag_span`]); `RoadWheels` clips every sample it emits, endpoints included.
    ///
    /// The view can afford the length all of that costs and the sim cannot: `wrap::station_params`
    /// re-reads the drawn polyline's length as a UNIFORM STRAIN and samples the pins in material
    /// arc length, so extra drawn millimetres are absorbed by construction and the sprocket phase
    /// lock never drifts.
    EveryCircle,
}

/// Chord-inset budget (m) for an [`SagClip::EveryCircle`] ride: how far the drape's straight
/// sample-to-sample chord is allowed to sit inside the hemisphere it is following.
///
/// DERIVED, not a dial — it is the SAME 1 mm the promotion search in [`sag_span`] already treats as
/// "not a penetration" (`pen > 1e-3`). Tying the two together means the clip's own discretisation
/// can never leave behind a cut that the support search would have called a penetration.
pub(crate) const SAG_CLIP_INSET: f32 = 1e-3;

/// How far a chord of length `c` across a circle of radius `r` sits INSIDE the arc it spans:
/// `r − √(r² − c²/4)`. The belt is drawn as straight segments and a straight segment across a curve
/// is inside it by exactly this much, so every clearance budget in the track view is built out of
/// this one function — and [`sag_clip_chord`] is its inverse, the other direction of the same
/// formula.
pub fn chord_inset(r: f32, c: f32) -> f32 {
    r - (r * r - 0.25 * c * c).max(0.0).sqrt()
}

/// The longest chord an [`SagClip::EveryCircle`] ride may lay across a circle of radius `r` while
/// following its upper hemisphere: the chord whose [`chord_inset`] is exactly [`SAG_CLIP_INSET`],
/// DERIVED by inverting `inset = r − √(r² − c²/4)` into `c = 2·√(2·r·d − d²)`.
///
/// Public because it is a CONTRACT, not an implementation detail: the drawn-belt clearance gate
/// (`wrap`'s sweep) budgets the cut the clip introduces by taking this chord's inset, so the bound
/// asserted there and the spacing produced here are the same number by construction.
pub fn sag_clip_chord(r: f32) -> f32 {
    2.0 * (2.0 * r * SAG_CLIP_INSET - SAG_CLIP_INSET * SAG_CLIP_INSET)
        .max(0.0)
        .sqrt()
}

/// Sag depth (m) of a drape: the parabola's own `h = √(3·chord·excess/8)`, i.e. how far a span of
/// `chord` metres carrying `excess` metres of slack hangs below its chord at mid-span. THE one
/// statement of the drape's shape parameter — [`sag_span`] draws it, `wrap::SlackSpring` derives
/// its pendulum rate from it, and every test that predicts a drape reads it here.
pub fn sag_depth(chord: f32, excess: f32) -> f32 {
    (3.0 * chord * excess / 8.0).sqrt()
}

/// The slack a closed belt has left for its return run: `belt_len − path_len − chord`, floored at
/// zero. The floor is the explicit length-budget clamp — a path longer than the belt runs the
/// return TAUT rather than laundering the deficit into the drape's shape (the infeasibility rule).
pub fn slack(belt_len: f32, path_len: f32, chord: f32) -> f32 {
    (belt_len - path_len - chord).max(0.0)
}

/// The drape parabola's ordinate at chord parameter `t`: the chord's own lerp pulled down by
/// `4h·t(1−t)`.
fn sag_y(from: Vec2, to: Vec2, h: f32, t: f32) -> f32 {
    from.lerp(to, t).y - 4.0 * h * t * (1.0 - t)
}

/// The upper hemisphere's ordinate at abscissa `x`, or `None` where the circle has no upper
/// hemisphere to speak of (`|x − c.x| ≥ r`). `None` rather than a clamped `c.y`, deliberately: a
/// sample OUTSIDE a circle's abscissa range must never be raised by it.
fn hemisphere_y(c: Vec2, r: f32, x: f32) -> Option<f32> {
    let dx = x - c.x;
    (dx.abs() < r).then(|| c.y + (r * r - dx * dx).sqrt())
}

/// [`hemisphere_y`] with the lateral boundary INCLUDED: the ordinate at `|x − c.x| = r` is `c.y` —
/// the silhouette corner — and a [`ride_bracket`] end clipped to the circle's abscissa range lands
/// exactly there, except that the `t ↔ x` round trip can put it a float or two OUTSIDE, where the
/// strict hemisphere answers `None` and the sample falls to the parabola BELOW the corner
/// (MEASURED 2026-07-27, Codex counterexample: a 2.97 mm cut on a constructed equal-radius span,
/// against the 1 mm contract — pinned by
/// `a_bracket_end_on_a_circles_lateral_boundary_still_rides_out`). The clamped `sqrt` absorbs that
/// rounding; a sample whose parabola already sits above `c.y` is left where it is by the caller's
/// `max`.
///
/// Called ONLY for a sample inside the circle's own ride bracket — exact float membership, decided
/// where the bracket was computed — which is what makes the corner inclusion unable to falsely
/// lift anything: a first cut used a spatial tolerance band here instead, and a legitimate sample
/// 5 µm outside a circle was raised 0.25 m to its corner (Codex counterexample, 2026-07-27, pinned
/// by `a_sample_just_outside_a_circle_is_not_lifted_to_its_corner`). The sim's
/// [`SagClip::RoadWheels`] has no brackets at all and keeps the strict [`hemisphere_y`], so its
/// bits cannot move through a function it never reaches.
fn silhouette_y(c: Vec2, r: f32, x: f32) -> f32 {
    let dx = x - c.x;
    c.y + (r * r - dx * dx).max(0.0).sqrt()
}

/// Does this circle's ride bracket admit abscissa `t`? Membership in `[t0, t1]` widened by ONE
/// float on each side — exactly the merge radius of [`sag_samples`]' dedup, which keeps the
/// EARLIER of two bit-equal-or-adjacent abscissas and can therefore hand the projection a bracket
/// end's immediate float neighbour instead of the end itself; an exact test there would drop the
/// silhouette corner on that coincidence (Codex finding, 2026-07-27). One float wider and no more:
/// a float is the corner to `f32` (~1e-7 at hull scale), while the false-lift case this must keep
/// excluding — a legitimate parabola sample near a circle — sat ~5 µm out, dozens of floats away
/// (pinned by `a_sample_just_outside_a_circle_is_not_lifted_to_its_corner`).
fn bracket_admits(bracket: Option<(f32, f32)>, t: f32) -> bool {
    bracket.is_some_and(|(t0, t1)| (t0.next_down()..=t1.next_up()).contains(&t))
}

/// Is `p` ON this circle, to within [`SPAN_END_ON_CIRCLE`]? The named form of the "sits on" test —
/// the drape's endpoint-ownership rule ([`sag_span`]) and the view's wrap-arc chord measurement
/// both ask exactly this question and must ask it at the same tolerance.
pub fn on_circle(p: Vec2, (c, r): (Vec2, f32)) -> bool {
    (p.distance(c) - r).abs() <= SPAN_END_ON_CIRCLE
}

/// Samples per drape span (the parabola's own uniform discretisation, before any
/// [`SagClip::EveryCircle`] refinement): `SAG_SEGMENTS + 1` abscissas at `t = i / SAG_SEGMENTS`.
const SAG_SEGMENTS: usize = 16;

/// How close a drape's END sample must sit to a circle for that circle to OWN it — i.e. for
/// [`sag_span`] to hand the sample through verbatim instead of projecting it up like any other
/// sample. See the ownership rule in [`sag_span`]'s doc.
///
/// DERIVED as a two-sided margin, not tuned. It must sit well ABOVE the float noise in the points
/// that are legitimately ON a circle — an [`external_tangent`] point or a promoted wheel top, both
/// good to ~1e-7 m at running-gear radii — and well BELOW [`SAG_CLIP_INSET`], because the most a
/// mis-ownership can cost is exactly this much radial cut left behind at one endpoint. 10 µm is two
/// orders of magnitude clear of each.
const SPAN_END_ON_CIRCLE: f32 = 1e-5;

/// The RIDE BRACKET: the chord-parameter interval an [`SagClip::EveryCircle`] ride must cover on
/// one circle, in closed form. Returns `None` when the circle is provably clear of the drape.
///
/// The whole detection question is one quadratic, and it is worth stating why. Write the drape's
/// ordinate as `P(t)` and the vertical gap the clip works in as
///
/// ```text
///   pen(t) = hemisphere(x(t)) − P(t)
/// ```
///
/// A point on the parabola at vertical gap `pen` is inside the circle by AT MOST `pen`
/// (`dist² = r² − 2·pen·√(r²−u²) + pen² ≥ (r − pen)²`), so a bound on the vertical measure is a
/// bound on the cut — which is what lets a scalar inequality answer a distance question.
///
/// The bracket is derived from the OSCULATING PARABOLA of the circle top, `r − dx²/(2r)`, which
/// over-estimates the hemisphere everywhere (`√(1−s) ≤ 1 − s/2`). So
///
/// ```text
///   pen(t) > d   ⟹   P(t) < c.y + √(r² − dx²) − d   ⟹   P(t) + dx(t)²/(2r) < c.y + r − d
/// ```
///
/// and the right-hand condition is a QUADRATIC in `t` (`P` is quadratic, `dx` is affine) with a
/// strictly positive leading coefficient, so its solution set is a single interval computed by the
/// quadratic formula. Intersected with the circle's own abscissa range and the span, that interval
/// is the bracket. It is a SUPERSET of `{pen > d}` — never a sample of it — so there is no
/// resolution to slip through and nothing to bisect. (The same trade [`super::oracle`] made when
/// its 8-segment sign-change scan + 24-step bisection became a closed-form first hit: a scan can
/// only ever state a detection bound, and a closed form has no checkpoints to straddle.)
///
/// `d` is [`SAG_CLIP_INSET`], and taking the threshold there rather than at zero is the honest half
/// of the contract, stated exactly instead of approximately: the ride does not claim to find every
/// penetration, it claims to leave none deeper than the 1 mm the support search already treats as
/// no penetration at all. Outside the bracket `pen ≤ d`, and every sample is `≥ P` while the chord
/// of a CONVEX parabola is `≥ P` too — so the drawn segment there sits at most `d` below the
/// hemisphere, i.e. at most `d` inside the circle. See [`sag_span`] for how the two halves compose.
///
/// Taking it at zero instead would also be unusable: the taut return chord is TANGENT to the end
/// circles, and a tangent line of slope `m` dips below the osculating parabola over an interval of
/// width `2r(√(1+m²) − 1)`. The `d` term swamps that (`(√(1+m²)−1)² ≪ 2d/r` at any running-gear
/// slope), which is what keeps a zero-slack drape refinement-free and bit-identical under either
/// clip setting.
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

/// Drape one return-run span with `excess` metres of slack as a parabola — and if the curve
/// dips into a road wheel, PROMOTE that wheel to a support: split the span at the wheel's top
/// and drape each side with its share of the remaining slack (the loose return run riding its
/// wheels, hanging in short spans between them — computed, not solved). Points arrive from
/// above by construction, so which side of a wheel the belt is on is given, never discovered.
///
/// `circles` is the side's FULL front→rear list, and the two roles it plays here are DIFFERENT
/// SETS. The SUPPORT SEARCH is always the road wheels alone (`circles[1..last]`): the span's own
/// endpoints are the upper tangent points *on* the sprocket and the idler, so offering those two
/// as supports would split the span at a point it already begins (or ends) at — a zero-length
/// sub-span and a divided-by-nothing slack share, not a drape. The CLIP set is the caller's
/// choice, and it is a real one: see [`SagClip`].
///
/// # What the emitted polyline guarantees about the circles it rides
///
/// Under [`SagClip::EveryCircle`] the drape is not merely projected out of the circles at fixed
/// abscissas — it is sampled at abscissas chosen so the POLYLINE clears them, which is the only
/// thing a renderer ever sees. The guarantee is that NO drawn segment cuts a circle deeper than
/// [`SAG_CLIP_INSET`], and it comes from splitting the span at the closed-form [`ride_bracket`]:
///
/// * INSIDE the bracket the samples walk the hemisphere at no more than [`sag_clip_chord`] per
///   step, so their chord is inside the arc by at most [`SAG_CLIP_INSET`]. Every sample is emitted
///   at `max(parabola, hemispheres)`, so a sample the parabola (or some OTHER circle) holds higher
///   only raises the segment off this circle, never lowers it;
/// * OUTSIDE it `pen ≤ SAG_CLIP_INSET` by the bracket's own construction — that is what the
///   bracket solves for. Every sample there sits at or above the parabola, and a chord of a CONVEX
///   parabola lies above the parabola too, so the whole drawn segment is within `SAG_CLIP_INSET`
///   of the hemisphere vertically, hence that far inside the circle at most.
///
/// The two halves meet AT a sample — the bracket's ends are pushed into the abscissa set — so no
/// segment straddles the switch from circle-following to parabola and neither argument has a gap to
/// cover for the other. Extra abscissas from other circles can only SUBDIVIDE a step, never lengthen
/// one, which is why the per-circle refinement survives being merged and why OVERLAPPING circles are
/// a non-case.
///
/// The one ride end that is not the bracket's own is the one a penetration running off the DOMAIN
/// leaves: the bracket is clamped into the span, so the ride starts (or finishes) at the span's own
/// limit, which may be penetrating. That end is covered by the ownership rule below — it is a span
/// ENDPOINT, and every circle but the one that OWNS it projects the endpoint onto its hemisphere, so
/// the ride still begins ON the circle rather than inside it.
///
/// # Who may hand a span endpoint through unclipped
///
/// Under [`SagClip::EveryCircle`] a span endpoint is handed through verbatim — never re-derived
/// through the clip's `sqrt` — by the circle it SITS ON, and by that circle ALONE
/// ([`SPAN_END_ON_CIRCLE`] is the "sits on" test, and it is a position test rather than an index
/// one so a promoted sub-span's ends, which sit on road wheels, are covered by the same rule). The
/// endpoint is the taut walk's tangent point or a promoted wheel top, a drape must begin exactly
/// where the run before it ended, and re-deriving `c.y + √(r² − dz²)` there could round a ULP off a
/// point the caller handed in exact.
///
/// Every OTHER circle clips that endpoint like any interior sample, and that is load-bearing rather
/// than tidy: a penetration whose interval runs off the END of the span is clamped there, so
/// [`sag_samples`] starts (or ends) the ride at the domain limit and the DEEPEST point of that
/// penetration is the endpoint itself. Exempting it from every circle at once would leave
/// the ride beginning inside one. Raising an endpoint does move where the drape attaches to the run
/// before it — but only ever in the case where the old attachment was inside a circle it does not
/// belong to, i.e. where the drawn belt was already cutting through geometry.
///
/// [`SagClip::RoadWheels`] clips every sample it emits including the two endpoints, which is safe
/// for the opposite reason: its clip set is the road wheels alone, so the only endpoint-bearing
/// circle it can contain is a wheel a recursion PROMOTED, and the endpoint there is that wheel's own
/// top — where the projection returns it.
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

/// [`sag_span`] plus the promotion RECURSION DEPTH — the one argument no caller ever has an opinion
/// about (every entry point passes `0`), kept off the public signature.
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
            if pen > 1e-3 && worst.is_none_or(|(_, w)| pen > w) {
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
    // Each circle's ride bracket, computed ONCE and shared by the abscissa refinement below and the
    // projection loop's silhouette dispatch — exact float membership in `[t0, t1]` is what scopes
    // the corner-inclusive [`silhouette_y`] to the samples the bracket itself produced (a spatial
    // tolerance band was tried first and falsely lifted a legitimate sample 5 µm outside a circle —
    // Codex counterexample, 2026-07-27, pinned by
    // `a_sample_just_outside_a_circle_is_not_lifted_to_its_corner`). `RoadWheels` gets no brackets
    // at all: the sim path refines nothing and projects through the strict [`hemisphere_y`] only.
    // A circle no bigger than the inset cannot be cut deeper than the inset and is the one input
    // the bracket's `1/(2r)` would blow up on, so it gets `None` here.
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
        // The span END as handed in — read BEFORE any circle raises `q`, so the ownership test
        // below asks about the point the caller gave, not about a point some other circle moved.
        // `RoadWheels` never has an owner (see the fn doc) and so never sets this.
        let end = (clip == SagClip::EveryCircle && (i == 0 || i == last)).then_some(q);
        // Safety clip (mm-scale grazes near tangency that promotion's point-split leaves) — and,
        // under `EveryCircle`, the whole dip out of a tangent point promotion cannot see.
        for (j, &(c, r)) in clipped.iter().enumerate() {
            // OWNERSHIP: a span end is exempt from the circle it SITS ON, and from that circle
            // only. Never from the whole clip set — a penetration whose interval runs off the end
            // of the span has its deepest point AT the end, and a blanket exemption would leave
            // that end inside a circle no ride ever lifted it out of, with the drawn chord to the
            // next sample cutting straight through. Ownership is tested by POSITION, not by index:
            // a promoted sub-span's ends are road-wheel tops, owned by circles in the middle of
            // the list, and the top-level span's ends by the two the taut walk left at a tangent.
            if end.is_some_and(|p| on_circle(p, (c, r))) {
                continue;
            }
            // A sample INSIDE this circle's own ride bracket must see the silhouette CORNER — a
            // bracket end clipped to the circle's abscissa range sits exactly on it, possibly a
            // float outside after the `t ↔ x` round trip ([`silhouette_y`]). Membership is decided
            // against the same bracket the abscissa came from ([`bracket_admits`] — one float of
            // slack, the dedup's own merge radius), so no sample the bracket did not produce can
            // be falsely lifted; everything else — including the whole sim path, whose `brackets`
            // is empty — projects through the strict [`hemisphere_y`].
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

/// The chord parameters `t ∈ [0, 1]` one [`sag_span`] drape is sampled at.
///
/// Always the [`SAG_SEGMENTS`] uniform steps — that set is the drape's own discretisation and is
/// what [`SagClip::RoadWheels`] emits verbatim, so the sim route's vertices are untouched by any of
/// this. Under [`SagClip::EveryCircle`] it is then REFINED per circle: the two ends of that
/// circle's [`ride_bracket`], plus a walk of the hemisphere between them at no more than
/// [`sag_clip_chord`] per step. A circle whose bracket is empty is provably clear to within the ride
/// budget and contributes nothing.
///
/// The walk is stepped in ANGLE rather than in `x`, because equal steps in `x` bunch into an
/// unbounded chord as the hemisphere goes vertical near `c.x ± r` — and the angular step is
/// [`wrap_joint_angle`] of the ride chord, the same chord relation the sprocket's own meshing uses.
/// That also bounds the work per circle outright (at most `π / step` samples), so there is no cap to
/// choose.
///
/// The refinement is per-circle but the guarantee survives merging, because extra abscissas can only
/// SUBDIVIDE another circle's steps — never lengthen them. A bracket end that lands ON a span
/// endpoint or on a uniform sample is a duplicate abscissa, which the dedup below drops; the
/// endpoints are re-pinned to `0` / `1` afterwards because the caller hands those two through
/// unclipped.
fn sag_samples(
    from: Vec2,
    to: Vec2,
    clipped: &[(Vec2, f32)],
    brackets: &[Option<(f32, f32)>],
) -> Vec<f32> {
    let uniform = || (0..=SAG_SEGMENTS).map(|i| i as f32 / SAG_SEGMENTS as f32);
    let dx = to.x - from.x;
    // No brackets is the sim path ([`SagClip::RoadWheels`] computes none); an x-degenerate span has
    // no single-valued hemisphere to ride (and no drape worth the name) — either way the vertical
    // projection in the caller still applies to the uniform samples.
    if brackets.is_empty() || dx.abs() < 1e-4 {
        return uniform().collect();
    }
    let mut ts: Vec<f32> = uniform().collect();
    for (&(c, r), bracket) in clipped.iter().zip(brackets) {
        // `None` is a circle that is provably clear to within the ride budget — or one no bigger
        // than the inset, which cannot be cut deeper than the inset by anything (see the bracket
        // construction in [`sag_span_at`]).
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
    // Collapse abscissas that are the SAME float or the next one up (a bracket end that landed on a
    // uniform sample, two circles entering at the same place): those, and only those, are a
    // segment with no interior to carry geometry. Nothing coarser may be merged — two bracket ends
    // a few microns apart are distinct points of the SHAPE, and dropping one would drop the ride
    // between them. The sub-micron segments this leaves behind cost nothing: `resample` skips
    // degenerate segments outright, and `polyline_len` sums them exactly.
    ts.dedup_by(|a, b| *a <= b.next_up());
    // The two ENDS are load-bearing: the caller hands `t = 0` / `t = 1` through unclipped, so they
    // must be exactly the span's endpoints even if a release abscissa one float away from one of
    // them won the dedup.
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

/// Chords [`arc`] splits every wrap into, whatever the sweep angle. A FIXED count, so the chord —
/// and therefore how far inside the true circle the drawn polyline sits — is a function of the wrap
/// ANGLE: a road wheel's few degrees cost microns, an end circle's ~165° costs the size of a link.
/// Named because the clearance budgets downstream ([`chord_inset`] of this chord, in `wrap`'s sweep
/// and `rig_geom`'s enclosure guarantee) are restatements of it and must not drift from it.
pub const ARC_SEGMENTS: usize = 10;

/// Points along a circle's arc from `from` to `to` (both on the circle), taking whichever sweep
/// has its midpoint heading toward `toward` — so the belt wraps the *outer* side of the wheel
/// rather than cutting across. Endpoints included.
pub fn arc(center: Vec2, radius: f32, from: Vec2, to: Vec2, toward: Vec2) -> Vec<Vec2> {
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
    (0..=ARC_SEGMENTS)
        .map(|i| center + Vec2::from_angle(a0 + delta * (i as f32 / ARC_SEGMENTS as f32)) * radius)
        .collect()
}

/// How far a point may be pushed INWARD (along `−out`, the direction a terrain conform displaces a
/// belt station) before it enters one of `circles`. `f32::INFINITY` when the inward ray misses every
/// circle; `0.0` when the point is already on or inside one.
///
/// This is the LOWER RUN's analogue of the drape's [`ride_bracket`], and deliberately the same kind
/// of statement: the drawn belt is the support envelope of the running gear UNION the ground, so no
/// producer may put a belt station inside a wheel. The drape solves it as an interval in the chord
/// parameter because it is riding along a curve; the conform solves it as a cap on one scalar depth
/// because it is displacing along a normal. Both leave the residual to the SAME budget — the
/// polygon [`chord_inset`] the drawn segments spend between stations, of which [`SAG_CLIP_INSET`] is
/// the drape's share — so "how close may the drawn shoe come to a rim" stays one number.
///
/// Closed form: with `v = point − c` and `|out| = 1`, `|v − out·d|² = r²` is
/// `d² − 2(v·out)·d + (|v|² − r²) = 0`, and the cap is its first non-negative root.
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

/// Deepest a polyline's SEGMENTS reach inside a circle (0 = it only ever touches). The thing a
/// renderer sees, and the only quantity [`SagClip::EveryCircle`] actually promises anything about —
/// a per-VERTEX measure would pass on a polyline whose chords cut straight through.
///
/// Test-only, and shared (precedent: [`super::derive::wrap_joint_angle`]): route's ride bound and
/// `wrap`'s drawn-belt bounds are the same measurement asked of different polylines, so they must
/// not be three separately-maintained copies of it.
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
    ///
    /// These numbers held THROUGH the end-circle sag clip, and that is the point of the
    /// [`SagClip::RoadWheels`] setting above: moving the drape onto the sprocket and idler
    /// hemispheres would have left the vertex count at 172 but moved 9 of them (the return run's
    /// first five and last four) and lengthened the loop by 11 mm. The view wants exactly that and
    /// gets it; the route must not have it, and this pin is what says so.
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

    /// A drape with NO slack is the external tangent chord itself, and the two [`SagClip`] settings
    /// must produce it BIT for bit. Otherwise the `belt_len = 0` taut build — the `hull_rest_y`
    /// spawn datum's source above, and the shape the wrap's diagnostic loop degenerates to when the
    /// belly eats the whole budget — would depend on which consumer asked for it.
    ///
    /// The no-op has three parts, and this test is the proof rather than the assumption Codex
    /// flagged. Two are STRUCTURAL:
    ///
    /// * at zero slack the parabola IS the chord, so no circle's hemisphere stands above it and
    ///   [`sag_samples`] refines nothing — both settings emit the same `SAG_SEGMENTS + 1` abscissas;
    /// * OWNERSHIP: the span's two endpoints are the only samples that sit ON a circle, i.e. the
    ///   only ones where re-deriving `c.y + √(r² − dz²)` could round a ULP away from the tangent
    ///   point the walk handed in — and [`SagClip::EveryCircle`] exempts each of them from exactly
    ///   that circle ([`SPAN_END_ON_CIRCLE`]).
    ///
    /// The third is MEASURED, and deliberately so. NEITHER setting is exempt anywhere else: under
    /// `RoadWheels` all 17 samples are offered to the road wheels, and under `EveryCircle` the 15
    /// interior ones are offered to every circle and the 2 ends to every circle BUT their own. On
    /// this rig every one of those projections is a `f32::max` returning its own argument, because
    /// the taut return chord — the upper external tangent of sprocket and idler — runs clear above
    /// every road wheel, and each end tangent point is outside the circle at the far end. That is a
    /// property of a real running gear rather than a theorem about arbitrary circles, and the
    /// bit-for-bit assertion below is exactly what states it for the shipped rig.
    #[test]
    fn a_zero_slack_drape_is_bit_identical_under_either_clip() {
        use crate::track::side::Side;
        let rig = crate::track::rig_geom::tiger_rig();
        let circles = rig.rest.get(Side::Right);
        let (front_c, front_r) = circles[0];
        let (rear_c, rear_r) = *circles.last().unwrap();
        let (rear_up, front_up) = external_tangent(rear_c, rear_r, front_c, front_r, 1.0);

        let span = |clip| {
            let mut out = Vec::new();
            sag_span(rear_up, front_up, 0.0, circles, clip, &mut out);
            out
        };
        let (roads, every) = (span(SagClip::RoadWheels), span(SagClip::EveryCircle));

        assert_eq!(
            roads.len(),
            SAG_SEGMENTS + 1,
            "a zero-slack drape must be the plain uniform sampling — no promotion, no refinement"
        );
        assert_eq!(
            every.len(),
            roads.len(),
            "`EveryCircle` refined a taut chord"
        );
        for (i, (a, b)) in roads.iter().zip(&every).enumerate() {
            assert_eq!(
                (a.x.to_bits(), a.y.to_bits()),
                (b.x.to_bits(), b.y.to_bits()),
                "sample {i} moved between clip settings at zero slack: {a:?} vs {b:?}"
            );
        }
        // And the endpoints are the tangent points themselves, not a rounded re-derivation.
        assert_eq!(
            every.first().map(|p| p.to_array()),
            Some(rear_up.to_array())
        );
        assert_eq!(
            every.last().map(|p| p.to_array()),
            Some(front_up.to_array())
        );
    }

    /// The [`SagClip::EveryCircle`] ride's contract, stated on the shape it actually produces: with
    /// slack on the shipped Tiger's return run, EVERY SEGMENT of the drape clears every circle to
    /// within [`SAG_CLIP_INSET`] — not merely every vertex, which is all a vertical projection of a
    /// fixed sample set can promise. [`SagClip::RoadWheels`] is checked on the same span for the
    /// contrast that motivates the split: it leaves the drape MEASURED 6.7 mm (a hair of slack) to
    /// 63.1 mm (a quarter metre of it) deep inside the end circles — one to two orders of magnitude
    /// past the ride budget, growing with slack rather than bounded by anything, and printed here
    /// beside the ridden figures (MEASURED 2026-07-27: 0.54–0.97 mm, under the 1 mm budget at every
    /// step of the sweep).
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
            // The widest ride bracket this span asks for, read off the very circles below — the
            // construction's own working set, printed so its scale is a measurement.
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

    /// **A grazing circle is held to the budget however narrow its penetration is.** The ride bound
    /// above is asserted on the shipped rig, where every penetration is comfortably wide; this
    /// asserts the same bound while the penetration shrinks continuously to nothing.
    ///
    /// This is the CONTINUITY half of the contract, and the one place the two-sided nature of the
    /// guarantee shows: as the circle withdraws, [`ride_bracket`] empties out and the clip stops
    /// riding it — and that is fine, because it empties out exactly when the penetration it stopped
    /// riding is under [`SAG_CLIP_INSET`], which is the threshold the bracket is solved at. The
    /// assertion is on the CUT throughout, so the handover from "ridden" to "ignored" has to be
    /// seamless or this fails; the counterfactual (a projection with no ride at all) is measured on
    /// the shipped rig by the sweep above.
    ///
    /// The sweep walks a small circle through the drape in 0.2 mm steps on a TILTED span, the case
    /// the vertical measure is worst at (the parabola's slope pushes the peak off the circle's own
    /// top). The band deliberately spans the promotion threshold too: below MEASURED 1 mm at its own
    /// abscissa the wheel is NOT promoted, so the ride is the only thing keeping the chord out of it.
    /// **The lateral-boundary counterexample** (Codex, 2026-07-27): a deep drape between two
    /// equal circles dives past both equators, so each circle's [`ride_bracket`] is clipped at the
    /// circle's own abscissa range — its end IS the silhouette corner `(c.x ± r, c.y)`. The strict
    /// [`hemisphere_y`] answers `None` at `|dx| ≥ r`, so before [`silhouette_y`] the corner sample
    /// fell to the parabola (MEASURED: −0.25 vs the corner's 0.0) and the walk's last segment cut
    /// 2.97 mm into the circle against the 1 mm contract. The geometry is exact: circle tops as
    /// span ends (the ownership rule exempts each end from its own circle only), `excess = 4/3`
    /// making `sag_depth(2, 4/3) = 1`, twice the radius — the drape unambiguously crosses the
    /// equators. No road wheels, so promotion is out of the picture and the ride is everything.
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

    /// The admission radius is EXACTLY the dedup's merge radius — one float each side, decided
    /// here so the two cannot drift apart silently: the dedup keeps the earlier of two adjacent
    /// abscissas, so the survivor of a merged bracket end is at most one float from the end, and
    /// anything farther must take the strict-hemisphere path.
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

    /// **The false-lift counterexample** (Codex, 2026-07-27, against the FIRST boundary fix): a
    /// spatial tolerance band on [`silhouette_y`] lifted a legitimate sample sitting ~5 µm outside
    /// a circle 0.25 m up to its corner. The scoping is now exact bracket membership instead of a
    /// band, so a sample the bracket did not produce must stay on the parabola: the span is
    /// stretched to 2.00002 m exactly so the uniform `t = 0.25` sample lands ~5 µm OUTSIDE the
    /// right circle's lateral boundary while the bracket end reconstructs the boundary itself.
    #[test]
    fn a_sample_just_outside_a_circle_is_not_lifted_to_its_corner() {
        let circles = [(Vec2::new(-2.0, 0.0), 0.5), (Vec2::new(0.0, 0.0), 0.5)];
        let (from, to) = (Vec2::new(0.0, 0.5), Vec2::new(-2.00002, 0.5));
        let excess = 4.0 / 3.0;
        let mut pts = Vec::new();
        sag_span(from, to, excess, &circles, SagClip::EveryCircle, &mut pts);
        let h = sag_depth(from.distance(to), excess);
        for q in &pts {
            // STRICTLY outside every circle's abscissa range: the bracket end at exactly
            // `|x − c.x| = r` is the legitimate silhouette corner (raised to `c.y` on purpose), so
            // the margin excludes it — the false lift this pins was ~5 µm out, well past 1 µm.
            let outside_all = circles.iter().all(|&(c, r)| (q.x - c.x).abs() > r + 1e-6);
            if !outside_all {
                continue;
            }
            // Recover this sample's parameter to evaluate the parabola where it actually sits.
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
        // And the ride contract still holds on the same span.
        for &circle in &circles {
            let cut = deepest_inside(&pts, circle);
            assert!(
                cut <= SAG_CLIP_INSET + 1e-6,
                "cut {cut} exceeds the ride budget"
            );
        }
    }

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

    /// **A span END inside a circle that does not own it.** The ride has one hole no probe grid can
    /// close by itself: when a penetration interval runs OFF the domain there is no crossing to
    /// bisect on that side, so [`sag_samples`] starts the ride at the limit `ta` (or ends it at
    /// `tb`) and the penetration's DEEPEST point is the span's own endpoint. A blanket
    /// end-exemption would then hand that endpoint through sitting inside a circle nothing ever
    /// lifts it out of. [`sag_span`] scopes the exemption by OWNERSHIP instead, and this is the case
    /// that separates the two.
    ///
    /// The fixture is synthetic — the shipped rig does not reach it (the bit pin above and `wrap`'s
    /// drawn-belt sweep are both unmoved by the ownership rule), so it is asserted here or nowhere —
    /// and it is DERIVED from the span's own start rather than eyeballed:
    ///
    /// * two equal end circles at equal height, so the taut walk leaves each at its TOP and both
    ///   span ends are exact;
    /// * a rogue wheel whose centre sits `off` from that start — BEHIND it in x, so the support
    ///   search skips it (`c.x <= lo`) and this stays a clip case rather than a promotion one, and
    ///   below it, so the endpoint falls inside. Its abscissa interval still covers the start, which
    ///   is what puts the penetration off the `t = 0` end;
    /// * the endpoint is `r − |off|` inside it, MEASURED 47.2 mm, 47× the ride budget.
    ///
    /// Both halves of the rule are exercised on the one drape: the SAME endpoint is legitimately on
    /// the idler, which must still hand it through, and the far end is on the sprocket, whose own
    /// penetration runs off the other end of the domain. The counterfactual is COMPUTED rather than
    /// remembered — the abscissa set does not depend on the clip decision, so putting the first
    /// point back where the blanket exemption left it IS that drawing.
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

        // The fixture IS the case — asserted, so it cannot quietly rot into a test of nothing.
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

        // The blanket end-exemption's drawing, and the proof this case discriminates.
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

        // The start was lifted ONTO the rogue's hemisphere — where the drape now attaches — by the
        // same "sits on" test that decides ownership.
        let lifted = pts[0];
        assert!(
            lifted.y > from.y,
            "the span's start was not lifted at all: {lifted:?} vs {from:?}"
        );
        assert!(
            (lifted.distance(rogue.0) - rogue.1).abs() <= SPAN_END_ON_CIRCLE,
            "the lifted start is not on the rogue wheel's hemisphere: {lifted:?}"
        );
        // …and the far end, whose own circle owns it, is still the tangent point the caller handed
        // in. (Its BIT-exactness is pinned by `a_zero_slack_drape_is_bit_identical_under_either_clip`;
        // what matters here is that the ownership rule did not move it.)
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
