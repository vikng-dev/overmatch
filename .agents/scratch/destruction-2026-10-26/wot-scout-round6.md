# WoT scout, round 6: vehicle-to-vehicle contact and ramming (received 2026-10-27)

Source: the `master` session in `../bounty`, by SendMessage, from the StranikS-Scan 1.46 readable
mirror (ENGINE / SCRIPT / DATA, file:line as given). The ramming dose formula is server-cell
(INFERRED); the trigger path, the replicated modifier and the client rendering are readable. Design
input only; WoT is arcade, structure not numbers. Game-state vocabulary: ramming doses HP pools.

## Who resolves contact

Server-authoritative and asymmetric with round 3's netcode: each own client simulates only its own
hull and reports position; the server detects two hulls touching, doses the pools on the cell and
fires client RPCs so observers render it. Remote hulls are never re-simulated on observers (filter
interpolation), so two hulls can briefly interpenetrate on an observer until corrected positions
stream in. No client-side physical resolution of two hulls touching.

## Trigger path (Vehicle.def + client scripts)

- `handleVehicleCollidedVehicle(vehA, vehB, hitPt, time)` (client_Avatar.py:2267), server-driven;
  the client only debounces the pair (repeat on the same (A, B) within 0.2 s ignored), computes
  `vehSpeedSum = |vehA.filter.velocity − vehB.filter.velocity|` for effect scale, plays
  `showVehicleCollisionEffect` and shakes both via `inputHandler.onVehicleCollision`. No dose.
- `showRammingEffect(energy FLOAT32, point VECTOR3)` (Vehicle.def:441), DetailDistance 100: sent
  only to clients within 100 m.
- `onStaticCollision(energy, V3, V3, UINT8, FLOAT32, INT8, UINT16)` (Vehicle.def:431), hull vs
  world, DetailDistance 100; native physics computes the impact energy.
- `rammingBonus` FLOAT32, CELL_PUBLIC (Vehicle.def:137): per-vehicle ramming modifier, server-side,
  not broadcast.

## Dose formula

Not readable. Corroboration: `ATTACK_REASON.RAM`, `VEHICLE_HIT_FLAGS.CHASSIS_DAMAGED_BY_RAMMING`
(524288), DEVICE_CRITICAL/DESTROYED_AT_RAMMING, `DESTR_BY_FALL_RAMMING = −2`, the FLOAT32
`rammingBonus`. The mass-and-speed shape (heavier and faster rammer doses more, both pools dosed,
chassis takes the state change) is consistent; reference mass and coefficients are server/xml.

## What an observer sees (client_Vehicle.py:496-531)

- Tier by relative speed and mass: Δspeed ≥ 3 m/s → heavy, else `collisionVehicleLight`; heavy by
  own mass (`physics['weight']`): < 18 t Heavy1, 18–46 t Heavy2, > 46 t Heavy3.
- `showCollisionEffect` (:512): effect at the hit point on the HULL node (random yaw/pitch without
  a surface normal), bound via `boundEffects.addNewToNode`, sound RTPC
  `RTPC_ext_collision_impulse_tank` driven by energy; definitions from the per-vehicle effect xml,
  one picked at random.
- Camera shake / input feedback `onVehicleCollision(veh, vehSpeedSum)`; `onCollisionWithVehicle`
  scales by energy / 1000 (client_Avatar.py:1710). `LOW_ENERGY_COLLISION_D = 0.3` gates low-energy
  track contact.

## Takeaways offered for Overmatch

- Resolve contact once, server-side; interpolate remote hulls and accept brief visual overlap.
  Compare against our open tab: remotes stay interpolated until tank-tank collision lands, which
  is the reopen trigger for opponent prediction.
- A light collision event (energy + point) to nearby clients only; effect by closing-speed + mass
  bracket; an audio parameter off energy.
- Debounce per pair (0.2 s) so grinding contact does not spam effects.
- One replicated per-vehicle ramming modifier; the math stays on the server. Note Yan's rule:
  impulse feel goes on the vehicle (hull overlay), never the camera.
