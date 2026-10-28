# WoT scout, round 7: impact feedback and effects (received 2026-10-27)

Source: the `master` session in `../bounty`, by SendMessage, from the StranikS-Scan 1.46 readable
mirror (ENGINE / SCRIPT / DATA, file:line as given). Mostly CONFIRMED client-side; terrain-mark pool
internals and the absence of deformation are INFERRED. Design input only; WoT is arcade, structure
not numbers. Closes the rounds 4–7 request. Feeds the impact-feedback epic (hull marks, terrain
marks, VFX).

## (a) Hull marks: damage stickers

- Engine: native `BigWorld.StickerModel`, one per space, attached to the vehicle supermodel
  (VehicleStickers.py:92-156, DestructibleStickers.py); it projects decal textures, the script feeds it.
- Data: the server replicates `damageStickers` ARRAY<UINT64> (ALL_CLIENTS, Vehicle.def). Each
  UINT64 packs stickerID + the shot-ray segment (segStart → segEnd) + hit component index. On change
  `set_damageStickers` (client_Vehicle.py:596) diffs previous and current as sets: removes gone, adds
  new via `DamageFromShotDecoder.decodeSegment` → `StickerModel.addDamageSticker(stickerID, segStart,
  segEnd, hitCompIndx, collisionComponent)`.
- Choice and projection: the stickerID (hole, ricochet scuff, scorch) is picked server-side; the
  native model projects it along the shot ray onto the hit collision component.
- Dedup by code (a duplicate is ignored); the vehicle side is set-based.
- Lifetime: persistent for the vehicle's life, no timer; count is server-capped by array length;
  distance LOD only (`setLODDistance(vDesc.type.emblemsLodDist)`, 1000 m for destructibles).

## (b) Terrain marks

- Track traces: laid by the native engine as the chassis moves (trailing decal strip), faded over
  distance/age; the visual config is native, not script-exposed.
- Craters / scorch from explosions: decals spawned by the effect system at the blast point, culled
  at `decalCullDistance = 400 m` (engine_config ObjectsFarPlaneManager:56; objects 2800, control
  points 1400). A native recycled pool, distance-culled; no persistent per-map accumulation.
- INFERRED: decal-only, no heightmap deformation (consistent with round 2).

## (c) Effects authoring

- Authoring: `vehicles.g_cache.shotEffects[effectsIndex]`, built from shell/effect xml. Each
  descriptor (read in `showDamageFromShot`, client_Vehicle.py:404-450) carries
  `physicsParams` {shellVelocity, shellMass, splashRadius, splashStrength}, `targetImpulse`,
  `caliber`, `shellType`, per-outcome effect groups `effectsDescr[hitEffectGroup]` = (keyPoints,
  effects), and newer CGF `hitPrefabs`.
- Trigger: `showDamageFromShot(attackerID, points, effectsIndex, damageFactor,
  lastMaterialIsShield)` server→client. The server sends which shell plus packed hit points; the
  client decodes the hit component and `hitEffectGroup` (max-priority hit) and plays the group via
  `appearance.boundEffects.addNewToNode(componentName, matrix, effects, keyPoints, …)`. The same
  path serves shot, collision and ramming effects; muzzle flashes, explosions, smoke and dust are
  particle systems (BigWorld Pixie xml) bound to model nodes.
- Scaling is data: `caliber` + `shellType` feed `onVehicleShaken` (camera shake by calibre and
  type); `splashRadius` / `splashStrength` drive `onProjectileExplosion`; `targetImpulse` rocks the
  struck hull (`receiveShotImpulse`).
- Fullscreen post (shock wave, flashbang, heat haze): only for the player's own vehicle,
  `showFullscreenEffs = isPlayerVehicle and isAlive()`.
- Budget: a `<ParticleSystem>` autoscaler (engine_config:194-201) targets ~22–26 fps
  (PS_PERFORMANCE_MIN_FPS 22, MAX_FPS 26, FIXED 29.9) and biases particle LOD down under load
  (SPEED_MULT 10, LOD_MULT 1.2); per-effect LOD via keyPoints in each effect xml.

## Takeaways offered, read against our rules

- Hull marks: pack mark id + impact-ray segment + component into one replicated value, project
  along the ray, replicate the live SET and diff it, cap the count server-side, distance-LOD rather
  than time-expire. Fits ADR-0039: a hull mark is physical and visible to everyone.
- Terrain: track traces and craters as recycled, distance-culled decals; no heightmap
  deformation. Our ONE-SURFACE terrain doctrine already makes craters cosmetic-only.
- VFX: per-round effect descriptors in data (physics params + calibre + type + per-outcome group);
  the server event carries an index and hit points; the client looks up and plays. Our rule: effects
  scale from physics, never from viewer distance, and impulse feel goes on the vehicle, never the
  camera, so WoT's calibre-scaled camera shake is NOT borrowed.
- Fullscreen post reserved for the local player's own hits.
- A particle autoscaler keyed to framerate so effects never sink the frame.
