# Static performance audit

**I would start with caching the static environment light, eliminating unchanged track fitting, and pricing the cheaper shadow/MSAA configuration.** These have the strongest combination of source evidence and likely gain. Terrain shading and retained tank/tree entity structure come next.

This report is read-only: no edits, builds, game runs or tests. Counts describe source/data inventories before visibility culling; **entities are not draw calls**. Asset counts are fixture data, not vehicle-specific optimization rules. Any references to crew, impacts or projectiles concern the **game’s damage model**, where crew are HP pools at stations.

**Evidence labels:** MEASURED = supplied measurements or existing recorded census; DERIVED = source/data arithmetic; ASSUMED = expected benefit or proposed choice; CLAIMED = unverified prose. Engine versions, source locations and item numbers are identifiers rather than measurements.

The target is **ASSUMED/product requirement: 120 fps**, or **DERIVED: 8.33 ms/frame**, at the requested display size. The supplied **MEASURED** GPU-bound duel and CPU-bound larger scene require separate remedies. Savings below overlap and must not be added together.

## Ranked opportunities

Ranking is **ASSUMED qualitative expected gain × confidence**, not a fabricated timing score.

### 1. Filter the static sky once instead of every frame

**Expected gain: ASSUMED large GPU opportunity; high confidence that the work is redundant.**

[world.rs:227](/Users/Yan/github/vikng-dev/overmatch/src/world.rs:227) attaches `GeneratedEnvironmentMapLight` to a cubemap constructed once. The compiled renderer nevertheless prepares and filters it every render frame:

- [generate.rs:842](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/light_probe/generate.rs:842): unconditional downsampling query.
- [generate.rs:942](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/light_probe/generate.rs:942): unconditional filtering query.
- [generate.rs:559](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/light_probe/generate.rs:559): repeated bind-group, view and uniform preparation.

**DERIVED recurring work per generated view:**

- Diffuse: `32² × 6 × 1024 = 6,291,456` environment samples.
- Specular: `6 × (64²×32 + 32²×64 + 16²×64 + 8²×128 + 4²×128 + 2²×256 + 1²×512) = 1,348,608` importance-sampling iterations; some skip their texture lookup.
- Mirror mip: `128² × 6 = 98,304` lookups.
- Copy, downsampling and convolution occupy **DERIVED five compute passes**, plus per-mip dispatches.

**Cheaper method:** retain prefiltered diffuse/specular maps and attach ordinary `EnvironmentMapLight`. This preserves the lighting and concealment. **DERIVED gain:** remove all steady-state generation work; milliseconds remain unknown.

Bevy supports prefiltered maps directly; verified against [v0.19.0 generator source](https://raw.githubusercontent.com/bevyengine/bevy/v0.19.0/crates/bevy_pbr/src/light_probe/generate.rs).

**Important attribution:** generation runs outside the camera’s render schedule. Its cost may also exist in the measured no-draw loop; its saving is not confined to the camera-rendering delta.

### 2. Cache track contacts and separate terrain fitting from belt animation

**Expected gain: ASSUMED large CPU opportunity at match scale; high confidence in unnecessary evaluations.**

[track/view.rs:637](/Users/Yan/github/vikng-dev/overmatch/src/track/view.rs:637) processes every tank each rendered frame. Wheel probes, wrap fitting and shoe placement run without an unchanged-input or settled-state rejection.

**DERIVED from fixture data and sampling loops:**

`16 wheels × 21 arc samples × 3 lateral stations = 1,008 wheel rays/tank/frame`.

At the requested match size this becomes **DERIVED 30,240 wheel rays/frame**, before wrap probes. Wrap adds **DERIVED three rays per dense station**: `src/track/wrap.rs:540,565`.

**Cheaper method:** cache raw contact targets by presented hull pose, terrain revision and print-response inputs. Continue cheap phase scrolling and smoothing at render cadence. For moving tanks, update contact targets less frequently and interpolate.

At target cadence, **DERIVED `1 − 64/120 = 46.7%` fewer evaluations** for a task moved to the fixed cadence. That is a work-count saving, not a measured frame-time saving. At today’s slower frame rate, moving it to that cadence does not inherently save work.

The supplied **MEASURED 1.74–2.14 ms/frame** caster cost includes shared simulation and presentation callers. It cannot all be credited to this change. Authority contact is already fixed-rate, and non-dynamic network replicas already skip its force calculation: `src/track/sim.rs:376,720`.

Bevy supports fixed schedules and timer conditions; verified in [v0.19.0 fixed-time source](https://raw.githubusercontent.com/bevyengine/bevy/v0.19.0/crates/bevy_time/src/fixed.rs).

**Constraint:** `HeightGrid::cast_ray` already uses bounded cell DDA and analytic triangle intersections. Replacing oblique casts with `height_at` changes contact semantics; thin-ridge counterexamples are documented at `src/track/oracle.rs:369`. Cache or specialize the exact query before approximating it.

### 3. Use fewer shadow cascades and cheaper casters while retaining the envelope

**Expected gain: ASSUMED substantial; strongest existing timing evidence.**

[settings.rs:2129](/Users/Yan/github/vikng-dev/overmatch/src/settings.rs:2129) constructs the cascades. The shipped configuration uses **DERIVED three cascades**, with **DERIVED 4096-square** maps.

The proposed cheaper configuration uses **ASSUMED two cascades at 2048-square**, retaining the configured shadow distance:

`1 − (2 × 2048²)/(3 × 4096²) = 83.3%`

**DERIVED reduction in nominal shadow-map texel capacity**, not equivalent frame-time reduction. Geometry submission, cascade fitting and fragment shadow sampling do not follow that ratio.

The supplied sweep gives **MEASURED shadows-off savings of 2.26 ms** in the duel and **DERIVED `19.16 − 16.36 = 2.80 ms`** in the larger scene. These price the whole shadow feature, not this cheaper configuration. Earlier cascade measurements support the direction but are not a current-scene forecast.

**Cheaper method:** fewer cascades, smaller maps, and silhouette-preserving caster proxies for decoration/running gear. Track-shoe shadow proxies already exist; do not count that win again.

Bevy supports cascade configuration, verified against [v0.19.0 cascade source](https://raw.githubusercontent.com/bevyengine/bevy/v0.19.0/crates/bevy_light/src/cascade.rs).

**Product cost:** shadows off buys the measured saving by removing shadow information. Smaller maps or approximate proxies can alter shadow coverage and leakage. Those costs must be evaluated rather than described as free.

### 4. Disable MSAA for the LOW candidate

**Expected gain: ASSUMED medium-to-large GPU opportunity; timing unmeasured.**

[camera.rs:320](/Users/Yan/github/vikng-dev/overmatch/src/camera.rs:320) enables HDR and the depth prepass; `src/settings.rs:482` selects the default **DERIVED four-sample MSAA** configuration.

**Cheaper method:** existing `Msaa::Off` setting.

**DERIVED work reduction:** multisample storage/coverage/depth sample count falls from four to one. Soft particles also stop fetching **DERIVED three additional depth samples per applicable fragment**, because `assets/shaders/vfx_billboard.wgsl:71` resolves every MSAA depth sample itself.

This does **not** mean four times less terrain fragment shading. Ordinary MSAA can shade once and replicate coverage.

Supported directly by [Bevy v0.19.0 `Msaa`](https://raw.githubusercontent.com/bevyengine/bevy/v0.19.0/crates/bevy_render/src/view/mod.rs).

**Product cost:** edge aliasing and possible coverage differences. The current conifer canopy is opaque geometry, so an alpha-to-coverage fallback is not its current rendering path.

### 5. Cache static terrain appearance instead of recomputing it per pixel

**Expected gain: ASSUMED potentially large GPU opportunity; medium confidence in net benefit.**

[terrain_blend.wgsl:405](/Users/Yan/github/vikng-dev/overmatch/assets/shaders/terrain_blend.wgsl:405) samples and blends material layers every fragment. [terrain_blend.wgsl:464](/Users/Yan/github/vikng-dev/overmatch/assets/shaders/terrain_blend.wgsl:464) computes world-fixed macro noise.

**DERIVED ordinary texture requests:** `6 + 3k`, where `k` is the number of sampled overlay layers, excluding lighting, shadow/environment sampling and near-print relief. The base ARM requests may benefit from texture cache/compiler reuse.

Macro tint performs **DERIVED two octaves × four hashes = eight sine hashes/terrain fragment**.

**Cheaper methods:**

- Cache the macro field in a texture.
- Cache far terrain’s mask-conditioned appearance into composite pages.
- Apply dynamic prints afterward; retain geometry and a suitable detail-normal layer.

**DERIVED potential:** replace the eight hashes with lookup work; composite material requests can fall toward **ASSUMED three–four requests** per affected fragment. Net saving depends on cache residency, resolution and memory bandwidth.

The game already uses the required `MaterialExtension` at `src/terrain_blend.rs:275`; [v0.19.0 PBR source](https://raw.githubusercontent.com/bevyengine/bevy/v0.19.0/crates/bevy_pbr/src/render/pbr_fragment.wgsl) verifies the material path. Composite-page management is custom work.

**Product cost:** finite-resolution caching changes appearance. Coarsening terrain geometry additionally changes concealment; that is a separate, larger cost.

### 6. Use lighting-only track prints; factor the sun contribution if relief remains

**Expected gain: ASSUMED potentially substantial on affected terrain pixels; average unknown.**

[terrain_blend.wgsl:328](/Users/Yan/github/vikng-dev/overmatch/assets/shaders/terrain_blend.wgsl:328) selects relief work. [terrain_blend.wgsl:489](/Users/Yan/github/vikng-dev/overmatch/assets/shaders/terrain_blend.wgsl:489) applies ordinary lighting, then reconstructs direct sun lighting to subtract the self-shadowed portion.

**DERIVED lookup counts per qualifying fragment:**

- Lighting-only near prints retain five near-store samples: center plus gradient neighbors.
- Parallax adds an initial sample plus up to **DERIVED 256** march samples.
- Sun relief adds **DERIVED six** march samples.
- Self-shadowed pixels additionally repeat directional BRDF and cascade-shadow work.

**Cheaper method:** existing Lighting setting. **DERIVED worst-case removal:** up to `1 + 256 + 6 = 263` near-store lookups plus duplicated direct-light work. Typical savings are much smaller and depend on print coverage.

Further methods: occupancy rejection for provably empty print regions; cached print gradients; fold sun visibility into its original lighting contribution.

Bevy supports the existing custom material path; factoring its lighting is custom shader work.

**Product cost:** Lighting preserves trodden coverage/darkening while changing relief. The earlier “free relief” measurement drew nothing and supplies no GPU evidence.

### 7. Reduce retained tank structure; use continuous distant belts where acceptable

**Expected gain: ASSUMED medium-to-large scaling benefit; medium confidence.**

[geometry_lod.rs:377](/Users/Yan/github/vikng-dev/overmatch/src/geometry_lod.rs:377) retains separate entities for every body rung. [track/link_view.rs:1035](/Users/Yan/github/vikng-dev/overmatch/src/track/link_view.rs:1035) selects a shoe rung but retains every shoe.

**DERIVED retained mesh entities/tank:**

`89 source + 270 rung + 194 shoes + 2 proxies = 555`.

**DERIVED larger-scene inventory:** `555 × 30 = 16,650` mesh entities, excluding simulation and hierarchy anchors.

**Cheaper methods:** merged immutable body pieces at material/moving-part seams; per-part mesh selection; distant continuous belts with scrolling detail.

Tracks already instance. Their existing last rung contains **DERIVED `194 × 166 = 32,204` triangles/tank**. A ribbon comparable to the caster contains **DERIVED 1,552**, a **DERIVED 95.2% geometry reduction**. Replacing shoe entities with side belts removes **DERIVED `(194 − 2) × 30 = 5,760` mesh entities**.

Bevy supports shared-handle instancing and HLOD: [automatic instancing](https://raw.githubusercontent.com/bevyengine/bevy/v0.19.0/examples/shader/automatic_instancing.rs), [visibility ranges](https://raw.githubusercontent.com/bevyengine/bevy/v0.19.0/crates/bevy_camera/src/visibility/range.rs). Animated belts require custom shader work.

**Product costs:** belt gaps/silhouette change; merged parts need correct articulation and detachment. Selecting one mesh handle also loses the current independent per-view LOD choice.

### 8. Merge identical standing-tree geometry; instantiate segmentation when needed

**Expected gain: ASSUMED meaningful CPU opportunity; high confidence in structural reduction.**

[trees/view.rs:29](/Users/Yan/github/vikng-dev/overmatch/src/trees/view.rs:29) loads the segmented hierarchy for every tree. The data contains **DERIVED 635 trees**, each retaining **DERIVED 57 primitive entities**, including **DERIVED 30 hidden caps**.

Standing geometry uses **DERIVED 27 primitives/tree**. Merge the exact standing geometry into trunk and canopy meshes:

`27 × 635 = 17,145` → `2 × 635 = 1,270`

**DERIVED reduction: 15,875 visible mesh entities, or 92.6%.** Hidden caps and their hierarchy can also be deferred. Keep simulation segments unchanged; construct the detailed view for transitions and compact it when settled.

Corresponding segments already share mesh/material handles across trees. This is primarily an ECS/hierarchy/visibility reduction, not the removal of a draw per tree segment. Compatible standing geometry bins could fall from **DERIVED 27 to two**.

Bevy’s verified instancing/HLOD support above accommodates this.

**Existing product cost:** [trees/view.rs:63](/Users/Yan/github/vikng-dev/overmatch/src/trees/view.rs:63) abruptly removes trees beyond **DERIVED/configured 600 m**. That already changes concealment wherever a removed tree hides a tank. Preserve cover with a continuing representation; extending disappearance is not a free LOW optimization. Impostors add silhouette/parallax error.

### 9. Reuse track scratch and shadow meshes; stop identical transform writes

**Expected gain: ASSUMED medium CPU/render-preparation benefit; high confidence in waste.**

[track/view.rs:793](/Users/Yan/github/vikng-dev/overmatch/src/track/view.rs:793) replaces each dynamic caster mesh every frame. [shadow_proxy.rs:351](/Users/Yan/github/vikng-dev/overmatch/src/track/shadow_proxy.rs:351) allocates attributes; topology and winding are rebuilt too.

**DERIVED raw proxy payload:**

`2 × 97 × (4 vertices × 32 bytes + 24 indices × 4 bytes) = 43,456 bytes/tank/frame`.

At match scale: **DERIVED 1,303,680 bytes/frame**, approximately **DERIVED 149.2 MiB/s at target cadence**, before extraction and copies.

Wrap plus proxy generation creates a **DERIVED lower bound of 34 nonempty vector buffers/tank/frame**, or **DERIVED 1,020/frame** at match scale.

`src/track/view.rs:776,831` also unconditionally dirties **DERIVED 194 shoe + 16 wheel transforms/tank/frame**, or **DERIVED 6,300 writes/frame**.

**Cheaper method:** retained scratch capacity; immutable topology; cached hull-local ribbon shape; equality-protected transforms. Root movement alone need not rebuild hull-local geometry.

Bevy supports mutable mesh assets and change-aware writes. [v0.19.0 change-detection traits](https://raw.githubusercontent.com/bevyengine/bevy/v0.19.0/crates/bevy_ecs/src/change_detection/traits.rs) verify `set_if_neq`.

**Product cost:** none for identical-result caching. Coarser caster geometry introduces shadow-shape error. Savings overlap track fitting and entity processing above.

### 10. Put effect animation in shared instance data; retain ribbon buffers

**Expected gain: ASSUMED medium during sustained fire; high confidence in allocation/material churn.**

[vfx/billboard.rs:271](/Users/Yan/github/vikng-dev/overmatch/src/vfx/billboard.rs:271) creates a unique material for each sprite; `:369` updates it every frame. Shared quads therefore do not establish ordinary shared-material instancing.

[vfx/trail.rs:522](/Users/Yan/github/vikng-dev/overmatch/src/vfx/trail.rs:522) rebuilds every active ribbon. Each ordinary drawable rebuild creates **DERIVED fifteen vectors**. At the **DERIVED eight-trail cap**, that is **DERIVED 120 buffers/frame**.

**Cheaper method:** shared atlas/LUT material per family, instance age/seed/motion parameters, shader-facing quads; retained ribbon buffers followed by GPU expansion from station/birth-time data.

**DERIVED gain:** remove per-sprite material animation and most full ribbon reconstruction. Draw reduction depends on transparent ordering and compatible contiguous batches; milliseconds are unknown.

Bevy supports this infrastructure through its [v0.19.0 custom instancing example](https://raw.githubusercontent.com/bevyengine/bevy/v0.19.0/examples/shader_advanced/custom_shader_instancing.rs). It requires implementation, not a setting.

**Product cost:** preserve smoke opacity, path breaks, effect timing and ordering.

### 11. Render smoke/dust at reduced resolution with depth-aware reconstruction

**Expected gain: ASSUMED potentially large during heavy effects; lower confidence in visual equivalence.**

Sources: `assets/shaders/vfx_billboard.wgsl:64`, `src/vfx/billboard.rs:109`, `src/vfx/trail.rs:130`.

**Cheaper method:** a reduced-resolution transparent target, retaining population, density and integrated opacity.

**DERIVED:** half width and height produce `0.5² = 25%` of the raster pixels: **DERIVED 75% less particle pixel work**, before depth handling and composition.

Bevy supports render targets and custom passes, verified by its [v0.19.0 render-to-texture example](https://raw.githubusercontent.com/bevyengine/bevy/v0.19.0/examples/3d/render_to_texture.rs). A suitable particle compositor is custom work.

**Product cost:** reconstruction can change tank silhouettes through smoke or leak across depth edges. It needs concealment acceptance, not just screenshot similarity.

### 12. Reduce the game damage-model inset’s render resolution

**Expected gain: ASSUMED substantial transient benefit; absent while inactive.**

[pen_cam/stage.rs:317](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/pen_cam/stage.rs:317) creates an HDR/bloom camera explicitly exempt from world render scaling. `src/damage_cam/pen_cam/hud.rs:265` allocates native-sized targets; `src/damage_cam/pen_cam/look.rs:542` adds hull/part mask views.

**Cheaper method:** reduce inset and associated mask resolution together; retain native UI labels. Cache completed presentations when no animation or camera change remains.

**DERIVED half-size target saving:** **75% of resolution-dependent pixel work**, plus reconstruction cost. The previously recorded **MEASURED approximately 4.3 ms inset delta** includes non-pixel costs, so multiplying the whole delta by that percentage would overstate the forecast.

Bevy render targets support this directly; mask reconstruction needs custom validation.

**Product cost:** feedback detail and mask-edge accuracy. Cached record-field pictures already exist; they are not all regenerated every frame.

### 13. Stop rebuilding unchanged HUD text and status summaries

**Expected gain: ASSUMED small-to-medium CPU benefit; high confidence.**

[hud.rs:91](/Users/Yan/github/vikng-dev/overmatch/src/hud.rs:91) allocates and replaces names every frame. `src/drive_hud.rs:222`, `src/crew_ui.rs:202,278`, and `src/sight/reticle.rs:266,812` reconstruct displayed strings or summaries.

**Cheaper method:** retain per-frame projection, cache names/order/ownership, refresh discrete state on change, and update numeric text only when its displayed value changes.

For numeric refresh at **ASSUMED 10 Hz**, **DERIVED `1 − 10/120 = 91.7%` fewer formatting opportunities** for that portion.

Bevy’s [v0.19.0 text measurement source](https://raw.githubusercontent.com/bevyengine/bevy/v0.19.0/crates/bevy_ui/src/widget/text.rs) confirms changed text triggers measurement/layout work. Change-aware assignment and string-capacity reuse are supported.

**Product cost:** bounded numeric-display latency if throttled. Do not slow reticle or nameplate positioning.

### 14. Lower bloom’s existing mip budget only after larger work is resolved

**Expected gain: ASSUMED small-to-medium GPU benefit; high confidence in supported mechanism.**

`src/camera.rs:325` uses `Bloom::NATURAL`.

**Correction to earlier prose:** bloom already uses a reduced-resolution pyramid. The verified natural preset has **DERIVED `max_mip_dimension = 512`**; it is not a full-resolution blur chain.

**Cheaper method:** **ASSUMED 256** maximum mip dimension. **DERIVED approximately 75% less pyramid pixel work**; full-resolution final composition remains.

Supported by [Bevy v0.19.0 bloom settings](https://raw.githubusercontent.com/bevyengine/bevy/v0.19.0/crates/bevy_post_process/src/bloom/settings.rs).

**Product cost:** halo quality. Removing bloom entirely changes the intended visibility of emissive effects.

## Full inventory

**Interpretation:** PBR geometry performs ordinary vertex world/clip transforms and normal/tangent handling, then fragment lighting with environment and applicable shadow sampling. Opaque depth/shadow paths generally avoid material fragment lighting. Geometry counts below do not multiply by views or cascades unless stated.

| Object or system | Counts and batching | LOD / shadows | Shader work or recurring computation |
|---|---|---|---|
| Shipped map | **DERIVED 635 trees = 201 + 225 + 209 aliases**, **74 houses**, **zero church instances**. All tree aliases resolve to the same asset. `assets/maps/kalinovo/prototypes.ron:16`; `level.json` instance records. | Whole-map inventory, before culling. | Placement occurs at startup. No other placed prop class in this manifest. |
| Tank asset and retained view | GLB metadata: **DERIVED 86 nodes; 89 source primitive entities; 58 source mesh records + 167 unique embedded rung records; 12 materials, nine textures**. Hidden templates/proxies leave **DERIVED 85 source primitives, 37,615 triangles, 10 materials**. Handles shared across tank instances. `src/tank/view.rs:56`. | **DERIVED 61 certificate chains**, producing **270 retained rung entities/tank** through repeated instances. Default body cast/receive. | No asset skins, morphs or authored animation. Standard PBR; some materials flat, others albedo/metal-roughness/normal mapped. Enclosed component views remain present behind opaque hull geometry. |
| Hull | Decoration source **DERIVED 13,297 triangles**. Its large textured primitive ladder is **DERIVED 13,171 → 7,004 → 4,654**. Other hull plates are separate primitives. | Existing per-primitive ranges; default casters. | Merge compatible immutable pieces; retain moving and material seams. Separate silhouette caster can reduce decorative shadow geometry. |
| Turret and barrel | Decoration **DERIVED 2,784 triangles**, with rung counts **1,330 → 748 → 445**. Main barrel source **DERIVED 1,304 triangles**. | Existing certificate ladders; default casters. | PBR; separate transforms preserve articulation. |
| Road wheels | **DERIVED 16 nodes × two primitives**, **660 triangles/wheel**, **10,560 total**. Corresponding steel/rubber meshes shared across nodes/tanks. `assets/tiger_1/tiger_1.tank.ron:311`. | Steel counts **DERIVED 404 → 244 → 96 → 56 → 20**; rubber **256 → 146 → 92 → 56**. Cast/receive. | Flat steel and textured rubber PBR. View transforms rewritten every frame. |
| Sprockets / idlers | **DERIVED two × 1,256** sprocket triangles; **two × 1,150** idler triangles. Shared across tank instances. | Existing ladders; default casters. | Ordinary PBR. Spin transforms already use equality-protected assignment: `src/track/view.rs:859,863`. |
| Track shoes | **DERIVED 194 entities/tank**, shared material, mirrored mesh per side/rung. Triangle ladder **DERIVED 1,520 → 656 → 400 → 252 → 166 per shoe**. `src/track/link_view.rs:968,1035`. | Whole belt selects one conservative rung; one compatible geometry bin per side/rung. Receives shadows; casting stops after proxy readiness. | Normal-mapped PBR. Meshes pooled, not regenerated. Per-frame placement and transform extraction remain. |
| Track camera geometry | Full source inventory **DERIVED `37,615 + 194×1,520 = 332,495 triangles/tank`**. Actual count depends on rung and culling. | Retained mesh entities **DERIVED 555/tank**. | LOD reduces triangles while retaining much entity structure. |
| Track shadow ribbons | **DERIVED two unique meshes/tank**, shared proxy material; **388 vertices / 776 triangles per side**. `src/track/shadow_proxy.rs:335`. | Dynamic default. Cast-only channel; camera excludes them. | Opaque depth-only caster. Entire mesh rebuilt each frame despite invariant topology. |
| Tree asset | **DERIVED 58 node records, 57 mesh primitives, three untextured materials/tree**: **15 trunk, 12 canopy, 30 caps**. Same handles shared across instances. `src/trees/view.rs:29`; `scripts/prop/gen_tree.py:68`. | No geometry chains: `assets/trees/fir_1/fir_1.lod.json:5`. Abrupt **DERIVED 600 m** range. Default cast/receive. | Opaque PBR. Canopy uses double-sided geometry cards, not textured alpha billboards. |
| Standing forest | **DERIVED 27 visible primitives, 1,224 triangles/tree**; whole forest **17,145 standing primitives / 777,240 triangles**. Retained primitives including caps: **DERIVED 36,195**. | Hidden caps retain entities. Corresponding segments can instance across trees. | Merge identical standing geometry for lower hierarchy/bin count. Triangle reduction alone may be less valuable. |
| Tree simulation/presentation | **DERIVED 15 segment collider entities/tree**, separate from view hierarchy. `src/trees.rs:871`. | Simulation state derivation already change-gated. | `src/trees/view.rs:206` visits roots every frame but skips unchanged settled writes at `:245`. An active-fall set avoids residual scans; **ASSUMED small gain**. |
| Buildings | **DERIVED 74 instances**, one shared cube and wall material; **DERIVED 888 triangles**. `src/scatter.rs:214,283`. | No explicit geometry/distance ladder; cast/receive. | Already simple flat PBR and instance-compatible. Merging can worsen culling granularity; **ASSUMED little benefit**. |
| Terrain meshes | Existing startup census **MEASURED 64 tiles / 309 retained meshes**. One shared terrain material, unique tile/rung meshes. `src/terrain_lod.rs:883`; existing `runs/track-prints/mud-parallax.log:17`. | At most **DERIVED 64 selected tile meshes/view** before frustum culling. Terrain casts shadows. | Generated at startup, not rebuilt every frame. Unique tiles do not ordinary-instance. |
| Terrain triangles | Existing rung totals **MEASURED 2,097,152 / 407,416 / 250,710 / 167,298 / 128,988**; resident sum **DERIVED 3,051,564**. | Fixed error budget **DERIVED one pixel**. Declared error vocabulary **DERIVED `[0,.02,.05,.10,.25,.50] m`**. `src/terrain_lod.rs:142,151`. | Resident totals are not drawn totals. **DERIVED upper bound 64 × (depth + main + three cascades) = 320** tile/pass appearances before exclusions. |
| Terrain material | **DERIVED one mask + one world-print + four base requests + three per sampled overlay**, excluding lighting/near relief. `assets/shaders/terrain_blend.wgsl:301,375,416`. | Weak layers already skipped. Terrain textures use **DERIVED eight-way anisotropy**: `src/world.rs:278`. | Tangent normals, AO-conditioned layer blend, eight sine hashes, PBR/environment/shadow lighting. Texture-request counts are not physical anisotropic tap counts. |
| Print relief | No separate geometry/draw. World/near stores sampled by terrain material. | Distance and projected-footprint fade already present. | Lighting gradient, optional parallax march, optional sun march and direct-light reconstruction; ranked above. |
| Print stamping/upload | Belt-child scan at `src/track_prints.rs:442`; dirty-tile flush/upload at `:498` and `src/track_prints/upload.rs:43`. | Cosmetic view layer. | Already uploads dirty tiles rather than full textures. Material sync is change-aware. Cache unchanged contacts; do not claim full-store upload every frame. |
| Sky | No ordinary-world `Skybox`/`Atmosphere` or sky mesh declaration found. Source light cubemap **DERIVED `128²×6×4 = 393,216 bytes`**. `src/world.rs:101,166`. | No caster or distance LOD. | Analytic source built once; filtered lighting regenerated each render frame. |
| Main camera | **DERIVED one battlefield camera**, HDR, bloom, depth prepass. `src/camera.rs:320`. | Default **DERIVED four-sample MSAA**. No ordinary-camera normal/motion/deferred prepass or SSAO declaration found. | Opaque depth then main shading and transparent effects. Terrain’s expensive fragment shading is not repeated wholesale in its opaque depth-only prepass. |
| Render scale | Native path omits custom upscale node; scaled path uses `MainPassResolutionOverride`. `src/render_scale.rs:355`. | Existing thresholds follow rendered height. | **DERIVED 75% scale → 56.25% main-pass pixels**, or **43.75% less pixel work** in affected passes. Full-size allocations, resolve and final post remain. Does not scale shadow maps or environment convolution. |
| Bloom / tonemapping | Existing reduced-resolution bloom pyramid; natural maximum dimension **DERIVED 512**. Final post spans output. | No world geometry or shadows. | Full-resolution final composition/tonemapping remain after upscale. HDR is needed for intended emissive highlights. |
| Directional shadows | **DERIVED three configured cascades**, **4096-square maps**, configured distance **350 m**. `src/settings.rs:99,438,2129`. | Ordinary world/body geometry casts; ready shoes use ribbons. | Per-cascade visibility, queue/submission and vertex/depth work; main PBR samples shadow maps. `src/render_policy.rs:195,228` owns shadow roles/channels. |
| Billboard effects | **DERIVED ring caps 96 + 16 ground marks**, up to **112 materials** in these rings. Shared quads, unique material per sprite. `src/vfx/billboard.rs:39,271`; `impact.rs:138`. | Transparent/additive, no shadow or depth-prepass writes. | Atlas + LUT reads; soft particles read depth. CPU age, facing and material updates every frame. |
| Smoke ribbons | **DERIVED cap eight trails**, **128 stored points/trail**; unique meshes, shared material. `src/vfx/trail.rs:32,40,102`. | No shadows/prepass; no geometry ladder. | CPU rebuild/upload; fragment **DERIVED two noise samples + one LUT sample**. |
| Muzzle lights | **DERIVED cap 12**. `src/vfx/muzzle.rs:108,481`. | Shipped policy is `MainGunOnly`: machine-gun shadows already disabled. | Main-gun flashes can add point-light cubemap shadows: **DERIVED six views/shadowed light**. Disabling them risks light leakage; proxying casters preserves more information. Transient cost unknown. |
| Game damage-model hull marks | **DERIVED nine shared material variants**, two entities/held mark. `src/vfx/hull_marks.rs:149,271`. | Forward decals; existing record-change fast path at `:219`. | Parallax uses **DERIVED maximum 12 layers**. No unconditional game-side geometry reconstruction found. |
| Offline shared HUD | **DERIVED 50 entities / 21 Text roots** from spawn paths, excluding menus, inset and dev egui. | No world shadows or geometric LOD. Many elements hidden by mode. | Nameplates **four**, drive texts **two**, game damage-model status/crew texts **two**, aim elements **four**, sight elements **38**. |
| Online HUD additions | Debug card **DERIVED 13 entities / eight text roots**; game damage-model hit cues **three entities / one text root**. Combined listed HUD **DERIVED 66 entities / 30 Text roots**. | Debug card already throttles expensive refresh. | UI batches depend on atlas/material/order, not one draw/entity. Reticle/projection work belongs at render cadence; constant strings do not. |
| Optic mask/graticule | **DERIVED one custom optic material**; graticule **31 entities**. `src/sight/reticle.rs:308,530`. | Screen-space only. | Fullscreen mask fragment work plus UI ticks/text; per-frame projection and material update. |
| Game damage-model inset | Inactive camera until shown; HDR/bloom target plus optional hull/part mask cameras. `src/damage_cam/pen_cam/stage.rs:317`; `look.rs:542`. | Explicit native-resolution exemption. Masks use MSAA off and tonemapping off. | Extra views and custom glass/field shading while active. Record-field pictures already cached by record/detail; not all work recurs every frame. |
| LOD/settings policy systems | `src/geometry_lod.rs:157`; `terrain_lod.rs:1044`; `settings.rs:1960`; `render_scale.rs:415`. | Existing change/equality guards. | No basis for claiming full LOD/settings reconstruction every frame. Render policy resolves dirty subtrees, not all meshes unconditionally. |
| Servo ownership | `src/aim.rs:434,474`: tank loop scans every servo. | Fixed authority schedule, not frame-rate presentation. | **DERIVED O(T×S)**, quadratic when servos scale with tanks. Owner lists/inverted iteration yield **DERIVED O(S)**; **ASSUMED low priority** pending attribution. |
| Other apparently nested loops | Belt selection, shoe placement and stamping iterate each belt’s own children. `src/track/link_view.rs:1035,1122`; `track_prints.rs:442`. | — | Linear in total shoes; not quadratic tank × all-shoe scans. Barrel copies already protect identical writes: `src/tank/view.rs:114`. |

### Metal versus Vulkan/DX12

On this Metal backend, wgpu implements indirect draw groups with a CPU loop issuing individual draws: `wgpu-hal-29.0.4/src/metal/command.rs:1555,1574`. Therefore remaining compatible mesh bins and pass appearances still carry CPU encoding cost.

Vulkan can issue a hardware multi-draw command when supported: `src/vulkan/command.rs:1165`. DX12 uses `ExecuteIndirect`: `src/dx12/command.rs:1415`. Compatible distinct-mesh submission can therefore be cheaper there.

**Instancing still helps Metal.** Shared mesh/material instances can collapse into one instanced draw. The forest’s primitive-entity inventory and the tank’s shoe count must not be reported as equivalent draw counts. Entity traversal, extraction, vertex work and shading remain costs on every backend.

## What only measurement can settle

- **Environment caching:** actual GPU/CPU saving, including its share of the apparent no-draw floor.
- **Track attribution:** shared caster time split between authority simulation and presentation; settled-cache hit rate versus moving tanks.
- **GPU priority:** terrain material, relief, MSAA, shadow geometry and post costs with geometry/visibility held constant.
- **Actual batching:** instanced bins, encoded draws and visible rungs per camera/cascade; compare Metal with Vulkan/DX12.
- **Active battle cost:** smoke overdraw, ribbon uploads, simultaneous main-gun shadow views and inset transients. Idle probes cannot price these.
- **Concealment equivalence:** continuous belts, tree impostors, reduced-resolution smoke, caster proxies and the existing tree cutoff.
- **Target attainment:** the requested online moving/firing scene on the reference PC. The M4’s stage balance cannot establish its final frame time.