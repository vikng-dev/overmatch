# Night measurements, 2026-10-31 (#186, #106)

What the dedicated server costs, per system and per moving wall piece. The pieces belong to the
game's environment destruction: game state only. Run on an Apple M4, 01:20–07:40 +07, owner away;
every local run held the machine lock. Labels: MEASURED, DERIVED, ASSUMED.

## TLDR

1. **Tick against moving (live) pieces, 29 bots.** Each cell is the window median of tick p50 /
   p95 in ms, the mean of a palindrome's two runs (MEASURED; the one-run cell is marked). The live
   column is the condition's median live count.

   | Live | M4 local, alpha.3, 1 client | Live | Edgegap 1 vCPU, alpha.4, no client | Edgegap 2 vCPU, alpha.4, no client | 2 vCPU, `OVERMATCH_SERVER_THREADS=4`, no client |
   |---|---|---|---|---|---|
   | 0 (lever off) | **4.52 / 5.12** | 0 | **18.5 / 23.8** | **17.5 / 20.6** | **13.4 / 15.2** |
   | ~350 | 7.41 / 8.31 | ~255 | 25.8 / 30.9 | 25.3 / 28.5 | 22.0 / 25.0 (~274 live) |
   | ~1190 | 9.53 / 10.26 | ~935 | 35.5 / 42.0 | 36.8 / 41.4 | 31.9 / 36.5 |
   | ~2490 | 11.88 / 13.07 | ~1545 | 37.1 / 43.6 | 35.1 / 40.1 (one run) | 33.0 / 37.0 (~1630 live) |
   | ~5080 | 14.51 / 16.01 | | | | |

   - The budget is 15.6 ms.
   - Both Edgegap sizes are over it at zero debris with Bevy's default pools. Their simulation then
     runs at 47–53 Hz instead of 64.
   - At ~1500 live the Edgegap simulation runs at ~25 Hz. The server stops keeping time, so the
     curve flattens there.
2. **Cost per moving piece (DERIVED from the condition medians, tick p50):**

   | Host | Range | µs per live piece |
   |---|---|---|
   | M4 | 350–1190 live | **2.5** |
   | M4 | 1190–2490 live | 1.8 |
   | M4 | 2490–5080 live | 1.0 |
   | M4, least squares | 350–5080 live (armed levels) | 1.45 |
   | Edgegap 1 vCPU | 0–935 live | **18** |
   | Edgegap 1 vCPU | 255–935 live | 14 |
   | Edgegap 2 vCPU | 0–950 live | 20 |
   | Edgegap 2 vCPU, threads=4 | 0–935 live | 20 |

   - **Edgegap pays 6–7× the M4 per piece.** The M4's first step, off to ~350, is 8.2 µs per piece:
     it also carries ~460 frozen pieces and the start of the frozen cap.
   - **The M4 curve is concave** (`local-curve/regress.txt`, per-second bins). Many "live" pieces
     are rubble that tanks keep waking rather than falling pieces: ASSUMED, from the `thawed`
     counter's 400–500 wakes per second. Why each piece gets cheaper as they multiply is
     unmeasured.
3. **Thread lever, and 1 against 2 vCPU** (alpha.4, MEASURED):
   - **The second vCPU is idle with the defaults.** On 2 CPUs Bevy sizes the pools
     compute/async/io = 1/1/1. The server holds ~102 % of one core, and 2 vCPU equals 1 vCPU (17.5
     against 18.5 ms).
   - **Default against 1 thread, 2 vCPU, with the client, A/B/B/A:**
     - zero debris: 1 thread 17.7 ms, default 19.8 ms, so 1 thread is faster by 2.1 ms;
     - at 12:2 and 50:2 the two are within noise (25.5 against 26.0, 23.8 against 23.6 ms).
   - **`OVERMATCH_SERVER_THREADS=4` on 2 vCPU** gives pools 2/1/1:
     - zero debris: tick p50 13.4 ms, p95 15.0–15.3, 64 Hz, 140 % CPU, physics p50 4.3 against
       8.0 ms;
     - this is the first Edgegap shape that holds 30 tanks in budget, with no headroom at p95;
     - it saves 2–5 ms of p50 at every level: 4.1 at zero, 3.3 at ~260 live, 4.9 at ~940 and 2.1
       at ~1600 (DERIVED, against the default-pool client-less curve).
   - **`OVERMATCH_SERVER_THREADS=6` on 2 vCPU** (pools 2/2/2, zero debris, 2 runs,
     `edgegap-2vcpu-noclient-t6/`) is worse than 4: tick p50 14.6–15.2 ms, p95 16.6–16.8, 61–62 Hz.
     So 4 is the value to use.
   - **The same lever on 1 vCPU is worse** (zero debris, 2 runs, `edgegap-1vcpu-noclient-t4/`):
     - p50 falls to 14.2–14.5 ms, but p95 rises to 88–89 ms;
     - the simulation drops to 46 Hz;
     - the CPU quota throttles the process 441–472 ms every second, against 23 ms with the
       defaults (`throttled_usec`).

     On one vCPU, keep one compute thread.
4. **The measured client cannot stay on a debris-loaded Edgegap server.**
   - It exited early at every armed level but one (2 vCPU, 12:2, default threads), on both
     sizes. Not a timeout: each of the 13 early exits is bevy_render quitting on a wgpu
     validation error a few frames after a bot respawn wave (#192; side findings).
   - The server curves in (1) therefore come from client-less deployments: the simulation alone,
     with no replication send. Send to one link was 0.1–1.8 ms on the M4 (off to ~5000 live)
     and ~0.4 ms on Edgegap at zero debris.
5. **Profile headline** (`server-profile.md`: alpha.3, M4, 30 tanks):
   - untraced tick p50 4.46 ms;
   - `track::sim::apply_track_forces` takes 26.7 % of busy time, +83 µs per tank, in one serial
     loop;
   - physics takes 38.5 %; bots 0.57 %;
   - **volley hitches of 20–200 ms come every ~3 s** from `integrate_projectiles` and
     `apply_deposits`.

6. **alpha.5 (the #186 tick cuts) on Edgegap, 30 bots, no client, A/B/B/A:**
   - 1 vCPU: tick p50 21.3 → 15.2 ms (−29 %);
   - 2 vCPU with `OVERMATCH_SERVER_THREADS=4`: 16.0 → 10.9 ms (−32 %), p95 13.0, 64 Hz.

   That shape now holds 30 tanks in budget (MEASURED; section below).

## What ran, from which tag

| Part | Tag | Binaries | Where |
|---|---|---|---|
| Server profile | v0.7.0-alpha.3 (`ecdc13e`) | profiling, `dev_ui,bitprobe,bevy/trace_chrome` (`profile/BUILD-alpha3.txt`) | local |
| Local debris curve | v0.7.0-alpha.3 | the same, spans filtered off at their callsites (`RUST_LOG=bevy_ecs=warn,bevy_app=warn`) | local |
| Edgegap curves | v0.7.0-alpha.4 (`1ed1c5d`) | server: the release image `0.7.0-alpha.4-1ed1c5d`. Client: profiling build, same features, spans filtered (`profile/BUILD-alpha4.txt`) | Edgegap Singapore (AMD EPYC 7713) + this Mac's offscreen client |
| alpha.4 server, local check | v0.7.0-alpha.4 | profiling, as above | local |

- **No alpha.3 server was deployed.** Its server package lacked `assets/buildings`, so the server
  panics at boot. Release run 41106272716 failed both smokes.
- **alpha.4 differs from alpha.3** by a server package that carries the buildings and a rebuilt
  Tiger (a two-part hull collider). It is not heavier, MEASURED locally at 30 tanks, default
  threads: tick p50 4198 against alpha.3's 4457 µs, p95 4597 against 5175. With one thread: 4993
  against 5163. So the Edgegap gap is the host, not the release: under pools of one thread the
  same 30-tank tick takes 18.5 ms there against 5.0 ms on the M4, 3.7× (DERIVED).
- **Registered:** Edgegap version `0.7.0-alpha.4-2vcpu`, 2048 vCPU units, max_duration 120, the same
  image (`tools/edgegap-register-2vcpu.sh`, through `scripts/ci/edgegap-version.sh`).
- **No measurement window overlapped a compile.** Every local measurement held
  `/tmp/overmatch-cargo.lock`, and every compile went through `scripts/serial-cargo`, never nested
  in another lockf.
  - **Build A's wrapper:** its shell was killed mid-build to stop a script edit reaching it. Its
    lockf and cargo ran on inside the lock to the end.
  - **The Edgegap client's frames are contaminated.** The Edgegap chains ran without the lock, as
    briefed, and the alpha.4 server build compiled during the 2-vCPU client chain. Its client frames
    are not reported.
  - **The reverse may also hold.** That client chain rendered offscreen on this Mac from 04:07 to
    05:01, without the lock. Another lane's lock hold (`scratchpad/st186/…`) began ~04:46, so its
    first ~15 min shared the Mac with that client (DERIVED from the lock queue's ages). The
    client-less chains loaded only Edgegap.

## The lever, and why "live" is not the brief's 0/64/256/512/1024

- **Conditions** (OVERMATCH_DEBRIS_STRESS, `<pieces>:<period_s>:<live_cap>:<frozen_cap>`):
  - locally, `off`;
  - on Edgegap, the zero is `1:100000:8192:512`: one piece at the first tick and none after, so the
    stats line still carries the debris census;
  - armed levels `12:2`, `50:2`, `100:2` and locally `200:2`, all with live cap 8192 and frozen cap
    512;
  - each chain runs as a palindrome. Warmup is 45 s and the window 180 s.
- **The live count does not settle** (calibration, `calib/`):
  - **Frozen cap 1:** live climbed without bound. Each new freeze despawned the previous frozen
    piece and woke what rested on it.
  - **No frozen cap:** frozen pieces piled up and cost more than live ones. At 200:2 the tick was
    12.3 ms with 1124 live and 3649 frozen, against 6.8 ms with 2152 live and 1 frozen (MEASURED,
    M4).
  - **Frozen cap 512** is the value the runs use.
- **Even at cap 512 the live count climbs inside a window.** At 200:2 locally it went from 1882 to
  7249.
  - **The rubble keeps waking.** The lever drops its breaches beside circling bots, and the
    `thawed` counter rises 400–500 times a second.
  - **The pieces are the lever's own.** Locally at 12:2, live jumped from ~50 to ~260 about 55 s
    in, when the entity count jumped too. But the census-only zero (below) stays at 0 live while
    the bots break houses. The house breakage therefore adds no live pieces of its own, and why it
    wakes the lever's pieces is unmeasured.

  So the lowest armed level holds ~250–350 live, not 64.
- **Bins.** `regress.py` adds a per-second view: every armed second binned by its own live count.
  On the M4:

  | Live (bin median) | 62 | 240 | 371 | 887 | 1413 | 3172 | 6099 |
  |---|---|---|---|---|---|---|---|
  | tick p50, µs | 4578 | 6383 | 7518 | 7966 | 10034 | 13014 | 15503 |

- **The census exists only while the lever is armed.** So the census-only zero `1:100000:8192:512`
  also ran locally (`local-zero/`, 2 runs, 06:00 +07).
  - **It matches `off`.** Tick p50 4332 / 4348 µs, p95 4647 / 4695, live 0, frozen 2, against
    `off` at 4520 / 5124 (MEASURED).
  - **The houses add no live pieces.** The entity count grew 18.8k → 26.5k while the bots broke
    houses, and live stayed 0.
  - **Price of the first armed level.** The step from zero to 12:2 (~350 live, ~460 frozen) is
    3.07 ms, 8.8 µs per live piece if every frozen piece is free (DERIVED).
  - **The runner marks both runs INVALID.** Its gate wants a lever that released pieces inside
    the window, which this zero never does by design. Their stats lines are used as they are.

## Local curve (M4, alpha.3, 29 bots + 1 idle GPU-less client; `local-curve/`)

First run / palindrome repeat, µs, MEASURED. All ten conditions passed the runner's gates.

| Lever | Live (median) | Frozen | tick p50 | tick p95 | physics p50 | send p50 (1 link) | CPU % of a core | Overruns/s (median) |
|---|---|---|---|---|---|---|---|---|
| off | (uncounted) | (uncounted) | 4657 / 4384 | 5525 / 4722 | 1136 / 954 | 120 / 101 | 75 / 67 | 0 |
| 12:2 | 343 / 358 | ~460 | 7271 / 7541 | 8314 / 8300 | 3506 / 3812 | 249 / 246 | 95 / 97 | 0 |
| 50:2 | 1223 / 1153 | ~505 | 10046 / 9019 | 10869 / 9655 | 5979 / 5094 | 508 / 480 | 127 / 115 | 1 |
| 100:2 | 2530 / 2452 | ~507 | 11856 / 11905 | 12928 / 13212 | 7488 / 7425 | 901 / 893 | 150 / 150 | 1 |
| 200:2 | 4998 / 5154 | ~510 | 14614 / 14400 | 16045 / 15973 | 9129 / 9157 | 1680 / 1760 | 164 / 165 | 4.5–6 |

- Physics carries most of the growth.
- Replication to the one link grows ~0.32 µs per live piece (DERIVED). It scales with the link
  count; that it scales linearly is ASSUMED.

## Edgegap (alpha.4, Singapore; `edgegap-*/`)

Each directory has `conditions.tsv`, the per-condition `*.server.json`, a stats-only log and a
`regress.txt`.

| Run | Directory | Conditions | Notes |
|---|---|---|---|
| 1 vCPU, 1 client | `edgegap-1vcpu/` | 3 of 10 | The zero ran VALID: 17.6 / 22.9 ms, 52.6 Hz, client down 6.4 kB/s. At 12:2 and 50:2 the client exited early (#192). The chain then stopped: an HTTP 429 (Edgegap's rate limit) on the log pull read as "0 bots spawned". |
| 1 vCPU, no client | `edgegap-1vcpu-noclient/` | 8 of 8 | The curve in TLDR 1. |
| 2 vCPU, 1 client, threads default / 1 | `edgegap-2vcpu/` | 16 of 16 | The client exited early in every armed condition but `d07-12:2-tdefault` (#192), so most server windows are short (43–131 lines). Down 6.4–6.7 kB/s at zero debris, 27.3 kB/s at ~277 live. |
| 2 vCPU, no client | `edgegap-2vcpu-noclient/` | 7 of 8 | The repeat of 100:2 was stopped mid-window by the client chain's final `stop-all-profile` sweep. |
| 2 vCPU, no client, threads=4 | `edgegap-2vcpu-noclient-t4/`, `-t4b/` | 8 of 8 | Pools 2/1/1. Two palindromes: zero and 50:2, then 12:2 and 100:2. |
| 1 vCPU, no client, threads=4 | `edgegap-1vcpu-noclient-t4/` | 2 of 2 | Zero debris only (TLDR 3). |
| 2 vCPU, no client, threads=6 | `edgegap-2vcpu-noclient-t6/` | 2 of 2 | Zero debris only, pools 2/2/2 (TLDR 3). |

- **Cost:** 55 deployments of ~4 min, the alpha.5 A/B included: 17 at 1 vCPU ($0.069/h) and 38 at
  2 vCPU ($0.138/h), about $0.43 (DERIVED, list price).
- **Stopped:** every `profile-` deployment is stopped, and the live count was confirmed 0 at the
  end.

## Side findings for the owner

- **Damage model, game state only:** "ballistics: union walk failed, stopping the round at
  contact" appears 15–27 times in a few minutes at 30 tanks, on alpha.3 and alpha.4 alike. 9 of 18
  in one run give the reason "a collider reached the world at a scale other than 1".
- **The client's early exits were a render error, not a timeout (#192, MEASURED from the
  `edgegap-*` client logs).** All 13 end in `Quitting the application due to Validation
  RenderError` (`pbr_prepass_pipeline`: vertex input `Location[0]` not provided), exit status 0.
  Each follows, in the same frame, one bevy_render "Use-after-free" line per empty mesh: the track
  shadow-proxy ribbons a respawn wave spawns empty until their first belt fit
  (`src/track/view.rs`, `attach_shadow_proxies`). Only debris-armed runs show it. One run
  (`d05-…-t1`) also lost its link first: a 3468 ms client frame, past netcode's 3 s timeout,
  then a reconnect. Reproduced locally (stall lever plus debris lever): the same quit on the
  first respawn wave.
- **The debris rule churns.** `wake_ahead` compares every frozen piece with every moving body each
  tick (`src/debris.rs`, `wake_ahead`). Frozen pieces were MEASURED expensive in the no-cap
  calibration above.

## Products

- `server-profile.md` and `profile/`: the per-system profile, item 1 of the brief.
- `local-curve/`: the M4 debris curve.
- `local-zero/`: the census-only zero on the M4.
- `edgegap-*/`: the Edgegap curves above.
- `calib/`: the lever calibration.
- `ab-alpha4-alpha5-*/`: the release A/B on Edgegap.
- `battery/`: the two battery blocks' outputs.
- `tools/`: the runners and analysis scripts.

## alpha.4 against alpha.5 on Edgegap (the orchestrator's late ask)

The #186 server-tick cuts landed in v0.7.0-alpha.5 (`cc8682d`, release run 41142524061, green).
Setup:

- **Deployments:** 30 bots, no client, debris lever off, A/B/B/A, 07:06–07:24 +07, each
  deployment its own `profile-` id, all stopped and the live count confirmed 0.
- **Products:** `ab-alpha4-alpha5-*/`, run by `tools/clientless-ab.sh`.
- **Registered:** `0.7.0-alpha.5-2vcpu`, 2048 units, max_duration 120, the same image
  `0.7.0-alpha.5-cc8682d`.

The cells are window medians, MEASURED: run 1 / run 2, ms.

| Host | Release | tick p50 | tick p95 | physics p50 | sim rate | Overruns/s |
|---|---|---|---|---|---|---|
| 1 vCPU, default pools | alpha.4 | 21.19 / 21.50 | 27.06 / 26.68 | 10.48 / 10.31 | 43 Hz | 48 |
| 1 vCPU, default pools | alpha.5 | **15.20 / 15.18** | 21.76 / 21.71 | 4.23 / 4.24 | 59 Hz | 26–27 |
| 2 vCPU, `OVERMATCH_SERVER_THREADS=4` | alpha.4 | 16.03 / 16.05 | 18.65 / 19.20 | 5.52 / 5.65 | 57–58 Hz | 43–44 |
| 2 vCPU, `OVERMATCH_SERVER_THREADS=4` | alpha.5 | **10.94 / 10.93** | **13.05 / 12.91** | 3.56 / 3.57 | 64 Hz | 1 |

- **1 vCPU:** alpha.5 is −6.15 ms p50 (−29 %) and −5.14 p95 (DERIVED).
  - p50 now sits just under the 15.6 ms budget. p95 does not, and the simulation still loses 5 Hz.
- **2 vCPU with 4 threads:** alpha.5 is −5.11 ms p50 (−32 %) and −5.95 p95.
  - 30 tanks now hold the budget with ~2.6 ms of p95 headroom, at 64 Hz.
- **Agreement:** the two halves of each A/B agree within 0.3 ms at p50.
- **Not like the curve's zero.** alpha.4 at 30 bots without the lever (21.3 ms) runs slower than
  the curve's 29 bots plus the census-only lever (18.5 ms). Those are other deployments, on hosts
  that differ from one deployment to the next, and with a bot more. So compare within this table
  only.

## The night battery (item 4)

**Ran:** cal-start (05:57–06:07 +07) and battle-30 (06:21–06:46), offscreen, with the alpha.4
client of `profile/BUILD-alpha4.txt`. The trace feature is compiled into that client and its spans
are filtered off, which last night's binary did not carry. Products: `battery/*.out`.

**Not run:**

- `vis-off`: visible windows.
- `prints-*`: the binary lacks `track_prints_demo`.
- duel-2: cancelled at 06:49 while still queued, since it could no longer end by 07:35.
- forest and cal-end.

All 21 conditions passed their gates.

- **cal-start, MEASURED (frame ms, p50 / p95):** cal-2 12.31 / 15.11, cal-30 17.29 / 21.45. Last
  night's cal-start (alpha.1) read 14.45 / 16.02 and 19.32 / 24.26. The two nights cannot be
  compared directly (`perf-2026-10-29/night-results-2026-10-30.md` §2).
- **battle-30, 30 tanks, A/B/B/A.** Δ is the lever's mean against the mean of its two bracketing
  bases; halves are each run against its adjacent base (DERIVED from the MEASURED
  percentiles). Bases ran 16.53–17.05 ms p50.

  | Lever | Δp50 | Halves | Δp95 | Last night's Δp50 |
  |---|---|---|---|---|
  | render scale 50 % | **−6.43** | −6.49, −6.37 | −6.82 | −6.29 |
  | MSAA off | **−3.47** | −3.56, −3.38 | −2.60 | −3.10 |
  | shadows off | **−3.38** | −3.33, −3.42 | −2.11 | −3.05 |
  | sky env light off | **−1.41** | −1.38, −1.43 | −2.87 | −1.20 |
  | cascades 3 → 2 | **−0.43** | −0.41, −0.44 | −1.05 | −0.72 |
  | shadow maps 4096 → 2048 | +0.05 | +0.10, 0.00 | −0.05 | −0.25 |

  The lever order and sizes hold from alpha.1 to alpha.4. The 2048 maps fell below the noise
  floor.
