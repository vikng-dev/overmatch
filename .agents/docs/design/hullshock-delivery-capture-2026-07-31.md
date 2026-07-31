# HullShock delivery on a real link — the capture ADR-0032 was waiting for

2026-07-31, binaries at `b525794` (main, working tree clean apart from the doc-correction branch
this file rides on). This is the reading ADR-0032 §"what the counters would show" names as "the
next evidence, and no claim in this ADR should be promoted to a product fact before then", and the
"cheap discriminating experiment" from the task-47 design memo. **No behaviour was changed.**

## Method

Dedicated server + idle target client + MG shooter client — the `scripts/shot/run-mg-armor.sh`
topology — at the CHOSEN standard condition `SPIKE_LATENCY_MS=80 SPIKE_JITTER_MS=10`, seeds 1–5
(`SPIKE_JITTER_SEED`), 3 840 shooter ticks (+128 target margin), `SPIKE_TRACE` +
`SPIKE_TRACE_SIM_FIELDS=1` on all three processes, `RUST_LOG=overmatch=debug` on the target.

Three deviations from the script as committed were REQUIRED — the script has not been runnable
since 2026-07-27 and every one of these is a pre-existing infrastructure defect, not a measurement
choice:

1. **`OVERMATCH_SERVER=127.0.0.1:5888` on both clients.** The script starts a local server but
   never points the clients at it; they dial the BAKED PRODUCTION droplet
   (`src/net/client.rs:43-46`, log line "source: baked default"). Verified by running it: both
   clients targeted `157.245.48.161:5888`.
2. **`SPIKE_SIM_WINDOWED=1` on both clients.** A headless client cannot load the mipped-KTX2 tank
   glb since `a43a557`: bevy_image 0.19's UASTC→uncompressed transcode slices the SOURCE level
   data with the DESTINATION block geometry (`bevy_image-0.19.0/src/ktx2.rs:209`) — every panic is
   exactly "expected w·h·4 (RGBA32), got w·h (UASTC at 16 B / 4×4 block)", a 4× mismatch on every
   mip. Headless takes that path because no RenderApp means no `CompressedImageFormatSupport`;
   windowed transcodes to a 4×4-block GPU format whose geometry coincidentally matches, which is
   why played builds never see it. The tank-asset connect gate then hangs the client forever.
3. **`SPIKE_SPAWN_POSE="149.3,8.75,293.9,0,0,0,1"` on the server.** The lane spawn's world-origin
   base sits on a heightmap slope since the alpha.10 terrain world; the tanks settle 23 m apart
   with 11 m of height difference, the hull-local `-8,0,0` aim never touches armor, and the run
   produces ZERO damage — the strict analyzer cannot pass at HEAD. The pose is the flattest 40 m
   patch on the shipped heightmap (MEASURED 4 cm relief, mean 6.73 m, center x=149.3 z=293.9);
   y = surface + the 2 m spawn clearance. Lane 1 restores the intended 8 m in-x separation.

With those three, the topology behaves as designed: tanks settle at (149.3, 6.65, 293.9) and
(157.3, 6.64, 293.9), and 499 MG rounds per weapon strike armor at 8 m across four belts of ~150
(one round per 6 ticks; the armor-striking weapon is **w=0** in the shot trace — both weapons are
7.9 mm, so filter on `cf` rows, not on `w==1`).

## Results — five seeds, identical shape

| seed | on_impact | on_budget | bypassed | undelivered | max_wait | adopted | rollbacks | w/ HullShock trg | window drops | UNORDERED |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | 181 | 0 | 4 | 0 | 0 | 181 | 303 | 189 | 0 | 0 |
| 2 | 183 | 0 | 4 | 0 | 0 | 183 | 319 | 189 | 0 | 0 |
| 3 | 180 | 0 | 4 | 0 | 0 | 180 | 310 | 187 | 0 | 0 |
| 4 | 183 | 0 | 4 | 0 | 0 | 183 | 317 | 191 | 0 | 0 |
| 5 | 176 | 0 | 4 | 0 | 0 | 176 | 294 | 192 | 0 | 0 |

All MEASURED from the target client's final `OrderingTally` diagnostics line, its adoption log
lines, and its `SPIKE_TRACE` rollback rows. `scripts/jitter/analyze.py` on seed 1 (target paired
with server): 303 rollbacks all `state`, depth mean 9.3 / max 28 ticks, trigger attribution
HullShock 362 · TankServos 21 · Position 1, render-error offset active 5.7 % of frames
(|cp| p95 0.335 m), client/server divergence ≈ 0 at every percentile, no silent desync windows, no
camera-space transients.

### Both delivery paths are live at 80/10 — and the bypass class is DECODED

- **Adoption works on a real link.** ~181 facts per run passed readiness, requested
  `ExternalEvent`, retired `Adopted`, released on impact. Zero budget releases, zero waits
  (`max_wait_ticks=0`), zero replay-window drops, zero UNORDERED, zero undelivered. The readiness
  gates ADR-0032 worried had "only ever been exercised in fixtures" pass continuously in play.
- **The native comparator ORDERS exactly 4 rollbacks per run — the 4 bypasses, and nothing else.**
  (CORRECTED 2026-07-31, second vendor pass.) The ~190 rollback rows carrying a HullShock `trg`
  are almost all adoption's OWN forced rollbacks wearing receive-time comparator TRIPS: the trg
  slot records trips, not orders, and receive-time dispatch trips it on the same PreUpdate the
  forced rollback fires. Matching rollback starts against the forced-install log lines leaves
  only ~15 policy-ordered rollbacks per run, of which exactly 4 carry HullShock — the bypasses.
  The comparator's ONLY observable production effect is the bypass class. (Related defect: the
  trip-slot clear is ordered only `.before(RollbackSystems::Check)` with no edge against
  `ReplicationSystems::Receive` — `src/trace.rs:838-848`'s "exact per-check attribution" claim
  holds by scheduler accident, not by construction.)
- **The bypasses are the first round of every belt, deterministically.** Server MG fire ticks show
  belts of ~150 rounds with 224-tick reload gaps (seed 1: fires resume at 506/1624/2742/3860);
  the bypassed facts are at 506/1624/2742/3861 in seed 1 and the same first-of-belt positions in
  every other seed. Mechanism: after a cold start or reload pause nothing is in flight, the
  episode's `HullShock` update outruns its impact visual by a MEASURED 1–3 ticks, and the native
  receive-time comparator orders the rollback before adoption's spark-wait can hold it. Mid-belt,
  impacts are continuously drawn, so every fact releases on impact with zero wait. This is a
  per-belt CLASS with a 100 % hit rate, not a jitter race — seeds don't move it.

### The denominator gap, now with a measured size

Fact sequences on the target run 1–190 but only 185 appear in any adoption log line
(181 adopted + 4 bypassed); #34, #65, #161, #183, #186 never appear at all. Consistent with
replication coalescing — `HullShock` carries STATE, so two episodes closing inside one send window
arrive as one final `count` (the exact behaviour `src/ballistics.rs`'s monotonic-counter doc
promises) — but proving that needs the per-fact server-side telemetry the memo already lists as
the missing denominator. ~2.6 % of episodes.

## Decision-table mapping (memo §"Cheap discriminating experiment")

Two rows of the table fire simultaneously, and they agree:

- "`released_on_impact` dominates, `undelivered=0`, and real adoptions occur" → **strong evidence
  for option (a)**.
- "`bypassed > 0` correlated with `HullShock` rollback triggers" → **the native comparator is
  defeating the ordering rule; option (a) has demonstrated value.**

The rows that would have argued otherwise all came up empty: no budget releases (view carrier is
timely), no readiness waits or drops (no reason to keep the native path as a fallback), no
Position/watchdog-delivered bypasses (no case for the application barrier on this evidence).

The memo's sparse-hit confirmation pass was NOT run: its purpose is to rule out the pre-staging
FIFO residual inflating `bypassed`, and that ambiguity does not exist here — each bypass is an
explicitly logged per-fact `Retirement::Delivered{spark_pending}` with a decoded, seed-independent
mechanism. The five sequence gaps are the residual's territory instead, and they need telemetry,
not another run of the same instrument.

## What this authorizes (item 2's gate)

Re-scoped option (a) — inert `HullShock` comparator, adoption as the sole INTENTIONAL trigger,
`Retirement::Delivered` + `OrderingTally::bypassed` retained — now has the real-link evidence the
memo demanded before any behaviour change. The concrete prize is the belt-start class: 4 shoves
per 4 belts landing a MEASURED 1–3 ticks (16–47 ms) before their spark, every run (CORRECTED —
the earlier "~8 ticks / 125 ms" read the rollback DEPTH field as the spark lead). Under adoption
those same facts would have released ON IMPACT after a 1–3 tick hold, well inside the 16-tick
budget, in 19 of 20 measured bypasses; the one open case is seed 5's first fact, whose belt-first
round RICOCHETED and whose covering spark carries authority tick 566 against fact tick 567 —
whether `VisualClaim::covers` spans it depends on `HullShock::opened`, which no current
instrumentation records, so the A/B must tolerate (and explicitly classify) one possible budget
release there. Acceptance per the grilled plan: per-fact ownership telemetry as the primary
instrument, `bypassed → 0` with `adopted` holding ≈ 181–183 on the same seeds,
`undelivered`/drops still 0, `trg == 0` as secondary wiring evidence only.

## Capture recipe (for repeat / A/B)

Runner: `scripts/shot/run-hullshock-capture.sh` — `run-mg-armor.sh` plus the three fixes above
(which are now also baked into `run-mg-armor.sh` itself); per-seed dirs with
`server-trace`/`target-trace`/`shooter-trace` JSONL (distinct `SPIKE_TRACE` prefixes per process —
a shared prefix makes the two clients clobber one `trace.client.jsonl`). Extraction:
`scripts/shot/extract-hullshock.py` (tally regex, log-line counts, trg histogram). A/B accounting
over the per-fact `k="fact"` rows: `scripts/shot/ab-compare-hullshock.py`.
