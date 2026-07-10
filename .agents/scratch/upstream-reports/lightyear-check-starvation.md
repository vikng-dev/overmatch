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

## Candidate fix status (adversarially reviewed 2026-07-11, branch `fix/deferred-rollback-check`)

Option (a) implemented and sound: per-component deferred marker recorded ONLY when the
receive-time check skips (set gate is the exact complement of the receive-time gate,
registry.rs:454-456 vs :488-497), compared once when local prediction passes the tick and
replicon completed it, removed on consumption; `balanced()`/lightyear_sync untouched; no
double-fire with the unchanged-entity scan (exclusive branches, rollback.rs:519-575). RED test
proof verbatim: "the deferred mismatch should trigger exactly one rollback from its confirmed
tick / left: [] / right: [Tick(77)]". Suites: rollback 28/28, prediction 19+1, sync 9; full
suite 155/1/3 — **single-threaded only; the suite is load-flaky in parallel at the 0.28.0 tag
(pre-existing) — disclose in the PR.** Minor: stale-mismatch consumption after a forced rollback
is marginally likelier (wasted, not incorrect, rollback); latent marker retention if upstream
ever prunes predicted-entity ConfirmedHistory (currently a dead path).

**BLOCKING EDGE, measured live (2026-07-11): rollback to a locally-future tick.** Deferred
markers are recorded when `confirmed_tick >= current_tick`; a backward SyncEvent (routine at
zero-margin connect) then drops the local tick below a recorded `mismatch_tick`, and the new
`rollback_tick = mismatch_tick` branch (rollback.rs:534-539) rolls FORWARD to a locally-future
tick — base could never do this. In-game at SPIKE_LATENCY_MS=10, watchdog off, flat course:
3 of 4 runs the client's timeline teleported ~280 ticks ahead at first deferred consumption and
the process ballooned to **7.5 GB RSS in ~2 s** (catch-up resim + prediction storage) before
jetsam killed it; the 4th survived permanently ~6,400 ticks ahead of the server. The patch is
NOT shippable without a guard: clamp deferred consumption to completed ticks (roll back to
`min(mismatch_tick, current_tick − 1)`) or drop/re-key markers above the local tick on a
backward SyncEvent. With the guard absent our watchdog workaround STAYS (retirement condition
not met). Where the edge does not fire the fix demonstrably works: beached-wedge run held
|Δp| p50 3.4 mm / p99 21.9 mm with mid-run rollbacks, vs unpatched 17.6 / 47.6 mm with zero
post-connect rollbacks.

**Fresh live evidence (this repo, 2026-07-11, watchdog gated OFF, SPIKE_LATENCY_MS=10):**
unpatched wedge run — |Δp| reached 55.3 mm (above the 50 mm rollback bar) with exactly 2
rollbacks, both in the connect window, none after, fresh authority arriving every tick. Also
2/24 standard 80/10 short runs showed a constant ~880 mm offset from connect persisting
un-rolled-back for hundreds of ticks.

## Our workaround + removal condition

`src/net/watchdog.rs` (commit 8ae795c): per-tick comparison of
`ConfirmedHistory::newest_present`-at-or-before `current_tick − 1` vs `PredictionHistory`, same
thresholds as the registered conditions, firing `StateRollbackMetadata::request_forced_rollback`
after 3 consecutive distinct breaching samples per component. Remove (or demote to debug-assert)
when upstream ships (a) or (b).
