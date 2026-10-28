# Hull mark rendering: making shell marks read as metal

2026-10-28 · research lane · **Subject: the visual layer of the game's damage model.** These are the cosmetic marks a
round leaves on a hull, view-only per ADR-0014. The question: how War Thunder and World of Tanks make a penetration
hole, a non-penetration dent, a ricochet gouge and scorch read as damaged metal with depth, and what Bevy 0.19 offers.

**Labels.** CLAIMED: a number the cited source states (for code, its default literal; a game may override it at
runtime). DERIVED: computed here from cited values. MEASURED: counted here today through public APIs. Nothing was
built or run. **Pins.** [D] = `GaijinEntertainment/DagorEngine` @ `2baf194` (2026-10-16), paths from its root.
[B] = `bevyengine/bevy` tag `v0.19.0`; the vendored `vendor/bevy_pbr-0.19.1-scalar-math/src/decal/forward.rs` diffs
identical to [B]'s.

## 1. Recommendation

**Use a procedural shape, not a photo atlas.** Describe each mark as an analytic shape on the plate and derive every
layer from the signed distance *d* to it: a dark core inside (*d* < 0), then a thin band of tilted normals (the lip),
then a bright bare-steel ring whose outer edge is torn by noise, then a soot halo that also erases the camo. War
Thunder's vehicle shader works exactly this way, with no hole textures at all (§2.1). Calibre is the shape's radius,
impact angle is its elongation, and the mark's kind picks which layers exist:

| kind | shape | layers |
|---|---|---|
| penetration | ellipse (or the shot's capsule) | core, lip, bare ring; soot only for the explosive family (WT gates it, §2.1) |
| non-penetration dent | ellipse, no core | shallow bowl normal, crazed-paint ring, bare spot at the centre for large calibres |
| ricochet | capsule (segment + radius) along the bounce | trough normal across the short axis, bare streak, tapering tail, anisotropic sheen |
| scorch | disc | soot and camo removal only (WT's separate burn-mark primitive) |

- **Angle.** A cylinder of radius *r* meeting a plate at θ from the normal cuts an ellipse *r* × *r*/cos θ, long axis
  along the shot's in-plane direction: 2.0× at 60°, 3.9× at 75° (DERIVED). Clamp it near 4×. WT says its scar shape
  "depends on the angle" (§2.3).
- **Bevy, step 1, with no custom WGSL.** Spawn one `ForwardDecal` per mark as a child of the hit part, using
  `ForwardDecalMaterial<StandardMaterial>`. Its base colour (+alpha), normal, metallic-roughness and depth (parallax)
  maps are **generated at startup** from the analytic profile (§3), a few noise seeds per kind, so the atlas is code,
  not art. Calibre sets the uniform scale, angle sets the X/Z stretch and yaw, and each mark gets a random roll
  (Gaijin's variation trick, §2.2). Materials are shared per (kind, seed), so per-mark variety rides the transform.
  Set `depth_fade_factor` to a few cm, not the 8 m default (§4a).
- **Step 2, War Thunder's approach, only if step 1's limits bite.** Step 1 marks are lit with the flat quad's normal
  (off on cast curves), and an overlay cannot cut a real hole. Step 2 moves the same distance field into the hull's own
  shader as a `MaterialExtension` on `StandardMaterial`, with marks in a per-tank storage buffer (§4c).
- **Clustered decals are not a route today:** feature off in our build, and documented as unusable on macOS (§4b).
  Stop-gap art: TextureCan's CC0 bullet-hole decal (§5), which adapts to calibre only by scaling, not to angle.

## 2. What the two games do

### 2.1 War Thunder: vehicle damage is a procedural distance field in the vehicle shader [D]
- **The library.** "Decals for dynmodels. Includes planar decals (permanent decorations) and dynamic (such as damage
  holes)" (`prog/gameLibs/publicInclude/render/decals/dynamicDecals.h:17`). Damage arrives as per-instance primitives,
  not textures: `CapsulePrim` (bottom, radius, top, exploded/material bits), `SpherePrim` and `BurnMarkPrim`
  (`dynamicDecals.h:52-68`), packed per instance (`prog/gameLibs/render/decals/dynamicDecals.cpp:503-545`).
  **There is no hole atlas.** The vehicle path samples one tiling noise texture, `dyn_decals_noise` (`dynamicDecals.cpp:177-178`).
- The shader is `prog/gameLibs/render/shaders/decals/dynamic_decals_inc.dshl` (`inc`), with constants in
  `prog/gameLibs/publicInclude/render/decals/dynamic_decals_params.hlsli` (`hlsli`):
  - **Shape.** A capsule's distance is the distance from the fragment's **model-space** position to the segment, minus
    the radius (`inc:236-240`). Caps are 10 capsules and 15 spheres (`inc:82-83`, CLAIMED). Laid along the shot path,
    a capsule turns angle into an elongated hole for free (DERIVED), and model space makes marks ride the moving part.
  - **Edge noise.** Two noise taps perturb *d* by *r*·(noise − 0.5), clamped to ±*r* (`inc:313-320`). Steel's octave
    weights are `(0.8, 0.7, 0.3, 1.0)` (`hlsli:21`, CLAIMED). This noise is what makes the edge look torn, not drawn.
  - **Zones.** The bare band is `alphaIn = 1 − saturate(d·20)⁴` and the soot band is `alphaOut = 1 − saturate(d_rim·3)⁴`
    (`inc:433-434`, `hlsli:6-14`). The soot rim sits 0.2·*r*·noise further out (`inc:331`, `hlsli:35`); the rim noise
    weights sum to 3.3 (`hlsli:29`), so the offset is ≤ 0.66 *r*. That makes the bare band about 5 cm and the soot
    falloff about 33 cm (all DERIVED, assuming model units are metres). The soot rim is enabled only for capsules
    flagged "exploded" (`inc:402-408`). Burn marks are rim-only spheres: they neither clip nor bare the metal (`inc:423-429`).
  - **Hole.** `clip(clipDist)` cuts a real hole through the surface (`inc:441-446`), hence "you can see the flame
    through any holes" (§2.3).
  - **Lip.** Inside the bare band, the normal leans 50% toward the hole axis, and the lean flips on back faces
    (`inc:448-454`). It reads as metal pushed in around the hole (DERIVED reading).
  - **Materials and camo.** In the bare band, metalness is 0.6 and smoothness 0.3 (`hlsli:38-39`, CLAIMED). Albedo goes
    to a cutting grey of 0.4, then to the noise texture's RGB at a box-projected model UV (`inc:146-149,470-478`,
    `hlsli:16`). Soot is colour 0.01 at 0.8 opacity, and **the camouflage weight is forced to 0 across the rim**
    (`inc:456-462`, `hlsli:17`). All of these are tunable from a config block (`dynamicDecals.cpp:192-219`).
- **What is not published.** `bullet_mark_noise` and `bullet_diff_mark_params` are set (`dynamicDecals.cpp:212-229`)
  but no published shader reads them, so WT's game-side vehicle shaders are not in the repo. That WT's tanks use this
  path is INFERRED from the `_ground_model` functions (`inc:264,288`) and `TANK_DECAL_SMOOTHNESS` in the sibling
  planar (emblem) shader, which is colour-only and ≤ 4 per model (`decals.dshl:34,87-88`, `planar_decals_params.hlsli:6`).

### 2.2 Gaijin's textured hole decals: daNetGame titles, world surfaces [D]
- `prog/daNetGameLibs/billboard_decals/shaders/billboard_decals.dshl`: **height lives in the diffuse alpha**, read by
  parallax occlusion mapping at 5 steps and strength 0.77 (`:118,254-257,291-299`, CLAIMED); the deepest heights turn
  see-through via `1 − saturate(a·4 − 3)⁸` (`:114-116,301-303`); a second array packs smoothness R, normal AG,
  metalness B (`:306-319`); each hole gets a random roll hashed from its id (`:55-57`).
- Projective decals fade the normal earlier than the colour on turning surfaces (colour over cosine −0.5 to 0, normal
  over 0.5 to 0.866; CLAIMED in `prog/daNetGame/shaders/rendinst_projective_decal.dshl:27-29,158-166,181`), a cheap
  guard against smeared normals on curved parts.

### 2.3 War Thunder, first-party posts
- *Dagor Engine 6.0: Visual improvements, effects and DLSS* (16 Nov 2020,
  https://warthunder.com/en/news/6946-development-dagor-engine-6-0-visual-improvements-effects-and-dlss-en):
  "Physically correct armour scars appear on ground vehicles in places where shells hit, now their shape depends on
  the angle of the shell's collision with the vehicle's armour"; "If armour is penetrated, holes of different
  diameters appear on the armour, depending on the calibre of the projectile"; "If a vehicle is on fire, you can see
  the flame through any holes."
- *New visual effects in the Drone Age update* (2 Sep 2022,
  https://warthunder.com/en/news/7843-development-new-visual-effects-in-the-drone-age-update-en): "smoke in the impact
  point, followed by the flame whirling out of the hole if the armor was penetrated at that point."

### 2.4 World of Tanks: public first-party material only
- **WoT Blitz** (Wargaming's mobile title, a separate client), *Graphics Improvements in Update 7.0*
  (https://na.wotblitz.com/en/news/updates/graphics-7-0/, undated on the page), all CLAIMED: "four sizes of hit marks
  that correspond to small (20–30 mm), medium (37–57 mm), main (75–107 mm), and large (102–183 mm) calibers"; "two
  images each" per shell family (AP, APCR, HEAT, HE); "Non-penetrating hits and ricochets will also leave marks. Their
  look doesn't depend on the shell type"; no marks on suspension, muzzle or attachments; fade from 100 to 105 m
  (arcade) and 400 to 420 m (sniper). A small, bucketed texture atlas, not a procedural system.
- **WoT PC, *Release Notes 2.4*** (https://worldoftanks.com/en/content/docs/release_notes/release-notes-2-4/): "New hit
  decals have been added." Known issue: "Hit marks are occasionally cut off on curved parts of a vehicle", the signature
  of a bounded planar projection (INFERRED). The 2.4 feature page (19 Aug 2026,
  https://worldoftanks.eu/en/news/general-news/2-4-upgraded-vfx-sfx/) lists "armor deformation" among distinct
  effects, with no method given.
- WoT internals belong to the other lane (`.agents/scratch/destruction-2026-10-26/wot-scout-round9.md` §2).

## 3. The layered-look recipe
From outside in. Widths are starting points to tune, not sourced figures. Each band's falloff is `1 − saturate(d/w)⁴`,
a flat interior with a soft edge (the form of `inc:433-438`).

1. **Soot halo** (explosive family and scorch). Pull albedo toward about 0.01, raise roughness, fade the camo to 0, and
   give it a ragged outer edge. Dagor's values are in §2.1.
2. **Chipped paint opening onto bare steel.** Remove the camo. Albedo becomes a bright neutral grey with low-frequency
   variation; metallic goes high and roughness mid. The ring's outer boundary is the noise-perturbed iso-line (two
   octaves, amplitude ∝ *r*), and that boundary is what sells it as torn.
3. **Lip, from a height profile.** A dent is a bowl, `h = −D(1 − (ρ/R)²)²` for ρ < R; a petalled exit adds a ring,
   `A·exp(−((ρ − R)/w)²)`. The normal comes from the gradient of *h*, so no authored normal map is needed. In-shader,
   Mikkelsen's surface-gradient bump mapping uses only "the surface position, the height value and the original
   normal" (*Bump Mapping Unparametrized Surfaces on the GPU*, 2010, https://mmikk.github.io/papers3d/mm_sfgrad_bump.pdf).
   For the baked route, finite-difference *h* into a tangent-space normal map at generation time.
4. **Core.** Near-black, albedo about 0.02, and it must show depth. Cheapest: parallax occlusion mapping on *h*
   (Tatarchuk, *Practical Parallax Occlusion Mapping for Highly Detailed Surface Rendering*, ATI 2006,
   https://www.advances.realtimerendering.com/s2006/Tatarchuk-POM.pdf), built into Bevy (§4c). A true bore (lit inner
   wall, dark far wall) is a ray-against-cylinder test in the fragment shader, the interior-mapping idea: "raycasting
   in the pixel shader is used to calculate the positions of floors and walls … without the need to actually model or
   store this interior" (van Dongen, *Interior Mapping*, CGI 2008,
   https://www.proun-game.com/Oogst3D/CODING/InteriorMapping/InteriorMapping.pdf). See also GPU Gems 2 ch. 8 (Donnelly,
   https://developer.nvidia.com/gpugems/gpugems2/part-i-geometric-complexity/chapter-8-pixel-displacement-mapping-distance-functions).
5. **Ricochet gouge.** Use a capsule or segment distance field ("Capsule / Line", https://iquilezles.org/articles/distfunctions/;
   2D `sdSegment`, https://iquilezles.org/articles/distfunctions2d/). The radius tapers from wide at the impact to thin
   at the exit, with a trough height profile across the short axis. Inside goes bare steel with no soot and noise
   stretched along the axis; align the anisotropic highlight with the axis.
6. **Dent.** Steps 2–3 without the core: paint crazed in a ring, with bare steel only at the centre.

Engine precedents: id Tech 6 samples all decals from one atlas with explicit gradients (`tex2Dgrad`), inside the
clustered loop and before lights (Sousa and Geffroy, *The Devil is in the Details*, SIGGRAPH 2016,
https://advances.realtimerendering.com/s2016/Siggraph2016_idTech6.pdf, slides 12 and 21). For normal blending, Bevy's
clustered decals use the Whiteout operator (*Blending in Detail*, https://blog.selfshadow.com/publications/blending-in-detail/).

## 4. Bevy 0.19's path [B]

### 4a. `ForwardDecal`: recommended for step 1
- **Types.** `bevy::pbr::decal::{ForwardDecal, ForwardDecalMaterial, ForwardDecalMaterialExt}`. `ForwardDecalPlugin`
  ships inside `PbrPlugin` (`crates/bevy_pbr/src/lib.rs:243`). Example: `examples/3d/decal.rs`.
- **Geometry.** A 1×1 quad in local XZ with normal +Y and generated tangents (`decal/forward.rs:32-39`). The shader
  re-projects UV onto the depth-prepass surface and fades alpha with the quad-to-surface distance along the normal
  (`decal/forward_decal.wgsl:22-52`). It divides the tangent by the transform's scale (`:24-25`), so non-uniform X/Z
  scale is the intended stretch control (DERIVED).
- **What it writes.** Everything `StandardMaterial` computes (base colour and alpha, normal map, metallic-roughness,
  emissive, `depth_map` parallax), because `pbr.wgsl` swaps only `world_position` and `uv`, then runs the normal PBR
  path (`render/pbr.wgsl:59-66,101-103`). But it is an **overlay**: `AlphaMode::Blend` is forced, shadows are off and
  the depth test is `Always` (`forward.rs:113-141`). It is lit with the quad's normal, not the hull's (DERIVED).
- **Requirements and limits.** The camera needs `DepthPrepass` (`forward.rs:58`), which Overmatch's camera has
  (`src/camera.rs:329`). Wasm needs WebGPU with MSAA off (`forward.rs:61`). There is no engine cap on count; each
  mark is one transparent draw (DERIVED). **`depth_fade_factor` defaults to 8.0 m** (`forward.rs:144-150`, CLAIMED).
  With the depth test off, a mark on a turret's far side would show through at alpha 1 − Δ/8 (DERIVED,
  `forward_decal.wgsl:49`), so set it to centimetres.
- **Distance fade, as Blitz does it.** `VisibilityRange` (`crates/bevy_camera/src/visibility/range.rs:80`) works here,
  because its dither runs before the decal code (`pbr.wgsl:55-57`).

### 4b. `ClusteredDecal`: the best blend, but not available to us
- **Type.** `bevy::light::ClusteredDecal` (`crates/bevy_light/src/cluster/mod.rs:213-283`) is a 1×1×1 box projecting
  along −Z. Fields: `base_color_texture`, `normal_map_texture` (OpenGL green-up), `metallic_roughness_texture`
  (B = metal, G = rough), `emissive_texture`, `tag: u32`. It blends into the surface's own `PbrInput` before lighting:
  OVER on colour, metallic and roughness mixed by decal alpha, Whiteout normals (only with `VERTEX_TANGENTS`), emissive
  added (`decal/clustered.wgsl:170-265`). Custom shading goes through `tag` and the iterator API (`clustered.wgsl:1-27`).
  Examples: `examples/3d/clustered_decals.rs`, `examples/3d/clustered_decal_maps.rs`.
- **Limits.** 1024 per view with partially-bound binding arrays, else 8 (`decal/clustered.rs:566-582`, CLAIMED). It
  samples mip 0 only (`clustered.wgsl:198-203`), so expect shimmer at range (DERIVED). All images share one sampler
  (`cluster/mod.rs:234-240`).
- **Blockers.** It needs the Cargo feature `pbr_clustered_decals` (`clustered.rs:553-561`), which is in neither Bevy's
  `default`/`3d` sets (root `Cargo.toml:134,140,617`) nor Overmatch's `Cargo.toml:17` (DERIVED: no match). Its docs say
  it "can't be used on WebGL 2, WebGPU, macOS, or iOS" (`cluster/mod.rs:225-228`). The gate itself is a capability
  check, so macOS availability is CLAIMED, not measured.

### 4c. Procedural hull extension: War Thunder's approach, step 2
- **Material.** `ExtendedMaterial<StandardMaterial, HullMarks>` with `impl MaterialExtension`. The fragment calls
  `pbr_input_from_standard_material`, edits `base_color`, `metallic`, `perceptual_roughness` and `N`, then calls
  `apply_pbr_lighting`, in forward and deferred (`examples/shader/extended_material.rs`,
  `assets/shaders/extended_material.wgsl:31-65`). Overmatch precedent: `TerrainMaterial` in `src/terrain_blend.rs`.
- **Per-tank mark list.** A `#[storage(n, read_only)]` `ShaderBuffer` (`examples/shader/storage_buffer.rs`), indexed
  by a per-entity `MeshTag(u32)` (`crates/bevy_mesh/src/components.rs:153-156`) and read via `mesh_functions::get_tag`
  (`render/mesh_functions.wgsl:165`). `get_local_from_world` (`:25`) moves the fragment into part-local space, so
  marks ride the turret the way Dagor's model-space primitives do.
- **Built-ins usable on any path.** Parallax: `StandardMaterial::depth_map` with
  `ParallaxMappingMethod::{Occlusion, Relief}` (`pbr_material.rs:739-769`, `parallax.rs:16-39`, example
  `examples/3d/parallax_mapping.rs`). Anisotropy: `anisotropy_strength`/`anisotropy_rotation`, where rotation 0
  follows the mesh tangent, so a scratch quad with its X axis along the gouge needs no per-mark material
  (`pbr_material.rs:560-602`, DERIVED); `anisotropy_texture` needs feature `pbr_anisotropy_texture` (`:631-633`).
  Back faces are culled by default (`pbr_material.rs:926`), so a real cut shows the world behind the tank unless an
  interior exists.

## 5. Texture sources (licence as stated by each site)
- **TextureCan**: "All the PBR textures, graphics and photos offered by TextureCan.com are under the Creative Commons
  CC0 1.0 Universe License"; "No credit or attribution is required"; redistribution allowed (https://www.texturecan.com/terms/).
  *Imperfection and Decal of Bullet Holes* (Imperfection 0003, https://www.texturecan.com/details/217/) and *Decal of
  Punch Through Holes* (Imperfection 0004, https://www.texturecan.com/details/218/, "These holes go inwards"). Packs
  "usually include" base colour, normal, roughness, displacement, metallic and AO maps, at 1K/2K/4K plus SBSAR
  (CLAIMED). These are the only CC0 bullet-hole sets with full PBR maps found.
- **OpenGameArt**: *Bullet Decal – bullet_hole_0.png* by musdasch, CC0, one 512×512 PNG, no normal map (CLAIMED,
  https://opengameart.org/content/bullet-decal-bullethole0png).
- **ambientCG** ("All ambientCG assets are provided under the Creative Commons CC0 1.0 Universal License", no credit
  needed, https://docs.ambientcg.com/license/): no bullet-hole decal (API search `bullet` returns 0; its 127 `Decal`
  assets are road lines, leaks, manholes, asphalt damage, gum; MEASURED). Layers: `PaintedMetal001`–`017`.
- **Poly Haven** ("Our assets are all licensed as CC0"; "You do not need to give credit", https://polyhaven.com/license):
  869 textures, none tagged bullet or decal (MEASURED via `api.polyhaven.com/assets?t=textures`). Layers: `metal_plate`,
  `blue_metal_plate`, `painted_metal_shutter`, `rusty_painted_metal`, `worn_corrugated_iron`.
- None adapts to calibre or angle. Use them as **layer textures** (bare steel, chipped edge, soot) inside the
  procedural zones, the way Dagor samples one noise texture for steel colour.

## 6. Open questions
1. Does a `ForwardDecal`'s UV re-projection compose with `depth_map` parallax? The path exists (`pbr.wgsl:59-66`); unrun.
2. Hole core: dark with parallax (step 1) or a real cut (step 2)? A cut needs interior geometry or an interior-mapped bore.
3. Hole size per round: nominal calibre or penetrator diameter (sub-calibre, HEAT)? Round data per ADR-0034.
4. Mark budget and eviction per tank? Dagor caps 10 capsules + 15 spheres; Blitz fades at 105/420 m (both CLAIMED).
   A per-tank ring like `GroundMarkRing` (`src/vfx/impact.rs`) fits.
5. Ship platforms: Windows/Linux only would reopen clustered decals; on the Mac dev box they are documented off.
6. LOD: forward decals survive `geometry_lod` swaps; a step-2 material must go on every rung.
7. Is quad-normal lighting acceptable on cast turrets? Needs a capture.
8. WT's real tuning (config overrides of the defaults above) and its game-side vehicle shaders are unpublished.
