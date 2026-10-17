# Netcode facts — MEASURED

Numbers that still decide things, distilled from the 2026-07-31 capture runs (MG belt workload,
flat-patch pose, 3840 ticks, two or more seeds per point, target-client traces). Tick rate 64 Hz,
so 1 tick = 15.625 ms. Reproduce with `LAT=<ms> JIT=<ms> scripts/shot/run-hullshock-capture.sh
<seed> <dir>`, then `scripts/shot/analyze.py`; the runs, their method and their side findings are
in git. *(Historical recipe — the HullShock capture suite was deleted with the prediction
machinery; it runs only at pre-demolition revisions.)*

## Delivery catch-up is a latency curve, not a constant

MEASURED `(P − S) + one-way latency` on facts arriving at a target client:

| latency / jitter | catch-up p50 | p95 | p50 in ms | dead-zone radius @ 773 m/s |
|---|---|---|---|---|
| 40 / 5 ms | 5 tk | 6 | 78 ms | ~60 m |
| 80 / 10 ms | **8 tk** | 9–10 | **125 ms** | **~97 m** |
| 160 / 20 ms | 13 tk | 14 | 203 ms | ~157 m |

Seed spread ≤1 tick at every percentile. Shape: one-way latency in ticks plus ≈2.7 ticks of sync
margin. 80/10 ms is the standard condition we quote. ADR-0032's DERIVED "~10 ticks / 156 ms /
<120 m" is one claim expressed three ways and corresponds to ≈100 ms one-way — a point we do not
ship at.

## Pen cam explanations: 0 of 284 arrived after their impact was drawn

MEASURED 2026-10-18 under `SPIKE_SHOT_TRACE` (the delivery's trace rows — server `xsend`, `link`;
client `xrx`, `xpres`, `xdraw` — are registered only when it is set; every row but `meta` carries
the wall clock `us`). `scripts/shot/run-pencam-latency.sh`: two clients 8 m apart, main gun + held
MGs, 200 s per condition, four conditions, 284 main-gun explanations; the MG sends none. Server
impact → explanation stored (wire) and → the cam able to open (open: the client's draw of the
impact, or the arrival if drawn first), shooter and victim ranges, ms:

| latency / jitter | wire p50 | wire p90 | wire max | open p50 | open p90 | open max | TimedOut |
|---|---|---|---|---|---|---|---|
| 0 / 0 | 7–8 | 11–15 | 17 | 46–52 | 63–68 | 115 | 0 of 74 |
| 60 / 10 | 72–77 | 79–86 | 93 | 115–119 | 136–137 | 143 | 0 of 70 |
| 120 / 30, seed 1 | 130–140 | 155–160 | 177 | 192–315 | 257–330 | 349 | 0 of 72 |
| 120 / 30, seed 2 | 133–144 | 151–170 | 212 | 231–314 | 341–392 | 406 | 0 of 68 |

The cam waits for the interpolation-delayed impact draw, so `open` follows each client's
interpolation delay. At 120/30 that delay differs between the two clients by 110–140 ms; INFERRED,
not measured: most likely one client's arrival estimator absorbed a post-stall batch, a harness
artefact. The
runs, the per-client split and what they leave unmeasured:
`.agents/scratch/damage-cam-2026-10-08/slice9-latency-2026-10-18.md`.

## `jitter_margin` 1.0 → 4.0 costs what it buys

Margin = `jitter × jitter_multiple + tick_duration × jitter_margin` (lightyear_sync 0.28
`sync.rs:126-128`), consumed by the input timeline forward **and** the interpolation timeline
backward. MEASURED at 80/10: catch-up p50 8 → 11 ticks, exactly +3. The same +3 lands on
interpolation delay (S−I ≈1.7 → ≈4.7 ticks: aim lead and ramming shear roughly 3× worse) and
widens the dead zone ~36 m. **Not a free win — never flip it without a feel test on the
interpolation side.**

## Two queue entries that are closed

- **"Predict the impact" is impossible.** The causing `FireEvent` reaches the victim client at a
  median of 0 to −1 ticks relative to the state fact itself; both ride the same link with the same
  one-way latency. There is no earlier signal — ADR-0037's rule that a foreign fact is
  presentable no earlier than its arrival rests on this measurement.
- **"Quantize" was never scopeable.** The silent-desync measure a quantization change would move
  (`|confp − server_p(conft)|`) read zero windows across all ten capture runs.

## Not measured

The dead-zone *consequence* — a single main-gun shot below the radius forfeiting spark-before-shove
— is evidenced only by the belt-first sub-population, which measured 1–3 ticks of spark lead at
8 m and so points **against** the derived pessimism. A decisive main-gun range sweep needs a
parametrized lane step, terrain-Y lanes under a harness pose, and stationary primary refire.
