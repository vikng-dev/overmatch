# Battle immediate start: follow-up verification

Subject: the game's match flow and damage-model test tooling, game state only. No damage-model
law or gameplay start rule changed. Lane `battle-no-wait-2`; verification date 2026-11-04.

| Finding | Disposition | Fixed in (commit subject) |
|---|---|---|
| 1 — `scripts/cost/run.sh` admission and fake clients | Verified. Fakes announce their input slots and wait for the runner's file before completing or failing execution. An admission exit identifies its client; cleanup no longer produces a false server-exit report. | `scripts: exercise cost-runner admission (#271)` |
| 2 — `src/net/client.rs::loading_waits_for_the_own_tanks_bound_view` | Verified. A playable snapshot and another tank's bound view now precede the own view. The fixture also retains coverage of the view arriving before the snapshot. | `net: expose the bound-view loading guard (#271)` |
| 3 — independent shot-client starts | Verified. `scripts/shot/run-mg-armor.sh` and `scripts/shot/run-pencam-latency.sh` wait for both slots through the same helper as the cost runner. Their script lifetimes and firing offsets use the shared tool epoch. | `scripts: admit both shot clients before workloads (#271)` |
| 4a — `.agents/docs/design/battle-cycle.md` Verification | Verified. The current description uses finite launch and script-completion bounds and describes the runner barriers. The retired fill-up evidence remains explicitly historical. | `net: align Battle docs and score participation (#271)` |
| 4b — `src/net/server.rs::handle_new_clients` | Verified. Ownership does not imply prediction: every client interpolates the hull. The matching respawn comment is corrected too. | `net: align Battle docs and score participation (#271)` |
| 4c — `src/net/battle_hud.rs::MatchUi::Clock` | Verified. The clock describes combat/results, excluding a waiting snapshot. | `net: align Battle docs and score participation (#271)` |
| 4d — `src/net/score.rs::standings_start_at_combat_participation_and_outlive_its_tanks` | Verified. A waiting-only departure has no standing; a tank still present at the first arrival enters through the production combat boundary. Combat spawns enter too, and their standings survive removal and respawn. | `net: align Battle docs and score participation (#271)` |
| 5 — `src/net/battle_hud.rs::hud_layout_app` | Verified. The fixture documentation describes its vitals, seat and drive readouts without promising a repair hint. | `net: align Battle docs and score participation (#271)` |

The decision is to coordinate these tools with `SPIKE_WORKLOAD_READY`; the Battle never waits
for a tool cohort. `scripts/lib/harness-start.zsh` owns the shared slot predicate, finite admission
bound and failure diagnostics for the cost runner and both shot runners. Callers retain ownership
of lane order, script lengths and firing schedules.

The runner tests also exposed Zsh skipping `EXIT` cleanup when `errexit` occurs inside a function.
The shot runners explicitly exit on admission failure, and both client processes are direct
children that cleanup can reap. A cost-runner harness-write failure now cancels the other client
waiting for the start file. Fake-process checks cover both failures. Public shot-runner entry
builds through `serial-cargo`, then holds one machine lock across hidden, unattended clients and
a loopback-bound server.

MEASURED mutation evidence: removing only `With<crate::tank::TankViewReady>` left the original
loading test green. With the revised fixture, the same mutant failed at
“a playable phase and another tank's view cannot present the own tank.” The production filter
was restored, and the client module passed. Both mutant runs used:

```sh
scripts/serial-cargo test --lib --features dev_ui,bitprobe loading_waits_for_the_own_tanks_bound_view
```

MEASURED gates:

- Client module plus `doc_citations`: 38 passed; one existing live-hosted test ignored because it
  requires the hosted API and can start a billed server. No hosted calls were made.
- Harness module plus `doc_citations`: 17 passed.
- Server, score and HUD modules plus `doc_citations`, `ui_ascii`, `net_boundary`, and `bake::`:
  101 passed. The score witness mounts `server::install_battle` and advances its real fixed schedule.
- `cargo fmt --all --check`, CI clippy with `--locked --workspace --all-targets --features
  dev_ui,bitprobe -- -D warnings`, changed Zsh syntax, and `git diff --check`: passed.
- The revised cost suite passes six tests. The new shot-runner suite passes three tests, each
  exercised against both runners: held late admission, admission failure, and execution failure.
  It also checks stale-file removal, withheld analysis on failure, and process cleanup.

The initial cost suite reproduced three failures among its four existing tests. After its fixture
correction, every tracked script suite completed successfully; no other script regression from
`net: start Battles on the first connected tick (#271)` was found. The new shot-runner suite is
included below. Python suites ran separately with Python and numpy available; shell suites ran
with their documented shell. Real asset-tool invocations ran under the machine lock, and their
cargo calls were routed through `serial-cargo` with the required feature set.

| Script suite | Result |
|---|---|
| `scripts/bays/test_bays.sh` | Passed |
| `scripts/bitprobe/test_diff.py` | Passed |
| `scripts/cost/test_run.py` | Passed |
| `scripts/disk/test_sweep.py` | Passed |
| `scripts/edgegap/test_session.py` | Passed; outbound calls are stubbed |
| `scripts/hooks/test_pushed_assets.sh` | Passed |
| `scripts/lod/test_refusals.py` | Passed |
| `scripts/perf/test_analyze.py` | Passed |
| `scripts/perf/test_collapse.py` | Passed |
| `scripts/perf/test_online_cost.py` | Passed |
| `scripts/perf/test_pencam_cost.py` | Passed |
| `scripts/prop/test_prop.py` | Passed |
| `scripts/shot/test_analyze.py` | Passed |
| `scripts/shot/test_pencam_latency.py` | Passed |
| `scripts/shot/test_runners.py` | Passed; new suite |
| `scripts/tank/test_asset_door.py` | Passed |
| `scripts/tank/test_build.py` | Passed |
| `scripts/tank/test_glb_ktx2.py` | Passed |
| `scripts/tank/test_ktx2_cache.sh` | Passed |
| `scripts/tank/test_refusals.py` | Passed |
| `scripts/tank/test_report.py` | Passed |

No script suite skipped a test. No backlog row names an edited item, so no row was closed.
Bevy test APIs were checked against the pinned 0.19.1 registry source; the connection marker's
server export was checked against Lightyear 0.30.1 source. No frame/tick timing, captures, screen
use, push, or issue comments. No owner decision remains.
