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
//! cannot follow a back-driven shaft. The old `|m|` read a backslide as high FORWARD rpm,
//! which produced the whole reproduced bug family: the governor cut drive to zero
//! mid-backslide (tank rolls backward on flat ground under full W at "2770 rpm", zero
//! force, indefinitely), the scheduler walked the ladder 1→6 while sliding backward at
//! −2..−3 m/s, and the fix-1a landing gate PASSED catastrophic on-grade upshifts (a
//! predicted backward landing `landing_m = −3.62` read as "9092 rpm" ≥ band + margin).
//!
//! Engine crank state ω_e (stage B): the crank is now REAL STATE
//! ([`TransmissionState::omega_e`], rad/s) with its own inertia J
//! ([`TransmissionParams::engine_inertia`]) — stage A's command-proxy rev floor is DEAD.
//! Per tick the crank produces a free torque `τ_free = τ_ind + τ_idle − τ_drag` (induced
//! torque at the crank's OWN rpm under the governor cut; a saturating idle-governor
//! recovery below idle; compression-braking drag, now ENGINE-side — the belt lost its
//! separate drag term and drag reaches the belt only through the coupling), and a
//! capacity-clamped main clutch couples it to the geared shaft
//! ([`clutch_coupling`] — the semi-implicit lock torque, the ONE seamed coupling-law
//! slot). Engaged, the belt's engine force is `F_c = k·s·τ_c` in place of the old
//! `f_p + f_drag`; a STALL GUARD slips the clutch one-sidedly so the crank never lands
//! below idle − [`STALL_GUARD_BAND_RPM`] (no stall death — that is a later,
//! playtest-gated rung). Declutched (shift window / the generalized neutral-idle seam),
//! the belt gets NO engine force and NO engine drag, and the crank REV-MATCHES toward the
//! larger of the landing shaft speed and the steer-demand rpm target (the steering member
//! is engine-driven in every regime). Launch rpm is now the emergent clutch-slip
//! equilibrium; pivot power spools with the crank; `readout` reports ω_e directly (the
//! state IS the display).
//!
//! Reserve scheduler (stage C): on every decision tick the regenerative adapters project the
//! two owned ground reactions onto the signed `m` axis and low-pass the positive load demand
//! `D` with a fixed-tick EMA (DERIVED time scale: 8 ticks / 64 Hz = 0.125 s). The filter freezes
//! through shift windows. For every gear `j`, full-throttle capability at current speed is
//! `F_j = min(torque_at(rpm_j)·G_j/r_s, 2·engine_force)` and reserve is `R_j = F_j − D`.
//! Upshifts retain every stage-A/B gate and add `R_next ≥ 0.10·D + 10 kN`; a negative current
//! reserve held for 13 decision ticks (DERIVED 0.203125 s) commands the highest lower gear that
//! clears the same margin, bounded by the signed-landing and over-rev gates. This CONFIRMED deficit
//! is a correction, not a preference: it is evaluated before an upshift and is exempt from the
//! band anti-hunting reversal dwell. The scheduler names one target; vehicle data
//! [`ShiftAddressing`] decides whether one window commits straight to it or a sequential box pays
//! one window per adjacent step. Every sequential continuation re-runs selection; released intent
//! or recovered demand cancels a stale target. Non-deficit ticks decay confirmation by one rather
//! than erasing its history.
//!
//! Overrun protection (stage C): if the signed geared shaft exceeds governed rpm plus
//! [`OVERRUN_UPSHIFT_MARGIN_RPM`], the box may upshift while coasting. The positive-landing gate,
//! reserve gate, detent-domain gate, and reversal dwell remain; the ordinary landing-rpm band is
//! waived because the protective shift's purpose is to lower an externally back-driven crank.
//!
//! Anti-rollback (stage C): held forward command near rest with negative effective reserve latches
//! [`TransmissionState::hill_hold`]. The hold uses the existing full-envelope brake stop-force
//! path—no extra force—and selects a capable launch gear through the same reserve rule. A shift
//! cut has effective `F = 0`, so a sequential cascade can engage the hold even when its landing
//! gear is statically capable. While latched, launch selection and `GRADE LIMIT` truth are
//! re-evaluated on every decision tick. Release compares transmitted coupling force against
//! `D + min(selection_margin, max(0, R_selected) / 2)`: a margin-short but capable gear can release
//! once it transmits its own modeled force. A release starts a [`HOLD_REENGAGE_TICKS`] cooldown;
//! near-rest chatter cannot re-latch during it, but actual rollback faster than the engagement
//! threshold overrides it. If no gear has non-negative reserve, [`SchedulerState::GradeLimit`]
//! stays exposed and the declared brakes remain applied while W is held.
//!
//! Pure math, no ECS (like [`forces`]): callers own the state. [`TransmissionState`] is the
//! only path-dependent state—gear, shift countdown, steering detent, direction, crank speed ω_e,
//! filtered demand, reserve-confirm counter, held target, scheduler status, hill-hold latch, and
//! re-engagement cooldown—carried as a plain LOCAL component / sandbox resource, NOT replicated,
//! NOT hashed (REV 13; only the offline composition and sandbox run the regenerative adapters).
//! **REV-14 rider:** all of those stage-C values, including the discrete
//! counter/target/status/hold/cooldown state, must join ω_e, gear, and shift countdown when the
//! regenerative box goes multiplayer; replay cannot derive an EMA history, an in-flight sequential
//! target, or a brake latch from the instantaneous belt.
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
//! | `DRAG_SAT_SPEED` (REMOVED, stage B) | — | the belt-side drag saturation ramp died with the belt-side drag term; engine-side drag saturates over the crank's own `ω_idle` (DERIVED from spec, no new const) — the same "fade only near standstill" role reflected engine-side, since any motoring crank sits at or above idle exactly as any driving belt exceeded 0.2 m/s |
//! | [`DRAG_THROTTLE_RELEASE`] | SIM POLICY | driver-intent shaping: where "open throttle" stops meaning "motoring"; part of the uniform input contract, same for every tank |
//! | [`DEAD`] | SIM POLICY | input deadzone on one shared axis mapping |
//! | [`PARK_ENGAGE_SPEED`] | SIM POLICY | latch threshold for "at rest" — a determinism/stability guard on the shared intent layer |
//! | [`HILL_HOLD_ENGAGE_SPEED`] | SIM POLICY | anti-rollback near-rest threshold, DERIVED as `5 × PARK_ENGAGE_SPEED` = 0.25 m/s; gives the existing brake/grip law enough stopping distance through a sequential cut without becoming a moving brake |
//! | [`DIRECTION_SWAP_SPEED`] | SIM POLICY | the intent seam where a held opposite throttle becomes a gear-direction change; uniform game semantics |
//! | [`NEUTRAL_THROTTLE`], [`NEUTRAL_M_SPEED`] | SIM POLICY | regime-entry thresholds for the L600 neutral turn (the neutral turn's SPEED SCALE — [`TransmissionParams::neutral_d_full`] — is spec-DERIVED); `NEUTRAL_M_SPEED` doubles as the hybrid's blend width into its power-limited pivot regime |
//! | [`POSTSHIFT_MARGIN_RPM`] | SIM POLICY | fix-1a anti-hunting: an upshift must PREDICT landing this far above the down band at the end of its own torque-cut window (the cut bleeds belt speed; the static band gap alone was erased in low gears — the measured 1-2-1-2 climb). Upshifts are also intent-gated (`propulsive > 0`) and L600-detent-deferred so the predictor is only consulted inside its domain (review round). Stage A: the predicted landing SHAFT speed must be POSITIVE on the engaged ladder — a sign-flipped landing always refuses (under `|m|` a backward landing read as high forward rpm and the gate blessed catastrophic on-grade upshifts) |
//! | [`OVERRUN_UPSHIFT_MARGIN_RPM`] | SIM POLICY | engine-protection threshold above governed rpm for a coasting/downhill protective upshift; keeps ordinary band noise from invoking the exceptional no-intent path |
//! | [`REVERSAL_DWELL_TICKS`] | SIM POLICY | fix-1b anti-hunting: a committed BAND shift blocks the OPPOSITE-direction BAND shift for this many ticks AFTER its interruption window (the dwell counts only outside the frozen window — review round); same-direction climbs stay free. A 13-tick CONFIRMED reserve deficit is a correction, not a preference, and is exempt |
//! | [`OVERREV_MARGIN_RPM`] | SIM POLICY | fix-1c: a downshift must land at least this far under the engine's max curve rpm — the box never commands an over-rev |
//! | [`RESERVE_MARGIN_FRACTION`] | SIM POLICY | common capability headroom: DERIVED policy value 0.10 of filtered demand keeps a target away from the zero-acceleration knife edge |
//! | [`RESERVE_MARGIN_FLOOR_N`] | SIM POLICY | common low-load/jitter floor: DERIVED policy value 10 kN total, 1.8% of Tiger weight and about half its fractional margin at the DERIVED 191.2 kN 20-degree demand |
//! | [`DEMAND_FILTER_TICKS`] | SIM POLICY | deterministic reaction low-pass: DERIVED 8 decision ticks = 0.125 s at 64 Hz; frozen through shift cuts |
//! | [`GRADE_CONFIRM_TICKS`] | SIM POLICY | persistence before a reserve downshift: DERIVED 13 ticks = 0.203125 s at 64 Hz, rejecting shorter load spikes |
//! | [`HOLD_REENGAGE_TICKS`] | SIM POLICY | DERIVED 32-tick = 0.5 s anti-oscillation cooldown after hill-hold release; a real rollback faster than [`HILL_HOLD_ENGAGE_SPEED`] overrides it |
//! | `gearbox.shift_addressing` / [`ShiftAddressing`] | VEHICLE DATA | the model accepts arbitrary targets; the spec declares whether this gearbox can address one directly or must step sequentially—an era/mechanism capability, not scheduler policy |
//! | [`WIDE_ON`]/[`WIDE_OFF`]/[`TIGHT_ON`]/[`TIGHT_OFF`] | SIM POLICY | stick-to-detent input mapping with hysteresis; the DETENT RATIOS they select are spec |
//! | [`TICK_HZ`] | SIM POLICY | the fixed simulation tick the shift countdown quantizes against |
//! | [`K_IDLE_DROOP_RPM`] | SIM POLICY | idle-governor gain, expressed as the droop width: FULL recovery torque (`torque_at(idle)`) is reached ~50 rpm below idle. A governor stand-in, not an engine datum — any governed engine gets the same recovery shape; the TORQUE it recovers with is the vehicle's own curve |
//! | [`STALL_GUARD_BAND_RPM`] | SIM POLICY | one-sided clamp band under idle: the coupling may never land the crank below `ω_floor = idle − band`, and (review round FIX 2) `ω_floor` is ALSO a hard end-of-tick clamp on ω_e — the floor IS the no-stall policy while stall death is deliberately unmodeled, so no legal spec corner (e.g. strongly negative τ_free from a large drag fraction over a weak idle curve) may carry the crank below it or to a negative speed. Sized so the idle governor SATURATES before the guard floor (band = 2× the droop width). The spec layer keeps `ω_floor > 0` by requiring `idle_rpm ≥ 300` (band 100 + 100 margin, spec.rs) |
//! | [`CLUTCH_OUT_M_SPEED`]/[`CLUTCH_IN_M_SPEED`] | SIM POLICY | coupling-seam hysteresis (review round FIX 3): declutch below 0.8×, re-engage at 1.2× of `NEUTRAL_M_SPEED` (or on any propulsive command) — a regime seam needs separated thresholds or it chatters, same doctrine as the steering detents |
//! | [`REV_MATCH_BAND_RPM`] | SIM POLICY | proportional band of the declutched rev-match drive (`u_match = clamp((ω_target − ω_e)/band, 0, 1)`): full fueling one band below target, tapering to zero at it — smooth approach at 64 Hz instead of bang-bang chatter. Match AUTHORITY is the vehicle's own torque curve / J |
//!
//! Moved OUT of this module to the spec (they were vehicle data wearing const clothing):
//! shift time (`gearbox.shift_secs` — a Tiger preselector and a T-34 crash box differ),
//! shift addressing (`gearbox.shift_addressing`), engine drag (`engine.drag_fraction` — an engine
//! datum). REMOVED rather than classified:
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

/// Gear-selection capability declared by the vehicle spec. The scheduler may name any target;
/// this datum decides whether one interruption reaches it directly or pays one window per
/// adjacent step.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Deserialize)]
pub enum ShiftAddressing {
    /// Preselector/automatic capability: one shift event commits straight to the legal target.
    Direct,
    /// Conservative crash-box capability: each event moves one adjacent gear and the held target
    /// is approached over repeated interruption windows.
    #[default]
    Sequential,
}

/// Observable state of the reserve scheduler. Kept compact and copyable because the same value is
/// local sim memory and the readout/HUD contract; it is not a wire type under REV 13.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SchedulerState {
    #[default]
    Normal,
    /// A reserve-commanded shift, retaining the original and final gear across a sequential
    /// cascade so the readout describes the capability target rather than each adjacent step.
    GradeShift {
        from: u8,
        to: u8,
    },
    HillHold,
    GradeLimit,
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
///
/// Stage-B re-derivation (window physics changed: the declutched window carries NO drag on
/// the belt, in prediction AND reality): 150 stays. At full throttle — the dominant
/// upshift intent — drag was already fully released (`hold_blend(1/0.5) = 0`), so the
/// predictor's full-throttle arithmetic is bit-identical to stage A; at partial throttle
/// the old predictor's drag term matched the old window's real drag, and both died
/// together. What the margin covers is unchanged: the frozen-reaction bleed error
/// (`r_mean/I × window`), which stage B does not touch.
const POSTSHIFT_MARGIN_RPM: f32 = 150.0;

/// Engine-protection overrun threshold (rpm): a positive signed shaft this far above the
/// governed point may upshift without propulsive intent. The ordinary landing-band check is
/// waived for that protective shift because lowering rpm is its purpose. SIM POLICY.
const OVERRUN_UPSHIFT_MARGIN_RPM: f32 = 150.0;

/// Fix-1b anti-hunting dwell (fixed ticks, 0.5 s at 64 Hz): after a shift commits, the
/// OPPOSITE-direction shift stays blocked this long. Same-direction shifts stay free — a
/// rapid 1-2-3 climb must not slow down. SIM POLICY.
const REVERSAL_DWELL_TICKS: u8 = 32;

/// Fix-1c over-rev margin (rpm): a downshift is refused if its landing rpm in the lower
/// gear would exceed the engine's max authored curve rpm minus this margin — the box never
/// commands an over-rev. SIM POLICY.
const OVERREV_MARGIN_RPM: f32 = 100.0;

/// Stage-C reserve margin as a fraction of the filtered mean-axis demand. Ten percent keeps a
/// target gear away from the zero-acceleration knife edge without encoding a vehicle-specific
/// force. SIM POLICY.
const RESERVE_MARGIN_FRACTION: f32 = 0.10;

/// Stage-C absolute reserve floor (N, both tracks together). 10 kN is large enough to dominate
/// contact-reaction float jitter yet only 1.8% of the Tiger's weight and about half the fractional
/// margin on the DERIVED 191 kN 20-degree demand. SIM POLICY.
const RESERVE_MARGIN_FLOOR_N: f32 = 10_000.0;

/// Stage-C load filter time scale in fixed decision ticks. An EMA divisor of eight is a
/// deterministic ~0.125 s DERIVED low-pass at the 64 Hz SIM POLICY; the state freezes while the box is declutched so
/// torque-cut reaction transients cannot rewrite grade demand. SIM POLICY.
const DEMAND_FILTER_TICKS: f32 = 8.0;

/// Consecutive reserve-deficit decision ticks required before a grade downshift (13 ticks DERIVED
/// = 0.203125 s DERIVED at the 64 Hz SIM POLICY). Shorter reaction spikes remain load telemetry, not shift commands.
/// SIM POLICY.
const GRADE_CONFIRM_TICKS: u8 = 13;

/// Hill-hold anti-oscillation cooldown after a release (32 fixed ticks = 0.5 s DERIVED at 64 Hz).
/// A genuine backward roll faster than [`HILL_HOLD_ENGAGE_SPEED`] overrides it. SIM POLICY.
const HOLD_REENGAGE_TICKS: u8 = 32;

/// Fuel-governor cut width (rpm): torque ramps linearly to zero over this band past the
/// governed rpm, so the top-speed equilibrium is a smooth root instead of a hard clip.
/// INFERRED numerical policy, not vehicle data.
const GOVERNOR_CUT_RPM: f32 = 100.0;

/// Idle-governor droop width (rpm): the idle governor's recovery torque ramps linearly from
/// zero at idle to FULL `torque_at(idle)` this far below it (gain = `torque_at(idle) /
/// (K_IDLE_DROOP_RPM·RPM_TO_RAD)` N·m per rad/s), saturating beyond. Stage B SIM POLICY —
/// see the classification table.
const K_IDLE_DROOP_RPM: f32 = 50.0;

/// Stall-guard band (rpm): the one-sided clamp under idle — the coupling reduces the clutch
/// torque so the crank never lands below `idle − STALL_GUARD_BAND_RPM`. At 2× the idle
/// droop width the idle governor is fully saturated at the guard floor, so `τ_free ≥
/// torque_at(idle) − τ_drag_max > 0` there and the guard can always hold it (the clutch
/// slips to protect the crank; stall DEATH is a later, playtest-gated rung). Stage B SIM
/// POLICY — see the classification table.
const STALL_GUARD_BAND_RPM: f32 = 100.0;

/// Declutched rev-match proportional band (rpm): full fueling one band below the landing
/// target, tapering to zero at it (`u_match = clamp((ω_target − ω_e)/band, 0, 1)`). Stage B
/// SIM POLICY — see the classification table.
const REV_MATCH_BAND_RPM: f32 = 200.0;

/// Clutch-seam hysteresis on |m| (stage-B review round, FIX 3) — the coupling seam is a
/// REGIME boundary and a single threshold chatters on it (traced: a boundary creeper at
/// constant sub-neutral throttle sawtoothed engage/declutch every few ticks — the engaged
/// tick's drag/creep impulse threw the belt back across the line the declutched tick let
/// it re-cross). Detent-style separated thresholds, derived from [`NEUTRAL_M_SPEED`]
/// (±20%): the clutch goes OUT below `NEUTRAL_M_SPEED × 0.8`, back IN at
/// `NEUTRAL_M_SPEED × 1.2` (or any propulsive drive command, which re-engages at any
/// speed — the launch). Deterministic state ([`TransmissionState::clutch_out`]), no blend.
/// SIM POLICY.
const CLUTCH_OUT_M_SPEED: f32 = NEUTRAL_M_SPEED * 0.8;
const CLUTCH_IN_M_SPEED: f32 = NEUTRAL_M_SPEED * 1.2;

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

/// Hill-hold near-rest threshold (m/s), derived from the existing parking-latch scale. Five times the
/// park threshold (0.25 m/s DERIVED = 0.90 km/h DERIVED) catches the sequential cascade before a perceptible
/// rollback while remaining firmly in the stop-force law's near-rest regime. SIM POLICY.
const HILL_HOLD_ENGAGE_SPEED: f32 = PARK_ENGAGE_SPEED * 5.0;

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
    /// Crank + flywheel + clutch rotational inertia J (kg·m²) — the engine-side inertia the
    /// stage-B crank state integrates against. Vehicle data (INFERRED at the authoring
    /// sites: class scaling, flywheel-dominant).
    pub engine_inertia: f32,
    /// Main clutch torque capacity (N·m) — the coupling's clamp: the largest torque the
    /// engaged clutch transmits before slipping (≈ 1.3 × peak engine torque by the usual
    /// sizing rule; INFERRED at the authoring sites). THE COUPLING-LAW SLOT's one datum —
    /// a torque-converter characteristic replaces the clamp for modern automatics later.
    pub clutch_capacity: f32,
    /// Gear-shift torque-interruption window in fixed ticks — DERIVED from the authored
    /// `shift_secs` (a Tiger preselector and a crash box shift very differently: vehicle
    /// data, not module policy).
    pub shift_ticks: u8,
    /// Whether a scheduler target is reached directly or one adjacent step per shift event.
    pub shift_addressing: ShiftAddressing,
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
    /// See [`TransmissionParams::engine_inertia`] (kg·m²).
    pub engine_inertia_kgm2: f32,
    /// See [`TransmissionParams::clutch_capacity`] (N·m).
    pub clutch_capacity_nm: f32,
    /// Gear-shift torque-interruption time (s) — see [`TransmissionParams::shift_ticks`].
    pub shift_secs: f32,
    /// See [`TransmissionParams::shift_addressing`].
    pub shift_addressing: ShiftAddressing,
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
            engine_inertia: a.engine_inertia_kgm2,
            clutch_capacity: a.clutch_capacity_nm,
            shift_ticks: (a.shift_secs * TICK_HZ).round().clamp(0.0, 255.0) as u8,
            shift_addressing: a.shift_addressing,
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
/// gear/window/detent/direction/brake/coupling state plus stage-C demand, confirmation, target,
/// scheduler status, and hill hold. Constructed at spawn from tank data; a plain local component /
/// sandbox resource under REV 13 (the complete REV-14 rider is in the module doc).
#[derive(Clone, Copy, PartialEq, Debug)]
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
    /// Engine crank speed ω_e (rad/s) — stage B's crank state. `0.0` is the UNINITIALIZED
    /// sentinel: `Default` cannot see the spec (the state must be constructible before the
    /// spec arrives — the same spawn-time invariant as the pre-sized element slabs,
    /// element-promotion-checklist.md), so the first [`regenerative`] step with params snaps
    /// it to the vehicle's idle. A live crank can never read 0.0 — the end-of-tick hard
    /// floor (review round FIX 2) keeps every stepped ω_e ≥ `idle −`
    /// [`STALL_GUARD_BAND_RPM`] `> 0` (the spec layer requires `idle_rpm ≥ 300`) — so the
    /// sentinel check is EXACTLY `== 0.0` and only ever fires at true spawn.
    pub omega_e: f32,
    /// Main-clutch-out latch (stage-B review round, FIX 3): the coupling-seam regime with
    /// hysteresis — set below [`CLUTCH_OUT_M_SPEED`] without propulsive drive, cleared at
    /// [`CLUTCH_IN_M_SPEED`] or on any propulsive command. Local scheduler-adjacent memory
    /// (REV 13, like `last_shift_dir`): not replicated, not hashed.
    pub clutch_out: bool,
    /// Filtered positive load demand on the signed mean shaft axis (N, both tracks). Updated only
    /// on decision ticks and frozen through shift windows so the torque cut cannot pollute it.
    /// Local scheduler memory (REV 13: not replicated, not hashed).
    pub demand_n: f32,
    /// Spawn seed for `demand_n`: the first owned reaction sample initializes the EMA directly
    /// instead of ramping from a fictitious zero-load history. Local discrete memory; REV-14
    /// rider in the module doc.
    pub demand_initialized: bool,
    /// Persistent decision-tick evidence that the current gear has negative reserve. Negative
    /// ticks increment it and other ticks decay it by one, so one contact-jitter sample cannot erase
    /// a nearly confirmed deficit. Saturating u8; local scheduler memory (REV 13), with the REV-14
    /// rider documented at module level.
    pub grade_confirm_ticks: u8,
    /// Held reserve target (1-based; zero means none). Direct addressing retains it through its
    /// one interruption window; Sequential retains it across every adjacent window.
    pub grade_target: u8,
    /// Scheduler/readout state. Local discrete sim memory under REV 13; REV-14 rider in the
    /// module doc.
    pub scheduler: SchedulerState,
    /// Anti-rollback latch. While true, the existing service-brake stop-force law runs at its full
    /// declared envelope until the selected launch gear transmits the capability-derived release
    /// threshold. Local discrete sim memory (REV 13), with a REV-14 rider in the module doc.
    pub hill_hold: bool,
    /// Remaining post-release hill-hold cooldown ticks. While nonzero, a near-rest deficit cannot
    /// re-latch; actual backward motion past [`HILL_HOLD_ENGAGE_SPEED`] overrides the cooldown.
    /// Local discrete sim memory under REV 13; REV-14 rider in the module doc.
    pub hold_reengage_ticks: u8,
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
            omega_e: 0.0,
            clutch_out: false,
            demand_n: 0.0,
            demand_initialized: false,
            grade_confirm_ticks: 0,
            grade_target: 0,
            scheduler: SchedulerState::Normal,
            hill_hold: false,
            hold_reengage_ticks: 0,
        }
    }
}

/// A compact operating-point readout of the joint drivetrain — the ONE place the HUD/legend
/// reads gear and rpm from, so the display never re-derives drivetrain math (the gear/rpm
/// relation lives here, beside the adapter that integrates on it).
#[derive(Clone, Debug, PartialEq)]
pub struct DriveReadout {
    /// Engine rpm — the crank state ω_e DIRECTLY (stage B: the state IS the display). The
    /// uninitialized sentinel (never stepped) reads idle; a live crank reads its honest
    /// speed, sub-idle grade lug included (the stall guard bounds it at idle −
    /// [`STALL_GUARD_BAND_RPM`]).
    pub rpm: f32,
    /// The engaged gear as a display label: `F1..Fn` forward, `R1..Rn` reverse.
    pub gear_label: String,
    /// Reserve-scheduler state, copied from the local transmission truth for HUD rendering.
    pub scheduler: SchedulerState,
}

/// Read the drivetrain operating point THROUGH THE LAW: the engaged gear from
/// [`TransmissionState`] against the active ladder, and the engine rpm from the CRANK STATE
/// ω_e (stage B — no belt-derived re-derivation: the crank slips against the shaft at
/// launch, rev-matches through shifts, and idles while back-driven, and the display shows
/// exactly that state). Pure (no ECS), so the HUD and any legend share one implementation.
pub fn readout(st: &TransmissionState, tp: &TransmissionParams) -> DriveReadout {
    let ladder: &[f32] = if st.reverse {
        &tp.gears_rev
    } else {
        &tp.gears_fwd
    };
    let top = ladder.len() as u8;
    let gear = st.gear.clamp(1, top);
    let rpm = if st.omega_e > 0.0 {
        st.omega_e / RPM_TO_RAD
    } else {
        // Uninitialized sentinel (state constructed, no regenerative step yet): idle.
        tp.engine.idle_rpm
    };
    DriveReadout {
        rpm,
        gear_label: format!("{}{gear}", if st.reverse { 'R' } else { 'F' }),
        scheduler: st.scheduler,
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

/// The engine's compression-braking drag torque at the CRANK (stage B — drag moved
/// engine-side; the belt lost its separate `f_drag` term and drag reaches the belt only
/// through the coupling): `drag_fraction × peak torque`, released as the fueling demand
/// opens (`hold_blend(u/DRAG_THROTTLE_RELEASE)` — a brake command is not fueling, so drag
/// stays engaged under it, exactly the old release contract), saturating over the crank's
/// own speed scale ω_idle (the engine-side reflection of the old belt-side
/// `DRAG_SAT_SPEED` role: fade only near a stopped crank — a motoring crank sits at or
/// above idle and reads full drag, as any driving belt exceeded 0.2 m/s). ω_e is never
/// negative, so the torque always opposes crank rotation.
fn engine_drag(tp: &TransmissionParams, omega_e: f32, u_fuel: f32) -> f32 {
    let omega_idle = tp.engine.idle_rpm * RPM_TO_RAD;
    tp.peak_torque_nm
        * tp.drag_fraction
        * forces::hold_blend(u_fuel / DRAG_THROTTLE_RELEASE)
        * (omega_e / omega_idle).clamp(0.0, 1.0)
}

/// Fix-1a: roll the shift's torque-cut window forward on the mean belt axis and return the
/// PREDICTED landing speed `m`. Exactly the live integration's mean — per side
/// `v += (Q − R)/I·dt` — under the shift window's own stage-B rules: the box is
/// DECLUTCHED, so the belt gets NO engine force and NO engine drag (the old predictor's
/// drag-through-the-landing-gear term died with the belt-side drag), and the ground
/// reaction is frozen at its current per-tick mean. Fixed-tick, f32-exact — prediction and
/// reality run the same (now purely reaction-driven) window law.
///
/// DOMAIN (review round): valid for the PROPULSIVE straight-line case ONLY — the only case
/// the scheduler consults it for (upshifts are intent-gated on `propulsive > 0` and
/// detent-deferred on the L600). It carries no brake term and no λ/steer state, so under
/// service braking or a geared turn it would over-predict the landing. Inside its domain,
/// frozen-R is CONSERVATIVE (the true post-cut reaction collapses with the slip), and the
/// single mean-axis clamp is an accepted approximation of the live per-side clamps.
fn predict_shift_landing_m(
    tp: &TransmissionParams,
    fp: &ForceParams,
    m: f32,
    r_mean: f32,
    dt: f32,
) -> f32 {
    let mut pm = m;
    for _ in 0..tp.shift_ticks {
        pm = (pm - r_mean / fp.inertia * dt).clamp(-fp.max_speed, fp.max_speed);
    }
    pm
}

/// Full-throttle mean-axis force available in one gear at the current signed shaft speed. The
/// engine reads only non-negative rpm (stage A); the result is capped by the existing two-track
/// low-speed/traction envelope (`engine_force` is authored per track).
fn available_force_in_gear(
    tp: &TransmissionParams,
    fp: &ForceParams,
    shaft: f32,
    gear: f32,
) -> f32 {
    let rpm = shaft.max(0.0) * gear / tp.sprocket_radius / RPM_TO_RAD;
    (tp.torque_at(rpm) * gear / tp.sprocket_radius).min(2.0 * fp.engine_force)
}

/// Positive capability headroom required of a selected gear (N, both tracks together).
fn reserve_margin(demand: f32) -> f32 {
    demand.max(0.0) * RESERVE_MARGIN_FRACTION + RESERVE_MARGIN_FLOOR_N
}

/// Modeled full-throttle reserve for one gear at the current shaft speed.
fn modeled_reserve_in_gear(
    tp: &TransmissionParams,
    fp: &ForceParams,
    shaft: f32,
    gear: f32,
    demand: f32,
) -> f32 {
    available_force_in_gear(tp, fp, shaft, gear) - demand
}

/// Choose the highest gear below `current` that clears reserve margin, then apply the downshift
/// over-rev bound. If the ideal gear itself over-revs, return the closest legal gear on the path
/// toward it (which may not yet clear margin); later decisions can continue after speed falls.
fn select_grade_target(
    tp: &TransmissionParams,
    fp: &ForceParams,
    ladder: &[f32],
    shaft: f32,
    current: u8,
    demand: f32,
) -> Option<u8> {
    if current <= 1 || shaft <= -PARK_ENGAGE_SPEED {
        return None;
    }
    let margin = reserve_margin(demand);
    let ideal = (1..current).rev().find(|&gear| {
        available_force_in_gear(tp, fp, shaft, ladder[(gear - 1) as usize]) - demand >= margin
    })?;
    let lowest_legal = (1..current).find(|&gear| {
        let rpm = shaft * ladder[(gear - 1) as usize] / tp.sprocket_radius / RPM_TO_RAD;
        rpm <= tp.max_curve_rpm() - OVERREV_MARGIN_RPM
    })?;
    Some(ideal.max(lowest_legal))
}

/// Select the highest legal launch gear at or below `current` that clears the ordinary scheduler
/// margin. If none does, accept the highest gear with merely non-negative reserve. The second arm
/// is what separates a truthful grade limit from a margin-short but physically capable launch.
fn select_hill_hold_target(
    tp: &TransmissionParams,
    fp: &ForceParams,
    ladder: &[f32],
    shaft: f32,
    current: u8,
    demand: f32,
) -> Option<u8> {
    let highest_with_reserve = |minimum: f32| {
        (1..=current).rev().find(|&gear| {
            let ratio = ladder[(gear - 1) as usize];
            let rpm = shaft * ratio / tp.sprocket_radius / RPM_TO_RAD;
            rpm <= tp.max_curve_rpm() - OVERREV_MARGIN_RPM
                && modeled_reserve_in_gear(tp, fp, shaft, ratio, demand) >= minimum
        })
    };
    highest_with_reserve(reserve_margin(demand)).or_else(|| highest_with_reserve(0.0))
}

/// Commit one grade-target event according to the vehicle's addressing capability.
fn commit_grade_shift(st: &mut TransmissionState, tp: &TransmissionParams, target: u8) {
    let from = match st.scheduler {
        SchedulerState::GradeShift { from, .. } => from,
        _ => st.gear,
    };
    st.grade_target = target;
    st.scheduler = SchedulerState::GradeShift { from, to: target };
    st.gear = match tp.shift_addressing {
        ShiftAddressing::Direct => target,
        ShiftAddressing::Sequential => st.gear.saturating_sub(1).max(target),
    };
    st.shift_ticks = tp.shift_ticks;
    st.last_shift_dir = -1;
    st.dwell_ticks = REVERSAL_DWELL_TICKS;
    st.grade_confirm_ticks = 0;
}

/// Re-evaluate a latched hill hold on a decision tick. Returns whether this call committed a new
/// shift step, allowing the ordinary scheduler to avoid spending the same zero-length window twice.
fn refresh_hill_hold(
    st: &mut TransmissionState,
    tp: &TransmissionParams,
    fp: &ForceParams,
    ladder: &[f32],
    shaft: f32,
    rollback_rescue: bool,
) -> bool {
    let downshift_allowed = shaft > -PARK_ENGAGE_SPEED || rollback_rescue;
    match select_hill_hold_target(tp, fp, ladder, shaft, st.gear, st.demand_n) {
        Some(target) if target < st.gear && downshift_allowed => {
            // A latched rollback that the current gear plus full brakes cannot arrest is the
            // deliberate exception to both ordinary grade-shift guards: this downshift is crew
            // action to GAIN enough launch capability, not a claim that the declutched landing will
            // be forward or in-band. Like the purpose-scoped protective-upshift waiver below, only
            // this named rescue path waives the landing-sign policy; free backslides never call it
            // with permission.
            commit_grade_shift(st, tp, target);
            st.scheduler = SchedulerState::HillHold;
            true
        }
        Some(_) => {
            st.grade_target = 0;
            st.scheduler = SchedulerState::HillHold;
            false
        }
        None => {
            st.grade_target = 0;
            st.scheduler = SchedulerState::GradeLimit;
            false
        }
    }
}

/// THE COUPLING-LAW SLOT (stage B): the engaged main clutch between crank and geared shaft,
/// solved semi-implicitly and capacity-clamped; returns the transmitted clutch torque τ_c.
/// This is deliberately ONE seamed function — a torque-converter characteristic replaces
/// the clamp here for modern automatic vehicles later (do NOT build the converter now);
/// everything upstream (τ_free) and downstream (belt split, re-anchor) is
/// coupling-law-agnostic.
///
/// The LOCK torque is the τ_c that lands `ω_e_next = k·s·m_next` under both semi-implicit
/// integrations (`ω_e_next = ω_e + (τ_free − τ_c)·dt/J`; `m_next = m + (k·s·τ_c +
/// F_other)·dt/I_m` with `I_m = 2·belt inertia` on the mean axis):
///
/// ```text
/// τ_c* = [(ω_e − k·s·m)/dt + τ_free/J − k·s·F_other/I_m] / (1/J + k²/I_m)
/// ```
///
/// clamped to ±`clutch_capacity` (beyond it the clutch slips honestly — the launch force is
/// the capacity, not the lock demand). `F_other` is the m-axis force sum EXCLUDING the
/// engine path — the summed ground reactions. This is a PREDICTOR approximation (review
/// round FIX 1): the later brake stop-forces, the FixedRadii λ mean-axis share
/// (`j_L + j_R = −e` does NOT cancel), and the belt ±max_speed clamp all move `m_next`
/// after this solve, and exact pre-accounting is CIRCULAR (the brake law reads the very
/// `q` that needs `F_c`). What makes the approximation safe is the end-of-step FEASIBILITY
/// check in [`regenerative`]: the crank re-anchors to the belt that actually integrated
/// only if the implied total clutch torque fits the capacity — otherwise the honestly
/// integrated (slipping) crank stands.
///
/// STALL GUARD (one-sided): if the transmitted τ_c would land the crank below
/// `ω_floor = idle − STALL_GUARD_BAND_RPM`, τ_c is REDUCED to land exactly at ω_floor
/// (the clutch slips to protect the crank; the guard never increases τ_c, and at the floor
/// the saturated idle governor keeps `τ_free > 0` for any sane torque curve). The
/// end-of-tick hard floor in [`regenerative`] backstops the legal-but-extreme spec corner
/// where even `τ_c = −capacity` cannot hold the floor (strongly negative τ_free).
#[allow(clippy::too_many_arguments)]
fn clutch_coupling(
    j_e: f32,
    capacity: f32,
    k: f32,
    s: f32,
    omega_e: f32,
    m: f32,
    tau_free: f32,
    i_m: f32,
    f_other: f32,
    omega_floor: f32,
    dt: f32,
) -> f32 {
    let tau_star = ((omega_e - k * s * m) / dt + tau_free / j_e - k * s * f_other / i_m)
        / (1.0 / j_e + k * k / i_m);
    let mut tau_c = tau_star.clamp(-capacity, capacity);
    let omega_next = omega_e + (tau_free - tau_c) * dt / j_e;
    if omega_next < omega_floor {
        // Land exactly at the floor: τ_guard = τ_free − J·(ω_floor − ω_e)/dt. By
        // construction τ_guard < τ_c (less torque = higher landing), so the guard only
        // ever reduces — one-sided.
        tau_c = (tau_free - (omega_floor - omega_e) * j_e / dt).clamp(-capacity, capacity);
    }
    tau_c
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
            // F and R project the same physical reactions with opposite signs. Old-ladder EMA and
            // confirmation history are therefore not evidence about the newly engaged direction;
            // let the observer below seed directly from this tick's new-ladder sample.
            st.demand_n = 0.0;
            st.demand_initialized = false;
            st.grade_confirm_ticks = 0;
            st.grade_target = 0;
            st.scheduler = SchedulerState::Normal;
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

    // Stage C demand observer: the contact reactions are the load signal the sim already owns.
    // Project their sum onto the engaged ladder's signed m-axis and keep only propulsive demand;
    // downhill assistance is zero demand, not negative reserve. The first sample seeds directly,
    // then a fixed 1/8 EMA filters contact chatter. The update is deliberately absent during a
    // shift window: the declutched cut changes slip/reactions and is not a change in the grade.
    if st.shift_ticks == 0 {
        let sample = (dir * (inp.reactions[0] + inp.reactions[1])).max(0.0);
        if st.demand_initialized {
            st.demand_n += (sample - st.demand_n) / DEMAND_FILTER_TICKS;
        } else {
            st.demand_n = sample;
            st.demand_initialized = true;
        }
    }

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
    let current_reserve =
        modeled_reserve_in_gear(tp, fp, shaft, ladder[(st.gear - 1) as usize], st.demand_n);

    // Stage C anti-rollback. Only held FORWARD propulsive intent can own the latch; release or
    // reverse intent drops it immediately and lets the established direction-swap semantics run.
    // Ordinary engagement is near rest on the existing PARK speed scale and only when the engaged
    // gear cannot pull the filtered load (or a paid shift cut transmits zero). A genuine backward
    // roll beyond the threshold also engages and overrides the post-release cooldown: that is
    // rollback, not near-rest chatter. While latched, selection runs on EVERY decision tick so a
    // changing EMA can retarget and GRADE LIMIT always describes current capability.
    let hold_cooldown_active = st.hold_reengage_ticks > 0;
    if hold_cooldown_active {
        st.hold_reengage_ticks -= 1;
    }
    let real_rollback = shaft < -HILL_HOLD_ENGAGE_SPEED;
    let forward_intent = !st.reverse && inp.throttle > DEAD;
    let mut hill_hold_step_committed = false;
    if !forward_intent {
        st.hill_hold = false;
        if matches!(
            st.scheduler,
            SchedulerState::HillHold | SchedulerState::GradeLimit
        ) {
            st.scheduler = SchedulerState::Normal;
            st.grade_target = 0;
        }
    } else {
        let in_engagement_zone = shaft.abs() < HILL_HOLD_ENGAGE_SPEED || real_rollback;
        let effective_deficit = real_rollback
            || current_reserve < 0.0
            // During a paid interruption the selected gear's static capability is not being
            // transmitted: effective F = 0, so reserve is `-D`. This catches a sequential cascade
            // that loses the climb inside an otherwise-capable landing gear.
            || (st.shift_ticks > 0 && st.demand_n > 0.0);
        if !st.hill_hold
            && in_engagement_zone
            && (!hold_cooldown_active || real_rollback)
            && effective_deficit
        {
            st.hill_hold = true;
        }
        if st.hill_hold && st.shift_ticks > 0 {
            // Finish the already-paid event under the brakes. A retained sequential target resumes
            // on the first decision tick after this window; starting another window here would
            // erase part of the declared shift cost.
            st.scheduler = SchedulerState::HillHold;
        } else if st.hill_hold {
            // The live selector still runs during every REAL rollback so HILL HOLD / GRADE LIMIT is
            // truthful. The negative-shaft shift permission is narrower: only when the current
            // gear's modeled force PLUS both full declared brakes still has negative arrest reserve
            // is the crew actively rescuing rather than freely backsliding. Direct pays one event;
            // Sequential repeats this capability decision after each paid window. This explicit
            // flag is the sole bypass for the ordinary signed-shaft and landing-sign guards.
            let braked_rollback_rescue =
                real_rollback && current_reserve + 2.0 * tp.brake_capacity_n < 0.0;
            hill_hold_step_committed =
                refresh_hill_hold(st, tp, fp, ladder, shaft, braked_rollback_rescue);
        }
    }
    let r_mean = (inp.reactions[0] + inp.reactions[1]) / 2.0;
    // Ordinary grade shifts still require a forward landing. The braked rollback rescue commits
    // through `refresh_hill_hold` above instead: its purpose is gaining capability, so that named
    // path deliberately waives this sign gate just as the protective-upshift arm deliberately
    // waives its ordinary landing-band gate for engine protection.
    let grade_landing_positive = dir * predict_shift_landing_m(tp, fp, m, r_mean, dt) > 0.0;
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
        let (dwell_ticks, last_shift_dir) = (st.dwell_ticks, st.last_shift_dir);
        let dwell_blocks = |shift_dir: i8| dwell_ticks > 0 && last_shift_dir == -shift_dir;
        let mut grade_step_committed = hill_hold_step_committed;

        // A held Sequential target is never an instruction to shift blindly. Re-run the same
        // selector against current intent, speed, and filtered demand at every continuation. A
        // recovered current gear (`reserve >= 0`) or released propulsive command cancels the stale
        // cascade; a changed capability target retargets while retaining the original HUD `from`.
        if !grade_step_committed && st.grade_target > 0 {
            let selected = (propulsive > 0.0 && current_reserve < 0.0)
                .then(|| select_grade_target(tp, fp, ladder, shaft, st.gear, st.demand_n))
                .flatten()
                .filter(|&target| target < st.gear);
            if let Some(target) = selected {
                let from = match st.scheduler {
                    SchedulerState::GradeShift { from, .. } => from,
                    _ => st.gear,
                };
                st.grade_target = target;
                st.scheduler = SchedulerState::GradeShift { from, to: target };
                let next = st.gear - 1;
                if shaft > -PARK_ENGAGE_SPEED
                    && grade_landing_positive
                    && shaft_rpm_geared(ladder[(next - 1) as usize])
                        <= tp.max_curve_rpm() - OVERREV_MARGIN_RPM
                {
                    commit_grade_shift(st, tp, target);
                    grade_step_committed = true;
                }
            } else {
                st.grade_target = 0;
                st.scheduler = SchedulerState::Normal;
            }
        }

        // Deficit evidence is leaky persistence, not a consecutive-run latch: one non-negative
        // reaction sample decays one tick rather than erasing twelve prior negative samples. The
        // actual confirmed correction below still requires the deficit and propulsive intent to be
        // present on this tick.
        if !grade_step_committed
            && propulsive > 0.0
            && st.gear > 1
            && shaft > -PARK_ENGAGE_SPEED
            && current_reserve < 0.0
        {
            st.grade_confirm_ticks = st.grade_confirm_ticks.saturating_add(1);
        } else {
            st.grade_confirm_ticks = st.grade_confirm_ticks.saturating_sub(1);
        }
        let confirmed_deficit = !grade_step_committed
            && propulsive > 0.0
            && st.gear > 1
            && shaft > -PARK_ENGAGE_SPEED
            && current_reserve < 0.0
            && st.grade_confirm_ticks >= GRADE_CONFIRM_TICKS;

        // A confirmed reserve deficit is a CORRECTION, not a preference. It owns the decision
        // before either upshift arm and is exempt from reversal dwell: the full 13-tick persistence
        // is the anti-hunting protection already paid for this correction.
        if confirmed_deficit {
            if grade_landing_positive
                && let Some(target) =
                    select_grade_target(tp, fp, ladder, shaft, st.gear, st.demand_n)
            {
                commit_grade_shift(st, tp, target);
            }
        } else {
            // The ordinary intent gate remains: service braking never upshifts because the landing
            // predictor has no brake term. The one exception is engine protection on a positive
            // signed shaft above governed + margin while coasting/downhill. It keeps the same
            // detent, reversal-dwell, positive-landing, and reserve gates; only propulsive intent
            // and the ordinary landing-rpm band are waived.
            let protective_upshift =
                service == 0.0 && rpm > tp.engine.governed_rpm + OVERRUN_UPSHIFT_MARGIN_RPM;
            let ordinary_upshift = propulsive > 0.0 && rpm > tp.shift_up_rpm;
            if !grade_step_committed
                && st.gear < top
                && (ordinary_upshift || protective_upshift)
                && !detent_turn
                && !dwell_blocks(1)
            {
                let g_up = ladder[st.gear as usize];
                // The predictor returns a SIGNED m; the gate reads its SIGNED shaft speed
                // (stage A): the landing must be POSITIVE on the engaged ladder AND clear the
                // down band + margin. A sign-flipped landing always refuses the upshift — under
                // `|m|` the traced grade case (r_mean = 221 kN, landing_m = −3.62) read as
                // "9092 rpm" and PASSED, committing catastrophic on-grade upshifts. No
                // at-rest threshold is needed HERE (review round): the rpm bound already
                // demands a landing ≥ down band + margin — solidly positive, far above any
                // numerical residual — so the sign check is a belt-and-braces refusal.
                let landing = predict_shift_landing_m(tp, fp, m, r_mean, dt);
                let landing_shaft = dir * landing;
                let next_reserve = modeled_reserve_in_gear(tp, fp, shaft, g_up, st.demand_n);
                if landing_shaft > 0.0
                    && (protective_upshift
                        || shaft_rpm_of(landing_shaft, g_up)
                            >= tp.shift_down_rpm + POSTSHIFT_MARGIN_RPM)
                    && next_reserve >= reserve_margin(st.demand_n)
                {
                    st.gear += 1;
                    st.shift_ticks = tp.shift_ticks;
                    st.last_shift_dir = 1;
                    st.dwell_ticks = REVERSAL_DWELL_TICKS;
                    st.grade_confirm_ticks = 0;
                    st.grade_target = 0;
                    st.scheduler = SchedulerState::Normal;
                }
            } else if !grade_step_committed
                && shaft > -PARK_ENGAGE_SPEED
                && rpm < tp.shift_down_rpm
                && st.gear > 1
                // A persistent capability deficit is owned by the confirmed reserve branch above;
                // the established band path remains unchanged for ordinary capable slowdowns.
                && current_reserve >= 0.0
                && !dwell_blocks(-1)
            {
                // Backslide hold (stage A, thresholded in the review round): while GENUINELY
                // back-driven the vehicle is NOT "running slow forward" — gear changes are
                // decisions about forward operation, and a FREE backslide HOLDS the engaged gear
                // (the negative signed rpm would otherwise downshift-walk forever). The one narrow
                // exception is handled above: when a latched hill hold's current gear plus declared
                // brakes cannot arrest the slide, `refresh_hill_hold` may downshift to gain that
                // capability while the shaft is negative. The threshold here is
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
                    st.grade_confirm_ticks = 0;
                    st.grade_target = 0;
                    st.scheduler = SchedulerState::Normal;
                }
            }
        }
        if st.hill_hold && st.scheduler != SchedulerState::GradeLimit {
            st.scheduler = SchedulerState::HillHold;
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

    // --- Engine crank state ω_e (stage B). The crank is real state with inertia J; stage
    // A's command-proxy rev floor is DEAD (launch rpm is now the emergent clutch-slip
    // equilibrium). The crank is NEVER negative — it cannot follow a back-driven shaft
    // (stage A's principle, now enforced by the stall guard instead of a floor).
    let omega_idle = tp.engine.idle_rpm * RPM_TO_RAD;
    if st.omega_e == 0.0 {
        // Uninitialized sentinel, EXACTLY 0.0 (review round FIX 2): Default cannot see
        // the spec — the state must be constructible before the spec arrives (the
        // pre-sized-slab spawn invariant) — so the first step with params snaps to idle.
        // A stepped crank can never return to 0.0: the end-of-tick hard floor keeps it
        // ≥ ω_floor > 0 (spec requires idle_rpm ≥ 300 > STALL_GUARD_BAND_RPM + margin),
        // so this branch fires at true spawn only — a broader `≤ 0` arm would instead
        // TELEPORT a floor-violating crank back to idle every tick (the traced
        // 600 → −3130 → 600 rpm oscillation under a legal strongly-negative-τ_free spec).
        st.omega_e = omega_idle;
    }
    let omega_e = st.omega_e;
    let k = g / tp.sprocket_radius;
    let omega_floor = (tp.engine.idle_rpm - STALL_GUARD_BAND_RPM) * RPM_TO_RAD;

    // COUPLING seam: engaged ⇔ not shifting ∧ not the neutral-idle regime. The neutral-idle
    // regime generalizes the L600 neutral-turn seam to BOTH regenerative adapters — no
    // propulsive drive near standstill means the driver has the main clutch out (an engaged
    // idle-governed crank at standstill would otherwise ride the clutch: idle torque through
    // a first-gear reduction is hundreds of kN of spurious creep/pivot-drag force). Keyed on
    // `propulsive` (not |throttle|) so a service-brake command at speed stays engaged
    // (engine braking through the coupling); the L600's own steering-regime check keeps its
    // historical |throttle| form — the seams coincide except transiently under
    // opposing-throttle-at-standstill, where the direction swap + shift window take over
    // within a tick.
    //
    // Review round FIX 3: the seam is a LATCH with hysteresis (`st.clutch_out`, the
    // steering-detent doctrine), not a single threshold — a boundary creeper chattered
    // engage/declutch on the bare NEUTRAL_M_SPEED line. Any propulsive command re-engages
    // at any speed (the launch); otherwise the belt must fall below CLUTCH_OUT_M_SPEED to
    // take the clutch out and climb past CLUTCH_IN_M_SPEED to put it back in.
    if propulsive >= NEUTRAL_THROTTLE || m.abs() >= CLUTCH_IN_M_SPEED {
        st.clutch_out = false;
    } else if m.abs() < CLUTCH_OUT_M_SPEED {
        st.clutch_out = true;
    }
    let engaged = !shifting && !st.clutch_out;

    // Fueling demand u. Engaged: the propulsive throttle (a brake command is not fueling).
    // Declutched: a proportional-band rev governor ([`REV_MATCH_BAND_RPM`]) toward the
    // LARGER of two targets —
    //   * the REV-MATCH target `|m|·k` (st.gear is already the landing gear during the
    //     window), so the clutch re-engages near-synchronous;
    //   * the STEER demand target `idle + (peak_torque_rpm − idle)·|steer|`: the steering
    //     member is engine-driven in every regime, so a steer command revs the crank
    //     whether or not the main clutch is out — the surviving half of the old `cmd_mag`
    //     rev-floor contract, now reached DYNAMICALLY (pivot power spools with the crank).
    // Deliberate deviations from the memo's shorthand (`τ_ind = propulsive·torque_at`,
    // `u_match` bang-bang), both documented for review: (1) without the steer target a
    // declutched pivot would idle at ~1/5 of its power budget and the memo's own spin-up
    // expectation (crank spool preceding pivot spool) could not occur; (2) the target is a
    // SPEED, not blind full fueling, because an unloaded crank under u = 1 spools past the
    // peak-power point to the governor cut-out where `torque_at·ω = 0` — the d-path draw
    // does not load the crank in this stage (deferred honestly; the power gate caps the
    // draw instead), so the steer demand must PARK the crank at the peak-torque point the
    // old floor used, or steady pivot power collapses to zero.
    let u_fuel = if engaged {
        propulsive
    } else {
        let omega_match = m.abs() * k;
        let omega_steer =
            omega_idle + (tp.peak_torque_rpm * RPM_TO_RAD - omega_idle) * inp.steer.abs().min(1.0);
        let omega_target = omega_match.max(omega_steer);
        ((omega_target - omega_e) / (REV_MATCH_BAND_RPM * RPM_TO_RAD)).clamp(0.0, 1.0)
    };

    // Free torque from the PRE-tick crank speed: induced torque at the crank's own rpm
    // (the governor cut now acts on the crank), the idle-governor recovery (linear over
    // K_IDLE_DROOP_RPM below idle, saturating at torque_at(idle) — it may stack over τ_ind
    // below idle: the governor stand-in's over-fueling stall resistance, bounded by the
    // clutch capacity and charged by the power gate), minus engine-side compression drag.
    let rpm = omega_e / RPM_TO_RAD;
    let idle_gain = tp.torque_at(tp.engine.idle_rpm) / (K_IDLE_DROOP_RPM * RPM_TO_RAD);
    let tau_idle =
        (idle_gain * (omega_idle - omega_e)).clamp(0.0, tp.torque_at(tp.engine.idle_rpm));
    let tau_ind = u_fuel * tp.torque_at(rpm);
    let tau_free = tau_ind + tau_idle - engine_drag(tp, omega_e, u_fuel);

    // Power available at the crank's operating point — the energy gate's per-tick bound.
    // Follows the crank, not the input slew: a standstill pivot's power SPOOLS as the
    // crank revs (the measured spin-up), and a lugged crank offers lug power.
    let p_avail = tp.torque_at(rpm) * omega_e;

    // The engine force on the mean belt axis: the coupling's transmitted torque reflected
    // through the gear (in place of the old f_p + f_drag — drag reaches the belt only
    // through the coupling now). Declutched, the belt gets NOTHING from the engine.
    let i_m = 2.0 * fp.inertia;
    let tau_c = if engaged {
        clutch_coupling(
            tp.engine_inertia,
            tp.clutch_capacity,
            k,
            dir,
            omega_e,
            m,
            tau_free,
            i_m,
            -(inp.reactions[0] + inp.reactions[1]),
            omega_floor,
            dt,
        )
    } else {
        0.0
    };
    let mut f_c = k * dir * tau_c;

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
                let a_l = f_c / 2.0 - inp.reactions[0];
                let a_r = f_c / 2.0 - inp.reactions[1];
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
    let p_l = (f_c + f_s) / 2.0 * vl;
    let p_r = (f_c - f_s) / 2.0 * vr;
    let pos = p_l.max(0.0) + p_r.max(0.0);
    let neg = (-p_l).max(0.0) + (-p_r).max(0.0);
    let net = pos - tp.recirculation * neg;
    let power_scale = if net > p_avail && net > 0.0 {
        p_avail / net
    } else {
        1.0
    };
    f_c *= power_scale;
    f_s *= power_scale;

    // Release is capability-based, not timer-based: only force actually transmitted through the
    // coupling (after the power gate) may hand the slope from the modeled brakes back to the
    // drivetrain. The release threshold is
    //
    //   D + min(selection_margin, max(0, modeled_selected_reserve) * 0.5).
    //
    // Thus a full-margin gear keeps the ordinary headroom, while a margin-short but capable gear
    // can release once it transmits its own modeled force: half its non-negative reserve is always
    // below that force. Equality is accepted for the zero-reserve knife edge. The hold remains
    // through every declutched window because `f_c = 0` there.
    let hill_brake_active = st.hill_hold;
    let selected_reserve = modeled_reserve_in_gear(tp, fp, shaft, g, st.demand_n);
    let release_margin = reserve_margin(st.demand_n).min(selected_reserve.max(0.0) * 0.5);
    if st.hill_hold && dir * f_c >= st.demand_n + release_margin {
        st.hill_hold = false;
        st.hold_reengage_ticks = HOLD_REENGAGE_TICKS;
        st.scheduler = SchedulerState::Normal;
        st.grade_target = 0;
    }

    // --- Integrate the crank: J·ω̇_e = τ_free − τ_c (the transmitted torque scaled by the
    // power gate exactly as the belt-side force was — one bookkeeping for both ends of the
    // clutch; a bound power gate leaves MORE speed on the crank, never less, so the stall
    // guard's floor promise survives scaling).
    st.omega_e = omega_e + (tau_free - tau_c * power_scale) * dt / tp.engine_inertia;

    // --- Assemble per-side sprocket forces.
    let mut q = [
        f_c / 2.0 + f_s / 2.0 + lambda * j[0],
        f_c / 2.0 - f_s / 2.0 + lambda * j[1],
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
        let envelope = h
            .max(service)
            .max(if hill_brake_active { 1.0 } else { 0.0 });
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

    // --- Drift kill / re-anchor (stage B + review round FIX 1): snap the crank to the
    // belt that ACTUALLY integrated only if the snap is FEASIBLE — the implied TOTAL
    // clutch torque `τ_impl = τ_free − (k·s·m_next − ω_e)·J/dt` must fit the capacity.
    // The coupling pre-solve's F_other (reactions only) is a PREDICTOR approximation:
    // brakes, the FixedRadii λ mean-axis share, and the belt speed clamp all move m_next
    // after it, and exact pre-accounting is circular (the brake law reads the q that
    // needs F_c) — so feasibility is decided HERE, on the final m_next, not on the
    // pre-solve's stale clamp flag. An eager flag let a full-opposing-throttle brake tick
    // snap the crank down the belt's brake-driven drop, implying ≈ 9.7 kN·m through a
    // 2.4 kN·m clutch (the traced teleport); an infeasible snap now leaves the honestly
    // integrated crank — the clutch is slipping, and that is the truth. Inside capacity
    // the snap is a legitimate clutch outcome regardless of the power gate (any
    // within-capacity landing is reachable), so no power_scale condition. Still guarded
    // by the stall floor: a snap may never land the crank below it.
    if engaged {
        let m_next = (next[0] + next[1]) / 2.0;
        let locked = k * dir * m_next;
        let tau_impl = tau_free - (locked - omega_e) * tp.engine_inertia / dt;
        if tau_impl.abs() <= tp.clutch_capacity && locked >= omega_floor {
            st.omega_e = locked;
        }
    }

    // --- Hard stall floor (review round FIX 2): the crank never ENDS a tick below
    // ω_floor, whatever the spec's torque/drag corner did to τ_free — the floor IS the
    // no-stall policy while stall death stays deliberately unmodeled (classification
    // table). This is also what makes the ω_e = 0.0 spawn sentinel unambiguous, and it
    // self-heals a NaN (f32::max drops the NaN operand).
    st.omega_e = st.omega_e.max(omega_floor);

    TransmissionReport {
        next_speeds: next,
        forces: q,
        // The crank state, post-tick — the report shows the same truth `readout` does.
        rpm: st.omega_e / RPM_TO_RAD,
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
            // Stage B lab crank: same class band as the vehicle authoring (J mid-band,
            // clutch ≈ 1.3 × the 2200 N·m peak).
            engine_inertia_kgm2: 4.0,
            clutch_capacity_nm: 2860.0,
            shift_secs: 0.31,
            shift_addressing: ShiftAddressing::Sequential,
            sprocket_radius_m: 0.34,
            half_tread_m: 1.25,
        })
    }

    /// The shipped Tiger's declared drivetrain, kept local to arithmetic tests so they exercise
    /// the same authored curve and speed anchors without reaching through the ECS/spec adapter.
    fn tiger_tp() -> TransmissionParams {
        TransmissionParams::from_authoring(&TransmissionAuthoring {
            idle_rpm: 600.0,
            governed_rpm: 2500.0,
            rated_rpm: 3000.0,
            torque_nm: &[
                (800.0, 1300.0),
                (2100.0, 1850.0),
                (2500.0, 1686.0),
                (3000.0, 1639.0),
            ],
            forward_speeds_kmh: &[2.8, 4.3, 6.2, 9.2, 14.1, 20.9, 30.5, 45.4],
            reverse_speeds_kmh: &[2.8, 4.3, 6.2, 9.2],
            shift_up_rpm: 2300.0,
            shift_down_rpm: 1400.0,
            steer_radii_m: &[
                (3.44, 10.2),
                (5.28, 15.6),
                (7.62, 22.5),
                (11.30, 33.4),
                (17.32, 51.2),
                (25.68, 76.0),
                (37.47, 110.8),
                (55.78, 165.0),
            ],
            steer_capacity_n: 250_000.0,
            recirculation: 0.9,
            brake_capacity_n: 96_000.0,
            drag_fraction: 0.25,
            engine_inertia_kgm2: 4.0,
            clutch_capacity_nm: 2400.0,
            shift_secs: 0.31,
            shift_addressing: ShiftAddressing::Direct,
            sprocket_radius_m: 19.0 * 0.130 / std::f32::consts::TAU,
            half_tread_m: 1.4904,
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

    /// Stage C reserve arithmetic at the slope investigation's reconstructed operating point.
    /// At the belt speed that puts Tiger F4 at 980 rpm DERIVED, its authored curve gives about
    /// 169 kN DERIVED total sprocket force (the investigation's 165 kN DERIVED rounding), below the DERIVED 20°
    /// grade demand `57_000 * 9.81 * sin(20°) = 191.2 kN`. F3 at the same speed has enough
    /// reserve to clear the DERIVED 10% + absolute margin.
    #[test]
    fn reserve_uses_authored_curve_and_traction_cap() {
        let tp = tiger_tp();
        let mut fp = lab_fp();
        fp.engine_force = 250_000.0;
        let f4 = tp.gears_fwd[3];
        let shaft = 980.0 * RPM_TO_RAD * tp.sprocket_radius / f4;
        let demand = 57_000.0 * 9.81 * 20.0_f32.to_radians().sin();

        let force_f4 = available_force_in_gear(&tp, &fp, shaft, f4);
        let force_f3 = available_force_in_gear(&tp, &fp, shaft, tp.gears_fwd[2]);
        let margin = reserve_margin(demand);

        assert!(
            (165_000.0..=172_000.0).contains(&force_f4),
            "F4 @ 980 rpm must reconstruct the investigation's ~165 kN force (got {force_f4:.0})"
        );
        assert!(
            force_f4 - demand < 0.0,
            "F4 must be in reserve deficit on 20° ({force_f4:.0} - {demand:.0})"
        );
        assert!(
            force_f3 - demand >= margin,
            "F3 must clear the reserve margin ({force_f3:.0} - {demand:.0} >= {margin:.0})"
        );
    }

    /// Stage C composes reserve with (rather than replacing) the established upshift gates.
    /// With zero window and isolated bands, the slope investigation's DERIVED operating point
    /// puts F4 at 980 rpm and F3 just above the test's up band. It therefore shifts on flat
    /// ground. Under the DERIVED 191.2 kN 20-degree load, F4's reserve is below the DERIVED
    /// 10% + 10 kN policy margin, so the otherwise-identical upshift is vetoed.
    #[test]
    fn grade_reserve_veto_blocks_f3_to_f4_on_20_degrees() {
        let mut tp = tiger_tp();
        tp.shift_ticks = 0;
        tp.shift_up_rpm = 980.0 * tp.gears_fwd[2] / tp.gears_fwd[3] - 1.0;
        tp.shift_down_rpm = 800.0;
        let mut fp = lab_fp();
        fp.engine_force = 250_000.0;
        fp.inertia = 16_000.0;
        let shaft = 980.0 * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[3];

        let mut flat = TransmissionState {
            gear: 3,
            ..Default::default()
        };
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut flat,
            &input(1.0, 0.0, [shaft, shaft], [0.0, 0.0]),
        );
        assert_eq!(
            flat.gear, 4,
            "the accepted flat-ground upshift must stay intact"
        );

        let demand = 57_000.0 * 9.81 * 20.0_f32.to_radians().sin();
        let mut grade = TransmissionState {
            gear: 3,
            ..Default::default()
        };
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut grade,
            &input(1.0, 0.0, [shaft, shaft], [demand / 2.0; 2]),
        );
        assert_eq!(
            grade.gear, 3,
            "the 20-degree reserve deficit must veto F3 -> F4"
        );
    }

    /// A reserve deficit must persist for the full 13 DERIVED decision ticks. Warm the DERIVED
    /// eight-tick EMA on flat ground, inject a 12-tick DERIVED 20-degree demand spike at an F5 operating point where F5 is
    /// deficient but F4 is capable, then remove it. Filtering plus confirmation must reject the
    /// transient without commanding any shift.
    #[test]
    fn transient_reserve_deficit_shorter_than_confirmation_does_not_downshift() {
        let mut tp = tiger_tp();
        tp.shift_ticks = 0;
        let mut fp = lab_fp();
        fp.engine_force = 250_000.0;
        fp.inertia = 16_000.0;
        let shaft = 1500.0 * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[4];
        let demand = 57_000.0 * 9.81 * 20.0_f32.to_radians().sin();
        let mut st = TransmissionState {
            gear: 5,
            ..Default::default()
        };

        for _ in 0..32 {
            step(
                TransmissionMode::FixedRadii,
                &fp,
                Some(&tp),
                &mut st,
                &input(1.0, 0.0, [shaft; 2], [0.0; 2]),
            );
        }
        for _ in 0..(GRADE_CONFIRM_TICKS - 1) {
            step(
                TransmissionMode::FixedRadii,
                &fp,
                Some(&tp),
                &mut st,
                &input(1.0, 0.0, [shaft; 2], [demand / 2.0; 2]),
            );
        }
        for _ in 0..32 {
            step(
                TransmissionMode::FixedRadii,
                &fp,
                Some(&tp),
                &mut st,
                &input(1.0, 0.0, [shaft; 2], [0.0; 2]),
            );
        }

        assert_eq!(
            st.gear, 5,
            "a sub-confirmation load spike must not downshift"
        );
        assert_eq!(
            st.grade_confirm_ticks, 0,
            "the cleared deficit resets confirmation"
        );
    }

    /// The scheduler names one capability target; addressing changes only how it is executed.
    /// This custom band setting isolates the reserve path at Tiger F6 = 600 rpm DERIVED test input under the
    /// DERIVED 20-degree demand: F4 lacks margin, F3 clears it, and F2 would over-rev. Direct
    /// commits F6 -> F3 in one event; Sequential pays F6 -> F5 first and holds F3 across the
    /// remaining windows.
    #[test]
    fn direct_and_sequential_execute_the_same_grade_target_differently() {
        let mut base = tiger_tp();
        base.shift_down_rpm = 0.0;
        base.shift_ticks = 2;
        let mut fp = lab_fp();
        fp.engine_force = 250_000.0;
        fp.inertia = 16_000.0;
        let shaft = 600.0 * RPM_TO_RAD * base.sprocket_radius / base.gears_fwd[5];
        let demand = 57_000.0 * 9.81 * 20.0_f32.to_radians().sin();
        let seeded = || TransmissionState {
            gear: 6,
            demand_n: demand,
            demand_initialized: true,
            grade_confirm_ticks: GRADE_CONFIRM_TICKS - 1,
            ..Default::default()
        };

        let mut direct_tp = base.clone();
        direct_tp.shift_addressing = ShiftAddressing::Direct;
        let mut direct = seeded();
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&direct_tp),
            &mut direct,
            &input(1.0, 0.0, [shaft; 2], [demand / 2.0; 2]),
        );
        assert_eq!(
            direct.gear, 3,
            "Direct must commit straight to the legal target"
        );
        assert_eq!(
            direct.scheduler,
            SchedulerState::GradeShift { from: 6, to: 3 }
        );

        let mut sequential_tp = base;
        sequential_tp.shift_addressing = ShiftAddressing::Sequential;
        let mut sequential = seeded();
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&sequential_tp),
            &mut sequential,
            &input(1.0, 0.0, [shaft; 2], [demand / 2.0; 2]),
        );
        assert_eq!(
            sequential.gear, 5,
            "Sequential may move only one adjacent gear"
        );
        assert_eq!(
            sequential.grade_target, 3,
            "Sequential must retain the F3 target"
        );
        assert_eq!(
            sequential.scheduler,
            SchedulerState::GradeShift { from: 6, to: 3 }
        );

        for _ in 0..8 {
            step(
                TransmissionMode::FixedRadii,
                &fp,
                Some(&sequential_tp),
                &mut sequential,
                &input(1.0, 0.0, [shaft; 2], [demand / 2.0; 2]),
            );
        }
        assert_eq!(
            sequential.gear, 3,
            "Sequential must eventually reach the held target"
        );
    }

    /// Direct addressing never bypasses the signed landing gate. The same F6 -> F3 reserve target
    /// as the addressing test is presented with the 20-tick DERIVED window; freezing the DERIVED
    /// 20-degree reaction through that cut predicts `landing_m < 0`, so no grade shift may commit.
    #[test]
    fn direct_skip_refuses_a_predicted_backward_landing() {
        let mut tp = tiger_tp();
        tp.shift_down_rpm = 0.0;
        tp.shift_ticks = 20;
        tp.shift_addressing = ShiftAddressing::Direct;
        let mut fp = lab_fp();
        fp.engine_force = 250_000.0;
        fp.inertia = 16_000.0;
        let shaft = 600.0 * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[5];
        let demand = 57_000.0 * 9.81 * 20.0_f32.to_radians().sin();
        let mut st = TransmissionState {
            gear: 6,
            demand_n: demand,
            demand_initialized: true,
            grade_confirm_ticks: GRADE_CONFIRM_TICKS - 1,
            ..Default::default()
        };

        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [shaft; 2], [demand / 2.0; 2]),
        );
        assert_eq!(
            st.gear, 6,
            "a sign-flipped direct landing must hold the engaged gear"
        );
        assert_eq!(st.shift_ticks, 0, "no interruption window may start");
        assert_eq!(st.scheduler, SchedulerState::Normal);
    }

    /// Hill hold is a stateful use of the existing brake law, not an extra force. At rest on the
    /// DERIVED Tiger 20-degree load, F5 has negative reserve, so held W engages the flag and
    /// Direct-addresses capable F3 while the full service-brake envelope keeps both belts stopped.
    /// Once the shift ends, F3 transmits more than demand + margin; the hold releases and the same
    /// tick begins a forward launch. Releasing W always clears the flag.
    #[test]
    fn hill_hold_engages_selects_launch_gear_and_releases_on_capability() {
        let mut tp = tiger_tp();
        tp.shift_ticks = 2;
        tp.shift_addressing = ShiftAddressing::Direct;
        let mut fp = lab_fp();
        fp.engine_force = 250_000.0;
        fp.inertia = 16_000.0;
        let demand = 57_000.0 * 9.81 * 20.0_f32.to_radians().sin();
        let mut st = TransmissionState {
            gear: 5,
            ..Default::default()
        };

        let first = step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [0.0; 2], [demand / 2.0; 2]),
        );
        assert!(
            st.hill_hold,
            "negative launch reserve must engage hill hold"
        );
        assert_eq!(
            st.gear, 3,
            "the hold must Direct-address the capable launch gear"
        );
        assert_eq!(st.scheduler, SchedulerState::HillHold);
        assert_eq!(
            first.next_speeds, [0.0; 2],
            "the modeled brakes hold through the cut"
        );

        let mut released = None;
        let mut speeds = first.next_speeds;
        for tick in 1..8 {
            let report = step(
                TransmissionMode::FixedRadii,
                &fp,
                Some(&tp),
                &mut st,
                &input(1.0, 0.0, speeds, [demand / 2.0; 2]),
            );
            speeds = report.next_speeds;
            if !st.hill_hold {
                released = Some(tick);
                break;
            }
        }
        assert!(
            released.is_some(),
            "capable F3 must release the hold after its window"
        );
        assert!(
            speeds[0] > 0.0 && speeds[1] > 0.0,
            "release must begin a forward launch"
        );
        assert_eq!(st.scheduler, SchedulerState::Normal);

        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(0.0, 0.0, speeds, [demand / 2.0; 2]),
        );
        assert!(!st.hill_hold, "command release always disengages hill hold");
    }

    /// D1 regression: a latched hold is a live capability decision, not a one-shot edge. A demand
    /// sample that makes a lower gear capable must clear the stale GRADE LIMIT state and retarget.
    #[test]
    fn hill_hold_rechecks_capability_while_latched() {
        let mut tp = tiger_tp();
        tp.shift_ticks = 0;
        let mut fp = lab_fp();
        fp.engine_force = 250_000.0;
        fp.inertia = 16_000.0;
        let demand = 57_000.0 * 9.81 * 20.0_f32.to_radians().sin();
        let mut st = TransmissionState {
            gear: 5,
            demand_n: demand,
            demand_initialized: true,
            scheduler: SchedulerState::GradeLimit,
            hill_hold: true,
            ..Default::default()
        };

        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [0.0; 2], [demand / 2.0; 2]),
        );

        assert_eq!(st.gear, 3, "the live hold must retarget the capable F3");
        assert_ne!(
            st.scheduler,
            SchedulerState::GradeLimit,
            "a capable gear must clear stale GRADE LIMIT truth"
        );
    }

    /// D1 regression: if the selected launch gear has non-negative reserve but cannot clear the
    /// full scheduler margin, transmitting its modeled force must still release the hold. The
    /// release margin is deliberately smaller than the selection margin in this case.
    #[test]
    fn hill_hold_margin_short_capable_gear_can_release() {
        let mut tp = tiger_tp();
        tp.shift_ticks = 0;
        let mut fp = lab_fp();
        fp.engine_force = 250_000.0;
        fp.inertia = 16_000.0;
        let k = tp.gears_fwd[0] / tp.sprocket_radius;
        tp.clutch_capacity = 2.0 * fp.engine_force / k;
        let modeled = available_force_in_gear(&tp, &fp, 0.0, tp.gears_fwd[0]);
        let demand = modeled - 10_000.0;
        assert!(
            modeled - demand < reserve_margin(demand),
            "fixture gear must be capable but margin-short"
        );
        let mut st = TransmissionState {
            gear: 1,
            omega_e: tp.engine.idle_rpm * RPM_TO_RAD,
            demand_n: demand,
            demand_initialized: true,
            scheduler: SchedulerState::HillHold,
            hill_hold: true,
            ..Default::default()
        };

        let report = step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [0.0; 2], [demand / 2.0; 2]),
        );

        assert!(
            report.forces[0] + report.forces[1] >= demand,
            "fixture must transmit at least demand"
        );
        assert!(!st.hill_hold, "a capable margin-short gear must release");
    }

    /// D1c regression: a successful handoff suppresses near-rest relatching for the fixed cooldown,
    /// but genuine backward motion beyond the engagement threshold overrides it immediately.
    #[test]
    fn hill_hold_release_cooldown_yields_to_real_rollback() {
        let mut tp = tiger_tp();
        tp.shift_ticks = 0;
        let mut fp = lab_fp();
        fp.engine_force = 250_000.0;
        fp.inertia = 16_000.0;
        let k = tp.gears_fwd[0] / tp.sprocket_radius;
        tp.clutch_capacity = 2.0 * fp.engine_force / k;
        let modeled = available_force_in_gear(&tp, &fp, 0.0, tp.gears_fwd[0]);
        let release_demand = modeled - 10_000.0;
        let mut st = TransmissionState {
            omega_e: tp.engine.idle_rpm * RPM_TO_RAD,
            demand_n: release_demand,
            demand_initialized: true,
            scheduler: SchedulerState::HillHold,
            hill_hold: true,
            ..Default::default()
        };

        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [0.0; 2], [release_demand / 2.0; 2]),
        );
        assert!(!st.hill_hold);
        assert_eq!(st.hold_reengage_ticks, HOLD_REENGAGE_TICKS);

        let deficit_demand = modeled + 100_000.0;
        st.demand_n = deficit_demand;
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [0.0; 2], [deficit_demand / 2.0; 2]),
        );
        assert!(!st.hill_hold, "near-rest chatter must respect the cooldown");

        st.demand_n = deficit_demand;
        let rollback = -(HILL_HOLD_ENGAGE_SPEED + 0.01);
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [rollback; 2], [deficit_demand / 2.0; 2]),
        );
        assert!(st.hill_hold, "a real rollback must override the cooldown");
    }

    fn correction_priority_fixture() -> (ForceParams, TransmissionParams, f32, f32) {
        let mut fp = lab_fp();
        fp.engine_force = 1_000_000.0;
        let mut tp = lab_tp();
        tp.shift_ticks = 0;
        tp.engine.governed_rpm = 4_000.0;
        tp.engine.torque_nm = vec![
            (0.0, 100.0),
            (1_100.0, 2_000.0),
            (1_800.0, 100.0),
            (2_900.0, 2_000.0),
            (4_000.0, 2_000.0),
        ];
        let shaft = 1_800.0 * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[1];
        let current = available_force_in_gear(&tp, &fp, shaft, tp.gears_fwd[1]);
        let lower = available_force_in_gear(&tp, &fp, shaft, tp.gears_fwd[0]);
        let upper = available_force_in_gear(&tp, &fp, shaft, tp.gears_fwd[2]);
        let demand = current + 12_000.0;
        assert!(lower - demand >= reserve_margin(demand));
        assert!(upper - demand >= reserve_margin(demand));
        (fp, tp, shaft, demand)
    }

    /// D2 regression: a threshold-confirmed reserve deficit is a correction and must beat an
    /// otherwise-valid above-band upshift preference on the same decision tick.
    #[test]
    fn confirmed_deficit_precedes_upshift_arm() {
        let (fp, tp, shaft, demand) = correction_priority_fixture();
        let mut st = TransmissionState {
            gear: 2,
            demand_n: demand,
            demand_initialized: true,
            grade_confirm_ticks: GRADE_CONFIRM_TICKS - 1,
            ..Default::default()
        };

        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [shaft; 2], [demand / 2.0; 2]),
        );

        assert_eq!(st.gear, 1, "confirmed deficit must correct downward");
    }

    /// D5 regression: reversal dwell protects band preferences from hunting; it must not block the
    /// correction after the full reserve-deficit confirmation has already been paid.
    #[test]
    fn confirmed_deficit_bypasses_reversal_dwell() {
        let (fp, mut tp, shaft, demand) = correction_priority_fixture();
        tp.shift_up_rpm = 10_000.0;
        let mut st = TransmissionState {
            gear: 2,
            last_shift_dir: 1,
            dwell_ticks: REVERSAL_DWELL_TICKS,
            demand_n: demand,
            demand_initialized: true,
            grade_confirm_ticks: GRADE_CONFIRM_TICKS - 1,
            ..Default::default()
        };

        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [shaft; 2], [demand / 2.0; 2]),
        );

        assert_eq!(
            st.gear, 1,
            "confirmed correction must ignore reversal dwell"
        );
    }

    /// D3 regression: every sequential continuation is selected again. Releasing propulsive intent
    /// cancels the held target instead of paying another stale adjacent shift window.
    #[test]
    fn sequential_target_cancels_when_propulsive_intent_releases() {
        let mut tp = tiger_tp();
        tp.shift_ticks = 0;
        tp.shift_addressing = ShiftAddressing::Sequential;
        let fp = lab_fp();
        let shaft = 1_700.0 * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[4];
        let mut st = TransmissionState {
            gear: 5,
            demand_initialized: true,
            grade_target: 3,
            scheduler: SchedulerState::GradeShift { from: 6, to: 3 },
            ..Default::default()
        };

        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(0.0, 0.0, [shaft; 2], [0.0; 2]),
        );

        assert_eq!(st.gear, 5, "released intent must not continue the cascade");
        assert_eq!(st.grade_target, 0);
        assert_eq!(st.scheduler, SchedulerState::Normal);
    }

    /// D3 regression: a held sequential target also cancels when the filtered demand recovers and
    /// the current gear is no longer deficient, even if the driver continues holding throttle.
    #[test]
    fn sequential_target_cancels_when_demand_recovers() {
        let (fp, mut tp, shaft, _) = correction_priority_fixture();
        tp.shift_addressing = ShiftAddressing::Sequential;
        tp.shift_up_rpm = 10_000.0;
        tp.shift_down_rpm = 0.0;
        let mut st = TransmissionState {
            gear: 2,
            demand_initialized: true,
            grade_target: 1,
            scheduler: SchedulerState::GradeShift { from: 3, to: 1 },
            ..Default::default()
        };

        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [shaft; 2], [0.0; 2]),
        );

        assert_eq!(st.gear, 2, "recovered demand must not continue the cascade");
        assert_eq!(st.grade_target, 0);
        assert_eq!(st.scheduler, SchedulerState::Normal);
    }

    /// D3 / finding 8a regression: F- and R-ladder demand projections have opposite signs. A
    /// direction swap must discard the old EMA and seed directly from the new ladder's sample.
    #[test]
    fn direction_swap_reseeds_demand_ema() {
        let mut tp = tiger_tp();
        tp.shift_ticks = 0;
        let fp = lab_fp();
        let reverse_demand = 20_000.0;
        let mut st = TransmissionState {
            demand_n: 100_000.0,
            demand_initialized: true,
            ..Default::default()
        };

        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(-1.0, 0.0, [0.0; 2], [-reverse_demand / 2.0; 2]),
        );

        assert!(st.reverse);
        assert_eq!(st.demand_n.to_bits(), reverse_demand.to_bits());
    }

    /// D8 regression: one capable sample decays accumulated evidence by one tick; it does not erase
    /// twelve prior deficit samples. Two more deficit samples must therefore confirm the correction.
    #[test]
    fn reserve_confirmation_decays_across_one_tick_jitter() {
        let mut tp = tiger_tp();
        tp.shift_ticks = 0;
        tp.shift_up_rpm = 10_000.0;
        tp.shift_down_rpm = 0.0;
        let mut fp = lab_fp();
        fp.engine_force = 250_000.0;
        let shaft = 600.0 * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[5];
        let demand = 57_000.0 * 9.81 * 20.0_f32.to_radians().sin();
        let mut st = TransmissionState {
            gear: 6,
            demand_initialized: true,
            ..Default::default()
        };

        for _ in 0..(GRADE_CONFIRM_TICKS - 1) {
            st.demand_n = demand;
            step(
                TransmissionMode::FixedRadii,
                &fp,
                Some(&tp),
                &mut st,
                &input(1.0, 0.0, [shaft; 2], [demand / 2.0; 2]),
            );
        }
        st.demand_n = 0.0;
        step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(1.0, 0.0, [shaft; 2], [0.0; 2]),
        );
        for _ in 0..2 {
            st.demand_n = demand;
            step(
                TransmissionMode::FixedRadii,
                &fp,
                Some(&tp),
                &mut st,
                &input(1.0, 0.0, [shaft; 2], [demand / 2.0; 2]),
            );
        }

        assert!(
            st.gear < 6,
            "decayed evidence must still reach confirmation"
        );
    }

    /// D6 regression: a positive signed shaft driven beyond the governed range by a downhill load
    /// receives a protective upshift with the throttle released, and the declutched crank slows.
    #[test]
    fn downhill_overrun_protective_upshift_lowers_crank_speed() {
        let mut tp = tiger_tp();
        tp.shift_ticks = 2;
        let mut fp = lab_fp();
        fp.engine_force = 250_000.0;
        fp.inertia = 16_000.0;
        let overrun_rpm = tp.engine.governed_rpm + 200.0;
        let shaft = overrun_rpm * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[3];
        let mut st = TransmissionState {
            gear: 4,
            omega_e: overrun_rpm * RPM_TO_RAD,
            demand_initialized: true,
            ..Default::default()
        };

        let report = step(
            TransmissionMode::FixedRadii,
            &fp,
            Some(&tp),
            &mut st,
            &input(0.0, 0.0, [shaft; 2], [-40_000.0; 2]),
        );

        println!(
            "protective overrun: F4 shaft {overrun_rpm:.1} rpm -> F{}; crank {overrun_rpm:.1} -> {:.1} rpm",
            st.gear, report.rpm
        );
        assert_eq!(
            st.gear, 5,
            "overrun must protectively upshift while coasting"
        );
        assert!(
            report.rpm < overrun_rpm,
            "declutched crank must slow from {overrun_rpm:.1} rpm, got {:.1}",
            report.rpm
        );
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
            engine_inertia_kgm2: 4.0,
            clutch_capacity_nm: 2860.0,
            shift_secs: 0.31,
            shift_addressing: ShiftAddressing::Sequential,
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

    /// Stage B: the HUD readout reports the CRANK STATE ω_e directly — the state IS the
    /// display. Unstepped (sentinel), it reads idle; driving in reverse it reads the
    /// crank's geared speed with the R label; back-driven forward while in R (the stage-A
    /// scenario), the crank cannot follow the negative shaft — the stall guard keeps ω_e
    /// idle-ish, and the readout shows exactly that state, never a fake forward rpm.
    #[test]
    fn readout_reports_crank_state() {
        let (fp, tp) = (lab_fp(), lab_tp());
        // Sentinel (constructed, never stepped): idle.
        let st = TransmissionState {
            reverse: true,
            ..Default::default()
        };
        let r = readout(&st, &tp);
        assert_eq!(r.gear_label, "R1");
        assert_eq!(
            r.rpm, tp.engine.idle_rpm,
            "the unstepped sentinel must read idle"
        );
        // Driving in reverse at a steady R1 speed: the lock puts the crank AT the geared
        // speed of the belt the transmission itself integrated (`k·s·m_next` — with this
        // harness holding the INPUT speeds externally, the belt it computes each tick sits
        // `k·τ_free·dt/I_m` above the held value, and the crank rides THAT belt exactly).
        let mut st = TransmissionState {
            reverse: true,
            ..Default::default()
        };
        let mut rep = TransmissionReport::default();
        for _ in 0..64 {
            rep = step(
                TransmissionMode::Hybrid,
                &fp,
                Some(&tp),
                &mut st,
                &input(-1.0, 0.0, [-2.0, -2.0], [0.0, 0.0]),
            );
        }
        let r = readout(&st, &tp);
        assert_eq!(r.gear_label, "R1");
        let m_next = (rep.next_speeds[0] + rep.next_speeds[1]) / 2.0;
        let geared = -m_next * tp.gears_rev[0] / tp.sprocket_radius / RPM_TO_RAD;
        assert!(
            geared > tp.engine.idle_rpm && (r.rpm - geared).abs() < 25.0,
            "driving in reverse, the crank readout must sit at the geared rpm of the \
             integrated belt ({geared:.0}), got {:.0}",
            r.rpm
        );
        // Back-driven while in R (rolling forward, shaft < 0): the crank never follows —
        // the stall guard bounds it at idle − STALL_GUARD_BAND_RPM, and the readout shows
        // the honest idle-ish crank, not a fake geared rpm.
        let mut st = TransmissionState {
            reverse: true,
            ..Default::default()
        };
        for _ in 0..64 {
            step(
                TransmissionMode::Hybrid,
                &fp,
                Some(&tp),
                &mut st,
                &input(-1.0, 0.0, [2.0, 2.0], [-40_000.0, -40_000.0]),
            );
        }
        let r = readout(&st, &tp);
        assert_eq!(r.gear_label, "R1");
        assert!(
            r.rpm >= tp.engine.idle_rpm - STALL_GUARD_BAND_RPM - 1.0
                && r.rpm <= tp.engine.governed_rpm,
            "a back-driven R shaft must read the idle-ish crank (≥ idle − band), got {:.0}",
            r.rpm
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
            engine_inertia_kgm2: 4.0,
            clutch_capacity_nm: 2860.0,
            shift_secs: 0.31,
            shift_addressing: ShiftAddressing::Sequential,
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
    /// one (and measurably NOT the modal one). Stage B: the scenario runs inside a shift
    /// window (`shift_ticks: 5`, declutched) so the engine path contributes NO m-axis
    /// force — what is pinned here is the SPLIT LAW, isolated from the crank coupling
    /// (engaged, the cold crank against a 4 m/s shaft would add a clutch transient that
    /// obscures the arithmetic).
    #[test]
    fn recirculation_splits_physical_output_powers() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let mut st = TransmissionState {
            gear: 5,
            shift_ticks: 5,
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

    /// Coast intent (stage B shape): zero throttle at speed applies the DECLARED
    /// compression-braking drag — `drag_fraction × peak torque` — at the CRANK, and it
    /// reaches the belt only through the engaged coupling. With the belt speed HELD
    /// constant (this harness feeds fixed speeds), the crank must be steady too, so the
    /// clutch transmits the FULL drag torque: the converged per-side force is exactly the
    /// old declared share `drag_fraction × peak × G/r_s / 2` (the steady state is
    /// coupling-law-invariant; only the transient shares drag with the crank's inertia).
    /// Convergence takes a few ticks: the coupling's per-tick contraction factor is
    /// `k²J/(I_m + k²J)` ≈ 0.22 in lab gear 3, plus the first ticks resolve the cold
    /// crank (sentinel → idle) against the geared shaft at clutch capacity.
    #[test]
    fn coast_drag_reaches_belt_through_coupling() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let mut st = TransmissionState {
            gear: 3,
            ..Default::default()
        };
        // Mid-band speed for gear 3 (no shift decision interferes).
        let g3 = tp.gears_fwd[2];
        let v = 1_300.0 * RPM_TO_RAD * tp.sprocket_radius / g3;
        let mut rep = TransmissionReport::default();
        for _ in 0..32 {
            rep = step(
                TransmissionMode::Hybrid,
                &fp,
                Some(&tp),
                &mut st,
                &input(0.0, 0.0, [v, v], [0.0, 0.0]),
            );
        }
        assert_eq!(st.gear, 3, "mid-band coast must not shift");
        let expect = -(tp.peak_torque_nm * tp.drag_fraction * g3 / tp.sprocket_radius) / 2.0;
        for side in rep.forces {
            assert!(
                (side - expect).abs() < 100.0,
                "converged coasting side force {side} N must be the declared drag share \
                 {expect} N through the coupling"
            );
        }
        // And the crank sits AT the geared speed (locked coast — the readout truth).
        let geared_rpm = v * g3 / tp.sprocket_radius / RPM_TO_RAD;
        assert!(
            (rep.rpm - geared_rpm).abs() < 25.0,
            "locked coast must carry the crank at the geared rpm ({geared_rpm:.0}), got {:.0}",
            rep.rpm
        );
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

    /// Stage B: a standing start under full W is CLUTCH-SLIP-LIMITED. From rest the lock
    /// torque `τ_c*` (lab arithmetic: `[ω_idle/dt + τ_free/J]/(1/J + k₁²/I_m)` =
    /// `[62.8·64 + 1650/4]/[0.25 + 84.8²·(1/16000)]` ≈ 6.3 kN·m) far exceeds the 2860 N·m
    /// clutch capacity, so the belt force is `k₁ × capacity` ≈ 242.5 kN — NOT the old
    /// rev-floor peak-torque value `peak × G₁/r_s` ≈ 186.6 kN. The crank must never dip
    /// below the stall-guard floor while the clutch slips (the saturated idle governor
    /// holds a sub-idle slip equilibrium ≈ 37 rpm of droop where `τ_ind + τ_idle` meets
    /// the capacity).
    #[test]
    fn launch_is_clutch_slip_limited() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let k1 = tp.gears_fwd[0] / tp.sprocket_radius;
        let expect = k1 * tp.clutch_capacity;
        let old_rev_floor = tp.peak_torque_nm * tp.gears_fwd[0] / tp.sprocket_radius;
        let floor = (tp.engine.idle_rpm - STALL_GUARD_BAND_RPM) * RPM_TO_RAD;
        let mut st = TransmissionState::default();
        for tick in 0..16 {
            let rep = step(
                TransmissionMode::Hybrid,
                &fp,
                Some(&tp),
                &mut st,
                &input(1.0, 0.0, [0.0, 0.0], [0.0, 0.0]),
            );
            let total = rep.forces[0] + rep.forces[1];
            assert!(
                (total - expect).abs() < 0.01 * expect,
                "tick {tick}: launch belt force {total:.0} N must be clutch-capacity \
                 limited ({expect:.0} N)"
            );
            assert!(
                (total - old_rev_floor).abs() > 0.1 * old_rev_floor,
                "the capacity-limited launch must be measurably NOT the old rev-floor \
                 value ({old_rev_floor:.0} N) — otherwise this test pins nothing"
            );
            assert!(
                st.omega_e >= floor - 1e-3,
                "tick {tick}: the slipping-clutch launch must never stall the crank \
                 below idle − band ({:.0} rpm)",
                st.omega_e / RPM_TO_RAD
            );
        }
    }

    /// Stage B: the stall guard under a grade lug — the crank NEVER lands below
    /// idle − [`STALL_GUARD_BAND_RPM`], in both slip regimes: (a) full-W lug against an
    /// impossible reaction (capacity-clamped slip: the sub-idle equilibrium sits where the
    /// saturated idle governor + low-end torque meet the 2860 N·m capacity, ≈ 37 rpm of
    /// droop — above the 100 rpm guard band); (b) a zero-throttle engaged backslide
    /// (τ_c* wants the crank at the NEGATIVE shaft speed — the guard slips the clutch
    /// instead and the belt receives the crank's forward τ_free through it).
    #[test]
    fn stall_guard_holds_crank_under_grade_lug() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let floor = (tp.engine.idle_rpm - STALL_GUARD_BAND_RPM) * RPM_TO_RAD;
        for (throttle, speeds, reactions, label) in [
            (
                1.0f32,
                [0.0f32, 0.0],
                [200_000.0f32, 200_000.0],
                "full-W lug",
            ),
            (0.0, [-2.0, -2.0], [-40_000.0, -40_000.0], "coast backslide"),
        ] {
            let mut st = TransmissionState::default();
            for tick in 0..128 {
                let rep = step(
                    TransmissionMode::Hybrid,
                    &fp,
                    Some(&tp),
                    &mut st,
                    &input(throttle, 0.0, speeds, reactions),
                );
                assert!(
                    st.omega_e >= floor - 1e-3,
                    "{label} tick {tick}: ω_e {:.0} rpm fell below the stall-guard floor \
                     ({:.0} rpm)",
                    st.omega_e / RPM_TO_RAD,
                    floor / RPM_TO_RAD
                );
                assert!(
                    rep.forces[0] > 0.0 && rep.forces[1] > 0.0,
                    "{label} tick {tick}: the slipping clutch must keep delivering \
                     FORWARD drive (forces {:?})",
                    rep.forces
                );
            }
        }
    }

    /// Stage B: rev-match across an upshift — the crank is CONTINUOUS through the window
    /// (no teleport: per-tick slew bounded by `(capacity + τ_free)/J·dt` ≈ 189 rpm/tick in
    /// the lab), lands within a bounded gap of the new geared speed at window end (drag-only
    /// shedding covers ≈ 410 of the ≈ 660 rpm step in the 0.31 s window; the clutch
    /// shoulders the ≈ 250 rpm residual at capacity for a few ticks — the bounded physical
    /// cost of the shift), and re-locks to the geared point within a handful of engaged
    /// ticks.
    #[test]
    fn rev_match_across_upshift_is_continuous() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let g1 = tp.gears_fwd[0];
        let g2 = tp.gears_fwd[1];
        let v_warm = 1_600.0 * RPM_TO_RAD * tp.sprocket_radius / g1;
        let v_up = 1_780.0 * RPM_TO_RAD * tp.sprocket_radius / g1;
        let mut st = TransmissionState::default();
        // Warm to the locked geared point below the up band.
        for _ in 0..32 {
            step(
                TransmissionMode::Hybrid,
                &fp,
                Some(&tp),
                &mut st,
                &input(1.0, 0.0, [v_warm, v_warm], [0.0, 0.0]),
            );
        }
        let target_rpm = v_up * g2 / tp.sprocket_radius / RPM_TO_RAD; // ≈ 1121
        let mut prev_rpm = st.omega_e / RPM_TO_RAD;
        let mut window_end_gap = None;
        let mut ticks_since_window = 0u32;
        let mut rep = TransmissionReport::default();
        for tick in 0..96 {
            rep = step(
                TransmissionMode::Hybrid,
                &fp,
                Some(&tp),
                &mut st,
                &input(1.0, 0.0, [v_up, v_up], [0.0, 0.0]),
            );
            let rpm = st.omega_e / RPM_TO_RAD;
            assert!(
                (rpm - prev_rpm).abs() <= 250.0,
                "tick {tick}: crank teleported {prev_rpm:.0} -> {rpm:.0} rpm"
            );
            prev_rpm = rpm;
            if st.gear == 2 && !rep.shifting && window_end_gap.is_none() {
                window_end_gap = Some((rpm - target_rpm).abs());
            }
            if window_end_gap.is_some() {
                ticks_since_window += 1;
            }
        }
        assert_eq!(st.gear, 2, "the upshift must have committed");
        let gap = window_end_gap.expect("the window must end inside the run");
        assert!(
            gap <= 400.0,
            "rpm at window end must be within 400 rpm of the geared landing \
             ({target_rpm:.0}), gap {gap:.0}"
        );
        assert!(
            ticks_since_window > 16,
            "post-window settling must be observed"
        );
        // Re-lock anchor: the geared rpm of the belt the transmission itself integrated
        // (this harness holds the INPUT speeds, so the lock's fixed point rides
        // `k·τ_free·dt/I_m` above the held value — the crank follows THAT belt exactly).
        let m_next = (rep.next_speeds[0] + rep.next_speeds[1]) / 2.0;
        let lock_rpm = m_next * g2 / tp.sprocket_radius / RPM_TO_RAD;
        let final_rpm = st.omega_e / RPM_TO_RAD;
        assert!(
            (final_rpm - lock_rpm).abs() < 50.0,
            "the engaged clutch must re-lock the crank to the geared point of the \
             integrated belt ({lock_rpm:.0}), got {final_rpm:.0}"
        );
    }

    /// Stage B: unloaded free-rev — declutched full steer at standstill (the pivot's crank
    /// demand) revs the crank from idle toward the steer-demand target (the PEAK-TORQUE
    /// rpm — the old floor's operating point, reached dynamically; deliberately NOT the
    /// governed cut-out, where `torque_at·ω = 0` would zero the pivot's power budget).
    /// Lab arithmetic: Δω = 500 rpm = 52.4 rad/s at ≈ τ/J ≈ 2000/4 = 500 rad/s² plus the
    /// proportional-band taper → ≈ 0.15–0.3 s to 95%; the steady point parks ≈ 30 rpm
    /// under the target where the taper's fueling meets the re-engaging drag. Pinned with
    /// margin.
    #[test]
    fn free_rev_reaches_steer_target_promptly() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let mut st = TransmissionState::default();
        let mut reached = None;
        for tick in 0..128 {
            step(
                TransmissionMode::Hybrid,
                &fp,
                Some(&tp),
                &mut st,
                &input(0.0, 1.0, [0.0, 0.0], [0.0, 0.0]),
            );
            let rpm = st.omega_e / RPM_TO_RAD;
            if reached.is_none() && rpm >= 0.95 * tp.peak_torque_rpm {
                reached = Some(tick + 1);
            }
        }
        let ticks = reached.expect("the crank must reach 95% of the steer target in 2 s");
        let secs = ticks as f32 / TICK_HZ;
        println!("lab free-rev idle -> 95% of peak-torque rpm: {secs:.3} s");
        assert!(
            (0.05..=0.6).contains(&secs),
            "free-rev time {secs:.3} s outside the pinned band"
        );
        let steady = st.omega_e / RPM_TO_RAD;
        assert!(
            (tp.peak_torque_rpm - 150.0..=tp.peak_torque_rpm + 50.0).contains(&steady),
            "the declutched full-steer crank must park at the peak-torque operating point \
             (~{:.0} rpm), got {steady:.0} — a cut-out park would zero pivot power",
            tp.peak_torque_rpm
        );
    }

    /// Review round FIX 1: service braking must never TELEPORT the crank through an
    /// infeasible snap. The eager `exact` flag was decided at the pre-brake coupling
    /// solve; the brake stop-force then dropped the belt ≈ 0.23 m/s per tick
    /// (120 kN / 8 t / 64 Hz) and the drift kill snapped the crank down with it —
    /// ≈ 20 rad/s per tick, an implied clutch torque
    /// `τ_impl = τ_free − Δω·J/dt ≈ −550 − 20·4·64 ≈ −5.7 kN·m` through a 2.86 kN·m
    /// clutch. Post-fix the snap is feasibility-checked on the FINAL belt state: the crank
    /// integrates honestly with the torque the clutch actually carried, so its per-tick
    /// change obeys `|Δω|·J/dt ≤ capacity + |τ_free|` (braking: |τ_free| = drag ≤
    /// drag_fraction × peak = 550 N·m → |Δω| ≤ (2860 + 550)·dt/J ≈ 13.3 rad/s).
    #[test]
    fn braking_never_teleports_crank_past_clutch_capacity() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let dt = 1.0 / 64.0;
        let mut st = TransmissionState::default();
        // Warm to a locked coast in gear 1 at m = 1.0 (held speeds).
        for _ in 0..32 {
            step(
                TransmissionMode::Hybrid,
                &fp,
                Some(&tp),
                &mut st,
                &input(0.0, 0.0, [1.0, 1.0], [0.0, 0.0]),
            );
        }
        assert!(
            st.omega_e / RPM_TO_RAD > tp.engine.idle_rpm,
            "the warm-up must have locked the crank above idle"
        );
        // Full opposing throttle, CLOSED loop: the brake-driven belt drop must not drag
        // the crank faster than the clutch can physically pull it.
        let drag_max = tp.peak_torque_nm * tp.drag_fraction;
        let slew_bound = (tp.clutch_capacity + drag_max) * dt / tp.engine_inertia + 0.1;
        let mut speeds = [1.0f32, 1.0];
        for tick in 0..64 {
            let m_pre = (speeds[0] + speeds[1]) / 2.0;
            if m_pre < 0.5 {
                break; // swap/declutch territory — the teleport window is over.
            }
            let omega_pre = st.omega_e;
            let rep = step(
                TransmissionMode::Hybrid,
                &fp,
                Some(&tp),
                &mut st,
                &input(-1.0, 0.0, speeds, [0.0, 0.0]),
            );
            let delta = st.omega_e - omega_pre;
            assert!(
                delta >= -slew_bound,
                "tick {tick}: braking dragged the crank {delta:.1} rad/s in one tick — \
                 an implied clutch torque past capacity (bound {slew_bound:.1} rad/s); \
                 the infeasible snap is back"
            );
            speeds = rep.next_speeds;
        }
        let floor = (tp.engine.idle_rpm - STALL_GUARD_BAND_RPM) * RPM_TO_RAD;
        assert!(
            st.omega_e >= floor,
            "the crank must end above the hard floor"
        );
    }

    /// Review round FIX 3: the coupling seam is a LATCH with hysteresis — a belt speed
    /// oscillating INSIDE the 0.4–0.6 m/s dead band (forced ±0.05 around the old single
    /// 0.5 threshold, which flipped the regime every crossing) produces ZERO regime
    /// flips; only genuine excursions past the separated thresholds transition it, once
    /// each. Scripted open-loop: park below 0.4 (one transition out), 64 boundary
    /// oscillations (none), one push past 0.6 (one transition in), 64 more oscillations
    /// (none).
    #[test]
    fn clutch_seam_hysteresis_kills_boundary_chatter() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let mut st = TransmissionState::default();
        let at = |st: &mut TransmissionState, v: f32| {
            step(
                TransmissionMode::Hybrid,
                &fp,
                Some(&tp),
                st,
                &input(0.0, 0.0, [v, v], [0.0, 0.0]),
            );
            st.clutch_out
        };
        // Park below CLUTCH_OUT_M_SPEED: the clutch goes out.
        assert!(at(&mut st, 0.3), "below 0.4 m/s the clutch must go out");
        // Boundary oscillation across the OLD single threshold: no flips.
        for tick in 0..64 {
            let v = if tick % 2 == 0 { 0.55 } else { 0.45 };
            assert!(
                at(&mut st, v),
                "tick {tick}: an in-band oscillation (0.45/0.55) must not re-engage — \
                 the single-threshold chatter is back"
            );
        }
        // A genuine excursion past CLUTCH_IN_M_SPEED re-engages…
        assert!(!at(&mut st, 0.65), "past 0.6 m/s the clutch must re-engage");
        // …and the same boundary oscillation now holds ENGAGED: no flips either way.
        for tick in 0..64 {
            let v = if tick % 2 == 0 { 0.55 } else { 0.45 };
            assert!(
                !at(&mut st, v),
                "tick {tick}: an in-band oscillation must not declutch after engagement"
            );
        }
    }
}
