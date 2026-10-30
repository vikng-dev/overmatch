# Server profile per system, 30 tanks (#186 step 1)

The dedicated server's tick, system by system. It touches the game's damage model only as event
cost: rounds striking houses, knocked-out bots respawning. Game state only.

Labels: MEASURED (a stats line or a trace span), DERIVED (arithmetic on labelled numbers), ASSUMED.

## TLDR

- **Tick, untraced (MEASURED):** p50 4457 µs, p95 5175 µs of the 15 625 µs budget at 30 tanks.
  The server uses 74 % of one M4 core with Bevy's default pools. With 1 thread: p50 5163, p95 5718,
  49.7 % of a core. So the default pools buy 0.7 ms of wall time for 24 points of CPU.
- **One system is the tick.** `track::sim::apply_track_forces` takes 2522 µs per tick, 26.7 % of all
  busy time. That is +83 µs per added tank (DERIVED), ten times any other per-tank system. It is one
  serial loop over the tanks in `FixedUpdate`, so it is the first target for the multithreading lane.
- **Physics is 38.5 % of busy time** (traced, DERIVED). From the untraced stats line it is 25 % of
  the tick's wall time (p50 1125 of 4457 µs). Its largest item is avian scanning every collider's
  AABB every tick (2.4 ms at 30 tanks). At 2 tanks the scan still costs 1.5 ms, so it scales with the
  collider count, not with motion.
- **Bots cost the server 54 µs per tick, 0.57 % of busy time** (traced). Their AI and input
  (`drive_bot`) is 1.2 µs of that. The rest is respawns of knocked-out bots: 28 per 60 s window,
  event cost that a player's respawn would cost too.
- **Volley hitches are the largest waste found.** Every ~3 s one tick runs 20–200 ms (MEASURED:
  untraced max 180 ms). The 29 bots fire on the same tick each reload. When their rounds strike
  houses, `ballistics::integrate_projectiles` (up to 90 ms) and `buildings::breakage::apply_deposits`
  (up to 84 ms) spike in that same tick. Real players would not fire in sync, but each strike's cost
  is real.

## Setup

- **Machine:** Apple M4 (4 performance + 6 efficiency cores, 16 GB), macOS 26.5.2, 2026-10-31
  02:26–02:46 +07. Other lanes compiled between runs, never during one: each run held the machine
  lock.
- **Binaries:** tag `v0.7.0-alpha.3` (`ecdc13e`), `--profile profiling`, with
  `--features dev_ui,bitprobe,bevy/trace_chrome` (`profile/BUILD-alpha3.txt`). The server's pools
  were compute/async/io = 4/3/3 (its own startup line). The shipped server has neither `dev_ui` nor
  `bitprobe`.
- **Scene:** a local headless server, `OVERMATCH_BOT=29`, plus one GPU-less idle scripted client
  (`SPIKE_SIMULATE_INPUT=1 SPIKE_SIM_IDLE=1`): 30 tanks, one link, ~21.3k entities.
  `OVERMATCH_SERVER_STATS_S=1`. The window is 60 s, starting 30 s after the client takes its input
  slot (`tools/profile-run.sh`).
- **Per-system lever:** bevy's `trace_chrome` (the `bevy/trace_chrome` feature; `TRACE_CHROME` names
  the file). Every system run is a `system` span, its deferred commands a `system_commands` span,
  every schedule run a `schedule` span, and every parallel query batch a `par_for_each` span.
- **Analysis (`tools/trace_profile.py`):**
  - A **tick period** runs from one `FixedMain` start to the next: one fixed tick plus the 2.55
    frames of the 4 ms runner around it (MEASURED).
  - A span's **exclusive time** is its own time minus the system, schedule and command spans nested
    in it on its thread.
  - **Busy** is the sum of every leaf system, command and parallel batch per period, across all
    threads. It is CPU time, not wall time.
  - A system that runs a schedule (`run_fixed_main_schedule`, avian's runners) is a container. It is
    left out of the ranking.
- **Runs:**
  - `p1`: traced, 30 tanks.
  - `p2`: the same, spans filtered off at their callsites (`RUST_LOG=bevy_ecs=warn,bevy_app=warn`):
    the untraced truth.
  - `p3`: as `p2` with `OVERMATCH_SERVER_THREADS=1`.
  - `p4`: traced, 2 tanks (1 bot + the client), for the per-tank slope.

## The truth, untraced (stats-line medians over the window, MEASURED)

| Run | Tanks | Threads | tick p50 | tick p95 | tick max (worst second) | physics p50 / p95 | send p50 | CPU (% of a core) |
|---|---|---|---|---|---|---|---|---|
| p2 plain | 30 | default (4/3/3) | **4457** | **5175** | 180 475 | 1125 / 1454 | 119 | 74.1 |
| p3 plain | 30 | 1 | 5163 | 5718 | 190 469 | 1780 / 2009 | 97 | 49.7 |
| p1 traced | 30 | default | 5350 | 6315 | 199 281 | 1740 / 2164 | 170 | 128.6 |
| p4 traced | 2 | default | 1778 | 2324 | 3 737 | 854 / 1248 | 126 | 93.5 |

**Tracing overhead:** p50 +20 %, physics +55 %, CPU +54 points, the chrome writer thread included
(DERIVED, p1 against p2). It is a cost per span, so the many small systems inflate the most. A
system that runs once per tick and nests no spans, such as `apply_track_forces`, carries almost none
(ASSUMED). The traced busy times below rank the systems; the untraced tick above is the total.

## Top 20 systems by mean, 30 tanks (traced, µs per tick period)

"Share" is of the 9458 µs of busy time per period. Per added tank = (mean at 30 − mean at 2) / 28
(DERIVED). `par:` rows are parallel query batches run on worker threads; the system that issued
each one is named in the note.

| # | System (schedule) | runs/tick | mean | p95 | share | at 2 tanks | per added tank |
|---|---|---|---|---|---|---|---|
| 1 | `track::sim::apply_track_forces` (FixedUpdate) | 1.00 | 2522 | 2698 | 26.7 % | 195 | **+83.1** |
| 2 | `par:` avian collider AABB update, from `update_moved_collider_aabbs` (PhysicsSchedule) | 5.50 | 1753 | 2429 | 18.5 % | 1034 | +25.7 |
| 3 | avian `update_moved_collider_aabbs` (PhysicsSchedule) | 1.01 | 638 | 874 | 6.7 % | 425 | +7.6 |
| 4 | `lightyear_avian3d` `update_child_collider_position` (RunFixedMainLoop, every frame) | 2.55 | 276 | 536 | 2.9 % | 236 | +1.4 |
| 5 | `par:` avian solver-body AABB query (PhysicsSchedule) | 3.70 | 264 | 426 | 2.8 % | 10 | +9.0 |
| 6 | `par:` avian collider-transform propagation (FixedPostUpdate) | 3.42 | 254 | 394 | 2.7 % | 132 | +4.4 |
| 7 | avian `update_solver_body_aabbs` (PhysicsSchedule) | 1.00 | 228 | 317 | 2.4 % | 31 | +7.0 |
| 8 | bevy `propagate_parent_transforms` (PostUpdate, every frame) | 2.55 | 219 | 332 | 2.3 % | 78 | +5.1 |
| 9 | avian `propagate_collider_transforms` (FixedPostUpdate) | 1.00 | 207 | 312 | 2.2 % | 95 | +4.0 |
| 10 | `ballistics::integrate_projectiles` (FixedUpdate) | 1.00 | 179 | 34 | 1.9 % | 2 | +6.3 |
| 11 | avian `update_narrow_phase` (PhysicsSchedule) | 1.00 | 152 | 227 | 1.6 % | 45 | +3.8 |
| 12 | bevy `mark_dirty_trees` (PostUpdate, every frame) | 2.55 | 148 | 214 | 1.6 % | 140 | +0.3 |
| 13 | avian `collect_collision_pairs` (PhysicsSchedule) | 1.00 | 138 | 202 | 1.5 % | 15 | +4.4 |
| 14 | `buildings::breakage::apply_deposits` (FixedUpdate) | 1.00 | 111 | 5 | 1.2 % | 2 | +3.9 |
| 15 | avian `update_child_collider_position` (PhysicsSchedule) | 1.00 | 102 | 167 | 1.1 % | 64 | +1.3 |
| 16 | `par:` bevy dirty-transform scan (PostUpdate, every frame) | 11.72 | 97 | 156 | 1.0 % | 71 | +0.9 |
| 17 | `tank::servo::drive_servos` (FixedUpdate) | 1.00 | 90 | 174 | 0.9 % | 9 | +2.9 |
| 18 | `net::diagnostics::fixed_nan_probe` (FixedPostUpdate) | 1.00 | 81 | 165 | 0.9 % | 55 | +0.9 |
| 19 | avian `update_solver_body_angular_inertia` (SubstepSchedule, 6 substeps) | 6.00 | 69 | 108 | 0.7 % | 32 | +1.3 |
| 20 | avian `integrate_velocities` (SubstepSchedule) | 6.00 | 61 | 112 | 0.6 % | 34 | +1.0 |

- **By p95** the order is the same down to #12. `integrate_projectiles` and `apply_deposits` drop
  out (p95 34 and 5 µs): their mean is made of the volley spikes. `update_mass_properties` enters
  at p95 97 µs.
- **Full tables:** `profile/tables.md`. It has the p95 ranking, the per-tank ranking, busy time by
  schedule and schedule wall times. `profile/p*-trace.profile.json` holds all 760 leaf keys with
  mean, p50, p95 and max.

## Shares

| Part | µs per tick, 30 tanks (traced) | Share of busy | At 2 tanks |
|---|---|---|---|
| Physics (PhysicsSchedule + SubstepSchedule, parallel batches included) | 3640 | **38.5 %** | 1830 |
| FixedUpdate (game systems; `apply_track_forces` is 2522 of it) | 3030 | 32.0 % | 262 |
| PostUpdate, every frame (transforms, replication send, visibility) | 1135 | 12.0 % | 893 |
| FixedPostUpdate (collider transforms, mass, net publishing) | 722 | 7.6 % | 358 |
| RunFixedMainLoop, every frame (`lightyear_avian3d` collider positions) | 293 | 3.1 % | 252 |
| PreUpdate, every frame (receive, inputs) | 276 | 2.9 % | 286 |
| Update, StateTransition, Last, First, SpawnScene, fixed First/Last | 358 | 3.8 % | 348 |
| **All** | **9458** | 100 % | **4209** |

- **Physics on the wall clock:** PhysicsSchedule takes 1884 µs per tick (traced) against FixedMain's
  5765 µs. Untraced, physics p50 is 1125 of the 4457 µs tick, 25 % (DERIVED). Avian runs
  PhysicsSchedule and SubstepSchedule on a single-threaded executor
  (`avian3d-0.7.0/src/schedule/mod.rs:90`, `dynamics/solver/schedule.rs:54`). Only the parallel query
  batches inside its systems spread across cores.
- **Bots' own share (AI and input), which real players do not cost the server:**

  | Item | µs per tick |
  |---|---|
  | `drive_bot` (writes the bots' constant command) | 1.2 |
  | `spawn_bots` + `schedule_bot_respawn`, commands included | 5.5 |
  | `respawn_dead_bots`, commands included (28 bots knocked out per 60 s window) | 47.5 |
  | **Total** | **54.3, 0.57 % of busy** |

  Mean `respawn_dead_bots` is spikes: 71 ms in one tick, p95 4.5 µs. A knocked-out player's respawn
  (`respawn_player_tanks`) is the same kind of work. A real player instead costs the input path:
  `receive_input_message` 5.1 + its commands 2.7, plus `authorize_controlled_targets` 3.2 µs per
  tick, for the one link here. The bots' tanks simulate exactly as a player's do. Their one
  distortion is timing: they spawn together and fire on every reload, so all 29 rounds leave on one
  tick.

## Waste candidates (bug-class fixes)

1. **Volley hitches** (`profile/p1-30t-trace.spikes.txt`). In the 60 s window, 28 ticks ran over
   15 ms, at ~3 s intervals. The worst lasted 199, 155, 86, 80, 75 and 70 ms (traced, MEASURED).
   Untraced, the worst seconds reached 180, 111, 76 and 63 ms. In those ticks:
   - `integrate_projectiles` took up to 89.6 ms;
   - `apply_deposits` (building breakage) took up to 84.2 ms;
   - `respawn_dead_bots` took 71.2 ms plus 18.0 ms of commands on one tick.

   That each volley spike is rounds striking houses is ASSUMED, from `apply_deposits` running in
   the same ticks. A 200 ms tick costs 12 ticks of catch-up. The p95 hides all of this. Lead: the
   per-strike cost of a building strike, and what a respawn builds at spawn.
2. **`apply_track_forces` at 83 µs per tank per tick**, in one serial loop over the tanks
   (`src/track/sim.rs:715`). Each tank reads shared data (`TrackField`, `TrackGear`, volumes) and
   writes only its own components. That makes it a candidate for `par_iter_mut` (ASSUMED from the
   query's shape, not checked for hidden shared writes). It is also worth checking for work
   repeated per element per tick that could be cached.
3. **Avian scans every collider's AABB every tick.** At 2 tanks the scan already costs 1034 µs of
   parallel batches plus 425 µs in the system. It grows only 33 µs per added tank (DERIVED), so the
   cost comes from the ~13.8k entities of map, trees and houses (MEASURED at 2 tanks), not from
   motion. Upstream knows: `avian3d-0.7.0/src/collider_tree/update.rs:888` says "TODO: par-iter over
   all colliders, check if they have actually changed since the LastPhysicsTick". Lever on our side:
   fewer static collider entities, for example one compound collider per tree or house.
4. **Every-frame work on a headless server.** The 4 ms runner runs Main 2.55 times per tick, but
   poses change only on ticks. Per tick (traced):
   - `lightyear_avian3d`'s `update_child_collider_position` takes 276 µs, on top of avian's own 102
     µs pass;
   - transform propagation in PostUpdate takes ~480 µs;
   - DefaultPlugins' client-only systems take ~600 µs: camera and light visibility, ui, picking,
     gizmos, sprites, audio, animation, scene spawning, input focus. This is an upper bound; span
     overhead dominates systems this small;
   - executor spans take 500 µs, over 449 executor runs.

   Lever: a server root without the client plugins. Price it A/B; the traced numbers only rank it.
5. **`net::diagnostics::fixed_nan_probe` takes 81 µs per tick** in an optimized build. It is a
   diagnostic that could be gated.
6. **Threads.** Bevy's default pools buy 0.7 ms of p50 (4457 against 5163 µs) for +24 points of
   one core. That trade inverts on a 1-vCPU host, where the extra threads only contend.

## Products

`profile/`:

- `p*.server.json`: the stats-line window per run (`scripts/perf/online_cost.py server-window`).
- `p*.profile.json`: the per-system tables.
- `p*.window.json`: the window's wall offsets.
- `p1-30t-trace.spikes.txt`: every span of 5 ms or more inside the window.
- `tables.md`.
- `BUILD-alpha3.txt`.

`tools/` holds the runner and the analysis scripts. The raw traces (11–12 GB each) are not kept.
