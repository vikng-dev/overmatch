# Capture clock — #385

The game's capture timeline, including its damage-model cues: game state only, with crew as HP pools at stations.

Offline and sandbox plans start when their composition is placed; online clients and dedicated servers retain their startup clock. `clock::install` suspends the plan-selected pacing before the first update, and `start_clock` restores it before Bevy's time update. Resetting the real clock at release excludes loading wall time. `Cues` gates frames, key holds and event cues, including a zero-time track-print set-down.

| Composition | Readiness |
|---|---|
| Offline scenarios and demos | Playing, at least one tank, every tank view bound |
| Armour sandbox | At least one tank, every tank view bound |
| Track sandbox | Constructed rig geometry published |
| Online clients / dedicated servers | Startup; existing clock ownership preserved |

The waiting state contains only the three real gates. Its installation records a monotonic wall deadline (Resolution: 120 s to allow asynchronous decode/binding while bounding failed readiness). Expiry panics with the anchor and its missing state, bound/total views, or absent rig geometry. The sandbox reports a target glb/scene load failure directly, even after spawn consumes its pending handle.

The shipped capture builder accepts a plan reference, so its registrations and frame-step installation order are tested with fixture plans. Impact vocabulary resolves once at parsing into typed values; bad keys refuse admission naming their field and repair. Terrain coordinates bind against the loaded grid's span or the flat slab's extent in PostStartup. Follow refuses tankless composition anchors before installation; use Pose there. The extra burst-view warm-up is a labelled Resolution constant for spawned views/pipelines before retargeting, separate from target warm-up.

Visual inspection of both final proof scenes confirmed the HUD is hidden. The crew status card and sight refusal toast show with Inherited visibility, so hiding their HUD root hides them too. The other explicit Visible HUD writers own roots that capture hides; the shared clock readout already inherits. The module docs name the remaining frame-based writes and message producers/readers with source lines: required construction messages are retained, and no shipped offline fixed reader consumes a loading-frame-count-dependent gameplay backlog. The zero-time demo's simulation write is held by Cues.

## Repeated-frame proof

MEASURED on Apple M4 / Metal. Each phase used one binary, five fresh processes per plan, initially empty configuration directories, 1280×720 physical pixels at scale 1, and the authored fixed step. Every process used `OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=hidden OVERMATCH_MUTE=1`; each exited successfully. The runner changed only frames.out in private copies of the tracked recipes and supplied each recipe's documented OVERMATCH_START. Hashes cover PNG bytes; they are observations, never regression-test targets.

BEFORE: detached `72e34cb5`, built and run in this same bay, then switched back. AFTER: the lane with `main` through `f279a1a5` merged and the review fixes. The table counts distinct SHA256 values for each requested frame across five runs.

| Plan / frame | BEFORE distinct / 5 | AFTER distinct / 5 |
|---|---:|---:|
| `hull-closeup`, 12 s | 2 | 1 |
| `w1d-wall`, 12 s | 3 | 1 |

Binary SHA256, BEFORE: `b8e2bc7601ec96f7c30537a8a1aa0d9fb917685d058d869aeb48eacf01ef90cd`.
Binary SHA256, AFTER: `4307007e8abf4531e40edd6192563bdd392fb0188a8cdb0f6532324462bd4fe2`.

Artifacts remain outside git under `/Users/Yan/.cache/overmatch/capture-clock/`: `before-proof.json`, `after-proof.json`, `before-<plan>-<run>/` and `after-<plan>-<run>/` hold hashes, exact flags, plans, logs, fresh config directories and frames. `run-proof-v2.py` reproduces the procedure. These phases include intervening main changes; the observation is repeatability within each phase, not isolation of every visual change to the clock.

## Mutant proof

Every mutation was applied independently, run through `scripts/serial-cargo test --lib --features dev_ui,bitprobe <test>`, observed failing a test (not merely compilation), and reverted. Source diffs, logs and restoration hashes are in `mutant-*.diff`, `mutant-*.log`, and `mutants.json` in the cache.

| Mutation | Failing test | Result |
|---|---|---|
| `empty-tanks` | `tank_anchors` | Test failed; restored |
| `mount-order` | `shipped_plan_builder` | Test failed; restored |
| `tank-view-anchor` | `release_discards` | Test failed; restored |
| `track-rig-anchor` | `track_anchor` | Test failed; restored |
| `release-real-reset` | `release_discards` | Test failed; restored |
| `capture-cue` | `shipped_plan_builder` | Test failed; restored |
| `keys-cue` | `shipped_plan_builder` | Test failed; restored |
| `impacts-cue` | `all_offline_cue` | Test failed; restored |
| `shot-cue` | `all_offline_cue` | Test failed; restored |
| `wall-cue` | `all_offline_cue` | Test failed; restored |
| `server-wall-cue` | `all_offline_cue` | Test failed; restored |
| `repair-cue` | `all_offline_cue` | Test failed; restored |
| `corner-cue` | `all_offline_cue` | Test failed; restored |
| `zero-time-demo` | `zero_time_set_down` | Test failed; restored |

## Gates and lenses

The merged final tree passed `cargo fmt --all --check`, CI clippy (`--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`), and the requested targeted groups: dev_capture, sandbox, crew_ui, track_prints, tests::ui_ascii; sight::reticle also passed for the toast fix. The capture binary built through serial-cargo with dev_ui,bitprobe. All mutations were removed before the final gates and captures. No full-suite run, push, external comment, agent delegation, debugger, or target-directory cleanup was used.

Self-review covered all six lenses: explicit failures and typed admission; one clock path per composition; generic readiness and owner-derived terrain/timestep values; comments limited to current invariants and units; production wiring and behavior tests with demonstrated failing mutants; no additional recurring authoritative-server serialization. No backlog row names an edited file. Main's retired plan-test history sentence is already gone in the merged version.

## Logged follow-up — deliberately untouched

- `src/dev_capture/repair.rs::start_panel_repair`: requested module-function checks still occur when the cue runs.
- `src/dev_capture/corner.rs::replay`: participant binding remains a cue-time check.
- `src/dev_capture/wall.rs`: the requested late target checks remain at their existing boundary.
- `src/dev_capture/fuze.rs::bind_target`: the requested bare staging numbers remain.
- `src/dev_capture/plan.rs`: the requested bare defaults remain.

Live network arrivals, physical device input and wall-paced sampling are not replay guarantees. Five repeats do not prove universal rendering determinism.
