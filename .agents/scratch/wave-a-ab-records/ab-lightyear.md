# Lightyear game-level A/B record (wave-A) — 2026-07-11

Branches: wave-a/ly-gate = d7d103e + SPIKE_NO_WATCHDOG lever (8643f9b); wave-a/ly-alone =
+ lightyear family fork override (b4c29bb). All runs SPIKE_LATENCY_MS=10 (never 0, §7),
watchdog OFF, SPIKE_PERTURB=0.

## A-side (unpatched 0.28.0, watchdog off) — starvation demonstrated raw

- short course: physics bit-exact (contractive course hides it), hsim-only mismatches.
  Client ticks sane (37..1032, worst replay 10× at fire).
- wedge (beached + SPIKE_SIM_FORWARD): 0% match, |Δp| p50 17.6 mm / p99 47.6 mm / max
  **55.3 mm — above the 50 mm rollback bar — with exactly 2 rollbacks, both connect-window,
  none after** (PredictionMetrics.rollbacks=2 whole run), fresh authority arriving every tick.
  Report #1's mechanism, live.

## B-side (fix/deferred-rollback-check, watchdog off) — the fix works where it works…

- wedge: divergence BOUNDED — |Δp| p50 3.4 mm / p99 21.9 mm (5× / 2× tighter than A);
  3 rollbacks including two MID-RUN (t+5.1 s, t+5.7 s — the wedge settle window) responding to
  threshold crossings. The deferred check is alive and correcting. Inside the falsifier
  reference bound (0.015–0.57 m).

## …and a DISQUALIFYING adverse finding on the short course

**3 of 4 B-short runs: client killed by jetsam after exploding to 7.5 GB RSS within ~2 s of
rollback-enable.** The 4th survived but ran broken. Mechanism (traces):

- Dying runs: client local tick TELEPORTS ~280 ticks forward (rows at ticks 37..46, then 324
  repeated 5×; second specimen 38..46 → 325/326) at the moment the first deferred mismatch is
  consumed (log: ROLLBACK-SNAP 0.85 m + rollback depth 11→18 in 40 ms, then silence).
- Surviving run: client advanced to tick ~7045 while the server script ends ~650 — permanently
  ahead, replaying 9× per tick around 6400+.
- A-side control, same condition: ticks sane. Patched wedge at lat10: sane. The edge fires
  depending on connect-time sync events (non-deterministic).

This is exactly the edge the adversarial review flagged: deferred markers are recorded when
`confirmed_tick >= current_tick`; a backward SyncEvent (connect resync at zero margin) then
drops the local tick BELOW the recorded mismatch_tick, and the new
`rollback_tick = mismatch_tick` branch (rollback.rs:534-539) rolls FORWARD to a locally-future
tick — the timeline jumps ~280 ticks ahead, prediction storage/catch-up resim balloons to GB,
jetsam kills the process. Base lightyear could never roll back to a future tick. The review
called it "ultra-edge"; at LAN latency it reproduces 3/4 at connect on the flat course.

## Verdicts

1. **Patch mechanism: validated at game level** (wedge: bounded + mid-run rollbacks vs raw
   starvation). The crate-level exactly-once proof stands.
2. **NOT adoptable as-is; the upstream PR MUST add a future-tick guard** — clamp deferred
   consumption to completed ticks (e.g. only roll back to min(mismatch_tick, current_tick−1))
   or drop/re-key markers above the local tick after a backward SyncEvent. Cheap fix, clear
   repro (this record + traces).
3. **Watchdog: KEEP** (retirement condition NOT met until the guarded fix ships upstream and
   re-passes this A/B).
4. Skip/recheck counters were not collected (would need lightyear trace-level logging; the
   crate test pins exactly-once semantics — behavioral evidence above suffices).
