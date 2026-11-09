Subject: instrumentation of the game server and its scenery model (game state only).

Added factor/kernel work counts, nested crumble solve/split wall unions, construction collider rehome counts, post-physics and FixedUpdate boundaries, child-collider writeback timing, separate tree update/optimize timings, First-through-Last and PostUpdate timers, and Linux host observations. The existing recorder and environment levers carry all fields.

Three local headless one-house captures completed, with 30 tanks and one real client. Every new macOS column is present and has positive samples. Every captured tick attributes 100% within floating conversion noise: phases plus additive leaves equal `tick_us`. Nested crumble and Avian details are excluded from that addition. No duration values are reported or priced.

Decision: count one numeric factorization attempt per `FactorPattern::refactor` entry; `Factor::new` delegates there and has its own `factor_new_calls` counter. Crumble solve and split remain nested wall unions because different owners can overlap them. Their spans are not additive. The owner task scheduling and game decisions stay in their existing path.

The table selects the largest `crumble_us` among ticks with both landings and prepared bodies; all three select tick 1628. Counts are MEASURED by the instrument; retry arithmetic is DERIVED below. Factor counts cover landing, standing-owner and sweep work in the tick. Rehome counts cover construction collider reuse at `ColliderBatch::apply`.

| Run | Tick | Attempts | Failed operations | Kernel calls | Kernel input chunks | Fresh factors | Failures | Ordering builds | Assemblies | Rehomed colliders |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| run-1 | 1,628 | 148 | 1,162,399 | 82 | 841 | 82 | 57 | 148 | 148 | 1,804 |
| run-2 | 1,628 | 145 | 1,232,730 | 80 | 833 | 80 | 59 | 145 | 145 | 1,804 |
| run-3 | 1,628 | 140 | 1,243,527 | 78 | 829 | 78 | 57 | 140 | 140 | 1,804 |

Kernel retries are zero at each selected tick: `factor_new_calls - kernel_of_calls = 0`; each kernel call attempts one factor. All failures therefore belong to part-graph refactors. DERIVED part-graph attempts are `factor_attempts - factor_new_calls`; successful part-graph attempts subtract `factor_failures` as well:

| Run | Part-graph attempts | Failed attempts / retries | Successful attempts | Attempts per success |
|---|---:|---:|---:|---:|
| run-1 | 66 | 57 | 9 | 7.333 |
| run-2 | 65 | 59 | 6 | 10.833 |
| run-3 | 62 | 57 | 5 | 12.400 |

Retries multiply part-graph attempt counts, while kernel construction has no retry multiplier at these ticks. These are work-count observations; they assign no duration or cause to the spike.

Every new runtime column follows. Cells are **positive samples / present samples** (MEASURED); zero is a measured absence of that work, not a missing field. Timer cells report sample counts only.

| Column | Run 1 | Run 2 | Run 3 |
|---|---:|---:|---:|
| `work.factor_attempts` | 40 / 2,575 | 180 / 2,487 | 38 / 2,601 |
| `work.factor_new_calls` | 37 / 2,575 | 64 / 2,487 | 31 / 2,601 |
| `work.factor_failed_ops` | 2 / 2,575 | 2 / 2,487 | 2 / 2,601 |
| `work.factor_failures` | 2 / 2,575 | 2 / 2,487 | 3 / 2,601 |
| `work.factor_ordering_builds` | 40 / 2,575 | 178 / 2,487 | 37 / 2,601 |
| `work.factor_assemblies` | 40 / 2,575 | 180 / 2,487 | 38 / 2,601 |
| `work.kernel_of_calls` | 37 / 2,575 | 64 / 2,487 | 31 / 2,601 |
| `work.kernel_of_chunks` | 37 / 2,575 | 64 / 2,487 | 31 / 2,601 |
| `work.rehomed_colliders` | 4 / 2,575 | 4 / 2,487 | 4 / 2,601 |
| `phases_us.bot_brain_us` | 2,575 / 2,575 | 2,487 / 2,487 | 2,601 / 2,601 |
| `phases_us.fixed_commands_us` | 2,575 / 2,575 | 2,487 / 2,487 | 2,601 / 2,601 |
| `phases_us.driving_us` | 2,575 / 2,575 | 2,487 / 2,487 | 2,601 / 2,601 |
| `phases_us.fixed_gameplay_us` | 2,575 / 2,575 | 2,487 / 2,487 | 2,601 / 2,601 |
| `phases_us.servos_us` | 2,575 / 2,575 | 2,487 / 2,487 | 2,601 / 2,601 |
| `phases_us.fixed_tail_us` | 2,575 / 2,575 | 2,487 / 2,487 | 2,601 / 2,601 |
| `phases_us.physics_mass_us` | 2,575 / 2,575 | 2,487 / 2,487 | 2,601 / 2,601 |
| `phases_us.collider_poses_us` | 2,575 / 2,575 | 2,487 / 2,487 | 2,601 / 2,601 |
| `phases_us.projectile_march_us` | 2,575 / 2,575 | 2,487 / 2,487 | 2,601 / 2,601 |
| `phases_us.damage_consequences_us` | 2,575 / 2,575 | 2,487 / 2,487 | 2,601 / 2,601 |
| `phases_us.recorded_consequences_us` | 2,575 / 2,575 | 2,487 / 2,487 | 2,601 / 2,601 |
| `child_collider_writeback_us` | 2,575 / 2,575 | 2,487 / 2,487 | 2,601 / 2,601 |
| `crumble_detail.solve_us` | 36 / 2,575 | 167 / 2,487 | 31 / 2,601 |
| `crumble_detail.split_us` | 2,573 / 2,575 | 2,487 / 2,487 | 2,599 / 2,601 |
| `avian.collider_tree_update_us` | 2,575 / 2,575 | 2,487 / 2,487 | 2,601 / 2,601 |
| `avian.collider_tree_optimize_us` | 2,575 / 2,575 | 2,487 / 2,487 | 2,601 / 2,601 |
| `frame_us` | 41 / 41 | 40 / 40 | 41 / 41 |
| `post_update_us` | 41 / 41 | 40 / 40 | 41 / 41 |
| `frames` | 41 / 41 | 40 / 40 | 41 / 41 |

| Run | Captured ticks | Stats rows | Attribution |
|---|---:|---:|---:|
| run-1 | 2,575 | 41 | 100% |
| run-2 | 2,487 | 40 | 100% |
| run-3 | 2,601 | 41 | 100% |

Linux-only fields are absent from the macOS stats type and absent in all three server logs. The Linux module is compiled and its stat-field, delta and typed-absence tests run in the local test build. Live Linux readings remain unexercised here. At startup, the armed Linux recorder logs THP mode and CPU topology/allowed CPUs. Per stats period, `linux_host` carries `clock_ticks_per_second`, host `steal_ticks`, process minor/major `page_faults`, and main/compute `thread_cpu` readings. Each reading is `Ok` or a named `Err`; a missing baseline and a regressing counter are separate absences. Thread CPU uses TID plus start time, and the live compute census is checked against Bevy's thread count.

Host field definitions follow the [kernel procfs documentation](https://www.kernel.org/doc/html/latest/filesystems/proc.html) and [THP controls](https://www.kernel.org/doc/html/latest/admin-guide/mm/transhuge.html). Bevy 0.19.1 and vendored Avian 0.7 source supplied the schedule and task-pool interfaces.

`frame_us` covers First through Last, including fixed ticks and their collectors, Update, PostUpdate and outbound send work. Runner pacing/sleep, startup and the stats-line collector are outside it. `post_update_us` is nested in the frame span; `frames` counts samples. The old summed `avian.collider_tree_us` is replaced by `collider_tree_update_us` and `collider_tree_optimize_us`.

Validation passed:

- `cargo fmt --all --check`.
- CI clippy: `scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`.
- Targeted tests: cost phases/stages/trace; server stats and Linux parsing/absence; fracture parts/kernel/factor counters; crumble; construction collider rehome counting; ram census and integration.
- Whole-tree guards: `doc_citations`, `ui_ascii`, `net_boundary`.
- Local client/server build: `scripts/serial-cargo build --locked --features dev_ui,bitprobe --bin overmatch --bin overmatch-server`.
- Three capture schema checks; every tick's additive sum checked by `scripts/perf/collapse.py` and the count analysis.

Capture command (run three times with distinct output directories):

```sh
cd /Users/Yan/github/vikng-dev/overmatch/.claude/worktrees/bay-2
BEVY_ASSET_ROOT="$PWD" OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1 \
scripts/perf/collapse.sh --target local --rehearsal \
--schedule /private/tmp/claude-502/-Users-Yan-github-vikng-dev-overmatch/13e20afd-7b0e-4ad2-9b6e-b78f39af0d6c/scratchpad/tracks/late-event.json \
--tanks 30 --clients 1 --duration 40 --bin "$PWD/target/counters-tick-bin" --out RUN_DIR
```

Raw captures: `/private/tmp/claude-502/-Users-Yan-github-vikng-dev-overmatch/13e20afd-7b0e-4ad2-9b6e-b78f39af0d6c/scratchpad/counters-tick/run-1`, `run-2`, `run-3` (each has server/client logs, `rows.json`, `tick-attribution.json`). Full count-only extraction: [`counts.json`](/private/tmp/claude-502/-Users-Yan-github-vikng-dev-overmatch/13e20afd-7b0e-4ad2-9b6e-b78f39af0d6c/scratchpad/counters-tick/counts.json).

The capture binaries are symlinks to this bay's freshly built `target/debug` client/server. Gate logs are preserved beside the raw captures under `gates/`. No push, merge, tag or release was performed.
