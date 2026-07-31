# The derived netcode numbers, measured (item 4, 2026-07-31)

ADR-0032 recorded three numbers as **DERIVED, never measured**: a ~10-tick delivery catch-up,
~156 ms, and a <120 m dead zone. This session measured them, plus the two queued knobs that could
be given a hypothesis. Instruments: the `cu` field on `fire_rx` rows (now aggregated by
`scripts/shot/analyze.py` as "fire catch-up" — the direct read of `(P − S) + one-way latency`),
the `k="fact"` per-fact telemetry, and `scripts/shot/run-hullshock-capture.sh` with the `LAT`/`JIT`
sweep levers. All runs: MG belt workload, flat-patch pose, 3840 ticks, target-client traces
(the shooter's own shots carry `cu=0` by construction and are excluded).

## 1. Catch-up is a latency curve, not a constant

| condition (latency/jitter) | cu p50 | cu p95 | p50 in ms | dead-zone radius @ 773 m/s |
|---|---|---|---|---|
| 40 ms / 5 ms  | **5**  | 6  | 78 ms  | ~60 m  |
| 80 ms / 10 ms | **8**  | 9–10 | 125 ms | ~97 m  |
| 160 ms / 20 ms | **13** | 14 | 203 ms | ~157 m |

Two seeds per point; seed spread ≤1 tick at every percentile. The shape is one-way latency in
ticks plus a constant ≈2.7 ticks of sync margin (`jitter_margin` 1.0 + jitter_multiple·jitter +
P−S residue). The derived "~10 ticks / 156 ms / <120.8 m" corresponds to ≈100 ms one-way — a point
we don't ship at; at the measured 80/10 reference the chain is **8 ticks / 125 ms / ~97 m**.
The three ADR numbers are ONE claim: (b) and (c) are arithmetic on (a).

Side finding from the REV-25 A/B arms (5 seeds each, 80/10): the baseline arm (native comparator
live) had the same p50 8 but a much heavier tail — p99 17, max 59 vs the treatment's p99 10,
max 13. The duplicate rollbacks the native trigger ordered were lengthening worst-case catch-up;
the flip tightened the tail.

## 2. `jitter_margin` 1.0 → 4.0: the +3 ticks are real, and so is the bill

Lever: `SPIKE_JITTER_MARGIN` (harness, default 1.0 = lightyear's default). Formula
(lightyear_sync 0.28 sync.rs:126-128): margin = `jitter × jitter_multiple +
tick_duration × jitter_margin`, consumed by the input timeline (objective forward) AND the
interpolation timeline (objective backward).

Measured at 80/10 (fresh binaries both arms): cu p50 **8 → 11**, Δ = **+3 ticks exactly at p50**
(means +3.7 ± 0.3 — sub-tick sync wobble). The falsifiable prediction held; the timeline model is
right. What that buys: the belt-first spark lead (measured 1–3 ticks) would be absorbed. What it
costs, by the same formula: interpolation delay ALSO +3 ticks (S−I grows ≈1.7 → ≈4.7 ticks —
aim lead and future ramming shear ~3× worse) and the dead zone widens ~36 m at 773 m/s. Verdict:
**not a free win — do not flip without a feel test on the interpolation side.** One margin-4.0 run
also showed a one-off ~60-tick catch-up excursion (~1 s stall) — environmental (captures ran on an
in-use machine; see the focus-steal task) but worth re-checking before any real adoption.

## 3. "Predict the impact": dead on arrival

Join per fact: the settling round's `fire_rx` arrival tick vs the fact's `staged_at`, across every
latency point. The causing FireEvent reaches the victim client at median **0 to −1 ticks** relative
to the fact itself (fire-first fraction 6–17%, lead never >3 ticks). FireEvent and state mutation
ride the same link with the same one-way latency — there is no earlier signal to predict the
being-hit from. ADR-0032's adoption machinery is load-bearing, not a workaround for a missing
prediction path. **Queue entry retired.**

## 4. "Quantize": returned to sender

One word in the 2026-07-29 handoff queue, no hypothesis, no failing metric — and the silent-desync
measure that a quantization change would move (`|confp − server_p(conft)|`) read zero windows in
all ten capture runs. Not scopeable as filed; needs a statement of intent before it earns a capture.

## What was NOT measured

The dead-zone **consequence** (single main-gun shot below the radius forfeits spark-before-shove)
is still evidenced only by the belt-first sub-population — which measured 1–3 ticks of spark lead
at 8 m, pointing AGAINST the derived pessimism. A decisive main-gun range sweep needs three small
harness changes (parametrized lane step, terrain-Y lanes under a harness pose, stationary primary
refire) that were deliberately not spent here. The radius half is fully covered by the curve above.

## Reproduction

`LAT=<ms> JIT=<ms> scripts/shot/run-hullshock-capture.sh <seed> <dir>` per point;
`scripts/shot/analyze.py --client <dir>/target.client.jsonl --server <dir>/server.server.jsonl`
prints the fire catch-up row; the fact join is the `belt_first` machinery in
`scripts/shot/ab-compare-hullshock.py`. Capture manifests record commit, condition, and binaries.
