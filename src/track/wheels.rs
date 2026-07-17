//! Field-driven VIEW wheel articulation (architecture §3): road wheels read the terrain oracle
//! directly and ease toward it — wheels first, then the route wraps them (`ground → wheels →
//! route`, acyclic). Pure functions; the caller owns where the lift is stored and which
//! transforms it writes (GLB view nodes in the game — never sim entities).
//!
//! Rise is a fast critically-damped ease integrated IMPLICITLY (unconditionally stable at any
//! ω·Δt); fall is ballistic (the wheel drops at gravity, not at a tuned rate). One signed
//! velocity scalar of cosmetic state, shared by both branches.

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
/// collocation columns.
pub struct WheelParams {
    pub reach: f32,
    pub ease_omega: f32,
    pub max_lift: f32,
    pub lateral_stations: [f32; 3],
    pub probe_reach: f32,
}

/// The deepest directional penetration of the wheel's lower arc into terrain — the lift target,
/// capped at `max_lift`. `pivot_local` is the wheel's REST pivot in hull-local space; `down` is
/// the hull's world down.
pub fn wheel_lift_target<O: TerrainOracle>(
    oracle: &O,
    affine: &Affine3A,
    down: Vec3,
    pivot_local: Vec3,
    params: &WheelParams,
) -> f32 {
    let mut target = 0.0_f32;
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
    target.min(params.max_lift)
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
