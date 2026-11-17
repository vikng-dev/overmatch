# Belt tick investigation

Subject: the video game's tank driving physics, game state only.

Base: `7c0156e309f2134605b21bd877021dc733be5d5f` (the bay's `main` at dispatch).
Branch: `lane/belt-tick`. No push, merge, external comment, or performance measurement.

## Disposition

This is a **partial implementation**. The independently reproduced control defect in #373 is
fixed. The shared contact-solver defect behind #347 and #353 is diagnosed and remains in
production. #312's sustained rigid-contact limit cycle was not reproduced in the wall case.
Do not treat this lane as completion of the driving-correctness prerequisite for optimization.

There are two demonstrated mechanisms (DERIVED from the paths below, with separate witnesses):

| Issue | Evidence | Result of this change |
|---|---|---|
| #373 | A drivetrain-only reversal reaches rest on alternate ticks under a load beyond both brake capacities. No solver contacts participate. | Fixed; class tests exercise both ladder directions. |
| #347 | The shipped headless slab pull alternates delivered force with the clutch Out, Slip, and In. The next tick's charge equals the previous tick's delivery. | Remains reproduced, with the same observations after the control fix. |
| #353 | The delayed billing path is present and exercised by #347. Its parked launch/sign-alternation symptom was not reproduced in the current parked probe's flat, rammed, house, and yard arrivals. | Shared root remains. These arrivals do not replace the original issue's eight poses. |
| #312 | The added rigid belt-lane wall pull has transient force reversals, without a sustained alternating-sign cycle. | No independent defect established; retain the observation case. |

## Exact tick boundary

All line references below are in this lane's tree. Only `update_park_latch` and its tests change
production behavior; the contact paths retain their base line numbers.

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

Before this change, `update_park_latch` released park whenever the requested command agreed
with belt motion, regardless of the engaged ladder (base `src/track/transmission.rs:1665`).
Rollback could therefore release park during a pending direction reversal.

- `swap_ladder_direction` refuses the cut while the belts move (`transmission.rs:1492`), so the
  old ladder remains engaged.
- `driver` makes an opposite-ladder request nonpropulsive (`:1572`).
- `update_clutch_state` uses park and near-rest speed to open the clutch (`:1698`). Removing
  park while rolling in the requested direction closes the old ladder's clutch.
- The idle catch enters free crank torque (`:1792`), the clutch can transmit it (`:1435`), and
  the combination can arrest motion that the brakes alone cannot hold.
- At rest the same park selector opens the clutch again; without the transmitted idle catch,
  the load restarts the slide. That slide then triggers the erroneous park release again.

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

Before any production edit, this command failed:

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
speeds, not measurements of a hull on rendered terrain. The finished regression covers the
same construction class through `class_drivetrains`, both ladder directions, and every tick
after sliding begins. The separate held-cut test checks exact zero belt speeds on every tick
of the authored reversal pause and paid shift window, with a load inside brake capacity.

The shipped-app slab probe was run before the production edit and again after it:

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

The added `rigid-wall-pull` case was run after the #373 control fix. It requests forward drive
on the forward ladder throughout; the added park-release predicate is true on that path, so
its release condition is the same as the base (DERIVED). It uses the scorecard's existing wall
dimensions, once per belt lane so the hull nose stays clear. The full-width wall attempt did not
reach both belts and was rejected by the probe's contact-premise assertion. With both lanes, the observation probe
passes and both belts have solver load. There are no loose pieces or soft-run normal laws in
this contact. Run with `BELT_FRICTION_ONLY=rigid-wall` in the command above. Over its fixture
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

`update_park_latch` now requires the motion, request, and **engaged** ladder to agree before
releasing park for drive. This is the same direction-admission contract the cut already uses.
An over-capacity rollback cannot make the Driver engage the opposite ladder's idle catch.
The brake still supplies its bounded stop force and gives way under excess load. No force,
capacity, hysteresis band, damping, smoothing, or extra delay was added. Both directions use
one predicate; the superseded release condition is replaced.

Behavior moved in the reversal fixture: once a brake-capacity breach starts sliding, it keeps
sliding instead of being reset to rest by the old ladder. The within-capacity cut remains at
zero belt speed every tick. Slab rest/drive observations and parked settling above did not
move in the reruns. No authored data was tuned to hold an earlier output.

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
- #373: fixed by this commit's control predicate and regression tests. No remaining item in
  its demonstrated park/clutch/idle-catch cycle.

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
- `track::transmission`: 104 tests passed, including both new invariants.
- `track::sim`: 32 tests passed.
- Selected rigid-wall, slab, and parked observation probes: passed, with the limits above.
- No function was deleted or renamed. No wire type definition changed; no protocol hash or
  revision update is needed. No fork file changed.
- The six lenses were applied as a lane self-review: no failure handling, alternative path,
  per-asset rule, duplicated ownership, historical pass condition, or new serial work is
  introduced by the edited items. The tests use class fixtures and owner-supplied capacities
  and cut durations. No backlog row names any edited file, so none is removed.
- The orchestrator's independent six-lens review remains required before merging.

Full local command output is in `/tmp/belt-tick-*.log`; the probe's existing CSV writer holds
`belt-tick-before-slab-pull.csv`, `belt-tick-after-slab-pull.csv`, and
`belt-tick-before-rigid-wall-pull.csv` under the bay's `target/belt-friction/`. The measurements
needed for this report are retained above; those temporary paths are not required to read it.
