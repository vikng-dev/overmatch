# lightyear 0.28: state rollback silently disabled at zero prediction margin

**Target:** github.com/cBournhonesque/lightyear · crates: lightyear_prediction 0.28, lightyear_sync 0.28
**Severity for us:** CRITICAL (fixed by repo-side watchdog, 8ae795c) · **Status:** unfiled

## Suggested title

State rollback silently disabled at zero prediction margin (balanced input delay + LAN latency)

## Mechanism (all verified in vendored 0.28 source)

For an entity that receives confirmed component updates every tick, the ONLY rollback-mismatch
detector runs at receive time: `write_history::<C>` → `record_confirmed_and_maybe_check`
(lightyear_prediction-0.28.0/src/registry.rs:386-484), gated at registry.rs:426-428 on
`confirmed_tick < current_tick` (STRICT). If the confirmed tick is at or ahead of the client's
current tick, the value is stored in `ConfirmedHistory` but the `should_rollback` comparison is
skipped — the trace-level log says "skipping rollback check until local prediction reaches
confirmed tick", but **no deferred re-check exists anywhere**: once the local tick passes the
sample, nothing re-compares it. The fallback unchanged-entity scan in `check_rollback`
(rollback.rs:577-644) explicitly excludes entities whose replicon `ConfirmHistory` contains the
completed tick (rollback.rs:583) — i.e. exactly the always-confirmed entities the receive path
was assumed to cover.

`InputDelayConfig::balanced()` (lightyear_sync-0.28.0/src/timeline/input.rs:181-259) absorbs all
RTT into input delay at LAN/loopback latency; the sync objective (input.rs:285-310) then holds
the client dead level with the server. Every confirmed update arrives with
`confirmed_tick >= current_tick`, forever → state rollback is permanently, silently dead while
everything looks healthy (ConfirmedHistory advances, data is fresh).

## Measured consequence

Client diverged **35–50 m** from the server with fresh authority arriving every tick and zero
rollbacks firing (tick-level traces available). One instrumented run: 3,296
`confirmed_history_future_skip_mismatch` skip events. Falsifier: `SPIKE_INPUT_DELAY_TICKS=0`
(restoring a real prediction window) capped divergence at 0.015–0.57 m. Margin histogram:
zero frames with margin ≥ 2 ticks after sync settles; 63/63 observed rollbacks coincided with
the rare early margin-≥2 frames.

## Suggested upstream fix

Either (a) deferred re-check: when the receive-time check skips a future sample, re-run the
comparison once the local tick passes it (the sample is already stored); or (b) include
explicitly-confirmed entities in the completion-time scan whenever the receive-time check was
skipped for that tick. Also consider documenting that `balanced()` at low RTT yields zero
prediction margin — users likely assume "recommended default" composes safely with state
rollback.

## Our workaround + removal condition

`src/net/watchdog.rs` (commit 8ae795c): per-tick comparison of
`ConfirmedHistory::newest_present`-at-or-before `current_tick − 1` vs `PredictionHistory`, same
thresholds as the registered conditions, firing `StateRollbackMetadata::request_forced_rollback`
after 3 consecutive distinct breaching samples per component. Remove (or demote to debug-assert)
when upstream ships (a) or (b).
