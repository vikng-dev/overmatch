# WoT scout, round 3: netcode (received 2026-10-27)

Source: the `master` session in `../bounty`, by SendMessage. Provenance as stated: entity defs and
filter/avatar scripts read directly; CONFIRMED = read, INFERRED marked. Design input only.

## Answers to our three open questions (the session's framing)
- **Remote hulls are never predicted**: pure server state through a native interpolating entity
  filter (WGVehicleFilter/AvatarFilter family), corrections blended inside the filter, never a snap or
  a re-sim. Only the OWN hull runs client physics, for zero input latency, reconciled toward the
  server's `ownVehiclePosition` {VECTOR3 pos, VECTOR3 dir, FLOAT32 speed, FLOAT32 rotationSpeed}.
- **Aim travels as one UINT16** `gunAnglesPacked`, ALL_CLIENTS: yaw 10 bits over [−π, π] (≈0.35°),
  pitch 6 bits over the gun's authored pitch limits (`common/gun_rotation_shared.py`,
  `encodeGunAngles`), at the 10 Hz server tick; remote turrets smoothed by the same filter
  (`syncGunAngles`).
- **A hit is synthesized client-side from a server verdict**: `onHealthChanged`, `showDamageFromShot`
  (packed stickers: decal id + penetration ray segment + component), device/crew state via
  `receiveVehicleDamagedDevices`, direction from the attacker/shot vector in the RPC; camera shake,
  sound, direction arc and decal are client reactions. Kill → post-mortem camera.

## Entity defs [DATA, CONFIRMED]
Vehicle.def flag census: 41 ALL_CLIENTS, 10 CELL_PUBLIC, 15 CELL_PRIVATE, 3 OWN_CLIENT, 2 BASE.
CELL_PRIVATE never leaves the cell (spotting internals: `invisibility`, `circularVisionRadius`,
`detectedVehicles`, `isObservedByEnemy`); ALL_CLIENTS to every client with the entity in AoI;
OWN_CLIENT to the controller only. `health` INT16 ALL_CLIENTS; `siegeState`/`physicsMode` UINT8;
`sixthSenseState` BOOL ALL_CLIENTS (the one spotting bit sent). Volatile position quantized by the
engine (bit widths INFERRED). Hull aux data packed: `unpackAuxVehiclePhysicsData` → (yaw, pitch, roll,
leftScroll, rightScroll). Per-property priority/detail levels exist in BigWorld; none seen set
(INFERRED default).

## Motion, input
- Own vehicle: `notifyInputKeysDown(movementDir, rotationDir, handbrakeFired)` drives native client
  physics; `filter.allowStrafeCompensation = not isPlayerVehicle` (remotes compensate, own does not).
- Remote track/wheel articulation is REPLICATED, not derived: `wheelsScroll` and `steeringAngles`
  packed, fed through `wheelsScrollFilter`/`wheelsSteeringFilter`, sampled at `BigWorld.time()`.
- Input is an edge-triggered flags bitmask (`MOVEMENT_FLAGS` FORWARD 1, BACKWARD 2, ROTATE_LEFT 4,
  ROTATE_RIGHT 8, + BLOCK_TRACKS, siege, handbrake), sent on key state change via
  `moveVehicle(flags, isKeyDown)`; the last intent persists on loss (INFERRED from the design).
- Filter delay native, INFERRED 0.1–0.2 s (1–2 ticks).

## Shooting, aim
- `cell.vehicle_shoot()` carries no aim; the server already holds the angles.
- The client draws its own tracer at once (`simplifiedPrediction`, `shotArgs`); the server replays the
  verdict via `showShooting` / `showDamageFromShot` / `showDamageFromExplosion`.
- Reload is server-owned; a local timer (`__gunReloadCommandWaitEndTime`) only stops spam.
- `VehicleGunRotator` ticks at `SERVER_TICK_LENGTH = 0.1 s`.
- **Server reticle** is a real option (`useServerAim` → `updateServerGunMarker`): where the server
  thinks the gun points, one round trip behind the local marker; players enable it to fire by the
  authoritative aim. A single-timeline client is always showing this one.

## AoI, spotting [CONFIRMED numbers]
- AoI radius 565.0 m (`constants.AOI.VEHICLE_CIRCULAR_AOI_RADIUS`), margin 5, hysteresis 570;
  `onEnterWorld`/`onLeaveWorld`.
- Spotting is a second, server-side filter over AoI (never sent): an unspotted enemy is withheld; on
  losing spotting mid-motion the client lingers the last pose then drops it. Cost: an O(N²) per-pair
  vision test per tick on the server plus its tuning; it buys bandwidth (unspotted enemies not
  streamed) and the stealth meta. Without it the per-client update set is every vehicle in AoI.

## Numbers read
Server tick 10 Hz; AoI 565/570 m; gunAnglesPacked 10+6 bits; `SHELL_TRAJECTORY_EPSILON_SERVER = 0.1`;
flag census above.

## The session's takeaways
1. Interpolate remote hulls, never predict them; a single timeline that adopts server state for the own
   hull is consistent, and should show the equivalent of the server reticle.
2. Quantize aim to ~16 bits at tick rate and smooth remote turrets like the hull.
3. Send health delta plus a compact hit descriptor; the receiver synthesizes everything felt.
