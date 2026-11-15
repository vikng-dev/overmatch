# Stack upgrade plan: Bevy 0.20.1, lightyear 0.31.0, a fork of avian3d 0.8.0 (2026-11-15)

Subject: the game's engine, netcode and physics dependencies. Game state only; nothing here touches
the damage model. Planning at `main` `23bbde63`: no tracked file edited, nothing filed anywhere.
The only code built for it is the prototype crate on `lane/stack-proto`, which is never merged.

Inputs: `.agents/scratch/avian-0.8-rebase-scout-2026-11-15.md` ("Avian scout"),
`.agents/scratch/bevy-lightyear-upgrade-scout-2026-11-15.md` ("Bevy scout"),
`.agents/scratch/stack-seams-proto-findings-2026-11-15.md` ("the prototypes": both seams pass on
stock avian3d 0.8.0 with zero fork lines), `.agents/scratch/stack-upgrade-prep-2026-11-15.md`
("the prep file": the results of section A1, the patch table's verification and the baseline
commands), the belt plan (a session note, summarised in #353), issue #353.

Labels: MEASURED = read from a file, a grep, a registry or a script on this date. DERIVED = my
arithmetic or judgement; the inputs are named. `0.8:` paths are under the published avian3d 0.8.0
crate's `src/`.

Prices are in **bay-hours and bay-days** (one bay occupied for that long), never person-days. The
calibration: 2,328 commits on `main` in 30 days with three bays (MEASURED, given in the brief),
which is about 26 commits per bay-day (DERIVED). Each price names what bounds it: review rounds
(a hard lane took six), CI runs (a cache-cold tests lane is 28 min, MEASURED, `ci.yml:74-76`), or
a wait on the owner's eyes.

## 0. What I checked, and what it changed

| Claim (source) | Check | Result |
|---|---|---|
| Tally "4 dropped, 2 carried, 10 rework, 3 conflict" (Avian scout, section 2) | Counted the table's fate cells | The table has 4 dropped (4, 7, 16, 18), 2 carried (2, 12), 11 rework, 2 conflict (5, 13) (MEASURED). **The third conflict is patch 14**, marked "rework, the largest". It meets the scout's own definition of conflict (upstream redesigned the ground under it): `ContactEdge` in 0.8 has no `island` field and no handles (`0.8:collision/contact_types/mod.rs:55-69`), while the fork's island walk reads `contact_edge.island` (fork `dynamics/solver/islands/mod.rs:1128`) and its reduction runs inside `NarrowPhase::update` between the pair update and status processing (fork `collision/narrow_phase/system_param.rs:175`), a path 0.8 rewrote around a queue. Corrected tally: 4 dropped, 2 carried, 10 rework, 3 conflict (5, 13, 14). The tally was right and the cell was wrong. |
| "Is status flushed to the constraint graph after `NarrowPhaseSystems::Last`?" (seam 14, candidate a, "unverified") | Read `0.8:dynamics/solver/plugin.rs:124-137,189-362`, `0.8:collision/narrow_phase/system_param.rs:160-360,890-940`, `0.8:dynamics/solver/constraint_graph.rs:196-345` | **Yes, by reading.** The narrow phase only pushes to `ContactStatusChangeQueue`; `apply_contact_status_changes` runs in `PhysicsStepSystems::First` and in `PhysicsStepSystems::Solver` before `SolverSystems::PrepareSolverBodies`. Nothing is applied inside the narrow phase. A `StartedGeneratingConstraints` change reads `contact_pair.manifolds.len()` when it is applied, so it sees a count reduced in `Last`. A `ManifoldCountChanged { delta }` carries a delta fixed before `Last`. The queue is `pub struct ContactStatusChangeQueue(pub Vec<ContactStatusChange>)` with public variants, so a game pass can push its own correcting `ManifoldCountChanged`. MEASURED by prototype 1: candidate (a) needs **0 fork lines**, not the scout's ≈10 (`manifold_count_change` stays `pub(crate)`). The pass must **fold** its delta into upstream's queued entry for the pair and remove the entry at zero: upstream measures its own delta against last step's reduced manifolds, so a pair reduced from 3 to 1 gets `+2` queued every step, and a second entry trips `debug_assert!(!contact_pair.manifolds.is_empty())` at `0.8:dynamics/solver/constraint_graph.rs:218` (MEASURED panic). |
| Recycling can be switched off without a patch | `0.8:collision/narrow_phase/system_param.rs` `'recycle` block; `:406` | `recycle_distance <= 0.0` breaks out before any reuse, and `recycle_distance_non_touching = recycle_distance.min(contact_tolerance)`. Decision 2 is one config value: `NarrowPhaseConfig { recycle_distance: 0.0, .. }`. 0 fork lines (MEASURED from source). |
| "#1032's `should_sleep` predicate takes a refusal of every miss in a few lines" (Avian scout 3.4) | `0.8:collision/contact_types/contact_graph.rs:785-820` | **Narrower than stated.** The predicate is `S: Fn(Option<Entity>) -> bool` and receives only the other body. It cannot tell a touching pair from a miss. The refusal needs the predicate widened to see the edge (`ContactEdge::is_touching`, in hand at the call), about 10 fork lines in `contact_graph.rs` and `islands/sleeping.rs` (DERIVED). That is the whole of the refusal: `main` holds no sleeping-pairs hold (fork `OVERMATCH_PATCH.md`, "Resting misses stay in the narrow phase"), so no flag and no pair-transfer method is carried. |
| Patch 17 can be game-side "if the increment fields are public (not checked)" | `0.8:dynamics/integrator/mod.rs:217-233,62-73` | `VelocityIntegrationData::{linear_increment, angular_increment}` are `pub`, and `IntegrationSystems::ClearVelocityIncrements` exists. 0 fork lines (MEASURED). |
| "The law finds its constraints with `contact_handles`" (seam 19, belt seam) | `0.8:dynamics/solver/plugin.rs:690-745`, `constraint_graph.rs:105-125` | `ContactConstraintHandle::local_index` indexes `manifold_handles`. `contact_constraints` skips a manifold whose constraint has no points, and skips static-static pairs, so the two vectors can differ in length. The game finds a constraint by its public `contact_id` and `manifold_index` fields, as `src/contact_transfer.rs` already scans. A caveat for both prototypes, not a blocker. |
| Substep slot for game-owned rows | `0.8:dynamics/solver/schedule.rs:58-70`, `xpbd/plugin.rs:36-42` | The substep order is `Velocity, WarmStart, SolveConstraints, Position, Relax, Damping`, chained. Upstream's own XPBD sets sit `.after(Relax).before(Damping)`, so the slot is a used one. A point with zero friction has `tangent_part: None` and Avian solves no friction row for it (`0.8:dynamics/solver/contact/mod.rs:26-47,311-345`). |
| Wake rule location (decision 3) | `0.8:dynamics/solver/plugin.rs:293-313` | Confirmed: `islands.add_contact` returns the island, and a sleeping one is pushed to `islands_to_wake` unconditionally. |
| "10 `src/tests/avian_*.rs` files hold 57 tests" | `grep -c '#\[test\]'` at `23bbde63` | 61 in 11 files (MEASURED): `avian_body_query_gap.rs` and `avian_compliant_contact.rs` are on `main`. **A twelfth guard file exists**: `src/net/ram/avian_tests.rs` (13 tests) holds the six `stationary_miss_*` tests, the witnesses of patch 13. |
| "Wake rule as a predicate" (decision 3) | `0.8:dynamics/solver/plugin.rs:191,298-313`; fork `narrow_phase/system_param.rs:440-541` | **A predicate alone is not enough.** By the time the wake is decided the pair's manifolds are pushed, and the fork's rule also moves a refused pair to the sleeping set. Upstream holds `Res<ContactGraph>` there. Either the hook also transfers the pair (≈30 to 45 fork lines, DERIVED) or the rule runs as a game pass in `NarrowPhaseSystems::Last`, by the method prototype 1 proved (0 fork lines, unproven). Prep file, section 7. |
| "The lightyear fork delegates to `ChildColliderPoses`" | `lightyear_avian3d` 0.31.0 `plugin.rs`, `rollback.rs` | A vendored crate cannot call the game crate. With the placement law in the game, the lightyear patch is "do not register `update_child_collider_position`" and the game registers its own system in that slot. `rollback.rs` is taken unpatched: patch 16 is dropped, and rollback is compiled but never registered (`rollback_resources` defaults to false). Prep file, section 2. |
| "`RenderDebugOverlay` added to default plugins" (Bevy scout) | `bevy_internal` 0.19.1 and 0.20.1 `default_plugins.rs`; `bevy` `Cargo.toml` features | Not in our build on either version (it needs `render_dev_tools`, which is in `dev`, not `default`). The real change is in `UiWidgetsPlugins`: one rename and four new members. Prep file, section 4. |
| glam after the move | crates.io dependency API | obvhs 0.4.0 asks `glam >=0.33, <0.35`; parry3d 0.31 rides `glamx ^0.3`, which asks `glam ^0.33` (MEASURED). DERIVED: the lock can hold one glam, and `AUDITED_SPLIT_GLAM` can become empty. obvhs then compiles against the `scalar-math` glam, so tree results may move. |
| Baseline instruments exist | each script's argument parser or header | All named in section A2 exist and take the flags shown (MEASURED; prep file, section 8). No tracked script wraps per-system tick attribution: the prep file proposes one. `scripts/perf/collapse.sh` already writes a coarser `tick-attribution.json`. |
| Bays | `scripts/bays/status` | bay-1 `lane/soft-run` (9 ahead), bay-2 `lane/ramp-test`, bay-3 free (MEASURED). |

Not checked: lightyear 0.31 beyond `lightyear_avian3d`; Bevy 0.20 beyond `DefaultPlugins`, the
widget and picking groups and the many-query iterators. Avian 0.8.0 uses `On<Add<T>>` and
`iter_many(..).matched()` in its own code (`0.8:dynamics/joints/joint_graph/plugin.rs:241`,
`0.8:dynamics/rigid_body/body_size_metrics.rs:110`), which agrees with two of the Bevy scout's
migration items. The Bevy scout's call-site counts were not recounted.

## A. Preparation, before the queue drains

### A1. Read-only (no bay)

Each item's result is a section of the prep file
(`.agents/scratch/stack-upgrade-prep-2026-11-15.md`), which the cutover briefs cite directly.

1. **The fork's export list is frozen** (prep file, section 1): every fork-only item the game
   calls, its sites, and its name and owner after the cutover. Slices B2 and B4 code against it.
   Two entries change the shape of the work: `ChildColliderPoses` is game-owned and the vendored
   `lightyear_avian3d` names no fork-only Avian item; the compliance law leaves `ContactManifold`
   for a game store keyed by contact and manifold index.
2. **`lightyear_avian3d` 0.31.0 against the fork** (prep file, section 2): `rollback.rs` is taken
   unpatched; the fork adds no field to `ColliderTree`, `ColliderTrees` or `MovedProxies` (that
   snapshot is field by field and would miss one); `plugin.rs` keeps one patch, the removal of
   its child-pose registration.
3. **The six `par_iter_many` sites** (prep file, section 3): one named legitimate absence
   (`track/world_contact/admission.rs:120`), five bugs with the `expect` text for each.
4. **The plugin lists** (prep file, section 4): in `server_plugin_floor`, one rename
   (`EditableTextInputPlugin` to `TextInputPlugin`) and four added disables (`DialogPlugin`,
   `ModalDialogPlugin`, `TabPlugin`, `PointerFocusPlugin`). No other list changes.
5. **The `ContactCompliance` section** of `vendor/avian3d-0.7.0-child-pose/OVERMATCH_PATCH.md` is
   drafted (prep file, section 5). It is committed only if `lane/soft-run` parks; that lane
   carries its own section, which replaces the draft whole when it merges.
6. **The issues** are drafted in section G.

### A2. One bay

**Baselines on 0.19, recorded once**, at the commit tagged for alpha.16 so "before" is a shipped
build. Outputs go under `.agents/scratch/stack-upgrade-baseline/` with the commit hash in each
file name. Bound: about 4 to 6 bay-hours, set by run lengths, not by review (DERIVED).

| Baseline | Command or script (all exist, MEASURED; flags and build prerequisites in the prep file, section 8) | Output |
|---|---|---|
| Cross-target bitprobe pair | `cargo run --release --locked --features bitprobe --bin bitprobe -- bitprobe-macos-aarch64.obp.gz` (through `scripts/serial-cargo`), then `gh workflow run bitprobe.yml --ref <tag or main>` and `python3 scripts/bitprobe/diff.py <mac> <linux>` (`scripts/bitprobe/README.md`) | Both dumps kept. Expected `IDENTICAL`. |
| Track scorecard | `scripts/track-scorecard` | `target/track-scorecard.{md,json}`, copied out |
| Track probes | `scripts/serial-cargo test --lib --features dev_ui,bitprobe <probe> -- --ignored --nocapture` for `belt_friction_probe`, `creep_stall_probe`, `rubble_climb_probe`, `turn_probe` (`src/headless_test/track_scorecard/`) | The printed tables |
| Parked on a pile | `/usr/bin/lockf -k -t 2400 /tmp/overmatch-cargo.lock python3 scripts/perf/pile_park.py --out target/pile-park` | `summary.json` and pooled tables |
| Scheduled collapse | `scripts/perf/collapse.sh --target local --schedule scripts/perf/match-collapse.json --rehearsal` | The run directory |
| Tick attribution | Per system: no tracked wrapper exists. The prep file (section 8) holds a proposed `scripts/perf/tick-trace.sh` and the move of the untracked `trace_profile.py` beside it; the baseline lane lands both, or repeats the two phases of `.agents/scratch/perf-2026-11-01/server-tick.md` by hand and records its command line. Per stage: the collapse run above already writes `tick-attribution.json` | Per-phase top-systems table; the collapse run's stage account |
| Damage-model cost harness (game state only) | `scripts/cost/run.sh idle1 fire1` then `scripts/cost/analyze.py` | Tables |
| Shader pipelines compile | `scripts/render/verify-shader-compile.sh 30` | Pass or fail, with the log |
| Shader captures | `OVERMATCH_CAPTURE=<plan> target/debug/overmatch --offline` for `scripts/capture/example.ron`, `repair.ron`, `wood-preview.ron`, `lod-showcase.ron`, `tab-board-large.ron`, `tab-board-small.ron` and the nine plans in `scripts/capture/surroundings/` | PNGs under `target/capture/`, copied out |
| Capture gaps | Compare the plans above with the 17 files in `assets/shaders/` and add plans, in this lane, for any pipeline no plan shows: pen-cam (six `pen_cam_*.wgsl`), `optic_mask.wgsl`, `render_scale_upscale.wgsl`, `vfx_billboard.wgsl`, `vfx_trail.wgsl`, belt meshes. Whether the existing plans cover them was not checked | New `.ron` plans on `main`, each with its PNG |

**Two seams, proven on stock avian3d 0.8.0.** Both passed with zero fork lines (MEASURED;
`.agents/scratch/stack-seams-proto-findings-2026-11-15.md`). The table of B2 and follow-up D1 are
built on them.

Where the code is:

- One standalone crate, `proto/stack-seams/`, on the lane branch `lane/stack-proto` (four
  commits, never merged, never pushed). Read it with `git show lane/stack-proto:proto/stack-seams/<path>`:
  `src/reduction_pass.rs`, `src/reduction.rs`, `src/belt.rs`, `tests/contact_reduction.rs`,
  `tests/belt_rows.rs`, and the last full run in `EVIDENCE.log` (15 tests green).
- Its `Cargo.toml` carries an empty `[workspace]` table, so the repo's manifest, lock and
  `vendor/` are not consulted. Dependencies: `avian3d = "0.8.0"` from crates.io with features
  `3d` and `parry-f32` only (**Avian's `parallel` feature is off**), and `bevy = "0.20"` with
  default features off.
- To run it again in a leased bay: `CARGO_TARGET_DIR=<bay>/proto/stack-seams/target
  scripts/serial-cargo test --profile test --manifest-path proto/stack-seams/Cargo.toml --
  --nocapture --test-threads=1`. `--profile test` is required: `serial-cargo` rewrites a bare
  `test` to `--profile edit`, which the crate does not define (`scripts/serial-cargo:34-36`).
  Remove `proto/stack-seams/target` afterwards (678 MB, MEASURED) so the bay returns clean.

**Seam 1: contact reduction is a game-side pass.**

- Form: one system in `NarrowPhaseSystems::Last` walks `ContactGraph::active_pairs_mut()`, takes
  the manifolds of every touching, constraint-generating, hook-free pair between two marked
  non-static bodies, reduces per body pair, and puts the kept manifolds back.
- The pass **folds its delta into upstream's queued `ManifoldCountChanged` entry** for the pair,
  removes the entry at zero, and pushes a fresh entry only where upstream queued none. It never
  pushes a second entry for a pair. A pair with a queued `StartedGeneratingConstraints` needs no
  correction: the flush reads the reduced `manifolds.len()`.
- It needs no `pub` change in Avian: `manifold_count_change` and `ContactStatusBits` stay
  private. The fork's `REDUCIBLE_CONTACTS` flag becomes a query for the marker component, which
  moves to the game. The fork's two status-site tolerances and its island walk are dropped with
  nothing in their place: 0.8.0 links a contact to its island when it starts touching and
  unlinks it when it stops, whatever its manifold count.
- MEASURED: over 600 steps under four rules the constraint graph matched every touching pair
  (0 mismatches in 296,999 to 418,591 pair comparisons per rule); three fork guards pass; kept
  points entered the solver with exactly last step's impulse on 40 of 40 point-steps.
- MEASURED with recycling at 0.05: after a disturbance, reduction stopped (64 manifolds and 256
  points where the baseline holds 4 and 4) unless the pass sets `ContactPair::recycling_cache =
  None` on the pairs it takes, which restores the baseline with no fork line. Input to D2.

**Seam 2: belt friction is game-owned rows.**

- Form: a contact hook sets the manifold's friction to 0, so Avian builds no tangent part. One
  system between `SubstepSolverSystems::Relax` and `Damping` finds the manifold's constraint by
  `contact_id`, reads each point's `normal_part.impulse`, and solves **friction and hold together
  as one 2x2 boxed problem** per manifold, exactly, by its nine active sets. When the hold holds,
  the drive is assigned its anchor speed, so "at rest" is exact. Solved one after the other, the
  rows under-deliver stick friction on a held belt by a factor of 1 + (effective mass / belt
  inertia): 13 times too soft at ratio 12 (MEASURED arithmetic).
- A drive is a plain struct in a game resource keyed by body. The row's impulses are read from
  that store (`step_friction_impulse`, `step_hold_impulse`, cleared in
  `SolverSystems::PreSubstep`); Avian's stored tangent impulse for a belt manifold stays zero.
- MEASURED: slip never reversed on any substep for mass-to-inertia ratios 0.2, 1, 5 and 12
  (0 of 1,200 rows each), where today's one-tick-late scheme alternates on 200 of 200 steps at
  ratio 1 and above. With the hold above friction the drive is exactly at rest on every row.
  With the hold below it the box decelerates at **B/(M+I)** (20 N over 10 kg + 1 kg: 1.8182 m/s²
  DERIVED, matched to four decimals), not B/M: once belt and box move together the hold slows
  both. The impulse on the bodies equals the impulse on the drive to 2.7e-7 N s per substep.
- A game system that writes the drives must sit in a solver set: the broad phase's hook
  parameter reads them, and `PhysicsSchedule` refuses the ambiguity at build (MEASURED panic).

What the spikes did not cover is listed as acceptance items on the slice that owns it: B2 for
reduction, D1 for the belt, D2 for recycling.

### A3. Settled on `main` before the cutover starts

- `lane/soft-run` is merged or parked, and `lane/ramp-test` has left its bay. The fork port needs
  a fixed base. Decision 4's witnesses are on `main` (`src/tests/avian_island_sleep.rs`), and the
  sleeping-pairs hold is recorded there as forgone, so no sleeping-pairs lane is carried.
- alpha.16 is tagged and released on the current stack (decision 5).
- All three bays are released.

## B. The cutover

### B0. Shape

- **One trunk branch, `lane/stack-cutover`, held in bay 1.** Two sibling lanes,
  `lane/stack-render` (bay 2) and `lane/stack-net` (bay 3), branch from the trunk after B1 and are
  merged into it inside bay 1 at the integration points below. A merge between lane branches in
  the trunk's own bay changes only that bay's files, index and HEAD, which the bay rule allows.
  The merge to `main` happens once, by the orchestrator, after the six lenses pass over the whole
  diff (decision 5).
- **The cutover takes all three bays.** No other building lane runs meanwhile, so `main` moves
  only by doc and orchestrator commits, and the trunk does not chase it.
- **The tree is red from B1 until B5.** A slice is "done" by its own checkpoint, stated per slice.
  One caveat about the checkpoints of B3 and B4: the game is one crate, and rustc reports
  type errors only after name resolution succeeds everywhere, so "no compiler error in the files
  this slice owns" is measured against the current frontier and re-measured after each
  integration merge.
- **Pushing.** Lanes never push. The Linux bitprobe leg and the full suite need a pushed ref, so
  the orchestrator pushes the trunk branch (not `main`) to run them. That needs the owner's
  approval (question F2).

### B1. Skeleton flip and mechanical sweep (bay 1, serial, before the fan-out)

One lane, a handful of commits, so that the three parallel slices start from the same resolved
lockfile and do not all edit the same 35 files.

- Manifest: `bevy` and `bevy_winit` 0.20.1, `avian3d` 0.8, `lightyear*` 0.31.0, `bevy_replicon`
  0.45, `bevy_egui` 0.43, `wgpu` 30, `glam` 0.33.x with `serde` and `scalar-math`.
- `vendor/`: new directories from the **pristine** published crates, `avian3d-0.8.0-<suffix>`,
  `bevy_pbr-0.20.1-<suffix>`, `lightyear_netcode-0.31.0-admission`,
  `lightyear_avian3d-0.31.0-child-pose`, each with an empty `OVERMATCH_PATCH.md`; the 0.19-era
  directories are deleted in the same commit (one path, no kept switch). `bevy_reflect` is not
  re-vendored if the lock resolves a glam whose scalar `BVec3A`/`BVec4A` have serde (0.33.12 does,
  Bevy scout); otherwise it is re-carried.
- `deny.toml` glam bounds and `src/tests/determinism_deps.rs` move to the new glam;
  `AUDITED_SPLIT_GLAM` becomes empty if `cargo tree -i glam` shows one glam (section 0).
- Mechanical sweeps, each a scripted commit over the whole tree: observer syntax
  (`On<Add, T>` to `On<Add<T>>`, 72 sites in 35 files), `MeshTag::new` (4), `get_aabb` (3),
  `set_if_different` (2), `TextInputPlugin` (1), pointer events (3), `#[extract_app(RenderApp)]`
  (3 derives). Counts are the Bevy scout's (MEASURED there, upper bounds).
- **Checkpoint:** `cargo metadata --locked` resolves; `cargo deny check bans` passes;
  `determinism_deps` logic passes when run as a script over the new lock (it is a lockfile parser
  and does not need the game to compile); a grep finds no old-form observer.
- Price: 3 to 5 bay-hours (DERIVED). Bounded by one orchestrator read; no lens round, since the
  slice is not a landing.

### B2. The 0.8 fork, standalone, with its guards (bay 1, trunk)

**Owns:** `vendor/avian3d-0.8.0-*/**`; new game modules under `src/avian_ext/` (proposed name; one
file per seam); `src/tests/avian_*.rs`; `src/net/ram/avian_tests.rs`. Touches no other game file.
The names it exports are frozen in the prep file, section 1.

**Built standalone.** `vendor/` is outside the workspace (`Cargo.toml:10`). A guard crate beside
the fork (in the bay, tracked on the trunk, deleted at B5) depends on the fork by path and on
Bevy 0.20.1, and includes the game-side seam modules and the guard tests by `#[path]`. That works
because the guard tests reference the game crate 0 to 3 times each (MEASURED by the Avian scout);
those few references are cut or stubbed in the guard crate. The seam modules must therefore depend
only on Avian and Bevy, which is also the right shape for them.

**Order inside the slice** (the Avian scout's, adjusted): carried and small items (2, 6, 11, 12,
17, the query-gap panic, upstream #1101); collider tree (3; witnesses for dropped 4 and 16);
narrow phase (8 and 9 matcher, 13 module, 14 pass, counters 10); solver and islands (5 wake rule,
15 rest test, the #1032 refusal, 19 solve arms); `recycle_distance: 0.0` in the game's config.
The solver-and-islands step opens with the row-5 check: the four wake witnesses ported into
`proto/stack-seams/` and run on stock 0.8.0 with nothing, then with the wake rule as a game pass
(prep file, section 7). Its result picks row 5's form before row 5 is written.

**Target form of each patch (decision 1).** "Fork lines after" counts lines inside upstream's
files and is DERIVED (the scout's seam table, adjusted by section 0). Forms: *hook* (a narrow
extension point in the fork, logic in the game), *module* (a self-contained fork file with one or
two call sites), *game-side* (no fork line), *kept* (stays a patch, with the reason), *dropped*.

| # | Patch | Form | Fork lines after | Game code | Witness |
|---|---|---|---|---|---|
| 1 | Child-collider pose writes | hook: `ColliderTransformPlugin` stays and its one registration of `update_child_collider_position` is deleted. (Disabling the plugin would also need `AncestorMarkerPlugin` made public: `ancestor_marker` is `pub(crate)`, `0.8:lib.rs:557`) | ≈4 | `src/avian_ext/child_poses.rs` owns `ChildColliderPoses`, `Reenabled`, `ReenabledPlugin`; the game adds its system in `PhysicsStepSystems::First` and, in place of lightyear's, in `RunFixedMainLoopSystems::AfterFixedMainLoop` | `src/tests/avian_child_pose_churn.rs` (9 tests) |
| 2 | Attachment refusal | game-side observer on `Insert<ColliderOf>`. Falls back to the 18-line patch (it merges clean) if the per-attachment observer shows in the spawn-batch cost | 0 | `src/avian_ext/attachment.rs` | the `should_panic(expected = "without a GlobalTransform")` test in `avian_child_pose_churn.rs` |
| 3 | Moved-collider bounds prune | kept: a query filter inside one upstream system; replacing the system means replacing the plugin | ≈40 | none | `src/tests/avian_moved_aabb_prune.rs` (2) |
| 4 | Standalone key write-back | dropped (upstream #1062) | 0 | none | `avian_collider_of_removal.rs` (2), run on unpatched code |
| 5 | Island contact waking, rest history | wake rule: a game pass in `NarrowPhaseSystems::Last` that takes a new contact between resting sides out of the status queue, links it to its island and moves the pair to the sleeping set (unproven; the row-5 check decides). Else a hook in `apply_contact_status_change` that holds the predicate **and** the transfer: a predicate alone leaves a constraint between bodies with no solver body. The rest-timer handling inside `WakeIslands` and the `Sleeping` removal hook stays a patch | ≈15 with the pass, ≈45 to 60 with the hook | `src/avian_ext/island_wake.rs`: "only an awake non-static body wakes an island" | `stationary_contact_does_not_wake_a_resting_island`, `an_impact_wakes_an_island_that_merges_again_before_the_solver`, `waking_a_merged_island_preserves_already_awake_rest_time`, `removing_sleeping_restarts_the_woken_bodys_rest_interval` |
| 6 | Prepared collider mass | kept: short of a custom `AnyCollider` there is no point to attach to | ≈26 | none | `avian_spawn_batch.rs` |
| 7 | Minimum speculative margin | dropped (upstream clamps to `contact_tolerance`) | 0 | none | `stationary_contacts_keep_the_configured_minimum_speculative_margin`, on unpatched code |
| 8, 9 | Unknown features; nearest warm start | hook: a matcher function in its own resource, called where upstream matches (not a field of `NarrowPhaseConfig`, which derives `Reflect`, `PartialEq` and serde). Not game-side: the old manifolds are a local of the per-pair closure | ≈15 | `src/avian_ext/contact_match.rs` | `src/tests/avian_warm_start.rs` (2) |
| 10 | Physics work counters | kept, shrunk: counts live inside the loops they count; use upstream's `recycled_count` and count the rest from the public graphs in the game's collector | ≈70 | the existing collector | the cost harness reads them; no guard of its own |
| 11 | Collider batches, explicit mass ownership | kept: it changes what Avian's own observers and mass systems do | ≈55 | none | `src/tests/avian_spawn_batch.rs` (4) |
| 12 | Pairless-layer broad-phase prune | kept: the only point is the whole broad-phase plugin | ≈35 | none | `src/tests/avian_broad_phase_layers.rs` |
| 13 | Stationary non-touching pair cache (kept, decision 2) | module: a new fork file `collision/narrow_phase/stationary_miss.rs` (≈120 lines, not in an upstream file) with two call sites in the pair update and one field on `ContactPair`. Not game-side: it must run inside the private per-pair closure, before the geometry query | ≈20 | none | the six `stationary_miss_*` tests in `src/net/ram/avian_tests.rs` |
| 14 | Body-pair contact reduction | game-side pass in `NarrowPhaseSystems::Last` that folds its delta into upstream's queued status entry, never a second entry. No `pub` change in Avian. The fork's reducible flag, status-site tolerances and island walk are dropped | 0 (MEASURED, seam 1) | `src/avian_ext/contact_reduction.rs`: the fork's 395-line `reduction.rs` with its imports changed (`f32`, `Vec3`, `Vec2` for `Scalar`, `Vector`), a driver of about 150 lines (DERIVED from the spike's pass), the `ReducibleContacts` marker and the counters | `src/tests/avian_contact_reduction.rs` (21), and the acceptance items below |
| 15 | Rest energy | hook: a rest-test function in a resource, called per body in `update_sleeping_states`; the default is upstream's test | ≈20 | `src/avian_ext/rest_test.rs` | the five `rest_*`, `the_original_velocity_test_*`, `a_long_bar_*`, `a_supported_bar_*` tests in `avian_island_sleep.rs` |
| 16 | Moved-list removal by index | dropped (upstream `MovedProxyList`). Our panic on a second entry becomes upstream's silent `false`; that is a fifth outcome by our rule, accepted here and named in the patch doc, with the witness kept | 0 | none | `src/tests/avian_moved_lists.rs`; its `should_panic(expected = "marked moved twice")` test is rewritten to state upstream's behaviour or deleted |
| 17 | Sleeping bodies' velocity increments | game-side: a system in `IntegrationSystems::ClearVelocityIncrements` (fields are public, section 0) | 0 | `src/avian_ext/sleeping_increments.rs` | `a_woken_body_takes_only_its_first_steps_gravity` |
| 18 | Serial small loops | dropped (upstream #1035 has its own floor); re-measured in D2 | 0 | none | none; a measurement |
| 19 | Compliant contact, with the soft run's rest length and inert points | hook: a `compliant` flag and two arms in `ContactNormalPart::solve_impulse`; the law runs in the game after `SolverSystems::PrepareContactConstraints`, finding constraints by `contact_id` (section 0) | ≈25 | `src/avian_ext/compliance.rs`, called by the track's soft run | `src/tests/avian_compliant_contact.rs` (on `lane/soft-run` today) |
| — | #1032 transfer refused for non-touching pairs (decision 4) | hook: `sleep_entity_with`'s `should_sleep` widened to see the edge; `SleepIslands` passes "touching, and the other body rests". No flag and no transfer method: `main` holds no sleeping-pairs hold | ≈10 | none | `a_miss_closed_by_the_last_awake_step_links_the_resting_islands`, `an_impact_wakes_an_island_that_merges_again_before_the_solver`, `an_impulse_on_a_sleeper_meets_its_neighbour_struck_in_the_same_step` (`src/tests/avian_island_sleep.rs`) |
| — | Narrow-phase query-gap panic | kept: an assertion inside the pair loop | ≈24 | none | `a_body_query_gap_still_panics_the_solver` |
| — | Upstream #1101 (`Without<ColliderDisabled>` in `solve_continuous`) | cherry-pick | ≈1 | none | none of ours |
| — | Contact recycling off | config value, no patch | 0 | a new `NarrowPhaseConfig { recycle_distance: 0.0, ..default() }` where the game composes physics (`src/net/physics.rs`); the game inserts none today | a one-line test that the shipped config has `recycle_distance == 0.0`, mounted on the shipped plugin |
| — | Belt friction rows | game-side, after the merge (#353) | 0 (MEASURED, seam 2) | follow-up D1 | follow-up D1 |

Sum: about 345 to 405 lines inside upstream files (DERIVED: 4+40+15+26+15+70+55+35+20+20+25+10+24+1
= 360 with the wake rule as a game pass, less up to 15 if stock `WakeIslands` already keeps rest
history; 390 to 405 with the wake hook), plus one fork-only file of about 120 lines. That is
under the scout's ≈500 with patch 13 kept; the epic's target is restated as about 350 to 400. About 900 lines change owner to the game (DERIVED by
the scout) and come under the game's lints and the six lenses.

Which rows still rest on a reading of 0.8.0 that nothing has run, and the guard that settles
each, are listed in the prep file, section 7. In order of risk: 5 (the wake rule's form), 10
(its size), 6 (not read), 4 and 12 (the scout's reading). Each is settled inside this slice.

**Acceptance items the seam-1 spike did not cover.** Patch 14 is done when all hold on the fork:

- all 21 guards of `src/tests/avian_contact_reduction.rs` are green. The spike ran 3; the other
  18 are not yet run on 0.8: the 14 pure-function guards of `reduce_body_pair`, and the
  world-level ones, among them `a_body_held_by_its_fifth_corner_stays_up`,
  `a_reduced_body_rests_on_its_loaded_supports_and_sleeps` and
  `only_marked_dynamic_body_pairs_reduce`;
- the landing speed is read per body from the slower body's `SleepThreshold`, as the fork does;
  the spike used a fixed 0.15 m/s;
- body-pair groups reduce in parallel, each owning disjoint pairs, as the fork does; the spike
  reduced them serially. Their effects (the manifolds put back, the queue corrections, the
  counters) commit in pair order;
- the pass and its guards are green with Avian's `parallel` feature on, which the game builds
  with and the spike did not;
- the pass sets `recycling_cache = None` on the pairs it takes, so that turning recycling on in
  D2 cannot stop reduction. With recycling off this changes nothing (MEASURED).
- *The side slices' vacated band:* `WorldContacts::modify_contacts` drops each side-slice point in
  the band its belt has vacated, using that tick's `TrackRise`. On Avian 0.8 a recycled pair returns
  before hooks and would keep last step's verdict as the belt rises or settles, so belt-face pairs
  (`BeltFace` colliders) must be exempt from recycling, or a witness must show the verdict unchanged
  by reuse. The hook also wakes a sleeping tank when an awake body's candidate pair reaches a belt
  face, so both the band and the wake need the hook called every step for every candidate pair.
  Input to D2.

**Checkpoint:** every guard green in the guard crate against the fork alone; each dropped patch's
witness green with that patch absent; the acceptance items above hold; `OVERMATCH_PATCH.md` has
one section per row above, each naming its form and its witness; the export list of the prep
file compiles; lines inside upstream files are counted and reported.

**Price:** 2 to 4 bay-days (DERIVED). Bounded by review rounds on the ≈900 moved lines (the six
lenses apply to `src/avian_ext/`; allow up to six rounds at 2 to 4 bay-hours each) and by the
row-5 check. The wake hook, if the pass fails, adds about half a bay-day.

### B3. Render (bay 2, `lane/stack-render`)

**Owns:** `vendor/bevy_pbr-0.20.1-*/**`; `assets/shaders/**`; `src/terrain_shadow.rs`,
`src/scene_depth.rs`, `src/render_scale.rs`, `src/render_cost.rs`, `src/frame_cost.rs`,
`src/settings/probe.rs`, `src/tank_surface.rs`, `src/track/belt_draw.rs`, `src/track_prints/**`,
`src/vfx/**`, `src/damage_cam/pen_cam/**`, `src/tests/gpu_layout.rs`,
`src/tests/bevy_mesh_removal_lifecycle.rs`, `scripts/render/verify-shader-compile.sh`.

- `bevy_pbr` re-port: drop the `directional_shadow_factor` backport (upstream in 0.20.1); drop the
  `MeshUniform` alignment patch only if `gpu_layout` and a windowed Metal boot agree; re-carry the
  mesh-instance removal batching against the restructured collection code; re-port the heightfield
  shadow (view bindings 39 and 40 are free in 0.20.1, Bevy scout) with its five shader edits
  rewritten as WESL. `src/terrain_shadow.rs:557-559` includes two vendored shader files by path:
  the paths and extension move with it.
- 17 shaders to WESL (2,494 lines, 37 import lines, 81 `#ifdef`-family lines, 33 substitutions;
  Bevy scout, MEASURED there). The two game-to-game imports (`track_belt`, `construction_holes`)
  go first.
- Render Rust: `scene_depth` (depth-stencil attachment rename; the view and aspect choice is
  re-derived, not renamed), `settings::probe` (`ExtractedWindows` to queries; `get_mapped_range`
  returns `Result`), `ShaderBuffer` constructors, wgpu 30, and an explicit `.before`/`.after` on
  the two `Res<RenderQueue>` uploads (`track_prints/upload.rs`, `pen_cam/picture.rs`) now that
  render sets order weakly.
- The game's damage-model depth masks keep `Tonemapping::None`: both 0.19.1 and 0.20.1 skip the pass (`bevy_core_pipeline/src/tonemapping/node.rs:46`, DERIVED).
- **Checkpoint (not green):** the vendored `bevy_pbr` builds standalone against Bevy 0.20.1; no
  `.wgsl` with `#import`, `#ifdef` or `#{..}` remains; no compiler error in owned files at the
  current frontier. The real gate is at B5: `scripts/render/verify-shader-compile.sh`, then the
  captures.
- **Price:** 1.5 to 2.5 bay-days (DERIVED), then one owner sitting at B6. Bounded by the owner's
  eyes: a shader that compiles and shades differently is silent.

### B4. Net and sim Rust (bay 3, `lane/stack-net`)

**Owns:** `vendor/lightyear_netcode-0.31.0-admission/**`,
`vendor/lightyear_avian3d-0.31.0-child-pose/**`, `src/net/**` except `src/net/ram/avian_tests.rs`,
`src/contact_transfer.rs`, `src/construction/**`, `src/debris/**`, `src/track/**` except
`belt_draw.rs`, `src/headless_test/**`, `src/bake.rs`, `src/lib.rs`, `src/flow.rs`,
`src/settings/ui.rs`, `src/run_window.rs`, `src/platform.rs`, and every file no other slice names.

- `lightyear_netcode` admission patch re-applied on 0.31.0 (upstream changed only
  `server_plugin.rs`).
- lightyear 0.31: `LinkOf { endpoint }` (1 site), replicon 0.45 deep paths, and a first-boot check
  for the B0001 panic on a system that pairs lightyear's interpolation parameter with its own
  `ConfirmedHistory<C>` query (13 lines in 3 files, Bevy scout).
- The six `par_iter_many` sites, per the prep file, section 3.
- Plugin lists, per the prep file, section 4; `damage_cam/pen_cam/game.rs:621` (`bevy::ecs::error::ignore`)
  reviewed so no app swallows a routed panic (ADR-0011).
- Game on the 0.8 fork API: `SolverBodies` by index in place of the `SolverBody` component (12
  sites in 5 files), `ConstraintGraph::contact_handles`, the `joint_graph` path, `convex_hull`
  borrows (25), `ContactConstraint` body indices in `contact_transfer.rs`,
  `headless_test/contact_account.rs`, `rubble_climb.rs`, `net/ram.rs`.
- `lightyear_avian3d` re-port: `rollback.rs` unpatched, and one patch in `plugin.rs` that removes
  the registration of its `update_child_collider_position` (prep file, section 2). It names no
  fork-only Avian item, so it does not wait for B2. The game registers
  `avian_ext::child_poses`' system in that slot, which does need B2's module.
- **Checkpoint (not green):** both vendored crates build standalone against the trunk's fork; no
  compiler error in owned files at the current frontier.
- **Price:** 1.5 to 2.5 bay-days (DERIVED). Bounded by the compile frontier. The export list is
  frozen, so B4 writes against `avian_ext` names before B2 delivers them.

### B5. Integration to green (bay 1, trunk; bays 2 and 3 fix forward on their lanes)

Order of merges into the trunk:

1. B2's fork replaces the pristine avian directory (it is already on the trunk).
2. `lane/stack-net`.
3. `lane/stack-render`.
4. The guard crate is deleted: the guards run as the game's own tests again.

Then, in order, each a gate:

1. `cargo fmt --all --check`; CI clippy
   (`scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`).
2. Targeted suites: the avian guards, track, net, construction, debris, render layout tests.
3. `scripts/render/verify-shader-compile.sh`.
4. The trunk is pushed (F2); the full suite runs on CI; `bitprobe.yml` is dispatched on the trunk
   and the pair is diffed.
5. Six lens reviewers over the whole diff, one per lens.

**Price:** 1 to 2 bay-days (DERIVED). Bounded by CI runs (the first is cache-cold, 28 min tests
lane MEASURED; expect several pushes) and by lens rounds over a large diff (up to six).

### glam 0.33 and determinism

Which checks compare bits today:

| Check | Compares | Path |
|---|---|---|
| Cross-target bitprobe pair | macOS aarch64 against Linux x86_64, same commit, 3,072 ticks at seven seams | `scripts/bitprobe/`, `.github/workflows/bitprobe.yml`, `src/bitprobe.rs` |
| Lockfile tripwire | that one glam holds the `scalar-math` pin; structural, no floats | `src/tests/determinism_deps.rs`, `deny.toml:60-61` |
| Scripted determinism run | two runs of one build | `src/headless_test.rs` (`scripted_determinism_run`) |
| Unit-level bit equalities | two code paths of one build | e.g. `track/rubble.rs` (`flat_ground_with_no_pieces_gives_bit_identical_track_forces`), `track/sim.rs` (`support_bits`), `track/forces.rs`, `ballistics/collect.rs`, `ballistics/motion.rs`, `firecontrol.rs`, `construction/fracture.rs`, `debris/freeze.rs`, `headless_test/frozen_rubble.rs` |

None of these stores bits from an earlier build (DERIVED from test names and the files' shape, not
from running them; the lane confirms when they run). So **no test claims "bits equal 0.19", and
none should be written.** glam 0.33 changes `Mat3` inverse and determinant, and `lerp`
(Bevy scout); parry 0.31 and obvhs 0.4 move too. Sim outputs will differ from 0.19 in the last
bits and then diverge.

What replaces the claim:

1. **Same build, two targets:** the bitprobe pair is `IDENTICAL` on the trunk. This is the
   determinism gate, as ADR-0028 requires for any math dependency change.
2. **Same build, two runs:** the scripted determinism run and the unit-level equalities are green.
3. **Across the upgrade, an observation:** `scripts/bitprobe/diff.py` of the 0.19 macOS dump
   against the 0.20 macOS dump, run once, reported as the first diverging seam and its growth. It
   is expected to exit 1 (or 2 if the dump schema moved). It gates nothing.
4. **Where a client derives state** (the destruction ledger, the presented shell march, ADR-0037):
   both peers run the same build, enforced by the protocol fingerprint, which changes with the
   avian, lightyear and replicon versions (`src/net/protocol.rs:76-82`).

## C. The behaviour pass (bay 1 on the green trunk, before the merge)

Re-run every baseline of A2 on the trunk and put before and after side by side in one report.
Emergent numbers that moved are observations for the owner, not regressions and not obligations.

**Expected differences, by cause** (Avian scout section 3 unless noted):

| Cause | What moves | Switch for attribution |
|---|---|---|
| Friction solved only in the relax pass (upstream #1036) | Every friction-borne result: belt hard contacts, the `belt_friction` and `creep_stall` probes, pile rest times, the scorecard's hard-contact lines | None without a fork line. Accepted |
| CCD rework (#996): speculative contacts opt-in, tight AABBs, swept CCD on by default against static and kinematic bodies | Fast dynamic-against-dynamic meetings (a tank into loose pieces, piece into piece); broad-phase pair counts | `SpeculativeCcd` per body (question F3) |
| Solver decoupling (#989): contact status and despawns queue, flushed in the solver | Anything reading `PhysicsIslands` or `ConstraintGraph` between a despawn and the next step sees older state than on 0.7 | `FlushContactStatusChangeQueue` at a named site, only where a reader needs it |
| Sleeping pairs (#1032) | Intended: none for non-touching pairs (refused). Touching pairs take upstream's transfer and `wake_entity_with` | The predicate of decision 4 |
| Contact recycling (#1033) | Intended: none (off). A nonzero `recycled_count` in the counters is a bug | `recycle_distance` |
| Tree optimisation as one compute task (#1046) | Timing on the two-thread hosted pool, not results | `use_compute_task` |
| Upstream's parallel floors (#1035) replace patch 18 | Timing only: small loops may go parallel or serial at different sizes | Measured in D2 |
| Patch 7 and patch 16 replaced by upstream's versions | Margin clamp: equal by reading. Moved list: a second entry is ignored, no panic | None |
| glam 0.33, parry 0.31, obvhs 0.4 (now under `scalar-math`) | Last-bit differences everywhere; tree shapes | None |
| `Tonemapping::None` skips the pass on both versions | The game's damage-model depth masks keep `None` | F5, settled |
| Weakly ordered render sets (Bevy scout) | A texture one frame late, if an upload lost its order | Explicit ordering in B3 |

**Agent-compared, in the report:** scorecard lines and probes; `pile_park` physics microseconds
per tick; collapse run; tick attribution top systems; cost harness; captures as image pairs with a
per-image difference figure; the bitprobe pair; a hosted end-to-end run on the trunk's server
image with the trunk's client (needs F8).

**Owner's sitting** (one session, about 60 to 90 minutes, DERIVED). The agent prepares the
builds, the capture pairs and a save of each spot beforehand.

1. Flip through the capture pairs: terrain, tank surface, construction, belts, VFX, surroundings.
   Anything that reads differently?
2. Windowed client on the product map: terrain shadow on hills and on the tank; no shimmer, no
   missing shadow.
3. Pen-cam after a hit (the game's damage-model view): picture brightness and colour as before.
4. Optic mask and render-scale upscale at two scales.
5. Drive on flat ground: pull away, each gear, brake to a stop, turn in place. Feel as before?
6. Drive over rubble and climb a pile; park on it and let go. Does the tank sit still?
7. Ram a house, drive through the fall. Any piece passing through the hull, or a jolt?
8. Park against a wall and against a second tank, braked. Any creep?
9. Watch a collapse settle. Do pieces come to rest, and stay at rest when a tank passes nearby?
10. Join the hosted trunk server from the trunk client; another tank's motion and a round's
    flight look as before.

A "no" on 1 to 4 goes back to bay 2; on 5 to 9 it is first attributed with the switches above.

**Price:** 1 to 2 bay-days plus the sitting (DERIVED). Bounded by the owner's eyes and by one
hosted deployment.

**Then:** the orchestrator merges the trunk to `main` once, and alpha.17 ships client and server
together (decision 5).

### Bounded estimate

| Stage | Bays | Bay-days (DERIVED) | Bound |
|---|---|---|---|
| A1 reading | 0 | none | agent reading |
| A2 baselines | 1 | 0.5 to 0.75 | run lengths |
| B1 skeleton | 1 | 0.4 to 0.6 | one read |
| B2 fork | 1 | 2 to 4 | review rounds, the row-5 check |
| B3 render | 1 | 1.5 to 2.5 | owner's eyes |
| B4 net and sim | 1 | 1.5 to 2.5 | compile frontier |
| B5 integration | 1, with fix-forwards on 2 | 1 to 2 | CI runs, lens rounds |
| C behaviour pass | 1 | 1 to 2 | owner sitting, hosted run |
| **Cutover total (B and C)** | 3 | **7.5 to 13.5** | |

With three bays the critical path is B1, B2, B5, C: about
**4.5 to 8.5 elapsed days** (DERIVED: 0.5 + 2 to 4 + 1 to 2 + 1 to 2, plus owner waits).
B3 and B4 fit inside B2's span.

## D. Follow-ups after the merge, in order

1. **#353, the belt as game-owned friction rows** (decision 6). Built from seam 2
   (`lane/stack-proto:proto/stack-seams/src/belt.rs`), with no fork line: the hook zeroes the
   manifold's friction, one substep system between `Relax` and `Damping` solves friction and hold
   **together as one 2x2 boxed problem** per manifold, and a game store holds the drive and the
   impulses (`src/contact_transfer.rs` reads them there). Deletes `HardReaction.belt`, the add in
   `feel_tick` and the forced zero in `belt_tick`. Re-measures the belt plan's list: the eight
   parked poses at both thresholds, `soft_run_probe`, `rubble_climb_probe`,
   `belt_friction_probe`, `creep_stall_probe`, `pile_park`. A manifold then names a drive and
   holds no per-step speed, which is the precondition for D2 on belt pairs.
   **Acceptance, all of** (the first three restate what the spike measured; the rest are what
   it did not cover):
   - slip never changes sign on any substep, for mass-to-inertia ratios from 0.2 to 12;
   - a hold above friction keeps the drive exactly at its anchor speed on every row; a hold
     below it lets the belt creep, and the body decelerates at B/(M+I) (hold capacity over body
     mass plus belt inertia), never B/M;
   - the impulse on the bodies equals the impulse on the drive, per substep;
   - *several manifolds on one drive* (a belt on two pieces and the ground): the rows share one
     hold budget and the three results above still hold. The spike compiled this path and never
     ran it;
   - *rotation unlocked*: with the body free to pitch, friction shifts the normal load, which
     the rows see one substep late since they run after Avian's rows. Slip still never reverses
     and a parked tank on a slope holds within its hold capacity. The spike locked rotation;
   - *the crank as anchor*: the anchor is a moving degree of freedom with its own inertia, not a
     fixed speed; the hold is against it and momentum is conserved over belt, crank and bodies;
   - *a direction per contact* from the belt's outline, not one direction per drive;
   - *Avian's `parallel` feature on*: the system is serial and takes `ResMut<SolverBodies>`; it
     is ordered against Avian's parallel solve, and the serial pass is priced on the hosted
     allocation's two cores under the `parallel-work` lens, with its named data dependency (one
     hold budget per drive);
   - the row's constraint list is built once per step, after
     `SolverSystems::PrepareContactConstraints`, by `contact_id` and `manifold_index`; the spike
     scanned every constraint each substep;
   - the system that writes the drives sits in a solver set, since the broad phase's hook
     parameter reads them and `PhysicsSchedule` refuses the ambiguity.
2. **Measure contact recycling and upstream's parallel floor** (decision 2), one change, one
   report. Three arms on the same build: recycling off (the shipped state), recycling on with our
   stationary-miss cache, recycling on with the cache deleted. Parallel floor: upstream's
   `MIN_PAR_ITER_ENTITIES = 4096` and the colour floor of 2 against the crossovers recorded in the
   0.7 patch doc ("Crossovers"), on the hosted allocation's two cores.
   **Acceptance, all of:**
   - *Hooks:* a parked tank whose belt starts turning has, on every tick from the first, a
     friction row using that tick's belt speed. Witness: the pair is forced to recycle (the hull
     held still) and the belt speed stepped; the row's surface speed equals the drive's speed on
     the same tick. After D1 this holds by construction; the test stays as the guard. Likewise the
     hook's `retain` and point-shaping rewrites: either the pair is exempt from recycling (a
     per-pair opt-out) or the witness shows they are unchanged by reuse.
   - *Contact reduction on a recycled pair:* MEASURED on stock 0.8.0 (seam 1): with recycling at
     0.05, reduction stopped after a disturbance (64 manifolds and 256 points 25 steps after a
     0.4 rad/s tip, where recycling off holds 4 and 4) unless the pass clears
     `ContactPair::recycling_cache` on the pairs it takes, which restored the baseline. The cause
     is DERIVED, not isolated by a run: a recycled pair returns before hooks and matching and
     refreshes only `penetration`, so the pass sees last step's reduced set with a stale
     `normal_speed` and a landing verdict stays for as long as the pair recycles. So the pass
     ships clearing the cache (B2), which means **reducible pairs never recycle**, and this
     follow-up measures whether any narrower exemption is worth having: over a 600-tick rest on
     a pile, the reduced point set of a recycled body pair must equal what reduction gives from
     a fresh query of the same pose, with
     `a_body_on_reduced_contacts_stays_supported_with_a_bounded_count` green and the disturbance
     case above restored. If it cannot, the cache-clearing stays and the report says so.
   - *Compliance:* no law persists on a manifold the soft run stopped owning.
   - *Rest:* the island-sleep guards are green, and rest time on the pile is no longer than with
     recycling off.
   - *Cost:* `pile_park` physics per tick falls by more than the run-to-run spread of its
     palindrome, and the collapse run's worst tick does not rise.
   - *Determinism:* the bitprobe pair is `IDENTICAL`.
   Adopt upstream's rule for each pair class that passes; delete the stationary-miss cache
   (patch 13 and its six tests) if the third arm passes everywhere. The same report settles the
   CCD opt-ins of F3 by measuring with and without them.
3. **Deletions the upgrade enables**, each its own small change:
   `platform::revoke_macos_activation` if a hidden capture run stays unfocused; the `MeshUniform`
   alignment patch if not settled in B3; the `deny.toml` RUSTSEC-2026-0344 ignore; the
   `bevy_reflect` fork if not settled in B1; the `obvhs` entries of `deny.toml` and
   `AUDITED_SPLIT_GLAM`; stale "same locked version" comments in `Cargo.toml`; the AGENTS.md
   lines naming Bevy 0.19 and avian3d 0.7; the vendored prunes that Bevy's per-column change
   ticks or replicon 0.45 make redundant, if the tick attribution of section C shows it.
4. **Decide the heightfield shadow's home** (F6): measure per-material sampling through the
   upstream `directional_shadow_factor` against the `bevy_pbr` fork.
5. **Candidates to offer upstream, as local drafts under `upstream/` only.** Nothing is filed,
   opened or commented anywhere; the owner sends them, if ever. In order of fit: the
   moved-collider bounds prune (upstream carries a TODO for it, `0.8:collider_tree/update.rs:913`);
   sleeping bodies' velocity increments (the two queries disagree,
   `0.8:dynamics/integrator/mod.rs:261-327`); writing only changed child poses; unknown contact
   features and nearest warm start, as two fixes; the counterexample to #1032's transfer of
   non-touching pairs, with our witness; a `should_sleep` that sees the edge; a pluggable rest
   test; a post-update contact slot; recycling skipping `modify_contacts` and the per-step
   `tangent_velocity` reset, if D2's witness shows it on stock code.

## E. Risks, tripwires, fallbacks

| Risk | Tripwire (stop when you see this) | Fallback |
|---|---|---|
| The reduction pass meets something the spike did not exercise (parallel groups, the `parallel` feature, the 18 guards) | A guard of `avian_contact_reduction.rs` is red on the fork for a reason inside Avian | Hook (b), a call slot in `NarrowPhase::update` after the pair update, ≈35 fork lines (DERIVED by the scout); plan unchanged otherwise |
| The wake rule cannot run as a game pass | The row-5 check: a wake witness is red with the pass on stock 0.8.0 | The hook with the transfer inside `apply_contact_status_change`, ≈30 to 45 fork lines |
| Game-owned belt rows fail with rotation unlocked, several manifolds or the crank anchor | A D1 acceptance item is red and the cause is the rows running after Avian's | One `Option<fn>` slot called from the friction block of `ContactConstraint::solve` (`0.8:dynamics/solver/contact/mod.rs:311-345`), so the row interleaves with Avian's; the belt plan's vendored surface drive (150 to 200 fork lines) only if that fails too |
| The seam modules do not build without the game crate | The guard crate needs more than a handful of stubs | Drop the guard crate; B2's checkpoint becomes "fork compiles standalone" and its guards run first at B5, which lengthens B5 |
| The trunk stays red too long | B5 has run 2 bay-days without CI clippy passing | Stop adding scope: land plain ports for the remaining seams and seam them after the merge |
| Three behaviour changes arrive together and cannot be told apart | The owner says "driving feels wrong" and the switches of section C do not isolate it | Opt every dynamic body into `SpeculativeCcd`; compare; if still wrong it is #1036 or numerics, and the merge waits for a named cause |
| A shader compiles and shades differently | A capture pair differs outside the expected set and no one can say why | That pipeline's port is redone against Bevy's own migrated shader of the same kind; the merge waits |
| The heightfield shadow re-port stalls | `bevy_pbr` does not build standalone after 1 bay-day | Ship the cutover with the fork's other items and the terrain shadow on the per-material path of D4, if the owner accepts the picture; else wait |
| Weakly ordered render sets | A track print or pen-cam texture is one frame late in a capture | Explicit `.before`/`.after`; a test that the upload system is ordered before its consumer |
| Cross-target bits differ | The bitprobe pair exits 1 on the trunk | Read the first diverging seam; check `cargo tree -i glam` for a second glam; no merge until `IDENTICAL` |
| `lightyear_avian3d` 0.31 names Avian types the fork changed | It fails to build against the fork, or panics at first boot | No fork field on `ColliderTree`, `ColliderTrees` or `MovedProxies` (the only field-by-field snapshot); fork state beside them goes in its own resource (prep file, section 2) |
| lightyear interpolation access panic (B0001) | First boot of client or server panics | Split the conflicting system's queries; fail-fast, so it cannot ship unseen |
| A panic is swallowed where an app installs a continuing error handler | `damage_cam/pen_cam/game.rs` keeps `bevy::ecs::error::ignore` after the flip | Remove or narrow it in B4 |
| alpha.16 needs a fix while all bays are on the cutover | A hosted or client defect is reported | Bay 2 parks its lane (committed) and is released for the fix; B3 resumes after |
| `main` drifts under the trunk | A doc or orchestrator commit conflicts at the merge | Merge `main` into the trunk daily inside bay 1 |
| Protocol fingerprint mismatch at release | A 0.16 client reaches a 0.17 server | Release client, server image and the matchmaking compatibility record in one step; the fingerprint refuses the mismatch and the player sees the cause |
| CI is cache-cold on every vendor rename | A fix-forward loop of 28-minute runs | Batch fixes per push on the trunk; one push per fix thread |

## F. Questions only the owner can answer

Each is phrased so that "yes" accepts the recommendation. Most consequential first.

1. **May the cutover hold all three bays for its span (about 4.5 to 8.5 days, DERIVED), with no other
   building lane, and bay 2 giving way if alpha.16 needs a fix?** Recommended: it is the only way
   three slices run at once, and a quiet `main` is what keeps the trunk from chasing it.
2. **May the orchestrator push the trunk branch (never `main`) to the origin, to run the full
   suite and the Linux bitprobe leg before the merge?** Recommended: the pair cannot be measured
   otherwise, and decision 5 wants `main` green.
3. **At the cutover, do all dynamic bodies opt into `SpeculativeCcd`, keeping 0.7's speculative
   contacts between dynamic bodies, with dropping it measured in D2?** Recommended: it follows
   decision 2's rule (change as little as possible, then adopt upstream by measurement). The cost
   is that tight AABBs give no gain until D2.
4. **Do the guards of dropped patches (4, 7, 16) stay, as tests of upstream behaviour we rely
   on?** Recommended: they are the tripwire on the next rebase. The one that asserted our panic
   in patch 16 is rewritten or deleted.
5. **Settled: the game's damage-model depth masks keep `Tonemapping::None`.** Both versions
   skip the pass; `Linear` would add one. See B3's source citation.
6. **Does the heightfield shadow stay a `bevy_pbr` fork through this cutover, with its move to
   per-material sampling measured afterwards (D4)?** Recommended: one more variable in the render
   pass is not worth it now. Still open: the fork is the costliest vendored item at every Bevy
   bump.
7. **Does `lane/soft-run` merge to `main` on 0.7 before the cutover starts, so the fork port has a
   fixed base?** Recommended: the alternative is porting its patch from a lane diff.
8. **May the behaviour pass spend one hosted deployment for the end-to-end check?**
   Recommended.
9. **Is the silent `false` of upstream's moved list (in place of our panic on a second entry)
   acceptable, named in the patch doc?** Recommended: keeping the panic re-opens a file upstream
   rewrote.

## G. Draft GitHub issues (not created)

Labels follow `.agents/docs/triage-labels.md`: a task carries one category and one state label; the
epic carries `epic` only. Blocked-by edges are native dependencies. Titles and bodies carry no
local path outside the repo, no account data, no session or agent id.

### Epic: Move the stack to Bevy 0.20, lightyear 0.31 and a smaller fork of Avian 0.8

Labels: `epic`

> **Destination.** The game runs on Bevy 0.20.1, lightyear 0.31.0 and a fork of avian3d 0.8.0
> that holds about 350 to 400 lines inside upstream's files, down from 1,905 (MEASURED on 0.7; the
> target is DERIVED). It ships as alpha.17, client and server together, because the protocol fingerprint
> changes. alpha.16 ships on the current stack first.
>
> **Decisions so far** (one line each; detail in the linked task)
> - The upgrade also shrinks the fork: each patch becomes a hook with its logic in the game, a
>   self-contained fork module, or states why it cannot. (#T6)
> - Contact recycling starts off and our stationary-miss cache stays; recycling is measured
>   afterwards as its own change. (#T12)
> - The island wake rule is the game's: a pass of its own if that passes its witnesses, else a hook in the fork. (#T6)
> - Non-touching pairs do not sleep with their island. (#T6)
> - One branch, merged once; `main` stays green. (#T9)
> - The belt fix (#353) follows the upgrade as its own change, as friction rows the game owns. (#353)
> - Contact reduction and belt friction both run on unpatched Avian 0.8.0 (prototyped). (#T2, #T3)
>
> - The game's damage-model mask tonemapping is settled: `None` skips the pass on both versions.
>
> **Not yet specified**
> - Whether dynamic bodies opt into speculative contacts at the cutover.
> - Whether the heightfield shadow stays a `bevy_pbr` fork.
>
> **Out of scope.** Bevy 0.21. Adopting new Bevy features (schedule shuffle seeds, weak chains,
> font fallback). Sending anything upstream.
>
> Plan: `.agents/scratch/stack-upgrade-plan-2026-11-15.md`.

### T1. Record the "before" set on the current stack

Labels: `enhancement`, `ready-for-agent`. Blocked by: none.

> Record once, at the alpha.16 commit, everything the upgrade's before-and-after comparison needs:
> the cross-target bitprobe pair (`scripts/bitprobe/README.md`), `scripts/track-scorecard` and
> the four track probes, `scripts/perf/pile_park.py`, the scheduled collapse
> (`scripts/perf/collapse.sh`), tick attribution by Bevy's chrome trace for the empty-map and
> combat phases, the cost harness (`scripts/cost/run.sh`), `scripts/render/verify-shader-compile.sh`
> and a capture of every custom-shader pipeline (`scripts/capture/`).
>
> **Done when**
> - each output is stored with its commit hash and exact command line;
> - every one of the 17 files in `assets/shaders/` appears in at least one capture, with new
>   capture plans added where none existed;
> - the bitprobe pair reads `IDENTICAL`.

### T2. Prototype: contact reduction as a game-side pass on stock Avian 0.8.0

Labels: `enhancement`, `ready-for-agent`. Blocked by: none. Closed on creation, as the record of
the result.

> Question: can body-pair contact reduction run as a game system in `NarrowPhaseSystems::Last`
> with no change to Avian?
>
> **Answer: yes, with no line of Avian changed.** Built as a standalone crate with its own
> `[workspace]` at `proto/stack-seams/` on a branch that is never merged, against unpatched
> avian3d 0.8.0 with its `parallel` feature off.
>
> **What the pass does.** It takes the manifolds of each touching, constraint-generating,
> hook-free pair between two marked bodies, reduces per body pair, and puts the kept ones back.
> It folds its change of manifold count into the status entry Avian already queued for the
> pair, removes the entry at zero, and pushes a fresh one only where none was queued. A second
> entry for the same pair trips a debug assertion in Avian's constraint graph. No visibility
> change in Avian is needed, and the fork's reducible flag, status-site tolerances and island
> walk have nothing to replace them.
>
> **Measured**
> - after the solver's flush, the constraint graph's manifold count equals the pair's for every
>   touching pair, each step of a 600-step run under four reduction rules;
> - ports of `a_body_on_reduced_contacts_stays_supported_with_a_bounded_count`,
>   `a_pair_reduced_to_nothing_leaves_and_rejoins_its_island` and
>   `an_island_split_keeps_pairs_reduced_to_nothing` are green;
> - kept points enter the solver with last step's impulses;
> - with contact recycling on, reduction stopped after a disturbance unless the pass clears the
>   recycling cache of the pairs it takes.
>
> **Not covered, carried to the fork task as acceptance items:** the other 18 reduction guards;
> the landing speed per body from its sleep threshold; parallel groups; Avian's `parallel`
> feature.

### T3. Prototype: belt friction as game-owned rows in the substep on stock Avian 0.8.0

Labels: `enhancement`, `ready-for-agent`. Blocked by: none. Closed on creation, as the record of
the result.

> Question for #353: can the belt's friction be solved by the game between
> `SubstepSolverSystems::Relax` and `Damping`, with the manifold's own friction set to zero, and
> meet the invariant "friction never reverses the slip it opposes"?
>
> **Answer: yes, with no line of Avian changed.** Same standalone crate as T2.
>
> **What the rows do.** Friction and the drive's hold are solved together, as one two-by-two
> problem with bounds per manifold. Solved one after the other they under-deliver stick friction
> on a held belt by a factor of one plus the ratio of effective mass to belt inertia. When the
> hold holds, the drive is assigned its anchor speed, so rest is exact.
>
> **Measured** (a box sliding at `u` on a slab; a drive with speed 0, inverse inertia `1/I`,
> hold capacity `B`; body mass `M`)
> - with friction below the hold: drive speed is exactly 0 on every row;
> - with friction above it: drive speed stays in `[0, u]`, slip never changes sign on any
>   substep, and the box decelerates at `B/(M+I)`, since the hold slows belt and box together;
> - the impulse on the bodies equals the impulse on the drive, per substep;
> - with effective mass ten times the drive's inertia and friction four times the hold, the
>   belt settles without alternating, where today's one-tick-late scheme alternates for good;
> - slip never reverses for mass-to-inertia ratios from 0.2 to 12.
>
> **Not covered, carried to #353 as acceptance items:** several manifolds on one drive;
> rotation unlocked; the crank as a moving anchor; a direction per contact; Avian's `parallel`
> feature.

### T4. Settle the fork's base on `main` before the cutover

Labels: `enhancement`, `ready-for-human`. Blocked by: none.

> The Avian port needs a fixed base. Land or park the soft-run lane; if it parks, add the
> drafted `ContactCompliance` section to the fork's `OVERMATCH_PATCH.md`; tag alpha.16. The list
> of fork-only items the game calls, and the name each has on 0.8, is written.
>
> **Done when** no lane with fork edits is open and the patch doc has one section per patch on
> `main`.

### T5. Cutover: manifest flip and mechanical sweep

Labels: `enhancement`, `ready-for-agent`. Blocked by: T1, T4.

> On the cutover branch: bump the manifest to the target set, replace the five vendored
> directories with pristine copies of the new versions (dropping `bevy_reflect` if glam's scalar
> bool vectors now have serde), move the glam pin, `deny.toml` bounds and the lockfile tripwire,
> and apply the scripted renames (observer syntax, `MeshTag::new`, `get_aabb`,
> `set_if_different`, pointer events, `extract_app`).
>
> **Done when** the lock resolves with one glam, `cargo deny check bans` passes, and no old-form
> observer remains. The tree does not compile yet; that is expected.

### T6. Cutover: the Avian 0.8 fork, each patch in its target form, with its guards

Labels: `enhancement`, `ready-for-agent`. Blocked by: T5.

> Re-create the fork on avian3d 0.8.0. Each patch takes the form in the plan's table: a hook with
> its logic in the game, a self-contained fork module, game-side with no fork line, kept as a
> patch with its reason, or dropped. Includes: contact reduction as a game-side pass that folds
> its count change into Avian's queued status entry; the wake rule in the game (a pass if its
> witnesses pass on unpatched code, else a hook that also moves the pair to the sleeping set);
> the refusal of sleeping non-touching pairs with their island, as a sleep predicate that sees
> the contact edge; contact recycling off by config with the stationary-miss cache kept as a
> fork module; child-collider placement owned by the game, with Avian's and lightyear's own
> passes unregistered; upstream's fix #1101.
>
> **Done when**
> - every `avian_*` guard and the six `stationary_miss_*` tests are green against the fork alone;
> - each dropped patch's witness is green on unpatched code;
> - for contact reduction, beyond the three guards the prototype ran: all 21 guards are green;
>   the landing speed is read per body from its sleep threshold; body-pair groups reduce in
>   parallel with their effects committed in pair order; everything holds with Avian's
>   `parallel` feature on; the pass clears the recycling cache of the pairs it takes;
> - the fork adds no field to the collider trees or the moved-proxy list;
> - the patch doc has one section per patch naming its form and its witness;
> - lines inside upstream's files are counted and reported against the target of about 350 to
>   400.

### T7. Cutover: render, the vendored `bevy_pbr` and 17 shaders to WESL

Labels: `enhancement`, `ready-for-agent`. Blocked by: T5.

> Re-port the `bevy_pbr` fork on 0.20.1 (heightfield shadow in WESL, mesh-instance removal
> batching; drop what upstream now has), port every file in `assets/shaders/` to WESL, and
> migrate the render-side Rust (depth-stencil attachment, window extraction, shader buffers,
> wgpu 30, explicit ordering for uploads through the render queue).
>
> **Done when** the vendored crate builds on its own, no shader uses the old preprocessor, and
> the owned files have no compiler error. The pipelines are proven at T9.

### T8. Cutover: lightyear 0.31, the vendored net crates and the sim's Rust migration

Labels: `enhancement`, `ready-for-agent`. Blocked by: T5.

> Re-apply the netcode admission patch on 0.31.0; re-port `lightyear_avian3d` with one patch
> (its child-collider pass is not registered; the game's runs in that slot); migrate to
> lightyear 0.31 and replicon 0.45; write the six `par_iter_many` sites as decided (five are
> bugs with an `expect`, one is a named absence; never a silent skip); in the server's plugin
> floor rename the text-input plugin and disable the four widget plugins Bevy 0.20 adds; move
> the game's reads of solver bodies, constraint handles and contact constraints to the 0.8
> shapes.
>
> **Done when** both vendored crates build against the fork and the owned files have no compiler
> error.

### T9. Cutover: integrate to green and prove determinism

Labels: `enhancement`, `ready-for-agent`. Blocked by: T6, T7, T8.

> Merge the three slices on the cutover branch and bring it to green: format check, CI clippy,
> targeted suites, shader pipeline compile on a real device, the full suite on CI, the
> cross-target bitprobe pair, six lens reviews over the whole diff.
>
> No test claims equality with the old stack's bits: glam 0.33 and the physics crates change
> arithmetic. Determinism is shown by the same build on two targets (`IDENTICAL`) and by the
> same build run twice. The old-against-new dump difference is reported once, as an observation.
>
> **Done when** all of the above pass on the branch.

### T10. Behaviour pass: compare before and after, and the owner's sitting

Labels: `enhancement`, `ready-for-human`. Blocked by: T9.

> Re-run the "before" set on the cutover branch and report both sides. Expected differences, by
> cause: friction solved only in the relax pass; the CCD rework; queued contact status; glam,
> parry and obvhs numerics; upstream's parallel floors. Recycling is off, so a nonzero recycled
> count is a bug.
>
> The owner judges, in one sitting: the capture pairs; terrain shadow; the pen-cam picture; optic
> mask and upscale; driving on flat ground and on rubble; parking on a pile; ramming a house;
> braked against a wall and against a tank; a collapse coming to rest; one hosted round trip.
>
> **Done when** the report is written and the owner has accepted or named each difference.

### T11. Merge the cutover and release alpha.17, client and server together

Labels: `enhancement`, `ready-for-human`. Blocked by: T10.

> One merge to `main`. The protocol fingerprint changes, so the client, the hosted server image
> and the matchmaking compatibility record ship in one step.

### #353 (existing): add blocked-by T11

> Edit the body's "Fix" paragraph: the belt's friction is rows the game owns, with no line of
> Avian changed (prototyped, T3). A contact hook zeroes the manifold's friction; one system in
> the substep, after Avian's relax pass, solves friction and the drive's hold together as one
> two-by-two bounded problem per manifold; the drive and the row's impulses live in a game
> store.
>
> **Done when** (the first three were measured by the prototype on a locked-rotation box with
> one manifold)
> - slip never changes sign on any substep, for mass-to-inertia ratios from 0.2 to 12;
> - a hold above friction keeps the drive exactly at its anchor speed; a hold below it lets the
>   belt creep and the body decelerates at hold capacity over body mass plus belt inertia;
> - the impulse on the bodies equals the impulse on the drive, per substep;
> - the same holds with several manifolds sharing one drive's hold budget;
> - the same holds with rotation unlocked, where friction shifts the normal load;
> - the anchor is the crank, a moving degree of freedom with its own inertia;
> - each contact takes its direction from the belt's outline;
> - it holds with Avian's `parallel` feature on, and the serial pass is priced on two cores;
> - the parked poses, the track probes and the parked-on-a-pile cost are re-measured and
>   reported.

### T12. Measure contact recycling and upstream's parallel floor; adopt what passes

Labels: `enhancement`, `ready-for-agent`. Blocked by: T11, #353.

> Three arms on one build: recycling off; on with our stationary-miss cache; on with the cache
> deleted. Also upstream's parallel floors against our recorded crossovers on two cores, and the
> speculative-contact opt-ins with and without.
>
> **Accept recycling for a pair class when all hold**
> - a contact hook's per-step writes reach the solver on the same tick for a recycled pair: a
>   belt that starts turning under a still hull has that tick's belt speed in its friction row;
> - a recycled body pair's reduced contact set equals the one a fresh query gives, a body on
>   reduced contacts stays supported with a bounded count, and reduction resumes after a
>   disturbance. Measured on unpatched Avian 0.8.0 with recycling on: reduction stopped after a
>   disturbance unless the reduction pass cleared the recycling cache of the pairs it took. The
>   pass ships clearing it, so reducible pairs do not recycle; this task keeps that or shows a
>   narrower exemption that passes;
> - no compliance law persists on a manifold its owner left;
> - the island-sleep guards are green and rest on a pile takes no longer;
> - parked-on-a-pile physics time per tick falls by more than run-to-run spread, and the worst
>   tick of a collapse does not rise;
> - the cross-target bitprobe pair is `IDENTICAL`.
>
> Delete the stationary-miss cache if the third arm passes everywhere.

### T13. Deletions the upgrade enables

Labels: `enhancement`, `ready-for-agent`. Blocked by: T11.

> Each its own small change, each with its check: the macOS activation revoke; the `MeshUniform`
> alignment patch; the RUSTSEC-2026-0344 ignore; the `bevy_reflect` fork; the audited second
> glam; comments and rules that name Bevy 0.19 or avian3d 0.7; vendored prunes made redundant by
> per-column change ticks or replicon 0.45, where the tick attribution shows it.

### T14. Decide the heightfield shadow's home

Labels: `enhancement`, `ready-for-human`. Blocked by: T11.

> Measure per-material sampling through the upstream directional shadow factor against the
> `bevy_pbr` fork: picture and frame cost. The owner chooses.

### T15. Draft the upstream candidates, locally

Labels: `enhancement`, `ready-for-human`. Blocked by: T11.

> Write drafts under `upstream/` for the fork items that read as fixes or small extension points.
> Nothing is filed, opened or commented in any external repository; the owner sends them, if ever.

Note for whoever creates these: the epic's "decisions so far" cites tasks by this draft's
numbering (#T2, #T3, #T6, #T9, #T12); replace each with the real issue number at creation. T2
and T3 are records of finished prototypes: create them closed, or fold their text into T6 and
#353 and renumber.
