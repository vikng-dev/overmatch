# A linear model of structure, breakage and fragments (Claude session)

Subject: the game's environment destruction model and its server cost, game state only. Read-only: nothing edited, built or run in the repo.

**Decisions I took.** 1-vCPU prices bound the 2-vCPU host; Mac prices × 3.7 give hosted ones; an awake server body costs 80 µs hosted all-in (35 physics + 45 upkeep). All three are inherited from `mechanisms-2026-11-05/review-claude.md`. New ASSUMED inputs: 50–100 ns hosted per chunk, joint or load visited once; a route from a chunk to the ground is 30–60 joints long in house_1.

## 1. Complexity today

Sizes: house_1 has N ≈ 1,120–1,160 chunks, J ≈ 3,185 joints and 55 elements (MEASURED, `review-claude.md`; `construction-weight-cost.md`). For one owner (a standing house or a loose piece): P its chunks, K outside loads on it, T touched chunks, Q candidate cuts weighed, F failures in its chain.

**One owner's law run** (DERIVED from `src/construction/fracture.rs`, main):

`cost ≈ F × [ 16·N + 2·J  +  7·P·K  +  T·(P+J)  +  Q·(P + J + 7·P·K) ]`

- `16·N + 2·J`: about 16 scratch arrays the size of the whole source house per pass, even for a three-chunk piece (`loose_groups` 1186, `loaded_graph` 1464–1466, `grounded` 2327, `ring_seeds` 3244, `RingLoads` 3378, `unseated` 2525, `crossing` 2215, cache compare 1780). Each piece also carries a full copy of the house's state (`breakage.rs:616–627`).
- `7·P·K`: a loose part's frame copies every outside load onto every one of its chunks as 7 points (1599–1640).
- `T·(P+J)`: one search to the ground per touched chunk (2332). In a loose piece every chunk counts as touched, because its frame gives each chunk a press (1580), so this is `P·(P+J)`.
- `Q·(…)`: for each candidate cut, a search for what it alone holds (`held`, 2355), then a scan of *every* framed load to add up its force (2994–2997). `bears_on_contact` can rescan all of them again for each bordering contact (2499).
- `F ×`: after every failure the whole check repeats, inside the same tick, until nothing fails (2304, 2444–2454).

**One tick** = the sum over every owner touched by any deposit, plus physics (roughly linear in awake bodies and contacts), plus release (hulls built per chunk). Every moving piece in contact reruns every law every tick (`breakage.rs:2190–2193`).

**The super-linear step** is the `Q·M` term: candidate cuts times framed loads, where M = 7·P·K is already chunks × loads.
- MEASURED in the whole-house collapse (`construction-weight-cost.md`): P = 962 and K = 2,126 gave M = 14,325,344 loads, exactly 7·962·2,127 + 2,126.
- One evaluation counted 23,350,643,246 steps (MEASURED). Each weighed cut adds M (3035), so that is about 1,630 full scans of 14.3 M loads (DERIVED).
- In the ram's steady state, the floor dominates. Each run initialises about 16 × 1,122 + 2 × 3,185 ≈ 24,000 house-sized slots, and 60 runs a tick make about 1.5 M slots a tick (DERIVED). MEASURED: 116,129 runs in 30.3 s changed nothing (`slowdown-hunt/report.md:20`).

**Exponential? No, polynomial.** Every pass cracks at least one joint (asserted, 2439), so F ≤ J; cuts are single joints plus planes, never subsets of joints (1712); ring growth stops at 2 (3264). Worst case F·Q·M ≈ J · J·P · 7·P·K, degree four to five in house size (DERIVED).

**Why it feels exponential:** every factor grows together in a collapse (loose chunks P, contacts K, cuts Q, failures F, owners per tick), so doubling a collapse multiplies the dominant term by about 2⁴ = 16 before the extra owners (DERIVED). MEASURED on the Mac, diagnostic run: owner evaluations per tick went 2, 2, 26, 77, 180; completed ticks took 0.67, 15.8, 19.4 and 42.3 s.

## 2. What the literature does

| Technique | Cost | Who shipped or uses it | Source |
|---|---|---|---|
| Support value spread up from the ground, lost per step, recomputed on change | the changed region | Valheim, 7 Days to Die, Rust | [gamever guide](https://gamever.io/knowledge-base/why-your-valheim-build-collapsed-stability-support-and-the-numbers-behind-it), [7DTD wiki](https://7daystodie.wiki.gg/wiki/Structural_Integrity), [Rust devblog 96](https://rust.facepunch.com/news/devblog-96) |
| Weight handed down shortest routes to anchored blocks, farthest first; worst block broken once per 10 updates | one sweep per anchor's routes | Space Engineers (released source, behind an option) | [MyAdvancedStaticSimulator.cs](https://github.com/KeenSoftwareHouse/SpaceEngineers/blob/54f2f0f3169cda687a25a438097902a43bdfa603/Sources/Sandbox.Game/Game/GameSystems/StructuralIntegrity/MyAdvancedStaticSimulator.cs), [MyStructuralIntegrity.cs](https://github.com/KeenSoftwareHouse/SpaceEngineers/blob/54f2f0f3169cda687a25a438097902a43bdfa603/Sources/Sandbox.Game/Game/GameSystems/StructuralIntegrity/MyStructuralIntegrity.cs) |
| One bond solve for the whole structure (conjugate gradient), a set number of rounds per frame, resumed next frame; loads added to the nearest node | rounds × bonds per frame | NVIDIA Blast | [stress.cpp](https://github.com/NVIDIA-Omniverse/PhysX/blob/f07ba77f2d8af805824385b4616549974d231a8d/blast/source/shared/stress_solver/stress.cpp), [cgnr.h](https://github.com/NVIDIA-Omniverse/PhysX/blob/f07ba77f2d8af805824385b4616549974d231a8d/blast/source/shared/stress_solver/math/cgnr.h), [docs](https://nvidia-omniverse.github.io/PhysX/blast/docs/api/extensions/ext_stress.html) |
| Coarse pieces as rigid bodies under constraints, exact sparse solve updated per change; break when impulse beats its steady baseline plus an offset | slide chart about flat up to ~1,900 constraints | The Finals | [GDC 2024 slides](https://media.gdcvault.com/gdc2024/Slides/GDC+slide+presentations/Isaksson_Mans_Engineering+Mayham+Technical.pdf) 22–30 |
| Finite elements; iterative solve capped per material; sleeping islands; "an arbitrary limit on the number of fracture events" per step | iterations × elements | Star Wars: The Force Unleashed | [Parker & O'Brien 2009](https://doi.org/10.1145/1599470.1599492) |
| Connection graph with strain limits; damage passed on by a factor; anchors and connectivity decide what falls | edges touched | Unreal Chaos | [Epic overview](https://dev.epicgames.com/documentation/en-us/unreal-engine/destruction-overview), [damage propagation](https://docs.unrealengine.com/PythonAPI/class/GeometryCollectionDamagePropagationData.html) |
| Connectivity only, no weight: chosen so players "can trust the environment" | the broken part | Teardown | [Gustafsson, SED](https://softwareengineeringdaily.com/wp-content/uploads/2024/12/SED1772-Teardown.txt) |
| Whole-world stress scan "every few seconds"; Havok only throws pieces | batch | Red Faction: Guerrilla | [Kennedy interview](https://www.gamewatcher.com/interviews/red-faction-guerrilla-interview/11408) |
| Impact fracture patterns; islands found after breaking decide support | local | NVIDIA research | [Müller et al. 2013](https://doi.org/10.1145/2461912.2461934) |
| Breakable joints: read the solver's joint force, switch the joint off (Avian's joints are position-based) | joints × substeps | Avian 0.7 (ours) | `vendor/avian3d-0.7.0-child-pose/src/dynamics/joints/mod.rs:173–200, 491–527`; [PBD](https://doi.org/10.1016/j.jvcir.2007.01.005) |
| Springs and beams stepped explicitly; bonded particles; peridynamics | bonds per step, very small steps for stiff matter (BeamNG: 2,000 steps/s) | BeamNG; rock and concrete research | [BeamNG architecture](https://documentation.beamng.com/beamng_tech/architecture/), [Potyondy & Cundall 2004](https://doi.org/10.1016/j.ijrmms.2004.09.011), [Silling 2000](https://doi.org/10.1016/s0022-5096(99)00029-0) |
| Lattice: break the worst bond, re-solve, repeat (today's chain, in research form) | breaks × full solve | concrete research | [Schlangen & van Mier 1992](https://doi.org/10.1007/BF02472449) |
| Exact masonry equilibrium (rigid blocks, optimisation): the expensive end | far above linear; offline | architecture tools | [Livesley 1978](https://doi.org/10.1002/nme.1620121207), [Whiting et al. 2009](https://doi.org/10.1145/1661412.1618458), [Kao et al. 2022](https://doi.org/10.1016/j.cad.2022.103216) |
| Tree-shaped constraint systems solved in one sweep; every lone-holder joint found in one pass; support kept up to date under deletions | linear; amortised | method | [Baraff 1996](https://doi.org/10.1145/237170.237226), [Tarjan 1974](https://doi.org/10.1016/0020-0190(74)90003-9), [Even & Shiloach 1981](https://doi.org/10.1145/322234.322235), [Holm et al. 2001](https://doi.org/10.1145/502090.502095) |
| Fragments: rubble as a height grid slumping to its angle of rest / material point method / stacks solved bottom-up in one pass | cells / particles + grid, small steps / contacts | research; film | [Sumner et al. 1999](https://doi.org/10.1111/1467-8659.00299), [Stomakhin et al. 2013](https://doi.org/10.1145/2461912.2461948), [Erleben 2007](https://doi.org/10.1145/1243980.1243986) |
| Fragments: fine debris on clients only, or an "abandon" level | zero server | Dagor, Chaos, World of Tanks | `slowdown-hunt/hosting-research.md` §2–3 |

**What the sources share.** None re-searches the structure for each candidate cut or expands every load onto every chunk.
- Cheap support rules push one number up from the ground (Valheim, 7 Days to Die, Rust; Medieval Engineers' own "runs well in real-time" algorithm is unpublished, [Rosa](https://blog.marekrosa.org/2015/01/medieval-engineers_22/)). Weight-carrying systems add each load once, to one node (Blast `addForce`), or pass it down a route once (Space Engineers).
- Global solvers keep one unknown per bond, solved in bounded rounds (Blast, Parker & O'Brien) or exactly on a coarse graph (The Finals); in Blast's form rigid motion makes no bond force, so a moving piece needs no frame copies. Breaks are staged everywhere: one block per 10 updates (Space Engineers), capped per step (Force Unleashed), a scan every few seconds (Red Faction).

## 3. Five models

**M1. Weight flows down a carrier tree: same laws, sums instead of searches.**
- **How:** each chunk names one neighbour that carries it toward the ground. One top-to-bottom pass leaves every joint holding the total of everything hanging from it: mass and its centre, how the mass spreads (for turning), pushes, presses and their turning. The weight law reads force and turning through a joint or plane straight from these; the push law reads inertia about a hinge from the same totals, walking its few blows along their own routes. A loose piece works on its own chunks only, its acceleration entering once as totals, not as 7 copies of every load on every chunk.
- **Breaking:** only what hung from a broken joint looks for another carrier among its neighbours (Even & Shiloach); if none reaches the ground, that part is loose.
- **Cost:** a full pass is O(N+J+K). A changed press updates only its route to the ground, O(route length). A failure costs O(what hung from it + route). At most one failure stage per owner per tick.
- **Arithmetic** (DERIVED):
  - full pass: (1,150 + 2 × 3,200 + 500–2,100) visits × 50–100 ns = 0.4–1.0 ms;
  - one changed press: 30–60 joints × 100 ns = 3–6 µs;
  - ram, steady: about 50 contact points plus 60 small pieces of about 45 visits each, 0.3–0.7 ms a tick (today 4.7).
- **Players see:** the same breaches, falls and roof failures wherever one route carries the load. A chain of 10 stages takes 0.16 s, against 0.49 s for a 1.2 m fall.
- **Lost:** nothing where one route carries. Parts two routes share (a lintel between piers, an arch) are unassessed today; one more pass over the same tree finds them in linear time (Tarjan) and keeps that verdict. Sums are added in a different order, so exact ties may round differently.
- **Doctrine cost:** base truths: same laws, but the carrier choice must never decide a shared load, or it becomes a rule. Stages per tick is a `Resolution` number. Server only, so one timeline holds. Generic, so tanks stay data.
- **Precedent:** Space Engineers' weight sweep; Baraff 1996; Tarjan 1974; staging per Space Engineers and Parker & O'Brien.

**M2. Shared-load solve, a few rounds per tick (Blast-style).**
- **How:** one sparse equation for the whole structure gives every bond the force that keeps the chunks together. It is solved by rounds of conjugate gradient, resumed each tick. Overloaded bonds fail.
- **Cost:** O(rounds × J) per active owner. ASSUMED 2 Gflop/s and ~0.5 Mflop a round for house_1, so 8 rounds take 2.0 ms a tick; on the 55-element graph, about 0.13 ms.
- **Players see:** lintels and arches share load and fail by it (new). A load reaches a support 30–60 joints away after 4–8 ticks, at one joint per round.
- **Lost:** same-tick verdicts. The answer depends on the number of rounds. Load sharing needs a bond stiffness: Blast's form assumes equal stiffness, a missing material fact (`StandIn`).
- **Precedent:** Blast; Force Unleashed; The Finals (exact and incremental, on coarse pieces).

**M3. Breakable joints inside the physics engine.**
- **How:** elements become bodies held by fixed joints. Avian reports each joint's force and can switch a joint off.
- **Cost:** a house awake is 55 × 80 µs + about 1.2 ms of joints (ASSUMED) = 5.6 ms. Standing houses become sleepers: 74 × 55 × 1.5–6 µs = 6–24 ms every tick (DERIVED), unless houses stay static until hit, and Avian rebuilds a body on a type change.
- **Lost:** brick-level breaches unless M1 runs inside each element. Iterative joint solvers misplace load in stiff frames (The Finals, slide 28).
- **Doctrine cost:** failures follow solver rounds and Avian upgrades.

**M4. Connectivity only (contrast).**
- **How:** no weight check. After a crack, search both sides at once and stop at the smaller.
- **Cost:** at most about 0.1 ms per crack.
- **Players see:** a house held by one brick stands, and nothing sags or collapses under weight (Teardown).
- **Doctrine cost:** drops weight, a base truth. This is the largest cost of the five.

**M5. Gameplay pieces on the server; the rest look-only or a rubble grid.**
- **How:** server bodies only above one generic size class (a `Unit`: stops a round, lifts a track). Smaller matter is flown by clients from the record (Chaos abandon level, Dagor). Optionally, settled small rubble fills a height grid that slumps to the substance's angle of rest (Sumner 1999).
- **Cost:** 40–100 bodies (ASSUMED) × 80 µs = 3.2–8.0 ms for a whole house. Today that is up to 962 × 35–80 µs = 34–77 ms if every fallen chunk moved alone (DERIVED). A grid of about 1,600 cells × 4 sweeps × 100 ns = 0.6 ms while it settles.
- **Players see:** big pieces still block tracks, shots and sight. Fine debris differs per client and hides nothing. A grid pile has no voids or overhangs.
- **Doctrine cost:**
  - ADR-0037 gains a look-only client layer.
  - The size class is one generic `Unit`.
  - The grid is a second authoritative surface, against the one-surface terrain rule; that is the owner's call.

## 4. Recommendation: M1, plus M5's server-body half, with staging

- **M1** replaces searching and re-adding with one pass of totals.
- **M5** keeps only gameplay-sized bodies on the server.
- **Staging:** one failure stage per owner per tick, and an admission budget for spawns.
- **Held back:** M2, the named next step if play shows lintels and arches matter; the rubble grid, until city scale.
- **Not recommended:** M3 (a standing cost of 6–24 ms, and wrong failure places); M4 (drops weight).

**Ram, worst tick, 1 vCPU, ms (DERIVED; X = spawn on the release tick, unmeasured, ASSUMED 2–6):**

| Term | Today after fix | M1 | M1 + M5 |
|---|---:|---:|---:|
| Base | 3.0 | 3.0 | 3.0 |
| Piece physics, 72 awake | 2.5 | 2.5 | 1.3 |
| Law runs, every tick | 4.7 | 0.3–0.7 | 0.2–0.5 |
| Per-body upkeep | 3.3 | 3.3 | 1.7 |
| Law work on the release tick | 4.4–46 | 0.5–1.0 | 0.5–1.0 |
| **Worst tick** | **17.9–59.5 + X** | **9.6–10.5 + X** | **6.7–7.5 + X/2** |

M1 fits while X < 5.1 ms. M1 + M5 fits while X < 16 ms.

**Whole house collapsing, worst tick, 1 vCPU, ms (DERIVED):**

| Term | Today | M1 | M1 + M5 |
|---|---:|---:|---:|
| Base | 3.0 | 3.0 | 3.0 |
| Laws | 58,000–156,000 | ≤ 2.0 | ≤ 2.0 |
| Bodies | 34–77 | 34–77 | 3.2–8.0 |
| Spawn, admitted | unmeasured | unmeasured | ≤ 2.0 |
| **Worst tick** | **≈ 10⁵** | **≈ 40–80** | **10.2–15.0** |

- Laws today: the Mac's 15.8–42.3 s × 3.7. Under M1: a 0.4–1.0 ms house pass plus at most 1.0 ms of pieces, about 10⁵ times less; bodies then break the budget alone.
- The combination fits while at most about 105 gameplay bodies are awake at once: (15.625 − 3 − 2 − 2) ms ÷ 80 µs. The second vCPU is upside and is not counted.

## 5. What to measure first

1. **Prototype M1 as a test-only function on house_1's real structure** (no game binary).
   - Compare its verdicts with today's `weigh` and `structural` on the weigh-cost lane's 36-evaluation comparison and the recorded first-break inputs; time a full pass and one route update on the Mac.
   - **Kill M1 if** any single-joint verdict differs beyond an exact tie; a full pass costs over 0.27 ms on the Mac (1.0 ms hosted); or the check that a plane alone holds its part grows faster than house size from house_1 to a doubled house (the one part whose linear form is still unproven).
2. **Census the scripted whole-house collapse:** loose-group sizes and awake bodies per tick. That sets the size class. **Kill M5's cap** if more than about 105 gameplay-sized bodies must be awake at once.
3. **Price X**, the spawn cost per released body, on the hosted allocation. M1 alone needs X under 5.1 ms on the ram's release tick.
