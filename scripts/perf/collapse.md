The video game's environment-destruction instrument uses real match clients and ordinary server simulation. Run `collapse.sh --help` for the full interface. A rehearsal records counts on the local machine; the hosted server supplies pricing evidence.

`rest-1.json`, `rest-3.json`, and `rest-6.json` each fell their named map houses together at 2 wall seconds after combat admission. They schedule no further breaks. Observe for 65 seconds and check `counts.json`: each target must have `falls > 0`, `awake == 0`, `sleeping > 0`, and at least 30 seconds in its unchanged sleeping suffix. These are sampled per-second observations. `falls` and `dust` count chunks; `awake` and `sleeping` count physical groups. Replicated pose carriers do not count as additional matter.

```sh
scripts/perf/collapse.sh --target local --rehearsal   --schedule scripts/perf/rest-1.json --tanks 30 --clients 2 --duration 65   --out target/harness-spread/clients-2
```

Repeat the same recipe with `--clients 4` and `--clients 8` for replication comparisons. Remaining tanks are ordinary bots. Each real client is a separate GPU-less process, with `OVERMATCH_UNATTENDED=1`, `OVERMATCH_WINDOW=headless`, and `OVERMATCH_MUTE=1`; it creates no window. For prebuilt binaries add `--bin /absolute/binary/directory` and set `BEVY_ASSET_ROOT` to this checkout.

`counts.json` retains bytes and covered wall seconds per server link, whether that link appeared in every admitted stats window, and update-frame counts for every client. Bytes/s is bytes divided by exact covered seconds, including encrypted netcode overhead and excluding IP/UDP headers; it counts queued sends rather than acknowledged delivery. Client frames come from the existing `SPIKE_FRAME_COST` recorder. The two boundary seconds are omitted for admission skew and buffered output. A client keeps up when at least 90% of its frames fit within one fixed tick and its frames occupy at least 90% of the 64 Hz wall-clock slots. `all_clients_keeping_up` records that gate. Run validity describes test fairness: clients must keep up, every link must appear in every admitted window, and the harness must observe complete, compatible data. A failure retains artifacts, prints `INVALID RUN` (or the observation failure), and exits nonzero. Exclude these unfair captures from workload count comparisons.

A server falling behind is a measured result. The saved `capacity` object keeps `valid` true when the test is fair, and separately records `server_over_budget`, `server_keeping_up`, and measured `tick_hz`. The console prints a `MEASURED server_over_budget=...` line with that rate; a fair slow-server run prints `COMPLETE` and exits zero. Only the multi-client population-capacity comparison uses `capacity_supported`: it requires both fairness and the server sustaining its fixed rate over complete windows (one boundary tick allowed). A false `capacity_supported` does not invalidate workload counts. Ram captures use the same fairness and server-performance reporting, omitting pre-capture and admission-straddling server periods. Server periods retain their full precision for the rate calculation.

Every armed `server_stats` period includes `send_clients` entries (`link`, `bytes`, `bytes_per_s`), aggregate `send_bytes`, `send_us`, and `send_us_per_client_s`. The last field amortizes shared send work over connected clients and wall seconds; it is not an independently timed path per client. The byte collector is timed as `send_census_us` and excluded from `send_us`. Without the stats lever, none of those collection systems is installed.

For an operator-started hosted match, print its environment first, then run the same client population. Replace `HOSTED_ADDRESS` and `DEPLOYMENT_ID` below with the existing deployment's values. These commands neither create nor stop a deployment.

```sh
scripts/perf/collapse.sh --target HOSTED_ADDRESS --deployment DEPLOYMENT_ID   --schedule scripts/perf/rest-1.json --tanks 30 --clients 8 --duration 65   --print-server-env

BEVY_ASSET_ROOT="$PWD" scripts/perf/collapse.sh   --target HOSTED_ADDRESS --deployment DEPLOYMENT_ID   --schedule scripts/perf/rest-1.json --tanks 30 --clients 8 --duration 65   --bin "$PWD/target/debug" --out "$PWD/target/harness-spread/hosted-8"
```

The operator passes the printed environment to the hosted server. It selects 22 bots for 8 clients (DERIVED: 30 total minus 8 real players). Repeat with the other client counts and generate the matching environment each time.

The contact census snapshots the completed physics step before sleeping and structural retirement.
`contact_census_us` and `sweep_census_us` are subtractable tick leaves; the latter is excluded from
`sweep_us`. First-contact fields describe first fall; `last_ground`/`last_loaded` describe subsequent
support transitions. `piece_census.episodes` preserves completed sleep episodes across wakeups,
and `islands_at_sleep` uses those episodes' last loaded support transition. `casts_per_source`
retains each source's cast count summed over all its owners, including zero-cast sources.
On Linux, `linux_host.host_probe_us` exposes the periodic `/proc` walk outside `frame_us`.
Lifecycle observations between fixed ticks are reported as `piece_census.lifecycle_outside_tick_us`,
separate from additive tick leaves. `awake_ticks` remains a piece's cumulative awake duration;
completed episodes retain their own immutable snapshots.
