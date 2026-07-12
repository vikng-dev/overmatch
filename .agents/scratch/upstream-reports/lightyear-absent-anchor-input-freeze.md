# lightyear 0.28: an `Absent` anchor permanently freezes the server's `ActionState` — held buttons keep firing after release

**Target:** github.com/cBournhonesque/lightyear · crates: lightyear_inputs 0.28, lightyear_inputs_native 0.28, lightyear_sync 0.28
**Severity for us:** CRITICAL — the server spends ammo and deals damage for inputs the player never authored, with nothing to roll back · **Status:** unfiled
**Lineage:** supplements OPEN issue [#1559](https://github.com/cBournhonesque/lightyear/issues/1559) (filed 2026-07-05, maintainer unconvinced: *"I still am not clear on what the problem actually is"*). We did not file #1559; this report adds two new seed routes, the end-to-end consequence, and a runnable reproduction. Also implicates [PR #1471](https://github.com/cBournhonesque/lightyear/pull/1471) (MERGED — `pop_keeping_last`), whose stated purpose this case defeats.

## Suggested title

`Absent`-anchored `SameAsPrecedent` chain permanently freezes server `ActionState`; adaptive input delay seeds it in ordinary play

## Mechanism (all verified in vendored 0.28.0 source)

A single `Compressed::Absent` entry poisons the server's `InputBuffer` for as long as a button is held:

1. **Unresolvable chain.** `get()` maps `Absent → None` and resolves `SameAsPrecedent` by recursing to
   `tick - 1` (`lightyear_inputs/src/input_buffer.rs:294-311`). A `SameAsPrecedent` chain anchored on an
   `Absent` therefore never resolves.
2. **`get_last()` dead-ends too.** It delegates to `get(end_tick)` (`input_buffer.rs:339-345`) and does
   **not** walk back past the `Absent` — so the buffer's fallback returns `None` in exactly the state
   where a fallback is needed.
3. **`pop_keeping_last()` degrades to `pop()` and WIPES the buffer.** Its guard is
   `get_last_with_tick()`, `None` in this state, so it falls through to `pop()`
   (`input_buffer.rs:227-237` → `246-251`), which clears the whole `VecDeque`. The server calls
   `pop_keeping_last(tick - 1)` every tick (`server.rs:763`). **PR #1471 added `pop_keeping_last`
   specifically to prevent a buffer wipe; it does not cover the `Absent` case.**
4. **The apply is silently skipped.** `update_action_state` (`server.rs:704-707`) has no `else` arm:
   when `get_predict(tick)` is `None` it does not touch the component, so the server's
   `ActionState<T>` stays **frozen at its last value** — indefinitely.
5. **Held inputs never re-anchor.** While a button is held (no value changes), every subsequent input
   window is all-`SameAsPrecedent` behind the `Absent`, so the chain stays unresolvable. #1559 puts it
   precisely: *"presses work, holds freeze."*

## Consequence (the part not yet engaged with upstream)

In a **server-authoritative game with client-side prediction**, a frozen `ActionState` means the server
keeps simulating a **held fire button the player has already released**. It spends ammo and deals damage
for ticks the client never authored — and unlike rollback netcode, there is **nothing to un-simulate**:
the damage is already replicated to the victim. #1559's reporter observed the same class (a rocket
detonating every 0.7 s from an abandoned body). In overmatch it manifests as **1–2 unrequested machine-gun
rounds after the player releases fire**, intermittently.

This distinction matters for triage: repeat-last input extrapolation is *safe* in rollback netcode
(nothing is committed) and *impossible* in Unreal-style reliable move queues (no gap to fill). It is
unsafe in exactly one family — server-auth + prediction, where the server commits irreversibly. That is
the family lightyear serves.

## Seed routes — how an `Absent` enters the buffer in ORDINARY play

**(a) Adaptive input delay — a stock lightyear preset. [NEW, ours]**
`InputDelayConfig::balanced()` recomputes `input_delay_ticks` from live RTT + jitter on **every**
`SyncEvent` (`lightyear_sync/src/timeline/input.rs:52-59`, `recompute_input_delay_on_sync`;
`rtt_ticks = ceil((rtt + jitter_margin) / tick_duration)`, capped by
`maximum_input_delay_before_prediction`). But:
- `buffer_action_state` writes at `tick + input_delay()` in **FixedPreUpdate**;
- `prepare_input_message` computes `end_tick = LocalTimeline::tick() + input_delay()` in **PostUpdate**, every frame.

When the delay **increments between those two points**, `end_tick` lands one tick past the client's own
buffer end, and native `build_from_input_buffer` encodes that trailing tick as `Compressed::Absent`
(`lightyear_inputs_native/src/input_message.rs:99-104`). The source already doubts this:
`// TODO: why is it marked as absent instead of SameAsPrecedent??` (`input_message.rs:158-160`).

On a WAN link whose RTT sits near a tick boundary the delay oscillates continuously, so this fires
repeatedly — **this is the intermittency**. Measured on our link: delay flipping 2↔3 at 64 Hz.

Note this is the **second** critical defect we have traced to `balanced()` — see
[lightyear-check-starvation.md](lightyear-check-starvation.md) (rollback silently disabled at zero
prediction margin). The adaptive-delay recomputation is not safe against lightyear's own buffer
invariants, which assume `end_tick` advances by exactly +1 per tick.

**(b) Connect time — guaranteed.** (Already noted in #1559.) A client's first input messages necessarily
cover windows referencing pre-connect ticks, so `build_from_input_buffer` encodes an `Absent` head every
session.

**(c)** `SyncEvent` tick snaps / rollback bursts underrunning the client's popped head (#1559 route 2).

## Evidence

- **Reproduction repo (runnable):** `lightyear-repro-1559` — failing tests against lightyear's public API
  (`get_last()` returns `None` behind an `Absent`; `pop_keeping_last` wipes the buffer; a held button never
  re-anchors) plus a headless client+server that holds a fire button, releases it, and prints
  *"player authored N rounds; server fired M rounds"* with M > N, using the stock `balanced()` preset.
- **Our scenario sweep** (8192 configs through the real lightyear pipeline —
  `build_from_input_buffer` → `update_buffer` → `get_predict` → `pop_keeping_last`): shipping config
  (`balanced()`) leaked in **4352 server-side / 4928 client-side** cases; with a **constant** input delay,
  **0**. Packet loss alone produced **0** leaks — the defect is the delay wobble, not starvation.
- Our detector for extrapolated ticks (`get(tick).is_none() && get_last().is_some()`) is **structurally
  blind** here: both conjuncts read false, because `get_last()` also dead-ends on the `Absent`.

## Suggested upstream fix directions (not prescriptive)

1. Do not encode an `Absent` tail in `build_from_input_buffer` — clamp `num_ticks` to the client buffer's
   real range (the existing `TODO` already gestures at this).
2. Make `get_last()` walk back to the last **resolvable** entry rather than dead-ending on `Absent`
   (this alone un-breaks `pop_keeping_last`'s guard, restoring PR #1471's intent).
3. In `pop`, carry the last resolvable `Input` as the new anchor instead of materialising `Absent` forward.
4. Give games an API to ask *"was this tick's input authored, or inherited?"* — issue [#492](https://github.com/cBournhonesque/lightyear/issues/492) proposed a per-action `handle_missing_input` hook; it was never implemented. Without it, every server-auth game must re-derive provenance itself.
5. Independently: make the adaptive delay recomputation preserve `Δend_tick == 1`, or document that
   `balanced()` is unsafe with the native input buffer.

## Our workaround (shipped)

- **Pin the input delay** — `fixed_input_delay(3)` replacing `balanced()`, restoring `Δend_tick == 1` and
  removing seed route (a). Guarded by a tripwire test that fails if an adaptive delay is ever
  reintroduced. Does **not** close route (b).
- **`TankCommand.for_tick`** (PROTOCOL_REV 5) — positive input attestation: the sim refuses to commit any
  *discrete* action (fire) on a command it cannot attest was authored for that exact tick. This is immune
  to `Absent` anchors, `SameAsPrecedent` gap-fills, reordering, and any future buffer regression, and it
  **replaces** our previous detector rather than stacking another special case on it.

**Removal condition:** when upstream resolves the `Absent` anchor (fix directions 1–3), `for_tick` may
stay (it is cheap insurance and canon-adjacent — Source's `CUserCmd` carries `tick_count` +
`command_number`), but the input-delay pin may be retired and `balanced()` reconsidered — *provided*
[lightyear-check-starvation.md](lightyear-check-starvation.md) is also resolved, since that defect is
also `balanced()`-triggered.

## Related, separately filable

`InputChannel` is registered `ChannelMode::UnorderedUnreliable` (`lightyear_inputs/src/plugin.rs:28`)
while its own doc comment states *"This is a Sequenced Unreliable channel… out-of-order delivery is
handled by sequencing"* (`lightyear_inputs/src/lib.rs:25-31`). `SequencedUnreliable` exists
(`lightyear_transport/src/channel/builder.rs:366`) and is not used. Consequence: reordered older input
messages **are** delivered; the client guards against stale ones (`client.rs:868`) but the server's
receive path does not, and `update_buffer` sets `last_remote_tick` unconditionally
(`input_message.rs:218`), so a reordered old packet rolls `last_remote_tick` **backward**. Either the doc
or the registration is wrong — a one-line answer either way. See `SECOND-ISSUE.md` in the repro repo.
