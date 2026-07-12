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

### Scope correction — "permanently freezes" is true but MISLEADING; do not argue it that way

The freeze is unbounded in **duration** but a **no-op in effect** except at one tick, and a maintainer who
tests it naively will see nothing wrong and close the report. The precise statement:

A `SameAsPrecedent` tail exists **only** for ticks whose command equalled its predecessor — that is exactly
what `InputBuffer::set` compresses. So on every tick *inside* the poisoned region, the player's authored
command **is** the frozen command, field for field: freezing it changes nothing. The first tick whose
command **changes** encodes a real `Compressed::Input`, lands past `last_remote_tick`, and re-anchors the
buffer. **A release is a change.** The freeze therefore cannot outlive the input it is freezing.

What survives is the **transition tick**: the delay jump opens a gap tick nobody authored, sitting between
the last *pressed* tick and the first *released* one, and the frozen value there is the old (pressed) one.
Measured across 32 seed positions: **worst case exactly 1 tick.**

**That one tick is the whole bug, and its severity is entirely a function of what the input DOES:**
- a **level** (throttle/steer/aim) → 15.6 ms of stale throttle on a 57-tonne vehicle: ~1 cm, erased by the
  next tick, beneath the suspension noise floor. Harmless. This is why "the server drives your tank away"
  is NOT a claim we make.
- a **consumable** (fire) → a round leaves the barrel. Ammo spent, damage dealt, replicated to the victim,
  **nothing to roll back**.

So the upstream argument must be: *"a discrete, irreversible action commits on a fabricated tick"* — not
*"state freezes indefinitely."* The former is trivially demonstrable and cannot be waved away; the latter
invites a maintainer to hold a stick, observe nothing, and close the issue.

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

## Two FURTHER defects from the same `balanced()` root — no `Absent` required [NEW, ours]

Seed route (a) above shows how a delay change can plant an `Absent`. But a delay change corrupts the
input stream **directly**, without any `Absent` at all, in two more ways. Both were reproduced over the
real pipeline (see Evidence); both produce a value that is byte-indistinguishable from real input, so
neither is visible to any buffer-shape check.

**(d) Delay SHRINKS → `end_tick` STALLS → the client's correction is silently DROPPED.**
`buffer_action_state` writes `set(local_tick + input_delay)`. When the delay drops by one, two
consecutive local ticks write the **same** buffer tick: the client correctly overwrites its own entry
with the newer command and re-sends it, but `end_tick` does not advance, so **every** tick in that
message is `<= last_remote_tick` and `update_buffer`'s write gate refuses all of them
(`lightyear_inputs/src/input_message.rs:195`). The server keeps the **superseded** value — forever.
If the superseded value was `pressed` and the revision was the player's RELEASE, the server fires a
round the client never predicted, off a perfectly ordinary `Compressed::Input(..)` entry.

`last_remote_tick` is a **receipt** watermark being used as a **simulation** watermark. A tick that has
been received but not yet simulated is still correctable. lightyear already *notices* this and logs it —
`detect_input_history_rewrite` (`server.rs:644`) emits *"server received a different input for a future
tick already covered by an earlier client input packet"* — and then drops the correction anyway.

**(e) Delay GROWS → `end_tick` JUMPS → the client's OWN buffer FABRICATES the skipped tick.**
The client skips a buffer tick, and `InputBuffer::set_raw` fills any skipped tick unconditionally with
`Compressed::SameAsPrecedent` (`input_buffer.rs:212`, *"if an input is missing, we consider that the
user repeated their last action"*). That is an **extrapolation written into the buffer as data**.
`get()` resolves it back to `Some(pressed)`, so the client fires the phantom round **itself** and ships
the fabrication to the server as truth — both ends fire a tick nobody authored.

Crucially, **the fabrication is not detectable by shape**: a genuinely HELD button also compresses to
`SameAsPrecedent` (`set()`, `input_buffer.rs:168-175`). They are the same bytes. `set_raw`'s gap-fill and
`set`'s compression need to be *different variants* (e.g. `Fabricated`) if a consumer is ever to tell
"the player repeated this" from "we filled this in" — which is fix direction 4 restated at the buffer
level.

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
- Focused cases in `tests/net_fire_release.rs` (ours, over the real lightyear types):
  `delay_shrink_strands_a_stale_pressed_tick_on_the_server` (defect **d** — the server fires on a
  `Compressed::Input(fire: true)` entry the client had already corrected);
  `delay_growth_fabricates_unauthored_pressed_ticks_on_both_ends` (defect **e** — a 1→3 delay jump
  fabricates TWO ticks and **both ends** fire them);
  `same_as_precedent_cannot_distinguish_fabrication_from_a_held_trigger` (why no shape rule can work);
  `absent_anchor_freezes_the_server_and_blinds_the_held_last_detector` and
  `absent_anchor_propagates_forward_through_pop` (the freeze above, incl. `pop` carrying the anchor
  forward one tick per tick — the poison sustains itself).

## Suggested upstream fix directions (not prescriptive)

1. Do not encode an `Absent` tail in `build_from_input_buffer` — clamp `num_ticks` to the client buffer's
   real range (the existing `TODO` already gestures at this).
2. Make `get_last()` walk back to the last **resolvable** entry rather than dead-ending on `Absent`
   (this alone un-breaks `pop_keeping_last`'s guard, restoring PR #1471's intent).
3. In `pop`, carry the last resolvable `Input` as the new anchor instead of materialising `Absent` forward.
4. Give games an API to ask *"was this tick's input authored, or inherited?"* — issue [#492](https://github.com/cBournhonesque/lightyear/issues/492) proposed a per-action `handle_missing_input` hook; it was never implemented. Without it, every server-auth game must re-derive provenance itself.
5. Independently: make the adaptive delay recomputation preserve `Δend_tick == 1`, or document that
   `balanced()` is unsafe with the native input buffer. Note this single change would close seed route
   (a) **and** defects (d) and (e) at once — every one of them is `Δend_tick != 1`.
6. For (d) specifically: gate `update_buffer`'s writes on the last **simulated** tick, not
   `last_remote_tick`, so a not-yet-simulated tick stays correctable. That needs a monotonic per-message
   sequence number to order two messages sharing an `end_tick` (the stall makes `end_tick` ambiguous).

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

## What fixing this unlocks for us

**Clean up — the pin and its tripwire, not the attestation.**

- Retired: `SHIPPING_INPUT_DELAY_TICKS` / `shipping_input_delay()` (`src/net/client.rs:48-100`) and the
  tripwire `input_delay_is_constant` (`src/net/client.rs:1468`), whose whole job is to fail the build
  if anyone reintroduces an adaptive delay. Both exist *only* because of this defect (plus
  [#1](lightyear-check-starvation.md); see Blocked-by).
- **NOT retired: `TankCommand::for_tick` (PROTOCOL_REV 5) and `fail_consumables_closed`**
  (`src/command.rs:132`, applied by the bridge at `net/protocol.rs:1482`). The invariant is **ours** —
  *a discrete action commits only on a tick the command can attest it was authored for* — and it
  covers cases no `Absent` fix touches: seed route (b) at connect, reordering (see *Related* below),
  the hold-last level freeze under genuine input starvation (commit 2ea6cf5), and any future buffer
  regression. It stays whatever upstream ships.
- What *does* change is its **status**: `for_tick` stops being load-bearing and becomes belt-and-braces,
  so its wire cost becomes **optional rather than paid under duress**. Measured (`input_message_wire_cost`,
  `net/protocol.rs`, real `NativeStateSequence` through real bincode): the stamp changes every tick and
  so defeats `Compressed::SameAsPrecedent` run-compression — **+20 B/message aiming** (156 → 176 B;
  +1.2 KB/s upstream per client at 64 Hz) and **+140 B/message parked-idle** (36 → 176 B; +8.8 KB/s),
  idle being the worst case precisely because it is the only regime where run-compression was still
  paying off. Once the buffer is trustworthy that cost is a *choice*: shrink the attested payload (a
  `u8` tick-LSB, or a fire-fields-only sub-struct carrying the stamp) and keep the invariant, or keep
  paying 20 B and not think about it. Today neither option is open — dropping or thinning attestation
  reopens unauthored rounds.

**Optimize.** Nothing in the sim or the frame budget; the cost of this defect is entirely wire bytes
(above) and the pinned ~47 ms of input latency (below). Note the pin is not *only* a cost: 3 ticks of
delay also buys the deepest input buffer, i.e. the most jitter tolerance before an input goes missing
at all, and the shallowest rollback window. Any retirement trades that away.

**Explore.**

- **Adaptive input delay** (`balanced()`): near-0 delay on a good link, more on a bad one — the way to
  get 0-tick *feel* on a LAN without 0-tick *risk* on a bad connection. **Blocked by TWO reports:** this
  one (a varying delay corrupts the input stream — `Δend_tick != 1`, defects a/d/e) **and**
  [lightyear-check-starvation.md](lightyear-check-starvation.md) (a delay that grows into the link's
  natural lead walks the prediction margin to zero, where rollback is silently dead). Either defect
  alone makes adaptive delay a shipping bug.
- **0-tick input delay** — worth stating exactly, because the intuition runs backwards. `no_input_delay()`
  is `minimum == maximum == 0`: **constant**, so it is *not* blocked by this report (no wobble, no
  fabricated ticks, and the anti-adaptive tripwire passes as soon as the constant is deliberately
  changed). It is also not blocked by #1 — it is #1's *falsifier*: dropping the delay to 0 restores the
  prediction margin. What it costs is stated in #1's unlocks section: deeper rollbacks (chaos
  amplification — wants
  [avian-solver-constraint-order.md](avian-solver-constraint-order.md)) and a zero jitter cushion, so a
  late input becomes a **dropped** trigger pull (we fail closed via `for_tick`) rather than an
  unauthored round. An experiment to run — with `SPIKE_INPUT_DELAY_TICKS=0` the lever already exists —
  not a free win.
- Fix direction **4** (a "was this tick authored or inherited?" API, issue #492) would give us natively
  what `for_tick` re-derives. We would still keep `for_tick`: it is 20 B and it does not depend on
  upstream getting provenance right. But every *other* server-auth game would stop having to invent it.

## Related, separately filable

`InputChannel` is registered `ChannelMode::UnorderedUnreliable` (`lightyear_inputs/src/plugin.rs:28`)
while its own doc comment states *"This is a Sequenced Unreliable channel… out-of-order delivery is
handled by sequencing"* (`lightyear_inputs/src/lib.rs:25-31`). `SequencedUnreliable` exists
(`lightyear_transport/src/channel/builder.rs:366`) and is not used. Consequence: reordered older input
messages **are** delivered; the client guards against stale ones (`client.rs:868`) but the server's
receive path does not, and `update_buffer` sets `last_remote_tick` unconditionally
(`input_message.rs:218`), so a reordered old packet rolls `last_remote_tick` **backward**. Either the doc
or the registration is wrong — a one-line answer either way. See `SECOND-ISSUE.md` in the repro repo.
