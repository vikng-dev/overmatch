# WoT scout, round 5: modules and crew as game state (received 2026-10-27)

Source: the `master` session in `../bounty`, by SendMessage, from the StranikS-Scan 1.46 readable
mirror (ENGINE / SCRIPT / DATA, file:line as given). Gameplay effects of a reduced state are
INFERRED (resolved server-side); the enum names confirm the structure. Design input only; WoT is
arcade, structure not numbers. Game-state vocabulary throughout: a module or crew member is
knocked out when its HP pool is emptied, reduced when lowered.

## Which slots carry state

`INVULNERABLE_EXTRAS` (constants :3853): engine, ammoBay, fuelTank, radio, gun, track,
turretRotator, surveyingDevice, crew. A device is not an HP pool: a three-state model,
functional → CRITICAL → DESTROYED. The hull is the only real HP pool.

## State vocabulary (`DAMAGE_INFO_CODES`, :1433)

- DEVICE_CRITICAL / DEVICE_DESTROYED, each with a cause suffix: _AT_SHOT, _AT_RAMMING, _AT_FIRE,
  _AT_WORLD_COLLISION, _AT_DROWNING. Two states, many causes.
- Repair walks back: DEVICE_REPAIRED_TO_CRITICAL (destroyed → critical), then DEVICE_REPAIRED.
  A reverse state walk, not an HP refill.
- Crew: TANKMAN_HIT (knocked out), TANKMAN_RESTORED; aggregate DEATH_FROM_INACTIVE_CREW.
- Engine extras: _AT_UNLIMITED_RPM and _AT_BURNOUT (over-rev / burnout is its own path).

## Hull health is a signed channel (Vehicle.def:73; `SPECIAL_VEHICLE_HEALTH` :1263)

`health` INT16, ALL_CLIENTS. Positive = HP; negative encodes the death mode
(DESTR_BY_FALL_RAMMING −2, FUEL_EXPLODED −3, AMMO_BAY_DESTROYED −5, TURRET_DETACHED −13), tested by
bit-OR. The client reads < 0 to pick the wreck variant. `AMMOBAY_DESTRUCTION_MODE` (:1258):
POWDER_BURN_OFF / POWDER_EXPLOSION / HE_DETONATION.

## What a reduced state means (INFERRED)

track: slower traverse, destroyed = immobilised until repaired. gun: accuracy and aim-time penalty,
destroyed = cannot fire. engine: power loss + burnout risk (`BURNOUT_WARNING`,
`BURNOUT_UNAVAILABLE_DUE_TO_BROKEN_ENGINE`), destroyed = immobile. ammoBay: reload penalty,
destroyed = detonation (the −5). fuelTank: fire risk. radio: spotting range. turretRotator:
traverse. surveyingDevice: view range. Magnitudes live in cell scripts and xml.

## Repair and timers

`REPAIR_TYPE` AUTO / KIT (:~1382): auto = crew repairs the worst device over server timers; kit =
instant consumable. In progress: `VEHICLE_MISC_STATUS.DESTROYED_DEVICE_IS_REPAIRING` (:1190).
Durations are server-side.

## Two replication tiers (Vehicle.def)

- Always broadcast: `health` INT16, `isCrewActive` BOOL (one aggregate crew flag), `engineMode`,
  `stunInfo`, `damageStickers` ARRAY<UINT64>, `publicStateModifiers` (status-effect extras visible
  to everyone: on fire, stunned), `crewCompactDescrs`.
- Pulled on demand: the per-device damaged list. `requestDamagedDevicesFromFor(OBJECT_ID, MAILBOX)`
  → `sendDamagedDevicesTo(MAILBOX)` (Vehicle.def:567-573), gated by
  `OTHER_VEHICLE_DAMAGED_DEVICES_VISIBLE` (:1183): own / platoon / certain equipment.

## Takeaways offered for Overmatch

- Modules as a small state enum plus a repair reverse-walk; broadcast only an aggregate (hull HP,
  crew-alive flag, status bitmask); pull detailed module state for the vehicles a client looks at.
- Cause-of-death folded into the health field's sign lets the client pick a death effect without a
  separate message.
- Cause-tagged state codes drive different feedback from one state change.

Note against our design: Overmatch's modules and crew ARE authored HP pools (GitHub #104), dosed by
lines and fields; the three-state device model is a contract shape to compare against, not a
direction. The replication split (aggregate always, detail on sight) is the borrowable part.
