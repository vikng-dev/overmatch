# Drivetrain slice 3: Relocate

Subject: Overmatch's multiplayer tank video-game driving physics, game state only.
The game's damage-model observations concern engine HP pools and simulated source availability.
Lane `lane/drive-relocate`, bay 2, based on `3fb3c393`.

## Ownership and schedule

The full row design was read from the owner's untracked main-checkout document,
`/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/drivetrain-rows-design-2026-11-17.md`.
This report records implementation decisions and observations; that document owns the design narrative.

- `FixedUpdate`: contact sampling, command slew and one Driver decision prepare `DriveTick`.
  `DrivetrainStep` retains its orders, affine-load reference speeds and mechanical scratch.
- `SubstepSchedule`: after `SubstepSolverSystems::WarmStart`, before `SolveConstraints`,
  `advance_drivetrains` calls the single `transmission::advance` with `Time<Substeps>`.
- `PhysicsSchedule`, `SolverSystems::PostSubstep`: publish belts, phases, crank and outcomes,
  record delivered mean forces in the Driver ledger, then project public engine RPM.
  **DERIVED from vendored source:** this set follows the entire substep loop, rather than each
  individual substep. No new fork patch is required or made.
- The resting trial runs the same `DriveTick::advance` sequence. A wake before contact sampling
  replaces the trial with a sampled pass; a later physics wake retains the accepted sequence,
  without repeating it or opening a fictitious ledger sample.
- The sandbox prepares and advances the same core on the same schedule boundaries. Its harness
  records after physics writeback so it observes the completed drivetrain.

Sampled loads remain one tick's affine law: `R(v) = R(v_ref) + slope*(v-v_ref)`. Each substep
uses its current speed in that law, with the slope implicit in its effective belt inertia.
Force telemetry sent to the Driver is impulse divided by total elapsed substep time. The
bitprobe retains the last substep's coherent operating point; its named work counters cover the
tick. A tick counts as held/locked only if every substep was; drive and brake sliding count if any substep did.

Deleted: the speed/phase landing in `belt_tick`, the combined production `transmission::step`,
`RestingStep`, the fixed-update `settle_rest` commit, and the sandbox's separate phase landing
and hard-contact speed override. Arithmetic fixtures call the same advance for one interval;
tick-level invariants call its physics substep sequence.

`DrivetrainStep` is required by spawn-built `TrackGripElements`, with a spawn-observer witness.
It is never registered for replication. No definition in `WIRE_TYPE_DEFS` changes:
`PROTOCOL_REV` remains **106**; the manifest tests pass without a re-pin.

## Equality and class invariants

The production substep installer is mounted in the equality witness. Across all four class
constructions, **Fixture** counts of 1, 6 and 11 substeps reproduce the direct advance sequence
at every intermediate state and at publication. The comparison includes raw floating-point bits
(crank, both belt speeds, both f64 phases, signed zero included), plus the complete transmission
state. Canonical state remains unchanged inside the loop. A paid shift decrements once per tick.
The adapter's equality bound is **exactly zero**: both paths receive identical duration values
and perform the same operations in the same order.

The tick-level slice-2 witnesses now run complete substep sequences: fixed-intent order stability,
passive crank-plus-belt energy, locked authored radii, exact-zero stops and static-load stop
handover. The radius allowance remains the data owner's rounding share plus f32 subtraction
roundoff. The energy allowance is **Resolution, DERIVED**: storing speeds in f32 at each of N
substeps contributes at most `gamma(2N) = 2N*epsilon/(1-2N*epsilon)` to squared-speed energy,
using f32 epsilon conservatively as unit roundoff. At N=6 this is about `1.430514e-6` relative;
the zero-energy corner stays exactly zero. It is not fitted to an observed energy change.

The engine admission gate still certifies a whole-tick explicit torque interval. This is
conservative for every subdivision, so relocation needs no relaxed admission or new parameter.

## First cost reading

**MEASURED:** Apple M4, macOS 26.5.2, local compute pool, `edit` profile, features
`dev_ui,bitprobe`, three runs before and after on the same machine. Each measured fixture
uses 640 fixed ticks after its existing settling/run-up. These are local observations, not
hosted two-core or release-profile performance claims.

The ignored `drive_relocate_cost_probe` brackets the production drivetrain systems only:
the driving pass, every substep advance and the final commit/RPM publication. Geometry/contact
sampling, Driver pricing and resting trials are included; other Avian work is excluded. Query
dispatch and bracket overhead are included, with no overhead subtraction.

| Existing fixture | Before mean us/tick, runs 1 / 2 / 3 | After mean us/tick, runs 1 / 2 / 3 |
|---|---|---|
| parked-yard | 63.390 / 63.261 / 63.071 | 163.010 / 163.601 / 164.230 |
| flat cost g2 tight | 141.015 / 141.063 / 140.543 | 379.256 / 400.467 / 412.582 |
| flat cost g4 wide | 140.165 / 140.971 / 140.827 | 380.450 / 421.718 / 412.728 |
| flat cost neutral pivot | 94.672 / 94.887 / 95.650 | 129.647 / 131.560 / 135.444 |

**MEASURED** work below was identical in all three runs. Every fixture recorded 3,840 advances
(640 ticks times the configured six substeps, DERIVED). Sleeping trials are included.

| Fixture | Rows / tank / substep, mean and max | KKT assignments / tank / substep, mean / max | Active Avian tank contact points / tick, mean / max |
|---|---:|---:|---:|
| parked-yard | 4 / 4 | 38.057812 / 39 | 9.175 / 132 |
| flat cost g2 tight | 5 / 5 | 95.191667 / 492 | 0.000 / 0 |
| flat cost g4 wide | 5 / 5 | 95.163281 / 236 | 0.000 / 0 |
| flat cost neutral pivot | 3 / 3 | 8.004948 / 27 | 0.000 / 0 |

The g2/g4 names are the existing run-up selectors, not locked gears. The parked case uses the
existing yard arrival and brake, including its transition into sleep; sleeping graph contacts
are not active solver rows. Its observed point maximum is 132, rather than the design's older
failed-run observation of 91. The flat turn/pivot fixtures use sampled ground and have no Avian
contact points. These are drivetrain rows only: slice 4 has not added belt contact rows.

Mechanical assignment counts exclude Driver candidate pricing; total tick counters include it.
The existing before counter omitted resting-trial work, so it is not used as a before/after work
comparison. The wall-clock brackets include that work in both implementations.

The design's typical-one-assignment assumption is false on these fixtures. **DERIVED from
`block::solve_limited`:** each call starts from an interior/fixed-coordinate seed; accepted
assignments are reused only across static-to-dynamic stages within that call, not across
substeps. This slice does not change the solver search. Its five physical rows plus the crank
impulse-limit face can have six ternary coordinates: `3^6 = 729` assignments per stage, rather
than the design's five-coordinate `3^5 = 243`. Thus the measured maximum of 492 is explained by
the actual admitted system, not a convergence retry or an unbounded loop. Row staging can add
further finite searches when a static stop breaks. Assignment caching/solver pricing remains a
real cost question for the later slice, not an assumed saving in this report.

Raw cost logs: `/tmp/drive-relocate-before-cost.log` and `/tmp/drive-relocate-final-cost.log`.
Reproduce with `OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless scripts/serial-cargo test --lib
--features dev_ui,bitprobe drive_relocate_cost_probe -- --ignored --nocapture`.

All owner-local passes use `par_iter_mut`: contact preparation, sleeping trials, substep advances
and publication. A tank's successive substeps and its block's assignment trials are serial by
state dependency. The nested Avian schedule orders our system after warm starting and before the
biased solve; the post-loop publication must precede sleeping and downstream readers. Those
schedule dependencies do not serialize independent tanks within the system. Contact effects on
other owners keep the existing source/entity and side/contact commit order. Slice 4 owns contact
rows; slice 5 owns grouping and hosted dispatch pricing.

## Probe observations beside slice 2

**MEASURED:** fresh before captures use the slice-2 implementation at this lane's base;
its report is `.agents/scratch/drive-crank-slice-2-report.md`. The 25-degree before readings
also reproduce that report. No model or asset parameter was changed to recover an old output.
The rigid-wall probe remains slice 4's restoration task.

### Slab pull

End hull speed after four seconds: **3.929259062 -> 3.424832106 m/s**.

| Clutch outcome | Ticks, slice 2 -> relocated | Left belt alternation amplitude m/s, slice 2 -> relocated | Left/right friction sign flips, slice 2 -> relocated |
|---|---:|---:|---|
| out | 40 -> 80 | 0.273659 -> 0.101335 | 38/36 -> 25/27 |
| slip | 155 -> 101 | 0.154561 -> 0.103393 | 69/69 -> 32/32 |
| in | 61 -> 75 | 0.010222 -> 0.002351 | 3/9 -> 9/6 |

The recorded gear sequences differ: slice 2 starts F4, shifts to F5 at driven tick 94 and F8 at
183; relocation first records F1, then F2 at 20, F3 at 63, F4 at 104 and F7 at 151. These are
zero-based probe ticks. It therefore spends 80 ticks in paid cuts instead of 40. This is an
observation of the existing selector under the relocated dynamics, not a gear-parameter change;
the capture does not retain the pre-demand selector reading/launch state to isolate its initial
F1 choice. The lower end speed accompanies this different launch/cut sequence.

### Rubble park

Travel covers the full 25-second release; the final window is the last 20 seconds.

| Fixture | Slice-2 travel m | Relocated travel m | Final-window sleep ticks, slice 2 / relocated |
|---|---:|---:|---:|
| flat | 0.000179421 | 0.000179421 | 1280 / 1280 of 1280 |
| rammed | 0.021932388 | 3.754382133 | 1280 / 1280 of 1280 |
| house | 0.014056585 | 0.017055145 | 1280 / 1280 of 1280 |
| yard | 0.025241429 | 0.023554930 | 1280 / 1280 of 1280 |

Both runs finish every park fixture's observation window asleep with both belts exactly zero.
The rammed-house arrivals are materially different: slice 2 never reaches the heap middle in
its 15-second drive and brakes for 0.046875 s, starting release at z=-16.182514 m. Relocation
reaches the middle after 9.484375 s, then exhausts the fixture's five-second brake window and
starts release at z=-12.512792 m, behind the crossed middle at z=-20.055725 m. `park_arrival`
uses a held -1 axis request, which can request reverse after the transmission stops, and exits
on whole-hull speed or the cap; it does **not** guarantee a stopped release state. Thus the
3.754382 m reading includes post-window stopping travel, rather than showing sustained creep
of the sleeping parked state. The raw moved observation is retained; neither the fixture nor
the drivetrain was adjusted to make these arrivals agree.

### 25-degree held pivots and release

Each case holds full steer for 64 seconds, then releases for five. Yaw averages the last three
held seconds. All distances below are planar. These are observations, not acceptance targets.

| Facing | Held yaw rad/s, slice 2 -> relocated | Hold travel m, slice 2 -> relocated | Release travel m, slice 2 -> relocated |
|---|---:|---:|---:|
| uphill | -0.083504148 -> -0.083516655 | 12.213123322 -> 12.213111877 | 0.023485437 -> 0.023502829 |
| downhill | -0.124657965 -> -0.124655377 | 12.791346550 -> 12.791519165 | 0.042793903 -> 0.042827494 |
| across | -0.006260733 -> -0.006261558 | 12.318467140 -> 12.318470955 | 0.032994319 -> 0.033002805 |

Every held tick retains mean hold in both implementations; the held belts remain exactly
opposite. Every release retains mean hold for 15 ticks, then side park for 305, finishing at
exact zero belt speed. The tiny yaw/travel changes are retained without tuning.

Raw logs are `/tmp/drive-relocate-{before,final}-{slab,park,pivot}.log`. Per-tick slab CSVs are
`target/belt-friction/relocate-{before,final}-slab-pull.csv`. The final-source captures reproduce
all preceding relocated JSON summaries exactly after removing the capture tag. Reproduction uses
the existing `belt_friction_probe` with `BELT_FRICTION_ONLY=slab-pull`, `rubble_park_probe`, and `pivot_probe`
with `TURN_PROBE_ONLY='pivot slope25'`, all with `OVERMATCH_UNATTENDED=1`,
`OVERMATCH_WINDOW=headless`, `scripts/serial-cargo test --lib --features dev_ui,bitprobe`, and
`-- --ignored --nocapture`.

## Investigated expectation

The existing free-hull coast test asserted that a held pedal with an emptied engine could never
travel farther than a released pedal. **MEASURED** after relocation: 13.903220 m held versus
13.858035 m released over its three-second fixture, final speeds 4.206536 versus 4.189877 m/s.
That ordering is not a law: pedal intent changes clutch impulse bounds and the launch floor even
when combustion is absent (`advance_rows`, `coupling_budget`). The source invariant is zero fuel
and idle-catch torque with dissipative motoring torque. The adapter witness now asserts those
facts under both pedal requests and reports the two coast distances as observations. The pure
`absent_combustion_keeps_drag_at_full_pedal` witness independently checks that construction drag
is identical at the same operating point. No production force law or calibration changes for this
failure.

## Diagnostic interval correction

The first full library run was **MEASURED: 2838 passed, 2 failed, 51 ignored**. Both failures
were the existing independent landing-force residual witnesses, on declared grounds and on the
course. They found a real diagnostic error: the probe retained the last substep's crank delta
but the tick report overwrote its duration with total tick time. Dividing that delta by the tick
understated inferred crank acceleration. The same report also overwrote the probe's side forces
with tick means, mixing another interval.

The probe now retains one coherent last mechanical interval (including its `h` and forces),
while `TransmissionReport` still supplies whole-tick mean forces to the Driver. The forecast
consumer reads the paid cut's tick duration from `sim_tick::sim_timestep`, independently of the
mechanical probe's duration. Its original same-instant residual assertion and Driver resolution
are unchanged. This correction changes diagnostics, not mechanical state or force laws.
Both original landing witnesses pass with their assertions and resolution unchanged.

The second full library run was **MEASURED: 2839 passed, 1 failed, 51 ignored**. The remaining
failure was the standing-matter energy book. It assumed one force/speed interval per tick and
charged the conveyor at the published end speed. **MEASURED:** its old accounting reported
9208.256 J against the unchanged 9081.167 J engine bound at fixture tick 192. That accounting
cannot represent the sum of the relocated intervals.

`BeltWorkProbe` now integrates each substep's sprocket work and time-centred travel. The book
subtracts belt kinetic gain and the previously billed hard reaction's work from that integrated
work. The current narrow-phase hook reads canonical belts before the substep loop, so its
conveyor charge uses tick-start belt speeds. This changes only diagnostic accounting; the engine
energy bound and all physical laws remain unchanged. The corrected book and both landing beds
passed in the 37-test focused preflight and again in the final full suite.

## Verification

All checks below ran after the last Rust source edit, with `OVERMATCH_UNATTENDED=1` and
`OVERMATCH_WINDOW=headless` for headless runs. Compiling commands used `scripts/serial-cargo`.

- `cargo fmt --all --check`: passed.
- CI clippy, `--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`: passed.
- Required `track::`, `trace::`, `net::protocol`, `tests::doc_citations` run with
  `--lib --features dev_ui,bitprobe`: **MEASURED: 413 passed, 0 failed**.
- `check --no-default-features --bin overmatch-server`: passed, with three existing dead-code
  warnings (weapon audio clips, engine-sound fields, UI steering-governor readout).
- Full library suite, `test --lib --features dev_ui,bitprobe`: **MEASURED: 2840 passed,
  0 failed, 51 ignored**. The ignored cost and physical probes were also run explicitly and passed.
- Wire registration, codec and type-manifest pins passed; no wire definition changed and
  `PROTOCOL_REV` remains **106**.

Final logs: `/tmp/drive-relocate-final-{fmt,cost,slab,park,pivot,targeted,server,suite}.log` and
`/tmp/drive-relocate-clippy-final.log`. The source stayed fixed throughout final verification.
The orchestrator's independent six-lens review and merge remain outside this building lane.
