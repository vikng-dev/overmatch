# Belt tick investigation

Subject: the video game's tank driving physics, game state only.

Base: `7c0156e309f2134605b21bd877021dc733be5d5f` (the bay's `main` at dispatch).
Branch: `lane/belt-tick`. No push, merge, external comment, or performance measurement.

## Disposition

This is a **partial implementation**. The independently reproduced control defect in #373 is
fixed at the no-pedal clutch decision after reproducing the round-1 missed entry. The round-2
no-request rollback regression is also reproduced and fixed at the existing park latch. The shared
contact-solver defect behind #347 and #353 is diagnosed and remains in production. #312's sustained rigid-contact limit cycle was not reproduced in the wall case.
Do not treat this lane as completion of the driving-correctness prerequisite for optimization.

There are two demonstrated mechanisms (DERIVED from the paths below, with separate witnesses):

| Issue | Evidence | Result of this change |
|---|---|---|
| #373 | A drivetrain-only reversal cycles through rest, or powers rollback when it skips rest. Steering-only rollback can also cycle across the neutral threshold. No solver contacts participate. | The no-pedal clutch opens against the engaged ladder; class tests cover both ladder directions and sampled/skipped rest. |
| #347 | The shipped headless slab pull alternates delivered force with the clutch Out, Slip, and In. The next tick's charge equals the previous tick's delivery. | Remains reproduced; before/after observations agree in round 1. Not re-measured in round 2. |
| #353 | The delayed billing path is present and exercised by #347. Its parked launch/sign-alternation symptom was not reproduced in the current parked probe's flat, rammed, house, and yard arrivals. | Shared root remains. These arrivals do not replace the original issue's eight poses. |
| #312 | The historical rigid belt-lane wall pull had transient force reversals, without a sustained alternating-sign cycle. | Historical observation only; the case is removed. The #353 lane owns any wall re-measurement. |

## Exact tick boundary

The contact paths retain their base line numbers. The control changes are in
`update_clutch_state` and `update_park_latch`; `requests_other_ladder` also unifies
direction-request admission and nonpropulsive control at their shared inclusive input threshold. Drivetrain references
in the control diagnosis below use symbols.

1. `src/track/sim.rs:557` installs driving in `FixedUpdate`. Avian's default schedule runs in
   `FixedPostUpdate` (`vendor/avian3d-0.8.0-overmatch/src/schedule/mod.rs:54`). Driving therefore
   reads the contact transfer store before this tick's physics replaces it.
2. `src/track/stop_contacts.rs:210` writes a belt's speed into the manifold's relative surface
   velocity. That speed is prescribed throughout the solver's substeps; the solver has no belt
   degree of freedom to change it.
3. `vendor/avian3d-0.8.0-overmatch/src/dynamics/solver/contact/mod.rs:312` solves tangential
   impulses in Relax, following that pass's normal solve. `contact/tangent_part.rs:211` adds
   surface velocity to the relative velocity. Its effective mass uses body response alone
   (`:221`), and its accumulated friction impulse is bounded by the current normal impulse
   (`:189`, `:238`). The missing inertia is the belt's, not a delayed normal load in Avian.
4. `src/contact_transfer.rs:774` records the delivered substep impulse after Relax. At the next
   driving pass, `src/track/sim.rs:1290` snapshots those completed-step transfers.
5. `src/track/stop_contacts.rs:308` projects delivered friction into `HardReaction.belt` and divides
   by the completed step's duration. `src/track/sim.rs:858` adds it to the next pass's belt reaction.
6. `src/track/sim.rs:818` puts that reaction in the drivetrain load. The reaction slope on the
   following line contains sampled support only; the added hard reaction has no implicit slope.
   `src/track/transmission.rs:1338` copies the load into `BeltTick`; `:1345` consumes it when
   integrating the belt.

The Driver also reads completed hard friction and grip as observations
(`src/track/driver.rs:347`). That historical sensing has a different job from charging the
physical belt a tick later. Removing the bill must preserve the Driver's observation.

DERIVED: with an unconstrained belt of effective inertia `I`, a contact's body effective mass
`m`, and slip `s`, the fixed-surface solver delivers `p = m s`. Its body then matches the old
surface speed. Charging the belt `-p/I` next tick leaves `s_next = -(m/I) s`. This is the
alternating feedback, even with an otherwise correct Coulomb bound. Finite clutch or brake
response changes its gain; a wedged contact can also increase the normal load. A sign change
of friction following the sign change this delayed bill creates does not establish an Avian
friction-sign bug.

The sampled channel is different: `belt_reactions` uses this pass's predicted hull motion and
reaction slope, and `BeltTick` includes that slope in its implicit mobility. No previous solver
impulse is needed there.

## The separate control feedback

On the base, `update_park_latch` released park whenever the request agreed with belt motion,
regardless of the engaged ladder. The original no-pedal clutch rule then coupled any moving
mean outside neutral, including rollback against that ladder:

- `swap_ladder_direction` refuses the cut while the belts move, so the old ladder remains.
- `driver` makes an opposite-ladder request nonpropulsive.
- `update_clutch_state` opened at park/rest but closed on rollback once park released.
- The idle catch enters `crank_free_torque`; `clutch_coupling` can transmit it through the old
  ladder and arrest motion the brakes alone cannot hold.
- At rest the clutch opens again. The load restarts the slide, then the same catch stops it.

Round 1 retained park during an opposite-ladder request. That prevented the repeated release
but did not fix the no-pedal clutch rule: an entry that skipped the rest band never set park.
It kept transmitting the catch against rollback while the latched path stayed clutch Out.
The final policy fixes that clutch decision and removes the redundant round-1 park predicate.

This is a decision made from a velocity that the decision itself changes. The brake-capacity
selector `brake_capacity_for_regime` also reads pre-step speed to choose static capacity, but
the witness exceeds **both** capacities. The selector therefore cannot explain its repeated
stops. No separate static-capacity defect is claimed or changed here.

Launch `engagement` also reads prior clutch regime and shaft speed, and changes the
crank floor used by the coupling solve. The actual lock feasibility is checked by
`settle_crank`, independently of that launch-control label. This investigation does not
establish a third defect in that launch policy.

## Reproduction evidence

All values in this section are MEASURED. Fixture loads, durations, and initial speeds come from
inline test data; no observed output becomes a pass condition.

In round 1, before any production edit, this command failed (the test is consolidated below):

```sh
scripts/serial-cargo test --lib --features dev_ui,bitprobe \
  an_over_capacity_slide_does_not_cycle_through_rest -- --nocapture
```

The witness uses a per-side load equal to the fixture's static brake capacity times a fixture
factor of `1.5`, with an opposite direction request. The relevant arithmetic trace was:

| Tick | Input belt speed (m/s) | Output belt speed (m/s) | Park after selection | Clutch |
|---|---:|---:|---|---|
| 0 | 2 | 0 | false | In |
| 1 | 0 | -0.2368421 | true | Out |
| 2 | -0.2368421 | 0 | false | In |

The test failed at tick 2: `over-capacity slide returned to rest`. These are drivetrain-fixture
speeds, not measurements of a hull on rendered terrain.

### Round-2 reproduction and policy check

The independent review's skipped-rest entry is real. Under the lab fixture's per-side load of
2 times static capacity, the round-1 code gave:

| Entry / tick | Input mean (m/s) | Output mean (m/s) | Park | Clutch | Catch transmitted |
|---|---:|---:|---|---|---|
| skipped-rest / 0 | 0.100000 | -0.178717 | false | In | yes |
| skipped-rest / 1 | -0.178717 | -0.520188 | false | In | yes |
| latched / 0 | 0 | -0.375000 | true | Out | no |
| latched / 1 | -0.375000 | -0.890625 | true | Out | no |

The stopped pause is initialized in both entries, so the latched entry cannot immediately
swap ladders. Across the 5-second observation the skipped entry kept clutch In while the
latched entry kept it Out. The review's approximate 0.05–0.23 m/s entry window is supported
by the measured 0.1 m/s witness. No sustained rest alternation was seen on that skipped path;
the defect is the different continuing control regime, not another demonstrated cycle.

After the root fix, skipped-rest tick 1 lands at -0.694342 m/s with clutch Out and no catch;
latched tick 1 still lands at -0.890625 m/s, now with park released and clutch Out. Both use
service braking thereafter. Their history-dependent velocity offsets are physical; the
common continuing regime and acceleration, not equal sampled velocities, are the invariant.

The strengthened `pending_reversal_does_not_power_rollback` failed on round 1 at entry 0.1,
tick 1: `old ladder powered rollback`. Its final class fixture bounds the load from maximum
clutch pull, brake capacity, inertia and `PARK_ENGAGE_SPEED`, guaranteeing a skipped-rest entry
without using a solved output as an oracle. It compares latched and skipped entries at that
same load, and retains the original modest-overload moving entry. Both ladder directions
must refuse cuts, never power rollback and never return an over-capacity slide to rest.
Its duration uses `TICK_HZ`.

Steering-only rollback is the same root. Fixture: no throttle, full steer, mean initially
-0.95 times `neutral_speed_m_s`, per-side load 1.5 times static capacity. In one class member,
round 1 produced the following mean inputs and selected states:

| Tick | Mean (m/s) | Clutch | Catch transmitted |
|---|---:|---|---|
| 0 | -0.475000 | Out | no |
| 1 | -0.869737 | In | yes |
| 2 | -0.090519 | Out | no |
| 3 | -0.485256 | Out | no |
| 4 | -0.879992 | In | yes |

It crossed the neutral threshold 230 times in 5 seconds, including 92 times in the final
2 seconds, with catch on 115 ticks. After the fix all three class members leave neutral once,
with no catch on any tick. `steering_rollback_does_not_close_the_main_clutch` failed before
the fix (`In` rather than `Out`) and passes afterwards in both ladder directions.

The same steering fixture rolling **along** the engaged ladder reported no driving ticks in
that run. This is an observation, not a universal braking guarantee: below gear idle the catch
can supply forward torque through an engaged clutch. One member crosses the neutral threshold
27 times before settling; no member crosses in the final 2 seconds. These observations are identical
before and after the root fix. This is the along-gear neutral boundary; no sustained cycle is
established in that direction and no additional policy is changed here.

The review was also right about `a_direction_request_holds_rest_until_its_ladder_is_engaged`:
with only the round-1 park predicate removed (the base production behavior), it still passed.
In that same run the old over-capacity regression failed at tick 2. The rest-only test and
the duplicate over-capacity test are removed; the strengthened test above owns their relevant
reversal evidence. `reversal_pause_and_cut_hold_a_grade_in_both_directions` retains the
within-capacity pause/cut hold invariant. The old crank-floor test's no-pedal backslide arm
required the retired coupling policy; it now exercises already-coupled **powered rollback**,
retaining its floor, bounded torque and forward-pull assertions.

Commands for the red/green control checks:

```sh
scripts/serial-cargo test --lib --features dev_ui,bitprobe -- \
  pending_reversal_does_not_power_rollback \
  steering_rollback_does_not_close_the_main_clutch
scripts/serial-cargo test --lib --features dev_ui,bitprobe -- \
  track::transmission track::sim tests::doc_citations
```

Temporary observation harness and logs are `/tmp/belt-tick-round2-policy-trace.rs` and
`/tmp/belt-tick-round2-{before,red,base,after}.log`. The harness is removed from the tree;
its values above are observations, never pass conditions. All round-2 behavior observations
are arithmetic drivetrain fixtures, not rendered play or timings.

### Round-3 no-request rollback reproduction

The review's P1 is confirmed on `e56b22ab` production code, before the park edit. The new
`released_rollback_stops_and_parks` fixture was first run with staffed controls, no input, park
off, first gear, and rollback above first-gear idle. Per-side downhill load is half the dynamic
brake capacity. The original test failed on its first per-tick park assertion,
`no request against gear must set park: shoved`, rather than at the runaway outcome:

```sh
scripts/serial-cargo test --lib --features dev_ui,bitprobe \
  released_rollback_stops_and_parks -- --nocapture
```

A separate observation recorded this MEASURED lab trace: entry -1.4814816 m/s, tick 0
-1.598669 m/s, tick 127 -16.481482 m/s.
Park stayed off, clutch Out, no driving. DERIVED: the 60 kN per-side load on 8,000 kg belts
adds -0.1171875 m/s per tick without any braking, matching the trace. This confirms the review's
mechanism; no hand-worked P1 claim was disproved.

In round 3, the test exercised three entry routes on all three constructions and
both ladders: an external shove into rollback, releasing the opposite key during an over-capacity
slide, and releasing steering after a pivot slides beyond neutral. The key-release entries first
run the actual held request and overload, then release. Each route branches to a load inside
dynamic capacity and one beyond even static capacity. The shove fixture specifies resulting
velocity; it does not claim collision integration.

MEASURED in round 3: all 18 within-capacity entries reach a two-sided brake-held rest, first held at tick
indices 1–15, and stay held through the observation. All 18 overloaded entries keep sliding with
park set, clutch Out and no driving. Held speeds end within 5.4e-9 m/s of zero; the test reads the
brake's held result and the owner's rest band rather than asserting floating cancellation is
bit-exact zero. That release phase flattened the belt difference every tick; the state
enumeration below carries both outputs into the next tick. These outputs are observations,
never numeric pass conditions.

`neutral_pivot_keeps_its_mean_hold_as_the_only_stop` checks both ladders and both construction
choices (`Held`/`Free`): mean drift against the gear does not set park, service or one-way brakes,
and only `Held` supplies a mean stop. `released_motion_coasts_without_brake_force` still proves
that a coast along the gear stays unbraked. The policy and rejected alternatives are owned by the
design doc linked below. #392 also owns the Free-mean threshold behavior measured below; no
hull-coupled reproduction of its below-idle reversal claim is made here.

### Round-1 shipped-app observations (not re-measured in round 2)

The shipped-app slab probe was run before the round-1 production edit and again after it:

```sh
OVERMATCH_UNATTENDED=1 BELT_FRICTION_ONLY=slab-pull \
  BELT_FRICTION_TAG=belt-tick-before \
  scripts/serial-cargo test --lib --features dev_ui,bitprobe \
  belt_friction_probe -- --ignored --nocapture
```

The after command uses tag `belt-tick-after`. Both commands pass as observation probes and
report the following same values; their passing status does not mean the defect is fixed.

| Clutch | Sample ticks | Left/right force-sign flips | Left/right force alternation amplitude (kN) | Left/right belt alternation amplitude (m/s) |
|---|---:|---|---|---|
| Out | 20 | 19 / 17 | 166.990 / 168.197 | 0.28047 / 0.28047 |
| Slip | 55 | 28 / 29 | 90.387 / 92.166 | 0.13311 / 0.13528 |
| In | 181 | 150 / 146 | 129.968 / 127.367 | 0.10495 / 0.10495 |

The invoice boundary is visible without inferring it from a phase plot. Left-side readings
from `belt-tick-before-slab-pull.csv`, rounded by the existing probe:

| Tick | Delivered this step (N) | Charged to the drivetrain this tick (N) |
|---|---:|---:|
| 97 | 195640 | -61633 |
| 98 | -212758 | 195640 |
| 99 | 162464 | -212758 |
| 100 | -163570 | 162464 |
| 101 | 164442 | -163570 |
| 102 | -166470 | 164442 |

The historical `rigid-wall-pull` case was run at `f3cae9f3` after the round-1 control edit.
It requested forward drive on the forward ladder throughout and never exercised that edit
(DERIVED). The case and its copied wall geometry are removed in round 2; the loose-piece probe
keeps its original subject. The #353 lane owns any future wall fixture and re-measurement. It uses the scorecard's existing wall
dimensions, once per belt lane so the hull nose stays clear. The full-width wall attempt did not
reach both belts and was rejected by the probe's contact-premise assertion. With both lanes, the observation probe
passes and both belts have solver load. There are no loose pieces or soft-run normal laws in
this contact. At `f3cae9f3` it ran with `BELT_FRICTION_ONLY=rigid-wall` in the command above;
that selector has no matching case in the current tree. Over its fixture
drive, Out has zero left/right sign flips, Slip has three/two, and In has zero/zero. The last
window does not show a sustained alternating-sign cycle: the final left belt stays near
`1 m/s`, and its friction remains positive. Transient reversals and a nonzero alternation
statistic across a moving trajectory do not prove #312's limit cycle.

The existing parked probe was also run before and after the production edit:

```sh
OVERMATCH_UNATTENDED=1 RUBBLE_CLIMB_TAG=belt-tick-before \
  scripts/serial-cargo test --lib --features dev_ui,bitprobe \
  rubble_park_probe -- --ignored --nocapture
```

The after command uses tag `belt-tick-after`. The no-input fixture runs for 25 s, with its final
20 s as the observation window. Every arrival sleeps through all 1280 ticks of that window,
with zero reaction-sign flips on both belts, before and after. No launch was seen.

| Arrival | Travel over the whole no-input interval (m), before and after |
|---|---:|
| flat | 0.00315286 |
| rammed | 0.05554761 |
| house | 0.00221066 |
| yard | 0.00372175 |

The maximum absolute belt speed in the final window is at floating cancellation scale on flat
ground (`1.19e-12 m/s` rounded upward), zero on the other arrivals. Stored hull velocities in a
sleeping body are not measured creep: sleep stops its pose integration. Travel above includes
the initial settling interval. These observations neither reproduce the original eight-pose
launch experiment nor rule out an active wedged pose.

## Fix, scope, and remaining work

The current Driver policy and rejected alternatives live in
[transmission-design.md](../../docs/design/track-model/transmission-design.md#released-motion-policy).
`update_clutch_state` owns no-pedal coupling; `update_park_latch` owns released-motion parking.
The round-1 opposing-ladder park predicate stays removed. No new threshold, timer, latch, force,
or authored data is introduced; existing dynamic/static brake capacities remain the limits.

Observations for the owner to judge by feel (MEASURED in the fixtures unless marked DERIVED):

| Case | What a player would see under this policy |
|---|---|
| Within static capacity, starting at rest | The reversal pause and paid cut hold rest, then launch in the requested direction. The existing hold test passes before and after. A moving entry also needs the dynamic brakes to arrest it. |
| Beyond static capacity, holding the opposite direction | Rollback continues with bounded service braking, clutch Out and no old-ladder catch. It no longer judders through rest, and a skipped-rest entry no longer produces a different coupled regime. Park releases when rollback agrees with the request; it does not flicker every tick. The cut waits for actual rest. |
| Between dynamic and static limits during the reversal pause | Undisturbed rest holds. A fixture nudge of -0.1 m/s at tick 8 formerly returned to rest (within rounding) in all three members. It now continues sliding: the next means are -0.170313, -0.178947 and -0.152083 m/s. The dynamic brakes cannot recover rest, so the direction cut remains pending until the load eases or outside motion arrests it. |
| Hill start | Propulsive pedal still launches through the one-way lever without rollback. Before/after fixture launch gears, final speeds and cut counts agree in the one comparison; no old output is pinned. |
| Pivot | Within dynamic capacity, a Held mean stops and pivots; only a neutral pivot keeps park and per-side brakes off. Outside neutral, held steering brakes rollback too. Between dynamic and static capacity, a moving Held mean cannot regain rest, although undisturbed rest holds. Beyond static capacity even rest breaks away. A Free mean has no stop inside neutral; its threshold behavior is measured below. Along-gear steering retains its neutral/geared transition. |
| No throttle, rolling against the gear on a hill | Outside a neutral pivot, dynamic brakes arrest motion within capacity and park holds rest. Between dynamic and static capacity a moving entry keeps sliding; undisturbed park holds. Beyond static capacity rest also breaks away. On `e56b22ab`, release with no steering left the vehicle unbraked inside capacity; on `3b3f338e`, held steering outside neutral still did. |
| Shoved backward while coasting | The same capacity-limited stop applies without a key press. A load between dynamic and static capacity cannot recover a moving entry. The fixture models velocity, not a collision. |
| Reversal key released during an overloaded slide | Release applies park. A load below dynamic capacity permits stopping; a load between dynamic and static capacity or above static capacity keeps the moving entry sliding without an old-gear catch. The held reversal's pause/cut behavior is unchanged. |
| Engine braking downhill along the gear | DERIVED: unchanged; the no-request rollback clause is false along the gear above the parking threshold. The existing coupling and unbraked-coast witnesses pass. Below gear idle the catch may power motion; #392 owns that pre-existing interaction. |

No authored data was tuned to preserve an earlier output. Rendered slope feel remains for the
owner; the slab and parked observations above are historical round-1 evidence, not round-4
validation of contact behavior.

The contact defect is a separate solver integration change, left unfinished in this lane:

- #347: replace delayed physical billing with a current-substep joint solve. Its acceptance
  remains open; the slab force alternation is still present.
- #353: the joint solve must carry actual belt and crank inertia, the shared brake/clutch
  budget across all manifolds, contact directions from the outline, and both tangential axes.
  Normal load must be read from the current substep. Remove `HardReaction.belt` and its
  physical read-back, and remove the exact-zero compensation in `sim.rs:1018` when the held
  constraint itself supplies exact rest. Retain hard-contact sensing and structural transfer
  reporting. The historical parked poses still need a matching reproduction and rerun.
- #312: no independent fix is justified by this wall run. Recheck the rigid observation with
  the shared solve; the current evidence supports only the shared billing path, not a
  separately reproduced limit cycle.
- #373: the reproduced reversal and steering-rollback clutch root is fixed across the class
  fixtures. Round 1 alone was incomplete. The owner's slope/pivot feel judgment and any
  different entry beyond those tested remain observations to assess, not discharged by this report.
- #392: below-idle catch opposing a reversal, the along-gear neutral boundary, and
  free launch-gear selection while rolling against the ladder remain open. The no-throttle
  Free-mean on/off brake boundary is also this issue's class; see the round-4 trace below.
  The review's hull-coupled P2-2 scenario is not established by these arithmetic fixtures.

The shared solve belongs game-side, through Avian's public schedules and solver-body state,
as the #353 comment and #358 prototype establish. A belt and its drivetrain are game degrees
of freedom; extending the engine's prescribed conveyor surface into that model is a game
divergence, not an upstream friction bug. No fork file changes or upstream draft are included.
The existing prototype's fixed anchor and single-manifold evidence does not discharge the
moving-crank and shared-budget requirements. A frozen effective belt mobility or a later
settlement of its impulse would preserve the rejected estimate-of-the-other-side approach.

### Round-4 held-steer reproduction and state enumeration

Before the production edit, the added held-steer entry in `released_rollback_stops_and_parks`
failed on `3b3f338e` with `within-capacity rollback must slow: steering held`. The per-tick
park assertion had already been removed, and both belt speeds were carried into each next tick.
Command:

```sh
scripts/serial-cargo test --lib --features dev_ui,bitprobe \
  released_rollback_stops_and_parks -- --nocapture
```

This confirms the review's last unbraked route. The selected exemption is `neutral_pivot`, not
all steering. The design doc owns the one policy sentence and HUD implications. No band, timer
or new latch hides the neutral boundary.

The final fixture enumerates states, not entry routes: each class construction, both ladders,
both mean constructions, no/along/against throttle, no/held steering, mean inside/outside neutral,
motion along/against the engaged gear, and loads below dynamic, between dynamic and static, and
above static capacity. Brake-only force is checked independently of powered thrust: a powered
request can add engine force, so its total acceleration is not a brake-capacity oracle.
Released rollback trajectories carry both belt outputs and assert stopping/holding or a
capacity-limited slide. Park is asserted at rest, never on every moving tick.

DERIVED coverage: 864 state entries = 3 class drivetrains × 2 mean constructions × 2 ladders ×
3 throttle requests × 2 steering requests × 2 neutral-window positions × 2 motion directions ×
3 loads. The brake-only solve checks every entry; 144 no-throttle rollback trajectories carry
both belt speeds through five simulated seconds. A supported Held pivot also releases its actual
counter-rotating outputs and must finish with both belts at rest and park set.

| Throttle request | Motion / steering state | Expected outcome |
|---|---|---|
| None | Against gear, no steer, either window | Below dynamic capacity: stop and park, then hold. Between dynamic and static, or above static: a moving entry slides with bounded braking. |
| None | Against gear, held steer outside neutral | Same dynamic brake budget. Held construction reaches its neutral pivot and holds the mean; Free construction has no stop once it reenters neutral. |
| None | Either direction, held steer inside neutral | Only the construction's mean hold acts: Held arrests supported mean drift and preserves counter-rotation; Free follows the load until it leaves neutral. A moving Held mean beyond dynamic capacity slides. |
| None | Along gear, not a neutral pivot | The existing unsteered sub-idle stop brakes; otherwise it coasts without stop-brake force. The coupled engine can still supply drag or below-idle catch. |
| Along gear | Along gear | No two-sided stop request; propulsion and the one-way launch lever retain their own job. |
| Along gear | Against gear | Capacity-limited service braking plus powered thrust; brake contribution is bounded independently of the engine. |
| Against gear | Either motion direction, either steering/window state | Capacity-limited service braking; after reaching rest the existing reversal pause/cut permits launch on the requested ladder. It cannot use the old ladder's engine catch. |

Clause audit: the neutral-pivot exemption replaces the all-steering exemption. The sub-idle
along-gear stop is not redundant with rollback; the at-rest latch holds a reversal pause and
requested motion releases it for launch. The clutch keeps park, signed-shaft, mean-rest and
neutral-pivot predicates: each has independent states. In particular, released counter-rotation
can have a resting mean with neither belt at rest. Only the duplicate Brake predicate and its
argument are removed: Brake already makes propulsive pedal zero. No new threshold is added.

HUD follow-up is listed, not patched: `standard_drive_row` still shows `P` from the retained
park bit, including at-speed rollback braking. Powered rollback can have the same physics with
either bit value. A clutch-only filter would incorrectly hide park on an unstaffed vehicle,
whose clutch springs In; authoritative brake-regime telemetry is absent from this view. A sound
fix needs more than a few local display lines, so no approximate HUD rule is introduced.

**Free construction: bounded but sustained chatter, not settling.** MEASURED in all 12
within-capacity held-steer trajectories (three constructions, both ladders, two entry windows),
with per-side load half dynamic brake capacity. Crossings persist in the final two seconds of
the five-second fixture observation:

| Fixture ladder size (forward gears) | Final-window absolute mean range (m/s) | Neutral crossings in final 128 ticks |
|---|---|---|
| 5 | 0.405559–0.624645 | 70 |
| 8 | 0.341318–0.657107 | 124 |
| 3 | 0.374155–0.630033 | 98–107 |

The range aggregates both ladder directions and both initial windows. One eight-gear fixture,
forward ladder, entry inside neutral, records this MEASURED trace:

| Tick | Input mean (m/s) | Next mean (m/s) | Park selected |
|---|---|---|---|
| 192 | -0.35018647 | -0.5080812 | false |
| 193 | -0.5080812 | -0.3473689 | true |
| 194 | -0.3473689 | -0.5052636 | false |
| 195 | -0.5052636 | -0.34499663 | true |
| 196 | -0.34499663 | -0.50289136 | false |
| 197 | -0.50289136 | -0.34299928 | true |
| 198 | -0.34299928 | -0.500894 | false |
| 199 | -0.500894 | -0.3413176 | true |

This is #392's regime-boundary class, not a discharged issue. It replaces the unbraked runaway
with the selected bounded stop/release policy; no band, timer or latch suppresses it. The Held
within-capacity trajectories settle on their mean hold, with no tail crossings. Loads between
dynamic and static, and above static, leave the moving entries sliding; no trajectory assertion
requires the two moving belts or their mean to change monotonically on every tick. Steering can
exchange their motion. The independent brake-only table checks the capacity bound instead.

Logs: `/tmp/belt-tick-r4-red.log` contains the failing held-steer reproduction;
`/tmp/belt-tick-r4-final-tests.log` contains the final class observations and green targeted run.

## Verification and ratchet

- `cargo fmt --all --check`: passed.
- CI clippy, locked workspace, all targets, `dev_ui,bitprobe`, warnings denied: passed.
- `track::transmission`: 105 tests passed, including both no-pedal clutch regressions and the
  two released-motion policy witnesses. The reversal test retains the modest-overload cycle witness.
- `track::sim`: 32 tests passed.
- `tests::doc_citations`: 3 tests passed. The shared drive-HUD format test also passed.
  Total targeted: 141 passed.
- Command: `scripts/serial-cargo test --lib --features dev_ui,bitprobe -- track::transmission track::sim standard_drive_row tests::doc_citations --nocapture`.
- Test build emitted the existing linker unwind-table-size warning; no Rust warning blocked CI clippy.
- Round-1 rigid-wall, slab and parked probes passed as observations, with the limits above;
  none was re-run in rounds 2 through 4. The wall case is removed.
- No wire type definition changed; no protocol hash or revision update is needed. No fork
  file changed. The temporary observation harness is removed.
- The six lenses were applied as a lane self-review: no failure handling, alternative path,
  per-asset rule, duplicated ownership, historical pass condition, or new serial work is
  introduced by the edited items. The tests use class fixtures and owner-supplied capacities
  and cut durations. No backlog row names any edited file, so none is removed.
- The orchestrator's six lenses passed rounds 1 through 3. Independent review of this round's diff
  remains the orchestrator's pre-merge responsibility; no agents were spawned here.

The new and retained policy witnesses distinguish these mutants:

| Test | Mutant it rejects |
|---|---|
| `released_rollback_stops_and_parks` | Exempting all steering leaves held-steer rollback unbraked (outcome red on `3b3f338e`); removing against-gear parking misses unsteered rollback; an unlimited brake violates the state-table force bound. Both ladders and all input states are enumerated. |
| `neutral_pivot_keeps_its_mean_hold_as_the_only_stop` | Applying released-motion parking to an active neutral pivot substitutes per-side brakes for its construction-selected mean hold. |
| `released_motion_coasts_without_brake_force` | Parking every released moving vehicle brakes a deliberate coast along the gear. |
| `pending_reversal_does_not_power_rollback` / `steering_rollback_does_not_close_the_main_clutch` | Removing the no-pedal direction check transmits the old gear's catch against rollback; a park-only fix misses skipped rest and active steering. Both failed in round 2's mutation runs. |

Full local command output is in `/tmp/belt-tick-*.log`; the probe's existing CSV writer holds
`belt-tick-before-slab-pull.csv`, `belt-tick-after-slab-pull.csv`, and
`belt-tick-before-rigid-wall-pull.csv` under the bay's `target/belt-friction/`. The measurements
needed for this report are retained above; those temporary paths are not required to read it.
