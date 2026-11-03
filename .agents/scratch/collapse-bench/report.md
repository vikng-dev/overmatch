Subject: the video game's damage model and environment destruction; game state only.

Run `scripts/perf/collapse.sh --duration 60 --bursts 20 --period 1`; add `--sample` for macOS symbols. Builds queue; locked headless runs reap their processes. Off mounts nothing; invalid values refuse startup with `LeverError`.

Decision: lowest map ordinal, first registry-ordered impact-fuzed round, vertical flight above the building's bounds centre. No client/tank. `FireShell::of_round` uses ordinary flight, fuze, deposits and breakage, skipping gun controls, reload, recoil and player transport. Idle fill-up does not gate these laws. Physics and wire unchanged.

Historical MEASURED M4/dev results, ten compute threads: no-rebuild command **60.62 s**, **0.50 s** freshness check; one-line rebuild **1m03s Cargo**, **161.54 s including queue**. **20 bursts**, **53 pieces**, zero clients/tanks. No prolonged one-third-speed incident reproduced; final **29 awake / 24 asleep**, near real time.

Historical DERIVED worst five-second window: **0.634x**, 0.3–5.2 s. Mean period p50: tick **20.50 ms**, physics **0.78 ms**, send **0.11 ms** (outside tick). Sample: `fracture::loose_groups`, `held`, `incident`, `breakage::first_chunk`; sampling perturbs timing.

Review fixes: drain stats at the deadline; overdue final stats fail incomplete instead of reporting healthy earlier data. Malformed deployment JSON/schema refuses retries. A subprocess witnesses production `run` refusing HE input; deleting its installer failed the test and reaped the child. Main closed dev-11/dev-12.

Current gates: formatting, CI clippy, 51 Rust HE/startup/guard/bake tests, 14 script tests. Original headless group: 28 passed, four ignored.

First registered version: **unassigned; none registered**. Orchestrator ships it. Edgegap `deploy ... --he 20:1` is wired; no deployment made.

Artifacts: `target/collapse-*`; table: historical unprofiled period p50s.

| time s | bursts | awake | asleep | speed | tick ms | physics ms | send ms | RSS MiB |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1.8 | 1 | 10 | 0 | 0.18x | 31.22 | 0.82 | 0.16 | 548.5 |
| 3.1 | 2 | 16 | 0 | 0.54x | 23.17 | 0.77 | 0.11 | 238.9 |
| 4.2 | 3 | 29 | 1 | 1.05x | 11.05 | 0.74 | 0.08 | 248.8 |
| 5.2 | 4 | 33 | 0 | 1.00x | 11.07 | 0.79 | 0.07 | 249.5 |
| 6.4 | 5 | 33 | 0 | 0.78x | 12.89 | 0.95 | 0.08 | 333.9 |
| 7.3 | 6 | 32 | 1 | 1.20x | 11.24 | 0.91 | 0.09 | 332.0 |
| 8.4 | 7 | 33 | 1 | 1.00x | 10.84 | 0.94 | 0.08 | 332.6 |
| 9.4 | 8 | 33 | 1 | 0.98x | 10.59 | 0.85 | 0.07 | 332.8 |
| 10.4 | 9 | 33 | 1 | 1.02x | 10.25 | 0.84 | 0.07 | 332.9 |
| 11.5 | 10 | 34 | 1 | 0.97x | 10.23 | 0.84 | 0.07 | 349.9 |
| 12.4 | 11 | 34 | 1 | 1.06x | 10.49 | 0.83 | 0.07 | 350.1 |
| 13.5 | 12 | 34 | 1 | 1.02x | 10.09 | 0.83 | 0.06 | 350.4 |
| 14.6 | 13 | 48 | 1 | 0.85x | 10.29 | 0.83 | 0.06 | 351.3 |
| 15.8 | 14 | 47 | 2 | 1.07x | 12.74 | 0.96 | 0.09 | 351.4 |
| 16.9 | 15 | 47 | 4 | 1.01x | 13.14 | 0.96 | 0.10 | 351.4 |
| 17.9 | 16 | 48 | 3 | 1.02x | 13.21 | 1.05 | 0.10 | 351.5 |
| 18.9 | 17 | 47 | 4 | 1.03x | 12.83 | 1.00 | 0.09 | 351.5 |
| 19.9 | 18 | 47 | 4 | 1.06x | 12.73 | 0.97 | 0.09 | 351.5 |
| 21.0 | 19 | 47 | 5 | 1.03x | 12.71 | 0.95 | 0.09 | 351.5 |
| 21.9 | 20 | 46 | 6 | 1.03x | 12.77 | 0.97 | 0.09 | 351.5 |
| 23.0 | 20 | 46 | 6 | 1.06x | 11.76 | 0.94 | 0.07 | 351.5 |
| 24.0 | 20 | 44 | 8 | 1.02x | 11.22 | 0.90 | 0.06 | 305.7 |
| 25.0 | 20 | 45 | 7 | 1.00x | 11.17 | 0.87 | 0.07 | 305.7 |
| 26.0 | 20 | 47 | 5 | 1.00x | 11.77 | 0.91 | 0.07 | 305.7 |
| 27.0 | 20 | 43 | 9 | 1.02x | 11.14 | 0.88 | 0.07 | 305.7 |
| 28.0 | 20 | 44 | 8 | 1.00x | 11.22 | 0.88 | 0.07 | 305.7 |
| 29.0 | 20 | 46 | 6 | 1.00x | 11.44 | 0.92 | 0.07 | 305.7 |
| 30.0 | 20 | 46 | 6 | 1.02x | 11.76 | 0.95 | 0.07 | 305.7 |
| 31.0 | 20 | 45 | 7 | 1.00x | 11.42 | 0.90 | 0.07 | 305.7 |
| 32.1 | 20 | 46 | 6 | 1.02x | 11.66 | 0.91 | 0.07 | 305.7 |
| 33.0 | 20 | 46 | 6 | 1.02x | 11.72 | 0.91 | 0.07 | 266.7 |
| 34.0 | 20 | 46 | 6 | 1.00x | 11.75 | 0.91 | 0.07 | 266.7 |
| 35.1 | 20 | 46 | 6 | 1.00x | 11.75 | 0.92 | 0.07 | 266.7 |
| 36.0 | 20 | 46 | 6 | 1.02x | 11.78 | 0.91 | 0.07 | 266.7 |
| 37.1 | 20 | 46 | 6 | 1.02x | 11.71 | 0.88 | 0.07 | 266.7 |
| 38.1 | 20 | 45 | 7 | 1.00x | 11.74 | 0.91 | 0.07 | 266.7 |
| 39.1 | 20 | 45 | 7 | 1.00x | 11.66 | 0.92 | 0.07 | 266.8 |
| 40.1 | 20 | 44 | 8 | 1.02x | 11.52 | 0.92 | 0.07 | 266.9 |
| 41.1 | 20 | 44 | 8 | 1.02x | 11.24 | 0.88 | 0.07 | 266.9 |
| 42.1 | 20 | 44 | 8 | 1.00x | 11.04 | 0.87 | 0.07 | 267.0 |
| 43.2 | 20 | 44 | 8 | 1.02x | 11.42 | 0.87 | 0.07 | 267.1 |
| 44.1 | 20 | 42 | 10 | 1.00x | 11.56 | 0.90 | 0.08 | 267.2 |
| 45.1 | 20 | 44 | 8 | 1.02x | 11.71 | 0.92 | 0.08 | 267.2 |
| 46.2 | 20 | 44 | 9 | 1.02x | 11.45 | 0.86 | 0.07 | 267.2 |
| 47.2 | 20 | 45 | 8 | 1.02x | 11.45 | 0.85 | 0.07 | 267.2 |
| 48.2 | 20 | 45 | 8 | 1.02x | 11.72 | 0.87 | 0.07 | 267.2 |
| 49.2 | 20 | 47 | 6 | 1.00x | 11.73 | 0.87 | 0.07 | 267.2 |
| 50.2 | 20 | 44 | 9 | 1.02x | 11.29 | 0.85 | 0.07 | 267.2 |
| 51.2 | 20 | 42 | 11 | 1.02x | 11.11 | 0.85 | 0.07 | 267.2 |
| 52.2 | 20 | 41 | 12 | 1.00x | 10.94 | 0.84 | 0.07 | 267.2 |
| 53.3 | 20 | 42 | 11 | 1.00x | 10.89 | 0.83 | 0.06 | 267.2 |
| 54.2 | 20 | 42 | 11 | 1.02x | 10.79 | 0.81 | 0.06 | 267.2 |
| 55.2 | 20 | 42 | 11 | 1.02x | 10.76 | 0.83 | 0.07 | 267.2 |
| 56.3 | 20 | 41 | 12 | 1.00x | 10.63 | 0.80 | 0.06 | 267.2 |
| 57.2 | 20 | 29 | 24 | 1.00x | 10.47 | 0.78 | 0.07 | 267.2 |
| 58.3 | 20 | 29 | 24 | 1.00x | 8.37 | 0.70 | 0.06 | 267.2 |
| 59.2 | 20 | 29 | 24 | 1.00x | 8.45 | 0.72 | 0.06 | 267.2 |
