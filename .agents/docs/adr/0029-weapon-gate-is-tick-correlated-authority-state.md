# Weapon gate is tick-correlated authority state

> **Status: accepted; shipped in declared `PROTOCOL_REV = 17`.**

The complete decision that permits a weapon to fire is one root-resident `WeaponGate`, replicated
by the authority and predicted by the owner. Its per-slot state is an absolute `ready_tick`, a
stable crew-work pause tick, and the discrete `belt_remaining`; Lightyear restores the atomic
component at the authority sample's producing tick and replay derives the present. Arrival time
never writes live simulation state.

## Context

The old implementation split one logical gate across two correlation regimes. `belt_remaining`
and the changing `reload_remaining` float lived in local-rollback `TankSim::weapons`, while
authority published only a `NetBelts` snapshot. `apply_net_belts` copied the latest-arriving belt
and swap value directly into the live client sim. While a belt had rounds, the cyclic timer remained
entirely local; only a dry snapshot re-anchored it.

Under jitter, the dry snapshot's arrival tick moved. The resulting local cyclic phase was MEASURED
about 2 ticks from authority; recoil then landed on different ticks, seeded hull-velocity and
contact differences, and amplified into MEASURED `demand_n` deltas up to 5.9 kN. The exact
`TankTransmission` gate correctly reported the consequence: MEASURED 990 rollbacks in 60 seconds
with jitter and zero at zero jitter. Fire identity, simulated-tick input, schedule order, and recoil
application were exonerated by the trace. The arrival-pumped weapon gate was the seed.

This is the same design role as Source's `m_flNextPrimaryAttack`: the client predicts from an
authority-owned next-ready time instead of owning an uncorrelated countdown. It also follows this
codebase's `TankTransmission` precedent exactly: one atomic replicated + owner-predicted component,
an exact rollback comparison, and producing-tick confirmed history.

## Decision

Add `WeaponGate { weapons: Vec<WeaponGateState> }` in deterministic name-sorted `WeaponIndex`
order. Each slot carries:

- `ready_tick: Option<u32>` — `None` with no pause is ready; `Some(tick)` is the absolute cyclic,
  belt-swap, or single-round reload deadline. Comparison uses Lightyear's ordinary numeric tick
  order.
- `paused_at_tick: Option<u32>` — the tick a crew-gated reload or swap stopped. It stays unchanged
  throughout the pause; resuming shifts `ready_tick` once by the elapsed paused interval.
- `belt_remaining: u32` — the complete discrete supply for an `Automatic`; zero for a `Single`.

Lightyear's `Tick` saturates at `u32::MAX`. Deadline arithmetic therefore uses checked addition and
saturating subtraction. If arm, pause, or resume cannot represent the resulting future deadline,
the gate fail-stops as `(ready_tick: None, paused_at_tick: Some(u32::MAX))`. That otherwise
unreachable pair is permanently not-ready: a deadline exactly at `MAX` may mature once, but a
subsequent delay freezes rather than firing repeatedly on the permanently saturated tick.

Mode and belt state derive the phase, so no separate phase tag is needed. For an automatic weapon,
a nonzero belt plus a deadline is cyclic recovery; a zero belt plus a deadline is a crew-gated
swap. For a single-shot weapon, a deadline is its crew-gated reload. An unmet crew requirement
records the pause once, preserving remaining work without a changing value on the wire.

`shooting::fire` reads and advances this component on the current simulation tick. It consumes the
belt and arms the next absolute deadline in the same tick as the unchanged recoil impulse. The
authority remains the final eligibility decision; the owner predicts from the same replicated
gate. A late or unattested input may still briefly predict a round the authority rejects, which is
accepted and self-corrects through ordinary rollback. It cannot permanently phase-shift every later
round because the correction is correlated to its producing tick.

Delete `NetBelts`, `BeltSnapshot`, `publish_net_belts`, and `apply_net_belts`. A new component is
clearer than extending `NetBelts`: the old name and lifecycle described incomplete, arrival-time
telemetry, while the replacement is the complete simulation eligibility gate and participates in
prediction history. `TankSim::WeaponState` retains only local rollback state that does not decide
eligibility: barrel recoil offset/velocity and the cosmetic tracer counter `rounds_fired`.

Construct `WeaponGate` synchronously from `TankSpec` in the root's spawn batch, beside
`TankTransmission`. Replicated client attachment waits for an owner gate and never reconstructs or
overwrites the received value. The component is owner-private through `CombatDisclosure`; observers
do not receive ammunition or readiness facts.

## Quantization and hashing

Authored seconds convert once per shot to `ceil(duration / fixed_tick)` integer ticks. This never
fires early and removes a per-tick-changing float, at the accepted cost that authored RPM lands in a
tick-divisor bucket. For the Tiger MG, authored 750 rpm is DERIVED 5.12 ticks at the declared 64 Hz,
therefore the 6-tick bucket and DERIVED 640 rpm effective cadence. Belt swaps and single reloads use
the same ceiling rule.

The gate joins the existing `hrld` state-hash stream. Its fixed field order is: gate-present bit;
weapon count; then per name-sorted slot, deadline-present bit, optional `ready_tick`, pause-present
bit, optional `paused_at_tick`, and `belt_remaining`. `rounds_fired` remains outside the cross-world
simulation hash because it chooses only tracer presentation, but remains in the same-platform
rollback digest.

The wire change removes plain `NetBelts` and adds predicted `WeaponGate` plus its embedded
`WeaponGateState`. Declared `PROTOCOL_REV` moves 16 to 17. MEASURED repins are
`WIRE_SURFACE_HASH = 0x5e1f8a967ada3e00`, `WIRE_TYPES_HASH = 0x723844250ceddb84`, and
`PROTOCOL_FINGERPRINT = 0x0b5036caa0951f04`.

## Consequences

- A stale arrival can add authority history but cannot rewind the live cyclic phase. Only rollback
  restores the sample at its producing tick before replay.
- Belt count, cyclic readiness, swap progress, and single-shot reload now correct atomically.
- Recoil, the force law, `demand_n`, input delay, and the exact `TankTransmission` gate are
  unchanged. Removing the seed lets that gate silence itself.
- The full jittered two-client combat capture remains the product verification. It should compare
  authority/owner fire ticks and `simf.wpn` gate fields, while watching `WeaponGate` and
  `TankTransmission` rollback attribution, recoil/hull-velocity transients, and `demand_n` bits.

## Related

[[0014-sim-view-split]] · [[0015-divergence-doctrine]] ·
[[0018-wire-surface-fingerprinted-and-refused]] · [[0020-fire-mode-mechanism-enum]] ·
[[0021-fire-replication-architecture]] · [[0022-input-attestation-not-detection]] ·
[[0030-servo-pose-is-owner-reconciled]] ·
`design/sim-divergence-and-determinism.md`
