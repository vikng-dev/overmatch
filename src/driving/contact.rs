use bevy::prelude::*;

/// Width (m) of the trust band the witness reconstruction may move the sphere probe's ground
/// distance ABOVE the TOI-based value: `ground_distance` is clamped to
/// `[toi_based, toi_based + SPHERE_CAST_TOI_SLACK]` in [`sphere_cast_ground_contact`].
///
/// The band exists because parry's cast TOI is ONE-SIDED: it converges from below (short), never
/// meaningfully long, so `toi_based` is a lower bound on the true distance and `toi_based +` the
/// worst observed short-error is a sound upper bound. Sized from live SUSP_TRACE data (idle at
/// rest on the 1000 m slab, pre-reconstruction runs, `probe-origin y − ground_distance` per wheel
/// per tick): short error p50 33 mm, p99 134 mm, max 199.75 mm over 19,828 samples (idle run;
/// 194.9 mm in a second run), long-side excursions sub-mm (a tilt-projection artifact of the
/// measurement, not the cast). 0.18 would have clipped 5/19,828 honest at-rest corrections
/// (residual up to ~20 mm → ~11 kN spring error, the very noise class the reconstruction
/// removes); 0.20 clipped none. If a pose ever exceeds the band the clamp degrades gracefully:
/// the residual error is only the excess over the band (mm scale) and one-sided-conservative
/// (slightly short, the old TOI's direction) — never a NEW discontinuity class (ADR-0015).
pub const SPHERE_CAST_TOI_SLACK: f32 = 0.20;

/// `(hub-to-ground distance, contact point)` for the `SuspensionProbe::Sphere` probe: the
/// witness-geometry distance reconstruction, GUARDED by the TOI-based value it replaced. `travel`
/// is the shape cast's reported travel (`hit.distance`), `point1`/`normal1` its witness pair on
/// the terrain in world space, `retract` the cast's back-off up the axis
/// (`SPHERE_PROBE_RETRACT` at the call site; a parameter so tests can pin the math at any
/// retract).
///
/// WHY not `hit.distance` alone: parry's shape-cast TOI solver (`gjk::minkowski_ray_cast`,
/// parry3d-0.27.0 `src/query/gjk/gjk.rs:661-780`) converges on a RELATIVE tolerance —
/// `eps_rel · max_bound`, `eps_rel = sqrt(10·f32::EPSILON) ≈ 1.09e-3` (gjk.rs:141-144, 676) —
/// with an early "upper bounds inconsistencies" return besides (gjk.rs:713-724), so its accuracy
/// scales with the TARGET collider's extent. Against the 1000 m ground slab (`world.rs`) the
/// returned distance comes up to ~200 mm SHORT — one-sided and deterministic but DISCONTINUOUS in
/// pose, so at rest the 551 kN/m spring turned it into 10–40 kN per-wheel force noise per tick,
/// pumping the hull's at-rest modes (~2.2 kW measured) into a sustained ~12 mm / 0.29° wobble —
/// the gunner-sight shake on flat ground. The witness pair of the SAME call is exact even when
/// the TOI is wrong (measured `hit.point1.y = 0.000000` throughout; avian-0.7.0 converts parry's
/// terrain-frame witness to world space at `src/spatial_query/system_param.rs:580-583`), so the
/// travel is reconstructed from geometry instead: the cast ball's centre at true contact sits one
/// radius out along the terrain's outward normal, `centre = point1 + normal1·r`; the travel from
/// the retracted cast origin is `(centre − cast_origin)·dir`; and the hub-to-ground distance is
/// `travel + r − retract`, exactly the probe site's offset algebra. Substituting
/// `cast_origin = origin − dir·retract` cancels the retract, leaving the pure hub-frame form
/// computed here: `(centre − origin)·dir + r`. On flat ground that reduces to the hub height
/// exactly (the radius cancels against the centre offset), so ride height, preload, and the
/// ray/sphere equilibrium identity are untouched. Measured (`tests/spherecast_scale.rs`): raw TOI
/// error 139 mm at 500 m half-extent → 0.0001 mm reconstructed.
///
/// The reconstruction is trusted ONLY when the witness pair is actually cast geometry:
/// - **Penetrating start** (`travel < 1e-5`): parry rebuilds the hit from the deepest-penetration
///   CONTACT pair instead — `time_of_impact < 1.0e-5` swaps `witness1`/`normal1` for
///   `contact_support_map_support_map`'s point/normal (parry3d-0.27.0
///   `src/query/shape_cast/shape_cast_support_map_support_map.rs:31-51`; avian passes
///   `stop_at_penetration: true` + `compute_impact_geometry_on_penetration: true`,
///   avian3d-0.7.0 `src/spatial_query/system_param.rs:566-572`). That `normal1` is the
///   MINIMUM-PENETRATION separation axis — unrelated to the cast axis, possibly pointing into
///   the terrain relative to the probe — and parry documents the whole witness set as unreliable
///   for this status (`shape_cast.rs:39-56`, `ShapeCastStatus::PenetratingOrWithinTargetDist`).
///   Reconstructing from it can push the ball centre BELOW the hub and report the wheel
///   unsupported exactly when it is jammed deepest. Fall back to `toi_based` = `r − retract`,
///   the old formula's value for a zero-travel hit: max compression, full spring + bump-stop —
///   the established hard-landing behavior (travel clamp + progressive bump-stop slice).
/// - **Non-finite witness**: fall back to `toi_based`, and synthesize the contact ON THE CAST
///   AXIS rather than passing the corrupt `point1` downstream into `apply_force_at_point` (the
///   same NaN funnel the probe-origin guard closes). From the offset algebra above, the on-axis
///   ground point sits `ground_distance` from the hub along the cast:
///   `contact = origin + dir · ground_distance` (the ray probe's exact form; for the sphere,
///   centre-at-contact `origin + dir·(gd − r)` plus one radius on down the axis).
/// - **Valid witness**: `ground_distance = reconstructed.clamp(toi_based, toi_based +`
///   [`SPHERE_CAST_TOI_SLACK`]`)`. On flat ground the honest correction is ≤ ~200 mm (measured),
///   inside the band, so the reconstruction passes through exact. Where the witness feature-flips
///   at a slab edge (the closest-point witness jumping face↔edge along the medial axis,
///   pose-discontinuously), the band bounds the step at the OLD TOI-error scale — no new
///   discontinuity class beyond what ADR-0015 already absorbs.
///
/// SIM-AFFECTING: this sets every spring force; both wire ends run the same code by construction
/// (`SimPlugin`), so no protocol knob is needed. Upstream report candidate #4: parry GJK
/// shape-cast relative tolerance vs large shapes. `pub` (re-exported at the crate root) for
/// `tests/spherecast_scale.rs`, which pins the helper's math (reconstruction, band, fallbacks)
/// against raw parry casts AND parry's TOI defect itself — it fails if parry fixes the tolerance
/// upstream, the workaround-retirement tripwire. It does NOT bind the `apply_suspension` call
/// site (a thin adapter over this helper); the live guard for that wiring is the idle at-rest
/// harness metric (p.y spread ≲ 0.02 mm).
pub fn sphere_cast_ground_contact(
    origin: Vec3,
    dir: Vec3,
    wheel_radius: f32,
    retract: f32,
    travel: f32,
    point1: Vec3,
    normal1: Vec3,
) -> (f32, Vec3) {
    // The old formula — trust the TOI. One-sided short (never meaningfully long), so it is a
    // LOWER bound on the true distance: conservative (over-compresses, never under-supports).
    let toi_based = travel + wheel_radius - retract;
    // `1.0e-5` mirrors parry's own penetration cutoff (shape_cast_support_map_support_map.rs:35):
    // below it the witness pair is a penetration contact, not cast geometry (doc above).
    if travel >= 1.0e-5 && point1.is_finite() && normal1.is_finite() {
        let centre_at_contact = point1 + normal1 * wheel_radius;
        let reconstructed = (centre_at_contact - origin).dot(dir) + wheel_radius;
        let ground_distance = reconstructed.clamp(toi_based, toi_based + SPHERE_CAST_TOI_SLACK);
        (ground_distance, point1)
    } else if point1.is_finite() {
        // Penetrating start with a finite witness: conservative distance, but the deep-contact
        // point is still the honest place for drive/friction to act (the old path's choice).
        (toi_based, point1)
    } else {
        // Corrupt witness: conservative distance + the synthesized on-axis contact (doc above).
        (toi_based, origin + dir * toi_based)
    }
}
