# Belt tick investigation

Subject: the video game's tank driving physics, game state only.

Base: `7c0156e309f2134605b21bd877021dc733be5d5f` (the bay's `main` at dispatch).
Branch: `lane/belt-tick`. No push, merge, external comment, or performance measurement.

## Disposition

This is a **partial implementation**. The independently reproduced control defect in #373 is
fixed at the no-pedal clutch decision after reproducing the round-1 missed entry. The shared
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

The contact paths retain their base line numbers. The control change is in
`update_clutch_state`; `requests_other_ladder` also unifies direction-request admission
and nonpropulsive control at their shared inclusive input threshold. Drivetrain references
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
selector is another feedback site (`:1471` chooses static capacity from pre-step speed), but
the witness exceeds **both** capacities. The selector therefore cannot explain its repeated
stops. No separate static-capacity defect is claimed or changed here.

Launch engagement also reads prior clutch regime and shaft speed (`:1728`), and changes the
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

The same steering fixture rolling **along** the engaged ladder is different: transmitted
torque brakes the roll, never powers it. One member crosses the neutral threshold 27 times
before settling; no member crosses in the final 2 seconds. These observations are identical
before and after the root fix. This is the neutral-pivot/engine-braking boundary, not the
rollback idle-catch defect; no sustained cycle is established in that direction and no
additional policy is changed here.

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

The Driver's no-propulsive-pedal clutch decision now requires motion along the engaged
ladder. Signed `shaft_omega` already expresses that direction; no new threshold, band, timer,
latch or force is introduced. `update_clutch_state` owns the policy. The park release returns
to its original request/motion rule. `requests_other_ladder` owns the opposing request at the
two remaining sites; the third spelling disappears with the redundant park predicate.

Decision: model a driver disengaging before the engine is back-driven against its firing
direction, braking until a stopped direction cut is possible. An intentional hill-start
pull still uses propulsive pedal and its launch clutch. Keeping the old gear coupled for an
uncommanded idle-catch hill hold is rejected; setting park while moving would cover the same
clutch error with a second control rule.

Observations for the owner to judge by feel (MEASURED in the fixtures unless marked DERIVED):

| Case | What a player would see under this policy |
|---|---|
| Within static capacity, starting at rest | The reversal pause and paid cut hold rest, then launch in the requested direction. The existing hold test passes before and after. A moving entry also needs the dynamic brakes to arrest it. |
| Beyond static capacity, holding the opposite direction | Rollback continues with bounded service braking, clutch Out and no old-ladder catch. It no longer judders through rest, and a skipped-rest entry no longer produces a different coupled regime. Park releases when rollback agrees with the request; it does not flicker every tick. The cut waits for actual rest. |
| Between dynamic and static limits during the reversal pause | Undisturbed rest holds. A fixture nudge of -0.1 m/s at tick 8 formerly returned to rest (within rounding) in all three members. It now continues sliding: the next means are -0.170313, -0.178947 and -0.152083 m/s. The dynamic brakes cannot recover rest, so the direction cut remains pending until the load eases or outside motion arrests it. |
| Hill start | Propulsive pedal still launches through the one-way lever without rollback. Before/after fixture launch gears, final speeds and cut counts agree in the one comparison; no old output is pinned. |
| Pivot | A within-capacity held mean still pivots in place. An overloaded rollback pivot now slides out of neutral without the old ladder catching it repeatedly. Rolling along the engaged ladder retains its engine-braking transition, including the finite threshold crossings noted above. |

DERIVED: the direction check also applies to ordinary no-pedal rollback with park released,
not only opposite-key reversals; it removes that unwanted main-clutch catch there too.
No authored data was tuned to preserve an earlier output. Rendered slope feel remains for the
owner; the slab and parked observations above are historical round-1 evidence, not round-2
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

The shared solve belongs game-side, through Avian's public schedules and solver-body state,
as the #353 comment and #358 prototype establish. A belt and its drivetrain are game degrees
of freedom; extending the engine's prescribed conveyor surface into that model is a game
divergence, not an upstream friction bug. No fork file changes or upstream draft are included.
The existing prototype's fixed anchor and single-manifold evidence does not discharge the
moving-crank and shared-budget requirements. A frozen effective belt mobility or a later
settlement of its impulse would preserve the rejected estimate-of-the-other-side approach.

## Verification and ratchet

- `cargo fmt --all --check`: passed.
- CI clippy, locked workspace, all targets, `dev_ui,bitprobe`, warnings denied: passed.
- `track::transmission`: 103 tests passed, including the two regressions that failed without
  the no-pedal direction check. The reversal test also retains the modest-overload cycle witness.
- `track::sim`: 32 tests passed.
- `tests::doc_citations`: 3 tests passed after removing/renaming tests and `rigid_wall_bed`.
- Round-1 rigid-wall, slab and parked probes passed as observations, with the limits above;
  none was re-run in round 2. The wall case is removed.
- No wire type definition changed; no protocol hash or revision update is needed. No fork
  file changed. The temporary observation harness is removed.
- The six lenses were applied as a lane self-review: no failure handling, alternative path,
  per-asset rule, duplicated ownership, historical pass condition, or new serial work is
  introduced by the edited items. The tests use class fixtures and owner-supplied capacities
  and cut durations. No backlog row names any edited file, so none is removed.
- The orchestrator's six lenses passed round 1. Independent review of this round's diff
  remains the orchestrator's pre-merge responsibility; no agents were spawned here.

Full local command output is in `/tmp/belt-tick-*.log`; the probe's existing CSV writer holds
`belt-tick-before-slab-pull.csv`, `belt-tick-after-slab-pull.csv`, and
`belt-tick-before-rigid-wall-pull.csv` under the bay's `target/belt-friction/`. The measurements
needed for this report are retained above; those temporary paths are not required to read it.
