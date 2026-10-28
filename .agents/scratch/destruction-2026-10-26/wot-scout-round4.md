# WoT scout, round 4: gunnery (received 2026-10-27)

Source: the `master` session in `../bounty`, by SendMessage, from the StranikS-Scan 1.46 readable
mirror (ENGINE / SCRIPT / DATA, file:line as given). Design input only. Yan's caveat for every WoT
round: WoT is further from our genre than War Thunder and much more arcade; read mechanisms for
structure, never numbers. A dispersion number comes to Overmatch later and stays small (the gunner
aims by hand; no aim-circle bloom).

## Structural correction

The gunnery pipeline has two halves with different owners. AIM (the dispersion cone) is computed
client-side every frame for the reticle from factors the server pushes down; fully readable.
RESOLUTION (penetration, damage roll, ricochet / overmatch / normalization angle tests, HE spall,
HEAT jet) runs in server cell scripts and native core that no client package ships, so neither the
mirror nor a decompiled client exposes those numbers. The client receives the OUTCOME as a flags
word plus an effect enum.

## (A) Dispersion [SCRIPT, readable]

`Avatar.getOwnVehicleShotDispersionAngle` (client_Avatar.py:2214-2265); inputs via the
`updateTargetingInfo` RPC (:1338-1351).

- Base: a gun cone half-angle `descr.gun.shotDispersionAngle` (radians, fully aimed), times a
  dimensionless factor ≥ 1.
- Disturbances combine in quadrature: `additiveSqr = (speed·chassisMoveFactor)² +
  (rotSpeed·chassisRotFactor)² + (turretRotSpeed·gunTurretFactor)² + afterShotFactor²`, scaled by
  `additiveShotDispersionFactor²` (equipment/crew); `idealFactor = multFactor · sqrt(1 + additiveSqr)`.
  Each source only ever enlarges the cone. `afterShot` has a burst variant.
- Settling: `aimingFactor = aimingStartFactor · exp((aimingStartTime − now) / aimingTime)`, clamped
  never below `idealFactor`; any move, rotate or shot raises the ideal and the circle chases it down
  with the gun's one `aimingTime` constant.
- Marker: diameter = 2 · distance · dispersionAngle, linear with range
  (client_VehicleGunRotator.py:584-595). Dual-accuracy is an artillery mode; ignore.
- Ownership: the factors are server-computed from vehicle + crew + equipment and replicated; the
  per-frame cone is local for the reticle only; the shot vector drawn from the cone is
  server-authoritative.

## (B) Resolution [structure only]

- Shell taxonomy (`BATTLE_LOG_SHELL_TYPES`, constants :2792-2808): HOLLOW_CHARGE, ARMOR_PIERCING,
  ARMOR_PIERCING_HE, ARMOR_PIERCING_CR, SMOKE, HE_MODERN, HE_LEGACY_STUN, HE_LEGACY_NO_STUN, FLAME,
  ARMOR_PIERCING_FSDS, plus `_DF` variants. Three HE families; stun is its own behaviour.
- Penetration with distance (`PIERCING_POWER.computePiercingPowerAtDist`, :2858-2867): two numbers
  per gun (pFirst, pLast); constant to 100 m, linear to pLast over the next 400 m, zero past
  maxDist + 4 m. No ballistic table.
- Random spread gates (:3636-3637): `GUARANTEED_RANDOMIZED_DAMAGE = 1.0`,
  `GUARANTEED_RANDOMIZED_PIERCING_POWER = 1.0` scale the familiar ±25 % roll on both.
- Hit contract server→client (:1382-1430): `VEHICLE_HIT_EFFECT` {ARMOR_PIERCED_NO_DAMAGE,
  INTERMEDIATE_RICOCHET, FINAL_RICOCHET, ARMOR_NOT_PIERCED, ARMOR_PIERCED, CRITICAL_HIT,
  ARMOR_PIERCED_DEVICE_DAMAGED}; two ricochet states, so a deflected round can travel and strike
  again. `VEHICLE_HIT_FLAGS` bitmask: pierced vs not, split across HP-bearing armour vs
  zero-damage-factor armour (spaced/external that stops the round without HP loss); device pierced /
  damaged; chassis / gun damaged; cause BY_PROJECTILE / BY_EXPLOSION / BY_RAMMING / COMPRESSION;
  ATTACK_IS_* tags; status bits FIRE_STARTED, STUN_STARTED, VEHICLE_KILLED. The client
  (client_Avatar.py:1479-1520) only maps bits to sounds and effects.
- Angle thresholds (~70° ricochet, 2× calibre overmatch, 3× no-normalization) live in the shared
  item-descriptor module and per-shell xml, resolved on the cell: not readable here.

## Takeaways offered for Overmatch

- Dispersion = base angle × steady multiplier × sqrt(1 + quadrature sum), settled by one
  exponential time constant: cheap even when the cone is tiny.
- Split AIM (client-predicted cone from replicated factors) from ROLL (server draws the vector).
- Hit outcomes as a flags word + effect enum; the pierced/not × HP-armour/zero-DF × device/chassis/gun
  × projectile/explosion/ramming matrix is a contract template independent of the armour math.
- Two-number penetration + a 100 m → 500 m linear law is a cheap readout form.
