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
//!   verbatim): every MP composition and every existing baseline runs this. The parity switch.
//! - [`TransmissionMode::Hybrid`] — the arcade-honest continuous regenerative box (design
//!   menu C/D): engine torque curve × gear ratio → propulsion force on `m`; a
//!   capacity-limited steering servo on `d` (continuous curvature command interpolating the
//!   authored radii); power conservation with inner-track recirculation at declared η.
//! - [`TransmissionMode::FixedRadii`] — the Tiger L600: the same machinery, but `d` is
//!   CONSTRAINED to `s·κ(gear, step)·|m|` (two steering detents per gear with hysteresis);
//!   the constraint force λ is solved semi-implicitly and clamped so each output's steering
//!   share stays inside the per-output capacity (beyond it the constraint slips). At `m≈0`
//!   the neutral turn is the MARGINAL
//!   brake-gated one the restoration literature describes: a slow capacity-limited
//!   counter-rotation at a declared fraction of the 1st-gear tight ratio.
//!
//! Brakes (design §3, the hold reframe): the ground reaction is ALWAYS applied to the belt
//! (never attenuated). The brake force is the capacity-limited STOP force
//! `Bᵢ = clamp(Rᵢ − Qᵢ − vᵢ·I/dt, ±cap)` — at rest exactly the static balance `Rᵢ − Qᵢ`
//! (a parked tank on a slope inside capacity holds EXACTLY), in motion strictly opposing
//! where the belt is headed (settles creep, saturates against a slide, never pushes through
//! zero). `cap` comes from the parking LATCH (zero command near standstill sets it, any
//! drive command releases it; latched = full `B_max` however fast a capacity breach
//! back-drives the belt), the legacy hold-blend entry envelope while unlatched, or the
//! service pedal (opposite-throttle driver intent). The `Governor` adapter keeps the old
//! hold blend verbatim instead.
//!
//! Pure math, no ECS (like [`forces`]): callers own the state. [`TransmissionState`] is the
//! only path-dependent state (gear, shift countdown, steering detent, direction) — carried as
//! a plain LOCAL component / sandbox resource, NOT replicated, NOT hashed (this is REV 13;
//! only the offline composition and the sandbox ever run the regenerative adapters).
//!
//! # The law/spec split (every constant in this module, classified)
//!
//! The module is the complete LAW; the spec block is the complete per-vehicle BEHAVIOR. The
//! test: would a different tank author it differently? If yes it must live in the spec —
//! everything below is what legitimately remains a module constant, with the rationale.
//!
//! | constant | class | rationale |
//! |---|---|---|
//! | [`GOVERNOR_CUT_RPM`] | SIM POLICY | numerical smoothing width so the top-speed equilibrium is a smooth root, not a hard clip; any governed engine gets the same treatment |
//! | [`DRAG_SAT_SPEED`] | SIM POLICY | anti-chatter ramp for the drag sign near standstill (the parking brake owns standstill); a solver guard, not a vehicle trait |
//! | [`DRAG_THROTTLE_RELEASE`] | SIM POLICY | driver-intent shaping: where "open throttle" stops meaning "motoring"; part of the uniform input contract, same for every tank |
//! | [`DEAD`] | SIM POLICY | input deadzone on one shared axis mapping |
//! | [`PARK_ENGAGE_SPEED`] | SIM POLICY | latch threshold for "at rest" — a determinism/stability guard on the shared intent layer |
//! | [`DIRECTION_SWAP_SPEED`] | SIM POLICY | the intent seam where a held opposite throttle becomes a gear-direction change; uniform game semantics |
//! | [`NEUTRAL_THROTTLE`], [`NEUTRAL_M_SPEED`] | SIM POLICY | regime-entry thresholds for the L600 neutral turn (the neutral turn's SPEED SCALE — `neutral_d_full × neutral_fraction` — is spec-derived) |
//! | [`WIDE_ON`]/[`WIDE_OFF`]/[`TIGHT_ON`]/[`TIGHT_OFF`] | SIM POLICY | stick-to-detent input mapping with hysteresis; the DETENT RATIOS they select are spec |
//! | [`TICK_HZ`] | SIM POLICY | the fixed simulation tick the shift countdown quantizes against |
//!
//! Moved OUT of this module to the spec (they were vehicle data wearing const clothing):
//! shift time (`gearbox.shift_secs` — a Tiger preselector and a T-34 crash box differ),
//! engine drag (`engine.drag_fraction` — an engine datum). REMOVED rather than classified:
//! `STEER_SERVO_BAND` — the steering servo is now the semi-implicit exact law (like the
//! brakes and λ), so no proportional band exists to tune; its droop was itself a
//! vehicle-scaling bug (the Tiger's neutral target sat inside the band). Everything else
//! the vehicle authors was already spec: torque curve, ladders, radii, capacities, brake,
//! η, neutral fraction.

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

/// The fixed simulation tick rate the shift countdown quantizes against (the module operates
/// on fixed 64 Hz ticks — module doc). SIM POLICY.
const TICK_HZ: f32 = 64.0;

/// Fuel-governor cut width (rpm): torque ramps linearly to zero over this band past the
/// governed rpm, so the top-speed equilibrium is a smooth root instead of a hard clip.
/// INFERRED numerical policy, not vehicle data.
const GOVERNOR_CUT_RPM: f32 = 100.0;

/// Belt-speed span (m/s) over which the engine-drag torque saturates — a viscous-near-zero
/// ramp so drag cannot sign-flip chatter around standstill (the parking brake owns standstill).
const DRAG_SAT_SPEED: f32 = 0.2;

/// PROPULSIVE throttle magnitude above which engine drag is fully released (blends out with
/// the hold-blend shape below it): an open throttle is not motoring. A BRAKE command
/// (throttle against the engaged ladder) is not propulsive — the engine keeps motoring, so
/// drag stays engaged under it.
const DRAG_THROTTLE_RELEASE: f32 = 0.5;

/// Input deadzone on the throttle axis for direction/brake intent (matches the swap logic's
/// historical deadband).
const DEAD: f32 = 0.05;

/// Belt speed (m/s) below which a zero command LATCHES the parking brake (released by any
/// drive command). A latch, not a blend: once parked the brake holds full capacity however
/// fast a capacity breach back-drives the belt — the engagement blend alone faded to zero
/// past `slip_saturation`, releasing the brake exactly when an over-capacity slope slid the
/// tank (codex-2).
const PARK_ENGAGE_SPEED: f32 = 0.05;

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
    /// Steering-member force capacity PER OUTPUT (N). The steering member drives the two
    /// outputs DIFFERENTIALLY — each output's steering share is bounded by its own
    /// gearing/grip-scale cap (this datum), so the belt-difference axis `F_s` carries up to
    /// 2× it (each side sees `F_s/2`), and the L600 constraint force λ is bounded by
    /// `capacity / max|jᵢ|`. Reading this datum as an `F_s` bound was the pivot-dead bug:
    /// it halves the yaw ceiling (Tiger: 373 kN·m < its ~478 kN·m footprint scrub — could
    /// not break away; the T-34 lab's 300 vs 224 kN·m masked it).
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
    /// Zero-throttle engine drag (compression braking) as a fraction of peak torque,
    /// reflected through the CURRENT gear — a drag TORQUE (design §3), never the negative
    /// half of rated power. Diesel motoring/compression braking runs ~20–30% of rated
    /// torque (INFERRED band, tagged at the authoring site).
    pub drag_fraction: f32,
    /// Gear-shift torque-interruption window in fixed ticks — DERIVED from the authored
    /// `shift_secs` (a Tiger preselector and a crash box shift very differently: vehicle
    /// data, not module policy).
    pub shift_ticks: u8,
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
    /// Steering-member force capacity PER OUTPUT (N) — see
    /// [`TransmissionParams::steer_capacity_n`] for the convention (the difference axis
    /// carries 2× this).
    pub steer_capacity_n: f32,
    pub neutral_fraction: f32,
    pub recirculation: f32,
    pub brake_capacity_n: f32,
    /// See [`TransmissionParams::drag_fraction`].
    pub drag_fraction: f32,
    /// Gear-shift torque-interruption time (s) — see [`TransmissionParams::shift_ticks`].
    pub shift_secs: f32,
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
            drag_fraction: a.drag_fraction,
            shift_ticks: (a.shift_secs * TICK_HZ).round().clamp(0.0, 255.0) as u8,
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
/// selected gear, shift countdown, steering detent, direction, parking latch. Constructed at
/// spawn from tank data; a plain local component / sandbox resource under REV 13.
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
    /// Parking-brake latch: set by a zero command near standstill, released by any drive
    /// command. Latched, the brake holds FULL capacity regardless of belt speed (see
    /// [`PARK_ENGAGE_SPEED`]).
    pub park: bool,
}

impl Default for TransmissionState {
    fn default() -> Self {
        Self {
            gear: 1,
            shift_ticks: 0,
            steer_step: 0,
            reverse: false,
            park: false,
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
    // reversal is a BRAKE command (service brakes, below) until the tank is nearly stopped.
    if st.shift_ticks == 0 {
        let want_rev = inp.throttle < -DEAD;
        let want_fwd = inp.throttle > DEAD;
        if (want_rev || want_fwd) && want_rev != st.reverse && m.abs() < DIRECTION_SWAP_SPEED {
            st.reverse = want_rev;
            st.gear = 1;
            st.shift_ticks = tp.shift_ticks;
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
            st.shift_ticks = tp.shift_ticks;
        } else if rpm < tp.shift_down_rpm && st.gear > 1 {
            st.gear -= 1;
            st.shift_ticks = tp.shift_ticks;
        }
    }
    let shifting = st.shift_ticks > 0;
    if shifting {
        st.shift_ticks -= 1;
    }
    let g = ladder[(st.gear - 1) as usize];

    // --- Driver intent (the game-layer W/S contract, declared HERE once with honest
    // mechanisms — the Governor conflated zero-throttle with brake-to-zero):
    //   * throttle WITH the engaged ladder → drive (`propulsive`);
    //   * throttle AGAINST it → SERVICE BRAKES (`service`, the declared brake capacity)
    //     until near standstill, where the ladder swap above engages the opposite gears —
    //     never `|throttle|`-drive in the engaged direction (that was the measured
    //     "cannot decelerate" bug: full reverse at speed produced full FORWARD force);
    //   * throttle released → coast under engine drag (compression braking through the
    //     CURRENT gear, growing as the box downshifts);
    //   * zero command at rest → the parking hold (unchanged).
    let dir = if st.reverse { -1.0 } else { 1.0 };
    let opposing = inp.throttle * dir < -DEAD;
    let propulsive = if opposing {
        0.0
    } else {
        inp.throttle.abs().clamp(0.0, 1.0)
    };
    let service = if opposing {
        inp.throttle.abs().clamp(0.0, 1.0)
    } else {
        0.0
    };

    // --- Parking latch (driver intent, cont.): a zero command near standstill sets the
    // lever; any drive command releases it. State, not a blend — see [`PARK_ENGAGE_SPEED`].
    if inp.throttle.abs() >= DEAD || inp.steer.abs() >= DEAD {
        st.park = false;
    } else if inp.speeds[0].abs().max(inp.speeds[1].abs()) < PARK_ENGAGE_SPEED {
        st.park = true;
    }

    // --- Engine operating point. Below the geared rpm the crank is allowed to rev toward the
    // torque peak with PROPULSIVE command (the declutch/slip band a preselector launch
    // actually uses — INFERRED simplification standing in for clutch-slip state; without it a
    // standing start would read idle rpm and a governed-out curve would read zero torque). A
    // brake command does not rev the crank.
    let cmd_mag = propulsive.max(inp.steer.abs()).clamp(0.0, 1.0);
    let rpm_floor = tp.engine.idle_rpm + (tp.peak_torque_rpm - tp.engine.idle_rpm) * cmd_mag;
    let rpm = rpm_geared(g).max(rpm_floor);
    let torque = tp.torque_at(rpm);
    let p_avail = torque * (rpm * RPM_TO_RAD);

    // --- Propulsion on m: torque curve × total reduction at the sprocket, throttle-scaled,
    // zero through a shift's interruption window. Engine drag is a reflected drag TORQUE
    // (design §3), saturating over DRAG_SAT_SPEED and released as the throttle opens —
    // release keys on the PROPULSIVE component only, so a brake command keeps the engine
    // motoring (compression braking stacks under the service brakes).
    let mut f_p = if shifting {
        0.0
    } else {
        dir * propulsive * (torque * g / tp.sprocket_radius)
    };
    let f_drag = -(tp.peak_torque_nm * tp.drag_fraction * g / tp.sprocket_radius)
        * (m / DRAG_SAT_SPEED).clamp(-1.0, 1.0)
        * forces::hold_blend(propulsive / DRAG_THROTTLE_RELEASE);

    // --- Steering. κ table indexed by the active gear (reverse mirrors the low forward
    // gears); `d` follows the steer SIGN regardless of travel direction — the superimposed
    // steering shaft is independent of the gear's direction, historically and mechanically.
    let kappa_idx = ((st.gear - 1) as usize).min(tp.steer_kappa.len() - 1);
    let (k_tight, k_wide) = tp.steer_kappa[kappa_idx];
    let mut f_s = 0.0;
    let mut lambda = 0.0;
    let mut j = [0.0f32; 2];
    // The difference-axis bound is 2× the per-output capacity: `F_s` splits `±F_s/2` onto
    // the outputs, and each output's share is what the per-output datum caps (see
    // [`TransmissionParams::steer_capacity_n`] — the pivot-dead convention fix).
    let f_s_max = 2.0 * tp.steer_capacity_n;
    // The steering member as a capacity-limited KINEMATIC servo, semi-implicit like the
    // brakes and λ: the F_s that lands `d` exactly on target after this tick's integration
    // (`d` dynamics: `d_next = d + (F_s/2 − R_d)/I·dt`), reaction-compensated, clamped to
    // the per-output convention's bound. Exact inside capacity, honest slip beyond it — and
    // no proportional band: the old P law's steady-state droop let the ground reaction eat
    // the command (the Tiger's whole neutral target, 0.21 m/s, sat INSIDE the 0.25 m/s
    // band, so a sustained pivot ran at ≤ half capacity and crawled at 0.03 rad/s — the
    // second vehicle-scaling defect of this fix round; the T-34's 0.46 m/s target masked
    // it).
    let r_d = (inp.reactions[0] - inp.reactions[1]) / 2.0;
    let servo =
        |target_d: f32| (2.0 * ((target_d - d) * fp.inertia / dt + r_d)).clamp(-f_s_max, f_s_max);
    match mode {
        TransmissionMode::Hybrid => {
            // Continuous curvature command, GEAR-INDEPENDENT: |steer| interpolates
            // 0..κ(R_min) where R_min is the vehicle's tightest authored radius (the
            // 1st-gear tight entry). This is the hydrostatic-superimposed family's defining
            // trait (design menu C: "infinitely variable… variable-speed pivot turns") — the
            // steer path bypasses the gearbox, so full lock always commands the minimum
            // radius and the POWER budget, not the ratio ladder, is what forces a fast tank
            // wide (measured: the power scale slows the hull into the radius it can afford —
            // the design's "strong turn-in, then physically required speed loss"). The
            // genuine-pivot floor keeps steering authority alive at m → 0.
            let k_full = tp.steer_kappa[0].0;
            let target = inp.steer.signum()
                * (inp.steer.abs() * k_full * m.abs()).max(inp.steer.abs() * tp.neutral_d_full);
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
                // so each output's share stays inside the per-output capacity (beyond it the
                // constraint slips). Zero ideal work: Q_c·v = λ·g, which the solve drives to
                // zero.
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
                let a_l = (f_p + f_drag) / 2.0 - inp.reactions[0];
                let a_r = (f_p + f_drag) / 2.0 - inp.reactions[1];
                // One |m| branch of the solve: on branch b, |m| linearizes as `b·m`, so
                // `g = jl·v_L + jr·v_R` with the branch's Jacobian. Returns λ (per-output
                // capacity clamp — straight-gear `max|j| = 1/2` gives the same 2× bound
                // the servo uses), the Jacobian, and the m the tick lands on under that λ
                // (brakes are disengaged in this regime: throttle is past the neutral
                // band, so no park/service term perturbs the prediction).
                let solve = |branch: f32| -> (f32, [f32; 2], f32) {
                    let e = s * kappa * branch;
                    let jl = (1.0 - e) / 2.0;
                    let jr = -(1.0 + e) / 2.0;
                    let g_now = jl * vl + jr * vr;
                    let denom = jl * jl + jr * jr;
                    let lambda_max = tp.steer_capacity_n / jl.abs().max(jr.abs()).max(1e-3);
                    let l = (-(g_now * fp.inertia / dt + jl * a_l + jr * a_r) / denom)
                        .clamp(-lambda_max, lambda_max);
                    let m_next = m + (a_l + l * jl + a_r + l * jr) / (2.0 * fp.inertia) * dt;
                    (l, [jl, jr], m_next)
                };
                let b0 = if m > 0.0 {
                    1.0
                } else if m < 0.0 {
                    -1.0
                } else {
                    0.0
                };
                let (l0, j0, m_next) = solve(b0);
                if b0 != 0.0 && m_next * b0 < 0.0 {
                    // The tick crosses m = 0: the pre-tick branch would project onto
                    // `d = s·κ·m` on the WRONG side of the |m| cusp — a one-tick
                    // steering-sign reversal (codex-4). Re-solve on the branch the belt
                    // actually lands on; if the branches disagree about the landing side
                    // (the genuine cusp), the constraint takes the tick off — λ = 0 is
                    // stable and passive there.
                    let (l1, j1, m1) = solve(-b0);
                    if m1 * b0 <= 0.0 {
                        lambda = l1;
                        j = j1;
                    }
                } else {
                    lambda = l0;
                    j = j0;
                }
            }
        }
        TransmissionMode::Governor => unreachable!("handled by the caller"),
    }

    // --- Power conservation: delivered ≤ engine power available at the operating point, with
    // inner-track negative power recirculated at η. One common scale on the drive + steer
    // forces (a tight turn slows the tank — the physically required speed loss). The split is
    // over the PHYSICAL OUTPUTS: the engine-borne per-side forces `(F_p ± F_s)/2` deliver
    // `Qᵢ·vᵢ` at each sprocket, and it is a SPROCKET going negative that recirculates —
    // modal powers (`F_p·m`, `F_s·d`) sum to the same total but mis-split it: with
    // `F_s > F_p` the inner sprocket is negative while both modal terms read positive, so η
    // was never charged (codex-3). The λ constraint transfers power between the tracks at
    // zero IDEAL work and is excluded from the engine budget — its declared-η transfer loss
    // is a known-open refinement (HQ: "L600 transfer loss"), not modeled here. Drag and
    // brakes only remove energy.
    let p_l = (f_p + f_s) / 2.0 * vl;
    let p_r = (f_p - f_s) / 2.0 * vr;
    let pos = p_l.max(0.0) + p_r.max(0.0);
    let neg = (-p_l).max(0.0) + (-p_r).max(0.0);
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
    for (i, qi) in q.iter_mut().enumerate() {
        // Engagement envelope: the parking LATCH holds full capacity (post-breach it keeps
        // rubbing at B_max instead of fading with speed — codex-2); unlatched, the legacy
        // smooth entry blend h (zero command + near-zero belt speed) eases the brake in
        // during settle; the service pedal is the driver-intent brake command. The paths
        // are mutually exclusive by construction (service ⇒ a drive command ⇒ unlatched,
        // h≈0). grip_stiffness = 0 keeps the kinetic-only parity semantics brakeless, like
        // the legacy hold.
        let h = if fp.grip_stiffness > 0.0 {
            if st.park {
                1.0
            } else {
                let target = inp.side_commands[i] * fp.max_speed;
                forces::hold_blend(target.abs() / fp.slip_saturation)
                    * forces::hold_blend(inp.speeds[i].abs() / fp.slip_saturation)
            }
        } else {
            0.0
        };
        let envelope = h.max(service);
        if envelope > 0.0 {
            let cap = envelope * tp.brake_capacity_n;
            // The capacity-limited STOP force `B = R − Q − vI/dt = −I·v_unbraked_next/dt`
            // (clamped): at rest it is exactly the static balance `R − Q` — the hold
            // gates' law, bit-identical — and in motion it opposes where the belt is
            // headed, so it SETTLES creep to zero instead of freezing v̇ at the creep
            // speed (the old `R − Q` alone did exactly that: B·v > 0 cancelling grip and
            // drag — codex-2 passivity), saturates at ±cap against a slide, and can
            // neither speed the belt up nor push it through zero.
            let stop = inp.reactions[i] - *qi - inp.speeds[i] * fp.inertia / dt;
            *qi += stop.clamp(-cap, cap);
        }
    }

    // --- Integrate both sides simultaneously: I·v̇ = Q − R (the reaction ALWAYS applies).
    let mut next = [0.0f32; 2];
    for (i, ni) in next.iter_mut().enumerate() {
        *ni = (inp.speeds[i] + (q[i] - inp.reactions[i]) / fp.inertia * dt)
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
            drag_fraction: 0.25,
            shift_secs: 0.31,
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
        for _ in 0..(tp.shift_ticks as usize + 5) {
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
    /// the authored `shift_secs` worth of ticks, then returns. (Throttle 1.0 keeps engine drag released, and
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
            assert!(zero_ticks <= tp.shift_ticks as usize, "window must end");
        }
        assert_eq!(zero_ticks, tp.shift_ticks as usize);
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

    /// Codex-4 regression: a tick that carries `m` through zero must not project the
    /// constraint onto the pre-tick |m| branch — that enforces `d = s·κ·m` on the wrong
    /// side of the cusp, flipping `d` AGAINST the commanded steer for a tick (a yaw
    /// impulse, and ringing if m chatters around zero). Codex's scenario: slow forward
    /// roll, tight detent, strong equal reactions during a shift interruption — the tick
    /// lands m well negative; `d` must stay on the steer's side.
    #[test]
    fn l600_constraint_survives_m_zero_crossing() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let mut st = TransmissionState {
            steer_step: 2,
            shift_ticks: 5,
            ..Default::default()
        };
        let (m0, d0) = (0.100f32, 0.043);
        let inp = input(0.5, 1.0, [m0 + d0, m0 - d0], [250_000.0, 250_000.0]);
        let rep = step(TransmissionMode::FixedRadii, &fp, Some(&tp), &mut st, &inp);
        let m_next = (rep.next_speeds[0] + rep.next_speeds[1]) / 2.0;
        let d_next = (rep.next_speeds[0] - rep.next_speeds[1]) / 2.0;
        assert!(
            m_next < 0.0,
            "the scenario must actually cross zero (m {m0} -> {m_next})"
        );
        assert!(
            d_next > -1e-4,
            "positive steer must not produce a flipped (negative) belt difference across \
             the crossing (d {d0} -> {d_next})"
        );
        // And the landing obeys the constraint on the branch it landed on: d = s·κ·|m|.
        let kappa = tp.steer_kappa[0].0;
        assert!(
            (d_next - kappa * m_next.abs()).abs() < 0.02,
            "the re-solved branch must land ON the geared ratio (d {d_next} vs κ|m| {})",
            kappa * m_next.abs()
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

    /// Codex-2 regression, half 1: the parking brake SETTLES creep instead of freezing it.
    /// The old `B = clamp(R − Q, ±cap)` at a small positive belt speed with `R > Q` set
    /// `v̇ = 0` exactly — positive brake work cancelling grip and drag, preserving creep
    /// forever. The stop-force law lands the belt at zero.
    #[test]
    fn parking_brake_settles_creep() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let mut st = TransmissionState::default();
        // Creep below the latch threshold, zero command, a ground reaction R > Q inside
        // capacity (codex's exact configuration).
        let rep = step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(0.0, 0.0, [0.03, 0.03], [20_000.0, 20_000.0]),
        );
        assert!(st.park, "zero command near standstill must latch the park");
        for v in rep.next_speeds {
            assert!(
                v.abs() < 1e-5,
                "the parked brake must settle creep to zero, not hold it (next = {v})"
            );
        }
    }

    /// Codex-2 regression, half 2: past a capacity breach the latched parking brake stays
    /// SATURATED against the slide — the blend-only envelope faded to zero once the
    /// back-driven belt passed `slip_saturation`, releasing the brake exactly when it was
    /// needed. The latched brake keeps rubbing at `B_max` however fast the belt slides.
    #[test]
    fn parking_brake_stays_saturated_past_breach() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let mut st = TransmissionState::default();
        let r_breach = 1.5 * tp.brake_capacity_n;
        let mut speeds = [0.0f32; 2];
        let mut last = TransmissionReport::default();
        for _ in 0..30 {
            let inp = input(0.0, 0.0, speeds, [r_breach, r_breach]);
            last = step(TransmissionMode::Hybrid, &fp, Some(&tp), &mut st, &inp);
            speeds = last.next_speeds;
        }
        assert!(
            st.park,
            "the latch must not release without a drive command"
        );
        assert!(
            speeds[0] < -fp.slip_saturation,
            "the breach must back-drive the belt well past the blend's fade band \
             (speed = {})",
            speeds[0]
        );
        for side in last.forces {
            assert!(
                side >= tp.brake_capacity_n,
                "sliding past the breach, the sprocket force must still carry the full \
                 saturated brake opposing the slide (got {side}, brake capacity {})",
                tp.brake_capacity_n
            );
        }
    }

    /// Discrete passivity of the whole brake stack: against a brakeless baseline
    /// (`grip_stiffness = 0` disables park/hold; same drag, same drive), the brake's
    /// contribution over one tick never pushes the belt PAST the baseline in its direction
    /// of motion, never reverses it through zero, and never increases |v_next| beyond the
    /// baseline's. Swept over speeds and reactions on both sides of capacity, latched and
    /// unlatched.
    #[test]
    fn brake_is_discretely_passive() {
        let tp = lab_tp();
        let fp_braked = lab_fp();
        let mut fp_free = lab_fp();
        fp_free.grip_stiffness = 0.0;
        for park in [false, true] {
            for v in [-0.6f32, -0.2, -0.03, 0.0, 0.03, 0.2, 0.6] {
                for r in [-1.5f32, -0.5, 0.0, 0.5, 1.5] {
                    let r = r * tp.brake_capacity_n;
                    let inp = input(0.0, 0.0, [v, v], [r, r]);
                    let mut st_b = TransmissionState {
                        park,
                        ..Default::default()
                    };
                    let braked = step(
                        TransmissionMode::Hybrid,
                        &fp_braked,
                        Some(&tp),
                        &mut st_b,
                        &inp,
                    );
                    let mut st_f = TransmissionState {
                        park,
                        ..Default::default()
                    };
                    let free = step(
                        TransmissionMode::Hybrid,
                        &fp_free,
                        Some(&tp),
                        &mut st_f,
                        &inp,
                    );
                    for i in 0..2 {
                        let (b, f) = (braked.next_speeds[i], free.next_speeds[i]);
                        assert!(
                            b.abs() <= f.abs() + 1e-4,
                            "park={park} v={v} R={r}: the brake increased belt speed \
                             (braked {b} vs free {f})"
                        );
                        assert!(
                            b * f >= -1e-6,
                            "park={park} v={v} R={r}: the brake pushed the belt through \
                             zero past the free trajectory (braked {b} vs free {f})"
                        );
                    }
                }
            }
        }
    }

    /// Energy honesty over 64-tick windows: Σ(Q_L·v_L + Q_R·v_R)·dt never exceeds the
    /// integrated engine power available plus released belt-inertia energy — regeneration
    /// recirculates, it does not create (the design's no-free-energy bound). Exercised over
    /// a launch, a driving turn, and a pivot, in both regenerative modes — and, for the
    /// codex-3 split, from an asymmetric rolling start with a hard steer command at gentle
    /// throttle (`F_s ≫ F_p`, `m > d > 0`): the case where one SPROCKET's power is negative
    /// while both MODAL powers read positive, so the modal split never charged η.
    #[test]
    fn energy_bound_no_free_energy() {
        let (fp, tp) = (lab_fp(), lab_tp());
        for (mode, throttle, steer, seed) in [
            (TransmissionMode::Hybrid, 1.0, 0.0, [0.0f32, 0.0]),
            (TransmissionMode::Hybrid, 0.7, 0.6, [0.0, 0.0]),
            (TransmissionMode::Hybrid, 0.0, 1.0, [0.0, 0.0]),
            // Codex-3: steer-dominant at a rolling start — inner sprocket goes negative.
            (TransmissionMode::Hybrid, 0.2, 1.0, [4.0, 2.0]),
            (TransmissionMode::Hybrid, 0.2, -1.0, [4.0, 2.0]),
            (TransmissionMode::FixedRadii, 1.0, 0.0, [0.0, 0.0]),
            (TransmissionMode::FixedRadii, 0.7, 0.8, [0.0, 0.0]),
            (TransmissionMode::FixedRadii, 0.0, 1.0, [0.0, 0.0]),
            (TransmissionMode::FixedRadii, 0.2, 1.0, [4.0, 2.0]),
        ] {
            let mut st = TransmissionState::default();
            let mut speeds = seed;
            let dt_s = 1.0_f64 / 64.0;
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
                    delivered +=
                        f64::from(rep.forces[0] * speeds[0] + rep.forces[1] * speeds[1]) * dt_s;
                    available += f64::from(rep.power_available) * dt_s;
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

    /// Codex-3 pin: the recirculation split reads the PHYSICAL sprocket powers, not the
    /// modal ones. Steer-only at an asymmetric rolling start (`F_p = 0`, saturated `F_s`,
    /// `v_L = 5, v_R = 3`): the outer sprocket delivers `F_s/2·v_L`, the inner ABSORBS
    /// `F_s/2·v_R` — physical net `= F_s/2·(v_L − η·v_R)`, while the modal split reads
    /// `F_s·d` with no negative term at all. The reported power_scale must be the physical
    /// one (and measurably NOT the modal one).
    #[test]
    fn recirculation_splits_physical_output_powers() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let mut st = TransmissionState {
            gear: 5,
            ..Default::default()
        };
        let (vl, vr) = (5.0f32, 3.0);
        let rep = step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(0.0, 1.0, [vl, vr], [0.0, 0.0]),
        );
        // Saturated servo (target far past the band): F_s = 2 × per-output capacity.
        let f_s = 2.0 * tp.steer_capacity_n;
        let (p_l, p_r) = (f_s / 2.0 * vl, -f_s / 2.0 * vr);
        let physical_net = p_l - tp.recirculation * -p_r;
        let expect = rep.power_available / physical_net;
        assert!(
            (rep.power_scale - expect).abs() < 1e-3,
            "power_scale {} must be the physical-output split {expect}",
            rep.power_scale
        );
        let modal = rep.power_available / (f_s * ((vl - vr) / 2.0));
        assert!(
            (rep.power_scale - modal).abs() > 0.02,
            "the physical split must be distinguishable from the modal one here \
             (physical {expect} vs modal {modal}) — otherwise this test pins nothing"
        );
    }

    /// The codex-1 regression (the "cannot decelerate" bug): a forward-moving tank given
    /// full REVERSE throttle must brake monotonically to near standstill (service brakes),
    /// then engage the reverse ladder at the swap seam and actually drive backward — the
    /// old code fed `dir × |throttle|` through the still-forward ladder, producing full
    /// FORWARD force and releasing engine drag: opposite input accelerated the tank.
    #[test]
    fn opposite_throttle_at_speed_brakes_then_reverses() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let mut st = TransmissionState {
            gear: 4,
            ..Default::default()
        };
        let mut speeds = [6.0f32, 6.0];
        let mut m = 6.0f32;
        let mut swapped_at = None;
        for tick in 0..1024 {
            // Reactions zero — the hardest case: the OLD code accelerated forward here.
            let inp = input(-1.0, 0.0, speeds, [0.0, 0.0]);
            let rep = step(TransmissionMode::Hybrid, &fp, Some(&tp), &mut st, &inp);
            let m_next = (rep.next_speeds[0] + rep.next_speeds[1]) / 2.0;
            if swapped_at.is_none() {
                assert!(
                    m_next <= m + 1e-4,
                    "tick {tick}: opposite throttle must never accelerate forward \
                     (m {m} -> {m_next})"
                );
            }
            if st.reverse && swapped_at.is_none() {
                assert!(
                    m.abs() < DIRECTION_SWAP_SPEED,
                    "the reverse ladder must engage only near standstill (m = {m})"
                );
                swapped_at = Some(tick);
            }
            speeds = rep.next_speeds;
            m = m_next;
        }
        assert!(
            swapped_at.is_some(),
            "the held reverse command never engaged the reverse ladder (m = {m})"
        );
        assert!(
            m < -0.5,
            "after the swap the tank must actually drive backward (m = {m})"
        );
    }

    /// Coast intent: zero throttle at speed applies the DECLARED compression-braking drag —
    /// `drag_fraction × peak torque` reflected through the current gear, split per side —
    /// and nothing else (no parking brake at speed, no thrust).
    #[test]
    fn coast_drag_is_declared_fraction_through_current_gear() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let mut st = TransmissionState {
            gear: 3,
            ..Default::default()
        };
        // Mid-band speed for gear 3 (no shift decision interferes).
        let g3 = tp.gears_fwd[2];
        let v = 1_300.0 * RPM_TO_RAD * tp.sprocket_radius / g3;
        let rep = step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(0.0, 0.0, [v, v], [0.0, 0.0]),
        );
        assert_eq!(st.gear, 3, "mid-band coast must not shift");
        let expect = -(tp.peak_torque_nm * tp.drag_fraction * g3 / tp.sprocket_radius) / 2.0;
        for side in rep.forces {
            assert!(
                (side - expect).abs() < 1.0,
                "coasting side force {side} N must be the declared drag share {expect} N"
            );
        }
    }

    /// The pivot-authority convention (the Tiger pivot-dead fix): the steering member
    /// drives the two OUTPUTS differentially, so each output may carry up to the full
    /// PER-OUTPUT capacity (`F_s` bounded by `2 × capacity`, `±capacity` per belt) — not
    /// `±capacity/2`, which halves the yaw moment and left the Tiger under its own
    /// footprint scrub. At rest under full steer the semi-implicit servo asks for the
    /// exact-landing force `2·target·I/dt`, capacity-clamped — each side must carry
    /// `min(capacity, target·I/dt)`, which for the lab's neutral targets is ≥ 98% of the
    /// per-output datum (and EXCEEDS the old difference-axis reading's `capacity/2` ceiling
    /// outright). Checked on both regenerative adapters (the L600 at rest is in its
    /// brake-gated neutral regime, which shares the servo).
    #[test]
    fn pivot_authority_is_per_output_capacity() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let dt = 1.0 / 64.0;
        for (mode, target) in [
            (TransmissionMode::Hybrid, tp.neutral_d_full),
            (
                TransmissionMode::FixedRadii,
                tp.neutral_d_full * tp.neutral_fraction,
            ),
        ] {
            let mut st = TransmissionState::default();
            let rep = step(
                mode,
                &fp,
                Some(&tp),
                &mut st,
                &input(0.0, 1.0, [0.0, 0.0], [0.0, 0.0]),
            );
            let expect = (target * fp.inertia / dt).min(tp.steer_capacity_n);
            assert!(
                (rep.forces[0] - expect).abs() < 1.0,
                "{mode:?}: left output must carry min(capacity, exact-landing) = {expect}, \
                 got {}",
                rep.forces[0]
            );
            assert!(
                (rep.forces[1] + expect).abs() < 1.0,
                "{mode:?}: right output mirrors it (counter-rotation), got {}",
                rep.forces[1]
            );
            assert!(
                expect > 0.9 * tp.steer_capacity_n,
                "the lab targets must exercise near-capacity authority ({expect} vs \
                 {}) — under the old difference-axis reading the ceiling was capacity/2",
                tp.steer_capacity_n
            );
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
