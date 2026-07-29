# Upstream: authoritative state arriving at or ahead of the client's local tick is never reconciled

Status: **DRAFT — NOT FILED.** Written to be sent as a new upstream issue by the repo owner, if and
when they choose. Nothing has been submitted to `cBournhonesque/lightyear`. No open upstream issue
covers this defect (searched 2026-07-29, see *Upstream status*).
Found: 2026-07-29, Overmatch, `feat/authoritative-facts`, while building fixtures for a
zero-lead loopback client. Root-caused from the published `lightyear_prediction-0.28.0` /
`lightyear_sync-0.28.0` sources, then cross-checked against upstream `main`.

Affects: **lightyear 0.28.0** (latest release, published 2026-06-26) and current `main`.

**A note on paths.** The crate layout moved twice, so the same file has three names:

| Ref | Path |
| --- | --- |
| published crate `lightyear_prediction-0.28.0` (line numbers below refer to this) | `src/registry.rs`, `src/rollback.rs`, `src/manager.rs` |
| upstream `main` (2026-07-29) | `crates/replication/prediction/src/{registry,rollback,manager}.rs` |
| at commit `6b29a12c` (2026-06-13) | `lightyear_prediction/src/registry.rs` |

`crates/lightyear_prediction/src/...` does **not** resolve at any of those refs.

---

## Symptom

With `RollbackMode::Check`, a predicted entity that the server explicitly updates at every
checkpoint — i.e. any actively driven entity — is **never** rollback-checked while the client's lead
over the server is zero or negative. Both routes that could catch the mismatch skip it, and neither
ever revisits the tick. The client's prediction can diverge from authority without bound; the
authoritative value is written into `ConfirmedHistory` and then simply never compared to anything.

The condition is not transient. Lead is a *steady-state* property of the sync configuration (see
*The lead arithmetic*), so for an affected configuration the skip holds for the whole session.

Upstream already emits a trace for the first half of it, so exposure is measurable without new
instrumentation: count events on target `lightyear_debug::prediction` with
`kind = "confirmed_history_future_skip_mismatch"` (`registry.rs:448-457`).

## Minimal reproduction

A loopback client (`rtt ≈ 0`, `jitter ≈ 0`) with a fixed input delay of 3 ticks and otherwise stock
sync config:

```rust
InputTimelineConfig::new(
    SyncConfig::default(),                        // jitter_margin 1.0, error_margin 1.0
    InputDelayConfig::fixed_input_delay(3),       // input.rs:214
)
// PredictionManager { rollback_policy: RollbackPolicy { state: RollbackMode::Check, .. } }
```

Server continuously mutates a replicated, predicted component on an entity every tick (so the
entity's Replicon `ConfirmHistory` contains every completed checkpoint). Force a client-side
prediction error. Expected: rollback. Actual: no rollback, ever.

`jitter_multiple` is irrelevant here — at zero jitter its term vanishes. Any tick rate works; the
cancellation is in dimensionless tick counts. `input_delay = 3` is simply the smallest integer that
drives lead to zero under stock margins; larger values make it more negative.

## The lead arithmetic — why `input_delay = 3` is exactly the boundary

`SyncedTimeline::sync_objective for InputTimeline`, `lightyear_sync-0.28.0/src/timeline/input.rs:285-315`:

```rust
let network_delay    = TickDelta::from_duration(ping_manager.rtt() / 2, tick_duration);   // :293
let jitter_margin    = TickDelta::from_duration(
    config.sync.jitter_margin(ping_manager.jitter(), tick_duration), tick_duration);      // :294
let input_delay: TickDelta = Tick(self.context.input_delay_ticks as u32).into();          // :300
let sync_error_margin = TickDelta::from_duration(
    tick_duration.mul_f32(config.sync.error_margin), tick_duration);                       // :301
let obj =
    remote + network_delay + jitter_margin + TickDelta::from_i32(1) + sync_error_margin
        - input_delay;                                                                     // :313-315
```

`SyncConfig::jitter_margin` (`sync.rs:126-128`) is `jitter * jitter_multiple + tick_duration *
jitter_margin`, and `SyncConfig::default()` (`sync.rs:131-140`) sets `jitter_margin: 1.0` and
`error_margin: 1.0`. So, in ticks:

```
lead = rtt/2 + (jitter · jitter_multiple + jitter_margin) + 1 + error_margin − input_delay
```

At `rtt = 0, jitter = 0` every measured term drops out and only the constants remain:

```
lead = 0 + (0 + 1.0) + 1 + 1.0 − 3 = 0
```

Exactly zero — the three constants cancel the input delay. And the sync controller is permitted to
sit up to `error_margin` behind the objective without correcting (`SyncContext::speed_adjustment`,
`sync.rs:179`, only acts once `offset.abs() > config.error_margin`), so the realised **integer lead
ranges over `{-1, 0}`**.

The client's `LocalTimeline` is driven by this timeline (the driving synced pipeline applies its
`SyncEvent` delta to `LocalTimeline`, `sync.rs:420-438`), and `LocalTimeline::tick()` is precisely
the `current_tick` the prediction gate compares against (`registry.rs:1214`). So a confirmed update
for server tick `T` arrives while `current_tick` is `T` (lead 0) or `T − 1` (lead −1). In both cases
`confirmed_tick >= current_tick`.

Generalising: **lead ≤ 0 whenever `input_delay >= rtt/2_ticks + jitter_margin_ticks + 1 +
error_margin`**, i.e. `input_delay >= 3` on stock margins at loopback, and correspondingly higher on
a real link.

## Root cause — two skips that each assume the other one covers it

### Skip 1 — receive time: strict `<` excludes equality, and nothing revisits

`PredictionRegistry::record_confirmed_and_maybe_check`, `registry.rs:386`:

```rust
let should_rollback = if check_mismatch
    && confirmed_tick < current_tick                 // registry.rs:427
    && !history_was_pruned_past_confirmed
{
    let history_value = predicted_history.as_ref().and_then(|h| h.get(confirmed_tick));
    self.should_rollback_check(confirmed_component.as_ref(), history_value)
} else {
    false
};
```

The comparison is strictly less-than, so `confirmed_tick == current_tick` takes the `else` branch.
The function then logs (`registry.rs:448-457`):

```rust
if check_mismatch && confirmed_tick >= current_tick {
    trace!(
        target: "lightyear_debug::prediction",
        kind = "confirmed_history_future_skip_mismatch",
        ...
        "skipping rollback check until local prediction reaches confirmed tick"
    );
}
```

The message promises a later check. **No such later check exists.** `should_rollback` is the only
thing that reaches `StateRollbackMetadata::record_mismatch`, and all four call sites are guarded by
it (`registry.rs:1086`, `:1114`, `:1136`, `:1294`; the definition is `manager.rs:205`). If the gate
does not pass, no mismatch is recorded, no pending obligation is stored, and the tick is not
enqueued anywhere. The confirmed value is written to `ConfirmedHistory` (correctly) and is then only
ever read by a rollback that this path can no longer trigger.

### Skip 2 — completed-tick fallback: skips on a premise Skip 1 invalidated

`check_rollback`, `RollbackMode::Check` arm, `rollback.rs:583`:

```rust
if confirm_history.contains(server_confirmed_replicon_tick) {
    trace!(... "Skipping unchanged rollback check for entity explicitly confirmed at completed mutate tick");
    return
}
```

justified by the comment at `rollback.rs:564-570`:

> Do not use `ConfirmHistory::last_tick()` for this skip. […] if `ConfirmHistory::contains` resolves
> the completed Replicon tick, **receive-time history writes already checked the explicit state for
> that tick.**

That premise is false exactly when Skip 1 fired. For an entity explicitly confirmed at the
checkpoint, `contains` is true, so the fallback returns early — on the belief that receive time
handled it, when receive time declined to.

### The intersection

An actively driven entity is explicitly confirmed at every checkpoint, so it always takes Skip 2.
When lead ≤ 0 it always takes Skip 1 as well. There is no third route. `has_mismatch`
(`rollback.rs:534`, `manager.rs:215`) reads only what `record_mismatch` wrote, and `record_mismatch`
was never reached.

### Guard asymmetry worth resolving either way

The two sites ask the same question with different strictness, undocumented:

- `rollback.rs:529` — `if server_confirmed_tick > tick { debug!("Confirmed mutate tick is in the future…") }` — **permits equality**, i.e. treats `confirmed == current` as checkable.
- `registry.rs:427` — `confirmed_tick < current_tick` — **excludes equality**, i.e. treats `confirmed == current` as future.

At minimum these should agree, or the divergence should be commented.

## Why the existing mitigations don't apply

- **`history_was_pruned_past_confirmed` (`registry.rs:424`)** guards the *older-than-history* case.
  It is orthogonal; here the confirmed tick is newer, not older.
- **`should_check_mismatch_at` (`manager.rs:226`, applied at `registry.rs:1226-1227`)** suppresses
  re-checks of already-processed or already-mismatched ticks. It is a de-duplicator, not a deferral
  queue — it never schedules anything.
- **`has_confirmed_tick_advanced` (`manager.rs:329`)** only decides whether the completed-tick scan
  runs at all this frame; it does not change what Skip 2 does once it runs.
- **The `1b43ae86` sync-floor fix** raised the objective by `+1 + error_margin`, which pushes lead
  positive for stock/no-input-delay configs — but it scales with the margins, not with
  `input_delay`, so any `input_delay` large enough still lands lead at 0 or below. That commit's own
  body says so (below).

## History — why this is currently believed handled

Chronological, all verified against the GitHub API on 2026-07-29:

1. **Issue #1402** (nuzzles, 2026-02-09) — "Default `no_input_delay` config doesn't reliably deliver
   inputs on localhost due to sync error margin tolerance." Closed 2026-05-18.
2. **PR #1472** (mmannerm) — "fix(prediction): clamp `confirmed_tick > local` in `check_rollback`."
   Closed **unmerged** 2026-05-17. It fixed the pre-Replicon form of this defect by clamping. The
   author closed it himself on the belief that the incoming Replicon path already solved it
   ([comment 4462541966](https://github.com/cBournhonesque/lightyear/pull/1472#issuecomment-4462541966),
   2026-05-15): "the replicon version stores mismatches at receive time and consumes them when the
   local tick is ready", "no longer silently dropped — it's deferred until local catches up, then
   consumed." The maintainer's closing reply (2026-05-17): "Thanks for the analysis; yes it looks
   like this particular case is now handled!"
   **In 0.28 no such deferred consumption exists** — `record_mismatch` is reachable only through the
   gate that skips this case.
3. **PR #1479 → commit `1b43ae86`** (maintainer, merged 2026-05-18), "fix(sync): raise
   sync_objective floor to remote + 1 (#1402) (#1479)". Body:
   > "Any user who tightens `jitter_margin` below 1.0 (a legitimate config for snappier sync) or
   > runs with `input_delay > network_delay + jitter_margin` re-opens the bug."

   That is a precise description of the configuration in *Minimal reproduction*. (The exact
   post-fix threshold is `input_delay >= network_delay + jitter_margin + 1 + error_margin`, since
   `1b43ae86` itself added the `+1 + error_margin` terms; the qualitative statement stands.)
4. **PR #1505** (monsterrdev, superseded) — reported future-confirmed updates from a 100-NPC,
   64 Hz-sim / 32 Hz-replication setup. Maintainer, 2026-06-10
   ([comment 4671091458](https://github.com/cBournhonesque/lightyear/pull/1505#issuecomment-4671091458)):
   > "It should be very rare/impossible to receive a confirmed update in the future compared to the
   > predicted updates."

   Then, 2026-06-12: "I think I will instead split the `PredictionHistory` and the
   `ConfirmedHistory` to make this easier to reason about." #1505 closed in favour of #1507.
5. **PR #1507 → commit `6b29a12c`** (maintainer, merged 2026-06-13), "feat(prediction): split
   ConfirmedHistory and PredictionHistory". This commit **introduced** the `confirmed_tick <
   current_tick` gate — the string is absent at parent `ef5639ff` (whose
   `add_confirmed_and_check_rollback` reads `if check_mismatch &&
   !history_was_pruned_past_confirmed`, with no confirmed-vs-current comparison at all) and present
   at `6b29a12c` and on `main`. Motivation, from the commit body:
   > "if a confirmed value in the future (after the latest predicted tick) is inserted, we do not do
   > rollback checks and we get out-or-order values in the buffer"

   And the PR body is explicit that the other half was deferred, under a heading **"Note on future
   confirmed updates"**:
   > "If a confirmed update arrives ahead of the local prediction timeline, there may be no predicted
   > value to compare at insertion time. **This PR does not add a separate "check later when the
   > local timeline reaches that tick" pass.** […] This is still an unusual state, and the main
   > rollback trigger remains the latest globally completed server mutate tick."

So the gap is documented upstream. What has changed is the premise: the state is not unusual under
input-delay-dominant configurations, it is the steady state, and the "main rollback trigger" named
as the backstop is exactly the completed-tick path that Skip 2 disables for these entities.

## Test coverage currently pins the broken half

`crates/tests/src/client_server/prediction/rollback.rs` on `main` has three tests in this area, and
all three assert only the negative:

- `test_future_confirmed_value_is_not_checked_by_unchanged_completed_tick` — inserts a confirmed
  value at `client_tick + 2`, steps one frame, asserts no rollback ("future confirmed sample should
  not rollback before local prediction reaches its tick"); **then steps two more frames, so the
  local tick has passed the confirmed tick, and asserts no rollback again** ("explicitly confirmed
  samples are skipped by the unchanged completed-tick scan").
- `test_future_confirmed_insert_is_not_checked_by_unchanged_completed_tick` — same shape,
  `frame_step(3)`, asserts no rollback.
- `test_future_completed_mutate_tick_is_not_marked_processed` — asserts only that the future tick
  is not marked processed.

No test asserts that reconciliation *eventually* happens. The second half of the first test asserts
that it does not. A fix will have to change that assertion; flagging it so the change is not read as
a regression.

## Two candidate fixes

### (a) Relax the gate to `<=`

`registry.rs:427` becomes `confirmed_tick <= current_tick`, matching `rollback.rs:529`.

- Small, local, removes the undocumented asymmetry.
- **Incomplete**: it fixes lead == 0 only. At lead == −1 — reachable from the same config purely
  through the sync deadband, no config change — `confirmed_tick == current_tick + 1` and the entity
  is still skipped by both routes. Given that lead oscillates over `{-1, 0}`, this converts a
  permanent skip into an intermittent one.
- It also does not restore the property the trace text advertises.

### (b) Keep the gate, add the deferred obligation the trace already promises

Record a pending obligation for `confirmed_tick`, and discharge it once `current_tick` reaches it,
comparing the stored authoritative value at `confirmed_tick` against the prediction history at
`confirmed_tick`.

- Complete: covers every lead ≤ 0, not just the equality case.
- Delivers what `"skipping rollback check until local prediction reaches confirmed tick"` claims,
  and what PR #1472 was closed believing already existed.
- Preserves `6b29a12c`'s motivation: nothing out-of-order is inserted into `PredictionHistory`; only
  the *comparison* is deferred.
- **Caveat from our own attempt, offered as a warning rather than a design.** We prototyped a
  deferred consumer that recorded markers for genuinely future ticks and consumed them forward. It
  produced a **~7.5 GB RSS blowup in 3 of 4 runs** — obligations accumulated faster than they were
  discharged, and forward-consumption relabelled authoritative state from tick `T` as state for the
  tick that happened to be current when the marker was drained. The rule that made it safe:
  **an obligation stays pending while `T >= current` and is discharged exactly once at `T`;
  authoritative state from `T` is never relabelled as state for `current - 1`.** Bounding the
  pending set is mandatory — `StateRollbackMetadata`'s existing 64-tick mask
  (`manager.rs:205-212`) is the natural bound and already returns `false` for
  out-of-window ticks.

Our view is that (b) is the correct fix and (a) is at best a partial mitigation, but the choice
between them — and whether the bound should be the existing mask or something else — is the
maintainer's. We are not asking for a decision from us; we are asking that the case be treated as
reachable rather than pathological.

## What we can supply

A minimal Bevy integration test in the shape above (zero-RTT loopback, `fixed_input_delay(3)`,
continuously-mutated predicted component, forced prediction error) that is RED on 0.28.0 — the thing
the maintainer asked for on #1505 and did not get. Say the word and we will adapt ours to the
upstream test harness.

---

## Verification ledger (2026-07-29)

**Verified against the published crate sources** (`~/.cargo/registry/.../lightyear_prediction-0.28.0`,
`lightyear_sync-0.28.0`; `Cargo.lock` confirms an unmodified crates.io checkout, checksum
`4ccf8a5a…`, and our `[patch.crates-io]` block touches only `bevy_*`):

- `registry.rs:426-429` gate, exact text and strictness.
- `registry.rs:448-457` trace `kind`, target and message text.
- All four `record_mismatch` call sites gated by `should_rollback`; `manager.rs:205` is the only
  writer of the mismatch mask, `manager.rs:215` the only reader.
- `rollback.rs:529` `>` vs `registry.rs:427` `<` asymmetry.
- `rollback.rs:583` early return and the `:564-570` justifying comment.
- `input.rs:285-315` objective formula; `sync.rs:126-128` `jitter_margin`; `sync.rs:131-140`
  defaults (`jitter_margin: 1.0`, `error_margin: 1.0`, `jitter_multiple: 4`); `sync.rs:179`
  deadband; `sync.rs:420-438` driving-timeline → `LocalTimeline`; `registry.rs:1214` `current_tick`
  source. Arithmetic re-derived from these values, not restated from memory.

**Verified against the GitHub API / raw file fetches:**

- `6b29a12c` exists (`6b29a12c834a52f96e4897c2f09baff8e55860be`, 2026-06-13T19:21:27Z), belongs to
  PR #1507, and its body contains the quoted "out-or-order values in the buffer" sentence (the typo
  is upstream's).
- The gate is absent at parent `ef5639ff57add4cbe5b4915822c3256b5bda4866` and present at
  `6b29a12c` and on `main` — verified by fetching the file at each ref.
- PR #1507 body contains the "Note on future confirmed updates" section, quoted above.
- PR #1505 maintainer comment 4671091458, quoted above.
- Commit `1b43ae86` body contains the `input_delay > network_delay + jitter_margin` sentence.
- Issue #1402 (closed 2026-05-18) and PR #1479 (merged 2026-05-18) titles/status.
- PR #1472 (mmannerm) closed unmerged 2026-05-17; comment 4462541966 is by **mmannerm**, not the
  maintainer.
- Upstream `main` still contains the gate, at `crates/replication/prediction/src/registry.rs`.
- Latest release is 0.28.0 (2026-06-26).
- The three test names and their assertions in `crates/tests/src/client_server/prediction/rollback.rs`.

**Corrected from our working notes:**

- The path `crates/lightyear_prediction/src/registry.rs` is wrong at every ref. Table at the top.
- The `git log -S` claim ("exactly one commit") was **not** verified as stated — we do not have a
  clone. The equivalent fact (absent at parent, present at `6b29a12c`) was verified by fetching the
  file at both refs, and that is what the report claims.
- The maintainer's `input_delay > network_delay + jitter_margin` threshold predates the
  `+1 + error_margin` terms that `1b43ae86` itself added; the report states the post-fix threshold
  and cites the sentence as the qualitative claim it is.
- `jitter_multiple = 2` is our config, not a trigger condition — at zero jitter it contributes
  nothing. Dropped from the repro. Upstream's default is 4.

**Not verified / deliberately excluded:**

- Nothing in the report rests on the belief that the maintainer *intended* the gate to be permanent;
  #1507's body says the opposite and is quoted instead.
- The exact character-level fidelity of PR/comment bodies routed through page fetch. The three
  load-bearing quotes (#1505 comment, `1b43ae86` body, #1507 body) were re-pulled through narrow
  endpoints; they should still be eyeballed in a browser before the issue is sent.
- Our 7.5 GB RSS figure is our own measurement on our own prototype, presented as such. It is not
  reproducible from upstream code and is offered only as a hazard note.

---

## For us, not for upstream

**This is not a blocker.** We are routing around it, and the workaround is entirely in our tree.

- The mechanism, the lead arithmetic, and the two gates are documented in-tree in
  `src/net/lead_zero_rollback.rs`, whose fixtures are RED on the current design and `#[ignore]`d
  deliberately as the acceptance test for the authoritative-facts work. The lead-arithmetic guard in
  that module is **not** ignored: it passes today and fails loudly if anyone edits a sync constant
  without noticing that `jitter_margin 1.0 + 1 + error_margin 1.0 − input_delay 3` cancels to zero.
- Our older fixtures (`net::arrival_rollback`, `net::hull_shock_rollback`) run at a lead of 8 with a
  deliberately anchored-away `ConfirmHistory`, which routes around **both** gates. They are green
  and they always were; they simply do not exercise the shipping regime. Worth remembering as a
  general lesson: a fixture built to isolate a mechanism can isolate it right past the gates that
  decide whether the mechanism ever runs.
- Do **not** ship a naive deferred-consumption patch of our own as a vendored fix. That is the
  7.5 GB path. If we vendor anything before upstream moves, the safe minimum is (a) — relax
  `registry.rs:427` to `<=` — which fixes lead 0 and leaves lead −1, and should be paired with
  pinning lead strictly positive.
- **Cheapest configuration-level escape**, if we want the defect off the table without vendoring:
  keep lead ≥ 1 by construction, i.e. hold `SHIPPING_INPUT_DELAY_TICKS` below
  `rtt/2 + jitter_margin + 1 + error_margin`. At loopback with stock margins that means
  `input_delay <= 2`. That trades input-delay depth for reconciliation, so it is a product call, not
  a free one — but it is a one-constant change and it is reversible.

**What retires the workaround:** an upstream release in which either (a) or (b) has landed *and*
`crates/tests/.../rollback.rs` contains a test asserting that a confirmed sample at
`confirmed_tick >= current_tick` **does** eventually cause a rollback. Until that positive assertion
exists upstream, a green upstream suite is not evidence, because today's suite pins the broken
behaviour. When it lands: un-`#[ignore]` the `lead_zero_rollback` fixtures, confirm they go green
against the new version, and keep the lead-arithmetic guard permanently — it is cheap and it guards
a four-constant coincidence that no one will remember.
