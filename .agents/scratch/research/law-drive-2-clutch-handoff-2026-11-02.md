# law-drive-2-clutch handoff (#146)

Subject: the game's driving and damage model, game state only. Crew are HP pools at stations.

New scope stopped at the MEASURED 60-minute boundary (06:55–07:55 UTC). Final whole-suite
failures were corrected and the gates completed afterward. No capture, frame/tick pricing,
issue comment, push, merge or release was performed. The bay remains leased to this lane.

## Evidence and dispositions

Commit subjects below are the stable references:

- S1: `driving: require complete drivetrain source paths (#146)`.
- S2: `driving: bound fuel and control launch slip (#146)`.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude capability seam | P2(a): Drive can omit physical sources and silently report full throughput | verified red: a fixture with both sources declared but either dependency omitted was admitted | S1 |
| Claude capability seam | P2(b): a zero task rate launches through the idle catch | verified red: the zero-rate fixture delivered 111,448 N total from rest | S1; represented directly by Out in S2 |
| Claude capability seam | P2(c): an engine outside the task path supplies full combustion | verified red through the omitted-source admission; mandatory sources now cannot be bypassed by any productive Pool/Backup path | S1 |
| Claude capability seam | P2(d): a transmission bottleneck also lowers combustion | verified current behavior; accepted as the one-task-rate contract, documented and pinned generically, rather than reverted | S1 |
| Claude D1 | first coupled tick measures lock-up/spool rather than sustained force | verified in the baseline and new logs: first delivery can be clutch-capacity torque or zero while spooling; the instrument now averages coupled settled samples | S2 |
| Claude D1 | absolute global M imposes one vehicle's Newtons on another | verified DERIVED: the old 327,846 N exceeds a 20 t fixture's 196,200 N weight; new M is a share of each vehicle's own weight, with a second-weight scaling test | S2 |
| Claude D0 | 50% persistent steer remembers its previous detent | verified red: `persistent_notches_have_one_steady_detent` got wide versus tight at 0.5; entry/release thresholds now lie between notches in data | S2 |
| Claude D0 / driving-06 | sandbox-device-gather statement is false | verified by the raw arrow-key contract; DriveFeel now states that sandbox input bypasses game notches | S2 |
| Claude dmg-4 | drag reconstruction bypasses the fuel cut | verified red: 763.88885 N m of credited combustion remained past cut in the lab fixture | S2 |
| Claude dmg-4 | idle guard's Resolution label states a behavior rather than numerical precision | verified: the guard, idle-catch and rev-match widths now live in required StandIn data | S2 |
| Claude dmg-4 | spec test comment couples drag fade to the stall band | verified; the gate and comment now describe positive idle headroom above the authored guard, independently of drag regularisation | S2 |
| Claude dmg-4 | sandbox-only headless helpers compile without dev_ui users | verified by their cfg-gated callers; all three helpers now share that gate | S2 |
| backlog load-01 | duplicate weapon keys overwrite declarations | verified red using a duplicate fixture mount; parsing now uses the existing duplicate-refusing keyed map | S1 |
| backlog dev-10 | bitprobe erases input/I/O categories and fixture bugs as strings | verified in the former signatures; usage, initialization timeout and I/O are typed, and broken fixture invariants panic at their source | S2 |
| whole-lib gate | neutral-pivot belt motion closes the main clutch and removes rev-match fuel | verified red in the shipped plugin; neutral steering shares its regime predicate with the clutch, with a direct state regression | S2 |
| whole-lib gate | top-speed test assumes equilibrium at complete fuel cut | verified red: MEASURED 10.810 m/s is inside the DERIVED 10.509–10.930 m/s governor ramp; the class contract now brackets the ramp, retaining the runaway and steady-speed checks | S2 |

The omitted-dependency, idle-catch bypass, duplicate-key, torque-bound, fuel-cut, steer-history
and pivot failures were executed before their fixes. Labels/helper/documentation findings were
verified by source inspection; no claim of a red execution is made for those prose/cfg findings.
The serving-crew tie-break was not reintroduced.

## Torque, launches and M

The engine net torque is bounded by the governed full-load curve for every sampled speed/pedal
on two fixture curves. Full pedal still attains that curve. Steady launch Slip transmits
`min(tau_free(omega_launch), clutch_capacity)` in either direction at full and part pedal, and
never more than the curve. Recovery to the launch floor is limited by freely reachable crank
speed, so it cannot create energy. These are class-invariant tests, not vehicle performance gates.

MEASURED simulated launch times, unchanged scheduler and identical ground fixtures:

| ground | rest to 0.5 m/s before | after | rest to 2 m/s before | after |
|---|---:|---:|---:|---:|
| concrete | 0.593750 s | 0.656250 s | 2.609375 s | 2.593750 s |
| sand | 0.640625 s | 0.671875 s | not reached in 40 s | not reached in 40 s |

MEASURED 27 sustained landing samples: signed forecast gap −909.425781…261,415.078125 N.
DERIVED span 262,324.503906 N; MEASURED observer residual bound 0.406250 N. DERIVED
`max(span, max(abs(gap))) + residual = 262,324.910156 N`, divided by the fixture's 559,170 N
weight and rounded strictly upward at one part per million, gives `M = 0.469133 W`.
That is DERIVED 262,325.099610 N on the measured rig, and 92,043.894600 N on a 20 t fixture.

The instrument excludes unsettled spool/lock-up ticks and averages up to one interruption's
coupled samples, using the post-tick shaft for In. A new cut closes a partial sample. It retains
first delivery and forecast-speed error separately. The large remaining gap includes coupled
crank acceleration and speed gained after the original forecast: this is a conservative envelope,
not certification of a small-margin gear law. Dirt's `M < 8.0 kN` premise is not met, so F8
convergence is unproved. Sand's previously measured F7 spare near −6 kN still refuses that upshift
for every positive M. The new M also narrowly exceeds the design's DERIVED 255 kN maximum
20-degree launch spare: lane 3 must account for crank inertia/forecast drift before treating this
envelope as usable C3 headroom. It must not reduce M merely to obtain a desired gear.

The uninterrupted-plane recoil fixture reads its incline every tick and bounds paired change by
support precision, independently of M. MEASURED paired L change is 0.093750 N. The prior short-ramp
fixture crossed support boundaries under the new launch and could not prove that invariant.

Distributions and the derivation are in `drive-clutch-measurements-2026-11-02.json`; architecture
section 5a owns the calibration narrative. The historical first-tick JSON remains evidence.

## Deletions and decisions

1. S1 deletes permissive Drive-source omission and duplicate-overwriting weapon parsing. Every
   productive Drive path must require Engine and Transmission; the existing provider gate then
   requires their volumes. Extra declared dependencies remain legal. File/field/fix diagnostics
   reach both the asset loader and bake door. Synthetic bake fixtures now declare both sources.
2. One evaluated task rate governs commanded combustion. A transmission-only bottleneck also
   lowers that rate; physical transmission condition separately bounds mechanical capacity.
3. S2 deletes additive commanded-plus-idle fuel, uncapped drag reconstruction past fuel cut,
   `clutch_out`, both clutch-speed hysteresis constants, the old latch and their tests. The new
   state is Out | Slip | In. Slip starts below idle and persists to idle + u*(peak-idle); a staffed
   zero task opens the clutch. Neutral steering keeps it Out even with unequal counter-rotating
   belts. Unstaffed controls preserve neutral Out; a released slipping clutch springs In.
4. The three actuator/stall widths keep their existing magnitudes but become required StandIn
   data in `assets/driving/engine.ron`. Idle admission reads the authored guard instead of a
   separate unexplained fixed floor. Numerical drag regularisation remains independent.
5. The wire enum is registered in the type inventory, exact projection/hashing cover it, and
   PROTOCOL_REV is deliberately 69. The missing band-confirm field mutation is also covered.
6. M loses its absolute-Newton global datum and first-tick measurement. Its dimensionless share
   is Resolution for landing-force deadband stability, scaled by each rig's own nominal weight.
   No gear-scheduler decision was replaced. The price probe uses propulsive command and the
   actual clutch regime; delivered output belongs to the solve seam, not transmission input.
7. Steering thresholds are required Taste data. Admission refuses a hysteresis band containing
   a persistent notch; changing notch spacing must move the thresholds with it. Fixed threshold
   constants and their old history-dependent test expectations are deleted.
8. The torque test no longer pins HUD wording. A scheduler-priority fixture no longer asserts
   the deleted instantaneous launch behavior; brake fixtures isolate clutch torque. Top-speed
   acceptance uses the governor ramp because drag now remains through fuel cut.
9. Bitprobe deletes string/boxed errors for outside input and fixture invariants; the existing
   self-describing payload schema carries the clutch tag and places solved forces at their seam.

## Gates and remaining work

MEASURED final gates: `cargo fmt --all --check`; CI workspace/all-target clippy with
`dev_ui,bitprobe` and `-D warnings`; full lib suite **2134 passed, 27 ignored, 0 failed**.
This includes doc_citations, ui_ascii, net_boundary, bake, headless, pen_cam and field_casts.
All binary runs were unattended, muted, headless and serialized by the machine lock.

The ratchet is incomplete; the time-limit handoff to lane 3 is explicit:

- D0/backlog driving-01, driving-04, driving-05, driving-07, driving-08, driving-09, driving-10:
  gauge effective hold intent; Taste literals in input/HUD/death-flow/slew tests; three lever
  reset sites; HUD module source contract; unchanged Node writes; font-row/gauge placement;
  typed LeverPress direction. These were inspected, not executed red in this lane, and remain
  open. Gauge/widget cardinality view-08 is also unchanged in the untouched HUD file.
- Bake backlog load-03, load-07, load-08, load-09: shipped Fixture admission in test builds;
  inferred turret assembly from optic ancestry; station-to-solid construction; nonzero axle
  assertion. Their source defects remain. Bake changes here only supply the newly required
  fixture sources and corresponding reference inventory. These rows were not deleted.
- Backlog ballistics-20 is only partially addressed: this lane removes the transmission test's
  literal HUD assertion; the sim tests' coupled wording assertions remain in the untouched sim
  file. driving-03 retains its existing lane-4/lane-5 ownership.
- D1 findings other than the two dispatched M findings remain with their owning lanes, including
  piece-support normals and turn/load integration tests. No affected file was silently declared
  clean. The large M/price discrepancy is an explicit lane-3 constraint above.

No owner approval is requested. The orchestrator owns any merge, push and bay release.
