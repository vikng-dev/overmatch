//! The NEW suspension-model derivations the editor prototypes — pure math, no ECS, unit-tested.
//!
//! These are the "universal laws" of the source-of-truth split (design `track-model/`): everything
//! a tank does NOT author is derived here from the sharp sources (glb geometry + a handful of RON
//! knobs). The editor calls them to visualize the model; when the model graduates into the game
//! (per Yan's plan — settle the editor first, then wire the sim), these functions move verbatim into
//! the sim/view tiers. Nothing here reads a file, an asset, or the ECS — it is all `f32` in, `f32`
//! out, so the tests below pin the laws directly.

use bevy::math::Vec2;

/// Standard gravity (m/s²) — the load every static-deflection law divides by.
pub const G: f32 = 9.81;

/// The suspension authoring knobs that are NOT yet in the `.tank.ron` (`SupportSpec` today carries
/// only the belt-support penalty law). These are the emergent-suspension inputs the editor tweaks
/// live; graduating the model adds them to `TrackSpec`. Defaults are a plausible Tiger torsion-bar
/// setup (soft, ~1.2 Hz heave, moderately damped) — a starting point to tune against the model, not
/// a sourced datum.
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
/// (what Blender models) down to the fully-extended, spring-unloaded pose. The editor's green
/// max-droop cast shape is the rest circles lowered by exactly this.
pub fn static_deflection(ride_frequency: f32) -> f32 {
    G / (std::f32::consts::TAU * ride_frequency).powi(2)
}

/// Critical-damping-referenced damper coefficient: `c = 2 ζ √(k m)` (N·s/m). Reported for the panel;
/// the cast-shape geometry doesn't need it, but the graduated sim will.
pub fn damping_coefficient(sprung_mass: f32, ride_frequency: f32, zeta: f32) -> f32 {
    2.0 * zeta * (spring_rate(sprung_mass, ride_frequency) * sprung_mass).sqrt()
}

/// Track pitch from the two pin markers = `|pin1 − pin0|`. The pitch is READ from the link glb's
/// `pin-0`/`pin-1` empties, never authored — the physical rigid-link loop's one immutable length.
pub fn pitch_from_pins(pin0: Vec2, pin1: Vec2) -> f32 {
    (pin1 - pin0).length()
}

/// Sprocket pitch radius that GUARANTEES meshing: `r = pitch·teeth/(2π)`. Deriving the radius from
/// the tooth count (rather than measuring it off the mesh) makes tooth pitch equal link pitch by
/// construction — pins can't walk off the teeth. `teeth` is the authored count.
pub fn sprocket_pitch_radius(pitch: f32, teeth: u32) -> f32 {
    pitch * teeth as f32 / std::f32::consts::TAU
}

/// Link count that fills a belt loop of `perimeter` at `pitch`: `round(perimeter/pitch)`. The
/// rounding residual is the loop's tension/sag budget (the material loop is exact; the wrap is not).
pub fn link_count(perimeter: f32, pitch: f32) -> usize {
    (perimeter / pitch).round().max(1.0) as usize
}

/// Pin-line radius for a running-gear circle: `wheel_radius + thickness/2` (route.rs convention —
/// the pins ride half a track-thickness outboard of the wheel tread).
pub fn pin_line_radius(wheel_radius: f32, thickness: f32) -> f32 {
    wheel_radius + thickness * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprocket_radius_guarantees_tiger_mesh() {
        // 19 teeth × 0.130 pitch / τ — the RON's own derived 0.3931 m (spec.rs:231, ron:18).
        let r = sprocket_pitch_radius(0.130, 19);
        assert!((r - 0.3931).abs() < 1e-3, "got {r}");
        // Meshing invariant: tooth pitch (arc per tooth) == link pitch, exactly.
        let tooth_pitch = std::f32::consts::TAU * r / 19.0;
        assert!((tooth_pitch - 0.130).abs() < 1e-4);
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
        assert!((pitch_from_pins(Vec2::ZERO, Vec2::new(0.130, 0.0)) - 0.130).abs() < 1e-6);
        // Orientation-free: a diagonal pin span of length 0.13.
        let p = Vec2::new(0.078, 0.104); // 3-4-5 → 0.130
        assert!((pitch_from_pins(Vec2::ZERO, p) - 0.130).abs() < 1e-4);
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
    fn pin_line_sits_outboard_of_the_tread() {
        assert!((pin_line_radius(0.458, 0.117) - (0.458 + 0.0585)).abs() < 1e-6);
    }
}
