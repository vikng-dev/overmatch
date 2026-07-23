//! The NEW suspension-model derivations the editor prototypes — pure math, no ECS, unit-tested.
//!
//! These are the "universal laws" of the source-of-truth split (design `track-model/`): everything
//! a tank does NOT author is derived here from the sharp sources (glb geometry + a handful of RON
//! knobs). The editor calls them to visualize the model; when the model graduates into the game
//! (per Yan's plan — settle the editor first, then wire the sim), these functions move verbatim into
//! the sim/view tiers. Nothing here reads a file, an asset, or the ECS — it is all `f32` in, `f32`
//! out, so the tests below pin the laws directly.

use bevy::math::Vec3;

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
/// offset carries it). See [`crate::suspension_editor::model`] for the marker read.
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
