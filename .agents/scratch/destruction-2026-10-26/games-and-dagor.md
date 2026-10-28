# Environment destruction: how shipped tank games do it, and what Dagor's source says

Research pass, 2026-10-26. Subject: game state and rendering of buildings, walls, fences and trees
breaking or falling when hit by game rounds or driven into by tanks. Extends
`.agents/scratch/destruction-survey-2026-08-13.md`; fills its §8.5 gap ("WT/WoT server-side
building state: no official statement") from the Dagor Engine source and War Thunder's own posts.
Tags: [S] sourced (URL or clone `file:line`), [S~] via a search summary of a source, [I] inference.
Numbers: REPORTED (a source or the code states it), DERIVED (computed here), MEASURED (none here).

## Summary

1. Dagor (War Thunder, Enlisted) makes destruction a **server-owned set of destroyed instance ids**, sent as an ordered snapshot on connect plus unordered deltas; everything physical (debris, falling trunks, canopy) is client-local, capped and lifetime-bounded, never replicated.
2. The authoritative model is **HP per instance + a state swap**: at zero HP the instance is removed and re-added as its "destroyed" pool (`nextRes`) at the same transform; the swapped model carries its own collision. No runtime fracture is authoritative.
3. Clients **predict** their own destruction and keep a 1 s restorable copy; the server confirms or the instance pops back. War Thunder's 2024 roadmap states this policy in plain words ("destroyed first on the player's side, and then server-side for all players").
4. A destroy delta costs one (cell, pool, offset) triple plus an optional 48-bit quantized impulse, sent only to players within 128 m; max 255 entries per message, 64 destroys per update batch.
5. Tank contact destroys through an **impulse threshold per instance** evaluated inside the vehicle's own collision pass; trees add a lever arm (impulse × contact height), so a nudge at the base never fells.
6. Fallen trees are client PhysBodies: a trunk capsule plus a canopy body on a spring joint; the trunk collides with tanks only until it has moved/tilted past a limit, the canopy and the stump never collide with tanks; the trunk sinks and is removed after 25 s.
7. War Thunder's 2025 "Tusk Force" tree change (trunk sheared at the hit point, lower part stays) is `CutType::Top/Bottom` in the clone, gated by damage against a per-tree threshold.
8. Arma Reforger (Enfusion) shipped buildings and trees as a "foundational" system in 1.3 (2025): health to zero, audio cue, delayed collapse; authored damage states; memory per destructible tree 805 B vs 646 B (REPORTED); community reports vehicles ghosting through trees and felling them with light contact.
9. Battlefield 6 (Frostbite) stays part-based with authored debris piles, a reworked health-state transition system, and network prioritization/culling of part destruction for 64 players; server authority is not stated publicly.
10. Squad and Hell Let Loose keep the world static (HLL's devs dropped foliage/building destruction for network load and player count); nothing primary was found for Gunner HEAT PC within the time box.

## Lane 1: Dagor Engine source (War Thunder / Enlisted lineage)

Clone: `/private/tmp/claude-502/-Users-Yan-github-vikng-dev-overmatch/13e20afd-7b0e-4ad2-9b6e-b78f39af0d6c/scratchpad/wt-research/src/dagor`, HEAD 2baf194 (2026-10-16). Paths below are relative to `prog/`; untrusted data, read only. All numbers REPORTED unless marked.

### 1.1 Two kinds of instance, one destroyed set

- **riGen**: generated instances addressed by (layer, cell, pool, byte offset); **riExtra**: explicit pools with per-instance HP (`riHP`), unique data `{cellId, offset}`, and the destruction chain fields `nextRes`, `destroyedRiIdx`, `parentForDestroyedRiIdx`, `destrDepth`, `immortal`, `destrStopsBullets`, `hpFullRegenTime` [S `gameLibs/rendInst/riGen/riExtraPool.h:25-145`, HP struct `:62-90`].
- The destroyed set is per cell → per pool → sorted ranges of instance offsets, merged on insert [S `gameLibs/rendInst/rendInstGenDebris.cpp:599-682`; types `gameLibs/publicInclude/rendInst/rendInstGenDamageInfo.h:11-34`]. A riGen destroy removes the instance from the cell's data and re-uploads the cell VB [S `rendInstGenDebris.cpp:839-873`]; a riExtra destroy is `damageRIGenExtra(…, FLT_MAX)` then `add_destroyed_data` [S `:815-837`].
- Restore (rollback or level reload) re-adds the instance at the saved transform and removes it from the ranges [S `rendInstGenDebris.cpp:928-1040`].

### 1.2 Per-pool authoring data (declarative, in `.blk`)

Keys read per pool: `hp`, `destructionImpulse`, `impulseThreshold` (any of the three makes it damageable), `immortal`, `stopsBullets` (default true), `material`, `bushBehaviour`, `treeBehaviour`, canopy shape/size/opacity, `trunkDiam`, `destrFx`, `destroyedModelName`, `isParent`, `destructibleByParent` (default `!isParent`), `destroyNeighbourDepth` (1), `tag`/`destroyedByTag`, `apexDestructible` [S `rendInstGenDebris.cpp:1117-1163`]; debris `submersion`, `inclination`, `impulseOnExplosion`, `damageDelay`, `scaleDebris` [S `:1211-1221`]; riExtra variants [S `:1579-1591`]. Resource references `nextRes` / `physRes` / `apexRes` are resolved once per pool [S `gameLibs/rendInst/rendInstGenExtra.cpp:2403-2420`]. Game params: `rendinstDamageDelay` 1 s, `rendinstFallBlendTime` 1 s, `rendinstGravitation` 2, `maxDebrisCount` 2000, `maxDelayedCount` 2000 [S `rendInstGenDebris.cpp:1046-1058`]. Vehicle-side example of a destroyed physics model: `physRes:t="m123_truck_destr_phobj"` [S `_docs/source/projects/wt/wt_designing_vehicle_deformations.md:440`].

Defaults in `DestrSettings`: `rendInstMaxLifeTime` 25 s, `maxTreeImpulseDamage` 1000, `hitPointsToDestrImpulseMult` 3000, `destrImpulseSendDist` 128 m, `destrMaxUpdateSize` 140, `riMaxSimultaneousDestrs` 64, pool `hp` default 300 with `hpFullRegenTime` 180 s [S `gameLibs/publicInclude/gamePhys/phys/rendinstDestr.h:73-101`; `riExtraPool.h:62-90`].

### 1.3 Damage paths (all end in the same state swap)

- Point/sphere/box damage: `apply_damage_to_ri` [S `gameLibs/gamePhys/phys/rendInstDestr/rendinstDestr.cpp:798-820`], `damage_ri_in_sphere` with a line-of-sight occlusion test before applying sphere damage [S `:822-896`], `doRIExtraDamageInBox` [S `:1011-1095`].
- `destroyRendinstInternal` [S `:535-694`] handles neighbour chains: `findRendinstNeighbors` [S `:477-512`], `isParent`/`destructibleByParent`/`destroyNeighbourDepth`, tag match. Authoring rule: a child is destroyed with its parent only if the child's collision lies inside the parent's collision bbox; strictly parent → child, two levels max [S `_docs/source/assets/collisions/static_object_collision.md:1306-1345`; `_docs/source/assets/all-about-blk/composit_blk.md:598-662`].
- HP decrement and swap: `applyDamageRIGenExtra` / `damageRIGenExtra` [S `rendInstGenExtra.cpp:2068-2184`]: at zero HP the instance is deleted and a new riExtra of `nextRes` is added at the same transform with `destrOffset = offset + 16`; deleting the original from a cell also deletes its destroyed-state copy [S `:1977-2009`].
- What a hit sees: `CollisionInfo` carries `isImmortal`, `stopsBullets`, `isDestr`, `destrImpulse`, `hp`, `initialHp`, bush/tree flags, parent flags, fx id, material; a riExtra with no HP and no destr model is forced immortal [S `gameLibs/rendInst/rendInstGenCollision.cpp:2213-2250`].

### 1.4 Tank contact (the "push through" question)

- The vehicle's collision pass wraps each contacted RI in a cached threshold object. Destructible RI: `RendinstImpulseThresholdData`; impulse → damage = `initialHp · impulse / destrImpulse`, or `impulse · destrImpulseHitPointsMult` when no `destructionImpulse` is authored; at zero HP the destructor calls `destroyRendinst(… DestroyedByCollision | AddDestroyedRi | ForceDestroy)` [S `gameLibs/gamePhys/collision/rendinst/rendinstCollisionUserInfo.cpp:24-85`]. Wrapper: [S `gameLibs/gamePhys/collision/rendinst/rendinstCollisionWrapper.cpp:40-98`].
- Trees use a shared capsule proxy sized from the tree bbox; threshold = `impulseThresholdMult · max(height − heightThreshold, 1)²` (or `destructionImpulse · widthScale`), times a radius factor [S `rendinstCollisionWrapper.cpp:100-159`]. The impulse is multiplied by `max(1, |contact point in tree space|)`, a lever arm, before being compared with the threshold; the destructor fells the tree with the last contact velocity and angular rate, `from_damage=false` [S `rendinstCollisionUserInfo.cpp:87-126`].
- So a tank does not "push through" a wall: it either absorbs the contact (threshold not reached, impulse returned to the vehicle) or the wall is destroyed and the vehicle keeps its momentum minus the absorbed part. The destroyed model (`nextRes`) is what the tank collides with afterwards [I from 1.3].
- Movable (non-destroyed) rendinsts that a tank can shove are a separate ECS feature: `phys_moved_rendinst` entities with `replicating` + `phys_obj_net_phys`, server-controlled hero lists; clients get only a visual impulse from `CmdRendinstDamage` [S `daNetGameLibs/ri_phys/templates/ri_phys.blk`; `daNetGameLibs/ri_phys/main/ri_phys.das:13-48`].

### 1.5 Networking: what the server tells clients

- Messages: `RiDestrSnapshotMsg` (server → all, RELIABLE_ORDERED) and `RiDestrUpdateMsg` (server → each connection, RELIABLE_UNORDERED); comment: pool updates must be ordered or an empty join snapshot can overwrite later updates [S `daNetGame/game/riDestrES.cpp.inl:51-70`]. Server callback on destroy [S `:261-299`], initial snapshot on client connect [S `:485-496`], batched flush [S `:498-539`], per-frame update [S `:551-568`].
- Snapshot wire format: cell count; per cell `int16 cellId` + pool count; per pool `poolIdx` + range count; ranges as delta-compressed `(startOffs − prevRange) >> shift`, `(endOffs − startOffs) >> shift`, shift 4 for riExtra (cellId < 0) and 3 for riGen [S `gameLibs/gamePhys/phys/rendInstDestr/netDestrSync.cpp:539-590`; reader `:592-841`].
- Update wire format: `uint8` entry count (≤ 255), a bitmask of which entries carry an impulse, entries grouped by cell and pool, impulse as `QuantizedDestrImpulsePos` (8 bits/axis, scale 8) + `QuantizedDestrImpulseVec` (8 bits/axis, scale 256) = 48 bits, included only when the receiver is within `destrImpulseSendDist` 128 m [S `netDestrSync.cpp:844-967`; quantizers `rendinstDestr.h:60-71`; `DestrUpdateDesc` `:118-159`; `can_sync_ri_destr` `:161-165`].
- Client prediction: a destroy on the client first creates a `RestorableRendinst` with ttl 1 s holding the saved instance buffer and the packed impulse [S `netDestrSync.cpp:129-154, 229-253`]; `update_restorables` restores anything the server did not confirm in time [S `:182-227`]; clients send their own destroy requests for the server to validate [S `:1079-1160`]. Applying a server destroy on a client: `destroy_rend_inst_from_net` [S `:376-452`]; for a tree the fall direction is seeded from `(offs + pool) ^ (cell < 16)` so every client fells it the same way without a vector on the wire [S `:420-426`].
- Pool ids are remapped by 32-bit FNV-1 name hash so a client and server with different pool orders agree [S `gameLibs/rendInst/riexSync.cpp:168-242`; `publicInclude/rendInst/riexSync.h`].
- Tree burning follows the same pattern: the server replaces a riGen tree with a riExtra copy, keeps a burnt-handle set, bumps the ridestr version and sends the set to each new client [S `daNetGameLibs/tree_burning/dedicated/treeBurningServerES.cpp.inl:25-76`]; a burnt tree is later felled through `create_tree_rend_inst_destr` with zero impulse [S `daNetGameLibs/tree_burning/common/treeBurningCommonES.cpp.inl:79-93`].

### 1.6 Debris (client-local, capped)

- A destroyed instance may spawn a `DynamicPhysObject` from `physRes` through `destructables::addDestructable` with `DestrObjProps` (`timeToLive` 15, `timeToKinematic`, `timeToSinkUnderground`, disintegration) [S `gameLibs/gamePhys/phys/rendInstDestr/riDestrProps.h:41-54`]. Pieces interact with Static and Character (vehicles, ragdolls) by default; Self and Projectile are off unless `selfCollision` / `projectileInteraction` is set, so debris does not stop rounds by default [S `gameLibs/publicInclude/gamePhys/phys/destructableObject.h:59-66, 266`; `gameLibs/gamePhys/phys/destructableObject.cpp:757-763`].
- Config defaults: `maxNumberOfDestructableBodies` 100, time-scale ramp from 50 bodies, `distForScaleDt` 100 m, `minDestrRadius` 40 m, out-of-view disappearance by relative size with soft/hard body limits 50/100, sink after 3 s inactive over 3 s [S `destructableObject.h:131-133, 212-284`]. Update loop: at most `DESTRUCTABLES_DELETE_MAX_PER_FRAME` deletions per frame, out-of-view culling by size vs distance, overflow culling [S `destructableObject.cpp:940-1037`]. Legacy "debris" are extra riExtra instances without collision, capped by `maxDebrisCount` [S `rendInstGenDebris.cpp:1398-1416`].
- Optional APEX (`ENABLE_APEX`) and procedural fracture (`frx`) paths exist; both are client visual and gated per pool [S `riDestrProps.h:32-39`; `rendInstGenExtra.cpp:2403-2420`].

### 1.7 Trees

- A tree is a position-only instance with an impostor (`isTree = posInst = hasImpostor()`); felling copies it to a riExtra `DestroyedRi` under external control, which requires the pool's `treeBehaviour` flag [S `rendInstGenDebris.cpp:316-374`; `publicInclude/rendInst/rendInstDebris.h:96-117`].
- New-physics path `create_tree_rend_inst_destr` [S `gameLibs/gamePhys/phys/rendInstDestr/treeDestr.cpp:417-1024`]: a damage hit above a threshold cuts the tree in half (`CutType::Top/Bottom`), comment "A big tank cannon does about 1000 damage" [S `:444-464`]; trunk capsule PhysBody with the canopy as a second body on a 6DoF spring joint, optional ground 6DoF joint when `constrained_phys` [S `:927-961`]; the canopy never collides with tanks [S `:823-825`]; the bottom stump never collides with tanks [S `:706-710`]; free parts collide with tanks until a distance/angle limit [S `:712-715`]. Non-physics fallback is a kinematic cylinder [S `:1004-1010`]. The dispatcher stamps `get_sync_time()` [S `:1028-1053`]. Tunables in `TreeDestr` / `BranchDestr` [S `publicInclude/rendInst/treeDestr.h:16-88`; loader `gameLibs/rendInst/treeDestr.cpp:12-88`].
- Lifetime: `update_ri_phys` [S `gameLibs/gamePhys/phys/rendInstDestr/rendinstPhys.cpp:161-617`]: velocity cap 40 m/s and ground-joint break cap 15 m/s [S `:183-184`]; an active body keeps ttl 5 s, an inactive one counts down; past the distance/angle limit the trunk's mask drops `EPL_KINEMATIC` (tanks) [S `:264-297`]; the ground joint is removed on over-speed or over-tilt [S `:299-321`]; once settled the body loses all collision, gravity −0.1, ttl 15 s, `disappearStartTimeAt` set [S `:335-349`]; then `removeRIGenExternalControl` [S `:350-358`]. Render cap `maxDestroyedTreeCount` 100 and visible distance per tree, with the stump hidden when over cap [S `:187, 568-601`]; `maxCutTreeCount` 20 [S `treeDestr.h:44-88`]. A stump touched by a tank releases its joint limits and falls over [S `rendinstPhys.cpp:436-466`].
- Old path (whole tree topples): [S `treeDestr.cpp:96-415`]. War Thunder's 2025 "sheared at the hit point" is the `CutType` gate above [I, lane 2].

## Lane 2: shipped games

**Enlisted (Dagor)** [S~][I]. Same code lineage as 1.1–1.7 (`daNetGame` is the open-sourced Enlisted-era framework) [I]. Public notes found: engineer structures fall when the surface under them is destroyed (1.24, 2022) [S~ mp1st]; the 2021 "Destruction" mode is scripted objective demolition [S~ thesixthaxis]. No official technical write-up found.

**War Thunder (Dagor)** [S]. 2015: destruction extended from vegetation and small objects to "wall fragments, ruins, and entire buildings" [S~ massivelyop]. 1.53 "Firestorm": PhysX Destruction, walls/ruins/buildings "react to in-game ballistic physics"; CEO: a destroyed wall "should vanish for all other players in the same battle" [S gaijinent.com/news/158]. 2024 roadmap: "improvements to the synchronization of the destruction of objects on ground maps … fences, bushes and other objects will be destroyed first on the player's side, and then server-side for all players" [S warthunder.com/en/news/8716]. 2025 "Tusk Force": "Before, trees would fall down in their entirety when shot at"; now "if you shot the top of a tree, the part above where you hit would be sheered off and the rest of the trunk would remain" [S warthunder.com/en/news/9710]; Ardennes bushes replaced by undergrowth "which does not roll on the ground after being destroyed" [S~ changelog]. Authoring: state swap per 1.2–1.3; server: destroyed set per 1.5; tanks: impulse threshold per 1.4. Regret visible in code: the client-predict/restore machinery and the ordering comment in 1.5 exist because the 2024 sync was wrong before [I].

**Arma Reforger (Enfusion)** [S]. Buildings and trees destructible since the 1.3 "Fire Support" experimental (Dec 2024) and release (Mar 2025): "Added: Trees can now be destroyed", "Added: Buildings can now be destroyed", "Added: Ruins for all buildings", modding "Support for more than three custom damage states" [S reforger experimental-december-13-2024] — i.e. authored damage states plus a ruin model, a state swap [I]. Dev Report 22: "a foundational version of destruction that is designed to be expanded over time"; "when a building's health reaches zero, audio cues are triggered first, signaling the imminent collapse", collapse after several seconds, soldiers inside die; houses are many entities removed "sequentially rather than all at once", custom volumes instead of bboxes; a destructible tree costs 805 B vs 646 B, "With 820,272 trees on Everon, this would require … 660MB" (REPORTED), hence a `RegionalDestructionManager` grid with constraints: "Entities from the same prefab must behave identically", destructibles cannot move, no tree hierarchies [S dev-report-22]. Networking and server authority: not stated [I: Enfusion replicates entity state; the damage state is such state]. Vehicles: community "World Collisions Overhaul" says base-game vehicles ghost through trees at some speed/mass pairs, trees fall "with the lightest bump", stumps let vehicles through, helicopters fell forests through the canopy; the mod makes felling depend on mass and velocity and makes stumps solid [S workshop 67323A7B77DD1C42]. Regret: "performance concerns" first, realism later [S dev-report-22].

**Squad (UE4 → UE5)** [S~][I]. No building or tree destruction found in any patch note; a 2018 player thread notes a tank cannot run over a small wooden fence [S~ steam 736220]. Deployables (walls, HABs) have HP and are destroyed, which is entity state, not world state [S~]. World is static [I].

**Hell Let Loose (UE4)** [S~]. Developer FAQ (pre-release, now 404): bridges, walls and fences were the destruction priority "because they would meaningfully affect strategy and traffic flow"; more complex foliage/building destruction was tested and dropped because "the network load would cut the total player count", choosing 100 players over a smaller destructible map [S~ hellletloose.com/faq via search]. Shipped destruction is limited to fixed objects [I].

**Gunner HEAT PC (Unity)** [I]. No primary source on environment destruction found in the time box (searches returned only roadmap chatter); unknown, do not cite.

**Battlefield 6 (Frostbite, 2025)** [S]. EA write-up: "part-based destruction and debris spawning systems" made "more extensible" (Talan Le Geyt, Frostbite); a reworked health transition system lets game code run logic on each state change (Johan Leijon, DICE); soldiers, ragdolls and vehicles are "chaos agents" that trigger destruction; "We wanted the result of destruction to stay, creating interesting ruins and level-designed debris piles" (Rickard Antroia, DICE); network work: "prioritization and culling of part destruction" so 64-player matches stay synchronized [S ea.com how-battlefield-6-redefined-destruction]. Technical director Christian Buhl: dropping PS4/Xbox One gave memory and CPU headroom [S~ insider-gaming]. Server authority not stated [I: Frostbite destruction has been server-owned state with client-side debris since Bad Company 2, per the earlier survey].

## Lane 3: trees

| Game | Felling trigger | The fall | Fallen collider: tanks / rounds / sight | Persistence | Networking |
|---|---|---|---|---|---|
| Dagor (WT, Enlisted) | Hit: damage ≥ per-tree threshold, cut Top/Bottom at the hit height; tank contact: lever-armed impulse ≥ `impulseThresholdMult·(h−h0)²·f(r)`; burning [S 1.4, 1.7] | Client PhysBody: trunk capsule + canopy body on 6DoF spring joint, optional ground joint; caps 40 m/s, 15 m/s joint break [S 1.7] | Tanks: trunk yes until moved/tilted past limit, canopy never, stump never; rounds: the riExtra copy keeps pool collision while alive [I]; sight: render only [I] | Body 5 s active → settle → 15 s → removed; `rendInstMaxLifeTime` 25 s; the destroyed-id persists server-side for the match [S 1.5, 1.7] | Server sends (cell,pool,offs) + optional 48-bit impulse ≤128 m; late joiners get the snapshot and fell it with a seeded direction; client predicts with 1 s restore [S 1.5] |
| War Thunder 2025 | As above; "sheared off" above the hit, lower trunk remains [S 9710] | As above (new path) | As above | As above | As above |
| Arma Reforger | Explosives (mortars) and vehicle contact; base game fells on "the lightest bump" [S workshop] | Physics-driven [I] | Tanks: stumps pass-through in base game [S workshop]; rounds/sight: not stated | Not stated | Not stated; entity damage state [I] |
| Battlefield 6 | Not covered by the sources found | — | — | "result of destruction to stay" (buildings) [S ea.com] | prioritized/culled part destruction [S ea.com] |
| Squad, HLL | None: trees static [S~][I] | — | Trees block tanks as static colliders [I] | — | — |
| GHPC | Unknown (no source) | — | — | — | — |

## What this means for a 20–30 tank, 1–4 km² server-authoritative game

1. The authoritative object is a **state per destructible instance** (intact / damaged / destroyed), owned by the server, built at spawn from map data (ADR-0014); never a physics result. Dagor's cell/pool/offset ranges are a compressed bitset; at ≤ 10⁴ destructibles a flat bitset is ≤ 1.25 kB (DERIVED) and needs no range coding.
2. Replicate it as **snapshot on connect + deltas**, snapshot ordered before deltas (Dagor's own warning). One lightyear component per map chunk, or one message type, is enough; no per-instance entity.
3. **State swap** is the authoring unit: intact mesh + collider, destroyed mesh + collider, same transform; chain destruction by parent bbox containment is a map-bake rule, not runtime logic.
4. Everything that moves (debris, trunk, canopy) is **client view**, capped (Dagor: 100 bodies, 100 trees drawn, 64 destroys per batch) and lifetime-bounded (15–25 s), and never blocks rounds by default; the server sends at most a quantized impulse for flavour.
5. **Tank contact** resolves in the server's vehicle collision pass: impulse vs an authored threshold, with a lever arm for trees; the result is the same state flip. With the one-timeline pivot, clients can simply wait for the flip (no predict/restore machinery needed); add prediction only if the ~100 ms delay reads badly in play.
6. **Rounds**: a per-pool `stopsBullets` flag and an HP decrement in the server's shot resolution; no fracture at the hit.
7. Fallen trees collide with tanks only until displaced; canopies and stumps never collide — this is the rule that stops physics fights between a 60 t hull and a client-local trunk.
8. Pitfalls recorded by others: snapshot/delta ordering (Dagor), instance id stability across versions (name-hash remap), per-tree memory at forest scale (Reforger: 805 B/tree), vehicles ghosting through or trivially felling trees (Reforger), network load capping player count (HLL).
9. Avian 0.7: a swap is collider removal + insertion on a static body; the cost of hundreds of such swaps per match is unmeasured here and should be measured before committing to per-fence destructibles.

## Sources

Dagor clone (lane 1): paths and lines above, all under `prog/` of the clone root stated in lane 1.
- [S] Arma Reforger Dev Report #22: https://reforger.armaplatform.com/news/dev-report-22
- [S] Arma Reforger experimental 2024-12-13 changelog: https://reforger.armaplatform.com/news/experimental-december-13-2024
- [S] Arma Reforger changelog 2025-03-27: https://reforger.armaplatform.com/news/changelog-march-27-2025
- [S] Arma Reforger "World Collisions Overhaul" (community mod page): https://reforger.armaplatform.com/workshop/67323A7B77DD1C42-WorldCollisionsOverhaul
- [S] EA, "How Battlefield 6 redefined destruction": https://www.ea.com/news/how-battlefield-6-redefined-destruction
- [S~] Insider Gaming, BF6 destruction after dropping PS4/Xbox One: https://insider-gaming.com/battlefield-6s-destruction-improved-after-dropping-ps4-and-xbox-one/
- [S] War Thunder roadmap spring–summer 2024: https://warthunder.com/en/news/8716-development-the-war-thunder-roadmap-for-spring-to-summer-2024-en
- [S] War Thunder, new effects and cockpits in Tusk Force (2025): https://warthunder.com/en/news/9710-development-new-effects-detailed-cockpits-coming-in-tusk-force-en
- [S~] War Thunder Tusk Force changelog: https://forum.warthunder.com/t/war-thunder-tusk-force-changelog/269064
- [S] Gaijin, War Thunder 1.53 Firestorm (PhysX Destruction): https://gaijinent.com/news/158
- [S~] MassivelyOP, 2015 WT destructibility: https://massivelyop.com/2015/08/09/war-thunder-is-within-a-hairs-breadth-of-a-complete-destructibility-system/
- [S~] Hell Let Loose FAQ (404 at fetch time; content via search summary): https://www.hellletloose.com/faq
- [S~] Enlisted 1.24 notes: https://mp1st.com/news/enlisted-update-1-24-released-this-january-31
- [S~] Enlisted "Destruction" mode (2021): https://www.thesixthaxis.com/2021/09/22/enlisted-destruction-game-mode-update-tunisia/
- [S~] Squad player thread on fences (2018): https://steamcommunity.com/app/736220/discussions/0/1700541698693575942
- Not found: any primary Gunner HEAT PC statement on environment destruction; any Enlisted or BF6 statement on server authority.
