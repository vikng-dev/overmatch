# Drivetrain slice 1: block, stops and neutral steering

Subject: the multiplayer tank video game's driving physics, game state only.

## Interface and slice boundary

`track::block::solve(&[Speed], &[Row]) -> Solution` is engine-independent f64 math. Speeds carry
value, inertia and free impulse; rows carry coefficients, target, finite impulse bounds and
optional dynamic bounds inside their static bounds. The result carries speeds, row impulses,
assignment states and dynamic outcomes. Each call starts cold. Assignment storage, and an input
for its warm start, belong to the slice that supplies that user.

The solver searches independent stuck sets and bounded assignments. Static eligibility requires
exact starting row rest; breakaway shrinks bounds and re-solves in the same call. Each Gram row
is normalized by its inertial norm on both axes before rank testing, so changing a row's units
cannot turn independence into dependence. Stuck targets are substituted into speed coordinates;
a coordinate stop is exactly zero, and a held mean has exactly opposite belt speeds.

`transmission::stop_rows` puts the neutral half-difference tie in the same block as the side stops
or mean hold. Its target is steer command times the construction's neutral full difference; its
impulse bounds follow per-output steering capacity, step and source work scale. There is no crank
coefficient. The delivered neutral force and landed speeds supply the existing source work
budget, excluding passive stop work. Geared steering and clutch forces remain free inputs;
their existing work estimate, crank law, Driver choices, contact path and wire remain unchanged.
The affine sampled-ground slope remains in effective belt inertia. Stop surfaces report held
from exact landed rest, including redundant rows at a bound with zero slip.

The mean hold still borrows the brake rating: the code labels this StandIn until the sheet
supplies the hold's own figure. The per-bisection block solve inside the work-limit search also
remains; slice 2 deletes it with the work limit. Neither is changed here.

A parked differential can transfer unequal loads between the two stops up to steering capacity.
The speeds are unique; redundant brake/tie impulses need not be unique or split exactly evenly.
The production test requires the pair to hold a load that exceeds one side's capacity but fits
the pair, and requires motion when the available steering transfer cannot carry that load.

## Review findings

1. Neutral steering and stops now solve together. The production parked-load test starts at
   exact rest; the held-pivot test requires both exact mean rest and the construction's commanded
   difference on unequal affine ground. Geared steering on the crank remains slice 2.
2. The original rammed-park table was not a comparison of like arrivals. The drive-phase
   investigation and corrected interpretation are below.
3. The badly scaled independent-row counterexample is covered: unit inertias/free impulses,
   row coefficients `[1,0]` and `[0,1e-6]`, and bounds ±10 and ±1e7 (all inline Fixture data).
   Its cancelling impulses are independently DERIVED as −1 and −1e6; both speeds must be zero.
4. The random-block certificate computes static sufficiency independently by cyclic coordinate
   minimization with initially eligible static bounds, then any dynamic breakaway. It compares
   expected regime flags as well as momentum, feasibility and variational inequalities. It does
   not select bounds from production's flags or share production elimination/assignment code.
5. The unused warm-start argument and its test arms are removed.
6. The held-pivot test now rejects zero steering.
7. `transmission-design.md` names the current rows and exact-rest static eligibility, retaining
   its historical observations and rationale.
8. The stop/breakaway fixture lives in one array reused by each solve and certificate.
9. The creep setup comment now only labels fixture units; the beyond-capacity hold assertion
   requires positive mean motion, independent of the Driver's parking threshold.

The rank, held-pivot and parked-load regression tests were each run before their fixes and each
failed on the reported defect. No authored value was tuned to recover an observation.

## Closing review

The neutral block row reads `steering_tie`'s weights and target, converting to f64 at the block
boundary and deriving impulse bounds from the per-output rating and those weights. The block
and work search share one optional neutral tie and its regime decision. The side-stop held
readout states its invariant: side row i constrains speed i, in construction order.

Slip allowances scale with each row's coefficients, target and evaluation magnitudes; impulse
allowances scale with its bounds. Their labelled Resolution covers f64 roundoff. The scaled
fixture independently derives saturated resistance and landing speed, checking both sign and
same-call static breakaway under changes of row units. Assignment search uses `find_map` with
the optimum invariant's `expect`; the module doc is wrapped.

The rolling brake fixture derives its stopping deadline from per-side inertia, input speed,
net braking capacity after downhill load and step, plus one tick for the park latch. Its trace
spans that deadline and an equally long parked window (DERIVED). The breach fixture's load is
its static factor times brake capacity plus a positive Fixture margin.

## Drive-phase investigation

MEASURED on this machine: the saved first run plus two new runs at `4ae55756` all produced the
same rammed arrival and parking readings. The saved reviewed slice-1 run plus two new runs at
`3d35b27c` likewise all produced the same readings for that side. Test-only changes and temporary
capacity diagnostics were present in the new slice-1 runs; they did not change its physics.

| Reading | Original base (three runs) | Reviewed slice 1 (three runs) |
|---|---:|---:|
| Drive time, s | 15.0 (cap) | 10.6875 |
| Reached the standing house's middle | No | Yes |
| Braking time, s | 0.515625 | 5.0 (cap) |
| Top drive speed, m/s | 3.373702049 | 3.373703480 |
| Released-window travel, m | 0.367229104 | 10.279593468 |
| Park ticks / window ticks | 1280 / 1280 | 574 / 1280 |
| Clutch In ticks | 0 | 706 |
| Belt-moving ticks | 0 | 706 |
| Maximum absolute belt speed, m/s | 0 | 0.495896 |

The launch lever's capacity hypothesis was instrumented at its actual production input: a
nonzero belt speed inside the old rest band, one-way lever ordered without service braking, and
rollback-opposing stop demand above dynamic capacity. MEASURED witness count: zero in each of
the two repeated slice-1 runs. Changing static eligibility therefore cannot alter the lever's
force on these trajectories. The new exact-rest rule nevertheless follows the design's row
table: a moving lever receives dynamic capacity, and static breakaway re-solves immediately.

MEASURED arithmetic-only comparison, two further identical runs: leave the reviewed solver and
all capacities unchanged, but use the former `BeltTick::land` f32 arithmetic whenever all row
impulses are zero. Both crossed the house at 11.546875 s, braked in 0.703125 s, travelled
0.236919537 m after release, and recorded 1280 park/Out ticks, zero belt-moving ticks and 1239
asleep ticks. The capacity witness count was again zero. Top speed was 3.373702288 m/s.

This intervention changes no force or bound: it changes free-landing roundoff. Its different
obstacle arrival and successful braking establish sensitivity of the collision/break trajectory
to that arithmetic. It does not reproduce the original base's stall, so it is not a claim that
this one arithmetic site accounts for every subsequent difference. The removed look-ahead does
not act during this drive: full-throttle driving is outside the neutral regime, and the launch
lever is one-sided. The changed drive phase is a numerically sensitive house collision, not loss
of launch-lever static capacity. There was no run-to-run scatter within any repeated group on
this machine; deterministic sensitivity is not evidence of nondeterministic execution. The f32
comparison arm and all diagnostics are removed; the committed block keeps its f64 arithmetic.

Most of the reviewed slice-1 run's released-window travel is creeping in gear before park is
latched, not a stop row losing a parked hold. The tank itself drives into a standing house; the
two sides reach different obstacle configurations and braking states. Their travel totals are
observations of different arrivals, not a like-for-like parking comparison or a contact fix.
Nothing in them is an acceptance target.

## Earlier observations retained

MEASURED, one original paired slab-pull run (`drive-rows-before` / `drive-rows-after`): final hull
speed 3.572333 / 3.554014 m/s; engaged-clutch left-belt alternation 0.104948 / 0.103886 m/s and
total delivered-friction alternation 257.18634 / 259.69501 kN; open-clutch left-belt alternation
0.280471 / 0.281113 m/s and delivered-friction alternation 335.18671 / 341.41586 kN. These are
observations, not invariant bounds. The rigid-wall probe is absent on this base.

MEASURED, the original paired rubble-park run's other cases: flat travel 0.000179421 /
0.000179421 m, house 0.00235261 / 0.04054038 m, yard 0.00329008 / 0.00359984 m. All had 1280
asleep ticks in the final observation window. The rammed case is interpreted above.

## Validation and current observations

MEASURED pivot observations: one complete sweep at `4ae55756` and one at `107e8b6f`,
each covering all 16 cases. The closing window is the runner's final three seconds of its
64-second hold; travel is planar net displacement over the hold or its five-second release.
Every closing window has belt speeds +0.287957 / −0.287957 m/s (rounded); the after mean is exactly
zero. The construction's target is 0.287957221 m/s, read from each probe's rig. These are observations,
not stored expectations. In the table, arrows are original base → final source.

| Ground | Closing yaw, rad/s | Hold travel, m | Release travel, m |
|---|---:|---:|---:|
| flat | -0.128212 → -0.128212 | 0.030481 → 0.030481 | 0.000546 → 0.000546 |
| slope5 uphill | -0.129147 → -0.129147 | 2.742561 → 2.742561 | 0.006846 → 0.006846 |
| slope5 downhill | -0.128560 → -0.128560 | 2.698096 → 2.698095 | 0.005791 → 0.005791 |
| slope5 across | -0.125650 → -0.125650 | 2.537159 → 2.537159 | 0.007336 → 0.007336 |
| slope10 uphill | -0.130111 → -0.130111 | 5.422625 → 5.422629 | 0.015787 → 0.015786 |
| slope10 downhill | -0.128636 → -0.128636 | 5.399934 → 5.399935 | 0.014898 → 0.014898 |
| slope10 across | -0.117816 → -0.117816 | 5.039417 → 5.039419 | 0.012301 → 0.012301 |
| slope15 uphill | -0.116515 → -0.116515 | 8.155975 → 8.155975 | 0.025219 → 0.025218 |
| slope15 downhill | -0.113761 → -0.113761 | 8.158844 → 8.158846 | 0.024704 → 0.024704 |
| slope15 across | -0.114937 → -0.114937 | 7.699623 → 7.699619 | 0.014556 → 0.014554 |
| slope25 uphill | -0.007204 → -0.007204 | 14.607827 → 14.607821 | 0.045340 → 4.053671 |
| slope25 downhill | -0.014843 → -0.014843 | 14.653071 → 14.653071 | 0.044546 → 3.982860 |
| slope25 across | -0.082080 → -0.082079 | 14.761036 → 14.761046 | 0.046988 → 0.046990 |
| slope35 uphill | -0.000003 → -0.000109 | 367.389793 → 365.281762 | 0.037092 → 0.000713 |
| slope35 downhill | 0.001086 → 0.001824 | 366.278435 → 366.234695 | 0.036974 → 0.038547 |
| slope35 across | -0.000075 → -0.000095 | 367.363888 → 367.362083 | 0.000718 → 0.000782 |

The longitudinal 25° release regresses to about 4 m from about 0.045 m (MEASURED, table above).
Steering releases at the authored rise rate, so the mean hold lasts through the release ramp.
DERIVED: the authored rise of 4/s and the probe's 1/64 s step decrement steer by 0.0625 per tick;
the last held command is 0.0625, above the Driver's 0.05 axis threshold. Multiplying that command
by the rig's 0.287957221 m/s full target gives belts at about ±0.018 m/s when the side stops take
over, rather than the full-pivot ±0.29 m/s (both DERIVED).

MEASURED at the first zero-steer tick in the uphill case: the reaction magnitudes total about
287.0 kN; both belts land at +0.106561 m/s and park remains latched with the clutch Out.
DERIVED from the shipped construction: the pair's dynamic capacity is 192 kN, versus 288 kN
static. The preceding mean hold starts at exact zero mean and can use static capacity. Any
nonzero starting side speed requires dynamic capacity; the side stops cannot carry the load,
and the belts never start a tick at rest again. Steering can transfer differential load but
cannot add common-mode brake capacity. The old speed-band selector could capture the moving
belts with static capacity.

Slice 2 owns this regression as a Driver handover rule: make-before-break between the mean hold
and side stops. It is not fixed here; no Driver decision or speed band changes. The 35° cases'
large hull travel also remains an observation of contact/grade limits.

MEASURED current rubble-park observations: all cases once, with rammed repeated a second time;
the two current rammed results are identical. Window counts are over 1280 ticks. These arrivals
remain different from the original base; the rammed comparison is not like-for-like.

| Case | Release travel, m | Asleep ticks | Park ticks | Clutch In ticks | Belt-moving ticks |
|---|---:|---:|---:|---:|---:|
| flat | 0.000179421 | 1280 | 1280 | 0 | 0 |
| rammed | 10.340805054 | 533 | 563 | 717 | 717 |
| house | 0.040540382 | 1280 | 1280 | 0 | 0 |
| yard | 0.003599836 | 1280 | 1280 | 0 | 0 |

The current rammed arrival still crosses at 10.6875 s and reaches the 5 s braking cap. Its
maximum belt speed in the window is 0.495881349 m/s; most travel again precedes parked rest.

MEASURED current slab-pull observation, one run: final hull speed 3.568040371 m/s;
engaged-clutch left-belt alternation 0.104372494 m/s and total delivered-friction alternation
255.921600 kN; open-clutch values 0.266415715 m/s and 324.168060 kN. The original paired readings
are retained above for comparison; no contact change was made.

Passed checks at `107e8b6f` (before the closing review):

- `cargo fmt --all --check`.
- `scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe track::`: 378 passed.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe tests::doc_citations`: 3 passed.
- The ignored full `pivot_probe`, `rubble_park_probe` (all cases and a rammed repeat), and
  `belt_friction_probe` selected with `BELT_FRICTION_ONLY=slab-pull`: passed.

`scripts/serial-cargo test --lib --features dev_ui,bitprobe` ran once: **2854 passed, 0 failed,
50 ignored**, 352.79 s (MEASURED).

All compiles use `scripts/serial-cargo` with `--features dev_ui,bitprobe`; probes use
`OVERMATCH_UNATTENDED=1` and their existing headless runner. Logs use `/tmp/drive-rows-r2-*.log`;
the tables here are the durable observations. No contact, Avian, wire or Driver code changed.

## Closing validation

MEASURED: restoring the absolute sign tolerance makes the new scaled-row test choose `Lower`
instead of the resisting `Upper`; restoring only the absolute breakaway tolerance makes it
retain static capacity. Both mutants failed their intended assertions. The final source
passes that test within `track::`.

Passed on the closing source, with `OVERMATCH_UNATTENDED=1`:

- `cargo fmt --all --check`.
- `scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe track::`: 379 passed (MEASURED).
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe tests::doc_citations`: 3 passed (MEASURED).
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe`: one run, **2855 passed, 0 failed,
  50 ignored**, 405.60 s (MEASURED).

Logs: `/tmp/drive-rows-closing-*.log`. No Driver decision changed. The release regression,
hold-rating stand-in and per-bisection block solve remain as described above.
