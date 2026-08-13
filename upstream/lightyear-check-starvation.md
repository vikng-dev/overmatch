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

## What fixing this unlocks for us

**Clean up.** `src/net/watchdog.rs` goes entirely (346 lines: the `PreUpdate` re-check system, the
`BREACH_STREAK_TO_FIRE = 3` persistence gate, the `newest_present_at_or_before` re-implementation of
a getter whose public form drops the sample tick, the per-component streak memory), plus its mount
(`net/client.rs:175`) and ADR-0015's Layer-2 row 1. With it goes the "one definition of desynced,
two detectors" coupling in `net/protocol.rs`: `ROLLBACK_POSITION_M` / `ROLLBACK_ROTATION_RAD` /
`ROLLBACK_VELOCITY` and the `*_error` metric functions are `pub(crate)` **only** so the watchdog can
re-run the receive-time comparison (protocol.rs:1029-1030); they collapse back to private, single-use
constants.

**Optimize.** The watchdog is a per-frame client pass over four `ConfirmedHistory` buffers — small
(it is an iterator `take_while` over a sorted buffer, no allocation), and it is not where the money
is. The money is that a fired watchdog rollback is a *forced* one: it bypasses the policy gates and
resimulates from the confirmed tick, so every firing pays a full replay. Upstream's deferred re-check
would fire the same rollback through the normal path with no backstop pass at all.

**Explore — this is the big one: input delay stops being a correctness knob.** lightyear's
`sync_objective` (lightyear_sync-0.28.0 timeline/input.rs:285-310) places the client's timeline at

```
remote + RTT/2 + jitter_margin + 1 + sync_error_margin − input_delay
```

Input delay is subtracted from the prediction lead **one for one** — `net/client.rs` states the same
thing from our side ("every tick of input delay is a tick prediction does NOT run ahead"). So any
input delay at or above a link's natural lead drives the prediction margin to zero, which is
*precisely this defect's trigger*. At our shipped `fixed_input_delay(3)` (~47 ms) that is not a corner
case: it is every low-ping client — loopback dev runs, LAN, and a real player near the server — which
is why the watchdog is still load-bearing in production and not merely a historical artifact of
`balanced()`.

With the deferred re-check shipped, zero prediction margin means only *"no rollback was needed"*
instead of *"rollback is silently dead"*, and input delay becomes a pure **feel/depth knob**: raise it
for shallower replays, lower it for snappier input, decide by playtest instead of by correctness. That
is the precondition for both experiments below.

- **0-tick input delay** (`InputDelayConfig::no_input_delay()`, i.e. `fixed_input_delay(0)` — the
  snappiest possible input: the client applies the command on the tick it was authored). **Correction
  to a natural assumption: 0-tick does not squeeze the prediction margin, it MAXIMISES it** — it is
  this defect's own falsifier (`SPIKE_INPUT_DELAY_TICKS=0`, above, restored a real prediction window
  and capped divergence at 0.015–0.57 m). 0-tick is also *constant*, so it does not reintroduce
  [#10](lightyear-absent-anchor-input-freeze.md)'s `Δend_tick != 1` seeds and the anti-adaptive tripwire
  (`input_delay_is_constant`, net/client.rs:1468) does not stand in its way. What 0-tick actually costs
  is stated honestly under **Trade** below; it is an **experiment to run, not a free win**.
- **Adaptive input delay** (`balanced()` — near-0 delay on a good link, more on a bad one, which is how
  you get the 0-tick *feel* without the 0-tick *risk* on a bad link). **Blocked by TWO reports:** this
  one *and* [lightyear-absent-anchor-input-freeze.md](lightyear-absent-anchor-input-freeze.md) — a
  varying delay corrupts the input stream (`Δend_tick != 1`) *and* can walk the margin to zero, and
  today either one alone is a shipping defect. Reconsider only when both are resolved.

**Trade, named — what 0-tick costs even when unblocked.** (a) It maximises the prediction window
(`maximum_predicted_ticks: 100`), so rollbacks get *deeper*, and a deep replay through our
friction/contact chaos amplifies: measured, applied visual correction ran 5.6× the same-tick sim
divergence at median and 43× at p90 (`net/protocol.rs`, the ROLLBACK_* doc). Making deep replays cheap
and non-amplifying is what [avian-solver-constraint-order.md](avian-solver-constraint-order.md) buys.
(b) It removes the input buffer's jitter cushion: at delay 3 a command reaches the server three ticks
before it is needed, at 0 it arrives just in time, so every jitter spike starves the server's buffer.
With our `for_tick` attestation the server then **fails closed** — the round is *not* fired. So 0-tick
converts "the server fires a round you never authored" into "your trigger pull is dropped when your
input arrives late", plus more rollback churn. That is a better failure, not no failure, and no
upstream fix can invent the missing input.
