# WoT scout round 12: track traces on terrain

Source: the `wot` session (WoT client mirror `model_assembler.py` and slice configs), 2026-10-28. Design input: the mechanisms are read for structure, never for numbers. R = read, I = inferred.

1. **Mechanism** (ENGINE+DATA, R; `Vehicular.VehicleTraces`, assembled in `model_assembler.assembleVehicleTraces`). A decal strip, or ribbon, is laid under each track into a trace buffer (`bufferPrefs`). It is not terrain deformation, and not a trail texture read by the terrain shader.
   - There is one strip per track side (`addTrackTrace` at `wlOffset`/`wrOffset`), or one per wheel on wheeled vehicles (`wheelsAnimator.getTraceConfigs`).
   - Layers: a per-ground albedo plus a `bump` (normal) entry (~lines 364-368).
   - No depth, parallax or AO channel is evident (I).
2. **Per tank** (DATA, R). The textures and geometry come from the chassis config, `chassisConfig.traces.tracesParams`: `textureSet`, `size`, `offset` and `centerOffsetFactor`. Similar chassis may share the art, but the binding is per chassis.
3. **Varies by ground** (ENGINE, R). `setTrackTextures` maps each terrain material kind to a texture (`DecalMap.getTextureSet`; `material_kinds.EFFECT_MATERIAL_IDS_BY_NAMES`).
   - The ground is known through `Vehicular.TerrainMatKindSensor` (`assembleTerrainMatKindSensor`). It casts down about 4 m at three points: left track mid, right track mid, and centre.
   - It reads the terrain collision's `TRIANGLE_MATERIALKIND` byte, the same taxonomy as round 10. There is no painted mask.
   - Left and right are sensed independently, so the two tracks can show different traces.
   - The sensor is active within about 100 m.
4. **Lifetime and budget** (R+I). Each track has a bounded strip buffer, recycled so that old segments drop. The cost is fixed per vehicle, however far the tank drives, and there is no evident per-map pool.
   - Culling: `tracesConfig.lodDist` and an LOD priority.
   - `activePostmortem` keeps the traces after the tank is knocked out.
   - The fade values live in native or packed data, which was not opened.
5. **Pivot turns and slip** (R+I). `setFlyingInfo(isLeftFlying, isRightFlying)` suppresses a lifted track's trace. The strip follows the real contact path (`setMovementInfo`), so a pivot turn smears it naturally (I). There is no separate slip or scuff texture.

**Takeaways:**
- The 3-point downward ground sensor picks a texture per ground type with no painted masks.
- Sensing the left and right tracks separately is nearly free.
- A fixed-length recycled strip bounds the cost.
- One material-kind byte serves both effects and traces.
