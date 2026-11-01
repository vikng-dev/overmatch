# Server tick inspection, 2026-11-01 (#250)

Subject: the game's dedicated server and its fixed tick, game state only. A knockout empties a
tank's HP pool. Labels: MEASURED (a stats line, a trace span, a test), DERIVED (arithmetic on
labelled numbers), ASSUMED.

The Edgegap symptoms this answers are in #250. Local numbers are from an Apple M4 and do not
transfer as times; per-system shares and before/after deltas do. Every Edgegap time below is
DERIVED from a local one with the 3.7x host ratio of `../perf-2026-10-31/README.md` ("alpha.4
differs from alpha.3"), which is itself DERIVED.

## TLDR

1. **Top costs per phase**, before this lane's fixes (MEASURED, bevy `trace_chrome`, per fixed
   tick, M4; share of the fixed schedules' busy time, `FixedFirst` through `FixedLast`):

   | Phase | Fixed busy | 1st | 2nd | 3rd | 4th |
   |---|---|---|---|---|---|
   | Empty map (server alone, 13 066 entities, no tank) | 1 619 us | `ballistics::motion` history + child placement, 718 us (44 %) | avian step (`PhysicsSchedule`), 364 us (22 %) | avian collider-transform propagation, 265 us (16 %) | `net::diagnostics::fixed_nan_probe`, 71 us (4 %) |
   | Combat (29 bots + 1 parked client, 18 250 entities) | 6 819 us | `track::sim::apply_track_forces`, 3 601 us (53 %) | avian step, 1 307 us (19 %) | `ballistics::motion` history + child placement, 780 us (11 %) | avian collider-transform propagation, 384 us (6 %) |

   Outside the tick, every 4 ms frame (3.33 per tick) adds 1 149 us (empty) and 1 469 us (combat)
   per tick of transform propagation, visibility, receive and send (MEASURED, same traces). On one
   vCPU those frames share the core with the tick.
2. **What serialises the tick: everything, and the drive step most.** In combat the fixed
   schedule's wall time equals its busy time (6 841 against 6 819 us, MEASURED, traced) on a
   machine whose compute pool has 10 threads, and the untraced server holds 59.5 % of one core at
   tick p50 5.2 ms (MEASURED). `apply_track_forces` ran its tanks with `par_iter_mut` in
   `v0.7.0-alpha.5`; "driving: carry tanks over pieces through the belt footprint (#238)"
   replaced that with one serial loop over the tanks and a shared reaction ledger. It is 53 % of
   the combat tick's work. That alpha.5's second core ran this share is ASSUMED: no alpha.5
   profile or measured bisect was taken. The owner directs optimization on one vCPU first.
3. **The empty map's tick goes mostly to the collider history** that "ballistics: retain composed
   collider poses between walks (#191)" added after alpha.5: two hash maps over every collider and
   every body rebuilt each tick, plus the per-collider pass of `seed_births`. Fixed: the tables
   are kept and only what moved is rewritten (fix 1). The NaN probe's full collider pass is fix 2.
4. **The rebuild's leaked pairs and the combat's pair growth have one cause:** a collider despawned off a body leaves
   its broad-phase proxy behind in Avian's standalone tree, and that proxy pairs with every collider
   its bounds overlap, for the rest of the process. The rebuild orphans the whole map's proxies
   (46 087 contact pairs where a fresh map holds none); a combat orphans every despawned part's.
   Fixed in the vendored Avian (fix 3): 0 pairs after the rebuild, fill-up physics 594 to 232 us
   (MEASURED). This attributes pair growth, not every tick-cost increase or entity growth;
   the unattributed costs remain listed below.

The TLDR's `ballistics::motion` aggregate includes child placement. The history-only sum is
585 us before (DERIVED: 432 + 153), 36 % of the empty tick, and 68 us after (DERIVED: 46 + 22).
The child-placement rows below are separate costs, never added to that aggregate a second time.

## Method

- **Build:** `scripts/serial-cargo build --locked --profile perf --features
  dev_ui,bitprobe,bevy/trace_chrome` with `CARGO_PROFILE_PERF_STRIP=false`, from main at "assets:
  name every jittered muzzle layer and the light rule's source (#245)" (before) and this lane's
  fixes (after). The `perf` profile is release codegen without LTO (`Cargo.toml`), so a time here
  compares with other `perf` times only.
- **Per-system lever:** Bevy 0.19's `trace_chrome` feature (bevy 0.19.1 `Cargo.toml`), analysed with
  `../perf-2026-10-31/tools/trace_profile.py` (its docstring defines a tick period, exclusive time
  and busy time). Untraced runs filter the spans off at their callsites
  (`RUST_LOG=bevy_ecs=warn,bevy_app=warn`). macOS `sample` ran inside the cycle run.
- **Scene:** the dedicated server on 127.0.0.1, `OVERMATCH_BOT=29`, `OVERMATCH_SERVER_STATS_S=1`,
  shipped Battle mode; for combat one headless parked harness client (`drive: Idle`), as
  `perf-1101/tools/cycle-run.sh` joins. Every run held the machine lock; no compile overlapped one.
- **The world rebuild** needs a whole Battle (16.5 min shipped). The cycle run used a measurement
  build whose `assets/modes/battle.ron` was edited to fill-up 20 s, combat 150 s, results 10 s and
  restored after the build (never committed); the mode data is not part of the protocol id
  (`net::protocol`), so the shipped client joins it.
- **Core-type noise:** the M4 mixes performance and efficiency cores, and a server at 40-60 % of
  one core migrates between them. The per-second tick p50 of one empty-map run spans 974-2 122 us
  (p10-p90, MEASURED). Before/after pairs are therefore run back to back in one lock hold, A B B A.

## Fixes 1 and 2: the collider history and the NaN probe

"ballistics: keep the collider history, rewrite only what moved (#250)": `ColliderMotion` keeps
its tables between ticks and rewrites the entries whose pose was written; `Remove` observers drop
the entries of removed colliders and bodies. "net: check a part's pose in the NaN probe only after
a write (#250)": the probe reads the parts whose pose was written since its last run.

Empty map, traced, per tick (MEASURED, M4; mean of a system's runs times its runs per tick):

| System | Before | After | |
|---|---|---|---|
| `motion::snapshot` | 432 us | 46 us | -89 % |
| `motion::seed_births` | 153 us | 22 us | -86 % |
| `fixed_nan_probe` | 71 us | 31 us | -56 % |
| *unchanged:* `motion::place_children` | 125 us | 278 us | x2.2 |
| *unchanged:* avian `update_child_collider_position` | 134 us | 274 us | x2.0 |

The after run's machine was slower: systems this lane did not touch cost 1.5-2.2x more in it
(rows 4-5, MEASURED), so the three reductions are lower bounds. The untraced A B B A runs (tick
p50 1 861 / 1 821 / 1 326 / 3 849 us, main / fixed / fixed / main, MEASURED) are too noisy to
read as a tick delta; the traced per-system table is the evidence. The history-only sum was 585 us
of a 1 619 us fixed tick (36 %) and is 68 us after (DERIVED: the rows above), so on Edgegap the
empty map's tick should fall by about 1.9 ms (DERIVED with the 3.7x ratio, so a rough figure).

Tests: `motion::tests::the_kept_history_equals_a_full_recompute_at_every_boundary` (the kept
tables equal a full recompute after births, moves, unwritten ticks, local-pose edits, despawns,
re-births and same-value writes) and
`diagnostics::tests::a_part_written_corrupt_after_its_first_check_still_stops`.

## The fill-up's world rebuild

Cycle run, short Battle, census system each 10 s (MEASURED):

| | Fill-up 1 | Fill-up 2 (after the rebuild) |
|---|---|---|
| physics p50 | 348 us | 594 us (+71 %) |
| contact pairs (active) | 0 | 46 087 |
| rigid bodies | 710 static | 710 static |
| entities | 13 069 | 13 075 |

A map of static bodies alone holds no contact pair (Avian's broad phase skips static against
static), so the 46 087 pairs involve standalone proxies: colliders with no body. There are none
on the map; they are the old map's despawned colliders.

The cause is in Avian 0.7.0, `collider_tree/update.rs`, the observer on `Remove<ColliderOf>`
("Case 3" in its comments): it moves the collider's proxy from its body's tree to the standalone
tree but never writes the new key back to the collider's `ColliderTreeProxyKey`. When the
collider is being despawned, `Remove<C>` ("Case 2") then removes the old key's id from the old
tree, which no longer holds it, and the standalone proxy is never removed. Its collider entity is
gone; its bounds stay where the collider was, and the rebuilt map's colliders sit exactly there.
Which observer runs first depends on component registration order, so a despawn leaks or not by
that order; the game's order leaks (the numbers above).

Fix 3: "physics: write the standalone proxy key back when a collider leaves its body (#250)",
an `OVERMATCH PATCH` in `vendor/avian3d-0.7.0-child-pose` (`OVERMATCH_PATCH.md`). The same cycle
run before and after it (MEASURED, M4, census each 10 s, physics p50 per tick):

| | Before fix 3 | After fix 3 |
|---|---|---|
| Fill-up 1 physics | 348 us | 298 us |
| Fill-up 2 physics (after the rebuild) | 594 us | 232 us |
| Fill-up 2 contact pairs | 46 087 | 0 |
| Combat 1 peak pairs | 2 442 | 425 |
| Results 1 pairs (one dynamic body) | 333 | 1 |
| Combat 2 first pairs | 46 885 | 150 |
| Combat 2 physics | 949 us | 661 us |
| Combat 2 tick p50 | 5 247 us | 4 821 us |

Guard: `src/tests/avian_collider_of_removal.rs`; both of its tests fail with the write-back removed
(run).

RSS on macOS is not comparable across the rebuild (memory compression: 438 MB then 260 MB,
MEASURED), so the +80 MB on Edgegap is not reproduced here; 46 087 leaked pairs and their
manifolds are the likely share of it (ASSUMED, not measured).

## What grows through a combat

Before fix 3 (MEASURED): contact pairs climb from 150 at combat start (30 tanks on the ground) to
2 442 after 145 s with 30-31 dynamic bodies throughout; 333 remain at Results with one dynamic
body; Battle 2's combat starts at 46 885 and climbs to 48 787 in 60 s. Physics p50 goes 358 to
821 us over Combat 1. Entities grow 18 229 to 18 272 (+43), so the pairs, not the entities, track
the physics growth: every part with a collider despawned off a body leaked a standalone proxy.
After fix 3 the pairs stay between 145 and 425 and fall to 1 at Results (table above).

## Remainder after server-perf-1

- **The serial drive step.** `apply_track_forces` is 53 % of the combat tick (3.6 ms of 6.8,
  MEASURED traced; about 3.7 ms per tick in `sample`, MEASURED untraced), most of it the belt's
  ground and rubble queries (`belt_tick` into `depth_along_lowered_ground`, `terrain_grid` ray
  casts and heights, `rubble::in_slab`). Re-parallelising it buys nothing on one vCPU; cutting the
  per-tank cost is the fix, in #238's code. The owner has since directed one-vCPU optimization;
  server-perf-2's per-tank changes are recorded below.
- **Three passes place every child collider each tick.** `motion::place_children`, Avian's
  `update_child_collider_position` and lightyear_avian's `update_child_collider_position` together
  cost 611-1 077 us per tick on the empty map (MEASURED, the before and after traces), each iterating every
  child collider, though no map collider moves. Not investigated further in this lane.
- **Frames outside the tick** (1.1-1.5 ms per tick, `mark_dirty_trees`, transform propagation,
  visibility) are the dedicated server running Bevy's transform and visibility plugins over a map
  that does not move. Not investigated further.
- **Unexplained in the first inspection:** the Edgegap +840 entities through a combat (locally +43 in 150 s; a full
  16.5 min Battle was not run). Also the short-mode cycle runs' Combat 1 tick p50 of 8.6 ms
  against 4.8-5.2 ms in their Combat 2 and 5.2 ms in the shipped-mode combat run (MEASURED, with
  and without `sample` alike), with the same tanks and entities.

## server-perf-2: verification of the previous lane's doubles

Subject: the game's damage model and dedicated server tick, game state only. Each review was
checked against the current source. The binding lane rules prohibit pricing runs even when the
brief requests them; this lane defers new tick and per-system timings to the orchestrator's
batched pricing lane. Existing MEASURED local timings above remain the baseline, not a claimed
after measurement for the changes below.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Codex | Enabling a stationary collider can leave its history absent | Verified: removing a disabling marker does not advance any watched pose tick; read disabling-component removals at birth and snapshot boundaries | ballistics: retain history when colliders are re-enabled (#250) |
| Codex | `Changed` filters still scan the world's matching entities | Verified; retained tables reduce composition and writes, not the change-tick scan | ballistics: retain history when colliders are re-enabled (#250) |
| Claude | Production placement-before-snapshot edge lacks a regression guard | Verified test gap; exercise `mount_collider_motion` itself across solver moves | ballistics: retain history when colliders are re-enabled (#250) |
| Claude | Composition test no longer rejects stale child Position | Verified test gap; invoke snapshot without a preceding placement pass | ballistics: retain history when colliders are re-enabled (#250) |
| Claude | No body-removal coverage of the retained body table | Verified test gap; remove the body in the full-recompute comparison | ballistics: retain history when colliders are re-enabled (#250) |
| Claude | Missing drained entries are legitimate but unnamed | Verified; removal observers can already have erased a last-interval mover | ballistics: retain history when colliders are re-enabled (#250) |
| Claude | Cost claims and frozen-membership claim overstate the invariant | Verified; clarify scans and immediate removal observers in module docs | ballistics: retain history when colliders are re-enabled (#250) |
| Claude | TLDR aggregate and history-only sums use one name | Verified; distinguish history + placement from history-only, correct rounded-row arithmetic | ballistics: retain history when colliders are re-enabled (#250) |
| Claude | Alpha.5 second-core attribution lacks a measurement | Verified; mark the attribution ASSUMED rather than measured | ballistics: retain history when colliders are re-enabled (#250) |
| Claude | Pair-leak attribution does not explain all tick/entity growth | Verified; limit the claim to pair growth and retain the open questions | ballistics: retain history when colliders are re-enabled (#250) |

The proxy-key write-back itself is sound: either observer order removes the proxy from its
current tree. No second patch to that fix is warranted. The existing Avian removal guards
remain the invariant tests.

## server-perf-2: each tank reuses its tick-start support candidates

"driving: reuse each tank's tick-start support candidates (#250)" retains the per-tank collider
lists already returned by `RubbleQuery::near`. The union still constructs one immutable shape and
body snapshot for the whole drive step. `RubbleBodies::select_source` selects only that source's
list, excludes its own body once, and sorts snapshot indices to preserve the previous gather
order. `in_slab` no longer scans surfaces gathered solely for another tank, and an empty local
list returns before building a slab AABB. Contact-to-surface lookups use the snapshot's collider
index rather than a linear search of the union. The reaction pass still has every recipient.

Evidence: `each_source_reuses_only_its_own_nearby_surfaces_without_changing_support` compares the
exact intersections before and after selection, verifies gather order and self exclusion, and
reads an unselected recipient from the shared snapshot. Candidate checks per fixture slab are
2 for the global list, 1 for the local list, and 0 for an empty local list (MEASURED, test). These
are operation counts, not tick timings or a production speedup claim.

Gates: formatting and CI clippy passed; 229 driving, headless and citation tests passed, with
3 existing ignored tests (MEASURED test-harness counts). No driving golden or tolerance changed.

## server-perf-2: share identical neighbouring terrain pin probes

"driving: reuse bit-identical terrain probes at shared belt pins (#250)" remembers the previous
link's ending probe for each lateral column during one contact-side pass. A start probe reuses
that result only when point, outward direction and reach match in every float bit, including
signed zero. The oracle is pure and immutable through the pass, so this preserves the depth
bits; midpoint and end casts, force order, material identity, grip and the transmission law stay
the same. Nothing is reused across ticks or across different probe inputs.

`shared_pin_probes_are_cast_once_without_changing_any_depth_bits` runs the actual contact pass,
checks every recorded collocation depth against a fresh oracle evaluation, and counts its casts:

| Fixture phase (fixture data) | Uncached requests (DERIVED from recorded collocations) | Actual casts (MEASURED, test) | Reduction (DERIVED) |
|---|---|---|---|
| 0 m | 225 | 171 | 24.0 % |
| 0.13 m | 225 | 174 | 22.7 % |

The baseline drive cost remains 3 601 us per combat tick (MEASURED local, previous lane's trace).
No after time was taken under this lane's binding rules; the orchestrator must price this fix and
the candidate-list fix together on the landed tree. Formatting, CI clippy and the same 229
passing driving/headless/citation tests cover this change. No result shift was observed and no
golden, physical parameter, command hold or grip shear modulus was changed.

## server-perf-2: one child placement law, changed inputs only

"physics: share changed-input child placement across tick boundaries (#250)" exports Avian's
`ChildColliderPoses` parameter and uses it in the pre-physics pass, the game's post-physics
`motion::place_children`, and lightyear's frame pass. The boundaries remain distinct because a
body can move during the solver and a replica can receive a new pose between physics steps.
Each boundary now composes only changed child inputs and the attached colliders of changed
bodies. An unchanged map collider costs change-tick scans, with no pose composition or write.
The local transform, attachment, own pose, body kind and collider membership all participate;
re-enabling a default-filtered entity refreshes membership. Every float-bit write uses the same
routine and pose expressions.

`physics_and_replication_share_the_changed_input_placement_law` plants a stale child pose
without advancing a change tick and observes that both passes leave it untouched; moving the
body then carries the child. The production-mount history test covers the post-physics pass.
The manual-attachment guard exercises `ColliderOf` without Transform children; its body must
have the `GlobalTransform` Avian's insertion hook reads to construct `RigidBodyColliders`.
The synthetic history fixtures now satisfy that same attachment contract.

The manual-attachment fixture follows its body's 1 m Z move (fixture data, MEASURED test);
the previous Avian/lightyear `With<Children>` filter skipped that valid attachment. Ordinary
hierarchies retain the same pose arithmetic. No gameplay golden or tolerance changed.

Gates: formatting, CI clippy and 44 collider/history/headless/citation tests passed, with
3 existing ignored tests (MEASURED test-harness counts). The existing proxy-removal tests pass.

| System | Before (MEASURED local, previous lane) | After |
|---|---|---|
| `motion::place_children` | 125–278 us/tick | Deferred to batched pricing |
| Avian `update_child_collider_position` | 134–274 us/tick | Deferred to batched pricing |
| All child placement passes, combined | 611–1 077 us/tick | Deferred to batched pricing |

These rows overlap: the combined row includes the individual rows. No new tick timing was taken.

## server-perf-2: sort only dynamic belt reactions

"driving: skip reaction sorting for contacts without dynamic recipients (#250)" classifies each
contact from the tick-start snapshot before constructing its sort key. Terrain-field support
has no recipient, and fixed or kinematic construction takes no dynamic impulse, so these
contacts leave before the canonical sort. Fixed structural reactions are already staged by
`record_static_contacts`; this changes no reaction outcome. Dynamic contacts keep the same
lexicographic geometry-bit key, stable relative order and sequential impulse arithmetic. The
key is a fixed-size array rather than a per-contact vector.

`field_and_fixed_support_never_enter_the_dynamic_reaction_sort` alternates field and fixed
contacts and measures sort-key construction. It failed on the preceding implementation with
32 keys and passes with 0 (MEASURED test counts, same fixture). The full targeted check passed
335 tests, including driving, recorded paths, collider history, contact transfer, headless and
real-UDP world reconstruction, plus the whole-tree guards; 3 existing tests were ignored
(MEASURED test-harness counts). No golden or physical parameter changed. New tick times remain
deferred to batched pricing.

## server-perf-2: what the entity count represents

Subject: the game's damage model and environment state, game state only. The census used the
shipped Battle mode, 29 bots and one parked headless client on 127.0.0.1. Both binaries ran from
this bay under one machine lock, `OVERMATCH_UNATTENDED=1`, `OVERMATCH_WINDOW=headless`; timing
recorders were disabled. The build used the warm `edit` profile because the readings are entity
membership, not execution times. A temporary exclusive `Last` system logged each archetype's
component list and live count at approximately 10 s intervals (MEASURED local). It was removed
from the source after the run, and both game processes were stopped.

The lossless readings are `server-entity-census.json.gz`: 46 snapshots and the component lists of
339 live archetypes (MEASURED local). The JSON's `snapshots` retain the per-archetype counts and
Battle phase; `archetypes` resolves their local IDs. The table lists classes contributing to growth; other archetype migrations
preserve their aggregate entity count. Elapsed time is since server startup, including fill-up.

| Archetype class / distinguishing components | Settled fill-up, 60.0 s | Last census, 451.8 s | Change |
|---|---|---|---|
| Intact element: `BuildingElement`, `Collider`, `ChildOf` | 2 294 | 2 282 | -12 |
| Expanded chunk: `BuildingChunk`, `Collider`, `ChildOf` | 0 | 608 | +608 |
| Live flight: `Projectile`, `Shot`, `ShellPath` | 0 | 16 | +16 |
| Loose wall body: `WallPiece`, `DebrisPiece`, `RigidBody` | 0 | 1 | +1 |
| Loose trunk body: `TreePiece`, `DebrisPiece`, `RigidBody` | 0 | 1 | +1 |
| Wall carrier: `WallPiece`, `DebrisPose` | 0 | 1 | +1 |
| Trunk carrier: `TreePiece`, `DebrisPose` | 0 | 1 | +1 |
| Tank roots: `NetTank` | 30 | 30 | 0 |
| Trunk segments: `TreeSegment` | 9 525 | 9 525 | 0 |
| **All entities** | **18 226** | **18 842** | **+616** |

Counts and elapsed times are MEASURED local; changes are DERIVED by subtraction. The entire
increase is accounted for: `608 - 12 + 16 + 1 + 1 + 1 + 1 = 616` (DERIVED). This same accounting
has zero remainder at every sampled combat boundary (MEASURED census checked by arithmetic).
The sampled peak was 18 867 entities at 230.6 s, with 43 live projectiles (MEASURED local).

The cause is construction becoming explicit, not abandoned ownership: `buildings::breakage::reconcile`
replaces a swapped element's coarse collider with its surviving chunk colliders. Its `have_chunks`
map prevents repeated births for an already-present chunk. `spawn_group` adds a loose body's
root and its carrier; the tree law reparents its existing segment entities and adds its own
body and carrier. The chunk population stabilised while the transient flight population varied.
Use this run as membership attribution; its play trajectory differs from the older inspection.

Decision: preserve these occupied game-state entities. Removing them to suppress the count
would change construction and contact geometry. This run found no unexplained local entity
growth in the observed combat window. The exact composition of Edgegap's +842 (DERIVED from #250's
MEASURED 18 236 and 19 078) remains unmeasured; applying the local attribution to that run is
an inference, not a verified remote census.

### Census limit and rebuild guard

The game's damage-model walk panicked in `ballistics::integrate_projectiles` after the last
census, with `UnexpectedExit` for sample 2, volume `18745v1`, primitive `18873v5`, shell 0,
`t=0.6176264910893549`, triangles `[11]` (MEASURED diagnostic). The archive keeps that diagnostic;
raw local logs are under `/tmp/server-perf-2-census/`. The census does not establish whether this
panic predates this lane. No flight-law exception, catch-and-continue path or gameplay quality
cut was added. The full shipped Battle and its rebuild were not completed.

The existing generic real-UDP test
`net::world_reset::tests::a_destroyed_world_is_startup_again_at_next_fill_up_over_real_udp`
passes on this lane. It compares the server's rebuilt construction census with its fresh census,
checks old roots are gone and checks carrier generations. In production `world::reconstruct`
sweeps map and loose-body roots with their children; `world_reset::start_next` sweeps the old
pose carriers and ledgers. This is fixture evidence for ownership cleanup, not a completed
shipped-cycle measurement.

Remaining verification: resolve the walk invariant and complete a shipped Battle; price the
landed per-system changes in the orchestrator's batched one-vCPU lane. G1 and the fill-up
command-path hold were left unchanged. All game source edits and temporary census instrumentation
are confined to this bay; no remote deployment or issue comment was made.

## server-perf-2 priced

Subject: the game's dedicated server tick, game state only. Every time below is MEASURED locally
(Apple M4, traced, us per fixed tick, the mean over the window) unless labelled; a "change of the
means" is DERIVED from the two runs of each tree.

**Trees.** Before: main at "docs: record the rebuild and combat pairs after the proxy key fix
(#250)", server-perf-1's last commit. After: main at "docs: record the sweep-ball-8 review and the
transit membership (#247, #254)", which carries server-perf-2's six commits, the #254 fix and 54
other commits (#215, #227, #236, #238, #245, #247, #249, #253). A delta is the tree's, not
server-perf-2's alone; where another commit causes one, the text names it.

**Method.** As "Method" above, with these differences. Each tree's server and harness client were
built from that tree (`--bin overmatch --bin overmatch-server`, the same `perf` +
`bevy/trace_chrome` build), each with its own tree's `assets/` as `BEVY_ASSET_ROOT` (the handshake
folds `PROTOCOL_REV`, 59 before and 62 after). One run: the server alone with `OVERMATCH_BOT=29`,
an empty-map window from 5 s after "listening" for 30 s; then the parked occupant joins, and the
combat window opens 15 s after "Battle 1 Combat" and runs 120 s, also read as minute 1 and minute 2.
Four more runs priced the server alone with `OVERMATCH_BOT=0`. Order: before-1; then after-1,
before-2, after-2 in one machine-lock hold; then after-3, before-3, after-4, before-4 (no bot) in
another. Each trace (21-25 GB per bot run, 4.2 GB per no-bot run) was profiled and deleted before
the next run started, so no run overlapped a profile or a compile. The profile is
`trace_profile.py`'s attribution over several windows in one pass (a scratch copy with a
byte-level line parse, equal to the tool's output on a synthetic trace; not committed). Tick total
is the TLDR's "Fixed busy": leaf busy time in `FixedFirst` through `FixedLast` and the physics
schedules. Every window held 63.9-64.0 Hz with 0 unpaired spans, and no run panicked. A `top`
sample in after-1's combat showed only the server (85 %) and the client (73 %) above 7 % CPU.

Rows overlap: the physics step includes Avian's `update_child_collider_position`; the tick total
includes every row except lightyear's pass, which runs once per frame in `RunFixedMainLoop`.

### Empty map: the server alone, no tank (`OVERMATCH_BOT=0`)

| System, us per tick | before-3 | before-4 | after-3 | after-4 | Change of the means |
|---|---|---|---|---|---|
| Drive step `track::sim::apply_track_forces` | 5 | 3 | 5 | 5 | +1 us |
| Child placement `motion::place_children` | 202 | 106 | 43 | 40 | -113 us (-73 %) |
| Child placement, Avian `update_child_collider_position` | 204 | 111 | 42 | 39 | -117 us (-74 %) |
| Child placement, lightyear_avian `update_child_collider_position` | 126 | 96 | 137 | 131 | +23 us (+21 %) |
| Avian `propagate_collider_transforms` and its batches | 368 | 238 | 387 | 373 | +77 us (+25 %) |
| Collider history `motion::snapshot` + `seed_births` | 55 | 46 | 82 | 78 | +29 us (+58 %) |
| Physics step (`PhysicsSchedule` + `SubstepSchedule`) | 500 | 321 | 353 | 331 | -68 us (-17 %) |
| **Tick total** | 1 438 | 913 | 1 189 | 1 138 | -12 us (-1 %) |
| `FixedMain` wall time | 1 985 | 1 367 | 1 774 | 1 701 | +62 us (+4 %) |
| Stats-line tick p50 (traced run) | 1 797 | 1 263 | 1 814 | 1 631 | +193 us (+13 %) |
| Entities | 13 068 | 13 068 | 13 070 | 13 070 | |

The two placement passes server-perf-2 rewrote fell 73-74 %, and their share of the tick from
24-28 % to 7 %. The tick total did not move measurably: before's two runs span 913-1 438 us, the
core-type noise of "Method", and lightyear's frame pass (+21 %), Avian's collider-transform
propagation (+25 %) and the collider history (+58 %) rose by about what the placement saved. That
the history rise is the disabling-component reads of "ballistics: retain history when colliders
are re-enabled (#250)" is ASSUMED. The largest cost left on the map is Avian's
collider-transform propagation, 373-387 us.

### Empty map with `OVERMATCH_BOT=29`: now 29 held tanks

The after tree's fill-up places the 29 bot tanks at startup and holds their commands ("net:
players wait out the fill-up in their held tank (#253)"); before, bots spawned at combat. This
window therefore prices 29 parked tanks after, not an empty map:

| System, us per tick | before-1 | before-2 | after-1 | after-2 |
|---|---|---|---|---|
| Drive step `track::sim::apply_track_forces` | 3 | 2 | 2 866 | 2 819 |
| Child placement, `place_children` + Avian's pass | 223 | 262 | 123 | 119 |
| Child placement, lightyear_avian's pass | 99 | 112 | 123 | 121 |
| Avian `propagate_collider_transforms` and its batches | 258 | 260 | 341 | 317 |
| Collider history `motion::snapshot` + `seed_births` | 47 | 53 | 188 | 182 |
| Physics step | 239 | 252 | 933 | 697 |
| **Tick total** | 990 | 1 036 | 4 842 | 4 704 |
| Stats-line tick p50 (traced run) | 1 411 | 1 518 | 4 682 | 4 618 |
| Entities / tanks | 13 068 / 0 | 13 068 / 0 | 18 058 / 29 | 18 058 / 29 |

Twenty-nine tanks standing still cost 2.8-2.9 ms of drive step, 59-60 % of this tick, against
3.0-3.1 ms for thirty moving tanks in the same tree's combat minute 1 (below), so most of the
drive step's cost does not depend on motion (DERIVED from those rows). A deployment with bots no longer idles at
the map's cost; one without bots still does.

### Combat, 30 tanks, 120 s

| System, us per tick | before-1 | before-2 | after-1 | after-2 | Change of the means |
|---|---|---|---|---|---|
| Drive step `track::sim::apply_track_forces` | 7 478 | 7 364 | 5 036 | 4 500 | -2 653 us (-36 %) |
| Child placement `motion::place_children` | 97 | 97 | 54 | 54 | -43 us (-44 %) |
| Child placement, Avian `update_child_collider_position` | 97 | 96 | 67 | 68 | -29 us (-30 %) |
| Child placement, lightyear_avian `update_child_collider_position` | 91 | 92 | 105 | 109 | +15 us (+17 %) |
| Avian `propagate_collider_transforms` and its batches | 392 | 394 | 353 | 365 | -34 us (-9 %) |
| Collider history `motion::snapshot` + `seed_births` | 187 | 184 | 187 | 188 | +3 us (+1 %) |
| Physics step (`PhysicsSchedule` + `SubstepSchedule`) | 1 111 | 1 116 | 1 200 | 1 018 | -5 us (0 %) |
| **Tick total** | 10 168 | 10 051 | 7 480 | 7 035 | -2 852 us (-28 %) |
| `FixedMain` wall time | 10 169 | 10 048 | 7 501 | 7 040 | -2 838 us (-28 %) |
| Stats-line tick p50 (traced run) | 9 154 | 8 761 | 6 957 | 6 627 | -2 166 us (-24 %) |
| Drive share of the tick total | 74 % | 73 % | 67 % | 64 % | -8 points |
| Entities | 18 252 | 18 252 | 18 330 | 18 332 | |

The drive step fell 36 % and its share of the tick from 73-74 % to 64-67 %; the tick total fell
28 %. Physics and the collider history did not move; the physics step's share rose from 11 % to
14-16 % only because the tick shrank. Lightyear's pass costs more per tick only because a lighter
tick leaves room for more frames per tick: 2.5-2.6 against 1.8, at 42 us per frame against 51.

### Combat, minute by minute

| us per tick | before-2 min 1 | after-1 min 1 | after-2 min 1 | before-2 min 2 | after-1 min 2 | after-2 min 2 |
|---|---|---|---|---|---|---|
| Drive step | 3 393 | 3 073 | 3 024 | 11 339 | 6 998 | 5 975 |
| **Tick total** | 5 951 | 5 448 | 5 546 | 14 155 | 9 512 | 8 525 |
| Stats-line tick p50 (traced run) | 5 512 | 5 212 | 5 296 | 14 438 | 9 537 | 8 379 |
| Drive share | 57 % | 56 % | 55 % | 80 % | 74 % | 70 % |

before-1 has no minute rows: its trace was profiled whole and deleted before the split was added.
The drive step grows through a combat at a constant entity count (18 252-18 335): before 3.4 to
11.3 ms from minute 1 to minute 2, after 3.0 to 6.0-7.0 ms. The saving grows with it, -10 % of
the drive step in minute 1 and -43 % in minute 2 (DERIVED, means). What grows is not measured.
That it is the support candidates near each tank, which before each tank scanned for every tank
and after scans only for itself ("driving: reuse each tank's tick-start support candidates
(#250)"), is ASSUMED. before-2's minute 2 ran at a traced tick p50 of 14.4 ms, one frame per
1.2 ticks.

### The fill-up after the rebuild: not reached

The shipped Battle runs 900 s of combat and 15 s of results before the rebuild. At these runs'
trace rate (about 100 MB/s, DERIVED from the trace sizes) a traced run to it would write about
100 GB, and a short-mode build edits the mode data (as the cycle run above did). Not priced.

### What remains on one core

The drive step: 4.5-5.0 ms of a 7.0-7.5 ms combat tick (64-67 %), and 6.0-7.0 ms (70-74 %) in
minute 2, still growing through the combat. The physics step follows at 1.0-1.2 ms (14-16 %),
then Avian's collider-transform propagation at 0.35-0.37 ms and the collider history at 0.19 ms.
Twenty-nine held tanks already cost 2.8 ms of drive step before combat.

For Edgegap (DERIVED, rough): the shares transfer, not the times. A 28 % cut applied to the
15.8-20.0 ms combat tick p50 Edgegap measured on alpha.7 (`perf-1101`) would give 11.4-14.4 ms
against the 15.6 ms budget. That base predates server-perf-1's fixes, the trace inflates the
systems it spans, and a 15-minute combat runs heavier than these two minutes, so only an Edgegap
re-measure settles it.
