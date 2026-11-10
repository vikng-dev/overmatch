# Rest energy lane report

Subject: rigid-body rest of destructible scenery in the video game. Game state only.

Decision: keep one kinetic-energy-per-mass rest law for all dynamic bodies, including tanks. The physical inertia distinguishes a slender body's long-axis roll from transverse rotation. The rest speed and rest interval retain their existing values. No runtime alternative remains.

## MEASURED count runs

Both runs used the twelve-house rehearsal, 30 tanks, one client and a 100-second capture, hidden, unattended and muted. These are local count observations; hosted pricing is still required. No captured tick exceeded one second.

| Count | Before | After |
|---|---:|---:|
| Captured ticks | 6,182 | 6,142 |
| Awake piece-ticks, all scenery | 226,031 | 96,758 |
| Awake piece-ticks, scheduled twelve | 225,714 | 96,460 |
| Island-ticks with an awake piece | 8,811 | 3,915 |
| Linear-over-threshold piece-ticks | 23,345 | 27,322 |
| Angular-only piece-ticks | 7,811 | 2,789 |
| Angular-only, below rest-energy piece-ticks | 5,239 | 1,735 |
| Scenery-island ticks held only by that last kind | 1,392 | 195 |

DERIVED from these observed counts: awake piece-ticks decreased 57.2%; scenery-island ticks decreased 55.6%. The baseline lifetime is censored for one pile, so these percentages describe the observed capture. They do not estimate hosted milliseconds.

The island counter requires an awake scenery piece, at least one reference-threshold crossing, and no member exceeding the energy limit. Non-piece members can disqualify the island. The initial census also admitted tank-only islands; the final comparison above repeats both runs with that corrected definition. Candidate island ticks are an instantaneous signature; sleeping still requires the uninterrupted `TimeToSleep` interval.

Both captures also observed unscheduled debris from `house_0043`: MEASURED 317 awake piece-ticks before, 298 after. The scheduled-twelve subtotal excludes it.

## MEASURED first all-sleeping events

Elapsed ticks from each construction's first piece birth, reconstructed from births, retirements, sleep episodes and awake-episode starts. At each event tick, all births and retirements are applied before testing whether at least one live piece remains and none is awake. This observes the first all-sleeping event; no fragment count, final pose or settle time is an oracle.

| Scheduled pile | Before, ticks | After, ticks |
|---|---:|---:|
| house_0001 | 139 | 83 |
| house_0002 | 411 | 255 |
| house_0003 | 94 | 231 |
| house_0004 | 103 | 99 |
| house_0005 | 183 | 110 |
| house_0006 | 92 | 65 |
| house_0007 | 92 | 86 |
| house_0008 | 135 | 171 |
| house_0009 | 95 | 485 |
| house_0010 | 366 | 369 |
| house_0011 | >4,268 (censored) | 211 |
| house_0012 | 386 | 576 |

All twelve scheduled piles were entirely sleeping at the end of the energy-law run. Some piles took longer under the new law; the table preserves those observations. A player could see different settling and fragment poses. Small long-axis rolls can stop sooner, while substantial transverse rotation can remain awake longer. No visual equivalence is claimed.

## Rest contract and engine facts

The vendor now evaluates `v² + omega.dot(I_world * omega) / m < v_rest²`. The factor one half in kinetic energy cancels. `v_rest` is `SleepThreshold::linear`, scaled by `PhysicsLengthUnit`; negative values still disable sleeping, and equality still prevents it. `TimeToSleep` and whole-island sleeping remain unchanged.

`SleepThreshold::angular` is removed from the component, default, reflection and serialization. Code setting it no longer compiles; users select the linear rest speed. A fixed 0.15 rad/s reference survives only in the contact census, whose current reader is the collapse instrument. It has no influence on sleeping.

The raw computed inertia is rotated to world space. The engine's zero/infinite mass has reciprocal zero, so the rotational contribution is zero; infinite/singular computed inertia returns a zero tensor, following its existing reciprocal convention. No division by zero is introduced. Locked axes mask force increments; `integrate_positions` still integrates prescribed velocities, so actual prescribed motion contributes using physical inertia rather than a singular lock-masked effective tensor. These facts were verified in the vendored computed mass properties, solver-body setup and integrator sources.

Body-owned timer updates run on the compute pool. Whole-island wake bits and split selection commit afterward; equal split timers choose the lower island id.

MEASURED in the shipped headless composition, after neutral settling: the parked tank read energy-equivalent speed 0.02338785 m/s against rest speed 0.15 m/s, and was awake at that observation. The temporary observation was removed; its emergent reading is not a test oracle.

## Regression evidence and gates

- The spin-axis/equal-specific-energy test failed against the separate angular threshold and passes with the energy law. It covers negligible long-axis roll, transverse rotation, and small/long bodies at equal energy per mass on both sides of rest.
- Frame-invariance and degenerate-mass guards pass, including prescribed motion with locked axes.
- The shipped-house hull-corner witness runs the admitted `house_0003` placement, a one-ULP X shift and a 0.01-rad yaw perturbation. MEASURED old last-motion-to-sleep gaps: 16, 12, 42 ticks; new gaps: 16, 32, 27 ticks. The last old case fails the DERIVED 39-tick deadline.
- Deadline: `ceil((TimeToSleep + visible_distance / (SleepThreshold::linear * PhysicsLengthUnit)) / fixed_dt) + 2`. The fixture observation resolution is 0.01 m; the final two ticks expose the completed solve and commit sleep. The test reads actual hull corners against held poses, accumulating creep, and asserts every live piece is sleeping.
- `cargo fmt --all --check`: passed before each code commit.
- CI clippy, locked workspace/all targets with `dev_ui,bitprobe` and `-D warnings`: passed.
- Whole headless gate plus targeted rest/census tests and required guards: 98 passed, six observation/pricing beds ignored. Final corrected-census/rest/whole-tree guard run: 57 passed.
- Required guards: `doc_citations`, `ui_ascii`, `net_boundary`, `net::protocol`, all passed.
- The final diff was checked against the six review lenses; no backlog row names an edited file. No `src/track/*` or `src/tank/spawn.rs` change.

## Commits, artifacts and follow-up

- `2243e9a9`: census counters and the first diagnosis rehearsal.
- `cd73b7cd`: rest law, angular-setting removal and energy guards.
- `bd8aaa76`: hull-motion witness and scenery-island qualification.
- This report records the final count comparison.

Raw final artifacts: `target/rest-energy-before-scene/` and `target/rest-energy-after-scene/`, especially `tick-attribution.json`, `rows.json` and `server.log`. The initial rehearsal and canceled all-island after run are excluded from the final comparison. The baseline was rebuilt temporarily from the pre-law vendor and census source with the same scenery qualification, then the energy source was restored. The branch contains only the energy path.

Out of scope: piece-on-ground contact reduction, the stationary-pair manifold flip and converting rubble to terrain. The ground manifold work remains an opening for the next count experiment; this lane changes no contact selection or body type.

Hosted timing remains for the orchestrator. Set the two variables to the existing deployment, then run this exact recipe without `--rehearsal`:

```sh
scripts/perf/collapse.sh --target "$OVERMATCH_TIMING_TARGET" --deployment "$OVERMATCH_TIMING_DEPLOYMENT" --schedule scripts/perf/match-collapse.json --tanks 30 --clients 1 --duration 100
```
