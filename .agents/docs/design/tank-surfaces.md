# Tank surfaces: materials, visuals and the livery

**Status:** stage 1 built (2026-10-31, epic #177): the shared visual library, the material table's
visual column, the livery data and one generic tank surface material. Later stages are listed at the
end. Labels: **MEASURED**, **DERIVED**, **ASSUMED**, **CLAIMED**. Research behind this doc:
`.agents/scratch/tank-texturing/scout-2026-10-30.md` (survey, pipeline constraints) and
`.agents/scratch/tank-texturing/tiger-materials-2026-10-30.md` (what the fixture tank's parts were).

## 1. The model (owner's decisions, 2026-10-30)

- **A material is the one property a face wears in Blender** — the material slot. There is one
  material per distinct ballistic behaviour: Rolled Armour, Cast Armour, Mild Sheet, Cast Mild, Gun
  Steel, Track Steel, Rubber, Glass, Wood and so on. Its numbers live in
  `assets/materials/materials.ron`; the material's name is the join key (design doc
  `armor-penetration-and-damage.md` §12: membership is the material, identity is never parsed).
- **Each material points to one visual** from a shared library (`visual: "<key>"`), and many
  materials share a visual: Cast Armour and Cast Mild both look like cast steel.
- **Whether a surface takes paint belongs to the visual**, not to the material: steel visuals take
  paint; rubber, glass, wood, brick and track steel do not. (This replaces the old per-substance
  `paintable` flag, which nothing read.)
- **The look stacks at runtime, bottom up:**
  1. the material's visual surface;
  2. Zimmerit, an optional coating layer;
  3. paint: one colour and a sheen;
  4. camouflage: one shared pattern plus a palette;
  5. markings;
  6. wear and dirt.

  Everything above the visual is data. Blender sets only the materials and, later, an unwrap.
- **Era-agnostic.** The library and the material list serve any era and any tank. The Tiger is the
  worked example (§4), never the spec (ADR-0034).

## 2. The material table

The era-agnostic list. "Key" is the `materials.ron` key, which existing data already uses (renames
are out of scope). Physical values and their provenance live in `materials.ron` beside each row;
the column here says where they come from.

| Material | Key | Visual | Takes paint | Physical values |
|---|---|---|---|---|
| Rolled Armour | `RHA` | `rolled_steel` | yes | the reference steel: an RHA-class datasheet (SSAB Armox 370T), MEASURED |
| Cast Armour | `Cast` | `cast_steel` | yes | factor DERIVED (cast armour ~5–15 % below rolled); no facts row |
| Mild Sheet | `MildSteel` | `rolled_steel` | yes | 1020 analogue (MakeItFrom), factor TUNED at ~0.8× RHA |
| Cast Mild | `CastMild` | `cast_steel` | yes | ASTM A27 grade 65-35, normalized (MakeItFrom, read 2026-10-30); factor TUNED at Mild Sheet's class |
| Gun Steel | `GunSteel` | `machined_steel` | yes | 4340 Q&T analogue (MakeItFrom) |
| Track Steel | `TrackSteel` | `track_steel` | no | Hadfield austenitic manganese, ASTM A128 class (Total Materia, Columbia alloy paper, via search summaries: CLAIMED); factor UNKNOWN |
| Rubber | `Rubber` | `rubber` | no | none sourced |
| Glass | — (not yet a material) | `glass` | no | none; added when a vision block wears it |
| Wood | `Wood` | `wood` | no | USDA FPL Wood Handbook |
| Engine block (interior) | `EngineBlock` | `cast_steel` | yes | grey cast iron × fill, DERIVED |
| Stowed rounds (interior) | `Ammunition` | `machined_steel` | yes | unknown |
| Penetrator core (worn by no tank) | `TungstenCarbide` | `machined_steel` | yes | CoorsTek datasheet |
| Masonry (buildings) | `Brick` | `brick` | no | BIA Technical Note 3A |

Every `Hard` row names a visual; the parser refuses one that does not. A `Witness` (crew, `Flesh`) is
not matter and names none: crew boxes keep the glb's flat legibility colour. An `Unworn` row (a burst
core) is worn by nothing and names none.

The two interior rows take paint because their visual does. Nothing draws them through a closed hull,
so the livery reaches them only in principle. If an interior ever shows (a launched turret's open
ring), the honest fix is an interior livery, not a second copy of a visual.

**Cast Mild and Track Steel are new rows with no mesh wearing them yet.** Each carries its handbook
facts; neither factor is a tuned fit. Cast Mild's factor is Mild Sheet's declared 800 on purpose: the
machinery castings this row will take over (sprockets, idlers) are in `MildSteel` today, so moving
them changes nothing ballistic until a law reads the facts (`balance-lives-in-data`: a declared factor
far from its facts' strength ratio, ≈ 385 DERIVED here, is a missing factor of the law, not a number
to tune). Track Steel's factor is UNKNOWN, carried at the same 800; the track links are not ballistic
volumes today.

## 3. The visual library

`assets/materials/visuals.ron`, view-only (the dedicated server never reads it). Maps live in
`assets/materials/visuals/<visual>/<visual>_{diff,nor_gl,arm}.ktx2`, 1024², UASTC 4×4 + zstd with a
full mip chain, cut by `scripts/encode-visuals-ktx2.sh` (the terrain's codec policy; loose KTX2
outside the glb, the terrain's way, because the glb door accepts no texture outside the glTF slots,
scout §1.4). Each folder's `cc.txt` names the source.

| Visual | Source | Licence | Repeat | Takes paint |
|---|---|---|---|---|
| `rolled_steel` | ambientCG Metal046A | CC0 | 1.0 m ASSUMED | yes |
| `cast_steel` | ambientCG Metal052C | CC0 | 0.5 m ASSUMED | yes |
| `machined_steel` | ambientCG Metal009 | CC0 | 0.5 m ASSUMED | yes |
| `track_steel` | ambientCG Metal046B (the track link's own source) | CC0 | 0.5 m ASSUMED | no |
| `rubber` | ambientCG Rubber004 | CC0 | 0.5 m ASSUMED | no |
| `wood` | Poly Haven rough_wood | CC0 | 0.5 m MEASURED (Poly Haven dimensions) | no |
| `brick` | Poly Haven brick_wall_006 (the buildings' wall pack, shared) | CC0 | 3.0 m MEASURED | no |
| `glass` | flat colour and factors: neither source carries a glass pack (APIs queried 2026-10-31) | — | — | no |

ambientCG publishes no real-world size, so those repeats are ASSUMED. Memory, DERIVED: a 1024² map
transcodes to 1 B/texel (BC7 or ASTC 4×4), +⅓ for mips ≈ 1.33 MiB, paid once per map in use,
whatever the number of tanks. A visual that takes paint loads its normal and ARM maps (≈ 2.7 MiB):
paint covers its albedo, which never loads (§6). One that does not loads all three (≈ 4 MiB). The
fixture tank wears three painted visuals (rolled, cast, machined) and one bare (rubber): ≈ 12 MiB,
plus the 1024² camouflage pattern ≈ 1.3 MiB.

## 4. Worked example: the Tiger I

The fixture tank's view glb wears these materials (MEASURED, parsing `assets/tiger_1/tiger_1.glb`'s
JSON, 2026-10-31). Blender is not touched in stage 1, so this is today's assignment.

| Part (primitives) | Material today | Visual | The research's verdict |
|---|---|---|---|
| hull plates, turret plates, cupola, commander's hatch (19) | `RHA` | rolled | rolled plate (A/B) |
| gun mantlet, gun mask, hull MG ball and mantlet (4) | `Cast` | cast | **open:** mantlet cast per Tank Encyclopedia (B), unresolved against Tank Archives; Jentz & Doyle or the Tigerfibel decides. MG ball: no source (GAP) |
| barrels, breech, MG bodies (6) | `GunSteel` | machined | forged and machined (B/C) |
| road-wheel discs (16), sprockets (2), idlers (2), smoke launchers (6) | `MildSteel` | rolled | discs pressed or welded (C). **Sprocket and idler: unverified** (C: cast or forged) — `CastMild` once a source settles it |
| road-wheel tyres (16) | `Rubber` | rubber | solid rubber tyre (B) |
| engine, transmission (2) | `EngineBlock` | cast | interior |
| ammunition racks (4) | `Ammunition` | machined | interior |
| crew stations (5) | `Flesh` (witness) | none | — |
| track link | `Mat_Track_Link` (not a material) | — | **unverified** (C: cast steel; pins forged). `TrackSteel` once the link wears a material; the belt keeps its own textured look until then |
| decor: tools, grilles, lights, fenders (`Mat_Visual`, `Mat_RunningGear_Paint`) | not a material | — | keeps its atlas until its faces get materials (stage 3) |

The early drum cupola was rolled, the later one cast (B); the model's cupola wears `RHA`.

The default livery (`assets/tiger_1/tiger_1.livery.ron`): Dunkelgelb (RAL 7028) matte, with Olivgrün
(RAL 6003) and Rotbraun (RAL 8017) stripes. Each colour is CLAIMED with its source in the file: no
RAL-published sRGB exists for 7028, so its value is a paint maker's matched swatch, and 6003 and 8017
are today's RAL Classic approximations, not wartime samples.

## 5. The livery

`assets/<tank>/<tank>.livery.ron`, view-only: `paint` (an sRGB colour and a sheen; perceptual
roughness is 1 − sheen) and an optional `camouflage` (a pattern key, the metres one repeat spans, up
to three colours). A second livery is a data edit: no code and no asset change.

Patterns are shared: `assets/materials/camouflage/<pattern>.ktx2`, a linear RGB mask whose channels
are the livery's colours' coverage, 0 where the paint shows. `stripes` is generated in-project
(`scripts/visuals/camo_stripes.py`, fixed seed, tiling by construction); its look is ASSUMED, not a
reconstruction of a documented scheme.

**Per-player liveries later.** Players will choose camouflage, so a livery must one day replicate.
Stage 1 builds nothing for it, and leaves this room: the livery is plain data the client reads, the
server never does, and the dress (§6) takes the livery it is handed. A chosen livery becomes a
component on the tank root that rides the wire; the dress then keys a material on (material, livery)
instead of material alone.

## 6. The tank surface material

`src/tank_surface.rs` and `assets/shaders/tank_surface.wgsl`: one generic
`ExtendedMaterial<StandardMaterial, TankSurface>` (the terrain blend's pattern).

- **Where it goes.** When a tank's view glb lands, every primitive wearing a material whose
  `materials.ron` row names a visual swaps its glb material for a surface material: one per
  (vehicle, material), shared by every instance of the vehicle. Decor, the collision and shadow
  proxies, the crew boxes and the track belt keep theirs. LOD rungs wear their source primitive's
  material (`geometry_lod`), and the pen cam's x-ray dresses and restores the surface material like
  the standard one.
- **Loading.** The swap happens the frame the view lands, before the visual's maps and the pattern
  have loaded, and bevy draws nothing of a material whose images are not yet on the GPU. Until they
  land, a surface material wears its glb material's own look (less its normal map) and binds no
  image of its own; it is rewritten in place a frame after the last one lands, so rungs and the
  x-ray, which hold its handle, follow. A painted visual never loads its pack's albedo: paint
  covers it whole.
- **Projection.** The plates' UVs are collapsed (scout §1.2), so the shader projects triplanar,
  never through UVs. The projection frame is the vehicle's **rest frame up to the nearest moving
  part**: each primitive's mesh-local position through its node chain, stopping below the first node
  the tank file articulates (servo, recoiling barrel, road-wheel station). Plates of one rigid part
  share one frame, so a stripe runs on across a weld; a part that moves carries its pattern with it,
  so nothing swims. The per-primitive transforms ride a storage table indexed by `MeshTag`, built once
  per vehicle at its first bind.
- **Layers.** A visual that takes paint: the livery's colour, matte by its sheen, metallic 0, the
  camouflage mixed over it by the pattern's coverage; the visual's normal and AO still show through.
  A visual that does not: its own albedo, roughness and metallic. Normals are blended by the whiteout
  rule; a projection whose weight is zero is not sampled.
- **Cost.** Not measured in the lane (other bays compile in parallel); the orchestrator prices it.
  Texture requests per pixel, DERIVED from the shader: a flat face reads one projection, 2 maps under
  paint (normal, ARM) plus 1 pattern, or 3 maps without paint; an edge blending two projections reads
  twice that.

## 7. Not built yet

- **Zimmerit, markings, wear and dirt.** The stack's layers 2, 5 and 6. Wear needs a per-tank mask,
  which needs an unwrap.
- **The unwrap and baked masks** (scout's stage 2): a unique UV1 per tank with AO and curvature bakes,
  camouflage coverage per pixel, edge wear.
- **Decor** (stage 3): the `Mat_Visual` atlas's faces get materials, and its 64 MiB atlas goes.
- **The pen cam's own views** (the stage, the kill cam's shooter, the demo) are separate scene
  instances and keep the glb's flat colours.
- **The glb's rubber textures** (BlenderKit, 2K) still ship inside the glb and load, unused, until the
  next export drops them.
