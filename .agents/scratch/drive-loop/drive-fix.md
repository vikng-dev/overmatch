# Drive-loop review fixes (#186)

Subject: the video game's authoritative tank-driving simulation and damage-model capability
tests; game state only. No physical law changed.

All dispatched findings were verified and accepted; none was rejected. The lens backlog has no
rows naming the edited files.

| Finding | Disposition |
|---|---|
| Error handling: missing `Position` | Fixed. Gathering enumerates tanks independently of position, then requires a dynamic tank's spawn-built pose with an invariant-naming `expect`. The regression removes `Position`. |
| Parallel work: `RubbleBodies::gather` | Fixed. Collider geometry and unique dynamic-body inertia prepare in batches on the existing compute pool. Collider indices and shared body slots follow gather order. Snapshot publication waits for all preparation. |
| Testing: recovery crank bound | Fixed. The test derives an angular-impulse envelope from the fixture's torque curve, construction drag, clutch capacity, inertia and timestep. Recovery propulsion evidence remains. |
| Testing: advected turning ratio | Fixed. The ratio is an observation; direct evidence requires loaded contact and a moment opposing yaw. |
| Brevity: named test comments | Fixed. Removed review attribution from the rig helper, dropout and enter-bound tests, and co-sourced slab fixture. Counterexamples, fixture parameters and derived stability rates remain. DERIVED: six net comment lines removed. |

## Evidence

- MEASURED: with the regression removing `Position`, the old gather failed because the expected
  panic did not occur. The fixed adapter passed its targeted tests.
- MEASURED: identical snapshot geometry, friction, motion, body state and indices with one,
  two and four preparation workers. The fixture includes a compound dynamic owner, axis locks,
  kinematic motion and fixed construction. This tests worker independence, not stored outputs.
- MEASURED: all ten printed rubble observations matched the before/after run exactly. For example,
  hull rise remained 0.099954 m and the slab reaction remained -1875 N s with or without the
  co-sourced brick. These are observations, not new test obligations.
- DERIVED: gross fuel/idle torque is bounded by curve peak plus construction drag; losses add at
  most drag and the clutch adds at most its capacity. For this straight-driving fixture,
  `|delta rpm| <= (curve_peak + 2 * drag + clutch_capacity) * dt / inertia / RPM_TO_RAD`.
  The bound uses pre-tick drag from the authored motoring law, without calling the torque solve
  for the expected answer. DERIVED bound: 197.03012 rpm per tick. MEASURED recovery: 1819.0322 rpm
  to 1882.299 rpm on the first tick, then 2185.7073 rpm and 2.8631518 m/s after the recovery interval.
- MEASURED: turning ratios were 0.958779 and 0.970015 at the fixture's respective yaw rates of
  0.2 and 0.6 rad/s. Their magnitude is reported for owner judgment, not pinned.

DERIVED expectation: independent triangulation, world-space transforms and inertia preparation
expose more of snapshot construction to the hosted cores. Task dispatch, intermediate vectors
and canonical index assembly remain costs; speedup is unmeasured. No pricing or captures ran.

## Gates and handoff

Every step passed per-file formatting, `cargo fmt --all --check`, CI clippy with
`--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`, targeted tests and
`doc_citations`. All builds and tests used `scripts/serial-cargo`, unattended and headless.

- MEASURED pose step: 32 adapter tests and three citation guards passed.
- MEASURED snapshot step: 91 tests passed, including all 28 enabled headless cases; four
  on-demand beds stayed ignored.
- MEASURED assertion step: both affected tests and three citation guards passed.
- MEASURED final guards: 109 tests passed across `track::forces`, `track::rubble`, `doc_citations`,
  `ui_ascii`, `net_boundary` and `bake::`.

Bevy APIs were checked against registry sources for Bevy 0.19.1 (`Query`, optional query data,
entity removal, `ParallelSlice`, `TaskPool`, `ComputeTaskPool`, `TaskPoolBuilder` and `Time`).
Avian APIs were checked in vendored avian3d 0.7.0 (`Position`, force reads, collider ownership,
axis locks and solver inertia).

The asymmetric reaction clamp, grip departure dwell, connected-group reaction partitioning and
duplicated membership logic remain unchanged, as dispatched. Their design decisions belong to
the owner. The orchestrator owns pricing, merge and bay release; the lane never pushes.
