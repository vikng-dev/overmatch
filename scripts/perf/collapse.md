The video game's environment-destruction instrument uses real match clients and ordinary server simulation. Run `collapse.sh --help` for the full interface. A rehearsal records counts on the local machine; the hosted server supplies pricing evidence.

`rest-1.json`, `rest-3.json`, and `rest-6.json` each fell their named map houses together at 2 wall seconds after combat admission. They schedule no further breaks. Observe for 65 seconds and check `counts.json`: each target must have `falls > 0`, `awake == 0`, `sleeping + frozen > 0`, and at least 30 seconds in its unchanged resting suffix. These are sampled per-second observations. `falls` and `dust` count chunks; `awake`, `sleeping` and `frozen` count physical groups (`src/debris/freeze.rs`). Replicated pose carriers do not count as additional matter.

```sh
scripts/perf/collapse.sh --target local --rehearsal   --schedule scripts/perf/rest-1.json --tanks 30 --clients 2 --duration 65   --out target/harness-spread/clients-2
```

Repeat the same recipe with `--clients 4` and `--clients 8` for replication comparisons. Remaining tanks are ordinary bots. Each real client is a separate GPU-less process, with `OVERMATCH_UNATTENDED=1`, `OVERMATCH_WINDOW=headless`; it creates no window. For prebuilt binaries add `--bin /absolute/binary/directory` and set `BEVY_ASSET_ROOT` to this checkout.

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
`sweep_us`. The census precedes the sleep timer's opening bracket explicitly. First-contact
fields describe first fall. `last_impact` records the latest solver point closing faster than the
piece's own linear sleep threshold, against any body, pieces included; waking in place is no impact.
`impact_to_sleep_s` includes any subsequent sliding or rolling and is not a visibly-still wait.

`last_moved` tracks a held pose anchor: translation plus rotation angle times the piece's bounding
radius must exceed the labelled visibility tolerance before the anchor refreshes. The radius
encloses all scaled colliders and their offsets at first observation. `thresholds.visible_motion_m`
reports that instrument's tolerance. Per-piece `still_s` is sleep minus last-moved in seconds;
`islands_at_sleep.still_s` is its members' minimum. Every sleep cohort participates, including
pieces that never carried ground load (`never_ground_loaded`) or impacted (`never_impacted`).
`below_sleep_speed_s` is a separate velocity proxy, not a visibility measure.

`piece_census.episodes` preserves completed sleep episodes across wakes. Pose anchors persist
through sleep and wake, so repeated episodes' stillness can include earlier sleeping time; use
`sleeps == 1` for first-sleep observations. `casts_per_source` retains each source's cast count
summed over all its owners, including zero-cast sources.
On Linux, `linux_host.host_probe_us` exposes the periodic `/proc` walk outside `frame_us`.
Lifecycle observations between fixed ticks are reported as `piece_census.lifecycle_outside_tick_us`,
separate from additive tick leaves. `awake_ticks` remains a piece's cumulative awake duration;
completed episodes retain their own immutable snapshots.

Each ram row's `hull` is the tank after that tick's physics step, and what moved it along its own
forward over the step (`src/net/ram.rs`, `HullRow`): solver momentum by the other body's kind and
the sweep's exchange, N s, and the sweep's verdicts on the cuts the hull met. The belts' implicit
support and traction and gravity are not channels, so the channels do not sum to m·Δv.
`impact.speed_m_s` is the horizontal speed of the row before first contact. `result.json` adds
`min_forward_m_s`, `largest_forward_change_m_s`, `crossing_s` and `stall_s`, from the contact tick
on, each defined in its `definitions`. The server's last line adds `structure`, the struck
construction at the capture's end (`src/net/ram/structure.rs`): the share of its volume built above
grade still in place (every corner within the recipe's `in_place_m`, a labelled `Unit`), the
volume-weighted displacement of what still stands on a body, the opening in `plan.panel` (the
component the approaching tank meets first, chosen before play), and the largest loose piece.

`ram-hall-wall.json` and `ram-hall-gable.json` are the same pass through the village hall
(`village_hall_0001`, 12 × 16 m): face-on through the middle of its west long wall, and end-on
through its north gable at the doorway. The run-up is laid from the instance's origin, so the
hall's walls stand 6 and 8 m inside it, and the long wall's approach climbs 0.9 m on the way: under
`ram.json`'s margin of 2 the hull met the wall at 9.87 m/s, short of the 9.98 m/s admission
(MEASURED, one local run), so both recipes state 3, with the approach time that takes. Their
deadline is 10 s, for a building 12 and 16 m through.

`--clip PLAN`, local `--ram` only, is `scripts/debris-lab`'s: the driving client renders the
capture plan's frames on a hidden window, in the run's folder (the plan's relative paths land
there) and with its own settings folder. Its frames open on the tank's approach to the recipe's
construction (`dev_capture`, `Anchor::Approach`), and the run completes only once the client logs
`capture: N of N frames saved`. Its tick times compare only with another clip run of the same plan
on the same machine.
