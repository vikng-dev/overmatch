# Pipeline references: generate + fracture buildings, structure-aware (game destruction state only)

Subject: game environment destruction as game state (pieces, links, anchors). All sources read 2026-10-30.
Tags: [S] read in source today; [S~] via search summary / listing only; [I] inference; [?] not found / not verified.
Numbers: REPORTED (stated by the source) or DERIVED (mine). Extends `authoring-tools.md` (Lane 0, Lane 3) and
`destruction-structural-houdini-2026-08-13.md` Part A; does not repeat them.

## Source table

| # | Source | What it gives us | License / copyright | Permitted use |
|---|---|---|---|---|
| 1a | NVIDIA Blast in **NVIDIA-Omniverse/PhysX** `blast/` (v5.0.6) — https://github.com/NVIDIA-Omniverse/PhysX/tree/main/blast | Chunk hierarchy + support flag + exact cover; bond {normal, area, centroid}; world bonds; CSR support graph; 3-stage damage (generate → apply → split); stress solver | BSD-3-Clause: repo `LICENSE.md` "Copyright (c) 2008-2025, NVIDIA Corporation"; `blast/PACKAGE-LICENSES/blast-sdk-LICENSE.md` BSD-3 text, "Copyright (c) 2016-2024" [S] | Code reusable under BSD-3 (keep notice + disclaimer, no NVIDIA endorsement); we take data model / ideas |
| 1b | NVIDIA Blast in **NVIDIAGameWorks/Blast** (v5.1.0, 28-Sep-2026) — https://github.com/NVIDIAGameWorks/Blast | Same SDK, revived standalone: builds without PhysX by default, docs in-repo (`docs/api/*.rst`) | Same BSD-3 text in `PACKAGE-LICENSES/blast-sdk-LICENSE.md`; GitHub API reports `license: null` only because there is no root LICENSE file [S]. Bundled deps (boost, vhacd, tinyobjloader) carry their own files, not read [?] | As 1a |
| 2a | Raghavachary 2002, "Fracture generation on polygonal meshes using Voronoi polygons", SIGGRAPH 2002 sketch — https://history.siggraph.org/learning/fracture-generation-on-polygonal-meshes-using-voronoi-polygons-by-paul | The usual citation for "Voronoi of scattered points = fracture pattern" | ACM SIGGRAPH copyright (sketch) [S~]; full text not found [?] | Citation / ideas only |
| 2b | Voro++ (Rycroft) — https://github.com/chr1shr/voro (LICENSE at repo root) | Cell-based Voronoi: each cell computed individually by plane-cutting an initial box — the Lane 0 algorithm | Modified BSD-3 (LBNL): "Copyright (c) 2008, The Regents of the University of California, through Lawrence Berkeley National Laboratory"; plus an "Enhancements" clause granting LBNL a licence to any fixes we publish [S] | Code reusable under its BSD terms; we only need the idea |
| 2c | Schvartzman & Otaduy 2014, "Fracture animation based on high-dimensional Voronoi diagrams", I3D 2014 pp. 15–22 — https://dblp.uni-trier.de/rec/bibtex/conf/si3d/SchvartzmanO14 | Non-convex / deformation-guided cells via a higher-dimensional centroidal Voronoi | ACM copyright (I3D proceedings) [S~] | Ideas only |
| 3 | Müller, Chentanez, Kim 2013, "Real Time Dynamic Fracture with Volumetric Approximate Convex Decompositions", ACM TOG 32(4) Art. 115 (SIGGRAPH 2013) — https://history.siggraph.org/?p=108389 | Convex-piece compound; pattern-at-impact; convex∩convex clip; island split; local fracture | ACM copyright (TOG) [S~]; author PDF not reachable today (host down / bad cert) [?]; DOI not verified [?] | Ideas only |
| 4 | Blender Cell Fracture — archived https://github.com/blender/blender-addons/tree/main/object_fracture_cell ; Extensions-platform v0.2.1 | Per-seed convex cell from bisector + bounds planes; per-axis anisotropic metric; Boolean with source mesh | `fracture_cell_calc.py` SPDX header: "2012 Blender Foundation", **GPL-2.0-or-later** [S]; Extensions v0.2.1 lists GPL-3.0-or-later (Oct-26 doc) [S] | **Ideas only. Never copy into the tree.** |
| 5a | Müller, Wonka, Haegler, Ulmer, Van Gool 2006, "Procedural Modeling of Buildings" (CGA shape), ACM TOG 25(3) 614–623, DOI 10.1145/1141911.1141931 — https://history.siggraph.org/?p=113547 | Mass model → component split → facade subdivision with semantic labels; occlusion queries | ACM copyright [S~] | Ideas only |
| 5b | Wonka, Wimmer, Sillion, Ribarsky 2003, "Instant Architecture", ACM TOG 22(3) 669–677 (SIGGRAPH 2003) — https://cg.tuwien.ac.at/research/publications/2003/Wonka-2003-Ins | Split grammars: a shape is split into labelled sub-shapes; control grammar distributes attributes | ACM copyright [S~]; two DOIs circulate (10.1145/882262.882324 vs 10.1145/1201775.882324) [?] | Ideas only |
| 5c | Burgess & Purkeypile, GDC 2013 "Skyrim's Modular Approach to Level Design" — slides https://www.slideshare.net/JoelBurgess/gdc2013-kit-buildingfinal ; repost https://www.gamedeveloper.com/design/skyrim-s-modular-approach-to-level-design | Kits: a small set of grid-snapped pieces with known roles | Authors' talk, all rights reserved (no licence stated) [S~]; not opened today [?] | Ideas only |
| 5d | Gumin, WaveFunctionCollapse — https://github.com/mxgmn/WaveFunctionCollapse (LICENSE) | Tiled model: cells take labelled tiles under adjacency rules | **MIT**, "Copyright (c) 2016 Maxim Gumin" [S] | Code reusable under MIT; ideas suffice |
| 6 | SideFX Labs Building Generator 4.0, Building Generator Utility 2.0, Building from Patterns 1.1 — help text at https://github.com/sideeffects/SideFXLabs/tree/Development/help/nodes/sop (the public docs source) | Blockout → floors sliced → regions classified (wall, corner, ledge) → modules by pattern grammar | `LICENSE.md`: "Copyright (c) 2026 Side Effects Software Inc." BSD-style, 2 clauses (retain notice; no SideFX endorsement) [S]; GitHub says NOASSERTION. Houdini itself is under SideFX's EULA [I] | Labs files are permissive on their face, but they run only inside Houdini; we read public docs only → **ideas only** (per brief) |
| 7 | Whiting, Ochsendorf, Durand 2009, "Procedural Modeling of Structurally-Sound Masonry Buildings", ACM TOG 28(5) Art. 112 (SIGGRAPH Asia 2009) — https://people.csail.mit.edu/ewhiting/resources/pubs/WhitingOchsendorfDurand-09.pdf ; https://dspace.mit.edu/handle/1721.1/73112 | Grammar output → rigid blocks + contact interfaces + ground reactions, with semantic labels and "nonstructural" tags | ACM copyright; author copy on MIT DSpace [S~] | Ideas only |

## 1. NVIDIA Blast

**Where it lives today [S].** Two live homes, both BSD-3-Clause text:
- `NVIDIA-Omniverse/PhysX/blast/`: `VERSION.md` 5.0.6 (CHANGELOG 29-May-2024), last Blast-path commit 2025-04-07 "Fix Blast build issues"; the PhysX README lists `blast/` as "Blast SDK — destruction and fracture simulation"; docs at https://nvidia-omniverse.github.io/PhysX/blast/index.html.
- `NVIDIAGameWorks/Blast`: revived, not archived, `VERSION.md` 5.1.0; CHANGELOG "[5.1.0] - 28-Sep-2026: Build Blast without PhysX by default … Documentation is now in-repository". The newer version is here, not in the PhysX monorepo.

**Data model (headers read for the model only).** File paths are identical in both repos under `blast/` / root:
- `include/lowlevel/NvBlastTypes.h` [S]:
  - `NvBlastChunk { centroid[3], volume, parentChunkIndex (UINT32_MAX = root), firstChildIndex, childIndexStop, userData }`.
  - `NvBlastChunkDesc { centroid, volume, parentChunkDescIndex, flags (SupportFlag), userData }`.
  - `NvBlastBond { normal[3] ("interface average normal"), area, centroid[3], userData }`.
  - `NvBlastBondDesc { bond, chunkIndices[2] }`: one index = UINT32_MAX makes a **world bond**.
  - `NvBlastSupportGraph { nodeCount, chunkIndices, adjacencyPartition (nodeCount+1), adjacentNodeIndices, adjacentBondIndices }`: a CSR adjacency list.
  - `NvBlastActorDesc`: a uniform or per-bond initial health and a per-support-chunk initial health. A health above `kUnbreakableLimit = 0.5*FLT_MAX` (REPORTED) makes it unbreakable.
- **Hierarchy and depth** (`docs/api/introduction.rst`, `definitions.rst`) [S]. Chunks form trees, and several root chunks may coexist. Any chunk may be tagged support, but support chunks must form an **exact cover**: "a walk from any leaf chunk to its ancestor root chunk will always encounter exactly one support chunk". A helper repairs the flags by pushing support as close to the root as possible.
  - Support chunks need not be leaves or share a depth.
  - Chunks below support level (subsupport) have no bonds. Each becomes a lone actor with its own chunk health.
  - Blast renders **visible chunks**: an ancestor stands in for its support descendants while all of them stay in one actor.
- **Damage: bonds vs chunks** (`api_ll_users_guide.rst`) [S]. Damage runs in three stages: `NvBlastActorGenerateFracture` → `ApplyFracture` → `Split`.
  - A `NvBlastDamageProgram` has two parts. The **graph shader** runs on the support graph and emits bond (and support-chunk) health decrements as `NvBlastBondFractureData { node0, node1, health }`. The **subgraph shader** runs on lone lower-support chunks and emits `NvBlastChunkFractureData`.
  - Bonds whose health reaches ≤ 0 are broken. Island detection on the support graph then makes a new actor per island. Every chunk connected to the world lands in one "world-bound" actor.
  - Bond and chunk geometry is "only used by damage shader functions": Blast's core is purely topological.
- **Stress extension** (`include/extensions/stress/NvBlastExtStressSolver.h`, `docs/api/extensions/ext_stress.rst`) [S]:
  - Inputs, per node: mass, volume and local position. `setAllNodesInfoFromLL(density)` takes them from chunk centroid and volume. A static node has mass 0, all world nodes are static, and the docs say gravity needs at least one 0-mass node.
  - Loads: `addForce` (FORCE or ACCELERATION), `addGravity`, `addCentrifugalAcceleration`. Collision impulses can go to the nearest node, which breaks "in a weak place instead of the place of contact".
  - Computes: forces spread iteratively across bonds. Each bond's load is split into compression/tension (along the bond normal) and shear (perpendicular to it).
  - Limits: `{compression,tension,shear}{Elastic,Fatal}Limit`, in pascals. Below the elastic limit nothing happens, and above the fatal limit the bond breaks at once. Defaults are compression 1.0 / 2.0; tension and shear are −1, meaning "use compression" (REPORTED).
  - Outputs: `getOverstressedBondCount` → `generateFractureCommands` (bond fractures), `getExcessForces` per actor, linear and angular error, `converged()`.
  - Cost: linear in `maxSolverIterationsPerFrame`. `graphReductionLevel` merges nodes ~2^level (REPORTED); the header calls it "fairly random … can lead to interlocked actors".
- **Authoring bond generation** (`include/extensions/authoring/NvBlastExtAuthoringBondGenerator.h`, `ext_authoring.rst`) [S]:
  - `FractureTool` fractures by Voronoi, by slicing (optionally noisy), or by "cutout" (slicing driven by an image). It can trim convex hulls against each other to stop neighbours interpenetrating.
  - `BondGenerationConfig { bondMode: EXACT | AVERAGE, maxSeparation }`. EXACT finds the true common surface. AVERAGE projects onto or intersects a midplane, for noisy or interpenetrating chunks.
  - Four entry points:
    1. `buildDescFromInternalFracture`: "based on marking triangles during fracture process". Bonds come from the cut itself, as in our Lane 0.
    2. `createBondBetweenMeshes`: for given pairs.
    3. `bondsFromPrefractured`: when "there is no info about which chunks should be connected", with `meshGroups`; the semantics of `meshGroups` were not read [?].
    4. A collision-hull variant of `bondsFromPrefractured`.
- **What it gives a structure-aware fracturer [I].** The two-level hierarchy maps directly onto structure: depth 0 = building, depth 1 = structural element (wall panel, slab, lintel, column), depth 2 = fracture cells.
  - Make the **elements** the support chunks, bonded across structural interfaces. The cells below them are then cosmetic subsupport debris.
  - Alternatively, make the cells the support chunks and keep the element as the "visible chunk" while intact.
  - World bonds are the foundation anchors.
  - Per-bond initial health is the hook for "law × area × material" without per-tank or per-building constants.

## 2. Voronoi fracture literature, and material-aware seeds

- **Raghavachary 2002** [S~]: a SIGGRAPH 2002 sketch (session "Surface & Volumetric Techniques") that produces fracture patterns on polygonal surfaces from Voronoi polygons. Later work (Sellán et al., arXiv 2111.05249) cites it for "Voronoi diagrams of randomly scattered points capture the stochastic quality of fracture". The full text was not found [?]. Use it as the citation, not as a method.
- **Voro++** (Rycroft) [S, README]: "computing the Voronoi cell for each particle individually, rather than … as a global network". Each cell is an initial box cut by bisector planes, storing volume, centroid and faces. This is exactly Lane 0's per-seed clip, and its face list gives neighbour ids and face areas. The canonical paper is Rycroft 2009, *Chaos* 19, 041111 (AIP) [S~, from recall, not re-verified today].
- **Material-aware or anisotropic seeds.** Found:
  - (a) Blender Cell Fracture's `cell_scale` is applied to the **bisector normal** per axis (`points_scale`, "rotate plane to new distance") [S, GPL; idea only]. This is a cheap anisotropic metric. It stretches cells along an axis, as for wood grain or long masonry courses, without moving seeds.
  - (b) Whiting 2009 orients interfaces by element: "blocks in walls and columns are laid in horizontal courses, while the blocks in arches and flying buttresses are cut radially" [S]. That is a brick/course pattern chosen by element type.
  - (c) Schvartzman & Otaduy 2014 lift Voronoi into a higher-dimensional embedding to get non-convex, deformation-guided fragments with artist control [S~].
  - (d) Houdini's material-specific fracture (concrete/wood/glass) is already in the August doc.
  - No primary source was found for a brick-and-mortar seed distribution as such [?]. [I] A course pattern is not a Voronoi at all: it is a deterministic split (rows, then staggered heads). It belongs in the generator, and Voronoi cells go *inside* each brick panel, or not at all.

## 3. Müller, Chentanez, Kim 2013 (VACD)

Venue: ACM TOG 32(4), Art. 115, July 2013, 10 pp. (SIGGRAPH 2013), ACM copyright [S~]. Author PDF unreachable today; the claims below come from the search summary [S~] plus recall, marked [R] (recalled, not re-read on 2026-10-30).
- **Method.** The visual mesh is represented by a **volumetric approximate convex decomposition**, a compound of convex pieces [S~]. A **user-defined fracture pattern** (a set of convex cells, e.g. Voronoi) is placed relative to the **impact location** [S~]. Each convex piece is intersected with each pattern cell: convex ∩ convex, cheap and robust [R].
  - Pieces falling in the same cell form one fragment. **Island detection** splits a cell whose pieces are disconnected [R].
  - Fracture is **local**: cells outside the pattern's region of influence are merged back into one piece, so a large object only fractures near the impact [R].
  - The visual mesh is clipped by the convex pieces, so it need not be closed [R].
  - Fragments become **compound rigid bodies** of convex pieces [R].
  - Its value per the summary: the decomposition lets patterns "be applied locally at low cost" [S~].
- **Fits offline use [I].** The convex-pieces-as-ground-truth representation (already ADR-0008's compound), patterns as pre-authored data, and convex∩convex as the only geometric kernel. Lane 0 is VACD run at compile time with the pattern fixed per building. Pattern-at-impact is the runtime part we do not want, since runtime re-fracture breaks the "sim state built at spawn" rule. It is still useful as a *future* option, if the server applied a seeded pattern deterministically and replicated only (pattern id, pose).

## 4. Blender Cell Fracture (ideas only, GPL)

License: `SPDX-License-Identifier: GPL-2.0-or-later`, "2012 Blender Foundation", in the archived `blender/blender-addons` (`object_fracture_cell/fracture_cell_calc.py`) [S]. The Extensions-platform v0.2.1 is listed as GPL-3.0-or-later [S, Oct-26 doc].

Algorithm, ideas only [S]: seed points are collected from verts, particles or strokes, with optional noise and shuffle. Then, for each seed:
1. Start from the 6 planes of the source's bounds (+ a margin).
2. Sort the other seeds by distance and add each perpendicular-bisector plane, with an optional `margin_cell` gap and an optional per-axis scale on the normal.
3. Early-out once a seed lies beyond `distance_max`, so far seeds cannot cut the cell.
4. `mathutils.geometry.points_in_planes` returns the cell's vertices and the planes that actually bound it, and a convex hull makes the mesh.

For a non-convex source, a Boolean (intersect) modifier against the original cuts each cell down, with optional island split. Recursion re-fractures chosen cells. Nothing records adjacency, and the RNG is unseeded (Oct-26 doc). Our Lane 0 must add both.

## 5. Procedural building generation: structure that can label pieces

- **CGA shape, Müller et al. 2006** [S~ abstract, details R]:
  - Shapes carry a *scope* (an oriented box) and a symbol.
  - A **mass model** is a union of volumetric primitives.
  - `Comp(f)` splits a volume into its faces (front/side/top), which become facade shapes. `Subdiv` and `Repeat` split along an axis into floors, then tiles, then window or door plus wall.
  - **Occlusion queries** stop windows or doors where another mass occludes the wall, and snap lines align splits across facades.
  - Labels come free [I]: every terminal knows whether it is a wall tile, floor band, window, door, ledge or roof. A split along Y between floor bands *is* a floor line, and a window's scope gives its opening plus the band above it (the lintel).
- **Split grammars, Wonka et al. 2003** [S~]: a parametric set grammar based on shape. Splits produce labelled sub-shapes, and a control grammar propagates attributes such as style and material through the hierarchy [R]. For us [I]: the split tree *is* a chunk hierarchy, building → floor → wall tile → opening/pier/lintel. Each split plane is a candidate structural interface with a known area.
- **Modular kits, Burgess / Purkeypile GDC 2013** [S~, not opened]: a small set of grid-snapped pieces with fixed roles (wall, corner, doorway, floor) [R]. For us [I]: a kit piece is a pre-labelled structural element. Its snap faces are the bond faces, and one fracture template per kit piece (seeds in the piece frame) gives per-instance fracture for free, seeded by instance.
- **WFC (MIT)** [S]: in the simple tiled model, each cell picks a tile from a set under adjacency constraints between tile edges. For us [I]: the solved grid gives each voxel its role (wall, window, corner, floor, empty), and the adjacency rules already say which faces join solid to solid. Bonds are the solved solid–solid faces, and anchors are the solid faces at y = 0.
  - Stålberg's Townscaper / Bad North talks were not fetched [?].

## 6. SideFX Labs Building Generator (public docs only; ideas only)

License [S]: `SideFXLabs/LICENSE.md`, "Copyright (c) 2026 Side Effects Software Inc.", is a permissive BSD-style text with 2 clauses (keep the notice; no SideFX endorsement). The HDAs need Houdini, which has its own EULA [I]. We read only the help text, which is the docs source.

What the docs say [S]:
- **Building Generator 4.0** "converts simple blockout geometry into detailed, modular buildings". It "slices them into floors, identifies structural regions such as walls, corners, and ledges, and replaces those regions with high-resolution modules sourced from a user-defined module library". Parameters: full floor height, seed, facade module pattern, concave/convex corner modules, top/bottom ledges, "sideslop" filler, and per-floor index/height overrides (floor 0 = ground).
- **Building Generator Utility 2.0** defines modules: name, weight, dimensions (auto or manual, because "modules containing protruding elements" have misleading bounds), priority and variations. It also defines **floor descriptions** as a pattern language: `<A>` fills and repeats, `[A]` places once, `[A]3` repeats 3 times, `A*` picks a variation. Corners, and hand-placed overrides by bounding box, complete it.
- **Building from Patterns 1.1** stacks floor descriptions bottom-up with the same `<>` / `[]` grammar, per blockout group. It can output a point cloud (module name + orient) for engine-side instancing.

Gives us [I]: a proven factoring to copy as data, not code: blockout → floor slicing → face classification (wall, corner, ledge) → a 1-D module pattern per floor side. Every module instance is a labelled element with a known cell (floor index, side, slot), which is exactly the element layer a fracturer needs.

## 7. Structure-aware: Whiting, Ochsendorf, Durand 2009

[S] Production rules (repeat, split, …) in the Müller 2006 style produce "a mass model representing the blocks of the structure, including the interfaces between all adjacent blocks". The system "carries semantic information including architectural labels (e.g. arch, wall, column) and rule parameters".
- Shapes can be tagged **"nonstructural"** and are then excluded from the analysis, e.g. a light roof frame or decoration.
- Interfaces are computed after generation, assuming "neighboring blocks have coplanar faces and do not interpenetrate", with bounding-box acceleration.
- Interface orientation is chosen per element: courses for walls and columns, radial cuts for arches and buttresses.
- The statics are rigid blocks with a 3-D force at each interface vertex: equilibrium per block, a friction constraint, and compression only (`f_n ≥ 0`). Reaction forces at the ground plane act as anchors.
- A penalty "measure of infeasibility" lets a parameter search tune thickness and size until the building stands.

For us [I]: this is the published proof that a grammar's split structure yields the block-and-interface graph directly, with labels. The "nonstructural" tag is our cosmetic-piece flag, and the per-element interface orientation is our seed/cut policy.

## Synthesis: generated structure → structure-aware fracture → support graph [I]

1. Generate as a split tree (CGA / split grammar / Labs-style floor patterns): mass → floors → facade tiles → {pier, opening, lintel, sill}, plus slabs and roof. Every terminal is an **element** with a label, a scope (oriented box, thickness = wall depth) and a material key.
2. Elements are the Blast "support" layer. Their shared split faces become **links** (area, centroid and normal are exact: the face is a scope rectangle). Openings are *absent* volume, so the piers either side of a window bond only through the lintel and sill. Load paths come out of the geometry with no authoring.
3. Bonds only across structural interfaces: wall–wall at corners, wall–slab at floor lines, lintel–pier at openings. Elements tagged nonstructural (trim, ledges, Whiting's "nonstructural") get one weak link to their host and no load role.
4. Ground anchors are the element faces lying on the foundation plane (world links with real area, Blast's `UINT32_MAX` bond). The "attached / detached" law is then union-find to an anchor (Part A).
5. Fracture is *per element*, inside its own scope, never across a structural interface. Seeds and cuts are aligned to element axes: courses for masonry walls (Whiting), a per-axis metric for timber (Cell Fracture's scaled bisector, re-derived), coarse slabs for floors, and no seeds in glass openings (one removable piece).
6. Cells inside an element are subsupport debris (Blast's lower level). Their mutual faces come free from the Lane 0 clip but carry no load. The intact element renders as one "visible chunk" until damaged.
7. Seed streams are keyed by (asset, element path in the split tree, instance), so a module reused 40 times fractures 40 different ways, deterministically, and recooks bit-identical.
8. Per-link strength stays a law: area × material × interface type (course joint, slab seat, lintel bearing), in data. Blast's elastic/fatal limits in pascals × bond area show the shape of that law, and per-building constants never enter.
9. Optional later: a static check at compile time (Whiting's compression-only equilibrium, or Blast-style stress at rest) catches generated buildings that cannot stand, and sets The Finals' "baseline" so only excess load breaks links.
