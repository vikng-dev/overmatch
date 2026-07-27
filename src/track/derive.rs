//! The NEW suspension-model derivations — pure math, no ECS, unit-tested.
//!
//! These are the "universal laws" of the source-of-truth split (design `track-model/`): everything
//! a tank does NOT author is derived here from the sharp sources (glb geometry + a handful of RON
//! knobs). They live beside the sandbox — the tier
//! that actually DRIVES the derived rig ([`super::rig_geom`] assembles them into geometry;
//! [`crate::track_sandbox::suspension_viz`] draws them). Nothing in THIS file reads a file, an asset, or the ECS —
//! it is all `f32` in, `f32` out, so the tests below pin the laws directly.
//!
//! One law that is deliberately NOT here: the hinge-angle limit. It looks derivable — the shoe mesh
//! decides how far a joint can fold — and a tri-tri sweep that measured it was built and thrown
//! away (2026-07-23). The Tiger's shoe is modelled with near-zero clearance, so the first-contact
//! angle moved with the penetration threshold (17.7° at 0.1 mm, 42.9° at 0.5 mm, 46.0° at 2 mm):
//! a guard built on it would pass or fail arbitrarily. And the mesh is only an upper bound in any
//! case — real articulation also spends pin/bushing clearance and end connectors the shoe does not
//! model. The limits are HAND-MEASURED and authored (`track.link_angle`); what survives here is
//! the DEMAND side, [`wrap_joint_angle`], which is pure geometry, and the clearance test that
//! stands on it (`super::rig_geom`) — plus the drape ride that steps by it (`super::route`).

use bevy::math::Vec3;

/// Standard gravity (m/s²) — the load every static-deflection law divides by.
pub const G: f32 = 9.81;

/// The suspension authoring knobs — the runtime mirror of the `.tank.ron` `suspension:`
/// block (`SuspensionSpec::params` is the seam). The sandbox tweaks these live; the game
/// reads them once at rig build. Defaults are a plausible Tiger torsion-bar setup (soft,
/// ~1.2 Hz heave, moderately damped) — the test fixture, not a sourced datum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SuspensionParams {
    /// Heave natural frequency (Hz). The single knob that sets spring softness: lower = softer =
    /// more static sink and more droop.
    pub ride_frequency: f32,
    /// Damping ratio ζ (dimensionless). 1.0 = critical; tanks run underdamped (~0.2–0.4).
    pub damping_ratio: f32,
    /// Bump-stop reserve (m): how far the wheel can rise ABOVE its loaded rest before bottoming.
    /// Sets the max-compression cast shape.
    pub bump_stop: f32,
}

impl Default for SuspensionParams {
    fn default() -> Self {
        Self {
            ride_frequency: 1.2,
            damping_ratio: 0.35,
            bump_stop: 0.12,
        }
    }
}

/// Undamped spring rate for a sprung mass: `k = m (2πf)²` (N/m). The emergent stiffness — you author
/// the *frequency* (a feel quantity that reads the same across tank weights), the rate follows.
pub fn spring_rate(sprung_mass: f32, ride_frequency: f32) -> f32 {
    sprung_mass * (std::f32::consts::TAU * ride_frequency).powi(2)
}

/// Static deflection under 1 g: `mg/k = g/(2πf)²` (m). Mass cancels — a 1.2 Hz suspension sinks the
/// same whether it carries 26 t or 57 t. This IS the max droop: the travel from the loaded rest pose
/// (what Blender models) down to the fully-extended, spring-unloaded pose. The green max-droop cast
/// shape ([`crate::track_sandbox::suspension_viz`]) is the rest circles lowered by exactly this.
pub fn static_deflection(ride_frequency: f32) -> f32 {
    G / (std::f32::consts::TAU * ride_frequency).powi(2)
}

/// Critical-damping-referenced damper coefficient: `c = 2 ζ √(k m)` (N·s/m). Reported for the panel;
/// the cast-shape geometry doesn't need it, but the graduated sim will.
pub fn damping_coefficient(sprung_mass: f32, ride_frequency: f32, zeta: f32) -> f32 {
    2.0 * zeta * (spring_rate(sprung_mass, ride_frequency) * sprung_mass).sqrt()
}

/// Track pitch from the two pin markers = `|Pin_End − Pin_Start|`. The pitch is READ from the glb's
/// `Pin_Start`/`Pin_End` empties, never authored — the physical rigid-link loop's one immutable
/// length.
pub fn pitch_from_pins(pin0: Vec3, pin1: Vec3) -> f32 {
    (pin1 - pin0).length()
}

/// Sprocket pin-line radius that GUARANTEES meshing: the pins seat as a CHORD of the pitch circle,
/// so `r = pitch / (2·sin(π/teeth))` — exact, not the `pitch·teeth/2π` ARC approximation (which
/// under-sizes by ~0.5% at 20 teeth; that was the RON's "0.3931 derived vs 0.3956 measured" gap).
/// This is the circle the pin CENTERS ride, so the route wraps it; the visible track-contact seat is
/// `r − pin_to_inner`. `teeth` is the authored count.
pub fn sprocket_pitch_radius(pitch: f32, teeth: u32) -> f32 {
    pitch / (2.0 * (std::f32::consts::PI / teeth as f32).sin())
}

/// The joint angle a wrap DEMANDS: how far each hinge must fold for a chain of `pitch`-long rigid
/// links to follow a circle of pin-line radius `r`.
///
/// The pins sit on the circle and the link between them is its CHORD, so the exterior angle at each
/// pin is `2·asin(pitch / 2r)` — the same chord relation [`sprocket_pitch_radius`] inverts. It is
/// the demand side of the hinge budget: the authored `track.link_angle.inward` is the supply, and
/// a wheel whose radius drops below `pitch / (2·sin(θ_max/2))` can no longer be wrapped at all. A
/// radius under half the pitch is geometrically impossible to wrap (the chord cannot exceed the
/// diameter) and returns π.
///
/// Two consumers, one relation: the wrap-clearance asset guard in `super::rig_geom`'s tests reads it
/// as a DEMAND against the authored hinge limit, and `super::route`'s drape ride steps its
/// hemisphere walk by it (the "pitch" there being the ride chord `route::sag_clip_chord`) — a chain
/// of chords following a circle is the same geometry whether the chords are links or samples.
pub fn wrap_joint_angle(pitch: f32, radius: f32) -> f32 {
    2.0 * (pitch / (2.0 * radius.max(1e-6))).clamp(-1.0, 1.0).asin()
}

/// Link count that fills a belt loop of `perimeter` at `pitch`: `round(perimeter/pitch)`. The
/// rounding residual is the loop's tension/sag budget (the material loop is exact; the wrap is not).
pub fn link_count(perimeter: f32, pitch: f32) -> usize {
    (perimeter / pitch).round().max(1.0) as usize
}

/// Pin-line radius from a running-gear contact surface: `contact_radius + pin_to_inner`. The track's
/// inner face rides the wheel/idler tread at `contact_radius`; the pin centers sit `pin_to_inner`
/// outboard of that (`pin_to_inner` is MEASURED from the link's Pin/Inner_Surface markers, not
/// assumed to be half the thickness). The two surface offsets — `pin_to_inner` and `pin_to_outer` —
/// are read independently, so there's no mid-plate assumption: asymmetric shoes just work, which is
/// also where the deferred grouser re-enters (put `Outer_Surface` on the cleat tip and the outer
/// offset carries it). See [`super::marker_model`] for the marker read.
pub fn pin_line_radius(contact_radius: f32, pin_to_inner: f32) -> f32 {
    contact_radius + pin_to_inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprocket_radius_guarantees_tiger_mesh() {
        // Chord-exact pin circle for the model's 20 teeth at the measured 0.13043 pitch.
        let r = sprocket_pitch_radius(0.13043, 20);
        assert!((r - 0.41688).abs() < 1e-3, "got {r}");
        // Meshing invariant: the pin CHORD between adjacent teeth equals the link pitch, exactly.
        let chord = 2.0 * r * (std::f32::consts::PI / 20.0).sin();
        assert!((chord - 0.13043).abs() < 1e-5);
        // The arc approximation would under-size it — the source of the RON's 2.5 mm gap.
        assert!(r > 0.13043 * 20.0 / std::f32::consts::TAU);
    }

    /// The wrap angle and the sprocket radius are ONE constraint seen from two ends — the chord
    /// relation, inverted. Anything that "fixes" one without the other breaks this.
    #[test]
    fn the_wrap_angle_inverts_the_sprocket_chord_relation() {
        let pitch = 0.13043;
        for teeth in [12_u32, 20, 33] {
            let r = sprocket_pitch_radius(pitch, teeth);
            // Wrapping a sprocket costs exactly one tooth pitch per joint: τ/teeth, no radius in
            // sight. That the radius-based formula reproduces it is the whole check.
            let per_tooth = std::f32::consts::TAU / teeth as f32;
            assert!(
                (wrap_joint_angle(pitch, r) - per_tooth).abs() < 1e-5,
                "{teeth} teeth: {} vs {per_tooth}",
                wrap_joint_angle(pitch, r),
            );
        }
        // Bigger circle, gentler joint — the monotonicity every clearance argument leans on.
        assert!(wrap_joint_angle(pitch, 0.30) > wrap_joint_angle(pitch, 0.42));
        // The Tiger's three wraps, from the current geometry (idler is the tightest).
        assert!((wrap_joint_angle(pitch, 0.3675).to_degrees() - 20.44).abs() < 0.02);
        assert!((wrap_joint_angle(pitch, 0.4126).to_degrees() - 18.19).abs() < 0.02);
        // A circle the chain cannot wrap at all: the chord would have to exceed the diameter.
        assert_eq!(wrap_joint_angle(pitch, pitch * 0.4), std::f32::consts::PI);
        // ...and the degenerate radius is clamped rather than dividing by zero.
        assert!(wrap_joint_angle(pitch, 0.0).is_finite());
    }

    #[test]
    fn static_deflection_is_mass_independent_and_softens_with_frequency() {
        // g/(2π·1.2)² ≈ 0.1727 m.
        assert!((static_deflection(1.2) - 0.1727).abs() < 1e-3);
        // Softer spring (lower f) droops more.
        assert!(static_deflection(0.9) > static_deflection(1.5));
        // Mass truly cancels: deflection depends only on f.
        let by_mass = |m: f32| G * m / spring_rate(m, 1.2);
        assert!((by_mass(26_000.0) - by_mass(57_000.0)).abs() < 1e-4);
        assert!((by_mass(57_000.0) - static_deflection(1.2)).abs() < 1e-4);
    }

    #[test]
    fn tiger_loop_recovers_authored_link_count() {
        // 12.610 m material loop / 0.130 pitch = 97 links (ron:6-7).
        assert_eq!(link_count(12.610, 0.130), 97);
        // And the residual is what the RON calls slack: 97×0.130 vs a 12.577 taut envelope.
        assert_eq!(link_count(12.577, 0.130), 97);
    }

    #[test]
    fn pitch_reads_off_pin_markers() {
        assert!((pitch_from_pins(Vec3::ZERO, Vec3::new(0.0, 0.130, 0.0)) - 0.130).abs() < 1e-6);
        // Orientation-free: a diagonal pin span of length 0.13.
        let p = Vec3::new(0.078, 0.0, 0.104); // 3-4-5 → 0.130
        assert!((pitch_from_pins(Vec3::ZERO, p) - 0.130).abs() < 1e-4);
    }

    #[test]
    fn spring_and_damper_are_positive_and_scale() {
        let k = spring_rate(57_000.0, 1.2);
        assert!(k > 0.0);
        // Doubling mass doubles rate at fixed frequency.
        assert!((spring_rate(114_000.0, 1.2) / k - 2.0).abs() < 1e-4);
        let c = damping_coefficient(57_000.0, 1.2, 0.35);
        assert!(c > 0.0);
        // Critical damping (ζ=1) is the √(km) reference × 2.
        let cc = damping_coefficient(57_000.0, 1.2, 1.0);
        assert!((cc / c - 1.0 / 0.35).abs() < 1e-3);
    }

    #[test]
    fn pin_line_sits_outboard_of_the_contact_surface() {
        // Tiger: measured tread 0.405 + measured pin→inner 0.0246 ≈ 0.4296 pin line (the correct
        // wheel circle — vs the old inflated 0.458 + thickness/2 = 0.5165).
        assert!((pin_line_radius(0.405, 0.0246) - 0.4296).abs() < 1e-4);
    }
}
