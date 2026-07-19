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
//!   counter-rotation at the DERIVED 1st-gear pivot scale
//!   [`TransmissionParams::neutral_d_full`].
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
//! Signed shaft (stage A): the regenerative adapters measure the geared shaft RELATIVE TO
//! THE ENGAGED LADDER — `shaft = dir·m` with `dir = −1` on the R ladder — so driving
//! normally reads POSITIVE and a BACK-DRIVEN belt (rolling against the engaged gear on a
//! grade) reads NEGATIVE. The SHAFT is signed (rigid gearing); the ENGINE never is — it
//! cannot follow a back-driven shaft, so the operating point floors at the non-negative
//! command-proxy rev floor (the implicit slipping clutch, until the ω_e crank-state arc
//! lands). The old `|m|` read a backslide as high FORWARD rpm, which produced the whole
//! reproduced bug family: the governor cut drive to zero mid-backslide (tank rolls
//! backward on flat ground under full W at "2770 rpm", zero force, indefinitely), the
//! scheduler walked the ladder 1→6 while sliding backward at −2..−3 m/s, and the fix-1a
//! landing gate PASSED catastrophic on-grade upshifts (a predicted backward landing
//! `landing_m = −3.62` read as "9092 rpm" ≥ band + margin).
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
//! | [`NEUTRAL_THROTTLE`], [`NEUTRAL_M_SPEED`] | SIM POLICY | regime-entry thresholds for the L600 neutral turn (the neutral turn's SPEED SCALE — [`TransmissionParams::neutral_d_full`] — is spec-DERIVED); `NEUTRAL_M_SPEED` doubles as the hybrid's blend width into its power-limited pivot regime |
//! | [`POSTSHIFT_MARGIN_RPM`] | SIM POLICY | fix-1a anti-hunting: an upshift must PREDICT landing this far above the down band at the end of its own torque-cut window (the cut bleeds belt speed; the static band gap alone was erased in low gears — the measured 1-2-1-2 climb). Upshifts are also intent-gated (`propulsive > 0`) and L600-detent-deferred so the predictor is only consulted inside its domain (review round). Stage A: the predicted landing SHAFT speed must be POSITIVE on the engaged ladder — a sign-flipped landing always refuses (under `|m|` a backward landing read as high forward rpm and the gate blessed catastrophic on-grade upshifts) |
//! | [`REVERSAL_DWELL_TICKS`] | SIM POLICY | fix-1b anti-hunting: a committed shift blocks the OPPOSITE-direction shift for this many ticks AFTER its interruption window (the dwell counts only outside the frozen window — review round); same-direction climbs stay free |
//! | [`OVERREV_MARGIN_RPM`] | SIM POLICY | fix-1c: a downshift must land at least this far under the engine's max curve rpm — the box never commands an over-rev |
//! | [`WIDE_ON`]/[`WIDE_OFF`]/[`TIGHT_ON`]/[`TIGHT_OFF`] | SIM POLICY | stick-to-detent input mapping with hysteresis; the DETENT RATIOS they select are spec |
//! | [`TICK_HZ`] | SIM POLICY | the fixed simulation tick the shift countdown quantizes against |
//!
//! Moved OUT of this module to the spec (they were vehicle data wearing const clothing):
//! shift time (`gearbox.shift_secs` — a Tiger preselector and a T-34 crash box differ),
//! engine drag (`engine.drag_fraction` — an engine datum). REMOVED rather than classified:
//! `STEER_SERVO_BAND` — the steering servo is now the semi-implicit exact law (like the
//! brakes and λ), so no proportional band exists to tune; its droop was itself a
//! vehicle-scaling bug (the Tiger's neutral target sat inside the band). Also REMOVED:
//! `neutral_fraction` (spec field DELETED, fix 3 of the correctness batch) — an
//! unprovenanced authored feel scalar; the DERIVED `neutral_d_full = κ_tight(F1) ×
//! v1_governed` is itself the correct emergent pivot scale for a fixed-radius box (the
//! radii table's own invariant: `κ_tight(g) × v(g)` is gear-independent). Everything else
//! the vehicle authors was already spec: torque curve, ladders, radii, capacities, brake, η.

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

/// Fix-1a anti-hunting margin (rpm): an upshift only commits if the belt state PREDICTED at
/// the end of the shift's own torque-cut window (same integration law, drive torque cut,
/// reaction frozen — [`predict_shift_landing_m`]) lands at least this far ABOVE the down
/// band in the new gear. The static band gap (~100 rpm at the Tiger's widest step) is
/// erased by the cut's own belt-speed bleed in low gears (~2500 rpm per m/s slope in gear
/// 2), which fired the down band the tick the freeze lifted — the measured 1-2-1-2 climb
/// trace. SIM POLICY.
const POSTSHIFT_MARGIN_RPM: f32 = 150.0;

/// Fix-1b anti-hunting dwell (fixed ticks, 0.5 s at 64 Hz): after a shift commits, the
/// OPPOSITE-direction shift stays blocked this long. Same-direction shifts stay free — a
/// rapid 1-2-3 climb must not slow down. SIM POLICY.
const REVERSAL_DWELL_TICKS: u8 = 32;

/// Fix-1c over-rev margin (rpm): a downshift is refused if its landing rpm in the lower
/// gear would exceed the engine's max authored curve rpm minus this margin — the box never
/// commands an over-rev. SIM POLICY.
const OVERREV_MARGIN_RPM: f32 = 100.0;

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
    /// the L600's brake-gated neutral-turn target. DERIVED, and the correct emergent pivot
    /// scale for a fixed-radius box: the radii table's own invariant makes `κ_tight(g) ×
    /// v(g)` gear-independent (Tiger: ≈ 0.337 m/s @ 3000 rpm in every gear). The authored
    /// `neutral_fraction` feel scalar that used to shrink it was DELETED (fix 3 — no
    /// provenance). The hybrid does not read this: its standstill pivot is POWER-limited
    /// (fix 2), not speed-targeted.
    pub neutral_d_full: f32,
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

    /// The engine's max authored curve rpm (the last torque point — the curve is authored
    /// ascending): the ceiling the fix-1c over-rev gate measures downshift landings against.
    pub fn max_curve_rpm(&self) -> f32 {
        self.engine.torque_nm[self.engine.torque_nm.len() - 1].0
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
    /// Direction of the last committed gear shift (+1 up, −1 down, 0 none) — the axis the
    /// fix-1b reversal dwell blocks against. Local scheduler memory (REV 13: not
    /// replicated, not hashed), reset by a ladder swap.
    pub last_shift_dir: i8,
    /// Remaining ticks of the fix-1b reversal dwell: while non-zero, the shift OPPOSITE to
    /// `last_shift_dir` stays blocked (same-direction shifts stay free).
    pub dwell_ticks: u8,
}

impl Default for TransmissionState {
    fn default() -> Self {
        Self {
            gear: 1,
            shift_ticks: 0,
            steer_step: 0,
            reverse: false,
            park: false,
            last_shift_dir: 0,
            dwell_ticks: 0,
        }
    }
}

/// A compact operating-point readout of the joint drivetrain — the ONE place the HUD/legend
/// reads gear and rpm from, so the display never re-derives drivetrain math (the gear/rpm
/// relation lives here, beside the adapter that integrates on it).
#[derive(Clone, Debug, PartialEq)]
pub struct DriveReadout {
    /// Engine rpm at the engaged gear from the MEAN belt speed, floored at idle — the geared
    /// crank speed the regenerative adapter reads before its command-dependent rev floor (a
    /// state-only readout cannot know the launch throttle, so it reports the geared point).
    pub rpm: f32,
    /// The engaged gear as a display label: `F1..Fn` forward, `R1..Rn` reverse.
    pub gear_label: String,
}

/// Read the drivetrain operating point THROUGH THE LAW: the engaged gear from [`TransmissionState`]
/// against the active ladder, and the geared engine rpm from the pre-tick belt speeds — the same
/// SIGNED-shaft relation [`regenerative`] runs (stage A: `rpm = max(dir·m·G / r_s, idle)`; a
/// back-driven shaft reads IDLE, because the engine cannot follow it — the old `|m|` read a
/// backslide as high forward rpm, the "F1 @ 2770 rpm" display lie). Pure (no ECS), so the HUD
/// and any legend share one implementation.
pub fn readout(st: &TransmissionState, tp: &TransmissionParams, speeds: [f32; 2]) -> DriveReadout {
    let m = (speeds[0] + speeds[1]) / 2.0;
    let ladder: &[f32] = if st.reverse {
        &tp.gears_rev
    } else {
        &tp.gears_fwd
    };
    let top = ladder.len() as u8;
    let gear = st.gear.clamp(1, top);
    let g = ladder[(gear - 1) as usize];
    let dir = if st.reverse { -1.0 } else { 1.0 };
    let rpm = (dir * m * g / tp.sprocket_radius / RPM_TO_RAD).max(tp.engine.idle_rpm);
    DriveReadout {
        rpm,
        gear_label: format!("{}{gear}", if st.reverse { 'R' } else { 'F' }),
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

/// The zero-throttle engine-drag force (compression braking) reflected through gear `g` at
/// mean-axis belt speed `m` (design §3): a drag TORQUE, saturating over [`DRAG_SAT_SPEED`]
/// and released as the PROPULSIVE throttle opens. ONE implementation shared verbatim
/// between the live tick and the fix-1a shift predictor, so prediction and reality run the
/// same law.
///
/// Sign audit (stage A): `m/DRAG_SAT_SPEED` is already SIGNED, so under a backslide
/// (`m < 0` in a forward gear) the drag comes out POSITIVE — opposing the actual belt
/// motion, as compression braking must. Correct as-is; deliberately untouched.
fn reflected_drag(tp: &TransmissionParams, g: f32, m: f32, propulsive: f32) -> f32 {
    -(tp.peak_torque_nm * tp.drag_fraction * g / tp.sprocket_radius)
        * (m / DRAG_SAT_SPEED).clamp(-1.0, 1.0)
        * forces::hold_blend(propulsive / DRAG_THROTTLE_RELEASE)
}

/// Fix-1a: roll the shift's torque-cut window forward on the mean belt axis and return the
/// PREDICTED landing speed `m`. Exactly the live integration's mean — per side
/// `v += (Q − R)/I·dt` with `Q = (f_p + f_drag)/2` — under the shift window's own rules:
/// drive torque cut (`f_p = 0`), engine drag through the LANDING gear (the live tick's `g`
/// is already the new gear during the window), and the ground reaction frozen at its
/// current per-tick mean. Fixed-tick, f32-exact.
///
/// DOMAIN (review round): valid for the PROPULSIVE straight-line case ONLY — the only case
/// the scheduler consults it for (upshifts are intent-gated on `propulsive > 0` and
/// detent-deferred on the L600). It integrates drag but carries no brake term and no
/// λ/steer state, so under service braking or a geared turn it would over-predict the
/// landing. Inside its domain, frozen-R is CONSERVATIVE (the true post-cut reaction
/// collapses with the slip), and the single mean-axis clamp is an accepted approximation
/// of the live per-side clamps.
fn predict_shift_landing_m(
    tp: &TransmissionParams,
    fp: &ForceParams,
    g_landing: f32,
    propulsive: f32,
    m: f32,
    r_mean: f32,
    dt: f32,
) -> f32 {
    let mut pm = m;
    for _ in 0..tp.shift_ticks {
        let f_drag = reflected_drag(tp, g_landing, pm, propulsive);
        pm = (pm + (f_drag / 2.0 - r_mean) / fp.inertia * dt).clamp(-fp.max_speed, fp.max_speed);
    }
    pm
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
            // A ladder swap is not an up/down shift: the reversal dwell restarts clean.
            st.last_shift_dir = 0;
            st.dwell_ticks = 0;
        }
    }
    let ladder: &[f32] = if st.reverse {
        &tp.gears_rev
    } else {
        &tp.gears_fwd
    };
    let top = ladder.len() as u8;
    st.gear = st.gear.clamp(1, top);

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

    // --- Auto-shift on engine-rpm bands, hysteresis from the band gap; a shift in flight
    // blocks further decisions until its interruption window has elapsed. Three SIM-POLICY
    // gates (the fix-1 anti-hunting batch) kill the shift-cut oscillation the static bands
    // alone could not — the cut's own belt-speed bleed erased the ~100 rpm band margin in
    // low gears (measured full-throttle climb trace: 1-2-1-2-1-2-3-2-…):
    //   a) upshifts are CONSIDERED only under propulsive drive AND (for the L600) with the
    //      steering detent released — the predictor-domain gates below — and must PREDICT
    //      a landing rpm ≥ down band + POSTSHIFT_MARGIN_RPM at the END of the torque-cut
    //      window ([`predict_shift_landing_m`] — the same integration the window itself
    //      runs);
    //   b) a committed shift blocks the OPPOSITE-direction shift for REVERSAL_DWELL_TICKS
    //      (same-direction climbs stay free);
    //   c) downshifts must land under the engine's max curve rpm − OVERREV_MARGIN_RPM.
    //
    // Stage A (signed shaft): the shaft speed is defined RELATIVE TO THE ENGAGED LADDER,
    // `shaft = dir·m` — rigid gearing has a sign. Driving normally shaft > 0; back-driven
    // (a grade rolling the tank against the engaged gear) shaft < 0, and its geared rpm is
    // NEGATIVE. The old `|m|` read a backslide as high forward rpm, which walked the
    // ladder upward mid-slide and (via the landing predictor) blessed sign-flipped
    // landings — see the module doc's stage-A paragraph for the reproduced trio.
    let shaft = dir * m;
    let shaft_rpm_of = |sh: f32, g: f32| sh * g / tp.sprocket_radius / RPM_TO_RAD;
    let shaft_rpm_geared = |g: f32| shaft_rpm_of(shaft, g);
    // Predictor-domain guard (review round): while the L600 detent is engaged the
    // constraint force λ loads the outputs in a way the predictor cannot model (it carries
    // no λ/steer state), so its landing prediction is invalid mid-geared-turn — DEFER
    // upshifts until the detent releases. Downshifts stay allowed (the over-rev gate still
    // applies). The broader "hold gear during any turn" UX rule is a separate pending
    // design decision, deliberately NOT implemented here.
    let detent_turn = mode == TransmissionMode::FixedRadii && st.steer_step != 0;
    if st.shift_ticks == 0 {
        // SIGNED shaft rpm (stage A): while back-driven this is negative, so the up band
        // can never fire mid-backslide (negative never exceeds the band).
        let rpm = shaft_rpm_geared(ladder[(st.gear - 1) as usize]);
        let dwell_blocks = |shift_dir: i8| st.dwell_ticks > 0 && st.last_shift_dir == -shift_dir;
        // Intent gate (review round): an upshift is only ever WANTED while actually
        // driving (`propulsive > 0`) — a braking or coasting driver never needs one — and
        // only there is the predictor inside its domain (it integrates drag but no brake
        // term; under service braking it over-predicted the landing by ~400 rpm: F7 @
        // 2500 rpm + full opposing throttle predicted 1652 on drag alone while the live
        // window with the brakes landed at 1262, below the down band → a false shift +
        // reversal cycle).
        if rpm > tp.shift_up_rpm
            && st.gear < top
            && propulsive > 0.0
            && !detent_turn
            && !dwell_blocks(1)
        {
            let g_up = ladder[st.gear as usize];
            let r_mean = (inp.reactions[0] + inp.reactions[1]) / 2.0;
            // The predictor returns a SIGNED m; the gate reads its SIGNED shaft speed
            // (stage A): the landing must be POSITIVE on the engaged ladder AND clear the
            // down band + margin. A sign-flipped landing always refuses the upshift — under
            // `|m|` the traced grade case (r_mean = 221 kN, landing_m = −3.62) read as
            // "9092 rpm" and PASSED, committing catastrophic on-grade upshifts. No
            // at-rest threshold is needed HERE (review round): the rpm bound already
            // demands a landing ≥ down band + margin — solidly positive, far above any
            // numerical residual — so the sign check is a belt-and-braces refusal.
            let landing = predict_shift_landing_m(tp, fp, g_up, propulsive, m, r_mean, dt);
            let landing_shaft = dir * landing;
            if landing_shaft > 0.0
                && shaft_rpm_of(landing_shaft, g_up) >= tp.shift_down_rpm + POSTSHIFT_MARGIN_RPM
            {
                st.gear += 1;
                st.shift_ticks = tp.shift_ticks;
                st.last_shift_dir = 1;
                st.dwell_ticks = REVERSAL_DWELL_TICKS;
            }
        } else if shaft > -PARK_ENGAGE_SPEED
            && rpm < tp.shift_down_rpm
            && st.gear > 1
            && !dwell_blocks(-1)
        {
            // Backslide hold (stage A, thresholded in the review round): while GENUINELY
            // back-driven the vehicle is NOT "running slow forward" — gear changes are
            // decisions about forward operation, and the backslide state HOLDS the engaged
            // gear (no downshift walk on a slide either; the negative signed rpm would
            // otherwise sit permanently under the down band). The threshold is
            // −PARK_ENGAGE_SPEED, the existing at-rest policy scale, NOT exact zero: the
            // brake stop-force/integration order leaves a stable numerical residual at
            // rest (measured ≈ −1.7e−9 m/s coasting to a stop in gear 3 against a 20 kN
            // reaction), and a hard `shaft >= 0` stranded the box in its cruise gear
            // forever. A residual orders of magnitude below the threshold downshifts
            // normally; a real slide (−0.5 m/s and beyond) still holds.
            let g_down = ladder[(st.gear - 2) as usize];
            if shaft_rpm_geared(g_down) <= tp.max_curve_rpm() - OVERREV_MARGIN_RPM {
                st.gear -= 1;
                st.shift_ticks = tp.shift_ticks;
                st.last_shift_dir = -1;
                st.dwell_ticks = REVERSAL_DWELL_TICKS;
            }
        }
    }
    // The dwell counts only OUTSIDE the interruption window (review round): the frozen
    // window blocks all decisions anyway, so draining the dwell inside it left only ~12
    // effective post-engagement ticks of the promised 32.
    if st.shift_ticks == 0 && st.dwell_ticks > 0 {
        st.dwell_ticks -= 1;
    }
    let shifting = st.shift_ticks > 0;
    if shifting {
        st.shift_ticks -= 1;
    }
    let g = ladder[(st.gear - 1) as usize];

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
    //
    // Stage A: the torque lookup evaluates at a NON-NEGATIVE engine rpm —
    // `engine_rpm = max(shaft_rpm, rpm_floor)` with the SIGNED shaft rpm. The engine is
    // never negative: it cannot follow a back-driven shaft (the rev floor is the implicit
    // slipping-clutch stand-in until the ω_e crank arc lands), so during a backslide
    // (shaft < 0) the engine keeps delivering FORWARD drive at the floored rpm (`f_p`
    // below follows the engaged direction via `dir·propulsive·…`). The governor cut inside
    // `torque_at` therefore applies only to real forward over-speed — never to `|shaft|`,
    // which is what cut drive to zero mid-backslide ("2770 rpm", zero force, rolling
    // backward under full W).
    let cmd_mag = propulsive.max(inp.steer.abs()).clamp(0.0, 1.0);
    let rpm_floor = tp.engine.idle_rpm + (tp.peak_torque_rpm - tp.engine.idle_rpm) * cmd_mag;
    let rpm = shaft_rpm_geared(g).max(rpm_floor);
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
    let f_drag = reflected_drag(tp, g, m, propulsive);

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
            // the design's "strong turn-in, then physically required speed loss").
            //
            // At m → 0 the SAME doctrine holds (fix 2): the hydrostatic family's pivot is
            // limited by the POWER budget, not by a speed target — the old neutral_d_full
            // FLOOR was a kinematic speed command that left the engine at ~1/6 of its
            // budget and pivoted at 0.131 rad/s. Standing still, the box commands steer
            // FORCE up to the capacity bound (steer-proportional, the per-output
            // convention's ±2×capacity on the difference axis) and the power-conservation
            // scale below is the binding limiter, so the pivot rate settles where engine
            // power balances scrub dissipation. The blend weight is continuous in BOTH
            // regime axes — `hold_blend` over |m| (NEUTRAL_M_SPEED) × |steer| — so no
            // one-tick force jump crosses either seam, and steer → 0 continuously returns
            // the whole force to the curvature servo, whose target is then 0: releasing
            // the stick actively ARRESTS the belt difference (review round: weighting on
            // |m| alone zeroed both terms at steer = 0 and left an airborne pivot
            // counter-rotating forever).
            let k_full = tp.steer_kappa[0].0;
            if inp.steer != 0.0 || d != 0.0 {
                let servo_f = servo(inp.steer.signum() * (inp.steer.abs() * k_full * m.abs()));
                let pivot_f = inp.steer * f_s_max;
                let w = forces::hold_blend(m.abs() / NEUTRAL_M_SPEED) * inp.steer.abs().min(1.0);
                f_s = servo_f + (pivot_f - servo_f) * w;
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
                // The marginal brake-gated neutral turn: a slow capacity-limited servo
                // toward the DERIVED pivot scale `neutral_d_full = κ_tight(F1) ×
                // v1_governed` — the radii table's own gear-independent invariant (fix 3
                // deleted the unprovenanced `neutral_fraction` that used to shrink it).
                f_s = servo(inp.steer * tp.neutral_d_full);
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
        // 1780 rpm: comfortably past the band AND past the fix-1a landing gate (unloaded
        // landing 1780 × 8/12.7 ≈ 1121 rpm ≥ down band 950 + margin 150).
        let v = m_for(1_780.0);
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

        // Drain the window AND the fix-1b reversal dwell at a mid-band speed for gear 2:
        // no hunting either way.
        let g2 = tp.gears_fwd[1];
        let v_mid = 1_300.0 * RPM_TO_RAD * tp.sprocket_radius / g2;
        for _ in 0..(tp.shift_ticks as usize + REVERSAL_DWELL_TICKS as usize + 5) {
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
        // 1780 rpm — past the up band and the fix-1a landing gate (see gear_shift_hysteresis).
        let v = 1_780.0 * RPM_TO_RAD * tp.sprocket_radius / g1;
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

    /// Fix-1a: the upshift commits only if the belt state PREDICTED at the end of the
    /// torque-cut window still lands above the down band + POSTSHIFT_MARGIN_RPM. Same
    /// operating point, two loads: unloaded the landing holds and the shift engages; under
    /// a heavy frozen reaction (25 kN/side) the cut would bleed ≈ 0.98 m/s
    /// (25 kN / 8 t × 20 ticks / 64 Hz) and land deep inside the down band — the shift
    /// must be refused. Pre-fix, exactly this bleed fired the down band the tick the
    /// freeze lifted: the measured 1-2-1-2 climb.
    #[test]
    fn upshift_landing_gate_blocks_shift_cut_hunting() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let g1 = tp.gears_fwd[0];
        let v = 1_780.0 * RPM_TO_RAD * tp.sprocket_radius / g1;
        let mut st = TransmissionState::default();
        step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [v, v], [25_000.0, 25_000.0]),
        );
        assert_eq!(
            st.gear, 1,
            "a landing predicted inside the down band must refuse the upshift"
        );
        let mut st = TransmissionState::default();
        step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [v, v], [0.0, 0.0]),
        );
        assert_eq!(st.gear, 2, "unloaded, the same operating point upshifts");
    }

    /// Stage A (signed shaft): a belt BACK-DRIVEN in a forward gear (m < 0, W held — the
    /// backslide) commits NO shifts in either direction, and the engine keeps delivering
    /// FORWARD drive (the governor must not cut). Pre-fix, `|m| = 2.5` in gear 1 read as
    /// 2025 rpm: past the up band (ladder walk while sliding backward) AND past the
    /// governed cut (torque → 0, so the tank back-slid under full W indefinitely). The
    /// signed shaft reads −2025 rpm: the up band can never fire, the down band is held
    /// (a backslide is not "running slow forward"), and the engine evaluates at the
    /// non-negative rev floor, delivering forward force.
    #[test]
    fn backslide_holds_gear_and_keeps_forward_drive() {
        let (fp, tp) = (lab_fp(), lab_tp());
        // Up-band side: gear 1 at m = −2.5 under a grade-like reaction.
        let mut st = TransmissionState::default();
        for tick in 0..96 {
            let rep = step(
                TransmissionMode::Hybrid,
                &fp,
                Some(&tp),
                &mut st,
                &input(1.0, 0.0, [-2.5, -2.5], [40_000.0, 40_000.0]),
            );
            assert_eq!(
                st.gear, 1,
                "tick {tick}: a backslide must not walk the ladder"
            );
            assert_eq!(
                st.shift_ticks, 0,
                "tick {tick}: no shift may commit during a backslide"
            );
            assert!(
                rep.forces[0] > 0.0 && rep.forces[1] > 0.0,
                "tick {tick}: the engine must keep delivering FORWARD drive during a \
                 backslide — the governor must not cut on |shaft| (forces {:?})",
                rep.forces
            );
        }
        // Down-band side: gear 3 back-driven — the signed rpm sits under the down band,
        // but the backslide state HOLDS the engaged gear (no downshift walk either).
        let mut st = TransmissionState {
            gear: 3,
            ..Default::default()
        };
        step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [-2.5, -2.5], [40_000.0, 40_000.0]),
        );
        assert_eq!(
            st.gear, 3,
            "a backslide must hold the engaged gear, not downshift-walk"
        );
        assert_eq!(st.shift_ticks, 0);
    }

    /// Stage A (signed landing gate): an upshift whose PREDICTED landing is sign-flipped
    /// (backward) is always refused. The traced grade case: at 1780 rpm in gear 1 under a
    /// frozen r_mean = 221 kN, the torque-cut window bleeds 221 kN / 8 t × 0.3125 s ≈
    /// 8.6 m/s — landing ≈ −6.4 m/s, BACKWARD. Under `|m|` that read as ≈ 3280 rpm ≥
    /// band + margin and the gate PASSED the catastrophic on-grade upshift; the signed
    /// gate requires a POSITIVE landing shaft.
    #[test]
    fn landing_gate_refuses_sign_flipped_landing() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let g1 = tp.gears_fwd[0];
        let v = 1_780.0 * RPM_TO_RAD * tp.sprocket_radius / g1; // above the up band
        let mut st = TransmissionState::default();
        step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [v, v], [221_000.0, 221_000.0]),
        );
        assert_eq!(
            st.gear, 1,
            "a sign-flipped predicted landing must refuse the upshift"
        );
        assert_eq!(st.shift_ticks, 0, "no shift may have committed");
    }

    /// Stage-A review round: the REVERSE-ladder mirror of the backslide test. Driving in
    /// R (dir = −1) while back-driven FORWARD (m > 0 → shaft = dir·m < 0): no shifts in
    /// either direction, and the drive force stays R-SIGNED and non-zero (the governor
    /// must not cut on |shaft| — pre-fix, |m| = 2.5 in R1 read 2025 rpm, past the
    /// governed cut, torque → 0). Uses a 3-gear reverse ladder so "no shifts" actually
    /// has shifts to refuse.
    #[test]
    fn reverse_backslide_holds_gear_and_keeps_reverse_drive() {
        let fp = lab_fp();
        let tp = TransmissionParams::from_authoring(&TransmissionAuthoring {
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
            reverse_speeds_kmh: &[8.0, 12.7, 20.4],
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
            recirculation: 0.9,
            brake_capacity_n: 120_000.0,
            drag_fraction: 0.25,
            shift_secs: 0.31,
            sprocket_radius_m: 0.34,
            half_tread_m: 1.25,
        });
        // Up-band mirror: R1 back-driven at m = +2.5 (|m| would read 2025 rpm — ladder
        // walk + governed cut pre-fix). Held S (reverse throttle), grade-like reaction.
        let mut st = TransmissionState {
            reverse: true,
            ..Default::default()
        };
        for tick in 0..96 {
            let rep = step(
                TransmissionMode::Hybrid,
                &fp,
                Some(&tp),
                &mut st,
                &input(-1.0, 0.0, [2.5, 2.5], [-40_000.0, -40_000.0]),
            );
            assert!(st.reverse, "tick {tick}: the R ladder stays engaged");
            assert_eq!(
                st.gear, 1,
                "tick {tick}: a reverse backslide must not walk the R ladder"
            );
            assert_eq!(st.shift_ticks, 0, "tick {tick}: no shift may commit");
            assert!(
                rep.forces[0] < 0.0 && rep.forces[1] < 0.0,
                "tick {tick}: the engine must keep delivering R-SIGNED drive during a \
                 reverse backslide (forces {:?})",
                rep.forces
            );
        }
        // Down-band mirror: R2 back-driven slowly forward (shaft = −0.3, a genuine slide
        // past the at-rest threshold) — the backslide state holds the engaged gear.
        let mut st = TransmissionState {
            gear: 2,
            reverse: true,
            ..Default::default()
        };
        step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(-1.0, 0.0, [0.3, 0.3], [0.0, 0.0]),
        );
        assert_eq!(
            st.gear, 2,
            "a reverse backslide must hold the engaged gear, not downshift-walk"
        );
        assert_eq!(st.shift_ticks, 0);
    }

    /// Stage-A review round: the HUD readout shares the signed-shaft convention in
    /// REVERSE. Driving in R (m < 0) reads a positive geared rpm and the R label;
    /// back-driven forward while in R, the shaft is negative — the engine cannot follow
    /// it, so the readout floors at idle instead of showing a fake forward rpm.
    #[test]
    fn readout_reverse_reads_signed_shaft() {
        let tp = lab_tp();
        let st = TransmissionState {
            reverse: true,
            ..Default::default()
        };
        // Driving in reverse: shaft = dir·m = +2.0 → geared rpm, positive, above idle.
        let r = readout(&st, &tp, [-2.0, -2.0]);
        assert_eq!(r.gear_label, "R1");
        let expect = 2.0 * tp.gears_rev[0] / tp.sprocket_radius / RPM_TO_RAD;
        assert!(
            (r.rpm - expect).abs() < 0.5 && r.rpm > tp.engine.idle_rpm,
            "reversing must read the geared rpm ({expect:.0}), got {:.0}",
            r.rpm
        );
        // Back-driven while in R (rolling forward): shaft < 0 → idle, not a fake rpm.
        let r = readout(&st, &tp, [2.0, 2.0]);
        assert_eq!(r.gear_label, "R1");
        assert_eq!(
            r.rpm, tp.engine.idle_rpm,
            "a back-driven R shaft must read idle (the engine cannot follow it)"
        );
    }

    /// Stage-A review round (FIX 1 regression): coasting to rest in a cruise gear must
    /// complete the downshift chain to gear 1. The brake stop-force/integration order
    /// leaves a stable numerical residual at rest (measured ≈ −1.7e−9 m/s: Hybrid, gear
    /// 3, zero command, 20 kN/side reaction) — a hard `shaft >= 0` backslide guard read
    /// that residual as "back-driven" and stranded the box in gear 3 forever. The guard's
    /// −PARK_ENGAGE_SPEED threshold lets numerical rest downshift normally.
    #[test]
    fn coast_to_rest_completes_downshift_chain() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let mut st = TransmissionState {
            gear: 3,
            ..Default::default()
        };
        let mut speeds = [-1.0e-5f32, -1.0e-5];
        for _ in 0..256 {
            let rep = step(
                TransmissionMode::Hybrid,
                &fp,
                Some(&tp),
                &mut st,
                &input(0.0, 0.0, speeds, [20_000.0, 20_000.0]),
            );
            speeds = rep.next_speeds;
        }
        assert!(
            speeds[0].abs() < PARK_ENGAGE_SPEED && speeds[1].abs() < PARK_ENGAGE_SPEED,
            "the scenario must actually be at (numerical) rest, got {speeds:?}"
        );
        assert!(st.park, "zero command at rest must have latched the park");
        assert_eq!(
            st.gear, 1,
            "coasting to rest must complete the downshift chain, not strand the cruise \
             gear behind the backslide guard"
        );
    }

    /// Fix-1b: after a shift commits, the OPPOSITE-direction shift is dwell-blocked for
    /// REVERSAL_DWELL_TICKS, but SAME-direction shifts stay free (a rapid 1-2-3 climb must
    /// not slow down).
    #[test]
    fn dwell_blocks_reversal_not_same_direction() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let rpm_v = |rpm: f32, g: f32| rpm * RPM_TO_RAD * tp.sprocket_radius / g;
        let mut st = TransmissionState::default();
        let at = |st: &mut TransmissionState, v: f32| {
            step(
                TransmissionMode::Hybrid,
                &fp,
                Some(&tp),
                st,
                &input(1.0, 0.0, [v, v], [0.0, 0.0]),
            );
        };
        // 1 → 2 commits (dwell armed).
        at(&mut st, rpm_v(1_780.0, tp.gears_fwd[0]));
        assert_eq!(st.gear, 2);
        // Drain the interruption window at a mid-band gear-2 speed; the dwell (32 ticks)
        // must still be live when the window (≈ 20 ticks) ends, or this test bites nothing.
        for _ in 0..tp.shift_ticks {
            at(&mut st, rpm_v(1_300.0, tp.gears_fwd[1]));
        }
        assert_eq!(st.gear, 2);
        assert!(st.dwell_ticks > 0, "the dwell must outlive the window");
        // SAME direction: 2 → 3 engages immediately despite the live dwell (1780 rpm is
        // past the up band, landing 1780 × 12.7/20.4 ≈ 1108 ≥ 1100 clears the fix-1a gate).
        at(&mut st, rpm_v(1_780.0, tp.gears_fwd[1]));
        assert_eq!(
            st.gear, 3,
            "same-direction shifts must not be dwell-blocked"
        );
        // OPPOSITE direction: drop below gear-3's down band. The downshift must wait out
        // the FULL dwell after the window — the dwell counts only outside the frozen
        // window (review round), so the reversal engages exactly at
        // window + REVERSAL_DWELL_TICKS.
        let v_low = rpm_v(900.0, tp.gears_fwd[2]);
        let mut ticks = 0usize;
        while st.gear == 3 {
            at(&mut st, v_low);
            ticks += 1;
            assert!(ticks < 200, "the downshift must eventually engage");
        }
        assert_eq!(
            ticks,
            tp.shift_ticks as usize + REVERSAL_DWELL_TICKS as usize,
            "the reversal must get the full post-engagement dwell (window {} + dwell {})",
            tp.shift_ticks,
            REVERSAL_DWELL_TICKS
        );
    }

    /// Review round (intent gate): upshifts are considered only under PROPULSIVE drive. A
    /// braking (opposing-throttle) or coasting driver at high rpm never needs one — and
    /// the landing predictor has no brake term, so consulting it there produced a false
    /// shift (predicted 1652 rpm on drag alone vs 1262 real under the brakes) followed by
    /// a reversal cycle.
    #[test]
    fn no_upshift_while_braking_or_coasting() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let g1 = tp.gears_fwd[0];
        let v = 1_780.0 * RPM_TO_RAD * tp.sprocket_radius / g1;
        for throttle in [0.0, -1.0] {
            let mut st = TransmissionState::default();
            step(
                TransmissionMode::Hybrid,
                &fp,
                Some(&tp),
                &mut st,
                &input(throttle, 0.0, [v, v], [0.0, 0.0]),
            );
            assert_eq!(
                st.gear, 1,
                "throttle {throttle}: no upshift without propulsive drive"
            );
            assert_eq!(st.shift_ticks, 0, "throttle {throttle}: no shift committed");
        }
    }

    /// Review round (predictor-domain guard): while the L600 steering detent is engaged
    /// the landing predictor has no λ/steer state, so upshifts are DEFERRED until the
    /// detent releases; downshifts stay allowed mid-turn.
    #[test]
    fn l600_detent_defers_upshift() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let v_up = 1_780.0 * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[0];
        // Detent engaged (tight) at an above-band operating point: upshift deferred.
        let mut st = TransmissionState {
            steer_step: 2,
            ..Default::default()
        };
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 1.0, [v_up, v_up], [0.0, 0.0]),
        );
        assert_eq!(st.gear, 1, "detent-active upshift must be deferred");
        // Same operating point, detent released: the upshift proceeds — it is the detent
        // that defers, not the operating point.
        let mut st = TransmissionState::default();
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [v_up, v_up], [0.0, 0.0]),
        );
        assert_eq!(st.gear, 2, "detent released, the upshift proceeds");
        // Downshifts stay allowed mid-turn (over-rev gate permitting).
        let v_low = 900.0 * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[2];
        let mut st = TransmissionState {
            gear: 3,
            steer_step: 2,
            ..Default::default()
        };
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 1.0, [v_low, v_low], [0.0, 0.0]),
        );
        assert_eq!(st.gear, 2, "downshifts stay allowed during a detent turn");
    }

    /// Review round (fix B): releasing the steer at a standstill pivot must actively
    /// ARREST the belt difference — with zero ground reactions (airborne), only the servo
    /// can. The |m|-only blend weight zeroed both force terms at steer = 0 (w = 1,
    /// pivot_f = 0), leaving the belts counter-rotating forever; the steer-scaled weight
    /// returns the released stick to the curvature servo, whose target is 0.
    #[test]
    fn hybrid_steer_release_arrests_pivot() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let mut st = TransmissionState::default();
        let mut speeds = [0.0f32; 2];
        // Spin up a standstill pivot (zero reactions — the worst case: nothing external
        // ever damps the belts).
        for _ in 0..64 {
            let rep = step(
                TransmissionMode::Hybrid,
                &fp,
                Some(&tp),
                &mut st,
                &input(0.0, 1.0, speeds, [0.0, 0.0]),
            );
            speeds = rep.next_speeds;
        }
        let d0 = (speeds[0] - speeds[1]) / 2.0;
        assert!(d0 > 0.1, "the pivot must actually be turning (d = {d0})");
        // Release the steer: d must decay to ~0 within a bounded window.
        for _ in 0..32 {
            let rep = step(
                TransmissionMode::Hybrid,
                &fp,
                Some(&tp),
                &mut st,
                &input(0.0, 0.0, speeds, [0.0, 0.0]),
            );
            speeds = rep.next_speeds;
        }
        let d1 = (speeds[0] - speeds[1]) / 2.0;
        assert!(
            d1.abs() < 0.01,
            "released steer must arrest the pivot (d {d0} -> {d1})"
        );
    }

    /// Fix-1c: a downshift whose landing rpm would exceed the engine's max curve rpm minus
    /// OVERREV_MARGIN_RPM is refused. Custom two-gear ladder with a 2.55 ratio step (a
    /// shape the spec-level hysteresis validation would reject — deliberately extreme to
    /// make the gate the ONLY thing standing between the down band and a 2295-rpm landing
    /// on an 1800-rpm curve): at 900 rpm in gear 2 (below the 950 down band) the landing
    /// in gear 1 ≈ 2295 > 1800 − 100 → refused; at 600 rpm the landing ≈ 1530 is inside
    /// the envelope and the downshift proceeds.
    #[test]
    fn overrev_gate_refuses_too_early_downshift() {
        let fp = lab_fp();
        let tp = TransmissionParams::from_authoring(&TransmissionAuthoring {
            idle_rpm: 600.0,
            governed_rpm: 1800.0,
            rated_rpm: 1800.0,
            torque_nm: &[
                (600.0, 1650.0),
                (1100.0, 2200.0),
                (1700.0, 1950.0),
                (1800.0, 0.0),
            ],
            forward_speeds_kmh: &[8.0, 20.4],
            reverse_speeds_kmh: &[8.0],
            shift_up_rpm: 1700.0,
            shift_down_rpm: 950.0,
            steer_radii_m: &[(3.0, 8.9), (7.7, 22.8)],
            steer_capacity_n: 240_000.0,
            recirculation: 0.9,
            brake_capacity_n: 120_000.0,
            drag_fraction: 0.25,
            shift_secs: 0.31,
            sprocket_radius_m: 0.34,
            half_tread_m: 1.25,
        });
        let g2 = tp.gears_fwd[1];
        let mut st = TransmissionState {
            gear: 2,
            ..Default::default()
        };
        let v = 900.0 * RPM_TO_RAD * tp.sprocket_radius / g2;
        step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [v, v], [0.0, 0.0]),
        );
        assert_eq!(
            st.gear, 2,
            "a landing past max curve rpm − margin must refuse the downshift"
        );
        let v = 600.0 * RPM_TO_RAD * tp.sprocket_radius / g2;
        step(
            TransmissionMode::Hybrid,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [v, v], [0.0, 0.0]),
        );
        assert_eq!(st.gear, 1, "an in-envelope landing must downshift");
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
    /// footprint scrub. At rest under full steer the Hybrid commands full steer FORCE
    /// outright (fix 2 — the power-limited pivot; the power scale cannot bind at v = 0),
    /// and the L600's brake-gated neutral regime asks the semi-implicit servo for the
    /// exact-landing force `2·neutral_d_full·I/dt`, capacity-clamped — for the lab data
    /// both must land each side at the FULL per-output datum (which EXCEEDS the old
    /// difference-axis reading's `capacity/2` ceiling outright).
    #[test]
    fn pivot_authority_is_per_output_capacity() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let dt = 1.0 / 64.0;
        for mode in [TransmissionMode::Hybrid, TransmissionMode::FixedRadii] {
            let mut st = TransmissionState::default();
            let rep = step(
                mode,
                &fp,
                Some(&tp),
                &mut st,
                &input(0.0, 1.0, [0.0, 0.0], [0.0, 0.0]),
            );
            let expect = match mode {
                // Fix 2: force command up to capacity, power-limited thereafter.
                TransmissionMode::Hybrid => tp.steer_capacity_n,
                // The neutral servo's exact-landing force, per-output capacity clamp.
                _ => (tp.neutral_d_full * fp.inertia / dt).min(tp.steer_capacity_n),
            };
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
