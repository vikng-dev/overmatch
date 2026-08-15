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
