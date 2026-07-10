# Handoff — the hsim divergence windows (task #24)

2026-07-10. For a dedicated session; the divergence instrument's first real case. Delete when
consumed. Read `.agents/docs/design/sim-divergence-and-determinism.md` §8 first (the instrument:
fields, analyzer, baseline, stated limits) — this handoff assumes it.

## The finding (MEASURED, baseline 2026-07-10, main @ 5fbb68e-era, 80/10, both courses)

- Physics state (`hpos`/`hrot`/`hlv`/`hav`) is **bit-exact client-vs-server on every shared
  tick** of both runs — through jitter, bump, washboard, steering, firing.
- **Every hash mismatch is `hsim`-only**: 106/1278 ticks (long course; windows ~1463–1535 and
  ~1767–1799) and 71/584 (short; window ~139–209). `hsim` covers the carried mechanism state:
  `TankSim` (per-mount servo current/previous/velocity via `ServoState::hash_fields`, weapon
  reload/recoil, per-wheel brush anchors incl. the Some/None grip discriminant) plus
  `DriveState` (throttle/steer).
- Windows **open at precisely identifiable moments** — the connect-time rollback replay, and
  just after the scripted shot — and **reconverge within ≤73 ticks**, then stay converged.
- Instrument limit that this session exists to remove: `hsim` is ONE boolean. No per-field
  decode, no magnitudes — attribution today is window-timing correlation only.

## Why it matters

Bounded and self-healing today (the carried state is contractive — ADR-0016: servos chase
targets, reloads count down, anchors re-grip; that is WHY it reconverges and why no pose-based
check ever saw it). But: (1) it is the **largest measured divergence term left** — everything
else is zero; (2) reload-timer divergence during a window is a concrete misfire-feel risk (a
fire click the client accepts and the server rejects), which scales up under any future
predict-both (frequent rollbacks = frequently re-opened windows); (3) it is the early-warning
class for **non-contractive** carried state — the first accumulator/toggle someone adds to
`TankSim` diverges PERMANENTLY by this same mechanism.

## STEP ZERO — validate the metric before chasing the mechanism

This repo already paid for skipping this once (the hc=0 saga, design doc §5–6: a metric cited
for months measured something else). Before any mechanism work, establish that the windows are
**final-timeline divergence** and not a join artifact: the trace stamps replay rows (`rp`) and
the analyzer keeps the last row per (tick, entity) — verify that during a rollback-replay span
the client rows being compared are the CORRECTED timeline's values, that the server rows align
on the same tick indices, and that a window isn't simply "ticks whose client row was captured
mid-replay at a different intra-tick phase than FixedLast". Read `record_tick` + the analyzer's
join with this question; if any doubt survives reading, instrument one window verbosely and
check by hand. Only a metric that survives this step is worth explaining.

## Hypotheses (UNVERIFIED — ranked, none is a conclusion)

1. **Restore fidelity / intra-tick phase:** rollback restores `TankSim`/`DriveState` from
   `PredictionHistory` (they are `local_rollback`-registered; the `strip_confirmed_history`
   observers — src/net/protocol.rs, upstream report #6 — already prevent the stale add-time
   seed). If the history SAMPLES the component at a different point in the tick than the hash
   reads it (FixedLast), the restored value is honest but phase-shifted, and the replay
   re-integrates from a value the original timeline never held at that boundary.
2. **Edge/fire semantics across replay:** the short course's window opens right after the
   scripted shot. `consume_edges` fires an edge exactly once in the original timeline; how does
   a replayed span treat an edge consumed inside it (input buffer restores the historical
   `ActionState` — does the reload timer get re-set at the same replayed tick, one off, or twice)?
3. **Brush anchors ride contact state:** anchors re-grip based on contact conditions during
   replay; the BVH-restore class (upstream report #5, latent post-poison-fix) means replayed
   contact discovery can differ transiently → anchors re-anchor at slightly different points →
   `hsim` differs while forces (and thus poses) stay effectively identical.
4. **Server-side writer asymmetry:** something writes carried state on one end only (a
   client-only or server-only system touching `TankSim` fields) — would show as windows
   correlated with mode-specific events rather than rollbacks per se. The window-at-connect
   observation weakly disfavors this but does not exclude it.

## Method

1. Step zero above.
2. **Per-field decode:** split `hsim` into sub-hashes (`hservo`/`hreload`/`hanchor`/`hdrive`)
   in `src/trace.rs` — same env-gating, same unit-test pattern as the existing hash tests
   (`zero_input_resume_is_identity` era tests show the shape); extend
   `scripts/divergence/analyze.py` to attribute windows per field. Optionally an env-gated
   verbose dump of the raw fields during mismatch ticks for magnitudes.
3. **Correlate with rollback telemetry:** the trace/diagnostics already carry rollback
   counts/depths — align window open/close ticks with rollback events and replay depth; deeper
   replays should widen windows if hypothesis 1/2/3 is right.
4. **Reproduce deliberately:** provoke rollbacks (jitter up; the watchdog's forced rollback;
   fire events at known ticks) and measure window population against the provocation — a
   dose-response curve is the cleanest mechanism evidence.
5. **Classify and act:** local fix (restore/replay/edge semantics) vs an addendum to upstream
   report #6 (or a new report) vs "bounded-and-accepted" with a documented tripwire (e.g. a
   test/metric that fails if windows stop reconverging or if a non-contractive field appears in
   `TankSim`). Write the outcome as design-doc §9, dated, with the numbers.

## Constraints & coordination

- The instrument stays zero-cost when `SPIKE_TRACE` is unset; hash canonicalization changes must
  update its unit tests (same-state, bit-flip localization, entity-id independence).
- Gates (five, foreground only — background-and-wait kills agent sessions), harness etiquette
  (one client pair, port 5888 clean, no SPIKE_LATENCY_MS=0, `pgrep -l rustc` before cold builds).
- **Concurrent sessions exist**: Yan's aim-intent work and the wave-A review (HANDOFF-wave-a-review.md)
  may run in parallel. Working dirs: the bay worktree `../overmatch-bay-1` or the main checkout —
  confirm with Yan which is free; do not share a working directory with another live session.
- Wave-A interaction, both directions: the lightyear deferred-check patch changes WHEN rollbacks
  fire (it adds the starved-regime rollback) — if the review session adopts forks while this one
  runs, re-baseline; conversely this session's per-field decode is exactly the tooling the
  review's A/B will want. Coordinate through the board (tasks #24/#28).

## Deliverables

Metric-validity verdict (step zero, explicit); per-field attribution with magnitudes; the
mechanism with dose-response evidence; the fix or the documented acceptance + tripwire; §9 in
the design doc; board update. The bar for "explained": someone reading §9 can predict which
field diverges, when a window opens, and when it closes, before running the trace.
