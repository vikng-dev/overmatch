# WoT scout, round 1: destruction + shells vs soft objects (received 2026-10-27)

Source: the `master` session in `../bounty` (Yan's WoT study), answering by SendMessage. Provenance as
stated by that session: client v.2.4.1.0 #981 on disk; readable scripts/values from the public
decompiled mirror (StranikS-Scan, branch 1.46), same engine and scripts; no server code exists to hold,
so everything server-side is INFERRED from entity defs and client effects. Labels: [ENGINE] BigWorld
native, [SCRIPT] Python, [DATA] res xml. Design input only; nothing here is code to transplant.

## Two unrelated destruction systems
- **System A, AreaDestructibles**: cheap props (trees, fences, lamps, small buildings), state per 100 m
  CHUNK, one cell entity per chunk, four replicated arrays of packed small-int tuples (ALL_CLIENTS):
  `fallenTrees` (destrID u16, fallPitchConstr u16, yaw<<2 | fallSpeed), `fallenColumns`,
  `destroyedFragiles` (destrID, isShotDamage), `destroyedModules` (destrID, matKind, isShotDamage).
  Per-object state is BINARY on the wire; HP lives only on the server. Client setters diff the new
  array against the previous set and animate only the additions. Late joiner: the chunk's arrays arrive
  as initial entity state and replay with no animation. [SCRIPT scripts/common/DestructiblesCache.py,
  scripts/client/AreaDestructibles.py; DATA scripts/entity_defs/AreaDestructibles.def]
- **System B, DestructibleEntity**: the few big HP buildings; real entity with replicated
  health/maxHealth, state machine per type with graded per-material armour, RPCs
  `onHealthChanged`, `showDamageFromShot(stickers)`, `showDamageFromExplosion`.

## Trees
- Fell trigger: shells AND vehicle contact, server-side, HP-based (Acacia health 20; most
  fragiles/atoms health 1). Ram formula server-only; readable inputs `kineticDamageCorrection`,
  `density`, `unitVehicleMass = 35000`. INFERRED: kinetic energy or impulse vs health, scaled by
  vehicle mass over 35 t.
- Fall: CANNED client animation (`_DestructiblesAnimator.showFallTree`, a scripted spring with
  springAngle/springStiffnes/springResist/airResist/buryDepth), not rigid-body physics; Havok only for
  cosmetic debris/explosion impulses. Event to ground INFERRED 1–2 s (fallSpeed 2 bits).
- After down: engine swaps collision; a bush deactivates; a trunk registers as a static obstacle
  (STATIC_OBSTACLE_ID 10000). INFERRED: a felled tree stops blocking vision and is effectively
  passable to vehicles and shots; standing trees block vision/LOS.
- Felling decision + array entry authoritative; the tumble cosmetic.

## Shells vs soft objects [DATA scripts/destructibles.xml, read server-side only]
- `maxHpForShootingThrough = 19`: health ≤ 19 → the shell passes through and the object is destroyed;
  above → the object stops the shell (hard cover).
- `projectilePiercingPowerReduction` per material: (factor, minReduction); every entry `0 25` in this
  build → a FLAT 25 mm penetration loss per destructible passed. No deflection, no speed change.
- Fence (health 1–10): shot through, −25 mm. Stone building module (health 40): stops. Bushes: soft,
  do not block the shell. System-B buildings always stop and absorb.

## Vehicle damage (gunnery) — the session flagged this WEAK, all INFERRED, unverified
Nominal damage ±25 % and penetration ±25 % rolled server-side; normalization per shell type;
3-caliber overmatch; 2-caliber rule; auto-ricochet > 70° unless overmatched; HE splash falloff and
armour absorption; residual penetration through spaced screens — all server. The client holds the
armour model for the aim indicator only. A dedicated round after decompiling the shell/penetration
scripts was offered.

## Server/client split
The tracer is a cosmetic replay of a server verdict; on hit the client receives effect codes, hit
point, flags (INFERRED from architecture + the System-B RPCs). Tick rate not readable (BaseApp/CellApp
config); conventionally ~10 Hz, no number quoted.

## Numbers read directly (scripts/destructibles.xml unless noted)
maxHpForShootingThrough 19; unitVehicleMass 35000 kg; piercing reduction flat 25 mm; Acacia_01 health 20,
density 0.5, mass 800, height 10, springAngle 0.26 rad, springStiffnes 20000, springResist 4000,
airResist 20, buryDepth 0.15; street lamp health 1, mass 200, height 3.2; fragile health distribution
over 1407 entries: 10→704, 1→627, 20→57, 35→13, 5→3, singletons 40/50/400; structure stone module 40;
catalogue 433 trees, 1407 fragiles, 95 falling atoms, 178 structures; chunk 100 m
(chunkID = round(pos·0.01)); SEARCH_AD_RADIUS 5.0; FALLING_DESTRUCTIBLES_IGNORE_ANGLE 45°;
_MAX_PITCH_TO_CHECK_TERRAIN 60°; _SHOT_EXPLOSION_SYNC_TIMEOUT 0.11 s; SPT_MATKIND_SOLID 71.

## Native call surface [ENGINE, WorldOfTanks.exe, names known from script calls]
getDestructibleFilename, get/setDestructibleMatrix, setDestructibleActive, checkDestructibleIsBush,
getDestructibleFallPitchConstr, destroyFragile, destroyModule, destroyFallAtom,
get/setChunkModelFashion, MaterialDisabler, WGStaticModelFashion, havokExplosion, isHavokActive,
getDestructibleEffectName/Category/Scale, getDestructibleLifetimeEffectChance, getChunkMatrix.

## The session's three takeaways
1. Per-chunk arrays of packed tuples, one entity per 100 m, HP never on the wire, late joiners get the
   chunk snapshot (no event replay).
2. Soft/hard rule: HP ≤ threshold → pass through for a flat penetration cost; above → blocks.
3. Felling: HP pool vs a single damage event; one-shot for throwaway props, 10–20 HP for things that
   need a real hit; ram = f(mass/reference mass, speed) vs that HP.
