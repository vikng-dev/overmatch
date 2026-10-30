# Perf triage, 2026-10-30: #158 #160 #161 #162 #163 #164 #166 #175 (epic #151)

This note covers frame cost only. #164 is about the inset that shows the game's damage-model feedback, where crew are HP pools at stations.

This was a read-only lane. Every pointer was checked against `main` at `a6dbf90`. None of the cited files changed between `e1cbc4a` (the audit) and `a6dbf90`.

Labels:
- **MEASURED** is the night battery (`night-results-2026-10-30.md`) or the slice 9b note.
- **DERIVED** is arithmetic on source or on MEASURED numbers.
- **ASSUMED** is a forecast.
- **CLAIMED** is unverified prose.

## Summary

| Issue | Class | Gain | On the M4 today? | Verdict | Size |
|---|---|---|---|---|---|
| #175 depth prepass | optimization ✓; not a setting | GPU. Ceiling −1.07 ms p50 at 2 tanks (MEASURED). The kept copy's cost is subtracted from that and is UNMEASURED. | Yes | ready-for-agent | M |
| #164 pen cam corner | optimization. The half-resolution part is really a setting. | Transient, +4.3 ms (MEASURED). Half resolution saves ≈0.35–0.53 ms (DERIVED; equal per-pixel cost ASSUMED). | Yes, while the corner shows | ready-for-agent, attribution first | S, then M |
| #162 VFX materials and ribbons | bug ✓ for sprites and allocations. The GPU ribbon is an optimization. | CPU during fire: up to 112 bind-group rebuilds per frame removed (DERIVED from source). ms UNMEASURED. | No, while the GPU paces | ready-for-agent | M |
| #160 terrain shader | optimization. The composite-page cache is a setting. | GPU. Terrain's share is UNMEASURED; ≤6.7–7.1 ms of the 2-tank frame scales with pixels (DERIVED). The macro bake is ASSUMED ≤0.2 ms. | Yes, if large | ready-for-agent, pricing first | S, then L |
| #166 servo loop | bug ✓ | Sim tick: 3,480 wasted visits per tick at 30 tanks (DERIVED), ≈3–14 µs (ASSUMED) | No | ready-for-agent | S |
| #158 555 meshes | optimization ✓ | CPU. Far tube: −1.7 ms view + ~0.9 pool-ms in battle (DERIVED, design doc). Rung swap: −270 entities per tank (DERIVED). | No; it matters under LOW or 50 % scale | **needs-owner** | L (tube) + M (swap) |
| #163 half-resolution smoke | optimization in name. The look changes at depth edges. | GPU in heavy smoke only. UNMEASURED. | Unknown | **needs-owner** | L |
| #161 print relief | optimization ✓, exact. The claim is partly stale. | GPU. ≤0.14–0.17 ms on a print-filled view (DERIVED), below the 0.2 ms trust floor | No | **needs-owner** (close) | S |

## #175 — the camera draws opaque geometry twice for depth

- **Claim:** TRUE. `DepthPrepass` is at `src/camera.rs:330`.
  - **Consumer 1:** the billboards' soft fade, `assets/shaders/vfx_billboard.wgsl:64–86`, gated on `DEPTH_PREPASS`.
  - **Consumer 2:** the hull marks' `ForwardDecal`, `src/vfx/hull_marks.rs:24–33`.
  - No other reader exists: `src/` uses no contact shadows, SSR, transmission or deferred.
- **Correction: hull marks hard-require the prepass.**
  - Bevy states the requirement at `vendor/bevy_pbr-0.19.1-scalar-math/src/decal/forward.rs:58`.
  - `forward_decal.wgsl:37` calls `prepass_depth` with no guard. That function exists only under `DEPTH_PREPASS` (`prepass_utils.wgsl:5`).
  - So turning the prepass off does not merely harden the marks' fade: the marks do not draw at all. The night lever cannot ship as a setting.
  - The night doc §6 row 7 ("feeds only the VFX soft-particle fade") is wrong for the same reason.
- **Gain:** GPU. The ceiling is −1.07 ms p50 at 2 tanks with MSAA 4× (MEASURED). It is not priced at 30 tanks.
  - Today's prepass also copies the depth (`bevy_core_pipeline-0.19.1/src/prepass/node.rs:245–253`). The plan below keeps a copy, so the net saving is 1.07 ms minus the copy's cost (UNMEASURED).
  - The copy moves 2560×1440 × 4 samples × 4 B = 59 MB read and 59 MB written. At 120 GB/s (CLAIMED, M4 spec) that is ≤ ≈1.0 ms if uncompressed (DERIVED worst case). With MSAA off it is a quarter of that.
- **IMR caveat:**
  - Apple GPUs remove opaque overdraw in hardware (CLAIMED, Apple's Metal guidance), so on the M4 the prepass is close to pure overhead.
  - On a desktop IMR GPU, the prepass's early-Z on the terrain shader may pay for itself (ASSUMED).
  - The M4 price therefore does not transfer to the reference PC.
- **Plan (same picture):**
  1. Keep `DepthPrepass` on the camera, so the bindings, the `DEPTH_PREPASS` def and the prepass depth texture all stay.
  2. In the render world, after extraction, drop the camera's entries from `ViewBinnedRenderPhases<Opaque3dPrepass>` and `<AlphaMask3dPrepass>`.
     - `run_prepass_system` returns early without them (`node.rs:156`), so the draw, the clear and the copy are all skipped.
     - Queueing tolerates the missing phase (`prepass/mod.rs:1210–1213`, `Option`s).
     - The main pass still clears and writes depth (`GreaterEqual`, write on, `render/mesh.rs:3641`).
  3. Add a `Core3d` system ordered `.after(main_opaque_pass_3d).before(main_transparent_pass_3d)` that copies `ViewDepthTexture` into `ViewPrepassTextures.depth`. Both systems are `pub` and chained (`core_3d/mod.rs:157`).
     - Both consumers draw in `Transparent3d`, after the copy.
  - **Files:** a new render module (or `src/camera.rs`), plus a `SPIKE_ABLATE` arm in `src/render_cost.rs`.
- **Parity:**
  - Per-pixel diff = 0 on captures of dust on terrain and of hull marks, including a mark behind a nearer surface, with MSAA 4× and off.
  - A render-world test: the camera's prepass phases are empty, and the copy sits between the two passes.
  - **One known difference:** opaque materials with `enable_prepass() = false` that the main camera draws now enter the depth. One is `LookMaterial` with an opaque blend (`src/damage_cam/pen_cam/look.rs:342–360`) in the kill cam lesson. The lane checks those captures.
- **Verdict:** ready-for-agent, with these refinements:
  - Build it as a lever first: today, the copy design, and off.
  - Price it A/B/B/A at 2 and 30 tanks, with MSAA 4× and off.
  - Make it the default if it nets ≥ 0.2 ms p50 on the M4.
  - Keep the prepass path selectable until a reference-PC run prices it.
- **Overlap:** `src/camera.rs` with the presets stream (the bloom row), which is small. `src/render_cost.rs` is shared by every lever lane.

## #164 — pen cam corner at half resolution

- **Claim:** TRUE at all three pointers.
  - `src/damage_cam/pen_cam/stage.rs:317`: `Hdr`, `Bloom::NATURAL`, `NativeResolution`.
  - `hud.rs:265`: logical size × scale factor.
  - `look.rs:542`: the hull and part mask cameras.
  - +4.3 ms p50 offline (MEASURED, slice 9b note).
- **The pixel premise is weak.**
  - The corner is 0.30 × the logical width, clamped to 384–640 px, at aspect 1.6 (`hud.rs:28–31`).
  - At 2560×1440 that is 256,000 px (scale factor 1) or 368,640 px (scale factor 2): 6.9–10 % of the main view (DERIVED).
  - The render-scale levers put ≈6.7–7.1 ms of the 2-tank frame in proportion to main-pass pixels (DERIVED: 5.32/0.75 and 2.93/0.4375).
  - If an inset pixel costs what a world pixel costs (ASSUMED), the inset's pixels cost 0.47–0.71 ms. Half resolution would save 0.35–0.53 ms (DERIVED).
  - So ≥ 3.6 of the 4.3 ms is probably per-view, not per-pixel. The note says the same: "the inset's passes, not the content".
- **A candidate per-view cost:**
  - Bevy's `check_visibility` sweeps every entity once per active camera (`bevy_camera-0.19.1/src/visibility/mod.rs:777–826`).
  - The corner adds up to 3 active cameras. The world holds ≈36k retained tree primitives (DERIVED, audit).
  - That this sweep is the driver is ASSUMED.
- **Class:**
  - Half resolution is a visible trade (softer masks), so it is a setting.
  - Caching a finished, still picture is exact.
- **Plan:**
  1. (S) Add `SPIKE_ABLATE` arms:
     - `inset_half`: × 0.5 on the size at `hud.rs:265`. The masks follow, because they are keyed by size.
     - `inset_masks`: no mask cameras.
     - `inset_bloom`: no bloom on the inset.
     - Run them A/B/B/A at night with `scripts/perf/run-pencam-cost.sh`.
  2. Let the price choose:
     - If the cost is per-view: fewer inset cameras, or fewer entities (#157, #158).
     - If it is per-pixel: half resolution as a Gameplay-tab row, with owner stills.
- **Verdict:** ready-for-agent, re-scoped to attribution first. No overlap with the active streams.

## #162 — a material per sprite, and ribbons rebuilt every frame

- **Claim:** TRUE.
  - `src/vfx/billboard.rs:271` adds a material per sprite, and `:369` calls `get_mut` every frame.
  - The caps are 96 (`:39`) + 16 (`impact.rs:138`) = 112.
  - The ribbon pointer `trail.rs:522` has moved to `:523`. The 15 vectors per rebuild are correct (`:575, 584, 608–612, 641–645, 675–677`).
- **It is worse than claimed.** A modified material is torn down and rebuilt: see `vendor/bevy_pbr-0.19.1-scalar-math/src/material.rs:1573–1582`, whose TODO reads "delete and recreate the bind group". That is up to 112 bind-group and uniform-buffer recreations per frame under fire (DERIVED).
- **Class:**
  - A bug, for the sprites and for the ribbon's allocations.
  - The ribbon's per-frame position rewrite is not waste. The strip faces the camera and drifts with age (`write_ribbon(…, cam)`).
  - Expanding the ribbon on the GPU is an optimization of ≤ 2,080 vertices per frame (DERIVED: 8 trails × 260).
- **Gain:**
  - CPU render preparation, during fire. ms UNMEASURED.
  - Hidden on the M4 at native resolution while the GPU paces.
  - The GPU side is fewer transparent draws once sprites batch (ASSUMED small).
- **Plan:**
  - **Sprites:**
    - One `VfxBillboardMaterial` per family (atlas, LUT, alpha mode).
    - Each sprite's `VfxParams` lives in one fixed 112-slot `ShaderBuffer`, indexed by `MeshTag` = its ring slot.
    - `bevy_render-0.19.1/src/storage.rs:175–186` reuses the GPU buffer when the size is unchanged. That means one `write_buffer` per frame and no bind-group churn.
  - **Ribbons:** keep the vectors and write through `attribute_mut`, keeping the CPU rewrite.
  - **Files:** `src/vfx/billboard.rs`, the sprite spawners, `assets/shaders/vfx_billboard.wgsl`, `src/vfx/trail.rs`.
- **Parity:**
  - The f32 lanes are the same, so `scripts/perf/run-fire-capture.sh` frames should be bit-identical.
  - A test: two sprites of one family share one handle.
  - A test: `age_billboards` raises no `Modified` event on any material.
- **Verdict:** ready-for-agent, with these refinements:
  - Replace "no ribbon is rebuilt whole each frame" with "no per-frame allocation".
  - The GPU ribbon goes to a later task.
  - Price it with the fire capture now, and A/B/B/A in #153's scene when that scene lands.
- **Overlap:** `vfx_billboard.wgsl` with #163. #175 leaves that shader untouched.

## #160 — terrain noise per pixel, and caching far terrain

- **Claim:** TRUE.
  - The macro tint is at `assets/shaders/terrain_blend.wgsl:464`, and the layer loop at `:405`.
  - The request count 6 + 3k is right: the mask, the trodden layer, the 4 base-pack maps, and 3 per painted layer.
- **Class:**
  - **The macro bake is exact only one way.** The GPU must bake the lattice hashes with today's `macro_hash`, and the shader must keep the 4-corner smoothstep. That is 8 loads in place of 8 hashes, which is not obviously cheaper in a shader already bound by texture fetches (ASSUMED).
  - A 1–2-fetch bilinear version is not exact. CPU and GPU `sin` differ at these arguments, and the filter weights have limited precision, so it changes the pattern.
  - **Far composite pages are lossy**, which makes them a setting ("terrain detail"). The terrain LOD budget stays untouched.
- **Gain:** GPU.
  - Terrain's share is UNMEASURED. It is bounded above by the 6.7–7.1 ms of the 2-tank frame that scales with pixels (DERIVED; that budget covers all per-pixel work).
  - The macro hash is 29.5 M `sin` per frame at worst (DERIVED). Its time is ASSUMED ≤ 0.2 ms.
- **Plan:**
  1. (S) Add `SPIKE_ABLATE` arms through shader defs:
     - `terrain_macro` skips the macro field;
     - `terrain_layers` draws the base pack only.
     - Price them A/B/B/A.
  2. Build composite pages only if the layers price ≥ 1 ms, with owner stills.
- **Verdict:** ready-for-agent, re-scoped to pricing.
- **Overlap:** `terrain_blend.wgsl` with #170's receiver integration (shadows stream). Land after it, or keep the lever to a def guard.

## #166 — the servo loop scans every servo for every tank

- **Claim:** TRUE at `src/aim.rs:434` and `:474`.
  - The system runs in `FixedUpdate` (`:111`), on the server and the clients.
  - The fixture tank has 4 servos (`tiger_1.tank.ron:223–226`, fixture data).
- **Gain:** sim tick.
  - 30 × 120 − 120 = 3,480 wasted visits per tick (DERIVED).
  - At 1–4 ns each (ASSUMED) that is 3–14 µs, against a 13.4–13.9 ms server tick at 30 tanks (DERIVED): ≤ 0.1 %.
- **Plan:**
  - The tank pass fills a scratch map: tank → (point, `to_local`, pose).
  - One pass over the servos then reads its root's entry.
  - Each servo's float operations are the same, so the result is bit-identical. Only `src/aim.rs` changes.
- **Parity:** a ≥ 3-tank test, with the current function kept as the oracle, plus the existing aim tests.
- **Verdict:** ready-for-agent. Drop the A/B/B/A acceptance: the gain is below every floor.

## #158 — each tank is 555 meshes

- **Claim:** TRUE (`src/geometry_lod.rs:377`, `src/track/link_view.rs:1035`).
  - The owner's comment of 2026-10-30 makes the track half the far rest-shape tube.
  - The tube comes third, after #155/#156 and #172, and its switch is judged by eye.
- **New finding:** 270 of the 555 are coincident rung siblings.
  - The module itself rejected siblings for the shoes, because a moving sibling pays propagation, the visibility sweep and extraction every frame (`geometry_lod.rs:10–28`).
  - A per-primitive mesh-handle swap (the shoes' mechanism) removes 270 entities per tank, 8,100 at 30 tanks (DERIVED). It needs no merged asset.
  - It gives up the per-view choice, but nothing loses by it:
    - the tanks' views are the `PlayerView` and its cascades, which inherit the camera's choice (`geometry_lod.rs:40`);
    - the x-ray mask copies already follow handle swaps.
  - That the picture stays the same is ASSUMED until a selector-parity test passes.
- **The material merge collides with per-part consumers:**
  - hull marks hang under a part's view node (`hull_marks.rs:1–3`);
  - the x-ray swaps materials per part;
  - parts detach.
- **Gain:** CPU.
  - Far tube: −1.7 ms view + ~0.9 pool-ms in battle (DERIVED, `track-layers-design.md` task 2; the layer split is ASSUMED).
  - Rung swap: UNMEASURED. It overlaps #173, which stops these hidden moving siblings from sending a message every frame.
- **Plan:**
  - The tube follows the design's task 2, with the owner's by-eye constant.
  - The swap goes in `geometry_lod.rs`, after #173. Its selector reproduces `VisibilityRange`'s law: the same distance basis and the same hysteresis.
- **Verdict:** **needs-owner.** The question is below.
- **Overlap:** the tracks stream (`src/track/*`). The vendored `bevy_pbr`, through #173.

## #163 — smoke and dust at half resolution

- **Claim:** TRUE.
  - `vfx_billboard.wgsl:64`.
  - The alpha mode is at `billboard.rs:105`; the issue cites `:109`, which is in the same impl.
  - `trail.rs:130`.
- **Class:**
  - An upsample cannot be pixel-identical at depth edges, so the acceptance "hides and shows exactly what full resolution does" can never pass as written.
  - Smoke opacity is on the list of things no preset may change (#159), so this cannot become a setting row either.
- **Gain:** GPU, in heavy smoke only. UNMEASURED. Idle probes raise no smoke.
  - The 75 % (DERIVED) is of particle pixel work only.
  - Apple GPUs blend on tile (CLAIMED), so the M4 may understate a PC's cost.
- **Plan:** wait for #153 to price smoke with a VFX lever, and for #175 to settle the depth source. Then:
  - a half-resolution target for the smoke-mass layers;
  - a depth-aware upsample;
  - a composite.
  - **Files:** a new render pass, `billboard.rs`, `trail.rs`, both shaders.
- **Verdict:** **needs-owner.** The question is below.

## #161 — print relief per pixel

- **Claim:** PARTLY STALE.
  - The relief block is entered by distance (`:328`). But the march already returns after one lookup where the pixel's own ground is unpressed (`terrain_blend.wgsl:204`). That landed in `c5382d3`, before the audit.
  - The sun march runs only when `landed_depth > 0` (`:340`).
  - So the acceptance "a pixel with no print runs no march" already holds.
  - `:489` then `:492` is correct: `sun_direct` reruns the BRDF and the cascade fetch on self-shadowed pixels.
- **What is left:**
  - **Empty pixels in print range** pay 6 lookups against 5 at LIGHTING. An occupancy bit could skip ≤ 6 well-cached lookups (UNMEASURED, ASSUMED small).
  - **The sun fold's ceiling** is SUN − PARALLAX = 0.14–0.17 ms p50 on a print-filled view (DERIVED from MEASURED §4). That ceiling also contains the sun march itself.
    - It sits below the 0.2 ms A/B/B/A trust floor.
    - LOW already uses Lighting (#159, the owner's table).
- **The fold needs a per-fragment direct-sun factor** in the vendored `pbr_functions.wgsl`. #170's receiver integration needs that same hook (shadow-tactics D), and once it exists the fold costs one multiply.
- **Verdict:** **needs-owner** (close). The question is below.

## Owner questions

1. **#158:** Should the static half change from "merge immutable pieces per material" to "one entity per body primitive, with its mesh handle swapped by distance" (the shoes' mechanism), landing after #173?
   - **Recommended: yes.** It removes 270 entities per tank (DERIVED), with the same picture, and it keeps each part's identity for hull marks, the x-ray and detachment.
2. **#163:** Pixel-exact is unattainable. Is "no difference visible at 1:1 on the concealment stills" the bar? The stills cover a tank behind and in front of smoke, at several ranges, in third person and in the optic.
   - **Recommended: yes,** and build only if #153 prices smoke at ≥ 0.5 ms p50.
3. **#161:** Should #161 close, with the sun fold carried as one line in #170's receiver-integration brief?
   - **Recommended: yes.** The empty-pixel skip already exists, and the remainder is ≤ 0.17 ms (DERIVED), below the measurement floor.

## Recommended order, after the active streams

Ranked by gain × confidence ÷ size:

1. **#175:** a GPU lever with a MEASURED ceiling that shows on the M4. Price the copy first.
2. **#164:** an S-sized attribution of a MEASURED 4.3 ms transient, over the 1.5 ms transient ceiling (ASSUMED).
3. **#160:** S-sized pricing of the largest unpriced GPU consumer. It can ride the same night as #164.
4. **#162:** certain waste, exact, M-sized. It is CPU, so it shows in battle on weaker CPUs.
5. **#166:** S-sized and exact, with a gain of about 0. A filler lane.
6. **#158:** the far tube after #155/#156/#172, and the rung swap after #173 once the owner answers.
7. **#163:** after #153, #175 and the owner's bar.
8. **#161:** close, and fold the sun term into #170.
