# What makes a rammed or collapsing house believable, and what we can skip

Subject: the video game's environment destruction model (houses, walls and trees cracking,
collapsing and leaving debris) and its server cost. Game state only: no crew or rounds involved.
Read-only research; nothing in the repository was edited, built or run.

Labels: **MEASURED** (a run in this repo, or a measurement the cited authors report), **DERIVED**
(computed here, inputs named), **CLAIMED** (a published or secondary claim I could not check, or
an engineering rule quoted from a guide). ASSUMED marks an input I chose.

**Decisions I took.** Viewing model: 1080p and the seed tank's two views
(`assets/tiger_1/tiger_1.tank.ron:471–472`: commander unaided 45°, gunner 2.5× with a 25° field);
its 57,000 kg is fixture data. The ram is `ram-measure/report.md`'s, impact 10.823669 m/s
(MEASURED); the swept wall width is ASSUMED 3.5 m. I did not redo the game surveys in
`collapse-survey.md` and `hosting-research.md`; I add only new primary facts. Ranking is by
believability kept per server millisecond saved, weighted by evidence strength.

## TL;DR

1. **Players judge a collapse by timing and big shapes, not stress.** People match a block
   tower's fate 66% of the time (MEASURED, Battaglia 2013), accept ~60 ms of contact delay and
   large trajectory errors (MEASURED, O'Sullivan 2003), and track about four objects (CLAIMED).
   Two hard bars: the breach answers the hull within ~4 ticks, and nothing precarious stands
   unexplained.
2. **Shipped games keep coarse truth and dress it.** The Finals solves metre-scale fragments,
   then adds "edge meshes" and VFX; Frostbite removes whole wall parts, then masks and particles;
   Siege's debris is "always small … ignored by gameplay … not replicated"; Red Faction scans
   stress "every few seconds" and lets buildings fall long after the damage.
3. **Strongest "don't need X": a full-speed ram needs no structural solve to decide its
   breach.** The bond work the hole frees is ~9×10⁻⁶ of the hull's kinetic energy, and even a
   slow push applies ~20× the wall's out-of-plane capacity (DERIVED). Strength decides only
   what is left around the hole.
4. **Direction:** the hull's sweep decides the breach; structure is checked per element (55 in
   house_1), not per chunk (1,122); the server simulates only gameplay-sized pieces; bricks,
   dust, jagged edges and sound are client decoration keyed to server events; a secondary
   collapse may lag up to about a second behind a creak.

## 1. What a player perceives, and where the bar is

| Aspect | Evidence | Bar for Overmatch |
|---|---|---|
| **Contact response timing** | Spheres colliding: about 60 ms of added delay was accepted on average (MEASURED, [O'Sullivan et al. 2003](https://gv2.scss.tcd.ie/cosulliv/Pubs/fidelity.pdf) §3.2.3). Ratings fell steadily from 0 to 100 to 300 ms delays, and participants called delayed collisions "stick together" (MEASURED, n = 20, [O'Sullivan & Dingliana 2001](https://gv2.scss.tcd.ie/cosulliv/Pubs/acmcollisions.pdf) §4.3, after Michotte). | The **breach** must appear within ~60 ms of hull contact: 3.84 ticks at 64 Hz (DERIVED). This is the one hard latency bar. |
| **Trajectory and momentum errors** | Thresholds for post-collision angle errors run to tens of degrees. Viewers prefer more energy after a collision to less (MEASURED, O'Sullivan 2003, Fig. 3). For extended bodies, people use heuristics and are "less competent" (Proffitt & Gilden 1989, as cited there). Adding a *random* spin to crude collisions made them more believable (O'Sullivan & Dingliana 2001 §4.4). | Exact fragment arcs are not perceived. Debris can be seeded and approximate, and should err toward livelier, not deader. |
| **Where anomalies are seen** | Detection of a 5 mm contact gap among similar moving objects fell from ~89% at fixation to ~51% at 37.5 mm and ~12% at 112.5 mm eccentricity (MEASURED, read from Fig. 8 of O'Sullivan & Dingliana 2001). Similar distractors hurt more than different ones. | In a pile of look-alike bricks, individual contact errors are invisible away from where the player is looking. |
| **Stability judgments** | On "will this tower fall?", people matched ground truth 66% of the time. Their errors fit a simulation with position noise of 20% of a block's side. A tower that is delicately balanced but stable was rated among the *least* stable (MEASURED, [Battaglia, Hamrick & Tenenbaum 2013](https://pmc.ncbi.nlm.nih.gov/articles/PMC3831455)). | Near the failure threshold, a player cannot tell "held at 99%" from "failed at 101%". A precarious remnant that stands **looks like a bug** even when physics says it holds. A conservative rule beats an exact one here. |
| **How many things are seen** | About four to five independently moving objects can be tracked at once (CLAIMED, summary of [Pylyshyn & Storm 1988](https://ruccs.rutgers.edu/images/personal-zenon-pylyshyn/docs/storm88.pdf)). Beyond that the visual system keeps ensemble statistics: mean size, spread, density ([Alvarez 2011](https://scholar.harvard.edu/alvarez/files/alvarez-2011-tics-ensemble.pdf)). | Of 150 bricks, the player sees "a burst of brick" plus at most a few big pieces. Debris needs the right count, size range, speed and spread, not the right individual paths. |
| **Fall timing and scale** | Viewers judge size and distance from a fall's average velocity and duration far more than from its acceleration profile (CLAIMED, abstract of [Hecht, Kaiser & Banks 1996](https://pubmed.ncbi.nlm.nih.gov/8920842)). Film miniatures are overcranked for this reason (practitioner rule). | Falls must run at real g and real durations: a 2.475 m eaves drop takes 0.71 s, and a 2.5 m pier topples from 5° in ~1.2 s (DERIVED). Never slow a fall to hide latency. Delay its **onset** instead. |
| **Changes hidden by disruption** | Large scene changes go unnoticed when they coincide with small brief "mudsplashes" ([O'Regan, Rensink & Clark 1999](https://www2.psych.ubc.ca/~rensink/publications/abs.99.5.html)). | Dust, flashes and the hull itself are natural curtains for representation swaps: proxy to chunks, or server piece to client debris. |
| **Anticipation** | "Anticipation, the preparation for an action" is a core animation principle ([Lasseter 1987](https://www.evl.uic.edu/aej/527/papers/lasseter.pdf)). Red Faction: damaged walls make "the building start to stress – stuff starts to crumble, pieces fall" (Kennedy, [VideoGamer](https://www.videogamer.com/previews/20090414111852-red-faction-guerrilla-interview)). | A creak, a dust trickle and a few crumbs before a secondary collapse read as cause, and buy the server time. |
| **Sound** | Listeners judge material mainly from how fast a contact sound decays ([Klatzky, Pai & Krotkov 2000](https://www.cs.ubc.ca/labs/lci/papers/docs2000/pai-KlaPai00.pdf)). Sound also arrives late with distance: 0.58 s at 200 m and 1.46 s at 500 m (DERIVED, 343 m/s). | Weight and material are sold by client audio at no server cost, and players already accept audio lagging a distant collapse. |
| **Resolution on screen** | A 0.225 m chunk (`house_1.house.ron`, three 75 mm courses, MEASURED) spans 2.9 px at 100 m and 1.0 px at 300 m unaided, or 5.5 px and 1.8 px in the 25° optic. A 1.5 m element spans 20 / 6.5 px unaided and 37 / 12 px in the optic (DERIVED). | Beyond ~100 m, chunk shapes are at the edge of visibility while element shapes stay visible. Brick detail matters only to the ramming player and close observers, who see it through dust during the ~1 s the hull is inside. |

**Salience, highest first:** (1) did the tank go through, and when (≤ 60 ms); (2) the hole's
silhouette and the sight lines and passage it opens; (3) did the roof come down, when and which
way; (4) what big things remain as cover and obstacles; (5) dust, sound and burst character.
**Below the floor:** chunk crack paths beyond ~100 m, individual brick trajectories, settling
under a few centimetres, and whether a load path near 100% of capacity holds or fails.

## 2. Shipped games: what each computes

Server column: "n/a" means single-player or peer host.

| Game | Simulated | Authored or faked | On a server | Cost published | Player reaction |
|---|---|---|---|---|---|
| **The Finals** ([GDC 2024 slides](https://media.gdcvault.com/gdc2024/Slides/GDC+slide+presentations/Isaksson_Mans_Engineering+Mayham+Technical.pdf)) | Voronoi fragments, metre-scale by eye from slides 13 and 39 (not stated), joined by constraints; sparse direct Cholesky (slide 22); break when `Impulse > Baseline + BreakOffset` (slide 25); "large stacks of simulated rigid bodies" (slide 18) | Houdini-authored fracture and connection graph (slides 13–15); **edge meshes** turn a clean fragment edge into splinters, then VFX dust (slides 41–46); "cutting corners" with indestructible parts (slide 29) | Everything: "we don't run physics on the clients" (`hosting-research.md` §2) | Incremental Cholesky chart ~0.0005–0.0015 on an axis labelled ms, up to ~1,900 constraints (CLAIMED, unit unclear, slide 30). Transforms ~400 kbit/s average, ~175 kbit/s peaks after quantisation and deltas (CLAIMED, slides 33, 38) | The genre's benchmark. Season 12 adds sagging and bending as a later layer ([KitGuru](https://kitguru.net/gaming/joao-silva/embark-studios-reveals-major-destruction-system-update-for-the-finals)) |
| **Battlefield / Frostbite** ([Kihl, SIGGRAPH 2010](https://www.advances.realtimerendering.com/s2010/Kihl-Destruction%20in%20Frostbite(SIGGRAPH%202010%20Advanced%20RealTime%20Rendering%20Course).pdf)) | Whole wall **parts** removed; Bad Company 2: a building collapses when "enough segments (walls) are destroyed" (CLAIMED, [community wiki](https://battlefield.fandom.com/wiki/Destruction)) | A low-resolution distance field (~2 m per texel, CLAIMED) plus a detail texture makes the jagged hole mask; "detail meshes around the destroyed section"; "particle effects and mesh debris for a more dramatic look" (slides 4–6, 10–11). BF6: "level-designed debris piles" ([EA](https://www.ea.com/news/how-battlefield-6-redefined-destruction)) | Part states are networked; BF6 does "prioritization and culling of part destruction and debris spawning" | none | Bad Company 2's collapses are the series' remembered high point; the rule is a part count, not stress |
| **Rainbow Six Siege** ([L'Heureux, GDC 2016](https://media.gdcvault.com/gdc2016/Presentations/LHeureux_Julien_Art_Of_Destruction.pdf)) | Procedural cuts through wall layers; no weight model | "No procedurally cut dynamic fragments: well-placed replacements, instanced, recycled aggressively"; fragments "don't collide together"; "always boxes" | Deterministic events, not meshes. "All dynamic objects and debris are always small, ignored by gameplay … not replicated" | "Roughly 6 ms for a wall"; one explosion through 2 drywall + 2 wood layers 8.1/10.3 ms on PC, "with a heap of salt" (CLAIMED) | Tactical destruction is the game's identity. Nobody reads debris as cover |
| **Red Faction: Guerrilla** | Geo-Mod stress over pre-broken pieces; "every few seconds, it's scanning everything" ([Kennedy](https://www.gamewatcher.com/interviews/red-faction-guerrilla-interview/11408)); a demo overlay "calculated every second" ([THQ diary](https://www.thesixthaxis.com/?p=10391)) | Havok "just to throw objects around"; artists retaught "to be architects" so buildings stand under their own weight | n/a | none | A building "could take damage but not actually collapse until 30 minutes later" (CLAIMED, THQ diary). The camera went third-person because first-person players could not see the stress ([VideoGamer](https://www.videogamer.com/previews/20090414111852-red-faction-guerrilla-interview)) |
| **Teardown** | Voxel connectivity only; vehicles carve holes at impact | Level design shows the limit first ("a very deliberate decision") | Host-authoritative commands plus client body sync ([2026 blog](https://blog.voxagon.se/2026/03/13/teardown-multiplayer.html)) | none | Missing integrity is "something a lot of people have requested". Gustafsson defends it: "you can trust the environment" ([SED transcript](https://softwareengineeringdaily.com/wp-content/uploads/2024/12/SED1772-Teardown.txt) 0:10–0:22) |
| **Space / Medieval Engineers, 7 Days to Die, Valheim** | Support or load scalars spread from the ground on change; Space Engineers breaks the worst block at most once per 10 updates | Rules and constants | Server | none | Accepted as game rules (see `collapse-survey.md` §1) |
| **Unreal Chaos, NVIDIA Blast** | Connection graph with strain or bond solve on a bounded iteration budget; graph reduction "experimental" | Cluster levels; Chaos "abandon" levels stop replication of small pieces | Up to a chosen level | Blast: time "linearly proportional to maxSolverIterationsPerFrame" (CLAIMED, [docs](https://nvidia-omniverse.github.io/PhysX/blast/docs/api/extensions/ext_stress.html)) | Engine features, not a reaction |
| **Instruments of Destruction** ([Radiangames](https://radiangames.com/pkrg/instruments/index.html)) | "Every structure … demolished piece by piece"; pieces break into smaller rubble (previews) | Architecture unpublished; by Red Faction: Guerrilla's lead technical designer | n/a | none | Praised for collapses that "behave like their real-world counterparts" (preview, CLAIMED) |
| **World of Tanks, War Thunder (Dagor), Hell Let Loose** | WoT: per-client detailed destruction; "most two-story ones won't fall down". Dagor: non-colliding client debris moved by scripted gravity. HLL dropped destruction to keep 100 players | — | WoT/Dagor: server keeps a coarse destroyed flag | none | The tank-genre precedent is that buildings matter as cover and obstacles, and their falls are presentation (`hosting-research.md` §2) |

**The pattern:** truth sits at the scale a player can act on (wall parts, metre fragments, a
support graph); detail is decoration on top (masks, edge meshes, particles, small debris);
signalled collapse latency is accepted (Red Faction); the one costly holdout, The Finals, still
solves coarse fragments, not bricks.

## 3. "We don't need X" findings

1. **We don't need a structural solve to decide a ram's breach.** (Strongest finding.) The hull
   carries 616.9 kN·s and 3.339 MJ (DERIVED: 57,000 kg at 10.823669 m/s). Freeing a 3.5 ×
   2.475 m region of 0.25 m wall costs **30 J of fracture work, 9×10⁻⁶ of that energy**, at
   brick's 10 J/m² (CLAIMED in `materials.ron`); every joint in house_1 together is ~1 kJ
   (DERIVED: median joint area 0.0326 m², MEASURED, × 3,258). Pushing the 4.0 t swept slab costs
   the tank **6.6%** of its speed, 219 kJ (DERIVED, plastic exchange). Even a slow push wins: the
   wall's out-of-plane capacity, 0.69 MPa (CLAIMED) × t²/6 = 7.2 kN·m/m, is ~25 kN over a 3.5 m
   strip loaded at 1 m, against traction 0.9·mg ≈ 503 kN (DERIVED). Whether the tank goes
   through is never in doubt for this house; strength decides only the edges, a local question.
   The settle lane's static solve spent ~12.5 ms on a ram and cracked nothing (per the brief):
   the wrong tool for this regime.
2. **We don't need chunk-level load sharing to decide what falls.** Near threshold, players'
   stability judgments are noise (66% on towers); elements (walls, piers, lintels, gables,
   plates, boards) are what stays visible beyond ~100 m (§1); inside an element, connectivity
   plus a masonry arching rule settles what hangs over a breach (R2).
3. **We don't need server physics for brick-sized debris.** Siege, Chaos's abandon levels, Dagor
   and World of Tanks all keep it off the server; players track about four objects and judge the
   rest as an ensemble; a 27 kg clump (median chunk, `collapse-survey.md` §0) cannot stop a
   57 t hull, lift a track or stop a round.
4. **We don't need a cascade to finish in the tick it starts.** The breach needs ≤ ~60 ms. A
   secondary collapse from lost support has no causal-launch bar: real ones take 0.7–1.2 s just
   to fall (DERIVED, §1), and Red Faction's took seconds to minutes. A signalled lead-in of up
   to ~1 s is plausible, **unproven for Overmatch** (experiment in R4).
5. **We don't need exact fracture edges on the server.** The Finals' edge meshes and
   Frostbite's ~2 m distance-field masks draw a jagged, brick-stepped outline over a coarser
   truth; the server's chunk outline already exceeds what either ships.
6. **We don't need solver convergence at an intact house's threshold; we need houses built with
   margin.** house_1's roof boards run at 99.3% of the nailed seam's 3,240 Pa (MEASURED, per
   `settle-codex.md`), so the solver spends its effort deciding a state no player could judge.
   Red Faction hit the same problem ("within two minutes, the entire world would collapse") and
   fixed the buildings, not the solver ("teach our artists how to be architects").
7. **We don't need sag or sway** (as `collapse-survey.md` found). The Finals shipped years
   before adding it.
8. **Counter-finding: we do need weight.** Connectivity-only floats precarious remnants:
   Teardown concedes it, The Finals' slide 20 shows a floating slab, and Battaglia shows
   precarious-but-standing reads as wrong. Weight checks stay, coarse and conservative.

## 4. Ranked approaches

**R1. The hull decides the breach: sweep, momentum and a local edge.**
- **Sees:** the wall bursts within one or two ticks of contact (the nose pierces 0.25 m in
  23 ms, DERIVED); a hull-shaped hole whose top steps up at the arching angle or opens to the
  eaves; brick thrown ahead at about tank speed; the hull checks ~7%, felt through the
  drivetrain (no camera kick).
- **Skips:** any equilibrium solve during contact, and the per-joint blow and press laws
  outside the swept volume.
- **Server cost bound:** the chunks inside the hull's swept volume, ~148 for the 2.17 m³ slab
  over ~2 ticks (DERIVED); at an ASSUMED 1 µs hosted per chunk, ≤ 0.15 ms a tick, then zero;
  one momentum exchange on the hull.
- **Risks:** a stronger construction (concrete, a timber frame, a future bunker) can resist the
  hull, so the sweep stands in for the strength law only while hull force exceeds the struck
  element's capacity; that ratio is computed from data (R2's element capacity), never set as a
  flag. Clean edges are dressed by R3.
- **Doctrine:** momentum conservation and occupancy are base laws, so behaviour still emerges.
  It breaks the decided "one assessment path through `run_laws` for every contact"; the single
  path becomes "the sweep removes, elements check" (owner decision).
- **Cheapest experiment:** log hull poses from `scripts/perf/ram.json`; compute the swept-chunk
  set in a test-only function; render it beside today's ledger result at 30, 100 and 300 m in
  both views (hidden window); the owner picks unlabelled. **Kill** if he prefers today's at
  gameplay range.

**R2. Structure at element scale; chunks only for local connectivity and arching.**
- **Sees:** walls, piers, lintels, gables, plates and roof parts stand or fall as units, in
  support order. A rammed gable brings its roof slope down; a rammed long wall leaves boards
  spanning gable to gable (DERIVED from the spec: boards bear on gables and plates, no
  rafters). A brick strip over a breach falls unless running bond can arch, which needs room for
  a 45° triangle of height span/2 plus 8 in (≈ 0.2 m) above it (CLAIMED,
  [NCMA TEK 17-01D](https://www.cmha.org/resource/tek-17-01d/)). For a 3.5 m breach that is
  ~1.95 m; under a 2.475 m eaves the strip stays only over a hull under ~0.5 m tall, so it
  always falls (DERIVED).
- **Skips:** chunk-level load sharing, chunk-exact crack paths inside an element, and
  near-threshold exactness.
- **Server cost bound:** 55 elements (MEASURED) and an ASSUMED ~150 interfaces: ~200 visits ×
  100 ns ≈ 20 µs per event (DERIVED). A two-support span (a lintel on two piers) is determinate
  by statics; anything more redundant takes one small solve, 55 × 6 = 330 unknowns factorized at
  load. The carrier prototype died on exact all-plane sections at chunk level, 8.4 ms on the
  Mac and 3.75× at double size (MEASURED); at element level the sections are the baked
  interfaces, so that superlinear term does not exist. Inside an element: union-find plus an
  upward support walk from the breach, O(affected chunks).
- **Risks:** elements must be cut where things fail (wall bays, not whole 6 m walls; the bake
  already splits piers, sills and lintels); damage inside an element lowers its interface
  capacity through remaining joint area (data), never a tuned factor; framed houses raise the
  count to tens or low hundreds.
- **Doctrine:** aligned with "at the resolution of variables a player can perceive or act on"
  (§1 is the evidence); interface capacities emerge from chunk joints and materials. It retires
  the chunk-level reference model and the design doc's "do not restrict failure to coplanar cut
  sets" goal (owner decision). If the running best-of-both experiment's bonded bodies are
  elements, it converges with this; this research adds whether the intra-element joint solve
  is needed at all.
- **Cheapest experiment, "predict the fall":** cut 20 clips 1 s before the outcome, from the
  carrier prototype's 49-evaluation corpus or scripted breaches; the owner predicts falls or
  stands; compare him, and the element rule, against the chunk solver. **Kill R2** if he agrees
  with the chunk solver well above chance exactly where the element rule disagrees.

**R3. Server bodies only at gameplay size; clients decorate.**
- **Sees:** identical big pieces for everyone; per client, brick-sized debris on seeded
  ballistic arcs that collide with terrain only (as Dagor does), plus dust, edge splinters and
  sound. The same seed gives near-identical arcs on every client.
- **Skips:** bodies, contacts, law runs and pose traffic for everything below a size class. The
  class is a game `Unit`, "can lift a track or stop a round", one generic law.
- **Server cost bound:** linear in server bodies at 35–80 µs each hosted (DERIVED in
  `review-claude.md`). Ram: an ASSUMED 10–20 bodies ≈ 0.4–1.6 ms. Whole house: 55 element
  bodies plus ≤ 2 landing splits each is ≤ 165 bodies, 5.8–13 ms (DERIVED); the top end needs
  R4's staging or a stricter class.
- **Risks:** a decorative brick can look like cover, so it fades or settles flat; client debris
  can visibly pass through a server slab, the price of terrain-only.
- **Doctrine:** ADR-0037 gains a decoration layer drawn at the shared delay from server events
  (no client simulation of gameplay); it meets the provisional "fine presentation debris may
  fade".
- **Cheapest experiment:** ABX at three distances on one collapse, server-simulated chunks
  against client-seeded debris. **Kill** if the owner identifies the server version in ≥ 9 of
  12 trials at ≥ 100 m.

**R4. Deferred, signalled secondary collapse; the breach stays immediate.**
- **Sees:** the breach within ≤ 4 ticks; then 0.25–1 s of creak, dust trickle, a few falling
  crumbs and a small settle (client, keyed to a server "pending" state); then real-g falls of
  0.7–1.2 s.
- **Skips:** finishing a cascade in one tick. Up to ~64 ticks of lead-in amortize whatever
  structural or spawn work remains under a per-tick `Resolution` budget, for example 1 ms.
- **Server cost bound:** per-tick work ≤ the budget, total unchanged; the 46 ms gables cascade
  (DERIVED in `review-claude.md`) becomes ≤ 1 ms a tick over ≤ 46 ticks.
- **Risks:** an unsignalled hang reads as a bug (the balanced-tower illusion); a tank under a
  pending roof sees it hold briefly; the delay must never touch the breach or slow a fall.
- **Doctrine:** "pending" is a named game state; the lead-in is `Resolution` with a cost
  reason; creak and dust are `Taste`; one timeline is kept.
- **Cheapest experiment:** clips of one secondary collapse with onset delays of 0, 0.25, 0.5, 1
  and 2 s, each with and without signals, plus breach delays of 0, 2, 4 and 6 ticks; the owner
  rates 1–3 (O'Sullivan's scale). The longest delay rated equal to 0 s sets the budget.

**R5. Houses with real margins (prerequisite, data only).**
- **Sees:** intact houses never self-collapse; damage reads as the cause. **Skips:** solver
  effort on intact states near threshold. **Cost:** zero at runtime, one bake-time pass.
- **Risks:** none visible; it needs the planned framed houses and sourced fixings.
  **Doctrine:** a base-truths fix to construction data.
- **Cheapest experiment:** a bake-time report of each design's minimum element safety factor
  under self-weight, flagging any below a sourced margin (Eurocode-style partial factors of
  ~1.5 are CLAIMED, not checked).

**Lower ranks** (sees · cost · doctrine · verdict):

| Rank | Approach | Sees | Server cost | Doctrine cost | Verdict |
|---|---|---|---|---|---|
| R6 | Precomputed per-design weak points: a bake-time table, per element, of what loses support if it goes | Same as R2 | O(1) lookup, but R2's walk is already ~20 µs; stale under multiple damage | Clean | Subsumed by R2; reopen at city scale |
| R7 | Rubble mound as server truth, pieces as client dressing | An honest heap: house_1's 13.4 m³ of brick is ~0.84 m deep over 24 m² at an ASSUMED ×1.5 bulking (DERIVED), real hull-down cover; no voids | One heightfield patch; ends the settling tail | Breaks the decided finite-mass rubble and the one-surface rule | Defer to city scale |
| R8 | Connectivity only (Teardown) | A house held by one brick; a floating gable roof after a ram | ~0.1 ms per crack | Drops weight, a base truth | Rejected on §1's stability evidence |
| R9 | Authored playback (Levolution, Chaos cache) | Cinematic, repeated falls that ignore tanks in the way | Near zero | Breaks base truths outright | Rejected |
| Ref | Today's chunk-scale strategy | Brick-exact cracks near threshold, redundant load sharing: mostly below the §1 floor | Ram worst ticks 677–683 ms, 193 awake (MEASURED, `ram-measure`); a static solve that stalls under a press (settle); 4× work for 1% forces (dyn-cells); 4,096 substeps (dyn-avian) | — | Keep only as an offline reference |

## 5. Adjustments to the current strategy

**Resolution.** Structural truth at the element (55 in house_1, tens to low hundreds in framed
houses); breach and hole outline at the chunk, decided by the sweep and local connectivity;
gameplay bodies at the size class; bricks only on screen.

**Ownership:**

| The server must own (cover, sight, traversal, timing) | A client may decorate (no gameplay effect, drawn at the shared delay) |
|---|---|
| Which chunks are gone: hole outline and sight lines (the ledger already does this) | Jagged edges beyond chunk precision: edge meshes or masks |
| Element states: standing, pending, released, with event ticks | Creak, rumble, dust trickle and crumbs during "pending" |
| Bodies at or above the size class, and their poses | Brick-sized debris on seeded ballistic arcs, terrain-only, fading or settling flat |
| The hull's momentum exchange with the wall | Dust clouds, sparks, splinters, the sound of every contact |
| Rubble that stops a hull or a round | Small rubble dressing on top of server pieces |

**Order:** R5 (data); R1 with R2, which fixes the ram and changes what a collapse costs; R3;
R4 only where R2 and R3 leave a spike. The pricing lanes already listed (event-driven law runs,
rest that ends, a lean wire) stay valid; most of their structural share disappears under R2.

**Measure by eye**, as unlabelled side-by-side clips from both views at 30, 100 and 300 m, in
hidden-window captures at night: sweep against today's breach (R1); "predict the fall" (R2);
server against client debris (R3); the onset and breach delay ladders (R4); one whole-house
collapse under R1–R4 against today's, judged by the five salient questions in §1.

## Sources

**Repository:** `.agents/scratch/mechanisms-2026-11-05/{review,linear}-{claude,codex}.md`,
`settle-codex.md`; `.agents/docs/design/construction-{carrier-prototype,destruction}.md`;
`.agents/scratch/ram-measure/report.md`; `destruction-2026-10-31-collapse/collapse-survey.md`;
`slowdown-hunt/{hosting-research,fidelity-levers}.md`; `lane/settle:…/bondsolve/settle.md`,
`lane/dyn-cells:…/dynamics/dyn-cells.md`, `lane/dyn-avian:…/dynamics/dyn-avian.md`;
`tools/constructions/house_1.house.ron`, `assets/tiger_1/tiger_1.tank.ron`,
`assets/materials/{materials,attachments}.ron`.

**Perception** (full text read unless marked): O'Sullivan et al. 2003
[pdf](https://gv2.scss.tcd.ie/cosulliv/Pubs/fidelity.pdf); O'Sullivan & Dingliana 2001
[pdf](https://gv2.scss.tcd.ie/cosulliv/Pubs/acmcollisions.pdf); Barzel, Hughes & Wood 1996,
plausible rather than accurate motion, abstract
[page](https://cs.brown.edu/people/jhughes/papers/Barzel-PMS-1996/main.htm); Battaglia et al.
2013 [PMC](https://pmc.ncbi.nlm.nih.gov/articles/PMC3831455); Pylyshyn & Storm 1988, summary
[pdf](https://ruccs.rutgers.edu/images/personal-zenon-pylyshyn/docs/storm88.pdf); Alvarez 2011,
abstract [pdf](https://scholar.harvard.edu/alvarez/files/alvarez-2011-tics-ensemble.pdf); Hecht,
Kaiser & Banks 1996, abstract [PubMed](https://pubmed.ncbi.nlm.nih.gov/8920842); O'Regan,
Rensink & Clark 1999, abstract [page](https://www2.psych.ubc.ca/~rensink/publications/abs.99.5.html);
Klatzky, Pai & Krotkov 2000, abstract [pdf](https://www.cs.ubc.ca/labs/lci/papers/docs2000/pai-KlaPai00.pdf);
Lasseter 1987 [pdf](https://www.evl.uic.edu/aej/527/papers/lasseter.pdf).

**Engineering:** NCMA TEK 17-01D, lintels and arching [page](https://www.cmha.org/resource/tek-17-01d/).

**Games**, beyond the two repo surveys: Isaksson GDC 2024 (all 48 slides read); Kihl SIGGRAPH
2010 (pp. 1–12 read); L'Heureux GDC 2016 (text extracted); Gustafsson SED 1772 transcript (read);
Red Faction: Guerrilla via [Gamewatcher](https://www.gamewatcher.com/interviews/red-faction-guerrilla-interview/11408),
[VideoGamer](https://www.videogamer.com/previews/20090414111852-red-faction-guerrilla-interview)
and [The Sixth Axis](https://www.thesixthaxis.com/?p=10391);
[EA on Battlefield 6](https://www.ea.com/news/how-battlefield-6-redefined-destruction);
[Battlefield wiki](https://battlefield.fandom.com/wiki/Destruction) (community);
[Radiangames](https://radiangames.com/pkrg/instruments/index.html);
[KitGuru](https://kitguru.net/gaming/joao-silva/embark-studios-reveals-major-destruction-system-update-for-the-finals);
[Blast stress docs](https://nvidia-omniverse.github.io/PhysX/blast/docs/api/extensions/ext_stress.html).

**Not found:** The Finals' fragment size in numbers, server tick or core count; Instruments of
Destruction's architecture; any user study specific to building collapse or fracture
plausibility (the nearest are the collision and stability studies above).
