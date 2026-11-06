# The map's surroundings: sky, haze, outland and boundary (design, 2026-11-06, revision 2)

Subject: what the player sees above and beyond the playable map, and the static bound at its edge.
The work covers view data, map data and one static collider. Where it touches rounds, bursts or
debris, the subject is the game's damage model, and the text uses the approved terms (AGENTS.md).

This file owns the design narrative; module docs point here and do not retell it.
The inventory records the starting implementation; lane acceptance and §3.7 track delivery.

Settled before this design, and not reopened here (brief, owner comment on #200, owner 2026-11-06):
- the sky is the procedural analytic sky (S1), drawn;
- the sky is as bright as the light it casts: one number, `sky_intensity_cd_m2`;
- clouds and the sun disk are baked into that sky (S5a), and the sun is fixed per map;
- Bevy's `Atmosphere` is out;
- the boundary today is an invisible wall at the map's edge; most of a map's edge later becomes a
  natural border (river, embankment, rising ground);
- for the playtest, rounds that leave the play area fly through the drawn outland; the follow-up is
  #280 "Rounds fired past the map edge end on the drawn land beyond it";
- the workaround rule: chase the root first, but a root fix that adds compute or is invalid loses
  to the simplest honest patch, and that patch is kept as the only path.

New terms, added to `.agents/GLOSSARY.md` by the lane that introduces each:
- **Play area** (W1a): the region bodies may occupy, a polygon in XZ between a floor and a ceiling.
- **Boundary** (W1a): the play area's side bound, as data and as the static wall built from it.
- **Outland** (L3b): the view-only land drawn beyond the terrain's edge.

---

## 1. Inventory: the map asset pipeline today

Classes:
- **B**: best practice, and the precedent new pieces follow.
- **P**: provisional bootstrapping. The row names what replaces it.
- **U**: unclear. The row says what is unclear.

Where code and prose disagree, the code wins, and the row says so.

### 1.1 Files in `assets/maps/kalinovo/`

| File | Producer | Read by | Class | Evidence |
|---|---|---|---|---|
| `level.json` | Houdini export, format `overmatch_bevy_level` v1, hand-edited since. The Houdini scene is not in the repo. | `map::parse` (`src/map.rs:558-623`); the content digest (`map.rs:485-504`); `scripts/lod/config.py:80-124` (the LOD certificate's right wall, the map diagonal); the server pack (`.github/actions/build-server/action.yml:114-115`) | **P**: the source is off-repo and edits land on the exported product. Replaced by #221's pipeline with tracked source. The provisional marks are already in the data: `"placeholder": true` on `house_proxy` and `church_proxy`, `spawn_points_note: "ASSUMED…"`. | 709 instances. MEASURED: `fir_a_0201` stands at x = 749.9 m, 0.09 m from the edge; 26 instances stand within 10 m. Houses are ≥ 117.9 m from the edge, spawn points ≥ 30 m. `church_proxy` has 0 instances. |
| `terrain_height.png` | the same off-repo export | `decode_height_grid` reads the bytes itself (`src/terrain_grid.rs:569`) and decodes them (`grid_from_png`, `:593`); the digest reads them again (`map.rs:493`) | Product **B** (LFS, `.gitattributes:20`); source **P**, as above. | Decoded once and downsampled once to 1025² (`terrain_grid.rs:90`). |
| `terrain_masks.png` | `scripts/map/extract_masks.py`, from a TIFF kept off-repo (`extract_masks.py:6-8`, `:39`) | `ground_field` for sim ground types (`src/ground_field.rs:145-213`); the view's mask image, built from that decode (`src/world.rs:514`, `:684-687`) | Product **B**. Script **P**: its source is off-repo, replaced by #221. **U**: the server reads the masks but the digest does not fold them (`map.rs:485-504`); W1a folds them (R1). | The script refuses misalignment at full resolution before writing (`extract_masks.py:17-20`). |
| `prototypes.ron` | hand-authored, game-owned | `map::parse` (`map.rs:562-584`), `check_prototype` (`:524`), the digest | **B** | Every id the placement uses or the author declares must resolve, or `map::parse` panics (`map.rs:571-584`). |
| `derived/map_ui.png` | a python/PIL recipe written only in a doc comment (`src/net/spawn_map.rs:46-61`) | `net::spawn_map`, a client plugin; excluded from the server pack (`action.yml:114-115`); pushed-asset list (`scripts/hooks/test_pushed_assets.sh:382`) | **P**: untracked generator. Replaced by a startup derivation from `HeightGrid` (R3a), a pure 1024² downshift, which deletes the recipe and the LFS product. No lane here does it. | `derived/` is "what WE generate from the authored data" (`map.rs:59-62`, `:86-89`). |

### 1.2 What the map places, and its sources

| Item | Producer | Read by | Class | Evidence |
|---|---|---|---|---|
| `tools/constructions/{fir_1.pieces,house_1.house}.ron` | hand-authored, tracked | `bake-constructions` (`tools/src/bin/bake-constructions.rs`) | **B** | A stage reads `tools/`, writes `assets/`, in canonical bytes, and its tests bake twice (`tools/src/lib.rs:3-6`). |
| `assets/constructions/<id>/<id>.construction.ron` | `bake-constructions` | `ConstructionLibrary::load`; the digest (`map.rs:500-501`) | **B**. Header: "Written by the construction bake … Do not edit." | The bake writes only what the game's gate admits (`bake-constructions.rs:4`). |
| `assets/constructions/<id>/<id>.dressing.ron` | `scripts/prop/build.py`, a tracked Blender pipeline that certifies its source, view and cutter input and re-exports to compare bytes (`build.py:1-8`) | the construction view | **B**: tracked inputs, reproducible output, a "Regenerate rather than edit" header (`fir_1.dressing.ron:1`). | |
| `assets/trees/fir_1/` (`.blend`, `.glb`, `.lod.json`, `.tree.ron`) | Blender source, its export, the LOD certificate | the prop and LOD pipelines (ADR-0033, ADR-0035) | **B** | Source and product both tracked (ADR-0036). |

### 1.3 Code

| Module | Role | Class | Evidence and notes |
|---|---|---|---|
| `src/map.rs` | parses and checks the manifest, the library and the constructions; computes the digest | **B**, except the digest's separate second load (**U**, below) | `MAP_ID` is a const (`:47`): one map ships at early access (PRODUCT.md), and no selection framework exists. The root vs `derived/` split is stated at `:59-62` and `:81-89`. `SpawnPoints::checked` refuses at the load edge, naming file and field (`:126-142`). `#[serde(default)]` on the author's prototype block (`:262`) is a named legitimate absence ("An export may omit the block"), read for keys only. **U**: `load` resolves constructions after `parse` returns (`:441-447`), and `content_digest` repeats parse and resolve on its own (`:485-504`). A cross-file check placed in `parse` could not see footprints, and the digest's load could skip a check `load` runs. W1a gives both one sequence (§3.6). **U**: `dir: path.parent().unwrap_or_else(…)` (`:608`) is a fallback in form and unreachable for a joined path; lens ratchet. |
| `src/terrain_grid.rs` | the one height decode, the heightfield collider, mesh tiles | **B**, except the pack paths and tile size (`:107-133`) and the spawn footprint constants (`:143-148`), which are backlog rows destruction-05 and destruction-09 (**P**) | `ForceFlatWorld` (`:139`) is a fixture world only. A spawn outside the span is a bug and panics (`:154-167`). Collider with `FIX_INTERNAL_EDGES` (`:693-708`). Edge nodes: `node_world_coord` (`:765`); normals: full-grid central differences, one-sided at the map's edges (`surface_normal_at`, `:770-783`); UVs: world XZ over `TEXTURE_TILE_M` (`:737`). |
| `src/terrain_lod.rs` | the startup LOD ladder | **B**: ONE-SURFACE, generated at Startup from the grid, so "no baked product can go stale" (`:11-15`). NO SKIRTS: full-density borders at every tile and level pairing (`:20-24`). Mikktspace tangents required at every rung (`:660-672`, `:694`). An OS failure starting tangent workers is a typed `TangentStartupError` (`:641-656`), and the process refuses to start (`:724-727`): the outside-input outcome. | |
| `src/terrain_shadow.rs` | the baked sun shadow field, startup (`:136`, `:284`) | **B**, except `SUN_ANGULAR_RADIUS_RAD` (`:71-74`, labelled "CLAIMED") and `MIN_PENUMBRA_M` (`:76-79`): backlog row view-13 (**P**) | Outside the field, and beyond the cascades, a receiver reads lit (vendored `render/shadows.wgsl:234-237`, `:280-322`). |
| `src/terrain_blend.rs` | the ground material | **B**, except `BLEND_LAYERS`, `BASE_GROUND` and the appearance consts (`:87-106`, `:190-216`): backlog rows destruction-05 and destruction-08 (**P**) | `prints_world` has no sampler of its own; it is read through the mask's clamping sampler (`:297-302`). §3.3 depends on this. |
| `src/ground_field.rs` | decodes the masks into ground types for the sim (`:145-213`), reading the file itself (`:196`) | **B** | |
| `src/scatter.rs` | places instances on every composition; both peers derive them, nothing is replicated (`:1-24`) | **B**. Graybox `Block`s are placeholders (`"placeholder": true`). | Statics are `[Terrain]` (`:318`). |
| `src/world.rs`, `src/world/look.rs` | sun, analytic sky, environment light, environment spawn | sky law **B**; sky data home **P** (moved by V0) | Sky data is `include_str!` of `assets/world/lighting.ron` (`look.rs:5-6`): global, not per map. `sky_radiance` (`world.rs:176-200`) is baked to 128² `Rgba8Unorm` (`:219-252`) and filtered once (`:256-415`). **A bug**: the bake evaluates radiance at cubemap coordinates (`:204-217`, `:228`), but Bevy samples every cubemap at (x, y, −z) of the world direction (`bevy_core_pipeline-0.19.1/src/skybox/skybox.wgsl:78-79`; vendored `light_probe/environment_map.wgsl:92-93`). Today's fill glow therefore sits mirrored across the XY plane: the sun is at azimuth +45° and the glow at −45° (DERIVED). L1a fixes it. **U**: the sun's `DirectionalLight` spawns on every composition, the server included (`:584-597`), and `world::plugin` parses the lighting file on the server (`:421`). That is view data on the server; V0 moves it. |
| `assets/world/lighting.ron` | 12 `Taste` rows (`:3-14`) | **P**: moves per map (V0) | `look.rs:72` admits `sky_intensity_cd_m2 = 0` and `look.rs:86-99` admits zero colour channels, which the new derivations divide by (§3.2, §3.4). |
| `src/net/spawn_map.rs`, `assets/ui/spawn_map.ron:2` | the spawn clamp, 0.95 of the half-extent as a `Unit` (`spawn_map.rs:41-44`) | **B** while the play area is the terrain square; **P** the day it is irregular (§3.6) | Backlog rows net-05, net-09, net-10. No lane here edits the file. |
| `assets/ballistics/physical_laws.ron:12` | `floor_m: -100`, `StandIn("Finite play boundary until maps author vertical play bounds…")` | **P**, replacement named; W2 retires it into the play area | Read at `src/ballistics.rs:2495`. |
| `src/aim.rs` | aim distance | **U**: the doc at `:41-42` claims the "same `Terrain \| Armor` mask the live shell marches against", but the march meets only `Armor`-member colliders plus the height grid (`ballistics.rs:747-757`, `:784-789`). Code wins: the aim ray stops on `Terrain`-only colliders that rounds pass through. A1 fixes it. | |
| `tools/` | offline Rust stages ("the map compiler", `tools/src/construction.rs:1`) | **B**: never shipped; nothing in the game depends on it (`tools/Cargo.toml:1-4`) | |
| `scripts/lod/config.py:80-124` | reads `MAP_ID` and `level.json` for the LOD certificate's right wall | **B** | |
| `.github/actions/build-server/action.yml:44-127` | the server's asset allowlist, stated positively: maps' json/png/ron minus `derived/` | **B** | `find … -not -path 'assets/maps/*/derived/*'` (`:114-115`) |
| `.gitattributes:17-22`, `scripts/hooks/test_pushed_assets.sh:340`, `:382`, `DEPLOY.md:20-45` | LFS rules, pushed-asset binding, packaging prose | **B** | |

---

## 2. The precedent: rules a map asset follows

Only rules the pieces below exercise.

**R1. A file's folder says who reads it.**

| Folder | Holds | Digest | Server pack |
|---|---|---|---|
| `assets/maps/<id>/` | authored data both binaries read: manifest, library, heightmap, masks, `boundary.ron` | folded | packed |
| `assets/maps/<id>/view/` (new) | authored data only the client reads: `sky.ron`, `outland.ron` | not folded | excluded by one `-not -path 'assets/maps/*/view/*'` line, plus a line in the allowlist comment |
| `assets/maps/<id>/derived/` | products we generate for the client: `map_ui.png` | not folded | excluded (today) |

- A look edit then moves neither the protocol id nor the server.
- Changing who reads a file means moving it between folders.
- `map.rs` states the three folders once, beside `DERIVED_DIR` (`:59-62`).
- Universal facts that belong to no map live once in shared data, never copied per map (§3.4's
  solar radius).

**R2. Source and product.**
- A generated file starts with a header naming its generator and source, and says not to edit it
  (the construction and dressing headers).
- Its source is tracked (ADR-0036). A product whose source is off-repo is P (Kalinovo's heightmap
  and masks).

**R3. Generators.** Best practice is defined by what a generator does, not where it lives:
- tracked inputs;
- reproducible output (canonical bytes, or a re-export compared byte for byte);
- a product header;
- admission of the product by the game's own load gate.

Where each kind lives follows from its toolchain:
- (a) **In the game, at Startup**, when the product is a pure, cheap function of shipped data. It
  writes nothing to disk (`terrain_lod.rs:11-15`, `terrain_shadow`).
- (b) **Offline**, when it is expensive, needs authoring tools, or certifies. Rust stages go in
  `tools/` (`bake-constructions`); Blender pipelines go in `scripts/` (`scripts/prop/build.py:1-8`).
- (c) **Provisional**: an untracked or uncertified one-off (`extract_masks.py`, whose source is
  off-repo; the `map_ui.png` doc-comment recipe). Each names its replacement.

**R4. Provisional is marked in one form per layer.**
- Data: `StandIn("<the missing law or fact>; replaced by <what>")` (`src/provenance.rs`), or the
  author's `"placeholder": true`.
- Code: a `PROVISIONAL:` paragraph in the module doc, naming the replacement and its issue.
- Docs: §3.7's ledger.
- Best practice carries no mark.

**R5. Every authored number in a new file carries its job label.** Code holds only `Resolution`
numbers, each with its reason.
- Every derived quantity a law divides by, or narrows into a texture format, is refused before
  play when its inputs make it non-finite or out of range, naming the rows that made it (§3.2,
  §3.3, §3.4).

**R6. One admitted snapshot per map.**
- `map::load` is the only sequence, and it runs in this order:
  1. read every root file once;
  2. parse;
  3. resolve the constructions;
  4. check across files (play area against spawn points and placements);
  5. compute the digest from the bytes it read;
  6. publish.
- `content_digest(root)` is `load(root).digest`, so the digest's caller (`src/net/protocol.rs:75`)
  gets the same checks.
- Mutation witnesses: editing `boundary.ron` or `terrain_masks.png` changes the digest.
- The height and mask decoders still read their files a second time (`terrain_grid.rs:569`,
  `ground_field.rs:196`). That double read is pre-existing and stays: moving those bytes into the
  snapshot edits `terrain_grid.rs` and pulls backlog rows destruction-05 and destruction-09 into a
  lane with no hour left for them. Recorded as the R6 remainder.

**R7. Sim and view.**
- Sim state is built at spawn from root data (ADR-0014).
- A view product never feeds the sim.
- The server composition (`SimPlugin` without a client root, `src/lib.rs:648-700`) opens nothing
  under `view/` or `derived/`. The CI server artifact smoke runs from the allowlisted assets, so it
  is the exclusion witness.

---

## 3. Design per piece

### 3.1 Sky (V0, V1, L1a, L1b)

**Data and owner.**
- `assets/maps/<id>/view/sky.ron` replaces `assets/world/lighting.ron` (V0). All 12 rows move with
  their labels, and the old file is deleted. The file is read from `map_dir(asset_root)/view/` at
  first use, client only.
- The sun's `DirectionalLight` spawns under the window guard, as the terrain mesh does
  (`world.rs:637`), and `world::plugin` stops parsing at build (`:421`). The server parses and
  spawns nothing of the sky.
- V1 moves the parse, `sky_radiance`, the bakes and the filter-once light out of `world.rs:164-415`
  and `world/look.rs` into a client-only `src/sky.rs`, with no behaviour change. `world.rs` keeps the
  ground, the scatter and the colliders. The world passes `sky::toward_sun()` to `terrain_shadow::install`; the shadow module already
  takes a direction rather than calling the sky itself. The sandbox calls `sky::sun_light` and
  `sky::sun_transform`.

**Orientation (L1a).** Each bake evaluates `sky_radiance` at the world direction its texel is
sampled for: `(c.x, c.y, −c.z)` of the cube direction `c`, the inverse of Bevy's sampling flip
(§1.3).
- Test (invariant: the reflected and drawn glow lies where the directional light comes from;
  relied on by every lit surface and by L2b's seam law): for two fixture sun bearings that are
  asymmetric about both axes, the bake, sampled the way Bevy samples, peaks on the sun's world
  azimuth.
- Face-size and Cube-view tests cannot catch this.

**Law.** One radiance function, two bakes: `sky_radiance(dir, Bake)`, where
`enum Bake { Light, Drawn }`.

| | `Light` (filtered into the environment light) | `Drawn` (`Skybox`) |
|---|---|---|
| Face size | 128², as today (`world.rs:169`) | 512². `Resolution`: 0.18° per texel. The optic's 25° field over about 1080 px is 0.023°/px, about 7.6× texel magnification (DERIVED); the gradient and the haze-faded clouds stay smooth at that. |
| Format | `Rgba16Float` (L1a). The generator's copy pass converts formats (vendored `light_probe/generate.rs:409`) and builds its own `D2Array` views (`:605-608`, `:781-787`). | `Rgba16Float`, with a `TextureViewDimension::Cube` view descriptor (`bevy_image-0.19.1/src/image.rs:641`) |
| Sun disk | absent: the `DirectionalLight` carries the sun's light. A disk here would add the sun's illuminance twice (DERIVED: disk radiance × disk solid angle = illuminance). | present (§3.4) |
| Below the horizon | ground bounce, today's law (`world.rs:197-199`) | the horizon value of the same azimuth: infinitely distant ground under haze, Koschmieder's limit, which the fog converges to (§3.2) |
| GPU memory (DERIVED) | 0.8 MB | 12.6 MB (512²·6·8 B) |

**Drawn (L1b).**
- `Skybox { image, brightness: sky_intensity_cd_m2, rotation: Quat::IDENTITY }`
  (`bevy_light-0.19.1/src/probe.rs:228-256`, exported as `bevy::light::Skybox`).
- It goes on every view whose `CameraProfile::draws_battlefield()` holds
  (`render_policy.rs:338-343`): commander, optic, kill cam lesson. It is attached by the system that
  attaches the environment light (`world.rs:316-350`).
- The pen cam inset keeps its own clear colour (`src/damage_cam/pen_cam/stage.rs:286`).

**Brightness.** One number, `sky_intensity_cd_m2`: the `Skybox` brightness and the environment
intensity (`world.rs:341`).
- Bevy scales the skybox by the view's exposure (`skybox/mod.rs:63-80`).
- L1b reports the drawn sky's luminance against the lit ground in frame 1. Raising the number also
  raises the fill; that is the settled trade.

**Startup cost (ESTIMATE).** About 1.6 M radiance evaluations for the drawn bake, tens of ms before
clouds. L4b splits it per face over `ComputeTaskPool` if it measures above one frame.

**Failures.**
- `sky.ron` absent or malformed: refused before play, naming the file, the field and the fix.
- `sky_intensity_cd_m2 = 0`: now refused (R5). A sky that casts and shows nothing divides §3.4's
  disk ratio by zero.
- A cubemap without a Cube view: Bevy skips the skybox with only a `warn_once`
  (`skybox/mod.rs:262-281`). Ruled out by the constructor and pinned by a test.
- A face size that is not a power of two: the generator panics (`generate.rs:1033-1040`). The
  existing `the_source_cubemap_matches_what_the_filter_requires` test covers the light bake;
  L1b extends that contract to both bakes.

**Sim/view.** View only; nothing is replicated.

**Class.** B.

**Culling.** Battlefield views inherit `far = 1000` m (`bevy_camera-0.19.1/src/projection.rs:422`),
but view culling never tests the far plane:
- `check_visibility` calls `intersects_sphere(…, false)` and `intersects_obb(…, true, false)`
  (`visibility/mod.rs:831-837`);
- `intersects_sphere` stops at the near plane when `intersect_far` is false (`primitives.rs:254-262`);
- `intersects_obb` skips the far half-space (`primitives.rs:271-289`);
- MEASURED in the plan's frame 7: terrain 2.1 km away draws.

No culling reach is added for the outland or distant life. L3b and L5b capture witnesses confirm it
on the shipped composition.

### 3.2 Haze (L2a plain fog, L2b shared lobe, L2c VFX)

**Law.** Koschmieder extinction (L2a).
- `DistanceFog { …, falloff: FogFalloff::from_visibility(V) }` (vendored `fog.rs:55-75`,
  `:312-317`).
- β = −ln(0.05)/V = 2.996/V (`:436`, `:463`). The fragment mixes toward the fog colour by
  `1 − e^(−βd)` (`render/fog.wgsl:41-50`).
- Haze reaches 90 % at 0.77·V (DERIVED).

**Fog colour.** `haze_linear × sky_intensity_cd_m2 × exposure`.
- The view's `Exposure`, or `Exposure::default()` (EV100 9.7, `bevy_camera-0.19.1/src/camera.rs:236-283`)
  when it has none; today no camera carries one.
- Bevy does not scale the fog's base colour by exposure (`render/fog.rs:70-110`), so the fog system
  multiplies once, reading `Option<&Exposure>` as `vfx/muzzle.rs:426-438` does.
- DERIVED for today's rows: (0.51, 0.39, 0.25) linear.

**Toward the sun: one unshadowed scattering law for sky and fog (L2b).**
- The sky's glow moves from azimuth-only (`world.rs:189-196`) onto a 3-D forward lobe,
  `max(dot(dir, toward_sun), 0)^glow_exponent`, keeping its `(1 − t)` fade above the horizon.
- The fog uses the same lobe through `directional_light_color`, set per channel to
  `(glow − haze) × glow_strength × sky_intensity / (sun_linear × sun_illuminance)` (DERIVED).
  Bevy's lobe multiplies `light.color`, which is premultiplied by illuminance
  (`render/light.rs:1731-1733`), and the exposure (`pbr_functions.wgsl:912-917`).
- **Unshadowed.** Bevy multiplies the fog's lobe by the receiver's shadow
  (`pbr_functions.wgsl:919-925`). That costs a filtered cascade sample, possibly a blend into the
  next cascade, and the heightfield lookup per fogged fragment (`render/shadows.wgsl:238-263`). It
  also makes the fog differ from the unshadowed sky in exactly the fragments the shared law claims
  equal.
  - A receiver's shadow is no statement about the kilometres of air in front of it, so the shadowed
    form is not the more honest law either.
  - L2b drops the shadow factor from fog scattering with a vendored patch in
    `vendor/bevy_pbr-0.19.1-scalar-math/src/render/pbr_functions.wgsl`, recorded in that crate's
    `OVERMATCH_PATCH.md`. The patch removes compute rather than adding it.
- Without the lobe, the horizon toward the sun is about 11 % brighter than the fog under it
  (DERIVED from today's rows: 0.4·haze + 0.6·glow against haze).
- Witnesses:
  - a CPU test pins the shared coefficients (invariant: the fog's horizon colour equals the drawn
    sky's horizon radiance at three azimuths; relied on by the outland's rim and the clouds);
  - the shader is exercised by L3b's capture of the outland rim against the sky at three azimuths,
    pixel values reported MEASURED and judged by eye (visual-parity doctrine: pixels point, they do
    not gate).
- Ablate keys `fog` (L2a) and `fog_scatter` (L2b) in `render_cost::Ablate`
  (`src/render_cost.rs:104-121`). Their user is the frame-cost runner. Both are reported, MEASURED on
  the M4.

**Visibility V.**
- One per-map row in `view/sky.ron`:
  `visibility_m: (12000.0, Taste("haze: meteorological visibility; playtest fork haze-visibility"))`.
- It is the same for every preset, because fog may not vary by preset (#159).
- V also sets the outland's reach (§3.3), the clump band (§3.5) and the cloud fade (§3.4).

| V | contrast kept at 1 km | at the map diagonal, 2.12 km | outland width 1.25·V |
|---|---|---|---|
| 8 km | 69 % | 45 % | 10 km |
| **12 km (default)** | **78 %** | **59 %** | **15 km** |
| 20 km | 86 % | 73 % | 25 km |

DERIVED: e^(−2.996·d/V). Every sightline in the map keeps contrast above the 5 % threshold at any V
above 2.12 km. The haze softens distance and never hides a tank: no abstract spotting (PRODUCT.md).

**The playtest fork.** L2a writes `.agents/scratch/playtest-forks/haze-visibility.md`:
- **Status**: open; settles by play.
- **Default**: 12 km.
- **Alternatives**: 8 km, a heavier golden-hour haze with a strong depth cue; 20 km, crisp, with a
  wider outland.
- **Why it's a playtest call**: does a tank at 1.5–2 km read in the optic, and does distance read
  as depth or as murk?
- **Revert cost**: low. One row; the outland width follows by derivation; no sim or record change.
- **Lives in**: §3.2, `view/sky.ron`, `src/sky.rs`.

**VFX (L2c).** The billboard and trail fragments apply no fog
(`assets/shaders/vfx_billboard.wgsl:51-52`, `vfx_trail.wgsl:38-39`). L2c applies the same factor
from the view's fog uniform in both.

**Failures.** Refused before play (R5), each naming its rows:
- V not finite, or ≤ 0;
- glow darker than haze in any channel (that makes negative in-scatter);
- any `sun_srgb` channel at 0 (the lobe divides by it);
- a derived `directional_light_color` that is not finite.

**Sim/view.** View only. Haze is the only law of visibility over distance, and it is the same for
every player.

**Class.** B, with the default under a playtest fork.

**Capture.** L2a: frames 1, 3, 4 and 7. L2b: frame 6, toward the sun, checked in the optic. L2c: a
tracer at range in the optic.

### 3.3 Outland ground (L3a topology, L3b spawn, L3c colour)

**Shape (L3a, a pure function of the grid, the width and `outland.ron`).**
- **Reach.** A square annulus around the terrain square, out to 1.25·V beyond the edge.
  - DERIVED: the residual land contrast there is e^(−3.745) = 2.4 %, half the visibility threshold.
  - The ratio is a `Resolution` in code with that reason. The drawn sky below the horizon is the
    same limit (§3.1), so the rim leaves no line.
- **Rows.** Closed square loops.
  - Loop 0 is the grid's own edge nodes: exact samples at `node_world_coord`.
  - Spacing grows geometrically from the grid's 1.465 m cell (`Resolution` ratio, about 1.1;
    `const` assert > 1).
  - The count along each side halves each time the spacing has doubled. At each 2:1 transition, the
    fine loop's odd vertices take the height interpolated on the coarse edge, so no T-junction opens.
  - ESTIMATE ≤ 50 k vertices at 15 km.
- **Sectors.** The mesh is cut into sectors, each with its own `Aabb`, cut along loop indices. Every
  boundary vertex appears in both adjacent sectors with identical attributes. Each loop's corner is
  one vertex. No `VisibilityRange`: its slots leak (`terrain_lod.rs:26-30`).
- **Height (P).** Odd reflection about each edge, `h(e + d) = 2·h(e) − h(e − d)`, separable at the
  corners, blended toward the edge's mean height by a smoothstep over `fade_m`. Data:
  `view/outland.ron`, `fade_m: (…, StandIn("outland height: odd-reflection fade; replaced by the authored outland (#221)"))`.
  - Odd reflection keeps the outward slope continuous, and the smoothstep's zero slope at the seam
    keeps it so.
  - So the grid's one-sided edge normal (`terrain_grid.rs:770-783`) equals the outland's own
    central difference at the seam by construction: `(2h₀ − 2h₋₁)/(2s) = (h₀ − h₋₁)/s`.
  - An even mirror would fold the slope into a crease one cell out.
- **Seam attributes.** Loop 0 copies the grid's normals (`surface_normal_at`), bit for bit, so f32
  rounding cannot open a shading line. UVs follow the grid's law, world XZ over `TEXTURE_TILE_M`
  (`terrain_grid.rs:737`), so they are continuous by construction.
- **Tangents.** The outland requires mikktspace tangents, as every terrain rung does
  (`terrain_lod.rs:660-672`), generated by the same `Mesh::generate_tangents` (`:694`). A generation
  failure is the same typed startup refusal.
  - At the seam the two meshes' tangents differ by their adjacent triangles. L3a reports the
    MEASURED largest angle, and L3c's edge capture judges it.
- **Tests** (invariant: the seam is crack-free and attribute-continuous; relied on by every view
  past the edge):
  - loop 0 equals the edge samples and normals bit for bit;
  - every transition's odd vertex lies on its coarse edge;
  - every sector boundary vertex matches across sectors.
  - The provisional shape itself is judged by eye, not pinned.
- **Best practice for the next map.** The authored terrain covers the play area plus a margin, and
  the play area is a polygon inside it. Rounds then meet the near outland as height grid, and only
  the far outland is view. The margin is the map author's call (#221).

**Spawn (L3b).**
- Spawned in `spawn_environment`'s windowed branch beside `terrain_lod::spawn` (`world.rs:766-777`),
  so the server builds none. The owner is `src/outland.rs`.
- Drawn with the terrain's own material handle (no new material yet).
- A new `VisualScope::OUTLAND`, `BattlefieldWorld` plus `ReceiveOnly`.
  - Same values as `BAKED_CASTER` (`render_policy.rs:243-246`), a different reason; the precedent is
    `LightProfile`'s two names over one mask (`render_policy.rs:362-371`).
  - It casts no shadow and receives cascade shadows near the edge. Beyond the cascades it reads lit,
    and the haze covers that beyond about 0.5·V. Accepted.
- A new `CasterClass::Outland` (`render_cost.rs:163-167`), ablate key `outland`.

**Colour (L3c).** A second `TerrainMaterial` asset, cloned from the terrain's in `outland.rs`:
- `masks` is a second image from `ground_mask_image` (`world.rs:514`) whose sampler mirrors
  (`ImageAddressMode::MirrorRepeat`, `bevy_image-0.19.1/src/image.rs:726-742`). C0 at the seam is
  all colour needs.
- `prints_world` and `prints_near` are empty images. Through the shared clamping sampler
  (`terrain_blend.rs:297-302`), the map's prints would otherwise mirror outward.
- Per-pixel cost is bounded by the terrain's.
- No edit to `terrain_blend.rs`, so backlog rows destruction-05 and destruction-08 stay out of this
  lane.

**No collider.** The outland is view (ADR-0014, ADR-0037). Rounds past the edge pass through it
(settled; #280).

**Failures.** Refused before play: `fade_m` not finite or ≤ 0, naming `outland.ron`.

**Class.** The mesh, seam and sectors are B. The odd-reflection height and the mirrored colour are P.

**Capture.**
- L3b: frame 5, outside the east edge looking back (no floating slab); frames 1, 3 and 7; and the
  rim against the sky at three azimuths, for §3.2's shader witness.
- L3c: frame 4, the optic at the edge: no colour seam, no clamp streaks, the tangent seam by eye.

### 3.4 Clouds and sun disk (L4a disk, L4b clouds)

Both are baked by `sky_radiance` and stay static, since the sun is fixed per map.

**Sun disk (L4a).** `Drawn` bake only.
- Angular radius: one shared row, `assets/world/sun.ron` (embedded like today's lighting file), per
  R1's universal-fact rule:
  `sun_angular_radius_rad: (0.00465, Measured("mean apparent solar semi-diameter, 16′, Astronomical Almanac"))`.
  - It replaces `terrain_shadow.rs:71-74`. The baked penumbra and the drawn disk then read one owner.
  - L4a closes backlog row view-13: it classifies `MIN_PENUMBRA_M` (`:76-79`) separately, as a
    `Resolution` with its f32-rounding reason.
- Radiance = `sun_illuminance_lux` / disk solid angle (DERIVED) × `sun_disk_gain`, a
  `Taste("sky: sun disk brightness under bloom")` in `view/sky.ron`. The taste scales the physical
  quantity rather than replacing it.
- At 512² the disk is about 3 texels across: soft, judged by eye.

**Clouds (L4b).**
- One cumulus layer in `view/sky.ron`:
  - `cloud_base_m: Taste("sky: cloud layer altitude")`;
  - `cloud_cover: Taste("sky: share of the sky clouded")`;
  - `cloud_seed: Taste("sky: cloud pattern")`.
- Lit by §3.2's lobe. Faded toward the horizon by Koschmieder over the slant path `h / sin(elevation)`,
  DERIVED from V with no second number.
- Clouds are sky, so they enter both bakes; L4b judges the fill change on hull shading.
- If the measured bake time exceeds a frame, L4b parallelizes the bake per face.

**Failures.** Refused before play (R5):
- the disk's ratio to `sky_intensity_cd_m2` not finite or above the f16 maximum (65504), naming both
  rows;
- `cloud_cover` outside 0..1;
- `cloud_base_m` not finite or ≤ 0.

**Class.** B.

**Capture.** L4a: frame 6, the disk with bloom. L4b: frame 6, lit rims toward the sun; frame 1; a
hull close-up; bake time MEASURED.

### 3.5 Distant life (L5a data, L5b draw)

**Form.**
- Instanced low-poly 3-D clumps on the outland. They are 3-D because the optic shows parallax at
  2 km (plan §4 H3).
- One mesh and one fog-enabled `StandardMaterial` per kind (`fog_enabled` defaults to true, vendored
  `pbr_material.rs:928`), so each kind instances automatically.
- Scope `OUTLAND`, `CasterClass::Outland`.

**Bindings are data (L5a).** `view/outland.ron` lists kinds. Each kind names a construction id
(Kalinovo's file names `fir_1` and `house_1`), a clump shape (`Spire` or `Roof`), a count and a
seed.
- The loader resolves each id through the map's `ConstructionLibrary` and takes size from its bounds
  (`construction::footprint`, `src/construction.rs:320`).
- An unresolved id is refused before play, naming the file, the kind and the library.
- No asset name appears in generation logic.

**Placement (P).** A seeded scatter over the band from the edge to 0.5·V; beyond that a clump is
over 78 % fogged (DERIVED). Each clump is projected onto §3.3's height function.
- Labels: `StandIn("outland life: seeded placement; replaced by authored placement with the map (#221)")`.
- The replacement uses `level.json`'s instance record shape (`map.rs:213-252`), so one placement
  format serves both.

**Failures.** Refused before play: a band outside 0..1.25·V or empty, a negative count, an
unresolved id.

**Class.** The form and the bindings are B; the placement is P.

**Capture (L5b).** Frame 4, the optic on the hill: a tree line at 2–5 km, hazed. This is also the
culling witness (§3.1). An optic traverse shows the parallax. Draw count and ms, MEASURED.

### 3.6 Boundary wall (W1a–W1d; the floor law: W2, W3)

**What it is.**
- The play area's side bound: an invisible static solid standing in for the natural border not yet
  built.
- The mechanism stays when art builds the border, because a river stops no tank. What is P is its
  square shape at the terrain's edge.

**Data: `assets/maps/<id>/boundary.ron`** (root, R1: both binaries read it, the digest folds it, the
pack carries it as a root `.ron`):

```ron
// The play area: a closed simple polygon in world XZ metres, counter-clockwise seen from +Y,
// between a floor and a ceiling.
(
    polygon: ([(-750.0, -750.0), (750.0, -750.0), (750.0, 750.0), (-750.0, 750.0)],
        StandIn("play area: the terrain square's edge; replaced by the natural border the map's art builds (#200)")),
    floor_m: (-100.0, StandIn("play area floor: where simulation ends below the terrain; replaced by authored vertical bounds")),
    ceiling_m: (…, StandIn("wall top: admitted, not derived; launched pieces have no speed ceiling (debris/launch.rs:73)")),
    friction: (0.0, Taste("boundary feel: no grip, so a tank grinding into the wall slides along it and cannot rear up or climb it")),
)
```

`map::load` (R6) turns it into one checked type, `PlayArea`, whose one constructor is the load gate
below. `MapManifest` carries it.

**Shape (W1c).**
- One `Collider::cuboid` per polygon edge, along the edge, from floor to ceiling, extruded outward
  by 20 m (`Resolution`: more than one tick's travel of a body at the edge; DERIVED 100 m/s at 64 Hz,
  `src/bitprobe.rs:37`, is 1.6 m).
- **Corners.** At a convex vertex the two boxes' inner faces meet on the vertex line, leaving no gap
  on the play side. At a reflex vertex their outward parts overlap.
- `RigidBody::Static`, `CollisionLayers::new([Layer::Terrain], LayerMask::ALL)`, like every static
  ground solid (`world.rs:632`, `scatter.rs:318`).
- No `FieldSurface`, no `TerrainMap` block, no spawn obstacle, no mesh.
- Spawned at Startup on every composition, beside the heightfield (`world.rs:626-636`).

**No grip (owner decision, a `Taste`).** The wall's `friction` row is 0 and goes on its colliders
as `substances::contact_friction(friction)` (`src/substances.rs:70-77`), which carries the
project's one pair rule. It replaces borrowing the terrain's solver stand-in
(`ground_types.rs:360-363`). Zero holds on every path that reads a contact's friction:
- **Avian's combine.**
  - `Friction`'s default rule is `Average` (vendored `avian3d-0.7.0-child-pose/src/dynamics/rigid_body/physics_material.rs:146-160`),
    and `Friction::ZERO` is `Average` too (`:164-168`). Under `Average`, zero would hand back half
    the partner's grip.
  - A pair takes the higher-priority rule, `Max > Multiply > Min > GeometricMean > Average`
    (`:7-8`, `combine` at `:204-213`).
  - `PAIR_RULE` is `GeometricMean` (`substances.rs:70-71`). It outranks `Average` and mixes
    `sqrt(μ·0) = 0`.
  - Every friction in the game is `PAIR_RULE` or Avian's `Average` default (`DefaultFriction`,
    `rubble.rs:163-169`); no `Max`, `Multiply` or `Min` exists (grep). So every hull, bump stop,
    debris piece and default collider pairs to zero.
- **The track's own grip law.** Collider support reads `mu: grip.pair(pieces.friction(collider))`
  (`src/track/forces.rs:1298-1301`), where `pair` is Avian's `combine` of the track's substance
  friction (`spec.rs:800-804`, `PAIR_RULE`) with the wall's (`forces.rs:161-163`). That gives zero.
  The successor on `lane/trackworld` is the same expression
  (`lane/trackworld:src/track/world_contact.rs:358-361`).
- Zero grip removes traction along the wall. It does not remove the fictitious upward *normal*
  that the belt-normal contact lends a vertical face (below), so W1c still waits for the convex
  adapter.
- Refused before play: `friction` not finite, or < 0.

**Landing response (P).** `Terrain` membership still makes a debris landing read soil compliance
(`debris/crumble.rs:491` → `contact_law.rs:47-51`, `Contactor::ground`). That is routing, not a
stated fact. The replacement is material selection through a generic contact interface, where every
collider names its material in data. That would also fix graybox Blocks of brick being read as soil.
It is a follow-up task under #200, not a lane here.

**Why a polygon of edge boxes.** "May become irregular later" and "Commit to one path" have one
answer: one representation and one constructor. The square is a four-vertex polygon, not a mode.
Rejected:
- four boxes from the terrain square: a second path the irregular border would replace;
- `Collider::polyline` or a trimesh: the belt admits only boxes and convex polyhedra
  (`lane/trackworld:src/track/world_contact/admission.rs:35-44`), and a surface with no thickness can
  tunnel;
- raising the heightmap at the edge: it changes the sim terrain, cannot be invisible, and is art's
  to do.

**What it stops.** Every row follows from an existing rule, with no case of its own.

| Body | Meets it | Why |
|---|---|---|
| hull proxies, bump stops | yes | `[Vehicle]`, filter `ALL` (`src/tank/spawn.rs:938`, `src/track/bump_stop.rs:162`) |
| belt support | admitted as an ordinary solid | `world_contact::solid` admits any non-sensor collider that is Vehicle-compatible (`lane/trackworld:src/track/world_contact/admission.rs:13-15`). The solid-admission contract requires no category-specific registration (#198). |
| track side slices | no | their filter omits `Terrain` (`bump_stop.rs:136-147`); trackworld deletes them |
| debris pieces | yes | `piece_layers` includes `Terrain` (`src/construction/breakage.rs:2688-2699`) |
| a burst's push rays (game's damage model) | yes, as a static surface | `SURFACES` includes `Terrain` (`breakage.rs:1293-1301`) |
| orbit camera | yes | `ground_distance` casts against `Terrain` (`world.rs:941-954`) |
| aim ray | no, after A1 | A1 gives aim the march's first-surface query |
| rounds | no | the march meets `Armor` colliders and the height grid (`ballistics.rs:747-757`, `:784-789`) |

**The belt's contact must use the wall's face (blocks W1c).**
- Admitted is not the same as correct. On main and on `lane/trackworld` alike, a collider patch's
  contact normal is the belt's own `-out`, not the face it meets, and its depth is the slab overlap:
  - main: `src/track/forces.rs:1256-1270`, `rubble.rs:766`;
  - lane: `lane/trackworld:src/track/world_contact.rs:316-330`.
- A vertical wall overlapping the belt would therefore lend upward support and traction: the same
  perch the house snag showed.
- Trackworld session 3, the convex adapter, returns the actual clipped faces and normals per belt
  element. **W1c spawns the wall only after that adapter lands on main.** No wall-specific exclusion
  is added in the meantime; the root fix is already the next lane of its chain and adds no compute.

**Escape and the floor.**
- **Ceiling.** Admitted, not derived: launch speed is impulse over mass with no ceiling
  (`debris/launch.rs:73`).
- **Below the floor, simulation ends.** One law for everything:
  - rounds retire there (W2, retiring `physical_laws.ron`'s `floor_m` into its named replacement);
  - debris pieces retire there (W3). This changes `src/debris.rs:42`, "A piece stays all match", and
    keeps its bound: live bodies never exceed the breakable chunks. A retire is a game state, logged
    with the piece's speed. A log of escapes is the evidence the ceiling needs raising.
  - A tank below the floor is a broken invariant, since the wall holds hulls (W1d's witnesses). It
    panics, naming the tank.
- No side retire: a round leaving a concave polygon can re-enter across a notch.

**Load gate.** In `map::load` after the constructions resolve (R6), refusing before play and naming
the file, the field and the fix.

W1a:
1. The polygon has ≥ 3 finite vertices, is simple, is counter-clockwise (positive signed area), and
   every vertex lies inside the terrain square.
2. `floor_m` is below the lowest decodable height and `ceiling_m` above the highest (both from
   `height_decode`, `map.rs:586-597`).

W1b:

3. Every spawn point lies inside the polygon, and so does the spawn-limit square (0.95·half,
   `spawn_map.rs:41-44`). That square is P, replaced by an inset of the polygon the day the boundary
   is irregular.
4. No placement's footprint (`construction::footprint`, or the Block transform) crosses the polygon;
   the refusal names the instance id. W1b moves Kalinovo's refused instances (`fir_a_0201` first)
   inward in `level.json` by the least distance that clears them, and lists them in its commit.
5. No edge's outward box enters the polygon's interior (a notch narrower than the wall).

**Aim (A1).** The aim ray takes the march's first-surface query: the height grid's exact caster
plus `Armor` colliders. `ground_distance` stays the camera's physical query.
- Without A1, a concave play area would put a visible tank across a notch behind wall that the aim
  ray stops on and the round passes.
- A1 also makes `aim.rs:41-42`'s claim true.

**Sim/view.** Sim state built at spawn from root data on every composition (ADR-0014). Never
replicated: both peers derive it (`scatter.rs:8-11`).

**Class.** The wall, the polygon, the gate and the floor law are B. The square polygon, the square
spawn limit, the invisibility and the routed landing response are P. The zero grip is an owner
`Taste`, labelled as one.

**Witnesses (W1c, W1d).**
- Tolerances are read from Avian's configured contact tolerances, their owner, never restated.
- W1c (invariant: no body crosses the side bound; relied on by the server sim):
  - a debris piece thrown outward, flat and spinning, stays inside;
  - `ground_distance` toward the edge returns the wall's distance;
  - no grip (invariant: the owner's zero-grip choice holds on every friction path): the wall's
    friction, read from `boundary.ron`, combined with every shipped substance's contact friction,
    with the track's grip friction through `GripParams::pair`, and with `DefaultFriction`, is zero,
    static and dynamic.
- W1d (same invariant, for hulls):
  - a tank at full throttle head-on, at 30° grinding along the wall, into a convex corner, and
    arriving airborne off a ramp, stays inside;
  - driven obliquely into the wall, it gains no height from it: no rearing, no climb, no perch.
    It slides along the wall.

**Capture.** W1d: third person at the east edge, the tank held at the wall and the camera pulled in.

### 3.7 Ledger: best practice and provisional

| Piece | Best practice now | Provisional now | Replaced by |
|---|---|---|---|
| Sky data (V0 delivered) | per-map `view/sky.ron`, admitted at client first use; sun behind the window gate; server pack excludes `view/` | none | none |
| Sky module (V1 delivered) | `src/sky.rs` owns admission, the sun, analytic bake and filter-once light; existing sky witnesses retain their assertions | none | none |
| Sky light bake (L1a delivered) | `Bake`; the light bake evaluated in world orientation (`world_direction`), `Rgba16Float`; `sky_intensity_cd_m2 = 0` refused | none | none |
| Drawn sky (L1b onward) | analytic sky; the drawn bake in world orientation; one brightness | none | none |
| Haze | Koschmieder fog; one unshadowed lobe for sky and fog | the 12 km default (playtest fork) | the fork's verdict |
| Outland | stitched, attribute-continuous view mesh to 1.25·V | odd-reflection height; mirrored mask | authored outland, or terrain past the boundary (#221) |
| Clouds, sun disk | baked, static; one shared solar radius | none | none |
| Distant life | instanced clumps bound to constructions in data | seeded placement | authored placement in the instance format (#221) |
| Boundary | polygon, edge boxes, load gate, floor law, no grip (owner `Taste`) | square polygon; square spawn limit; invisible; routed landing response | the natural border's polygon (#200); a polygon-inset spawn limit; generic contact materials |
| Map load | one admitted sequence and digest (R6) | the decoders' second read (R6 remainder) | the snapshot carrying the bytes, with destruction-05 and destruction-09 |
| Map sources | tracked, reproducible, admitted generators | off-repo Houdini scene and TIFF; `map_ui.png` recipe | #221's pipeline; a startup derivation |

---

## 4. Lanes

**Order.**
- Three chains run in parallel: the sky chain **V0 → V1 → L1a → L1b → L2a → L2b**, the boundary
  chain **W1a → W1b**, and **A1**.
- After the sky chain: **L2c**, **L3b** (with L3a) and **L4a**, then **L3c**, **L4b** and **L5b**.
- **L3a** and **L5a** are pure and start early.
- **W2** and **W3** follow W1a.
- **W1c** waits for W1b and trackworld session 3 on main; **W1d** follows W1c.

**Every lane.**
- Under an hour; commits each green step.
- Builders take no timing runs; the orchestrator prices landed changes on the M4 frame-cost runner.
- Capture lanes re-shoot their assigned frames hidden and muted. The owner judges look by eye.

**Capture plans.** `scripts/capture/surroundings/p1-normal.ron` through
`p7-farplane.ron` are the one home of frames 1–7, and `hull-closeup.ron` of the hull close-up.
Each header records its start pose and a hidden, muted command under the machine lock. Outputs go
to `target/capture/sky/`.
Frames: 1 third person toward the south edge; 2 the same spot in the optic; 3 the highest hill;
4 that hill in the optic; 5 outside the east edge looking back; 6 the diagonal toward the sun;
7 the diagonal away from the sun, beyond the camera far plane; the hull close-up from the
south-east, square to the sun's bearing, so the faces in view take their fill from the sky.

**Ratchet.** The backlog rows naming files these lanes edit:
- view-13 (`terrain_shadow.rs`) goes to L4a.
- No lane edits `terrain_grid.rs` or `terrain_blend.rs` (destruction-05, destruction-08,
  destruction-09) or `spawn_map.rs` (net-05, net-09, net-10).
- A builder who finds it must edit one stops and reports: the rows come with the edit and need a
  lane of their own.
- Any other file a lane edits carries no backlog row today (`.agents/lenses/backlog.md`, read
  2026-11-06).

| Lane | Builder | Blocked by | Change | Acceptance | Capture | Leaves P |
|---|---|---|---|---|---|---|
| **V0** View folder | Codex | none | Moves `lighting.ron` to `maps/kalinovo/view/sky.ron`, read at first use. The sun spawns under the window guard; `world::plugin` stops parsing. Adds the `view/` allowlist line. States R1 in `map.rs`. | A missing or malformed `sky.ron` panics, naming file, field and fix (test). The server artifact smoke passes without `view/`. Frames compared once in the report; no behaviour change claimed. | 1, 6 | none |
| **V1** Sky module | Codex | V0 | Moves the sky code into `src/sky.rs`, mechanically. | Existing sky tests pass unmoved in meaning; frames compared once in the report. | 1 | none |
| **L1a** World-oriented light bake | Claude | V1 | Adds `Bake`, `Rgba16Float`, and the (x, y, −z) evaluation; refuses `sky_intensity_cd_m2 = 0`. | Asymmetric-bearing test (§3.1). The fill glow moves to the sun's side; hull shading compared in the report. | hull close-up, 6 | none |
| **L1b** Draw the sky | Claude | L1a | Adds the `Drawn` bake (512², Cube view, horizon value below the horizon), the `Skybox` on battlefield views, and ablate key `sky`. | Cube-view test; face-size test covers both bakes; grey gone in frames 1–7; pen cam corner unchanged; sky-to-ground luminance reported. | 1, 2, 4, 6; kill cam lesson | none |
| **L2a** Plain haze | Claude | L1b | Adds `visibility_m`, `DistanceFog` with exposure-scaled colour, the V refusal, the fork file, and ablate key `fog`. | Cost MEASURED. | 1, 3, 4, 7 | 12 km default |
| **L2b** One lobe | Claude | L2a | Moves the glow onto the 3-D lobe and derives `directional_light_color`. Adds the vendored unshadowed-scatter patch and its `OVERMATCH_PATCH.md` entry, the glow, sun-channel and finite refusals, and ablate key `fog_scatter`. | Shared-coefficient test (§3.2). Cost MEASURED. | 6, in the optic | none |
| **L2c** VFX under haze | Claude | L2a | Billboards and trails apply the fog factor. | A tracer and smoke at 1–2 km fade as the ground behind them; cost MEASURED. | optic tracer | none |
| **L3a** Outland topology | Claude | none (pure) | `src/outland.rs`'s mesh function: loops, transitions, sectors, odd-reflection height, seam normals and UVs, tangents. | §3.3's tests. Vertex count and seam-tangent angle MEASURED. | none | odd-reflection height |
| **L3b** Outland spawn | Claude | L3a, L2b | Spawns from `view/outland.ron` (`fade_m` refusal) to 1.25·V, with the terrain material. Adds `VisualScope::OUTLAND`, `CasterClass::Outland`, ablate key `outland`, and the Outland glossary entry. | Cost MEASURED; rim pixels at three azimuths reported (§3.2's shader witness). | 5, 1, 3, 7; rim ×3 | none new |
| **L3c** Outland colour | Claude | L3b | The second `TerrainMaterial`: mirrored mask image, empty print stores. | Cost MEASURED. | 4, at the edge | mirrored mask |
| **L4a** Sun disk | Claude | L1b | Adds the disk to the `Drawn` bake, the shared `assets/world/sun.ron` radius read by `terrain_shadow`, `MIN_PENUMBRA_M` classified (closes view-13), and the f16 refusal. | Penumbra tests pass reading the shared row. | 6 | none |
| **L4b** Clouds | Claude | L4a, L2b | Adds the cloud rows and their refusals, the lobe-lit layer and the V-derived fade; per-face parallel bake if needed. | Bake time MEASURED; hull shading compared in the report. | 6, 1, hull | none |
| **L5a** Clump data | Codex | L3a | Kinds in `view/outland.ron` resolved through the construction library, with refusals and the seeded placement (pure). | Every refusal fires on its fixture; placement lies in the band and on the height function. | none | seeded placement |
| **L5b** Clump draw | Claude | L5a, L3b | Instanced meshes per kind, scope and caster class. | Draw count and ms MEASURED. | 4 (the culling witness); optic traverse | none |
| **W1a** Play area | Codex | none | `boundary.ron`, `PlayArea`, gate checks 1–2, the R6 sequence with `content_digest = load().digest`, masks in the digest, and the Play area and Boundary glossary entries. | Each refusal fires on its fixture. Editing `boundary.ron` or the masks changes the digest. | none | square polygon |
| **W1b** Cross-file gate | Codex | W1a | Gate checks 3–5; the refused Kalinovo instances moved in `level.json`. | Each refusal fires on its fixture; the shipped map loads. | none | square spawn limit |
| **W1c** Wall | Codex | W1b; trackworld session 3 on main | Spawns the edge boxes with the `friction` row through `contact_friction`, plus its refusal. | W1c witnesses (§3.6), the no-grip pair witness included. | none | invisible; routed landing response |
| **W1d** Hulls at the wall | Codex | W1c | Driving witnesses only. | W1d witnesses (§3.6): the oblique drive gains no height. | east edge, third person | none |
| **W2** Round floor | Codex | W1a | Subject: the game's damage model; the brief carries the vocabulary section. The march retires below `PlayArea.floor_m`; `physical_laws.ron`'s `floor_m` is deleted. | A round fired outward over the edge is retired once below the floor (invariant: every march ends; relied on by the server's per-round cost). | none | none |
| **W3** Piece floor | Codex | W1a | Debris pieces below the floor retire, logged with speed; a tank below the floor panics, naming it. `debris.rs:42` is restated. | A piece placed below the floor is retired within one tick (invariant: no body falls forever). | none | none |
| **A1** Aim meets what rounds meet | Codex | none | Subject: the game's damage model. `aim_distance` uses the height grid's exact caster plus `Armor`; the doc claim at `aim.rs:41-42` is corrected. | Aim over a `Terrain`-only fixture block reaches the height grid behind it, as the march does (invariant: the aim predicts the round; relied on by the aim dots). | none | none |

The plan's L6 (an `Atmosphere` spike) is out: settled.

---

## 5. Owner decisions left

1. **When the wall arrives.** W1c waits for trackworld session 3, the convex adapter, on main.
   Spawning it earlier would let a tank's belt find upward support on an invisible vertical face:
   a perch. **Recommended:** wait. Until then a tank can still drive off the edge, as today.
2. **Haze default.** 12 km under the playtest fork (alternatives 8 km and 20 km). Settled in play;
   nothing is needed before L2a.
3. **When the outland gets authored land.** Odd-reflection height now; an authored outland, or
   terrain past the boundary, comes with #221's map plan. **Recommended:** keep the odd reflection
   for the playtest.

Not decided here: the LOW reference (GTX 1650 or Steam Deck) belongs to #151, and these lanes price
on the M4.

Settled since revision 1:
- Rounds past the edge fly through the outland for the playtest (#280).
- **The invisible wall has no grip** (owner, 2026-11-06): `friction: 0`, a `Taste` for feel in
  `boundary.ron`, so a tank grinding into the wall slides along it and cannot rear up or climb it.
  It replaces the borrowed terrain friction. §3.6 shows that zero holds through Avian's combine
  and the track's grip law.
