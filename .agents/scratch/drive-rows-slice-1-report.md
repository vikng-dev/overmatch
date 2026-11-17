# Drivetrain slice 1: block and stops

Subject: the multiplayer tank video game's driving physics, game state only.

## Built interface

`track::block::solve(&[Speed], &[Row], &[State]) -> Solution` is engine-independent f64 math.
A speed carries value, inertia and free impulse. A row carries its coefficient vector, target,
finite lower/upper impulse bounds and optional dynamic bounds inside its static bounds. An empty
assignment starts cold; otherwise the supplied assignment is tried first, then increasing Hamming
distance. The result carries speeds and each impulse, stuck/lower/upper state and dynamic outcome.
Dependent stuck sets are rejected as assignment candidates. Static eligibility requires exact
starting row rest. Broken static bounds shrink once per row and solve again in the same call.
Speed-coordinate elimination substitutes stuck targets, including exact zero for coordinate stops
and exact opposite speeds for the zero mean.

`transmission::stop_rows` builds current side stops or the mean hold from construction capacities
and unchanged Driver controls. Existing clutch, steering and ceiling-brake forces enter as free
impulses. The sampled ground's slope remains in effective inertia. Sprocket forces include row
impulses, while the speeds come directly from the block. No engine API, contact adapter, Driver
choice, replicated type, crank law, work gate or recirculation law was added or replaced.

Removing the named even-stop predictor also removes its look-ahead contribution to steering.
The remaining steering predictor receives the clutch force, and its result is free input to the
stop block. A held mean's steering difference on unequal affine loads is therefore an observation,
not an exact-target promise. The production mean-rest invariant remains tested. Differential
load sharing is proved in the generic block fixture; production steering becomes a row in slice 2.

## Deleted

DERIVED line counts below count each original function from its declaration through its closing
brace, excluding documentation and blank separators.

| Item | Function lines removed |
|---|---:|
| `brake_capacity_for_regime` (whole selector, including speed test) | 12 |
| `BeltTick::held_mean_force` | 11 |
| `apply_brake_stop_forces` | 33 |
| `mean_hold_capacity` | 10 |
| `even_stop_capacity` | 18 |
| `apply_mean_hold` | 13 |

The contact-conditioned zero-speed initializer and its comments in `sim::belt_tick` occupied
7 lines; one direct assignment replaces them. The steering predictor lost 8 lines net, including
its argument and documentation. The obsolete tests removed are
`two_parked_brakes_share_a_neutral_stop_evenly` (25 function lines),
`static_brake_capacity_requires_every_hold_predicate` (26), and
`brake_is_discretely_passive` (32). Their analytic-selector/passivity evidence is superseded by
the block optimality certificate, exact-rest/breakaway and coupled-load proofs. The old even-share
steering promise belongs to the deleted predictor, not to the slice-1 free-force adapter.

## Proofs

- 600 inline Fixture blocks, each cold, warm and with perturbed free impulses: an independent
  certificate reconstructs momentum, verifies impulse feasibility and checks the dual variational
  inequality at both endpoints of every bound. These are sufficient global convex optimality
  conditions. It uses none of the production elimination, assignment or dot-product code. Cases
  include varying dimension, targets, asymmetric bounds, redundant scaled rows, zero rows and
  static-to-dynamic transitions.
- A stuck coordinate stop is exactly `0.0`; a held zero mean sums to exactly zero. Production
  parking and unequal-affine-load mean-hold tests also require exact rest.
- Breakaway uses dynamic impulse over inertia in the breaking call. Nonzero starting slip gets
  dynamic capacity even below the former speed band. A labelled floating-point allowance applies
  to nonzero arithmetic; rest assertions remain exact.
- A differential plus two stops holds unequal loads whose larger individual demand exceeds one
  stop capacity but whose total fits the pair. Its nonzero transfer and momentum certificate
  reject independent side clamping.
- A shaft with one clutch and one brake per output, and no steering tie, solves through the same
  interface. Opening one clutch lets its brake hold that output while the other side remains
  independently connected.

## Design claims checked

The named slice-1 functions and the cited `sim` override were present. Their cited line numbers
matched this base closely enough to locate them, but all edits use symbols. Affine mobility,
clutch/steering/work ordering, Driver stop orders, service capacity and construction validation
were checked in source. The existing probes are ignored library tests, and their runner has no
window or GPU; the rigid-wall case is absent on this base, as the design states.

Corrections/qualifications found:

- The tight radius/speed ratio is approximately constant, not identical across all eight authored
  rows. DERIVED from the rounded table: 1.2279069767 to 1.2290322581 m per km/h. A future admission
  check needs an explicit construction/rounding decision; no steering data changed here.
- The cited public `tangent_part` field is on each `ContactConstraintPoint`, not on its enclosing
  `ContactConstraint`. A future stripping adapter must visit its points.
- The finite `3^m` assignment bound is per fixed-bound solve. Static breakaway can restart the
  solve, so it is not an upper bound for the whole static/dynamic call. The generic interface
  deliberately does not bake in five rows or three speeds.
- Keeping steering as a free force while deleting the even-stop look-ahead does not preserve the
  old exact pivot difference on unequal affine loads. The slice boundary explicitly keeps only
  the mean-stop invariant; jointly solving that difference is slice 2.

The idle curve is end-clamped: its 600 rpm idle reads 1300 N m from the first 800 rpm point
(DERIVED from the sheet and `raw_torque_at`). Avian source confirms positive-friction tangent
creation, friction only during Relax, bounds from the current normal impulse, public per-point
tangent parts, single-threaded substep scheduling, and the carried tangent warm-start field.
`ContactTransfers::friction` currently reads tangent-part impulses, so slice 4 needs a deliberate
publication path for game rows. No named XPBD joint/constraint construction was found in `src`.
The failed solver, hand-derived contact-response/gain tables and parallel-copy cost are not relied
on by this slice and remain unverified; no claim of their correctness or cost is made here.

## Slice 2 handoff

The solver accepts arbitrary coefficient rows, one-sided bounds, moving targets, redundant
constraints and a previous assignment. Slice 2 still needs the crank speed/inertia in the actual
block, clutch and crank-geared steering rows, the ceiling row, Driver P1/P2 orders, and replacement
of the analytic clutch/steering/crank-settlement/work paths. The production caller starts cold;
it does not yet retain assignments between ticks. Orders/warm-assignment storage and the common
substep integrator remain part of the later relocation work. No wire fields were added to hold
that cache, and no later slice was implemented.

## Validation and probe observations

All final commands passed:

- `cargo fmt --all --check`.
- `scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe track::`: 376 passed.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe tests::doc_citations`: 3 passed.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe`: run once; 2852 passed,
  0 failed, 50 ignored, 442.39 s (MEASURED).
- The ignored `belt_friction_probe` selected with `BELT_FRICTION_ONLY=slab-pull`, and
  `rubble_park_probe` with all of its cases: each passed before and after.

All builds/tests used the queue and `--features dev_ui,bitprobe`; all probe invocations used
`OVERMATCH_UNATTENDED=1` and their existing headless runner. Before/after tags are
`drive-rows-before` and `drive-rows-after`. Process logs are `/tmp/drive-rows-*.log` on this
machine; the observations below are the durable record. No rigid-wall case exists here, so none
was restored or run.

MEASURED slab-pull observations (one paired run):

| Reading | Before | After |
|---|---:|---:|
| Final hull speed, m/s | 3.572333 | 3.554014 |
| Engaged-clutch left-belt alternation amplitude, m/s | 0.104948 | 0.103886 |
| Engaged-clutch total delivered-friction alternation, kN | 257.18634 | 259.69501 |
| Engaged-clutch left/right sign flips | 150 / 146 | 138 / 152 |
| Open-clutch left-belt alternation amplitude, m/s | 0.280471 | 0.281113 |
| Open-clutch total delivered-friction alternation, kN | 335.18671 | 341.41586 |

MEASURED rubble-park observations (one paired run; asleep ticks are over the runner's final
1280-tick observation window):

| Case | Parking travel before, m | After, m | Asleep ticks before | After |
|---|---:|---:|---:|---:|
| Flat | 0.000179421 | 0.000179421 | 1280 | 1280 |
| Rammed | 0.367229 | 10.279593 | 805 | 542 |
| House | 0.00235261 | 0.04054038 | 1280 | 1280 |
| Yard | 0.00329008 | 0.00359984 | 1280 | 1280 |

MEASURED rammed-case belt-moving ticks increased from 0 to 706; maximum absolute belt speed
in that window changed from 0 to 0.495896 m/s. This is a substantial parking change. It is an
observation of the changed brake/static-rest and free-steering path, not a new acceptance target
or a diagnosed contact fix. Nothing was tuned to recover an earlier result. Flat's residual belt
speed is now exactly zero; the before run reported a 1.37e-15 m/s residual on one side.

