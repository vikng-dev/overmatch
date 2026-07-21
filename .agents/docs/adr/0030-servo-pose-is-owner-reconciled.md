# Servo pose is owner-reconciled authority state

> **Status: accepted; shipped in declared `PROTOCOL_REV = 18`.**

The complete turret/gun servo integrator is one root-resident `TankServos` component, replicated by
the authority, predicted by the owner, and restored from confirmed history at the authority sample's
producing tick. Its deterministic slot order follows the existing name-sorted `ServoIndex` order.
Each `ServoState` carries `current`, `previous`, and `velocity`; replay needs all of them to derive
the same fixed-step pose.

## Context

The firing pose and turret collision-proxy pose came from `TankSim::servos`. `TankSim` was
`local_rollback`, so an owner rollback restored the client's own drifted history rather than any
server pose. The authority also published `ServoAngles`, but the client applied that public snapshot
only to non-predicted remotes. The predicted owner therefore had no authority-to-simulation bridge
for turret or gun orientation.

Under jitter, the confirmed capture found a MEASURED servo-pose delta on 56 of 63 storm ticks.
Controlled A/B runs found a MEASURED turret-proxy contribution up to 103 kN in `demand_n`; firing
also reads the same pose for `bore = muzzle_rotation * -Z`, then applies recoil opposite that bore.
Servo drift could therefore seed both contact-force and recoil-force divergence before the exact
`TankTransmission` gate correctly reported the downstream mismatch.

This is the same state rule established by
[[0029-weapon-gate-is-tick-correlated-authority-state]]: state that determines physics must be
authoritative, tick-correlated, and rollback-restored before replay. Arrival time must never write
the latest snapshot directly into live owner simulation.

## Decision

Extract the integrator from `TankSim` into:

```text
TankServos {
    states: Vec<ServoState> // deterministic ServoIndex order
}

ServoState field order: current, previous, velocity
```

The extraction is narrower than replicating `TankSim`: recoil spring state and the cosmetic tracer
counter remain local rollback state, while only the pose-determining integrator crosses the wire.
Promoting `ServoAngles` instead would either omit `previous` and `velocity` or turn the established
public remote target stream into a differently shaped simulation component. Keeping the two roles
separate preserves disclosure and remote presentation.

The authority and predicted owner run `restore_servo_truth` and `drive_servos` directly against
`TankServos`. Lightyear registers the component as replicated + predicted with one atomic rollback
condition. Slot count and every float compare exactly by raw bits: matching NaN payloads compare
equal and signed zero remains distinct. The component contains no tick fields, so this change adds
no tick arithmetic; the project's saturating Lightyear tick rule remains unchanged.

`TankServos` is constructed synchronously from `TankSpec` data in the same root spawn batch as
`Tank`, `TankTransmission`, and `WeaponGate`. A joining predicted owner cannot attach or promote its
dynamic body until the private authority snapshot exists. A late-role replica keeps its public
remote mechanism in a distinct local `RemoteServos` component, leaving the arriving `TankServos`
value untouched; promotion removes `RemoteServos` before the first predicted physics tick.

`ServoAngles` remains plain-replicated and public. A non-predicted remote still applies its turret
and gun values as `ServoCommand.target`, and its local `RemoteServos` mechanism performs the same
fixed-step chase and view interpolation as before. `CombatDisclosure` makes `TankServos`
owner-private, so observers and ownerless bots do not receive the exact integrator.

## Ordering, hashing, and regression

Rollback restores `TankServos` before `GameplaySet`; gameplay pose readers then see the restored
sim-node transforms, and `drive_servos` advances the integrator after `GameplaySet`. The input is
already tick-correlated through `TankCommand.aim -> ServoCommand.target`, so ordinary replay derives
the present without another authority field.

The existing `hsrv` stream now hashes the reconciled `TankServos` inventory for the authority and
predicted owner. Its fixed field order is deterministic servo slot order, then `current`,
`previous`, `velocity` per slot. The verbose `simf.srv` array uses the same order. Remote trace rows
hash their local `RemoteServos` presentation state, but authority/owner equality — the storm signal
— is sourced from `TankServos` on both ends.

The storm-killer regression forces a rollback from a deliberately stale live servo component. It
asserts that the producing-tick authority snapshot replaces all integrator fields, the next replay
tick advances that restored state, the firing-tick servo transform matches its reconciled angle by
raw quaternion bits, and the production recoil impulse is exactly opposite the captured replayed
bore. The existing `WeaponGate`, recoil, and hull-velocity assertions remain in the same regression.

## Wire and consequences

The wire adds predicted `TankServos` and embedded `ServoState`; public `ServoAngles` remains in its
existing registration. Declared `PROTOCOL_REV` moves from 17 to 18. MEASURED repins are
`WIRE_SURFACE_HASH = 0x44c3b31a1cdc0134`, `WIRE_TYPES_HASH = 0x66bed94f4232074b`, and
`PROTOCOL_FINGERPRINT = 0x9559994b215667bc`.

- The owner now receives producing-tick authority for every field needed to reproduce turret/gun
  pose; stale local servo history cannot survive reconciliation.
- Interpolated remotes retain the public `ServoAngles` path and do not predict `TankServos`.
- The force law, `TankTransmission` gate, `WeaponGate`, recoil formula, and turret collider coupling
  are unchanged.
- The full jittered combat capture remains product verification. Compare authority/owner `hsrv` and
  `simf.srv`, then watch `TankServos` rollback attribution, recoil bore/impulse, `htrn`, and the raw
  `demand_n` bits. The expected result is no servo-pose delta and bit-identical `demand_n` at the
  former storm ticks.

## Related

[[0014-sim-view-split]] · [[0015-divergence-doctrine]] ·
[[0018-wire-surface-fingerprinted-and-refused]] ·
[[0029-weapon-gate-is-tick-correlated-authority-state]]
