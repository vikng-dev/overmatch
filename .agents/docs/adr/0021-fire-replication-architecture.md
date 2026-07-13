# Shot replication separates trajectory presentation from authoritative consequence

The authority resolves every projectile contact, penetration, ricochet, and damage result. A
client may predict its own firing response immediately, but no transient shot message grants
gameplay authority. Replicated combat state remains the durable result; shot messages explain and
present discrete causes and confirmations.

## Identity and source facts

`ShotId { CombatantId, weapon, fire_tick }` is constructed with the shell at spawn. The current
weapon mechanism emits at most once per weapon slot per fixed tick; that invariant makes the tuple
unique. A future mechanism that can emit twice from one slot in one tick must widen `ShotId` before
shipping.

`FireShell` carries `FireMechanism` from the spawn-ready weapon specification. Transport policy is
therefore selected from simulation data present at spawn, never inferred later from caliber, VFX,
an asset, or a replicated entity's eventual components.

## Delivery classes

| Fact | Delivery | Reason |
|---|---|---|
| Automatic-weapon `FireEvent`, `RicochetKeyframe`, `ImpactConfirm` | `FireVisualBatch` on `UnorderedUnreliable` | Frequent presentation may expire; it must not create reliable cosmetic debt. |
| Single-shot `FireEvent`, `RicochetKeyframe`, `ImpactConfirm` | Individual message on `UnorderedReliable` | A cannon's start and post-bounce trajectory are sparse, legible events worth repairing. |
| Owner-private `DamageConfirm` | Individual message on a separate `UnorderedReliable` channel | An authority-confirmed damaging action must reach its shooter exactly once and must not disclose target internals to observers. |

Automatic facts live in per-combatant queues and are admitted round-robin into bounded batches.
Current-tick facts outrank older repair copies when the admission budget saturates.
Each fact gets three send opportunities and expires after 16 authority ticks. Batches use a
serialized worst-case upper-bound cap of 1,100 bytes and a four-batch per-tick admission budget.
All four values are **DERIVED STARTING DEFAULTS**, not measured product limits. The cap sits below
Lightyear 0.28's **DERIVED 1,156-byte** unfragmented-message ceiling. The sizer includes the
**DERIVED four-byte** maximum message ID and Bevy's **MEASURED nine-byte** encoding of a Lightyear recipient-mapped entity;
`Entity::PLACEHOLDER` is not a valid worst case.

At 64 Hz, the four-batch admission ceiling is a **DERIVED 281,600 application bytes/s per public
recipient** before Lightyear packing. This is a shot-visual work bound, not an observed link rate or a
whole-connection bandwidth reservation; reliable outcomes, replication, acknowledgements, and
control traffic sit outside it.

The authority captures the owning connection when a shot fires. A later damage receipt targets
only that owner and contains stable shot identity plus the authority damage tick—no target entity,
HP amount, crew state, or module state. Public and private facts never share a recipient boundary.

Lightyear's configured channel priority fields are not an enforced bandwidth policy while its
bandwidth limiter remains disabled. Separate channels currently provide delivery and disclosure
seams, not a claimed cross-channel scheduler guarantee. Enabling Lightyear's post-packing token
bucket is blocked on a representative whole-link baseline: it would also cap replication and can
delay a quota-rejected reliable packet until its normal resend interval. No arbitrary quota is a
correctness default.

## Receive rules

- No receive rule infers causality from cross-channel arrival order. `ShotId`, ricochet sequence,
  and `after_bounces` carry the ordering facts.
- A fire whose mapped shooter root is not ready waits in a bounded `ShotId` queue. It resolves only
  to an exact agreeing entity mapping or one unique live `CombatantId`; ambiguity fails closed.
- Ricochet and terminal facts enter the sanctioned-outcome buffer without waiting for a shooter
  entity. A received ricochet re-seeds the cosmetic shell from the authority's post-bounce state.
- Visual batches are deduplicated within their bounded presentation horizon. The **DERIVED
  2,048-entry** fire ledger exceeds the 1,200 distinct IDs accepted across 100 ticks at the current
  30-combatant, dual-750-RPM load envelope. Owner-private damage
  receipts remain deduplicated for the whole connection identity scope and are cleared on a new
  connection. An explicit Battle epoch in `ShotId` is a blocking prerequisite for any session flow
  that keeps one connection alive across Battles; match-local ids may not be reused before it exists.
- A cosmetic shell that reaches armor without a sanctioned outcome holds invisibly for a bounded
  interval, then dissolves. It never improvises the authority's result.

## Evidence and remaining tuning

`net::shot_transport` exposes enqueue, application-send acceptance, expiry, deferral, batch-size,
route-conflict, and error counters. An accepted send call means Lightyear buffered the fact for the
resolved recipients; it is not a delivery acknowledgement. `net::shot_loss` exercises production
protocol registration over real loopback UDP with seeded loss, delayed observer delivery, mixed
single/automatic trajectories, private damage confirmation, and a **DERIVED 30-combatant ×
two-weapon** same-tick volley. Its 30-receiver probe adds a **DERIVED one-second 768-RPM-per-slot**
automatic stream and injects one reliable cannon plus owner-private damage during contention.

**MEASURED on 2026-07-13:** repeated local runs presented all 1,800 volley receiver/shot pairs
exactly once at 10% configured inbound loss, delivered the contended cannon fire exactly once to all
30 receivers, and delivered its damage receipt once to its owner and to no observer. One run took
4.05 seconds; the slowest cannon presentation completed after 44 harness orchestration steps. Raw
counters remain opaque Lightyear link payloads—including control, acknowledgements, and
replication—not per-shot bytes or IP/UDP headers.

The exact automatic copy count, expiry, visual density, and failure presentation remain playtest
and network-measurement decisions. Per-recipient visual interest and an enforced aggregate
whole-link bandwidth budget remain acceptance work before claiming a production 30-player network
envelope; gameplay-visible tanks may not be hidden as an optimization.

## Related

[[0014-sim-view-split]] · [[0015-divergence-doctrine]] ·
[[0016-replicate-causes-derive-consequences]] ·
[[0017-mutual-contact-resolves-on-the-authority]] ·
[[0018-wire-surface-fingerprinted-and-refused]]
