# Replay the settling census

Subject: the video game's environment destruction; game state and library counters only.

Use a leased bay and export:

```sh
export OVERMATCH_CARGO_LOCK_WAIT_S=10800 OVERMATCH_UNATTENDED=1
export OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1
```

The before source is the commit subject `scratch: destruction unification designs, plan and review (#106)`; the after source is `physics: preserve island rest history and dormant contacts (#106)`. Apply `before-counters.patch` to the former, or `after-counters.patch` to the latter. These are temporary observational probes, never shipping code. `residual-pose.patch` is an alternative after probe with all-body poses. Remove a probe before applying another.

Each probe adds the ignored diagnostic `buildings::breakage::tests::diag_settle_pile`, using the hunt's native construction inputs and the production stress owner's independent pile constructor. It changes no sleep setting and never forces sleep.

```sh
SETTLE_KIND=native SETTLE_SECONDS=80 scripts/serial-cargo test --lib --features dev_ui,bitprobe buildings::breakage::tests::diag_settle_pile -- --exact --ignored --nocapture --test-threads=1
SETTLE_KIND=1000 SETTLE_SECONDS=120 scripts/serial-cargo test --lib --features dev_ui,bitprobe buildings::breakage::tests::diag_settle_pile -- --exact --ignored --nocapture --test-threads=1
scripts/serial-cargo build --locked --bin overmatch-server --bin overmatch --features dev_ui,bitprobe
```

Apply `collector.patch` to the hunt's existing `measure.py`. It disables the stats/pricing lever and buffers incomplete log lines. Use these census-only invocations, with sampling disabled:

```sh
python3 .agents/scratch/slowdown-hunt/evidence/measure.py before-shell --profile debug --variant 'dev opt1; census and counters; no pricing' --duration 120 --shells 20 --samples ''
python3 .agents/scratch/slowdown-hunt/evidence/measure.py before-ram --profile debug --variant 'dev opt1; census and counters; no pricing' --duration 150 --plan .agents/scratch/slowdown-hunt/evidence/ram.ron --samples ''
```

Use `after-shell` and `after-ram` for the after source. The collector takes the machine lock itself, launches only local connections, checks completeness and reaps its children. Never nest its lock. Redirect large diagnostic output under `target/settle-islands/`. The raw logs used here remain there; `summary.json` retains every compact census row, counter totals and end-island summaries.

The native coordinate is nominal loop ticks divided by the fixture's tick rate. Its construction-input helper performs extra app updates outside that loop, identically before and after. Production coordinates are collector wall seconds; result rows exclude shutdown grace. Final-window medians use the last declared window, open at its starting boundary.

`stale-id-mutant.patch` applies to the shipping after source and restores early island-ID capture. Running `tests::avian_island_sleep::an_impact_wakes_an_island_that_merges_again_before_the_solver` then fails before the solver. Remove the mutant before the normal checks. The original stationary-contact and awake-timer witnesses also fail on the before source.

Counter definitions: `contact_requests` counts requests at touching starts, constraint activation and contact loss, before deduplication. `wake_requests` counts requested island IDs across library wake commands. `timer_resets` counts assignments to zero, including already-zero values; after also counts the initiating reset moved into the marker-removal hook. `awake_timer_resets` counts those assignments on already-awake merge members. Counters cover all physics bodies; awake/asleep census columns cover loose pieces.

No tick or physics cost is recorded: the binding lane rules reserve pricing for the orchestrator. Remove all probes and the collector patch before formatting, clippy, tests and committing.
