# Drivetrain slice 2: crank, clutch, steering

Subject: Overmatch's video-game driving physics, game state only. Lane `lane/drive-crank`, bay 2.
Initial implementation: `4be2f99d`. Base: `5c76e105`. Review closure is recorded below.
The implementation starts from the reviewed slice-1 block/stops base. No Avian, contact law,
per-substep relocation or parallel-group change is included. No authored force, inertia, engine
curve or steering radius was tuned to recover a prior observation.

## Construction and rows

| Design row | Implementation |
|---|---|
| Crank speed | A real f64 block coordinate with the authored rotational inertia. The existing torque curve, fuel governor, idle catch and drag supply its free impulse. There is no post-solve crank settlement or energy cap. |
| Clutch | One row per declared `Clutch(Mean)`, `Clutch(Left)` or `Clutch(Right)`. The mean row is `omega_e - dir*k*(vL+vR)/2 = 0`; side paths use their own output. Bounds express open, engine-brake-only or bidirectional drive, narrowed by the crank guard budget. Lock/slip are solved outcomes. |
| Steering | Declared `Steering` supplies `d - sign(steer)*c*omega_e = 0`; straight has `c=0`. Tight/wide crank ratios derive from the first radius pair. Every forward/reverse radius checks the same ratio at load. Steering impulse reacts on the crank as well as the belts. |
| Stops L/R | Declared `Stop(Left)` and `Stop(Right)`, joint with crank, clutch and steering. Park/service are two-sided; launch is one-sided. Static capacity requires exact starting rest and a lever. A held park retains that static budget under a service request; the row is not duplicated. Breakaway re-solves at dynamic bounds in the same call. |
| Mean hold | Optional `Stop(Mean)`, a passive latch. It keeps its common-mode static support through steering release until the straight tie arrests differential motion and both belts start a tick exactly at rest. Only then do park or the one-way launch stops take over. Its required `Unknown` sheet rating remains 192 kN, independently authored from the side rating. |
| Crank ceiling | One unilateral row per connected clutch output at `crank_limit/k`: a mean clutch protects mean speed, side clutches protect their own side speeds, each using the service device once. A staffed Driver enables it outside a shift and other full stops; a sampled crank-speed test does not switch it. Independent crank guard bounds remain. |
| Contact | Deferred to slice 4. The sampled affine ground law and late solver-contact reaction publication remain. |

`Coupling` is construction data in the tank sheet and the inline fixtures. The generic production
assembler reads that list. A mean stop requires a steering tie, so differential motion can be
arrested during handover. Both side stops are required for park, service and direction handover;
missing or duplicate connections are refused before play, naming the construction and repair.
The two-side-clutch fixture supplies no steering tie and drives, turns and stops through `step`,
including that same data admission and row assembly.

The shared crank guard bounds the actual linear combination of all crank-side row impulses.
`block::ImpulseLimit` is a generic convex-polytope facility with no drivetrain knowledge. The
exact search includes the active combination face and its KKT multiplier, so unused steering
rating remains available to the clutch. The independent polygon checker proves momentum,
feasibility and the variational inequality over every vertex, including redundant physical rows.
A box-assignment feasibility filter would lose the optimum: unit free speeds `(1,1)`, unit
inertias, impulse boxes `[-2,2]`, and `p1+p2 >= -1` minimize at `(-0.5,-0.5)`, where both impulses
are interior but both physical slips are `0.5` (DERIVED). The bordered solve represents it exactly.

## Driver rules

| Decision | Result |
|---|---|
| P1: active couplings | No speed test chooses a clutch or steering regime. Intent and paid interruption set bounds; the block decides lock/slip. `ClutchState` is solved feedback consumed by the Driver and displays: `Out` means an open order, while a closed order reports `Slip` or `In`. Zero capacity under a guard does not manufacture a free gear selection. |
| P2: passive stops | Park and mean hold latch from the named passive-entry conditions and survive speed changes or breakaway. Intent releases them. Every mean-stop exit retains it until its replacement can start at exact rest; propulsion waits during that transfer. |
| Gear/ladder/shift | Existing gear law and authored interruption retained. Reversal countdown advances only with both belts exactly at rest; any motion refreshes it. Park stays two-sided through the pause and paid cut. A launch lever is not ordered during a cut. |
| Clutch | Open through a shift or the initial state with no sensed gear choice; Unchosen, TakingUp and Locked are distinct states. At a staffed station with no propulsive pedal: engine-brake-only. With pedal: both directions, launch floor until the first solved lock. The initial no-ledger state is independent of speed. |
| Stops | Brake intent (including a phase hold) or an opposite-ladder request orders service. A request along the ladder orders the rollback-opposing launch lever, replacing the old against-motion service switch. Released rollback/sub-idle stop and exact rest can latch park. |
| Pivot | A steer-only request releases side park. A constructed mean stop can latch below the data-owned entry speed or on rollback, and persists through overload. Release, Brake, either ladder request and loss of staff share exact-rest handover. Requests center the tie and defer propulsion until the transfer can finish. A construction without the mean stop can roll without crossing a mechanical regime boundary. |
| Free gear selection | A genuinely unchosen initial/new-ladder launch can select for free before closing. Starts/rollback use the launch range; a rolling unchosen entry may select any safe gear. A paid gear shift has already selected its ratio and retains it when closing. Prices retain the signed shaft speed. A real overrun with no safe landing retains its gear under the guards. |
| Candidate pricing | Candidate acceleration uses the production row assembly with hull-coupled inertia. Sustained/landing fuel prices retain the engine-law envelope. Paid reverse rev matching reads the signed shaft along its ladder. |

Without an operator the existing spring clutch remains passive and closed in both directions;
served fuel and idle catch are zero, so it can absorb rollback into a backward crank without a
new active regime. Staffed released-pedal operation retains the engine-brake-only bound.

Clutch lock/slip, brake static/dynamic, geared turn/pivot and engine braking/free roll are outcomes.
The design narrative is in `transmission-design.md`; module comments state local interfaces and
invariants.

## Authored data

- Deleted `neutral_throttle`, the copied `TICK_HZ`, the 24-step bisection, the work gate and
  `recirculation`, including spec adapters, inline/Blender fixtures and obsolete trace fields.
- `dead` and `park_engage_speed` are required Taste data in `assets/driving/actuation.ron`.
  Rest perception still serves sensing/wake decisions, never friction, clutch or swap selection.
- `neutral_speed_m_s` remains a Taste for passive pivot entry, with no neutral regime selector.
- The radius table has an explicit one-percent relative rounding bound, labelled Resolution.
  The shipped radius values are unchanged. Synthetic tables that contradicted their own gear
  ladders were corrected to their construction relationship; they are Fixture data.
- `brake_force` and `brake_static_factor` retain their values and now carry typed Unknown labels
  naming the missing rating/comparison and what would settle them. The sheet records the 20-degree sizing inference (DERIVED 0.343 g), the rejected grip-limit
  sizing, and the inferred static-factor range 1.05-2.0. Future measured ratings can replace the unknowns without changing a law.
- The old prescribed `neutral_d_full` field is gone. Instruments can read the tight member's
  speed at the governor as derived metadata; the solver always uses the actual crank.

## Proof and issue classes

Every new proof names the invariant and its consumer; inputs are inline Fixture data. Numerical
allowances cover f32/f64 arithmetic rather than an earlier observation.

| Proof | Evidence |
|---|---|
| 6,048 fixed-intent states (DERIVED count) | Four constructions (including independent side clutches), each valid mean path, all three launch phases, staff/latch variations, unequal loads and reaction slopes, both ladders, three load classes, three requests, straight/wide/tight steer, both entry windows and motion directions. The actual drivetrain integrates throughout. Within each fixed Driver phase, at most one settling transition and no subsequent order flips; a held reversal permits at most one direction cut. Paid direction/cut phases are explicit authorised transitions, not counted as chatter. The absent sensing ledger isolates gear decisions from the row-order test. |
| No free energy | Without combustion or external work, total crank-plus-belt kinetic energy cannot rise, including steering and asymmetric rolling inputs. Affine contacts additionally satisfy the backward-Euler energy inequality after billing their external endpoint work across launch, staff and latch states. Existing free-crank tests prove drag cannot reverse it or increase its energy. A solved steering impulse cannot be billed to the belts alone. |
| Locked radius | Across both ladders and detents on every class fixture with a Steering member, synchronous unloaded rows hold and reproduce `R=b*abs(mean/d)` within the authored rounding bound plus subtraction roundoff. The expected half-tread comes from its construction owner. |
| Static handover | Unequal loads and slopes with tight -> wide -> straight steering transfer existing support to park/service/either ladder request from exact rest when the full demand fits both devices. A separate derived overload witness proves that a below-rating starting reaction sum can still break away under differential deceleration. Loss of staff retains its passive support through stored-crank discharge; the shipped plugin proves eventual supported park and sleep on level ground. |
| Two side clutches | The production path drives both belts, opens/brakes the inner path to turn, and stops both with service, with no steering tie. |
| Combined crank guards | Both clutch and steering reactions together remain within freely reachable floor/ceiling bounds. No settlement clamp supplies missing energy. |
| Load admission | Bad radius relationships, incomplete/duplicate coupling lists and a mean stop without its handover mechanism are refused before play. |
| Reverse matching | A paid shift gives the same crank response for equivalent signed shaft speeds on either ladder. |

The slice-1 independent block optimality, redundant-row, scaled-unit and breakaway checks remain.

- #392 case 1 and #373: no pedal forbids the clutch impulse that drives the belts. An idle catch
  can spin the crank but cannot power the old ladder against a pending reversal.
- #392 case 2: there is no pivot/geared selector to flicker. An overloaded hold slides at dynamic
  capacity; engine braking can add through its row when the shaft actually overruns the crank.
- #392 case 3: free selection receives the signed shaft speed; rollback is not priced as rest.
  The one-way launch lever covers rollback under an along-ladder request.
- #392 case 4: a Free mean has no speed-window regime, and park is not toggled at its edge.

## Deletions

The following are DERIVED definition-line counts against the lane base, excluding their leading
comments. They describe removed implementations, not a line-count target.

| Removed implementation | Definition lines |
|---|---:|
| `clutch_coupling` | 45 |
| `update_clutch_state` | 27 |
| `engagement` | 16 |
| `settle_crank` | 42 |
| `engine_work` | 45 |
| `recirculated_work` | 10 |
| `steering_force` | 55 |
| `steering_tie` | 14 |
| `neutral_turn` | 4 |
| `neutral_pivot` | 3 |
| `apply_ceiling_brakes` | 28 |
| `crank_guards` | 17 |
| `stop_rows` | 129 |
| `tied_mean` | 10 |

Those definitions total **445 lines DERIVED**. `BeltTick` and `CrankTick`, their remaining
predictor helpers, selector state transitions, obsolete gate/probe fields and regime-only test
arms also disappear. `ClutchState` itself remains the wire/readout enum, with no engagement
selector. The retired test blocks total **1,305 lines DERIVED** (including their comments);
retained physical witnesses were amended to the crank tie and row outcomes. No one-for-one test
replacement or old-reading pin was added. The canonical hash pin is an interface exception, not
a physics observation.

Retired test arms:

- `a_held_mean_pivots_in_place_on_a_slope_and_a_free_mean_does_not`.
- `a_held_mean_gives_way_beyond_its_capacity_and_slides_once_breached`.
- `energy_bound_no_free_energy`.
- `recirculation_splits_physical_output_powers`.
- `pivot_authority_is_per_output_capacity`.
- `steady_launch_slip_transmits_the_bounded_free_torque`.
- `stall_guard_holds_crank_under_grade_lug`.
- `free_rev_parks_in_the_governor_band_beneath_the_steer_target`.
- `a_geared_turn_integrates_the_crank_onto_the_steered_belts`.
- `the_coupled_belt_settles_on_a_stiff_ground`.
- `clutch_states_cover_rest_launch_lock_and_shift`.
- `supply_loss_reaches_a_locked_clutch_on_its_tick`.
- `pure_engine_braking_does_not_pay_an_inner_to_outer_transfer_loss`.
- `zero_task_rate_springs_clutch_in_at_rest_and_in_motion`.
- `empty_station_at_rest_springs_engaged_and_retains_park`.
- `moving_lug_does_not_restart_launch_slip`.
- `landing_resolution_bounds_independent_weight_rigs`.
- `law_arrested_climb_reenters_launch_slip`.
- `released_motion_coasts_without_brake_force`.
- `released_rollback_stops_and_parks`.
- `observe_released_rollback`.
- `neutral_pivot_keeps_its_mean_hold_as_the_only_stop`.
- `steering_rollback_does_not_close_the_main_clutch`.
- `staffed_zero_rate_stops_and_parks_without_engine_work`.
- `backward_live_crank_has_no_idle_floor`.
- `virtual_free_crank_energy_does_not_cap_curve_power`.
- `reverse_radii_belong_to_the_reverse_gear`.
- `a_curve_tail_does_not_move_the_crank_envelope`.
- `a_ladder_that_cannot_launch_inside_the_crank_limit_is_refused_before_play`.
- `forward_crank_rollback_keeps_gear_under_service`.

## Wire and initial verification

Wire changed: `TransmissionState` adds the mean-stop latch and explicit launch phase. Its exhaustive
projection and probe state schema contain ten fields; `LaunchState` is included in the wire type graph. `PROTOCOL_REV` advances 102 -> 103 in this
lane; `WIRE_TYPES_HASH` is `0x52df1f2c325e8ff4`. Registration/surface names are unchanged. The exact
canonical-state interface vector is re-pinned for the extra fields, and the inventory test mutates
both new fields independently. Protocol tests verify the pin and compatibility contract.

Initial implementation validation (MEASURED): **full library suite, run once: 2853 passed, 0 failed,
50 ignored, 349.84 s test execution**. The full run includes the corrected unmanned fixture,
all load refusals, protocol/trace pins and production driving tests. The requested ignored
measurement runners were also run explicitly as detailed below.

Validation commands use `OVERMATCH_UNATTENDED=1` and `--features dev_ui,bitprobe`; every compile
runs through `scripts/serial-cargo`. `cargo fmt --all --check` and CI clippy pass.
`track::`: **361 passed**; `tests::doc_citations`: **3 passed**; `net::protocol`: **26 passed**;
`trace::state_hash`: **7 passed**. The production headless batch had 54 passes and one invalid
overspeed fixture failure; its corrected unmanned-coast test passes separately and in the full-library run. All requested observation runners pass.

The implementation was checked locally with the six lenses, without spawning reviewers (the lane
brief forbids agents). The orchestrator still owns the independent pre-merge six-lens review.
The manifest-only edit in `protocol.rs` does not touch the existing `net-22` servo-presentation
backlog item; that issue remains with its owner decision, outside drivetrain scope.

## Design claims corrected and decisions

The design's Driver table governs staffed orders. An absent operator retains the existing
passive spring clutch, bidirectional with no fuel or catch; it is not a new speed-selected
regime. This is an explicit retained passive construction policy.

- Crank energy alone can rise without fuel by absorbing belt energy. The valid passive proof is
  total crank-plus-belt energy with no fuel or external work, plus a free-crank dissipation proof.
- One ratio per steering detent is approximate in the rounded table. The load gate states its
  rounding allowance; it neither restores each table entry as a separate law nor retunes radii.
- A clutch-only crank guard ignores steering's crank reaction. The generic aggregate bound
  protects their actual sum without reserving unused ratings.
- A launch-pending bit conflates a genuinely unchosen launch with take-up of an already selected
  paid shift. `LaunchState` makes that distinction explicit; it prevents a new free selection on
  closing a cut. The selected ratio and the data's one-window selector limit stay coherent.
- The force law cannot combust a backward crank. Gear pricing cannot purchase a paid cut
  against forward combustion while the actual crank is backward; row orders stay intent-based.
- The unmanned coast fixture started its locked crank above the asserted crank guard: its fixed
  5 m/s entry in a launch gear exceeded the gear's crank-limit speed. Its entry now respects the
  gear's data-derived governor. Narrowed coupling bounds cannot teleport an invalid initial crank
  back under a guard. The independent motoring-force balance and guard assertions remain.
- The brake-publication test demanded a fully saturated contact sample within 1 N. That trajectory
  is not a contract of coupled recoverable shear. The composition test compares delivered
  traction plus rolling against the bots' actual forecast capacity (`delivered <= forecast`)
  and reports unused capacity as an observation.
- A one-way lever must not replace the two-sided park while a direction cut is still paid.
  The retained park now spans the whole pause/cut, before the new intent releases it.
- The soil's K bounds recoverable strain per element, not global travel of a rotating hull.
  The production stop test now checks element strain and reports body rebound as an observation.
- Brake force and static factor were not sourced/derived ratings; the sheet now states Unknown.
- The design's line references refer to an earlier tree. Slice 1 had already removed the old
  brake/hold force passes; slice 2 replaced its actual two-speed `stop_rows` assembler rather
  than reintroducing those passes. The rigid-wall probe is still absent, as the design notes.

## Probe observations

All readings in this section are **MEASURED** at the shipped fixed tick. Yaw is the closing
three-second mean; release travel is planar displacement over the five-second release window,
not path length. Entry-gear labels identify probe setup, not a promise that the Driver retains
that gear. Each turn case keeps its throttle through steer release. No reading is a test target.

Run counts: the `turn_probe` filter runs both the 55-case turn sweep and the 16-case pivot sweep.
It ran once on each version; standalone `pivot_probe` adds a second full pivot sweep per version.
The pivot sweeps reproduce their same-version readings. Slab-pull and rammed rubble have two
independent runs per version; each pair reproduces exactly. Flat/house/yard rubble ran once per
version. Extra across-slope diagnostic runs are described below.

### Turn sweep

Every value below is MEASURED; speed and yaw are signed in the probe frame.

| Entry case | Yaw before -> after (rad/s) | Crank before -> after (rpm) | Release before -> after (m) |
|---|---:|---:|---:|
| flat neutral pivot | -0.128220 -> -0.105812 | 2045.05 -> 2047.90 | 0.000641 -> 0.000363 |
| flat g1 wide | -0.046974 -> -0.046824 | 2557.84 -> 2560.37 | 20.184256 -> 20.226742 |
| flat g1 tight | -0.136724 -> -0.136706 | 2552.38 -> 2554.67 | 19.748479 -> 19.790282 |
| flat g2 wide | -0.046974 -> -0.046824 | 2557.84 -> 2560.37 | 20.184256 -> 20.226742 |
| flat g2 tight | -0.136724 -> -0.136706 | 2552.38 -> 2554.67 | 19.748479 -> 19.790282 |
| flat g3 wide | -0.046974 -> -0.046824 | 2557.84 -> 2560.37 | 20.184256 -> 20.226742 |
| flat g3 tight | -0.136724 -> -0.136706 | 2552.38 -> 2554.67 | 19.748479 -> 19.790282 |
| flat g4 wide | -0.046974 -> -0.046824 | 2557.84 -> 2560.37 | 20.184256 -> 20.226742 |
| flat g4 tight | -0.136724 -> -0.136706 | 2552.38 -> 2554.67 | 19.748479 -> 19.790282 |
| flat g5 wide | -0.047668 -> -0.047533 | 2552.04 -> 2555.91 | 24.355520 -> 24.380325 |
| flat g5 tight | -0.137164 -> -0.137165 | 2546.41 -> 2550.09 | 23.953317 -> 23.988416 |
| flat g6 wide | -0.048789 -> -0.048722 | 2544.31 -> 2549.79 | 30.518109 -> 30.546216 |
| flat g6 tight | -0.137713 -> -0.137764 | 2538.10 -> 2543.69 | 30.221647 -> 30.240067 |
| flat g7 wide | -0.049835 -> -0.049722 | 2534.01 -> 2541.19 | 38.735549 -> 38.762403 |
| flat g7 tight | -0.138973 -> -0.139026 | 2526.39 -> 2534.54 | 38.496770 -> 38.518623 |
| flat g8 wide | -0.045846 -> -0.046923 | 2310.92 -> 2382.20 | 52.431358 -> 53.007408 |
| flat g8 tight | -0.119641 -> -0.124230 | 2153.30 -> 2248.60 | 49.792798 -> 51.648485 |
| flat g3 mirrored tight | 0.136724 -> 0.136706 | 2552.38 -> 2554.67 | 19.748494 -> 19.790176 |
| flat g3 lifted tight | -0.128208 -> -0.105809 | 2045.05 -> 2047.90 | 0.000458 -> 0.000348 |
| flat g5 mirrored tight | 0.137164 -> 0.137165 | 2546.41 -> 2550.09 | 23.953386 -> 23.988309 |
| flat g5 lifted tight | -0.030591 -> -0.105808 | 567.31 -> 2047.90 | 3.769061 -> 0.000329 |
| flat g1 third-throttle tight | -0.115389 -> -0.129566 | 2155.23 -> 2422.60 | 11.166656 -> 11.934894 |
| flat g2 third-throttle tight | -0.115389 -> -0.129566 | 2155.23 -> 2422.60 | 11.166656 -> 11.934894 |
| flat g3 third-throttle tight | -0.115389 -> -0.129566 | 2155.23 -> 2422.60 | 11.166656 -> 11.934894 |
| one-cell g2 tight | -0.136724 -> -0.136706 | 2552.38 -> 2554.67 | 19.748479 -> 19.790282 |
| one-cell g4 tight | -0.136724 -> -0.136706 | 2552.38 -> 2554.67 | 19.748479 -> 19.790282 |
| one-cell g4 wide | -0.046974 -> -0.046824 | 2557.84 -> 2560.37 | 20.349293 -> 20.392779 |
| one-cell neutral pivot | -0.128220 -> -0.105812 | 2045.05 -> 2047.90 | 0.000641 -> 0.000363 |
| slope2 across neutral pivot steer1 | -0.127934 -> -0.105619 | 2045.10 -> 2047.94 | 0.002609 -> 0.001331 |
| slope2 across neutral pivot steer-1 | 0.127814 -> 0.105580 | 2045.10 -> 2047.93 | 0.002089 -> 0.001297 |
| slope2 uphill neutral pivot steer1 | -0.128391 -> -0.105868 | 2045.12 -> 2047.97 | 0.003529 -> 0.002501 |
| slope2 uphill neutral pivot steer-1 | 0.128391 -> 0.105868 | 2045.12 -> 2047.97 | 0.003529 -> 0.002501 |
| slope5 across neutral pivot steer1 | -0.126055 -> -0.104491 | 2045.37 -> 2048.10 | 0.005982 -> 0.003199 |
| slope5 across neutral pivot steer-1 | 0.125754 -> 0.104384 | 2045.34 -> 2048.10 | 0.005515 -> 0.003185 |
| slope5 across g1 tight | -0.129685 -> -0.129661 | 2537.30 -> 2538.66 | 14.985031 -> 15.001163 |
| slope5 across g2 tight | -0.129685 -> -0.129661 | 2537.30 -> 2538.66 | 14.985031 -> 15.001163 |
| slope5 across g4 wide | -0.046238 -> -0.046092 | 2550.58 -> 2552.66 | 17.993777 -> 18.021309 |
| slope5 uphill neutral pivot steer1 | -0.128733 -> -0.105617 | 2045.47 -> 2048.28 | 0.008133 -> 0.005801 |
| slope5 uphill neutral pivot steer-1 | 0.128733 -> 0.105617 | 2045.47 -> 2048.28 | 0.008133 -> 0.005801 |
| slope5 uphill g1 tight | -0.135164 -> -0.135146 | 2548.96 -> 2550.98 | 19.679056 -> 19.704981 |
| slope5 uphill g2 tight | -0.135164 -> -0.135146 | 2548.96 -> 2550.98 | 19.679056 -> 19.704981 |
| slope5 uphill g4 wide | -0.044880 -> -0.044734 | 2543.93 -> 2545.49 | 16.075351 -> 16.105270 |
| slope10 across neutral pivot steer1 | -0.118907 -> -0.100196 | 2046.24 -> 2048.69 | 0.011373 -> 0.006443 |
| slope10 across neutral pivot steer-1 | 0.118258 -> 0.099916 | 2046.21 -> 2048.67 | 0.011164 -> 0.006497 |
| slope10 uphill neutral pivot steer1 | -0.128610 -> -0.103286 | 2046.73 -> 2049.38 | 0.016563 -> 0.011384 |
| slope10 uphill neutral pivot steer-1 | 0.128610 -> 0.103286 | 2046.73 -> 2049.38 | 0.016564 -> 0.011384 |
| pieces neutral pivot | -0.082457 -> -0.098219 | 2041.20 -> 2049.51 | 0.001915 -> 0.001628 |
| shipped-gentle g2 tight | -0.132446 -> -0.132425 | 2545.55 -> 2547.44 | 18.027234 -> 18.061497 |
| shipped-gentle g4 tight | -0.132446 -> -0.132425 | 2545.55 -> 2547.44 | 18.027234 -> 18.061497 |
| shipped-gentle g4 wide | -0.045572 -> -0.045423 | 2549.24 -> 2551.21 | 17.160622 -> 17.193612 |
| shipped-gentle neutral pivot | -0.128784 -> -0.105542 | 2045.41 -> 2048.14 | 0.006256 -> 0.004007 |
| shipped-rough g2 tight | -0.127502 -> -0.159932 | 2584.31 -> 2884.58 | 25.510817 -> 62.254679 |
| shipped-rough g4 tight | -0.127502 -> -0.159932 | 2584.31 -> 2884.58 | 25.510817 -> 62.254679 |
| shipped-rough g4 wide | -0.047541 -> -0.053650 | 2578.96 -> 2742.70 | 27.559413 -> 62.383562 |
| shipped-rough neutral pivot | -0.122623 -> -0.102670 | 2046.18 -> 2048.89 | 0.011535 -> 0.006873 |

### Pivot sweep

Every value below is MEASURED. Both 25-degree along-slope cases end with exact-rest belts and
park latched. The across-slope case instead breaks its mean stop; it is not a side-stop handover.

| Case | Yaw before -> after (rad/s) | Release before -> after (m) |
|---|---:|---:|
| pivot flat | -0.128212 -> -0.105810 | 0.000545915 -> 0.000328684 |
| pivot slope5 uphill | -0.129147 -> -0.103650 | 0.006845549 -> 0.005160439 |
| pivot slope5 downhill | -0.128560 -> -0.103164 | 0.005790648 -> 0.004863878 |
| pivot slope5 across | -0.125650 -> -0.106140 | 0.007335906 -> 0.003655665 |
| pivot slope10 uphill | -0.130111 -> -0.096242 | 0.015785744 -> 0.008948038 |
| pivot slope10 downhill | -0.128636 -> -0.095409 | 0.014897556 -> 0.008880661 |
| pivot slope10 across | -0.117816 -> -0.107601 | 0.012300717 -> 0.009029717 |
| pivot slope15 uphill | -0.116515 -> -0.088730 | 0.025218397 -> 0.011434802 |
| pivot slope15 downhill | -0.113761 -> -0.087805 | 0.024703642 -> 0.011588993 |
| pivot slope15 across | -0.114937 -> -0.103962 | 0.014553654 -> 0.017068779 |
| pivot slope25 uphill | -0.007204 -> -0.083505 | 4.053671056 -> 0.023488267 |
| pivot slope25 downhill | -0.014843 -> -0.124658 | 3.982859879 -> 0.042794878 |
| pivot slope25 across | -0.082079 -> -0.006261 | 0.046990164 -> 4.313269076 |
| pivot slope35 uphill | -0.000109 -> -0.000013 | 0.000712606 -> 0.029692020 |
| pivot slope35 downhill | 0.001824 -> 0.000385 | 0.038547337 -> 0.001236952 |
| pivot slope35 across | -0.000095 -> 0.000408 | 0.000782186 -> 0.001175559 |

Flat pivot belt speed changes from +/-0.287957 to +/-0.235883 m/s MEASURED, at
2045.05 -> 2047.90 rpm MEASURED. Its speed now follows crank speed instead of a prescribed
neutral target. The 35-degree hull slides remain contact/grade observations; a held belt mean
does not imply a held hull on a sliding ground contact.

"Across" denotes only the starting heading. The 64-second held pivot releases at heading
1.722 rad (MEASURED), 8.7 degrees beyond the fall line (DERIVED), after 12.3 m hold travel
(MEASURED). From about 20 s it stalls at about -0.006 rad/s while backing downhill at
-0.229 m/s (MEASURED); the right belt grips and the left slides.

The 57 t weight is 559.2 kN, the 25-degree grade is 236.3 kN, and its along-hull share at release
is `236.3 sin(1.722) = 233.6 kN` (DERIVED; f_long 233.8 kN MEASURED). Static mean capacity is
288 kN and dynamic is 192 kN (DERIVED). At tight-to-wide, MEASURED starting belts are
`[0.22872825,-0.22872825]` m/s, reaction slopes `[83.25533,806006.6]` N s/m and reactions
`[140303.55,76738.63]` N. The crank drops 1985.8 -> 1966.2 rpm (MEASURED), so the half-difference
drops 0.2287 -> 0.0763 m/s (DERIVED from the construction ratio 3.44/10.2), Delta=0.1524 m/s.

With mean constrained to zero, summing the implicit belt equations cancels equal base belt
inertia and gives `hold = R_L + R_R + (s_R-s_L) Delta` (DERIVED). Here the 217.0 kN starting load
(MEASURED) plus the 122.8 kN differential-step term gives 339.9 kN (DERIVED), agreeing with the
339.79 kN replay (MEASURED), beyond 288 kN. The extra term is differential motion loading the
asymmetric soil response with the hull frozen for this solve, not gravity or a wrong mean row.

The broad invariant "starting load inside static capacity stays at rest through any handover"
is false. The supported invariant requires the FULL coupled demand, including differential
changes, to fit static capacity; the existing stop persists until replacement starts at exact
rest. The proof now covers unequal loads/slopes and tight -> wide -> straight. No detent patch
or rating change hides breakaway. About 22 kN rolling plus 192 kN dynamic stop is only 214 kN
against 233.6 kN grade, leaving 0.33 m/s^2 downhill (DERIVED); the logged 0.30 m/s^2 is MEASURED.
Dynamic support ends at 20.1 degrees, static at 31.0 degrees (DERIVED, without rolling). In that
band an undisturbed rest can hold while a breached slide cannot be arrested by this rating.
Before this slice, uphill/downhill ended with about 236 kN along the hull and slid; across ended
at heading 2.406 rad with 158 kN and held (MEASURED). Afterwards, across slides at 233.6 kN while
uphill/downhill hold at 197/50 kN (MEASURED). Which starting heading reaches the band changed;
the class did not. The owner's unresolved rating and gearbox-input construction are recorded as
Unknown data; contact/hull response within the tick belongs to slice 4.

Shipped-rough full-throttle release grows 25.5 -> 62.3 m and crank 2584 -> 2885 rpm (MEASURED,
initial slice comparison). Full throttle downhill at a crank-limited top gear explains the
class, but the upper-end motion remains for owner judgement. At 35 degrees every starting
heading slides 365-387 m during hold in both initial versions (MEASURED); ground grip 0.574 is
below tan(35 degrees)=0.700 (DERIVED), so belt mean support cannot prevent hull ground sliding.

### Slab-pull

Every value below is MEASURED. Alternation is the probe's detrended adjacent-tick amplitude;
friction is whole-hull delivered longitudinal force. Grouping uses solved clutch outcomes.

| Quantity | Before | After |
|---|---:|---:|
| Final hull speed (m/s) | 3.568040371 | 3.368829727 |
| in: left-belt alternation (m/s) | 0.104372494 | 0.011465541 |
| in: delivered friction alternation (kN) | 255.921600 | 79.586403 |
| in: classified ticks | 179 | 117 |
| slip: left-belt alternation (m/s) | 0.153418779 | 0.099798590 |
| slip: delivered friction alternation (kN) | 250.589188 | 232.096436 |
| slip: classified ticks | 57 | 52 |
| out: left-belt alternation (m/s) | 0.266415715 | 0.274693251 |
| out: delivered friction alternation (kN) | 324.168060 | 335.805176 |
| out: classified ticks | 20 | 87 |

The contact reaction still arrives a tick late. Smaller locked-clutch alternation is an
observation, not evidence that #353 is solved; slice 4 owns that contact coupling.

### Rubble park

Every value below is MEASURED. Counts cover the 1280-tick observation window.

| Case | Travel before -> after (m) | Asleep before -> after (ticks) | Park before -> after (ticks) | Moving belts before -> after (ticks) |
|---|---:|---:|---:|---:|
| flat | 0.000179421 -> 0.000179421 | 1280 -> 1280 | 1280 -> 1280 | 0 -> 0 |
| rammed | 10.340805054 -> 0.020582624 | 533 -> 1280 | 563 -> 1280 | 717 -> 0 |
| house | 0.040540382 -> 0.001984692 | 1280 -> 1280 | 1280 -> 1280 | 0 -> 0 |
| yard | 0.003599836 -> 0.005523772 | 1280 -> 1280 | 1280 -> 1280 | 0 -> 0 |

The rammed arrival differs with the drivetrain, so this compares two complete probe executions,
not identical parked poses. Original parked-pose comparisons remain slice 5. The disappearance of
released creep is visible here; it is not a changed sleep or contact rule.

## Independent-review closure

### Dispositions

- **Lens P1:** fixed in `track/block.rs` and `transmission::advance_rows` with generic
  `ImpulseLimit`, active combination faces and bordered KKT equations. The bound applies to
  actual combined crank impulse; the advance reservation is deleted. Checker proof and the
  interior-sliding counterexample are described above.
- **Review P2-1:** `mean_hold_force` is required when `Stop(Mean)` exists and forbidden without
  that construction, finite and positive with its static capacity checked at load. The sheet
  retains 192 kN with `Unknown` provenance. No independent simultaneous brake linkage is
  established; the existing mean/side exclusion stays. No gearbox-input torque law is added.
- **P2-2:** one gate clears the mean latch, only when both belts start at exact rest. Release,
  Brake, both ladder requests and loss of staff use it; requested motion centers steering and
  defers propulsion while pending. Along-ladder intent then uses the statically admitted one-way
  launch lever; other paths retain park through rest, reversal pause and paid cut. The state-space
  proof asserts this gate for every observed exit, and the ramp proof holds steer during both
  ladder requests to catch a bypass or a handover that could never finish.
- **P2-3:** the state-space proof covers side clutches, all launch phases, staffed/unstaffed,
  an already-latched hold, unequal loads, and nonzero unequal reaction slopes. Handover includes
  a tight/wide/straight ramp; energy includes an affine-ground external-work account. The broad
  starting-load handover invariant is rejected by the summed-equation derivation above.
- **P2-4:** one-crank-ratio admission, kappa and member capacity apply only to a constructed
  `Steering` member. The side-clutch Fixture has its own gear-independent radii and runs through
  production admission and `step`; it supplies no steering member capacity or Driver kappa price.
- **P2-5:** the production contact adapter compares delivered braking with the bots' forecast
  (`delivered <= forecast`), recording the gap rather than requiring saturated stops.
- **P2-6:** the sheet restores mean-hold citations, brake sizing and static-factor range; the
  design owns the short "Neutral mean hold" and "Released-motion policy" rationale, rejected
  alternatives and the owner's no-gearbox-input-brake ruling.
- **P2-7:** the across-slope account is corrected above. Both sweeps print the release heading
  and planar hold travel as named MEASURED values, also included in the JSON record.
- **Review P3s:** `ClutchState` is documented as Driver/display feedback; the open-clutch comment
  states that the ceiling brake is absent. Unstaffed passive handover can reach side park and
  expose held rest to sleep. Kappa derives once and its field names `c G/r`. Inner and ceiling
  braking cannot bill the same side device twice. Trace capacity follows the projection length;
  the stale architecture constant reference is corrected. HUD `P` includes the mean latch and
  the design retains the open complete-brake-display question. `net-22` is untouched.
- **Lens P3s:** one helper owns the no-reversal drag bound; candidate pricing owns its
  implementation directly; nested state fixtures, duplicate driving assertion, steering alias,
  duplicate candidate rationale and readout prose are removed. The reversal pause assertion
  derives time from the fixture input's timestep.

### Gear walk and slab observation

MEASURED level full-pedal gear walk: `[1,3,4,5,6,7,8]`; every paid step respects `max_step`.
The earlier intermediate `[1,3,8]` is not reproduced. A paid cut retains its selected ratio on
closure; it cannot re-enter free initial selection. The production Fixture test checks that rule.

MEASURED slab-pull selector events: ticks 20 F2, 61 F3, 101 F4, 159 F5, 249 F8. There are no
lower-gear returns, so the extra cuts are not hunting. Four complete 20-tick interruptions plus
seven ticks of the fifth give 87 open ticks (DERIVED from the trace). The old open-clutch/free
selection path could advance ratios without paying all these cuts; the explicit selected-launch
phase now pays them. No Driver rule is changed to recover the old one-cut observation. Neither a
particular gear sequence nor monotonicity is a class invariant: a different authored torque curve
or load can legitimately require a downshift. Selector reach and paid-cut retention are class
invariants and remain proved.

MEASURED review slab-pull is unchanged from the initial slice-2 report: end speed 3.368829727 m/s;
In/Slip/Out counts 117/52/87; left-belt alternation 0.011465541/0.099798590/0.274693251 m/s;
whole-hull friction alternation 79.586403/232.096436/335.805176 kN. The straight solve does not use
the removed steering reservation. These are observations, not targets.


### Review probe readings

Each review mechanical version ran one complete `turn_probe` filter (55 turn and 16 pivot cases),
one slab-pull and one four-case rubble-park run. Candidate versions (aggregate bound, output
ceiling, unit-invariant solver, shared hold-exit gate) each have one recorded run; their readings
are retained in the metrics file. All four review versions reproduce identical readings for
these runners. The final ceiling rule follows the clutch output; none of these observations reached a state that distinguishes the intermediate
per-side ceiling. Final finite-system assertions change no admitted solve. Small text summaries
are in `drive-crank-slice-2-review-metrics.json`; raw logs remain outside git in `/tmp`.

All values in this table are MEASURED; "slice 2" is the reviewed implementation, not slice 1.
Release is planar net displacement over five seconds. Hold travel printed at release starts
immediately before the first held tick; old offline summaries used the first held sample.

| Case | Yaw slice 2 -> review (rad/s) | Crank slice 2 -> review (rpm) | Release slice 2 -> review (m) |
|---|---:|---:|---:|
| Flat pivot | -0.105810386 -> -0.105810393 | 2047.904836 -> 2047.904837 | 0.000328684 -> 0.000328684 |
| Flat F8 tight | -0.124229609 -> -0.124496819 | 2248.599407 -> 2253.341230 | 51.648485 -> 51.706278 |
| Flat F8 wide | -0.046922591 -> -0.046977794 | 2382.204876 -> 2384.894376 | 53.007408 -> 53.017342 |
| Pivot 25 deg uphill | -0.083505174 -> -0.083504148 | 2052.536924 -> 2052.536940 | 0.023488267 -> 0.023485437 |
| Pivot 25 deg downhill | -0.124657843 -> -0.124657965 | 2054.860987 -> 2054.861070 | 0.042794878 -> 0.042793903 |
| Pivot 25 deg across start | -0.006260826 -> -0.006260733 | 2055.862565 -> 2055.862583 | 4.313269076 -> 4.313269437 |
| Shipped rough F2/F4 tight | -0.159931920 -> -0.150779226 | 2884.580235 -> 2677.847009 | 62.254679 -> 62.460270 |
| Shipped rough F4 wide | -0.053649789 -> -0.047974520 | 2742.696822 -> 2711.334641 | 62.383562 -> 60.956724 |

The across-start 25-degree pivot releases at heading 1.721987605 rad after 12.318467140 m hold
travel (MEASURED); both along-start cases finish with parked zero-speed belts, across still slides.
The maximum release change from the reviewed slice is 1.426838148 m on shipped-rough wide, and
maximum yaw change is 0.009152693 rad/s on shipped-rough tight (MEASURED). Steering budget sharing
changes the driven trajectory and therefore which shipped terrain is sampled. No old trajectory
is a target. Rubble park reproduces the reviewed slice exactly: flat/rammed/house/yard travel
0.000179421/0.020582624/0.001984692/0.005523772 m, each with 1280 sleeping observation ticks
(MEASURED). Slab-pull and its five paid cuts are unchanged as recorded above.

### Closing verification and additional findings

MEASURED full-library run, once at the completed solver checkpoint: **2,865 passed, zero failed,
50 ignored, 625.58 s**. It included the production unstaffed-pivot sleep test and the ordinary
shipped-app tests. The all-exits hold gate and aggregate-unit sign certificate were tightened
after that run; the final source passed the full `track::` filter (**372 passed**, 6,048 Driver
entry configurations inside its state-space proof), the shipped unstaffed-sleep and full-pedal
gear-walk tests, and steering-capacity construction admission. The full library was not repeated.
`cargo fmt --all --check`, CI clippy with `--locked --workspace --all-targets --features
dev_ui,bitprobe -- -D warnings`, `tests::doc_citations` (**3 passed**) and `net::protocol`
(**26 passed**) pass. `PROTOCOL_REV` remains **103** and the normalized type pin remains
`0x52df1f2c325e8ff4`; this closure changes no serialized field or variant.

The braking forecast comparison passes on every tick of its stop/rebound window. MEASURED peak
forecast-minus-delivered force is **166,751.562 N**, against a published contact capacity of
**155,261.391 N**; the gap includes rebound and settled ticks. Peak stop travel is **1.860231 m**
and rebound **0.023943 m** (MEASURED). The force relation is an upper capacity bound; its unused
share is reported, never imposed as a saturation target.

Additional root-level findings closed in this change:

- A zero-width impulse is a fixed coordinate. Treating an open coupling as an interior speed
  equality could create roundoff force; the search now fixes its impulse exactly instead.
- Aggregate-face multipliers can leave physical interior rows sliding. Clutch feedback and static
  breakaway therefore read physical row slip, and speed snapping uses an independent physical
  equality basis. A combination can resolve dependent impulses without making duplicate speed
  equalities independent.
- Rank and multiplier-sign decisions must survive both row and aggregate unit changes. Symmetric
  KKT equilibration and row-derived multiplier allowances preserve that contract; independent
  polygon certificates cover both signs and the unit transforms. Unrepresentable effective-mass
  arithmetic fails visibly before NaN can pass comparisons.
- A ceiling belongs to its constructed clutch output. A mean clutch protects the mean speed;
  side clutches protect their own outputs, and an inner side stop cannot be billed twice.
- Every mean-stop exit uses one exact-rest gate, including an along-ladder request with steering
  still held. Propulsion waits for that transfer, rather than fighting the retained brake.
- Compact Fixture brake labels and its independent mean rating belong to that fixture, not to
  inherited seed provenance. No shipped rating was adjusted.

The four complete review turn/pivot sweeps, four slab pulls and four rubble-park sweeps pass.
Raw logs remain in `/tmp`; only the **125 kB MEASURED text summary** is committed. No hosted
run or owner play is claimed; those observations remain with the orchestrator's release judgement.

## What slice 3 needs

Relocate this same mechanical integration to substeps. Separate tick-rate Driver orders and sampled
loads from `advance_rows`, retain one state/impulse path for the server, sandbox and resting trial,
commit crank/belts/phases at the declared PostSubstep point, and prove equivalent six-call step
sequences. Retain the passive latches and exact-rest handover. Move orders/cache into the planned
spawn-constructed server-only component and measure cost, including assignment trials. The engine
slope refusal must use the actual substep resolution there. Contact rows, warm starts, per-point
belt direction and reaction ownership are still slice 4; group parallelism remains slice 5.
