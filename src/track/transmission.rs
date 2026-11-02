//! The declared transmission — the JOINT two-output drivetrain between contact-force
//! calculation and belt integration. One call computes BOTH sprocket forces `Q_L, Q_R` from
//! the pair of belt speeds, the drive command, and this tick's ground reactions `R_L, R_R`,
//! then integrates `I·v̇ᵢ = Qᵢ − Rᵢ` for both sides simultaneously; each `Rᵢ` carries the
//! ground's rolling resistance beside the grip's reaction (`forces::rolling_resistance`), so every
//! reader of a reaction here sees it as load, and the shift-landing predictor, which relaxes the
//! grip's share through a torque cut, keeps that share whole. Internally it works in the
//! superimposed coordinates `m = (v_L+v_R)/2` (propulsion) and `d = (v_L−v_R)/2` (steering
//! difference); the per-side belt speeds stay the authoritative external state (design §2 —
//! reparameterizing saves nothing and every contact call is per-side).
//!
//! **The design narrative lives in `.agents/docs/design/track-model/transmission-design.md`
//! and is not retold here**: the architecture menu, the staged findings and their
//! dispositions, and every measured gate. That document's `stage A` / `stage B` / `stage C`
//! headings are the cross-reference — the rules below name the stage that settled each one.
//! What follows is only what a reader needs in order to USE or MODIFY this module: the entry
//! points, the invariants and their units, and the classification of every constant the
//! module still owns.
//!
//! One law, the fixed-radius geared regenerative box (design menu B, the L600 family): engine
//! torque curve × gear ratio → propulsion force on `m`; `d` CONSTRAINED to `s·κ(gear, step)·|m|`
//! (two steering detents per gear with hysteresis); the constraint force λ solved
//! semi-implicitly and clamped so each output's steering share stays inside the per-output
//! capacity (beyond it the constraint slips); power conservation with inner-track recirculation
//! at declared η. At `m≈0` the neutral turn is the MARGINAL brake-gated one the restoration
//! literature describes: a slow capacity-limited counter-rotation at the DERIVED 1st-gear pivot
//! scale [`TransmissionParams::neutral_d_full`]. A vehicle whose drivetrain is another family
//! adds that family to its spec data and its law here; the design doc's menu names the
//! candidates.
//!
//! Brakes (design §3, the hold reframe): the ground reaction is ALWAYS applied to the belt
//! (never attenuated). The brake force is the capacity-limited STOP force
//! `Bᵢ = clamp(Rᵢ − Qᵢ − vᵢ·I/dt, ±cap)` — at rest exactly the static balance `Rᵢ − Qᵢ`
//! (a parked tank on a slope inside capacity holds EXACTLY), in motion strictly opposing
//! where the belt is headed (settles creep, saturates against a slide, never pushes through
//! zero). `cap` comes from the parking LATCH (zero command near standstill sets it; any
//! throttle OR steer intent releases it — steer by design, the neutral pivot must move the
//! belts, and it re-latches when the stick returns to zero at standstill), the hill-hold
//! latch, the hold-blend entry envelope
//! while unlatched, or the service pedal (opposite-throttle driver intent). A latched belt
//! strictly inside [`PARK_ENGAGE_SPEED`] gets the authored static breakaway multiplier; a
//! moving belt, service braking, and every post-breach latched slide stay at dynamic
//! `B_max`.
//!
//! Signed shaft (stage A): the box measures the geared shaft RELATIVE TO
//! THE ENGAGED LADDER — `shaft = dir·m` with `dir = −1` on the R ladder — so driving
//! normally reads POSITIVE and a BACK-DRIVEN belt (rolling against the engaged gear on a
//! grade) reads NEGATIVE. The SHAFT is signed (rigid gearing); the ENGINE never is — it
//! cannot follow a back-driven shaft. Reading the shaft through `|m|` is the REFUTED form: it
//! makes a backslide indistinguishable from high forward rpm, so every gate downstream (the
//! governor cut, the band arm, the fix-1a landing gate) reads a backslide as a reason to
//! shift UP.
//!
//! Engine crank state ω_e (stage B): the crank is REAL STATE
//! ([`TransmissionState::omega_e`], rad/s) with its own inertia J
//! ([`TransmissionParams::engine_inertia`]). Per tick it produces a free torque
//! `τ_free = τ_ind + τ_idle − τ_drag` (induced torque at the crank's OWN rpm under the
//! governor cut; a saturating idle-governor recovery below idle; compression-braking drag,
//! ENGINE-side — the belt has NO separate drag term and drag reaches it only through the
//! coupling), and a capacity-clamped main clutch couples it to the geared shaft
//! ([`clutch_coupling`] — the semi-implicit lock torque, the ONE seamed coupling-law slot).
//! Engaged, the belt's engine force is `F_c = k·s·τ_c`; a STALL GUARD slips the clutch
//! one-sidedly at idle − [`STALL_GUARD_BAND_RPM`] only while combustion can sustain idle.
//! The game's damage model permits a reduced engine to run down to rest. Declutched (shift window / the generalized
//! neutral-idle seam), the belt gets NO engine force and NO engine drag, and the crank
//! REV-MATCHES toward the larger of the landing shaft speed and the steer-demand rpm target
//! (the steering member is engine-driven in every regime). Launch rpm is the emergent
//! clutch-slip equilibrium; pivot power spools with the crank; `readout` reports ω_e
//! directly (the state IS the display).
//!
//! Reserve scheduler (stage C): on every decision tick the box projects the
//! two owned ground reactions onto the signed `m` axis and low-pass the positive load demand
//! `D` with a fixed-tick ASYMMETRIC EMA — rise [`DEMAND_FILTER_TICKS`], fall
//! [`DEMAND_FALL_FILTER_TICKS`]: rising demand is believed fast for safety, a collapsing
//! sample slowly (pessimistic about LOSING load). The filter freezes through shift windows.
//! For every gear `j`, full-throttle capability at current speed is
//! `F_j = min(torque_at(rpm_j), clutch_capacity)·G_j/r_s` and reserve is `R_j = F_j − D`.
//! Upshifts retain every stage-A/B gate, require `reserve_margin` of headroom, and hold the FULL
//! ordinary-upshift predicate — band AND landing AND reserve, every condition the commit
//! itself needs — for [`UPSHIFT_CONFIRM_TICKS`] consecutive decision ticks, HARD-reset on any
//! tick the predicate fails. A negative current reserve held for [`GRADE_CONFIRM_TICKS`]
//! commands the highest lower gear that clears the same margin, bounded by the signed-landing
//! and over-rev gates. This CONFIRMED deficit is a correction, not a preference: with a target
//! it owns the decision — committing, or HOLDING the gear (a pending correction suppresses the
//! ordinary upshift arm; only a None selection falls through). A SHALLOW deficit (inside the
//! reserve-margin scale) waits out the post-upshift reversal dwell — the post-window
//! re-acceleration shear rides the demand EMA and manufactured deficits at capability
//! boundaries (the measured F5↔F6 / R1→R2→R3→R1 limit cycles); evidence keeps accumulating
//! through the dwell, so it corrects at expiry. A DEEP deficit (beyond the margin scale — a
//! genuine steep grade) overrides the deferral and corrects immediately, before window + dwell
//! can bleed the landing sign negative. The scheduler names one target; vehicle data
//! [`ShiftAddressing`] decides whether one window commits straight to it or a sequential box
//! pays one window per adjacent step. Every sequential continuation re-runs selection; released
//! intent or recovered demand cancels a stale target. Non-deficit ticks decay confirmation by
//! one rather than erasing its history.
//!
//! Overrun protection (stage C): on overrun — back-driven while coasting or service-braking,
//! the hill doing the driving — the box HOLDS its gear. Engine braking is the point of being
//! in gear on a descent, so a governed-plus-margin protective upshift there is the REJECTED
//! form: it walks the ladder UP and sheds the reflected retardation, backwards vs real
//! practice ("descend in the gear that climbs"). The protective upshift survives only as a
//! LAST RESORT at the mechanical-protection ceiling: when the signed geared shaft AND the
//! crank ω_e both exceed the engine's max authored curve rpm
//! ([`TransmissionParams::max_curve_rpm`] — the Tiger's rated 3000, the same ceiling the
//! fix-1c over-rev gate measures downshifts against), the box may upshift regardless of intent
//! to save the crank; between governed and that ceiling the climbing dial IS the warning.
//!
//! THE SAFETY MECHANISM for the crank is the OVER-REV SLIP GUARD, not gear selection: the
//! coupling guard, the re-anchor condition, and the end-of-tick clamp bound the crank at
//! `max_curve_rpm + OVERREV_MARGIN_RPM` unconditionally — order-proof, direction-symmetric,
//! independent of every scheduling decision (mechanics in [`clutch_coupling`]). Gear selection
//! during overrun is therefore BEST-EFFORT, not load-bearing: the rescue picks the first
//! higher gear whose STATICALLY-projected shaft rpm (current shaft through the candidate ratio
//! — no window pricing) sits at or below the ceiling; Direct skips straight there, Sequential
//! steps one gear per paid window toward it (gear index strictly rises, the ladder is finite —
//! termination is trivial). A mispriced or overtaken landing costs one guard-clamped window
//! and a re-decision, not a runaway; if no gear statically clears, the box holds and the guard
//! alone owns the crank (a blind commit would only shed reflected braking). Worst-case
//! landing-pricing machinery — decayed predictors, sustained-reaction bounds, λ allowances —
//! is deliberately ABSENT: it sprouted a defect per attempt while the guard beneath already
//! made its success optional, and its cost is at most an occasional extra clamped window on
//! extreme descents (the steep probes ride the guard identically with and without it).
//! PRIORITY RULE: while the crank rescue is active (shaft AND crank past the ceiling) it owns
//! the decision tick — no capability downshift may pre-empt it (structurally the over-rev gate
//! already refuses every lower gear there; the explicit gate makes the priority a stated
//! invariant rather than an emergent one). CRANK CORROBORATION binds both arms: a belt
//! transient the engaged coupling never carried endangers nothing and must not fire the rescue
//! (BOTH speeds must read past the ceiling), and the ORDINARY band arm needs the same
//! corroboration whenever the shaft reads past governed + [`CRANK_CORROBORATION_MARGIN_RPM`],
//! so no transient drives gear selection through either arm. Under PROPULSIVE intent ordinary
//! band shifting is untouched — a driver pushing downhill still gets upshifts and high rpm.
//! The reversal dwell remains on the protective shift; the ordinary landing-rpm band, the
//! reserve gate, AND the landing-sign test are all waived for it — its purpose is to lower an
//! externally back-driven crank, not to accelerate. Above the fuel cut every gear's modeled
//! force is zero, so the reserve gate could never re-legalize the rescue while rpm only rose,
//! and the static projection of a shaft the trigger already proved past the ceiling is
//! positive by construction.
//!
//! Anti-rollback (stage C): a held PROPULSIVE command on the ENGAGED ladder — either
//! direction, since backing up a slope is a climb and every quantity here is already
//! ladder-signed (`shaft = dir·m`, selection walks the engaged ladder, release compares
//! `dir·f_c`) — near rest with negative effective reserve latches
//! [`TransmissionState::hill_hold`]. The hold uses the existing full-envelope brake stop-force
//! path—no extra force—and selects a capable launch gear through the same reserve rule. A shift
//! cut has effective `F = 0`, so a sequential cascade can engage the hold even when its landing
//! gear is statically capable. While latched, launch selection and `GRADE LIMIT` truth are
//! re-evaluated on every decision tick. Release compares transmitted coupling force against
//! `D + min(selection_margin, max(0, R_selected) / 2)`: a margin-short but capable gear can
//! release once it transmits its own modeled force. A release starts a [`HOLD_REENGAGE_TICKS`]
//! cooldown; near-rest chatter cannot re-latch during it, and it is never overridable — the
//! latch engages ONLY near rest, and cross-motion intent-vs-motion belongs to the
//! `back_driven_intent` service braking, which decelerates the hull back into the zone. If no
//! gear has non-negative reserve, [`SchedulerState::GradeLimit`] stays exposed and the declared
//! brakes remain applied while the climb command is held.
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
//! below is what legitimately remains a module constant. The table is the INVENTORY and the
//! classification only; each constant's own doc comment carries its rationale, its units and
//! its measured bounds, and is the single source for them.
//!
//! | constant | class |
//! |---|---|
//! | [`GOVERNOR_CUT_RPM`] | SIM POLICY |
//! | [`DRAG_THROTTLE_RELEASE`] | SIM POLICY |
//! | [`DEAD`] | SIM POLICY |
//! | [`PARK_ENGAGE_SPEED`] | SIM POLICY |
//! | [`HILL_HOLD_ENGAGE_SPEED`] | SIM POLICY |
//! | [`DIRECTION_SWAP_SPEED`] | SIM POLICY |
//! | [`NEUTRAL_THROTTLE`], [`NEUTRAL_M_SPEED`] | SIM POLICY |
//! | [`POSTSHIFT_MARGIN_RPM`] | SIM POLICY |
//! | [`LANDING_REACTION_DECAY`] | SIM POLICY |
//! | [`CRANK_CORROBORATION_MARGIN_RPM`] | SIM POLICY |
//! | [`MOTORING_DRAG_BASE_SHARE`] | SIM POLICY |
//! | [`REVERSAL_DWELL_TICKS`] | SIM POLICY |
//! | [`OVERREV_MARGIN_RPM`] | SIM POLICY |
//! | [`RESERVE_MARGIN_FRACTION`] | SIM POLICY |
//! | [`RESERVE_CAPACITY_FRACTION`] | SIM POLICY |
//! | [`DEMAND_FILTER_TICKS`] | SIM POLICY |
//! | [`DEMAND_FALL_FILTER_TICKS`] | SIM POLICY |
//! | [`GRADE_CONFIRM_TICKS`] | SIM POLICY |
//! | [`UPSHIFT_CONFIRM_TICKS`] | SIM POLICY |
//! | [`HOLD_REENGAGE_TICKS`] | SIM POLICY |
//! | [`WIDE_ON`]/[`WIDE_OFF`]/[`TIGHT_ON`]/[`TIGHT_OFF`] | SIM POLICY |
//! | [`TICK_HZ`] | SIM POLICY |
//! | [`K_IDLE_DROOP_RPM`] | SIM POLICY |
//! | [`STALL_GUARD_BAND_RPM`] | SIM POLICY |
//! | [`CLUTCH_OUT_M_SPEED`]/[`CLUTCH_IN_M_SPEED`] | SIM POLICY |
//! | [`REV_MATCH_BAND_RPM`] | SIM POLICY |
//! | `gearbox.shift_addressing` / [`ShiftAddressing`] | VEHICLE DATA |
//! | [`CRANK_DRAG_FADE_RPM`] | Resolution: numerical rest band, independent of the healthy idle guard |
//!
//! Moved OUT of this module to the spec (they were vehicle data wearing const clothing):
//! shift time (`gearbox.shift_secs` — a Tiger preselector and a T-34 crash box differ),
//! shift addressing (`gearbox.shift_addressing`), engine drag (`engine.drag_fraction` — an engine
//! datum). REMOVED rather than classified:
//! `STEER_SERVO_BAND` — the steering servo is now the semi-implicit exact law (like the
//! brakes and λ), so no proportional band exists to tune; its droop was itself a
//! vehicle-scaling bug (the Tiger's neutral target sat inside the band). Also REMOVED:
//! `neutral_fraction` (spec field DELETED) — an
//! unprovenanced authored feel scalar; the DERIVED `neutral_d_full = κ_tight(F1) ×
//! v1_governed` is itself the correct emergent pivot scale for a fixed-radius box (the
//! radii table's own invariant: `κ_tight(g) × v(g)` is gear-independent). Everything else
//! the vehicle authors was already spec: torque curve, ladders, radii, capacities, brake, η.

use bevy::ecs::error::BevyError;

use super::forces::{self, ForceParams};

#[cfg(feature = "bitprobe")]
use crate::bitprobe::TransmissionProbe;

/// Gear-selection capability declared by the vehicle spec. The scheduler may name any target;
/// this datum decides whether one interruption reaches it directly or pays one window per
/// adjacent step. VEHICLE DATA, not scheduler policy: the model accepts arbitrary targets, and
/// whether a gearbox can address one is an era/mechanism capability the spec declares.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Deserialize)]
pub enum ShiftAddressing {
    /// Preselector/automatic capability: one shift event commits straight to the legal target.
    Direct,
    /// Crash-box capability: each event moves one adjacent gear and the held target is approached
    /// over repeated interruption windows.
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

/// The fixed simulation tick rate the shift countdown quantizes against (the module operates
/// on fixed 64 Hz ticks — module doc). SIM POLICY.
const TICK_HZ: f32 = 64.0;

/// Fix-1a anti-hunting margin (rpm): an upshift only commits if the belt state PREDICTED at
/// the end of the shift's own torque-cut window (same integration law, drive torque cut,
/// reaction DECAYING per [`LANDING_REACTION_DECAY`] — [`predict_shift_landing_m`]) lands at
/// least this far ABOVE the down band in the new gear. The static band gap (~100 rpm at the Tiger's widest step) is
/// erased by the cut's own belt-speed bleed in low gears (~2500 rpm per m/s slope in gear
/// 2), which fired the down band the tick the freeze lifted — the measured 1-2-1-2 climb
/// trace. SIM POLICY.
///
/// The declutched predictor carries no crank drag because the coupling transmits nothing.
/// With whole combustion, the full-pedal gross/drag pair retains the authored net curve;
/// construction drag remains on the crank even while the belt is disconnected.
///
/// What the margin covers (the predictor decays the drive shear, keeps the ground's rolling
/// resistance whole and uses the coupled vehicle mass, [`predict_shift_landing_m`]): residual
/// model error, chiefly the SUSTAINED grade component of the window's deceleration that outlives
/// the decayed drive-shear reaction. On a steep sustained grade that residual can exceed the
/// margin; what then prevents hunting is the architecture around the landing, not
/// predictor exactness — the reversal dwell blocks the down band for 32 post-window ticks,
/// and the upshift's own reserve gate guarantees the landed gear can out-pull the filtered
/// demand and recover rpm inside that dwell.
///
/// DOMAIN. ORDINARY upshifts are intent-gated (`propulsive > 0`) and L600-detent-deferred, so
/// the full predictor is consulted only inside the domain it is valid on; the protective
/// ceiling rescue does not consult it at all — its selection is a best-effort STATIC
/// projection, with the over-rev slip guard as the actual crank bound (module doc).
///
/// The band bound is only half the gate. Stage A: the predicted landing SHAFT speed must also
/// be POSITIVE on the engaged ladder, so a sign-flipped landing always refuses. Read through
/// `|m|` a backward landing (traced: `r_mean` = 221 kN, `landing_m` = −3.62) read as
/// "9092 rpm", cleared this margin, and blessed catastrophic on-grade upshifts.
/// Crate-visible (like [`reserve_margin`] and [`predict_shift_landing_m`]) so read-only
/// instrumentation states the landing gate as `shift_down_rpm + POSTSHIFT_MARGIN_RPM` from
/// THIS constant instead of restating the number — see the driving-feel probes in
/// `headless_test`. Visibility only; no scheduling behaviour reads it differently.
pub(crate) const POSTSHIFT_MARGIN_RPM: f32 = 150.0;

/// Fix-1a landing-predictor reaction decay, per predicted tick. The
/// pre-cut reaction is dominated by the DRIVE shear the grip elements carry; once the
/// torque cut lands, that shear is no longer sustained and the bristle field relaxes over
/// [`forces::GRIP_SHEAR_MODULUS_M`] (75 mm) of relative motion — at the ~1 m/s slip scale
/// of a loaded low-gear climb that is τ ≈ 75 ms ≈ 4.8 ticks at 64 Hz. The decay is the
/// DETERMINISTIC per-tick multiplicative stand-in for that exponential: 0.8 ≈ 1 − dt/τ
/// (no `exp()` in sim code); over a 20-tick window the geometric sum charges an effective
/// Σρᵏ ≈ 4.9 ticks of full reaction instead of the frozen model's 20. The predictor that
/// applies this decay also decelerates the COUPLED vehicle mass, the grip law's own side weight
/// over g — see [`predict_shift_landing_m`]. SIM POLICY.
const LANDING_REACTION_DECAY: f32 = 0.8;

/// Standard gravity (m/s²) — the same value the grip law authors its side weight with
/// (`forces::GripParams::of`, through `track::derive::G`). Needed here
/// only to turn that weight back into the landing predictor's coupled mass.
const GRAVITY_M_PER_S2: f32 = 9.81;

/// Crank-corroboration margin (rpm) above governed: past this floor a shaft reading is
/// trusted by the ORDINARY band arm only if the crank reads past it too (a clutch-infeasible
/// belt transient must not drive gear selection; locked driving keeps crank == shaft,
/// propulsive slip puts the crank ABOVE the shaft, never below). This margin fires no shift
/// on its own: on overrun the box holds its gear for engine braking, and the last-resort
/// rescue sits at the mechanical-protection ceiling
/// ([`TransmissionParams::max_curve_rpm`] — see the module doc and the decision block).
/// SIM POLICY.
const CRANK_CORROBORATION_MARGIN_RPM: f32 = 150.0;

/// Fix-1b anti-hunting dwell (fixed ticks, 0.5 s at 64 Hz): after a shift commits, the
/// OPPOSITE-direction shift stays blocked this long. Same-direction shifts stay free — a
/// rapid 1-2-3 climb must not slow down. The dwell counts only OUTSIDE the interruption
/// window (the frozen window blocks every decision anyway, and draining the dwell inside it
/// left ~12 of the promised 32 post-engagement ticks).
///
/// Second consumer: a SHALLOW confirmed reserve deficit waits this dwell out while HOLDING
/// the gear rather than committing — evidence keeps accumulating and the ordinary upshift
/// stays suppressed, so a real deficit corrects at expiry while the post-window
/// re-acceleration shear that inflated the demand EMA decays. A deficit DEEPER than
/// [`reserve_margin`] overrides the deferral immediately (measured limit cycles and the
/// near-arrest rationale are on the deferral block in [`run_shift_decision`]). SIM POLICY.
const REVERSAL_DWELL_TICKS: u8 = 32;

/// Fix-1c over-rev margin (rpm): a downshift is refused if its landing rpm in the lower
/// gear would exceed the engine's max authored curve rpm minus this margin — the box never
/// commands an over-rev. The SAME margin sizes the over-rev slip guard's
/// band ABOVE the curve top (`ω_over = max_curve_rpm + OVERREV_MARGIN_RPM`, see
/// [`clutch_coupling`]) — one policy scale polices the ceiling from both sides.
/// Crate-visible so read-only instrumentation (the steep-descent crank-bound probes) can
/// assert against the guard point without restating it. SIM POLICY.
pub(crate) const OVERREV_MARGIN_RPM: f32 = 100.0;

/// Stage-C reserve margin as a fraction of the filtered mean-axis demand — the common
/// capability headroom. DERIVED policy value: ten percent keeps a target gear away from the
/// zero-acceleration knife edge without encoding a vehicle-specific force. SIM POLICY.
const RESERVE_MARGIN_FRACTION: f32 = 0.10;

/// ASSUMED scheduler resolution policy, dimensionless: reserve a small fraction of the
/// vehicle's bounded drive/contact capacity even when measured demand is near zero.
const RESERVE_CAPACITY_FRACTION: f32 = 0.02;

/// Stage-C load filter RISE time scale in fixed decision ticks. An EMA divisor of eight is a
/// deterministic ~0.125 s DERIVED low-pass at the 64 Hz SIM POLICY; the state freezes while the box is declutched so
/// torque-cut reaction transients cannot rewrite grade demand. The filter is ASYMMETRIC:
/// this constant governs a RISING sample only (safety — new load is
/// believed fast, and the rising edge stays bit-identical to the old symmetric filter);
/// a falling sample follows [`DEMAND_FALL_FILTER_TICKS`]. SIM POLICY.
const DEMAND_FILTER_TICKS: f32 = 8.0;

/// Stage-C load filter FALL time scale in fixed decision ticks.
/// "Pessimistic about losing load": a COLLAPSING
/// demand sample is believed slowly, at 32 ticks = 0.5 s, while rising demand keeps the
/// fast [`DEMAND_FILTER_TICKS`] rise. The measured mechanism: a rearward main-gun shot on
/// a climb unloads the suspension, the raw contact-projected demand collapses ~50% for
/// ~22–25 ticks, the old symmetric 8-tick EMA followed it down, and the stage-C reserve
/// gate opened FALSELY — one leg of the reproduced F1→2→3→1 fire-backward-uphill shift
/// storm (the other leg is [`UPSHIFT_CONFIRM_TICKS`]'s landing-gate flip). Probe-measured
/// bounds, so nobody retunes this blindly: fall 16 still let the dropout fire the gate
/// (17 ticks open); fall 32 suppresses it entirely at ≤8 ticks of added legit demand lag
/// on the clean 10% climb; fall ≥64 explodes latency at capability-boundary upshifts
/// (125–400 ticks) because the inflated post-window demand takes seconds to bleed off.
/// SIM POLICY.
const DEMAND_FALL_FILTER_TICKS: f32 = 32.0;

/// Consecutive reserve-deficit decision ticks required before a grade downshift (13 ticks DERIVED
/// = 0.203125 s DERIVED at the 64 Hz SIM POLICY). Shorter reaction spikes remain load telemetry, not shift commands.
/// SIM POLICY.
const GRADE_CONFIRM_TICKS: u8 = 13;

/// Consecutive decision ticks the FULL ordinary-upshift predicate — every condition the
/// commit itself needs: propulsive intent, signed shaft rpm above the up band, detent
/// released, crank corroboration, no pending deficit correction, AND the fix-1a landing
/// gate AND the stage-C reserve gate — must hold before the upshift ARMS. A band-only
/// counter is NOT enough (see below). The evidence is HARD-RESET by any tick
/// the predicate fails, not leaky like the deficit's, and the difference is the signal:
/// the deficit reads contact-reaction demand, which jitters tick-to-tick around a
/// persistent truth (one clean sample must not erase twelve dirty ones), while a single
/// tick that fails any commit condition genuinely refutes "ready" — a hard reset makes
/// the gate immune to EVERY decaying transient shorter than the window.
///
/// Why the FULL predicate (probe-measured): a band-only
/// counter guarded a sub-condition that on a lugging climb is PERMANENTLY true — the
/// probe showed `band_confirm_ticks` saturated at 255 in F1 on every steep grade — while
/// the landing-band gate that a recoil actually flips was evaluated instantaneously. A
/// rearward shot's ~0.14 m/s forward shove lifts the predicted landing past
/// `shift_down + POSTSHIFT_MARGIN_RPM` for ~4 ticks (62 ms, measured uncontaminated on
/// the 45% grade where the box refused), which committed the F1→2→3→1 shift storm and
/// cost ~26% of the climb per shot at 40%.
///
/// Why N = 8 (probe-sized, `probe_fire_backward_uphill` + the integrator study): the
/// recoil evidence lasts 4 consecutive ticks and is suppressed at N ≥ 6; legit upshifts
/// on the clean 10% climb entered their commit with the full predicate already true for
/// 11/13/13/1/1 ticks, so N = 8 costs at most 7 ticks ≈ 110 ms of added latency on the
/// worst legit upshift (measured mean 2.8) while holding 2× margin over the recoil
/// transient. The PROTECTIVE ceiling rescue is exempt — crank safety must not wait a
/// confirmation. Crate-visible so read-only instrumentation (the driving-feel probes)
/// states the requirement from THIS constant instead of restating the number. SIM POLICY.
pub(crate) const UPSHIFT_CONFIRM_TICKS: u8 = 8;

/// Hill-hold anti-oscillation cooldown after a release (32 fixed ticks = 0.5 s DERIVED at 64 Hz).
/// Never overridable: the latch engages only near rest, so a genuine roll
/// is braked by `back_driven_intent` until the hull re-enters the zone. SIM POLICY.
const HOLD_REENGAGE_TICKS: u8 = 32;

/// Fuel-governor cut width (rpm): torque ramps linearly to zero over this band past the
/// governed rpm, so the top-speed equilibrium is a smooth root instead of a hard clip.
/// INFERRED numerical policy, not vehicle data — a smoothing width, and any governed engine
/// gets the same treatment. SIM POLICY.
const GOVERNOR_CUT_RPM: f32 = 100.0;

/// Idle-governor droop width (rpm): the idle governor's recovery torque ramps linearly from
/// zero at idle to FULL `torque_at(idle)` this far below it (gain = `torque_at(idle) /
/// (K_IDLE_DROOP_RPM·RPM_TO_RAD)` N·m per rad/s), saturating beyond. Expressing the gain as a
/// droop WIDTH is what keeps it policy: it is a governor stand-in, not an engine datum — any
/// governed engine gets the same recovery shape, and the TORQUE it recovers with is the
/// vehicle's own curve. SIM POLICY.
const K_IDLE_DROOP_RPM: f32 = 50.0;

/// Resolution: idle guard band (rpm), twice the idle-governor droop width so the governor
/// saturates before the clutch slips. The game's damage model guards this speed only when
/// combustion can exceed motoring drag there (`crank_guards`); otherwise the floor is rest.
/// The guard cannot add crank energy during recovery from below the band.
const STALL_GUARD_BAND_RPM: f32 = 100.0;

/// Resolution: regularise the drag sign only within one rpm of rest; the public readout
/// resolves whole rpm. The nonnegative crank integration clips a crossing to rest;
/// the healthy idle guard must not set a physical run-down time constant.
const CRANK_DRAG_FADE_RPM: f32 = 1.0;

/// Declutched rev-match proportional band (rpm): full fueling one band below the landing
/// target, tapering to zero at it (`u_match = clamp((ω_target − ω_e)/band, 0, 1)`) — a smooth
/// approach at 64 Hz instead of bang-bang chatter. Only the BAND is policy; the match
/// AUTHORITY is the vehicle's own torque curve over its own J. SIM POLICY.
const REV_MATCH_BAND_RPM: f32 = 200.0;

/// Clutch-seam hysteresis on |m| (stage B) — the coupling seam is a
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

/// Propulsive command magnitude at which the healthy engine reaches its authored net curve.
/// Below it the hold-blend shapes the transition from motoring to combustion; construction
/// friction remains at every command. A brake command is not propulsive. SIM POLICY.
const DRAG_THROTTLE_RELEASE: f32 = 0.5;

/// Shape of the rising motoring-torque curve ([`engine_drag`]): the CONSTANT
/// share of the mid-band magnitude; the remaining `1 − share` scales linearly with crank
/// speed, normalized at mid-band `ω_mid = (idle + governed)/2`. Physical premise
/// (Chen–Flynn-form friction mean effective pressure): motoring losses are a Coulomb/
/// compression base plus viscous/pumping terms that GROW with piston speed — roughly
/// doubling across the operating range — so a back-driven crank resists harder the faster
/// the hill spins it, and each gear acquires a natural downhill equilibrium speed instead
/// of a flat-torque runaway. The split is engine-class-uniform SIM POLICY; the vehicle's
/// authored `drag_fraction × peak_torque` stays the magnitude anchor (exact at ω_mid, so
/// ordinary mid-band coast feel is unchanged). One third base / two thirds linear puts the
/// Tiger at ≈ 0.59× at idle, 1× at 1550, ≈ 1.41× at governed 2500, ≈ 1.62× at rated 3000.
const MOTORING_DRAG_BASE_SHARE: f32 = 1.0 / 3.0;

/// Input deadzone for direction/brake intent on the THROTTLE axis, and for "is the stick at
/// zero" on the STEER axis ([`update_park_latch`], [`update_hill_hold`]'s climb intent): one
/// shared deadzone across the drive-axis mapping, the same for every tank. SIM POLICY.
const DEAD: f32 = 0.05;

/// Belt speed (m/s) below which a zero command LATCHES the parking brake (released by any
/// drive command). A latch, not a blend: once parked the brake holds full capacity however
/// fast a capacity breach back-drives the belt — the engagement blend alone faded to zero
/// past `slip_saturation`, releasing the brake exactly when an over-capacity slope slid the
/// tank.
pub(crate) const PARK_ENGAGE_SPEED: f32 = 0.05;

/// Hill-hold near-rest threshold (m/s), derived from the existing parking-latch scale. Five times the
/// park threshold (0.25 m/s DERIVED = 0.90 km/h DERIVED) catches the sequential cascade before a perceptible
/// rollback while remaining firmly in the stop-force law's near-rest regime. SIM POLICY.
const HILL_HOLD_ENGAGE_SPEED: f32 = PARK_ENGAGE_SPEED * 5.0;

/// |m| below which a commanded direction reversal actually swaps the F/R ladder (above it the
/// opposing gear force acts as driveline braking first — you cannot slam reverse at speed).
const DIRECTION_SWAP_SPEED: f32 = 0.5;

/// L600 neutral-turn entry: |throttle| below this AND |m| below [`NEUTRAL_M_SPEED`] puts the
/// box in the brake-gated pivot regime instead of the radius constraint. REGIME-ENTRY
/// thresholds only — the neutral turn's SPEED SCALE is the spec-DERIVED
/// [`TransmissionParams::neutral_d_full`], not a module constant. SIM POLICY.
const NEUTRAL_THROTTLE: f32 = 0.1;
/// The |m| half of that entry test. [`CLUTCH_OUT_M_SPEED`]/[`CLUTCH_IN_M_SPEED`] are derived
/// from it. SIM POLICY.
const NEUTRAL_M_SPEED: f32 = 0.5;

/// Steering-detent hysteresis on |steer| (design: two steps per gear, `|steer| ≥ 0.5` tight):
/// straight→wide engages at `WIDE_ON`, releases at `WIDE_OFF`; wide→tight at `TIGHT_ON`,
/// back at `TIGHT_OFF`. STICK-TO-DETENT input mapping only — the detent RATIOS these select
/// are spec ([`TransmissionParams::steer_kappa`], authored as `steering.radii`). SIM POLICY.
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
    /// Per FORWARD gear `(κ_tight, κ_wide)` where `κ = half_tread / R` — the L600 detents.
    /// Reverse gears index the same table (R1–R4 mirror F1–F4).
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
    /// `neutral_fraction` feel scalar that used to shrink it was DELETED as unprovenanced.
    pub neutral_d_full: f32,
    /// Inner→outer recirculation efficiency η (mechanical ~0.9, INFERRED tag at the authoring
    /// site).
    pub recirculation: f32,
    /// Per-side service/parking brake capacity at the sprocket (N).
    pub brake_capacity_n: f32,
    /// Static breakaway capacity multiplier for a latched, at-rest belt. Dynamic dissipation,
    /// service braking, and every moving slide use [`Self::brake_capacity_n`] unchanged.
    pub brake_static_factor: f32,
    /// Zero-throttle engine drag (compression braking): the MID-BAND anchor of the rising
    /// motoring-torque curve, as a fraction of peak torque. [`engine_drag`] equals
    /// `drag_fraction × peak` at `ω_mid = (idle + governed)/2` and grows linearly with
    /// crank speed from a [`MOTORING_DRAG_BASE_SHARE`] base (pumping and friction losses
    /// rise with speed, giving each gear a bounded downhill equilibrium).
    /// A drag TORQUE at the crank (design §3), never the negative half of rated power.
    /// Diesel motoring/compression braking runs ~20–30% of rated torque mid-band
    /// (INFERRED band, tagged at the authoring site).
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
    /// DERIVED per-side belt translation + reflected wheel inertia (kg); the coupling uses twice this value.
    pub belt_inertia: f32,
    /// Gear-shift torque-interruption time (s) — see [`TransmissionParams::shift_ticks`].
    pub shift_secs: f32,
    /// See [`TransmissionParams::shift_addressing`].
    pub shift_addressing: ShiftAddressing,
    pub sprocket_radius_m: f32,
    /// Track half-tread `b` (m) — the spec's `plane_x`.
    pub half_tread_m: f32,
}

pub(crate) const RPM_TO_RAD: f32 = std::f32::consts::TAU / 60.0;

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
                "sprocket radius and track half-tread",
                a.sprocket_radius_m.is_finite()
                    && a.sprocket_radius_m > 0.0
                    && a.half_tread_m.is_finite()
                    && a.half_tread_m > 0.0,
            ),
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
                // Sanity bounds beyond finiteness: an absurd finite torque can overflow
                // reflected torque and crank work. Refuse it before the physical solve.
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
                // DERIVED healthy idle-guard scale: the 100 rpm guard band needs
                // positive headroom. The game's damage model can still reach rest.
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
                // Every post-upshift landing must remain above the down band PLUS the runtime
                // landing margin: the fix-1a gate demands `landing ≥ shift_down +
                // POSTSHIFT_MARGIN_RPM` at the end of the torque-cut window, so a triple that
                // clears only the bare down band can NEVER legally shift at its own trigger
                // rpm — it "works" solely by over-running the band to the governor (the
                // shipped Tiger's original 2300/1400 did exactly that: min step 0.651 landed
                // 1498 < 1550). Fail loudly at asset load instead.
                "gearbox shift-band hysteresis vs ratio steps",
                a.forward_speeds_kmh
                    .windows(2)
                    .chain(a.reverse_speeds_kmh.windows(2))
                    .all(|w| {
                        a.shift_up_rpm * w[0] / w[1] > a.shift_down_rpm + POSTSHIFT_MARGIN_RPM
                    }),
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
                "derived belt inertia floor (coupling divisor)",
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
        let derived = Self {
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
        };
        derived.check_derived(a.belt_inertia)?;
        Ok(derived)
    }

    /// The authored scalars can each be finite while their ratios, squared coupling terms or
    /// force capacities overflow/underflow. Refuse those coupled outputs before publication.
    fn check_derived(&self, belt_inertia: f32) -> Result<(), BevyError> {
        let fail = |field: &str| {
            BevyError::from(format!(
                "track.powertrain.transmission: non-finite or nonpositive derived {field}; correct gearbox speeds, steering radii, sprocket radius or inertia"
            ))
        };
        for &gear in self.gears_fwd.iter().chain(&self.gears_rev) {
            let coupling = gear / self.sprocket_radius;
            let inverse_inertia =
                1.0 / self.engine_inertia + coupling * coupling / (2.0 * belt_inertia);
            let force = self.peak_torque_nm.min(self.clutch_capacity) * coupling;
            if ![gear, coupling, inverse_inertia, force]
                .into_iter()
                .all(|value| value.is_finite() && value > 0.0)
            {
                return Err(fail("gear reduction, coupling divisor or drive force"));
            }
        }
        for &(tight, wide) in &self.steer_kappa {
            if ![tight, wide]
                .into_iter()
                .all(|value| value.is_finite() && value > 0.0 && (1.0 + value * value).is_finite())
            {
                return Err(fail("steering curvature"));
            }
        }
        if !(2.0 * belt_inertia).is_finite()
            || !self.neutral_d_full.is_finite()
            || self.neutral_d_full <= 0.0
            || !(2.0 * self.brake_capacity_n).is_finite()
            || !(2.0 * self.steer_capacity_n).is_finite()
            || ![self.governed_speed(false), self.governed_speed(true)]
                .into_iter()
                .all(|speed| speed.is_finite() && speed > 0.0)
        {
            return Err(fail(
                "neutral speed, governed speed or paired force capacity",
            ));
        }
        Ok(())
    }

    /// DERIVED top-gear speed (m/s) at the fuel governor's rpm for the selected ladder.
    /// This scales the brake's command blend; external loads may drive the belts faster.
    pub fn governed_speed(&self, reverse: bool) -> f32 {
        let ladder = if reverse {
            &self.gears_rev
        } else {
            &self.gears_fwd
        };
        let top = ladder
            .last()
            .expect("authoring requires a nonempty gear ladder");
        self.engine.governed_rpm * RPM_TO_RAD * self.sprocket_radius / top
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
    /// Main-clutch-out latch (stage B): the coupling-seam regime with
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
    /// Consecutive decision-tick evidence that the FULL ordinary-upshift predicate holds
    /// (propulsive intent, signed shaft rpm above the up band, detent
    /// released, crank corroboration, no pending deficit correction, landing gate, reserve
    /// gate). HARD-RESET by any non-qualifying tick — see [`UPSHIFT_CONFIRM_TICKS`] for
    /// why this one is not leaky. Saturating u8. The wire slot is unchanged since REV 20
    /// (same u8, same projection position); the COUNTED predicate — and so the field's
    /// replicated dynamics — changed in REV 21 (it no longer saturates on a lugging climb:
    /// a failing landing or reserve gate now zeroes it every decision tick).
    pub band_confirm_ticks: u8,
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
    /// re-latch — never overridable (the latch itself is near-rest-only, and a
    /// moving roll is `back_driven_intent` braking territory until it re-enters the zone).
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

/// The exhaustive REV-14 transmission inventory in its canonical replication/hash/trace order.
/// Adding state fails this destructure until the field is classified exactly once here.
pub(crate) fn transmission_state_projection(
    state: &TransmissionState,
) -> [TransmissionProjectionValue; 17] {
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
        band_confirm_ticks,
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
        U8(gear),
        U8(shift_ticks),
        U8(steer_step),
        Bool(reverse),
        Bool(park),
        I8(last_shift_dir),
        U8(dwell_ticks),
        F32(omega_e),
        Bool(clutch_out),
        F32(demand_n),
        Bool(demand_initialized),
        U8(grade_confirm_ticks),
        U8(band_confirm_ticks),
        U8(grade_target),
        scheduler,
        Bool(hill_hold),
        U8(hold_reengage_ticks),
    ]
}

impl TransmissionState {
    /// Construct complete regenerative transmission state synchronously from validated vehicle
    /// data. The crank starts at the authored idle speed; demand remains intentionally unseeded
    /// until the first owned contact-reaction sample arrives.
    pub fn from_spec(tp: &TransmissionParams) -> Self {
        Self::with_crank_speed(tp.engine.idle_rpm * RPM_TO_RAD)
    }

    /// A state at rest with its crank stopped: the fixture for tests whose subject is not the
    /// transmission (replication scope, probe layout, HUD formatting).
    #[cfg(test)]
    pub(crate) fn stopped() -> Self {
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
            band_confirm_ticks: 0,
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
    /// sub-idle lugging and, in the game's damage model, a crank at rest when combustion
    /// cannot sustain idle (`crank_guards`).
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

/// The game's damage model: independent staffing, task throughput and local module condition.
/// The drivetrain consumes the task rate once; source condition also remains available when
/// the task has no throughput. Brakes and construction losses retain their own laws.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DriveSupply {
    pub control_authority: bool,
    pub task_rate: f32,
    /// The engine's held HP share; the idle governor reads its own source condition.
    pub engine_condition: f32,
    /// The transmission's held HP share; clutch and steering capacity read this source.
    pub transmission_condition: f32,
}

impl DriveSupply {
    /// The game's damage model: the sandbox lab's staffed, full-rate, whole-source fixture.
    pub const WHOLE: Self = Self {
        control_authority: true,
        task_rate: 1.0,
        engine_condition: 1.0,
        transmission_condition: 1.0,
    };
}

/// One tick's joint input: the SHAPED drive axes plus the per-side mixed commands (the brake
/// envelope consumes the sides; the drive and steering laws consume the axes), the pre-tick
/// belt speeds, and this tick's summed longitudinal ground reactions.
pub struct TransmissionInput {
    /// The game's damage model: what the crew, engine and transmission each supply this tick.
    pub supply: DriveSupply,
    pub throttle: f32,
    pub steer: f32,
    pub side_commands: [f32; 2],
    pub speeds: [f32; 2],
    pub reactions: [f32; 2],
    /// Each side's rolling resistance inside `reactions` (N, `forces::SideReport::rolling_resistance`):
    /// the ground's share, which a cut drive does not relax.
    pub rolling: [f32; 2],
    /// Unfiltered terrain/construction observation; instrumentation only.
    pub perceived_load: super::load::PerceivedLoad,
    pub dt: f32,
    /// The driver holds the service brakes: no drive at any throttle, the full service brakes while
    /// the tank moves, and the parking latch once it is at rest (`driver_intent`).
    pub brake: bool,
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
    /// Combustion power capacity at the pre-tick operating point (W). The work gate also
    /// accounts for crank inertia and the speeds reached during this tick.
    pub power_available: f32,
    #[cfg(feature = "bitprobe")]
    pub(crate) bitprobe: TransmissionProbe,
}

/// The game's damage model: construction motoring drag at the crank (N m), independent of
/// pedal and combustion. Its authored mid-band anchor is `drag_fraction × peak torque`;
/// the affine Coulomb/viscous shape rises with speed. Only the numerical rest band fades it.
/// Combustion reconstructs the gross torque behind the authored net curve (`engine_torques`).
fn engine_drag(tp: &TransmissionParams, omega_e: f32) -> f32 {
    let omega_mid = (tp.engine.idle_rpm + tp.engine.governed_rpm) * 0.5 * RPM_TO_RAD;
    let rise = MOTORING_DRAG_BASE_SHARE + (1.0 - MOTORING_DRAG_BASE_SHARE) * (omega_e / omega_mid);
    tp.peak_torque_nm
        * tp.drag_fraction
        * rise
        * (omega_e / (CRANK_DRAG_FADE_RPM * RPM_TO_RAD)).clamp(0.0, 1.0)
}

/// Fix-1a: roll the shift's torque-cut window forward on the mean belt axis and return the
/// PREDICTED landing speed `m`, under the shift window's own stage-B rules: the box is
/// DECLUTCHED, so the belt gets NO engine force and NO engine drag (the old predictor's
/// drag-through-the-landing-gear term is absent). The ground reaction changes the mean
/// speed under the same inertia law as the live belt integration.
///
/// THE WINDOW MODEL (the physical premise). The pre-cut `r_mean` holds the DRIVE shear the
/// grip elements carry and the ground's rolling resistance `rolling_mean` inside it. Cutting
/// the drive removes the shear: the bristle field relaxes over [`forces::GRIP_SHEAR_MODULUS_M`]
/// of relative motion (~5 ticks — see [`LANDING_REACTION_DECAY`]), so that share DECAYS
/// geometrically instead of staying frozen. The rolling resistance does not relax: the belt
/// still rolls on its ground through the window, so it is charged whole every tick (above the
/// fade band, where an upshift happens). And the mass that reaction decelerates is the
/// COUPLED one: through the still-gripping (merely relaxing) bristle the belt pair carries
/// the hull, so per side the inertia is `fp.inertia + (W/2)/g` — the belt's links and wheel
/// moments reflected at belt speed, plus this side's share of the sprung vehicle — `W/2` the
/// grip law's own [`forces::GripParams::side_weight`], no new vehicle datum. The rejected
/// frozen-`r_mean`-over-belt-inertia form charges the window to the belt inertia alone: on a
/// 57 t Tiger with an authored 32 t of belt inertia across both sides absorbing 20 ticks of
/// full driving reaction, on a 10% grade it predicted a 156 rpm landing against a 1550 rpm
/// floor and refused F1→F2 forever
/// (measured governor-pinned at 2591 rpm / 0.67 m/s), while the same tank climbed F1→F4 on
/// the flat. Units stay the live mean-axis convention (the per-side integration
/// `v += (Q − R)/I·dt`): per-side mean reaction over per-side inertia.
///
/// Deterministic by construction: fixed `shift_ticks` iterations, one multiply-add per
/// tick, no transcendentals — the exponential decay IS the per-tick multiplicative
/// constant. A support-only rig (no [`forces::ForceParams::grip`]) contributes no vehicle mass and
/// degenerates to the bare belt inertia, conservatively.
///
/// DOMAIN: valid for the PROPULSIVE
/// straight-line case ONLY — the only case the ORDINARY band gate consults it for (band
/// upshifts are intent-gated on `propulsive > 0` and detent-deferred on the L600). It
/// carries no brake term and no λ/steer state; the GRADE paths read only its SIGN, which
/// is robust in those regimes. The protective ceiling rescue does NOT consume it at all:
/// its decayed reaction models DRIVE shear relaxing after a torque cut, which on genuine
/// overrun would mis-model the hill-driven window — the rescue's selection is best-effort
/// static projection with the over-rev slip guard as the crank's actual bound (module
/// doc). The un-modeled
/// residual (the sustained grade deceleration that outlives the drive shear) is owned by
/// [`POSTSHIFT_MARGIN_RPM`]'s architecture note.
pub(crate) fn predict_shift_landing_m(
    tp: &TransmissionParams,
    fp: &ForceParams,
    m: f32,
    (r_mean, rolling_mean): (f32, f32),
    dt: f32,
) -> f32 {
    let side_vehicle_mass = fp
        .grip
        .map_or(0.0, |grip| grip.side_weight / GRAVITY_M_PER_S2);
    let i_coupled = fp.inertia + side_vehicle_mass;
    let mut pm = m;
    let mut shear = r_mean - rolling_mean;
    for _ in 0..tp.shift_ticks {
        pm -= (shear + rolling_mean) / i_coupled * dt;
        shear *= LANDING_REACTION_DECAY;
    }
    pm
}

/// The game's damage model: the full-load curve is net of construction losses. Reconstruct
/// the combustion torque it implies, scale only that source, and charge drag whole. The pedal
/// blend retains the healthy net curve at every command. The crank and reserve scheduler share
/// these three torques (N m), so an emptied engine keeps friction even under full command.
fn engine_torques(
    tp: &TransmissionParams,
    supply: DriveSupply,
    omega: f32,
    command: f32,
) -> [f32; 3] {
    let drag = engine_drag(tp, omega);
    let released = 1.0 - forces::hold_blend(command / DRAG_THROTTLE_RELEASE);
    let induced = supply.task_rate * (command * tp.torque_at(omega / RPM_TO_RAD) + released * drag);
    let idle_capacity = supply.engine_condition * tp.torque_at(tp.engine.idle_rpm);
    let idle = (idle_capacity * (tp.engine.idle_rpm * RPM_TO_RAD - omega)
        / (K_IDLE_DROOP_RPM * RPM_TO_RAD))
        .clamp(0.0, idle_capacity);
    [induced, idle, drag]
}

/// Full-throttle mean-axis force available in one gear at the current signed shaft speed. The
/// engine reads only non-negative rpm. Its torque and the clutch's capacity bound the shaft
/// torque; gearing converts it to belt force. Contact grip bounds traction separately.
fn available_force_in_gear(
    tp: &TransmissionParams,
    supply: DriveSupply,
    shaft: f32,
    gear: f32,
) -> f32 {
    // Sustained capability prices at least idle; the governor's temporary sub-idle recovery
    // is not reserve the scheduler can spend. The crank and scheduler share the torque law.
    let rpm = (shaft.max(0.0) * gear / tp.sprocket_radius / RPM_TO_RAD).max(tp.engine.idle_rpm);
    let [induced, idle, drag] = engine_torques(tp, supply, rpm * RPM_TO_RAD, 1.0);
    (induced + idle - drag).min(supply.transmission_condition * tp.clutch_capacity) * gear
        / tp.sprocket_radius
}

/// Instrument for the accepted Driver law: price the crank at the landing shaft speed,
/// or the commanded launch speed while slipping, bounded by clutch and contact grip.
/// This observes the existing torque law; it does not select a gear or change delivery.
#[cfg(feature = "bitprobe")]
pub(crate) fn price_driver_force(
    tp: &TransmissionParams,
    supply: DriveSupply,
    shaft: f32,
    gear: f32,
    command: f32,
    grip_n: f32,
) -> f32 {
    let launch_rpm = tp.engine.idle_rpm + command * (tp.peak_torque_rpm - tp.engine.idle_rpm);
    let shaft_rpm = shaft.max(0.0) * gear / tp.sprocket_radius / RPM_TO_RAD;
    // The design prices a fresh sub-idle landing as a launch slip; other landings are In.
    // pricing every sub-peak landing at the peak would invent a slip the clutch never made.
    let rpm = if shaft_rpm < tp.engine.idle_rpm {
        launch_rpm
    } else {
        shaft_rpm
    };
    let [induced, idle, drag] = engine_torques(tp, supply, rpm * RPM_TO_RAD, command);
    ((induced + idle - drag).min(supply.transmission_condition * tp.clutch_capacity) * gear
        / tp.sprocket_radius)
        .min(grip_n)
}

/// DERIVED low-demand headroom (N): a fraction of the lesser of drivetrain force capacity and
/// nominal material-pair grip. A support-only calibration has no grip population and no such floor.
fn reserve_floor(fp: &ForceParams, tp: &TransmissionParams, supply: DriveSupply) -> f32 {
    let contact = match fp.grip {
        Some(grip) => 2.0 * grip.side_weight * grip.on_ground(),
        None => 0.0,
    };
    let reduction = tp.gears_fwd[0].max(tp.gears_rev[0]);
    let drive = (supply.task_rate * tp.peak_torque_nm)
        .min(supply.transmission_condition * tp.clutch_capacity)
        * reduction
        / tp.sprocket_radius;
    RESERVE_CAPACITY_FRACTION * drive.min(contact)
}

/// DERIVED positive capability headroom (N, both tracks), from actual demand and vehicle scale.
pub(crate) fn reserve_margin(
    fp: &ForceParams,
    tp: &TransmissionParams,
    supply: DriveSupply,
    demand: f32,
) -> f32 {
    demand.max(0.0) * RESERVE_MARGIN_FRACTION + reserve_floor(fp, tp, supply)
}

/// Modeled full-throttle reserve for one gear at the current shaft speed.
///
/// Crate-visible (with [`reserve_margin`] and [`predict_shift_landing_m`]) so read-only
/// instrumentation recomputes the scheduler's own decision inputs from this law instead of
/// duplicating it — see the driving-feel probes in `headless_test`.
pub(crate) fn modeled_reserve_in_gear(
    tp: &TransmissionParams,
    supply: DriveSupply,
    shaft: f32,
    gear: f32,
    demand: f32,
) -> f32 {
    available_force_in_gear(tp, supply, shaft, gear) - demand
}

/// Choose the highest gear below `current` that clears reserve margin, then apply the downshift
/// over-rev bound. If the ideal gear itself over-revs, return the closest legal gear on the path
/// toward it (which may not yet clear margin); later decisions can continue after speed falls.
fn select_grade_target(
    tp: &TransmissionParams,
    fp: &ForceParams,
    supply: DriveSupply,
    ladder: &[f32],
    shaft: f32,
    current: u8,
    demand: f32,
) -> Option<u8> {
    if current <= 1 || shaft <= -PARK_ENGAGE_SPEED {
        return None;
    }
    let margin = reserve_margin(fp, tp, supply, demand);
    let ideal = (1..current).rev().find(|&gear| {
        available_force_in_gear(tp, supply, shaft, ladder[(gear - 1) as usize]) - demand >= margin
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
    supply: DriveSupply,
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
                && modeled_reserve_in_gear(tp, supply, shaft, ratio, demand) >= minimum
        })
    };
    highest_with_reserve(reserve_margin(fp, tp, supply, demand))
        .or_else(|| highest_with_reserve(0.0))
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
    st.band_confirm_ticks = 0;
}

/// Re-evaluate a latched hill hold on a decision tick. Returns whether this call committed a new
/// shift step, allowing the ordinary scheduler to avoid spending the same zero-length window twice.
fn refresh_hill_hold(
    st: &mut TransmissionState,
    tp: &TransmissionParams,
    fp: &ForceParams,
    supply: DriveSupply,
    ladder: &[f32],
    shaft: f32,
    rollback_rescue: bool,
) -> bool {
    let downshift_allowed = shaft > -PARK_ENGAGE_SPEED || rollback_rescue;
    match select_hill_hold_target(tp, fp, supply, ladder, shaft, st.gear, st.demand_n) {
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

/// The crank's side of one tick, fixed before the coupling solve and shared by
/// [`clutch_coupling`] and [`settle_crank`]: the pre-tick crank speed `omega_e` and free torque
/// `tau_free` (rad/s, N·m), the crank-to-belt ratio `k` (gear / sprocket radius) and direction sign
/// `dir`, the guard band `[omega_floor, omega_over]` (rad/s) and the step `dt` (s).
#[derive(Clone, Copy)]
struct CrankTick {
    omega_e: f32,
    tau_free: f32,
    k: f32,
    dir: f32,
    omega_floor: f32,
    omega_over: f32,
    dt: f32,
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
/// engine path — the summed ground reactions. This is a PREDICTOR approximation: the later
/// brake stop-forces, the λ mean-axis share
/// (`j_L + j_R = −e` does NOT cancel), all move `m_next`
/// after this solve, and exact pre-accounting is CIRCULAR (the brake law reads the very
/// `q` that needs `F_c`). What makes the approximation safe is the end-of-step FEASIBILITY
/// check in [`regenerative`]: the crank re-anchors to the belt that actually integrated
/// only if the implied total clutch torque fits the capacity — otherwise the honestly
/// integrated (slipping) crank stands.
///
/// STALL GUARD (one-sided): if the transmitted τ_c would land the crank below
/// `ω_floor` from `crank_guards`, τ_c is REDUCED to land at that floor. The game's damage
/// model uses the idle band only when combustion can sustain it; otherwise rest is the floor.
/// A recovery tick also bounds the floor by the freely reachable crank speed, so the guard
/// cannot create energy. The guard only reduces clutch torque.
///
/// OVER-REV SLIP GUARD (the stall guard's exact mirror, bought with field
/// evidence: on the rescaled steep world a back-driven crank followed the belt to ~9000
/// rpm, tripling its mechanical limit, because gear selection alone cannot bound the crank
/// when the ladder runs out — a top-gear reverse descent has NO gear to shift to). If the
/// transmitted τ_c would land the crank ABOVE `ω_over = max_curve_rpm +
/// OVERREV_MARGIN_RPM`, τ_c is RAISED to land exactly at ω_over: the clutch slips rather
/// than let the hill grenade the engine (the crew analogue: riding the clutch on an
/// over-revving descent; engine DAMAGE stays deliberately unmodeled, exactly like stall
/// death). At the guard point the crank still transmits its FULL motoring drag — the
/// maximum braking sustainable without over-rev — so retardation degrades gracefully, it
/// does not vanish. Scope of that claim: in a STRAIGHT overrun both
/// sprocket powers are regenerative, `net ≤ 0`, and the power gate cannot scale the
/// transmitted torque — the guard-point drag reaches the belt in full (pinned by
/// magnitude in `overrun_slip_guard_bounds_crank_at_top_gear`); under a simultaneously
/// POWERED steer the common gate may scale τ_c with every other engine-borne force. The
/// crank BOUND itself is unconditional either way — the guard, the re-anchor condition,
/// and the end-of-tick clamp never pass ω_over regardless of scaling. The guard sits one
/// OVERREV_MARGIN_RPM above the rescue trigger
/// ([`TransmissionParams::max_curve_rpm`]), so the protective upshift's crank
/// corroboration still reads past the ceiling and fires before the guard saturates.
fn clutch_coupling(
    crank: CrankTick,
    j_e: f32,
    capacity: f32,
    m: f32,
    i_m: f32,
    f_other: f32,
    #[cfg(feature = "bitprobe")] probe: &mut TransmissionProbe,
) -> f32 {
    let CrankTick {
        omega_e,
        tau_free,
        k,
        dir: s,
        omega_floor,
        omega_over,
        dt,
    } = crank;
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
    } else if omega_next > omega_over {
        // Land exactly at the over-rev guard point: same formula, other side. By
        // construction τ_guard > τ_c (more torque = lower landing), so this side only
        // ever increases — one-sided, and mutually exclusive with the stall branch.
        tau_c = (tau_free - (omega_over - omega_e) * j_e / dt).clamp(-capacity, capacity);
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

/// The F/R ladder swap: near standstill ONLY. Above [`DIRECTION_SWAP_SPEED`] a commanded
/// reversal is a BRAKE command (the service brakes) until the tank is nearly stopped.
///
/// A ladder swap is not an up/down shift, so the reversal dwell restarts clean. F and R project
/// the same physical reactions with OPPOSITE signs, so the old ladder's demand EMA and
/// confirmation history are not evidence about the newly engaged direction: both are dropped so
/// the demand observer seeds directly from this tick's new-ladder sample.
fn swap_ladder_direction(
    tp: &TransmissionParams,
    st: &mut TransmissionState,
    inp: &TransmissionInput,
    m: f32,
) {
    if st.shift_ticks != 0 {
        return;
    }
    let want_rev = inp.throttle < -DEAD;
    let want_fwd = inp.throttle > DEAD;
    if (want_rev || want_fwd) && want_rev != st.reverse && m.abs() < DIRECTION_SWAP_SPEED {
        st.reverse = want_rev;
        st.gear = 1;
        st.shift_ticks = tp.shift_ticks;
        st.last_shift_dir = 0;
        st.dwell_ticks = 0;
        st.demand_n = 0.0;
        st.demand_initialized = false;
        st.grade_confirm_ticks = 0;
        st.band_confirm_ticks = 0;
        st.grade_target = 0;
        st.scheduler = SchedulerState::Normal;
    }
}

/// The tick's resolved driver intent, measured against the ENGAGED ladder.
struct DriverIntent {
    /// Ladder sign: `+1` forward, `−1` reverse. Every signed drivetrain quantity (`shaft`, the
    /// release test, the demand projection) is measured against it.
    dir: f32,
    /// Drive command WITH the engaged ladder, `0..1`.
    propulsive: f32,
    /// Service-brake command, `0..1` — the driver-intent share of the declared brake capacity.
    service: f32,
}

/// Resolve the shaped drive axes into the declared W/S contract, with honest mechanisms — never
/// conflating zero-throttle with brake-to-zero:
///   * throttle WITH the engaged ladder → drive (`propulsive`);
///   * throttle AGAINST it → SERVICE BRAKES (`service`, the declared brake capacity) until near
///     standstill, where [`swap_ladder_direction`] engages the opposite gears — never
///     `|throttle|`-drive in the engaged direction (that was the measured "cannot decelerate"
///     bug: full reverse at speed produced full FORWARD force);
///   * throttle WITH the ladder while the belt is BACK-DRIVEN (past the swap, or a rollback
///     under W) → drive AND service brakes together until the belt crosses zero (the
///     through-zero half of the signed-intent contract);
///   * throttle released → coast under engine drag (compression braking through the CURRENT
///     gear, growing as the box downshifts);
///   * zero command at rest → the parking hold (unchanged).
///
/// THE THROUGH-ZERO HALF (feel): a drive command held while the belt still moves AGAINST the
/// engaged ladder keeps the driver's foot on the BRAKE as well. This closes the measured
/// free-roll gap in the held-S stop→reverse flow: the ladder swap commits at
/// |m| < DIRECTION_SWAP_SPEED while the hull still rolls the OLD way, `opposing` flips false the
/// same tick, and the declutched swap window then carried NEITHER drive NOR brake — downhill the
/// tank re-accelerated into its own reversal (the reported "jumpy" crossing). W while rolling
/// backward is the symmetric case (intent opposing MOTION brakes with everything; the same press
/// flows into motion once the belt crosses zero and the back-driven arm drops). Thresholded at
/// the existing −[`PARK_ENGAGE_SPEED`] policy scale so the at-rest numerical residual and a
/// from-rest launch never read as back-driven.
///
/// THE BRAKE HOLD ([`TransmissionInput::brake`]): no drive, and the full service brakes until the
/// tick starts parked; from then on the parking latch alone holds the tank, with its static
/// breakaway capacity, exactly as a released stick at rest does. The axes are released with the
/// brake held, so the latch engages once they have slewed to zero at standstill.
fn driver_intent(inp: &TransmissionInput, reverse: bool, parked: bool, m: f32) -> DriverIntent {
    let dir = if reverse { -1.0 } else { 1.0 };
    if inp.brake {
        return DriverIntent {
            dir,
            propulsive: 0.0,
            service: if parked { 0.0 } else { 1.0 },
        };
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
    let back_driven_intent = propulsive > 0.0 && dir * m < -PARK_ENGAGE_SPEED;
    let service = if back_driven_intent {
        propulsive
    } else {
        service
    };
    DriverIntent {
        dir,
        propulsive,
        service,
    }
}

/// Stage-C demand observer: the contact reactions are the load signal the sim already owns.
/// Project their sum onto the engaged ladder's signed m-axis and keep only propulsive demand;
/// downhill assistance is zero demand, not negative reserve. The first sample seeds directly,
/// then the EMA filters contact chatter. The update is deliberately ABSENT during a shift
/// window: the declutched cut changes slip/reactions and is not a change in the grade.
///
/// The time constants are ASYMMETRIC. A RISING sample keeps the established 8-tick rise (safety
/// — new load is believed fast, and the rising edge stays bit-identical to the old symmetric
/// filter), a FALLING sample is believed at the slow 32-tick scale: the shot-unloaded suspension
/// halves the raw sample for ~22–25 ticks and the symmetric filter followed it down far enough
/// to open the reserve gate falsely — see [`DEMAND_FALL_FILTER_TICKS`] for the measured bounds.
/// An equal sample moves nothing under either divisor.
fn update_demand_filter(
    st: &mut TransmissionState,
    inp: &TransmissionInput,
    dir: f32,
    #[cfg(feature = "bitprobe")] probe: &mut TransmissionProbe,
) {
    if st.shift_ticks != 0 {
        return;
    }
    let sample = (dir * (inp.reactions[0] + inp.reactions[1])).max(0.0);
    #[cfg(feature = "bitprobe")]
    {
        probe.demand_sample = sample;
        probe.demand_updated = true;
    }
    if st.demand_initialized {
        let filter_ticks = if sample > st.demand_n {
            DEMAND_FILTER_TICKS
        } else {
            DEMAND_FALL_FILTER_TICKS
        };
        st.demand_n += (sample - st.demand_n) / filter_ticks;
    } else {
        st.demand_n = sample;
        st.demand_initialized = true;
    }
}

/// Stage-C anti-rollback: maintain the hill-hold latch and its re-engagement cooldown, and
/// return whether this call committed a launch-gear shift step (so the ordinary scheduler does
/// not spend the same zero-length window twice).
///
/// Only held PROPULSIVE intent on the ENGAGED ladder can own the latch — EITHER ladder: every
/// quantity here is already ladder-signed (`shaft = dir·m`, the selector walks `ladder`, release
/// compares `dir·f_c`), and gating the latch on `!st.reverse` left the reverse climb WITHOUT the
/// documented near-rest fallback — a confirmed reverse deficit whose landing sign had crossed
/// could only lug/brake-cycle in its tall gear, since the deficit path's fallback owner
/// ("decelerate into the hill-hold seam") did not exist on the R ladder. Backing up a slope is a
/// climb; the hold, launch selection, and GRADE LIMIT truth are direction-agnostic. Release or
/// OPPOSING intent drops the latch immediately and lets the established direction-swap semantics
/// run.
///
/// Engagement is NEAR REST ONLY: the latch is the standstill seam of the climb, and |shaft|
/// below the existing near-rest threshold is its whole domain. The earlier form also engaged on
/// `real_rollback` at ANY speed, which mis-fired on fast cross-motion — held reverse throttle
/// while the hull still moves forward reads propulsive intent with `shaft = −5`, and the latch
/// grabbed a 5 m/s "rollback" (the symmetric forward quadrant too); worse, a force-based release
/// while cross-moving could re-latch the very next tick because the rollback arm overrode the
/// cooldown. Intent-vs-motion at speed belongs to the back-driven service braking (drive + full
/// service brakes braking CONTINUOUSLY to rest — the "jumpy S" seam family), after which the
/// decelerated hull enters the zone and latches legitimately. The cooldown is likewise NOT
/// overridable: a breach that accelerates past the threshold is being braked by that path
/// anyway, and re-latching mid-motion was exactly the release/re-latch chatter the cooldown
/// exists to prevent. While latched, selection runs on EVERY decision tick so a changing EMA can
/// retarget and GRADE LIMIT always describes current capability.
#[expect(
    clippy::too_many_arguments,
    reason = "the hill-hold law's inputs: state, construction, contact, supply, ladder, shaft, intent and reserve"
)]
fn update_hill_hold(
    st: &mut TransmissionState,
    tp: &TransmissionParams,
    fp: &ForceParams,
    supply: DriveSupply,
    ladder: &[f32],
    shaft: f32,
    propulsive: f32,
    current_reserve: f32,
) -> bool {
    let hold_cooldown_active = st.hold_reengage_ticks > 0;
    if hold_cooldown_active {
        st.hold_reengage_ticks -= 1;
    }
    let real_rollback = shaft < -HILL_HOLD_ENGAGE_SPEED;
    let climb_intent = propulsive > DEAD;
    let mut hill_hold_step_committed = false;
    if !climb_intent {
        st.hill_hold = false;
        if matches!(
            st.scheduler,
            SchedulerState::HillHold | SchedulerState::GradeLimit
        ) {
            st.scheduler = SchedulerState::Normal;
            st.grade_target = 0;
        }
    } else {
        let in_engagement_zone = shaft.abs() < HILL_HOLD_ENGAGE_SPEED;
        let effective_deficit = current_reserve < 0.0
            // During a paid interruption the selected gear's static capability is not being
            // transmitted: effective F = 0, so reserve is `-D`. This catches a cascade that
            // loses the climb inside an otherwise-capable landing gear — the 20° Sequential
            // approach loses it through ORDINARY band-down windows (scheduler Normal, no
            // retained target), so the arm cannot be scoped to a grade-scheduler owner.
            // A paid interruption masks transmitted force. Only demand above the vehicle's
            // model/contact-scale resolution counts as a grade load rather than residual contact
            // variation. This bound scales with the construction and its material pair.
            || (st.shift_ticks > 0 && st.demand_n > reserve_floor(fp, tp, supply));
        if !st.hill_hold && in_engagement_zone && !hold_cooldown_active && effective_deficit {
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
                refresh_hill_hold(st, tp, fp, supply, ladder, shaft, braked_rollback_rescue);
        }
    }
    hill_hold_step_committed
}

/// Update the L600 steering detent (hysteresis on |steer|), the ONE detent truth this tick's
/// gear decisions and this tick's steering solve both read.
///
/// This must run BEFORE the shift scheduler. The update used to live inside the
/// steering law BELOW the scheduler, so `detent_turn` read the PREVIOUS tick's step: seven
/// qualifying straight ticks followed by a detent engage on the committing tick still
/// incremented the confirmation to N and committed an ordinary upshift on the exact tick the
/// turn began — and the same invocation then applied the λ constraint in the NEW gear, where the
/// landing predictor is explicitly invalid. The update depends only on `steer` and the previous
/// step, and nothing between the old and new sites read it, so every non-engage tick is
/// unchanged.
fn update_steer_detent(st: &mut TransmissionState, steer: f32) {
    let a = steer.abs();
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
}

/// The per-tick quantities every gear-selection arm prices its decision against, derived ONCE
/// before the decision so all of them read one truth.
struct DecisionInputs {
    /// Ladder sign, `+1` forward / `−1` reverse.
    dir: f32,
    /// Mean belt speed (m/s).
    m: f32,
    /// SIGNED geared shaft speed `dir·m` (stage A) — rigid gearing has a sign.
    shaft: f32,
    /// Mean of this tick's two ground reactions and of their rolling-resistance shares (N), the
    /// landing predictor's input.
    r_mean: (f32, f32),
    dt: f32,
    /// Drive command WITH the engaged ladder, `0..1`.
    propulsive: f32,
    /// Modeled full-throttle reserve of the CURRENT gear at the current shaft speed.
    current_reserve: f32,
    /// Whether the fix-1a predictor's landing is forward on the engaged ladder.
    grade_landing_positive: bool,
    /// Whether the L600 detent is engaged — λ loads the outputs outside the predictor's domain.
    detent_turn: bool,
    /// Whether [`update_hill_hold`] already spent a shift window this tick.
    hill_hold_step_committed: bool,
    /// The game's damage model: the tick's drivetrain supply every reserve is priced against.
    supply: DriveSupply,
}

/// One decision tick of the gear scheduler: auto-shift on engine-rpm bands with hysteresis from
/// the band gap, plus the stage-C reserve corrections. Only called with no shift in flight — a
/// shift in flight blocks further decisions until its interruption window has elapsed.
///
/// Three SIM-POLICY gates (the fix-1 anti-hunting batch) kill the shift-cut oscillation the
/// static bands alone could not — the cut's own belt-speed bleed erased the ~100 rpm band margin
/// in low gears (measured full-throttle climb trace: 1-2-1-2-1-2-3-2-…):
///   a) upshifts are CONSIDERED only under propulsive drive AND (for the L600) with the steering
///      detent released — the predictor-domain gates — and must PREDICT a landing rpm ≥ down
///      band + [`POSTSHIFT_MARGIN_RPM`] at the END of the torque-cut window
///      ([`predict_shift_landing_m`] — the same integration the window itself runs);
///   b) a committed shift blocks the OPPOSITE-direction shift for [`REVERSAL_DWELL_TICKS`]
///      (same-direction climbs stay free);
///   c) downshifts must land under the engine's max curve rpm − [`OVERREV_MARGIN_RPM`].
///
/// Stage A (signed shaft): the shaft speed is defined RELATIVE TO THE ENGAGED LADDER,
/// `shaft = dir·m` — rigid gearing has a sign. Driving normally shaft > 0; back-driven (a grade
/// rolling the tank against the engaged gear) shaft < 0, and its geared rpm is NEGATIVE. The old
/// `|m|` read a backslide as high forward rpm, which walked the ladder upward mid-slide and (via
/// the landing predictor) blessed sign-flipped landings — see the module doc's stage-A paragraph
/// for the reproduced trio.
fn run_shift_decision(
    st: &mut TransmissionState,
    tp: &TransmissionParams,
    fp: &ForceParams,
    ladder: &[f32],
    inputs: &DecisionInputs,
) {
    let &DecisionInputs {
        dir,
        m,
        shaft,
        r_mean,
        dt,
        propulsive,
        current_reserve,
        grade_landing_positive,
        detent_turn,
        hill_hold_step_committed,
        supply,
    } = inputs;
    let top = ladder.len() as u8;
    let shaft_rpm_of = |sh: f32, g: f32| sh * g / tp.sprocket_radius / RPM_TO_RAD;
    let shaft_rpm_geared = |g: f32| shaft_rpm_of(shaft, g);

    // SIGNED shaft rpm (stage A): while back-driven this is negative, so the up band
    // can never fire mid-backslide (negative never exceeds the band).
    let rpm = shaft_rpm_geared(ladder[(st.gear - 1) as usize]);
    let (dwell_ticks, last_shift_dir) = (st.dwell_ticks, st.last_shift_dir);
    let dwell_blocks = |shift_dir: i8| dwell_ticks > 0 && last_shift_dir == -shift_dir;
    let mut grade_step_committed = hill_hold_step_committed;
    // PRIORITY RULE (module doc): while shaft AND crank both read past the
    // mechanical-protection ceiling, the crank rescue owns the decision tick — no
    // capability downshift may pre-empt it. Structurally the over-rev gate already
    // refuses every lower gear at such a shaft; this explicit gate turns that emergent
    // fact into a stated invariant on both grade-commit paths below.
    let ceiling = tp.max_curve_rpm();
    let crank_rescue_active = rpm > ceiling && st.omega_e / RPM_TO_RAD > ceiling;

    // A held Sequential target is never an instruction to shift blindly. Re-run the same
    // selector against current intent, speed, and filtered demand at every continuation. A
    // recovered current gear (`reserve >= 0`) or released propulsive command cancels the stale
    // cascade; a changed capability target retargets while retaining the original HUD `from`.
    if !grade_step_committed && st.grade_target > 0 {
        let selected = (propulsive > 0.0 && current_reserve < 0.0)
            .then(|| select_grade_target(tp, fp, supply, ladder, shaft, st.gear, st.demand_n))
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
            if !crank_rescue_active
                && shaft > -PARK_ENGAGE_SPEED
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
    // Raw confirmed deficit AND its correction target, computed INDEPENDENT of the
    // dwell: the deferral below must know whether a correction EXISTS before the
    // ordinary arms run. The first cut gated `confirmed_deficit` on the dwell itself,
    // which let control fall through to the ORDINARY upshift arm while deferring — a
    // genuine deficit could be upshifted OVER, and that upshift reset
    // `grade_confirm_ticks` and restarted the dwell: evidence erased, correction never
    // fires.
    let confirmed_deficit = !grade_step_committed
        && propulsive > 0.0
        && st.gear > 1
        && shaft > -PARK_ENGAGE_SPEED
        && current_reserve < 0.0
        && st.grade_confirm_ticks >= GRADE_CONFIRM_TICKS;
    let deficit_target = confirmed_deficit
        .then(|| select_grade_target(tp, fp, supply, ladder, shaft, st.gear, st.demand_n))
        .flatten();

    // A confirmed reserve deficit is a CORRECTION, not a preference: with a target it
    // owns the decision ahead of both upshift arms — by committing, or by HOLDING
    // (`deficit_pending` suppresses the ordinary upshift below). It falls through to
    // the ordinary arms ONLY when selection genuinely returns None (at governor-pinned
    // cruise the equilibrium reserve is zero by definition and dithers negative, the
    // leaky confirmation latches, and no legal lower gear exists — every candidate
    // over-revs — so the old unconditional pre-emption silently swallowed the upshift
    // arm forever).
    //
    // A SHALLOW correction WAITS OUT the post-UPSHIFT reversal dwell. The demand EMA's
    // reaction sample carries the rebuilding drive shear after a shift window, not just
    // the grade: measured on the 10% climb, demand inflated 74 → 93 kN across the F5→F6
    // window and re-acceleration, manufacturing a "confirmed" deficit of −2..−3 kN
    // against a 19 kN margin exactly at the capability boundary — a sustained F5↔F6
    // limit cycle (~2 s period; the reverse ladder's field-reported R1→R2→R3→R1 is the
    // same attractor). Evidence still ACCUMULATES through the dwell (leaky, saturating),
    // so a real-but-shallow grade deficit corrects at dwell expiry — at most 0.5 s late
    // — while a transient the box itself created decays with the shear. The
    // post-DOWNSHIFT dwell never blocks it (`dwell_blocks` is direction-aware): a deeper
    // deficit is a same-direction correction and downshifts again freely.
    //
    // DEEP-deficit override (near-arrest safety): a deficit deeper than the
    // reserve-margin scale is not boundary dither — the pollution transient measured
    // −2..−3 kN against a 19 kN margin, while a genuine steep grade reads tens of kN
    // past it — so it corrects IMMEDIATELY, dwell or not. This is the
    // earlier-intervention half of the near-arrest policy: a steep grade entered right
    // after an upshift must not bleed speed through window + dwell until the predicted
    // landing goes sign-negative and the correction becomes uncommittable (the lug).
    // The landing-sign gate itself stays intact: in the extreme where the sign has
    // ALREADY crossed, the correction holds (`deficit_pending`) and the vehicle
    // decelerates into the near-rest hill-hold path, whose latch + full brake envelope
    // and launch-gear selection are the designed owners of an arrested climb — that
    // fallback is deliberate, not accidental.
    let deep_deficit = -current_reserve > reserve_margin(fp, tp, supply, st.demand_n);
    let deficit_deferred = deficit_target.is_some() && dwell_blocks(-1) && !deep_deficit;
    let mut deficit_step_committed = false;
    if let Some(target) = deficit_target
        && !deficit_deferred
        && grade_landing_positive
        // Priority rule: an active crank rescue owns the tick (see the block top).
        && !crank_rescue_active
    {
        commit_grade_shift(st, tp, target);
        deficit_step_committed = true;
    }
    // Pending = a correction target exists but did not commit this tick (dwell-deferred
    // or landing-sign-blocked): HOLD — the ordinary upshift arm is suppressed so the
    // pending correction can neither be upshifted over nor have its evidence reset.
    let deficit_pending = deficit_target.is_some() && !deficit_step_committed;

    // Crank-corroboration floor terms, hoisted here because the full-predicate
    // confirmation below counts them; the corroboration RATIONALE lives on the
    // ordinary arm's comment block further down.
    let corroboration_floor = tp.engine.governed_rpm + CRANK_CORROBORATION_MARGIN_RPM;
    let crank_rpm = st.omega_e / RPM_TO_RAD;
    let shaft_past_floor = rpm > corroboration_floor;
    let crank_past_floor = crank_rpm > corroboration_floor;

    // Upshift confirmation: the counter counts the FULL ordinary-upshift predicate —
    // every condition the commit itself needs — and HARD-resets on any tick where it
    // fails. The earlier band-only counter was blind to the gate a recoil actually
    // flips: on a lugging climb the band sub-condition is permanently true (the counter
    // saturated at 255) while the fix-1a landing gate stayed instantaneous, and a
    // rearward shot lifts the predicted landing past it for ~4 measured ticks — enough
    // to commit the F1→2→3→1 shift storm. See [`UPSHIFT_CONFIRM_TICKS`] for the probe
    // evidence and the N = 8 sizing.
    //
    // The committed-this-tick flags short-circuit FIRST so the landing/reserve gates
    // are never priced against a gear another arm already changed this tick (a commit
    // also zeroes the counter itself, in `commit_grade_shift` and both commit sites
    // below). The reversal dwell is deliberately NOT part of the counted predicate:
    // it is itself a timer, and stacking the confirmation onto it would double-charge
    // a legitimate post-downshift upshift — evidence accumulates through the dwell
    // exactly like the deficit's does.
    let upshift_ready = !grade_step_committed
        && !deficit_step_committed
        && propulsive > 0.0
        && st.gear < top
        && rpm > tp.shift_up_rpm
        && !detent_turn
        && (!shaft_past_floor || crank_past_floor)
        && !deficit_pending
        && {
            // The fix-1a landing gate and the stage-C reserve gate, verbatim from the
            // old commit arm (stage A: the predictor returns a SIGNED m, and the
            // landing must be POSITIVE on the engaged ladder AND clear the down band
            // + margin — a sign-flipped landing always refuses; under `|m|` the traced
            // grade case, r_mean = 221 kN, landing_m = −3.62, read as "9092 rpm" and
            // PASSED, committing catastrophic on-grade upshifts. No at-rest threshold
            // is needed here: the band bound already demands a landing ≥ down band +
            // margin, solidly positive, far above any numerical residual.)
            let landing = predict_shift_landing_m(tp, fp, m, r_mean, dt);
            let landing_shaft = dir * landing;
            let g_up = ladder[st.gear as usize];
            let next_reserve = modeled_reserve_in_gear(tp, supply, shaft, g_up, st.demand_n);
            landing_shaft > 0.0
                && shaft_rpm_of(landing_shaft, g_up) >= tp.shift_down_rpm + POSTSHIFT_MARGIN_RPM
                && next_reserve >= reserve_margin(fp, tp, supply, st.demand_n)
        };
    if upshift_ready {
        st.band_confirm_ticks = st.band_confirm_ticks.saturating_add(1);
    } else {
        st.band_confirm_ticks = 0;
    }

    if !deficit_step_committed {
        // The ordinary intent gate remains: service braking never BAND-upshifts (the
        // landing predictor has no brake term) and the L600 detent defers BAND upshifts
        // (λ loads the outputs outside the predictor's domain). On OVERRUN the box
        // HOLDS its gear: with no propulsive intent the ordinary arm never arms, and
        // the PROTECTIVE upshift is a LAST RESORT at the mechanical-protection ceiling
        // — the engine's max authored curve rpm (`max_curve_rpm`, the Tiger's rated
        // 3000; the same ceiling the fix-1c over-rev gate measures downshift landings
        // against). The old floor of governed + margin shed exactly the engine braking
        // a descent exists to use; between governed and the ceiling the climbing dial
        // is the warning, not a shift trigger. When it does fire, the protective shift
        // is free of the intent gate, the detent deferral, the landing-rpm band, and
        // the reserve gate (all folded into `upshift_ready` above): its purpose is to
        // LOWER an externally back-driven crank, not to accelerate the vehicle, and
        // descending under service brakes or mid-steer is the overrun's NORMAL case. It
        // does not read the propulsive landing predictor at all: its selection is
        // BEST-EFFORT static projection with the over-rev slip guard as the actual
        // safety bound (see the module doc and the branch below). Reversal dwell still
        // applies to it.
        //
        // BOTH speeds must read past the ceiling because the rescue protects the
        // CRANK: a belt-side transient (contact/settle impulse) whose re-anchor is
        // clutch-infeasible never carries the crank with it, so there is nothing to
        // protect and it must not fire (measured on the 20° approach fixture: a
        // few-tick belt spike past the old floor — with the reserve gate waived —
        // upshifted F6→F7 mid-climb and cost the crest). A genuine overrun locks the
        // crank to the geared shaft within ticks, so the real rescue is delayed only
        // until the crank is actually in danger.
        //
        // The SAME corroboration binds the ORDINARY arm above the UNCHANGED
        // corroboration floor, governed + CRANK_CORROBORATION_MARGIN_RPM (invariants
        // preserved through the descent inversion): a clutch-infeasible belt transient
        // must not drive gear selection through EITHER arm — a shaft reading past
        // governed + margin with the crank nowhere near it is by definition a spike the
        // engaged coupling did not carry (locked driving keeps them equal; propulsive
        // clutch slip puts the crank ABOVE the shaft, never below), and its
        // band/landing/reserve arithmetic is evaluated at a fictitious operating point.
        // Below the floor the ordinary gates stand unchanged: every in-band reading
        // there is a physical operating point the reserve and landing gates already
        // price. (The floor terms themselves are computed above the confirmation
        // counter, which counts them.)
        let protective_upshift = crank_rescue_active;
        // The ordinary arm IS the confirmed predicate: `upshift_ready` carries every
        // commit condition — intent, band, detent, corroboration, `!deficit_pending` (a
        // pending correction HOLDS the gear — the ordinary arm must not upshift over
        // it, or the commit would reset the very confirmation evidence and dwell
        // deferring the correction), landing, and reserve — and the counter requires it
        // to have held N consecutive decision ticks. The PROTECTIVE arm stays exempt
        // from all of it: crank protection outranks a capability correction and must
        // not wait.
        let ordinary_upshift = upshift_ready && st.band_confirm_ticks >= UPSHIFT_CONFIRM_TICKS;
        if !grade_step_committed
            && st.gear < top
            && (ordinary_upshift || protective_upshift)
            && !dwell_blocks(1)
        {
            let next_gear = if protective_upshift {
                // BEST-EFFORT rescue selection (see the module doc's safety-mechanism
                // paragraph): pick the first higher gear whose STATICALLY-projected
                // shaft rpm — the current shaft through the candidate ratio, no window
                // pricing — sits at or below the ceiling. Direct skips straight there;
                // Sequential pays one adjacent step per window toward it (gear index
                // strictly rises, the ladder is finite: termination is trivial).
                // Safety does NOT ride on this choice: the over-rev slip guard
                // bounds the crank unconditionally, so a landing the hill overtakes
                // costs one guard-clamped window and a re-decision. Three rounds of
                // worst-case landing pricing (decayed predictor, sustained-reaction
                // bound, λ allowance) each sprouted a new defect while the guard
                // made their success optional — deliberately deleted. If no gear
                // statically clears (an extreme overspeed near the top of the
                // ladder), hold: a commit would only shed reflected engine braking
                // while the guard already owns the crank. The landing-rpm band AND
                // the reserve gate remain waived, same rationale as ever: the shift
                // lowers the crank, it does not accelerate, and above the fuel cut
                // every gear's modeled force is zero. The static projection is ≥
                // positive by the trigger (shaft past the ceiling), so no
                // landing-sign test applies here.
                ((st.gear + 1)..=top)
                    .find(|&cand| shaft_rpm_of(shaft, ladder[(cand - 1) as usize]) <= ceiling)
                    .map(|target| match tp.shift_addressing {
                        ShiftAddressing::Direct => target,
                        ShiftAddressing::Sequential => st.gear + 1,
                    })
            } else {
                // Ordinary band arm: `ordinary_upshift` already proved the landing and
                // reserve gates THIS tick — they live inside `upshift_ready`, the very
                // predicate the confirmation counted — so the adjacent upshift commits
                // unconditionally here.
                Some(st.gear + 1)
            };
            if let Some(next_gear) = next_gear {
                st.gear = next_gear;
                st.shift_ticks = tp.shift_ticks;
                st.last_shift_dir = 1;
                st.dwell_ticks = REVERSAL_DWELL_TICKS;
                st.grade_confirm_ticks = 0;
                st.band_confirm_ticks = 0;
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
            // Backslide hold (stage A): while GENUINELY back-driven the vehicle is NOT
            // "running slow forward" — gear changes are decisions about forward operation, and
            // a FREE backslide HOLDS the engaged gear (the negative signed rpm would otherwise
            // downshift-walk forever). The one narrow exception is handled above: when a
            // latched hill hold's current gear plus declared brakes cannot arrest the slide,
            // `refresh_hill_hold` may downshift to gain that capability while the shaft is
            // negative. The threshold here is −PARK_ENGAGE_SPEED, the existing at-rest policy
            // scale, NOT exact zero: the brake stop-force/integration order leaves a stable
            // numerical residual at rest (measured ≈ −1.7e−9 m/s coasting to a stop in gear 3
            // against a 20 kN reaction), and a hard `shaft >= 0` stranded the box in its cruise
            // gear forever. A residual orders of magnitude below the threshold downshifts
            // normally; a real slide (−0.5 m/s and beyond) still holds.
            let g_down = ladder[(st.gear - 2) as usize];
            if shaft_rpm_geared(g_down) <= tp.max_curve_rpm() - OVERREV_MARGIN_RPM {
                st.gear -= 1;
                st.shift_ticks = tp.shift_ticks;
                st.last_shift_dir = -1;
                st.dwell_ticks = REVERSAL_DWELL_TICKS;
                st.grade_confirm_ticks = 0;
                st.band_confirm_ticks = 0;
                st.grade_target = 0;
                st.scheduler = SchedulerState::Normal;
            }
        }
    }
    if st.hill_hold && st.scheduler != SchedulerState::GradeLimit {
        st.scheduler = SchedulerState::HillHold;
    }
}

/// The parking latch (driver intent — the Tiger manual's "am Hang" lever): a zero command near
/// standstill sets the lever; any THROTTLE intent releases it instantly. STEER intent releases
/// it too, by DESIGN CHOICE: a neutral pivot at standstill must move the belts, so steer alone
/// releases the latch for the pivot arm, and the same rule re-latches automatically once the
/// stick returns to zero at standstill — no separate re-arm input. Held, it runs the
/// full-capacity stop-force path (static breakaway at rest), so it holds any grade inside the
/// declared brake capacity. State, not a blend — see [`PARK_ENGAGE_SPEED`]; the `park` bool is
/// already REV-14 replicated state (projection/comparator/trace), deterministic from replicated
/// inputs, so rollback/prediction see one latch and there is no wire change.
fn update_park_latch(st: &mut TransmissionState, inp: &TransmissionInput) {
    // The game's damage model: control authority alone permits moving the existing lever.
    if !inp.supply.control_authority {
        return;
    }
    if inp.throttle.abs() >= DEAD || inp.steer.abs() >= DEAD {
        st.park = false;
    } else if inp.speeds[0].abs().max(inp.speeds[1].abs()) < PARK_ENGAGE_SPEED {
        st.park = true;
    }
}

/// THE COUPLING SEAM latch: engaged ⇔ not shifting ∧ not the neutral-idle regime. The
/// neutral-idle regime generalizes the L600 neutral-turn seam from steering to the whole drive —
/// no propulsive drive near standstill means the driver has the main clutch out (an engaged
/// idle-governed crank at standstill would otherwise ride the clutch: idle torque through a
/// first-gear reduction is hundreds of kN of spurious creep/pivot-drag force). Keyed on
/// `propulsive` (not |throttle|) so a service-brake command at speed stays engaged (engine
/// braking through the coupling); the L600's own steering-regime check keeps its historical
/// |throttle| form — the seams coincide except transiently under opposing-throttle-at-standstill,
/// where the direction swap + shift window take over within a tick.
///
/// The seam is a LATCH with hysteresis (`st.clutch_out`, the steering-detent doctrine), not a
/// single threshold — a boundary creeper chattered engage/declutch on the bare
/// [`NEUTRAL_M_SPEED`] line. Any propulsive command re-engages at any speed (the launch);
/// otherwise the belt must fall below [`CLUTCH_OUT_M_SPEED`] to take the clutch out and climb
/// past [`CLUTCH_IN_M_SPEED`] to put it back in.
fn update_clutch_latch(st: &mut TransmissionState, propulsive: f32, m: f32, supply: DriveSupply) {
    if supply.control_authority && supply.task_rate == 0.0 {
        st.clutch_out = true;
    } else if propulsive >= NEUTRAL_THROTTLE || m.abs() >= CLUTCH_IN_M_SPEED {
        st.clutch_out = false;
    } else if m.abs() < CLUTCH_OUT_M_SPEED {
        st.clutch_out = true;
    }
}

/// The crank's free torque `τ_free = τ_ind + τ_idle − τ_drag` from the PRE-tick crank speed, and
/// the engine power available at that operating point.
///
/// Fueling command u. Engaged: the unscaled propulsive throttle (a brake command is not
/// fueling). The game's damage model scales gross combustion while retaining construction drag.
/// Declutched: a proportional-band rev governor ([`REV_MATCH_BAND_RPM`]) toward the LARGER of two
/// targets —
///   * the REV-MATCH target `|m|·k` (`st.gear` is already the landing gear during the window), so
///     the clutch re-engages near-synchronous;
///   * the STEER demand target `idle + (peak_torque_rpm − idle)·|steer|`: the steering member is
///     engine-driven in every regime, so a steer command revs the crank whether or not the main
///     clutch is out — the surviving half of the old `cmd_mag` rev-floor contract, now reached
///     DYNAMICALLY (pivot power spools with the crank).
///
/// The induced torque uses the crank's own rpm. The game's damage model scales combustion
/// and the idle governor with their sources; construction motoring drag stays unscaled.
/// `engine_work` charges the physical sprocket work against combustion and stored crank
/// energy, so steering loads the crank even while the main clutch is out.
#[expect(
    clippy::too_many_arguments,
    reason = "the free-torque law's inputs: params, input, engagement, throttle, belt speed, ratio and both crank speeds"
)]
fn crank_free_torque(
    tp: &TransmissionParams,
    inp: &TransmissionInput,
    engaged: bool,
    propulsive: f32,
    m: f32,
    k: f32,
    omega_e: f32,
    omega_idle: f32,
    #[cfg(feature = "bitprobe")] probe: &mut TransmissionProbe,
) -> (f32, f32) {
    let u_fuel = if engaged {
        propulsive
    } else {
        let omega_match = m.abs() * k;
        let omega_steer =
            omega_idle + (tp.peak_torque_rpm * RPM_TO_RAD - omega_idle) * inp.steer.abs().min(1.0);
        let omega_target = omega_match.max(omega_steer);
        ((omega_target - omega_e) / (REV_MATCH_BAND_RPM * RPM_TO_RAD)).clamp(0.0, 1.0)
    };

    let supply = inp.supply;
    let rpm = omega_e / RPM_TO_RAD;
    let [tau_ind, tau_idle, tau_drag] = engine_torques(tp, supply, omega_e, u_fuel);
    let tau_free = tau_ind + tau_idle - tau_drag;

    let p_avail = supply.engine_condition * tp.torque_at(rpm) * omega_e;
    #[cfg(feature = "bitprobe")]
    {
        probe.u_fuel = u_fuel * supply.task_rate;
        probe.rpm = rpm;
        probe.tau_idle = tau_idle;
        probe.tau_induced = tau_ind;
        probe.tau_drag = tau_drag;
        probe.tau_free = tau_free;
        probe.power_available = p_avail;
    }
    (tau_free, p_avail)
}

/// The steering member's contribution to this tick: the difference-axis force `F_s`, the L600
/// constraint force λ, and λ's per-output Jacobian `j`. Returned as `(f_s, lambda, j)`; a tick
/// that commands neither leaves all three at zero.
///
/// κ is table-indexed by the ACTIVE gear (reverse mirrors the low forward gears); `d` follows the
/// steer SIGN regardless of travel direction — the superimposed steering shaft is independent of
/// the gear's direction, historically and mechanically.
///
/// The servo is a capacity-limited KINEMATIC one, semi-implicit like the brakes and λ: the `F_s`
/// that lands `d` exactly on target after this tick's integration (`d` dynamics:
/// `d_next = d + (F_s/2 − R_d)/I·dt`), reaction-compensated, clamped to the per-output
/// convention's bound. Exact inside capacity, honest slip beyond it — and no proportional band:
/// the old P law's steady-state droop let the ground reaction eat the command (the Tiger's whole
/// neutral target, 0.21 m/s, sat INSIDE the 0.25 m/s band, so a sustained pivot ran at ≤ half
/// capacity and crawled at 0.03 rad/s — a vehicle-scaling defect the T-34's 0.46 m/s target
/// masked). The difference-axis bound is 2× the per-output capacity: `F_s` splits `±F_s/2` onto
/// the outputs, and each output's share is what the per-output datum caps (see
/// [`TransmissionParams::steer_capacity_n`] — the pivot-dead convention fix).
fn steering_force(
    tp: &TransmissionParams,
    fp: &ForceParams,
    st: &TransmissionState,
    inp: &TransmissionInput,
    m: f32,
    d: f32,
    f_c: f32,
) -> (f32, f32, [f32; 2]) {
    let dt = inp.dt;
    let [vl, vr] = inp.speeds;
    let kappa_idx = ((st.gear - 1) as usize).min(tp.steer_kappa.len() - 1);
    let (k_tight, k_wide) = tp.steer_kappa[kappa_idx];
    let mut f_s = 0.0;
    let mut lambda = 0.0;
    let mut j = [0.0f32; 2];
    let steer_capacity = inp.supply.transmission_condition * tp.steer_capacity_n;
    let f_s_max = 2.0 * steer_capacity;
    let r_d = (inp.reactions[0] - inp.reactions[1]) / 2.0;
    let servo =
        |target_d: f32| (2.0 * ((target_d - d) * fp.inertia / dt + r_d)).clamp(-f_s_max, f_s_max);
    // The steering detent (`st.steer_step`) was already updated for THIS tick
    // above the shift scheduler — one detent truth per tick; this law only
    // consumes it.
    let neutral = inp.throttle.abs() < NEUTRAL_THROTTLE && m.abs() < NEUTRAL_M_SPEED;
    if neutral {
        // The marginal brake-gated neutral turn: a slow capacity-limited servo
        // toward the DERIVED pivot scale `neutral_d_full = κ_tight(F1) ×
        // v1_governed` — the radii table's own gear-independent invariant (the
        // unprovenanced `neutral_fraction` that used to shrink it was deleted).
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
            let lambda_max = steer_capacity / jl.abs().max(jr.abs()).max(1e-3);
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
            // steering-sign reversal. Re-solve on the branch the belt actually
            // lands on; if the branches disagree about the landing side (the
            // genuine cusp), the constraint takes the tick off — λ = 0 is stable
            // and passive there.
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
    (f_s, lambda, j)
}

/// Signed sprocket work after the declared recirculation loss. The split is physical:
/// one sprocket can absorb work while both modal axes report positive work.
fn recirculated_work(tp: &TransmissionParams, sides: [f32; 2]) -> f32 {
    let positive = sides[0].max(0.0) + sides[1].max(0.0);
    let negative = (-sides[0]).max(0.0) + (-sides[1]).max(0.0);
    if positive == 0.0 {
        return -negative;
    }
    // Only work transferred to the positive output recirculates. Pure engine braking
    // returns work through the main clutch without an inner-to-outer transfer loss.
    positive - negative + (1.0 - tp.recirculation) * negative.min(positive / tp.recirculation)
}

/// The engine's work allocation for one tick: a common force scale and the remaining crank
/// energy (J). The game's damage model has one source budget: combustion's free-crank step
/// plus stored crank inertia. Positive/recirculated work uses each sprocket's midpoint speed,
/// so acceleration from rest costs energy on the tick it happens.
struct EngineWork {
    scale: f32,
    crank_energy: f32,
}

fn engine_work(
    tp: &TransmissionParams,
    fp: &ForceParams,
    inp: &TransmissionInput,
    crank: CrankTick,
    motor: [f32; 2],
    #[cfg(feature = "bitprobe")] probe: &mut TransmissionProbe,
) -> EngineWork {
    let omega_free = (crank.omega_e + crank.tau_free * crank.dt / tp.engine_inertia)
        .clamp(0.0, crank.omega_over);
    // DERIVED: the unclamped constant-torque work is tau_free * dt times mean crank
    // speed, including construction losses. Guard clipping discards excess work.
    let energy = 0.5 * tp.engine_inertia * omega_free * omega_free;
    let spendable = (energy - 0.5 * tp.engine_inertia * crank.omega_floor.powi(2)).max(0.0);
    let sides_at = |scale: f32| {
        [0, 1].map(|i| {
            let force = motor[i] * scale;
            let next = inp.speeds[i] + (force - inp.reactions[i]) * inp.dt / fp.inertia;
            force * (inp.speeds[i] + next) * 0.5 * inp.dt
        })
    };
    let work = |scale| recirculated_work(tp, sides_at(scale));
    let scale = if work(1.0) <= spendable {
        1.0
    } else {
        let (mut lo, mut hi) = (0.0, 1.0);
        // Resolution: 24 bisections resolve the unit force scale to f32's mantissa. Positive
        // work is convex in scale; zero is feasible even with an empty engine and stopped crank.
        for _ in 0..24 {
            let mid = (lo + hi) * 0.5;
            if work(mid) <= spendable {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    };
    #[cfg(feature = "bitprobe")]
    {
        let [left, right] = sides_at(scale).map(|joules| joules / inp.dt);
        probe.power_left = left;
        probe.power_right = right;
        probe.power_positive = left.max(0.0) + right.max(0.0);
        probe.power_negative = (-left).max(0.0) + (-right).max(0.0);
        probe.power_net = work(scale) / inp.dt;
        probe.power_scale = scale;
    }
    EngineWork {
        scale,
        crank_energy: (energy - work(scale)).max(0.0),
    }
}

/// Hill-hold release: capability-based, not timer-based — only force actually transmitted through
/// the coupling (after the power gate) may hand the slope from the modeled brakes back to the
/// drivetrain. The release threshold is
///
///   D + min(selection_margin, max(0, modeled_selected_reserve) * 0.5).
///
/// Thus a full-margin gear keeps the ordinary headroom, while a margin-short but capable gear can
/// release once it transmits its own modeled force: half its non-negative reserve is always below
/// that force. Equality is accepted for the zero-reserve knife edge. The hold remains through
/// every declutched window because `f_c = 0` there.
#[expect(
    clippy::too_many_arguments,
    reason = "the release law's inputs: state, construction, contact, supply, shaft, ratio, sign and force"
)]
fn release_hill_hold(
    st: &mut TransmissionState,
    tp: &TransmissionParams,
    fp: &ForceParams,
    supply: DriveSupply,
    shaft: f32,
    g: f32,
    dir: f32,
    f_c: f32,
) {
    // Nothing to release, and the threshold below is a pure function of state — so on the common
    // path (no hold latched) neither term is worth computing.
    if !st.hill_hold {
        return;
    }
    let selected_reserve = modeled_reserve_in_gear(tp, supply, shaft, g, st.demand_n);
    let release_margin =
        reserve_margin(fp, tp, supply, st.demand_n).min(selected_reserve.max(0.0) * 0.5);
    if dir * f_c >= st.demand_n + release_margin {
        st.hill_hold = false;
        st.hold_reengage_ticks = HOLD_REENGAGE_TICKS;
        st.scheduler = SchedulerState::Normal;
        st.grade_target = 0;
    }
}

/// The reframed brake (design §3) applied to the assembled per-side sprocket forces: −R always
/// reaches the belt; near zero command + zero belt speed the parking/service brake statically
/// balances what the drivetrain doesn't, inside its capacity. `h` reuses the hold-blend
/// SHAPE ([`forces::hold_blend`]) purely as the engagement envelope (tick-stable, exact at rest); a rig with no grip
/// (support-only rigs, e.g. calibration) keeps the brake disengaged, like the hold it extends.
fn apply_brake_stop_forces(
    q: &mut [f32; 2],
    tp: &TransmissionParams,
    fp: &ForceParams,
    st: &TransmissionState,
    inp: &TransmissionInput,
    service: f32,
    hill_brake_active: bool,
) {
    let dt = inp.dt;
    for (i, qi) in q.iter_mut().enumerate() {
        // Engagement envelope: the parking LATCH holds full capacity (post-breach it keeps
        // rubbing at B_max instead of fading with speed); unlatched, the smooth entry blend h
        // (zero command + near-zero belt speed) eases the brake in during settle; the service
        // pedal is the driver-intent brake command. The paths overlap only on the tick the latch
        // engages, where both are full: service comes from a drive command (which releases the
        // latch) or a brake hold that started the tick unlatched. A support-only rig (no grip)
        // keeps the brake disengaged.
        let h = if fp.grip.is_some() {
            if st.park {
                1.0
            } else if inp.supply.control_authority {
                let target = inp.side_commands[i] * tp.governed_speed(st.reverse);
                forces::hold_blend(target.abs() / fp.slip_saturation)
                    * forces::hold_blend(inp.speeds[i].abs() / fp.slip_saturation)
            } else {
                // The game's damage model: an unmanned station cannot ease on the brakes.
                0.0
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
            // drag — a passivity defect), saturates at ±cap against a slide, and can
            // neither speed the belt up nor push it through zero.
            let stop = inp.reactions[i] - *qi - inp.speeds[i] * fp.inertia / dt;
            *qi += stop.clamp(-cap, cap);
        }
    }
}

/// Finish the crank: re-anchor it to the belt that ACTUALLY integrated when the snap is
/// feasible, then clamp it into the guard band unconditionally.
///
/// DRIFT KILL / RE-ANCHOR (stage B): snap the crank to the integrated belt only if the implied
/// TOTAL clutch torque `τ_impl = τ_free − (k·s·m_next − ω_e)·J/dt` fits the capacity. The
/// coupling pre-solve's `F_other` (reactions only) is a PREDICTOR approximation: brakes, the
/// FixedRadii λ mean-axis share, the mean-axis speed limit, and (only on numerical runaway) the
/// per-belt safety ceiling all move `m_next` after it, and exact pre-accounting is circular (the
/// brake law reads the q that needs F_c) — so feasibility is decided HERE, on the final `m_next`,
/// not on the pre-solve's stale clamp flag. An eager flag let a full-opposing-throttle brake tick
/// snap the crank down the belt's brake-driven drop, implying ≈ 9.7 kN·m through a 2.4 kN·m
/// clutch (the traced teleport); an infeasible snap now leaves the honestly integrated crank —
/// the clutch is slipping, and that is the truth. Inside capacity the snap is a legitimate clutch
/// outcome regardless of the power gate (any within-capacity landing is reachable), so no
/// `power_scale` condition. The snap respects BOTH guard points: it may never land the crank
/// below the stall floor NOR above the over-rev guard — past either, the clutch is slipping by
/// policy and the honestly integrated crank stands.
///
/// The game's damage model: `omega_floor` is the reachable idle guard only while combustion
/// sustains it, otherwise rest. The caller also bounds the final crank by the energy left
/// after sprocket work; a feasible clutch snap cannot create energy. `omega_over` bounds
/// over-rev while the clutch slips.
///
/// `engaged` carries the engaged clutch's supplied capacity (N·m); `None` while declutched.
/// For an engine that cannot idle `ω_floor` is rest ([`crank_guards`]).
fn settle_crank(
    st: &mut TransmissionState,
    tp: &TransmissionParams,
    engaged: Option<f32>,
    next: [f32; 2],
    crank: CrankTick,
    #[cfg(feature = "bitprobe")] probe: &mut TransmissionProbe,
) {
    let CrankTick {
        omega_e,
        tau_free,
        k,
        dir,
        omega_floor,
        omega_over,
        dt,
    } = crank;
    if let Some(capacity) = engaged {
        let m_next = (next[0] + next[1]) / 2.0;
        let locked = k * dir * m_next;
        let tau_impl = tau_free - (locked - omega_e) * tp.engine_inertia / dt;
        #[cfg(feature = "bitprobe")]
        {
            let feasible =
                tau_impl.abs() <= capacity && locked >= omega_floor && locked <= omega_over;
            probe.reanchor_attempted = true;
            probe.reanchor_locked = locked;
            probe.reanchor_tau_impl = tau_impl;
            probe.reanchor_feasible = feasible;
        }
        if tau_impl.abs() <= capacity && locked >= omega_floor && locked <= omega_over {
            st.omega_e = locked;
        }
    }

    st.omega_e = st.omega_e.max(omega_floor).min(omega_over);
    #[cfg(feature = "bitprobe")]
    {
        probe.omega_end = st.omega_e;
    }
}

/// The crank's two guard points this tick (rad/s): the stall floor and the over-rev bound.
///
/// The game's damage model: the stall guard protects an engine that can idle, whose saturated
/// idle governor exceeds its motoring drag at the guard point. An engine short of that (no
/// combustion, or too little) has nothing to guard, so its floor is rest: the crank runs down
/// under drag or follows the belt through the clutch, and the guard never pulls the belt to
/// hold a crank speed the engine cannot.
///
/// The over-rev bound (see [`clutch_coupling`]) sits one OVERREV_MARGIN_RPM above the rescue
/// trigger, so the protective upshift's crank corroboration can still read past the ceiling
/// before the clutch slip saturates.
fn crank_guards(
    tp: &TransmissionParams,
    supply: DriveSupply,
    omega: f32,
    tau_free: f32,
    dt: f32,
) -> (f32, f32) {
    let guard_point = (tp.engine.idle_rpm - STALL_GUARD_BAND_RPM) * RPM_TO_RAD;
    let can_idle =
        supply.engine_condition * tp.torque_at(tp.engine.idle_rpm) > engine_drag(tp, guard_point);
    let omega_floor = if can_idle { guard_point } else { 0.0 };
    let omega_over = (tp.max_curve_rpm() + OVERREV_MARGIN_RPM) * RPM_TO_RAD;
    // The game's damage model: recovery from rest cannot jump to the idle band for free.
    let reachable = (omega + tau_free * dt / tp.engine_inertia).max(0.0);
    (omega_floor.min(reachable), omega_over)
}

/// The input half of the optional drivetrain trace, kept out of the physical solve.
#[cfg(feature = "bitprobe")]
fn input_probe(
    st: &TransmissionState,
    inp: &TransmissionInput,
    m: f32,
    d: f32,
) -> TransmissionProbe {
    TransmissionProbe {
        throttle: inp.throttle,
        steer: inp.steer,
        side_commands: inp.side_commands,
        speeds: inp.speeds,
        reactions: inp.reactions,
        dt: inp.dt,
        mean_speed: m,
        difference_speed: d,
        perceived_load_n: inp.perceived_load.total_n(),
        grip_limit_n: inp.perceived_load.grip_n,
        demand_pre: st.demand_n,
        omega_pre: st.omega_e,
        ..Default::default()
    }
}

/// The output half of the optional force trace. The operating point is signed shaft speed,
/// reduction and delivered mean-axis force; recording it cannot mutate the physical solve.
#[cfg(feature = "bitprobe")]
fn force_probe(
    tp: &TransmissionParams,
    inp: &TransmissionInput,
    (shaft, reduction, delivered): (f32, f32, f32),
    probe: &mut TransmissionProbe,
) {
    probe.priced_force_n = price_driver_force(
        tp,
        inp.supply,
        shaft,
        reduction,
        inp.throttle.abs(),
        inp.perceived_load.grip_n,
    );
    probe.delivered_force_n = delivered.min(inp.perceived_load.grip_n);
}

/// Advance the interruption window and the dwell on their separate clocks. The returned flag
/// names this tick's interruption, before consuming its last tick.
fn advance_shift_window(st: &mut TransmissionState) -> bool {
    // The dwell counts only OUTSIDE the interruption window: the frozen window blocks all
    // decisions anyway, so draining the dwell inside it left only ~12 effective post-engagement
    // ticks of the promised 32.
    if st.shift_ticks == 0 && st.dwell_ticks > 0 {
        st.dwell_ticks -= 1;
    }
    let shifting = st.shift_ticks > 0;
    if shifting {
        st.shift_ticks -= 1;
    }
    shifting
}

/// Advance the joint drivetrain one fixed tick and integrate both belt speeds: the m/d
/// superimposed model documented at module level.
pub fn step(
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
    let mut bitprobe = input_probe(st, inp, m, d);

    swap_ladder_direction(tp, st, inp, m);

    let ladder: &[f32] = if st.reverse {
        &tp.gears_rev
    } else {
        &tp.gears_fwd
    };
    let top = ladder.len() as u8;
    st.gear = st.gear.clamp(1, top);

    let DriverIntent {
        dir,
        propulsive,
        service,
    } = driver_intent(inp, st.reverse, st.park, m);

    update_demand_filter(
        st,
        inp,
        dir,
        #[cfg(feature = "bitprobe")]
        &mut bitprobe,
    );
    #[cfg(feature = "bitprobe")]
    let demand_post = st.demand_n;

    let shaft = dir * m;
    let ratio = ladder[(st.gear - 1) as usize];
    let current_reserve = modeled_reserve_in_gear(tp, inp.supply, shaft, ratio, st.demand_n);

    let hill_hold_step_committed = update_hill_hold(
        st,
        tp,
        fp,
        inp.supply,
        ladder,
        shaft,
        propulsive,
        current_reserve,
    );

    let r_mean = (
        (inp.reactions[0] + inp.reactions[1]) / 2.0,
        (inp.rolling[0] + inp.rolling[1]) / 2.0,
    );
    // Ordinary grade shifts still require a forward landing. The braked rollback rescue commits
    // through `refresh_hill_hold` instead: its purpose is gaining capability, so that named path
    // deliberately waives this sign gate just as the protective-upshift arm deliberately waives
    // its ordinary landing-band gate for engine protection.
    let grade_landing_positive = dir * predict_shift_landing_m(tp, fp, m, r_mean, dt) > 0.0;

    update_steer_detent(st, inp.steer);

    // Predictor-domain guard: while the L600 detent is engaged the constraint force λ loads the
    // outputs in a way the predictor cannot model (it carries no λ/steer state), so its landing
    // prediction is invalid mid-geared-turn — DEFER upshifts until the detent releases.
    // Downshifts stay allowed (the over-rev gate still applies). The broader "hold gear during
    // any turn" UX rule is a separate pending design decision, deliberately NOT implemented.
    let detent_turn = st.steer_step != 0;
    if st.shift_ticks == 0 {
        run_shift_decision(
            st,
            tp,
            fp,
            ladder,
            &DecisionInputs {
                dir,
                m,
                shaft,
                r_mean,
                dt,
                propulsive,
                current_reserve,
                grade_landing_positive,
                detent_turn,
                hill_hold_step_committed,
                supply: inp.supply,
            },
        );
    }
    let shifting = advance_shift_window(st);
    let g = ladder[(st.gear - 1) as usize];

    update_park_latch(st, inp);

    // --- Engine crank state ω_e (stage B). The crank is real state with inertia J; stage
    // A's command-proxy rev floor is DEAD (launch rpm is now the emergent clutch-slip
    // equilibrium). The crank is NEVER negative — it cannot follow a back-driven shaft
    // (stage A's principle, now enforced by the stall guard instead of a floor).
    let omega_idle = tp.engine.idle_rpm * RPM_TO_RAD;
    let omega_e = st.omega_e;
    let k = g / tp.sprocket_radius;

    update_clutch_latch(st, propulsive, m, inp.supply);
    let engaged = !shifting && !st.clutch_out;

    let (tau_free, p_avail) = crank_free_torque(
        tp,
        inp,
        engaged,
        propulsive,
        m,
        k,
        omega_e,
        omega_idle,
        #[cfg(feature = "bitprobe")]
        &mut bitprobe,
    );

    // The engine force on the mean belt axis: the coupling's transmitted torque reflected
    // through the gear (in place of the old f_p + f_drag — drag reaches the belt only
    // through the coupling now). Declutched, the belt gets NOTHING from the engine.
    let i_m = 2.0 * fp.inertia;
    let (omega_floor, omega_over) = crank_guards(tp, inp.supply, omega_e, tau_free, dt);
    let crank = CrankTick {
        omega_e,
        tau_free,
        k,
        dir,
        omega_floor,
        omega_over,
        dt,
    };
    let tau_c = if engaged {
        clutch_coupling(
            crank,
            tp.engine_inertia,
            inp.supply.transmission_condition * tp.clutch_capacity,
            m,
            i_m,
            -(inp.reactions[0] + inp.reactions[1]),
            #[cfg(feature = "bitprobe")]
            &mut bitprobe,
        )
    } else {
        0.0
    };
    let f_c_pre_scale = k * dir * tau_c;

    let (f_s_pre_scale, lambda, j) = steering_force(tp, fp, st, inp, m, d, f_c_pre_scale);

    let work = engine_work(
        tp,
        fp,
        inp,
        crank,
        [
            (f_c_pre_scale + f_s_pre_scale) * 0.5 + lambda * j[0],
            (f_c_pre_scale - f_s_pre_scale) * 0.5 + lambda * j[1],
        ],
        #[cfg(feature = "bitprobe")]
        &mut bitprobe,
    );
    let power_scale = work.scale;
    let f_c = f_c_pre_scale * power_scale;
    let f_s = f_s_pre_scale * power_scale;
    let lambda = lambda * power_scale;

    let hill_brake_active = st.hill_hold;
    release_hill_hold(st, tp, fp, inp.supply, shaft, g, dir, f_c);

    // --- Integrate the crank: J·ω̇_e = τ_free − τ_c (the transmitted torque scaled by the
    // power gate exactly as the belt-side force was — one bookkeeping for both ends of the
    // clutch). The work allocation below also charges steering and caps any clutch re-anchor
    // at the crank energy left after both sprockets have done their work.
    let omega_integrated = omega_e + (tau_free - tau_c * power_scale) * dt / tp.engine_inertia;
    st.omega_e = omega_integrated;

    // --- Assemble per-side sprocket forces.
    let mut q = [
        f_c / 2.0 + f_s / 2.0 + lambda * j[0],
        f_c / 2.0 - f_s / 2.0 + lambda * j[1],
    ];

    apply_brake_stop_forces(&mut q, tp, fp, st, inp, service, hill_brake_active);

    // --- Integrate both sides simultaneously: I·v̇ = Q − R (the reaction ALWAYS applies).
    let next = [
        inp.speeds[0] + (q[0] - inp.reactions[0]) / fp.inertia * dt,
        inp.speeds[1] + (q[1] - inp.reactions[1]) / fp.inertia * dt,
    ];

    settle_crank(
        st,
        tp,
        engaged.then_some(inp.supply.transmission_condition * tp.clutch_capacity),
        next,
        crank,
        #[cfg(feature = "bitprobe")]
        &mut bitprobe,
    );
    st.omega_e = st
        .omega_e
        .min((2.0 * work.crank_energy / tp.engine_inertia).sqrt());
    #[cfg(feature = "bitprobe")]
    {
        force_probe(tp, inp, (dir * m, g, dir * f_c), &mut bitprobe);
        bitprobe.power_scale = power_scale;
        bitprobe.omega_end = st.omega_e;
    }
    // The tick's own values; the stages that take the probe filled what they compute.
    #[cfg(feature = "bitprobe")]
    let bitprobe = TransmissionProbe {
        direction: dir,
        demand_post,
        shaft_speed: shaft,
        gear_reduction: g,
        k,
        omega_idle,
        omega_floor,
        shifting,
        engaged,
        i_mean: i_m,
        f_other: -(inp.reactions[0] + inp.reactions[1]),
        f_c_pre_scale,
        f_s_pre_scale,
        lambda,
        j,
        omega_integrated,
        forces: q,
        next_speeds: next,
        ..bitprobe
    };

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
