//! The declared transmission — the JOINT two-output drivetrain between contact-force
//! calculation and belt integration (transmission-design.md; phase 2.5 of the element
//! promotion arc). One call computes BOTH sprocket forces `Q_L, Q_R` from the pair of belt
//! speeds, the drive command, and this tick's ground reactions `R_L, R_R`, then integrates
//! `I·v̇ᵢ = Qᵢ − Rᵢ` for both sides simultaneously. Internally it works in the superimposed
//! coordinates `m = (v_L+v_R)/2` (propulsion) and `d = (v_L−v_R)/2` (steering difference);
//! the per-side belt speeds stay the authoritative external state (design §2 — reparameterizing
//! saves nothing and every contact call is per-side).
//!
//! Three adapters behind one mode enum:
//! - [`TransmissionMode::Governor`] — the EXACT legacy math ([`forces::governor_belt`],
//!!   verbatim): every MP composition and every existing baseline runs this. The parity switch.
//! - [`TransmissionMode::Hybrid`] — the arcade-honest continuous regenerative box (design
//!   menu C/D): engine torque curve × gear ratio → propulsion force on `m`; a
//!   capacity-limited steering servo on `d` (continuous curvature command interpolating the
//!   authored radii); power conservation with inner-track recirculation at declared η.
//! - [`TransmissionMode::FixedRadii`] — the Tiger L600: the same machinery, but `d` is
//!   CONSTRAINED to `s·κ(gear, step)·|m|` (two steering detents per gear with hysteresis);
//!   the constraint force λ is solved semi-implicitly and clamped to the steering capacity
//!   (beyond capacity the constraint slips). At `m≈0` the neutral turn is the MARGINAL
//!   brake-gated one the restoration literature describes: a slow capacity-limited
//!   counter-rotation at a declared fraction of the 1st-gear tight ratio.
//!
//! Brakes (design §3, the hold reframe): the ground reaction is ALWAYS applied to the belt
//! (never attenuated); at zero command a per-side capacity-limited static brake supplies
//! `Bᵢ = clamp(Rᵢ − Qᵢ, ±h·B_max)`, with the legacy hold-blend shape reused ONLY as the
//! engagement envelope `h` (zero command + near-zero belt speed → h→1, so a parked tank on a
//! slope inside brake capacity holds EXACTLY, and a slope past capacity back-drives the belt
//! honestly). The `Governor` adapter keeps the old hold blend verbatim instead.
//!
//! Pure math, no ECS (like [`forces`]): callers own the state. [`TransmissionState`] is the
//! only path-dependent state (gear, shift countdown, steering detent, direction) — carried as
//! a plain LOCAL component / sandbox resource, NOT replicated, NOT hashed (this is REV 13;
//! only the offline composition and the sandbox ever run the regenerative adapters).

use super::forces::{self, ForceParams};

/// Which drivetrain adapter computes the sprocket forces. Per-vehicle SPEC eventually
/// (`TankSpec.track.powertrain.transmission.architecture` selects between the regenerative
/// adapters); `Governor` is what every composition without an explicit selection runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TransmissionMode {
    /// The legacy per-side symmetric governor + hold blend, bit-for-bit (the parity switch).
    #[default]
    Governor,
    /// `Regenerative { continuous }` — the arcade-honest hybrid (design menu D).
    Hybrid,
    /// `Regenerative { fixed_radii }` — the L600 geared-steering adapter (design menu B).
    FixedRadii,
}

impl TransmissionMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Governor => "governor (legacy parity)",
            Self::Hybrid => "hybrid (continuous regenerative)",
            Self::FixedRadii => "L600 (fixed-radius regenerative)",
        }
    }
}

/// Gear-shift torque interruption (fixed 64 Hz ticks): the OLVAR preselector's shift takes a
/// noticeable fraction of a second with drive uncoupled. 20 ticks ≈ 0.31 s. Declared model
/// policy (INFERRED — no per-vehicle shift-time datum reached; refine with the codex table).
pub const SHIFT_TICKS: u8 = 20;

/// Fuel-governor cut width (rpm): torque ramps linearly to zero over this band past the
/// governed rpm, so the top-speed equilibrium is a smooth root instead of a hard clip.
/// INFERRED numerical policy, not vehicle data.
const GOVERNOR_CUT_RPM: f32 = 100.0;

/// Engine drag torque as a fraction of peak torque — the reflected drag-torque curve stand-in
/// (design §3: engine braking is a drag TORQUE, never the negative half of rated power).
/// INFERRED ~8% of peak (typical diesel motoring torque); the codex data pass may refine it.
const ENGINE_DRAG_FRACTION: f32 = 0.08;

/// Belt-speed span (m/s) over which the engine-drag torque saturates — a viscous-near-zero
/// ramp so drag cannot sign-flip chatter around standstill (the parking brake owns standstill).
const DRAG_SAT_SPEED: f32 = 0.2;

/// Throttle magnitude above which engine drag is fully released (blends out with the
/// hold-blend shape below it): an open throttle is not motoring.
const DRAG_THROTTLE_RELEASE: f32 = 0.5;

/// Steering-servo proportional band (m/s of `d` error at which the servo saturates to the
/// declared steering capacity). Sized against the grip law's `slip_saturation` scale.
/// INFERRED control policy.
const STEER_SERVO_BAND: f32 = 0.25;

/// |m| below which a commanded direction reversal actually swaps the F/R ladder (above it the
/// opposing gear force acts as driveline braking first — you cannot slam reverse at speed).
const DIRECTION_SWAP_SPEED: f32 = 0.5;

/// L600 neutral-turn entry: |throttle| below this AND |m| below [`NEUTRAL_M_SPEED`] puts the
/// box in the brake-gated pivot regime instead of the radius constraint.
const NEUTRAL_THROTTLE: f32 = 0.1;
const NEUTRAL_M_SPEED: f32 = 0.5;

/// Steering-detent hysteresis on |steer| (design: two steps per gear, `|steer| ≥ 0.5` tight):
/// straight→wide engages at `WIDE_ON`, releases at `WIDE_OFF`; wide→tight at `TIGHT_ON`,
/// back at `TIGHT_OFF`.
const WIDE_ON: f32 = 0.15;
const WIDE_OFF: f32 = 0.05;
const TIGHT_ON: f32 = 0.55;
const TIGHT_OFF: f32 = 0.45;

/// The engine's declared operating envelope: a piecewise-linear torque curve (N·m over rpm,
/// ascending, clamped at the ends) under a fuel governor at `governed_rpm`.
#[derive(Clone, Debug)]
pub struct EngineParams {
    pub idle_rpm: f32,
    pub governed_rpm: f32,
    /// `(rpm, N·m)` authoring points, ascending rpm.
    pub torque_nm: Vec<(f32, f32)>,
}

/// The joint transmission's declared data — everything vehicle spec, nothing tuned-to-feel.
/// Built from authored tables via [`TransmissionParams::from_authoring`] (the spec block /
/// the sandbox's T-34 lab values).
#[derive(Clone, Debug)]
pub struct TransmissionParams {
    pub engine: EngineParams,
    /// Total reduction (engine rev per sprocket rev) per forward gear, 1-based order. DERIVED
    /// from authored per-gear speeds (the anchors) against the spec's own sprocket radius —
    /// speed ratios are r_s-independent, so the ladder survives the open 19-vs-20-tooth
    /// sprocket discrepancy (tiger-transmission-data.md implementation rule).
    pub gears_fwd: Vec<f32>,
    pub gears_rev: Vec<f32>,
    /// Sprocket pitch radius (m): belt speed = engine speed / G × r_s.
    pub sprocket_radius: f32,
    /// Auto-shift rpm bands (hysteresis: the gap between them must exceed one ratio step).
    pub shift_up_rpm: f32,
    pub shift_down_rpm: f32,
    /// Per FORWARD gear `(κ_tight, κ_wide)` where `κ = half_tread / R` — the L600 detents;
    /// the hybrid reads `κ_tight` as its full-lock continuous curvature. Reverse gears index
    /// the same table (R1–R4 mirror F1–F4).
    pub steer_kappa: Vec<(f32, f32)>,
    /// Steering-member force capacity on the `d` axis (N) — bounds the hybrid servo AND the
    /// L600 constraint force λ (beyond it the constraint slips).
    pub steer_capacity_n: f32,
    /// Full neutral-turn belt-speed half-difference (m/s): `κ_tight(F1) × v(F1 @ governed)` —
    /// the hybrid's genuine pivot scale.
    pub neutral_d_full: f32,
    /// The L600's brake-gated pivot fraction of [`Self::neutral_d_full`] (marginal neutral
    /// turn, literature synthesis).
    pub neutral_fraction: f32,
    /// Inner→outer recirculation efficiency η (mechanical ~0.9, INFERRED tag at the authoring
    /// site).
    pub recirculation: f32,
    /// Per-side service/parking brake capacity at the sprocket (N).
    pub brake_capacity_n: f32,
    /// Derived at construction: the torque curve's peak (the low-speed rev target).
    pub peak_torque_rpm: f32,
    pub peak_torque_nm: f32,
}

/// The authored tables the params derive from — the shape the spec block and the sandbox lab
/// both author. Speeds, radii, and anchor rpm are the source data; reductions and curvatures
/// are derived here so two numbers that must agree stay one number.
pub struct TransmissionAuthoring<'a> {
    pub idle_rpm: f32,
    pub governed_rpm: f32,
    /// The rpm the per-gear speeds are anchored at (the Tiger's are quoted @ 3000).
    pub rated_rpm: f32,
    pub torque_nm: &'a [(f32, f32)],
    /// Per-gear top BELT speeds (km/h) at `rated_rpm`, 1st..top.
    pub forward_speeds_kmh: &'a [f32],
    pub reverse_speeds_kmh: &'a [f32],
    pub shift_up_rpm: f32,
    pub shift_down_rpm: f32,
    /// Per forward gear `(R_tight, R_wide)` turn radii (m).
    pub steer_radii_m: &'a [(f32, f32)],
    pub steer_capacity_n: f32,
    pub neutral_fraction: f32,
    pub recirculation: f32,
    pub brake_capacity_n: f32,
    pub sprocket_radius_m: f32,
    /// Track half-tread `b` (m) — the spec's `plane_x`.
    pub half_tread_m: f32,
}

const RPM_TO_RAD: f32 = std::f32::consts::TAU / 60.0;

impl TransmissionParams {
    pub fn from_authoring(a: &TransmissionAuthoring) -> Self {
        assert!(
            !a.forward_speeds_kmh.is_empty()
                && !a.reverse_speeds_kmh.is_empty()
                && a.torque_nm.len() >= 2
                && a.steer_radii_m.len() == a.forward_speeds_kmh.len(),
            "transmission authoring tables must be populated (one radii pair per forward gear)"
        );
        let omega_rated = a.rated_rpm * RPM_TO_RAD;
        let gear = |v_kmh: &f32| omega_rated * a.sprocket_radius_m / (v_kmh / 3.6);
        let gears_fwd: Vec<f32> = a.forward_speeds_kmh.iter().map(gear).collect();
        let gears_rev: Vec<f32> = a.reverse_speeds_kmh.iter().map(gear).collect();
        let steer_kappa: Vec<(f32, f32)> = a
            .steer_radii_m
            .iter()
            .map(|&(tight, wide)| (a.half_tread_m / tight, a.half_tread_m / wide))
            .collect();
        let (peak_torque_rpm, peak_torque_nm) =
            a.torque_nm
                .iter()
                .copied()
                .fold((a.idle_rpm, 0.0f32), |best, (rpm, t)| {
                    if t > best.1 { (rpm, t) } else { best }
                });
        // Genuine neutral-steer scale: 1st gear tight curvature × 1st gear governed speed.
        let v1_governed = a.forward_speeds_kmh[0] / 3.6 * (a.governed_rpm / a.rated_rpm);
        let neutral_d_full = steer_kappa[0].0 * v1_governed;
        Self {
            engine: EngineParams {
                idle_rpm: a.idle_rpm,
                governed_rpm: a.governed_rpm,
                torque_nm: a.torque_nm.to_vec(),
            },
            gears_fwd,
            gears_rev,
            sprocket_radius: a.sprocket_radius_m,
            shift_up_rpm: a.shift_up_rpm,
            shift_down_rpm: a.shift_down_rpm,
            steer_kappa,
            steer_capacity_n: a.steer_capacity_n,
            neutral_d_full,
            neutral_fraction: a.neutral_fraction,
            recirculation: a.recirculation,
            brake_capacity_n: a.brake_capacity_n,
            peak_torque_rpm,
            peak_torque_nm,
        }
    }

    /// Engine torque (N·m) at `rpm`: piecewise-linear over the authored points (end-clamped),
    /// under the fuel-governor cut past `governed_rpm`.
    pub fn torque_at(&self, rpm: f32) -> f32 {
        let pts = &self.engine.torque_nm;
        let raw = if rpm <= pts[0].0 {
            pts[0].1
        } else if rpm >= pts[pts.len() - 1].0 {
            pts[pts.len() - 1].1
        } else {
            let mut t = pts[0].1;
            for w in pts.windows(2) {
                let ((r0, t0), (r1, t1)) = (w[0], w[1]);
                if rpm >= r0 && rpm <= r1 {
                    t = t0 + (t1 - t0) * ((rpm - r0) / (r1 - r0).max(1e-3));
                    break;
                }
            }
            t
        };
        let cut = (1.0 - (rpm - self.engine.governed_rpm) / GOVERNOR_CUT_RPM).clamp(0.0, 1.0);
        raw * cut
    }

    /// The belt speed (m/s) the top forward gear reaches at the governed rpm — the
    /// gearing-implied top speed the straight-line gate asserts against.
    pub fn geared_top_speed(&self) -> f32 {
        let g = *self.gears_fwd.last().expect("non-empty ladder");
        self.engine.governed_rpm * RPM_TO_RAD * self.sprocket_radius / g
    }
}

/// The joint transmission's path-dependent state — the ONLY memory (design §2's REV-14 list):
/// selected gear, shift countdown, steering detent, direction. Constructed at spawn from tank
/// data; a plain local component / sandbox resource under REV 13.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TransmissionState {
    /// 1-based gear in the active ladder.
    pub gear: u8,
    /// Remaining torque-interruption ticks of an in-flight shift.
    pub shift_ticks: u8,
    /// L600 steering detent: 0 straight (locked differential), 1 wide, 2 tight.
    pub steer_step: u8,
    /// Which ladder is engaged (reverse uses the R ladder).
    pub reverse: bool,
}

impl Default for TransmissionState {
    fn default() -> Self {
        Self {
            gear: 1,
            shift_ticks: 0,
            steer_step: 0,
            reverse: false,
        }
    }
}

/// One tick's joint input: the SHAPED drive axes plus the per-side mixed commands (the brake
/// envelope and the governor adapter consume the sides; the regenerative adapters consume the
/// axes), the pre-tick belt speeds, and this tick's summed longitudinal ground reactions.
pub struct TransmissionInput {
    pub throttle: f32,
    pub steer: f32,
    pub side_commands: [f32; 2],
    pub speeds: [f32; 2],
    pub reactions: [f32; 2],
    pub dt: f32,
}

/// What the joint solve produced: the integrated next belt speeds, the per-side sprocket
/// forces actually applied (telemetry — the `engine` slot of the harness rows), and the
/// operating point for HUD/legend display.
#[derive(Clone, Copy, Debug, Default)]
pub struct TransmissionReport {
    pub next_speeds: [f32; 2],
    pub forces: [f32; 2],
    pub rpm: f32,
    pub gear: u8,
    pub reverse: bool,
    pub steer_step: u8,
    pub shifting: bool,
    /// Power-conservation scale applied to the drive/steer forces this tick (1 = unconstrained).
    pub power_scale: f32,
    /// Engine power available at the operating point (W) — the energy gate's per-tick bound.
    pub power_available: f32,
}

/// Advance the joint drivetrain one fixed tick and integrate both belt speeds.
///
/// `Governor` is the exact legacy per-side path ([`forces::governor_belt`]) — `state` is not
/// touched, and the results are bit-identical to the shipped `step_side` tail. The
/// regenerative adapters implement the m/d superimposed model documented at module level.
pub fn step(
    mode: TransmissionMode,
    fp: &ForceParams,
    tp: Option<&TransmissionParams>,
    state: &mut TransmissionState,
    inp: &TransmissionInput,
) -> TransmissionReport {
    let (mode, tp) = match (mode, tp) {
        (TransmissionMode::Governor, _) | (_, None) => (TransmissionMode::Governor, None),
        (m, Some(tp)) => (m, Some(tp)),
    };
    match mode {
        TransmissionMode::Governor => {
            let mut report = TransmissionReport {
                power_scale: 1.0,
                ..Default::default()
            };
            for i in 0..2 {
                let (engine, next) = forces::governor_belt(
                    fp,
                    inp.side_commands[i],
                    inp.speeds[i],
                    inp.reactions[i],
                    inp.dt,
                );
                report.forces[i] = engine;
                report.next_speeds[i] = next;
            }
            report
        }
        _ => regenerative(
            mode,
            fp,
            tp.expect("regenerative modes carry params"),
            state,
            inp,
        ),
    }
}

fn regenerative(
    mode: TransmissionMode,
    fp: &ForceParams,
    tp: &TransmissionParams,
    st: &mut TransmissionState,
    inp: &TransmissionInput,
) -> TransmissionReport {
    let dt = inp.dt;
    let [vl, vr] = inp.speeds;
    let m = (vl + vr) / 2.0;
    let d = (vl - vr) / 2.0;

    // --- Direction: the F/R ladder swap happens near standstill only; above it a commanded
    // reversal drives the propulsion force against the motion (driveline braking) first.
    const DEAD: f32 = 0.05;
    if st.shift_ticks == 0 {
        let want_rev = inp.throttle < -DEAD;
        let want_fwd = inp.throttle > DEAD;
        if (want_rev || want_fwd) && want_rev != st.reverse && m.abs() < DIRECTION_SWAP_SPEED {
            st.reverse = want_rev;
            st.gear = 1;
            st.shift_ticks = SHIFT_TICKS;
        }
    }
    let ladder: &[f32] = if st.reverse {
        &tp.gears_rev
    } else {
        &tp.gears_fwd
    };
    let top = ladder.len() as u8;
    st.gear = st.gear.clamp(1, top);

    // --- Auto-shift on engine-rpm bands, hysteresis from the band gap; a shift in flight
    // blocks further decisions until its interruption window has elapsed.
    let rpm_geared = |g: f32| m.abs() * g / tp.sprocket_radius / RPM_TO_RAD;
    if st.shift_ticks == 0 {
        let rpm = rpm_geared(ladder[(st.gear - 1) as usize]);
        if rpm > tp.shift_up_rpm && st.gear < top {
            st.gear += 1;
            st.shift_ticks = SHIFT_TICKS;
        } else if rpm < tp.shift_down_rpm && st.gear > 1 {
            st.gear -= 1;
            st.shift_ticks = SHIFT_TICKS;
        }
    }
    let shifting = st.shift_ticks > 0;
    if shifting {
        st.shift_ticks -= 1;
    }
    let g = ladder[(st.gear - 1) as usize];

    // --- Engine operating point. Below the geared rpm the crank is allowed to rev toward the
    // torque peak with command (the declutch/slip band a preselector launch actually uses —
    // INFERRED simplification standing in for clutch-slip state; without it a standing start
    // would read idle rpm and a governed-out curve would read zero torque).
    let cmd_mag = inp.throttle.abs().max(inp.steer.abs()).clamp(0.0, 1.0);
    let rpm_floor = tp.engine.idle_rpm + (tp.peak_torque_rpm - tp.engine.idle_rpm) * cmd_mag;
    let rpm = rpm_geared(g).max(rpm_floor);
    let torque = tp.torque_at(rpm);
    let p_avail = torque * (rpm * RPM_TO_RAD);

    // --- Propulsion on m: torque curve × total reduction at the sprocket, throttle-scaled,
    // zero through a shift's interruption window. Engine drag is a reflected drag TORQUE
    // (design §3), saturating over DRAG_SAT_SPEED and released as the throttle opens.
    let dir = if st.reverse { -1.0 } else { 1.0 };
    let mut f_p = if shifting {
        0.0
    } else {
        dir * inp.throttle.abs() * (torque * g / tp.sprocket_radius)
    };
    let f_drag = -(tp.peak_torque_nm * ENGINE_DRAG_FRACTION * g / tp.sprocket_radius)
        * (m / DRAG_SAT_SPEED).clamp(-1.0, 1.0)
        * forces::hold_blend(inp.throttle.abs() / DRAG_THROTTLE_RELEASE);

    // --- Steering. κ table indexed by the active gear (reverse mirrors the low forward
    // gears); `d` follows the steer SIGN regardless of travel direction — the superimposed
    // steering shaft is independent of the gear's direction, historically and mechanically.
    let kappa_idx = ((st.gear - 1) as usize).min(tp.steer_kappa.len() - 1);
    let (k_tight, k_wide) = tp.steer_kappa[kappa_idx];
    let mut f_s = 0.0;
    let mut lambda = 0.0;
    let mut j = [0.0f32; 2];
    let servo =
        |target_d: f32| ((target_d - d) / STEER_SERVO_BAND).clamp(-1.0, 1.0) * tp.steer_capacity_n;
    match mode {
        TransmissionMode::Hybrid => {
            // Continuous curvature command: |steer| interpolates 0..κ_tight of the current
            // gear (interpolating radii continuously is exactly what makes this the hybrid,
            // not an L600 — design menu B's closing note), with the genuine-pivot floor so
            // steering authority never vanishes at low m.
            let target = inp.steer.signum()
                * (inp.steer.abs() * k_tight * m.abs()).max(inp.steer.abs() * tp.neutral_d_full);
            if inp.steer != 0.0 || d != 0.0 {
                f_s = servo(target);
            }
        }
        TransmissionMode::FixedRadii => {
            // Steering detents with hysteresis on |steer|.
            let a = inp.steer.abs();
            st.steer_step = match st.steer_step {
                0 => u8::from(a >= WIDE_ON),
                1 => {
                    if a >= TIGHT_ON {
                        2
                    } else {
                        u8::from(a >= WIDE_OFF)
                    }
                }
                _ => {
                    if a < TIGHT_OFF {
                        1
                    } else {
                        2
                    }
                }
            };
            let neutral = inp.throttle.abs() < NEUTRAL_THROTTLE && m.abs() < NEUTRAL_M_SPEED;
            if neutral {
                // The marginal brake-gated neutral turn: a slow servo toward the declared
                // fraction of the 1st-gear tight ratio, capacity-limited.
                f_s = servo(inp.steer * tp.neutral_d_full * tp.neutral_fraction);
            } else {
                // The geared-radius constraint g = d − s·κ·|m| = 0, solved semi-implicitly:
                // λ is the force that lands g at zero after this tick's integration, clamped
                // to the steering capacity (beyond it the constraint slips). Zero ideal work:
                // Q_c·v = λ·g, which the solve drives to zero.
                let s = if inp.steer > 0.0 {
                    1.0
                } else if inp.steer < 0.0 {
                    -1.0
                } else {
                    0.0
                };
                let kappa = match st.steer_step {
                    0 => 0.0,
                    1 => k_wide,
                    _ => k_tight,
                };
                let e = s
                    * kappa
                    * if m > 0.0 {
                        1.0
                    } else if m < 0.0 {
                        -1.0
                    } else {
                        0.0
                    };
                let jl = (1.0 - e) / 2.0;
                let jr = -(1.0 + e) / 2.0;
                let g_now = d - s * kappa * m.abs();
                let a_l = (f_p + f_drag) / 2.0 - inp.reactions[0];
                let a_r = (f_p + f_drag) / 2.0 - inp.reactions[1];
                let denom = jl * jl + jr * jr;
                lambda = (-(g_now * fp.inertia / dt + jl * a_l + jr * a_r) / denom)
                    .clamp(-tp.steer_capacity_n, tp.steer_capacity_n);
                j = [jl, jr];
            }
        }
        TransmissionMode::Governor => unreachable!("handled by the caller"),
    }

    // --- Power conservation: delivered ≤ engine power available at the operating point, with
    // inner-track negative power recirculated at η. One common scale on the drive + steer
    // forces (a tight turn slows the tank — the physically required speed loss). The λ
    // constraint transfers power between the tracks and is excluded (zero ideal work); drag
    // and brakes only remove energy.
    let p_p = f_p * m;
    let p_s = f_s * d;
    let pos = p_p.max(0.0) + p_s.max(0.0);
    let neg = (-p_p).max(0.0) + (-p_s).max(0.0);
    let net = pos - tp.recirculation * neg;
    let power_scale = if net > p_avail && net > 0.0 {
        p_avail / net
    } else {
        1.0
    };
    f_p *= power_scale;
    f_s *= power_scale;

    // --- Assemble per-side sprocket forces.
    let mut q = [
        (f_p + f_drag) / 2.0 + f_s / 2.0 + lambda * j[0],
        (f_p + f_drag) / 2.0 - f_s / 2.0 + lambda * j[1],
    ];

    // --- The reframed brake (design §3): −R always reaches the belt; near zero command +
    // zero belt speed the parking/service brake statically balances what the drivetrain
    // doesn't, inside its capacity. h reuses the legacy hold-blend SHAPE purely as the
    // engagement envelope (tick-stable, exact at rest); grip_stiffness = 0 keeps the
    // kinetic-only parity semantics brakeless, like the legacy hold.
    for i in 0..2 {
        let h = if fp.grip_stiffness > 0.0 {
            let target = inp.side_commands[i] * fp.max_speed;
            forces::hold_blend(target.abs() / fp.slip_saturation)
                * forces::hold_blend(inp.speeds[i].abs() / fp.slip_saturation)
        } else {
            0.0
        };
        let cap = h * tp.brake_capacity_n;
        q[i] += (inp.reactions[i] - q[i]).clamp(-cap, cap);
    }

    // --- Integrate both sides simultaneously: I·v̇ = Q − R (the reaction ALWAYS applies).
    let mut next = [0.0f32; 2];
    for i in 0..2 {
        next[i] = (inp.speeds[i] + (q[i] - inp.reactions[i]) / fp.inertia * dt)
            .clamp(-fp.max_speed, fp.max_speed);
    }

    TransmissionReport {
        next_speeds: next,
        forces: q,
        rpm,
        gear: st.gear,
        reverse: st.reverse,
        steer_step: st.steer_step,
        shifting,
        power_scale,
        power_available: p_avail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lab ForceParams: only the fields the transmission reads matter (inertia, max_speed,
    /// slip_saturation, grip_stiffness envelope switch, and the governor's own knobs).
    fn lab_fp() -> ForceParams {
        ForceParams {
            thickness: 0.04,
            columns: [(-0.25, 1.0 / 6.0), (0.0, 2.0 / 3.0), (0.25, 1.0 / 6.0)],
            support_stiffness_per_m: 680_000.0,
            support_damping_per_m: 80_000.0,
            engage_depth: 0.02,
            probe_reach: 0.5,
            mu: 0.9,
            lateral_ratio: 0.55,
            slip_saturation: 0.4,
            max_speed: 15.0,
            engine_power: 186_500.0,
            engine_force: 120_000.0,
            governor_gain: 60_000.0,
            inertia: 8_000.0,
            grip_stiffness: forces::grip_stiffness(0.9, 26_500.0 * 9.81),
        }
    }

    /// A lab transmission: T-34-flavoured plausible tables (the sandbox's config shape).
    fn lab_tp() -> TransmissionParams {
        TransmissionParams::from_authoring(&TransmissionAuthoring {
            idle_rpm: 600.0,
            governed_rpm: 1800.0,
            rated_rpm: 1800.0,
            torque_nm: &[
                (600.0, 1650.0),
                (1100.0, 2200.0),
                (1700.0, 1950.0),
                (1800.0, 0.0),
            ],
            forward_speeds_kmh: &[8.0, 12.7, 20.4, 32.6, 52.2],
            reverse_speeds_kmh: &[8.0],
            shift_up_rpm: 1700.0,
            shift_down_rpm: 950.0,
            steer_radii_m: &[
                (3.0, 8.9),
                (4.8, 14.2),
                (7.7, 22.8),
                (12.3, 36.4),
                (19.7, 58.3),
            ],
            steer_capacity_n: 240_000.0,
            neutral_fraction: 0.5,
            recirculation: 0.9,
            brake_capacity_n: 120_000.0,
            sprocket_radius_m: 0.34,
            half_tread_m: 1.25,
        })
    }

    fn input(
        throttle: f32,
        steer: f32,
        speeds: [f32; 2],
        reactions: [f32; 2],
    ) -> TransmissionInput {
        TransmissionInput {
            throttle,
            steer,
            side_commands: [
                (throttle + steer).clamp(-1.0, 1.0),
                (throttle - steer).clamp(-1.0, 1.0),
            ],
            speeds,
            reactions,
            dt: 1.0 / 64.0,
        }
    }

    /// The Governor adapter IS the legacy tail: per side, bit-equal to `governor_belt`.
    #[test]
    fn governor_adapter_matches_legacy_belt() {
        let fp = lab_fp();
        let mut st = TransmissionState::default();
        let inp = input(0.7, 0.3, [4.2, -1.1], [23_000.0, -9_500.0]);
        let report = step(
            TransmissionMode::Governor,
            &fp,
            Some(&lab_tp()),
            &mut st,
            &inp,
        );
        for i in 0..2 {
            let (engine, next) = forces::governor_belt(
                &fp,
                inp.side_commands[i],
                inp.speeds[i],
                inp.reactions[i],
                inp.dt,
            );
            assert_eq!(report.forces[i], engine);
            assert_eq!(report.next_speeds[i], next);
        }
        assert_eq!(
            st,
            TransmissionState::default(),
            "governor must not touch state"
        );
    }

    /// Auto-shift: crossing the up band shifts up exactly once (the interruption window
    /// blocks a second decision), the mid-band is quiet in both directions, and the down
    /// band shifts down — the hysteresis gap is what kills hunting.
    #[test]
    fn gear_shift_hysteresis() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let mut st = TransmissionState::default();
        // rpm(gear1) at m: m·G1/r_s in rad/s → rpm. G1 ≈ ω_rated·r_s/v1.
        let g1 = tp.gears_fwd[0];
        let m_for = |rpm: f32| rpm * RPM_TO_RAD * tp.sprocket_radius / g1;

        // Above the up band → one upshift, then the window holds further decisions.
        let v = m_for(1_750.0);
        step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [v, v], [0.0, 0.0]),
        );
        assert_eq!(st.gear, 2);
        assert!(st.shift_ticks > 0);
        step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [v, v], [0.0, 0.0]),
        );
        assert_eq!(
            st.gear, 2,
            "no second decision inside the interruption window"
        );

        // Drain the window at a mid-band speed for gear 2: no hunting either way.
        let g2 = tp.gears_fwd[1];
        let v_mid = 1_300.0 * RPM_TO_RAD * tp.sprocket_radius / g2;
        for _ in 0..(SHIFT_TICKS as usize + 5) {
            step(
                TransmissionMode::Hybrid,
                &fp,
                Some(&tp),
                &mut st,
                &input(1.0, 0.0, [v_mid, v_mid], [0.0, 0.0]),
            );
        }
        assert_eq!(st.gear, 2);
        assert_eq!(st.shift_ticks, 0);

        // Below the down band → downshift.
        let v_low = 900.0 * RPM_TO_RAD * tp.sprocket_radius / g2;
        step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [v_low, v_low], [0.0, 0.0]),
        );
        assert_eq!(st.gear, 1);
    }

    /// The shift is a torque interruption: propulsion force is zero for exactly
    /// [`SHIFT_TICKS`] ticks, then returns. (Throttle 1.0 keeps engine drag released, and
    /// reactions are zero, so the per-side force IS the propulsion share.)
    #[test]
    fn shift_torque_interruption_window() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let mut st = TransmissionState::default();
        let g1 = tp.gears_fwd[0];
        let v = 1_750.0 * RPM_TO_RAD * tp.sprocket_radius / g1;
        let inp = input(1.0, 0.0, [v, v], [0.0, 0.0]);
        let mut zero_ticks = 0;
        loop {
            let r = step(TransmissionMode::Hybrid, &fp, Some(&tp), &mut st, &inp);
            if r.shifting {
                assert_eq!(
                    r.forces[0], 0.0,
                    "torque must be interrupted through the shift"
                );
                assert_eq!(r.forces[1], 0.0);
                zero_ticks += 1;
            } else if zero_ticks > 0 {
                assert!(r.forces[0] > 0.0, "torque must return after the window");
                break;
            }
            assert!(zero_ticks <= SHIFT_TICKS as usize, "window must end");
        }
        assert_eq!(zero_ticks, SHIFT_TICKS as usize);
    }

    /// The L600 constraint converges to the geared ratio: under sustained throttle + tight
    /// steer with no ground reaction, d/|m| lands on κ_tight of the active gear.
    #[test]
    fn l600_constraint_holds_geared_ratio() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let mut st = TransmissionState::default();
        let mut speeds = [0.0f32; 2];
        let mut last = TransmissionReport::default();
        for _ in 0..400 {
            let inp = input(0.5, 1.0, speeds, [0.0, 0.0]);
            last = step(TransmissionMode::FixedRadii, &fp, Some(&tp), &mut st, &inp);
            speeds = last.next_speeds;
        }
        assert_eq!(st.steer_step, 2, "|steer| = 1 must engage the tight detent");
        let m = (speeds[0] + speeds[1]) / 2.0;
        let d = (speeds[0] - speeds[1]) / 2.0;
        assert!(m > 0.5, "the tank must be driving (m = {m})");
        let kappa = tp.steer_kappa[(last.gear - 1) as usize].0;
        let ratio = d / m.abs();
        assert!(
            (ratio - kappa).abs() < 0.01 * kappa.max(0.05),
            "d/m = {ratio} must hold κ_tight = {kappa} (gear {})",
            last.gear
        );
    }

    /// Steering detent hysteresis: the tight step engages at ≥ TIGHT_ON and releases only
    /// below TIGHT_OFF (the |steer| ≥ 0.5 design threshold, hysteresis-wrapped).
    #[test]
    fn steer_step_hysteresis() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let mut st = TransmissionState::default();
        let mut at = |steer: f32| {
            step(
                TransmissionMode::FixedRadii,
                &fp,
                Some(&tp),
                &mut st,
                &input(0.5, steer, [2.0, 2.0], [0.0, 0.0]),
            );
            st.steer_step
        };
        assert_eq!(at(0.10), 0, "below WIDE_ON stays straight");
        assert_eq!(at(0.30), 1, "wide engages");
        assert_eq!(
            at(0.50),
            1,
            "0.5 is inside the tight hysteresis band from below"
        );
        assert_eq!(at(0.60), 2, "tight engages at ≥ TIGHT_ON");
        assert_eq!(at(0.50), 2, "0.5 holds tight from above");
        assert_eq!(at(0.40), 1, "below TIGHT_OFF releases to wide");
        assert_eq!(at(0.02), 0, "below WIDE_OFF releases to straight");
    }

    /// The brake reframe: a parked belt inside capacity holds EXACTLY (v̇ = 0, bit-zero);
    /// slope demand beyond B_max back-drives the belt honestly.
    #[test]
    fn brake_capacity_breach_backdrives() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let mut st = TransmissionState::default();
        // Inside capacity: R = 0.8·B_max, zero command, zero speed → exact hold.
        let r_in = 0.8 * tp.brake_capacity_n;
        let rep = step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(0.0, 0.0, [0.0, 0.0], [r_in, r_in]),
        );
        assert_eq!(
            rep.next_speeds,
            [0.0, 0.0],
            "inside capacity the brake holds exactly"
        );
        // Past capacity: R = 1.5·B_max → the belt is back-driven.
        let r_out = 1.5 * tp.brake_capacity_n;
        let rep = step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(0.0, 0.0, [0.0, 0.0], [r_out, r_out]),
        );
        assert!(
            rep.next_speeds[0] < 0.0 && rep.next_speeds[1] < 0.0,
            "slope demand past B_max must back-drive the belt (got {:?})",
            rep.next_speeds
        );
    }

    /// Energy honesty over 64-tick windows: Σ(Q_L·v_L + Q_R·v_R)·dt never exceeds the
    /// integrated engine power available plus released belt-inertia energy — regeneration
    /// recirculates, it does not create (the design's no-free-energy bound). Exercised over
    /// a launch, a driving turn, and a pivot, in both regenerative modes.
    #[test]
    fn energy_bound_no_free_energy() {
        let (fp, tp) = (lab_fp(), lab_tp());
        for (mode, throttle, steer) in [
            (TransmissionMode::Hybrid, 1.0, 0.0),
            (TransmissionMode::Hybrid, 0.7, 0.6),
            (TransmissionMode::Hybrid, 0.0, 1.0),
            (TransmissionMode::FixedRadii, 1.0, 0.0),
            (TransmissionMode::FixedRadii, 0.7, 0.8),
            (TransmissionMode::FixedRadii, 0.0, 1.0),
        ] {
            let mut st = TransmissionState::default();
            let mut speeds = [0.0f32; 2];
            let dt = 1.0 / 64.0;
            for window in 0..6 {
                let mut delivered = 0.0f64;
                let mut available = 0.0f64;
                let e0: f64 = speeds
                    .iter()
                    .map(|&v| 0.5 * f64::from(fp.inertia) * f64::from(v) * f64::from(v))
                    .sum();
                for _ in 0..64 {
                    // Synthetic ground reaction: a drag opposing each belt (30 kN/(m/s),
                    // saturating at 25 kN) — enough load to exercise the power limiter.
                    let reactions = speeds.map(|v| (v * 30_000.0).clamp(-25_000.0, 25_000.0));
                    let inp = input(throttle, steer, speeds, reactions);
                    let rep = step(mode, &fp, Some(&tp), &mut st, &inp);
                    delivered += f64::from(rep.forces[0] * speeds[0] + rep.forces[1] * speeds[1])
                        * f64::from(dt);
                    available += f64::from(rep.power_available) * f64::from(dt);
                    speeds = rep.next_speeds;
                }
                let e1: f64 = speeds
                    .iter()
                    .map(|&v| 0.5 * f64::from(fp.inertia) * f64::from(v) * f64::from(v))
                    .sum();
                let released = (e0 - e1).max(0.0);
                assert!(
                    delivered <= available + released + 500.0,
                    "{mode:?} t={throttle} s={steer} window {window}: delivered {delivered:.0} J \
                     > available {available:.0} J + released {released:.0} J"
                );
            }
        }
    }

    /// The gearing-implied top speed: the lab ladder's top gear at governed rpm is the
    /// authored 52.2 km/h × (governed/rated) — the value the sandbox straight-line gate
    /// asserts the measured speed against.
    #[test]
    fn geared_top_speed_matches_authoring() {
        let tp = lab_tp();
        let expect = 52.2 / 3.6 * (1800.0 / 1800.0);
        assert!((tp.geared_top_speed() - expect).abs() < 0.01);
    }
}
