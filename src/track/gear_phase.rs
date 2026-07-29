//! The running gear's PHASE LAW: belt travel → the angle every rotating node must carry, and the
//! one measurement that seats it — shared verbatim by the game's [`super::view`] and the sandbox's
//! `track_sandbox::wheel_view`.
//!
//! It used to be a ~240-line verbatim copy in each. That is exactly the kind of duplication a phase
//! lock cannot survive: the whole point of the derivation below is that ONE constant seats twenty
//! teeth, and two copies of it are two constants.
//!
//! # Three rolling radii, because there are three contacts
//!
//!   * **road wheels** roll on the FLAT INNER FACE of the shoes along the belly, so their no-slip
//!     radius is the measured tread — hub to that face. On flat ground the belly shoes are
//!     stationary against the ground, so a road wheel that spins at anything but `belt / tread`
//!     visibly scrubs.
//!   * **the idler** is toothless but WRAPPED, the opposite kinematic case: the belt segment around
//!     it rotates about the hub and the inextensible PIN LINE sets that wrap's angular rate, so it
//!     turns at `travel / R_pin` — the sprocket case minus teeth. Rolling it at the inner-face
//!     radius over-rotates it by pin→inner/R ≈ 7 %.
//!   * **the sprocket is not a radius at all** — see below.
//!
//! [`spin_angle`] answers the first two; [`tooth_angle`] answers the third.
//!
//! # The sprocket is TOOTH-LOCKED, not speed-derived
//!
//! A sprocket does not roll: its teeth sit between consecutive pins, so one link of belt travel is
//! *exactly* one tooth of rotation — `Δθ = τ / teeth` per `pitch` of travel, by definition, with no
//! radius in the statement at all. The teeth therefore stay seated in the same pin gaps forever, so
//! a meshing error you SEE is a real geometry error rather than two clocks running apart.
//!
//! It is worth being precise about why the obvious `phase / pitch_radius` is wrong, because the
//! radius is right there in [`super::rig_geom::RigGeom::rest`] and it is off by only 0.4 %. That
//! stored radius is the CHORD-exact pitch radius `pitch / (2 sin(π/teeth))` — the circle the pins
//! actually sit on, which is what the route must wrap. But the pins are joined by straight CHORDS of
//! length `pitch`, and a chord is shorter than the arc it subtends: belt travel per tooth is
//! `pitch`, while arc per tooth is `2π·R_chord/teeth`. Dividing travel by `R_chord` under-rotates by
//! `2·teeth·sin(π/teeth) / τ` — 0.41 % at 20 teeth, one whole tooth of drift every ~244 links
//! (~32 m of driving). The tooth statement has no such error term because it never divides a length
//! by a radius.
//!
//! # ...and it is PHASE-locked as well, which the rate alone does not give you
//!
//! A rate lock says the teeth never drift. It says nothing about WHERE they sit, and for a long time
//! nothing did: the sprocket's absolute angle was whatever fell out of the mesh's authored
//! orientation plus wherever the belt phase happened to be zero, which on the shipped Tiger left
//! every tooth a CONSTANT 5.99° — a third of a tooth, 44 mm of arc at the tip — off its pin gap.
//! Perfectly stable, permanently wrong, and invisible to a rate test.
//!
//! The rule (Yan, 2026-07-23) is the one a real sprocket obeys: **a tooth TIP bisects each adjacent
//! pin pair**, so pins sit at ±½ tooth from every tip and seat in the gullets. That is the same
//! geometry the chord-exact pitch radius already states — it is BY DEFINITION the circle on which
//! the chord between adjacent pins is one pitch, i.e. pins τ/teeth apart — so the phase lock adds no
//! new assumption, only the missing constant. [`tooth_angle`] is built from three facts, none of
//! them typed in:
//!
//!   1. where zero phase puts the first pin — [`super::rig_geom::RigGeom::belt_origin_angle`];
//!   2. the rule — half a tooth further round is where a tip must be;
//!   3. where this sprocket's teeth actually ARE — measured off its own mesh at bind
//!      ([`measure_tooth_tip_angle`]), never asserted. The shipped Tiger authors tooth 0 pointing
//!      straight up to within 0.094°, and both sides agree, but a re-export that turns the sprocket
//!      by half a tooth must move the calibration with it rather than silently un-mesh the track.
//!
//! # The residual, and the drift that used to sit beside it
//!
//! Measured end to end on the shipped Tiger, the nearest tip sits within a fraction of a degree of
//! the exact half tooth, on BOTH sides, in both drive directions, and it STAYS there: the wrap
//! spaces its pins at the MATERIAL pitch, not the drawn one ([`super::wrap::station_params`]), so
//! the drawn loop is read as a uniform-strain image of `pitch × link_count` and sampled in material
//! arc length. Resampling at the naive `polyline_len / link_count` instead spaces the pins at the
//! DRAWN pitch — ~0.07 % off on this rig — and the belt walked out from under the teeth at exactly
//! that rate: one tooth per ~160–195 m.

use bevy::math::Affine3A;
use bevy::mesh::VertexAttributeValues;
use bevy::prelude::*;

// ---------------------------------------------------------------------------------------------
// The phase law
// ---------------------------------------------------------------------------------------------

/// Belt travel → axle angle for a wheel the belt ROLLS on, wrapped per revolution in `f64` before
/// the `f32` cast so a long session's accumulated travel never erodes the angle's precision.
///
/// The sign is the one place the rotation convention lives, and it is forced: positive phase scrolls
/// the belly toward `+z` (the loop is ordered sprocket → belly → idler), so a wheel's contact point
/// must travel `+z`, and a positive rotation about `+x` moves the bottom of a wheel toward `−z`.
/// Hence negative. A degenerate radius parks the wheel rather than emitting a NaN transform that
/// would propagate into every child of the hull.
pub(crate) fn spin_angle(travel: f64, radius: f32) -> f32 {
    if radius <= 1e-6 {
        return 0.0;
    }
    let circumference = f64::from(radius) * std::f64::consts::TAU;
    -(travel.rem_euclid(circumference) / f64::from(radius)) as f32
}

/// Where a tooth TIP must sit at belt travel `travel`: the side-plane angle (rad) of the tip that
/// bisects the pin pair straddling the loop's arc-length origin.
///
/// Two terms and no fitting: `origin` (the angle zero phase puts pin 0 at) `+ ½ tooth` (the rule
/// itself) `+` one tooth per pitch of travel (the proven rate lock), wrapped per full REVOLUTION of
/// the sprocket (`teeth × pitch` metres of belt) in `f64`. Increasing, because positive travel
/// scrolls the belt forward around the sprocket's front, i.e. counter-clockwise in the `(z, y)`
/// side plane the angle is measured in.
pub(crate) fn tooth_tip_angle(travel: f64, pitch: f32, teeth: u32, origin: f32) -> f32 {
    let tooth = std::f32::consts::TAU / teeth as f32;
    let per_revolution = f64::from(pitch) * f64::from(teeth);
    let turn = (travel.rem_euclid(per_revolution) / per_revolution * std::f64::consts::TAU) as f32;
    origin + tooth / 2.0 + turn
}

/// Belt travel → sprocket node spin about the hull's lateral axis (rad): **put this mesh's own tooth
/// tip where the rule wants a tip**.
///
/// `mesh_tip` is where the mesh carries a tip at zero spin ([`measure_tooth_tip_angle`]); the target
/// is [`tooth_tip_angle`]. A positive spin about the hull's `+x` DECREASES a side-plane angle, so
/// the difference is taken tip-minus-target — the same flip [`spin_angle`] carries.
///
/// The tooth-per-pitch RATE is untouched by the calibration: only the target moves with travel, and
/// it moves by exactly one tooth per pitch, so the spin does too. Any representative of `mesh_tip`
/// modulo one tooth is as good as any other — which is precisely what makes a single measured
/// constant enough to seat all of them. A degenerate rig parks the sprocket rather than producing a
/// NaN transform.
pub(crate) fn tooth_angle(travel: f64, pitch: f32, teeth: u32, origin: f32, mesh_tip: f32) -> f32 {
    if f64::from(pitch) * f64::from(teeth) <= 1e-6 {
        return 0.0;
    }
    mesh_tip - tooth_tip_angle(travel, pitch, teeth, origin)
}

/// Reduce an angle to the signed representative nearest zero within one `period` — "how far off is
/// it, and which way", for readouts and for assertions about a periodic quantity.
pub(crate) fn fold(angle: f32, period: f32) -> f32 {
    (angle + period / 2.0).rem_euclid(period) - period / 2.0
}

// ---------------------------------------------------------------------------------------------
// Posing a bound node
// ---------------------------------------------------------------------------------------------

/// The node's local transform for `dy` metres of travel along `up` and `angle` of spin about `axle`.
///
/// Two things are load-bearing. The spin PRE-multiplies the authored rotation — that applies it in
/// the node's PARENT space, so it is a rotation about the hull's lateral axis whatever the node's
/// own baked orientation is; post-multiplying would spin about the node's own `x`, which a baked
/// 180° Y flip silently reverses (one track's wheels turning backwards). And the translation is
/// untouched by the spin, which is what makes this a rotation about the NODE ORIGIN — the
/// hand-corrected true axle (bake invariant `rotating_nodes_carry_their_own_axle_origin`) — rather
/// than about the model origin. The authored scale is carried, never assumed.
pub(crate) fn gear_transform(
    rest: &Transform,
    up: Vec3,
    axle: Vec3,
    dy: f32,
    angle: f32,
) -> Transform {
    Transform {
        translation: rest.translation + up * dy,
        rotation: Quat::from_axis_angle(axle, angle) * rest.rotation,
        scale: rest.scale,
    }
}

/// [`gear_transform`] for a HULL-FRAMED, hull-FIXED role (sprocket / idler): no travel, and the axle
/// is the hull's own `+X` because the node's parent chain to the root is identity. The game's gear
/// nodes are exactly that case.
pub(crate) fn gear_spin_transform(rest: &Transform, angle: f32) -> Transform {
    gear_transform(rest, Vec3::ZERO, Vec3::X, 0.0, angle)
}

// ---------------------------------------------------------------------------------------------
// Reading the teeth off the sprocket mesh
// ---------------------------------------------------------------------------------------------

/// Fraction of the rim radius a vertex must reach to count as tooth-TIP land. Two per cent of the
/// Tiger's 0.4328 m rim is 8.7 mm, which takes the whole tip land (its four corners span 0.4323 to
/// 0.4328 m) and nothing else: the next feature inboard is the gullet floor, 39 mm down.
const TIP_BAND: f32 = 0.98;
/// Quantile of the in-plane radii that anchors "this is the rim", and how far past it a vertex may
/// still sit and count. Same construction (and same reason) as [`super::marker_model`]'s disc
/// radius: a rim is a RING of hundreds of vertices so it always clears the quantile, while a stray
/// from a boolean or a loose greeble never does — and a raw `max` is the one statistic a single
/// stray destroys.
const RIM_QUANTILE: f32 = 0.95;
const RIM_BAND: f32 = 1.01;
/// How sharply the tip band must actually cluster on a `teeth`-fold grid before the measurement is
/// believed (the mean resultant length of the fitted harmonic, 0 = no structure, 1 = a delta). The
/// Tiger scores 0.587 — the tip is a LAND, not a point, so its own angular width caps the score well
/// below 1; anything that fails this is not a sprocket with this many teeth.
const TOOTH_CONCENTRATION: f32 = 0.25;

/// The `teeth`-fold phase of the tip land in a sprocket's `(radius, side-plane angle)` cloud — i.e.
/// one angle at which the mesh carries a tooth tip, reduced to `[0, τ/teeth)`.
///
/// The estimator is the `teeth`-th circular harmonic of the tip band: sum `e^{i·teeth·α}` over the
/// band and take the argument. That is not a heuristic — it IS the definition of the phase of a
/// `teeth`-fold rotational symmetry, so it uses every vertex of every tooth rather than picking a
/// feature, it is exact for a symmetric tip land whatever the land's width, and it needs no
/// clustering, no bin size and no ordering. Its magnitude comes out for free as the confidence that
/// the thing measured really has that symmetry ([`TOOTH_CONCENTRATION`]). `None` if the cloud does
/// not read as a tooth ring — the caller leaves the node unbound and retries.
pub(crate) fn measure_tooth_tip_angle(polar: &[(f32, f32)], teeth: u32) -> Option<f32> {
    if teeth == 0 || polar.len() < teeth as usize {
        return None;
    }
    let mut radii: Vec<f32> = polar.iter().map(|&(r, _)| r).collect();
    radii.sort_by(f32::total_cmp);
    let rim = radii[((RIM_QUANTILE * (radii.len() - 1) as f32) as usize).min(radii.len() - 1)];
    let tip = radii.iter().rev().find(|r| **r <= rim * RIM_BAND)?;
    let band = tip * TIP_BAND;

    // `f64` for the accumulation only: the sum runs over thousands of terms and its ARGUMENT is the
    // whole answer, so cancellation in the tail is not something to hand to an f32 accumulator.
    let (mut sx, mut sy, mut n) = (0.0_f64, 0.0_f64, 0_u32);
    for (_, angle) in polar.iter().filter(|&&(r, _)| r >= band) {
        let harmonic = f64::from(teeth) * f64::from(*angle);
        sx += harmonic.cos();
        sy += harmonic.sin();
        n += 1;
    }
    if n == 0 {
        return None;
    }
    let concentration = sx.hypot(sy) / f64::from(n);
    if concentration < f64::from(TOOTH_CONCENTRATION) {
        warn!(
            "running gear: a sprocket's rim does not read as a {teeth}-fold tooth ring \
             (concentration {concentration:.3} over {n} rim vertices, need \
             {TOOTH_CONCENTRATION}). Either the mesh's tooth count is not the authored one or the \
             node's origin is off its axle - the track cannot be phase-locked to teeth that are \
             not there; retrying.",
        );
        return None;
    }
    let tooth = std::f32::consts::TAU / teeth as f32;
    Some(((sy.atan2(sx) / f64::from(teeth)) as f32).rem_euclid(tooth))
}

/// Measure the tooth-tip phase of a bound sprocket node from its own mesh, in the HULL's side plane.
///
/// `hull_from_node` is the node's full hull-from-node affine — the game composes it from the node's
/// captured REST transform (its gear nodes are hull-framed), the sandbox from the whole parent chain
/// it walks. Doing the measurement in HULL space rather than mesh space is what makes it survive the
/// export: a baked 180° Y flip, a non-unit scale or a re-parenting all change where the teeth are in
/// the node's own frame, and none of them change where they are on the tank — which is the only
/// frame the answer is used in ([`gear_transform`] spins about the hull's lateral axis).
///
/// The mesh hangs on PRIMITIVE children of the node (`bevy_gltf` spawns one child per primitive), so
/// the walk composes hull ← node ← primitive and takes every position through it. `None` if the mesh
/// cannot be read yet or does not look like a `teeth`-fold star; the caller retries next frame.
pub(crate) fn sprocket_tooth_tip(
    node: Entity,
    hull_from_node: Affine3A,
    children: &Query<&Children>,
    transforms: &Query<&Transform>,
    primitives: &Query<&Mesh3d>,
    meshes: &Assets<Mesh>,
    teeth: u32,
) -> Option<f32> {
    // The node ORIGIN is the axle (hand-corrected onto it, and `super::marker_model` guards that),
    // so it is the centre every tooth angle is measured about — never a vertex statistic, which on a
    // toothed rim is not a circle in the first place.
    let axle = hull_from_node.transform_point3(Vec3::ZERO);
    let mut polar: Vec<(f32, f32)> = Vec::new();
    for child in children.get(node).ok()?.iter() {
        let Ok(primitive) = primitives.get(child) else {
            continue;
        };
        let mesh = meshes.get(&primitive.0)?;
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            continue;
        };
        let hull_from_mesh = hull_from_node
            * transforms
                .get(child)
                .map_or(Affine3A::IDENTITY, Transform::compute_affine);
        polar.extend(positions.iter().map(|p| {
            let v = hull_from_mesh.transform_point3(Vec3::from(*p)) - axle;
            // The side plane, by definition and not by inference: every axle in a tank's running
            // gear is lateral, so the tooth ring lives in `(z, y)` — the same plane the route,
            // `belt_origin_angle` and the spin axis are all expressed in, and `atan2(y, z)` is the
            // angle all three of them mean.
            (Vec2::new(v.z, v.y).length(), v.y.atan2(v.z))
        }));
    }
    measure_tooth_tip_angle(&polar, teeth)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::side::Side;

    /// The Tiger's authored counts, as the export carries them. INPUTS to the pure math below, not
    /// assertions about the file — a re-authoring that changes them must not turn this suite red.
    const TEETH: u32 = 20;
    const PITCH: f32 = 0.130;
    /// A belt origin and a mesh tooth phase that are both deliberately NOT round numbers, so a
    /// calibration term that silently dropped out of the rate math would show up here.
    const ORIGIN: f32 = 1.5199;
    const MESH_TIP: f32 = 0.0163;

    /// The spin this module would write for `travel`, at the fixture rig.
    fn spin(travel: f64) -> f64 {
        f64::from(tooth_angle(travel, PITCH, TEETH, ORIGIN, MESH_TIP))
    }

    /// THE sprocket property: one link of belt travel is exactly one tooth of rotation, forever.
    /// Checked after 5 000 links — a whole session of driving — because the failure mode this module
    /// exists to prevent is a slow drift, not a first-frame error.
    #[test]
    fn one_link_of_travel_is_exactly_one_tooth() {
        let tooth = std::f64::consts::TAU / f64::from(TEETH);
        let seated = spin(0.0);
        for links in [1_i32, 2, 19, 20, 21, 244, 5_000] {
            let travel = f64::from(links) * f64::from(PITCH);
            // Measured against the SEATED zero-phase angle, not against zero: the phase lock adds a
            // constant, and a rate test that could not tell the two apart is the test that let a
            // 5.99° mis-mesh sit here for a month. Rotation is negative (see `spin_angle`), and only
            // defined modulo a full turn.
            let residual = (seated - spin(travel) - f64::from(links) * tooth)
                .rem_euclid(std::f64::consts::TAU);
            let err = residual.min(std::f64::consts::TAU - residual);
            assert!(
                err < 1e-5,
                "{links} links landed {err} rad off a whole tooth"
            );
        }
    }

    /// ...and why it is not `phase / pitch_radius`. The chord-exact pitch radius is the circle the
    /// pins sit on, so it is right for the ROUTE and wrong for the ROTATION: chords are shorter than
    /// their arcs, so dividing travel by it under-rotates, and the error accumulates into a full
    /// tooth within a lap of the course. This test is the receipt for that claim.
    #[test]
    fn the_chord_radius_would_drift_a_whole_tooth() {
        let chord_radius = PITCH / (2.0 * (std::f32::consts::PI / TEETH as f32).sin());
        let tooth = std::f64::consts::TAU / f64::from(TEETH);
        // One link in, the two agree to well under a tooth — which is exactly why the bug hides.
        let one_link = f64::from(PITCH);
        let by_radius = -f64::from(spin_angle(one_link, chord_radius));
        assert!((by_radius - tooth).abs() < 0.01 * tooth);
        // ...but the RATES differ, and that is what accumulates. Measured over one link, where
        // neither angle has wrapped yet, so the comparison is of two raw rotations and not of two
        // residues: the tooth lock turns further per link, by a constant fraction.
        let per_link = spin(0.0) - spin(one_link);
        assert!(
            per_link > by_radius,
            "the chord radius under-rotates - a chord is shorter than its arc",
        );
        let links_to_a_tooth = tooth / (per_link - by_radius);
        assert!(
            (links_to_a_tooth - 244.0).abs() < 15.0,
            "expected a whole tooth of drift within ~244 links, got {links_to_a_tooth}",
        );
        // The tooth lock has none of it, at any distance.
        for links in [244.0, 2_440.0] {
            let travel = links * f64::from(PITCH);
            let residual = (spin(0.0) - spin(travel) - links * tooth).rem_euclid(tooth);
            assert!(residual.min(tooth - residual) < 1e-4);
        }
    }

    /// The rolling wheels turn the way a wheel turns: driving forward scrolls the belly toward `+z`,
    /// so the wheel's CONTACT POINT must travel `+z` with it, and the arc it sweeps must equal the
    /// belt travel exactly — that is what "no visible slip on flat ground" means numerically.
    #[test]
    fn a_rolling_wheel_matches_the_ground_it_rolls_on() {
        let radius = 0.41;
        let travel = 0.37;
        let angle = spin_angle(travel as f64, radius);
        assert!(
            angle < 0.0,
            "positive travel must turn the wheel negatively"
        );
        assert!(
            ((-angle * radius) - travel).abs() < 1e-5,
            "the swept arc must equal the belt travel",
        );
        // The bottom of the wheel really does move toward +z under that rotation.
        let bottom = Quat::from_rotation_x(angle) * Vec3::new(0.0, -radius, 0.0);
        assert!(bottom.z > 0.0, "the contact point went the wrong way");
        // And travel that wraps many revolutions still lands on the same angle as the wrapped one.
        let laps = travel as f64 + 7.0 * f64::from(radius) * std::f64::consts::TAU;
        assert!((spin_angle(laps, radius) - angle).abs() < 1e-4);
    }

    /// The composition rule, against the two things the export has actually shipped: a baked 180° Y
    /// rotation and a non-unit scale. Travel must stay hull-vertical, spin must stay about the
    /// HULL's lateral axis (not the node's flipped one), the scale must survive, and the node ORIGIN
    /// — the hand-corrected axle — must not move when only the spin changes.
    #[test]
    fn a_flipped_scaled_node_still_travels_up_and_spins_about_the_axle() {
        let rest = Transform {
            translation: Vec3::new(1.67, 0.51, -1.88),
            rotation: Quat::from_rotation_y(std::f32::consts::PI),
            scale: Vec3::splat(0.8),
        };
        let angle = -0.7;
        let posed = gear_transform(&rest, Vec3::Y, Vec3::X, 0.045, angle);

        assert_eq!(posed.scale, rest.scale, "the authored scale must survive");
        assert_eq!(
            posed.translation,
            rest.translation + Vec3::Y * 0.045,
            "travel is hull-vertical and nothing else",
        );
        // The node origin (the axle) is exactly the translation — spin does not move it.
        let spun_only = gear_transform(&rest, Vec3::Y, Vec3::X, 0.045, angle * 3.0);
        assert_eq!(spun_only.translation, posed.translation);

        // The visible rotation is a pure `angle` about the hull's +x, composed onto the rest pose —
        // NOT about the node's own (flipped) x. Post-multiplying would give the opposite sense.
        let want = Quat::from_rotation_x(angle) * rest.rotation;
        assert!(posed.rotation.angle_between(want) < 1e-5);
        let wrong = rest.rotation * Quat::from_rotation_x(angle);
        assert!(
            posed.rotation.angle_between(wrong) > 1.0,
            "the flip makes the two conventions genuinely different - that is the point",
        );
        // Concretely: a point on the rim still sweeps the right way in hull space.
        let rim = |t: &Transform| t.transform_point(Vec3::new(0.0, -0.4, 0.0)) - t.translation;
        assert!(
            rim(&posed).z > rim(&rest).z,
            "the bottom of the wheel must move toward +z",
        );
    }

    /// The hull-fixed shorthand the game's sprocket/idler take: the captured REST translation and
    /// scale are carried verbatim (overwriting them drags the node to the tank origin at phase
    /// zero), and the spin is a pure +X rotation composed in PARENT space.
    #[test]
    fn gear_spin_keeps_the_rest_translation_and_scale() {
        let rest = Transform {
            translation: Vec3::new(1.67, 0.51, -1.88),
            rotation: Quat::from_rotation_y(std::f32::consts::PI),
            scale: Vec3::splat(0.8),
        };
        let posed = gear_spin_transform(&rest, -0.7);
        assert_eq!(posed.translation, rest.translation);
        assert_eq!(posed.scale, rest.scale);
        assert_eq!(gear_spin_transform(&rest, 0.0).rotation, rest.rotation);
        assert_eq!(posed, gear_transform(&rest, Vec3::Y, Vec3::X, 0.0, -0.7));
    }

    /// A degenerate rig (a radius or a count the derivation could not fill in) parks the wheel
    /// instead of producing a NaN transform that would propagate into every child of the hull.
    #[test]
    fn a_degenerate_radius_parks_the_wheel() {
        assert_eq!(spin_angle(1.0, 0.0), 0.0);
        assert_eq!(tooth_angle(1.0, PITCH, 0, ORIGIN, MESH_TIP), 0.0);
        assert_eq!(tooth_angle(1.0, 0.0, TEETH, ORIGIN, MESH_TIP), 0.0);
        // ...and a mesh that is not a tooth ring is refused rather than averaged into a phase.
        assert_eq!(measure_tooth_tip_angle(&[], TEETH), None);
        let smooth_disc: Vec<(f32, f32)> = (0..720)
            .map(|i| (0.43, std::f32::consts::TAU * i as f32 / 720.0))
            .collect();
        assert_eq!(measure_tooth_tip_angle(&smooth_disc, TEETH), None);
    }

    // -----------------------------------------------------------------------------------------
    // The phase lock
    // -----------------------------------------------------------------------------------------

    /// A synthetic sprocket rim: `teeth` tip lands of `land` radians each, centred on
    /// `phase + k·τ/teeth`, plus a hub ring well inside them. Returned in the `(radius, angle)` form
    /// the measurement consumes, so the estimator is driven by a shape whose answer is KNOWN.
    fn synthetic_rim(teeth: u32, phase: f32, land: f32) -> Vec<(f32, f32)> {
        let tooth = std::f32::consts::TAU / teeth as f32;
        let mut polar = Vec::new();
        for k in 0..teeth {
            let centre = phase + tooth * k as f32;
            for j in 0..9 {
                let t = j as f32 / 8.0 - 0.5;
                polar.push((0.4328, centre + land * t));
            }
        }
        // The hub: a smooth ring at a radius the tip band must exclude, and four times as many
        // vertices as the teeth have, so anything that failed to band-limit would be swamped by it.
        for i in 0..(teeth * 4) {
            polar.push((0.3465, tooth * i as f32 / 4.0 + 0.37));
        }
        polar
    }

    /// The estimator recovers the phase of a known tooth ring — at any phase, any land width, and
    /// with a hub ring outvoting the teeth four to one.
    #[test]
    fn the_tooth_phase_estimator_recovers_a_known_ring() {
        let tooth = std::f32::consts::TAU / f32::from(20_u8);
        for phase in [0.0_f32, 0.0016, 0.1, tooth - 0.01] {
            for land in [0.001_f32, 0.05, 0.096] {
                let got = measure_tooth_tip_angle(&synthetic_rim(20, phase, land), 20)
                    .expect("a 20-fold ring measures");
                assert!(
                    fold(got - phase, tooth).abs() < 1e-4,
                    "phase {phase} land {land} measured as {got}",
                );
            }
        }
        // The answer is a REPRESENTATIVE modulo one tooth, so a ring authored a whole tooth round
        // reads identically — which is exactly why one measured constant seats all twenty teeth.
        let a = measure_tooth_tip_angle(&synthetic_rim(20, 0.1, 0.05), 20).unwrap();
        let b = measure_tooth_tip_angle(&synthetic_rim(20, 0.1 + tooth, 0.05), 20).unwrap();
        assert!((a - b).abs() < 1e-4);
    }

    /// **THE RULE**, on pure math: a tooth tip bisects every adjacent pin pair, so a pin lands in a
    /// gullet — at any phase, in either direction, forever.
    ///
    /// Pins are derived, not pinned: pin `k` sits at `origin + k·τ/teeth` when the belt has not
    /// moved (the chord-exact pitch circle is BY DEFINITION the one where consecutive pins are one
    /// tooth apart), and the whole set walks forward by `τ/teeth` per pitch of travel. The tooth the
    /// mesh carries is then rotated by [`tooth_angle`], and the assertion is that the two interleave
    /// at exactly half a tooth.
    #[test]
    fn a_pin_lands_in_a_gullet_at_every_phase() {
        let tooth = std::f32::consts::TAU / TEETH as f32;
        for links in [
            0.0_f64, 0.25, 0.5, 1.0, 1.5, 7.0, 19.5, 20.0, 41.0, -1.0, -13.75, -400.0, 5_000.0,
        ] {
            let travel = links * f64::from(PITCH);
            // Where the belt puts its pins on the sprocket, from the belt's own facts alone.
            let pin = ORIGIN + (links as f32) * tooth;
            // Where the mesh's teeth end up, from the spin this module writes. A spin of `θ` about
            // the hull's `+x` takes a side-plane angle to `angle − θ`.
            let tip = MESH_TIP - tooth_angle(travel, PITCH, TEETH, ORIGIN, MESH_TIP);
            let offset = fold(tip - pin, tooth);
            assert!(
                (offset.abs() - tooth / 2.0).abs() < 1e-4,
                "at {links} links the nearest tip sits {:.4}° from a pin - it must be exactly \
                 {:.4}° (half a tooth) for the pin to seat in a gullet",
                offset.to_degrees(),
                tooth.to_degrees() / 2.0,
            );
            // Said the other way round, because it is the way you check it on screen: the GULLET
            // (half a tooth off a tip) is where the pin is, to within a rounding.
            let gullet = tip + tooth / 2.0;
            assert!(fold(gullet - pin, tooth).abs() < 1e-4);
        }
    }

    /// The same rule on the SHIPPED Tiger, end to end: the sprocket mesh out of the glb, the belt
    /// origin out of the derived rig, and no constant in between. This is the test a re-export has
    /// to get past — turn the sprocket in Blender, or move the idler so the top run's tangent point
    /// shifts, and the calibration must follow rather than the track quietly un-meshing.
    #[test]
    fn the_shipped_tiger_seats_its_pins_in_its_gullets() {
        let rig = super::super::rig_geom::tiger_rig();
        let tooth = std::f32::consts::TAU / rig.teeth as f32;
        for side in [Side::Left, Side::Right] {
            let node = match side {
                Side::Left => "Sprocket_L",
                Side::Right => "Sprocket_R",
            };
            let mesh_tip = glb_sprocket_tip(node, rig.teeth);
            let origin = rig.belt_origin_angle(side);
            println!(
                "{node}: tooth tips at {:.4}° + k·{:.2}°, {:+.4}° off straight up; belt origin \
                 {:.4}°; seating correction {:+.4}°",
                mesh_tip.to_degrees(),
                tooth.to_degrees(),
                fold(mesh_tip - std::f32::consts::FRAC_PI_2, tooth).to_degrees(),
                origin.to_degrees(),
                fold(
                    tooth_angle(0.0, rig.pitch, rig.teeth, origin, mesh_tip),
                    tooth
                )
                .to_degrees(),
            );

            // The AUTHORING contract Yan states — "tooth 0 points straight up". Not assumed anywhere
            // in the derivation (the measured angle is what is actually used); asserted here so that
            // if a re-export stops honouring it, the report says so out loud instead of the
            // calibration silently absorbing it.
            assert!(
                fold(mesh_tip - std::f32::consts::FRAC_PI_2, tooth).abs() < 0.005,
                "{node}'s teeth are no longer authored with tooth 0 straight up",
            );

            for links in [0.0_f64, 0.5, 3.0, 9.25, 20.0, 137.0, -6.5] {
                let travel = links * f64::from(rig.pitch);
                let pin = origin + (links as f32) * tooth;
                let tip = mesh_tip - tooth_angle(travel, rig.pitch, rig.teeth, origin, mesh_tip);
                assert!(
                    (fold(tip - pin, tooth).abs() - tooth / 2.0).abs() < 1e-4,
                    "{node} at {links} links: the tips stopped bisecting the pins",
                );
            }
        }
    }

    /// The tooth-tip phase of one sprocket node of the SHIPPED glb, in the model's own side plane.
    ///
    /// A local glb walk rather than a reuse of [`super::super::marker_model`]'s: that reader is a
    /// CONTRACT on the marker set (it aborts the process on a gap, and it measures rim radii, not
    /// phases), while this needs raw positions of one named node under its full transform chain.
    /// Both sprocket nodes are top-level in today's export, so "the node's world transform" and "the
    /// hull-local one" coincide — the hull origin IS the model origin (see `rig_geom`'s frame note).
    fn glb_sprocket_tip(node_name: &str, teeth: u32) -> f32 {
        use bevy::math::Mat4;

        let path = crate::assets::asset_root().join(crate::tank::TIGER_GLB_PATH);
        let gltf::Gltf { document, mut blob } =
            gltf::Gltf::open(&path).expect("the Tiger glb opens");
        let buffers = [blob.take().expect("the glb carries its binary chunk")];
        let scene = document.scenes().next().expect("the glb carries a scene");
        let mut stack: Vec<(gltf::Node, Mat4)> =
            scene.nodes().map(|n| (n, Mat4::IDENTITY)).collect();
        while let Some((node, parent)) = stack.pop() {
            let world = parent * crate::track::marker_model::node_matrix(&node);
            if node.name() == Some(node_name)
                && let Some(mesh) = node.mesh()
            {
                let axle = world.transform_point3(Vec3::ZERO);
                let mut polar = Vec::new();
                for primitive in mesh.primitives() {
                    let reader = primitive.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
                    for p in reader
                        .read_positions()
                        .expect("the sprocket carries positions")
                    {
                        let v = world.transform_point3(Vec3::from(p)) - axle;
                        polar.push((Vec2::new(v.z, v.y).length(), v.y.atan2(v.z)));
                    }
                }
                return measure_tooth_tip_angle(&polar, teeth)
                    .unwrap_or_else(|| panic!("{node_name} does not read as a tooth ring"));
            }
            for child in node.children() {
                stack.push((child, world));
            }
        }
        panic!("{node_name} is not in the shipped glb");
    }
}
