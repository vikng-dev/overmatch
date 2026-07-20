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
//! drive command releases it), the hill-hold latch, the legacy hold-blend entry envelope
//! while unlatched, or the service pedal (opposite-throttle driver intent). A latched belt
//! strictly inside [`PARK_ENGAGE_SPEED`] gets the authored static breakaway multiplier; a
//! moving belt, service braking, and every post-breach latched slide stay at dynamic
//! `B_max`. The `Governor` adapter keeps the old hold blend verbatim instead.
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
//! re-engagement cooldown. REV 14 discharges the multiplayer rider: the ECS wrapper replicates and
//! rolls back the complete state atomically, and the determinism trace hashes every field exactly.
//! Replay cannot derive an EMA history, an in-flight sequential target, or a brake latch from the
//! instantaneous belt.
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
//! | [`BELT_RUNAWAY_LIMIT_MULTIPLIER`] | SIM POLICY | pure numerical runaway protection on each regenerative output, DERIVED per vehicle as `1.5 × max_speed`; legal steering differential may exceed `max_speed`, and this ceiling must never bind in legal operation |
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

use bevy::ecs::error::BevyError;

use super::forces::{self, ForceParams};

#[cfg(feature = "bitprobe")]
use crate::bitprobe::TransmissionProbe;

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
/// replicated sim memory and the readout/HUD contract.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
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
pub(crate) const PARK_ENGAGE_SPEED: f32 = 0.05;

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

/// Pure numerical runaway protection for each regenerative belt output. The ceiling is
/// DERIVED per vehicle as `1.5 × max_speed`; unlike the mean-axis top-speed limit, it has no
/// physical role and must never bind in legal operation (including an authored outer-belt
/// steering differential). SIM POLICY — see the classification table.
const BELT_RUNAWAY_LIMIT_MULTIPLIER: f32 = 1.5;

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
    /// The authored per-forward-gear `(R_tight, R_wide)` table (m), retained verbatim for
    /// presentation. The sim law consumes [`Self::steer_kappa`]; retaining the source table keeps
    /// the HUD from re-deriving and rounding away the authored radii.
    pub steer_radii_m: Vec<(f32, f32)>,
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
    /// Static breakaway capacity multiplier for a latched, at-rest belt. Dynamic dissipation,
    /// service braking, and every moving slide use [`Self::brake_capacity_n`] unchanged.
    pub brake_static_factor: f32,
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
    pub brake_static_factor: f32,
    /// See [`TransmissionParams::drag_fraction`].
    pub drag_fraction: f32,
    /// See [`TransmissionParams::engine_inertia`] (kg·m²).
    pub engine_inertia_kgm2: f32,
    /// See [`TransmissionParams::clutch_capacity`] (N·m).
    pub clutch_capacity_nm: f32,
    /// Per-side belt + reflected-drivetrain inertia (kg); the coupling uses twice this value.
    pub belt_inertia: f32,
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
    /// Validate and derive runtime parameters from the one authored transmission shape shared by
    /// tank specs, the sandbox lab vehicle, and arithmetic tests.
    pub fn from_authoring(a: &TransmissionAuthoring) -> Result<Self, BevyError> {
        if a.forward_speeds_kmh.is_empty() || a.reverse_speeds_kmh.is_empty() {
            return Err("transmission.gearbox ladders must be non-empty".into());
        }
        if a.steer_radii_m.len() != a.forward_speeds_kmh.len() {
            return Err(format!(
                "transmission.steering.radii must have one (tight, wide) pair per forward \
                 gear ({} pairs for {} gears)",
                a.steer_radii_m.len(),
                a.forward_speeds_kmh.len()
            )
            .into());
        }
        for (field, ok) in [
            (
                "gearbox speeds",
                a.forward_speeds_kmh
                    .iter()
                    .chain(a.reverse_speeds_kmh)
                    .all(|v| v.is_finite() && *v > 0.0),
            ),
            (
                "steering.radii",
                a.steer_radii_m.iter().all(|(tight, wide)| {
                    tight.is_finite() && wide.is_finite() && *tight > 0.0 && *wide > 0.0
                }),
            ),
            (
                // Sanity bounds beyond finiteness: an absurd finite torque can overflow the
                // reflected-drag multiplication, and the resulting infinity meets a released
                // throttle as `infinity * 0.0` inside the shift-landing predictor.
                "engine.torque_curve",
                a.torque_nm.len() >= 2
                    && a.torque_nm.windows(2).all(|w| w[0].0 < w[1].0)
                    && a.torque_nm.iter().all(|(rpm, torque)| {
                        rpm.is_finite()
                            && torque.is_finite()
                            && *rpm > 0.0
                            && *rpm <= 20_000.0
                            && *torque >= 0.0
                            && *torque <= 100_000.0
                    }),
            ),
            (
                "engine rpms",
                [a.idle_rpm, a.governed_rpm, a.rated_rpm]
                    .iter()
                    .all(|v| v.is_finite() && *v > 0.0),
            ),
            (
                // The sim hard-clamps at `idle - 100 rpm` (DERIVED from the stall-guard band);
                // keep that floor positive with margin.
                "engine.idle_rpm floor",
                a.idle_rpm >= 300.0,
            ),
            (
                "gearbox shift bands",
                a.shift_up_rpm.is_finite()
                    && a.shift_down_rpm.is_finite()
                    && a.shift_down_rpm > 0.0
                    && a.shift_down_rpm < a.shift_up_rpm,
            ),
            (
                // Ladders ascend and fit the runtime's u8 gear index.
                "gearbox ladder shape (ascending, u8-indexable)",
                a.forward_speeds_kmh.len() <= u8::MAX as usize
                    && a.reverse_speeds_kmh.len() <= u8::MAX as usize
                    && a.forward_speeds_kmh.windows(2).all(|w| w[0] < w[1])
                    && a.reverse_speeds_kmh.windows(2).all(|w| w[0] < w[1]),
            ),
            (
                // Every post-upshift landing must remain above the down band, or the box hunts
                // up-down on a boundary speed.
                "gearbox shift-band hysteresis vs ratio steps",
                a.forward_speeds_kmh
                    .windows(2)
                    .chain(a.reverse_speeds_kmh.windows(2))
                    .all(|w| a.shift_up_rpm * w[0] / w[1] > a.shift_down_rpm),
            ),
            (
                "steering capacity/efficiency + brake_force",
                a.steer_capacity_n.is_finite()
                    && a.steer_capacity_n > 0.0
                    && (0.0..=1.0).contains(&a.recirculation)
                    && a.brake_capacity_n.is_finite()
                    && a.brake_capacity_n > 0.0,
            ),
            (
                "brake_static_factor",
                a.brake_static_factor.is_finite()
                    && (1.0..=2.5).contains(&a.brake_static_factor)
                    && (a.brake_capacity_n * a.brake_static_factor).is_finite(),
            ),
            (
                "engine.drag_fraction",
                (0.0..=1.0).contains(&a.drag_fraction),
            ),
            (
                "engine.inertia_kgm2",
                a.engine_inertia_kgm2.is_finite() && (0.1..=100.0).contains(&a.engine_inertia_kgm2),
            ),
            (
                "engine.clutch_capacity_nm",
                a.clutch_capacity_nm.is_finite()
                    && (100.0..=50_000.0).contains(&a.clutch_capacity_nm),
            ),
            (
                // The coupling lock denominator includes `k^2 / (2 * belt_inertia)`; tiny
                // positive values overflow that term even though the generic belt law accepts
                // every finite positive inertia.
                "powertrain.inertia floor (coupling divisor)",
                a.belt_inertia.is_finite() && a.belt_inertia >= 1.0,
            ),
            (
                // The u8 countdown represents at most 255 ticks (DERIVED 255 / 64 = 3.984375 s).
                "gearbox.shift_secs",
                (0.0..=3.0).contains(&a.shift_secs),
            ),
        ] {
            if !ok {
                return Err(format!("track.powertrain.transmission: invalid {field}").into());
            }
        }

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
        Ok(Self {
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
            steer_radii_m: a.steer_radii_m.to_vec(),
            steer_capacity_n: a.steer_capacity_n,
            neutral_d_full,
            recirculation: a.recirculation,
            brake_capacity_n: a.brake_capacity_n,
            brake_static_factor: a.brake_static_factor,
            drag_fraction: a.drag_fraction,
            engine_inertia: a.engine_inertia_kgm2,
            clutch_capacity: a.clutch_capacity_nm,
            shift_ticks: (a.shift_secs * TICK_HZ).round().clamp(0.0, 255.0) as u8,
            shift_addressing: a.shift_addressing,
            peak_torque_rpm,
            peak_torque_nm,
        })
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
/// scheduler status, and hill hold. Constructed at spawn from tank data and replicated atomically
/// through [`crate::track::sim::TankTransmission`] under REV 14.
#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
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
    /// fix-1b reversal dwell blocks against. Reset by a ladder swap.
    pub last_shift_dir: i8,
    /// Remaining ticks of the fix-1b reversal dwell: while non-zero, the shift OPPOSITE to
    /// `last_shift_dir` stays blocked (same-direction shifts stay free).
    pub dwell_ticks: u8,
    /// Engine crank speed ω_e (rad/s) — stage B's crank state. Initialized explicitly from
    /// vehicle data by [`Self::from_spec`]; the regenerative tick path has no in-band sentinel.
    pub omega_e: f32,
    /// Main-clutch-out latch (stage-B review round, FIX 3): the coupling-seam regime with
    /// hysteresis — set below [`CLUTCH_OUT_M_SPEED`] without propulsive drive, cleared at
    /// [`CLUTCH_IN_M_SPEED`] or on any propulsive command.
    pub clutch_out: bool,
    /// Filtered positive load demand on the signed mean shaft axis (N, both tracks). Updated only
    /// on decision ticks and frozen through shift windows so the torque cut cannot pollute it.
    pub demand_n: f32,
    /// First-sample marker for `demand_n`: the contact-derived seed is unavailable at spawn, so
    /// the first owned reaction sample initializes the EMA directly instead of ramping from a
    /// fictitious zero-load history.
    pub demand_initialized: bool,
    /// Persistent decision-tick evidence that the current gear has negative reserve. Negative
    /// ticks increment it and other ticks decay it by one, so one contact-jitter sample cannot erase
    /// a nearly confirmed deficit. Saturating u8.
    pub grade_confirm_ticks: u8,
    /// Held reserve target (1-based; zero means none). Direct addressing retains it through its
    /// one interruption window; Sequential retains it across every adjacent window.
    pub grade_target: u8,
    /// Scheduler/readout state.
    pub scheduler: SchedulerState,
    /// Anti-rollback latch. While true, the existing service-brake stop-force law runs at its full
    /// declared envelope until the selected launch gear transmits the capability-derived release
    /// threshold.
    pub hill_hold: bool,
    /// Remaining post-release hill-hold cooldown ticks. While nonzero, a near-rest deficit cannot
    /// re-latch; actual backward motion past [`HILL_HOLD_ENGAGE_SPEED`] overrides the cooldown.
    pub hold_reengage_ticks: u8,
}

/// One field in the authoritative REV-14 transmission projection. Float values retain their raw
/// bits; the scheduler carries its pinned trace/hash tag plus stable `from`/`to` slots.
#[derive(Clone, Copy, Debug)]
pub(crate) enum TransmissionProjectionValue {
    U8(u8),
    I8(i8),
    Bool(bool),
    F32(f32),
    Scheduler { tag: u8, from: u8, to: u8 },
}

impl TransmissionProjectionValue {
    /// Bit-exact equality under the atomic replication contract.
    pub(crate) fn bit_eq(self, other: Self) -> bool {
        match (self, other) {
            (Self::U8(a), Self::U8(b)) => a == b,
            (Self::I8(a), Self::I8(b)) => a == b,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::F32(a), Self::F32(b)) => a.to_bits() == b.to_bits(),
            (
                Self::Scheduler {
                    tag: a_tag,
                    from: a_from,
                    to: a_to,
                },
                Self::Scheduler {
                    tag: b_tag,
                    from: b_from,
                    to: b_to,
                },
            ) => a_tag == b_tag && a_from == b_from && a_to == b_to,
            _ => false,
        }
    }
}

/// A named field in the exhaustive authoritative transmission projection.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TransmissionProjectionField {
    pub(crate) name: &'static str,
    pub(crate) value: TransmissionProjectionValue,
}

/// The exhaustive REV-14 transmission inventory in its canonical replication/hash/trace order.
/// Adding state fails this destructure until the field is classified exactly once here.
pub(crate) fn transmission_state_projection(
    state: &TransmissionState,
) -> [TransmissionProjectionField; 16] {
    let TransmissionState {
        gear,
        shift_ticks,
        steer_step,
        reverse,
        park,
        last_shift_dir,
        dwell_ticks,
        omega_e,
        clutch_out,
        demand_n,
        demand_initialized,
        grade_confirm_ticks,
        grade_target,
        scheduler,
        hill_hold,
        hold_reengage_ticks,
    } = *state;
    let scheduler = match scheduler {
        SchedulerState::Normal => TransmissionProjectionValue::Scheduler {
            tag: 0,
            from: 0,
            to: 0,
        },
        SchedulerState::GradeShift { from, to } => {
            TransmissionProjectionValue::Scheduler { tag: 1, from, to }
        }
        SchedulerState::HillHold => TransmissionProjectionValue::Scheduler {
            tag: 2,
            from: 0,
            to: 0,
        },
        SchedulerState::GradeLimit => TransmissionProjectionValue::Scheduler {
            tag: 3,
            from: 0,
            to: 0,
        },
    };
    use TransmissionProjectionValue::{Bool, F32, I8, U8};
    [
        TransmissionProjectionField {
            name: "gear",
            value: U8(gear),
        },
        TransmissionProjectionField {
            name: "shift_ticks",
            value: U8(shift_ticks),
        },
        TransmissionProjectionField {
            name: "steer_step",
            value: U8(steer_step),
        },
        TransmissionProjectionField {
            name: "reverse",
            value: Bool(reverse),
        },
        TransmissionProjectionField {
            name: "park",
            value: Bool(park),
        },
        TransmissionProjectionField {
            name: "last_shift_dir",
            value: I8(last_shift_dir),
        },
        TransmissionProjectionField {
            name: "dwell_ticks",
            value: U8(dwell_ticks),
        },
        TransmissionProjectionField {
            name: "omega_e",
            value: F32(omega_e),
        },
        TransmissionProjectionField {
            name: "clutch_out",
            value: Bool(clutch_out),
        },
        TransmissionProjectionField {
            name: "demand_n",
            value: F32(demand_n),
        },
        TransmissionProjectionField {
            name: "demand_initialized",
            value: Bool(demand_initialized),
        },
        TransmissionProjectionField {
            name: "grade_confirm_ticks",
            value: U8(grade_confirm_ticks),
        },
        TransmissionProjectionField {
            name: "grade_target",
            value: U8(grade_target),
        },
        TransmissionProjectionField {
            name: "scheduler",
            value: scheduler,
        },
        TransmissionProjectionField {
            name: "hill_hold",
            value: Bool(hill_hold),
        },
        TransmissionProjectionField {
            name: "hold_reengage_ticks",
            value: U8(hold_reengage_ticks),
        },
    ]
}

impl TransmissionState {
    /// Construct complete regenerative transmission state synchronously from validated vehicle
    /// data. The crank starts at the authored idle speed; demand remains intentionally unseeded
    /// until the first owned contact-reaction sample arrives.
    pub fn from_spec(tp: &TransmissionParams) -> Self {
        Self::with_crank_speed(tp.engine.idle_rpm * RPM_TO_RAD)
    }

    /// Canonical inert state for a vehicle with no declared regenerative transmission. The
    /// Governor adapter never reads or mutates this state, so its absent crank has no vehicle
    /// speed to initialize from and remains explicitly zero rather than acting as a sentinel.
    pub(crate) fn for_governor() -> Self {
        Self::with_crank_speed(0.0)
    }

    fn with_crank_speed(omega_e: f32) -> Self {
        Self {
            gear: 1,
            shift_ticks: 0,
            steer_step: 0,
            reverse: false,
            park: false,
            last_shift_dir: 0,
            dwell_ticks: 0,
            omega_e,
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
    /// Engine rpm — the crank state ω_e DIRECTLY (stage B: the state IS the display), including
    /// an honest sub-idle grade lug (the stall guard bounds it at idle −
    /// [`STALL_GUARD_BAND_RPM`]).
    pub rpm: f32,
    /// The engaged gear as a display label: `F1..Fn` forward, `R1..Rn` reverse.
    pub gear_label: String,
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
    let rpm = st.omega_e / RPM_TO_RAD;
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
    #[cfg(feature = "bitprobe")]
    pub(crate) bitprobe: TransmissionProbe,
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
/// reality run the same (now purely reaction-driven) window law. `max_speed` bounds this
/// vehicle/mean axis, matching the live regenerative integration; it is not a per-belt bound.
///
/// DOMAIN (review round): valid for the PROPULSIVE straight-line case ONLY — the only case
/// the scheduler consults it for (upshifts are intent-gated on `propulsive > 0` and
/// detent-deferred on the L600). It carries no brake term and no λ/steer state, so under
/// service braking or a geared turn it would over-predict the landing. Inside its domain,
/// frozen-R is CONSERVATIVE (the true post-cut reaction collapses with the slip), and the
/// mean-axis clamp is therefore the exact live speed-limit semantic in this domain.
fn predict_shift_landing_m(
    tp: &TransmissionParams,
    fp: &ForceParams,
    m: f32,
    r_mean: f32,
    dt: f32,
) -> f32 {
    let mut pm = m;
    for _ in 0..tp.shift_ticks {
        pm = clamp_mean_speed(pm - r_mean / fp.inertia * dt, fp.max_speed);
    }
    pm
}

/// Apply the regenerative path's speed limits without conflating its superimposed axes:
/// `max_speed` bounds the vehicle/mean axis `m`, while the legal steering difference `d`
/// passes through unchanged. Only the much wider per-belt runaway ceiling can clip `d`.
///
/// The in-range branch returns the raw integration results directly so scenarios that never
/// touched the old clamp retain their exact f32 values.
fn limit_regenerative_belt_speeds(raw: [f32; 2], max_speed: f32) -> [f32; 2] {
    let mean = (raw[0] + raw[1]) / 2.0;
    let limited_mean = clamp_mean_speed(mean, max_speed);
    let mut limited = if limited_mean != mean {
        let correction = limited_mean - mean;
        [raw[0] + correction, raw[1] + correction]
    } else {
        raw
    };

    let runaway_limit = BELT_RUNAWAY_LIMIT_MULTIPLIER * max_speed;
    for speed in &mut limited {
        if *speed > runaway_limit {
            *speed = runaway_limit;
        } else if *speed < -runaway_limit {
            *speed = -runaway_limit;
        }
    }
    limited
}

fn clamp_mean_speed(mean: f32, max_speed: f32) -> f32 {
    mean.clamp(-max_speed, max_speed)
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
    #[cfg(feature = "bitprobe")] probe: &mut TransmissionProbe,
) -> f32 {
    let tau_star = ((omega_e - k * s * m) / dt + tau_free / j_e - k * s * f_other / i_m)
        / (1.0 / j_e + k * k / i_m);
    let mut tau_c = tau_star.clamp(-capacity, capacity);
    let omega_next = omega_e + (tau_free - tau_c) * dt / j_e;
    #[cfg(feature = "bitprobe")]
    {
        probe.tau_star = tau_star;
        probe.tau_clamped = tau_c;
        probe.omega_coupled = omega_next;
    }
    if omega_next < omega_floor {
        // Land exactly at the floor: τ_guard = τ_free − J·(ω_floor − ω_e)/dt. By
        // construction τ_guard < τ_c (less torque = higher landing), so the guard only
        // ever reduces — one-sided.
        tau_c = (tau_free - (omega_floor - omega_e) * j_e / dt).clamp(-capacity, capacity);
    }
    #[cfg(feature = "bitprobe")]
    {
        probe.tau_c = tau_c;
    }
    tau_c
}

/// Per-side brake capacity for the current regime. Static breakaway is a zero-work latch property,
/// not a stronger moving brake: every missing predicate returns the authored dynamic capacity.
pub(crate) fn brake_capacity_for_regime(
    tp: &TransmissionParams,
    latch_active: bool,
    service: f32,
    belt_speed: f32,
) -> f32 {
    if latch_active && service == 0.0 && belt_speed.abs() < PARK_ENGAGE_SPEED {
        tp.brake_capacity_n * tp.brake_static_factor
    } else {
        tp.brake_capacity_n
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
    #[cfg(feature = "bitprobe")]
    let mut bitprobe = TransmissionProbe {
        throttle: inp.throttle,
        steer: inp.steer,
        side_commands: inp.side_commands,
        speeds: inp.speeds,
        reactions: inp.reactions,
        dt,
        mean_speed: m,
        difference_speed: d,
        demand_pre: st.demand_n,
        omega_pre: st.omega_e,
        ..Default::default()
    };

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
    #[cfg(feature = "bitprobe")]
    {
        bitprobe.direction = dir;
    }
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
        #[cfg(feature = "bitprobe")]
        {
            bitprobe.demand_sample = sample;
            bitprobe.demand_updated = true;
        }
        if st.demand_initialized {
            st.demand_n += (sample - st.demand_n) / DEMAND_FILTER_TICKS;
        } else {
            st.demand_n = sample;
            st.demand_initialized = true;
        }
    }
    #[cfg(feature = "bitprobe")]
    {
        bitprobe.demand_post = st.demand_n;
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
    #[cfg(feature = "bitprobe")]
    {
        bitprobe.shaft_speed = shaft;
    }
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
    let omega_e = st.omega_e;
    let k = g / tp.sprocket_radius;
    let omega_floor = (tp.engine.idle_rpm - STALL_GUARD_BAND_RPM) * RPM_TO_RAD;
    #[cfg(feature = "bitprobe")]
    {
        bitprobe.gear_reduction = g;
        bitprobe.k = k;
        bitprobe.omega_idle = omega_idle;
        bitprobe.omega_floor = omega_floor;
        bitprobe.shifting = shifting;
    }

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
    #[cfg(feature = "bitprobe")]
    {
        bitprobe.engaged = engaged;
    }

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
    #[cfg(feature = "bitprobe")]
    {
        let tau_drag = engine_drag(tp, omega_e, u_fuel);
        bitprobe.u_fuel = u_fuel;
        bitprobe.rpm = rpm;
        bitprobe.tau_idle = tau_idle;
        bitprobe.tau_induced = tau_ind;
        bitprobe.tau_drag = tau_drag;
        bitprobe.tau_free = tau_free;
        bitprobe.power_available = p_avail;
    }

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
            #[cfg(feature = "bitprobe")]
            &mut bitprobe,
        )
    } else {
        0.0
    };
    let mut f_c = k * dir * tau_c;
    #[cfg(feature = "bitprobe")]
    {
        bitprobe.i_mean = i_m;
        bitprobe.f_other = -(inp.reactions[0] + inp.reactions[1]);
        bitprobe.f_c_pre_scale = f_c;
    }

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
    #[cfg(feature = "bitprobe")]
    {
        bitprobe.f_s_pre_scale = f_s;
        bitprobe.lambda = lambda;
        bitprobe.j = j;
        bitprobe.power_left = p_l;
        bitprobe.power_right = p_r;
        bitprobe.power_positive = pos;
        bitprobe.power_negative = neg;
        bitprobe.power_net = net;
        bitprobe.power_scale = power_scale;
    }
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
    #[cfg(feature = "bitprobe")]
    {
        bitprobe.omega_integrated = st.omega_e;
    }

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
            // Static breakaway capacity applies ONLY to a latched, at-rest belt without a
            // service-brake command. The speed test is per belt and reads the PRE-TICK state:
            // as soon as a breached belt leaves the at-rest band, this same tick uses the
            // dynamic cap. The scheduler's rollback-rescue arithmetic deliberately continues to
            // read `brake_capacity_n`, so no moving rescue path quietly gains static capacity.
            let capacity =
                brake_capacity_for_regime(tp, st.park || hill_brake_active, service, inp.speeds[i]);
            let cap = envelope * capacity;
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
    let raw_next = [
        inp.speeds[0] + (q[0] - inp.reactions[0]) / fp.inertia * dt,
        inp.speeds[1] + (q[1] - inp.reactions[1]) / fp.inertia * dt,
    ];
    let next = limit_regenerative_belt_speeds(raw_next, fp.max_speed);
    #[cfg(feature = "bitprobe")]
    {
        bitprobe.forces = q;
        bitprobe.raw_next = raw_next;
        bitprobe.next_speeds = next;
    }

    // --- Drift kill / re-anchor (stage B + review round FIX 1): snap the crank to the
    // belt that ACTUALLY integrated only if the snap is FEASIBLE — the implied TOTAL
    // clutch torque `τ_impl = τ_free − (k·s·m_next − ω_e)·J/dt` must fit the capacity.
    // The coupling pre-solve's F_other (reactions only) is a PREDICTOR approximation:
    // brakes, the FixedRadii λ mean-axis share, the mean-axis speed limit, and (only on
    // numerical runaway) the per-belt safety ceiling all move m_next after it, and exact
    // pre-accounting is circular (the brake law reads the q that
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
        #[cfg(feature = "bitprobe")]
        {
            let feasible = tau_impl.abs() <= tp.clutch_capacity && locked >= omega_floor;
            bitprobe.reanchor_attempted = true;
            bitprobe.reanchor_locked = locked;
            bitprobe.reanchor_tau_impl = tau_impl;
            bitprobe.reanchor_feasible = feasible;
        }
        if tau_impl.abs() <= tp.clutch_capacity && locked >= omega_floor {
            st.omega_e = locked;
        }
    }

    // --- Hard stall floor (review round FIX 2): the crank never ENDS a tick below
    // ω_floor, whatever the spec's torque/drag corner did to τ_free — the floor IS the
    // no-stall policy while stall death stays deliberately unmodeled (classification
    // table). It also self-heals a NaN (f32::max drops the NaN operand).
    st.omega_e = st.omega_e.max(omega_floor);
    #[cfg(feature = "bitprobe")]
    {
        bitprobe.omega_end = st.omega_e;
    }

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
        #[cfg(feature = "bitprobe")]
        bitprobe,
    }
}

#[cfg(test)]
#[path = "transmission/tests.rs"]
mod tests;
