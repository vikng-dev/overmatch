# Destructible walls and houses: the design space, for one grilling session

Scout report, 2026-10-30. **Subject: the game's environment destruction, which is game state only**
(collision, cover, sight, traversal, HP pools, dose). Vocabulary per `.agents/GLOSSARY.md` and
AGENTS.md: a panel's HP pool is **emptied**, a structure is **knocked out** of standing, rounds and
fields **dose** what they cross. Destruction slice 2 of epic #106. Read-only: no repo edits, builds
or runs. Numbers are labelled MEASURED (a repo run measured it), DERIVED (arithmetic here or in a
cited note), ASSUMED (a choice made here), CLAIMED (a source asserts it). Survey tags: [S] read in
the source, [S~] via a summary or secondary, [I] inference, [?] not found.

## Gist

1. **Today rounds fly straight through houses:** a house is one cuboid on `[Terrain]` only, and the march never tests that layer (`src/scatter.rs:256`, `src/ballistics.rs:772-810`). Only hulls and the aim ray meet it.
2. **Recommendation, hybrid (e): walls as panels with HP pools, like trunk segments,** plus a derived support rule, derived rubble and client-only debris. A shot breaks the panel it emptied, walls crumble piece by piece, an unsupported roof drops, and a house shot to pieces ends as grounded stubs and rubble, with no authored ruin.
3. **Panels are the generator's own elements** (piers, lintels, slabs between openings and floor lines). The owner's Rust generate-and-fracture pipeline (§5: CGA, split grammars, Blast's bond model, Whiting 2009) emits them with their links and anchors, so the fracture is structure-aware by construction.
4. **It reuses the tree slice almost whole:** identity, the ledger (generalised past trees), derive on arrival, `PoolEmptied`, the ram law, the transition tick, the merged view, the digest. **No new damage law:** the walk and the fields already dose any hard box.
5. **Wire:** only shot or rammed panels cross it (~8 B each, DERIVED); collapse and rubble derive on every peer from integers and compiler data, with no trig. That is a lower determinism risk than the trees'.
6. **Cost is event-driven:** ~888 static panel colliders (DERIVED) against the trees' 9,768, which cost ~1.08 ms of idle tick (MEASURED). The server uses 86–89 % of its tick at 30 tanks, and the LOW-class frame is ~19 ms against 8.33 (night results).
7. **Rejected:** a two-state swap alone (misses #106's crumbling walls); chunks released by physics as gameplay (server tick, ~84 KB/s per collapse per client, DERIVED); voxels or runtime fracture (everything rebuilt).
8. **Departures are stated per reference** (§3): WoT's two states, The Finals' solver and transform stream, Siege's runtime cut, BF6's hand-made piles and Chaos's server clusters.
9. **Slices:** B0 houses block rounds → B1 panels break (the compiler's first stage) → B2 rams → B3 collapse → B4 presentation → B5 the 30-tank gate.
10. **Ten owner questions (§7), each with a recommended answer.**

## 1. Where the repo stands (only what bears on buildings)

| Fact | Evidence |
|---|---|
| A house is `Collider::cuboid(1,1,1)` under a scaled `Transform`, `RigidBody::Static`, `[Terrain] × ALL`, plus a `TerrainMap` block the belts sense. 74 houses, 0 churches placed (MEASURED, `codebase-scout.md` §1). | `src/scatter.rs:256`, `:278-299`; `assets/maps/kalinovo/prototypes.ron` (`house_proxy` 4 × 3 × 6 m incl. a 0.5 m skirt) |
| Rounds never meet a house: armour rays filter `Layer::Armor`, the ground is `HeightGrid::cast_ray`. Hulls stop at a house by convex contact; the aim ray sees `Terrain | Armor`. | `src/ballistics.rs:772-810`, `:812-910`; `src/aim.rs:79`; `src/tank/spawn.rs:872` |
| A tree is the template: one static root with the state, one child per segment with a convex frustum on `[Terrain, Armor]`, its own `BallisticVolume` and `ComponentHealth`; `PoolEmptied` breaks it at that segment; a ram breaks it when `Σ impulse × lever ≥ fell_k × dbh³` in one tick; every peer derives the body from the state word. | `src/trees.rs:1-34`, `:888-1070` |
| The ledger: one replicated entity, `(index u16, word u16, tick u32)` entries appended by replicon diffs; a late joiner gets the whole list; the replica refuses a word no state of that tree encodes. Tree-specific in its apply (`TreeBody`, `TreeState`). | `src/net/destruction.rs:1-12`, `:26-46`, `:121-170` |
| Standing trees draw two merged instanced meshes; a fallen one draws its segmented glb hierarchy (#157, landed today). | `src/trees/view.rs:1-15` |
| `TerrainMap.revision` exists for "future streaming/destruction"; the belt field rebuilds on a bump (O(blocks)); nothing bumps it today. Spawn avoidance reads the live block list. | `src/world.rs:22-29`; `src/track/terrain.rs:42-53`; `codebase-scout.md` §1 |
| No damage law is per class: "same law, same path, no branch for every hard pool … only the consequence of an emptied pool is per class". Fields (spall cone, fragment cone) stop at the first hard body and dose it whole (`first_solid`). The **blast sphere is not built**. | #103 §1; `src/ballistics/resolve.rs:740-770`; `.agents/GLOSSARY.md:409`; design doc §5 "Not built" |
| Wood is `Hard`, 40 reference-mm per metre from crushing strength over the reference flow strength (Unknown provenance); no masonry or timber entry exists. | `assets/materials/materials.ron:221-228` |
| Performance: 30-tank frame on the M4 (≈ LOW's GTX 1650 GPU class) 19.3 ms p50 at 1440p, GPU-limited, CPU floor ≤ 12.5 ms; server tick +0.34 ms per tank, 13.4–13.9 ms p50 projected at 30 of a 15.6 ms tick; ~235 kB/s down per client at 30 tanks. | MEASURED / DERIVED, `perf-2026-10-29/night-results-2026-10-30.md` TLDR; `preset-tiers-research.md` |
| Segmented trees cost +0.73 to +1.09 ms per frame, all of it simulation (9,768 static colliders, ~1.08 ms idle tick). Cause unattributed; #150 measures it first. | MEASURED, `forest-frame-cost-2026-10-29.md`; #150 |
| Shadows (#170, owner direction 2026-10-30): three layers; trees and later houses in a world-anchored destructible bake, one tile redrawn per change, the falling object live in the cascades until it settles. Redraw cost unpriced. | #170 comments |

DERIVED consequence of the materials row: masonry by the same formula (crushing strength over the
reference flow strength, ASSUMED 10–40 MPa for brick to concrete) is 10–40 reference-mm per metre,
so a 0.4 m wall is 4–16 reference-mm. **A wall is concealment and a field shield far more than
armour against a solid round**, and under the energy-lost dose a solid round that perforates it
loses little energy and doses it little, while a burst's fragment cone enters it whole. That
reproduces War Thunder's observed "solid rounds barely touch buildings, burst rounds wreck them"
(`destruction-survey-2026-08-13.md` §6.2, [S~]) from the existing law, with no building rule.

## 2. The options

Every option keeps the settled contract of #106: destruction is server-owned state, never a physics
result; states on the wire, derived on arrival; built at spawn from data (ADR-0014).

### (a) Whole-object swap to a ruined state, plus debris

- **Player sees:** a house takes hits with no visible change (decals at most), then on its last HP collapses in one event into a ruin; debris is a client burst.
- **Authoritative change:** collision, cover, sight and traversal all flip at once to the ruin's authored colliders. No gap at the shot point; a wall cannot be breached without the whole house going.
- **Wire:** one ledger entry per building, ~8 B (DERIVED); WoT sends health 4 B + flag 1 B per object (CLAIMED, round 15).
- **Determinism:** trivial (two authored collider sets).
- **Cost at 30 tanks, LOW:** lowest. Intact set ~5 colliders per house (4 walls + roof); one swap and one shadow tile redraw per collapse.
- **Authoring:** per building type, two view models, two collider sets and a collapse effect; the ruin is a second model to build by hand or generate.
- **Verdict:** a valid floor and the cheapest, but it does not reach #106's destination ("walls with thickness that crumble", "segmented breaks at the shot point"). WoT's choice fits WoT, where a building is an HP bar on an arcade map. Kept as the terminal state that (b) derives, never authored separately.

### (b) Per-panel HP pools with pre-authored breaks (the trees' shape)

- **Player sees:** a shot breaks the panel it emptied: a wall-sized hole at the shot point, rubble at its foot, sight and fire through the gap. Repeated hits crumble a wall panel by panel; a tank rams a panel out and drives through.
- **Authoritative change:** per panel. A broken panel's collider leaves the walk and the hull world (`ColliderDisabled`, `engine-delta.md`, the no-despawn switch); a derived rubble block appears on its footprint (a hard body for rounds, fields and sight; a `TerrainMap` block the belts climb). Fields shadowed by the panel now pass.
- **Wire:** one entry per directly broken panel (~8 B, DERIVED from postcard varints: index 2 B, word 2–3 B, tick 3–5 B). A late joiner's snapshot is the whole ledger: at most ~888 house panels ≈ 7–9 KB (DERIVED, every panel shot out). Against ~235 kB/s per client in play, negligible.
- **Determinism:** the server's march and ram law decide (one authority); every peer derives from integers (which panels are down) and authored f32 data (panel slabs, rubble blocks precomputed by the asset build). No trig, no search at runtime: lower risk than the trees' rest pose.
- **Cost at 30 tanks, LOW:** ~888 static convex colliders (DERIVED, ASSUMED 12 per one-storey graybox house: 3 + 3 + 2 + 2 wall panels of 2 m + 2 roof slabs). On the trees' measured rate that is ~0.1 ms of idle server tick (DERIVED, ASSUMES the cost is linear in static colliders, which #150 has not attributed). Lever if needed: #150's "one simple collider per object until first dosed" (a wall is one slab until hit; the split is data, so deterministic). GPU: an intact house draws merged and instanced like a standing tree (#157); a damaged one re-merges its remaining panels on each change (event-driven), not one draw per panel (~900 draws worst case, DERIVED). One shadow tile redraw per break event, coalesced per frame.
- **Authoring:** per building type, a panel layout, which the Rust generator emits from its own split tree (§5); real buildings place kit meshes on those panels (standard modular practice, so the split costs little extra); edge caps revealed beside a broken panel (the trees' cap pattern, A5); one generic rubble mesh per material scaled to a footprint.
- **Verdict:** reaches the destination at the trees' price. Recommended core.

### (c) Pre-fractured chunks with constraint data, released by impulse (Havok/APEX/Blast walls, Battlefield, The Finals)

- **Player sees:** the most physical result: chunks break off where hit and the structure comes down under gravity.
- **Authoritative change, two sub-options:**
  - *Server-simulated chunks:* cover and traversal follow where the chunks settle. Server tick: dozens of dynamic bodies per collapse on a tick already at 86–89 % (DERIVED); Embark replaced the sequential-impulse solver for structure because it sagged wrongly (Avian is one; `destruction-survey-2026-08-13.md` §1.7 [S]). Wire: settling poses at 33 B each (`engine-delta.md` row 39, DERIVED) × 40 chunks × 64 Hz ≈ 84 KB/s per client per collapsing building for ~3 s (DERIVED); The Finals pays ~175 kbit/s peak after heavy compression (REPORTED).
  - *Client-simulated chunks:* poses diverge per client (APEX docs say so, [S~]); large chunks then become per-client cover and sight, which PRODUCT.md forbids ("Replication frequency, rendering LOD, occlusion … must not silently become … concealment rules"). Siege's own press line: falling debris processed per client "can create different sight lines for different players" (`destruction-structural-houdini-2026-08-13.md` A1, [S~]).
- **Wire (client-simulated):** a release bitmask, ~1 bit per chunk.
- **Determinism:** server-only sim is fine; replaying physics on clients is not.
- **Cost at 30 tanks, LOW:** 1,480–2,960 static chunk leaves at 20–40 per house (DERIVED) → ~0.16–0.33 ms of idle tick on the trees' rate (DERIVED, linear ASSUMED); live dynamic casters during each collapse; client bodies enter the unfiltered `aabb_intersections_with_aabb_callback` walks (`engine-delta.md` row 8).
- **Authoring:** fracture + links: ≈ 22 h for a Rust Voronoi cutter in the compiler, then ≈ 0 per building; Houdini Indie ≈ 38 h + $299 (DERIVED, `authoring-tools.md` §Lane 4).
- **Verdict:** as gameplay, too expensive for this server and frame. As presentation only, small and capped, it is (b)'s debris rung.

### (d) Voxels or runtime fracture (Teardown; Red Faction's GeoMod)

- **Player sees:** arbitrary holes of any shape at the exact shot point.
- **Authoritative change:** per voxel (or per runtime CSG cut).
- **Wire:** Teardown found voxel state "simply too much data" and shipped ordered destruction commands with join-in-progress capped or disabled after heavy destruction ([S~], `destruction-survey-2026-08-13.md` §5.2). A 10 cm grid over a 0.4 m-thick shell of the graybox house is ~24,000 voxels per house, ~1.8 M for the map (DERIVED).
- **Determinism:** integer voxel edits can be deterministic; parry's `Voxels` raycast is unverified for cross-platform bits, and the repo already routes the ground around parry's float path (`src/ballistics.rs:760-767`). Runtime float CSG agrees across platforms only if geometry is replicated.
- **Cost:** a new collector arm in the walk (it reads convex and certified trimesh only, `codebase-scout.md` §3), a voxel or remeshing renderer on a LOW GPU already at 2.3× budget, per-shot edit cost (Siege's procedural cut: ~0.33 ms per bullet hole, 8–10 ms per multi-layer explosion on PC, REPORTED §2.7).
- **Authoring:** a voxel pipeline beside Blender.
- **Verdict:** rejected on evidence: every cost axis is a rebuild, and the gain (hole shape finer than a panel) is presentation that decals and edge caps buy at a fraction of the cost.

### (e) Hybrid: recommended

(b)'s panels as the authoritative structure, plus:
- a **support rule** derived on every peer: a panel with no path to a ground-anchored panel through standing panels (and, recommended, no more than N panels of horizontal span from a grounded column, an integer from data, Valheim's shape) drops to rubble; only the panels a round or a ram broke are on the wire;
- **derived rubble**: a low block per dropped panel, precomputed by the asset build (footprint at the panel's base, height from its volume), cover for rounds, fields and sight, a block the belts climb;
- **(a) as the emergent end state**: a house shot to pieces is its grounded stubs plus rubble, never a second authored model;
- **(c)'s chunks as client-only presentation**: particles and, on higher presets, a capped few small rigid bits with no collision on any gameplay layer; never large enough to read as cover (Siege's rule);
- **decals for hits short of a break** (the hull-marks path, #124), so partial damage reads without geometry.

Fences, garden walls and sheds are the same asset type with one row of panels; there is no
separate clutter system (where WoT runs one, §3).

### Option table

| | (a) swap | (b) panels | (c) chunks | (d) voxels | (e) hybrid |
|---|---|---|---|---|---|
| Breach at the shot point | no | yes, panel-sized | yes | yes, exact | yes, panel-sized |
| Crumbles piecewise / collapses | one event | piecewise; no collapse | both, physical | both | both, derived |
| Cover, sight, traversal authoritative | yes | yes | only if server-simulated | yes | yes |
| Wire per event | ~8 B per building | ~8 B per broken panel | 1 bit per chunk, or ~84 KB/s while settling | commands; JIP replay | ~8 B per broken panel |
| Join snapshot (map) | ≤ 74 entries | ≤ ~7–9 KB | bitmask, or poses | command history | ≤ ~7–9 KB |
| Determinism risk | none | low (integers + data) | high if clients simulate | medium–high | low |
| Server idle tick (DERIVED, linear ASSUMED) | ~0.04 ms | ~0.1 ms | ~0.16–0.33 ms + dynamics | new collector | ~0.1 ms |
| Client frame while idle | ~0 | ~0 (merged) | ~0 | renderer | ~0 (merged) |
| Authoring per type | two models + two collider sets | panel layout (generated) | fracture + links (tooling) | voxel pipeline | panel layout (generated) |
| Reaches #106's destination | no | partly | yes | yes | yes |

## 3. How shipped games do it

Already covered in the repo (cited, not repeated): The Finals, Rainbow Six Siege, Battlefield 6
editorial, Red Faction Guerrilla's readable chunk format, Teardown via 80.lv, WoT's graphics toggle,
War Thunder's APEX devblog, APEX docs (`destruction-survey-2026-08-13.md`); the support-graph
spectrum, Blast, Chaos's derived connection graph, Valheim
(`destruction-structural-houdini-2026-08-13.md` Part A); Dagor's source, Arma Reforger, Squad, Hell
Let Loose's FAQ, Enlisted, BF6 (`games-and-dagor.md`); WoT's client scripts for trees, clutter,
contact and hull marks (`wot-scout-round1`–`11`); authoring tools (`authoring-tools.md`).

### New this pass

**World of Tanks, buildings and walls** (WoT client scripts via peer session, round 15, 2026-10-30;
CLAIMED; `wot-scout-round15-buildings.md`):
- Two systems split by size. Clutter (fences, posts, hedges, small walls, ~1 HP) is batched per map chunk as packed index arrays; at ≤ 19 HP a round passes through for a per-material penetration loss.
- Buildings and structural walls have exactly two authored states, alive and destroyed, each a visual model plus BSP collision; server float HP, dosed by solid and burst rounds alike (blast scaled 0.5 by default); destruction swaps the whole model and its collision. Nothing is pre-fractured; no per-fragment HP.
- Partial damage is decals only (the hull-sticker technology). Rubble blocks only if the ruin was authored with collision. Debris is never simulated or networked. Delta: health 4 B + flag 1 B + 8 B per mark.
- Both states load up front; the ruin is lower-poly with a simpler BSP.

**World of Tanks, official** (worldoftanks.com "20 secrets", read 2026-10-30) [S]: only some
one-storey buildings fall ("most two-story ones won't fall down"); a knocked-down tree conceals like
a bush; "the enemy sees trees and other destructible things knocked over even when they can't spot
you": destruction state reaches every client whatever spotting says. Our global ledger does the same,
and with no spotting rule it must.

**Unreal Engine 5 Chaos** (official docs: Destruction Overview; Python API
`GeometryCollectionComponent` 5.5; read 2026-10-30) [S]: pre-fractured clusters held by a
nearest-neighbour connection graph with strain limits; impulses from collisions or fields break
connections. Replication sends break state, and transforms only down to a chosen cluster level:
past `replication_abandon_after_level` the server stops updating fragments and clients simulate them
alone, optionally with no collision (`abandoned_collision_profile_name`); past
`replication_max_position_and_velocity_correction_level` a client receives only the initial break
velocity. Release notes via summary [S~]: 5.3 made server-authoritative particles unbreakable on the
client. **The engine-level form of "debris is never replicated": our panel is the abandon level, and
nothing below it is gameplay.**

**Frostbite 2** (Kihl, "Destruction Masking in Frostbite 2 using Volume Distance Fields", SIGGRAPH
2010 Advances course, PDF read 2026-10-30) [S]: destroying a part of a house removes a piece of
geometry, adds authored detail meshes around the hole, then draws a distance-field mask from
auto-generated spheres per destructible part (~2 m per texel, typically 8×8×8, REPORTED). Frostbite 1
needed a hand-made UV mask per destructible part; that workflow cost drove the rewrite. **The look of
a breach is derived from which parts are gone**, which is B4's edge caps and mask.

**Red Faction Guerrilla / Armageddon** (CBS Q&A with Volition's Eric Arnold; Game Developer,
"The Destructible World: Building Red Faction: Armageddon" with Chad Greene; read 2026-10-30) [S]:
buildings carry invisible pre-made shards unhidden on destruction, shard size per material, a
separate high-detail intact model; "world anchoring" decides what stays; "artists had to become
structural engineers to prevent their creations from crumbling as soon as they are dropped in
game"; a vehicle on a weakened bridge brings it down. The GDC 2009 talk ("Blowing Up the Outside
World", Arnold and Hanna) is listed but its media were unreachable [?].

**Company of Heroes 3** (SEGA press "Destruction Dev Diary" [S]; Relic's own fracturing post
unreachable, via summaries [S~]; read 2026-10-30): progressive damage (marks, then holes) ending in
collapse; a collapse can leave new cover inside the remaining walls; falling debris doses nearby
units; buildings are layered shells with damage states by where they were hit. HP model, sight
after collapse and sync [?].

**THE FINALS, Season 12** (2026-10-20, patch notes via MP1st [S~]; read 2026-10-30): gradual
destruction (buildings "hinge, sag, bend, and deform"), blast waves, a physics-engine upgrade "to
handle the extra load", "debris piles that dynamically grow". The reference keeps moving toward more
server physics, the direction our server tick cannot follow.

**Teardown** (Gustafsson, "The unlikely story of Teardown Multiplayer", 2026-03-13, primary read
2026-10-30) [S]: destruction rewritten in fixed point and replicated as a reliable ordered stream of
deterministic commands ("cut hole in this shape at voxel coord x,y,z"); transforms by unreliable,
per-client-prioritised state sync at ~1 Mbit/s per client (REPORTED); late join replays the command
log, buffer-capped, and join-in-progress is disabled when it fills; the 2021 prototype that sent
voxel data "completely choked the connection".

**Hell Let Loose** (2020 FAQ via the Wayback Machine, read 2026-10-30) [S]: "bridges, walls and
fences are at the top of the destruction priority list" because they shape "the flow of traffic …
for armor and vehicles"; complex building and foliage destruction was tested and dropped because
"the load on the network would compromise the total player count". **Squad:** no building
destruction found through its UE5 move (9.0) [?].

**Valheim** (secondaries [S~]): support decays with the connection count from a ground-anchored
piece; longer pieces span more; a colour readout shows what is left. The cheap, deterministic
graph-distance rule recommended for B3.

Not reached in the time box: DICE talks on destruction networking or Levolution; Havok's
breakable-constraint docs. The survey's full notes:
`building-destruction-web-survey.md` beside this report.

### Where this design departs from each reference, and why

| Reference | What we keep | Where we depart | Why |
|---|---|---|---|
| WoT buildings (r15) | server HP, state on the wire, whole collision swap per state, debris client-only, partial damage as decals, both states loaded up front | per-panel states instead of two per building | #106 asks for breaks at the shot point and crumbling walls; WoT is arcade and its building is an HP bar |
| WoT clutter (r1, r15) | small objects cheap and batched (our ledger is already one entity) | no `maxHpForShootingThrough` flag and no second system | the walk already charges chord × factor, so a fence costs a round almost nothing by law, and its small pool empties on a hit; the pass-through emerges from data |
| Dagor / War Thunder | snapshot + deltas, destroyed set keyed by stable ids, swap to an authored destroyed state, impulse threshold for hull contact | no client prediction and restore; contact law read after the solver (default) | one timeline (ADR-0037): clients wait ~100 ms for the flip; Dagor's restore machinery exists because its clients destroy first |
| The Finals | derived connection graph, server-owned, buildings from a generator | no structural solver, no transform replication | server tick at 86–89 % at 30 tanks; Avian is sequential-impulse, which Embark showed sags wrongly; 84 KB/s per collapse (DERIVED) |
| Rainbow Six Siege | debris small, never replicated, never cover; two collision tiers | no runtime procedural cut | ms-scale cut cost per hit (REPORTED) and per-client sight lines; the panel is our cut |
| Battlefield 6 | per-part health with transitions; persistent rubble as cover; the apple-core | rubble derived per panel, not level-designed piles | in-house solo authoring; a derived pile costs nothing per building |
| Red Faction Guerrilla | support graph over pieces | connectivity (plus a span limit), no stress | stress forced Volition's artists "to be architects" (`destruction-structural-houdini-2026-08-13.md` A2 [S]): per-building tuning we cannot pay |
| Teardown | — | no voxels | wire, walk, renderer, memory; JIP capped after heavy destruction |
| Arma Reforger | authored damage states, ruins | no delayed collapse in the sim | a delayed sim collapse puts presentation timing into authority; the view may delay instead (fork) |
| UE5 Chaos | break state replicated, fragments below a level abandoned to clients with no collision | no impulse-driven clusters; the abandon level is the panel and nothing is simulated below it on the server | server tick; a panel is the smallest gameplay piece by design |
| Frostbite 2 | the breach look derived from which parts are gone | edge caps and a mask from the panel set, no per-part artist spheres | the mask is a view; the generator knows every panel's edges |
| Company of Heroes 3 | collapse leaves cover; damage shown by where a building was hit | a collapse doses no tank | simplest model: debris is presentation; a collapse dose would need a field from rubble, a law we do not have |
| Hell Let Loose | walls, fences and bridges first, for traffic flow | buildings too | PRODUCT.md requires them; the per-panel ledger is bytes, not HLL's cost |

## 4. Rounds, fields and tanks against each option

What the damage model does today (`src/ballistics/` walk, #103): a round's line through a hard
volume slows by chord × the substance's factor, and the energy it lost doses that volume's pool at
the one energy-to-HP rate (`joules_per_hp` 3000 J, TUNED); a stopped round deposits all it had
left; a perforating round throws a spall cone from its body exit; an armed burst round throws its
fragment cone (the ring) where its delay runs out; every field direction stops at the first hard
body it enters and doses it whole; the blast sphere is not built. Tanks fell trees by
`Σ impulse × lever ≥ fell_k × dbh³` per tick, read after the solver (`src/trees.rs:936-1050`).

| Interaction | (a) swap | (b)/(e) panels | (c) chunks | (d) voxels |
|---|---|---|---|---|
| Solid round meets a wall | one building pool takes the line's energy; the wall stays until the house goes | the panel crossed takes it; a thin wall barely slows the round (4–16 reference-mm for 0.4 m, DERIVED) | per chunk, then a release impulse; the walk must read many small convex pieces | a new collector arm |
| Round crosses a house | two wall chords if walls are slabs (today: none) | two panels dosed, entry and exit | many chunks | many voxels |
| Burst round at a wall | its fragment cone doses the one pool | the ring's directions dose every panel they enter whole, and the panels shadow what is behind: a burst short of a wall spares the tank behind it | same, per chunk | per voxel |
| Blast sphere (not built) | a box like any other when it lands | each panel in reach, shadowed by panels in front | per chunk | per voxel |
| Spall cone from a wall exit | from the substance's data | same; masonry may author a wide debris cone, or none (data, not law) | same | same |
| Tank pushes or rams | building-level threshold | per panel, the trees' law generalised (below) | impulse into constraints (server solver) | per voxel |
| Rubble after a break | the ruin's authored collision | derived block per panel: stops rounds and fields, blocks sight, belts climb it | settled chunks (authoritative only server-side) | voxel debris |

**The ram law for panels (recommended, simplest model).** The tree law compares a tick's torque
about the base with `fell_k × dbh³`, which is a constant times the trunk's section modulus
(π d³ / 32). Generalise it to one law for every breakable piece: `Σ impulse × lever ≥ k_substance ×
Z`, where `Z` is the piece's section modulus about its base in the push direction (a wall panel of
width `w` and thickness `t`: `w t² / 6`, DERIVED from the shape at load). Trees keep their numbers
(their `fell_k` absorbs π/32); masonry gets its own `k`. It reuses `ram_contacts` and `ram_torque`
as they stand and the measured impulses (`ram-impulses.md`: push 13.3 kN·s·m, 10 km/h ram 119–151,
MEASURED), so calibration is a data row, not a measurement campaign. Thick walls hold, thin ones go.
Illustration on wood's constant (DERIVED; ram torque ≈ 46 kN·s per m/s × 1.15 m lever, MEASURED
inputs from `ram-impulses.md` §3e; masonry's real `k` is a later data row): a 2 m × 0.25 m panel
(`Z` 0.021 m³) breaks to a ~13 km/h ram, a 2 m × 0.4 m one (`Z` 0.053 m³) needs ~33 km/h, and both
hold a standstill push (13.3 kN·s·m against thresholds of 191 and 489).

**The hull at the break.** Read after the solver (as for trees), the tick of the break has already
stopped the hull: a 10 km/h ram leaves ~0.5 m/s (MEASURED for the trunk, `ram-impulses.md` §3c).
Dagor instead breaks inside the vehicle's collision pass and returns the impulse, so the vehicle
keeps its momentum minus the absorbed part (`games-and-dagor.md` §1.4, [S]). Avian's
`modify_contacts` runs before the solver and could discard the contact on a predicted impulse
(closing speed × hull mass), but no hook exists in the repo and the slot is project-wide
(`engine-delta.md` row 18). Default: the trees' after-solver read; record the feel as a playtest fork.

**Sight.** There is no spotting rule (PRODUCT.md): sight is what each client renders. So every
client must render the authoritative panel set and rubble, the ledger stays global
(`NetworkTarget::All`, no interest management), debris stays too small to conceal, and the LOD
certificate bounds a ruin's silhouette like any mesh.

**Traversal.** A house today is both a hull collider and a `TerrainMap` block the belts climb. On a
break: the panel's collider is disabled, its rubble block joins `TerrainMap.blocks`, and the
revision bumps (the belt field rebuilds in `PreUpdate`, O(blocks); spawn avoidance follows the live
list). Whether intact walls stay climbable blocks is question 9.

## 5. References for our own pipeline

Owner direction (2026-10-30): the long-term pipeline generates buildings and fractures them together,
structure-aware (panels, floors, openings, lintels), in Rust inside the map compiler
(`authoring-tools.md` Lane 0). **No map compiler exists yet**: it is a design
(`houdini-map-export-contract-brainstorm-2026-08-09.md`: validate, canonicalise, sort, hash, recook
twice in CI); today `map::load` parses `level.json` and `prototypes.ron` at startup. The building
generator is its first stage. Sources are open or published only; SideFX read through public docs
only; nothing decompiled. Full notes, with paths into each source: `pipeline-references.md` beside
this report (all read 2026-10-30).

| Source | What we take | License | Use |
|---|---|---|---|
| NVIDIA Blast, `NVIDIAGameWorks/Blast` v5.1.0 (2026-09-28) and `NVIDIA-Omniverse/PhysX/blast` v5.0.6 | The data model: a chunk hierarchy where support chunks form an exact cover; bonds `{normal, area, centroid}` between chunk pairs; world bonds as anchors; a CSR support graph; damage to bonds, then island detection; subsupport chunks as unbonded debris; bonds generated from the cut ("EXACT" mode); a stress extension with elastic and fatal limits per bond in pascals | BSD-3-Clause (`PACKAGE-LICENSES/blast-sdk-LICENSE.md`) [S] | Data model and ideas; code would be reusable with the notice, none is needed |
| Voro++ (Rycroft), github.com/chr1shr/voro | Each Voronoi cell computed alone by cutting a box with bisector planes, keeping its faces, neighbours and areas: Lane 0's kernel | Modified BSD-3 (LBNL), with a clause granting LBNL a licence to published fixes [S] | Idea; our convex clip is ours |
| Raghavachary 2002, SIGGRAPH sketch | The canonical citation for Voronoi fracture patterns | ACM copyright [S~] | Citation |
| Müller, Chentanez, Kim 2013, "Real Time Dynamic Fracture with Volumetric Approximate Convex Decompositions", ACM TOG 32(4) | Convex pieces as ground truth (ADR-0008's compound); a fracture pattern as data; convex ∩ convex as the one kernel; island split; fracture local to a region | ACM copyright [S~]; author PDF unreachable | Ideas; pattern-at-impact stays a future presentation option, never sim |
| Blender Cell Fracture (`object_fracture_cell`) | Per-seed cell from bisector planes; a per-axis scale on the bisector normal as an anisotropic metric (grain, courses) | GPL-2.0-or-later (archived add-on), GPL-3.0-or-later (Extensions v0.2.1) [S] | **Ideas only; never copied into the tree** |
| Müller, Wonka, Haegler, Ulmer, Van Gool 2006, "Procedural Modeling of Buildings" (CGA shape), ACM TOG 25(3) | Mass model → `Comp` into faces → `Subdiv`/`Repeat` into floors and tiles → opening, pier, lintel; every terminal labelled; occlusion queries stop openings where masses meet | ACM copyright [S~] | Ideas |
| Wonka, Wimmer, Sillion, Ribarsky 2003, "Instant Architecture", ACM TOG 22(3) | Split grammars: the split tree is a labelled hierarchy; every split plane a candidate interface of known area | ACM copyright [S~] | Ideas |
| Whiting, Ochsendorf, Durand 2009, "Procedural Modeling of Structurally-Sound Masonry Buildings", ACM TOG 28(5) | Grammar output → blocks plus interfaces with labels; a "nonstructural" tag; ground reactions as anchors; interface orientation per element (courses in walls, radial in arches); a compile-time feasibility check | ACM copyright; author copy on MIT DSpace [S~] | Ideas |
| Burgess and Purkeypile, GDC 2013, "Skyrim's Modular Approach to Level Design" | Kits of grid-snapped pieces with fixed roles: a kit piece is a pre-labelled element, its snap faces the bond faces | Talk, all rights reserved [S~]; slides not opened | Ideas |
| Gumin, WaveFunctionCollapse | Tiles under adjacency rules: solid–solid faces are bonds, solid faces at ground anchors | MIT [S] | Ideas; code reusable under MIT |
| SideFX Labs Building Generator 4.0, Utility 2.0, Building from Patterns 1.1 (public help text in `sideeffects/SideFXLabs`) | The factoring: blockout → floors sliced → regions classified (wall, corner, ledge) → a 1-D module pattern per floor side (`<A>` repeat, `[A]` once, `[A]3`, `A*` variation) | Labs files: BSD-style 2-clause, © Side Effects Software [S]; they run only inside Houdini, under SideFX's EULA | **Ideas only, from public docs** |

**How the generated structure feeds the fracture and the support graph** [I, after the sources above]:
1. The generator emits a split tree: mass → floors → facade tiles → {pier, opening, lintel, sill}, plus floor slabs and roof. Each terminal is an **element** with a label, an oriented box (thickness = wall depth) and a material key (CGA, split grammars, Labs' factoring).
2. Elements are the gameplay pieces: the panels of option (b), Blast's support chunks. The fracture at this level *is* the generator's own splits, so it is structure-aware by construction and needs no Voronoi.
3. Links only across structural interfaces the generator knows: wall–wall at corners, wall–slab at floor lines, lintel–pier at openings, wall–roof. Area, centroid and normal are exact (a shared face is a scope rectangle). An opening is absent volume, so the piers either side of a window bond through the lintel and the sill. Trim and ledges are tagged nonstructural (Whiting): one link to their host, no load role.
4. Anchors are the element faces on the foundation plane: world links with real area (Blast's world bond).
5. Below the element, an optional Voronoi or course fracture inside its own box only, seeded by (asset, element path, instance), for client debris (Blast's subsupport level, Chaos's abandoned level). Masonry by courses (Whiting), timber by a stretched metric (Cell Fracture's idea, re-derived), glass as one removable piece.
6. Link strength stays a law in data, area × material × interface type, never a per-building constant (ADR-0034's rule, read for buildings). A compile-time check that every generated building stands (Whiting's equilibrium, or a stress-at-rest baseline as in The Finals) can come later.

**How it maps onto the tree slice's contracts:**

| Tree slice | Building |
|---|---|
| `<id>.tree.ron` read at load, sim built at spawn from it, never from the glb (ADR-0014) | `<id>.building.ron` written by the compiler: elements (id = split-tree path, label, box, material), links `(a, b or world, area, centroid, normal)`, rubble block per element; the sim spawns from it |
| A segment is a child convex collider with its own pool | An element is a child convex collider with its own pool |
| One `TreeState` word per tree on the ledger | One ledger entry per directly broken element: `(instance ordinal, element + 1, tick)`; derived collapses never cross the wire |
| Derive on arrival: body and rest pose (libm trig) | Derive on arrival: disabled colliders, union-find to anchors plus the span limit, rubble blocks, `TerrainMap` revision, view; integers and compiler-output f32, no trig |
| The map digest folds the tree data; the certificate carries `sim_ron_sha` | The digest folds the building RON's bytes; the same certificate field |
| glb nodes bound by extras (`tree_segment`) | Grayboxes draw meshes built from the RON at view attach; real buildings bind kit meshes by extras (`building_element`) later |

## 6. Recommended slices

Each slice is one agent session or a few; "owner sees" is what the end of it shows. All gates per
AGENTS.md; every number labelled. References are §5's.

| Slice | Builds | Reuses; references | Owner sees |
|---|---|---|---|
| **B0 — houses are matter** | The graybox house becomes hollow: four wall slabs and a roof slab as convex volumes on `[Terrain, Armor]` with a masonry `BallisticVolume` (new `materials.ron` entries, provenance `Unknown`), still a `TerrainMap` block; no pools. Server idle tick and forest/duel frame MEASURED before and after. | the trees' convex-child pattern; `materials.ron` | Tracers strike walls, slow through them or stop; a burst short of a wall leaves the tank behind it untouched; the pen cam shows the field shadowed. |
| **B1 — a panel breaks at the shot point** | The compiler's first stage: a Rust generator that writes `assets/buildings/<id>/<id>.building.ron` (elements as slabs from a split tree, rubble block per element, links and anchors written now for B3), canonical and recooked twice in a test; per-element pools; `PoolEmptied` → element down; the ledger generalised from trees to any map instance (word decoded per kind); derive: collider disabled, rubble block in, revision bump; view: meshes from the RON, merged while intact, re-merged on change; map digest folds the RON. | `MapInstance`, `DestructionLedger`, `PoolEmptied`, `derive_systems`, #157's merged view, A13's certificate; CGA and split grammars for the tree, Labs' factoring for floors and patterns, Blast's chunk/bond/world-bond model and Lane 3 for the schema, Voro++'s per-cell clip for exact faces | Shoot a wall: one panel goes, rubble at its foot, see and fire through the gap; a second client and a late joiner see the same house. |
| **B2 — tanks breach walls** | The section-modulus ram law over elements and trunks alike (`ram_contacts` generalised from "standing tree" to "standing piece"); masonry `k` from the measured impulses; the hull-at-break playtest fork filed. | `ram_contacts`, `ram_torque`, `ram-impulses.md` data | A Tiger at 10 km/h breaches a thin wall and drives over its rubble; a push from a standstill does not; a thick wall holds. |
| **B3 — collapse** | On every change, union-find per building from the anchors over standing elements' links, plus the span limit; dropped elements become rubble with no wire entry. | the ledger's derive-on-arrival; Blast's support graph and island detection, Coumans' union-find, Valheim's span, Whiting's nonstructural tag and ground anchors | Shoot out a corner: the roof section above comes down; a house shot to pieces ends as stubs and rubble, identical on every client. |
| **B4 — presentation** | The element crumbles over ~0.5–1 s fused to the transition tick (`Fell`/`FallClock`); edge caps beside a broken element; dust and a particle burst (capped); optional client-only sub-fracture of the broken element into a capped few chunks with no gameplay layer; decals for hits short of a break (#124's path); the #170 destructible-layer tile redraw per change, coalesced per frame. | the tree fall's clock, caps, hull marks; Frostbite 2's derived breach look, Blast's subsupport and Chaos's abandon level for debris, Voronoi (Raghavachary, Voro++) with courses (Whiting) and Cell Fracture's stretched metric (ideas only) for the sub-fracture | It reads right on the LOW preset: no ghost shadows, no hitch. |
| **B5 — the perf gate** | The 30-tank battle scene (#153) with houses shot out during the run; A/B/B/A frame, server tick, ledger bytes, shadow redraw per event. | #152 runner, #153 scene | Before/after numbers on the issue; the #150 lever applied to elements if idle cost shows. |
| **Then** | Real buildings: the generator places kit meshes modelled in Blender (fracture never in Blender, `authoring-tools.md` Lane 1); layout variety by grammar or WFC; a compile-time check that each building stands; fences and garden walls as one-row buildings; small vehicles crushed (separate). | B1's trio; Burgess' kits, WFC (MIT), Whiting's feasibility check | The first modelled house on Kalinovo. |

Order note: B0 stands alone and is worth landing first; it changes play today (houses become cover
against fields and concealment against fire). B2 and B3 are independent after B1.

## 7. The owner's questions, each with a recommended answer

1. **Granularity: panels, or a whole-building swap?** Panels: the generator's elements, one storey tall and ~1.5–2 m wide between openings (the trunk segment's scale), so a breach is a gap a tank fits through. The whole-building ruin is derived from panels, never authored.
2. **Does a broken panel leave rubble that is cover and terrain?** Yes: a derived low block per panel that stops rounds and fields, blocks sight, and the belts climb. BF6's "rubble remains as cover" without its level-designed piles.
3. **Support rule: none, connectivity, or stress?** Connectivity from ground anchors plus an integer span limit from data (Valheim's shape). No stress solver: it needs per-building tuning and a solver Avian is not.
4. **Ram law?** One law for trees and panels: `Σ impulse × lever ≥ k_substance × section modulus`, calibrated from the measured impulses. Thick walls hold a push and break to a ram; thin ones break to a push.
5. **When a tank breaks a wall, does it stop for that tick or keep its momentum?** Ship the trees' after-solver read (it stops for a tick) and file it as a playtest fork; a pre-solver hook is the alternative if it reads badly.
6. **How big is a panel's pool?** HP per cubic metre of the substance × the panel's volume (the trees use area because a trunk is severed across its section; a wall is crossed through its face). Loose calibration: a burst round breaches a brick panel in one or two hits, a solid round takes several (TUNED later).
7. **Does the blast sphere come first?** No. Panels take today's line dose and fragment cone like any box; the blast sphere lands in #103 and panels take it with no wall code.
8. **Debris?** Client-only, never on a gameplay layer, never large enough to look like cover; particles only in the first slices, small rigid bits later as a graphics setting (WoT ships its Havok fragments as one).
9. **Do intact houses stay belt-climbable blocks?** No: intact walls are hull colliders only; only rubble joins the block list, so a tank breaches a house instead of climbing it.
10. **When does cover change: at the break tick, or after the crumble?** At the break tick, with the view crumbling over ~0.5–1 s: the trees' fork A14, extended to panels; no delayed collapse in the sim (Reforger's audio-then-collapse can live in the view).

Smaller calls, recommended without a question: one ledger for every destructible (entries keyed by
map instance, words decoded per kind; widen the word past `u16` only if a building passes 255
panels); `ColliderDisabled`, never despawn, for a broken panel; a building's sim data is the
compiler's RON, canonical and hashed, never read from the glb (ADR-0014); fences are the same asset
type with one row of panels (ADR-0034's spirit: no per-object system); a collapse doses no tank
(debris is presentation; CoH3 does dose units, which would need a rubble field law we do not have);
the ledger stays global, as WoT's destruction reaches every client whatever spotting says.

## Sources

Repo (read 2026-10-30): `src/scatter.rs`, `src/trees.rs`, `src/trees/view.rs`,
`src/net/destruction.rs`, `src/ballistics.rs`, `src/ballistics/resolve.rs`, `src/aim.rs`,
`src/world.rs`, `src/track/terrain.rs`, `assets/materials/materials.ron`, `assets/trees/wood.ron`,
`assets/maps/kalinovo/prototypes.ron`, `.agents/PRODUCT.md`, `.agents/GLOSSARY.md`, ADR-0014,
ADR-0034, `.agents/docs/design/sim-divergence-and-determinism.md` §11, issues #103, #106, #149,
#150, #153, #170; `.agents/scratch/destruction-2026-10-26/` (`games-and-dagor.md`,
`engine-delta.md`, `codebase-scout.md`, `authoring-tools.md`, `ram-impulses.md`,
`slice-1-fallen-tree.md`, `forest-frame-cost-2026-10-29.md`, `wot-scout-round1`–`11`,
`wot-scout-round15-buildings.md`); `.agents/scratch/destruction-survey-2026-08-13.md`;
`.agents/scratch/destruction-structural-houdini-2026-08-13.md`;
`.agents/scratch/perf-2026-10-29/` (`night-results-2026-10-30.md`, `preset-tiers-research.md`,
`shadow-tactics.md`).

Peer session: WoT client scripts via the `wot` session, round 15, received 2026-10-30 (CLAIMED),
`.agents/scratch/destruction-2026-10-26/wot-scout-round15-buildings.md`.

Web, new this pass (all read 2026-10-30; full notes and every link in
`building-destruction-web-survey.md` and `pipeline-references.md` beside this report):
- WoT "20 secrets": https://worldoftanks.com/en/news/general-news/20-secrets
- UE5 Chaos Destruction Overview: https://dev.epicgames.com/documentation/en-us/unreal-engine/destruction-overview
- UE5 Python API `GeometryCollectionComponent` (5.5): https://dev.epicgames.com/documentation/en-us/unreal-engine/python-api/class/GeometryCollectionComponent?application_version=5.5
- Kihl, SIGGRAPH 2010 (PDF): https://www.advances.realtimerendering.com/s2010/Kihl-Destruction%20in%20Frostbite(SIGGRAPH%202010%20Advanced%20RealTime%20Rendering%20Course).pdf
- CBS, Red Faction: Guerrilla Q&A: https://www.cbsnews.com/news/red-faction-guerrilla-q-a/
- Game Developer, Red Faction: Armageddon: https://www.gamedeveloper.com/design/the-destructible-world-building-i-red-faction-armageddon-i-
- GDC Vault, "Blowing Up the Outside World" (metadata only): https://gdcvault.com/play/1670/Blowing-Up-the-Outside-World
- SEGA, CoH3 Destruction Dev Diary: https://sega.prezly.com/company-of-heroes-3-destruction-dev-diary
- Relic, CoH3 Destruction Overview Part 1 (unreachable; summaries only): https://community.companyofheroes.com/coh-franchise-home/company-of-heroes-3/forums/1-general-discussion/threads/2212-company-of-heroes-3-destruction-overview-part-1-modeling-fracturing
- MP1st, THE FINALS Season 12 notes: https://mp1st.com/?p=317079
- Gustafsson, Teardown multiplayer: https://blog.voxagon.se/2026/03/13/teardown-multiplayer.html
- Hell Let Loose FAQ, 2020 snapshot: https://web.archive.org/web/2020id_/https://www.hellletloose.com/faq
- NVIDIA Blast: https://github.com/NVIDIAGameWorks/Blast and https://github.com/NVIDIA-Omniverse/PhysX/tree/main/blast (BSD-3-Clause)
- Voro++: https://github.com/chr1shr/voro (modified BSD-3, LBNL)
- Raghavachary 2002: https://history.siggraph.org/learning/fracture-generation-on-polygonal-meshes-using-voronoi-polygons-by-paul
- Müller, Chentanez, Kim 2013: https://history.siggraph.org/?p=108389
- Blender Cell Fracture: https://github.com/blender/blender-addons/tree/main/object_fracture_cell (GPL-2.0-or-later)
- Müller et al. 2006 (CGA): https://history.siggraph.org/?p=113547, DOI 10.1145/1141911.1141931
- Wonka et al. 2003: https://cg.tuwien.ac.at/research/publications/2003/Wonka-2003-Ins
- Whiting, Ochsendorf, Durand 2009: https://people.csail.mit.edu/ewhiting/resources/pubs/WhitingOchsendorfDurand-09.pdf
- Burgess and Purkeypile, GDC 2013: https://www.slideshare.net/JoelBurgess/gdc2013-kit-buildingfinal
- WaveFunctionCollapse: https://github.com/mxgmn/WaveFunctionCollapse (MIT)
- SideFX Labs help text: https://github.com/sideeffects/SideFXLabs/tree/Development/help/nodes/sop (Labs `LICENSE.md` BSD-style; Houdini under its EULA)
