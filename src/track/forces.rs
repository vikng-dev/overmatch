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

/// The static-grip shear modulus (m): the Janosi–Hanamoto `K` for RUBBER TRACK PADS ON
/// FIRM GROUND (Wong & Chiang's measured parameter set — the terramechanics provenance,
/// static-friction-design.md §3). Full grip develops over this much shear; a 20° park
/// settles back ~28 mm before holding — the vehicle "rocks onto its brakes". When the
/// ground-type mechanic lands, TERRAIN owns this dial (firm soil ~10 mm … loose sand 25 mm).
///
/// Deliberately NOT the design draft's 5 mm park target: at 64 Hz that stiffness drove a
/// full-amplitude coupled roll/yaw limit cycle (measured: 32 Hz side-swap, 7× per-tick load
/// swings — the textbook undamped-bristle oscillation). At 75 mm every coupled mode sits
/// deep inside the semi-implicit stability region (ωΔt ≈ 0.27; per-tick growth damping
/// ratio ≈ 0.07 of the limit).
pub const GRIP_SHEAR_MODULUS_M: f32 = 0.075;

/// Grip stiffness (N/m of shear, per side): the side's nominal saturated grip `μ·W/2`
/// developed over one shear modulus. Declared from vehicle weight + terrain — never tuned.
pub fn grip_stiffness(mu: f32, weight_n: f32) -> f32 {
    mu * weight_n / 2.0 / GRIP_SHEAR_MODULUS_M
}

/// Bristle damping ratio ζ: the elastic-zone bristle is an undamped integrator in the hull's
/// resonance loop — without its damping partner a large disturbance (spawn drop, explosion
/// kick) can capture a saturated oscillation attractor the clipped kinetic term cannot damp
/// (measured: per-tick load flips on slope parks; the literature's textbook pairing —
/// TMeasy `F = c·x + d·ẋ`, Pacejka's Besselink term). σ1 = 2ζ√(K·m_side) reduces to the
/// closed form `2ζ·K·√(shear/(μ·g))` via the stiffness derivation — no new vehicle datum.
const GRIP_DAMPING_RATIO: f32 = 0.15;

/// Element-regime bristle damping ratio: LuGre sizes the bristle damper σ1 = 2ζ√(σ0·m)
/// with ζ = 0.5 (Canudas de Wit et al., "A New Model for Control of Systems with
/// Friction", IEEE TAC 1995 — the canonical bristle-mode damping choice, NOT a measured
/// rubber loss factor). Sized against the 64 Hz explicit bound: 143.5 kN·s/m per side is
/// 47% below the ~270 kN·s/m regularizer slope that measurably rang the contact modes
/// (ADR-0026 §3), with an 11.7× Jury margin on the measured 1.6 Hz post-stop yaw mode —
/// the wobble this constant exists to close (one ≤0.03 rad/s rebound instead of a ζ=0.15
/// ring; the Dupont elastic zone is deliberately lossless there, and lowering it was
/// quantitatively dead: ζ_eq ≈ 0.002 at the observed amplitude, 79× short).
///
/// The AGGREGATE regime keeps [`GRIP_DAMPING_RATIO`] = 0.15: ζ = 0.5 was tried there and
/// made the limit cycle WORSE — its load-weighted mean slip carries support-damper
/// transients the per-element geometric slip does not.
const GRIP_ELEMENT_DAMPING_RATIO: f32 = 0.5;

/// Elastic fraction of the grip budget: below `GRIP_BREAKAWAY·C` the state is a PURE spring
/// (zero plastic flow) — the Dupont elasto-plastic branch that kills presliding drift
/// (plain Dahl ratchets downhill under oscillating loads; our at-rest suspension limit
/// cycle + MG recoil are exactly that). α blends smoothly to full Dahl flow at the cap.
const GRIP_BREAKAWAY: f32 = 0.5;

/// Element membership bounds, as fractions of the side's nominal weight: a pad ENTERS the
/// grip population above the high bound and STAYS down to the low bound (hysteresis).
/// Membership reads the ELASTIC load only, never the damped load's sign — the damped load
/// carries the support damper's separation transients, and keying reset to it erased a
/// parked tank's strain on one noisy tick (netcode review defect 1: unsafe for parking).
/// The bounds are tiny against a real bearing column (~0.1% / 0.025% of side weight vs the
/// several percent a grounded column carries) — they exclude numeric dust, not load.
///
/// LOW-LOAD POLICY (netcode review defect 3): strain INTEGRATES only while the element is a
/// member — accumulation shares exactly the membership hysteresis, one regime, one set of
/// bounds. A barely-touching pad never clears the enter bound, so it accumulates nothing
/// and later real load finds no stored-up strain to fire as a sudden force; a member that
/// fades into the leave..enter band holds (not grows) its strain. Force always scales by
/// the CURRENT elastic load, so engagement fades continuously in every branch.
const GRIP_ELEMENT_ENTER_FRACTION: f32 = 1e-3;
const GRIP_ELEMENT_LEAVE_FRACTION: f32 = 2.5e-4;

/// Contact-loss dwell (fixed 64 Hz ticks): a pad whose elastic load falls below the leave
/// bound keeps its stored shear this long before departure is definitive and it forgets —
/// its shear was relieved by ground it no longer touches. 125 ms rides out washboard
/// corner-contact flicker and separation ticks; it is far shorter than a material link's
/// return to the contact patch under drive (one belt loop), and a parked belt never
/// advects, so stale world-space strain is never re-presented to different ground. Chosen
/// over a geometric departure test: one u8 per element, deterministic, and departure
/// geometry is already implied by the load evidence the dwell counts down on.
const GRIP_ELEMENT_LOSS_DWELL_TICKS: u8 = 8;

/// The canonical belt-phase decomposition: travel → (integer wrap count, residual station
/// offset in `[0, pitch)`). ONE function feeds BOTH the resample offset and material
/// identity — computing them separately (f32 residual here, f64 floor there) let rounding
/// disagree for one tick at exact pitch multiples, mis-keying element state by a whole link
/// (netcode review defect 2). The residual comes from the exact f64 remainder; the wrap
/// count is derived FROM that residual (never from an independently rounded division), and
/// the carry branch absorbs the one remaining seam: the f32 cast of a residual just under
/// `pitch` can round up to exactly `pitch`, which `resample`'s internal `rem_euclid` would
/// silently treat as offset 0 — the wrap count must advance with it.
///
/// CONTRACT: `pitch` is a finite positive link pitch and the wrap count is representable in
/// `i64`. A degenerate pitch (denormal/zero) would send `phase/pitch` past `i64::MAX` and the
/// cast saturates rather than wrapping cleanly — the force path can't reach this (a degenerate
/// pitch exits before material identity), but the guard documents the assumption for other
/// callers.
pub fn phase_decompose(phase: f64, pitch: f32) -> (i64, f32) {
    debug_assert!(
        pitch > 1e-6,
        "phase_decompose needs a finite positive pitch"
    );
    let p = f64::from(pitch);
    let r = phase.rem_euclid(p);
    let mut wraps = ((phase - r) / p).round() as i64;
    let mut offset = r as f32;
    if offset >= pitch {
        wraps += 1;
        offset = 0.0;
    }
    (wraps, offset)
}

/// `1 − smoothstep` on [0, 1] — the belt-hold blend factor.
fn hold_blend(x: f32) -> f32 {
    let t = x.clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

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
    /// Static-grip bristle stiffness per side (N/m of shear) — [`grip_stiffness`]. `0.0`
    /// disables the whole static regime (grip state AND belt-hold): the law is then
    /// bit-identical to the kinetic-only law — the parity switch the gates rely on.
    pub grip_stiffness: f32,
}

/// PROTOTYPE (sandbox A/B, not wired into the game): per-element isotropic shear state — the
/// Wong & Chiang resultant-j / Janosi–Hanamoto form proper. One accumulated shear vector per
/// material link × lateral column (flat index `link * 3 + column`), WORLD space, meters.
/// Each grounded element resists relative ground motion in ANY direction (no friction
/// ellipse, no `lateral_ratio`); turning resistance emerges from the footprint geometry —
/// fore and aft elements strain in opposing directions under yaw, which is exactly the
/// rotational stick the per-side aggregate resultant cannot represent (its load-weighted
/// mean slip cancels antisymmetric lateral slip — the "pivots on ice" defect).
/// State rides MATERIAL identity (advects with belt phase, same mapping as the witness
/// link). A pad's memory survives as long as the pad is plausibly on the ground: membership
/// is keyed to ELASTIC load with enter/leave hysteresis plus a short loss dwell
/// ([`GRIP_ELEMENT_LOSS_DWELL_TICKS`]), and only definitive departure (dwell expired —
/// cycled to the return run, or lifted off) forgets — memory expires within one
/// contact-patch dwell while driving; a parked tank holds it through contact flicker.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GripElements {
    /// The pads' accumulated shear against the ground (bristle deflection, world-space
    /// meters), one per material link × lateral column, flat index `link * 3 + column`.
    pub strain: Vec<Vec3>,
    /// Ticks of contact-loss grace left per element before its shear is forgotten. A
    /// parallel `Vec<u8>` rather than a widened strain entry: 1 byte/element with no
    /// padding (a `(Vec3, u8)` entry rounds up to 16 bytes) and the strain slab stays
    /// densely packed — this state is headed for replication/rollback, every byte counts.
    pub dwell: Vec<u8>,
}

impl GripElements {
    /// Both slabs pre-sized for `link_count` material links × 3 lateral columns, at rest
    /// (zero strain, zero dwell) — the ONLY legal construction for a caller entering the
    /// element regime. `step_side` no longer resizes at runtime (the REV-14 fixed-size
    /// invariant, element-promotion-checklist.md addendum): wrong-length slabs skip the
    /// regime instead of being silently rebuilt.
    pub fn for_links(link_count: usize) -> Self {
        let n = link_count * 3;
        Self {
            strain: vec![Vec3::ZERO; n],
            dwell: vec![0; n],
        }
    }
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
    /// The side's elastic grip resultant (N), circularized: `x` longitudinal, `y` is the
    /// LATERAL force divided by `lateral_ratio` (so both axes share one budget `C = Σ μ·load`).
    /// A generalized force, NOT a world anchor — distributed through the tick's contacts in
    /// load proportion. Zero ≡ today's kinetic-only law, bit-for-bit.
    pub grip: Vec2,
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

/// One contact's telemetry (viz / traces). `load` is the ACTUAL damped load that scaled the
/// friction ellipse this tick — `load_elastic` is the spring-only component (static-sink
/// analysis); inferring ellipse utilization from the elastic value understates grip under
/// dynamic compression (codex steer review: the telemetry trap).
pub struct BeltContact {
    pub point: Vec3,
    pub load: f32,
    pub load_elastic: f32,
    /// Longitudinal slip (belt speed − ground-point speed, m/s).
    pub slip: f32,
    /// Lateral scrub speed at the contact (m/s).
    pub slip_lat: f32,
    /// Scalar friction components along the contact's longitudinal / lateral axes (N).
    pub f_long: f32,
    pub f_lat: f32,
    pub normal: Vec3,
    pub traction: Vec3,
}

/// What one side's tick produced.
#[derive(Default)]
pub struct SideReport {
    pub state: SideState,
    pub apps: Vec<ForceApp>,
    pub contacts: Vec<BeltContact>,
    /// The engine force actually applied to the belt this tick (post-governor, post-clamp, N).
    pub engine_force: f32,
    /// Ground reaction summed into belt dynamics (Σ f_long, N).
    pub belt_reaction: f32,
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
/// `elements`: `Some` runs the per-element isotropic shear regime (see [`GripElements`])
/// instead of the per-side aggregate — the caller owns the state vector and must construct
/// it pre-sized to `count * 3` ([`GripElements::for_links`]; wrong-length slabs skip the
/// regime — see the invariant below). `None` = the shipped aggregate law (the game's MP
/// compositions).
pub fn step_side<O: TerrainOracle>(
    input: &SideInput,
    state: SideState,
    affine: Affine3A,
    dt: f32,
    params: &ForceParams,
    oracle: &O,
    vel_at: impl Fn(Vec3) -> Vec3,
    elements: Option<&mut GripElements>,
) -> SideReport {
    let mut report = SideReport {
        state,
        ..Default::default()
    };
    let belt_speed = state.speed;
    let mut belt_reaction = 0.0;

    let pitch = polyline_len(input.loop_pts) / input.count.max(1) as f32;
    let (wraps, station_offset) = phase_decompose(state.phase, pitch);
    let mut stations = resample(input.loop_pts, pitch, station_offset);
    stations.truncate(input.count);
    let n = stations.len();
    if n < 3 {
        return report;
    }

    let k = params.grip_stiffness;
    // INVARIANT (REV-14 blocker, element-promotion-checklist.md addendum): the caller owns
    // FIXED-SIZE slabs, constructed `count * 3` at spawn ([`GripElements::for_links`]). The
    // branch that used to live here cleared and re-sized mismatched slabs at runtime — which
    // erases valid strain exactly when it matters most: a predicted root that runs one
    // driving tick before its authoritative JIP seed arrives records a zeroed field into
    // local prediction history and replay follows it; a snapshot with one mismatched slab
    // wipes real state instead of surfacing the violation. Wrong-length slabs are therefore
    // a caller defect: assert loudly in debug, and in release SKIP the element regime for
    // this tick (the aggregate law runs instead) WITHOUT touching the stored state.
    let element_len = input.count * 3;
    let elements = elements.filter(|e| {
        let sized = e.strain.len() == element_len && e.dwell.len() == element_len;
        debug_assert!(
            sized,
            "GripElements slabs must be pre-sized to count*3 = {element_len} at construction \
             (strain {}, dwell {}) — runtime resizing erases rollback-visible strain",
            e.strain.len(),
            e.dwell.len(),
        );
        sized
    });
    let elements_mode = elements.is_some() && k > 0.0;
    // Membership evidence pass 1 cannot put in `cols`: columns whose damped support load
    // clipped to zero while elastic penetration remains — the pad is still ON the ground
    // (a separating/noisy tick), and element strain must survive it. Station order —
    // deterministic, fixed bound (`count * 3`).
    let mut held: Vec<(usize, f32)> = Vec::new();

    // PASS 1 — geometry, support, and slip per contact column. Forces are NOT emitted yet:
    // the elastic grip resultant needs this tick's total budget and load-weighted slip
    // BEFORE per-contact traction can include it. Pass 2 emits apps in the exact
    // support-then-traction per-contact order the one-pass law used — application order is
    // the bit-reproducibility contract, and it is unchanged.
    struct ColumnContact {
        p: Vec3,
        normal: Vec3,
        load: f32,
        load_elastic: f32,
        long_dir: Vec3,
        lat_dir: Vec3,
        has_plane: bool,
        slip_long: f32,
        slip_lat: f32,
        /// Flat material-element index (`link * 3 + column`) — the per-element grip key.
        element: usize,
    }
    let mut cols: Vec<ColumnContact> = Vec::with_capacity(n);

    // Material identity under advection: station `i` samples arc `offset + i·pitch`, so the
    // material link there is `i − wraps` (mod count) — when the sampling offset wraps, the
    // identity shift absorbs the jump (the witness-link mapping). `wraps` and the offset come
    // from the SAME [`phase_decompose`] call — the pair is consistent by construction.
    let material =
        |i: usize| -> usize { (i as i64 - wraps).rem_euclid(input.count as i64) as usize };

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
        for (ci, &(offset, weight)) in params.columns.iter().enumerate() {
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

            // Support: penalty spring along the belt's own inward normal, at the column's
            // share of the per-metre coefficients.
            let normal = -out;
            let vel = vel_at(p);
            let engage = (pen_max / params.engage_depth).clamp(0.0, 1.0);
            let load_elastic = weight * params.support_stiffness_per_m * area * engage;
            let load = weight
                * (params.support_stiffness_per_m * area
                    - params.support_damping_per_m * contact_len * vel.dot(normal))
                .max(0.0)
                * engage;
            if load <= 0.0 {
                if elements_mode && load_elastic > 0.0 {
                    held.push((material(i) * 3 + ci, load_elastic));
                }
                continue;
            }

            let drive = -affine.transform_vector3(Vec3::new(0.0, tan2.y, tan2.x));
            let long_plane = drive - drive.dot(normal) * normal;
            let has_plane = long_plane.length() > 1e-4;
            let (long_dir, lat_dir, slip_long, slip_lat) = if has_plane {
                let long_dir = long_plane.normalize();
                let lat_dir = normal.cross(long_dir).normalize_or_zero();
                (
                    long_dir,
                    lat_dir,
                    belt_speed - vel.dot(long_dir),
                    vel.dot(lat_dir),
                )
            } else {
                (Vec3::ZERO, Vec3::ZERO, 0.0, 0.0)
            };

            cols.push(ColumnContact {
                p,
                normal,
                load,
                load_elastic,
                long_dir,
                lat_dir,
                has_plane,
                slip_long,
                slip_lat,
                element: material(i) * 3 + ci,
            });
        }
    }

    // The elasto-plastic grip update (static-friction-design.md §3). Budget C = Σ μ·load —
    // the ELASTIC load: the damped actual load carries the support damper's tick-scale
    // transients, and feeding those into an integrating state amplified a marginal mm-scale
    // Nyquist wobble into a full force limit cycle (measured: ±90 kN damped-load alternation
    // over a ±11 kN elastic wobble, hull perfectly smooth — the support damper converts
    // wobble rate into load asymmetry, grip fed it back). The Coulomb budget follows the
    // sustained weight-bearing force; the kinetic regularizer keeps damped load, as shipped.
    // Slip resultant in FORCE sign convention (x: +slip_long drives +long force; y:
    // −slip_lat drives +lat force — matching the kinetic law's signs).
    let budget: f32 = cols
        .iter()
        .filter(|c| c.has_plane)
        .map(|c| params.mu * c.load_elastic)
        .sum();

    // PROTOTYPE: the per-element isotropic shear regime (see [`GripElements`]). Each grounded
    // element integrates its own world-space shear vector at the SAME shear modulus, breakaway
    // branch, and damping ratio as the aggregate law (per-element stiffness μ·load/K sums to
    // exactly the aggregate side stiffness — same coupled-mode class the 75 mm modulus was
    // validated against, plus the yaw/pitch bristle modes the aggregate never had). Force is
    // the strain direction itself — isotropic, NO ellipse: lateral-vs-longitudinal asymmetry
    // and turning resistance are left to emerge from footprint geometry.
    let mut elem_g: Vec<Vec2> = Vec::new();
    if elements_mode {
        let elems = elements.unwrap();
        // Membership bounds from the side's nominal weight, recovered from the declared
        // stiffness (`grip_stiffness = μ·W/2/K`) — no new vehicle datum.
        let side_weight = k * GRIP_SHEAR_MODULUS_M / params.mu;
        let enter = side_weight * GRIP_ELEMENT_ENTER_FRACTION;
        let leave = side_weight * GRIP_ELEMENT_LEAVE_FRACTION;
        let mut kept = vec![false; element_len];
        elem_g = vec![Vec2::ZERO; cols.len()];
        // The damping partner, normalized per unit budget: same closed form as the
        // aggregate's `grip_damp / C` (σ1 = 2ζ√(K·m) reduced through the stiffness
        // derivation), at the element regime's LuGre-canonical ζ.
        let d_coef =
            2.0 * GRIP_ELEMENT_DAMPING_RATIO / (GRIP_SHEAR_MODULUS_M * params.mu * 9.81).sqrt();
        for (idx, c) in cols.iter().enumerate() {
            // ENTER above the high bound; STAY down to the low bound while already a member
            // (hysteresis + dwell refresh). Never the damped load's sign — see
            // [`GRIP_ELEMENT_ENTER_FRACTION`].
            let member =
                c.load_elastic >= enter || (c.load_elastic >= leave && elems.dwell[c.element] > 0);
            if member {
                elems.dwell[c.element] = GRIP_ELEMENT_LOSS_DWELL_TICKS;
                kept[c.element] = true;
            }
            if !c.has_plane || c.load_elastic <= 0.0 {
                continue;
            }
            // World-space shear rate in FORCE convention: the direction friction pushes the
            // hull (+slip_long along long_dir, −slip_lat along lat_dir — the kinetic law's
            // signs, vectorized).
            let sdot = c.long_dir * c.slip_long - c.lat_dir * c.slip_lat;
            let j1 = if c.load_elastic >= enter {
                let speed = sdot.length();
                let j0 = elems.strain[c.element];
                // Dupont elasto-plastic α, per element: pure spring below breakaway or when
                // slip unloads it; smoothstep to full Dahl flow at one shear modulus of
                // strain.
                let alpha = if j0.dot(sdot) < 0.0 {
                    0.0
                } else {
                    let m = ((j0.length() / GRIP_SHEAR_MODULUS_M - GRIP_BREAKAWAY)
                        / (1.0 - GRIP_BREAKAWAY))
                        .clamp(0.0, 1.0);
                    m * m * (3.0 - 2.0 * m)
                };
                let mut j1 = (j0 + sdot * dt) / (1.0 + alpha * (dt / GRIP_SHEAR_MODULUS_M) * speed);
                // Keep the strain in the contact tangent plane (terrain curvature rotates it
                // out), and cap at one shear modulus (the Dahl saturation the rational update
                // converges to; the projection is the α < 1 safety net, as in the aggregate).
                j1 -= j1.dot(c.normal) * c.normal;
                let j_len = j1.length();
                if j_len > GRIP_SHEAR_MODULUS_M {
                    j1 *= GRIP_SHEAR_MODULUS_M / j_len;
                }
                elems.strain[c.element] = j1;
                j1
            } else {
                // Below the enter bound (hysteresis band, or a fading straggler): stored
                // strain HELD, not integrated — the low-load policy (see
                // [`GRIP_ELEMENT_ENTER_FRACTION`]); force still scales by the current
                // elastic load, so engagement fades continuously.
                elems.strain[c.element]
            };
            let mut g = j1 / GRIP_SHEAR_MODULUS_M + sdot * d_coef;
            let g_len = g.length();
            if g_len > 1.0 {
                g /= g_len;
            }
            elem_g[idx] = Vec2::new(g.dot(c.long_dir), g.dot(c.lat_dir));
        }
        // Columns the damped support clipped out this tick but whose elastic penetration
        // remains: still on the ground — membership continues, stored strain holds, no force
        // (support omitted the contact entirely).
        for &(el, e) in &held {
            if e >= enter || (e >= leave && elems.dwell[el] > 0) {
                elems.dwell[el] = GRIP_ELEMENT_LOSS_DWELL_TICKS;
                kept[el] = true;
            }
        }
        // Definitive departure only: the dwell rides out contact flicker; when it expires
        // the pad has truly left the ground (cycled to the return run, or lifted off) and
        // forgets — its shear was relieved by ground it no longer touches.
        for ((&kept, dwell), strain) in kept
            .iter()
            .zip(elems.dwell.iter_mut())
            .zip(elems.strain.iter_mut())
        {
            if !kept {
                let d = dwell.saturating_sub(1);
                *dwell = d;
                if d == 0 {
                    *strain = Vec3::ZERO;
                }
            }
        }
    }

    let mut grip_damp = Vec2::ZERO;
    let grip_next = if !elements_mode && k > 0.0 && budget > 0.0 {
        let mut s_bar = Vec2::ZERO;
        for c in cols.iter().filter(|c| c.has_plane) {
            s_bar += params.mu * c.load_elastic * Vec2::new(c.slip_long, -c.slip_lat);
        }
        s_bar /= budget;
        // Transport old memory into the current budget (a shrinking footprint clips force
        // continuously; contact loss → zero — no reset regime).
        let q_len = state.grip.length();
        let q0 = if q_len > budget {
            state.grip * (budget / q_len)
        } else {
            state.grip
        };
        // Dupont α: pure elastic below the breakaway fraction, and whenever slip unloads
        // the spring (q·s̄ < 0); smoothstep to full Dahl flow at the cap. This is the
        // drift-free branch — plain Dahl (α ≡ 1) ratchets under oscillating loads.
        let alpha = if q0.dot(s_bar) < 0.0 {
            0.0
        } else {
            let m =
                ((q0.length() / budget - GRIP_BREAKAWAY) / (1.0 - GRIP_BREAKAWAY)).clamp(0.0, 1.0);
            m * m * (3.0 - 2.0 * m)
        };
        // The damping partner (see GRIP_DAMPING_RATIO): a per-side viscous term on the
        // load-weighted slip, distributed and ellipse-capped exactly like q.
        grip_damp = 2.0
            * GRIP_DAMPING_RATIO
            * k
            * (GRIP_SHEAR_MODULUS_M / (params.mu * 9.81)).sqrt()
            * s_bar;
        // Backward-Euler rational form of q̇ = K·s̄ − α·(K/C)·|s̄|·q — dissipative and
        // self-limiting; the final projection is a safety net for the α < 1 band.
        let q1 = (q0 + k * dt * s_bar) / (1.0 + alpha * (k * dt / budget) * s_bar.length());
        let q1_len = q1.length();
        if q1_len > budget {
            q1 * (budget / q1_len)
        } else {
            q1
        }
    } else {
        Vec2::ZERO
    };

    // PASS 2 — emit forces in the original per-contact order: support, then traction.
    let mut elem_sum = Vec2::ZERO;
    for (idx, c) in cols.iter().enumerate() {
        report.apps.push(ForceApp {
            force: c.normal * c.load,
            point: c.p,
        });

        let mut f_long = 0.0;
        let mut f_lat = 0.0;
        let mut traction = Vec3::ZERO;
        if c.has_plane {
            let grip = params.mu * c.load;
            let grip_lat = grip * params.lateral_ratio;
            if elements_mode {
                // The per-element regime: each element's own capped strain+damping direction,
                // scaled by ITS elastic load — isotropic, full μ in every direction.
                let grip_el = params.mu * c.load_elastic;
                let g = elem_g[idx];
                f_long = grip_el * g.x;
                f_lat = grip_el * g.y;
                elem_sum += Vec2::new(f_long, f_lat);
            } else if grip_next == Vec2::ZERO {
                // The kinetic-only law, verbatim (the parity branch: with grip disabled or
                // no stored force, these are the exact shipped expressions — bit-identical).
                f_long = grip * (c.slip_long / params.slip_saturation).clamp(-1.0, 1.0);
                f_lat = -grip_lat * (c.slip_lat / params.slip_saturation).clamp(-1.0, 1.0);
                let e = (f_long / grip).powi(2) + (f_lat / grip_lat).powi(2);
                if e > 1.0 {
                    let s = e.sqrt().recip();
                    f_long *= s;
                    f_lat *= s;
                }
            } else {
                // The grip regime: force comes from the STRAIN STATE (plus its small viscous
                // partner), distributed in elastic-load proportion and ellipse-capped per
                // contact — Janosi–Hanamoto proper. The kinetic regularizer is deliberately
                // ABSENT here: near zero slip its slope is μN/slip_saturation — a ~270 kN·s/m
                // explicit damper per side that sits at the 64 Hz stability margin and rang
                // the contact modes (the old sim never noticed: creep kept slip saturated,
                // dF/dv = 0). Under sustained slide the Dahl state saturates to C·ŝ — exactly
                // the kinetic law's saturated ellipse — so steady sliding behavior converges;
                // what changes is a physical relaxation lag (~C/K of slip distance) in force
                // DIRECTION during fast slides, and fuller sub-saturation traction.
                let grip_el = params.mu * c.load_elastic;
                let grip_el_lat = grip_el * params.lateral_ratio;
                let mut gx = (grip_next.x + grip_damp.x) / budget;
                let mut gy = (grip_next.y + grip_damp.y) / budget;
                let e = gx * gx + gy * gy;
                if e > 1.0 {
                    let s = e.sqrt().recip();
                    gx *= s;
                    gy *= s;
                }
                f_long = grip_el * gx;
                f_lat = grip_el_lat * gy;
            }
            traction = c.long_dir * f_long + c.lat_dir * f_lat;
            report.apps.push(ForceApp {
                force: traction,
                point: c.p,
            });
            belt_reaction += f_long;
        }

        report.contacts.push(BeltContact {
            point: c.p,
            load: c.load,
            load_elastic: c.load_elastic,
            slip: c.slip_long,
            slip_lat: c.slip_lat,
            f_long,
            f_lat,
            normal: c.normal,
            traction,
        });
    }

    // Belt dynamics + advection: governor toward the command under the constant-power curve,
    // ground reaction, reflected inertia; phase advects at the PRE-update speed. The HOLD
    // blend h lets the locked drivetrain bear the ground reaction at zero command + zero
    // belt speed (h→1) instead of being back-driven through finite governor gain — the
    // measured dominant longitudinal parking leak. During motion h→0: unchanged dynamics.
    // Legitimate force balance: the belt's 1-D coordinate is fully known here. A future
    // neutral/clutch or brake-damage mechanic weakens this term explicitly.
    let target = input.command * params.max_speed;
    let avail = engine_available(params, belt_speed);
    let engine = (params.governor_gain * (target - belt_speed)).clamp(-avail, avail);
    let hold = if k > 0.0 {
        hold_blend(target.abs() / params.slip_saturation)
            * hold_blend(belt_speed.abs() / params.slip_saturation)
    } else {
        0.0
    };
    let next = belt_speed + (engine - (1.0 - hold) * belt_reaction) / params.inertia * dt;
    report.state.speed = next.clamp(-params.max_speed, params.max_speed);
    report.state.phase = state.phase + f64::from(belt_speed * dt);
    // In the element regime the aggregate slot carries the summed element force (long, lat) —
    // telemetry only (the element state is authoritative and lives with the caller).
    report.state.grip = if elements_mode { elem_sum } else { grip_next };
    report.engine_force = engine;
    report.belt_reaction = belt_reaction;
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flat horizontal ground: terrain fills `y < surface_y`. Direction-agnostic on purpose
    /// — only the loop's bottom run ever probes into it in the rig below.
    struct FlatGround {
        surface_y: f32,
    }
    impl TerrainOracle for FlatGround {
        fn depth_along(&self, station: Vec3, _out: Vec3, reach: f32) -> f32 {
            (self.surface_y - station.y).min(reach)
        }
    }

    /// A closed rectangular pin-line loop in the side plane (z, y), bottom run traversed +z
    /// so its outward normal points DOWN (`out2 = (tan.y, −tan.x)`). `bottom_y` positions the
    /// pin line; the contact face sits `thickness/2` further out.
    fn rect_loop(bottom_y: f32) -> Vec<Vec2> {
        vec![
            Vec2::new(-1.0, bottom_y),
            Vec2::new(1.0, bottom_y),
            Vec2::new(1.0, bottom_y + 0.5),
            Vec2::new(-1.0, bottom_y + 0.5),
            Vec2::new(-1.0, bottom_y),
        ]
    }

    /// A small parked test vehicle for the element regime: perimeter 5 m, 25 links (pitch
    /// 0.2 m), 5 mm of bottom-face penetration at `bottom_y = 0.02`. The support damping is
    /// sized so a 2 m/s separation velocity clips every damped load to zero while elastic
    /// penetration remains — the exact one-tick dropout of netcode review defect 1.
    fn rig_params() -> ForceParams {
        ForceParams {
            thickness: 0.05,
            columns: [(-0.1, 0.25), (0.0, 0.5), (0.1, 0.25)],
            support_stiffness_per_m: 2.0e6,
            support_damping_per_m: 1.0e5,
            engage_depth: 0.002,
            probe_reach: 0.5,
            mu: 0.9,
            lateral_ratio: 0.5,
            slip_saturation: 0.5,
            max_speed: 10.0,
            engine_power: 1.0e5,
            engine_force: 1.0e5,
            governor_gain: 1.0e4,
            inertia: 500.0,
            grip_stiffness: grip_stiffness(0.9, 30_000.0 * 9.81),
        }
    }

    fn rig_tick(
        loop_pts: &[Vec2],
        state: &mut SideState,
        elems: &mut GripElements,
        surface_y: f32,
        vel: Vec3,
    ) -> SideReport {
        let input = SideInput {
            loop_pts,
            count: 25,
            plane_x: 0.0,
            command: 0.0,
        };
        let report = step_side(
            &input,
            *state,
            Affine3A::IDENTITY,
            1.0 / 64.0,
            &rig_params(),
            &FlatGround { surface_y },
            |_| vel,
            Some(elems),
        );
        *state = report.state;
        report
    }

    fn strain_sum(elems: &GripElements) -> f32 {
        elems.strain.iter().map(|j| j.length()).sum()
    }

    /// Netcode review defect 1: one separating tick (damped load clipped to zero, elastic
    /// penetration intact) must NOT erase parked strain — the pre-fix law reset every
    /// element its damped-load contacts missed, which is unsafe for parking.
    #[test]
    fn element_strain_survives_damped_load_dropout() {
        let loop_pts = rect_loop(0.02);
        let mut state = SideState::default();
        let mut elems = GripElements::for_links(25);
        // Build presliding strain: hull creeping +z against a stationary belt.
        let mut report = rig_tick(&loop_pts, &mut state, &mut elems, 0.0, Vec3::ZERO);
        for _ in 0..20 {
            report = rig_tick(
                &loop_pts,
                &mut state,
                &mut elems,
                0.0,
                Vec3::new(0.0, 0.0, 0.05),
            );
        }
        let enter =
            rig_params().grip_stiffness * GRIP_SHEAR_MODULUS_M / 0.9 * GRIP_ELEMENT_ENTER_FRACTION;
        assert!(
            report.contacts.iter().any(|c| c.load_elastic >= enter),
            "rig premise: grounded columns must clear the enter bound"
        );
        let before = elems.strain.clone();
        assert!(strain_sum(&elems) > 0.01, "rig premise: strain accumulated");
        // The dropout tick: strong separation velocity, every damped load clips to zero.
        let report = rig_tick(
            &loop_pts,
            &mut state,
            &mut elems,
            0.0,
            Vec3::new(0.0, 2.0, 0.0),
        );
        assert!(
            report.contacts.is_empty(),
            "rig premise: the separation tick must clip all damped loads"
        );
        assert_eq!(elems.strain, before, "one noisy tick erased parked strain");
        // Elastic membership holds through ANY number of such ticks — reset needs departure.
        for _ in 0..(4 * GRIP_ELEMENT_LOSS_DWELL_TICKS as usize) {
            rig_tick(
                &loop_pts,
                &mut state,
                &mut elems,
                0.0,
                Vec3::new(0.0, 2.0, 0.0),
            );
        }
        assert_eq!(
            elems.strain, before,
            "sustained damped-load noise erased strain"
        );
    }

    /// The counterpart: definitive departure (no elastic contact at all) forgets, exactly one
    /// loss dwell after the last contact evidence.
    #[test]
    fn element_strain_forgets_on_definitive_departure() {
        let loop_pts = rect_loop(0.02);
        let mut state = SideState::default();
        let mut elems = GripElements::for_links(25);
        for _ in 0..20 {
            rig_tick(
                &loop_pts,
                &mut state,
                &mut elems,
                0.0,
                Vec3::new(0.0, 0.0, 0.05),
            );
        }
        assert!(strain_sum(&elems) > 0.01, "rig premise: strain accumulated");
        // Ground removed: the pad has left contact for good.
        for _ in 0..(GRIP_ELEMENT_LOSS_DWELL_TICKS as usize - 1) {
            rig_tick(&loop_pts, &mut state, &mut elems, -10.0, Vec3::ZERO);
        }
        assert!(
            strain_sum(&elems) > 0.01,
            "strain forgotten before the loss dwell expired"
        );
        rig_tick(&loop_pts, &mut state, &mut elems, -10.0, Vec3::ZERO);
        assert_eq!(strain_sum(&elems), 0.0, "departed strain must be forgotten");
        assert!(elems.dwell.iter().all(|&d| d == 0));
    }

    /// Netcode review defect 3 (hysteresis band): an EXISTING member whose elastic load fades
    /// into [leave, enter) — dwell still live, slip continuing — must HOLD its accumulated
    /// strain, not keep integrating it. Integration is gated on `load_elastic >= enter`, NOT on
    /// membership: writing the branch `if member` would let strain GROW in the band (the pad is
    /// a member there) while the parity/departure tests stayed green. This pins that the branch
    /// is load-keyed.
    #[test]
    fn element_member_fading_into_band_holds_strain() {
        let params = rig_params();
        let side_weight = params.grip_stiffness * GRIP_SHEAR_MODULUS_M / params.mu;
        let enter = side_weight * GRIP_ELEMENT_ENTER_FRACTION;
        let leave = side_weight * GRIP_ELEMENT_LEAVE_FRACTION;

        // Phase 1 — deep contact (loads ≫ enter), slipping: accumulate real presliding strain.
        let deep = rect_loop(0.02);
        let mut state = SideState::default();
        let mut elems = GripElements::for_links(25);
        let mut report = rig_tick(&deep, &mut state, &mut elems, 0.0, Vec3::ZERO);
        for _ in 0..20 {
            report = rig_tick(
                &deep,
                &mut state,
                &mut elems,
                0.0,
                Vec3::new(0.0, 0.0, 0.05),
            );
        }
        assert!(
            report.contacts.iter().any(|c| c.load_elastic >= enter),
            "rig premise: phase-1 contacts clear the enter bound"
        );
        assert!(strain_sum(&elems) > 0.01, "rig premise: strain accumulated");

        // Phase 2 — shallow contact (loads ≈ 100 N, mid-band [leave, enter)), same 25-link count
        // so element identity + strain slab persist. The pad is fading but still a live member
        // (dwell refreshed each tick); slip continues, so an `if member` integration branch WOULD
        // move the strain. It must hold instead.
        let band = rect_loop(0.024);
        let held = elems.strain.clone();
        for _ in 0..(2 * GRIP_ELEMENT_LOSS_DWELL_TICKS as usize) {
            let report = rig_tick(
                &band,
                &mut state,
                &mut elems,
                0.0,
                Vec3::new(0.0, 0.0, 0.05),
            );
            let loaded: Vec<f32> = report
                .contacts
                .iter()
                .map(|c| c.load_elastic)
                .filter(|&l| l > 0.0)
                .collect();
            assert!(
                !loaded.is_empty() && loaded.iter().all(|&l| (leave..enter).contains(&l)),
                "rig premise: fading contacts must sit in [leave, enter); got {loaded:?}"
            );
        }
        assert!(
            elems.dwell.iter().any(|&d| d > 0),
            "rig premise: the fading pad stays a live member (dwell refreshed)"
        );
        assert_eq!(
            elems.strain, held,
            "strain integrated in the hysteresis band — the branch must gate on load_elastic >= enter, not membership"
        );
    }

    /// Netcode review defect 3: an element that never clears the enter bound must not
    /// integrate strain — a barely-touching pad accumulating full strain would fire a sudden
    /// force the moment real load arrives.
    #[test]
    fn element_below_enter_bound_never_integrates() {
        // 0.05 mm of face penetration: real contact, but orders below the enter bound.
        let loop_pts = rect_loop(0.024_95);
        let mut state = SideState::default();
        let mut elems = GripElements::for_links(25);
        let leave =
            rig_params().grip_stiffness * GRIP_SHEAR_MODULUS_M / 0.9 * GRIP_ELEMENT_LEAVE_FRACTION;
        for _ in 0..50 {
            let report = rig_tick(
                &loop_pts,
                &mut state,
                &mut elems,
                0.0,
                Vec3::new(0.0, 0.0, 0.05),
            );
            assert!(
                !report.contacts.is_empty()
                    && report.contacts.iter().all(|c| c.load_elastic < leave),
                "rig premise: contacts exist and sit below the leave bound"
            );
        }
        assert!(elems.strain.iter().all(|&j| j == Vec3::ZERO));
        assert!(elems.dwell.iter().all(|&d| d == 0));
    }

    /// The decomposition contract: offset strictly in `[0, pitch)` — never `pitch` itself,
    /// which `resample`'s internal `rem_euclid` would alias to 0 while material identity
    /// stayed a link behind — and the pair reconstructs the phase, so the two consumers
    /// cannot disagree.
    fn assert_canonical(phase: f64, pitch: f32) {
        let (wraps, offset) = phase_decompose(phase, pitch);
        assert!(
            (0.0..pitch).contains(&offset),
            "offset {offset} outside [0, {pitch}) at phase {phase}"
        );
        let rebuilt = wraps as f64 * f64::from(pitch) + f64::from(offset);
        let tol = f64::from(pitch) * 1e-6 + phase.abs() * 1e-9;
        assert!(
            (rebuilt - phase).abs() <= tol,
            "wraps {wraps} + offset {offset} rebuilds {rebuilt}, phase {phase}"
        );
    }

    #[test]
    fn phase_decompose_pitch_boundaries() {
        // Next-representable f64 on both sides of positive AND negative pitch multiples —
        // the exact ticks where the old split (f32 remainder vs f64 floor) could disagree.
        for pitch in [0.152_87_f32, 0.125] {
            let p = f64::from(pitch);
            for k in [-40_i64, -3, -1, 0, 1, 2, 7, 40] {
                let m = k as f64 * p;
                for phase in [m, m.next_down(), m.next_up()] {
                    assert_canonical(phase, pitch);
                }
            }
        }
    }

    #[test]
    fn phase_decompose_long_travel() {
        // ≈10 km of accumulated travel: the precision regime the f64 phase exists for
        // (codex phase-B finding 8) — boundary neighbours must stay canonical out here too.
        let pitch = 0.152_87_f32;
        let p = f64::from(pitch);
        let m = (10_000.0 / p).floor() * p;
        for phase in [
            m,
            m.next_down(),
            m.next_up(),
            m + 0.3 * p,
            -m,
            (-m).next_down(),
            (-m).next_up(),
        ] {
            assert_canonical(phase, pitch);
        }
    }

    #[test]
    fn phase_decompose_carry_advances_wraps() {
        // The carry seam itself: an f64 residual just under one pitch whose f32 cast rounds
        // UP to exactly `pitch`. Without the carry, resample would alias the offset to 0 (a
        // full link ahead) while `wraps` lagged — the one-tick material mis-key.
        let pitch = 0.152_87_f32;
        let p = f64::from(pitch);
        let r = p - 1e-12;
        assert_eq!(r as f32, pitch, "test premise: the cast rounds up");
        let (wraps, offset) = phase_decompose(5.0 * p + r, pitch);
        assert_eq!(offset, 0.0);
        assert_eq!(wraps, 6);
    }
}
