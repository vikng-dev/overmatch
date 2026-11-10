Subject: server performance instruments of a video game. Game state only.

Lane `phase-cpu`, based on `02d77439`, records process CPU beside every exclusive fixed-tick phase and leaf wall interval. Both clocks debit nested sweep census time and overlapping belt readers exactly once. The phase/leaf clock remains installed only when `OVERMATCH_SERVER_STATS_S` is armed. Tick rows carry `phases_cpu_us`, `stages_cpu_us` and `tick_cpu_us`; period rows summarize the same measurements. The reader validates matching CPU/wall keys and conservation of tick CPU.

Decision: use existing libc 0.2.189 without a dependency change. Linux uses `clock_gettime(CLOCK_PROCESS_CPUTIME_ID)`, whose [documented scope is all process threads](https://www.man7.org/linux/man-pages/man2/clock_gettime.2.html). macOS uses `getrusage(RUSAGE_SELF)` directly: [Apple's implementation](https://raw.githubusercontent.com/apple-oss-distributions/Libc/main/gen/clock_gettime.c) implements its process clock through that same call. macOS CPU resolution is microseconds. Wall and CPU edges are sequential reads, so ratios have clock-edge skew; background process threads are included.

MEASURED on this Apple M4/macOS rehearsal host: an isolated 100,000-read observation in the edit profile averaged **0.182 us/read**. Linux read latency was not measured. DERIVED from the clock wiring: each tick adds **22 phase-boundary reads + 2 per leaf invocation**; the capture records the actual count as `work.process_cpu_reads`. Nested crumble details remain wall-only diagnostics inside their containing leaf, not additive phases.

`quota_used_pct` divides all process CPU percent by quota cores, using the existing cgroup quota reader; uncapped, unavailable, invalid and non-Linux denominators have typed absences. Existing cgroup `nr_throttled`/`throttled_usec` period deltas remain intact. Touching `server_host.rs` also closed verified lens backlog row `net-08`: an OS failure to read available parallelism now visibly refuses startup rather than inventing one core.

Decision: retain **one-thread estimates**, explicitly separate from measured durations. DERIVED formula: `wall * clamp(2 - cpu / wall, 0, 1)`, evaluated as `clamp(2 * wall - cpu, 0, wall)` to include zero wall. It equals actual one-thread wall only when exactly one or two threads run throughout; waits, background work and wider pools violate that premise. The tick period summary sums per-tick estimates; the top three exclusive phase/leaf estimates use each period's totals. They need not sum to the whole-tick estimate. This local host has ten compute workers, so its clamped zeros **do not establish an absence of serial work**.

Before/after instrument counts (DERIVED from the schema): **0 -> 21 CPU-timed phases; 0 -> 13 CPU-timed leaves**. No paired baseline gameplay run was taken, so no performance or gameplay-count delta is claimed. MEASURED after: **3,624 captured ticks**, **60 complete report windows**, **30 tanks**, **one real client**, **three scheduled events**, **all 12 targets with recorded departures**. CPU reads/tick: **358 minimum / 384.517660 mean / 528 maximum**, **1,393,492 total**. All 63 emitted period rows had matching phase CPU/wall keys. The shared reader accepted every tick's CPU/wall keys and CPU total. **Zero ticks exceeded one second; longest tick 130.991 ms**, so the known track-contact rerun condition did not occur. The client kept up; the server did not meet its capacity budget in this unpriced local run. Input-arrival mismatch diagnostics were present in the logs; the run completed with its link/population/combat checks intact.

A player gets no gameplay, quality or presentation change. Arming stats adds the clock reads and accounting inside the measured tick; their hosted cost remains unpriced. With stats off, these process phase/leaf reads do not run.

Validation: formatting; CI clippy (`--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`); targeted cost (22), server_stats (16), server_host (3), ram (17), doc_citations (3), ui_ascii (4), net_boundary (1), net::protocol (28); Python collapse reader (42). All passed. A final combined pass covers those same Rust filters after the final comment clarification. The native linker reports its oversized unwind-table warning; clippy is clean. The full suite is left to CI under the lane rules. No collider/body change required `headless_test`.

Exact completed rehearsal command (headless, unattended, muted):

```sh
cd /Users/Yan/github/vikng-dev/overmatch/.claude/worktrees/bay-2
OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1 scripts/perf/collapse.sh --rehearsal --schedule scripts/perf/match-collapse.json --tanks 30 --clients 1 --duration 60 --out target/phase-cpu-rehearsal
```

Raw evidence is under `target/phase-cpu-rehearsal/`: `rows.json`, `tick-attribution.json`, `counts.json`, and the server/client logs. No temporary binary symlink directory was used. Hosted pricing remains with the orchestrator; the exact invocation, with its server/deployment environment supplied, is:

```sh
scripts/perf/collapse.sh --target "$SERVER" --deployment "$DEPLOYMENT" --schedule scripts/perf/match-collapse.json --tanks 30 --clients 1 --duration 60
```

Full new table, pasted from the rehearsal. Tick means are MEASURED; busy threads and one-thread estimates are DERIVED as described above. Quota and throttling are unavailable on this non-Linux host.

| end s | tick mean ms | busy threads | quota used | throttled periods | one-thread estimate ms/tick | top 3 phase estimates ms/tick |
|---:|---:|---:|---:|---:|---:|---|
| 1.21 | 3.325 | 2.55 | n/a:not_linux | n/a | 0.000 | collider_poses 0.235, servos 0.127, physics_prepare 0.097 |
| 2.21 | 3.514 | 2.48 | n/a:not_linux | n/a | 0.000 | collider_poses 0.273, servos 0.126, physics_prepare 0.104 |
| 3.22 | 3.505 | 2.54 | n/a:not_linux | n/a | 0.000 | collider_poses 0.287, servos 0.129, physics_prepare 0.101 |
| 4.22 | 3.426 | 2.52 | n/a:not_linux | n/a | 0.000 | collider_poses 0.286, servos 0.124, physics_prepare 0.108 |
| 5.24 | 3.389 | 2.58 | n/a:not_linux | n/a | 0.000 | collider_poses 0.281, servos 0.126, physics_prepare 0.097 |
| 6.25 | 3.431 | 2.60 | n/a:not_linux | n/a | 0.000 | collider_poses 0.282, servos 0.128, physics_prepare 0.102 |
| 7.25 | 3.386 | 2.60 | n/a:not_linux | n/a | 0.000 | collider_poses 0.256, servos 0.125, physics_prepare 0.097 |
| 8.27 | 3.426 | 2.60 | n/a:not_linux | n/a | 0.000 | collider_poses 0.273, servos 0.127, physics_prepare 0.098 |
| 9.27 | 4.101 | 2.24 | n/a:not_linux | n/a | 0.795 | collider_poses 0.345, servos 0.134, physics_prepare 0.128 |
| 10.30 | 6.710 | 2.34 | n/a:not_linux | n/a | 1.042 | construction_ecs 1.264, collider_poses 0.281, servos 0.122 |
| 11.30 | 8.917 | 2.92 | n/a:not_linux | n/a | 0.000 | collider_poses 0.355, contact_census 0.154, child_collider_writeback 0.149 |
| 12.30 | 8.183 | 2.72 | n/a:not_linux | n/a | 0.000 | collider_poses 0.305, physics_prepare 0.189, child_collider_writeback 0.126 |
| 13.32 | 8.893 | 2.58 | n/a:not_linux | n/a | 0.000 | collider_poses 0.295, physics_prepare 0.189, contact_census 0.136 |
| 14.33 | 5.192 | 2.74 | n/a:not_linux | n/a | 0.000 | collider_poses 0.266, servos 0.123, child_collider_writeback 0.100 |
| 15.33 | 5.015 | 2.57 | n/a:not_linux | n/a | 0.223 | collider_poses 0.294, damage_consequences 0.170, physics_mass 0.145 |
| 16.33 | 4.431 | 2.65 | n/a:not_linux | n/a | 0.010 | collider_poses 0.253, servos 0.138, child_collider_writeback 0.097 |
| 17.35 | 4.010 | 2.83 | n/a:not_linux | n/a | 0.000 | collider_poses 0.230, servos 0.121, child_collider_writeback 0.090 |
| 18.36 | 4.062 | 2.81 | n/a:not_linux | n/a | 0.000 | collider_poses 0.244, servos 0.118, child_collider_writeback 0.090 |
| 19.38 | 4.018 | 2.85 | n/a:not_linux | n/a | 0.000 | collider_poses 0.232, servos 0.123, child_collider_writeback 0.090 |
| 20.39 | 9.904 | 2.36 | n/a:not_linux | n/a | 2.601 | construction_ecs 3.260, collider_poses 0.300, physics_mass 0.136 |
| 21.39 | 18.874 | 2.71 | n/a:not_linux | n/a | 1.291 | construction_ecs 2.720, collider_poses 0.474, contact_census 0.354 |
| 22.40 | 13.954 | 2.96 | n/a:not_linux | n/a | 0.000 | collider_poses 0.436, contact_census 0.305, child_collider_writeback 0.222 |
| 23.40 | 9.162 | 2.80 | n/a:not_linux | n/a | 0.000 | collider_poses 0.321, child_collider_writeback 0.142, servos 0.126 |
| 24.42 | 6.222 | 2.94 | n/a:not_linux | n/a | 0.000 | collider_poses 0.297, servos 0.123, child_collider_writeback 0.118 |
| 25.43 | 5.547 | 2.88 | n/a:not_linux | n/a | 0.027 | collider_poses 0.282, servos 0.120, physics_mass 0.119 |
| 26.43 | 6.603 | 2.51 | n/a:not_linux | n/a | 1.401 | collider_poses 0.526, driving 0.300, physics_mass 0.142 |
| 27.45 | 6.490 | 2.72 | n/a:not_linux | n/a | 0.000 | collider_poses 0.430, construction_ecs 0.138, servos 0.134 |
| 28.45 | 5.684 | 2.98 | n/a:not_linux | n/a | 0.077 | collider_poses 0.290, physics_mass 0.120, child_collider_writeback 0.117 |
| 29.45 | 5.782 | 2.89 | n/a:not_linux | n/a | 0.073 | collider_poses 0.296, servos 0.125, child_collider_writeback 0.119 |
| 30.50 | 16.027 | 2.29 | n/a:not_linux | n/a | 4.219 | construction_ecs 5.424, recorded_consequences 0.476, collider_poses 0.413 |
| 31.51 | 45.197 | 2.28 | n/a:not_linux | n/a | 8.812 | construction_ecs 20.359, contact_census 0.479, collider_poses 0.439 |
| 32.52 | 20.830 | 3.03 | n/a:not_linux | n/a | 0.000 | collider_poses 0.500, intake 0.493, contact_census 0.418 |
| 33.52 | 20.652 | 3.07 | n/a:not_linux | n/a | 0.000 | collider_poses 0.426, intake 0.411, contact_census 0.366 |
| 34.53 | 24.808 | 2.97 | n/a:not_linux | n/a | 0.000 | collider_poses 0.595, contact_census 0.479, child_collider_writeback 0.304 |
| 35.53 | 28.259 | 2.87 | n/a:not_linux | n/a | 0.000 | collider_poses 0.540, contact_census 0.526, intake 0.356 |
| 36.54 | 22.479 | 2.93 | n/a:not_linux | n/a | 0.000 | collider_poses 0.550, contact_census 0.424, child_collider_writeback 0.271 |
| 37.55 | 22.495 | 2.84 | n/a:not_linux | n/a | 0.000 | collider_poses 0.527, contact_census 0.413, child_collider_writeback 0.264 |
| 38.56 | 19.786 | 2.85 | n/a:not_linux | n/a | 0.000 | intake 0.451, collider_poses 0.432, contact_census 0.315 |
| 39.57 | 20.525 | 2.80 | n/a:not_linux | n/a | 0.000 | collider_poses 0.406, contact_census 0.347, intake 0.286 |
| 40.57 | 19.550 | 2.84 | n/a:not_linux | n/a | 0.000 | collider_poses 0.438, contact_census 0.348, child_collider_writeback 0.240 |
| 41.57 | 14.369 | 2.94 | n/a:not_linux | n/a | 0.000 | collider_poses 0.368, child_collider_writeback 0.212, contact_census 0.211 |
| 42.58 | 14.077 | 2.97 | n/a:not_linux | n/a | 0.000 | collider_poses 0.338, child_collider_writeback 0.210, contact_census 0.200 |
| 43.60 | 14.869 | 2.94 | n/a:not_linux | n/a | 0.000 | collider_poses 0.373, child_collider_writeback 0.217, contact_census 0.206 |
| 44.61 | 14.591 | 2.98 | n/a:not_linux | n/a | 0.000 | collider_poses 0.310, child_collider_writeback 0.208, contact_census 0.198 |
| 45.62 | 14.523 | 2.99 | n/a:not_linux | n/a | 0.007 | collider_poses 0.337, child_collider_writeback 0.207, intake 0.201 |
| 46.62 | 14.035 | 3.00 | n/a:not_linux | n/a | 0.000 | collider_poses 0.356, child_collider_writeback 0.211, contact_census 0.201 |
| 47.62 | 14.045 | 3.03 | n/a:not_linux | n/a | 0.000 | collider_poses 0.339, child_collider_writeback 0.212, contact_census 0.199 |
| 48.64 | 14.087 | 2.97 | n/a:not_linux | n/a | 0.000 | collider_poses 0.356, intake 0.225, child_collider_writeback 0.212 |
| 49.65 | 13.281 | 3.05 | n/a:not_linux | n/a | 0.000 | collider_poses 0.315, child_collider_writeback 0.200, physics_mass 0.174 |
| 50.65 | 13.321 | 2.99 | n/a:not_linux | n/a | 0.014 | collider_poses 0.352, intake 0.217, construction_ecs 0.201 |
| 51.66 | 13.477 | 3.00 | n/a:not_linux | n/a | 0.000 | collider_poses 0.399, child_collider_writeback 0.206, contact_census 0.194 |
| 52.67 | 14.493 | 2.90 | n/a:not_linux | n/a | 0.314 | intake 0.385, collider_poses 0.349, field_walk 0.266 |
| 53.68 | 11.514 | 3.12 | n/a:not_linux | n/a | 0.000 | collider_poses 0.343, child_collider_writeback 0.190, construction_ecs 0.181 |
| 54.70 | 6.547 | 3.72 | n/a:not_linux | n/a | 0.000 | collider_poses 0.347, physics_mass 0.152, child_collider_writeback 0.146 |
| 55.71 | 6.802 | 3.61 | n/a:not_linux | n/a | 0.189 | collider_poses 0.359, intake 0.275, physics_mass 0.154 |
| 56.71 | 6.406 | 3.79 | n/a:not_linux | n/a | 0.000 | collider_poses 0.343, physics_mass 0.152, child_collider_writeback 0.147 |
| 57.73 | 6.428 | 3.80 | n/a:not_linux | n/a | 0.000 | collider_poses 0.357, physics_mass 0.153, child_collider_writeback 0.147 |
| 58.75 | 6.446 | 3.77 | n/a:not_linux | n/a | 0.000 | collider_poses 0.353, physics_mass 0.154, child_collider_writeback 0.146 |
| 59.76 | 6.405 | 3.77 | n/a:not_linux | n/a | 0.000 | collider_poses 0.356, physics_mass 0.152, child_collider_writeback 0.145 |
| 60.78 | 6.688 | 3.72 | n/a:not_linux | n/a | 0.000 | collider_poses 0.352, physics_mass 0.158, child_collider_writeback 0.147 |
