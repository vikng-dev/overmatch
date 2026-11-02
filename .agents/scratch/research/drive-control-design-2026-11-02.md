# Drive control from the root: intent, Driver, gearbox (design, 2026-11-02)

**Subject.** How the game turns W/S/A/D into a Tiger's Driver working its gearbox. Where this note
touches crew, its subject is the game's damage model: a Driver station whose crew member is out
of action.

**Labels.** **(M)** MEASURED by a source or a lane run; **(D)** DERIVED, with inputs shown or read
from code; **(R)** RECALLED. Number jobs: Fact, Unit, Taste, Resolution, StandIn.

**Inputs.** [RES] `research/drive-control-2026-11-02.md`; [INV] `codex/inventory-drive-control.md`;
[DIAG] `research/diag-soft-launch-report.md`. Source keys are as in [RES].

**Tiger arithmetic used throughout** (D, from `tiger_1.tank.ron`):

| Gear | Force factor G/r_s (per m) | Force at the 1850 N·m peak | Speed at governed |
|---|---|---|---|
| F1 | 404 | 747 kN | 0.648 m/s |
| F2 | 263 | 487 kN | 0.995 m/s |
| F3 | 182 | 337 kN | 1.435 m/s |
| F4 | 123 | 227 kN | 2.130 m/s |
| F6 | 54 | 100 kN | 4.838 m/s |
| F7 | 37 | 69 kN | 7.060 m/s |

| Quantity | Value (D) |
|---|---|
| Weight W | 559 kN |
| 20° slope share | 191 kN |
| Speed one shift's 0.31 s cut loses on 20° | 1.04 m/s |
| Coupled mass (hull plus belt inertia) | 66.5 t |
| Grip on flat concrete, μ 0.9 | 503 kN |
| Grip on sand, μ 0.4 | 224 kN |

## Decision

**Settled by the owner (2026-11-02):**

- Intent is a signed share of engine power.
- One generic Driver law each for throttle, gear, clutch and brakes. The gear law: the permitted
  gear with the most spare force at its landing speed and current throttle, against the perceived
  slope, ground and steering load.
- Hill hold is the brake rule.
- No brake key. S at speed means a full stop, a 0.5 s hold, then reverse.
- Taps form a signed 25 % lever, and a hold clears the notch on release.
- Held for later: the brakes when the Driver station is knocked out.

**This note's calls:**

- **Keep every drivetrain law.** The four Driver laws (§2) replace the scheduler, the hill-hold
  latch, the hold blend and the swap gate.
- **The owner's gear law, with three amendments,** each forced by a derived failure of the literal
  law:
  - **C1:** a lower gear is permitted only on a deficit or when the engine is lugging, because
    the literal law hunts F1↔F2.
  - **C2:** on overrun the Driver holds its gear, because "most spare force" there means least
    engine braking.
  - **C3:** from rest the Driver launches in the gear that can hold the most speed, because the literal
    law traps a Tiger in F1 on a 20° face.
- **The brake rule is a lever that carries only the shortfall against rollback,** so "release
  once the gear outpulls the slope" happens by force balance and never by a decision.
- **With nobody at the Driver station, the Driver policy does no work.** Controls return to
  or retain their construction rests. The clutch springs engaged after any already-paid cut,
  including at rest, so the tank receives motoring drag instead of freewheeling.
- **Taps:** a press heads for the next notch for its first 0.15 s and becomes a hold only after
  that, so a tap that steps down never flashes the brakes.
- **Size of the change.** State goes from 17 fields to 8 (D); 16 of the 33 module constants go
  (D). One number is new: the hysteresis M (Resolution, set by measurement).

## 1. The root model

| Layer | Owns | Obeys |
|---|---|---|
| Command (client) | keys → notch lever + hold override | §4 |
| Shaping (server) | the pedal's travel | rise rate 4/s; snap through zero on reversal |
| Driver (a crew station's policy) | throttle, gear, clutch, brakes | four laws (§2); sees only what a driver sees |
| Drivetrain (construction, data) | engine, governor, crank, clutch, box, L600, brakes | the laws below, all already in `transmission.rs` |
| Ground | grip, rolling resistance, slope | the contact law (unchanged) |

**Physical facts the drivetrain obeys:**

- **Engine.**
  - A full-load curve.
  - A governor cut.
  - Motoring drag that rises with speed.
  - Crank inertia.
  - **One fuel rack:** the engine never exceeds its full-load torque. Today it does: see "Code
    findings" in §2.
- **Clutch.**
  - It carries torque up to its capacity (Unknown).
  - Slip makes heat, which is not modelled (StandIn). The Fibel's start-in-1–4 rule stands in
    for it.
- **Box.**
  - Every shift is a window with no drive (0.31 s, Unknown).
  - The Fibel's allowed transitions (M [F pp.40–41]): start in 1–4; skip 1–2 gears; reverse in
    1–4; the direction lever is locked above 4th.
- **Steering.** Two radii per gear.
- **Brakes.** 0.343 g (D), consistent with the Fibel's 0.36 g (D [F p.34]). Static breakaway is
  1.5× (Unknown).

**What the Driver observes:**

- the tachometer (crank ω_e);
- the speed (mean belt speed);
- the commanded intent: the notch or hold value, **not** the ramping pedal, so a 0.25 s ramp
  never decides anything;
- the steer detent;
- the perceived load, below.

**The perceived load,** L = W·sin θ + c_rr·W·cos θ + M_r / R_g:

- θ is the slope of the support under the tracks.
- c_rr comes from ground data.
- M_r / R_g is the turn's resisting moment over gear g's radius in the current detent. M_r takes
  the uniform-pressure form μ_t·W·ℓ/4 (R).
- No term carries the hull's acceleration, ride or recoil. Those terms are why today's box needs
  an EMA, two confirm counters, a dwell and a deep-deficit override [INV §3].

**Intent u ∈ [−1, 1]** is the fuel-rack share: the share of full-load torque at the current rpm,
which is the share of the power available there. Why not the alternatives:

- **A speed wish** needs a per-tank speed scale and hides lugging and the drive cut.
- **A force demand** is identical at full intent and adds a layer.

## 2. Driver architectures

| | **A. Shift map** | **B. Capability law (owner's, amended)** | **C. Engine-speed band** | **D. Fibel crew model** |
|---|---|---|---|---|
| Core | Up and down lines over speed and pedal, with hysteresis and anti-hunt offsets [US5938711] | Price each permitted gear with the drivetrain's own force law at its landing, against L | Keep the crank in a band from the curve (peak torque to governed) | Execute the manual's rules with a reaction time |
| Numbers (job) | Lines × gears × pedal points per tank (Taste, plus a StandIn for the missing load law) | M (Resolution); the Fibel's limits (Fact) | Band DERIVED; landing margin (StandIn) | Start 1–4, skip 1–2 (Fact); latency (Taste); soft-ground classes (StandIn) |
| Launch and hill start | A fixed gear; a timed hold (Eaton's 3 s, M [E]) | Preselect the gear that holds the most speed; controlled slip; one-way lever | Slip at the band floor; a separate hold | "1–4 if suitable"; the Fibel is silent on hills |
| Soft ground | Blind to load | Refuses F7 on sand for the true reason, −6.0 kN at its landing (M [DIAG]) | A landing below the band fires a downshift: today's F1 hold [DIAG] | "Do not shift in swamps": a rule by ground kind |
| Main risk | Numbers doing two jobs; a retune per tank | Gap between priced and delivered force | Today's defects return | Gaps filled with tuned numbers |

**Recommendation: B.** It is the only candidate whose decision and delivery read one model; the
tangle in [INV §3] is three estimators disagreeing. A is what industry builds when the controller
cannot measure load, and this sim can. D's reasons are right, and B prices them: the swamp rule
exists because a cut loses speed against high resistance.

### The four Driver laws

Two shared quantities first.

**Deliverable force, F_g(v, u).**

- It is `engine_torques` at command u, at the crank rpm that gear g gives at speed v. While the
  clutch slips, it is taken at the launch rpm instead.
- It is capped by clutch capacity and by grip μN, then multiplied by G_g / r_s.
- It is the same function the crank already runs, so the force chosen is the force delivered.

**Spare force** = F_g(v_land, u) − L, with v_land = v − L·t_cut / M_c. It is closed form, with no
shear decay.

**Gear law.**

- *The choice:* the permitted gear with the most spare force, "more" meaning more by M.
- *Permitted* means four things: within the box's step limit; landing at or below the ceiling;
  no upshift inside a detent turn, because the radius changes with the gear (M [F p.41]); and
  the amendments below.
- **C1, hysteresis.** A lower gear is permitted only when the current gear loses at the commanded
  throttle (spare < −M), or when the tachometer reads below the curve's first point.
  - *Why:* the literal law hunts. On concrete, F1 runs out at 0.648 m/s, and F2 lands at 0.55 m/s
    with 407 kN. F1 at its own landing, 0.45 m/s, offers 685 kN, which grip caps at 503 kN. So
    F1 wins, regains 0.648 m/s in about 0.03 s, and the loop repeats (D).
- **C2, overrun.** While the commanded throttle cannot overcome the engine's own drag, the Driver
  holds its gear.
  - *Why:* there, "most spare force" means least engine braking. It would upshift on every
    lift-off and walk up the ladder on every descent, which is the form today's module already
    rejects.
- **C3, launch.** From rest, the Driver takes the highest launch gear whose spare force clears M:
  the gear that holds the most speed. The Driver preselects it while parked, priced at full throttle; the
  Fibel preselects before a halt (M [F p.43]).
  - *Why:* the literal law always launches in F1, which has the most force. On 20°, one cut loses
    1.04 m/s, more than F1's whole range, so the tank can never leave F1 and crawls at 0.65 m/s.
  - A tank *arriving* at 4 m/s cascades by the same law F6 → F5 → F4 and climbs at about
    2.05 m/s (D). Speed would depend on how the tank got there, threefold.
  - On flat concrete C3 picks F4, which reaches 2 m/s in about 0.6 s against about 1.2 s through
    four cuts from F1 (D).
  - *Cost:* at the edge of capability the slip is long. F4 on 20°, with 15 kN spare, slips about
    8 s to 1.79 m/s (D). The missing truth there is clutch heat, not a launch rule.

**Clutch law.** Three states, `Out | Slip | In`, replacing `clutch_out`.

- **`Out`** covers paid shift windows and the Driver's released rest. Without control work,
  the clutch springs engaged when any already-paid window expires; Out does not persist at rest.
- **`Slip`** begins under intent with the shaft below idle in a launch gear. It holds the crank at
  ω_launch(u) = idle + u·(peak − idle) until the shaft reaches that speed [US4081065]. This is the
  form the steer rev target already uses: no number.
- **`In`** otherwise. The stall guard (StandIn) and the over-rev guard stay as they are.

**Throttle law.**

- Fuel = max(commanded share, idle catch), never more than 1.
- The idle catch is today's "idle governor" StandIn, reclassified as the Driver's foot keeping a
  lugged engine alive. It leaves with the Driver.
- During a window the box's own rev-match blip runs (the OLVAR's *Gasgeber*).

**Brake law.**

- **At rest the park lever is on.** Under intent it carries only the shortfall against motion
  opposite the intent: the stop-force law, clamped to one side. It is therefore off exactly when
  the gear outpulls the slope. No release decision exists to disagree with the gear choice,
  which is the cause of S2.
- **Motion against intent gets full service brakes.** Then, at rest:
  1. Hold the brakes for 0.5 s (Taste; a candidate player setting).
  2. Change direction for one cut, still held by the lever.
  3. Launch by C3.
- **Zero intent below first gear's idle speed** (0.156 m/s, D): stop and park. This is the
  Fibel's stop, and it replaces the hold blend.
- **At the crank ceiling,** the Driver holds the tachometer there with the service brakes ("never
  over 3000", M [F p.38]), instead of the protective upshift.

**With nobody at the Driver station,** the policy does no work. Construction names each rest:

- the fuel rack returns closed: this construction has no independent idle actuator;
- the clutch springs engaged after an already-paid window expires, including at rest, since
  hydraulic pressure is what opens it (D [RES §1.1]);
- the player's intent lever, the gear and the park lever stay as they were left;
- steering returns to center and the service brakes are released.

The closed-rack rest resolves the proposed idle-fuel conflict: the Driver's catch leaves with
its work, and no construction governor replaces it. Released command axes are zero and an
unpowered clutch is engaged after a paid cut expires. These fixed rests have no authoring field:
there is no adjustable empty-station pedal or single-variant clutch selector. An independent
idle actuator, if modelled later, belongs to construction rather than an unstaffed pedal share.
The engine resists motion in either crank direction, firing only
when turning forward. The clutch slip guard bounds the crank; it does not promise a vehicle-speed
bound on a grade exceeding guarded drag and contact resistance. See the
[lane 5 record](law-drive-5-ceiling-2026-11-02.md) for the measured S3 limit.
MEASURED there: F6 on a ten-degree descent reaches 13.315221 m/s after 20 simulated seconds,
still accelerating (final-second range 12.902819–13.315221 m/s), while the crank stays within
3100 rpm. The slip guard caps crank speed and thus drag; it cannot bound belt speed when grade
force exceeds that drag plus rolling resistance. The owner must decide whether another loss or
changed over-rev contract belongs in the model. No brakes at an empty station or belt-speed clip
are implied. In a lower gear with a root below the guard, S3 now asserts the equilibrium derived
from the authored drag anchor, gear ratio and ground resistance, including a weaker-drag mutant.

**Numbers B needs.**

- M (Resolution): the measured gap between priced and delivered force, plus the observer's spread.
- Launch gears 1–4, a step limit of 3, and direction change at rest (Fact, M).
- t_cut 0.31 s (Unknown, already in the asset).
- The 0.5 s hold (Taste).
- The at-rest speed 0.05 (Resolution, already in code).

**Code findings** (D, read from code; not run):

1. **The fuel rack is counted twice.** `engine_torques` adds the idle catch to the full-pedal
   torque. At the 500 rpm stall guard that gives 1300 + 1300 = 2600 N·m, 41 % over the 1850 N·m
   peak, clamped only by the 2400 N·m clutch.
2. **Chosen and delivered launch force disagree.** `available_force_in_gear` prices a launch at
   1300 N·m, while the bogged clutch delivers 2400.
3. **The cause of the unmanned freewheel.** `update_clutch_latch` takes the clutch out below
   0.4 m/s with no pedal. That is a driver's act, and it runs with nobody at the station.
4. **The steer notches collide with the detent hysteresis.** The 50 % notch sits inside the tight
   detent's 0.45–0.55 band, so its radius depends on history.

## 3. WWII driver vs later automatic

| Truth | Tiger I: OLVAR + L600 [RES §1] | Later: X1100, CD-850, Renk [RES §2] |
|---|---|---|
| Coupling | Friction clutch, eased in, opened hydraulically for each shift | Converter, 2.2:1 at stall (M, X1100), with lock-up |
| Gear choice | Driver-selected from 8F/4R; skip ≤ 2 | A range; output speed and engine load schedule the gear |
| Shift | Every shift cuts the drive | Power shift: drive is kept |
| Reversal | At rest; lever locked above 4th | Inhibited above 1.3–2.2 m/s (D, from 3–5 mph) |
| Idle, in gear | Lever to 0: no creep | Creeps; parking brake in the box |

**For a Tiger, the automation is its Driver:** a crew member working its own semi-automatic box,
not a converter automatic. The player still feels the Tiger: the drive cut and its clunk, the
launch slip, the pause before reversing.

**A later tank selects the other kind through data,** with no per-tank branch:

- `coupling: Clutch { capacity } | Converter { stall_ratio, capacity_factor, lockup }`: the
  module's existing coupling-law slot. With a converter the clutch law has nothing to slip, and
  creep appears, so an M1 with an empty Driver station creeps in D: a truth.
- `shift: { secs, drive: Cut | Kept }`. With `Kept`, v_land = v, and the same gear law gives the
  converter box's smooth climb.
- `launch_gears`, `max_step`, and `direction_change: AtRest | Below(m/s)`.

## 4. Input

The owner settled the core: the signed lever, a press under 0.15 s counting as a tap, a hold
clearing the notch, no brake key, and the notch shown on the drive gauge and in the engine pitch.
This note decides the rest:

- **A press heads for the next notch during its first 0.15 s,** and becomes a hold only after
  that.
  - *Why:* if every press began as a hold, an S tap stepping down from cruise would first command
    −1, a full brake against motion for up to 0.15 s: a jolt with every tap.
  - *Cost:* a hold reaches full in 0.34 s instead of 0.25 s, and an S hold from cruise starts
    braking 0.15 s later (D).
- **Snap through zero:** a held key opposing the current value sets the axis to 0 at once (Unity's
  "Snap" [U]; PhysX [PX]). Notch steps and releases ramp at the rise rate.
- **Where the notch lives:** in the client's command layer, as player input. The command's
  existing bounded axis carries the value, so there is no wire type change. The server shapes it
  and runs the Driver.
- **Steer detent thresholds go between the notches** (Taste): wide on 0.15 / off 0.05, and tight
  on 0.65 / off 0.55. Then 25 % and 50 % always give the wide radius, and 75 % and 100 % the
  tight one.
- **Zero intent:**
  - moving: coast in gear with engine braking;
  - below 0.156 m/s: stop and park;
  - at rest: the park lever holds to 30° on Tiger data (D, in the asset);
  - on a descent: keep rolling, braking only at the ceiling.

  There is no timed hold: Eaton's 3 s [E] is a truck aid, and the lever holds indefinitely.

## 5. The decision tree

Yes accepts this note's answer, and each question's prerequisites come before it. The owner's
settled items are not asked again. Tags: **[source]** settled by a cited fact; **[design]** the
owner's call on merit; **[playtest]** a taste to settle at the controller.

1. Do the drivetrain laws stay, with the Driver's laws running only while someone serves the
   Driver station? [design]
2. Is the engine one fuel rack, so commanded fuel plus the idle catch never exceeds full-load
   torque? [source: construction]
3. Do the Fibel's limits become data (launch gears 1–4, step ≤ 3, direction change at rest),
   deleting `shift_addressing` and the 2300/1150 bands? [source, F pp.40–41]
4. Does the Driver price gears at the commanded value, not at the ramping pedal? [design]
5. Is the clutch `Out` at rest and in windows, a slip at ω_launch(u) during a launch, and `In`
   otherwise? [design; US4081065]
6. C1: are lower gears permitted only on a deficit or a lugging tachometer? [design: the literal
   law hunts]
7. C2: on overrun, does the Driver hold its gear? [design: the literal law sheds engine braking]
8. C3: from rest, launch in the highest launch gear whose spare clears M, preselected while
   parked? [design: the literal law traps F1 on 20°; playtest the long slip at the edge]
9. Is M a Resolution number, sized from the measured gap between priced and delivered force?
   [design]
10. Is the release rule a lever that carries only the shortfall against rollback? [design]
11. At the crank ceiling, does the Driver brake instead of upshifting? [source F p.38; design]
12. With nobody at the Driver station, do the controls rest by construction? And may a
    back-driven, engaged crank turn backward, resisting only with motoring drag? [design; D from
    RES §1.1]
13. Does a press head for the next notch for 0.15 s before it becomes a hold? [playtest]
14. Do the steer detent thresholds sit between the notches? [playtest]
15. Are later automatics data families (coupling, drive kept or cut, direction rule)? [design]

## 6. Migration

Each lane bumps the wire REV deliberately. One timeline is unaffected: decisions run on the
server, and `TankTransmission` replicates to the owner only. The four measured symptoms:

- **S1:** the from-rest first-gear hold on soft ground.
- **S2:** hill hold stepping F3→F2→F1 on 20°.
- **S3:** the tank with an empty Driver station freewheeling down 10°.
- **S4:** steady states that depend on how the tank arrived.

**Lane 0: input.** The client's notch lever and hold override, plus the snap in
`drive::shape_drive`.

- *Proof:* taps step +50 → +25 → 0 → −25. A hold's release clears to 0. A stepping-down tap never
  sends a value opposing the motion. A reversal hold snaps on its first tick.

**Lane 1: the perceived load.** An instrument; it deletes nothing.

- *Change:* add L to `TransmissionInput`, printed beside `demand_n`.
- *Proof:* on the 10/20/30° course, L matches W·sin θ + c_rr·W·cos θ within its spread. In the
  fire-backward-uphill probe, the shot moves L by less than M.
- *Also measures:* the gap between priced and delivered force after each landing. That sets M.

**Lane 2: clutch and fuel.**

- *Change:* one fuel rack; `ClutchState`; the launch slip, which is the stall guard's formula with
  its floor raised to ω_launch(u).
- *Deletes:* `clutch_out`, `CLUTCH_OUT_M_SPEED`, `CLUTCH_IN_M_SPEED`.
- *Proof:* a steady slip transmits τ_free(ω_launch) and never more than the curve. Launch times on
  flat ground and sand are recorded before and after.

**Lane 3: the gear law, with C1–C3.**

- *Deletes:*
  - the arms of `run_shift_decision`, `select_grade_target`, and the landing decay;
  - the demand EMA, both confirm counters and the dwell;
  - `grade_target`, crank corroboration and the protective rescue;
  - `POSTSHIFT_MARGIN_RPM`, the duplicate `GRAVITY_M_PER_S2`, and the asset's bands and
    `shift_addressing`.

  The hill-hold latch calls the new launch pick until lane 4.
- *Proof for S1:* remove the `#[ignore]` from
  `a_tank_leaves_first_gear_from_rest_on_a_soft_ground`. Expected (D): sand launches in F4, with
  grip capping F1–F4 at 224 kN, then climbs F5 → F6. F7 is refused at −6 kN (M [DIAG]).
- *Proof for S4:* `contact_law_shipped_ground_speeds` asserts, per ground and entry, that the final
  gear is sustainable and the next gear's landed spare is below M: the class invariant of the
  unstable equilibrium.
  - Expected for sand (D): it stays split, F6 from rest and F7 rolling. Both ends satisfy the
    invariant, so the split is a vehicle truth.
  - Expected for dirt (D): it converges on F8 if M < 8.0 kN (M [DIAG]).
- *Proof for the shot:* the fire-backward-uphill probe shows no shift on the shot.

**Lane 4: the brake law.**

- *Change:* the one-way lever; full brakes against motion; the 0.5 s hold, a new countdown field;
  direction change at rest; the 0.156 m/s stop.
- *Deletes:*
  - `hill_hold` and its update, refresh and release functions;
  - `HOLD_REENGAGE_TICKS`, `HILL_HOLD_ENGAGE_SPEED` and `DIRECTION_SWAP_SPEED`;
  - the hold blend and its `SLIP_SATURATION` use;
  - the back-driven arm.

  `HillHold` and `GradeLimit` become states derived for the HUD.
- *Proof for S2:* the 20° test runs at the derived 4,750 kg belt; the 16,000 kg fixture is deleted.
  The tank launches in the C3 gear with at most one cut, and no belt moves against intent faster
  than the at-rest speed. Expected (D): F4, with 36 kN spare over the fixture's 191 kN.
- *Also asserts:* S released inside the 0.5 s hold leaves the tank parked. S held through it
  reverses after exactly one cut.

**Lane 5: the ceiling and the empty station.**

- *Change:* the brake at the ceiling; the controls' construction rest; a crank that turns backward
  through an engaged clutch, firing nothing and resisting with motoring drag.
- *Deletes:* the protective upshift, `CRANK_CORROBORATION_MARGIN_RPM`, and the static projection.
- *Proof for S3:* with the Driver station's crew member out of action, at 5 m/s on 10°, the clutch
  never goes out. Speed on the descent, and on the rollback after an uphill coast, is bounded by
  the gear's motoring drag or by the over-rev guard, as measured.
  **Acceptance status:** the lane 5 record certifies clutch engagement and the crank bound,
  but the F6 vehicle-speed bound is unmet; the record names the construction limit.

**Where it ends** (D): 8 fields remain: `gear`, `shift_ticks`, `steer_step`, `reverse`, `park`,
`omega_e`, `clutch` and `reversal_hold_ticks`.

- **Remain StandIn:** the idle catch, `GOVERNOR_CUT_RPM`, `STALL_GUARD_BAND_RPM` and
  `MOTORING_DRAG_BASE_SHARE`.
- **Remain Resolution:** `TICK_HZ`, `DEAD`, `PARK_ENGAGE_SPEED` and `REV_MATCH_BAND_RPM`.
- **Remain Taste:** the detent and neutral-turn thresholds.
