Subject: the video game’s environment destruction model and its server cost, game state only.

MEASURED on Apple M4, macOS 26.5.2, 16 GiB, `available_parallelism=10`, compute/async/IO pools `10/1/1`, `dev` game opt1 with opt3 dependencies and `dev_ui,bitprobe`. Every run held the machine lock, headless, muted and unattended, with one real UDP client and no bots. These are local before/after observations; they do not price the hosted two-vCPU server.

The physical model is the baseline at commit subject **net: show observed server tick rate (#279)**. The lane adds clocks, capture and operator input. Construction edits are optional read-only timing-resource parameters and bracket boundaries; parallel owner evaluation remains intact. `server.rs` changes only `spawn_pending_tanks`; the loop is left to tick-clock.

The recipe is `scripts/perf/ram.json`. DERIVED from the loaded tank’s transmission: governed speed 10.509260 m/s; ideal top-gear acceleration distance multiplied by the operator margin gives a 136.600067 m run-up. The server resolves the spawn height from its loaded terrain. The measured plan starts at (-641.434143, 39.625946, 153.384186). No vehicle or construction name selects behavior in code.

Operator choices: `heading_deg` is DERIVED from the selected instance’s authored yaw, rounded for the recipe. `runup_margin=2` is Resolution for approach distance, verified by the speed gate rather than treated as an acceleration prediction. `minimum_speed_fraction=0.95` is the declared full-speed admission tolerance. `drive_s=18` is Resolution for withdrawing throttle around the observed contact, with impact speed independently checked by the admission gate. `settle_s=2` confirms zero awake pieces; `deadline_s=5` bounds the connected observation. Script and deadline seconds are simulation seconds.

The initial 30 s throttle / 120 s observation attempt timed out its real link during the overload; the log establishes the timeout, without isolating which endpoint caused the service gap. It is excluded: the capture subsequently contained zero tanks. The tracked recipe releases input around the ram and closes at the five-second deadline. Both valid runs end with awake pieces; neither proves settlement. The earlier recipe-attestation rejection occurred before tank spawn and is also excluded.

MEASURED contact windows retain the whole first-contact tick through the deadline tick: 321 ticks, 5.015625 simulation seconds, capture ticks 1303–1623 in both runs. P99 is nearest-rank over those raw ticks; period percentiles are never averaged.

| Run | Impact m/s | Tick p50 ms | Tick p99 ms | Worst tick ms | Awake peak / final | Over-budget ticks | End |
|---|---:|---:|---:|---:|---:|---:|---|
| A | 10.823669 | 6.800 | 92.157 | 677.211 | 193 / 145 | 30 / 321 | deadline |
| B | 10.823669 | 7.881 | 94.007 | 682.863 | 193 / 153 | 30 / 321 | deadline |

DERIVED shares below divide summed mechanism wall time by summed clocked tick wall time. Contact readout and sleep are subtracted from the containing Avian bracket before reporting shared physics. The structure share includes owner load preparation, laws, launch geometry and ordered effects; it is not an isolated `fracture::weigh` measurement. Physics serves every body, including the tank, so this instrument does not pretend to isolate loose-piece solver cost.

| Mechanism | A % | B % | Boundary |
|---|---:|---:|---|
| Intake | 4.4527 | 4.3490 | Queue inputs, contact conversion, deposit geometry casts |
| Owner laws and release | 69.3032 | 70.9581 | Owner evaluation through ordered effects and prepared departures |
| Collider preparation | 0.1523 | 0.1490 | Reconciliation and carrier partitioning; deferred ECS application/AABB updates remain in other |
| Contact readout / rest cache | 1.4896 | 1.4291 | Solver momentum readout and resting-contact cache, inside physics |
| Shared physics, net | 13.8955 | 12.8572 | Avian step less contact readout and sleep |
| Island sleep | 0.0843 | 0.0661 | Avian Sleeping bracket |
| Ledger publication | 0.0013 | 0.0010 | Construction-ledger diff commands inside the tick |
| Other fixed work | 10.6212 | 10.1905 | Residual, including track work, ECS and engine work outside brackets |

MEASURED send totals are 52.513 / 49.989 ms across the windows. DERIVED send/tick-time ratios are 1.215% / 1.113%. Send is outside the fixed tick and is not added to the table’s 100%; its bracket includes replicon’s scan/serialization through lightyear’s socket flush. The stats line retains p50/p95/p99/max/total per period for each mechanism.

Both worst ticks occur 0.203125 simulation seconds after first contact, at capture tick 1316, with 73 awake pieces. MEASURED owner-stage times are 672.601 / 678.255 ms, DERIVED 99.32% / 99.33% of those ticks. Both windows have 30 ticks over the owner’s 15.625 ms budget. Those overruns are bugs, not an acceptable average.

MEASURED observer costs: 38.0 ns per stage bracket over 100,000 iterations, including the loop, in the `edit` test artifact. The per-tick collector runs after `TickClosed`, outside `tick_us`:

| Run | Collector mean us | Collector p99 us | Collector max us |
|---|---:|---:|---:|
| A | 0.937 | 1.750 | 3.959 |
| B | 0.786 | 1.750 | 4.041 |

MEASURED whole-observer idle observations use one parked real client at the derived spawn. Each sample contains the same 2,048 trace ticks, after an eight-second warmup. Server process CPU comes from OS cumulative CPU time over the observed wall interval. Ordinary stats off/on use ABBA order; the two raw-ram capture observations follow it. Raw capture uses the server’s route pose, ordinary capture the harness pose copied from that plan. All binaries are the same instrumented build.

| Mode | Process CPU, % of one core | Tick p50 us | Tick p99 us |
|---|---:|---:|---:|
| off-a | 33.147 | 1350.792 | 2142.042 |
| on-a | 35.338 | 1416.541 | 2094.667 |
| on-b | 36.668 | 1441.500 | 2405.000 |
| off-b | 34.373 | 1424.666 | 2227.459 |
| ram-on-a | 32.521 | 1295.375 | 2920.500 |
| ram-on-b | 33.384 | 1268.875 | 5120.125 |

DERIVED ordinary-stats mean p50 difference: +41.292 us; off-run spread: 73.874 us. This includes the pre-existing observer. The raw-capture runs have lower mean p50/CPU than the off runs, so these observations do not identify a clean end-to-end incremental price; no negative cost is claimed. Run variability exceeds the ordinary-stats shift; the individual sources of that variability were not isolated. The direct bracket and collector readings above are the resolved costs. Raw row formatting and stdout sit outside `TickClock`; the whole-process CPU observations include them. Hosted overhead still needs its own measurement.

Run the tracked instrument from a leased bay. It builds through the queue and takes the machine lock itself; never nest another lock around it.

```sh
scripts/perf/collapse.sh --target local --ram scripts/perf/ram.json --out target/ram-measure/bounded-a
scripts/perf/collapse.sh --target local --ram scripts/perf/ram.json --out target/ram-measure/bounded-b
```

For an operator-started two-vCPU deployment, with TARGET=IP:PORT, DEPLOYMENT=ID and a directory containing the matching prebuilt client:

```sh
scripts/perf/collapse.sh --target "$TARGET" --deployment "$DEPLOYMENT" --ram scripts/perf/ram.json --bin "$CLIENT_BIN_DIR"
scripts/perf/collapse.sh --ram scripts/perf/ram.json --print-server-env
```

Exact server environment output:

```text
OVERMATCH_RAM={"instance":"house_0001","heading_deg":-3.917,"runup_margin":2.0,"minimum_speed_fraction":0.95,"drive_s":18,"deadline_s":5,"settle_s":2}
OVERMATCH_BOT=0
OVERMATCH_SERVER_STATS_S=1
OVERMATCH_UNATTENDED=1
OVERMATCH_WINDOW=headless
OVERMATCH_MUTE=1
```

CI’s `build-server` invokes the default-feature server build. `net::ram`, its install in `server_stats`, and the spawn hook have no development-feature gate. The original harness pose is also ungated, but it requires the file on the server; the client’s script alone cannot set the server pose. The tracked recipe therefore supplies a server-owned JSON operator lever with this runner as its user. Hosted log polling/backoff/allocation checks are shared with collapse’s existing `ServerLogs` reader. No Edgegap or AWS operation was performed by this lane.

PASS: 24 Python runner tests; 195 targeted Rust/whole-tree guard tests (three pre-existing ignored tests); `cargo fmt --all --check`; CI clippy with `--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`; server/client builds. Guards include `doc_citations`, `ui_ascii`, `net_boundary` and `bake::`. Touched-file backlog lookup found no matching rows.

Raw evidence stays under `target/ram-measure/`: `bounded-a/result.json`, `bounded-b/result.json`, their server/client logs, `bracket-overhead.log`, `overhead/result.json`, `overhead-ram/result.json`, and the gate logs. The temporary overhead drivers there reproduce the idle observations. The invalid long attempt is `baseline-a/`; the pre-spawn attestation rejection is `run-a/`.

Left to the orchestrator: the hosted two-vCPU run, hosted observer-overhead pricing, and a longer settlement observation after the loop-starvation fix. The current report deliberately certifies only the bounded connected windows.
