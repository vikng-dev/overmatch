# Transmission declaration — design brief (codex, 2026-07-18)


Put a single two-output transmission module between contact-force calculation and belt integration:

\[
I\dot v_i = Q_i-R_i,\qquad i\in\{L,R\}
\]

`R_i` remains the contact-law reaction. The transmission jointly computes `Q_L,Q_R`; it must not remain two independent calls to the symmetric governor in [forces.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/track/forces.rs:762).

Use:

\[
m=(v_L+v_R)/2,\qquad d=(v_L-v_R)/2
\]

where `m` is propulsion speed and `d` is steering difference.

## 1. Architecture menu

### A. Clutch-and-brake — T-34

\[
Q_i=C_i+B_i+E_i
\]

- `C_i`: clutch force. While slipping, \(|C_i|\le C_{\max}\) and it opposes clutch slip; when locked it is the constraint force enforcing `v_i = gearbox output speed`.
- `B_i`: track brake, opposing motion with \(|B_i|\le B_{\max}\).
- `E_i`: engine drag, only while that clutch is engaged.
- Both clutches share one engine torque/power budget. Opening one clutch lets the remaining side receive the available common-engine power.

This matches the real staged control: pull first disengages the steering clutch, then applies the brake ([T-34 service manual](https://www.allworldwars.com/T-34%20Tank%20Service%20Manual.html?level=1)).

The symmetric governor is replaced by three asymmetric paths:

\[
F_{\rm drive}\le \min(F_{\rm gear},P_{\rm engine}/|v|),\quad
F_{\rm brake}\le B_{\max},\quad
F_{\rm engdrag}=T_{\rm drag}(\omega)g/r_s
\]

Per-vehicle data: engine torque curve, ratios/final drive, clutch capacity, sprocket brake torque, efficiencies. No neutral counter-rotation.

### B. Fixed-radius regenerative/geared steering — Tiger L600 style

For steering sign \(s\) and selected gear/detent:

\[
d-s\kappa_{g,j}m=0,\qquad
\kappa=b/R_{g,j}
\]

Equivalently:

\[
r=v_{\rm inner}/v_{\rm outer}=(R-b)/(R+b)
\]

The lossless constraint contribution is:

\[
Q_L=F_p/2+\lambda(1-s\kappa)/2
\]
\[
Q_R=F_p/2-\lambda(1+s\kappa)/2
\]

It does zero ideal constraint work; inner-track negative power returns to the outer track. Efficiency adds a loss \((1-\eta)|P_{\rm transfer}|\).

A continuous steer axis must select discrete states: straight → wide radius → tight radius, with hysteresis. Axis magnitude may control engagement during transition, but interpolating radii continuously is a hybrid, not an L600 model.

Per-vehicle data: gear ratios, two radii/ratios per gear, output and steering-member torque capacities, efficiency, emergency/service brakes.

### C. Continuous regenerative — Merritt-Brown / HSWL family

\[
2I\dot m=F_p-(R_L+R_R)
\]
\[
2I\dot d=F_s-(R_L-R_R)
\]

with output forces:

\[
Q_L=(F_p+F_s)/2,\qquad Q_R=(F_p-F_s)/2
\]

Power is:

\[
P_{\rm outputs}=F_pm+F_sd
\]

Negative power from one output offsets positive power at the other, less losses. At `m=0`, the steering shaft alone produces `vL=+d`, `vR=-d`: genuine neutral steer. Modern RENK units explicitly use infinitely variable hydrostatic superimposed steering and support variable-speed pivot turns ([official HSWL 106 data](https://staging.renk.com/_Resources/Persistent/3/b/3/b/3b3bb5892d9d3a81fbb8fbbeabd1b0231abde5a5/HSWL%20106.pdf)).

Per-vehicle data: propulsion gears, steering-shaft ratio/displacement, steering torque and power capacities, efficiencies, engine/retarder curves, service-brake torque.

### D. Arcade-honest hybrid

Use C’s `m/d` power-flow math with real gearbox ratios, but omit explicit planetary/hydraulic shaft simulation:

- Real engine torque/power envelope.
- Real gear and final-drive ratios.
- Declared steering-output torque/power capacity.
- Regenerative power accounting.
- Declared brakes.
- Continuous `d` command.

This preserves physical emergence while avoiding clutch-pressure, hydraulic-pressure and planetary-carrier state.

## 2. Coupling and netcode

Keep authoritative `vL,vR`.

Reparameterizing to `m,d` saves no state—two floats remain—and both belt phases must still remain independent. It would also force conversions around every per-side contact calculation. Use `m,d` internally inside the transmission module only.

Minimal scheduling change:

1. Evaluate both contact patches at their pre-tick belt speeds; obtain `R_L,R_R`.
2. Solve the joint transmission once.
3. Integrate both speeds simultaneously; advect both existing phases.

Constraint forces are algebraic and need not replicate. REV 14 should add only genuine path-dependent transmission state:

- selected gear;
- shift countdown/phase;
- steering detent for fixed-radius boxes;
- clutch lock/slip state if modeled;
- engine speed only if it cannot be derived while shifting.

That state must be constructed from tank data at spawn, alongside the existing rollback state.

## 3. Braking law

Author brake torque, then convert at the sprocket:

\[
B_{\max}=T_{\rm brake,sprocket}/r_s
\]

The Tiger’s declared \(r_s=0.3931\) m means 250 kN would equal **98.3 kN·m DERIVED** brake torque. That is only a comparison value, not valid provenance; ship values must come from an actual vehicle brake/output-torque source.

Do not attenuate ground reaction with `(1-h)R`. Always apply `-R`, and turn ADR-0026 hold into a capacity-limited static brake:

\[
B_i=\operatorname{clamp}(R_i-Q_{other,i},-hB_{\max},hB_{\max})
\]

while stationary; when sliding, it saturates opposing motion. Thus hold remains smooth, but a slope exceeding brake capacity back-drives the belt honestly. The 20° Tiger load is about **95.6 kN/side DERIVED**, so brake capacity becomes testable rather than infinite.

Retarding paths:

- Clutch/brake: steering/service brake; engine drag only with clutch engaged; no recovery.
- Fixed-radius differential: regenerative steering constraint; common-mode engine drag; separate service/emergency brakes.
- Continuous regenerative: steering shaft, common-mode engine drag/retarder, service/parking brakes.
- Engine braking should be a reflected drag-torque curve \(T_{\rm drag}(\omega)\), not the negative half of rated engine power. It only becomes “power-like” after multiplying torque by engine speed.

## 4. Gearing

The existing envelope has a knee at:

\[
260\,{\rm kW}/250\,{\rm kN}=1.04\ {\rm m/s}
\]

Above that, it behaves like an ideal continuously variable transmission. It captures broad acceleration and hill-climb limits, but not:

- per-gear force bands;
- engine RPM and torque-curve shape;
- torque interruption during shifts;
- gear-dependent engine braking;
- reverse ratios;
- the Tiger’s gear-dependent steering radii.

Therefore a gear set is warranted for the Tiger: without it, an “L600” model is only cosmetic. The minimum rollback cost is gear plus shift phase/ticks; engine RPM may remain derived outside clutch-slip/shift periods.

## 5. Feel and yaw budget

For comparison, these are **DERIVED instantaneous/onset ceilings**, with \(b=1.49\) m, 250 kN/output, 520 kW total engine, and the repo-recorded ~478 kN·m scrub. The clutch row temporarily assumes a 250 kN brake solely for comparison.

| Architecture | 4 m/s | 8 m/s | Against 478 kN·m |
|---|---:|---:|---|
| Current symmetric governor | 194 kN·m | 97 kN·m | Far short |
| Clutch/brake | 566 kN·m | 469 kN·m | Strong at 4; marginal at 8 |
| Fixed-ratio regenerative | \(372.5(1+r)\) kN·m | same capacity law | 373–745; e.g. `r=.4` → 521 |
| Continuous/hybrid regenerative, turn onset | 745 kN·m | 745 kN·m | Comfortable margin |

The current neutral-steer launch is **745−478=267 kN·m excess DERIVED**, explaining the snap. Under the constant-power curve, scrub balance occurs around belt difference `d=1.62 m/s`, or **1.09 rad/s yaw DERIVED**. A real pivot ratio, engine curve and reflected gear inertia should make this statelier—not another slew dial.

For regenerative steering, sustainable rather than onset torque depends on turn radius:

\[
P_{\rm yaw}=M_{\rm scrub}V/R
\]

With 520 kW, sustaining 478 kN·m requires approximately `R ≥ 3.68 m` at 4 m/s and `R ≥ 7.35 m` at 8 m/s. Tighter commands must slow the tank. That is the desirable emerging high-speed behavior: strong turn-in, followed by physically required speed loss.

## Correctness batch (2026-07-19) — four fixes, post-implementation

Applied after the phase-2.5 implementation landed; all four are SIM POLICY or datum
re-anchors — the Governor parity path, the grip law, μ, and the wire surface are untouched.

### 1. Shift scheduler anti-hunting (SIM POLICY, all vehicles)

The static up/down bands (2300/1400, gap > one ratio step) were sound in statics but not
under the shift's own dynamics: the 0.31 s torque-cut window bleeds belt speed while
`I·v̇ = Q − R` keeps subtracting the ground reaction, and the low gears' rpm-per-speed
slope (~2500 rpm per m/s in Tiger gear 2) turned ~0.19 m/s of bleed into ~480 rpm —
erasing the ~100 rpm static margin, so the down band fired the tick the freeze lifted
(measured full-throttle climb trace `[1,2,1,2,1,2,3,2,3,4,3,4,5,6,7,8]`). Three gates,
fixed-tick deterministic, in `transmission.rs`:

- **Predicted-landing gate on upshifts** (`POSTSHIFT_MARGIN_RPM = 150`): the upshift only
  commits if rolling the shift window's own integration (drive torque cut, engine drag
  through the landing gear, reaction frozen at its current per-tick mean — the same code
  path via `reflected_drag`) lands the rpm ≥ down band + margin. Frozen-R is conservative
  under load (the true post-cut reaction collapses with slip), so a loaded box revs
  further up each gear before shifting — correct hill behavior for free. *(REFUTED on
  grades by stage A below: the landing rpm was computed through `|m|`, so a landing
  driven NEGATIVE by the frozen reaction read as a huge positive rpm and PASSED the gate
  — the abs() annulled the gate exactly where it mattered. The claim held only while
  landings stayed positive, i.e. on the flat.)*
- **Reversal-only dwell** (`REVERSAL_DWELL_TICKS = 32` = 0.5 s): a committed shift blocks
  the opposite-direction shift; same-direction 1-2-3 climbs stay free. State:
  `last_shift_dir` + `dwell_ticks` in `TransmissionState` (local, REV 13 unaffected).
- **Over-rev gate on downshifts** (`OVERREV_MARGIN_RPM = 100`): a downshift landing past
  the engine's max curve rpm − margin is refused.

Post-fix measured climb trace: `[1,2,3,4,5,6,7,8]`, gated monotone by
`gear_climb_monotone_tiger`; unit gates for all three policies in `transmission.rs`.

### 2. Hybrid pivot is power-limited (doctrine correctness)

The Hybrid floored its standstill steer target at `neutral_d_full` — a kinematic speed
command that used ~68 kW of the ~407 kW budget and pivoted at 0.131 rad/s, contradicting
this document's own §C doctrine (the hydrostatic family is limited by the POWER budget).
Now at `m → 0` the box commands steer FORCE up to the per-output capacity bound and the
existing power-conservation scale is the binding limiter: the pivot rate settles where
engine power balances scrub dissipation. The moving curvature servo is unchanged; the two
regimes blend continuously on |m| over `NEUTRAL_M_SPEED` (`hold_blend`). Measured
emergent Tiger pivot: **0.654 rad/s** (belts ±1.40 m/s), against the §5 prediction of the
0.5–0.6+ rad/s class; gate `pivot_tiger_hybrid` ≥ 0.35 rad/s.

### 3. `neutral_fraction` deleted (unprovenanced authored scalar)

The RON's `neutral_fraction: 0.75` was an INFERRED feel scalar with no source. The
derived datum `neutral_d_full = κ_tight(F1) × v1_governed = 0.2808 m/s` IS the correct
emergent value for a fixed-radius box — the radii table's own invariant makes
`κ_tight(g) × v(g)` gear-independent (≈ 0.337 m/s @ 3000 rpm in every gear). Field
removed from spec struct, RON, validation, and the L600 neutral path (which now servos to
`neutral_d_full` directly). Measured L600 neutral turn: **0.131 rad/s** (belts exactly
±0.281 m/s); gate `pivot_tiger_l600` ≥ 0.10 rad/s.

### 4. Brake datum re-anchored (was circular)

`brake_force: 250_000` per side was sized by the grip-limit rule against μ = 0.9 —
circular (sized against the very friction it was to be tested by) and energy-impossible
(~2.9 MW through two 1940s Argus discs at speed; §3 above already flagged that ship
values need a real brake source). Re-anchored (final value set in the review round below)
to a DUAL documented anchor: the settled 20° park-hold capability (W·sin 20°/2 ≈
95.6 kN/side) and 0.343 g total service decel (inside the 0.2–0.35 g band realistic for
WWII 57-t heavies) → `brake_force: 96_000` per side. The stop-force law
`B = clamp(R − Q − vI/dt, ±cap)` and the park latch are untouched — only the datum moved.
Measured service-brake stop 6 → 1 m/s: **2.23 s** (analytic ≈ 0.5 s input slew +
5.0 m/s ÷ ~3.6 m/s² brake+drag); gate `decel_tiger` ≤ 3 s, coast leg unchanged (10.7 s);
new gate `slope_park_holds_20_deg_tiger` pins the 20° hold (measured drift 0.000 m over
4 s, latch engaged). At this stage, before the static/dynamic split below, 30° ramps
back-drove (139.8 kN/side DERIVED demand > dynamic capacity). Supersede the inferred
values with a real Argus brake/output-torque rating when sourced.

### Review round (same day) — three adversarial findings, dispositions

1. **Landing gate consulted outside its domain (High).** The predictor integrates drag
   but no brake term and carries no λ/steer state, yet upshifts were considered under
   service braking (F7 @ 2500 rpm + full opposing throttle: predicted landing 1652 rpm on
   drag alone, live window with brakes landed 1262 — below the down band → false shift +
   reversal cycling) and during L600 geared turns. Fixed by INTENT-gating: upshifts are
   considered only while `propulsive > 0` (a braking/coasting driver never needs one),
   and the L600 DEFERS upshifts while a steering detent is engaged (downshifts stay
   allowed; the over-rev gate still applies — the broader "hold gear during any turn" UX
   rule remains a separate pending design decision). The predictor's doc now states its
   domain honestly: propulsive straight-line only, frozen-R conservative there, single
   mean-axis clamp an accepted approximation of the per-side clamps. Also: the dwell now
   counts only OUTSIDE the interruption window, so a reversal gets the full 32
   post-engagement ticks (it previously drained to ~12 during the frozen window). Tests:
   `no_upshift_while_braking_or_coasting`, `l600_detent_defers_upshift`, and the dwell
   test now pins the exact window + 32 timing.
2. **Hybrid steer release at pivot cancelled the d-arrest servo (Medium).** At m ≈ 0 with
   steer released, the |m|-only blend weight kept w = 1 while pivot_f = 0 — f_s = 0, so
   an airborne pivot kept counter-rotating forever. The blend weight is now
   `hold_blend(|m|/NEUTRAL_M_SPEED) × |steer|`: continuous in both axes, and steer → 0
   returns the whole force to the curvature servo whose target is then 0 (active arrest).
   Test: `hybrid_steer_release_arrests_pivot`. Measured pivot rates unchanged
   (0.654 / 0.131 rad/s).
3. **84 kN broke the settled 20° hill-hold (Medium).** The single 0.3 g anchor
   (167.7 kN total) sat just under the 20° slope demand (191.2 kN total) that ADR-0026
   and the test course had settled as capability. Re-anchored to the dual anchor above
   (96 kN/side) and the previously missing slope-park gate now pins it.

## Stage A (2026-07-19) — signed shaft correctness

`rpm_of` measured the shaft through `|m|`, so a belt BACK-DRIVEN in a forward gear read
as high FORWARD rpm. Design principle (director): **the SHAFT is signed** — rigid gearing
— **the ENGINE is never negative** — it cannot follow a back-driven shaft; the existing
command-proxy rev floor is the implicit clutch slip until the ω_e crank-state arc lands.
All three consequences were reproduced before the fix:

1. the fuel governor cut drive to zero on backslides (tank rolling backward on flat
   ground under full W, F1 showing "2770 rpm", zero force, indefinitely);
2. the scheduler's up band fired repeatedly during backslides (measured ladder walk 1→6
   while sliding backward at −2..−3 m/s);
3. the fix-1a landing gate PASSED catastrophic on-grade upshifts — a predicted BACKWARD
   landing (`landing_m = −3.62`, traced r_mean = 221 kN) read as "9092 rpm" ≥ band +
   margin. This is the refutation of the "correct hill behavior for free" claim in fix 1
   above.

The fix, all in `transmission.rs` (Governor float path untouched — parity green; no
wire/replication, no μ/grip changes, deterministic f32):

- **Signed shaft**: `shaft = dir·m` (dir = −1 on the R ladder); signed
  `shaft_rpm = shaft·G/r_s` used by the scheduler, the landing gate, and the engine
  operating point.
- **Scheduler**: the up band compares the signed rpm (negative can never exceed it); the
  down band additionally requires `shaft ≥ 0` — a back-driven vehicle is not "running
  slow forward". Gear changes are decisions about FORWARD operation; the backslide state
  HOLDS the engaged gear in both directions.
- **Landing gate**: the predicted landing's SIGNED shaft speed must be positive AND its
  rpm ≥ down band + `POSTSHIFT_MARGIN_RPM`; a sign-flipped landing always refuses.
- **Engine side**: torque evaluates at `engine_rpm = max(shaft_rpm, rpm_floor)` — never
  negative; during a backslide the engine keeps delivering forward drive at the floored
  rpm (`f_p = dir·propulsive·…`, verified), and the governor cut applies only to real
  forward over-speed. The HUD `readout` shares the convention (a back-driven shaft reads
  idle, not a fake forward rpm).
- **`reflected_drag` audited**: `m/DRAG_SAT_SPEED` is already signed and opposes the
  actual belt motion under back-drive — correct, left as-is.

Deliberately NOT in this stage: engine crank state (ω_e — next arc), grade-aware
scheduling, skip-shifts, manual gear hold.

Measured gates: `backslide_holds_gear_and_keeps_forward_drive`,
`landing_gate_refuses_sign_flipped_landing` (units);
`ramp_climb_20_deg_never_upshifts_backward_tiger` (headless, real Tiger L600): from REST
mid-face on the 20° course ramp under held W the Tiger crests in **7.1 s, gear trace
[1]** — F1 holds the grade, no upshift is predicted to land, and it never moves backward
while a shift commits. Flat-ground behavior is undisturbed (driving with the ladder,
`dir·m ≡ |m|`): the anti-hunting climbs, top speed, decel, and pivot gates are unchanged.

### Stage-A review round (same day) — three findings, dispositions

1. **Hard `shaft >= 0` down-guard stranded the cruise gear at rest (real defect).**
   Coasting to rest in a cruise gear (Hybrid, gear 3, zero command, 20 kN/side reaction),
   the brake stop-force/integration order leaves a stable numerical residual of
   ≈ −1.7e−9 m/s — the exact-zero guard read it as "back-driven" and blocked the
   downshift chain forever (tank parks stranded in gear 3). The backslide hold must only
   engage for a GENUINE backslide: the guard is now `shaft > −PARK_ENGAGE_SPEED`,
   reusing the existing at-rest policy scale — residuals orders of magnitude below it
   downshift normally, a real slide at −0.5 m/s+ still holds. The up band needs no
   threshold (negative can't exceed it either way), and the landing gate's `> 0` needs
   none either: its rpm bound already demands a landing ≥ down band + margin, solidly
   positive. Regression: `coast_to_rest_completes_downshift_chain` (proven to bite —
   fails under the reverted `>= 0` guard).
2. **Spec validation accepted absurd torque data.** Any finite non-negative torque passed
   (including `f32::MAX`); `reflected_drag`'s multiplication then overflows to ∞, and
   `∞ × 0.0` (full drag release) is NaN inside the landing predictor. The gate fails
   SAFE on NaN (`NaN > 0` is false — the upshift refuses), but the spec layer now
   refuses the data outright: every torque point ≤ 100 kN·m (an order of magnitude above
   any tank engine) and every curve rpm ≤ 20 000 (far past any piston redline), on top
   of the existing finite/ascending checks (`spec.rs`).
3. **Coverage: reverse-ladder mirror + reverse readout.** Reverse symmetry HOLDS by
   construction (`dir` mirrors everything), now pinned:
   `reverse_backslide_holds_gear_and_keeps_reverse_drive` (driving in R, back-driven
   forward → shaft < 0: no shifts on a 3-gear R ladder, drive stays R-signed, governor
   does not cut) and `readout_reverse_reads_signed_shaft` (R label + positive geared rpm
   while reversing; idle while back-driven).

## Stage B (2026-07-19) — engine crank state ω_e

The crank is now real state: `TransmissionState.omega_e` (rad/s, f32) with inertia J
(`engine.inertia_kgm2`), coupled to the geared shaft by a capacity-clamped main clutch
(`engine.clutch_capacity_nm`). Stage A's command-proxy rev floor is DELETED — launch rpm
is the emergent clutch-slip equilibrium. All in `transmission.rs` behind the regenerative
path (Governor float path untouched — parity test green bit-for-bit; no wire/replication,
no μ/grip changes, deterministic f32 at 64 Hz).

**Spec provenance.** `inertia_kgm2: 4.0` — INFERRED by engine-class scaling
(flywheel-dominant; large tank engines land in a 2.5–6 kg·m² band: a ~0.5 m, 50–80 kg
flywheel rim alone is 2–4 kg·m²), mid-band. `clutch_capacity_nm: 2400` ≈ 1.3 × the
1850 N·m peak (the usual dry-clutch sizing margin; no Argus/OLVAR rating reachable).
Lab (T-34 config + unit tests): J = 4.0, capacity = 2860 (1.3 × 2200). Validation:
finite, positive, J ≤ 100, capacity ≤ 50 000. Spec pin test updated deliberately.

**Update law (per tick, inside `regenerative`, replacing the rpm_floor block):**

1. Free torque from the PRE-tick crank: `τ_free = τ_ind + τ_idle − τ_drag` with
   `τ_ind = u·torque_at(ω_e)` (the governor cut now acts on the crank),
   `τ_idle = clamp(gain·(ω_idle − ω_e), 0, torque_at(idle))` (gain = full recovery over
   `K_IDLE_DROOP_RPM` = 50 rpm of droop), `τ_drag = drag_fraction·peak·
   hold_blend(u/DRAG_THROTTLE_RELEASE)·sat(ω_e/ω_idle)`. Drag moved ENGINE-side: the
   belt lost its separate `f_drag` term and drag reaches the belt only through the
   coupling (`reflected_drag` deleted; the old belt-side `DRAG_SAT_SPEED` role is played
   by the ω_idle saturation).
2. COUPLING (engaged ⇔ not shifting ∧ not neutral-idle, where neutral-idle =
   `propulsive < NEUTRAL_THROTTLE ∧ |m| < NEUTRAL_M_SPEED` — the L600 neutral seam
   GENERALIZED to both adapters; without it an engaged idle-governed crank at standstill
   rides the clutch at hundreds of kN of creep/pivot-drag force): ONE seamed function
   `clutch_coupling` — the coupling-law slot; a torque-converter characteristic replaces
   the clamp for modern automatics later —
   `τ_c* = [(ω_e − k·s·m)/dt + τ_free/J − k·s·F_other/I_m] / (1/J + k²/I_m)` with
   `k = G/r_s`, `s` the ladder direction, `I_m = 2·belt inertia`, `F_other = −ΣR`
   (λ/brakes excluded — near-zero in engaged drive regimes; the end-of-step drift kill
   re-anchors an exact lock). `τ_c = clamp(τ_c*, ±capacity)`; belt receives
   `F_c = k·s·τ_c` split per side IN PLACE OF `f_p + f_drag`; crank integrates
   `ω_e += (τ_free − τ_c·power_scale)·dt/J`. STALL GUARD (ships with this): a landing
   below `ω_idle − STALL_GUARD_BAND_RPM` (100) reduces τ_c one-sidedly to land exactly
   at the floor — the clutch slips to protect the crank; the band is 2× the idle droop
   so the saturated idle governor guarantees the floor is holdable. No stall death
   (later, playtest-gated).
3. DECLUTCHED (shift window / neutral-idle): belt gets NO engine force and NO drag; the
   crank runs a proportional-band rev governor (`REV_MATCH_BAND_RPM` = 200) toward
   `max(|m|·k_landing, idle + (peak_torque_rpm − idle)·|steer|)`. Two DOCUMENTED
   deviations from the memo's shorthand: (a) the steer-demand target (the surviving half
   of the old `cmd_mag` contract) — without it a declutched pivot idles at ~1/5 of its
   power budget; (b) the target is a SPEED, not blind full fueling — an unloaded crank
   under u = 1 spools past the peak-power point to the governor cut-out where
   `torque_at·ω = 0` (the d-path draw does not load the crank in this stage — deferred
   honestly; the power gate caps the draw instead), so the steer demand must park the
   crank at the peak-torque point or steady pivot power collapses to zero.
4. `predict_shift_landing_m` rewritten to the new window physics: reaction-only bleed
   (no drag — the window is declutched in reality too, so prediction and reality agree).
   `POSTSHIFT_MARGIN_RPM` = 150 re-derived UNCHANGED: at full throttle (the dominant
   upshift intent) drag was already fully released (`hold_blend(1/0.5) = 0`), so the
   predictor's arithmetic is identical there; the margin covered reaction-bleed
   prediction error, which is untouched.
5. Power gate: `p_avail = torque_at(ω_e)·ω_e` — the crank, not the input slew. The
   rpm_floor hack is deleted.
6. End of step: if engaged ∧ τ_c unclamped ∧ guard quiet ∧ power_scale = 1, snap
   `ω_e = k·s·m_next` exactly (drift kill; refused if it would land below the guard
   floor).
7. `readout` returns ω_e directly — the state IS the display (rpm is still rpm; HUD
   line shape unchanged).

**REV-14 rider.** ω_e is LOCAL state under REV 13 (not replicated, not hashed). Its wire
registration rides the later netcode arc with the rest of the REV-14 list: it is sim
state a rollback replay must restore — NOT derivable from the belt, because the clutch
slips.

**Measured gates (all re-derived; before → after on the declared Tiger data):**

| gate | before | after |
|---|---|---|
| flat-ground full-W climb | monotone [1..8] | monotone [1,2,3,4,5,6,7,8] — survives |
| top speed (30 s) | 10.49 m/s | 10.49 m/s (governed equilibrium now ON the crank) |
| coast 6→2 m/s | 10.6 s | 12.2 s — the belt's drag share is `I_m/(I_m + k²J)` ≈ 0.85 in F7 (crank+belt decelerate together) and shift windows are genuinely drag-free; gate ≤ 14 s holds |
| service brake 6→1 m/s | 2.23 s | 2.31 s (gate ≤ 3 s) |
| L600 pivot | 0.131 rad/s | 0.1314 rad/s (steady preserved) |
| Hybrid pivot | 0.654 rad/s | 0.637 rad/s (crank parks at peak-torque point minus ~30 rpm rev-governor droop) |
| Hybrid pivot spin-up (NEW gate) | 0.94 s | 0.95 s to 90% yaw — NOT the memo's 1.2–1.5 s: the power gate cannot bind at v ≈ 0, so the capacity-limited early phase hides the ~0.4 s crank spool under the ~0.5 s steer slew. The gate pins 0.95 s ± margin AND the crank state itself (908 rpm @ 0.1 s → 2064 rpm steady) — the discriminator the yaw time alone is not |
| 20° from-rest crest | 7.1 s, trace [1] | 8.1 s, trace [1] — the launch is gentler (clutch-limited, then lock) |
| launch wheelspin (20° ramp, max belt-vs-hull slip) | 0.370 m/s | 0.155 m/s — 58% cut: the lock catches within ticks and the reflected crank inertia (k₁²J ≈ 20× belt inertia in F1) pins the belt |
| slope park 20° | holds, 0.000 m | holds, 0.000 m |
| readout | geared-rpm proxy | crank truth (F7 @ 2345 rpm driven; sub-idle lug reads honestly) |

New unit pins: `launch_is_clutch_slip_limited` (standing start full W: belt force =
k₁ × capacity ≈ 242.5 kN lab, measurably NOT the old rev-floor 186.6 kN),
`stall_guard_holds_crank_under_grade_lug` (full-W lug + coast backslide: ω_e never below
idle − 100 rpm; forward drive persists through the slipping clutch),
`rev_match_across_upshift_is_continuous` (≤ 250 rpm/tick slew, ≤ 400 rpm gap at window
end, re-locks to the geared point), `free_rev_reaches_steer_target_promptly` (lab
idle → 95% of peak-torque target in 0.05–0.6 s; parks at the peak-torque point, NOT the
cut-out), `coast_drag_reaches_belt_through_coupling` (steady coast force through the
clutch = the declared drag share exactly — the steady state is coupling-law-invariant),
`readout_reports_crank_state`. Backslide behavior: forward drive during slides now comes
through the clutch — capacity-clamped under full W (`F_c = k·2400`), `τ_free`-limited on
a coast slide; the stage-A backslide unit tests hold unchanged.

Deliberately NOT in this stage: grade-aware scheduling / skip shifts (stage C), stall
death, torque-converter coupling, d-path crank loading (the power gate stands in).

### Stage-B review round (same day) — four findings, dispositions

Adversarial review of the landed stage B: coupling signs, rev-governor stability, the
stage-A regression surface, and the REV-13 boundary all held; four findings fixed:

1. **Stale `exact` flag let the drift kill violate clutch capacity (Critical).** The
   flag was decided at the pre-brake/pre-λ coupling solve, but brakes, the FixedRadii λ
   mean-axis share (`j_L + j_R = −e` does NOT cancel), and the belt ±max_speed clamp all
   move `m_next` afterwards — the snap then teleported the crank with the belt (traced:
   a full-opposing-throttle F1 brake tick implied τ_c ≈ 9.7 kN·m through the 2.4 kN·m
   clutch). Replaced by an END-OF-STEP feasibility check on the final belt state:
   `τ_impl = τ_free − (k·s·m_next − ω_e)·J/dt`; snap only if `|τ_impl| ≤ capacity` (and
   ≥ the stall floor), else the honestly integrated slipping crank stands. The
   pre-solve's reactions-only `F_other` is now documented as a PREDICTOR approximation —
   exact pre-accounting is circular (the brake law reads the q that needs F_c) — made
   safe by exactly this check. The power_scale condition dropped (any within-capacity
   landing is a legitimate clutch outcome). Regression (proven to bite against the
   eager flag): `braking_never_teleports_crank_past_clutch_capacity` — per-tick crank
   slew bounded by `(capacity + drag)/J·dt` through a closed-loop brake stop.
2. **Stall guard + sentinel not spec-robust (High).** The guard's ±capacity clamp meant
   a legal strongly-negative τ_free (big drag fraction over a weak idle curve) could
   carry ω_e below the floor and negative, where the old `≤ 0` sentinel teleported it
   back to idle every tick (traced 600 → −3130 → 600 rpm oscillation). Fixed: (a) HARD
   end-of-tick clamp `ω_e ≥ ω_floor` — policy-honest, the floor IS the no-stall policy
   while stall death stays unmodeled (classification table updated; also self-heals NaN
   via f32::max); (b) the sentinel tightened to exactly `== 0.0` — it now fires at true
   spawn only — with spec validation `idle_rpm ≥ 300` (100 band + 100 margin +
   headroom) keeping `ω_floor > 0` always.
3. **Engagement seam chatter (Medium).** A boundary creeper sawtoothed engage/declutch
   on the single NEUTRAL_M_SPEED line. The seam is now a LATCH with detent-style
   hysteresis: `TransmissionState.clutch_out` (local, REV 13), out below
   `NEUTRAL_M_SPEED × 0.8` (0.4 m/s) without propulsive drive, back in at
   `NEUTRAL_M_SPEED × 1.2` (0.6 m/s) or on any propulsive command (the launch).
   Deterministic, no blend. Regression: `clutch_seam_hysteresis_kills_boundary_chatter`
   — a forced ±0.05 oscillation around the old 0.5 threshold produces ZERO regime
   flips; only genuine excursions past the separated thresholds transition, once each.
4. **Validation lower bounds (Medium).** The solver's numeric assumptions are now spec-
   protected: `inertia_kgm2 ∈ [0.1, 100]` (the coupling divides by J),
   `clutch_capacity_nm ∈ [100, 50 000]`, `idle_rpm ≥ 300` (finding 2's floor), and —
   with a transmission declared — `powertrain.inertia ≥ 1.0` (the lock denominator's
   `k²/I_m` term; the generic finite/> 0 check admitted 1e-30). Negative cases for each.

All gates re-measured after the fixes — deltas are noise: crest 8.1 → 8.2 s, driven
readout 2345 → 2343 rpm, ramp launch slip 0.155 → 0.154 m/s; coast 12.2 s, brake
2.31 s, pivots 0.637/0.1314 rad/s, spin-up 0.95 s (crank 908 → 2064 rpm), climb
monotone [1..8], top speed 10.49 m/s, slope park exact, Governor parity bit-identical —
all unchanged.

## Stage C (2026-07-19) — reserve scheduler and anti-rollback

Stage C replaces speed-band-only grade decisions inside the regenerative adapters with a
load-aware composition. The Governor adapter remains the untouched parity path; there are no
wire/replication changes, no manual controls, no crew-delay model, no terrain look-ahead, and no
time/random input. All arithmetic is deterministic f32/integer-tick at the **64 Hz SIM POLICY**.

**Demand and reserve law.** On each decision tick (`shift_ticks == 0`), the scheduler projects the
owned ground reactions onto the signed mean-shaft axis and filters positive demand with
`D_n = D_(n-1) + (sample - D_(n-1))/8`. The **8-tick = 0.125 s DERIVED** EMA freezes through a
shift window so the torque cut cannot masquerade as a grade change. At current belt speed, each
gear's full-throttle capability is

`F_j(v) = min(torque_at(max(0, v) * G_j / r_s) * G_j / r_s, 2 * engine_force)`,

and `R_j = F_j - D`. Required headroom is `0.10 D + 10 kN` (**DERIVED policy values**): the
fraction avoids a zero-acceleration target, while 10 kN is 1.8% of Tiger weight and about half the
fractional margin on the **191.2 kN DERIVED** 20-degree demand. The authored-curve reconstruction
gives **~169 kN DERIVED** for Tiger F4 at **~980 rpm DERIVED** (the slope investigation's
**165 kN DERIVED** rounding), so F4 is correctly deficient against **191.2 kN DERIVED** while F3
clears margin.

**Scheduler composition.** Upshifts retain the asymmetric stage-A/B contract—up band,
propulsive intent, straight/detent domain, predicted positive landing, postshift band margin, and
reversal dwell—then add `R_next >= margin`. Flat ground remains the same path because reserve is
ample. Downshifts retain the ordinary low-rpm path while the current gear is capable; a negative
reserve instead owns a **13-tick = 0.203125 s DERIVED** confirmation, after which the target is the
highest lower gear clearing margin. A shorter spike resets the counter. The target is bounded by
the current-speed over-rev gate; if the ideal target over-revs, the closest legal gear toward it is
chosen. A direct skip must also predict a positive signed landing through its one cut window.

**Capability principle: the model accepts all variants; the spec declares the vehicle.**
`gearbox.shift_addressing` is `Direct | Sequential`, with `Sequential` the conservative serde
default because adjacent, separately paid shifts assume no arbitrary-selection mechanism.
`Direct` commits the legal target in one event/window. `Sequential` retains the same original→final
target in local state and steps one adjacent gear per event, paying every window. The Tiger OLVAR
authors `Direct` from its arbitrary-gear preselection provenance; the T-34-class lab box stays
`Sequential`. The enum shape itself rejects unknown authored values during deserialization.

**Anti-rollback.** Forward command near rest with negative effective reserve latches hill hold.
The near-rest threshold is **0.25 m/s DERIVED** as `5 × PARK_ENGAGE_SPEED`; an active shift cut has
effective `F = 0`, allowing the hold to catch a sequential cascade whose landing gear is statically
capable but temporarily disconnected. Hold calls the existing service-brake stop-force path at its
full declared envelope—no duplicate or hidden force—and selects a launch gear through the same
reserve law. It releases only when post-power-gate coupling force exceeds `D + margin`, with the
brake envelope retained for that handoff tick. Command release or reverse intent clears it. If no
gear has non-negative reserve, `GRADE LIMIT` remains exposed and the declared brakes stay applied.

**REV-14 rider.** `demand_n`, its spawn seed, `grade_confirm_ticks`, held target,
`SchedulerState`, and `hill_hold` are local `TransmissionState` under REV 13. They are sim state a
future rollback replay must restore alongside gear/shift/crank; none is derivable from an
instantaneous belt sample. No replication field was added in this stage.

**Coupling seam only.** A future torque converter belongs at the existing `clutch_coupling` seam
and would author its own characteristic. Stage C does not implement one.

| gate | Stage-C result |
|---|---|
| Reserve arithmetic + 20-degree upshift veto | F4@**~980 rpm DERIVED** gives **~169 kN DERIVED** < **191.2 kN DERIVED** demand; F3 clears margin; F3→F4 veto unit green |
| Confirmation transient | **12 ticks DERIVED test input** does not shift; counter clears before the **13-tick DERIVED** threshold |
| Direct vs Sequential unit | same F6→F3 target: Direct commits F3 in one event; Sequential commits F5 and later reaches F3 through paid windows |
| Direct signed landing | negative predicted landing refuses the skip; no window starts |
| 20-degree F6 approach, Direct | **3.281 s MEASURED** crest, trace `[6,4]`, reserve state `F6→F4`, **1.393 m/s MEASURED** minimum uphill speed, **0.0000 m MEASURED** retreat, no hill hold |
| 20-degree F6 approach, Sequential | **5.938 s MEASURED** crest, trace `[6,5,4,3,2,1]`, **43 ticks MEASURED** hill hold, **-0.124 m/s MEASURED** minimum course-tangent settling speed; **0.0364 m MEASURED** static-compliance settle (inside the existing **0.05 m DERIVED gate bound**), not a slide-off |
| 20-degree hill-hold launch from F5 | hold releases at **0.906 s MEASURED**; reaches 0.5 m uphill at **2.062 s MEASURED**; **0.0000 m MEASURED** retreat |
| 30-degree no-capable-gear fixture | weak-engine/160 kN-per-side modeled-brake variant exposes `GRADE LIMIT` for **384/384 ticks MEASURED** (6 s DERIVED), drift **0.0041 m MEASURED**, belt m **0.0000 m/s MEASURED** |
| Existing 20-degree from-rest F1 | **8.2 s MEASURED**, trace `[1]`, launch slip **0.154 m/s MEASURED**—unchanged |
| Existing flat anti-hunting | monotone `[1,2,3,4,5,6,7,8]` **MEASURED**—unchanged |
| Existing top speed, coast, brake | **10.49 m/s MEASURED**; **12.2 s MEASURED** from release to the **2 m/s DERIVED gate threshold**; **2.31 s MEASURED** from opposite command to the **1 m/s DERIVED gate threshold** |
| Existing pivots and spin-up | L600 **-0.1314 rad/s MEASURED**; Hybrid **-0.6373 rad/s MEASURED**; Hybrid reaches the **90% DERIVED threshold** in **0.95 s MEASURED**, crank **908 rpm MEASURED** at the **0.1 s DERIVED sample** and **2064 rpm MEASURED** steady |
| Existing slope park/backslide | 20-degree park drift **0.0000 m MEASURED** over the **4 s DERIVED gate window**; signed-shaft forward/reverse backslide units green |
| Governor parity | bit-identical parity test green **MEASURED**; float path untouched |
| Wire and spec pins | wire-surface/types/fingerprint and Tiger schema/bind/validation pins all green **MEASURED**; no replication registration changed |

## Ranked recommendation

1. **Tiger: fixed-radius, geared regenerative model behind the joint transmission seam.** Historically characteristic, fixes high-speed sluggishness, and derives stately pivot behavior from ratios and power.
2. **Arcade default: geared regenerative `m/d` hybrid.** Best controls and least mechanism state while retaining honest energy flow.
3. **Full continuous Merritt-Brown/HSWL adapter** for vehicles that actually use it.
4. **Clutch-and-brake adapter** only for T-34-class vehicles; its lack of neutral steer and heat-wasting turns should remain genuine traits.

### Stage C review round (2026-07-19)

Adversarial review (codex) rejected the first cut with 1 blocking + 5 serious findings;
all dispositioned: hill-hold is now a LIVE latch (per-tick reselection, truthful
GradeLimit, reserve-scaled release formula, 32-tick re-engage cooldown overridden by
real rollback); a CONFIRMED reserve deficit outranks the upshift arm and is exempt
from the reversal dwell (a correction, not hunting); sequential targets revalidate
intent + demand every step; the demand EMA reseeds on direction swap; protective
overrun upshift (governed + 150 rpm, no propulsive intent needed) closes the downhill
over-rev path; confirmation counter decays instead of hard-resetting; reverse-ladder
HUD labels. The 30° gate now boots the untouched Tiger blueprint and asserts the
HONEST result: the Tiger CLIMBS 30° from a hill-hold launch (500 kN modeled F1 launch
force vs 279.6 kN demand) — the previous synthetic fixture faked a grade limit. The
parked 30° slide was correct under the then-single 192 kN DERIVED brake capacity; this
finding is SUPERSEDED by the static-vs-dynamic split below. A
two-world bit-exact FixedRadii slope replay (full TransmissionState incl. EMA,
counter, target, latch; 512 ticks) now guards stage-C determinism.

### Static brake capacity + steering visibility (2026-07-20)

The brake datum is now two limits. `brake_force` remains the dynamic per-side capacity for every
service stop, moving belt, post-breach latched slide, and scheduler rollback-arrest calculation.
The new required `brake_static_factor` multiplies it only when a parking or hill-hold latch is
active, no service-brake command exists, and that individual belt is strictly inside the
`PARK_ENGAGE_SPEED` at-rest band. Leaving the band selects dynamic capacity in that same tick.

Tiger authoring uses **1.5× INFERRED provisional**: static hold does no work, sourced material
ratios span **1.05–2.0×**, and the Argus discs are described as self-energizing by both British
Report No. 19 and D 656/30, but no numerical Argus rating survives. That produces
`2 × 96,000 × 1.5 = 288,000 N` **DERIVED** static capacity against
`57,000 × 9.81 × sin(30°) = 279,585 N` **DERIVED** demand, an **8,415 N DERIVED** margin.
The real 30° park gate measured **0.0393 m MEASURED** drift over **4 s DERIVED**; the unchanged
20° gate measured **0.0000 m MEASURED**. A synthetic **1.1× INFERRED test fixture** has only
**211,200 N DERIVED** static capacity, breaches, drops to **96,000 N/side MEASURED** dynamic
capacity at the moving-band transition, and measured **−0.527 m/s MEASURED** belt speed plus
**1.825 m MEASURED** downhill travel after **2 s DERIVED**. Service braking remains
**2.36 s MEASURED** from 6 to 1 m/s.

The offline FixedRadii HUD now renders the live detent with the authored gear-table radius:
`STEER I R~165m` for F8 wide and `STEER II R~3m` for F1 tight, blank at released detent. The
field is fixed-width and ASCII-only. Hybrid remains intentionally blank: its instantaneous target
is internal continuous solve state, and duplicating that derivation in UI would make the display a
second drivetrain law rather than a read of authored/live truth.
