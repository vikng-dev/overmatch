//! Regression tests for the sphere-probe distance reconstruction and its guard
//! ([`overmatch::sphere_cast_ground_contact`]): parry's shape-cast TOI
//! (`gjk::minkowski_ray_cast`, parry3d-0.27.0 `src/query/gjk/gjk.rs:661-780`) converges on a
//! RELATIVE tolerance (`eps_rel * max_bound`, `eps_rel ≈ 1.09e-3` for f32), so `hit.distance`
//! against a large collider is wrong at the millimetre-to-centimetre scale — measured up to
//! ~139 mm short casting a wheel-radius ball onto a 500 m-half-extent slab (up to ~200 mm live
//! on the 1000 m world slab), the energy pump behind the at-rest suspension limit cycle
//! (gunner-sight shake on flat ground). The witness point/normal of the SAME hit are exact when
//! the cast is non-penetrating, and the game's suspension reconstructs the hub-to-ground
//! distance from them, clamped to a band above the TOI-based value, with conservative fallbacks
//! for penetrating starts and corrupt witnesses.
//!
//! SCOPE — what these tests bind, exactly: the helper's math (reconstruction, band clamp,
//! fallback paths) against raw parry casts, and parry's TOI defect itself (the workaround-
//! retirement tripwire — if parry fixes the tolerance upstream, `raw_toi_error_still_present`
//! fires). They do NOT bind the `apply_suspension` call site, which is a thin adapter over the
//! helper; the live guard for that wiring is the idle at-rest harness metric (p.y spread
//! ≲ 0.02 mm).
//!
//! Run with `-- --nocapture` for the error-vs-extent table.

use avian3d::parry::math::{Pose, Vector};
use avian3d::parry::query::{ShapeCastOptions, cast_shapes};
use avian3d::parry::shape::{Ball, Cuboid};
use overmatch::{SPHERE_CAST_TOI_SLACK, sphere_cast_ground_contact};

/// The Tiger's effective roadwheel radius — the ball the sphere probe actually casts.
const WHEEL_RADIUS: f32 = 0.5166;
/// The game's probe retract (`SPHERE_PROBE_RETRACT`), used where a test mirrors the live cast.
const RETRACT: f32 = 0.3;

/// Cast the probe ball straight down onto a flat slab (top face at y = 0, like
/// `spawn_environment`), mirroring avian's `SpatialQuery::cast_shape` parry options exactly
/// (system_param.rs:559-586): parry shape 1 = the static target, shape 2 = the cast shape
/// carrying the velocity (so `witness1`/`normal1` are on the TERRAIN in its local frame), with
/// `stop_at_penetration: true` + `compute_impact_geometry_on_penetration: true` (avian's
/// `ShapeCastConfig` defaults). Returns `(time_of_impact, world point1, world normal1)`.
fn cast_down(half_extent: f32, start: Vector) -> Option<(f32, Vector, Vector)> {
    let ball = Ball::new(WHEEL_RADIUS);
    let ground = Cuboid::new(Vector::new(half_extent, 0.5, half_extent));
    let ground_pose = Pose::from_translation(Vector::new(0.0, -0.5, 0.0));
    let dir = Vector::new(0.0, -1.0, 0.0);
    let hit = cast_shapes(
        &ground_pose,
        Vector::ZERO,
        &ground,
        &Pose::from_translation(start),
        dir,
        &ball,
        ShapeCastOptions {
            max_time_of_impact: 2.0,
            stop_at_penetration: true,
            compute_impact_geometry_on_penetration: true,
            ..Default::default()
        },
    )
    .expect("cast_shapes should not error")?;
    // Convert the witness to world space the way avian hands it to `apply_suspension`
    // (system_param.rs:580-583).
    Some((
        hit.time_of_impact,
        ground_pose.transform_point(hit.witness1),
        ground_pose.rotation * hit.normal1,
    ))
}

/// Max raw-TOI / mean raw-TOI / max reconstructed ground-distance error (m) for a ball cast
/// straight down onto a flat-topped cuboid of the given half-extent, sweeping sub-millimetre
/// start offsets (the at-rest pose-jitter regime).
fn cast_error(half_extent: f32) -> (f32, f32, f32) {
    let dir = Vector::new(0.0, -1.0, 0.0);
    let mut max_err_raw = 0.0_f32;
    let mut sum_err_raw = 0.0_f32;
    let mut max_err_reconstructed = 0.0_f32;
    let mut count = 0u32;
    // Start heights around the retracted probe start (~0.877 m hub + 0.3 retract), stepped by
    // 0.05 mm — the per-tick pose delta scale at rest; lateral offsets like the wheel stations.
    for i in 0..200 {
        let y = 0.877_f32 + 0.3 + (i as f32) * 5.0e-5;
        for &(x, z) in &[(1.4_f32, 2.8_f32), (-1.4, -0.4), (1.4, -2.0), (-1.4, 2.0)] {
            let start = Vector::new(x, y, z);
            let Some((toi, point1, normal1)) = cast_down(half_extent, start) else {
                continue;
            };
            // Treat the cast start as the hub with a zero retract — legal: the helper takes the
            // retract as a parameter, and with zero retract the TOI-based model is simply
            // `toi + r`. Exact hub-to-ground distance is then just `y`.
            // Raw model — what the suspension used to compute.
            let raw = toi + WHEEL_RADIUS;
            max_err_raw = max_err_raw.max((raw - y).abs());
            sum_err_raw += (raw - y).abs();
            // Fixed model — the game's guarded witness-geometry reconstruction, fed exactly what
            // avian hands `apply_suspension`. On the flat slab the honest correction is inside
            // the TOI band, so the clamp must pass the reconstruction through EXACT — this also
            // fails if the band ever clips flat-ground corrections.
            let (reconstructed, contact) =
                sphere_cast_ground_contact(start, dir, WHEEL_RADIUS, 0.0, toi, point1, normal1);
            max_err_reconstructed = max_err_reconstructed.max((reconstructed - y).abs());
            assert_eq!(
                contact, point1,
                "non-penetrating cast must keep the witness contact"
            );
            count += 1;
        }
    }
    (
        max_err_raw,
        sum_err_raw / count as f32,
        max_err_reconstructed,
    )
}

#[test]
fn spherecast_reconstruction_beats_raw_toi_at_scale() {
    let mut raw_at_500 = 0.0_f32;
    for half in [5.0_f32, 50.0, 500.0] {
        let (max_raw, mean_raw, max_reconstructed) = cast_error(half);
        println!(
            "ground half-extent {half:7.1} m: raw TOI distance error max {:9.4} mm  mean {:9.4} mm  witness-reconstructed max {:9.4} mm",
            max_raw * 1000.0,
            mean_raw * 1000.0,
            max_reconstructed * 1000.0
        );
        // The bound the suspension relies on: reconstruction exact to < 0.001 mm at EVERY slab
        // size (measured 0.0001 mm) — far below the raw TOI's 0.25 mm best case, five orders
        // below its 139 mm worst.
        assert!(
            max_reconstructed < 1.0e-6,
            "witness-geometry reconstruction degraded at half-extent {half}: max error {max_reconstructed} m"
        );
        if half == 500.0 {
            raw_at_500 = max_raw;
        }
    }
    // The defect premise: raw TOI is still centimetres-wrong against a large slab (measured
    // 139 mm). If this fires, parry fixed the relative tolerance upstream (report candidate #4)
    // and the reconstruction workaround can be re-evaluated.
    assert!(
        raw_at_500 > 0.01,
        "raw shape-cast TOI is now accurate at 500 m half-extent (max error {raw_at_500} m) — \
         parry may have fixed the GJK tolerance; re-evaluate the witness reconstruction"
    );
}

/// Penetrating start (the hard-landing / rollback-restore-to-deep-pose regime): parry returns
/// TOI ≈ 0 with the witness rebuilt from the deepest-penetration CONTACT pair
/// (shape_cast_support_map_support_map.rs:31-51 under avian's options), whose normal is the
/// minimum-penetration axis — NOT cast geometry. The helper must NOT reconstruct from it: it
/// falls back to the TOI-based value `travel + r − retract`, the old formula's max-compression
/// path (full spring + bump-stop — the established hard-landing behavior), never "unsupported".
#[test]
fn penetrating_start_falls_back_to_conservative_toi_path() {
    // Ball centre 0.2 m above the slab top: the ball (r = 0.5166) starts 0.3166 m INTO the slab.
    let start = Vector::new(1.4, 0.2, 2.8);
    let dir = Vector::new(0.0, -1.0, 0.0);
    let (toi, point1, normal1) = cast_down(500.0, start).expect("penetrating cast must hit");
    assert!(
        toi < 1.0e-5,
        "expected a penetrating-start hit (TOI ~ 0), got TOI = {toi}"
    );
    let (ground_distance, contact) =
        sphere_cast_ground_contact(start, dir, WHEEL_RADIUS, RETRACT, toi, point1, normal1);
    // The old conservative path, exactly: `travel + r − retract` (= 0.2166 m at travel 0), which
    // is SHORTER than the rest length, so the wheel reads maximally compressed — supported.
    let toi_based = toi + WHEEL_RADIUS - RETRACT;
    assert_eq!(
        ground_distance, toi_based,
        "penetrating start must return the TOI-based conservative distance, not a reconstruction"
    );
    // The finite penetration-contact witness stays the force application point (old behavior).
    assert_eq!(contact, point1);
}

/// A witness that reconstructs OUTSIDE the trust band (feature flip at a slab edge, or any
/// witness pathology) must clamp to the band, bounding the step at the old TOI-error scale.
#[test]
fn out_of_band_reconstruction_clamps_to_toi_band() {
    let origin = Vector::new(0.0, 1.0, 0.0);
    let dir = Vector::new(0.0, -1.0, 0.0);
    let travel = 0.5_f32;
    let toi_based = travel + WHEEL_RADIUS - RETRACT;

    // Witness far BELOW the surface: reconstruction says 1.5 m — over the band top. Unclamped,
    // an over-long distance is what flips a deep wheel to "unsupported"; the clamp caps it.
    let (too_long, _) = sphere_cast_ground_contact(
        origin,
        dir,
        WHEEL_RADIUS,
        RETRACT,
        travel,
        Vector::new(0.0, -0.5, 0.0),
        Vector::new(0.0, 1.0, 0.0),
    );
    assert_eq!(too_long, toi_based + SPHERE_CAST_TOI_SLACK);

    // Witness ABOVE the surface: reconstruction says 0.5 m — below the TOI lower bound (parry's
    // TOI is never meaningfully long, so anything shorter than it is witness error, not truth).
    let (too_short, _) = sphere_cast_ground_contact(
        origin,
        dir,
        WHEEL_RADIUS,
        RETRACT,
        travel,
        Vector::new(0.0, 0.5, 0.0),
        Vector::new(0.0, 1.0, 0.0),
    );
    assert_eq!(too_short, toi_based);

    // In-band control: a flat-ground-consistent witness with a TOI ~33 mm short (the measured
    // p50 regime) passes through EXACT — the band must never clip the honest correction.
    let true_distance = 1.0_f32;
    let short_travel = (true_distance - WHEEL_RADIUS + RETRACT) - 0.033;
    let (exact, _) = sphere_cast_ground_contact(
        origin,
        dir,
        WHEEL_RADIUS,
        RETRACT,
        short_travel,
        Vector::new(0.0, 0.0, 0.0),
        Vector::new(0.0, 1.0, 0.0),
    );
    assert!(
        (exact - true_distance).abs() < 1.0e-6,
        "in-band reconstruction must be exact, got {exact} vs {true_distance}"
    );
}

/// A non-finite witness must not leak downstream: conservative TOI-based distance, and the
/// contact SYNTHESIZED on the cast axis (`origin + dir·ground_distance`) instead of the corrupt
/// `point1` flowing into `apply_force_at_point`.
#[test]
fn non_finite_witness_synthesizes_on_axis_contact() {
    let origin = Vector::new(0.0, 1.0, 0.0);
    let dir = Vector::new(0.0, -1.0, 0.0);
    let travel = 0.6834_f32;
    let toi_based = travel + WHEEL_RADIUS - RETRACT;

    let (ground_distance, contact) = sphere_cast_ground_contact(
        origin,
        dir,
        WHEEL_RADIUS,
        RETRACT,
        travel,
        Vector::splat(f32::NAN),
        Vector::new(0.0, 1.0, 0.0),
    );
    assert_eq!(ground_distance, toi_based);
    assert!(
        contact.is_finite(),
        "corrupt witness must never escape as the contact"
    );
    assert_eq!(contact, origin + dir * toi_based);

    // Non-finite NORMAL with a finite point: no reconstruction (falls back to the TOI-based
    // distance), but the finite witness point remains the contact.
    let point1 = Vector::new(0.0, 0.0, 0.0);
    let (gd2, contact2) = sphere_cast_ground_contact(
        origin,
        dir,
        WHEEL_RADIUS,
        RETRACT,
        travel,
        point1,
        Vector::splat(f32::NAN),
    );
    assert_eq!(gd2, toi_based);
    assert_eq!(contact2, point1);
}
