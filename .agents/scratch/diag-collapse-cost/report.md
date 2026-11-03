# Collapse server cost

Subject: the video game's environment destruction and dedicated-server game state only.

**Cause candidate, moderate confidence for the owner's incident; high confidence locally:**
awake rubble contacts repeatedly invoke structural weight checks over full building graphs;
large loose-piece piles can independently saturate Avian physics.

The owner's log confirms slow simulation with zero tanks through its final timestamp,
12:59:18 UTC (MEASURED); it cannot attribute the cost. The earlier belt-support explanation
cannot explain that empty-server interval. Battle's results recovery remains a different,
unattributed observation: removing tanks removes additional work, but not rubble.

## Measurements

SG: registered `0.7.0-alpha.10`, Singapore EPYC 7713, MEASURED quota/parallelism one core,
compute/async/IO pools 1/1/1, no throttling. Mac: M4, ten compute threads, **dev optimization**;
these timings do not transfer to release. Matching clients were built from the release tag.

MEASURED stats below, summarized as window medians of period p50s; `*` is one observed period.
Offsets are DERIVED from summed printed periods since first pieces. CPU is percent of one core.

| Run | Clients/tanks | Awake/asleep | Hz | CPU % | Tick / physics / send, ms |
|---|---:|---:|---:|---:|---:|
| SG empty | 0/0 | 0/0 | 63.9 | 50.8 | 2.71 / .32 / .20 |
| SG one tank, no shots | 1/1 | 0/0 | 63.9 | 58.3 | 3.63 / .44 / .33 |
| SG real fire +1 min | 1/1 | 2/18 | 63.9 | 66.9 | 5.41 / .79 / .36 |
| SG real fire +10 min | 0/0 | 2/18 | 64.0 | 60.3 | 4.53 / .63 / .23 |
| SG fixture +1 min | 0/0 | 990/10 | 21.7 | 98.0 | 45.38 / 41.66 / .39 |
| SG fixture +5 min | 0/0 | 995/5 | 23.5 | 99.8 | 41.75 / 38.77 / .31 |
| SG fixture +10 min | 0/0 | 990/10 | 23.6 | 99.7 | 41.85 / 38.99 / .36 |
| Mac quiet tank | 1/1 | 0/0 | 63.9 | 32.9 | 1.95 / .33 / .11 |
| Mac native breakage +30 s* | 0/0 | 108/51 | 28.4 | 108.4 | 34.97 / 1.63 / .10 |
| Mac native breakage +3.5 min | 0/0 | 71/97 | 42.2 | 111.5 | 23.50 / 1.37 / .11 |
| Mac fixture, 2,000 pieces | 0/0 | 1929/71 | 64.0 | 256.0 | 12.57 / 11.52 / .19 |

Real projectiles reached a building, verified by its breakage logs, but produced only
MEASURED 20 pieces: **no whole-building remote collapse reproduced**. Their observation spans
DERIVED 625 s. The separate fixture spans DERIVED 641 s; it is not a building-collapse substitute.
Locally, temporary `WallCharge` inputs through the production server plugin released
MEASURED 168 native pieces (MEASURED configuration: four diagnostic inputs, 16 MJ each). Its client stopped before scripted fire. Some pieces slept; the
native run ended after DERIVED 230 s of observation, still slow.

## Named code and options

Mac `sample`, DERIVED inclusive shares of non-wait samples: native
`breakage::apply_deposits` 80–83%, nested `fracture::weigh` 76–79%.
`loose_groups`, `incident`, `held`, sorting and allocation dominate. `Live::group` retains the
whole source graph; `weigh` rebuilds adjacency and load paths on repeated contact deposits.
Physics and sending are minor here. Fixture samples instead put 47% in contact-manifold
queries and 21% in `ContactConstraint::solve`.

Baseline samples show collider-pose/transform maintenance, dirty-tree/AABB scans and scheduler
wakes, plus belt forces and motion-history snapshots. The empty world already consumes most
of the quiet tank's CPU; one-tank cost does not justify a tank-capacity extrapolation.

Smallest preservation-first candidate: cache immutable adjacency and evaluate owned subgraphs,
reusing weight-check scratch storage. Intended player-visible change: none; reprice before
promising real time. Independent-root parallelism could help this serial system, but extra
cores alone do not divide its loop. Extra cores can help the parallel physics fixture; the Mac
comparison demonstrates headroom, not a cross-host speedup measurement.

Reducing fragment resolution is another option: fewer, larger pieces and coarser breakage for
players. Earlier sleeping would stop small motions sooner; neither option has been priced.

Still unexplained: the owner's exact collapse, persistent island activity, reconnect trigger,
and first-match exit. No panic occurred in these runs. No fresh-world reset was measured.

## Evidence and cleanup

Artifacts: `target/diag-collapse-cost/`: `remote-repro-final.log`, `remote-stress-final.log`,
`remote-empty.log`, their metrics, `summary.json`, `verdict.json`, `remote-client-3.log`,
`stress-client.log`, `local-client.log`, `local-server.log`, `local-stress-server.log`,
`local-events.json`, and `sample-*.demangled.txt`.
Temporary targeting/charge changes are recorded in `instrumentation.patch` and reverted.
The cold full-debug profiling build was canceled; dev symbols supplied the Mac profiles.
All local processes stopped; all profile deployments stopped, live profile count zero.

Validation: formatting, CI clippy, and MEASURED 48 required guard/bake tests passed. No game-code change remains.
