# WoT frame-cost mechanisms (round 13, from the `wot` session)

Received 2026-10-29 for the frame-cost triage (#151). Mechanisms and pointers only.
- **Sources:** the client's loose configs (`engine_config.xml`, `decals/gpu_decals_config.xml`), a readable mirror (`model_assembler.py`), and earlier rounds.
- **Labels:** [DATA, R] is read from config data; [SCRIPT, R] and [ENGINE, R] are read from script and engine sources. These are CLAIMED by the `wot` session.
- **GAP** marks a fact that needs the packed graphics settings or the executable, neither of which was opened.

## 1. Trees and forests

- **Middleware and batching** [DATA, R].
  - Trees are SpeedTree middleware.
  - `engine_config.xml` holds global LOD rungs `lod0`..`lod4` and `floraVBSize`.
  - Flora (grass, and the tree billboards or cards) is batched into shared vertex buffers, bounded by `floraVBSize`.
- **GAP:** the impostor or billboard rungs, and which trees cast shadows at what LOD, are not in the loose configs.
- **A falling tree switches representation** [SCRIPT, R; AreaDestructibles].
  - A standing tree is static flora.
  - When destroyed it becomes `DESTR_TYPE_TREE`, and the client swaps in a separate animated fall model (`showFallTree`, a 60 Hz pendulum).
  - At touchdown `destroyFallAtom` finalises it.
  - The path is static flora, then an animated model, then settled. Bushes only deactivate; they swap no model.

## 2. Tank tracks [ENGINE + DATA, R; `model_assembler.py`]

- **No per-link instancing or simulation for rendering.**
  - The visible track is a spline strip of segment models: `setupSplineTracks`, with `segmentModelLeft` and `segmentModelRight` laid along a left and a right spline.
  - A scrolling texture (`wheelsScroll`, `transmissionScroll`) makes the links read as tread.
- **Fit to the road wheels.**
  - The native `Vehicular.TrackNodesAnimator` bends the track over each wheel, per track node (thickness, gravity, `maxAmplitude`, damping), driven by a `wheelsDataProvider`.
  - The spline takes its movement data from the wheels animator (`setSplineTrackMovementData`).
  - This runs on the CPU and switches off with distance through `chassisLodDistance`. It is not GPU skinning of links.
  - Wheeled vehicles take a separate path, `GeneralWheelsAnimator`.

## 3. Draw calls

- **Flora** is batched into shared vertex buffers.
- **Submission bounds:** the global LOD rungs `lod0`–`lod4` and the cull distances `decalCullDistance`, `controlPointCullDistance` and `farPlaneDist` bound what is submitted.
- **GAP:** static batching or merging of map props, instancing of repeated props, and any draw-call or triangle budget.

## 4. Shadows

- **GAP in full:** cascade count and distances, which objects cast, and simplified shadow proxies.

## 5. Terrain

- **GAP:** mesh LOD and the number of material layers the shader blends.
- **Track marks are decals** [ENGINE, R; round 12]. They are not a deformation, and not a terrain texture layer.
  - The native `Vehicular.VehicleTraces` writes a recycled decal strip.
  - The strip is textured by terrain material kind through a `TerrainMatKindSensor`.
  - The terrain mesh and material are untouched, and each vehicle's buffer is bounded.

## Where WoT spends less than rendering raw

- **Tracks:** a scrolling texture on a spline strip, plus a CPU node animator that switches off with distance, instead of simulating or skinning hundreds of links.
- **Flora:** shared vertex buffers, aggressive global LOD rungs and cull distances.
- **Marks:** track marks and hull damage marks live in recycled decal strips or clustered atlas buffers, so their cost is bounded and never accumulates as geometry.
- **Particles:** an autoscaler (`engine_config` `PS_PERFORMANCE_*`, `criticalFPS`) lowers the spawn rate or LOD when frame time crosses a threshold. This is a dynamic quality governor, not a fixed budget.
- **Terrain kind:** one material-kind byte serves both ground effects and traces, so the ground is classified once, not twice.

**For a CPU-bound frame with 30 tanks:** in WoT, nearly every cosmetic subsystem carries an LOD link and a cull distance: the track node animator, traces, flora and particles. The heaviest of them also back off when frame rate drops. The pattern is that nothing cosmetic runs at full rate past its LOD distance. If Overmatch's 30-tank cost is per-tank cosmetic work running at full detail at every distance, gating it by distance and priority is the lever.

## Follow-ups offered (not requested)

The `wot` session can unpack the client packages, or use its reverse-engineering box, for these gaps:
- shadow cascades and shadow-casting LOD;
- tree impostor LOD;
- terrain mesh LOD and the material-layer count;
- static batching and instancing, and the draw-call budget.

**Held:** ask for these only once the measurement lane says which ones matter.
