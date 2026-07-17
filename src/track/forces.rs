//! The belt force model (architecture §1 "SIM forces", phase B): support, traction, and belt
//! dynamics for one track side — the SINGLE implementation, consumed by the game's sim plugin
//! and by the track sandbox (which is where every piece of it was developed and feel-tested;
//! provenance in `track_sandbox` steps 17–26 and HQ.md).
//!
//! Pure math, no ECS: the caller supplies the pose affine, a velocity field, the terrain
//! oracle, and applies the returned forces itself (in report order — force-accumulator float
//! order is part of bit-reproducibility). Everything here is deterministic closed-form
//! arithmetic — no spatial queries, no BVH, safe under rollback replay by construction.
//!
//! The model, per station segment × three lateral collocation columns:
//! - **Support**: directional field depth at pin/mid/pin on the outer face → two-piece
//!   clipped-linear pressure profile → penalty spring along the belt's own inward normal
//!   (minus normal-velocity damping, soft engagement ramp), applied at the profile centroid on
//!   the terrain surface. Roll/pitch/weight transfer are lever-arm implicit.
//! - **Traction**: slip-saturated friction on an ellipse — longitudinal slip against the belt
//!   surface speed, lateral scrub against the hull's side motion (`lateral_ratio` of the
//!   grip), combined magnitude capped. Longitudinal force reacts back into belt dynamics.
//! - **Belt dynamics**: constant-power engine curve under a low-speed force cap, a governor
//!   chasing `command × max_speed`, ground reaction, reflected inertia; phase advection.

use bevy::math::{Affine3A, Vec2, Vec3};

use super::oracle::TerrainOracle;
use super::route::{polyline_len, resample};

/// Belt-speed floor (m/s) for the constant-power curve — keeps stall force finite. Global
/// numerical policy, not vehicle data.
const STALL_SPEED: f32 = 0.5;

/// The force model's parameters: vehicle data (spec-authored) + the per-metre support law.
/// Nothing here is solver-quality policy — quality lives in the station/column geometry the
/// caller authors (link pitch sets station density).
pub struct ForceParams {
    /// Plate thickness (m); the pin line runs mid-plate, contacts probe the outer face.
    pub thickness: f32,
    /// Lateral collocation columns: (offset from centreline, weight). Weights sum to 1;
    /// edge offsets at ±width/2 with Simpson-style weights reproduce a uniform strip's load
    /// AND roll moment exactly.
    pub columns: [(f32, f32); 3],
    /// Support spring (N/m per metre of contacting belt) and damping (N·s/m per metre).
    pub support_stiffness_per_m: f32,
    pub support_damping_per_m: f32,
    /// Soft-engagement ramp depth (m): full support only past this penetration.
    pub engage_depth: f32,
    /// Terrain probe reach (m).
    pub probe_reach: f32,
    /// Coulomb friction coefficient (longitudinal) and the lateral share of the grip ellipse
    /// (< 1 is what lets a skid-steer pivot).
    pub mu: f32,
    pub lateral_ratio: f32,
    /// Slip speed (m/s) at which friction saturates to μ·load.
    pub slip_saturation: f32,
    /// Powertrain: top belt speed (m/s), per-track engine power (W) and low-speed force cap
    /// (N), governor gain (N per m/s of speed error), reflected belt+drivetrain inertia (kg).
    pub max_speed: f32,
    pub engine_power: f32,
    pub engine_force: f32,
    pub governor_gain: f32,
    pub inertia: f32,
}

/// One side's dynamic state — the caller owns it (the game's `TrackDrive` component, the
/// sandbox's `BeltSpeed`/`BeltPhase` resources).
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct SideState {
    /// Belt surface speed (m/s).
    pub speed: f32,
    /// Belt travel (m) — advects the force stations; also the view's scroll phase. `f64`: it
    /// grows unbounded and an f32 loses sub-pitch precision within a long match's driving
    /// distance (codex phase-B finding 8).
    pub phase: f64,
}

/// One side's per-tick input.
pub struct SideInput<'a> {
    /// The CLOSED pin-line loop polyline (last point == first), side plane (z, y). Rest
    /// geometry — road-wheel articulation is view-only and carries no force.
    pub loop_pts: &'a [Vec2],
    /// Station count (the material link count).
    pub count: usize,
    /// Signed track-centreline x (hull-local).
    pub plane_x: f32,
    /// Drive command −1..1 (throttle ± steer, capability-gated by the caller).
    pub command: f32,
}

/// One force application, in emission order. The caller applies these verbatim — order is
/// part of bit-reproducibility (float accumulation).
pub struct ForceApp {
    pub force: Vec3,
    pub point: Vec3,
}

/// One contact's telemetry (viz / traces): world application point, elastic load, slip,
/// inward normal, traction vector.
pub struct BeltContact {
    pub point: Vec3,
    pub load: f32,
    pub slip: f32,
    pub normal: Vec3,
    pub traction: Vec3,
}

/// What one side's tick produced.
#[derive(Default)]
pub struct SideReport {
    pub state: SideState,
    pub apps: Vec<ForceApp>,
    pub contacts: Vec<BeltContact>,
}

/// Integrate `max(0, pen(x))` over one linear piece of a pressure profile: `pen` runs
/// `p0 → p1` across `[x0, x1]`. Returns `(∫pen dx, ∫x·pen dx, contacting length)`, clipping
/// the sub-range where the profile is negative (that part of the plate is clear of the
/// ground). Closed form, so the plate's resultant force and centroid are smooth functions of
/// pose — no sampling noise.
pub fn clipped_linear_piece(x0: f32, x1: f32, p0: f32, p1: f32) -> (f32, f32, f32) {
    let w = x1 - x0;
    if w <= 0.0 || (p0 <= 0.0 && p1 <= 0.0) {
        return (0.0, 0.0, 0.0);
    }
    if p0 >= 0.0 && p1 >= 0.0 {
        // Trapezoid: A = w·(p0+p1)/2; M = ∫x·pen dx with pen linear in x.
        let area = w * (p0 + p1) / 2.0;
        let moment = w * (p0 * (2.0 * x0 + x1) + p1 * (x0 + 2.0 * x1)) / 6.0;
        return (area, moment, w);
    }
    // One end negative: clip at the zero crossing and integrate the positive triangle.
    let xc = x0 + w * (p0 / (p0 - p1));
    if p0 > 0.0 {
        clipped_linear_piece(x0, xc, p0, 0.0)
    } else {
        clipped_linear_piece(xc, x1, 0.0, p1)
    }
}

/// The drivetrain force available to spin one track's belt at the given belt speed: a
/// constant-power curve (force × speed can't exceed `engine_power`) under the low-speed
/// torque cap `engine_force`.
fn engine_available(params: &ForceParams, belt_speed: f32) -> f32 {
    (params.engine_power / belt_speed.abs().max(STALL_SPEED)).min(params.engine_force)
}

/// Advance one side by one fixed tick: compute support + traction at the advected stations
/// (probing `oracle` at the presented `affine`, reading the hull's velocity field through
/// `vel_at`), integrate belt dynamics, and return the forces for the caller to apply IN
/// ORDER. Force application does not feed back into `vel_at` within a tick (velocities
/// integrate later), so reading everything first and applying afterwards is exact.
pub fn step_side<O: TerrainOracle>(
    input: &SideInput,
    state: SideState,
    affine: Affine3A,
    dt: f32,
    params: &ForceParams,
    oracle: &O,
    vel_at: impl Fn(Vec3) -> Vec3,
) -> SideReport {
    let mut report = SideReport {
        state,
        ..Default::default()
    };
    let belt_speed = state.speed;
    let mut belt_reaction = 0.0;

    let pitch = polyline_len(input.loop_pts) / input.count.max(1) as f32;
    let mut stations = resample(
        input.loop_pts,
        pitch,
        state.phase.rem_euclid(f64::from(pitch)) as f32,
    );
    stations.truncate(input.count);
    let n = stations.len();
    if n < 3 {
        return report;
    }

    for i in 0..n {
        let a = stations[i];
        let b = stations[(i + 1) % n];
        let seg = b - a;
        let len = seg.length();
        if len < 1e-4 {
            continue;
        }
        let tan2 = seg / len;
        let out2 = Vec2::new(tan2.y, -tan2.x);

        let wa = affine.transform_point3(Vec3::new(input.plane_x, a.y, a.x));
        let wb = affine.transform_point3(Vec3::new(input.plane_x, b.y, b.x));
        let out = affine
            .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
            .normalize_or_zero();
        let axis = (wb - wa) / len;
        let lat = out.cross(axis);
        let face = out * (params.thickness / 2.0);

        // WIDTH: the shoe is sampled as three lateral COLUMNS (edges + centre): each column
        // runs the full profile machinery on its own three stations with its weight of the
        // per-metre coefficients and applies its resultant at its own point — roll torque
        // from a curb under one track edge, cross-slope contact, and half-off-a-ledge
        // support all emerge from the application points.
        for (offset, weight) in params.columns {
            let shift = lat * offset;
            let ca = wa + shift;
            let cb = wb + shift;

            // The three collocation stations, on the outer face; depth along the link's own
            // outward normal (cast semantics).
            let pen_a = oracle.depth_along(ca + face, out, params.probe_reach);
            let pen_m = oracle.depth_along((ca + cb) / 2.0 + face, out, params.probe_reach);
            let pen_b = oracle.depth_along(cb + face, out, params.probe_reach);
            let pen_max = pen_a.max(pen_m).max(pen_b);
            if pen_max <= 0.0 {
                continue;
            }

            let (a1, m1, l1) = clipped_linear_piece(0.0, len / 2.0, pen_a, pen_m);
            let (a2, m2, l2) = clipped_linear_piece(len / 2.0, len, pen_m, pen_b);
            let (area, moment, contact_len) = (a1 + a2, m1 + m2, l1 + l2);
            if area <= 0.0 {
                continue;
            }
            // Resultant at the terrain surface, on this column: the profile's own value at
            // the centroid position. (The normal force is offset-invariant along its own
            // line; the traction lever is not.)
            let x_c = moment / area;
            let pen_c = if x_c <= len / 2.0 {
                pen_a + (pen_m - pen_a) * (x_c / (len / 2.0))
            } else {
                pen_m + (pen_b - pen_m) * ((x_c - len / 2.0) / (len / 2.0))
            }
            .max(0.0);
            let p = ca + axis * x_c + out * (params.thickness / 2.0 - pen_c);

            // (1) Support: penalty spring along the belt's own inward normal, at the
            // column's share of the per-metre coefficients.
            let normal = -out;
            let vel = vel_at(p);
            let engage = (pen_max / params.engage_depth).clamp(0.0, 1.0);
            let load = weight
                * (params.support_stiffness_per_m * area
                    - params.support_damping_per_m * contact_len * vel.dot(normal))
                .max(0.0)
                * engage;
            if load <= 0.0 {
                continue;
            }
            report.apps.push(ForceApp {
                force: normal * load,
                point: p,
            });

            // (2) Traction: slip-saturated friction on the ellipse; grip scales with the
            // column's load.
            let mut slip_long = 0.0;
            let mut traction = Vec3::ZERO;
            let drive = -affine.transform_vector3(Vec3::new(0.0, tan2.y, tan2.x));
            let long_plane = drive - drive.dot(normal) * normal;
            if long_plane.length() > 1e-4 {
                let long_dir = long_plane.normalize();
                let lat_dir = normal.cross(long_dir).normalize_or_zero();
                slip_long = belt_speed - vel.dot(long_dir);
                let s_lat = vel.dot(lat_dir);
                let grip = params.mu * load;
                let grip_lat = grip * params.lateral_ratio;
                let mut f_long = grip * (slip_long / params.slip_saturation).clamp(-1.0, 1.0);
                let mut f_lat = -grip_lat * (s_lat / params.slip_saturation).clamp(-1.0, 1.0);
                let e = (f_long / grip).powi(2) + (f_lat / grip_lat).powi(2);
                if e > 1.0 {
                    let s = e.sqrt().recip();
                    f_long *= s;
                    f_lat *= s;
                }
                traction = long_dir * f_long + lat_dir * f_lat;
                report.apps.push(ForceApp {
                    force: traction,
                    point: p,
                });
                belt_reaction += f_long;
            }

            // Telemetry load = the elastic component only, at the column's weight.
            report.contacts.push(BeltContact {
                point: p,
                load: weight * params.support_stiffness_per_m * area * engage,
                slip: slip_long,
                normal,
                traction,
            });
        }
    }

    // Belt dynamics + advection: governor toward the command under the constant-power curve,
    // ground reaction, reflected inertia; phase advects at the PRE-update speed.
    let target = input.command * params.max_speed;
    let avail = engine_available(params, belt_speed);
    let engine = (params.governor_gain * (target - belt_speed)).clamp(-avail, avail);
    let next = belt_speed + (engine - belt_reaction) / params.inertia * dt;
    report.state.speed = next.clamp(-params.max_speed, params.max_speed);
    report.state.phase = state.phase + f64::from(belt_speed * dt);
    report
}
