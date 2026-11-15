# Stack seams on stock avian3d 0.8.0: findings (2026-11-15)

**Both prototypes pass with zero fork lines.** Nothing private in Avian had to be named.

**`FINDINGS.md` is not committed.** The harness refused the write of a findings `.md` from a subagent, and I did not work around it. This message is the text to commit as `proto/stack-seams/FINDINGS.md`.

- **Where:** branch `lane/stack-proto` in bay-3, four commits, not pushed, bay clean. Crate at `.claude/worktrees/bay-3/proto/stack-seams/`, with its own `[workspace]`, on crates.io `avian3d 0.8.0` and `bevy 0.20.1`.
- **Evidence:** every MEASURED number is printed by the tests. The last full run (15 tests, all green) is committed as `proto/stack-seams/EVIDENCE.log`.
- **References:** file and line are into the registry source of avian3d 0.8.0, relative to its `src/`.
- **How it ran:** `CARGO_TARGET_DIR=<bay>/proto/stack-seams/target scripts/serial-cargo test --profile test --manifest-path proto/stack-seams/Cargo.toml -- --nocapture --test-threads=1`. The queue script takes a manifest path as is. `--profile test` is required because it rewrites a bare `test` to the game's `edit` profile.
- **Build settings:** Avian's `parallel` feature off, no debug info (disk), debug assertions on.
- **Disk:** 34 GiB free before the first build, 33 GiB after, 30 GiB at the end. `proto/stack-seams/target` (678 MB) is removed; the bay's `target/` is untouched.
- No timing was measured.

## 1. Contact reduction as a game-side pass: PASS, 0 fork lines

Files: `src/reduction_pass.rs` (the pass), `src/reduction.rs` (the fork's file, copied; only its imports changed), `tests/contact_reduction.rs`.

### What runs where

- The pass is one system in `NarrowPhaseSystems::Last` (`collision/narrow_phase/mod.rs:236-243`, chained after `Update` at `:103-112`).
- It walks `ContactGraph::active_pairs_mut()` (`collision/contact_types/contact_graph.rs:327`) and takes `ContactPair::manifolds` (`collision/contact_types/mod.rs:151`) of every touching, constraint-generating, hook-free pair between two marked non-static bodies.
- It reduces per body pair, puts the kept manifolds back, and corrects the public `ContactStatusChangeQueue` (`narrow_phase/system_param.rs:119-151`, exported at `narrow_phase/mod.rs:24`).
- The solver drains that queue in `apply_contact_status_changes`, before `SolverSystems::PrepareSolverBodies` (`dynamics/solver/plugin.rs:131-137`, `:189-221`, `:284-362`). Constraints are therefore built from the reduced set in the same step.

### Evidence (all MEASURED)

| Criterion | Test | Result |
|---|---|---|
| Constraint graph agrees after the flush, 600 steps | `the_constraint_graph_matches_every_touching_pair_for_600_steps` | 0 mismatches under four rules. Pairs compared: hull 296,999; keep at most 0: 418,591; 1: 374,895; 2: 348,985 |
| Fork guard 1 | `a_body_on_reduced_contacts_stays_supported_with_a_bounded_count` | 64 touching pairs on 4 manifolds and 4 points; centre of mass over the kept outline on 19 of 19 awake steps |
| Fork guard 2 | `a_pair_reduced_to_nothing_leaves_and_rejoins_its_island` | 60 touching pairs with no manifold, 60 of 60 still linked in an island; two lift-and-return cycles, no panic |
| Fork guard 3 | `an_island_split_keeps_pairs_reduced_to_nothing` | 10 such pairs between neighbouring slabs; both removals pass |
| Kept points keep their warm start | `kept_points_carry_last_steps_impulses` | 40 of 40 kept points over 10 resting steps entered the solver with exactly last step's stored impulse, all above zero |
| No line of Avian changed | the crate depends on crates.io 0.8.0 | 0 |

- **600-step coverage:** each run had pairs starting (5,790 to 17,951 queued) and stopping (5,122 to 18,487), and three of the four had count changes left in the queue. Bodies were asleep on 18 steps of the hull run and 39 of the keep-0 run.
- **Guard 1's unreduced twin:** the same 64 touching pairs, on 64 manifolds and 256 points.
- **Touching, started and ended status stay upstream's.** The pass never writes a flag, and a pair reduced to nothing stays `TOUCHING`.

### What had to be worked around

1. **Fold the correction into upstream's queued entry; do not push a second one.**
   - Upstream measures `manifold_count_change` against last step's manifolds, which are the reduced ones (`narrow_phase/system_param.rs:801`, `:907-908`). A pair reduced from 3 to 1 gets `+2` queued every step, and the pass's `-2` must cancel it.
   - The pass adds its delta to that entry and removes the entry at zero. In the 600-step hull run 31,093 corrections were folded and all cancelled, and 1,985 were pushed fresh.
   - 253 pairs had a queued `StartedGeneratingConstraints` and needed nothing, since the flush reads the reduced `manifolds.len()` (`dynamics/solver/plugin.rs:299`).
   - Pushed as a separate entry, the flush trips `debug_assert!(!contact_pair.manifolds.is_empty())` at `dynamics/solver/constraint_graph.rs:218` (`a_separately_queued_correction_trips_the_constraint_graph`, MEASURED panic). The plan's A2 wording "pushes `ManifoldCountChanged` with its own delta" should say "folds".
2. **`manifold_count_change` and `ContactStatusBits` are not needed.** They are `pub(crate)` (`contact_types/mod.rs:157`) and `pub(super)` (`system_param.rs:90`). The scout's candidate (a) budgeted about 10 lines to open the first; the queue alone is enough.
3. **The fork's island patch for this is not needed.** 0.8.0 links a contact to its island at `Started` and unlinks at `Stopped`, whatever its manifold count (`plugin.rs:303-313`, `:323-338`). Guards 2 and 3 pass on that.
4. **`ContactPairFlags::REDUCIBLE_CONTACTS` becomes a query in the pass** (`RigidBody` plus the marker). The marker component moves to the game.
5. **0.8.0 no longer exports `Scalar` or `Vector`** (`math/mod.rs:69` is `pub(crate)`), so the ported file names `f32`, `Vec3` and `Vec2`. No other line of the 395 changed.

### Contact recycling (record only, input to D2)

Test `record_what_recycling_does_to_the_pass`: the same fixture, 20 steps at rest, then a 0.4 rad/s tip.

| Recycling | Pass clears the pair's cache | At rest | 25 steps after the tip |
|---|---|---|---|
| off (the plan's config) | no | 4 points | 4 points |
| 0.05 | no | 4 points; the pass took 136 pairs against 1,216 | **64 manifolds, 256 points: reduction stopped** |
| 0.05 | yes | 4 points | 4 points |

- The graph agreed on every step in all three (0 mismatches).
- A recycled pair returns before hooks and matching and refreshes only `penetration` (`system_param.rs:594-698`), so the pass sees last step's reduced set with a stale `normal_speed`. My reading of the 256 points is that a landing verdict then stays for as long as the pairs recycle. That cause is DERIVED from the source, not isolated by a run.
- `ContactPair::recycling_cache` is a public field (`contact_types/mod.rs:168`). Setting it to `None` on the pairs the pass takes makes upstream regenerate them and restores the baseline result, with no fork line.
- The centre of mass was off the kept outline on 9 or 10 of the tip's steps in all three rows, the baseline included. That is the fixture tipping, not recycling.

### Not covered

- Of the fork's 21 guards, 3 world-level ones were ported and run. The 14 pure-function guards of `reduce_body_pair` were not run here; the function compiles unchanged.
- Not ported: `a_body_held_by_its_fifth_corner_stays_up`, `a_reduced_body_rests_on_its_loaded_supports_and_sleeps`, `only_marked_dynamic_body_pairs_reduce`.
- The landing speed is a fixed 0.15 m/s, not read per body from `SleepThreshold` as the fork does.
- Groups are reduced serially; the fork reduces them in parallel.

## 2. Belt friction as game-owned rows: PASS, 0 fork lines

Files: `src/belt.rs`, `tests/belt_rows.rs`. Subject: the game's track belt as a scalar degree of freedom.

### What runs where

- A contact hook (`collision/hooks.rs:187`) sets `ContactManifold::friction` to 0 (`contact_types/mod.rs:368`), so Avian builds no tangent part for the manifold (`dynamics/solver/contact/mod.rs:176`).
- `solve_belt_rows` runs in `SubstepSchedule`, after `SubstepSolverSystems::Relax` and before `Damping`. The set order is at `dynamics/solver/schedule.rs:60-70`; Avian's own systems are at `plugin.rs:165` and `:168-179`.
- Default `SubstepCount` is 6 (MEASURED).
- A drive is a plain struct in a game resource keyed by the body that carries the belt: speed, inverse inertia, anchor speed, hold capacity, friction coefficient, direction.

### Access the row needs, and whether 0.8.0 exposes it

All of it is public.

| Need | 0.8.0 item |
|---|---|
| The manifold's constraint | `ConstraintGraph::colors` (`constraint_graph.rs:147`), `GraphColor::contact_constraints` (`:94`), matched on `ContactConstraint::contact_id` (`contact/mod.rs:99`) |
| Normal impulse of each point this substep | `ContactConstraintPoint::normal_part` (`contact/mod.rs:28`), `ContactNormalPart::impulse` (`contact/normal_part.rs:16`) |
| Anchors and normal | `anchor1`, `anchor2` (`contact/mod.rs:36`, `:39`), `ContactConstraint::normal` (`:89`) |
| Body velocities, read and write | `body_index1`, `body_index2` (`:61`, `:64`); `SolverBodies::get`, `get_mut` (`solver_body/mod.rs:652`, `:658`); `SolverBody::linear_velocity`, `angular_velocity`, `velocity_at_point` (`:68`, `:78`, `:112`) |
| Inverse mass and inertia | `SolverBodies::get_inertia` (`:664`); `SolverBodyInertia::effective_inv_mass`, `effective_inv_angular_inertia` (`:450`, `:478`) |
| Which body is which entity | `ContactGraph::get_by_id` (`contact_graph.rs:199`), `ContactPair::body1`, `body2` |

A static body has no solver body (`get` returns `None`); the row treats it as immovable.

### Evidence (all MEASURED)

Fixture: a 10 kg box, rotation locked, on a static slab that carries the drive.

**The witness of #353: slip never reverses, for any ratio of effective mass to belt inertia.** A belt at 0.1 m/s under a resting box, no hold, 200 steps.

| Ratio | In the step | One tick late (the control) |
|---|---|---|
| 0.2 | 0 alternations by step and by substep; 0 of 1,200 rows reversed slip | first-step gain -0.200; slip +0.100, -0.020, +0.004; decays to 0 |
| 1 | same | gain -1.000; alternates on 200 of 200 steps at 0.100 m/s |
| 5 | same | gain -5.000; 200 of 200; grows to 0.500 m/s and stays |
| 12 | same | gain -12.000; 200 of 200; grows to 1.200 m/s and stays |

- Tests: `in_step_slip_never_alternates_for_any_inertia_ratio` and `one_tick_late_alternates_and_grows_above_one`.
- In the step, the pair ends at the speed its momentum fixes, to the sixth decimal (ratio 12: 0.007692 m/s for both belt and box).
- The same holds with friction saturated (coefficient 0.05), where slip falls monotonically: +0.100, +0.091, +0.082 at ratio 0.2.
- The control is today's scheme on stock Avian: the hook writes the belt speed into `tangent_velocity`, Avian's row runs against it, and the belt is billed after the step. Its measured gain equals the belt plan's derived `-ratio` to three decimals.

**The plan's criteria.**

| # | Criterion | Test | Result |
|---|---|---|---|
| 1 | Friction 49.05 N under a 100 N hold: drive exactly at rest | `a_hold_above_friction_keeps_the_drive_exactly_at_rest` | 0 of 72 rows left the drive off 0.0; box decelerates at 4.9050 m/s² against μg = 4.9050 |
| 2 | Friction 49.05 N over a 20 N hold, belt 1 kg, box at 1 m/s | `a_hold_below_friction_lets_the_belt_creep_at_the_hold` | drive speed in [0, u] on 180 of 180 rows; slip never changed sign; box decelerates at 1.8182 m/s² |
| 3 | Bodies' impulse equals the drive's, per substep | same | largest imbalance 2.7e-7 N s against a largest row impulse of 0.128 N s |
| 4 | Red case: mass ten times the inertia, friction four times the hold, 0.1 m/s | `the_red_case_settles_in_step_and_alternates_one_tick_late` | in the step: 0 alternations, then belt exactly 0.0 with the box stopped |
| 5 | No line of Avian changed | crates.io 0.8.0 | 0 |

- **Red case detail:** in the step the belt runs +0.073, +0.056, +0.039, +0.021, +0.004, then 0.0. One tick late, slip alternates on 200 of 200 steps and the belt swings between 0 and 0.575 m/s for good.
- **Criterion 2's number is B/(M+I), not B/M.** Once belt and box move together the hold slows both, so the deceleration is 20/(10+1) = 1.8182 m/s² (DERIVED), which the run matches to four decimals. B/M would be 2.0000. I asserted against B/(M+I) within 2 %; the plan and T3 should say so.

### What had to be worked around

1. **Friction and hold must be solved together, not one after the other.**
   - The plan's build ("solves one tangent row, then solves a capacity-limited hold") under-delivers stick friction on a held belt by a factor of 1 + ratio: 13 times too soft at ratio 12 (`rows_solved_one_after_the_other_under_deliver_stick_friction`, MEASURED arithmetic).
   - The spike solves the two as one 2x2 boxed problem, exactly, by trying its nine active sets (`belt::solve_row`).
   - When the hold holds, the drive is assigned its anchor speed, so "exactly at rest" is exact and not within rounding.
2. **`PhysicsSchedule` refuses ambiguous access.** The broad phase's hook parameter reads the drives, so a game system that writes them must sit in a solver set. Left unordered it panicked at schedule build (MEASURED on the first run); `clear_step_stores` now sits in `SolverSystems::PreSubstep`.
3. **A constraint is found by scanning, not through its handle.** `ConstraintGraph::contact_handles` gives indices into `manifold_handles`, and `prepare_contact_constraints` skips entries (`plugin.rs:700-751`), so those indices do not address `contact_constraints` (DERIVED from the source). The spike scans every constraint each substep; the real thing should build its own list once per step, after `SolverSystems::PrepareContactConstraints`.

### Where the game reads the row's impulse afterwards

- From its own store: `Drive::step_friction_impulse` and `step_hold_impulse`, summed over the substeps and cleared in `SolverSystems::PreSubstep`.
- Avian's `store_contact_impulses` (`plugin.rs:1058-1091`) writes only its own parts, so `warm_start_tangent_impulse` of a belt manifold stays zero.
- `src/contact_transfer.rs` would read the game's store.

### Recycling

Test `record_the_rows_under_contact_recycling`: with recycling at 0.05, 26 pair-steps were recycled in 30 steps. Rows (180) and final speeds (0.056816 m/s) were identical to the run without. The rows read the drive from the game's store, not from the manifold, so a skipped hook does not matter to them.

### Not covered

- **One manifold per drive**, with one row per manifold at the centroid of its points weighted by normal impulse. Several manifolds on one drive are solved in turn against a shared hold budget; that path compiled and was never exercised.
- **Rotation is locked in the fixture**, so friction never shifted the normal load. The rows run after all of Avian's rows in the substep, and the normals see their effect one substep later.
- **No warm start:** the row starts from zero each substep, which is exact for a single row per drive.
- **Missing from the spike:** a crank anchor (a moving anchor with its own inertia), a direction per contact from the belt's outline, and the `parallel` feature. The system is serial and takes `ResMut<SolverBodies>`.

## 3. What this changes in section B2

- **Fork target: unchanged**, at about 476 lines inside upstream files plus the one 120-line file. That sum already assumed 0 for patch 14 and 0 for the belt. Both are now shown, so neither the 35-line call slot nor the 150 to 200-line vendored surface drive is added.
- **Patch 14: game-side pass, 0 fork lines.** `reduction.rs` moves to `src/avian_ext/contact_reduction.rs` with its imports changed, and a driver of about 150 lines (DERIVED from the spike's pass) replaces the fork's `reduce_body_pair_contacts`. Three notes for the row:
  - the pass folds its delta into upstream's queued entry;
  - the fork's `REDUCIBLE_CONTACTS` flag, its status-site tolerances and its island walk (fork `islands/mod.rs:1130`) all go, with nothing in their place;
  - the scout's "≈10 lines, `manifold_count_change` made `pub`" for candidate (a) is 0.
- **Belt friction rows: game-side, 0 fork lines, confirmed for #353.** The form is a hook that zeroes friction, one substep system between `Relax` and `Damping`, a 2x2 block solve of friction and hold per manifold, and a game store for the impulses. A2 and T3 should replace "then solves a hold" with the block solve, and "B/M" with "B/(M+I)".
- **D2 (recycling):** the pass can exempt its pairs by clearing `recycling_cache`. Without that, reduction stopped after a disturbance in the one case measured.
- **A2 bookkeeping:** the crate is at `proto/stack-seams/` with its target at `proto/stack-seams/target`, per this lane's brief, not `prototypes/avian08-seams/` under the bay's `target/`.

Had either failed, the cheapest fork seams would have been:
- **Reduction:** the scout's candidate (b), a call slot in `NarrowPhase::update` after `update_contacts`, about 35 lines.
- **Belt:** one `Option<fn>` slot called from the friction block of `ContactConstraint::solve` (`contact/mod.rs:311-345`).

Neither is needed.

## Decisions I made

- **Ported the fork's real hull selection**, not only the plan's "keep at most K" stand-in; both run in the 600-step test.
- **Asserted criterion 2 against B/(M+I).**
- **Ran the gating tests with `recycle_distance = 0.0`**, the plan's config, and recorded 0.05 separately.
- **Left `parallel` off.**
