# Night battery results, 2026-10-30 (#151, #152)

Frame cost and server tick only; no damage-model code. Apple M4, 2560x1440, 01:40–03:44 +07, owner away.
Binary: profiling build of `83fee31` (0.7.0-alpha.1), `bin/night-83fee31`, offline and online alike (CLAIMED, manifests).
All 17 offline parts passed their gates (`runs/night/battery-status.txt`); online 2 and 11 tanks passed, 29 tanks failed (`runs/night.log`).
Convention: per-condition percentiles are MEASURED (analyzer, `scripts/perf/analyze.py`); every Δ is DERIVED from them. A/B/B/A Δ = mean(X-a, X-b) − mean(bracketing bases); "halves" = X-a − base before, X-b − base after.

## TLDR

1. Offscreen measures the real window's frame: Δ ≤ 0.24 ms at p50/p95, 2 and 30 tanks. Use it with no correction. It misses the visible window's ~1 % of ~25 ms frames, so p99/worst still need a visible run.
2. Inside an A/B/B/A block a 0.2 ms p50 Δ is real: halves agree to a median 0.05 ms. Across blocks, or against the daytime runs, it is not: the frame got 0.4–0.5 ms faster between 01:59 and 02:11 and stayed faster (UNEXPLAINED).
3. Savings at p50, 30 / 2 tanks: render scale 50 % −6.3/−5.3, MSAA off −3.1/−2.8, shadows off −3.1/−2.1, bloom off −1.7 (2 tanks only), depth prepass −1.1, sky env light −1.2/−0.9, cascades 3→2 −0.7/−0.5, 2048 maps −0.25/−0.37, UI 0. Trees in view +1.2. Print relief +0.47.
4. The 30-tank frame is GPU-limited at native scale, not CPU-bound as #151 and `frame-attribution.md` say: MSAA off cuts 3.1 ms and half scale cuts 6.3. The CPU floor is ≤ 12.5 ms at 30 tanks and ≤ 9.1 ms at 2, so no GPU lever alone reaches 8.33 ms.
5. Online, 1 vCPU Singapore: the client is +1.6 ms p50 over offline at 2 tanks; at 11 tanks, ~28 ms frames each followed by a ~7 ms one lift p95 to 27–29 ms. Server: +0.34 ms tick per tank, projecting 13.4–13.9 ms p50 at 30 (86–89 % of 15.6). Its core saturates at ~23–26 tanks (DERIVED). Down: ~235 kB/s per client at 30 (DERIVED).

## 1. Visible vs offscreen (`runs/night/vis-off-analysis.out`)

Order vis-a, off-a, off-b, vis-b per tank count, 60 s each. Means of the a/b runs:

| Tanks | Stat | Visible | Offscreen | Off − vis | Halves |
|---|---|---|---|---|---|
| 2 | p50 | 14.55 | 14.68 | **+0.13** | +0.27, +0.00 |
| 2 | p95 | 15.94 | 16.18 | **+0.24** | +0.37, +0.12 |
| 2 | p99 | 21.93 | 16.83 | −5.10 | −3.29, −6.90 |
| 30 | p50 | 19.33 | 19.28 | **−0.06** | −0.08, −0.03 |
| 30 | p95 | 24.13 | 24.29 | **+0.16** | +0.22, +0.11 |
| 30 | p99 | 25.42 | 25.44 | +0.02 | −0.17, +0.22 |

- Two identical visible runs differ by 0.29 ms at p50 (vis-2-a 14.40, vis-2-b 14.69; MEASURED). Every off−vis p50 sits inside that.
- Only visible frames have a slow population, MEASURED: >20 ms frames 45 and 51 visible vs 2 and 4 offscreen, of ~4100 at 2 tanks; >33 ms frames 6 and 7 visible vs 0 and 0 at 30 tanks. The 2-tank ones cluster at 25–26 ms, three 120 Hz refresh periods. That they come from present pacing is ASSUMED.
- **Verdict:** offscreen is a valid proxy for p50 and p95. Correction: none. It reads p95 +0.1–0.4 ms high in all four halves; a −0.2 ms p95 adjustment is optional and below the p95 noise. It is not a proxy for p99/worst or stutter: those need a visible run. The earlier retarget showed 0.1 / 0.7 ms (CLAIMED, `focus-gap-diagnosis.md`); the 30-tank gap is gone.

## 2. Drift and noise floor

| Condition | Day 10-29 (`94f20cf`, owner working) | cal-start 01:52 | cal-end 03:28 | End − start | Start − day |
|---|---|---|---|---|---|
| cal-2 p50 | 14.34 | 14.45 | 14.15 | −0.30 | +0.11 |
| cal-2 p95 | 17.72 | 16.02 | 15.77 | −0.25 | −1.70 |
| cal-30 p50 | 19.31 | 19.32 | 18.91 | −0.41 | +0.01 |
| cal-30 p95 | 24.36 | 24.26 | 23.77 | −0.49 | −0.10 |

- At p50, day and night agree to within 0.11 ms, although the night binary adds the #168 bookkeeping fix (`3258f11`, CLAIMED from git log). A CPU saving hidden behind the GPU would look exactly like this (ASSUMED). The day's p95 at 2 tanks was 1.7 ms worse: owner load.
- **Where the drift sits** (MEASURED, `runs/night/duel-2.out`): duel-2 bases p50 14.65, 14.61 until 01:59, then 14.34, 14.36 (02:03–02:07), then 14.11–14.18 for all 7 later bases (02:11–02:36). The battle-30 bases hold 18.78–18.91 (02:37–03:02), and cal-end matches them. A step of about −0.5 ms, then flat. Cause UNEXPLAINED.
- **Noise:**

| Measure | p50 | p95 |
|---|---|---|
| Adjacent identical bases after the step, 2 tanks (6 pairs) | ≤ 0.07 | ≤ 0.21 |
| Adjacent identical bases, 30 tanks (6 pairs) | ≤ 0.14 | ≤ 0.36 |
| A/B/B/A half disagreement, 16 levers | median 0.05, max 0.17 | median 0.13, max 0.64 |

- **Verdict:** a 0.2 ms p50 delta can be trusted inside an A/B/B/A block, where it is 4× the median half disagreement and the bracketing cancels the step. It cannot be trusted at p95, nor between blocks, nor against the daytime numbers. The three largest p50 half disagreements (0.15–0.17) are the quads that straddle the step.

## 3. Lever prices (A/B/B/A, 60 s per run)

**`duel-2`, 2 tanks** (bases 14.11–14.65; `runs/night/duel-2.out`), ranked by p50 saving:

| Lever | Δp50 | Halves | Δp95 | Halves | Note |
|---|---|---|---|---|---|
| render scale 50 % | **−5.32** | −5.40, −5.24 | −4.81 | −4.80, −4.81 | frame 9.1–9.2 ms |
| render scale 75 % | **−2.93** | −2.94, −2.92 | −2.55 | −2.61, −2.49 | |
| MSAA off | **−2.82** | −2.83, −2.81 | −2.49 | −2.41, −2.58 | daytime single pair −2.75 |
| shadows off | **−2.14** | −2.23, −2.06 | −2.10 | −2.17, −2.03 | |
| bloom off | **−1.71** | −1.68, −1.73 | −0.13 | −0.06, −0.20 | p95 at floor: 16 % of frames stay at 15–16 ms (MEASURED histogram), why UNMEASURED |
| depth prepass off | **−1.07** | −1.05, −1.09 | −0.81 | −0.83, −0.79 | |
| sky env light off | **−0.91** | −0.95, −0.88 | −0.73 | −0.75, −0.71 | |
| cascades 3 → 2 | **−0.46** | −0.46, −0.46 | −0.50 | −0.54, −0.46 | |
| shadow maps 4096 → 2048 | **−0.37** | −0.40, −0.34 | −0.40 | −0.52, −0.28 | |
| UI off | −0.00 | −0.01, −0.00 | +0.04 | +0.03, +0.05 | **below floor** |

**`battle-30`, 30 tanks** (bases 18.78–18.91; `runs/night/battle-30.out`):

| Lever | Δp50 | Halves | Δp95 | Halves | Note |
|---|---|---|---|---|---|
| render scale 50 % | **−6.29** | −6.27, −6.31 | −6.09 | −6.02, −6.17 | frame 12.5 ms |
| MSAA off | **−3.10** | −3.11, −3.09 | −5.42 | −5.74, −5.10 | p99 only −0.19 |
| shadows off | **−3.05** | −3.13, −2.98 | −5.91 | −6.00, −5.81 | |
| sky env light off | **−1.20** | −1.16, −1.24 | −1.44 | −1.45, −1.44 | |
| cascades 3 → 2 | **−0.72** | −0.73, −0.71 | −0.71 | −0.67, −0.76 | |
| shadow maps 4096 → 2048 | **−0.25** | −0.28, −0.23 | −0.20 | −0.35, −0.06 | p95 **at floor** |

- Halves agree in every lever (largest p50 disagreement 0.17).
- **The 30-tank frame is GPU-limited at native scale.** MSAA off saves more at 30 tanks than at 2 (3.10 against 2.82), and half scale brings the frame to 12.5 ms. The CPU chain is therefore ≤ 12.5 ms at 30 tanks (MEASURED bound). That contradicts "30 tanks: the main world paces the frame" (`frame-attribution.md`, DERIVED under the sampler) and #151's "19.2 ms with 30, CPU-bound".
- At 30 tanks the base frames spread over 12–24 ms (MEASURED histogram). MSAA off or shadows off collapses that to 13–18 ms, which is why their p95 saving is ~5.5 ms against ~3 ms at p50.
- This battery prices GPU-side and critical-path cost only. A CPU-only saving (UI, track view) reads ~0 while the GPU paces.

## 4. Track-print relief and trees

**Prints, mud pack, demo hold on the prints** (`runs/night/prints-{fwd,rev}.out`; 50 s warmup). Δ is against LIGHTING:

| Level | p50 fwd / rev | Δp50 fwd / rev | Δp95 fwd / rev |
|---|---|---|---|
| LIGHTING | 11.85 / 11.84 | — | — |
| PARALLAX | 12.17 / 12.15 | **+0.32 / +0.31** | +0.27 / +0.30 |
| SUN SHADOW | 12.31 / 12.33 | **+0.46 / +0.48** | +0.34 / +0.45 |

Forward and reverse agree within 0.02 ms at p50. This is the first valid price: `track-prints-relief-cost.md` measured frames that were never drawn, and its "relief is free" is retracted. The base pack was not run.

**Trees vs graybox cones, 2 tanks** (`runs/night/forest-{fwd,rev}.out`):

| Scene | Cones p50 fwd / rev | Trees p50 fwd / rev | Δp50 | Δp95 | Δp99 |
|---|---|---|---|---|---|
| forest in view | 13.66 / 13.70 | 14.77 / 14.90 | **+1.11 / +1.20** | +1.41 / +1.57 | +2.35 / +3.00 |
| duel, no fir in frame | 14.05 / 14.06 | 14.20 / 14.30 | **+0.14 / +0.24** | +0.46 / +0.65 | +0.46 / +0.77 |

Forward and reverse agree within 0.1 ms at p50. Trees in view also add a tail: >20 ms frames rise from 0 to 15 and 27 (MEASURED).

## 5. Online (`runs/online-{1,2}/summary.txt`)

Edgegap, Singapore, version 0.7.0-alpha.1, one visible scripted client (aim sweep; pen-cam corner and kill cam off). Two passes. **29 tanks is INVALID in both passes**: the client timed out 10 and 11 times and reconnected. Its numbers are contamination, not cost.

**Client frame (ms):**

| Tanks | Pass 1 p50 / p95 | Pass 2 p50 / p95 | Offline comparator p50 / p95 | Δp50 | Δp95 |
|---|---|---|---|---|---|
| 2 | 16.07 / 17.60 | 16.24 / 18.35 | vis-2, 14.55 / 15.94 (MEASURED) | **+1.61** | +2.04 |
| 11 | 17.01 / 26.66 | 17.59 / 28.53 | 16.09 / 18.57 (DERIVED: linear between vis-2 and vis-30, linearity ASSUMED) | **+1.2** | **+9.0** |
| 29 | 13.55 / 15.12 | 13.66 / 15.46 | — | contaminated | contaminated |

- The comparison is not like-for-like: the online run adds the aim sweep and the replication path, and turns two overlays off. Which of these costs the 1.6 ms is UNMEASURED.
- **The 11-tank p95** (MEASURED): 5.9 % and 7.2 % of frames run over 24 ms, at ~27–28 ms; 68 % and 72 % of those are followed by a frame under 10 ms (~7 ms). At 2 tanks online the share is 1.8–2.2 %; offline it is 0.9–1.0 % at 2 tanks, and offline 30 tanks has 0.1–0.2 % of frames under 10 ms. Mechanism UNMEASURED; a catch-up frame after a long one is ASSUMED.

**Server.** 2 and 11 tanks: medians of the per-second `server_stats` lines inside the window (MEASURED). 29 tanks: medians over the `tanks: 29` lines of `bots28/bots28.server.log`, **observed while the client was reconnecting**. Budget 15 625 µs.

| Tanks | tick p50 µs | tick p95 µs | cpu_pct (1 core) | Overruns | Entities |
|---|---|---|---|---|---|
| 2 | 4131 / 4146 | 4720 / 4874 | 71.5 / 69.7 | median 0 | 11.3 k |
| 11 | 7101 / 7287 | 7827 / 8339 | 82.1 / 82.9 | median 0, max 2/s | 12.8 k |
| 29 (reconnecting) | 12 947 / 12 994 | 18 195 / 17 918 | 100.1 / 100.0 | 11 % / 8 % of ticks | 15.7 k |

DERIVED from 2 → 11 tanks, both passes:
- Tick p50: **+330 / +349 µs per tank**, with an intercept of ~3.45 ms at 0 tanks. At 30 tanks: **13.4–13.9 ms p50**, 86–89 % of budget. At 29: 13.0–13.6, which matches the 12.9–13.0 observed there.
- Tick p95: +345 / +385 µs per tank. At 30 tanks: 14.4–15.7 ms, 92–100 % of budget. Observed at 29: 17.9–18.2, over budget.
- CPU: +1.18 / +1.47 % of a core per tank, reaching 100 % at **26 / 23 tanks**; 105–111 % projected at 30. Observed at 29: 100 %, saturated.
- So the first limit is the server's one core, not the tick budget. The deployment is 1 vCPU (CLAIMED, `run-online-night.sh` runbook). A saturated core stretching the tick's wall time would explain the p95 overshoot (ASSUMED).

**Client bytes** (nettop, kB = 1000 B):

| Tanks | Down kB/s | Up kB/s |
|---|---|---|
| 2 | 22.3 / 21.9 | 14.7 / 14.2 |
| 11 | 90.1 / 91.4 | 13.3 / 13.0 |
| 29 | 35.5 / 41.7 contaminated | 1.2 / 1.0 contaminated |

DERIVED, linearity ASSUMED:
- Down: +7.5 / +7.7 kB/s per added tank, so **~235 kB/s ≈ 1.9 Mbit/s per client at 30 tanks**. 30 human clients would mean ~7 MB/s (≈ 56 Mbit/s) of server egress.
- Up: flat at 13–15 kB/s.

## 6. What this changes for #151's queue

Ranked by MEASURED p50 saving, 30 tanks where priced, else 2:

| # | Item | Measured | Line |
|---|---|---|---|
| 1 | Render scale (LOW option, no issue yet) | −6.29 / −5.32 (50 %), −2.93 (75 %, 2 t) | Largest lever by far; costs sharpness. Worth a #159 row. |
| 2 | MSAA → #159 | −3.10 / −2.82 | Largest preset item; a post-process AA's cost is unmeasured. |
| 3 | Shadows → #169 / #170 / #171 | all off −3.05 / −2.14 | The ceiling for all three together, and it grows by 0.9 ms from 2 to 30 tanks. #169 splits it by caster before #170 or #171 is chosen. |
| 4 | Bloom → #159 "smaller bloom" | off −1.71 (2 t) | p50 only (p95 −0.13). Not priced at 30. |
| 5 | Sky env light → #154 | −1.20 / −0.91 | The ceiling for "compute once"; rank #154 up. |
| 6 | Forest trees → #157 | +1.11–1.20 in view, +0.14–0.24 out of view | The ceiling for two-mesh trees, plus their tail. |
| 7 | Depth prepass (new) | −1.07 (2 t) | It feeds only the VFX soft-particle fade (CLAIMED, `src/camera.rs` comment). Candidate LOW row or cheaper fade; not in the queue. |
| 8 | Shadow cascades → #159 | −0.72 / −0.46 | |
| 9 | Print relief → #161 | SUN +0.47, PARALLAX +0.31 over LIGHTING | The ceiling for #161, on a print-filled view. |
| 10 | Shadow maps 2048 → #159 | −0.25 / −0.37 | Smallest real lever. |
| 11 | HUD → #165 | 0.00, below the floor | No frame gain on the M4 while the GPU paces; any CPU saving stays hidden. |
| 12 | Track view → #155 / #156 / #172 | not priced (no lever) | 3.89 pool-ms per frame, serial, at 30 tanks (DERIVED, daytime sampler). It shortens frames only below the GPU (≤ 12.5 ms at 30 tanks); price it with a lever on top of LOW or 50 % scale. |

- The #159 LOW trio (2 cascades, 2048 maps, MSAA off) sums to −4.07 ms at 30 tanks and −3.65 at 2 (DERIVED; additivity ASSUMED). The CPU floor caps it.
- For 120 fps (8.33 ms): even 50 % scale leaves 9.1 ms at 2 tanks and 12.5 at 30 (MEASURED), so the CPU path (#173, track view, transforms) is on the route to the target.
- Update #151's "19.2 ms with 30, CPU-bound" to GPU-limited at native scale.
- For #152: offscreen is validated for p50/p95. Compare absolute numbers only within a block. Future nights should keep A/B/B/A, or settle ~30 min before the first lever block.
