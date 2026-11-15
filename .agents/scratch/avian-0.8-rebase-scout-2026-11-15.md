# Avian 0.8 rebase scout (2026-11-15)

Subject: the game server's physics engine dependency (rigid bodies, contacts, rest). Game state only.
Read-only scout: nothing built, nothing run, no tracked file edited. `main` at `252939a0`.

Sources used, and how:

- **Pristine 0.7.0**: the cargo registry copy, `~/.cargo/registry/src/index.crates.io-*/avian3d-0.7.0`.
- **0.8.0**: the published crate, downloaded from `static.crates.io` and unpacked in the session
  scratchpad (not in the repo). Paths below written `0.8:<path>` are relative to its `src/`.
- **Fork**: `vendor/avian3d-0.7.0-child-pose` on `main`; lanes read with `git diff main...<lane>`.
- **Upstream**: the GitHub API, read-only (`avianphysics/avian`; `Jondolf/avian` redirects there).
- Numbers are labelled MEASURED (counted here by `diff`, `diffstat`, `grep -c`, `git merge-file`) or
  DERIVED (judgement or arithmetic from those).
- One source is second-hand: the Bevy 0.19 to 0.20 migration guide was read through a summarising
  fetch that covered 100,000 of its 110,008 characters. Its entry list is a lead, not a count.

## 1. Release state (the gate)

| Fact | Value | Source |
|---|---|---|
| avian3d 0.8.0 | published 2026-10-20, not yanked; tag `v0.8.0`, release the same day | crates.io API `crates/avian3d`; `gh api repos/avianphysics/avian/releases` |
| upstream `main` | 1 commit past `v0.8.0` (MEASURED): `2c2d6934`, 2026-11-04, a `Without<ColliderDisabled>` in `solve_continuous` (#1101) | `gh api .../compare/v0.8.0...main` |
| milestone 0.8 | still open: 16 closed, 4 open (#958, #872, #817, #437), none blocking | `gh api .../milestones`, `.../issues?milestone=11` |
| Bevy required by 0.8.0 | `bevy = "0.20"`, plus `bevy_math`, `bevy_shape`, `bevy_curve` 0.20 | `0.8:../Cargo.toml:326-346` |
| Bevy we pin | 0.19 (`Cargo.toml:28`), locked 0.19.1 (`Cargo.lock:675`) | repo |
| other 0.8 dependency moves | parry3d 0.27 to 0.31, obvhs 0.3 to 0.4, bevy_heavy 0.5 to 0.6, glam_matrix_extras 0.3 to 0.4, bevy_transform_interpolation 0.5 to 0.6 | both `Cargo.toml`s |

**The target is 0.8.0.** Upstream main adds one fix we can cherry-pick.

**0.8 does not match our Bevy.** It cannot be adopted on Bevy 0.19, and no avian 0.7.x supports
Bevy 0.20 (the Bevy move is upstream PR #1084, +1362 −832 over 34 files, MEASURED from the PR API).
The rebase is therefore a whole-stack move. Everything that must move has a published version:

| Crate (ours) | Now | Needed | Exists? | Source |
|---|---|---|---|---|
| bevy, bevy_winit | 0.19.1 | 0.20.x | 0.20.1, 2026-10-28 | crates.io |
| lightyear, lightyear_transport, lightyear_sync, lightyear_core, lightyear_inputs, lightyear_inputs_native | 0.30.1 | 0.31.0 (bevy ^0.20) | 2026-10-21 | crates.io `lightyear/0.31.0/dependencies` |
| lightyear_avian3d (vendored fork) | 0.30.1 | 0.31.0 (avian3d ^0.8) | yes | same |
| lightyear_netcode (vendored fork) | 0.30.1 | 0.31.0 | yes | same |
| bevy_replicon | 0.44.3 | ^0.45.2 (asked by lightyear_replication 0.31) | 0.45.5 | crates.io |
| bevy_egui | 0.42 | 0.43.x (bevy ^0.20, egui ^0.36, our locked egui) | 0.43.1 | crates.io |
| wgpu (named directly, `Cargo.toml:45-47`) | 29 | ^30 (bevy_render 0.20.1) | yes | crates.io |
| glam (pinned 0.32.1, `scalar-math`) | 0.32.1 | ^0.33.2 (bevy_math 0.20.1) | yes | crates.io |
| bevy_reflect, bevy_pbr (vendored `scalar-math` forks) | 0.19.1 | 0.20.1, re-vendored | to redo | `vendor/*/OVERMATCH_PATCH.md` |

Not checked: `winit` (bevy_winit 0.20.1 still asks ^0.30, so likely unmoved), `ron`, `gltf`, `image`.
Lightyear 0.31.0 has no GitHub release notes (the releases API tops out at 0.30.0), so its
migration cost is unread.

## 2. The fork's surface

MEASURED, `diff -ruN` of registry 0.7.0 against `vendor/avian3d-0.7.0-child-pose/src` on `main`:

- **31 files, 1905 insertions, 342 deletions**: 30 modified, 1 new (`collision/contact_types/reduction.rs`, 395 lines).
- `Cargo.toml` is identical; 78 `OVERMATCH PATCH` tags.
- The diff agrees with `OVERMATCH_PATCH.md` except for one omission, below.

**Finding: an undocumented patch on `main`.** `ContactCompliance` (a per-manifold spring-damper
normal law) is in the vendored tree and used by the game (`src/track/belt_segment.rs:212-217`), but
`OVERMATCH_PATCH.md` on `main` has no section for it (0 matches for "complian", MEASURED). The
section exists only on `lane/soft-run`.

### Trial merge

Each modified fork file was three-way merged against 0.8.0 with `git merge-file --diff3` (output in
the scratchpad only). MEASURED:

- 12 of 30 files merge with no textual conflict.
- 18 files conflict: 66 hunks, 1,638 lines inside conflict regions (base and both sides counted).
- A clean textual merge is not a compile. The fork's added lines use identifiers 0.8 renamed or
  moved: `Scalar` 38 times, avian's `Vector` 9, the `SolverBody` component 8, `constraint_handles` 2.

| File (under `src/`) | Fork +/− | Upstream 0.7 to 0.8 +/− | Conflict hunks / lines |
|---|---|---|---|
| `collision/narrow_phase/system_param.rs` | 336 / 57 | 325 / 229 | 17 / 482 |
| `collider_tree/update.rs` | 132 / 72 | 282 / 163 | 11 / 312 |
| `utils.rs` | 56 / 4 | 75 / 0 | 2 / 140 |
| `dynamics/solver/plugin.rs` | 58 / 20 | 407 / 69 | 6 / 117 |
| `collision/collider/collider_transform/plugin.rs` | 218 / 23 | 9 / 9 | 2 / 116 |
| `dynamics/solver/islands/sleeping.rs` | 59 / 32 | 78 / 68 | 2 / 87 |
| `dynamics/solver/solver_body/plugin.rs` | 22 / 8 | 140 / 63 | 5 / 76 |
| `collision/diagnostics.rs` | 55 / 1 | 7 / 1 | 3 / 71 |
| `dynamics/integrator/mod.rs` | 29 / 15 | 66 / 51 | 5 / 60 |
| `collision/contact_types/mod.rs` | 113 / 36 | 67 / 51 | 2 / 27 |
| `dynamics/solver/islands/mod.rs` | 5 / 2 | 237 / 177 | 1 / 27 |
| `collision/collider/parry/mod.rs` | 26 / 0 | 208 / 163 | 2 / 26 |
| `collider_tree/tree.rs` | 98 / 8 | 66 / 25 | 2 / 25 |
| `dynamics/solver/solver_body/mod.rs` | 28 / 2 | 303 / 27 | 2 / 21 |
| `dynamics/solver/contact/normal_part.rs` | 20 / 1 | 10 / 12 | 1 / 15 |
| `collider_tree/optimization.rs` | 8 / 9 | 117 / 60 | 1 / 13 |
| `dynamics/rigid_body/mass_properties/mod.rs` | 27 / 3 | 69 / 78 | 1 / 13 |
| `collision/collider/collider_hierarchy/plugin.rs` | 18 / 5 | 4 / 4 | 1 / 10 |
| 12 clean files | 157 / 44 in total | | 0 |

The 12 clean files: `collider_tree/diagnostics.rs`, `collision/broad_phase/bvh_broad_phase.rs`,
`collision/collider/collider_hierarchy/mod.rs`, `collision/collider/collider_transform/mod.rs`,
`collision/mod.rs`, `collision/narrow_phase/mod.rs`, `dynamics/rigid_body/forces/plugin.rs`,
`dynamics/rigid_body/mass_properties/system_param.rs`, `dynamics/rigid_body/sleeping.rs`,
`dynamics/solver/contact/mod.rs`, `dynamics/solver/diagnostics.rs`, `lib.rs`.

### Patch by patch

Fate is one of: **dropped** (upstream does it), **carried** (reapplies as is), **rework** (same
intent, new code), **conflict** (upstream redesigned the ground under it). Per-patch sizes are
DERIVED from the per-file counts above, since patches share files.

| # | Patch (`OVERMATCH_PATCH.md` section) | Files | Size | Fate on 0.8 | Evidence |
|---|---|---|---|---|---|
| 1 | Child-collider pose writes, `ChildColliderPoses`, `Reenabled` | `collider_transform/plugin.rs`, `mod.rs` | ≈220 | **rework**, small | Upstream still assigns every child pose unconditionally. Its changes there are f64 `.real()` casts, `par_for_each_mut` and `iter_many(..).matched()` (diff of that file, 9 / 9) |
| 2 | Attachment refusal | `collider_hierarchy/mod.rs` | 11 / 7 | **carried** | Upstream file unchanged (0 / 0) |
| 3 | Moved-collider bounds prune, `moved_aabb_candidates` | `collider_tree/update.rs`, `diagnostics.rs` | ≈70 | **rework** | Upstream still fetches every collider and checks `LastPhysicsTick` inside the loop, with a TODO saying so (`0.8:collider_tree/update.rs:913-934`). The system around it was rewritten (#1047, #1049) |
| 4 | Standalone key write-back | `collider_tree/update.rs` | ≈5 | **dropped** | Upstream now queries `&mut ColliderTreeProxyKey` and writes `*proxy_key = new_proxy_key` (`0.8:collider_tree/update.rs:143-199`; PR #1062) |
| 5 | Island contact waking and rest history | `narrow_phase/system_param.rs`, `islands/sleeping.rs` | ≈120 | **conflict** | #989 moved island linking and waking out of the narrow phase into a queue flushed by the solver (`0.8:dynamics/solver/plugin.rs:284-362`). That code still wakes any sleeping island a new contact links. Our rule (a wake only through an awake non-static participant, resolved after all changes) has to be rewritten in `apply_contact_status_change` and `WakeIslands` |
| 6 | Prepared collider mass (`with_mass_properties_cache`) | `collider/parry/mod.rs` | 26 / 0 | **rework**, small | No such cache upstream (0 matches). The file changed 208 / 163 for f64 and Parry 0.31; 2 hunks conflict |
| 7 | Minimum speculative margin | `narrow_phase/system_param.rs` | ≈15 | **dropped** | Upstream: `speculative_distance = speculative_distance.max(contact_tolerance)`, used for both the query distance and the pruning test (`0.8:.../system_param.rs:794-798`, `:861-863`). Our witness `stationary_contacts_keep_the_configured_minimum_speculative_margin` decides it |
| 8 | Unknown contact features | `contact_types/mod.rs` | ≈40 | **rework**, small | `match_contacts` is unchanged upstream: equal `UNKNOWN` ids still match as identities (`0.8:collision/contact_types/mod.rs:435-477`) |
| 9 | Nearest warm-start contact | `contact_types/mod.rs`, `narrow_phase/system_param.rs` | ≈80 | **rework** | Same function: first match wins, per old manifold (`0.8:.../system_param.rs:894-903`). Matching now runs only on pairs that were not recycled |
| 10 | Physics work counters | `solver/diagnostics.rs`, `solver/plugin.rs`, `collision/diagnostics.rs`, `utils.rs` | ≈150 | **rework** | Solver loops now go through `SolverBodiesAccess`; `par_for_each` changed; upstream added `recycled_count`. The "stationary non-touching" counters depend on patch 13's fate |
| 11 | Complete collider batches, explicit mass ownership | `collider_hierarchy/plugin.rs`, `mass_properties/mod.rs`, `system_param.rs`, `lib.rs` | ≈60 | **rework**, small | Upstream changed only observer syntax in the hierarchy plugin (`On<Add, X>` to `On<Add<X>>`). `mass_properties/mod.rs` changed 69 / 78 with 1 conflict hunk |
| 12 | Pairless-layer broad-phase prune, `broad_phase_candidates` | `broad_phase/bvh_broad_phase.rs`, `collision/diagnostics.rs` | 32 / 9 + counter | **carried** | Upstream changed 1 line; the merge is clean |
| 13 | Stationary non-touching pair cache | `contact_types/mod.rs`, `narrow_phase/system_param.rs` | ≈150 | **conflict**, probably dropped | Contact recycling (#1033) returns before the geometry query when relative motion is under a tolerance, for non-touching pairs too (`recycle_distance_non_touching`, `0.8:.../system_param.rs:590-697`). Ours is exact on its inputs, upstream's a tolerance. Keep one |
| 14 | Body-pair contact reduction | `contact_types/reduction.rs` (new), `narrow_phase/system_param.rs`, `islands/mod.rs` | ≈450 | **rework**, the largest | Upstream still has `// TODO: Manifold reduction` (`0.8:.../system_param.rs:890`). Three things move under it: handles left `ContactEdge` for `ConstraintGraph::contact_handles`; island links left for `PhysicsIslands::contact_nodes` (our `contact_edge.island.is_none()` walk at fork `islands/mod.rs:1130` has no field to read); recycled pairs keep last step's reduced point set without re-running reduction |
| 15 | Rest energy | `islands/sleeping.rs`, `solver_body/mod.rs`, `rigid_body/sleeping.rs` | ≈90 | **rework** | Upstream's test is still the two velocity thresholds, serial, with a TODO to parallelise (`0.8:dynamics/solver/islands/sleeping.rs:186-246`). It reads `SolverBodies` by index, not a component |
| 16 | Moved-list removal by index | `collider_tree/tree.rs`, `update.rs`, `optimization.rs` | ≈130 | **dropped** | Upstream has `MovedProxyList` (dense list and sparse index, O(1) removal) and an index map in `MovedProxies` (`0.8:collider_tree/tree.rs:36-98`, `update.rs:555-600`). Our `mark_moved` panic on a second entry becomes upstream's silent `false` from `push` |
| 17 | Sleeping bodies' velocity increments | `integrator/mod.rs`, `forces/plugin.rs` | ≈40 | **rework**, small | Unfixed upstream: `pre_process_velocity_increments` still runs over every body with `VelocityIntegrationData`, while `clear_velocity_increments` clears only `With<SolverBodyIndex>` (`0.8:dynamics/integrator/mod.rs:261-327`) |
| 18 | Serial small loops | `utils.rs` + 4 files | ≈110 | **dropped**, then re-measured | Upstream #1035: `par_for_each_mut(MIN_PAR_ITER_ENTITIES = 4096, ..)`. The note already in `OVERMATCH_PATCH.md:467-478` stands. `prepare_contact_constraints` now splits over colours at a floor of 2 (`0.8:dynamics/solver/plugin.rs:690`) |
| 19 | Compliant contact (undocumented on `main`) | `contact_types/mod.rs`, `solver/contact/mod.rs`, `normal_part.rs`, `narrow_phase/system_param.rs` | ≈60 | **rework**, small | See section 4 |

Tally (DERIVED from the table): 4 dropped, 2 carried, 10 rework, 3 conflict.

Lane patches not on `main` that the rebase must also carry:

- `lane/sleeping-pairs` (1 commit, 24 vendor lines): a panic when a pair's posed colliders are
  missing from the narrow-phase query. Rework, small.
- `lane/sleeping-pairs-hold` (2 commits): the sleeping-pairs hold. See section 3.
- `lane/soft-run` (8 commits; 69 / 17 in vendor): section 4.

### The other vendored forks

All four move in the same change, because Cargo resolves one Bevy.

| Fork | Patch size (MEASURED) | What the move involves |
|---|---|---|
| `lightyear_avian3d-0.30.1-child-pose` | 10 insertions, 38 deletions in `plugin.rs`, `rollback.rs` | 0.31 changed `plugin.rs` by 161 lines and grew `rollback.rs` to 1,291 lines, and still has its own `update_child_collider_position` (`plugin.rs:910`). 0.31's rollback names `ColliderTrees`, `MovedProxies`, `MovedProxyList`, `ConstraintGraph`, `PhysicsIslands`, so it is written against 0.8's types: any fork field added to those must stay compatible |
| `lightyear_netcode-0.30.1-admission` | 141 / 19 in 4 files, 11 tags | Upstream 0.30.1 to 0.31.0 touched only `server_plugin.rs` (53 lines). Likely a clean reapply |
| `bevy_reflect-0.19.1-scalar-math` | 1 tag | Re-vendor on 0.20.1. Whether glam 0.33 scalar `BVec3A`/`BVec4A` gained serde was not checked |
| `bevy_pbr-0.19.1-scalar-math` | 19 tags | Re-vendor on 0.20.1. Includes a backport of bevyengine/bevy#24936 that 0.20 may already contain (unchecked) |

## 3. Upstream changes that touch what the game uses

Sources: `migration-guides/0.7-to-0.8.md`, the v0.8.0 release notes, and source. MEASURED: upstream
0.7 to 0.8 is 104 files, 8434 insertions, 5283 deletions under `src/`.

### Behaviour changes (results move, not just names)

1. **Contact recycling (#1033), on by default** (`recycle_distance: 0.05`, `recycle_angle: 0.175`,
   `0.8:collision/narrow_phase/mod.rs:198-229`). A pair whose relative pose moved less than the
   tolerance keeps its manifolds and only updates penetration.

   **It returns before the per-step reset of `tangent_velocity` and before `modify_contacts`**
   (the `return` at `0.8:.../system_param.rs:697`; the reset is at `:830`, the hook at `:879-881`).
   Our track hook rewrites the manifolds and writes the belt speed there every step
   (`src/track/stop_contacts.rs:102-140`, `manifold.tangent_velocity += sign * direction * speed`).
   DERIVED from reading, not run: on stock 0.8 a recycled belt contact keeps last step's belt
   speed, friction and compliance. A parked tank whose belt starts turning is exactly the case
   where the relative pose does not move.

   Choices: disable recycling (`recycle_distance = 0`, a config value, forfeits the gain); a small
   fork patch that never recycles a pair carrying `MODIFY_CONTACTS`; or the belt as a solver degree
   of freedom (section 4), after which the hook no longer writes a speed.

2. **CCD rework (#996).** In 0.7 `default_speculative_margin` was `Scalar::MAX`
   (0.7 `collision/narrow_phase/mod.rs:252`). In 0.8 the field is gone: speculative contacts are
   opt-in per body (`SpeculativeCcd`), AABBs stay tight, and swept CCD is on by default for fast
   dynamic bodies against static and kinematic ones only. The game sets none of these (0 sites for
   `SpeculativeMargin`, `SweptCcd`, `default_speculative_margin`, MEASURED), so it takes the new
   defaults. Fast dynamic-against-dynamic meetings (a tank into loose pieces, piece into piece)
   lose their speculative contact unless a body opts in.

3. **Friction only in the relax pass (#1036).** `ContactConstraint::solve::<USE_BIAS>` applies
   tangent impulses only when `!USE_BIAS` (`0.8:dynamics/solver/contact/mod.rs:303-345`). Every
   friction-borne result (belt hard contacts, piles, scorecards) will move.

4. **Sleeping pairs (#1032).** `SleepIslands` now moves every pair whose other body is asleep or
   immovable to the sleeping set, non-touching pairs included (`0.8:.../islands/sleeping.rs:418-447`),
   and `wake_entity_with` returns all of them.

   Our decision stands against the transfer half. The witness is on `lane/sleeping-pairs-hold`,
   not on `main`: `a_miss_closed_by_the_last_awake_step_links_the_resting_islands`
   (`src/tests/avian_island_sleep.rs:378` there). `main`'s copy of that file has no test about
   non-touching pairs. The lane's `OVERMATCH_PATCH.md` already has the instruction, "At the rebase
   onto a release with #1032" (lines 224-232 there): keep `RESTING_MISS`, `sleep_pair`,
   `wake_misses`, `return_moved_misses`; give upstream's `should_sleep` predicate a refusal of
   every miss; take upstream's `wake_entity_with`.

   0.8 makes the refusal cheap: the predicate is a parameter of `sleep_entity_with`
   (`0.8:collision/contact_types/contact_graph.rs`, PR #1032 diff), so the refusal is a few lines
   in `SleepIslands`, not a re-fork of the graph.

5. **Solver decoupling (#989).** Islands and the constraint graph are no longer updated the moment
   a collider or body despawns; changes queue and flush in the solver
   (`FlushContactStatusChangeQueue` forces it). Any game code reading `PhysicsIslands` or
   `ConstraintGraph` between a despawn and the next step sees older state than on 0.7.

6. **Tree optimisation (#1046)** runs as one task on the `ComputeTaskPool`
   (`use_async_tasks` renamed `use_compute_task`). The hosted compute pool is 2 threads; worth one
   look under the `parallel-work` lens.

Not used by the game, so no effect: `JointDamping` (0 sites), XPBD joint fixes (no joint types in
`src/`, MEASURED).

### API changes against the game's call sites

MEASURED by `grep -rE` over `src/`: 322,333 lines; `avian3d::` appears 493 times in 126 files.

| Change in 0.8 | Game sites | Where |
|---|---|---|
| `SolverBody` and `SolverBodyInertia` are no longer components; they live in `SolverBodies`, indexed by `SolverBodyIndex` | 12 sites, 5 files | `src/contact_transfer.rs:52,226,402`; `src/net/contact_census.rs:11,597`; `src/tests/avian_island_sleep.rs:395,488,492`; `src/construction/breakage/tests.rs:1028`; `sweep_intake.rs:214,220` |
| `ContactEdge::constraint_handles` moved to `ConstraintGraph::contact_handles` | 1 | `src/tests/avian_island_sleep.rs:84` |
| `dynamics::solver::joint_graph` moved to `dynamics::joints::joint_graph` | 2 | `src/tests/avian_island_sleep.rs:8,111` |
| `Collider::convex_hull` / `convex_decomposition` borrow their inputs | 25 | 8 shown in the grep, e.g. `src/construction.rs:265`, `src/bake.rs:290` (a `map(Collider::convex_hull)` that needs a closure) |
| `ContactConstraint` holds `body_index1/2` (`SolverBodyIndex`) in place of entities | readers of `graph.colors[..].contact_constraints` | `src/contact_transfer.rs:551-729,1264`; `src/headless_test/contact_account.rs:175-248`; `rubble_climb.rs:1107`; `src/net/ram.rs:313`. Field-by-field impact not traced |
| `Scalar` to `Real`/`f32`, `Vector` to `RVector`, `AdjustPrecision`, `AsF32` | 0 (the 9 `Scalar` hits are prose) | |
| `AabbContext`, `ContactManifoldContext`, `NarrowPhasePlugin::new`, `SweptCcdSystems`, `ColliderTreeOptimization`, `JointDamping` | 0 | |
| Cargo features `f32` and `simd` removed | 0 (we enable only `serialize`, `Cargo.toml:29`) | |
| System sets | 105 sites, 21 files use them; **no rename**. Every `*Systems` enum is identical bar one added variant, `SolverSystems::ContinuousCollision` | extracted from both trees and diffed |
| `VelocityIntegrationData` (unchanged path) | 3 | `src/track/sim.rs:1640,1768,2614` |

Fork-only API the game calls, which the rebased fork must keep exporting: `ChildColliderPoses`,
`Reenabled`, `WatchedDisabling` (8 sites, 2 files); `ReducibleContacts` (4, 2);
`with_mass_properties_cache` (3, 2); `AncestorMarker` (1); `ContactCompliance` and `.compliance`
(13, 6); the fork's diagnostics fields (16 sites, 4 files).

Bevy 0.20 itself is the larger game-side cost and is outside this scout. Indicators only
(MEASURED greps against the migration guide's entries, read second-hand):

- observer syntax `On<E, B>` to `On<E<B>>`: 72 sites, 35 files;
- `Entity::PLACEHOLDER` uses: 39; `Name::new(`: 77; `TextFont`: 43; `BorderRadius`: 18;
- direct `wgpu::`: 105 sites in 4 files (wgpu 29 to 30);
- `bsn!` / `WorldAssetRoot`: 28 sites (BSN syntax changed);
- 303 files import bevy.

## 4. Work in flight

### `lane/soft-run`: rest length and inert points in a compliant contact

What it changes in the fork (MEASURED: 5 files, 69 / 17 including the doc section):

- `ContactCompliance` gains `rest` and `bears()`;
- `ContactConstraint::generate` subtracts `rest` from `initial_separation`, and cold-starts and
  neutralises a point the law does not bear;
- comment changes in `normal_part.rs`.

Where it lands in 0.8:

- **`ContactConstraint::generate`** (`0.8:dynamics/solver/contact/mod.rs:106-212`). The per-point
  loop is line-for-line the 0.7 loop, so the `inert` test, the `warm_start` gate and the
  `initial_separation` term drop in. Two differences: the function takes `&SolverBodyInertia`
  directly, and our extra `substep_secs` argument must be threaded from
  `prepare_contact_constraints` (`0.8:dynamics/solver/plugin.rs:662-750`).
- **`ContactNormalPart::solve_impulse`** (`0.8:.../normal_part.rs:114-160`). `use_bias` became a
  const generic `USE_BIAS`; our two `compliant` arms slot into the same `if` chain.
- **`manifold.compliance = None` reset** in the narrow phase. This one is harder on 0.8:
  a recycled pair skips the reset, so the invariant "reset by the narrow phase each step; set
  after it and before the solver" fails for recycled pairs. The soft run's own system rewrites
  the law each step for manifolds it owns (`lane/soft-run:src/track/soft_run.rs:530`), so a law on
  a manifold it stops owning would persist. The patch needs the reset on the recycle path too.

Verdict: **neutral to slightly harder.** The solver side is a mechanical port (DERIVED: under a
day); the recycle interaction is new and needs a witness.

### The belt as a degree of freedom in the contact solve (plan, candidate b)

The plan (`belt-loop-plan.md`, section 4) wants: a "surface drive" per body (speed, inverse
inertia, one capacity-limited hold); a friction row that adds the drive's inverse inertia to its
effective mass and applies its impulse to the drive's speed; a hold row. It estimates 150 to 200
vendor lines in `contact/mod.rs`, `tangent_part.rs`, `solver_body/` and `contact_types/mod.rs`.

Where each piece lands in 0.8:

| Piece | 0.8 location | Easier or harder |
|---|---|---|
| Drive state | A parallel `Vec` in the `SolverBodies` resource beside `bodies` and `inertias` (`0.8:dynamics/solver/solver_body/mod.rs:572-577`), reached through `SolverBodiesAccess` | **Easier.** The plan's cost "`SolverBody` grows from 56 to 64 bytes" goes away: 0.8 already keeps side arrays per solver body, and no ECS component is added |
| Friction row | `ContactTangentPart::solve_impulse` and the friction block of `ContactConstraint::solve` (`0.8:.../contact/mod.rs:303-345`) | **Easier.** `tangent_part.rs` changed by 18 lines 0.7 to 0.8, all type renames (MEASURED). Friction now runs in the relax pass only, so the row and the drive update have one call site per substep, not two |
| Hold row | Next to the friction block, in the same `!USE_BIAS` branch | Neutral. Whether a hold solved only in the relax pass meets "a braked belt within its capacity is exactly at rest" is for its witness to say |
| Effective mass with drive inertia | `ContactTangentPart::generate`, called from `ContactConstraint::generate` | Neutral |
| Single writer per drive | The colouring guarantee is restated in 0.8 (`0.8:dynamics/solver/plugin.rs:815-816`, `:919-920`) | Neutral. The overflow colour is solved serially, as in 0.7 |
| Manifold names a drive and a direction | `ContactManifold` (`0.8:collision/contact_types/mod.rs:381-407`) | **Easier, and it pays back.** A manifold that names a drive holds no per-step speed, so it survives recycling: the speed lives on the drive, written once per step by a game system. That removes the conflict in section 3.1 for belt contacts, though the hook's `retain` and `shape_contacts` rewrites still do not run on a recycled pair |

Verdict: **0.8 makes the belt change easier**, and the belt change is what lets belt contacts
recycle. If it is written on 0.7 first, keep the drive in a side table keyed by body, not as a
`SolverBody` field, so it ports without a redesign.

## 5. What 0.8 lets us delete, and where server time may come from

Fork code that goes (table in section 2): patches 4, 7, 16 and 18 outright, and patch 13 if
recycling covers its cases. DERIVED size: about 260 lines without 13, about 410 with it, of the
1,905 inserted.

Their game-side guards become guards of upstream behaviour: `src/tests/avian_moved_lists.rs`
(169 lines), `avian_collider_of_removal.rs` (94), one test in `avian_island_sleep.rs`. Keeping
them as regression witnesses or deleting them is the owner's call.

Game code: nothing in `src/` is deleted by 0.8 on its own. `HardReaction.belt` and the forced zero
go with the belt change, not with the rebase.

Named candidates for server time. No numbers are claimed: none was run on 0.8, and upstream's PR
figures are its own scenes on its own machine.

| Change | What it removes | Applies to us when |
|---|---|---|
| Contact recycling (#1033) | Geometry queries and re-matching for pairs that barely moved; upstream reports its largest narrow-phase gain on resting stacks | Piles at rest but awake (the parked-tank pile), provided recycling stays on for those pairs |
| Skip friction in the bias pass (#1036) | Half of the friction solves per substep | Always |
| Contiguous solver bodies (#1037) | ECS lookups per constraint in the solve loops | Always; `src/contact_transfer.rs` must follow |
| Tight AABBs (#996) | Velocity-expanded bounds, so fewer broad-phase pairs and narrow-phase visits around moving bodies | A moving tank beside scenery, falling pieces |
| Tree refit and large-scene work (#1047, #1048, #1049), obvhs 0.4 insertion (#1085) | Fixed per-step cost with many static colliders; spawn cost | The product map (thousands of static colliders), map rebuild between Battles |
| #1032 `wake_entity_with` and scratch buffers | Allocation per wake or sleep | Sleep and wake of large islands |

One relevant number we already hold (MEASURED by `lane/sleeping-pairs-hold` on the 0.7 fork,
recorded in that lane's `OVERMATCH_PATCH.md:201-209`, not re-run here): parked on a pile, physics
per tick was 430.9 µs on `main`, 335.8 with the hold patch, 291.4 with #1032's transfer. The hold
patch survives the rebase, so that gain is ours on either version.

## Conclusions

### The gating fact

**Avian 0.8.0 is released (2026-10-20) and requires Bevy 0.20; we pin Bevy 0.19.** There is no
avian 0.8 for Bevy 0.19 and no avian 0.7 for Bevy 0.20. The rebase is one atomic move of Bevy
0.19 to 0.20, lightyear 0.30.1 to 0.31.0, bevy_replicon 0.44 to 0.45, bevy_egui 0.42 to 0.43,
wgpu 29 to 30, glam 0.32 to 0.33, and all five vendored forks. Every needed version is published.

### Staged plan

Only slices 0 and 5 land on `main` separately. Slices 1 to 4 are checkpoints on one cutover lane,
each with its own gate, because Cargo resolves a single Bevy. Effort is DERIVED, in lane-days for
one building lane including its lens reviews, from the conflict counts in section 2.

| # | Slice | Gate | Effort |
|---|---|---|---|
| 0 | **Pre-work on `main`, on 0.7.** (a) Land or park `lane/soft-run` and `lane/sleeping-pairs-hold`, so the rebase has a fixed base. (b) Add the missing compliance section to `OVERMATCH_PATCH.md`. (c) Move the game's reads of solver internals behind their existing owners (`contact_transfer.rs`, `contact_census.rs`), so slice 3 edits two files. (d) Record the "before" set once: track scorecards, `pile_park`, the scheduled-collapse recipes, tick attribution | Existing targeted tests; no behaviour change | 2–3 |
| 1 | **Fork on 0.8.0, standalone.** A new `vendor/avian3d-0.8.0-…` from the published crate, built on its own against Bevy 0.20 (`vendor/` is outside the workspace). Reapply in this order: carried and small-rework patches (1, 2, 6, 8, 11, 12, 17, 19); collider tree (3; drop 4 and 16); narrow phase (9, 10, 14, the recycling decision, 13 kept or dropped); solver and islands (5, 15, the #1032 refusal, drop 18). Cherry-pick upstream #1101. Port the fork guards with it: the 10 `src/tests/avian_*.rs` files hold 57 tests and reference the game crate 0 to 3 times each (MEASURED) | Fork guards green against the 0.8 fork alone; each dropped patch's witness green on unpatched upstream code | 12–19 |
| 2 | **Stack cutover compiles.** Bump the manifest; re-vendor `bevy_reflect`, `bevy_pbr`, `lightyear_netcode`, `lightyear_avian3d` on their new versions; migrate the game to Bevy 0.20 and lightyear 0.31 | CI clippy and the targeted suites green | 8–15, low confidence: Bevy and lightyear migration were not scouted |
| 3 | **Game on the 0.8 fork API.** The sites in section 3: `SolverBodies` in place of components, `contact_handles`, the `joint_graph` path, `convex_hull` borrows, `ContactConstraint` body indices | Same | 1–2 |
| 4 | **Behaviour pass.** Decide CCD opt-ins; re-run the "before" set; re-measure the serial/parallel crossovers on upstream's helper (`OVERMATCH_PATCH.md:467-478`); report what moved | One before/after comparison in the lane report; owner judges feel | 4–8 |
| 5 | **After the merge, on `main`.** The belt as a solver degree of freedom on the 0.8 fork, then recycling re-enabled for belt contacts; delete patch 13 if recycling measures as good | Its own witnesses | 2–4, excluding the belt change itself |

Total: 29–51 lane-days (DERIVED sum).

### Top risks

1. **Recycling skips contact hooks** (section 3.1). Stock 0.8 leaves a recycled belt contact with
   last step's belt speed. Found by reading; it needs a witness before anything else is trusted.
2. **Three default behaviours change at once** (CCD defaults, friction in the relax pass,
   recycling), on top of our reduction and compliance patches. Piles, belt contacts and rest times
   will all move, and attribution between the three will be hard unless slice 4 toggles them one
   at a time.
3. **Reduction on recycled pairs** (patch 14). The hull selection's history ("points that carried
   load last step") assumes a fresh point set each step.
4. **Island waking rule** (patch 5) must be rewritten inside upstream's new queue-and-flush path;
   it is the patch whose ground changed most.
5. **The cutover is atomic and long.** `main` keeps moving under a lane of several weeks; the
   fork's 18 conflicting files are also the files active lanes edit.
6. **`lightyear_avian3d` 0.31 rollback** names 0.8's internal types; fork-added fields in those
   types must stay compatible with its snapshots.
7. **Unscouted cost**: Bevy 0.20 and lightyear 0.31 migration of 322,333 lines.

### Open questions for the owner

1. Is the Bevy 0.20 move wanted now for its own sake? The avian rebase alone does not justify it
   on server time until measured, and it cannot be had without it.
2. Do `lane/soft-run` and `lane/sleeping-pairs-hold` land on 0.7 before the cutover starts, or
   wait and be written once on 0.8?
3. Does the belt change go first on 0.7 (it is the fix for a measured instability), or after the
   rebase, where it is cheaper and also unlocks recycling for belt contacts?
4. CCD: accept 0.8's default (swept against static and kinematic only), or opt tanks and loose
   pieces into `SpeculativeCcd` to keep 0.7's behaviour between dynamic bodies?
5. Recycling in the interim: off globally, or a fork patch that exempts hook-modified pairs?
6. Witnesses for dropped patches: keep them as guards of upstream behaviour, or delete them?
7. Patch 13 (stationary miss cache): is "exact on its inputs" a requirement, or is upstream's
   tolerance acceptable once measured?

## Seam analysis (follow-up)

Question: which patches can shrink to a narrow extension point in the fork, with the logic in the
game crate, so the next rebase does not re-merge solver, narrow-phase and contact-graph internals?

Everything here is judged from reading 0.8.0 source. Nothing was compiled; every "fork lines
after" figure is DERIVED and every seam is unproven until built. `0.8:` paths are under the
published crate's `src/`.

**What a seam buys.** Rebase cost comes from lines edited *inside upstream's files* (66 conflict
hunks in 18 files, section 2), not from fork size as such. A new file in the fork, or logic in the
game, costs no merge. Logic moved to the game still breaks when upstream renames a public type,
but as a compile error at a named site, not a three-way merge. It also comes under the game's
lints and six lenses, which `vendor/` is exempt from.

**What 0.8.0 already offers as extension points:**

- Plugin replacement: `PhysicsPlugins` is a plugin group, each plugin can be disabled
  (`0.8:lib.rs:795-830`). The game already does this for `PhysicsTransformPlugin`.
- System-set slots: `NarrowPhaseSystems::{First, Update, Last}` (`0.8:collision/narrow_phase/mod.rs:236-240`),
  `SolverSystems` (10 variants) and `SubstepSolverSystems::{WarmStart, SolveConstraints, Relax, Damping}`,
  chained (`0.8:dynamics/solver/schedule.rs:32-70`).
- `CollisionHooks::{filter_pairs, modify_contacts}` (`0.8:collision/hooks.rs:147-187`), per pair.
- Public solver state: `SolverBodies::get_mut` / `get_inertia` / `access`
  (`0.8:dynamics/solver/solver_body/mod.rs:658-728`); `ConstraintGraph.colors[..].contact_constraints`
  and `ConstraintGraph::contact_handles` (`0.8:dynamics/solver/constraint_graph.rs:82-94,196`);
  every field of `ContactConstraintPoint` and `ContactNormalPart`, `softness` and
  `initial_separation` included (`0.8:dynamics/solver/contact/mod.rs:26-47`, `normal_part.rs:14-31`).
- `ContactGraph::sleep_entity_with(.., should_sleep)`, a predicate parameter
  (`0.8:collision/contact_types/contact_graph.rs:785`).

Not offered: anything between the pair update and status processing inside `NarrowPhase::update`
(`update_contacts` is private, `0.8:.../system_param.rs:393`); `ContactPair::manifold_count_change`
is `pub(crate)` (`0.8:collision/contact_types/mod.rs:157`); the sleep test and
`AwakeIslandBitVec` are private (`0.8:dynamics/solver/islands/sleeping.rs:164,186`); the child-pose
systems are `pub(crate)` (`0.8:collision/collider/collider_transform/plugin.rs:68,107`).

### Patch by patch

"Fork lines after" counts lines inside upstream files. Patches 4, 7, 16 and 18 are dropped by 0.8
and 13 is expected to be; they need no seam.

| # | Patch | Seam shape | Fork lines after | Does 0.8 offer it? | Run-time cost | Upstream contribution? |
|---|---|---|---|---|---|---|
| 1 | Child-collider pose writes | Disable `ColliderTransformPlugin`; the game adds upstream's `propagate_collider_transforms` plus its own `ChildColliderPoses` system in `PhysicsStepSystems::First` | ≈5 (two functions made `pub`) from ≈220 | Partly: the plugin is replaceable, the functions are `pub(crate)` | None | Yes, as a fix: writing only changed poses. Accepted, the patch disappears |
| 2 | Attachment refusal | A game observer on `Insert<ColliderOf>` that panics when either side lacks `GlobalTransform` | 0 from 18 | Yes, through Bevy observers; no Avian point needed (the silent return is `0.8:collision/collider/collider_hierarchy/mod.rs:96-99`) | One observer run per attachment. Batch spawns attach thousands, so measure it, or check in the game's own constructors | Plausible as an error or warning |
| 3 | Moved-collider bounds prune | **Not a seam.** It is a query filter inside one system of `ColliderTreePlugin`; replacing the system means replacing the plugin | ≈40 | No | None | The best candidate: upstream carries a TODO for exactly this (`0.8:collider_tree/update.rs:913`) |
| 5 | Island contact waking, rest history | Half a seam. A wake predicate consulted in `apply_contact_status_change` (`0.8:dynamics/solver/plugin.rs:303-313`), held in a resource, the rule in the game. The rest-timer handling inside the `WakeIslands` command stays a patch | ≈45 from ≈120 | No | One indirect call per contact that starts on a sleeping island (rare) | Rest-timer preservation reads as a bug fix; the wake rule is a policy upstream may not want |
| 6 | Prepared collider mass | **Not a seam** short of a custom `AnyCollider` type, which is far larger than the patch | 26 | No | None | Plausible |
| 8, 9 | Unknown features, nearest warm start | A matcher function in `NarrowPhaseConfig` beside the `match_contacts` flag (`0.8:collision/narrow_phase/mod.rs:215`), called where upstream matches (`0.8:.../system_param.rs:894-903`) with the new and old manifolds | ≈15 from ≈120 | No: the hook never sees the old manifolds | One indirect call per touching pair that was not recycled | Better sent as two fixes than as a seam; both are bug-shaped |
| 10 | Physics work counters | **Not a seam.** Counts live inside the loops they count. Shrink instead: use upstream's `recycled_count`, and take what the game's collector can count from the public graphs | ≈70 from ≈150 | Partly (public graphs, `recycled_count`) | None | Partly |
| 11 | Collider batches, explicit mass ownership | **Not a seam.** It changes what Avian's own observers and mass systems do | ≈55 | No | None | Plausible as optimisations; the `AncestorMarker` re-export is one line |
| 12 | Pairless-layer broad-phase prune | The broad phase is a replaceable plugin, but the game would then own a copy of the whole BVH broad phase. Keep the patch: it merges clean | ≈35 | A point exists and is the wrong size | None | Plausible |
| 14 | Body-pair contact reduction | A pass over `ContactGraph::active_pairs_mut()` run by the game after the pair update. Two candidates: (a) a game system in `NarrowPhaseSystems::Last`, needing only `manifold_count_change` made `pub`, **if** status changes are flushed to the constraint graph after that set (unverified); (b) a call slot inside `NarrowPhase::update`. Either way the "touching with no manifold" tolerances at the status sites stay | ≈10 (a) or ≈35 (b), from ≈450; `reduction.rs` (395 lines) moves to the game | (a) partly, (b) no | One pass per step; no per-contact indirection | A post-update contact slot is plausible. Upstream lists manifold reduction as a TODO (`0.8:.../system_param.rs:890`), but ours is opinionated |
| 15 | Rest energy | A rest-test function in a resource, called per body in `update_sleeping_states` (`0.8:dynamics/solver/islands/sleeping.rs:186-246`) with the solver body, inertia and threshold; default is upstream's test | ≈20 from ≈90 | No | One indirect call per awake body per step | A pluggable sleep criterion is plausible |
| 17 | Sleeping bodies' velocity increments | Game-side, no fork: a system in `IntegrationSystems::ClearVelocityIncrements` that clears `VelocityIntegrationData` for sleeping bodies | 0 to 10 from ≈40 | Yes, if the increment fields are public (not checked) | A query over sleeping bodies per substep | Yes, as a fix: the mismatch between the two queries is plain (`0.8:dynamics/integrator/mod.rs:261-327`) |
| 19 + soft run | Compliant contact, rest length, inert points | The fork keeps only the solve arms: a `compliant` flag and two branches in `ContactNormalPart::solve_impulse`. The law moves to a game system after `SolverSystems::PrepareContactConstraints` that finds its constraints with `contact_handles` and writes `softness`, `initial_separation` and the warm start through public fields | ≈25 from ≈130 (main plus lane) | Mostly: the fields, the lookup and the slot are public | One game pass over soft-run points per step; nothing added to Avian's loops | Per-contact softness, perhaps |
| — | Belt as a degree of freedom | Game-owned friction rows. The hook sets the manifold's friction to 0; a game system between `SubstepSolverSystems::Relax` and `Damping` reads each point's `normal_part.impulse`, solves the belt-and-bodies tangent row and the hold, and writes `SolverBodies` | **0**, against the plan's 150–200 | Yes | A serial pass over belt contact points per substep | Nothing to send |
| — | Sleeping pairs (`lane/sleeping-pairs-hold`) | Half a seam. The refusal of #1032's transfer becomes a predicate resource passed to `sleep_entity_with`. Flagging a resting miss inside the pair update, and the pair-transfer methods on `ContactGraph`, stay | ≈100 | The predicate parameter, yes; the rest, no | None beyond the lane's own scan | The counterexample is worth sending as a report on #1032 |
| — | Narrow-phase query-gap panic (`lane/sleeping-pairs`) | **Not a seam**: an assertion inside the pair loop | 24 | No | None | No |

Two consequences worth their own line:

- **The compliance seam removes the recycling problem for the soft run.** With no
  `ContactManifold::compliance` there is no per-step reset for a recycled pair to skip
  (section 4).
- **The belt seam has real unknowns.** Its rows are solved after all of Avian's rows in each
  substep, not interleaved with them, and its impulses are no longer in `ContactConstraint`, so
  `src/contact_transfer.rs` reads them from the game's store. Whether it meets the plan's witness
  ("friction never reverses the slip it opposes") has to be shown, and it is cheap to show first.

Rejected variant: solving the soft run's spring entirely in the game with the contact removed
from Avian. A pair with no constraint does not link islands, so the tank would no longer hold
loose pieces awake or wake them.

### Fork size if every feasible seam is taken

| | Files | Lines in upstream files | Source |
|---|---|---|---|
| Today, on 0.7 | 31 | +1905 −342 | MEASURED |
| Plain port to 0.8, today's patches | ≈26 | ≈1,500–1,650 | DERIVED: today less the dropped patches |
| Plain port plus soft run, belt, sleeping pairs | ≈28 | ≈1,950–2,200 | DERIVED |
| Every seam, today's patches | ≈16 | ≈380 | DERIVED: sum of the table (5+0+40+45+26+15+70+55+35+35+20+10+25) |
| Every seam plus soft run, belt, sleeping pairs, the panic | ≈18 | ≈500 | DERIVED |

About 900 lines move into the game crate (child poses ≈220, reduction ≈450, matcher ≈120, rest
test ≈90, compliance law ≈40; DERIVED). They are not deleted, they change owner.

If upstream took the fixes that are bug-shaped (1, 3, 8, 9, 17), the remainder would be about 300
lines (DERIVED). That is the owner's to send; this scout files nothing.

### Effort against a plain port

All DERIVED.

- This rebase: seams add about 3–6 lane-days to slice 1 (12–19 becomes 15–25). The work is
  writing each seam, moving the logic, and bringing ≈900 lines under the game's lints and lenses.
- The belt: about 1–2 days saved if the game-owned rows pass their witness, since there is no
  vendor patch to write or review.
- Each later rebase: slice 1 falls from 12–19 to roughly 3–6, because the conflict surface drops
  from 18 files to a handful of small, mostly additive edits.

### Reconciled with the Bevy and lightyear scout

`.agents/scratch/bevy-lightyear-upgrade-scout-2026-11-15.md` prices the flip (its slice 2) at 7–16
lane-days from its own call-site counts and excludes the Avian fork. My earlier slice 2 (8–15,
"low confidence, not scouted") priced the same work; it is withdrawn in favour of that figure.
Other overlaps: both scouts record a baseline, and its slice 6 (measure the server tick) is inside
my behaviour pass. One correction from it: the `bevy_reflect` fork is probably dropped, not
re-vendored (glam 0.33.12 lifts the serde gate).

Combined order. The dependency graph allows no intermediate lockfile, so B, C and D merge
together.

| # | Slice | Lands alone? | Effort (DERIVED) |
|---|---|---|---|
| A | Pre-work and baselines on Bevy 0.19 / Avian 0.7: settle the in-flight lanes, document the compliance patch, route solver reads through their owners, record the sim and render "before" sets once (mine 0, theirs 0) | yes | 2–3.5 |
| B | Avian fork on 0.8.0, standalone, guards ported (mine 1) | no | plain 12–19; with seams 15–25 |
| C | The flip (theirs 2: manifest, vendored re-ports including `lightyear_avian3d` on the new fork, compile fixes, shaders, verification), plus the game on the 0.8 fork API (mine 3) | only with B | 8–18 |
| D | Behaviour and measurement pass: CCD decision, recycling, crossovers, server tick (mine 4, theirs 6) | with C | 4–8 |
| E | Follow-ups: their 3 and 4, my 5. Their optional slice 5 is excluded | yes, each | 3–5 |

**Combined total: 29–54 lane-days with a plain port, 32–60 with seams.**

### Recommendation

Take the seams, selectively, as part of slice B. Do not port plainly and plan to seam later: that
pays the 0.8 merge once for code that is then moved.

- **Take:** 1 (child poses), 2 (refusal), 8 and 9 (matcher), 14 (reduction), 15 (rest test),
  17 (game-side clear), 19 with the soft run (compliance law in the game), the #1032 predicate,
  and the belt as game-owned rows. These are the patches sitting in the three hottest files
  (`narrow_phase/system_param.rs`, `solver/plugin.rs`, `islands/sleeping.rs`), and for five of
  them 0.8 already has the point or needs one visibility change.
- **Leave as small patches:** 3, 6, 10, 11, 12 and the query-gap panic. None can be a seam at a
  sensible size, and together they are about 250 lines.
- **Prove two things before committing to the plan:** that reduction can run in
  `NarrowPhaseSystems::Last` (candidate a), and that game-owned belt rows pass the belt plan's
  witness. Both are small experiments on the 0.8 crate alone.
- **Do not seam on 0.7 first.** The seams attach to 0.8's shapes (`SolverBodies`, the status
  queue, the `should_sleep` parameter). The one exception is the belt: the substep slots exist in
  0.7 too, so if it cannot wait it can be written fork-free there, with solver bodies read as
  components.
