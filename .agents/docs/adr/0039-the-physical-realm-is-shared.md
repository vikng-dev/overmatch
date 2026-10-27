# 0039 — The physical realm is shared; only added views choose an audience

Status: PROPOSED 2026-10-10. Promotes to ACCEPTED when the first public fact under ruling 3
ships.

## Ruling

1. **Every physical mutation to a tank is visible to anyone with eyes on it**, enemy or ally,
   and is never gated: holes, scrapes, lodged shells, smoke, fire, a burst barrel, a thrown
   track, open hatches, exhaust.
2. **Only views the game adds have an audience**: the corner and the victim's glass. The
   audience is a server-side policy applied per recipient. The policy values live in the
   owner-direction record, not here.
3. **Mechanism** (proposed; none of it is built). A "visible from outside" facet on a
   component entry in the tank file (`VolumeSpec`, `src/spec.rs:36`; data, never a name in
   code, per [[0034-tanks-are-data-no-per-tank-overrides]]) selects the facts that replicate
   publicly beside `NetTankStatus`: one lost-bit per `FunctionRole` the tank file declares
   (`src/damage.rs:141`; engine and gun today). Facts that are not volumes (burning, a track
   side) get public fields of their own when their sim facts exist.
   A public lost-fact is a consequence on the wire, allowed under
   [[0016-replicate-causes-derive-consequences]] only because its cause, owner-only health, is
   unavailable to observers: one derive function, birth at spawn per [[0014-sim-view-split]],
   and a `WIRE_SURFACE` entry with a REV bump per [[0018-wire-surface-fingerprinted-and-refused]].
   `NetCrew` and `WeaponGate` stay owner-only under `CombatDisclosure`
   (`src/net/disclosure.rs:42`); no other internal state is on the wire. The record goes out as
   one new message whose recipients the
   policy picks; the owner-private damage receipt of [[0021-fire-replication-architecture]]
   keeps its boundary and still carries no target internals.

## Why

Two framings had crept in together: the code's owner-only default (`src/net/disclosure.rs:11`)
and the research lanes' "non-text leaks intelligence" balance framing. Combined, they turned
visibility into a per-fact decision and produced "marks are private to the shooter". A mark is
on the hull; hiding it from an observer contradicts the world the players share. The balance
question is real only for the added views, and that is where it now lives.

## What this does not decide

The content of each view and the audience values
(`.agents/scratch/damage-cam-2026-10-08/product/in-battle/owner-direction-2026-10-10.md`), and
the public facts beyond engine and gun, which the slices that add their sim facts name.
