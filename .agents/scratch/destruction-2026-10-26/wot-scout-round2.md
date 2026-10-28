# WoT scout, round 2: rendering, terrain, shadows, optimization (received 2026-10-27)

Source: the `master` session in `../bounty`, by SendMessage. Provenance as stated: rendering is mostly
[ENGINE] native C++ (not decompiled); what follows is READ from `res/engine_config.xml`, decal
configs, client scripts and asset formats, plus BigWorld/Core architecture marked INFERRED. Design
input only.

## Trees and vegetation
- Every catalogue tree is a SpeedTree `.srt` (433 types; e.g. `vegetation/Broadleaves/Acacia/Acacia_01.srt`),
  and the same object is the destructible (health/physicParams from round 1). LOD ladder, canopy cards,
  wind and the far billboard are SpeedTree's own [INFERRED: mesh rungs → camera-facing billboard, switch
  by distance/screen size with cross-fade; billboard rung casts no shadow]. Switch distances live inside
  the `.srt`, unreadable without the SDK.
- Flora/vegetation is GPU-instanced per chunk: `instancingVBSize=512000`, `floraVBSize=16000000` (16 MB).
- Instances per map live in chunk `.cdata`; INFERRED hundreds to low thousands on a 1 km map.

## Terrain [mostly ENGINE "terrain2", INFERRED unless noted]
- Chunked heightmap, chunk = 100 m (READ). Up to ~4+ layers per chunk blended by per-chunk masks
  (READ `control_point_grid_step=1.0`, `control_point_hmap_offset=0.1`; mask resolution INFERRED
  128–256²). Per-chunk geo-mip / vertex-morph LOD, no skirts [INFERRED].
- **Craters and track marks are decal-only; the terrain mesh is never deformed** (READ:
  `decals_config.xml`, `gpu_decals_config.xml`, `ObjectsFarPlaneManager.decalCullDistance=400`,
  `vehicleTraces` in `client/vehicle_systems/model_assembler.py::assembleVehicleTraces`, LOD-gated).

## Shadows and lighting [INFERRED unless noted]
- READ: `animation.dynamicShadows=false` by default (presets enable); a dynamic-resolution autoscaler
  `criticalFPS=15`, `minScale=0.5` halves render resolution under load before dropping quality;
  `reducedFrameRate=60` unfocused.
- CSM count/resolution per preset not read; INFERRED 2–3 cascades, up to 2048–4096 at max. "Core"
  rewrite [INFERRED; `RenoirCore.WindowsDesktop.dll` ships]: deferred + PBR + HDR, SSAO, better CSM;
  sun via CSM + baked terrain/static lightmaps in `.cdata` + ambient/IBL probes.

## Materials and damage visuals [READ]
- Fashion system: destruction changes a model by disabling/swapping materials on the shared merged chunk
  model (`BigWorld.MaterialDisabler`, `WGStaticModelFashion`, `get/setChunkModelFashion`), no new
  geometry, ~0 extra draw calls.
- Damage stickers: `DestructibleEntity.damageStickers = ARRAY<UINT64>` ALL_CLIENTS; each packs
  (stickerID, segStart, segEnd, componentIndex): the penetration ray segment through the armour and the
  collision component; client projects the decal along the ray onto the hit component
  (`client/DestructibleStickers.py`, `client/VehicleStickers.py::DamageSticker`). Bit widths INFERRED.
- Track marks = `vehicleTraces` decals (distance gated); dust/mud = particles.

## Streaming and visibility
- Chunk 100 m, streamed by distance (`DestructiblesManager.onChunkLoad/onChunkLoose` hook native
  streaming). `.cdata` INFERRED: heightmap+normals+masks, static placements, flora, baked lighting, BSP
  collision, nav.
- Occlusion = GPU hardware occlusion queries, read one frame late (READ
  `renderer.useLatentOcclusionQueries=true`); no portals, no baked PVS.
- Far plane (READ `ObjectsFarPlaneManager`): `cullVolume=2800`, `controlPointCullDistance=1400`,
  `decalCullDistance=400`, `farPlaneDist≈1500` (superShot; per-preset otherwise); fog hides it. AoI
  (INFERRED ~564 m) is separate from draw distance. `StreamedTexturePrecache Mode=AUTO`.

## Budgets
- READ: `globalVBSize=4194304`, `globalIBSize=43690`, `instancingVBSize=512000`, `floraVBSize=16 MB`.
- READ streaming budget per preset: ULTRA 1.0, MAX 0.6, HIGH 0.5, MEDIUM 0.4, LOW 0.3, MIN 0.2.
- INFERRED drop order: dynamic shadows, SSAO/post, draw distance + flora density, texture resolution,
  then DRR scaling. Textures BC/DXT `.dds`. Tank tris per LOD INFERRED (LOD0 ~10–15k, far ~1–2k).

## CPU side [READ]
- Native: rendering, animation, physics, streaming, culling, terrain, SpeedTree. Python: entity ticks,
  effect orchestration, input, camera, GUI, destructible bookkeeping (orchestrates native work).
- Animation LOD by distance: skeletal update every N frames: ≥8 m → 2, ≥20 m → 3, ≥50 m → 4,
  ≥125 m → 5, ≥312 m → 6. `animation.parallel=true`, `optimize=true`, `streamCacheSizeKB=51200`;
  `concurrentRenderer=true`, `enableBalanceCPU=true`; Scaleform GC suspended during load.
- Static chunk props merged into batched draws (`WGStaticModelFashion`); a break disables materials on
  the merged model. `helpers.newFakeModel()` placeholder for effect attachment.

## The session's takeaways for a growing forest
1. Far rung as imposter/billboard, cross-faded by screen-space error, no shadow at that rung.
2. Instance per chunk, one combined VB; a handful of draws per chunk, not per tree.
3. Latent GPU occlusion queries over portals/PVS for an open 1 km map.
4. Decals, not deformation, for craters and tracks; cull decals at 400 m.
5. Animation frame-skip by distance for the tanks.
