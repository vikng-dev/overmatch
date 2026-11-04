# Drive-loop investigation and parallel evaluation (#186)

Subject: the video game's authoritative tank-driving CPU work; game state only.

The lane completes belt-force parallelization. Servo inspection is complete; its implementation remains open below.

## Waste inspection

| Site | Finding and disposition |
|---|---|
| `src/track/forces.rs::update_grip_elements` | DERIVED: six material-element bookkeeping arrays were allocated per side per update. `ElementWork` combines them into one, retaining the separate per-contact output vector: five fewer allocations per side, ten per tank update. Load accumulation, membership, strain advancement and force emission keep their sequence. |
| `src/track/forces.rs::collect_column_contacts` | The shoe's lateral bounds depend only on side construction. Compute them once before the segment walk. |
| `src/track/sim.rs::apply_track_forces` | Point-velocity reads repeatedly express the same world COM transform and hull velocities. Read those once per tank and use Avian's own scalar point-velocity expression. Actual compiler hoisting and CPU savings are unmeasured. |
| `src/tank/servo.rs::drive_servos` | Every integrating servo rebuilds its root's `part_facts` maps and scans the same volume membership; powered nodes also repeat the same crank-power evaluation. These inputs are constant across one tank's servo walk. This waste remains unresolved with the servo ratchets below. |

Negative results: the belt adapter runs in `FixedUpdate`, outside physics substeps. Contact staging walks current reports once; `ContactTransfers::keep` drains pending contacts for the physics step. There is no retained-point rescan in this adapter. The nearby-collider `filter().collect()` can reuse its input allocation; its `retain` rewrite is not counted as an allocation saving.

## Ownership and effects

The production `driving_systems` installer owns the same-update sequence:

1. `gather_drive_supports`: parallel nearby-collider and source-inertia reads on Bevy's compute pool. Thread-local queues drain into sources sorted by entity; a sorted, deduplicated collider union constructs the common tick-start snapshot.
2. `apply_track_forces`: `par_iter_mut` over disjoint tank state and Avian force items. Each source borrows the common geometry/motion and owns its candidate membership. Within a tank, side, contact and force-application order are unchanged.
3. `commit_track_contacts`: static contacts stage in ascending source-entity, side and emission order. Moving reactions retain `piece_impulses`' canonical geometry order and sequential recipient-velocity updates; this coupled solve needs that order. Snapshot shape copies are released after commit.

The contact-transfer writer and broad mutable support-body query exist only at the commit boundary. Snapshot gathering uses read-only body data and a narrow tank pose/body query. An incomplete dynamic driving bundle panics before owner evaluation instead of disappearing from the worker query.

RPM publication runs in parallel after crank calculation. Diagnostic capture selects the greatest dynamic source entity, independently of worker completion. Player root replacement publishes before `DrivingForces`; its regression test mounts the production scheduling helper and verifies that the first driving reader sees the complete replacement in the same update. Other inspected population changes run in `Update`, before `GameplaySet`, or in physics/structural work in `FixedPostUpdate`.

Engine APIs were checked against registry Bevy 0.19.1 (`QueryData`, `QueryParIter`, `Parallel`, schedule configurations) and vendored Avian 0.7.0 (`ForcesItem` reads, point velocity and force application). No private pool or owner-sized task dispatch was introduced.

This is the explicit ownership pattern for this lane: immutable tick input publication, parallel owner evaluation, ordered shared effects. The proposed servo follow-up can use the same separation after its ratchets are resolved.

## Evidence

MEASURED: the disposable comparison ran the existing `DrivenHull` fixture for 256 ticks at 64 Hz: throttle 1 through tick 192, then -0.5 through tick 256, with zero imposed grade. Hull pose stays fixed; longitudinal speed integrates the adapter's returned tangential force. Every tick contributes physical increments, carried drive/transmission/grip state and force telemetry to the cumulative digest.

MEASURED: before the lane, after scratch consolidation, and after force/snapshot parallelization, the recorded force, torque and speed samples and cumulative digest match exactly. These are observations, not new driving obligations. The disposable test was removed.

| Tick | Hull force Z (N) | Longitudinal speed (m/s) | Cumulative digest |
|---|---:|---:|---|
| 64 | -194615.31 | 1.6088057 | `788078a36b9ec4ea` |
| 128 | -86867.72 | 2.9722695 | `aa6438714daf104c` |
| 192 | -73634.69 | 3.9779878 | `a5348f0406daabf8` |
| 256 | 152339.61 | 1.1801531 | `9433548aa9d9a6fb` |

Permanent tests compare the same tank at populations of one and eight, with reversed query order; check source-selected capture and shared static staging order; and retain the existing reversed-order stacked-body and shared-piece reaction tests. Borrowed support selections cannot change another source's membership. The production driving installer owns the tested phase wiring.

Gates: `cargo fmt --all --check`; CI clippy with `--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`; targeted track, contact-transfer and server tests plus `doc_citations`, `ui_ascii`, `net_boundary`, `bake::` (MEASURED: 177 passed); `headless_test` (MEASURED: 28 passed, four on-demand measurement beds ignored). Every compile/run used `scripts/serial-cargo`, unattended and headless. No captures or frame/tick pricing were taken.

DERIVED expectation: fewer bookkeeping allocations and independent tank contact calculations sharing the existing compute pool should reduce allocator work and expose more of the drive span to the hosted cores. Snapshot construction, coupled reactions and dispatch remain costs; the speedup is unmeasured. Tonight's measurement battery owns pricing.

## Remaining servo work

`servo.rs` is unchanged. Its file ratchet requires closing `load-27`, `load-31` and `net-22` together with an edit; each finding still matches the current code: role-indexed whole-assembly inertia, instantaneous zero-work arrest, and remote angles applied from plain replication. The clock row explicitly routes its decision through #187, whose requested capture is excluded by this lane's no-capture rule. The lane does not bypass those rows or change the clock and physical laws inside a CPU refactor.

The implementation follow-up is concrete: a root-owned pass computes facts and available crank power once and advances the root's slot-ordered states in parallel; a node-owned pose pass writes transforms from those completed states. Resolve authored moving inertia, passive braking and the presentation clock first. This lane leaves servo fact reuse and servo parallelization open.

The green waste step is committed as “driving: combine tick-local grip scratch storage (#186)”. The parallel step is “driving: evaluate tank owners on the compute pool (#186)”.
