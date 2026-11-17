# Drivetrain slice 3: Relocate

Subject: the game's driving physics, game state only. Where an engine's HP state is involved,
the subject is the game's damage model: an HP pool emptied means the engine is knocked out.
Lane `lane/drive-relocate`, bay 2. Fixes follow the independent review and six lens verdicts
for `63cf3d97`; `main` was merged first, retaining both sides of `src/headless_test.rs`.

## Current schedule and ownership

- FixedUpdate samples contact, shapes commands and makes one Driver decision. A server-local,
  spawn-built `DrivetrainStep` owns pending mechanics and the last accepted block assignment.
- Each physics substep advances the crank and belts after WarmStart. Surface preparation then
  writes each belt-face constraint's velocity before SolveConstraints. PostSubstep publishes
  belts, phases, crank and tick outcomes once, after the entire substep loop.
- The resting trial advances the same mechanical sequence on the last sampled loads. Its h is
  derived from Avian's upcoming physics interval, including the physics clock's relative speed;
  the previous Time<Physics> delta is stale at FixedUpdate. A late island wake retains an accepted
  trial, without advancing it again or opening another Driver ledger sample.
- The sandbox uses the same integrator, assignment history and surface preparation.

Verified against `vendor/avian3d-0.8.0-overmatch`: `contact/mod.rs` defines the public
`ContactConstraint::tangent_velocity` as world-space v2-v1 (m/s), and `ContactConstraint::solve`
passes it to the tangent solve during Relax. `solver/plugin.rs` prepares and solves the public
`ConstraintGraph.colors[*].contact_constraints`; the separate ContactConstraints resource is not
the active store. No fork patch is needed.

Surface preparation is the sole game writer. The narrow-phase hook only filters geometry,
wakes bodies and sets material friction. Thus a belt contact uses its own substep's belt speed,
rather than `63cf3d97`'s tick-start speed. Sampled ground remains a tick-constant affine load,
implicit in each belt's effective inertia; the previous tick's solver reaction is still billed
once to the belts. This does not yet add the design's joint belt/contact rows.

Owner-local preparation, trials, advances and publication retain parallel tank iteration.
Surface preparation reads immutable owner states and writes disjoint graph constraints in parallel.
Its ordering after all advances is a data dependency: a belt-on-belt contact reads both owners.
Avian's graph is the atomic effect boundary. Its VisitCounts reader only inspects graph lengths,
which surface-velocity writes leave invariant, so those systems need no mutual order.

## Finding dispositions

- **P1-1:** option (b), current-substep constraint velocities; hook velocity write deleted.
  The energy book now charges actual substep friction impulses at those velocities, including
  Avian's partial warm-start share. Its engine-energy bound is unchanged.
- **P2-1:** TrackDrive publication uses `set_if_neq`. The resting publication witness names
  replication volume as its consumer and asserts no component change tick.
- **P2-2:** DriveTick carries the accepted KKT faces across substeps. At commit it returns them
  to spawn-built server-local history for the next tick. A changed ordered row Jacobian/static
  layout or impulse-limit layout invalidates history; only no history starts Interior (fixed
  coordinates still use their required face). Every trial checks feasibility and optimality.
  Candidate Driver pricing has its own cold, hypothetical solve, never a tank's live history.
- **P2-3:** arrival uses explicit brake intent, with a visible watchdog refusal. Park travel
  begins at the first tick with both belts held and the hull passing the production rest law;
  a sleeping hull's stored velocities are resume state, not integrated motion.
- **P2-4:** `locked` means every substep locked. This changes what the Driver samples: its held
  window now certifies the whole interval whose mean delivered force it receives, rather than
  only the end state. The launch observation and attribution are below; no parameter was tuned.
- **Parallel-work P2:** apply_track_forces reads TrackDrive immutably.
- **P3:** deleted-item doc citations and their narratives now name current entry points; tests
  read SubstepCount from an app mounting the shipped physics installer. The resting trial uses
  Avian's physics interval. Unread row/work counters are bitprobe-only. Commit takes pending
  work before checking body type. The no-pending comment names stopped gameplay and late wakes.
  drive_servos documents its previous-tick EngineRpm read. Redundant query items, cfg blocks,
  repeated rationale and component lookups are removed. The pre-existing second-spec speed
  ripple is an observation; contact, launch and independently derived load checks remain.

No WIRE_TYPE_DEFS definition changes; PROTOCOL_REV remains **106**.

## Independent invariants

The production schedule witness compares every intermediate crank/belt/phase bit and complete
transmission state against direct advances for four class constructions and Fixture substep
counts 1, 6 and 11. It checks delayed publication, one paid-shift decrement per tick, and an
active belt contact's current-substep velocity. The geometric witness covers both collider
orders, rotated radial/lateral faces and consecutive positive, negative and zero speeds.

The independent duration oracle removes all coupling rows from a free-belt Fixture and derives
`v_end = v_start + F*t/I`, without reading the integrator. It checks a whole tick for one,
app-configured and eleven substeps. Its allowance is DERIVED from f32 rounding stores and duration
division, not from measured outputs. Accepted-face reuse and layout invalidation have a separate
block witness. No emergent output is pinned.

## Re-measured cost

MEASURED: Apple M4, edit profile, features dev_ui,bitprobe; the same four fixtures, three runs,
640 measured ticks after their run-up. Brackets include driving preparation, sleeping trials,
all mechanical advances, surface preparation and publication; other Avian work and diagnostic
conveyor-work collection are outside them. No overhead subtraction. These are local measurements,
not release-profile or hosted two-core claims. The before wall times were visibly noisy;
assignment counts are deterministic across the three before runs.

| Fixture | Before us/tick, runs 1 / 2 / 3 | After us/tick, runs 1 / 2 / 3 | Assignments/substep mean/max, before -> after |
|---|---|---|---|
| parked-yard | 201.061 / 482.020 / 451.948 | 92.510 / 94.123 / 92.743 | 38.057812/39 -> 1.003125/7 |
| flat cost g2 tight | 419.572 / 607.696 / 441.356 | 142.132 / 144.750 / 143.389 | 95.191667/492 -> 1.191667/492 |
| flat cost g4 wide | 420.750 / 493.977 / 788.442 | 140.949 / 142.735 / 142.676 | 95.163281/236 -> 1.061458/236 |
| flat cost neutral pivot | 149.119 / 167.911 / 294.202 | 144.302 / 140.711 / 140.815 | 8.004948/27 -> 1.011719/27 |

MEASURED assignment means/maxima repeat exactly across all three runs. Each fixture has 3840
advances (DERIVED 640 ticks * the app's six substeps). Mean rows/substep remain 4 parked, 5 in
both turns and 3 in the pivot. The rare search maxima remain in moving fixtures; warm starting
reduces typical work, not the admitted worst-case enumeration. No force, inertia or taste was tuned.


Raw logs: `/tmp/drive-fix-before-cost.log`, `/tmp/drive-fix-after-cost.log`.
Reproduce with `OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless scripts/serial-cargo test --lib
--features dev_ui,bitprobe drive_relocate_cost_probe -- --ignored --nocapture`.

For reference, the original slice-2 -> 63cf3d97 means were MEASURED 63 -> 163 us parked,
141 -> 379-422 us turning, and 95 -> 130-135 us pivoting. The old cold mechanical assignment
means/maxima per substep were 38.057812/39, 95.191667/492, 95.163281/236 and 8.004948/27.

## Moved observations and attribution

### Rammed park

The existing saved summary did not record reverse/driving/brake-slide flags. An instrumented
replay of the unchanged arrival reproduced its saved release z=-12.512791634 m and release
travel 3.754382133 m. MEASURED during its 320-tick brake window: reverse selected at zero-based
tick 40, drive first delivered at tick 60; 280 ticks in reverse and 203 delivering drive.
Brake sliding occurred only in ticks 0-7. It reversed under power; this was not a sustained
braked slide off the heap. The old held -1 axis became reverse demand after the reversal hold.
The old travel included subsequent stopping, so it was not a parked-state creep measurement.

MEASURED after explicit brakes, travel from the first resting tick over the 25-second release:

| Fixture | Old release-based travel m | New rest-based travel m | New final-window sleep ticks |
|---|---:|---:|---:|
| flat | 0.000179421 | 0.000179421 | 1280/1280 |
| rammed | 3.754382133 | 0.054024875 | 1280/1280 |
| house | 0.017055145 | 0.016133204 | 1280/1280 |
| yard | 0.023554930 | 0.024185244 | 1280/1280 |

The new rammed arrival reaches the middle after 7.796875 s, brakes for 0.40625 s and starts
park measurement at z=-20.435371399 m. It no longer reverses under power. The arrival has also
changed through the current-substep contact coupling; the shorter drive-to-middle time is an
observation of that dynamics change, separate from correcting the measurement origin. The
54 mm observation includes residual settling within the production rest envelope; it does not
claim 54 mm of motion after sleep. Every final window has belts exactly zero and 1280/1280
sleeping ticks. Raw logs: `/tmp/drive-fix-{before,after}-park.log`.


### Slab launch and pull

The original slice-2 -> 63cf3d97 observation was MEASURED 3.929259062 -> 3.424832106 m/s
at four seconds, with first recorded F4 -> F1 and paid-cut ticks 40 -> 80. The latter adds
DERIVED 40/64 = 0.625 s of interrupted drive. The original report lacked pre-throttle state.

The unchanged replay before this fix reproduces 3.424832106 m/s. Immediately before demand:
F1, park=true, clutch=Slip, launch=Locked, crank=61.73857 rad/s; the Driver reading has
observed=false, speed=0.0028977322 m/s, belt_speed=0, resistance=-41.972633 N. These are
MEASURED observations, not launch requirements.

MEASURED after current-substep surface preparation: before demand, F1/park=true/clutch=Slip,
launch=Unchosen and the same crank speed 61.73857 rad/s; sensed speed=-0.0004749672 m/s,
resistance=-37.342133 N, observed=false. The first driven tick selects F4, then F5 at tick 85
and F8 at tick 151. It spends 40 ticks out, 171 slipping and 45 in; end hull speed is
4.331298351 m/s after four seconds.

Attribution: current-substep contact coupling changes the settling history. Previously a
mechanical interval had latched launch=Locked before demand; now it stays Unchosen. The
canonical latch is set by a locked mechanical solve (`advance_rows`), while zero demand cannot
choose a launch gear (`driver::choose`). An Unchosen launch enters the launch-domain choice;
the old Locked state bypassed that choice and began in the already engaged F1. The captured
pre-demand state explains the changed first gear. The all-substeps-locked Driver sampling rule
remains in force in both fix-run captures; it is a separate change from slice 2, not a new
setting introduced to recover F4. The new end speed and cut sequence are observations of this
coupling and selector history, not targets.

| Clutch | Ticks, 63cf3d97 -> fix | Left belt alternation amplitude m/s | Friction sign flips L/R |
|---|---:|---:|---:|
| Out | 80 -> 40 | 0.101335 -> 0.159197 | 25/27 -> 22/21 |
| Slip | 101 -> 171 | 0.103393 -> 0.113343 | 32/32 -> 71/70 |
| In | 75 -> 45 | 0.002351 -> 0.004957 | 9/6 -> 6/0 |

Raw launch/state summaries: `/tmp/drive-fix-{before,after}-slab.log`;
series: `target/belt-friction/fix-{before,after}-slab-pull.csv`.


### Other observations

The original 25-degree held-pivot changes were MEASURED below 1.5e-4 relative in yaw, with
sampled ground and no active solver belt contacts. No physical or game parameter changed in
this fix. The game's damage-model coast observation remains: with an engine HP pool emptied,
a held pedal can let stored crank motion push the belts through the clutch; its travel ordering
against a released pedal is not a law. The source invariants remain zero combustion/idle catch
and dissipative construction drag.

## Verification

Final-source checks, all through scripts/serial-cargo where compiling, with features
dev_ui,bitprobe except the explicitly requested no-default-feature server check:

- cargo fmt --all --check: passed.
- CI clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings: passed.
- Targeted track:: (384), headless track_scorecard (16), net::protocol (18),
  tests::doc_citations (3): MEASURED **421 passed**, none failed; nine probe tests ignored by the
  ordinary scorecard run. Cost, slab and park probes were separately run explicitly and passed.
- check --no-default-features --bin overmatch-server: passed; the same three pre-existing
  dead-code warnings for weapon audio, engine sound fields and steering readout.
- Full library suite: MEASURED **2851 passed, 0 failed, 51 ignored**, once on final production source.
- Wire registration, codec and type-manifest tests passed unchanged: PROTOCOL_REV=106.

Final-source logs are `/tmp/drive-fix-final-{fmt,clippy,track,scorecard,protocol,docs,server,suite}.log`.
All physical captures repeat the preceding fix-run summaries exactly after the diagnostic cache
correction; that correction changes only the energy book's first-substep warm-start accounting.
Production source stayed fixed through the final checks. The layout-invalidation test fixture
was then strengthened to distinguish a wrongly retained face from a fresh Interior seed;
the final block run passed **15 tests**, with formatting and CI clippy passing again.
Follow-up logs: `/tmp/drive-fix-commit-{fmt,clippy,block}.log`.

The six-lens independent review and merge to main belong to the orchestrator. This lane does not push.
