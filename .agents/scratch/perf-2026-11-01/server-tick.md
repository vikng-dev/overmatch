# Server tick inspection, 2026-11-01 (#250)

Subject: the game's dedicated server and its fixed tick, game state only. A knockout empties a
tank's HP pool. Labels: MEASURED (a stats line, a trace span, a test), DERIVED (arithmetic on
labelled numbers), ASSUMED.

The Edgegap symptoms this answers are in #250. Local numbers are from an Apple M4 and do not
transfer as times; per-system shares and before/after deltas do. Every Edgegap time below is
DERIVED from a local one with the 3.7x host ratio of `../perf-2026-10-31/README.md` ("alpha.4
differs from alpha.3"), which is itself DERIVED.

## TLDR

1. **Top costs per phase** (MEASURED, bevy `trace_chrome`, per fixed tick, M4; share of the
   fixed schedules' busy time, `FixedFirst` through `FixedLast`):

   | Phase | Fixed busy | 1st | 2nd | 3rd | 4th |
   |---|---|---|---|---|---|
   | Empty map (server alone, 13 066 entities, no tank) | 1 619 us | `ballistics::motion` collider history, 718 us (44 %) | avian step (`PhysicsSchedule`), 364 us (22 %) | avian collider-transform propagation, 265 us (16 %) | `net::diagnostics::fixed_nan_probe`, 71 us (4 %) |
   | Combat (29 bots + 1 parked client, 18 250 entities) | 6 819 us | `track::sim::apply_track_forces`, 3 601 us (53 %) | avian step, 1 307 us (19 %) | `ballistics::motion` collider history, 780 us (11 %) | avian collider-transform propagation, 384 us (6 %) |

   Outside the tick, every 4 ms frame (3.33 per tick) adds 1 149 us (empty) and 1 469 us (combat)
   per tick of transform propagation, visibility, receive and send (MEASURED, same traces). On one
   vCPU those frames share the core with the tick.
2. **What serialises the tick: everything, and the drive step most.** In combat the fixed
   schedule's wall time equals its busy time (6 841 against 6 819 us, MEASURED, traced) on a
   machine whose compute pool has 10 threads, and the untraced server holds 59.5 % of one core at
   tick p50 5.2 ms (MEASURED). `apply_track_forces` ran its tanks with `par_iter_mut` in
   `v0.7.0-alpha.5`; "driving: carry tanks over pieces through the belt footprint (#238)"
   replaced that with one serial loop over the tanks and a shared reaction ledger. It is 53 % of
   the combat tick's work and was the share alpha.5's second core ran. Not fixed here (see "Not
   fixed").
3. **The empty map's tick goes mostly to the collider history** that "ballistics: retain composed
   collider poses between walks (#191)" added after alpha.5: two hash maps over every collider and
   every body rebuilt each tick, plus the per-collider pass of `seed_births`. Fixed: the tables
   are kept and only what moved is rewritten (fix 1). The NaN probe's full collider pass is fix 2.
4. **The rebuild and the combat growth have one cause:** a collider despawned off a body leaves
   its broad-phase proxy behind in Avian's standalone tree, and that proxy pairs with every collider
   its bounds overlap, for the rest of the process. The rebuild orphans the whole map's proxies
   (46 087 contact pairs where a fresh map holds none); a combat orphans every despawned part's.
   Fixed in the vendored Avian (fix 3).

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
read as a tick delta; the traced per-system table is the evidence. The motion history was 586 us
of a 1 619 us fixed tick (36 %) and is 67 us after (DERIVED: the rows above), so on Edgegap the
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
an `OVERMATCH PATCH` in `vendor/avian3d-0.7.0-child-pose` (`OVERMATCH_PATCH.md`).

RSS on macOS is not comparable across the rebuild (memory compression: 438 MB then 260 MB,
MEASURED), so the +80 MB on Edgegap is not reproduced here; 46 087 leaked pairs and their
manifolds are the likely share of it (ASSUMED, not measured).

## What grows through a combat

Same cycle run (MEASURED): contact pairs climb from 150 at combat start (30 tanks on the ground)
to 2 442 after 145 s with 30-31 dynamic bodies throughout; 333 remain at Results with one dynamic
body; Battle 2's combat starts at 46 885 and climbs to 48 787 in 60 s. Physics p50 goes 358 to
821 us over Combat 1. Entities grow 18 229 to 18 272 (+43), so the pairs, not the entities, track
the physics growth. Every despawned part with a collider on a body leaks a standalone proxy the
same way (fix 3). The combat tick's own growth is not separated here: the `sample` run inflated
the cycle run's combat ticks, so its tick column is not used.

## Not fixed, and owner calls

- **The serial drive step.** `apply_track_forces` is 53 % of the combat tick (3.6 ms of 6.8,
  MEASURED traced; about 3.7 ms per tick in `sample`, MEASURED untraced), most of it the belt's
  ground and rubble queries (`belt_tick` into `depth_along_lowered_ground`, `terrain_grid` ray
  casts and heights, `rubble::in_slab`). Re-parallelising it buys nothing on one vCPU; cutting the
  per-tank cost is the fix, in #238's code. Owner call: whether Edgegap stays on one vCPU (then
  only the per-tank cost matters) or moves to two (then the loop's parallel form is worth having).
- **Three passes place every child collider each tick.** `motion::place_children`, Avian's
  `update_child_collider_position` and lightyear_avian's `update_child_collider_position` together
  cost 611-1 077 us per tick on the empty map (MEASURED, the before and after traces), each iterating every
  child collider, though no map collider moves. Not investigated further in this lane.
- **Frames outside the tick** (1.1-1.5 ms per tick, `mark_dirty_trees`, transform propagation,
  visibility) are the dedicated server running Bevy's transform and visibility plugins over a map
  that does not move. Not investigated further.
- **Unexplained:** the Edgegap +840 entities through a combat. Locally the entities grow +43 in
  150 s; a full 16.5 min Battle was not run.
