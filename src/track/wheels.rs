//! Field-driven VIEW wheel articulation (architecture §3): road wheels read the terrain oracle
//! directly and ease toward it — wheels first, then the route wraps them (`ground → wheels →
//! route`, acyclic). Pure functions; the caller owns where the lift is stored and which
//! transforms it writes (GLB view nodes in the game — never sim entities).
//!
//! The wheel's cosmetic travel spans the whole CONTACT ENVELOPE: from the DROOP ("green")
//! envelope (`max_droop` below the rest line, where the chain-clamped springs put a wheel over a
//! dip or in the air) up to the bump stop (`max_lift` above rest, where an obstacle bottoms it).
//! `wheel_lift_target` therefore reports the TRUE signed deepest reach of the lower arc — negative
//! on open ground (the wheel wants to drop to green), positive under terrain — clamped to that
//! `[-max_droop, max_lift]` band. A caller with `max_droop = 0` gets the old lift-only floor at
//! rest.
//!
//! Rise is a fast critically-damped ease integrated IMPLICITLY (unconditionally stable at any
//! ω·Δt); fall is ballistic (the wheel drops at gravity, not at a tuned rate), toward the target —
//! so an airborne wheel droops to green at gravity rather than snapping. One signed velocity
//! scalar of cosmetic state, shared by both branches.

use bevy::math::{Affine3A, Vec3};

use super::oracle::TerrainOracle;

/// Probe stations along a road wheel's lower arc as (sin θ, cos θ) from straight down, every 5°
/// to ±50° — fixed samples, so the wheel's terrain read is deterministic like every other
/// oracle consumer. Density matters: the lift target is FROZEN between an edge crossing two
/// adjacent probes, then catches up in one step — 5° keeps that step under the honest
/// circle-on-edge ramp.
const WHEEL_ARC: [(f32, f32); 21] = [
    (-0.766, 0.643),
    (-0.707, 0.707),
    (-0.643, 0.766),
    (-0.574, 0.819),
    (-0.500, 0.866),
    (-0.423, 0.906),
    (-0.342, 0.940),
    (-0.259, 0.966),
    (-0.174, 0.985),
    (-0.087, 0.996),
    (0.0, 1.0),
    (0.087, 0.996),
    (0.174, 0.985),
    (0.259, 0.966),
    (0.342, 0.940),
    (0.423, 0.906),
    (0.500, 0.866),
    (0.574, 0.819),
    (0.643, 0.766),
    (0.707, 0.707),
    (0.766, 0.643),
];

/// View wheel-lift parameters. `reach` is the wheel's ground surface (wheel radius + the track
/// plate riding between it and the ground); the lateral stations are the shoe's physics
/// collocation columns. `max_lift`/`max_droop` are the two ends of the contact envelope, both
/// POSITIVE magnitudes (project convention: never signed limits) — how far above rest an obstacle
/// may raise the wheel, and how far below rest it may drop.
pub struct WheelParams {
    pub reach: f32,
    pub ease_omega: f32,
    pub max_lift: f32,
    /// How far BELOW the rest line the wheel may droop (m, ≥ 0). `0.0` reproduces the old lift-only
    /// behaviour: the target then floors at rest instead of dropping toward the green envelope.
    pub max_droop: f32,
    pub lateral_stations: [f32; 3],
    pub probe_reach: f32,
}

/// The TRUE signed deepest reach of the wheel's lower arc — positive where terrain intrudes,
/// negative on open ground (the wheel wants to drop) — clamped to the contact envelope
/// `[-max_droop, max_lift]`. `pivot_local` is the wheel's REST pivot in hull-local space; `down`
/// is the hull's world down.
///
/// `TerrainOracle::depth_along` returns negative clearance, so on clear ground the arc max is
/// negative and the target lands at `-max_droop` (full droop). A caller passing `max_droop = 0.0`
/// gets exactly the old lift-only behaviour: the clamp floors at rest, and the seed of
/// `f32::NEG_INFINITY` is bit-equivalent to the old `0.0` seed once any station reads clearance
/// (both fold to the same non-negative max under a `min(max_lift)` cap).
pub fn wheel_lift_target<O: TerrainOracle>(
    oracle: &O,
    affine: &Affine3A,
    down: Vec3,
    pivot_local: Vec3,
    params: &WheelParams,
) -> f32 {
    let mut target = f32::NEG_INFINITY;
    for (s, c) in WHEEL_ARC {
        for offset in params.lateral_stations {
            let local = pivot_local + Vec3::new(offset, -params.reach * c, params.reach * s);
            target = target.max(oracle.depth_along(
                affine.transform_point3(local),
                down,
                params.probe_reach,
            ));
        }
    }
    target.clamp(-params.max_droop, params.max_lift)
}

/// Advance one wheel's lift state toward `target`: implicit critically-damped rise
/// (`v' = (v + ω²·e·Δt) / (1 + ωΔt)²` — stable for any ωΔt, settles ≈ 4.7/ω), ballistic fall
/// (an upward launch decelerates first). `dy`/`dvel` are the caller's stored state.
pub fn wheel_lift_step(dy: &mut f32, dvel: &mut f32, target: f32, dt: f32, params: &WheelParams) {
    let err = target - *dy;
    if err >= 0.0 {
        let wdt = params.ease_omega * dt;
        *dvel = (*dvel + params.ease_omega * params.ease_omega * err * dt)
            / (1.0 + 2.0 * wdt + wdt * wdt);
        *dy = (*dy + *dvel * dt).min(target);
    } else {
        *dvel -= 9.81 * dt;
        *dy = (*dy + *dvel * dt).clamp(target, params.max_lift);
        if *dy <= target {
            *dy = target;
            *dvel = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flat ground at height `y`, read with the oracle's DIRECTIONAL-cast semantics: penetration
    /// along a downward probe is `y − station.y` (positive when the station sits below the plane),
    /// saturated to ±`reach` like a real contact cast. Mirrors `envelope::FlatRest`, but at an
    /// arbitrary plane height so a scenario can sit the ground far below (open air), at the tread
    /// (kissing), or above it (a tall step).
    struct FlatGround {
        y: f32,
    }

    impl TerrainOracle for FlatGround {
        fn depth_along(&self, station: Vec3, out: Vec3, reach: f32) -> f32 {
            if out.y >= -1e-6 {
                return -reach;
            }
            ((self.y - station.y) / -out.y).clamp(-reach, reach)
        }
    }

    fn params(max_droop: f32, max_lift: f32) -> WheelParams {
        WheelParams {
            reach: 0.4,
            ease_omega: 45.0,
            max_lift,
            max_droop,
            lateral_stations: [-0.08, 0.0, 0.08],
            probe_reach: 0.5,
        }
    }

    /// Over open ground every arc station reads clearance, so the true max is negative and the
    /// target lands at exactly full droop (`-max_droop`), NOT the rest line — the whole point of
    /// the envelope: a wheel over a dip drops to green rather than hanging at rest.
    #[test]
    fn over_open_ground_the_wheel_droops_to_exactly_negative_max_droop() {
        let p = params(0.17, 0.12);
        // Rest pivot high above a distant floor: the lowest arc point is well clear.
        let pivot = Vec3::new(0.0, 3.0, 0.0);
        let target = wheel_lift_target(
            &FlatGround { y: 0.0 },
            &Affine3A::IDENTITY,
            Vec3::NEG_Y,
            pivot,
            &p,
        );
        assert_eq!(target, -p.max_droop);
    }

    /// With the tread + plate exactly kissing flat ground (pivot at `reach` above the surface) the
    /// deepest station sits on the plane — zero penetration, zero clearance — so the target is ~0
    /// (rest), inside the envelope and unclamped.
    #[test]
    fn on_flat_ground_kissing_the_tread_the_target_is_about_zero() {
        let p = params(0.17, 0.12);
        let pivot = Vec3::new(0.0, p.reach, 0.0);
        let target = wheel_lift_target(
            &FlatGround { y: 0.0 },
            &Affine3A::IDENTITY,
            Vec3::NEG_Y,
            pivot,
            &p,
        );
        assert!(target.abs() < 1e-6, "kissing target {target} should be ~0");
    }

    /// A tall step buries the lower arc: the deepest station saturates well past the bump stop, so
    /// the target clamps to exactly `max_lift`.
    #[test]
    fn a_tall_step_clamps_the_target_at_max_lift() {
        let p = params(0.17, 0.12);
        // Ground a whole reach above the pivot — the bottom stations are deep inside it.
        let pivot = Vec3::new(0.0, 0.4, 0.0);
        let target = wheel_lift_target(
            &FlatGround { y: pivot.y + 0.4 },
            &Affine3A::IDENTITY,
            Vec3::NEG_Y,
            pivot,
            &p,
        );
        assert_eq!(target, p.max_lift);
    }

    /// `max_droop = 0.0` reproduces the OLD lift-only computation bit for bit on a scenario with
    /// real penetration: the new signed-max-then-clamp folds to the same value the old
    /// `max`-from-zero-then-`min(max_lift)` did, because the clamp's lower bound is exactly the old
    /// zero floor.
    #[test]
    fn max_droop_of_zero_reproduces_the_lift_only_behaviour_bit_for_bit() {
        let p = params(0.0, 0.12);
        // Bottom station 3 cm inside the surface, upper arc clear — a genuine mixed read.
        let pivot = Vec3::new(0.0, p.reach, 0.0);
        let ground = FlatGround { y: 0.03 };
        let affine = Affine3A::IDENTITY;
        let down = Vec3::NEG_Y;

        // The pre-envelope algorithm, verbatim: seed 0.0, max the arc, cap at max_lift.
        let old = {
            let mut t = 0.0_f32;
            for (s, c) in WHEEL_ARC {
                for offset in p.lateral_stations {
                    let local = pivot + Vec3::new(offset, -p.reach * c, p.reach * s);
                    t = t.max(ground.depth_along(
                        affine.transform_point3(local),
                        down,
                        p.probe_reach,
                    ));
                }
            }
            t.min(p.max_lift)
        };
        let new = wheel_lift_target(&ground, &affine, down, pivot, &p);
        assert_eq!(
            new, old,
            "max_droop=0 must match the old lift-only target exactly"
        );
        assert!(
            new > 0.0,
            "the scenario should carry real penetration, got {new}"
        );
    }
}
