# Authoritative shooting-event transport — decision record

**Scope.** Bevy + Lightyear **0.28** over UDP; 64 Hz; 20–30 players.  A tank can
produce two 750-rpm MG shots from one trigger, so the stress case is 60 public
fires in one server tick.  This is a transport/presentation recommendation, not
an authority change: the server remains the only source of damage and combat
state, and a missing cosmetic event must not change gameplay correctness.

**Implemented decision in one sentence.** Send every public fact for automatic
weapons in small, independently packed, bounded-TTL `UnorderedUnreliable`
batches with two scheduled re-copies and `ShotId` dedup; send every public fact
for sparse single-shot weapons, plus owner-private damage receipts, as individual
`UnorderedReliable` messages. Do **not** add an unreliable first copy merely to
make those reliable facts "fast": in Lightyear, a newly queued reliable message
is eligible on its first send pass; the repair retry only happens after loss.

The exact byte budgets, repeat count, TTL, and priority must be chosen from the
harness measurements at the end of this document; numbers stated below are
explicitly labelled DERIVED or MEASUREMENT questions.

## Evidence and terms

### SOURCED FACT — Lightyear 0.28 delivery and scheduling

- Lightyear exposes unordered unreliable, sequenced unreliable, unordered
  reliable, sequenced reliable, and ordered reliable modes.  The unordered
  reliable contract is retries/ACKs without the ordered receiver gate; ordered
  reliable is the mode that guarantees application delivery order.
  [Lightyear 0.28 `ChannelMode`](https://docs.rs/lightyear/0.28.0/lightyear/prelude/enum.ChannelMode.html)
- The 0.28 reliable sender records a newly buffered message with no prior
  send-time, selects it on its next send pass, and retains it until ACK.  A
  resend is eligible only after `max(rtt_resend_factor * current_RTT,
  rtt_resend_min_delay)`; defaults are factor **1.5** and zero minimum.  There
  is no `ReliableSettings` retry-count or expiry field.  This proves the
  important distinction: reliable is not a mandatory one-RTT delay before its
  first transmission, nor is it a deadline guarantee.
  [Reliable settings](https://docs.rs/lightyear_transport/0.28.0/src/lightyear_transport/channel/builder.rs.html)
  and [sender implementation](https://docs.rs/lightyear_transport/0.28.0/src/lightyear_transport/channel/senders/reliable.rs.html)
- First-send time is still scheduling time: a channel's `send_frequency` gates
  send attempts (the default duration attempts each update if possible), and
  the message/channel priority only matters when Lightyear's bandwidth priority
  mechanism has a quota to arbitrate.
  [Channel settings](https://docs.rs/lightyear/0.28.0/lightyear/prelude/struct.ChannelSettings.html)
  and [priority manager](https://docs.rs/lightyear_transport/0.28.0/src/lightyear_transport/packet/priority_manager.rs.html)
- Lightyear packs small messages into transport packets and fragments an
  oversized transport message; its 0.28 transport packet maximum is 1200 B and
  its fragment payload maximum reserves header/metadata room.  The project
  source yields a **1,156 B** maximum unfragmented transport-message payload
  (**DERIVED:** 1,200 - 17 - 27). This is a useful local
  guardrail, not an Internet PMTU promise.
  [Packet/fragment source](https://docs.rs/lightyear_transport/0.28.0/src/lightyear_transport/packet/packet.rs.html)
  and [packet builder](https://docs.rs/lightyear_transport/0.28.0/src/lightyear_transport/packet/packet_builder.rs.html)

### SOURCED FACT — UDP constraints

- UDP itself has no delivery, order, flow-control, or congestion-control
  guarantee.  An application using it should control *aggregate* rate, pace
  bursts, and apply congestion consideration to retransmissions too.
  [RFC 8085 §§3.1, 3.1.6, 3.3](https://datatracker.ietf.org/doc/html/rfc8085#section-3.1)
- IP fragmentation should be avoided: losing any fragment loses the whole
  datagram, and RFC 8085 notes that middleboxes may discard fragments.  It
  recommends PMTU information/DPLPMTUD and independently receivable/retryable
  application fragments where needed.
  [RFC 8085 §3.2](https://datatracker.ietf.org/doc/html/rfc8085#section-3.2),
  [RFC 8899](https://www.rfc-editor.org/rfc/rfc8899.html), and
  [RFC 8900](https://www.rfc-editor.org/info/rfc8900/)

### SOURCED FACT — corroborating production transports/engines

- Valve's GameNetworkingSockets keeps reliable messages ordered **within a
  lane**, supports lane priority/weights to control send-side HOL, and explicitly
  says that lane priority controls *wire send order*, not a cross-lane receive
  order guarantee.  Its API can report a full reliable send buffer; reliable
  remains a best effort across connection failure/close rather than an
  end-to-end deadline.
  [Steamworks lanes and receive semantics](https://partner.steamgames.com/doc/api/ISteamNetworkingSockets#ConfigureConnectionLanes),
  [send failure/queue result](https://partner.steamgames.com/doc/api/ISteamNetworkingSockets#SendMessageToConnection),
  and [Valve public transport overview](https://github.com/ValveSoftware/GameNetworkingSockets)
- Godot's official multiplayer documentation similarly separates unreliable,
  unreliable-ordered, and reliable modes, and recommends separate channels so
  unrelated ordered streams do not interfere.  It calls reliable's performance
  cost significant.  This supports channel separation as a general practice,
  but is not evidence that another engine's exact timing applies to Lightyear.
  [Godot high-level multiplayer](https://docs.godotengine.org/en/4.2/tutorials/networking/high_level_multiplayer.html)
- Unreal documents that execution order between reliable and unreliable RPCs is
  never guaranteed.  Therefore this protocol must carry its own causal fields
  (`ShotId`, bounce ordinal, terminal's preceding-bounce count) and never infer
  cross-channel order.
  [Unreal replicated-object execution order](https://dev.epicgames.com/documentation/en-us/unreal-engine/replicated-object-execution-order-in-unreal-engine)

## Latency, HOL, and backlog analysis

### Normal path

**SOURCED FACT.** A fresh Lightyear reliable message can take the same next
channel send opportunity as an unreliable message.  Thus replacing an
individual outcome/receipt's reliable first copy with `unreliable + reliable`
does not inherently reduce normal-path latency.  Both may also be delayed by
the update/send cadence, packet construction, a bandwidth quota, OS queueing,
and the network.

**DERIVED for Overmatch.** Give reliable single-shot facts and owner-private
receipts their own channels and queue them in the same server tick they resolve.
This protects them from *application* ordering behind automatic-fire batches. It
does not make a latency SLA. The project currently leaves Lightyear's bandwidth
limiter disabled, so configured channel priority values are metadata, not an
enforced scheduler policy.

### Loss/retry path

**SOURCED FACT.** The Lightyear default retry eligibility is about 1.5 × current
RTT, after the original first send.  A lost reliable outcome is therefore
normally visible after a loss-detection/retry interval plus one-way delivery,
not simply "one extra frame."  The source exposes no max-retry/expiry policy.

**DERIVED for Overmatch.** For a 60 ms RTT example only, the first retry is
eligible after roughly 90 ms (DERIVED, not measured) plus pacing/queueing; do
not bake this into presentation hold duration.  A held cosmetic shell needs an
explicit bounded wait/fail-closed policy, and should resolve a late terminal
idempotently if still alive.  Under prolonged loss or a failed connection, it
must dissolve/timeout cosmetically and wait for the normal replicated combat
state; it must never grant damage or decide a hit.

### Receiver HOL and sender HOL

**SOURCED FACT.** `UnorderedReliable` avoids the ordered-receiver wait for an
earlier message ID.  It does not make the network independent: all work still
shares packets, the socket, and any enabled priority/bandwidth limiter.

**DERIVED for Overmatch.** Never put a firing sequence, a whole 20-tick window,
or a public FireBurst behind `OrderedReliable`.  `UnorderedReliable` outcomes
can arrive in any order, so the existing causal guards must be protocol, not
channel, logic: `ShotId`; bounce `sequence`; terminal `after_bounces`; and
first-wins receipt/outcome handling.

The reliable sender retains every unacknowledged message and accumulates
priority. Consequently, a stream of automatic-weapon visual facts in a reliable
queue can grow a stale retransmit tail and consume future packet budget precisely
when a fresh cannon continuation or private receipt needs it. A separate channel
mitigates ordering but not the per-link aggregate bottleneck. Keep automatic facts'
retry life in an application-owned TTL window that can be discarded.

## Option comparison

| Choice | Normal path | Loss path / HOL | Burst and stale-backlog result | Decision |
|---|---|---|---|---|
| Per-event `UnorderedReliable` for every fact | Fresh send is eligible immediately; not automatically delayed. | Each lost fact retries after RTT-derived delay; no ordered receive HOL. | At a 60-event volley every client gets 60 retained reliable records (and more outcomes). They continue retrying without TTL/cancellation; stale cosmetic debt competes with new facts. | Reject for automatic-weapon facts; use for sparse single-shot trajectories and private receipts. |
| Unreliable immediate + unordered-reliable repair for all facts | Usually sends duplicate initial bytes; not intrinsically faster than the reliable first send. | Repair is useful only after loss; cross-channel order is not guaranteed. | Doubles normal-path pressure in exactly the burst to protect; a reliable repair can still become stale. | Reject as a blanket scheme. Consider only after measured terminal-hold failures and only with a bounded, separately instrumented critical stream. |
| Fixed-count unreliable copies per event | First copy is immediate; no retry wait. | With independent loss probability `p`, exactly `n` copies miss with `p^n` (DERIVED model; real losses can be correlated). No stale tail after `n`. | Predictable bounded byte multiplier; 3 copies make 10% loss 0.1% and 30% loss 2.7% residual in the independent model. | Use for fire/keyframe cosmetic reconstruction, pending real-loss measurements. |
| One global sliding window | A compact batch can repeat current and very recent facts. | Duplicates are harmless only with true per-shot IDs/dedup. | A 20-tick global window can explode during sustained MG fire and a 60-event volley, make messages fragment, and its capacity policy can starve one combatant. | Reject as the primary window. |
| Per-combatant sliding windows | Naturally bounds each shooter and makes fairness explicit. | Still needs dedup and does not guarantee that every recipient receives a copy. | 30 small batches add framing overhead and can waste packet packing; a shooter that bursts can need many batches anyway. | Use per-combatant queues/fairness, but packetize them into capped global datagrams. |
| Replicated counters/state | Efficient for latest-state recovery. | Cannot recreate each historical muzzle, bore, tracer choice, or ricochet path without extra event data; counters only reveal a gap. | Good audit/diagnostic and optional repair trigger; insufficient public tracer protocol by itself. | Add a monotonic sequence to identity; do not replace events with counters. |
| TTL/priority | Priority selects which eligible data gets packet budget only when the bandwidth scheduler is enabled; TTL prevents useless future work only if the application owns/drop-cancels it. | Priority does not alter delivery correctness or cross-channel order. | Lightyear 0.28's reliable settings have no message TTL/cancel. Application TTL is practical for unreliable cosmetic queues; reliable backlog requires strict admission and monitoring. | Required now: capped TTL-based cosmetic queue and separate channels. Enforced priority is deferred until a measured aggregate budget exists. |

## Recommended wire protocol

### 1. Identity and idempotence (required before changing transport)

**VERIFIED from repository inspection and tests.** The present `ShotId` is
`{CombatantId, weapon, fire_tick}`. Current firing advances each weapon slot at
most once per fixed tick; the 60-fire stress case is 30 combatants times two
distinct weapon slots, so the tuple is unique under the current mechanism.
That rate invariant is part of the identity contract. A future mechanism that
can emit twice from one weapon in one tick must first widen the identity, for
example:

```
ShotId = { match_epoch, CombatantId, weapon, fire_tick, shot_ordinal }
```

where `shot_ordinal` is a deterministic per-(combatant, weapon, fire_tick)
ordinal, or preferably a monotonic per-combatant `shot_seq`; choose one only
after proving rollback gives the same value on replay.  It must be constructed
synchronously at shell spawn from rollback-safe data, never from a loaded
asset or later entity attachment.  This follows the repository's sim/view
invariant.

Receipts use the full `ShotId`. At the client, retain receipt IDs for the
whole identity scope (a `HashSet`, or a carefully designed monotonic sequence
bitmap with non-reuse proof), not a small LRU: a delayed duplicate arriving
after eviction violates "exactly once at presentation." The current scope is
one Lightyear connection and the ledger is cleared on a new connection. If one
connection later spans multiple Battles, add a Battle epoch before reusing ids.
Every public fire/keyframe/terminal path is first-wins/idempotent by that same
ID; terminal records include `after_bounces` and are applied only after the
known bounces or a cosmetic expiry rule.

### 2. Public fire and ricochet: lossy, finite repair

| Field | Recommendation |
|---|---|
| Delivery | `UnorderedUnreliable`; no reliable repair record. |
| Unit | A `CosmeticEventBatch` containing independent facts, split before the serialized message reaches a conservative cap below the **DERIVED 1,156 B** Lightyear ceiling. The implementation uses a **DERIVED 1,100 B** upper-bound cap and sizes Bevy's **MEASURED nine-byte** recipient-mapped entity representation plus Lightyear's **DERIVED four-byte** maximum message ID. `Entity::PLACEHOLDER` encodes smaller and is not an upper bound. |
| Retry | Send each entry on its emission tick plus the next **two** send opportunities (three copies total, DERIVED starting point). Store only the per-entry copy count and expiry, not a 20-tick full snapshot. |
| TTL | Expire at the smaller of the cosmetic catch-up lifetime and a measured send-age budget. After expiry, drop silently and record it. Never fragment a batch; repartition it. |
| Fairness | Queue by combatant, then packetize a round-robin selection into capped batches. This prevents one 2-shot shooter or a pathological stream from consuming every current-tick batch. |
| Priority | Logically below reliable single-shot/receipt traffic. The current numeric priority is not enforced because no Lightyear bandwidth quota is enabled. |
| Receiver | Dedup `ShotId`; accept keyframes/terminals before fire into a bounded `ShotId` outcome buffer. A late/missing fire costs only cosmetics. |

**DERIVED byte warning.** Do not send one `FireBurst` that contains a growing
window.  Pack many *independent small batches* per UDP packet when they fit;
split at the application layer when they do not.  This preserves useful
messages if another batch is lost and avoids turning a 60-event volley into an
all-or-nothing transport fragment set.

### 3. Public single-shot trajectory: sparse reliable facts

| Field | Recommendation |
|---|---|
| Delivery | Individual `FireEvent`, `RicochetKeyframe`, and `ImpactConfirm` facts on a dedicated `UnorderedReliable` channel. These are the sparse, legible cannon trajectory whose start and post-bounce continuation must be repairable. Automatic-weapon equivalents stay in the bounded visual batch. |
| Timing | Queue on the authority tick that resolves it. Do not wait to aggregate a public cosmetic window. |
| Retry | Leave Lightyear ACK/retry enabled. There is no source-backed TTL/cancel control, so admit only sparse single-shot facts, monitor their unacked age/count, and treat connection loss as failure—not an infinite promise. |
| Presentation failure | Hold the visual shell for a measured bounded interval. If no confirm, fade/truncate it; replication remains authoritative. A later confirm can still produce a terminal effect if its `ShotId` is retained. |
| Ordering | `after_bounces` and sequence fields, not channel timing, govern causality. |

Sending an additional unreliable copy of a reliable single-shot fact is **not** the default.
It increases burst bytes but cannot make Lightyear's first reliable scheduling
pass happen earlier.  Revisit only if measurements show that a three-copy
bounded cosmetic outcome fast-path materially improves the hold experience at
the accepted bandwidth; it must still dedup and must never be used for
gameplay.

### 4. Owner-private damage receipt: sparse reliable fact

Send a one-recipient `DamageConfirm { ShotId, damage_tick }` on a separate
`UnorderedReliable` channel. Target the owner captured at
fire time, never `All`, and carry no target/internals.  The privacy boundary is
the server's explicit recipient list; do not rely on a client filtering a
public message.  It contains no replica entity reference, so respawn/despawn
cannot make its identity invalid.

On receipt, atomically insert `ShotId` into the match-lifetime receipt set and
emit the presentation event only on a new insert.  ACK/retry duplicates then
become harmless.  If the connection fails before receipt, the UI may show a
non-authoritative timeout but no combat state is repaired from cosmetics.

### 5. Entity-readiness races

**DERIVED from current protocol shape.** A public `FireEvent` uses an
entity-mapped shooter for remote recoil; mapping can be unresolved while the
stable `CombatantId`/`ShotId` is already valid.  Do not drop the only cosmetic
copy merely because that view entity is not ready.  Buffer a bounded pending
fire keyed by `ShotId` until either its `CombatantId → display entity` mapping
appears or its cosmetic TTL passes.  Outcomes and receipts have no entity
dependency and should be consumed immediately.  The fallback may reconstruct a
tracer without recoil, but cannot change sim state or authority.

## Required queue and failure rules

- Enforce a per-client outbound byte budget and packet rate across *all*
  channels.  Pace a same-tick 60-event volley across the nearest send
  opportunities rather than emitting an unbounded burst.  Record deferred
  age; do not quietly convert excess traffic into IP/Lightyear fragments.
- The public cosmetic queue is droppable: newest/current fire should outrank
  expiring re-copies, and expiration is an intentional visual failure.
- The terminal/receipt reliable queues are non-droppable while the link lives,
  but have a hard operational alert on unacked count, unacked bytes, and age.
  Lightyear's public reliable settings do not offer selective expiry, so the
  safety valve is **admission** (only sparse facts) plus connection lifecycle,
  not pretending a TTL exists.
- Maintain separate per-combatant producers, a global fair packetizer, and
  per-recipient state.  A server broadcast is multiplied by 20–30 recipients;
  judge bandwidth at the server NIC and each client's downlink, not just one
  serialized event.
- Treat loss as possibly correlated.  The `p^n` fixed-copy calculation is a
  useful DERIVED lower-complexity model, not evidence of Wi-Fi/cellular results.

## Measurement plan — real UDP harness

All entries below are **MEASUREMENT questions**, not acceptance claims.  Run
both the current all-reliable/window design and this candidate with the same
message serialization, MTU guard, server tick, and recipient count.  Seeded
loss must apply to whole UDP datagrams and include independent and at least one
bursty/correlated loss mode if the harness supports it.

| Axis | Matrix / capture |
|---|---|
| Loss | 0%, 10%, 30% seeded datagram loss; record seed and whether loss is independent or bursty. |
| Delay | Baseline, then representative one-way latency + jitter combinations; record configured and observed RTT/jitter rather than assuming them. |
| Producers | 1, 20, 30 shooters; include one same-tick 60-event volley and sustained held-MG firing. |
| Recipients | 20 and 30 connected observers; separately report shooter and observer arrival. |
| Wire accounting | Serialized bytes/event and batch; Lightyear message bytes; UDP payload and packet bytes; packets/tick and packets/s; per-client and aggregate server egress; channel framing overhead; number of batches/packet. |
| Fragmentation | Count application batches over the chosen cap, Lightyear transport fragments, and IP-fragment/EMSGSIZE observations. Required target: no shooting-event message crosses the measured safe cap. |
| Arrival latency | Per fact class, emission→first arrival p50/p95/p99/max, separated into first-send success and retry arrival; terminal hold duration and terminal-before/after-fire/keyframe ordering. |
| Queue health | p50/p95/p99/max message age at send; deferred/dropped cosmetic entries by reason; reliable unacked count/bytes/age; priority/budget deferrals; packet loss and retransmit counts. |
| Correctness of presentation | Shot IDs emitted, received, deduped, duplicate-suppressed, late-expired; exactly one cosmetic shell at most per public shot; exactly one hit-confirm presentation per private receipt; missing/truncated cosmetics; no private payload observed by any non-owner. |
| Readiness | Delay replica/entity mapping independently from events; count pending fires resolved with recoil, resolved without recoil, expired, and accidental duplicate spawns. |

### Candidate acceptance questions

1. At the 60-event volley, are all batches below the actual safe serialized
   payload cap and free of Lightyear fragmentation?
2. Does terminal/receipt p99 emission-to-arrival and cosmetic hold p99 meet the
   playtest bar at 10% and 30% loss without a growing unacked reliable tail?
3. Does the public three-copy scheme provide enough visual continuity under
   *correlated* loss, or does it simply overload the link?  If it overloads,
   lower repeat count/TTL or add a selective state repair—not a larger global
   snapshot.
4. Does a synthetic old receipt remain suppressed after hundreds of newer
   receipts, and does a new connection reset that identity scope?
5. When budgets are exceeded, do fresh terminal/receipt records still leave
   before old cosmetic repeats, and is every cosmetic failure observable in the
   trace?

## What sources do not settle

No source can select Overmatch's byte cap, cosmetic hold duration, retry-copy
count, or acceptable p99.  RFCs establish fragmentation/congestion duties;
Lightyear source establishes current send/retry behavior; Valve/Godot/Unreal
corroborate channel separation and lack of cross-stream ordering.  They do not
model this game's projectile lifetime, player perception, actual serialization
size, server egress capacity, or loss correlation.  The harness and a playtest
must settle those values.
