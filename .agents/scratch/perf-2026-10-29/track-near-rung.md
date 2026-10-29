# Verdict

**Replace per-link entities with belt-level GPU instances, while retaining the current CPU fit at render cadence.** Publish a compact table of rigid shoe frames, reuse that table for CPU print stamping, and draw the existing shoe meshes from it. Keep the shadow ribbon, but give it retained topology and eventually generate its positions from the same belt data.

This preserves the current picture without simulating a different belt or replacing visible shoes with a textured strip.

**Expected saving per near tank:**

- **DERIVED:** remove **194 shoe entities** and their transform propagation; convert the existing belt anchors into **two mesh entities**, reducing shoe mesh bookkeeping from **194 entries to two — 99.0%**.
- **DERIVED:** eliminate **194 unconditional shoe-transform writes per frame**, leaving wheel and gear animation.
- **DERIVED, proposed format:** upload **3,104 bytes per tank per frame** for midpoint and direction records, plus belt metadata. Previous-frame records double that storage.
- **ASSUMED:** substantial reduction in shoe-related CPU propagation, extraction, visibility and preparation. **A defensible millisecond saving per tank is unavailable:** the measurements do not isolate those shoes from the rest of the scene.
- **DERIVED:** **no reduction in visible shoe triangles**. This is primarily a CPU representation improvement. It cannot honestly promise a fraction of today’s total GPU cost.

For the first prototype, compute the compact frames on the CPU. Prints already need CPU poses, so moving their inexpensive frame construction into compute would initially duplicate work. Test compute placement afterward if curve sampling or frame construction remains significant.

The current implementation is therefore **not the best scalable representation**, but its geometric fitting remains a sound starting point. The supplied evidence does not establish an exact-picture replacement that makes the near tank’s entire rendering cost dramatically smaller.

Evidence labels throughout: **MEASURED** means an existing measurement; **DERIVED** means source arithmetic or calculation from those measurements; **ASSUMED** means a proposed design or forecast; **CLAIMED** means unverified narrative. Engine versions, issue numbers and source locations are identifiers.

# Comparison

All exact alternatives below retain the existing shoe geometry, mirrored meshes, materials, phase registration and shadow-proxy shape unless explicitly stated.

| Approach | Visual parity | CPU cost against today | GPU cost against today | Build risk and prototype requirement |
|---|---|---|---|---|
| **Keep entities; cache inputs, guard writes, retain buffers** | Exact with complete cache keys and unchanged filter evolution. | Avoids redundant probes, allocations and unchanged writes. Moving shoes still incur per-link transforms and render bookkeeping. | Same geometry and vertex work. Mesh retention alone does not guarantee partial GPU uploads. | **Small–medium.** Establish the baseline first, especially parked tanks. |
| **Recommended: compact CPU frame table + GPU instances** | Exact rigid placement, subject to shader arithmetic validation. CPU print poses come from the same records. | Fit remains; cheap frame calculation remains; per-link ECS, propagation and extraction disappear. | Same geometry, plus frame-buffer access. Existing automatic batching means draw-count savings may be negligible. | **Medium–large.** Must cover opaque rendering, depth, feedback materials, masks and visibility. |
| **Upload curve; find endpoints and place shoes in the vertex shader** | Exact only if it reproduces the current polyline, material reparameterization and endpoint-chord placement. Smooth spline tangents change the picture. | Removes render-side CPU placement, but prints still need CPU sampling. Fit remains. | Repeats curve searches and frame construction for every processed vertex in every pass. | **Large.** Demonstrate that repeated vertex work costs less than a frame table. |
| **Upload curve/joints; compute a GPU frame table** | Same parity potential as the preceding route. | Removes render-side frame construction; retains CPU work needed by prints. | Constructs each frame once, then reuses it across vertices and passes. Adds dispatch, dependencies and buffer traffic. | **Large.** Compare directly with CPU-uploaded frames; do not assume compute wins. |
| **Static repeated-shoe mesh + frame-table material** | Exact geometry: each vertex has a shoe index and receives that shoe’s rigid transform. | Similar entity saving to custom instancing, while retaining ordinary material phases. | Same triangles. Duplicates shoe geometry in a shared asset and adds an index attribute. | **Medium.** Strong competing prototype because phase integration is simpler. |
| **Stock skinned belt** | Rigid weights can preserve shoes. Blended weights or fewer joints bend them and alter contact/shading. | Removes shoe mesh entities, but a rigid joint per shoe preserves changing joint entities and propagation. | Same shoe geometry plus skinning work. A reduced continuous belt changes the picture. | **Medium–large; weak return.** Verify joint costs before pursuing. |
| **Tick-rate fit + interpolated render curve/phase** | Generally approximate: contact onset, sag and clearances can differ between samples. | Fewer fit evaluations only when render cadence exceeds fit cadence. Entity removal is a separate improvement. | Same geometry, potentially interpolation and duplicate history reads. | **Large for strict parity.** Must prove terrain contact and temporal behavior. |
| **WoT-style spline strip with scrolling texture** | **Fails the requested near-view parity.** Changes gaps, silhouette, rigid articulation and close shading. | Can remove much of the link machinery. | Can substantially reduce geometry. | Relevant for another distance rung; unsuitable as the answer here. |

The WoT description is **CLAIMED prior art**, not independently verified engine behavior: [wot-render-mechanisms.md:21](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/perf-2026-10-29/wot-render-mechanisms.md:21). It also does not provide enough evidence to infer precisely how its segment geometry compares with these shoes.

## What the recommended renderer actually preserves

Today, a shoe spans consecutive sampled pin joints. Its orientation follows their **chord**, and its origin is corrected using the authored template frame: [link_view.rs:809](/Users/Yan/github/vikng-dev/overmatch/src/track/link_view.rs:809).

The replacement should reproduce that construction:

1. Keep the fitted closed polyline and current joint sampling.
2. Produce each shoe’s midpoint and normalized chord direction.
3. Apply the template correction and rigid rotation to every vertex, normal and tangent.
4. Apply the presented hull transform.

**Do not stretch a shoe to fill its chord or bend its vertices along the curve.** Either changes today’s contact and silhouette.

The current phase decomposition also rotates entity-to-station assignment so material identity travels around the belt: [link_view.rs:1113](/Users/Yan/github/vikng-dev/overmatch/src/track/link_view.rs:1113). Replace the entity identity with a stable key such as `(tank lifetime, side, material-link index)`.

For full GPU curve placement, upload the actual closed polyline and cumulative lengths. Sample consecutive endpoints using the same material-to-drawn-length mapping as [wrap.rs:674](/Users/Yan/github/vikng-dev/overmatch/src/track/wrap.rs:674). Interpolating a coarse collection of frames is not automatically equivalent.

**DERIVED payload examples for the supplied fixture:**

| Representation | Payload per tank, excluding headers |
|---|---:|
| Current pin joints: side-plane position | `194 × 2 × 4 = 1,552 bytes` |
| Proposed rigid frames: midpoint and direction | `194 × 4 × 4 = 3,104 bytes` |
| Proposed affine reference palette | `194 × 12 × 4 = 9,312 bytes` |

These are proposed formats, not measurements of Bevy’s current upload sizes.

## Bevy support, verified against source

The project patches its PBR renderer to the vendored Bevy source identified in [Cargo.toml:132](/Users/Yan/github/vikng-dev/overmatch/Cargo.toml:132).

- **Custom instancing exists.** The [official versioned example](https://github.com/bevyengine/bevy/blob/v0.19.0/examples/shader_advanced/custom_shader_instancing.rs) supplies an instance buffer and custom draw command. However, it registers `Transparent3d` and disables built-in frustum culling. It is a mechanism example, not a complete opaque PBR replacement.
- **Material storage buffers exist.** The [versioned storage-buffer example](https://github.com/bevyengine/bevy/blob/v0.19.0/examples/shader/storage_buffer.rs) uses `ShaderBuffer`, `#[storage(..., read_only)]` and `MeshTag`.
- **`MeshTag` is an indexing mechanism.** It can select a belt-table offset; it does not manufacture additional mesh instances or remove existing entities. See [mesh_functions.wgsl:165](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/render/mesh_functions.wgsl:165).
- **Material extensions can replace vertex processing** for main, prepass and deferred rendering: [extended_material.rs:33](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/extended_material.rs:33). Shadows use prepass specialization: [light.rs:2505](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/render/light.rs:2505).
- **Stock skin joints are entities.** `SkinnedMesh` contains `joints: Vec<Entity>` in the [versioned source](https://github.com/bevyengine/bevy/blob/v0.19.0/crates/bevy_mesh/src/skinning.rs#L16-L20). The vendored renderer queries their global transforms and constructs joint matrices: [skin.rs:286](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/render/skin.rs:286), [skin.rs:399](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/render/skin.rs:399).

A stock skinned mesh therefore does not solve the principal transform problem. A custom palette without entity joints becomes essentially the proposed renderer.

# Consumer audit and sim–view boundary

The following consumer requirements apply to custom instances, compute placement and the repeated-mesh alternative. Entity-preserving changes retain the existing paths. Stock skinning also needs these adaptations wherever shoe entities disappear.

References to the pen cam, raycasts and hull marks here concern **the game’s damage model**.

| Consumer | What the code actually consumes | Replacement contract |
|---|---|---|
| **Print stamping** | Traverses belt children; reads `TrackLink` global transforms; tracks grounding and sweeps by shoe entity. [track_prints.rs:402](/Users/Yan/github/vikng-dev/overmatch/src/track_prints.rs:402), [stamper.rs:196](/Users/Yan/github/vikng-dev/overmatch/src/track_prints/stamper.rs:196). | Iterate stable material-link records. Construct canonical world poses from the shared frame table and presented hull transform. Preserve grounding, press accumulation, sweep interpolation and snap resets. **No GPU readback.** |
| **Shadow proxy** | Already consumes fitted joints directly, not link transforms. [view.rs:789](/Users/Yan/github/vikng-dev/overmatch/src/track/view.rs:789). | Retain the same joint polygon and section. Initially retain CPU mesh generation; then move ribbon position generation to a shader or compute path. Preserve caster readiness and render scope. |
| **Live feedback/x-ray view** | Walks tank descendants, replaces `StandardMaterial`, and creates mask-copy children. Shoes are included; shadow proxies are excluded. [xray.rs:694](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/pen_cam/game/xray.rs:694). | Add explicit belt drawing for the feedback look and mask cameras. Share deformation, rung, phase and visibility. Generic material replacement would otherwise discard the belt vertex shader. |
| **Feedback mask copies** | Copy source mesh handles and follow rung swaps. [stage.rs:635](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/pen_cam/stage.rs:635), [xray.rs:955](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/pen_cam/game/xray.rs:955). | Draw the same GPU instances or repeated mesh with deformation-aware mask materials. Preserve material-specific culling, blending and ordering. |
| **Separate pen-cam stage** | Uses separately classified `ViewMesh` geometry. It does not query the live `TrackLink` pool. [stage.rs:453](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/pen_cam/stage.rs:453). | Its existing scene remains unchanged. Do not accidentally redirect it to the live belt representation. |
| **Raycasts and picking** | Spawned links contain mesh, material, transform and parent, with no collider: [link_view.rs:974](/Users/Yan/github/vikng-dev/overmatch/src/track/link_view.rs:974). Searches found no shoe mesh-picking consumer; game raycasts use simulation collider geometry. | Retain simulation colliders. There is no discovered shoe-raycast service to reproduce. Any future shoe picking needs an explicit CPU belt query or GPU ID pass. |
| **Hull marks** | Marks attach through `SimParts` and `ViewNode`, not shoe entities. Their forward decals depend on scene depth. [hull_marks.rs:210](/Users/Yan/github/vikng-dev/overmatch/src/vfx/hull_marks.rs:210). | Keep attachment behavior. Ensure deformed shoes write correct prepass depth so existing decal occlusion/fading remains correct. |
| **LOD selection** | Chooses a whole-side rung from the nearest shoe’s current world-space mesh origin, measured from declared `PlayerView`. [link_view.rs:1021](/Users/Yan/github/vikng-dev/overmatch/src/track/link_view.rs:1021). | Compute the same minimum from CPU frame records, including template-origin correction. Select the shared instance mesh or repeated-mesh rung. A belt-center distance changes transitions. |
| **Showcase and sandbox** | Showcase pins `ShoeBelt`; sandbox controls shoe visibility and uses the shared placer. [lod_showcase.rs:301](/Users/Yan/github/vikng-dev/overmatch/src/lod_showcase.rs:301), [sandbox link_view.rs:103](/Users/Yan/github/vikng-dev/overmatch/src/track_sandbox/link_view.rs:103). | Preserve the belt pin API and layer switches. Keep the current placer as a reference adapter during development. |
| **Lifetime and hot reload** | Reinstantiation despawns belts and proxies, then rebinds. [view.rs:241](/Users/Yan/github/vikng-dev/overmatch/src/track/view.rs:241). | Release GPU allocations and invalidate pose/history records with the same tank/belt lifetime. |

**The links are view-only; no link-related breach was found.** Their spawn path adds no simulation components. Track forces run separately in fixed update from simulation state, gear data and the terrain oracle: [sim.rs:368](/Users/Yan/github/vikng-dev/overmatch/src/track/sim.rs:368), [sim.rs:760](/Users/Yan/github/vikng-dev/overmatch/src/track/sim.rs:760). Presentation plugins are mounted separately: [lib.rs:587](/Users/Yan/github/vikng-dev/overmatch/src/lib.rs:587), [lib.rs:663](/Users/Yan/github/vikng-dev/overmatch/src/lib.rs:663).

The replacement must keep that direction: simulation phase/speed and presented pose feed cosmetic belt data. GPU output must never initialize or update rollback state.

# Cost and visual limits

## What the existing timings establish

The attribution report labels its system timings **DERIVED from MEASURED sampling**, rather than directly instrumented measurements.

- **DERIVED:** current `drive_track_views` averages `0.29/2 = 0.145 ms/tank/frame` in the smaller scene and `3.89/30 ≈ 0.130 ms/tank/frame` in the larger scene.
- **DERIVED:** its height-grid ray work averages approximately **0.034 ms/tank/frame** in both samples.
- **DERIVED:** the remaining driver work averages approximately **0.096–0.111 ms/tank/frame**. That includes fitting, placement, filters, wheels and ribbon generation; it is not all removable placement cost.

Consequently, removing shoe entities does **not** justify claiming the entire driver saving. Its larger potential benefit lies in engine work outside that driver.

The **DERIVED 5.59 ms transform** and **5.42 ms mesh-preparation** totals belong to the whole smaller scene. [Issue #168](https://github.com/vikng-dev/overmatch/issues/168) also identifies an unresolved re-extraction/channel suspicion. Neither total can be assigned proportionally to the shoes.

Bevy already has change-filtered mesh extraction and changed queues: [mesh.rs:1907](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/render/mesh.rs:1907), [mesh.rs:2493](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/render/mesh.rs:2493). The problem is not evidence that every unchanged shoe is blindly rebuilt.

## GPU geometry

For the supplied fixture, assuming every shoe reaches each relevant pass:

| Work | DERIVED count per tank |
|---|---:|
| Shoe triangles in main pass | `194 × 1,520 = 294,880` |
| Shoe triangles in depth prepass | `294,880` |
| Combined shoe triangles | `589,760` |
| Ribbon triangles per shadow view | `2 × 97 × 8 = 1,552` |
| Ribbon triangles across the configured shadow cascades | `1,552 × 3 = 4,656` |

These are **submitted triangle estimates before visibility rejection**, not measured GPU invocations.

Let `V` be the actual shoe vertex count. Full visibility produces approximately **DERIVED `388V` shoe vertex invocations across main and depth**, subject to GPU indexing/cache behavior. Triangle corners are not interchangeable with unique vertices.

All exact-geometry alternatives retain those triangles. Compute can amortize placement arithmetic; it cannot remove the geometry itself.

A whole-belt draw also loses today’s per-shoe frustum rejection unless explicitly restored. It may process **more** vertices when the player camera sees only part of its own tank. Supply conservative deformed bounds; optionally compact visible instances per view if measurements justify it.

## Temporal and shading parity

- Preserve the mirrored mesh’s winding, normals and tangent handedness: [link_view.rs:823](/Users/Yan/github/vikng-dev/overmatch/src/track/link_view.rs:823).
- Share identical position evaluation between main and depth shaders. Bevy explicitly warns about intermediate precision with equal-depth testing: [mesh_functions.wgsl:58](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/render/mesh_functions.wgsl:58).
- No motion-vector prepass or motion-blur use was found in `src`. If enabled later, retain **previous presented-frame shoe deformation**, not merely the belt entity’s previous transform. Bevy’s prepass evaluates previous deformation explicitly: [prepass.wgsl:135](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/prepass/prepass.wgsl:135).
- On spawn, teleport, terrain revision or rebind, seed current geometry before revealing it; initialize previous history consistently. Do not interpolate across the discontinuity.
- Keep shoes casting until their replacement ribbon is ready. The current readiness handoff is deliberate, not redundant machinery.

## Split cadence is a separate approximation

**DERIVED:** fitting at the proposed **ASSUMED 64 Hz** instead of **ASSUMED 120 rendered frames/s** reduces fit evaluations by `1 − 64/120 = 46.7%`.

That calculation does not establish parity or a corresponding frame-time reduction. At the report’s **MEASURED approximately 55 rendered frames/s**, performing every fixed-rate fit could instead increase evaluation count.

More fundamentally:

- Tick-world geometry is not the presented hull pose.
- Interpolating curves can delay an obstacle’s contact onset or temporarily enter terrain/wheels.
- Different polyline topologies cannot safely be interpolated by array index.
- Existing wheel, belly and slack filters evolve with render time.
- GPU phase must remain synchronized with CPU prints and wheel/tooth animation.

The exact optimization is to cache unchanged **raw targets**, continue filter evolution and phase at render cadence, and skip geometry only when its actual dependencies are unchanged. Tick interpolation should be evaluated later as a declared visual tradeoff.

# Attack on today’s implementation

| Location | Work beyond what the picture requires | Exact-result remedy |
|---|---|---|
| [view.rs:637](/Users/Yan/github/vikng-dev/overmatch/src/track/view.rs:637) | Every tank performs the complete view pipeline every rendered frame without an unchanged-input rejection. | Separate raw terrain targets, filter evolution, fitted geometry and phase sampling. |
| [view.rs:715](/Users/Yan/github/vikng-dev/overmatch/src/track/view.rs:715), [wheels.rs:92](/Users/Yan/github/vikng-dev/overmatch/src/track/wheels.rs:92) | Repeats the wheel fan even when pose and terrain-response inputs are unchanged. **DERIVED fixture count:** `16 × 21 × 3 = 1,008 queries/frame`. | Cache exact raw targets. Continue wheel filters while they evolve. |
| [wrap.rs:540](/Users/Yan/github/vikng-dev/overmatch/src/track/wrap.rs:540), [wrap.rs:565](/Users/Yan/github/vikng-dev/overmatch/src/track/wrap.rs:565) | Rebuilds and probes dense lower-run stations. | Cache immutable geometry and raw probe results where dependencies permit; preserve directional-query semantics. |
| [view.rs:727](/Users/Yan/github/vikng-dev/overmatch/src/track/view.rs:727), [wrap.rs:199](/Users/Yan/github/vikng-dev/overmatch/src/track/wrap.rs:199), [wrap.rs:533](/Users/Yan/github/vikng-dev/overmatch/src/track/wrap.rs:533) | Repeated transient vector allocation for circles, stations and intermediate fields. | Retain per-rig scratch capacity. **DERIVED audit lower bound:** **34 nonempty vector buffers/tank/frame**, including proxies. |
| [view.rs:776](/Users/Yan/github/vikng-dev/overmatch/src/track/view.rs:776) | Unconditionally writes every shoe transform, even when equal. | Baseline: `set_if_neq`. Structural fix: replace shoe transforms with contiguous frame records. |
| [view.rs:831](/Users/Yan/github/vikng-dev/overmatch/src/track/view.rs:831) | Unconditionally writes wheel translation and rotation. | Compare the complete intended pose before writing. |
| [link_view.rs:809](/Users/Yan/github/vikng-dev/overmatch/src/track/link_view.rs:809) | Converts chord direction through angle/quaternion construction for every shoe. | After parity validation, use normalized planar direction directly for canonical rigid placement. |
| [link_view.rs:974](/Users/Yan/github/vikng-dev/overmatch/src/track/link_view.rs:974) | Represents every cosmetic shoe as a full mesh entity and hierarchy leaf. | Belt-level instances or repeated geometry with a shared frame table. |
| [link_view.rs:1041](/Users/Yan/github/vikng-dev/overmatch/src/track/link_view.rs:1041) | Fetches every shoe’s global transform to choose a belt-wide rung. | Scan the contiguous frame table for the same corrected world origins. |
| [view.rs:793](/Users/Yan/github/vikng-dev/overmatch/src/track/view.rs:793) | Replaces each dynamic ribbon mesh every frame. | Retain mesh buffers first; generate positions from shared GPU belt data later. |
| [shadow_proxy.rs:351](/Users/Yan/github/vikng-dev/overmatch/src/track/shadow_proxy.rs:351), [shadow_proxy.rs:377](/Users/Yan/github/vikng-dev/overmatch/src/track/shadow_proxy.rs:377) | Reallocates attributes and rebuilds indices. Winding is rechecked from geometry. | Retain topology while count/winding invariants hold. Preserve winding validation where those invariants are not guaranteed. |
| [track_prints.rs:453](/Users/Yan/github/vikng-dev/overmatch/src/track_prints.rs:453) | Reconstructs access to an already-known belt through children and entity queries. | Iterate shared CPU pose records with stable material identities. |

**DERIVED:** current raw ribbon attributes and indices total approximately **43,456 bytes/tank/frame**, before extraction/copies. Retained topology avoids repeated CPU construction; a procedural ribbon avoids repeatedly publishing full mesh payloads.

Two safeguards matter:

- A cache key needs presented pose, terrain revision, print-response inputs, articulated circles and relevant geometry. **An unchanged hull pose alone is insufficient.**
- The current ribbon follows the **phase-sampled joint polygon**. Phase changes can change that polygon even on an unchanged fitted curve. Ignoring phase in ribbon invalidation changes its shadow.

Existing optimizations deserve credit: rung selection already guards unchanged state and uses `set_if_neq`; sprocket and idler transforms do too. Shoes already batch. None of those wins should be counted again.

The design narrative’s **CLAIMED historical approximately 56 µs/tank/frame** is not a current all-in budget. Its scalable rendering seam already describes entity-per-link as bring-up: [architecture.md:397](/Users/Yan/github/vikng-dev/overmatch/.agents/docs/design/track-model/architecture.md:397). The source and current attribution, rather than that historical figure, should govern this decision.

# Throwaway prototype plan

**Decision: prototype entity-free rendering before changing fit cadence.**

1. **Establish the exact baseline.** Guard shoe/wheel writes, retain scratch and ribbon topology, and cache unchanged raw targets. Keep filter evolution and phase behavior. This separates easy waste removal from the renderer’s structural benefit.

2. **Publish one shared belt snapshot.** Keep current CPU fitting and sampling. Include rigid frames, stable material identities, corrected origins for LOD, joints for shadows, and discontinuity state. Make prints consume it without entity queries or GPU readback.

3. **Compare the rendering adapters.**
   - Custom GPU instances using shared shoe geometry.
   - Static repeated-shoe geometry with a shoe-index attribute and material extension.

   Both consume identical frame data. Start with the affine reference palette; compact it only after confirming parity.

4. **Include difficult consumers immediately.** Implement live feedback look and mask rendering, depth, scope inheritance, LOD swaps, partial-frustum views and ribbon readiness. An attractive main-camera screenshot is insufficient.

5. **Attack visual parity.** Compare parked sag, rough-ground articulation, phase wrapping, reverse motion, skid, wheelspin, close normal-map lighting, optic changes, spawn, teleport and hot reload. Check print contact/sweeps and the ribbon polygon against the reference path. Confirm feedback transparency and mask composition as well as ordinary opaque rendering.

6. **Measure the claimed saving.** A future authorized implementation lane should compare current, optimized baseline and both entity-free adapters in the same session. Attribute fit, frame construction, print work, propagation, extraction/preparation and GPU passes separately. Include views where most of the belt is outside the frustum. Investigate the unresolved issue #168 re-extraction population independently.

7. **Test compute only if warranted.** Compare CPU-uploaded frames against compute-generated frames from the same joints. Try full curve sampling only if its retained CPU cost matters. Keep whichever reduces total CPU/GPU frame time while passing the same visual checks.

**ASSUMED build size:** baseline caching is small to medium; the repeated-mesh prototype is medium; production custom instancing is medium to large because feedback materials, depth and visibility are part of the renderer. Tick-interpolated fitting is a separate, larger visual-policy change.

No files were changed, no builds or application runs were performed, and the excluded documents were not opened.