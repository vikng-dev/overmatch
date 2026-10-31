# Structural collapse of buildings in games: survey

Subject: Overmatch's environment destruction model (buildings and walls), game state only.
Read 2026-10-31. Every number is labelled:
**MEASURED** (I counted it in repo data or a source file), **DERIVED** (computed here, inputs named),
**ASSUMED** (an input chosen here), **CITED** (stated in a primary source I read) or
**CLAIMED** (stated by a source I could not check: marketing, press, a community wiki, or a repo value
the repo itself labels CLAIMED). `[n]` points to the source list at the end. `[I]` marks my inference.
`[?]` marks something I could not find.

## TLDR

1. Shipped systems fall into three groups. Some model connectivity only: Teardown, Unreal Chaos by default, Siege and Blast's core. Some run a weight check when a structure changes: Space Engineers, 7 Days to Die, Medieval Engineers and probably Red Faction Guerrilla. The Finals runs a full solve every tick, on the server.
2. Weight affects our brick walls only through bending and tension. Self-weight compression is about 1/230 of the masonry's crushing strength. A 1 m² panel cantilevered off one 22.5 cm seam overloads our 0.69 MPa seam strength, yet the same seam can hold about 8.6 m² of wall hanging straight down (all DERIVED).
3. A static weight solve of house_1 (1,163 chunks and 3,185 joints, MEASURED) costs about 10⁸ flop with a sparse direct solver, roughly 10–40 ms on one core (DERIVED). Iterative solvers are no cheaper at this size, and when stopped early they do not carry load to distant supports, which is why Embark replaced sequential impulses with Cholesky.
4. Option (c) costs that much every tick, or about 110 explicit substeps per tick, for each active building, and it adds a pose stream. Its only extra visible effects are sagging, hinging and swaying. The Finals added those as a separate layer in October 2026.
5. Recommendation: (b), in two steps. First, add gravity to our existing section check: it is exact wherever a single cut set holds a group, needs no new solver and changes nothing on the wire. Add a sparse-Cholesky elastic solve only if spans supported at both ends need it. Reject (c) for the slice.

## 0. Our baseline (repo facts)

- **What runs today.** Hits crack joints (`fracture::crack`). The structural check (`fracture::structural`, `src/buildings/fracture.rs:873`) cracks a cut set when a push's angular impulse exceeds `√(2 I G_f A)`. It returns at once when there are no pushes (`fracture.rs:880–882`), so **gravity never enters it**. Support (`fracture::fall` / `loose_groups`, `fracture.rs:508,659`) frees any group with no path of intact joints to an anchor or a buried chunk. Freed groups fall as single bodies on the authority. Every crack and fall is a record entry that replicas derive from (`src/buildings/breakage.rs:1–38`) [50].
- **The design doc's target** (`.agents/docs/design/wall-destruction.md:404–497`) is rigid blocks with compliant seams. It states: "A path to ground identifies connectivity. It does not establish sufficient bearing, sliding resistance or bending capacity", and "Do not restrict failure to individual seams or coplanar cut sets" [50].
- **house_1, the reference case** (`assets/buildings/house_1/house_1.building.ron`, last bake 7d95f932):
  - 1,163 chunks: 1,005 brick and 158 wood (MEASURED);
  - 3,185 joints and 81 ground anchors (MEASURED);
  - brick mass 24,727 kg and wood 1,094 kg (MEASURED, summed `mass_kg`);
  - median brick chunk 0.014625 m³ and 26.98 kg; median joint area 0.0326 m² (MEASURED);
  - walls 0.25 m thick (element depth 1.75–2.0) and piers 2.925 m tall (profile −0.45 to 2.475) (MEASURED).
- **Brick seam data** (`assets/materials/materials.ron:308–335`) [50]:
  - density 1,845 kg/m³, tensile 0.69 MPa (modulus of rupture normal to bed joints) and G_f 10 J/m² (all CLAIMED there);
  - masonry assemblage compressive strength 12 MPa, stated in the row's comment (CLAIMED there);
  - **no Young's modulus** and no shear cohesion are recorded for brick (MEASURED: the brick row has `facts: None`).
- **Server tick** is 64 Hz (`src/bitprobe.rs:37`), so one tick is 15.6 ms (DERIVED) [50].

## 1. What shipped games and engines do

| System | What it models | When it runs | Cost (published) | Multiplayer sync | Our option |
|---|---|---|---|---|---|
| **NVIDIA Blast, core** | Support graph of chunks and bonds. Damage lowers bond health; island detection splits actors [1][5] | On damage | [?] | None; the SDK is physics-agnostic [1] | (a) |
| **Blast ExtStress** | Quasi-static stress over the bond graph under gravity, forces and centrifugal load. Bonds are rigid; impulses are solved by CGNR; tension, compression and shear limits are set in Pa [1][2][3][4] | Every frame, warm-started across frames, default 25 iterations (CITED [2]) | "Computation time is linearly proportional to `maxSolverIterationsPerFrame`" (CITED [1]) | None [1] | (b), but run continuously |
| **Unreal Chaos** | Connection graph from nearest neighbours. Strain is spent by collision and field impulses. Anchored pieces are static, and a cluster is one rigid body [6][7]. Damage can propagate through the graph [10]. An optional threshold model uses "physical material strength and how connected a cluster is" [8], with strengths in MPa [9]. No gravity-load solve is documented [?] | On impulse | [?] | Server replicates break state and cluster transforms up to a configured level; clients simulate below it [7] | (a) plus impulse strain |
| **Havok Destruction** | Advertised as "brittle fracture, deformation, and structural integrity analysis" [11]. DICE adopted it for Battlefield 3 [12]. Internals are not public [?] | [?] | [?] | [?] | [?] |
| **Red Faction: Guerrilla** | A "stress system" with weight. Early builds collapsed "within two minutes" because buildings "couldn't support their own weight" [14]. Damage makes "the building start to stress" [15]. The shipped link record holds `YieldMax` and `Area` for a chunk pair (CITED, reverse-engineered [16]) | "Constantly tracking", "real time" [13] | [?] | [?] | (b), likely (b)/(c) [I] |
| **Teardown** | Connectivity only. A house "only hangs on a single voxel, it is still connected", and finding connected parts across "hundreds of millions of voxels" was hard [17]. A 2026 prototype structural-integrity solver exists in Voxagon's new engine [19] | On voxel removal | [?] | Deterministic structure commands plus state sync for bodies; late join replays the command log [18] | (a) |
| **The Finals** | Fragments joined by constraints, solved by Embark's own sparse direct (Cholesky) rigid-body solver. Connection force is split into tension, compression, shear and bending with per-material multipliers. A connection breaks when `Impulse > Baseline + BreakOffset` [20]. The October 2026 season adds a "gradual" layer: floors sag, walls bend [21] | Every physics tick, on the server [20][51] | Incremental Cholesky chart: about 0.0005–0.0015 (axis labelled "ms") for 2 to ~1,900 constraints; the unit of work per point is not stated (CITED, read from chart [20] p30) | Server replicates transforms: ~400 kbit/s average in a large event, about 175 kbit/s peak after quantisation and delta compression (CITED [20] p33, p38) | (c) |
| **Battlefield (Frostbite)** | Part-based destruction with "health transitions" [22]. BF3 used Havok Destruction [12]. No weight solve is published [?] | On damage | [?] | "Prioritization and culling of part destruction and debris spawning" events [22] | Authored states, (a)-like |
| **Rainbow Six Siege** | A "connection-based leaf graph" over procedurally cut surfaces. "Game interacts with connections. Leaf graph manages state" [23] p31–32. No weight model is shown [?] | On damage; async and time-sliced [23] p63–68 | About 6 ms CPU budgeted per wall. Benchmarks: 8.1/10.3 ms on PC for one explosion through 2 drywall and 2 wood layers, "to take with a heap of salt" (CITED [23] p58, p74) | Deterministic events, not mesh states; "easy to do JIP with events" [23] p76–78 | (a) |
| **Space Engineers** (archived source) | Heuristic load paths: breadth-first search from static blocks spreads each block's mass along paths, sideways weighted 0.25. "Tension" is supported weight (CITED [24] `MyAdvancedStaticSimulator.cs:134–141, 575–700`) | When the block count or a block's integrity changes, on a worker thread; the result is swapped in on a later frame (CITED [24] `:191–250`) | [?] | Server-only: if the worst tension exceeds 10, it destroys that block, at most once per 10 updates (CITED [24] `MyStructuralIntegrity.cs`). The feature flag defaults to off (CITED [24] `MyFakes.cs:134`) | (b), heuristic, off the tick |
| **Medieval Engineers** | Havok Destruction plus "our own structural integrity algorithm", which "runs well in real-time" (CLAIMED [25]). The algorithm is unpublished; the Space Engineers archive likely holds the same code [I] | Real time [25] | [?] | [?] | (b) |
| **7 Days to Die** | Rules: a block is either vertically supported, with unlimited load above it, or limited by a per-face horizontal load. Wood is 40, concrete 120; masses are wood 5, concrete 10 (CLAIMED, community wiki [26]) | When a block is placed or removed; unstable blocks "immediately collapse" [26] | [?] | [?] | (b), rule-based |
| **Valheim** | A support scalar propagated from ground-connected pieces, recomputed every 0.5 s (CLAIMED, community wiki via repo note [27]; HTTP 402 today, not re-read) | Periodic | [?] | [?] | (b)-proxy |

Further notes:

- **Blast's stress model.** It solves `(Cᵀ I⁻¹ C) J = −Cᵀ v` for bond impulses `J` with CGNR, citing "Golub and Van Loan, section 11.3.9" (CITED [3] `stress.cpp`, `cgnr.h`).
  - Bonds are rigid. In a redundant structure, load is therefore shared by an inertia-weighted least-norm rule, not by elastic stiffness [I].
  - Stress is impulse over bond area. Bending is folded in as `bend × 2 / nodeDist` (CITED [4] `NvBlastExtStressSolver.cpp:344–372`).
  - Graph reduction "should be considered experimental" (CITED [1]).
  - Gravity needs at least one zero-mass node (CITED [1]).
- **The Finals' baseline.** Each connection's resting impulse is subtracted, so only excess load breaks it (CITED slide text [20] p25; the interpretation is [I], as in the repo's earlier survey).
  - The slide pair at [20] p28 shows the sequential-impulse solver distributing force visibly wrongly across a lintel spanning several piers, beside the sparse direct solver.
  - With and without structural analysis: without it, a shot-out wall leaves a floating slab [20] p20.
- **Teardown's level design** compensates for connectivity-only. The first building was chosen "to just show it immediately. This is how it works", and some parts are split with "physical joints" to fake support [17].

## 2. The engineering side

### 2.1 The models in plain terms

- **Rigid-block-and-spring model (RBSM).** Each chunk is rigid, with six degrees of freedom. Neighbours are tied by normal and shear springs spread over their shared face, so the face carries force and bending moment. Deformation lives in the seams, not the blocks. Origin: Kawai 1978 [28]. This is the design doc's "rigid blocks with distributed, compliant seams" (`wall-destruction.md:406–421`).
- **Discrete-element model (DEM).** The same blocks and contacts, but integrated in time explicitly with small steps. Contacts can open, slide and close, and static equilibrium is reached by damped time stepping. Origin: Cundall 1971 [29]. Standard for masonry in UDEC/3DEC (Lemos 2007 review [30]). Masonry micro-models put the seams on the real mortar joints (Lourenço 1996 [31]).
- **Limit analysis of rigid blocks.** Blocks are rigid, joints carry no tension and cannot crush. The question is whether some compression-only, friction-bounded set of joint forces balances the weights; if one exists, the structure stands. This is Heyman's "safe theorem" [32], solved as a linear program by Livesley [33] and used in graphics by Whiting et al. 2009 [34]. Kao et al. 2022 (CRA) couple it with elastic contact for assemblies [35]. The answer is yes or no plus the forces; it says nothing about deformation.
- **Sequentially linear analysis (SLA).** This is option (b) as an engineering method. Run one linear solve, scale the load until the most critical element reaches its strength, weaken that element, and solve again: "a series of linear analyses … in a sequence of events" (Rots, Belletti, Invernizzi 2008 [36]).

### 2.2 What a static weight solve costs for about 1,000 blocks (DERIVED)

Inputs:

- house_1's 1,163 chunks and 3,185 joints (MEASURED, §0);
- 6 degrees of freedom per chunk, so n = 6,978 unknowns (DERIVED);
- one 6×6 stiffness block per chunk and two per joint: 7,533 blocks, about 2.7 × 10⁵ scalar nonzeros (DERIVED);
- walls are one chunk thick, so the joint graph is close to planar: a folded surface (MEASURED geometry, [I] for the planarity);
- an effective single-core rate of **2–10 GFLOP/s** for sparse kernels of this size (ASSUMED).

| Step | Cost | Time on one core |
|---|---|---|
| Sparse Cholesky factorisation with nested dissection. For planar graphs, fill is O(N log N) and work O(N^1.5) (Lipton–Rose–Tarjan; George's model constants (31/4)·N·log₂N and (829/84)·N^1.5 [40]), times b² and b³ for 6×6 blocks | factor ≈ 8.5 × 10⁷ flop; L ≈ 3.3 × 10⁶ nonzeros, about 26 MB in f64 while held (DERIVED) | **8–42 ms** per full factorisation of the whole house (DERIVED) |
| Same, for one wall of about 300 chunks | ≈ 1.1 × 10⁷ flop (DERIVED) | **1–6 ms** (DERIVED) |
| Solve with an existing factor (forward and back substitution) | ≈ 4·nnz(L) ≈ 1.3 × 10⁷ flop (DERIVED) | about 1–7 ms (DERIVED) |
| One cracked joint: rank ≤ 6 update or downdate of L (Davis & Hager 1999 [41]) | Touches only the columns on the elimination-tree path: between microseconds and a refactorisation (DERIVED bound). Embark's chart shows ~10⁻³ "ms" per incremental step (CITED [20], unit of work unstated) | — |
| Unpreconditioned conjugate gradient | One matrix-vector product ≈ 5.4 × 10⁵ flop. Iterations ≈ ½·√κ·ln(2/ε) [42], with κ between (L/ℓ)² and (L/ℓ)⁴ (membrane versus bending), L/ℓ ≈ 13–40 chunks and ε = 10⁻⁶: **~100 to ~12,000 iterations**, 5 × 10⁷ to 6 × 10⁹ flop (DERIVED) | comparable to or far worse than direct |
| Gauss–Seidel or sequential impulse | Iterations ∝ κ, i.e. hundreds to millions of sweeps (DERIVED from the same κ) | does not converge in a frame |

Most of the cost comes from the size of the active region (N^1.5). Limiting the solve to the damaged load path, as the design doc already requires (`wall-destruction.md:495`), is the main lever [I].

### 2.3 Iterative versus direct

- **Direct (Cholesky)** is exact to rounding and costs a predictable amount per factorisation. One factor serves many right-hand sides. A broken joint is a low-rank change that can be updated rather than refactored [41]. The cost is memory for L.
  - Licence trap: SuiteSparse's update module (CHOLMOD/Modify) is GPL-2.0+, while its Cholesky core is LGPL-2.1+ (CITED [43]).
- **Iterative methods** are cheap per step and can be warm-started across frames, as Blast does [2][3].
  - Iteration count grows with the condition number, which grows with span over chunk size and with bending [42].
  - Stopped early, they under-transmit load to distant supports. That is the failure Embark showed for sequential impulse on a lintel [20] p28.
  - CGNR, Blast's choice, squares the condition number (it solves the normal equations [3]).
  - Blast rescales mass and length because rotations and translations otherwise mix badly (CITED [3] `stress.cpp`, `MASS_AND_LENGTH_SCALING`).
- **For a one-shot, event-driven check on about 1,000 blocks, direct wins** (DERIVED, §2.2). Iterative methods fit a per-frame stream when error can be tolerated (Blast's use case [1]).

### 2.4 Do results converge with chunk size?

- **Resultant force and moment on a cut converge immediately.** Statics fixes them, whatever the chunking (DERIVED).
- **Peak stress at re-entrant corners does not converge.** Window corners carry a stress singularity in linear elasticity, so a strength check cracks earlier as chunks get finer. This mesh dependence is what crack-band theory corrects by tying softening to fracture energy (Bažant & Oh 1983 [38]). Lattice and spring networks with random geometry also avoid preferring the mesh's directions (Bolander & Saito 1998 [39]).
- **Failure can only form along baked joints.** A block assembly's collapse load is therefore an upper bound that falls toward the true value as more candidate surfaces are added. Discontinuity layout optimisation (DLO) shows this numerically: "closer estimates could be obtained when the nodal density was increased" (Smith & Gilbert 2007 [37]). Coarser chunks make walls look stronger (DERIVED from the upper-bound theorem).
- The design doc's own gate is that stiffness, failure load and dissipated work must converge across cuts (`wall-destruction.md:472`). That gate is the right test [50].

### 2.5 What gravity does to our walls (DERIVED, linear stress over the joint face)

Inputs: ρ = 1,845 kg/m³, f_t = 0.69 MPa and 12 MPa assemblage strength (CLAIMED in the repo); wall thickness t = 0.25 m (MEASURED); one side joint 0.225 m × 0.25 m = 0.05625 m² (MEASURED, a typical side-joint area).

| Case | Result |
|---|---|
| Compression at the base of a 2.925 m wall, ρgh | 52.9 kPa, **1/227 of 12 MPa** |
| All 24.7 t of house_1's brick on one 0.065 m² joint, centred | 3.7 MPa, **still below 12 MPa** |
| Panel hanging straight down from one side joint, pure tension | It holds up to **8.6 m²** of 25 cm wall |
| Panel cantilevered sideways from one side joint, bending; σ = 3ρgHL²/h_j² | It holds only while H·L² ≤ 0.643 m³: a full-height (2.925 m) panel up to **0.47 m** wide, one course (0.225 m) up to **1.69 m**. A 1 m × 1 m panel reaches **1.07 MPa > 0.69 MPa** and falls |

Weight never crushes these walls, so compression does not decide collapse. Bending at a narrow neck does. The hanging case (8.6 m² holds) and the cantilever case (1 m² falls) differ by an order of magnitude, so **the seam's tension law decides what a weight check shows**, not the solver.

The repo's 0.69 MPa is a flexural modulus of rupture. Heyman's limit analysis assumes zero tension [32], and under that assumption both the hanging and the cantilevered panel fall [I].

## 3. Mapping to our three options

| | (a) Connectivity only (today, plus the impulse bending check) | (b) Weight check when the wall changes | (c) Full compliant model every tick |
|---|---|---|---|
| **Who sits here** | Teardown [17], Chaos [6][7], Siege [23], Blast core [1], Bullet's fracture demo [47]; Battlefield is authored part states [22] | Space Engineers [24], 7 Days to Die [26], Medieval Engineers [25], RFG (probable) [13][14], Valheim (proxy) [27]; Blast ExtStress is (b) run every frame [1] | The Finals [20][21] |
| **Wall hanging by one joint** | Stays (Teardown's admitted gap [17]) | Falls when the joint is in bending past f_t: a 1 m² cantilevered panel falls. With f_t = 0.69 MPa it stays when hanging straight down up to ~8.6 m² (DERIVED §2.5) | Same as (b), plus a visible swing or rock before it fails |
| **Overhang left after a pier is lost** | Stays while linked | Falls once its root cut set is overloaded. A lintel or gable still held at both ends needs the full solve (b2) | Same, plus a droop first |
| **Sagging floor or lintel** | No | No: blocks are rigid and the check is static | Yes (The Finals' 2026 layer [21]) |
| **Slow progressive collapse** | Only through freed bodies' physics | A discrete cascade: crack, re-check, crack. Its pace comes from the physics of the freed groups, or from an artificial stagger (Space Engineers waits 10 updates between failures [24]; RFG shows a delay [15]) | Continuous, with the timing coming from the mechanics |
| **Vehicle or rubble on a weakened floor** | No | Only if contact forces enter as static loads at check time [I] | Yes (RFG's weakened bridge [13]) |
| **Server cost class** | O(joints) per change event: graph search, microseconds (DERIVED) | **(b1)** gravity in the section check: same class as today's search, per event. **(b2)** full solve: ~10⁷–10⁸ flop per event per building, 1–40 ms on one core (DERIVED §2.2), can run off the tick as Space Engineers [24] and Siege [23] do | Per tick, per active building. Implicit: a factorisation each tick, about 85 Mflop × 64 Hz ≈ 5.4 GFLOP/s, 0.5–2.7 cores (DERIVED). Explicit DEM: with E = 5 GPa (ASSUMED; no brick E in our data), k = EA/ℓ = 1.44 × 10⁹ N/m and m = 27 kg, the stable step is ≈ 0.14 ms, about 114 substeps per 15.6 ms tick, ~73 Mflop per tick per building (DERIVED) |
| **Wire** | Discrete record (as today) | Unchanged: the outputs are cracks, already in `WallRecord` [50] | A pose stream for every moving block: The Finals pays about 175 kbit/s peak per client after compression (CITED [20]); the repo's own estimate is ~84 KB/s per collapsing building (DERIVED in `scout.md:77`) |

Under ADR-0037 (one timeline), (b) runs only on the authority and its cracks reach clients through the record. No client needs a deterministic solver (repo rule, AGENTS.md) [50].

## 4. Reusable code and licences

| Code | Use for | Language | Licence (CITED from the repository) |
|---|---|---|---|
| NVIDIA Blast stress solver (`source/shared/stress_solver/`, about 1,490 lines including headers, MEASURED; plus a 1,805-line extension wrapper, MEASURED) [3][4] | (b) as a rigid-bond, least-norm iterative solve; easy to port | C++ | BSD-3-Clause [5] |
| faer (`faer/src/sparse/linalg/cholesky.rs`; the LLT/LDLT update module is dense-only as far as the file list shows) [44] | (b2) sparse direct factorisation | Rust | MIT |
| nalgebra-sparse (`CscCholesky`) [45] | (b2), simpler | Rust | Apache-2.0 |
| Clarabel.rs (interior-point conic solver) [46] | (b) in Heyman's no-tension form, as a linear or second-order cone program (Whiting [34] / Livesley [33]) | Rust | Apache-2.0 |
| compas_cra (coupled rigid-block analysis, ships IPOPT) [35] | Reference formulation for compression-only plus elastic contact | Python | MIT (IPOPT is EPL-2.0) |
| SuiteSparse CHOLMOD [43] | Sparse factor update and downdate | C | Cholesky LGPL-2.1+, Modify **GPL-2.0+** |
| Bullet `FractureDemo` (`btFractureDynamicsWorld`: union-find over glued pieces) [47] | (a) reference | C++ | zlib |
| Avian 0.7 `JointForces` + `JointDisabled` (the docs' own breakable-joint recipe; XPBD joints with substeps) [48] | (c) prototype on our engine. Iterative, so it fails the way Embark's sequential impulse did [I] | Rust | already a dependency |
| Space Engineers `StructuralIntegrity/*` [24] | Reading only | C# | Keen EULA: "use this source code only for developing mods for Space Engineers" (CITED) — **not usable** |
| Unreal Chaos, Havok | — | — | Proprietary or source-available under licence — not usable [I] |

## 5. Recommendation

**Take option (b), in two steps, and reject (c) for the slice.**

1. **(b1) Add gravity to the existing section check.**
   - When the wall changes (after a strike's cracks and before support), find each group that a cached cut set alone holds to the ground. The code's `held` already computes this group (`fracture.rs:712`).
   - Statics then fixes the force and moment through that cut exactly: the group's weight and its lever arm. Compare N/A + M/S with f_t, and N/A with crushing, using the joint areas and centroids already baked.
   - Crack the worst cut and repeat until nothing fails, as `structural` already loops.
   - No solver, no new material row, no wire change. It is the same cost class as today's search.
   - What players gain: a wall hanging by a joint in bending falls, and overhangs fall (§2.5, §3).
2. **(b2) Only if playtests show it matters:** walls and gables still held at both ends ought to fail under their weight once partly damaged.
   - Then assemble the rigid-block-spring stiffness from the joint table (the design doc's `k_n ~ E'/ℓ`) and factor the damaged component with faer.
   - Crack the worst joint and update or refactor, as in SLA [36].
   - Run it on a worker if it exceeds the tick (§2.2) and record the cracks at the tick they land.
   - This needs a sourced Young's modulus for brick, which is new data.
3. **Reject (c) for the slice.**
   - Its cost class is per tick: about one factorisation, or ~114 explicit substeps, per tick per active building (DERIVED). It also needs a pose stream (§3).
   - Our engine's joints are iterative, the solver class Embark replaced for structural loads [20].
   - Its unique visible effects (sagging, hinging, swaying) are the layer The Finals added in its October 2026 season, on top of an already shipped structural model [21].

Reason: (b1) closes the stated gap ("a section stays up while one joint still links it") for every case where a single cut set holds the load. Those are the cases a player sees: a hanging wall and an overhang. It costs almost nothing and adds no new data. Each later step is optional and keeps the same record and wire.

**Conflicts and owner calls this raises** (stated here, not resolved):

- The design doc says "Do not restrict failure to individual seams or coplanar cut sets" (`wall-destruction.md:496`). (b1) is restricted to the cached cut sets. Treat (b1) as a stepping stone or not; (b2) removes the restriction.
- **The tension law decides what (b) shows** (§2.5). The repo's 0.69 MPa lets about 8.6 m² of wall hang straight down from one seam; Heyman's no-tension assumption [32] drops it. That is a data and law choice, not a solver choice.
- Whether a cascade is paced by physics alone, or staggered by a fixed delay as in Space Engineers [24]. I recommend physics alone: a fixed delay is presentation, not mechanics [I].
- Optionally, the bake could report a building that cannot stand under its own weight. Red Faction's team learned this the hard way: "within two minutes, the entire world would collapse" [14]. This is noted, not proposed as a gate.

## Sources

Read 2026-10-31 unless noted. Repository files are named by path.

1. NVIDIA, Blast `docs/api/extensions/ext_stress.rst` — https://github.com/NVIDIAGameWorks/Blast/blob/master/docs/api/extensions/ext_stress.rst
2. NVIDIA, Blast `include/extensions/stress/NvBlastExtStressSolver.h` (settings, limits in Pa, default 25 iterations) — same repo
3. NVIDIA, Blast `source/shared/stress_solver/stress.h`, `stress.cpp`, `math/cgnr.h` — same repo
4. NVIDIA, Blast `source/sdk/extensions/stress/NvBlastExtStressSolver.cpp` — same repo
5. NVIDIA, Blast `PACKAGE-LICENSES/blast-sdk-LICENSE.md` (BSD-3, © 2016–2024) — same repo
6. Epic, "Destruction Overview", UE 5.8 docs — https://dev.epicgames.com/documentation/en-us/unreal-engine/destruction-overview
7. Epic, Python API `GeometryCollectionComponent` (5.7): damage_model, damage_propagation_data, anchoring functions, replication levels — https://dev.epicgames.com/documentation/en-us/unreal-engine/python-api/class/GeometryCollectionComponent?application_version=5.7
8. Epic, `DamageModelTypeEnum` (5.7) — https://dev.epicgames.com/documentation/en-us/unreal-engine/python-api/class/DamageModelTypeEnum?application_version=5.7
9. Epic, `PhysicalMaterialStrength` (5.7) — https://dev.epicgames.com/documentation/en-us/unreal-engine/python-api/class/PhysicalMaterialStrength?application_version=5.7
10. Epic, `GeometryCollectionDamagePropagationData` (5.7) — https://dev.epicgames.com/documentation/en-us/unreal-engine/python-api/class/GeometryCollectionDamagePropagationData?application_version=5.7
11. Worthplaying, "Havok 6.0 adds two new products", 2008-09-16 (press release) — https://worthplaying.com/article/2008/9/16/news/54861-havok-60-adds-two-new-products-to-its-portfolio/
12. Havok, "Havok Destruction" product page, 2012 (Wayback, fetched with curl; includes the DICE / Battlefield 3 quote by Torbjörn Söderman) — https://web.archive.org/web/2012/http://www.havok.com/products/destruction
13. CBS News, "Red Faction: Guerrilla Q & A" (Eric Arnold, Volition), 2009-06-30 — https://www.cbsnews.com/news/red-faction-guerrilla-q-a/
14. Worthplaying, "Red Faction: Guerrilla PC developer interview" (Sean Kennedy), 2009-09-14 — https://worthplaying.com/article/2009/9/14/interviews/68376-red-faction-guerrilla-pc-developer-interview/
15. VideoGamer, "Red Faction: Guerrilla interview" (Sean Kennedy), 2009-04-14 — https://www.videogamer.com/previews/20090414111852-red-faction-guerrilla-interview/
16. rfg-modding, RfgTools++ `ChunkTypes.h` (reverse-engineered, `static_assert`-pinned struct sizes; MIT) — https://github.com/rfg-modding/RfgToolsPlusPlus/blob/master/RfgTools%2B%2B/RfgTools%2B%2B/formats/meshes/ChunkTypes.h
17. Software Engineering Daily ep. 1772, "Teardown" with Dennis Gustafsson, transcript (2024-12) — https://softwareengineeringdaily.com/wp-content/uploads/2024/12/SED1772-Teardown.txt
18. Gustafsson, "The unlikely story of Teardown Multiplayer", 2026-03-13 — https://blog.voxagon.se/2026/03/13/teardown-multiplayer.html (read 2026-10-30 via curl, per `.agents/scratch/destruction-2026-10-30-buildings/web-survey.md` §4)
19. 80.lv, "This real-time voxel building demolition is so satisfying", 2026-09-01 (Voxagon structural-integrity prototype) — https://80.lv/articles/this-real-time-voxel-building-demolition-is-so-satisfying/
20. Isaksson, "Engineering Mayhem: Technical Deep Dive into Environmental Destruction in THE FINALS", GDC 2024 slides (48 pp.; pages cited) — https://media.gdcvault.com/gdc2024/Slides/GDC+slide+presentations/Isaksson_Mans_Engineering+Mayham+Technical.pdf
21. KitGuru, "Embark Studios reveals major destruction system update for The Finals" (Season 12 "gradual" layer; from an Embark video by Roman Smirnov and Tim Overvliet) — https://kitguru.net/gaming/joao-silva/embark-studios-reveals-major-destruction-system-update-for-the-finals
22. EA, "How Battlefield 6 redefined destruction" (Antroia, Le Geyt, Leijon) — https://www.ea.com/news/how-battlefield-6-redefined-destruction
23. L'Heureux, "The Art of Destruction in Rainbow Six: Siege", GDC 2016 slides (96 pp.; pages cited) — https://media.gdcvault.com/gdc2016/Presentations/LHeureux_Julien_Art_Of_Destruction.pdf
24. Keen Software House, Space Engineers source (archived): `Sources/Sandbox.Game/Game/GameSystems/StructuralIntegrity/{MyStructuralIntegrity,MyAdvancedStaticSimulator}.cs`, `Sources/Sandbox.Game/Engine/Utils/MyFakes.cs`, `EULA.txt` — https://github.com/KeenSoftwareHouse/SpaceEngineers
25. Rosa, "Medieval Engineers" blog post, 2015-01 — https://blog.marekrosa.org/2015/01/medieval-engineers_22/
26. 7 Days to Die Wiki (community), "Structural Integrity" — https://7daystodie.wiki.gg/wiki/Structural_Integrity
27. Valheim Wiki (community), "Building stability" — https://valheim.fandom.com/wiki/Building_stability (HTTP 402 today; as summarised in `.agents/scratch/destruction-structural-houdini-2026-08-13.md` §A2)
28. Kawai, T. (1978), "New discrete models and their application to seismic response analysis of structures", *Nuclear Engineering and Design* 48(1):207–229, doi:10.1016/0029-5493(78)90217-0 (citation confirmed by search; full text not read)
29. Cundall, P. A. (1971), "A computer model for simulating progressive, large-scale movements in blocky rock systems", Proc. ISRM Symposium, Nancy (bibliographic, not re-fetched)
30. Lemos, J. V. (2007), "Discrete element modeling of masonry structures", *International Journal of Architectural Heritage* 1(2):190–213 (bibliographic, not re-fetched)
31. Lourenço, P. B. (1996), *Computational strategies for masonry structures*, PhD thesis, TU Delft (bibliographic; also cited in `materials.ron`)
32. Heyman, J. (1966), "The stone skeleton", *International Journal of Solids and Structures* 2(2):249–279 (bibliographic, not re-fetched)
33. Livesley, R. K. (1978), "Limit analysis of structures formed from rigid blocks", *International Journal for Numerical Methods in Engineering* 12(12):1853–1871 (bibliographic, not re-fetched)
34. Whiting, E., Ochsendorf, J., Durand, F. (2009), "Procedural modeling of structurally-sound masonry buildings", *ACM Transactions on Graphics* 28(5) (SIGGRAPH Asia) (bibliographic, not re-fetched)
35. Kao, G. T.-C., et al. (2022), "Coupled Rigid-Block Analysis: Stability-Aware Design of Complex Discrete-Element Assemblies", *Computer-Aided Design* 146:103216, doi:10.1016/j.cad.2022.103216; code at https://github.com/BlockResearchGroup/compas_cra (MIT; README read)
36. Rots, J. G., Belletti, B., Invernizzi, S. (2008), "Robust modeling of RC structures with an 'event-by-event' strategy", *Engineering Fracture Mechanics* 75(3–4):590–614, doi:10.1016/j.engfracmech.2007.03.027 (abstract via search)
37. Smith, C., Gilbert, M. (2007), "Application of discontinuity layout optimization to plane plasticity problems", *Proceedings of the Royal Society A* 463:2461–2484 — https://eprints.whiterose.ac.uk/3397/1/gilbertm1.pdf (read)
38. Bažant, Z. P., Oh, B. H. (1983), "Crack band theory for fracture of concrete", *Matériaux et Constructions* 16:155–177 (bibliographic, not re-fetched)
39. Bolander, J. E., Saito, S. (1998), "Fracture analyses using spring networks with random geometry", *Engineering Fracture Mechanics* 61:569–591 (bibliographic, not re-fetched)
40. George, A. (1973), "Nested dissection of a regular finite element mesh", *SIAM Journal on Numerical Analysis* 10(2):345–363; Lipton, R. J., Rose, D. J., Tarjan, R. E. (1979), "Generalized nested dissection", *SIAM Journal on Numerical Analysis* 16(2):346–358 (bibliographic; constants as I recall them, not re-fetched)
41. Davis, T. A., Hager, W. W. (1999), "Modifying a sparse Cholesky factorization", *SIAM Journal on Matrix Analysis and Applications* 20(3):606–627 (bibliographic, not re-fetched)
42. Shewchuk, J. R. (1994), "An Introduction to the Conjugate Gradient Method Without the Agonizing Pain", CMU-CS-94-125 (convergence in O(√κ) iterations) (bibliographic, not re-fetched)
43. SuiteSparse, `CHOLMOD/Doc/License.txt` (per-module licences) — https://github.com/DrTimothyAldenDavis/SuiteSparse
44. faer-rs (MIT), `faer/src/sparse/linalg/cholesky.rs` — https://github.com/sarah-quinones/faer-rs
45. nalgebra (Apache-2.0), including nalgebra-sparse — https://github.com/dimforge/nalgebra
46. Clarabel.rs (Apache-2.0) — https://github.com/oxfordcontrol/Clarabel.rs
47. Bullet3 `examples/FractureDemo/btFractureDynamicsWorld.cpp` (zlib, per `LICENSE.txt`) — https://github.com/bulletphysics/bullet3
48. avian3d 0.7.0, `src/dynamics/joints/mod.rs` (`JointForces`, `JointDisabled` recipe) and `src/lib.rs` (`xpbd_joints` default feature), local registry source; docs: https://docs.rs/avian3d/0.7.0/
49. Licences via the GitHub API: JoltPhysics MIT, rapier Apache-2.0, PhysX BSD-3-Clause.
50. Repo: `src/buildings/fracture.rs`, `src/buildings/breakage.rs`, `.agents/docs/design/wall-destruction.md`, `assets/materials/materials.ron`, `assets/buildings/house_1/house_1.building.ron`, `src/bitprobe.rs`, `.agents/scratch/destruction-2026-10-30-buildings/scout.md`, AGENTS.md.
51. GamesBeat, "Embark Studios unveils The Finals" ("server-side destruction") — https://gamesbeat.com/embark-studios-unveils-the-finals-team-based-shooter-game/ (as quoted in `.agents/scratch/destruction-survey-2026-08-13.md` §1.2; not re-read)

## Decisions taken without asking

- house_1 (1,163 chunks) stands in for "about 1,000 blocks".
- Single-core sparse throughput is ASSUMED at 2–10 GFLOP/s, and masonry E at 5 GPa (only for the explicit-step estimate; the repo records no brick modulus).
- Earlier repo surveys served as leads. I re-read primaries where reachable and marked the rest.
- The recommendation is (b), staged b1 → b2.
