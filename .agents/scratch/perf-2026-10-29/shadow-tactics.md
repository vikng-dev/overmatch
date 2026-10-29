Prioritize **fewer cascades, merged tank casters, and cheaper filtering**. The strongest larger redesign is **fixed-sun terrain occlusion shared by every receiver material**. Static-world caching is promising, but requires renderer work; a stationary sun alone does not make Bevy’s camera-fitted cascades cacheable.

The ranking below is **ASSUMED ordinal gain × confidence**, not a measured score. Configuration values read from source are labelled **DERIVED**. Versions, source locations and ranking identifiers are identifiers rather than measurements.

| Rank | Tactic | Expected gain | Confidence |
|---|---|---|---|
| A | Fewer cascades, with retuned splits | Largest established lever: **MEASURED historical ~1.16 ms per removed cascade** | High |
| B | Merge tank casters into shared meshes per rigid assembly and LOD | **ASSUMED meaningful sub-millisecond saving** in the crowded scene | Medium–high |
| C | Cheaper main-pass shadow filtering | **DERIVED 88.9% fewer comparison lookups** with hardware filtering; frame saving unknown | High for work reduction; medium for frame gain |
| D | Replace terrain raster casting with a fixed-sun occlusion field | **ASSUMED potentially substantial**, attacking work tank proxies cannot remove | Medium |
| E | Tree caster proxies, including an exact-merge alternative | **ASSUMED substantial in forests**, smaller elsewhere | Medium |
| F | Exclude enclosed and shadow-irrelevant tank parts | **ASSUMED small–moderate**, inexpensive to investigate | High for mechanism |
| G | Cache static-world shadow depth | **ASSUMED potentially substantial gross saving**, uncertain net saving | Low–medium |
| H | Coarser meshes in farther cascades | **ASSUMED modest initially**; saves geometry work while retaining passes | Medium |
| I | Update far cascades less frequently | **ASSUMED modest average saving**, with pacing and gameplay risks | Low–medium |
| J | Reduce map resolution | Small established lever: **MEASURED historical 0.27 ms across the entire resolution ladder** | High |

These savings overlap. They must not be added together.

**Evidence that constrains the ranking**

The current sweep establishes **MEASURED shadows-off savings of 2.26 ms** in the duel and **DERIVED `19.16 − 16.36 = 2.80 ms`** in the crowded scene. Those price the entire feature: caster visibility, submission, rasterization and receiver sampling. They do not identify which component costs the time. [Sweep](.agents/scratch/perf-2026-10-29/sweep-30-2026-10-29.md)

The older caster experiment establishes a **MEASURED historical split of 1.14 ms tank casters and 2.08 ms envelope plus terrain**, totalling **MEASURED 3.22 ms**. Its tank result already includes the existing track proxies. The remainder is neither a terrain-only measurement nor a fully recoverable static-cache budget. [Caster-share experiment:71](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/shadow-caster-share-2026-08-13.md:71)

Shadow commands account for **DERIVED `288/430 = 67.0%`** of duel draws and **DERIVED `621/901 = 68.9%`** of crowded-scene draws. However, shared meshes already instance, and the measured camera-driver encode time did not increase with the draw count. Draw reduction is useful evidence of less work, not a linear frame-time predictor. [Frame attribution:149](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/perf-2026-10-29/frame-attribution.md:149)

The duel is GPU-bound; the crowded scene is main-world-bound, according to that attribution. Consequently, caster collection savings can matter in the crowded scene even when GPU raster savings do not shorten its critical path.

**A. Fewer cascades, with split and overlap tuning**

**Mechanism and support.** The game explicitly configures cascade count, maximum distance, first bound and overlap at [settings.rs:2129](/Users/Yan/github/vikng-dev/overmatch/src/settings.rs:2129). The shipped values are **DERIVED three cascades**, a **DERIVED 350 m** envelope, a **DERIVED 40 m** first bound and **DERIVED 0.2** overlap.

Bevy supports all these controls. Its builder spaces intermediate bounds geometrically; it has no lambda field. Arbitrary practical-split/lambda bounds can instead be written into public `CascadeShadowConfig.bounds`. [Versioned cascade source](https://github.com/bevyengine/bevy/blob/v0.19.0/crates/bevy_light/src/cascade.rs#L25-L52)

The current bounds are **DERIVED approximately `[40, √(40×350), 350] = [40, 118.3, 350] m`**. Dropping a cascade without retuning leaves **DERIVED `[40, 350] m`**, substantially worsening the far cascade’s near-edge density. Split tuning should accompany the count reduction.

**Expected gain.** The historical **MEASURED ~1.16 ms** marginal cascade cost is the best timing evidence available. Applying it to the current scene is **ASSUMED**, pending `cascades-2`. Split tuning itself mainly redistributes quality and caster coverage; there is no defensible millisecond forecast.

Smaller overlap reduces duplicated coverage and the pixels that sample adjacent cascades. Bevy explicitly performs the second lookup in overlap regions. [shadows.wgsl:246](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/render/shadows.wgsl:246)

**Product cost.** Retaining the envelope preserves shadow existence, but coarser or unstable edges can still change a tank read. Reduced overlap makes cascade seams more apparent. Preserve barrel, hull-down and treeline-shadow readability during camera motion and optic changes.

**Pricing lever.** Existing `cascades-2`; new independent `shadow-splits` and `shadow-overlap` controls. An overlap-only lever must also repair [settings.rs:2137](/Users/Yan/github/vikng-dev/overmatch/src/settings.rs:2137): the current reconciliation replaces the config only when its bounds differ.

The lockfile uses Bevy **0.19.1**. Runtime cascade growth needs that patch release’s visibility-queue fix; stock **0.19.0** is unsafe for that operation. [settings.rs:112](/Users/Yan/github/vikng-dev/overmatch/src/settings.rs:112)

**B. Merged tank casters**

**Mechanism and support.** Add shared shadow-only meshes, partitioned by vehicle kind, rigid assembly and shadow LOD. Silence the corresponding detailed casters through `VisualScope`. The existing policy already supplies cast-only and receive-only roles. [render_policy.rs:225](/Users/Yan/github/vikng-dev/overmatch/src/render_policy.rs:225)

A literal single rigid mesh for the whole tank is insufficient: turret articulation and barrel recoil have independent view transforms. [tank/view.rs:93](/Users/Yan/github/vikng-dev/overmatch/src/tank/view.rs:93), [tank/view.rs:107](/Users/Yan/github/vikng-dev/overmatch/src/tank/view.rs:107)

Start with merged rigid assemblies. A genuinely single articulated caster would require a skinned or custom vertex-transform path. Avoid rebuilding a unique merged mesh per tank every frame: that adds CPU/upload work and can sacrifice shared-mesh instancing.

Bevy supports ordinary shadow-only proxy entities in the vendored renderer. The existing ribbons demonstrate the arrangement. Their material is opaque, allowing the inexpensive depth-only shadow path. [shadow_proxy.rs:290](/Users/Yan/github/vikng-dev/overmatch/src/track/shadow_proxy.rs:290)

**Expected gain.** **ASSUMED meaningful sub-millisecond crowded-scene gain.** As a conditional planning calculation, recovering an **ASSUMED 50–80%** of the historical caster cost yields **DERIVED `1.14×0.5…0.8 = 0.57…0.91 ms`**. That is a scenario, not a current forecast.

The historical **MEASURED 1.14 ms** is the relevant ceiling for that experiment, minus proxy cost. Hundreds of entities do not imply hundreds of independent draws: corresponding meshes already batch across tanks.

**Product cost.** Approximation can erase narrow barrel shadows, fill gaps or move contact edges. Preserve articulation and separated assemblies. Build declarations generically from asset data. Keep the existing terrain-conforming track ribbons rather than counting their saving again.

**Pricing lever.** New `tank-casters=parts|merged-exact|proxy`, alongside a tank-casters-off attribution arm. Compare exact merging before simplifying the silhouette.

**C. Cheaper receiver filtering**

**Mechanism and support.** Bevy supports `ShadowFilteringMethod` on the camera. Gaussian is the default; the renderer selects it when no override exists. [Versioned enum/default](https://github.com/bevyengine/bevy/blob/v0.19.0/crates/bevy_light/src/lib.rs#L273-L299), [mesh.rs:465](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/render/mesh.rs:465)

For the ordinary directional PCF path:

| Method | Source-counted comparison lookups per cascade sample |
|---|---|
| `Hardware2x2` | **DERIVED one**, with a hardware **CLAIMED 2×2** footprint |
| `Gaussian` | **DERIVED nine**, approximating a **CLAIMED 5×5** kernel |
| `Temporal` | **DERIVED eight**, with rotating offsets |

The calls are visible at [shadow_sampling.wgsl:10](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/render/shadow_sampling.wgsl:10), [134](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/render/shadow_sampling.wgsl:134) and [200](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/render/shadow_sampling.wgsl:200).

**Expected gain.** Hardware filtering removes **DERIVED `(9−1)/9 = 88.9%`** of those lookup instructions. Temporal removes **DERIVED `(9−8)/9 = 11.1%`**. Neither percentage applies to total shadow time. Caster draws and cascade passes remain.

**ASSUMED small–moderate frame gain**, most promising in the GPU-bound duel. Milliseconds require an isolated measurement.

**Product cost.** Hardware filtering gives sharper, more jagged and potentially shimmering boundaries. Temporal filtering needs suitable temporal AA to avoid noisy moving edges. Both can change silhouette recognition inside the gameplay envelope.

**Pricing lever.** New `shadow-filter=gaussian|hardware|temporal`, keeping maps, casters and splits identical. A cheaper filter only in farther cascades is possible with custom shader logic; the stock component chooses per camera.

**D. Fixed-sun terrain occlusion**

**Mechanism and support.** The sun direction is fixed: elevation and azimuth are constants, converted into the spawn transform; the inspected source has no animation writer. The angles are **DERIVED/configured 17° elevation and 45° azimuth**. [world.rs:38](/Users/Yan/github/vikng-dev/overmatch/src/world.rs:38), [world.rs:79](/Users/Yan/github/vikng-dev/overmatch/src/world.rs:79), [world.rs:391](/Users/Yan/github/vikng-dev/overmatch/src/world.rs:391)

That permits a map-wide bake—or generation when the heightfield loads—of the highest terrain occluder toward the sun. Query it using receiver world position, then remove terrain from cascade caster lists.

The existing terrain material is extensible. [terrain_blend.rs:245](/Users/Yan/github/vikng-dev/overmatch/src/terrain_blend.rs:245) A later upstream change adds a custom directional-shadow factor specifically for heightfield shadows; it is absent from this vendor and would need a backport or equivalent shared shader change. [Upstream custom-shadow change](https://github.com/bevyengine/bevy/pull/24936)

**Critical scope:** a ground-only mask sampled in `terrain_blend.wgsl` replaces ground self-shadowing only. Removing terrain casters would then remove ridge shadows from tanks, trees and houses. The field must be queried by those materials too, at each fragment’s height.

Apply it to direct sunlight, while retaining dynamic tank-on-ground shadows from cascades. Do not darken albedo or ambient lighting. The current terrain shader already distinguishes direct sunlight for rut shading. [terrain_blend.wgsl:248](/Users/Yan/github/vikng-dev/overmatch/assets/shaders/terrain_blend.wgsl:248), [489](/Users/Yan/github/vikng-dev/overmatch/assets/shaders/terrain_blend.wgsl:489)

**Expected gain.** Let `T` be terrain caster cost and `B` the added field-sampling cost: **DERIVED net saving `T−B`**. `T` has not been isolated. The historical **MEASURED 2.08 ms** remainder includes other envelope costs, so it cannot be assigned wholesale to terrain.

**ASSUMED potentially substantial gain**, with medium confidence. The upstream author reports an unquantified improvement; that is **CLAIMED**, not a transferable timing. The upstream hook still samples cascades before multiplying the custom factor, so adding it alone does not remove cascade work. [Composition source](https://github.com/bevyengine/bevy/blob/75ea1ba/crates/bevy_pbr/src/render/pbr_functions.wgsl#L569-L581)

**Product cost.** Field resolution, bias and interpolation can expose or conceal a hull-down tank. Elevated receivers, terrain boundaries and slopes nearly parallel to sunlight need particular attention. Generate from the canonical heightfield rather than a camera-selected terrain LOD.

**Pricing lever.** New `terrain-casters=on|off` for attribution, followed by `terrain-shadow=raster|field`. The off arm is an instrument, not a product setting.

**E. Tree caster proxies**

**Mechanism and support.** The tree view loads a segmented hierarchy, hides unused caps and applies one camera-distance visibility range to its mesh leaves. It does not currently establish a separate shadow LOD. [trees/view.rs:69](/Users/Yan/github/vikng-dev/overmatch/src/trees/view.rs:69)

A trunk cylinder plus canopy cone can cast through `SHADOW_PROXY`, while the detailed tree receives through the existing policy. Bevy supports that arrangement.

Test an **exact merged caster** as another arm. The audit records **CLAIMED 27 standing primitives per tree**; merging into **ASSUMED two caster meshes** gives **DERIVED `(27−2)/27 = 92.6%`** fewer caster mesh identities before batching. That is not an equivalent draw or timing reduction. [Render audit:163](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/perf-2026-10-29/render-audit.md:163)

**Expected gain.** **ASSUMED substantial in forest views**, unpriced in milliseconds. Exact merging reduces mesh/bin and visibility overhead while retaining triangles. A cone additionally reduces geometry and alpha-tested foliage work.

**Product cost.** An opaque canopy cone fills the real canopy’s holes, potentially making tree shade denser and hiding a tank. Preserve trunk shadows. Proxies must follow the existing fall presentation and settled pose rather than leaving standing shadows behind. [trees/view.rs:177](/Users/Yan/github/vikng-dev/overmatch/src/trees/view.rs:177)

**Pricing lever.** New `tree-casters=detail|merged-exact|proxy|off`. Tonight’s forest cones-versus-trees comparison changes the visible representation too; it cannot isolate caster savings.

**F. Exclude small and enclosed parts**

**Mechanism and support.** The current tank binding deliberately keeps enclosed component views because the hull occludes them. They nevertheless inherit ordinary casting. [tank/view.rs:56](/Users/Yan/github/vikng-dev/overmatch/src/tank/view.rs:56)

Bevy’s `NotShadowCaster` excludes an entity from shadow collection. Here, express that decision through `VisualScope`, whose resolver owns the marker. Directly inserting it elsewhere risks a later policy resolution reversing the choice. [render_policy.rs:437](/Users/Yan/github/vikng-dev/overmatch/src/render_policy.rs:437)

**Expected gain.** **ASSUMED small–moderate**, bounded by the tank-caster budget and overlapping merged proxies. It saves little draw work when the remaining instances still require the same bin, though it can reduce per-instance geometry and collection.

**Product cost.** “Small” is not equivalent to irrelevant: barrel shadows are gameplay information. Enclosed geometry can become exposed when assemblies separate. Use declarative caster roles and state-aware presentation, with no vehicle-specific branches.

**Pricing lever.** New `tank-caster-detail=all|exterior|essential`, plus caster counts by declared role.

**G. Cache static-world shadow depth**

**Mechanism and support.** There is no stock static-shadow cache policy in the inspected renderer. Its texture cache reuses allocations; it does not preserve valid scene shadow contents. Directional attachments are configured to clear, and the renderer visits the cascade shadow views each frame. [light.rs:1884](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/render/light.rs:1884), [light.rs:2843](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/render/light.rs:2843)

Fixed sunlight helps, but Bevy fits cascades from the camera transform and projection. Camera movement changes their projection and covered region. [Versioned fitting source](https://github.com/bevyengine/bevy/blob/v0.19.0/crates/bevy_light/src/cascade.rs#L185-L240)

Two viable designs are:

- Persistent static depth matching each cascade projection, restored before drawing dynamic casters. Camera movement requires refreshes or scrolling exposed regions.
- World-anchored light-space depth tiles, sampled alongside dynamic cascades. This avoids camera-dependent static projections but changes bindings and receiver shaders.

Both require static/dynamic classification, persistent storage, invalidation and correct depth composition. Falling trees must become dynamic; completed pose changes must invalidate cached coverage. For equivalence, combine occluder depths before filtering rather than blindly multiplying independently filtered factors.

**Expected gain.** **ASSUMED potentially substantial gross saving**, reduced by copies, tile maintenance, extra sampling and retained dynamic passes. The historical **MEASURED 2.08 ms** remainder is only a loose ceiling for that experiment.

**Product cost.** Stale shadows, atlas seams and refresh stalls can all change a tank read.

**Pricing lever.** New `static-shadow-cache`, with separate caster, maintenance and composition counters.

A scoped upstream/plugin search found **no verified compatible cached-static-CSM plugin**. Bevy’s graphics tracker lists caching as an aspiration; it is not delivery evidence. The custom heightfield-shadow hook above is useful upstream work, but does not implement caching. [Graphics tracker](https://github.com/bevyengine/bevy/discussions/4630)

**H. Coarser meshes in farther cascades**

**Mechanism and support.** Ordinary `VisibilityRange` is already respected by sun shadows, but selection uses the associated camera before iterating cascade frusta. Therefore every cascade uses the camera-selected LOD; it cannot choose its own through that component. [Versioned visibility source](https://github.com/bevyengine/bevy/blob/v0.19.0/crates/bevy_light/src/lib.rs#L409-L430)

The renderer does retain separate cascade identities and visible-entity tables, providing a place for a custom selector. [light.rs:1896](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/render/light.rs:1896), [2921](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/render/light.rs:2921)

**Expected gain.** **ASSUMED modest initially.** If far-cascade geometry work is `V` and the replacement retains fraction `r`, the geometry-only saving is **DERIVED `(1−r)V`**. Pass count, receiver sampling and many draws remain.

**Product cost.** Low sunlight amplifies vertical caster error: **DERIVED `cot(17°) ≈ 3.27`** times that error on a flat receiver, potentially more on grazing slopes. The terrain module already records this constraint. [terrain_lod.rs:62](/Users/Yan/github/vikng-dev/overmatch/src/terrain_lod.rs:62)

**Pricing lever.** New `shadow-lod-offset` per cascade. Start with proxy LOD selected by camera distance, which requires less renderer surgery.

**I. Half-rate far cascades**

**Mechanism and support.** No stock update-frequency control was found. Skipping a render requires retaining its depth texture **and matching matrices**, while ensuring camera movement has not exposed uncovered regions.

**Expected gain.** For far-cascade render cost `F`, alternating updates save approximately **DERIVED `F/2`**, minus maintenance. Treating the historical **MEASURED 1.16 ms** marginal cascade price as an **ASSUMED comparable far-pass cost** gives **DERIVED 0.58 ms average saving**. This is illustrative, not a forecast.

**Product cost.** Moving tanks and barrels leave delayed shadows inside the gameplay envelope. Camera turns can expose stale coverage. Alternating expensive and cheap frames can improve an aggregate statistic while worsening pacing. Updating dynamic casters every frame while caching static far casters is more faithful, but requires tactic G.

**Pricing lever.** New `far-shadow-period`, with per-frame update flags, frame intervals and camera-coverage refreshes.

**J. Lower resolution**

**Mechanism and support.** This is already supported through `DirectionalLightShadowMap`. [settings.rs:2099](/Users/Yan/github/vikng-dev/overmatch/src/settings.rs:2099)

**Expected gain.** The historical whole-ladder saving is **MEASURED 0.27 ms**. Changing **DERIVED/configured 4096-square maps to 2048-square maps** removes **DERIVED 75%** of nominal texels per map, not of shadow time.

**Product cost.** Larger texels increase bias-driven detachment. The settings documentation gives **DERIVED historical estimates of approximately 0.53 m versus 1.07 m** at the configured envelope. Those positions matter when reading a tank’s contact with terrain. [settings.rs:278](/Users/Yan/github/vikng-dev/overmatch/src/settings.rs:278)

**Pricing lever.** Existing `res-2048`. Price it independently before bundling it with fewer cascades.

**Build plan**

Each row is **ASSUMED one agent session**, with a concrete reviewable result. These are future implementation slices; this lane performed none.

| Session | Deliverable |
|---|---|
| Attribution | Same-binary caster-class exclusions and per-cascade counts, retaining `SPIKE_PHASE_COUNTS`; distinguish terrain, trees, houses and tanks. |
| Stock controls | Independent filter, split and overlap levers; fix overlap-only reconciliation. Use tonight’s existing count/resolution results when available. |
| Caster roles | Declarative receive-only roles for enclosed and nonessential parts, with exposed-state handling. |
| Tank merge | Shared exact caster meshes per rigid assembly and LOD, with atomic readiness and detailed-caster fallback. |
| Tree comparison | Exact merged and cylinder/cone caster arms, following existing tree poses. |
| Terrain field | Canonical-heightfield occlusion-field generator, fingerprinting and a debug visualization. |
| Receiver integration | Shared direct-sun field sampling across terrain and ordinary receivers; switch terrain raster casting independently. |
| Cascade LOD | Bounded custom per-cascade caster selector, using existing retained cascade identities. |
| Static storage prototype | Persistent static depth for fixed projections and a controlled dynamic-over-static composition arm. |
| Cache movement | Camera-driven refresh, scene invalidation and moving-tree handling; measure maintenance separately. |
| Update scheduling | Half-rate far-cascade experiment with matching retained matrices and coverage-triggered refresh. |

Preserve the **DERIVED/configured 350 m** receiver envelope throughout comparative product candidates. Do not implement caster-distance culling at that boundary: an off-camera caster can project a shadow into the visible region. Bevy deliberately disables the directional shadow near-plane rejection for this reason. [light.rs:1888](/Users/Yan/github/vikng-dev/overmatch/vendor/bevy_pbr-0.19.1-scalar-math/src/render/light.rs:1888)

**What only measurement can settle**

- Current terrain, tree, house and tank caster shares. The historical remainder cannot separate them.
- Whether proxy savings shorten the crowded scene’s main-world critical path.
- Main-pass filtering cost, isolated from shadow-map generation.
- Forest-specific gain from exact merging versus approximate proxies.
- Cache maintenance cost under movement, optics changes and tree transitions.
- Shadow equivalence on hull-down tanks, narrow barrel shadows, treelines and grazing slopes.
- Frame pacing from skipped updates, rather than p50 alone.
- Combined gains after the individual levers have been priced.

The repository’s product research makes the rendered picture the visibility model: shadow changes that reveal or hide a tank are gameplay changes. That is a cost to evaluate for every tactic, including filtering and proxies. [Performance-target research:327](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/perf-2026-10-29/performance-target-research.md:327) WoT’s shadow section remains an explicit evidence gap. [WoT mechanisms:38](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/perf-2026-10-29/wot-render-mechanisms.md:38)

**Decision:** start with cascade/split tuning and exact merged tank casters; add hardware filtering as an inexpensive isolated probe. Pursue the shared fixed-sun terrain field before committing to general static-CSM caching. Report delivered inline to preserve the read-only instruction; no edits, builds or game runs were performed.