# Track prints with depth: every approach, cheapest to most ambitious

2026-10-28 · research lane · **Subject: terrain surface marks.** Not the damage model. Nothing was built or run.

**Labels.** MEASURED: counted or timed (here or by a cited run). DERIVED: computed here from cited values. ASSUMED:
an input chosen here, to be replaced by a measurement. CLAIMED: stated by a source (for code, its default literal).
**Pins.** [D] = `GaijinEntertainment/DagorEngine` @ `2baf194` (2026-10-16), paths from its root, read from a local
clone. [B] = the crates in `Cargo.lock`: bevy 0.19.1, wgpu/naga 29.0.4, avian3d 0.7.0, parry3d 0.27.0, read from the
cargo registry; `bevy_pbr` from `vendor/bevy_pbr-0.19.1-scalar-math/`. Repo paths are from the repo root.
**Scope.** The sister lane owns how a print follows from the shoe's shape and the ground's hardness. This note uses
"footprint texture" and "depth scale" as inputs from that lane.

## 1. Comparison

| # | Approach | What the player sees | GPU (M4 / mid x86) | Other costs | Build | Shipped games |
|---|---|---|---|---|---|---|
| 1 | **Decal ribbon** per track side: a terrain-conformed strip using `ForwardDecalMaterial`, with a normal map and Bevy's POM | A bumpy dark band with a fake inner depth up close. Flat at grazing angles and in silhouette. No shadow cast inside it. A centre-line strip draws pivot turns as a thin ring. Length capped by the ring buffer. | Alpha-blended re-shade of the print pixels. DERIVED small at a few % of the screen; the terrain pass has never been timed alone. Per-segment decals would cost a draw each, and Metal is CPU-bound on draws. | 0 wire. About 0.5 MB of RAM. A late joiner misses old prints. | S–M | WoT (per-chassis ribbon, albedo + normal); Dagor `tire_tracks` (engine library, WT's use unpublished); CoH2 (press report: persistent tread marks) |
| 2 | **Trail map read by the terrain shader.** Every grounded shoe is stamped. The shader takes a normal from the gradient, then runs POM and a march toward the sun near the camera, and darkens the ground further out. | Prints sit *in* the ground: the far wall shows, the near floor is hidden, and the 17° sun shadows the wall. Pivot scrub and slip smear come out right. Persists the whole battle (coarse far, sharp near). Still flat in silhouette, and the belt floats over deep ruts. | DERIVED ~0.1–0.3 ms on M4, less on x86, scaled from Dagor's CLAIMED 0.5–0.6 ms on Xbox One S for a heavier pipeline. | 0 wire. ~32–43 MiB GPU. 5–20 MB CPU log per battle. 1 texture, 0 new samplers. | M–L | Dagor (strips into the terrain clipmap); BF3 (virtual texture with dynamic updates); Batman AO consoles (relief map); Unreal RVT and render targets |
| 3 | **View-only real geometry:** a camera-centred clipmap mesh displaced by grid + trail map in the vertex shader | Real rut walls: silhouettes on crests, real cascade and contact shadows, true occlusion | ~0.8M extra tris DERIVED, drawn in 3–4 passes (≈2.7 tanks of rung-0 shoes per pass). Not convertible to ms from repo measurements. | Rebuilds the near terrain renderer. Render mesh differs from the grid by up to the rut depth. Plus all of #2. | L–XL | Batman AO PC (DX11 tessellation); RotTR (HW tessellation); Dagor `deform_hmap`; MudRunner (VTF grid); UE Virtual Heightfield Mesh |
| 4 | **Physical deformation:** a server-owned fine rut layer, read by the belt oracle | Ruts are *felt*: belts drop in, and the shoe's 3 columns feel the edges. Looks still need #2 or #3. | That of #2 or #3 | Server CPU DERIVED 6–13 % of a core at 30 tanks. ~16 MB server RAM. Late-join snapshot of MBs. Opens the PRODUCT "simulation-static" fork. ONE-SURFACE must be restated. | XL | War Thunder "Ground Breaking" (craters and entrenching; Dagor `terraform`); MudRunner/SnowRunner (mud physics on separate data); BF3 (crater depth map) |
| 5 | **Hybrids:** 1→2 on one log; 2 + 3 within ~20 m; 2 for the view + 4 for feel | The sum of the parts | Sum | Sum | — | Dagor runs all three layers: strips into the clipmap, `deform_hmap` near the camera, and `terraform` |

Build sizes are ASSUMED: S is days, M is 1–2 weeks, L is 3–6 weeks, XL is 6+ weeks.
RDR2 and Forza Horizon publish no mechanism; no primary source was found for either (§7).

## 2. What every approach draws on

**The ground cannot hold a print.**
- One grid of 1025² samples serves the oracle, the collider and the render mesh (`src/terrain_grid.rs:12-31`). Its
  spacing is 1.465 m on the shipped 1500 m world (`:156-160`; DERIVED, 1500/1024).
- The fixture shoe is 0.72 m wide (`src/track/rig_geom.rs:970`, fixture) at a 0.130 m pitch (MEASURED off markers,
  `src/track/link_view.rs:1718`).
- So a rut is narrower than one grid cell. **Every approach adds a layer finer than the grid.** The questions are
  only whether that layer is view-only, and whether it is real geometry.

**The view.**

| Fact | Value | Source |
|---|---|---|
| Commander field | 45° vertical | `assets/tiger_1/tiger_1.tank.ron:399`, fixture; fallback `src/camera.rs:428` |
| Gunner sight | 25° vertical, 2.5× | `tiger_1.tank.ron:398`, fixture |
| Orbit radius | 5–18 m | `src/camera.rs:185-189` |
| Orbit pivot | 2.5 m above the turret ring | `src/camera.rs:175` |
| Reference screen | 1964 px tall | `src/settings.rs:180` |
| Sun elevation | 17° | `src/world.rs:38` |
| First shadow cascade | 1.66 cm texels | CLAIMED, `src/settings.rs:195` |

DERIVED from these, with the camera ASSUMED ~5 m above the ground:
- **Pixel size.** The commander view spends 4.0×10⁻⁴ rad per pixel: 4 mm at 10 m, 12 mm at 30 m, 40 mm at 100 m.
- **Rut width on screen.** A 0.72 m rut is 180 px wide at 10 m, 60 px at 30 m and 18 px at 100 m. Through the gunner
  sight it is still ~11 px wide at 300 m. **Prints are seen at range in the optic,** so a 100 m cull pops.
- **Shoe-pitch detail.** At 10 m one pitch spans ~15 px (view elevation 27°). At 30 m it spans ~2 px (9.5°). **Per-shoe
  detail dies at about 30 m;** the rut as a whole does not.
- **Depth cue 1, occlusion.** A rut of depth *d* hides *d*/tan(elevation) of its own floor. A 5 cm rut at 30 m hides
  0.30 m of its floor. A 15 cm rut hides 0.9 m, more than its whole width, so only the far wall shows.
- **Depth cue 2, sun shadow.** The 17° sun casts a shadow 3.27·*d* long into the rut: 16 cm for a 5 cm rut. That is
  ~10 texels of cascade 0, so real geometry would resolve it.
- Both cues are strong under this camera and sun. **A normal map gives neither of them; POM gives the first; a sun
  march or real geometry gives the second.**

**The client already knows where every shoe touches.**
- The track view conforms each belt to the oracle on every windowed client, at ~56 µs per tank per frame (CLAIMED,
  `src/track/view.rs:1-17`). The server never runs it.
- The per-shoe instancing places every shoe's pose each frame (`src/track/link_view.rs:1-110`).
- Tank poses reach every client; the disclosure filter covers only internal combat state (`src/net/disclosure.rs:38`).
  INFERRED from that: **view-only prints need no wire.** Each client derives them, which is ADR-0016's replicate-causes
  pattern.
- Cost of that choice: a late joiner misses prints laid before it joined. Issue 125 leaves "client-local or carried"
  open.

**The material's binding budget.** `TerrainBlend` says texture slots are the binding constraint
(`src/terrain_blend.rs:14-15`). wgpu's limits point at samplers instead:
- Metal grants 128 sampled textures but only **16 samplers per stage** (`wgpu-hal-29.0.4/src/metal/adapter.rs:785-794`).
- DX12 resource-binding tier 1 also caps samplers at 16 (`dx12/adapter.rs:834-837`).
- Bevy asks the adapter for its full limits by default (`bevy_render-0.19.1/src/renderer/mod.rs:300-310`).
- DERIVED inference: a trail texture fits if it **reuses `masks_sampler`** and declares `#[texture(n)]` with no
  `#[sampler]`. If it brings its own sampler, it competes for the scarce slot.

**Frame-cost context.** I found no declared frame budget in the repo, and issues 102 and 123 state none. They and the
code carry component costs, all on M4:
- the sun shadow pass is ~3.2 ms at 30 tanks, ~2.1 ms of it envelope + terrain (MEASURED,
  `.agents/scratch/shadow-caster-share-2026-08-13.md:6-9`);
- each cascade costs ~1.16 ms (MEASURED, `src/settings.rs:99-104`);
- the forest adds +1.08 ms of tick (MEASURED, issue 123);
- the four-pack terrain blend cost +0.03 ms (MEASURED according to
  `.agents/scratch/war-thunder-research-2026-10-26/rendering.md:63`; the run's provenance was not found).

**Engine facts that gate #3.**
- wgpu has no tessellation stage. naga 29's `ShaderStage` is Vertex, Task, Mesh, Fragment, Compute and four
  ray-tracing stages (`naga-29.0.4/src/ir/mod.rs:323-350`).
- Mesh shaders exist only as `EXPERIMENTAL_MESH_SHADER` (`wgpu-types-29.0.4/src/features.rs:1240-1264`). naga
  compiles them only for Vulkan; Metal and DX12 need passthrough shaders.
- Bevy 0.19's render crates never use mesh shaders. I grepped `bevy_render`, `bevy_core_pipeline` and `bevy_pbr` for
  them and got 0 matches (MEASURED).
- What Bevy 0.19 does offer:
  - vertex-shader texture fetch through `MaterialExtension::vertex_shader` and `prepass_vertex_shader`, the latter
    also driving shadows (`vendor/bevy_pbr-0.19.1-scalar-math/src/extended_material.rs:36,66`);
  - compute-written meshes (`bevy-0.19.1/examples/shader_advanced/compute_mesh.rs:1-8,68-73`);
  - depth-only render-to-texture (`examples/shader_advanced/render_depth_to_texture.rs:1-16`);
  - screen-space contact shadows: `ContactShadows` on the camera
    (`vendor/bevy_pbr-0.19.1-scalar-math/src/contact_shadows.rs`) with `contact_shadows_enabled` on the light
    (`bevy_light-0.19.1/src/directional_light.rs:96`).

## 3. Approach 1: decal ribbons or stamps

**Mechanism.**
- Each track side gets an emitter that appends a node whenever the conformed contact point has moved 0.5–1 m.
  Those are Dagor's defaults: segment 0.5–1.0 m, a new segment past a 30° turn, 256 nodes per track and 64 emitters,
  all CLAIMED (`prog/gameLibs/render/tireTracks.cpp:112-156`).
- The strip is one mesh per emitter. Its material is `ForwardDecalMaterial<StandardMaterial>`, with base colour and
  alpha, a normal map, and a `depth_map` read by `ParallaxMappingMethod::Occlusion`. Hull marks already ship this
  combination (`src/vfx/hull_marks.rs:181-195`).
- **The decal shader needs no quad.** `ForwardDecal` is a marker that installs the 1×1 quad
  (`decal/forward.rs:26-48,58-66`). The shader works per fragment from the mesh's own normal, tangent and UV
  (`decal/forward_decal.wgsl:22-52`). So a custom ribbon mesh can carry the material (DERIVED; never run on a non-quad
  mesh).
- What it buys: the decal reprojects onto whatever the depth prepass drew. That absorbs the terrain LOD rungs, which
  sit up to 5–50 cm off the exact surface (`src/terrain_lod.rs:142`), and it also hides prints behind tanks.
- **Clip the ribbon to the grid's own triangles** (`HeightGrid::height_at`, `src/terrain_grid.rs:362`). It then lies
  exactly on rung 0. The decal's UV correction, which assumes 1 UV per local unit, stays near zero. Bevy also warns
  of distortion at steep views (`forward.rs:59`), the normal case on ground.
- **Per-segment `ForwardDecal` entities are a trap.** 30 tanks × 2 sides × 100 m / 0.75 m is ≈8,000 transparent
  entities (DERIVED). Metal has no hardware multi-draw-indirect, so wgpu loops the draws on the CPU (the profiling
  memory). One ribbon per emitter is 60 draws.

**What the player sees.**
- Up close, a dark compacted band with grouser relief and fake depth inside it. POM shows inner depth, but only
  within the band's flat plane.
- **Grazing light.** The normal map shades the walls. **There is no self-shadow:** Bevy's POM marches no ray toward
  the light (`render/parallax_mapping.wgsl`, whole file). The band does receive cascade shadows, on its flat plane.
- **Silhouette:** none.
- **Pivot turns** are the weak point. In a neutral pivot each track's contact patch (length *L*, width *w*, centred
  *g*/2 from the hull centre) sweeps an annulus from *g*/2 − *w*/2 out to √((*g*/2 + *w*/2)² + (*L*/2)²).
  - With *g* = 2.6 m and *L* = 3.6 m (ASSUMED), that is a ring 0.94–2.5 m in radius (DERIVED).
  - A centre-line ribbon draws a 0.72 m band at radius 1.3 m instead.
  - Dagor patches this by blending an omnidirectional "drift" texture while moving sideways
    (`tireTracks.cpp:115,558-577`; `emit(... omnidirectional_tex_blend ...)` in
    `prog/gameLibs/publicInclude/render/tireTracks.h`).
  - WoT's strip "follows the real contact path" with no slip texture (round 12 §5, INFERRED there).
- **Persistence:** the ring length, 128–256 m per track at Dagor's defaults (CLAIMED). Longer costs proportionally
  more vertices and pixels.
- **Range:** Dagor culls at `maxVisibleDist` 100 m (CLAIMED, `tireTracks.cpp:144`). That pops in our optic (§2), so
  fade by mip instead of culling.

**Cost.**
- **GPU.** Every print pixel is re-shaded with full PBR and alpha-blended over the terrain. At an ASSUMED 2–10 % screen
  coverage, that adds the same fraction of the terrain's main-pass shading. The terrain pass has never been timed
  alone, so there is no ms figure. M4 and x86 scale alike.
- **CPU.** Bevy re-prepares a changed asset whole: an image re-uploads in full (`bevy_render-0.19.1/src/texture/gpu_image.rs:62-95`),
  and meshes take the same render-asset path (DERIVED). Fix: chunk each ribbon into ~32-node meshes so only the
  newest chunk re-uploads.
- **Memory.** Dagor's node is 32 B: a `Point3` and five `uint32` (`tireTracks.cpp:67-99`). 256 nodes × 60 emitters
  is ≈490 KB (DERIVED).
- **Network:** 0.

**Build in this repo.**
- Emitter: fed by the wrap's grounded stations. Same rule as WoT's lifted-track suppression (round 12 §5): no contact,
  no node.
- Ribbon mesher: `src/vfx/trail.rs` already rebuilds a smoke ribbon (precedent).
- Print maps: generated at startup from the footprint profile, the way `src/vfx/hull_marks/texture.rs` builds the hull
  marks.
- About a week (ASSUMED).

**Risks.**
- Alpha sorting against smoke billboards.
- Decals project onto the shoes themselves: the shoes are in the depth prepass and sit within the fade distance of the
  ground. Lay nodes only behind the last grounded shoe.
- The pivot ring (above).
- Throwaway: everything except the node log is replaced if #2 follows.

**Games.**
- **World of Tanks.** A per-chassis ribbon under each track, with a texture per ground kind picked by a three-point
  downward sensor. Albedo plus a `bump` normal; no depth or parallax channel evident. A fixed-length recycled strip;
  lifted tracks lay nothing (round 12 §1–5).
- **War Thunder.** Dagor ships `tire_tracks`. Its consumer is closed source, so WT's use is INFERRED.
- **Company of Heroes 2.** "Thousands of footprints, tread-marks, and tire-tracks build up on the map", persistent for
  the match. "Frame rates will continue to drop the longer a match goes on" (SemiAccurate, 15 Jul 2013: a press
  account, not Relic's).
- **Arma Reforger:** a community workshop mod adds persistent decal tire tracks. The base games' mechanism was not
  verified: the BI wiki returned 403.

## 4. Approach 2: a trail map read by the ground's shader

**Mechanism.**

*Stamps.*
- Each frame, every grounded shoe of the conformed belt is stamped. A stamp is that shoe's oriented footprint texture,
  scaled by the sister lane's depth.
- A shoe is re-stamped only once it has moved more than half a texel. A non-slipping shoe never moves on the ground,
  so straight driving stamps about one new shoe per pitch per side.
- Slip and pivots re-stamp every grounded shoe. That **smears the print by the sim's own slip and scrubs the full pivot
  annulus**, with no special case.

*Stores.*

| Store | Size | Coverage | Texel | Memory (with mips) |
|---|---|---|---|---|
| **L0**, near, toroidal | 2048² RG8 (R depth, min-blend; G scuff, max-blend) | 96 m around the camera | 4.7 cm, Dagor HIGH-class | 10.7 MiB |
| **World layer** | 4096² R8 (scuff and mean depth) | the whole 1500 m map | 0.37 m | 21.3 MiB |
| Optional **L1** for the optic | 2048² RG8 | 384 m | 19 cm | +10.7 MiB |
| **CPU stamp log**, spatially indexed | 16 B per stamp | the battle | — | 5–20 MB per battle (below) |

All DERIVED. wgpu has the Min and Max blend operations (`wgpu-types-29.0.4/src/render.rs:99-101`). When L0
recentres, its newly exposed strip is redrawn from the CPU log.

The log's size, DERIVED with an ASSUMED battle of 30 tanks × 2.7 km: one stamp per pitch per side is 1.25M stamps,
20 MB. One per 0.5 m is 5.2 MB.

*Stamping pass.* A render-world job draws instanced quads into the textures. `src/damage_cam/pen_cam/picture.rs` is
the in-repo precedent: a custom `RenderPipeline` writing an R32Float atlas through `RenderQueue`. The load is at most
~1,800 quads a frame at 30 tanks under full slip (DERIVED), which is negligible.

*Shader, in `assets/shaders/terrain_blend.wgsl`.*
1. Sample the world layer everywhere (1 tap): darken, roughen.
2. Inside L0: build a surface-gradient normal from the depth (Mikkelsen's surface gradient, cited in the hull-mark
   note), then run POM along the view ray.
3. **Offset `in.uv` and `in.world_position` before calling `pbr_input_from_standard_material`,** so the base pack and
   the three layers shift together.
4. March 4–8 taps toward the fixed sun for a soft self-shadow (Tatarchuk 2006 §"Soft Shadows Computation").
5. Use `textureSampleLevel` inside the loops. Bevy does this because DX12's FXC fails on gradient instructions in loops
   (`render/parallax_mapping.wgsl:15-24`). The existing shader already takes its gradients up front for the same family
   of reasons.
6. Fade POM to normal-only by ~50 m (ASSUMED). Beyond that the relief is ≤ 2 px (§2) and POM at grazing angles needs
   20+ steps (DERIVED: a 5 cm rut seen at 3° is 0.95 m, or 20 L0 texels).

*Free hook.* The shader already holds the painted mask weights (recesses, slopes, lowlands). Each pack can answer a
stamp differently, for example mud dark and wet, sand light and shallow, with no ground sensor. Compare WoT's
three-point ray sensor (round 12 §3). The sister lane picks the law.

**What the player sees.**
- Up to ~50 m, a sunk print: the far wall lit or shadowed by the 17° sun, the near floor occluded, grousers visible
  inside ~30 m.
- Beyond that, a darkened, roughened, normal-shaded band. It mip-filters, so it neither pops nor shimmers.
- **No silhouette.** POM writes no depth, so:
  - **Belts.** The tank's own belt rides the grid while its rut is drawn below. That is visible as hover where a
    ≥10 cm rut leaves the trailing idler. Fix, view-only: let the wrap conform read a CPU mirror of the depth, the
    way Dagor feeds its deformed height to foot IK only (`prog/gameLibs/anim/footLockerIKCtrl/footLockerIKCtrl.cpp:148`).
  - **Contact shadows and soft particles** see flat ground.
- **Persistence:** the whole battle at world-layer resolution, sharp near the camera. The world doesn't depend on
  where you have been.
- **Range:** the optic's 300 m view reads the 0.37 m world layer, ~5 px per texel at 300 m (DERIVED), so it looks soft.
  L1 fixes that.

**Cost.**
- **GPU.**
  - +1 tap per terrain pixel. On print pixels inside L0, ~20–26 more taps (normal, POM, sun march).
  - Anchor: Dagor's whole deformation feature, which draws vehicles into a depth map and then runs compute blur and
    edge passes, costs "~0.5-0.6ms on Xbox One S" (CLAIMED,
    `prog/daNetGame/render/world/worldRenderer.cpp:1290-1295`). Our stamps replace that per-frame vehicle render with
    tiny quads.
  - With an M4 ASSUMED ~3× an Xbox One S, that is DERIVED **~0.1–0.3 ms on M4**, and less on a mid-range x86 card
    (ASSUMED ~2× the M4).
  - On Apple's tile-based GPU, the stamping job is one extra pass, and pass boundaries cost (the shadow-perf memory).
    Batch it into one encoder per frame.
- **CPU:** log appends, plus redrawing the exposed strip when L0 recentres. Small; unmeasured.
- **Memory:** 32–43 MiB GPU and 5–20 MB CPU (above).
- **Network:** 0. If late joiners must see old prints, a carried coarse log works like `DestructionLedger`, an
  append-only replicon diff (`src/net/destruction.rs:1-12`). At one node per metre per side and 8 B a node, that is
  ~1.3 MB per battle, or 4.8 KB/s per client at 30 tanks moving 10 m/s (DERIVED).

**Build in this repo (M–L, ASSUMED 2–4 weeks).** The pieces are the stamp source (from the wrap), the log, the
render-world stamping job (pen-cam precedent), the L0 toroidal update, the world layer, and the WGSL. The material
gains one texture binding and shares a sampler.

**Risks.**
- POM stair-stepping at grazing angles: adaptive step count and a distance fade.
- The bugs of toroidal addressing.
- Complexity in the costliest material.
- The belt float above.
- Samplers, if one is added rather than shared.

**Games.**
- **War Thunder's engine, Dagor.** It draws the same tire-track strips *into the terrain clipmap*, with per-type write
  masks for albedo, normal, smoothness and AO (`tireTracks.cpp:539-544,825-864`, `render_strips(apply_clipmap_writemask)`).
  This is approach 2's store, filled from approach 1's node log.
- **Battlefield 3.** A virtual texture at 32 samples per metre, whose tile compositing (0.2–0.5 ms on PS3) carries
  "Dynamic updates: Destruction". Detail splats reach 500–1000 samples per metre out to 50–100 m. All CLAIMED, GDC 2012.
- **Batman: Arkham Origins, consoles.** Virtual displacement by relief mapping, independent of triangle density,
  over a heightmap that actors render into from below (CLAIMED, slides; paraphrased). Updates take "< 1.0ms GPU on PS3/360", in 2 MB. All CLAIMED,
  GDC 2014.
- **Unreal.** Runtime Virtual Texturing lets actors write into a runtime virtual texture that the landscape samples.
  `DrawMaterialToRenderTarget` is the stock render-target trail primitive (Epic docs).

## 5. Approach 3: real geometry, for the view only

**Mechanism.**
- wgpu has no tessellation (§2), so detail must exist as vertices.
- Replace the near terrain with a **geometry clipmap**: "nested regular grids centered about the viewer", whose heights
  are fetched in the vertex shader (Asirvatham & Hoppe, GPU Gems 2 ch. 2).
- Vertex height = the grid's exact anti-diagonal triangle interpolation, sampled from a 4 MiB R32F copy of
  `HeightGrid`, plus the L0 depth from #2. The base therefore stays the ONE surface, and only the rut offset is
  view-only.
- The same displacement goes in `prepass_vertex_shader`, which is what shadows use.
- Bevy's `compute_mesh` example is an alternative for building patches on the GPU. A static grid mesh with vertex
  texture fetch is simpler.
- Rings, DERIVED:

| Ring | Extent | Spacing | Triangles |
|---|---|---|---|
| L0 | 16 m | 4 cm | 320k |
| L1 | 32 m | 8 cm | 240k |
| L2 | 64 m | 16 cm | 240k |
| Total | | | ≈0.8M |

**The seam is the architecture change.**
- Render tiles are 187.5 m: 128 cells (`src/terrain_grid.rs:162-165`), drawn as RTIN rungs (`src/terrain_lod.rs:1-60`).
- A 64 m patch sits inside a tile, so the coarse tile must lose its overlap. Overlaying cannot work, because ruts lie
  *below* the coarse surface and would be hidden by it.
- Discarding terrain fragments forfeits hidden-surface removal on Apple GPUs for the largest opaque surface (CLAIMED
  by Apple's Metal guidance; not re-verified here).
- Workable options:
  - split render tiles to ~32 m (33² is a legal RTIN size) so the near ones can be hidden; or
  - make the clipmap the near-field renderer out to the first tile ring.
- Either one reopens the LOD certification for the near field.

**What the player sees.**
- Real walls and berms, silhouettes where a rut crosses a crest, true occlusion of small objects.
- Cascade 0 resolves the 16 cm shadow of a 5 cm rut (§2), and `ContactShadows` adds fine contact darkening.
- If the wrap conform reads the same layer, the belt sits in its own rut.
- From a camera ≥ ~5 m up (ASSUMED), most of the gain over #2 is on crests and close up.

**Cost.**
- ≈0.8M triangles each in the prepass, the main pass and cascade 0 (and part of cascade 1): 2.4–3.2M a frame
  (DERIVED). For scale, one tank's rung-0 shoes are 294,880 triangles (MEASURED count, `src/track/link_view.rs:106-107`),
  so each pass adds about 2.7 tanks of shoes.
- I can't convert this to ms from repo data. A cascade costing ~1.16 ms on M4 (MEASURED) says vertex load in shadow
  passes is not free on Apple; re-measure on x86.
- Plus all of #2's cost, because this approach consumes #2's L0.

**Build (L–XL).** A new near-field terrain renderer, its seam with the RTIN tiles, displaced prepass and shadow paths,
and certification for the near field. Plus #2.

**Risks.**
- **Doctrine cost:** the render mesh is no longer the grid's own samples near the camera (`src/terrain_grid.rs:22-25`),
  though the deviation is bounded by the rut depth.
- Micro-triangles at the ring edges.
- Precision of vertex texture fetch.
- Two terrain renderers to keep in agreement.

**Games.**
- **Batman: Arkham Origins, PC.** DX11 tessellation, captured as a minimum height field plus projected displacement.
  Meshes are diced at runtime, so dynamic shadows and ambient occlusion respond to the deformation (CLAIMED, slides;
  paraphrased).
- **Rise of the Tomb Raider.** Deferred deformation moves the cost from rendering the deformation heightmap to
  rebuilding the depression and elevation during the snow render pass (paraphrased). It uses hardware tessellation, pushes snow up
  at the trail's sides, and fills trails over time. Taken from the GPU Pro 7 chapter's catalogue abstract and table of
  contents; the chapter text is paywalled and the SIGGRAPH 2015 PDF returned 403, so finer details are unverified.
- **Dagor `deform_hmap`.**
  - Vehicles render "pressure" into an orthographic depth map from below (`prog/daNetGame/shaders/dynamic_render_deform_hmap_inc.dshl`).
    The map covers a camera-centred window: 2048² over 90 m on high, 1024² over 60 m otherwise
    (`worldRenderer.cpp:1302-1304`). It is reprojected as the camera moves (`deformHeightmap.cpp:272-315`).
  - Edges are raised by a Laplacian (`deform_hmap.dshl:394`).
  - The terrain vertex shader then does `worldPos.y += deformAmount` (`prog/gameLibs/heightmap/shaders/heightmap_common.dshl:139`)
    on a near grid subdivided by hardware tessellation or `lod0SubDiv` (`prog/gameLibs/heightmap/heightmapRenderer.cpp:161-192,325`).
    The pixel shader bends normals and darkens albedo (`prog/daNetGame/shaders/heightmap.dshl:478-480`).
  - **It is view-only and forgets.** Only the window persists. The deformed height feeds foot IK and nothing physical
    (`footLockerIKCtrl.cpp:148`).
  - It is disabled on low settings outside snow levels (`worldRenderer.cpp:1290-1298`).
- **Spintires: MudRunner.**
  - Terrain is "a simple 2d grid of vertices" with two textures fetched in the vertex shader.
  - Blocks are 16 m, with a 25² height texture per block and a 128² mud texture only on mud blocks: 12.5 cm per texel,
    DERIVED.
  - "CPU forms various primitives and draws them into Texture 2 (RT)" (Zagrebelnyy, Game Developer, 16 Nov 2017).
- **Unreal Virtual Heightfield Mesh.** A component that renders a heightfield mesh from a virtual-texture heightmap
  (experimental plugin, Epic API docs).

## 6. Approach 4: physical deformation

**Mechanism.**
- **The state.** An immutable base grid plus a sparse layer of depressions:
  - 8 m tiles at 10 cm (80² cells), u8-quantised to ~1 cm;
  - written in tick order by integer operations, so the layer is identical on every peer and in replays;
  - this is the banked shape in the terrain-world memory.
- **The writer.**
  - The belt sim already probes 3 lateral columns per station (`src/track/forces.rs:11,227`). That is
    97 × 3 × 2 = 582 probes per tank per tick on the fixture (DERIVED). Its grounded columns *are* the stamp
    positions.
  - Depth follows the sister lane's pressure × hardness law. It saturates as deepest-wins, Dagor's `DYN_MIN` mode
    (`prog/gameLibs/publicInclude/terraform/terraform.h:75`).
- **The reader.**
  - `BlockField` is "a min-fold of two EXACT analytic terms" (`src/track/oracle.rs:10`). It gains a third: a ray
    against the fine layer over a few cells.
  - `TrackField` rebuilds the whole field on a revision bump (`src/track/terrain.rs:17-23`). So the layer must sit
    behind interior mutability, not a revision.
- **The collider stays coarse.**
  - The 1.465 m Avian heightfield cannot hold a rut.
  - parry 0.27's `HeightField` has no in-place height setter, only constructors, `set_scale` and `set_cell_status`
    (`parry3d-0.27.0/src/shape/heightfield3.rs:84-551`). An edit is a new shape through `Collider::set_shape`
    (`avian3d-0.7.0/src/collision/collider/parry/mod.rs:551`).
  - The hull collider is a backstop. Its divergence from the oracle is bounded by the rut depth, against an ASSUMED
    ~0.4–0.5 m of ground clearance. Ballistics and line of sight ignore centimetres.
- **The view.**
  - Clients do not simulate (ADR-0037), so they need the layer only to draw it and to conform their belts.
  - **Option a:** clients derive it from replicated poses (0 B/s, approximate).
  - **Option b:** replicate the operations, for an exact match.
  - Either way a late joiner needs a snapshot. Dagor's `terraform` serialises run-lengths of non-zero cells
    (`prog/gameLibs/terraform/terraformComponent.cpp:40-101`) and warns above 128 KB (`:100`).
- **Showing it:** through #2 or #3, fed from this layer, so the drawn rut and the felt rut are one.

**What the player feels and sees.**
- Belts settle into ruts, and a tank following a rut is nudged by its walls under the outer columns.
- In soft ground, sinkage and rolling resistance come from the sister lane's law. Craters and berms are the same
  machinery at a larger scale (WT below).
- Looks are #2's or #3's.
- Persistence is permanent unless a refill law exists (RotTR and Batman refill over time).

**Cost.** All DERIVED from ASSUMED inputs.
- **Server CPU, reads:** 1.12M probes/s at 30 tanks and 64 Hz. The 64 Hz tick is from issue 102. With ASSUMED 50–100 ns
  extra per probe, that is 5.6–11 % of a core.
- **Server CPU, writes:** ~180 per tank per tick, at ASSUMED 20–50 ns each, is 0.7–1.7 % of a core.
- **Server RAM:** ~16 MB, from 30 × 2.7 km × 2 × 1 m of ground touched at 10 cm per cell. At Dagor's default 25 cm
  cells (`terraform.h:65`, CLAIMED) the same area is ~2.6 MB.
- **Wire:**
  - option a: 0 steady;
  - option b: ~1 KB/s per tank (12 B per 0.25 m per side at 10 m/s), ~29 KB/s per client at 30 tanks;
  - late-join snapshot: 16 MB raw, 1.6–5.4 MB at an ASSUMED 3–10× compression.
- **Client:** that of #2 or #3.

**Doctrine costs.**
- PRODUCT says terrain is "simulation-static **today**" and makes deformation "an open fork, settled by dedicated
  research at the world-detail milestone" (`.agents/PRODUCT.md:50`).
- ONE-SURFACE (`src/terrain_grid.rs:12-31`) must be restated: the oracle reads a finer surface than the collider, by at
  most the rut depth.
- ADR-0014 is satisfied: the layer is a world resource built at map load, not late per-entity state.
- ADR-0034: the law is generic, and the per-tank inputs (mass, contact area, footprint) come from tank data.
- Feedback loops need saturation: a pivoting tank must not dig without limit.

**Build (XL, ASSUMED 6+ weeks).** The layer, the oracle term, the write law (with the sister lane), determinism tests
across Mac and Linux, replication and the late-join snapshot, and the view through #2. Plus a PRODUCT decision record.

**Games.**
- **War Thunder, "Ground Breaking" (15 Oct 2021).**
  - "Soil, sand and snow are now not only authentically crushed under the tracks of tanks"; bombs of ~50 kg and up
    "form a pit"; craters and dozer-blade dugouts give cover; "lifting from the bottom of a deep hole to its rim can
    become a problem for vehicles".
  - In the engine this is `terraform`: 256² patches, 4 cells per metre by default, u8 altitude
    (`terraform.h:23-26,63-65`). Its cells are averaged into the physics heightmap through `setHeightmapHeightUnsafe`
    (`prog/gameLibs/terraform/terraform.cpp:270-290`).
  - Whether *track* crushing is physical is not stated publicly.
  - Earlier, WT 1.77 (20 Feb 2018) let players "see the suspension working on less uneven ground like ruts". Those are
    authored ruts, rendered by displacement mapping.
- **MudRunner / SnowRunner.** "Mud displacement is calculated entirely on GPU", while "Our physics engine performs
  collision and traction detection between the wheels of the vehicle and terrain" (80.lv, 4 Jan 2019). Mud physics
  (CPU) and mud rendering (GPU) "operate on completely different sets of data" (Game Developer, 2017). So their *felt*
  mud and *seen* mud are separate models.
- **Battlefield 3.** A "Destruction depth map" "controls crater depth around ie static models". The slides don't say
  whether collision changes.

## 7. Prior art the brief named that is not documented

- **Red Dead Redemption 2:** I found no Rockstar talk or paper on snow or mud deformation. What exists online is
  forum speculation and third-party recreations.
- **Forza Horizon:** I found no Playground talk on snow or mud tracks.
- **Arma 3:** the BI config reference returned 403, so the track-mark mechanism is unconfirmed. A Reforger community
  mod adds "persistent, physically accurate tire tracks" with decals.
- **World of Tanks:** answered from client sources by round 12 (§3). No depth channel.
- **Company of Heroes:** persistence is a press account (§3). The mechanism is not public.

## 8. Hybrids

- **1 → 2, sharing one log.** Dagor does exactly this: the same `tire_tracks` nodes render either as projective decals
  or into the terrain clipmap (`tireTracks.cpp:825-885`). Starting with #1 is worth it only if the log's schema is
  #2's.
- **2 + 3 within ~20 m.** POM beyond. This spends geometry only where silhouettes and cast shadows can be seen, and
  still needs the seam work of #3.
- **2 for looks + 4 for feel.** The physics layer at 10 cm, with grouser detail at 4.7 cm added on top for the view
  only. The drawn rut, the felt rut and the belt view then share one coarse truth.
- **Dagor as the reference hybrid.** Texture strips everywhere, view geometry in a 60–90 m window, and physics only at
  crater and dugout scale.

## 9. Recommendation

**Build approach 2, driven by per-shoe stamps from the client's conformed belt and stored in a CPU log. Skip
ribbons. Treat #3 and #4 as later, separate decisions.**

Why #2. Under a 17° sun and a camera 5–18 m out, depth reads through two cues: the rut hiding its own near floor, and
its far wall taking the sun's shadow (§2). #2 delivers both inside 50 m and degrades smoothly beyond. Its other
merits:
- Stamping each grounded shoe gets pivot scrub and slip smear right **from the sim's own slip**, where ribbons need
  Dagor's drift-texture patch.
- It persists for the whole battle in a fixed ~32–43 MiB.
- It costs 0 wire, one texture and no new sampler.
- Its GPU estimate, DERIVED from Dagor's claim, is ~0.1–0.3 ms on M4. That is small next to the measured 1.16 ms per
  cascade.
- It lives entirely in view code.
- Each ground pack can answer the stamp its own way, through the mask weights the shader already holds.

Why not #1 first. Ribbons can't self-shadow without custom WGSL. They misdraw pivots and need distance culling that
pops in the optic. Once those are fixed, the cost approaches #2's without #2's persistence. Only the node log would
survive.

Why not #3 now. It rebuilds the near terrain for silhouettes, which from a camera ≥ ~5 m up mostly show on crests.
Reopen it if captures of #2 show the flat crest or the belt float is the remaining tell. If so, first try the
view-only belt sink, which is cheap.

Why not #4 now. It is XL. At print scale (centimetres on firm ground) the felt effect is small unless the sister
lane's soil law makes ruts deep. The payoff of physical deformation sits at crater and berm scale (cover, line of
sight, getting stuck), which is PRODUCT's open fork. Settle it there, and **shape #2's stamp log so it can become that
fork's integer operation log**: tick, side, pose, footprint id, quantised depth.

**Steps.**
1. Stamp log, world layer, and darkening plus normal (M).
2. Add L0 with POM and the sun march; capture under grazing light and through a pivot.
3. Optional: L1 for the optic; the view-only belt sink.
4. Decide: carried log for late joiners (~1.3 MB per battle) or accept client-local prints.

That last step is issue 125's open question. **Decision taken here: client-local for now.**

## 10. Sources

**Repo.**
- `src/terrain_grid.rs:12-31,156-165,362`; `src/terrain_blend.rs:14-15,111,153-174`;
  `assets/shaders/terrain_blend.wgsl`; `src/terrain_lod.rs:1-60,142`.
- `src/track/view.rs:1-17`; `src/track/link_view.rs:1-110,106-107,1718`; `src/track/rig_geom.rs:964-970`;
  `src/track/forces.rs:11,227,291,626`; `src/track/oracle.rs:10,220,271`; `src/track/terrain.rs:17-23`.
- `src/camera.rs:175,185-189,329-330,428`; `src/world.rs:38`; `src/settings.rs:99-104,180-195`;
  `assets/tiger_1/tiger_1.tank.ron:398-399`; `src/spec.rs:524-541`.
- `src/hull_marks.rs`; `src/vfx/hull_marks.rs:1-35,181-195`; `src/vfx/hull_marks/texture.rs`; `src/vfx/trail.rs`;
  `src/damage_cam/pen_cam/picture.rs:1-30,49`.
- `src/net/destruction.rs:1-12`; `src/net/disclosure.rs:38`.
- `.agents/PRODUCT.md:50`; ADR-0014, ADR-0037, ADR-0039.
- `.agents/scratch/impact-feedback-2026-10-28/hull-mark-rendering.md`;
  `.agents/scratch/impact-feedback-2026-10-28/wot-scout-round12-track-traces.md`;
  `.agents/scratch/shadow-caster-share-2026-08-13.md:6-9`;
  `.agents/scratch/war-thunder-research-2026-10-26/rendering.md:63`.
- Issues 102, 123 and 125 (`gh issue view`).

**Bevy, wgpu, avian and parry** (cargo registry and `vendor/`).
- `bevy_pbr`:
  - `decal/forward.rs:26-66,100-147`;
  - `decal/forward_decal.wgsl:22-52`;
  - `render/parallax_mapping.wgsl`;
  - `parallax.rs`;
  - `pbr_material.rs:736-769`;
  - `extended_material.rs:36-105`;
  - `contact_shadows.rs`.
- `bevy_light-0.19.1`: `src/cluster/mod.rs:213-228` (clustered decals: not on macOS); `src/directional_light.rs:96`.
- `bevy_render-0.19.1`: `src/texture/gpu_image.rs:62-95`; `src/renderer/mod.rs:300-310`;
  `src/render_resource/bind_group.rs:128-170`.
- `bevy-0.19.1` examples: `examples/shader_advanced/{compute_mesh,render_depth_to_texture}.rs`;
  `examples/3d/{decal,parallax_mapping,render_to_texture,contact_shadows}.rs`.
- wgpu and naga:
  - `naga-29.0.4/src/ir/mod.rs:323-350`;
  - `wgpu-types-29.0.4/src/features.rs:1240-1264`;
  - `wgpu-types-29.0.4/src/render.rs:99-101`;
  - `wgpu-hal-29.0.4/src/metal/adapter.rs:785-794`;
  - `wgpu-hal-29.0.4/src/dx12/adapter.rs:834-837`.
- Physics: `parry3d-0.27.0/src/shape/heightfield3.rs:84-551`;
  `avian3d-0.7.0/src/collision/collider/parry/mod.rs:541-551,1114-1128`.

**Dagor [D].**
- Tire tracks:
  - `prog/gameLibs/render/tireTracks.cpp:67-160,539-577,825-885`;
  - `prog/gameLibs/publicInclude/render/tireTracks.h`;
  - `prog/gameLibs/render/shaders/tires_common.dshl`.
- Deformable heightmap:
  - `prog/daNetGame/render/world/deformHeightmap.{h,cpp}`;
  - `prog/daNetGame/render/world/worldRenderer.cpp:1290-1318`;
  - `prog/daNetGame/shaders/{deform_hmap.dshl:140-250,394, deform_hmap_common.dshl, dynamic_render_deform_hmap_inc.dshl}`.
- Terrain renderer: `prog/gameLibs/heightmap/shaders/heightmap_common.dshl:130-139`;
  `prog/daNetGame/shaders/heightmap.dshl:477-480`; `prog/gameLibs/heightmap/heightmapRenderer.cpp:161-192,325`.
- Foot IK: `prog/gameLibs/anim/footLockerIKCtrl/footLockerIKCtrl.cpp:148`.
- Terraform: `prog/gameLibs/publicInclude/terraform/{terraform.h:23-26,63-75, terraformComponent.h}`;
  `prog/gameLibs/terraform/{terraform.cpp:270-290, terraformComponent.cpp:40-101}`;
  `prog/gameLibs/publicInclude/heightmap/heightmapPhysHandler.h:282`.

**Web.**
- War Thunder, *Ground Breaking: self-entrenching and bomb craters*, 15 Oct 2021:
  https://warthunder.com/en/news/7378-development-ground-breaking-self-entrenching-and-bomb-craters-en
- War Thunder, *Dagor Engine 5.0: new landscape creation technology*, 20 Feb 2018:
  https://warthunder.com/en/news/5341-development-dagor-engine-5-0-new-landscape-creation-technology-en
- Barré-Brisebois, *Deformable Snow Rendering in Batman: Arkham Origins*, GDC 2014:
  https://gdcvault.com/play/1020177/Deformable-Snow-Rendering-in-Batman. Slide quotes are from the author's upload:
  https://www.slideshare.net/colinbb/gdc2014-deformable-snow-rendering-in-batman-arkham-origins
- Michels and Sikachev, *Deferred Snow Deformation in Rise of the Tomb Raider*, GPU Pro 7 (CRC Press 2016),
  pp. 3–16:
  - https://gameenginegems.com/gemsdb/article.php?id=1314
  - https://www.taylorfrancis.com/books/9780203705179/chapters/10.1201/b22483-18
  - SIGGRAPH 2015 talk PDF, which returned 403:
    https://history.siggraph.org/wp-content/uploads/2022/10/2015-Talks-Michels_Labs-R_D-Rendering-Techniques-in-Rise-of-the-Tomb-Raider.pdf
- Zagrebelnyy, *Mud and Water of Spintires: MudRunner*, Game Developer, 16 Nov 2017:
  https://www.gamedeveloper.com/programming/mud-and-water-of-spintires-mudrunner
- 80.lv, *MudRunner: porting a successful title to console*, 4 Jan 2019:
  https://80.lv/articles/mudrunner-porting-a-successful-title-to-console
- Widmark, *Terrain in Battlefield 3*, GDC 2012:
  https://media.contentapi.ea.com/content/dam/eacom/frostbite/files/gdc12-terrain-in-battlefield3.pdf
- SemiAccurate, *Company of Heroes 2: Setting the record straight*, 15 Jul 2013:
  https://www.semiaccurate.com/2013/07/15/company-of-heroes-2-relic-performance/
- Epic:
  - Runtime Virtual Texturing:
    https://dev.epicgames.com/documentation/en-us/unreal-engine/runtime-virtual-texturing-in-unreal-engine
  - Virtual Heightfield Mesh: https://dev.epicgames.com/documentation/unreal-engine/API/Plugins/VirtualHeightfieldMesh
  - `DrawMaterialToRenderTarget`:
    https://dev.epicgames.com/documentation/unreal-engine/API/Runtime/Engine/UKismetRenderingLibrary/DrawMaterialToRenderTarget
- Arma Reforger community mod *Vehicle Track System*:
  https://reforger.armaplatform.com/workshop/668AD0DF0171A2B7-VehicleTrackSystem
- Asirvatham and Hoppe, GPU Gems 2 ch. 2:
  https://developer.nvidia.com/gpugems/gpugems2/part-i-geometric-complexity/chapter-2-terrain-rendering-using-gpu-based-geometry
- Tatarchuk, *Practical Parallax Occlusion Mapping*, 2006, "Soft Shadows Computation":
  https://www.advances.realtimerendering.com/s2006/Tatarchuk-POM.pdf
- Mikkelsen, *Bump Mapping Unparametrized Surfaces on the GPU*, 2010: https://mmikk.github.io/papers3d/mm_sfgrad_bump.pdf
