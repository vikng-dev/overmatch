# Verdict: replace with GPU shoes plus a certified far rest tube

Build **CPU frame-table GPU shoes, retaining the current fit initially, followed by a separately validated far rest tube**. Defer L1. This proposed architecture is **ASSUMED**; its performance advantage needs measurement.

L1’s strongest CPU benefit—removing shoe entities—also comes from the near-rung proposal. Its cheaper fit can be evaluated independently of its tube renderer. Its remaining distinctive benefit is reduced GPU geometry, which the supplied evidence has not priced.

The design’s arithmetic mostly reproduces. Its claims of bounded visual error, unchanged concealment, and predictable frame savings do not.

**Labels:** numerical quantities below are **MEASURED**, **DERIVED**, **ASSUMED**, or **CLAIMED**. Layer names, versions, source locations, and asset indices are identifiers. “Layers” references `track-layers-design.md`; “near” references `track-near-rung.md`.

## High severity — The cheaper fit has no demonstrated error bound

[Layers:187–199](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/perf-2026-10-29/track-layers-design.md:187) concludes that each error being smaller than **CLAIMED ε = 88 mm** keeps L1 inside that budget. That conclusion does not follow.

- Geometry removal and fit displacement can reinforce each other. A conservative additive allowance for the stated geometry, wheel and lateral errors is **DERIVED `88 + 60 + 54 = 202 mm`**, approximately **DERIVED `2.30 px`** at the proposed switch. This is a conservative allowance, not evidence that those maxima actually coincide.
- The wheel calculation bounds an edge-crossing ramp, not arbitrary missed obstacles. The proposed equal-cos samples occur at **DERIVED angles `0°, 28.25°, 40.37°, 50°`** on either half. Existing samples include **DERIVED `20°`**: [wheels.rs:27](/Users/Yan/github/vikng-dev/overmatch/src/track/wheels.rs:27).
  
  For an **ASSUMED witness radius `0.5 m`**, put a narrow obstacle under that existing sample. Its longitudinal position is **DERIVED `0.171 m`**; the first noncentral coarse sample lies at **DERIVED `0.237 m`**. An **ASSUMED obstacle height `0.15 m`** above the resting ground produces approximately **DERIVED `0.120 m`** of additional lift at the existing sample while the coarse samples miss it. That exceeds the proposed budget. The oracle supports block intersections; it supplies no minimum obstacle width: [oracle.rs:361](/Users/Yan/github/vikng-dev/overmatch/src/track/oracle.rs:361).
- The lateral calculation assumes a slope-change limit. No supporting invariant establishes the **ASSUMED `0.3`** limit.
- The **MEASURED historical `63.1 mm`** end-circle penetration is a fixture sweep result, not a universal drape bound. Its own source describes penetration as growing with slack: [route.rs:782](/Users/Yan/github/vikng-dev/overmatch/src/track/route.rs:782).
- Filter continuity does not repair these proofs. The current belly filter explicitly rises immediately: [wrap.rs:194](/Users/Yan/github/vikng-dev/overmatch/src/track/wrap.rs:194).

**Fix:** certify the complete rendered displacement, including geometry removal and fit approximation. Preserve additional samples around discontinuities or retain the accurate fit when no bound is available. Keep draw representation and fit quality independently selectable.

## High severity — Suspension travel does not bound rest-belt error against terrain

[Layers:181](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/perf-2026-10-29/track-layers-design.md:181) substitutes fixture compression travel for the entire rest-shape deviation.

The implementation has separate compression and droop limits: [view.rs:706](/Users/Yan/github/vikng-dev/overmatch/src/track/view.rs:706). Droop is derived from spring and chain constraints: [rig_geom.rs:245](/Users/Yan/github/vikng-dev/overmatch/src/track/rig_geom.rs:245). Belly conform additionally reads terrain and caps displacement against running-gear circles, rather than against suspension compression travel: [wrap.rs:565](/Users/Yan/github/vikng-dev/overmatch/src/track/wrap.rs:565), [wrap.rs:615](/Users/Yan/github/vikng-dev/overmatch/src/track/wrap.rs:615).

Consequently:

- **DERIVED conditional:** a genuine rest-belt displacement of `0.200 m` projects to approximately `1 px` at the proposed D12.
- **DERIVED conditional:** over the design’s fixture span `4.99 m`, an **ASSUMED local terrain/hull slope mismatch `0.3`** produces approximately **DERIVED `0.749 m`** of end clearance or intersection when centered. At D12 that is approximately **DERIVED `3.74 px`**.

The latter is an absolute terrain-gap example, **not** a measured switch displacement: the accurate belt can also bridge terrain. It demonstrates why wheel travel alone proves neither terrain contact nor the difference between representations. A tank aligned with a uniform slope does not automatically have this error; crests, dips and hull/terrain misalignment matter.

Freezing rotation is also underpriced. The **CLAIMED tooth size `0.65 px`** does not bound the appearance of spokes, openings, markings or changing highlights: [layers:340](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/perf-2026-10-29/track-layers-design.md:340).

**Fix:** derive a whole-belt rest-error certificate from asset geometry and the supported articulation/conform envelope. Include wheel appearance separately. Where terrain-dependent validation requires probes, price those probes; “no fit” is not automatically free.

## High severity — Subpixel geometry does not establish gap, shading or concealment parity

The design acknowledges tone risk but treats gap closure as harmless once narrow: [layers:244](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/perf-2026-10-29/track-layers-design.md:244), [layers:326](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/perf-2026-10-29/track-layers-design.md:326).

At D01:

- Pitch is **DERIVED `1.482 px`**.
- The stated wedge opening is approximately **DERIVED `0.089 px`**.
- An idealized periodic slit with those dimensions has **DERIVED open coverage about `6%`**.

A narrow opening can change average coverage even when its individual shape is unresolved. Replacing sky visible through gaps with opaque metal changes brightness. Matching an opaque tube against one background cannot preserve transmission against every background.

The Nyquist argument establishes that individual links alias; it does not establish that an opaque band reproduces their filtered coverage. The existing proxy documentation already records visible ribbon through shoe gaps: [shadow_proxy.rs:91](/Users/Yan/github/vikng-dev/overmatch/src/track/shadow_proxy.rs:91).

Shading parity is likewise unproven:

- The shipped `Mat_Track_Link` contains a normal texture, base-color texture and metallic/roughness texture: `tiger_1.glb`, material identifier `11`.
- L1 proposes albedo and roughness baking without a normal map: [layers:174](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/perf-2026-10-29/track-layers-design.md:174).
- The existing ribbon’s normals are cosmetic corner normals, unsuitable as evidence of a lit replacement’s fidelity: [shadow_proxy.rs:367](/Users/Yan/github/vikng-dev/overmatch/src/track/shadow_proxy.rs:367).
- The certificate measures surface distance; its shading diagnostic explicitly depends on lighting, roughness and normal maps: [measure.py:1063](/Users/Yan/github/vikng-dev/overmatch/scripts/lod/measure.py:1063), [measure.py:1093](/Users/Yan/github/vikng-dev/overmatch/scripts/lod/measure.py:1093).

Thus **“Concealment: none” is unsupported**. Simulation cover can remain unchanged while visual spotting changes through coverage, lighting, terrain contact and shadows.

**Fix:** judge filtered coverage and contrast against sky, terrain and vegetation, including edge-on views. If opacity or normal-detail reconstruction is needed, price its material/pass cost. A cross-fade can soften the transition; it cannot correct the final representation.

## High severity — The optic burst is unpriced, and hysteresis cannot absorb it

[Layers:248](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/perf-2026-10-29/track-layers-design.md:248) says the burst is priced in captures, but [layers:424](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/perf-2026-10-29/track-layers-design.md:424) merely requests a future trace.

The projection increases **DERIVED `1.868×`**. The **ASSUMED hysteresis `10%`** prevents distance-boundary chatter; it neither schedules promotions nor limits their cost.

Using the design’s **ASSUMED optimized costs**:

| Promotion scenario | DERIVED work estimate, conditional on those costs |
|---|---:|
| ASSUMED battle split promotes every eligible tank: L0/L1/L2 becomes `14/16/0` | `0.725 → 1.798 ms`, an increase of `1.073 ms` |
| ASSUMED synchronized group of `30` tanks, L1→L0 | `1.14 → 2.55 ms`, an increase of `1.41 ms` |

The latter also reattaches **DERIVED `5,820` shoe entities**. Rebuilding their current proxy payloads represents approximately **DERIVED `1.30 MB`** before render-side copies. Hierarchy mutation, propagation, renewed visibility/extraction, preparation and cold resource readiness have no measured price here.

There is also a scheduling hazard in the precedent being reused: view facts are read in `Update`, while camera placement changes the lens in `PostUpdate`: [view.rs:407](/Users/Yan/github/vikng-dev/overmatch/src/view.rs:407), [camera.rs:244](/Users/Yan/github/vikng-dev/overmatch/src/camera.rs:244). Following that path can render the newly magnified view using the previous profile. Hysteresis cannot conceal that error.

**Fix:** resolve the intended lens before selection and fitting; prepare promotion data before revealing it. Price both the toggle frame and subsequent preparation frames. GPU shoes remove the hierarchy-reattachment component, though fitting and resource readiness still need measurement.

## High severity — Far prints matter, and layer selection changes persistent history

The map spans **DERIVED `1500 m`** from its declared bounds: [level.json:26](/Users/Yan/github/vikng-dev/overmatch/assets/maps/kalinovo/level.json:26). World coverage uses **DERIVED `4096` texels per side**, giving **DERIVED `0.366 m`** per texel: [store.rs:69](/Users/Yan/github/vikng-dev/overmatch/src/track_prints/store.rs:69).

That resolution does not make missing or extra trail segments invisible:

- World texels store fractional footprint coverage, including contributions smaller than a texel: [store.rs:44](/Users/Yan/github/vikng-dev/overmatch/src/track_prints/store.rs:44).
- The shader always samples world coverage. Near detail and relief fade separately: [terrain_blend.wgsl:309](/Users/Yan/github/vikng-dev/overmatch/assets/shaders/terrain_blend.wgsl:309).
- History remains at **DERIVED near resolution `0.046875 m`** and returns as the camera approaches: [store.rs:28](/Users/Yan/github/vikng-dev/overmatch/src/track_prints/store.rs:28), [store.rs:58](/Users/Yan/github/vikng-dev/overmatch/src/track_prints/store.rs:58).

The **ASSUMED contact tolerance `0.03 m`** accepts much less upward clearance than L2’s **CLAIMED error `0.200 m`**. Widening it can stamp beneath a visibly hovering belt. Leaving it unchanged can omit a real trail. Existing contact normalization can also place a stamp on terrain beneath an imperfectly contacting drawn shoe: [stamper.rs:120](/Users/Yan/github/vikng-dev/overmatch/src/track_prints/stamper.rs:120).

L1 can alter these decisions too. Promotion rebasing can additionally create artificial shoe motion unless contact memory handles representation discontinuities.

The doctrine discussion incorrectly narrows the rule to effect size. ADR-0023 explicitly includes **behavior**: [ADR-0023:3](/Users/Yan/github/vikng-dev/overmatch/.agents/docs/adr/0023-vfx-honesty-scaled-by-physics.md:3). Camera-dependent persistent touchdown history is a real product cost.

**Fix:** keep persistent contact generation independent of draw-layer selection. Initially retaining accurate contact fitting gives back much of the forecast fit saving. A cheaper canonical contact producer is a separate optimization whose price and fidelity must be demonstrated.

Also, the near store’s **DERIVED span `96 m`** is its full width, not its radius; the optional off-screen policy misnames it: [layers:254](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/perf-2026-10-29/track-layers-design.md:254).

## Medium severity — “Same ribbon” preserves only an identical-joint caster swap

An identical opaque tube on identical joints can preserve the existing caster geometry. That limited claim is credible.

The runtime design also changes fitting, then freezes geometry. Those changes move the shadow:

- The shipped sun elevation is **DERIVED from source `17°`**: [world.rs:38](/Users/Yan/github/vikng-dev/overmatch/src/world.rs:38).
- A **CLAIMED fit-height difference `60 mm`** displaces a flat-ground shadow approximately **DERIVED `0.196 m = 0.060 / tan(17°)`**.
- At D01 this can project to approximately **DERIVED `2.23 px`** when the displacement lies across the view.

That is a conditional geometric displacement before shadow filtering, not a measured visible pop.

Furthermore, **DERIVED from settings**, the default shadow distance is `350 m`, with selectable `700 m` and `1000 m` alternatives: [settings.rs:384](/Users/Yan/github/vikng-dev/overmatch/src/settings.rs:384). The claim that nothing casts beyond the default is not generally true.

**Fix:** validate actual fitting transitions and L2 promotion/demotion under every supported shadow range. Preserve atomic caster readiness; the existing code deliberately retains shoes until replacement geometry exists: [view.rs:785](/Users/Yan/github/vikng-dev/overmatch/src/track/view.rs:785).

## Medium severity — Distances reproduce, but the feature inputs and projection claim need correction

The camera fields are correct: **DERIVED commander field `45°`**, **DERIVED optic field `25°`**, read from [tank data:398](/Users/Yan/github/vikng-dev/overmatch/assets/tiger_1/tiger_1.tank.ron:398). The optic’s **DERIVED authored magnification `2.5×`** is descriptive data, not an additional projection multiplier: [spec.rs:535](/Users/Yan/github/vikng-dev/overmatch/src/spec.rs:535).

Using **ASSUMED native heights** below, **CLAIMED ε values `0.088 m` and `0.200 m`**, **ASSUMED budget `1 px`**, and **CLAIMED rounded radius `0.38 m`**:

`k = H / (2 tan(vfov/2)); D = εk + r`

| View | ASSUMED H | DERIVED k | DERIVED D01, m | DERIVED D12, m |
|---|---:|---:|---:|---:|
| Commander | 1080 px | 1303.675 | 115.103 | 261.115 |
| Commander | 1440 px | 1738.234 | 153.345 | 348.027 |
| Optic | 1080 px | 2435.783 | 214.729 | 487.537 |
| Optic | 1440 px | 3247.710 | 286.178 | 649.922 |

The requested rounded distances therefore reproduce.

However, reading the shipped GLB JSON gives:

- **DERIVED pitch `130.42593 mm`**, from the pin translations.
- **DERIVED pin-to-inner offset `24.64000 mm`**, rather than the quoted historical value.
- **DERIVED horn extension `88.64046 mm`**, from `Link` position accessor identifier `276`, `Link_Box` accessor identifier `271`, and the box-node translation.
- **DERIVED grouser extension `46.26015 mm`**.

Thus even the current horn lower bound slightly exceeds the proposed placeholder. Using it gives **DERIVED D01 distances approximately `115.94 / 154.46 / 216.29 / 288.26 m`** with the certificate radius. The full deviation still needs certification.

Finally, Euclidean distance is not perspective depth. At the horizontal edge of an **ASSUMED aspect ratio `16:9`** commander view, a screen-vertical feature can project **DERIVED `1.242×`** larger than this radial-distance estimate. The selector measures Euclidean distance: [link_view.rs:1045](/Users/Yan/github/vikng-dev/overmatch/src/track/link_view.rs:1045).

**Fix:** use current asset-derived certificates and rendered physical height, including render scale. Apply a conservative projection bound for off-axis geometry. Treat **ASSUMED `1 px`** as a geometric tolerance requiring appearance validation, not an invisibility theorem.

## Medium severity — The savings total combines separate optimizations and unmeasured wall-time assumptions

The weighted sum works:

**DERIVED, conditional on ASSUMED costs and occupancy:**

`4 × 85 + 10 × 38 + 16 × 0.3 = 724.8 µs`

That explains the forecast **DERIVED `0.73 ms`**. It does not establish a measured saving.

[Layers:394](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/perf-2026-10-29/track-layers-design.md:394) combines layers, exact waste removal and parallelism into **CLAIMED wall saving about `3.3 ms`**. Its own limitations say the critical path and shoe-related render costs are unmeasured.

For a close battle with **ASSUMED all `30` tanks in L0**:

- **DERIVED layer-selection saving: none.**
- **DERIVED optimized serial work: `30 × 85 µs = 2.55 ms`.**
- Against **DERIVED sampled baseline `3.56 ms`**, approximately **DERIVED `1.01 ms`** comes from other optimizations, not the layers.

The **ASSUMED proportional shoe saving `58 µs/tank`** cannot be assigned from whole-scene engine totals. The near study correctly makes that distinction: [near:110](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/perf-2026-10-29/track-near-rung.md:110).

The new split is plausible but not reproducible from committed artifacts. Its rows sum to approximately **DERIVED `117.2 µs`**, consistent with a rounded **CLAIMED `119 µs`**. The historical **CLAIMED `56 µs`** referred to the wrap comparison: [architecture.md:97](/Users/Yan/github/vikng-dev/overmatch/.agents/docs/design/track-model/architecture.md:97). Even crediting the entire new sink and drape buckets explains only **DERIVED `33.4 µs`** of the **DERIVED nominal increase `63 µs`**. It does not prove a like-for-like regression.

**Fix:** publish reproducible attribution and separate exact optimizations, representation changes, fit approximations, scheduling and critical-path effects. Report close-range occupancy as well as the assumed battle distribution.

## Medium severity — L1 has not earned its separate renderer

After GPU shoes, L1’s forecast CPU advantage over optimized near fitting is only **DERIVED `47 µs/tank`**, or **DERIVED `0.47 ms`** for the **ASSUMED mid-range population `10`**. Its fitting changes can be applied to GPU shoes without introducing another material representation.

L1 retains a potentially valuable GPU advantage: **DERIVED camera triangles `32,204 → 1,552`**, approximately **DERIVED `95.2%` fewer**, using the design’s counts. GPU instancing preserves geometry work and can lose per-shoe culling: [near:132](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/perf-2026-10-29/track-near-rung.md:132). This benefit deserves measurement, not dismissal.

Its proposed moving mesh payload is nevertheless approximately **DERIVED `37.6 KB/tank/frame`**, versus the near study’s **ASSUMED compact frame format, DERIVED `3,104 B/tank/frame`**. It also introduces coverage, shading, phase-seam and shadow-handover obligations.

**Fix:** build GPU shoes first, then a certified far rest tube. Add L1 only if measured mid-range GPU savings justify its implementation and visual costs.

## Source checks that hold, with narrower wording

- **Transform propagation:** verified in installed locked Bevy source. Moving parents cause descendant transform multiplication regardless of visibility: [systems.rs:707](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_transform-0.19.1/src/systems.rs:707). Detaching a stationary hidden pool stops inherited motion work. But **“hiding saves nothing” is false broadly**: hidden meshes skip visibility evaluation and rendering work: [visibility/mod.rs:802](/Users/Yan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_camera-0.19.1/src/visibility/mod.rs:802).
- **MP force simulation:** verified. The client registers replica attachment, which inserts static bodies; the force loop skips them before command shaping: [client.rs:656](/Users/Yan/github/vikng-dev/overmatch/src/net/client.rs:656), [rig.rs:55](/Users/Yan/github/vikng-dev/overmatch/src/net/rig.rs:55), [sim.rs:727](/Users/Yan/github/vikng-dev/overmatch/src/track/sim.rs:727). The system may execute, but replica tanks do not perform belt-force simulation.
- **Phase:** L0/L1 registration is credible when both consume identical sampled joints. Current placement additionally rotates stable material identity by the phase quotient: [link_view.rs:1113](/Users/Yan/github/vikng-dev/overmatch/src/track/link_view.rs:1113). Preserve that identity for print memory, the texture seam, reverse motion and handover. A shared phase scalar alone does not prove all these contracts.
- **Doctrine costs:** ADR-0034 requires generic derivation from asset data, including radii, sections, droop and certificates—not fixture-shaped bounds. ADR-0014 permits view-owned frame tables and rest meshes; none may initialize or modify simulation state. No new sim/view breach is established by this proposal. Independent print fitting and coverage-preserving rendering are costs to budget explicitly, not reasons to stop optimization.

## What the owner must see side by side before approving

- **Representation comparisons:** current shoes, GPU shoes, proposed L1 and rest tube, at native resolution through both cameras. Include broadside, edge-on and top-run views against sky, dark terrain and vegetation. Show coverage/contrast and lighting differences, not only mean luma.
- **Actual handovers:** compare identical-joint rendering separately from real fit changes. Include narrow obstacles, lateral ridges, crests, dips, slope transitions and articulated wheels. Show terrain intersection and separation.
- **Phase and shadows:** forward/reverse travel, pitch wrap, wheelspin, skid and promotion after prolonged rest mode. Compare shadow edges under the shipped sun and every supported cascade range; retain replacement-readiness evidence.
- **Persistent prints:** identical routes with different camera distances and optic use, followed by inspection of the same history nearby. Show world coverage as well as relief, including missed stamps, invented stamps and handover sweeps.
- **Optic traces:** warm and cold synchronized promotions, including the first magnified frame and subsequent preparation. Attribute fitting, hierarchy work, propagation, extraction, uploads and GPU time.
- **Performance alternatives:** compare the optimized existing path, GPU shoes, GPU shoes with cheaper fitting, the proposed layered design and the proposed far-tube alternative. Include a close battle. For the game’s damage-model feedback views, verify deformation-aware look/mask rendering alongside ordinary rendering.

No files were changed, no builds or application runs were performed, and the excluded documents were not opened.