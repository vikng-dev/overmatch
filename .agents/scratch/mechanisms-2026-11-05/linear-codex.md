Subject: the video game’s environment destruction model; game state only. Read-only research: no edits, builds, tests, binaries or git changes.

**Complexity today**

**DERIVED: today’s structural work is polynomial, not exponential.** The expensive step is repeatedly asking “what would this cut leave unsupported?” and then weighing that answer. A graph walk itself is cheap; repeating walks and load scans across candidate cuts, then repeating the entire assessment after each failure, creates the growth. [Assessment loops](/Users/Yan/github/vikng-dev/overmatch/src/construction/fracture.rs:1859)

For one owner assessment, let `N` be source chunk slots, `J` joints, `L` outside loads, `P` combined structural/weight passes, and `D=N+J`. Let `T≤N` be touched chunks and `S≤2J` candidate sections: each joint appears alone and in at most one plane section.

The following are **DERIVED conservative upper bounds**, assuming bounded geometry per chunk; they describe work, not milliseconds. Write logarithms as `log(2+x)`.

| Step | Cost and derivation |
|---|---|
| Local intake and direct cracking | `O(L²+JL+J²)` upper bound: comparing old/new presses, testing load sources against joints, and looking up material classes. Contact-only inputs skip direct cracking because they carry no removal energy. |
| Frame moving groups | Expanded load count `M=O(N+L+Σ n_g l_g)`, hence `O(NL+N+L)` worst case. Each outside load contributes seven inertia points **on every chunk of its group**. Group selection also costs up to `O(GL log N)`. |
| Find reference paths | Per pass, `O(TD log D)` upper bound: a separate walk and sorted path for each touched chunk. |
| Try sections and weigh their groups | Up to `ST` cut trials per pass. A conservative bound is `O(ST[D log D + J(N+M log N)])`: cut walks repeat; each successful group scans loads; bearing checks can call `field` again for successive boundary joints. |
| Ring checks and rests | Per pass, conservative `O(ND log D + NJ³ + NM log M)`, plus `O(N[D log D+M log N])` for rests. Ring direction search tries pairs of boundary normals and checks each against all normals: **cubic in that boundary’s size**. |
| Release groups | For `G_new` groups, source-sized state copying and filtering cost up to `O(G_new[D log D+L log N])`, plus collider preparation. Motion thereafter retains member colliders. |

Thus one assessment costs local intake plus **`P` times the middle four rows**, plus release. Every successful pass opens an intact joint; neither loop closes one. **DERIVED: combined `P≤J+2`**, including the two final held passes. There is no search through all `2^J` broken/intact combinations. [Framing](/Users/Yan/github/vikng-dev/overmatch/src/construction/fracture.rs:1537), [weight loop](/Users/Yan/github/vikng-dev/overmatch/src/construction/fracture.rs:2259), [force scan](/Users/Yan/github/vikng-dev/overmatch/src/construction/fracture.rs:2970), [ring search](/Users/Yan/github/vikng-dev/overmatch/src/construction/fracture.rs:3427)

**MEASURED:** the open lane recorded `962` active chunks, `2,870` joints and `2,126` outside loads expanding into `14,325,344` records. **DERIVED:** `7×962×(2,126+1)+2,126=14,325,344`; at the measured `96` bytes each, that stream alone occupies `1.375 GB`. Its final optimized version remains unpriced. [Lane evidence](/Users/Yan/github/vikng-dev/overmatch/.claude/worktrees/bay-1/.agents/docs/design/construction-weight-cost.md:97)

For a tick, sum the assessments over ordered deposits and affected owners; add contact conversion, release/refitting and physics. Contacts already form **one combined deposit per physics step**, but that deposit can assess many moving owners. Conversion includes sorting, `O(C log C)`, and an intact element may require searching its chunks. [Intake and dispatch](/Users/Yan/github/vikng-dev/overmatch/src/construction/breakage.rs:897)

**DERIVED:** physics adds integration and fixed solver work approximately proportional to `G+C`, but collision discovery needs another count: candidate collider pairs. With `H` hulls, candidates can reach `O(H²)` in a crowded configuration. Fewer bodies alone does not bound contacts when every body retains many member hulls. Sleeping removes solver work but can leave non-touching nearby pairs active. [Release](/Users/Yan/github/vikng-dev/overmatch/src/construction/breakage.rs:2424), [rest lifecycle](/Users/Yan/github/vikng-dev/overmatch/src/debris.rs:1)

**MEASURED historical evidence:** the hosted ram reached an `85.831 ms` individual tick; `32.013 ms` was a window median. These predate current changes and are not today’s two-vCPU price. [Known review](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/mechanisms-2026-11-05/review-codex.md:1)

**What the literature does**

Complexities below are **DERIVED from the described algorithms**, unless explicitly labelled MEASURED. Fixed passes bound work; they do not guarantee a fully settled answer.

| Technique | Complexity and limitation | Who shipped it / precedent | Primary source |
|---|---|---|---|
| Solve forces directly on a bond graph | Fixed `I` sparse iterations: `O(L+I(N+J))`; previous results help the next solve. Exact convergence is not bounded by fixed `I`. | NVIDIA Blast SDK; no particular shipped title established here. | [Guide](https://nvidia-omniverse.github.io/PhysX/blast/docs/api/extensions/ext_stress.html), [solver source](https://raw.githubusercontent.com/NVIDIA-Omniverse/PhysX/main/blast/source/shared/stress_solver/math/cgnr.h) |
| Coarse deforming structure with detailed graphics attached | Fixed sparse iterations are linear in simulation nodes/connections. Authors limit fracture work and omit some immediate redistribution. | Digital Molecular Matter, *The Force Unleashed*. | [Parker & O’Brien 2009](https://graphics.berkeley.edu/papers/Parker-RTD-2009-08/Parker-RTD-2009-08.pdf) |
| Break joints using forces physics already calculated | Fixed sweeps cost `O(I(J+C))`, plus collision discovery and bodies; threshold scan `O(J)`. | PhysX and Jolt capabilities, not proof of a particular construction game’s implementation. | [PhysX joints](https://nvidia-omniverse.github.io/PhysX/physx/5.4.0/docs/Joints.html), [Jolt architecture](https://jrouwe.github.io/JoltPhysicsDocs/5.3.0/) |
| Local position corrections, with material stiffness | XPBD uses fixed-size local constraint updates: `O(I(N+J+C))`. Limited convergence still affects coupled structures. | Real-time research; applicable to rigid pieces and deforming networks. | [XPBD](https://matthias-research.github.io/pages/publications/XPBD.pdf), [rigid bodies](https://matthias-research.github.io/pages/publications/PBDBodies.pdf) |
| Mass points joined by breakable springs | `O(s(N+J))` for fixed substeps `s`, before contacts. Stiff springs can require small steps. | BeamNG ships node/beam structures with stiffness, damping and breaking force. | [Developer documentation](https://documentation.beamng.com/modding/vehicle/sections/beams/) |
| Support links, cached values and queued updates | A bounded queue limits tick work; arbitrary propagation is not automatically linear or promptly complete. | Rust developer history documents links, caches and queues; its original model explicitly omitted weight. | [Links](https://commits.facepunch.com/andererandre/rust_reboot/main/building_system_2), [queue/cache changes](https://commits.facepunch.com/jmthifequtgisn.html?p=2910), [original model](https://rust.facepunch.com/news/devblog-35) |
| Periodic structural assessment, separate rigid-body motion | Published cadence reduces frequency; algorithmic complexity remains undisclosed. | *Red Faction: Guerrilla*: developer describes periodic Geo-Mod scans and Havok motion. | [Developer interview](https://www.gamewatcher.com/interviews/red-faction-guerrilla-interview/11408) |
| Physical destruction reshaping the playspace | Shipped capability established; no linear-time or two-core budget established by the accessible abstract. | *The Finals*. | [Embark GDC talk](https://gdcvault.com/play/1034280/Engineering-Mayhem-Technical-Deep-Dive) |
| Local geometric fracture and cheaper tiny debris | Avoids full rigid bodies for fine detail. **MEASURED by authors:** large initial building fractures below `50 ms`, using GPU physics; structures did **not** break under their own weight. | Müller et al. research demo, not a complete support solution. | [Müller et al. 2013](https://matthias-research.github.io/pages/publications/fractureSG2013.pdf) |
| Precomputed fracture patterns or cached motion | Fixed mode count gives linear projection work; cached motion avoids solving the fall. Changed topology and interference restrict reuse. | *Breaking Good* research; Chaos supports cached destruction and rigid clusters. | [Fracture modes](https://www.dgp.toronto.edu/projects/breaking-good/), [Chaos](https://dev.epicgames.com/documentation/en-us/unreal-engine/destruction-overview) |
| Material points exchanging motion through a grid | Fixed stencil: `O(s(Q+H))`, or `O(sI(Q+H))` with local iterations. Replaces fragment-pair contacts with grid work. | MPM/PB-MPM research; GPU results do not establish two-vCPU feasibility. | [MPM source](https://github.com/yuanming-hu/taichi_mpm/blob/master/mls-mpm88-explained.cpp), [EA PB-MPM](https://media.contentapi.ea.com/content/dam/ea/seed/presentations/seed-siggraph2024-pbmpm-paper.pdf) |
| Rigid-block equilibrium with friction | Solves a constrained global optimization problem; linear equations do **not** mean linear runtime. Useful expensive reference. | Masonry research/tools. | [COMPAS formulation](https://blockresearchgroup.github.io/compas_rbe/latest/intro.html) |

Peridynamics also replaces crack-path searching with local bond interactions: fixed steps cost `O(N+J)` each, but neighbourhood size and required steps remain costs. Published acceleration work relies heavily on GPUs. It supplies a useful model family, not a cheap-server guarantee. [Primary paper](https://www.sciencedirect.com/science/article/pii/S016784422200204X)

I found no adequate primary-source complexity proof for Valheim or 7 Days to Die. Medieval Engineers confirms its own integrity algorithm; Instruments of Destruction confirms physical interaction, without exposing a usable complexity bound. [Keen founder](https://blog.marekrosa.org/2015/01/medieval-engineers_22/), [Radiangames](https://www.radiangames.com/pkrg/instruments/index.html)

**Five models for Overmatch**

For comparable arithmetic, use the supplied ram graph `N=1,163, J=3,185` and the measured collapse pass `N=962, J=2,870, L=2,126`. Ram load count remains `Lr`. These are different observed workloads, not universal construction sizes. All following operation counts are **DERIVED**; proposed counts are labelled **Resolution trials**, not measured performance.

**1. Persistent bond forces, then rigid groups — my preference.**

Collect pushes and turning effects once at each chunk. Keep the previous bond-force solution, perform bounded sparse solver work, open overloaded bonds together, then find disconnected groups once. This replaces trial cuts, ring searches and per-source inertia expansion with one shared force calculation. Blast is the clearest precedent above.

With an **eight-iteration Resolution trial** and two additional graph sweeps, bookkeeping is approximately `10(N+J)+L`: **ram `43,480+Lr`; collapse `40,446` graph-item visits**, each carrying fixed-size mathematics. Storage is `O(N+J+L)`. Motion still costs bodies, hulls and contacts.

Players retain support loss, weak connections, breached cover and interacting falls; crack order and collapse timing change. The losses are today’s section/ring verdicts and its immediate complete cascade. Summing forces preserves their total push and turn, **not today’s nonlinear per-source fracture bookkeeping**.

Doctrine cost: bond stiffness, strength and separation energy need material/construction data; iteration count is `Resolution`. Incomplete force sharing must remain visible to diagnostics, never concealed by stronger materials. Server authority and the shared presentation time survive; the same generic network handles houses, walls and trees.

**2. Route weight toward the ground in a directed network.**

Find distance from supports in one breadth-first walk. Give each chunk routes toward nearer supported chunks; accumulate weight and turning demand back along those routes. Check each connection once. No path combinations or trial cuts.

Three graph sweeps plus load intake give `O(N+J+L)`: **ram `13,044+Lr`; collapse `13,622` item visits**, followed by the same rigid-group motion as model 1. Rust’s support-link approach is precedent for the representation, not proof of this proposed weight law.

Players get fast, legible support cascades and lasting rubble. Cover and openings remain authoritative, but arches, braces, hanging structures and competing support routes can fail incorrectly. The price of guaranteed one-way calculation is losing mutual force sharing.

Doctrine cost: routing is a substantial `StandIn` for physical equilibrium, declared in generic data; sweep timing is `Resolution`. Server timing remains unified. It is cheap enough to be a serious option, but its directional bias is a gameplay concession.

**3. Make structure and fragments one network of rigid cells and breakable joints.**

Every gameplay cell has mass and motion from the start. Physics computes joint forces alongside contacts; material limits break joints. Gravity, pushing, falling and landing use one solver. This follows the PhysX/Jolt mechanism and XPBD research.

An **eight-sweep Resolution trial** costs `O(8(N+J+C)+L)`: **ram `34,784+8C+Lr`; collapse `32,782+8C`**, plus collision discovery. There is no separate weight check, but making every current chunk a body is expensive.

Using the supplied historical `35–75 µs` physics/body range gives a **conditional DERIVED** `40.7–87.2 ms` for `1,163` bodies and `33.7–72.2 ms` for `962`, before other work. Coarse cells are therefore essential. [Coefficient limits](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/slowdown-hunt/report.md:53)

Players see continuous leaning, bending and joint failure; blocking and cover follow moving cells directly. Losses include current fracture bookkeeping and potentially rigid-looking construction: insufficient solver work may produce visible softness.

Doctrine cost: strong alignment with material properties and construction, but more stiffness data and demanding numerical validation. Pass count is `Resolution`; it must not become a material-tuning knob. Generic server cells preserve one timeline.

**4. Treat fine rubble as flowing material, keeping large pieces rigid.**

After breakage, small matter transfers mass and motion through a shared three-dimensional grid. It can pile and move without testing every fragment against every neighbour. Large slabs retain rigid-body interaction. MPM supplies the precedent.

For an **eight-substep Resolution trial**, particle/grid transfers cost approximately `8(54Q+H)` visits: two 27-node transfers per particle. With one coarse particle per observed chunk and `H=4,096` visited grid nodes, **ram `535,184`; collapse `448,352` visits**, plus material calculations, structure and rigid coupling. Both particle adequacy and required substeps are unmeasured.

Players see convincing bulk flow and persistent pile-shaped blocking. Individual small pieces lose orientation, wedging and identifiable gaps; insufficient resolution can smear narrow sight lines. This changes the representation of matter, not merely when today’s bodies run.

Doctrine cost: density, friction and cohesion remain material facts; grid and particle spacing are `Resolution`. Server-owned occupancy preserves one timeline. Generic material transfer rules satisfy “tanks are data,” but two-way tank/pile interaction becomes a new law to implement and price.

**5. Bake a finite menu of failure and collapse responses.**

Precompute preferred breaking patterns and representative falls. Runtime selects a response from support loss and impact location, then updates authoritative geometry and coarse bodies. Fracture modes and Chaos caches establish the ingredients; this combined selection policy is a proposal.

For an **eight-response Resolution trial**, scanning responses costs `O(8(N+L)+N+J)`: **ram `13,652+8Lr`; collapse `28,536` visits**, plus playback/collision handling. These counts assume a fixed menu; baking every possible support state would merely move exponential work offline.

Players get inexpensive, convincing major collapses, but repeated shapes and falls. Unexpected partial support, a tank beneath the roof or a second impact during collapse may contradict playback. Responsive interference requires retaining live physical bodies, reducing the saving.

Doctrine cost: highest departure from emergent behaviour. Authored outcomes must be honestly labelled `Taste` or `StandIn`; they are not measured material laws. Timestamped server playback preserves one timeline, and asset data keeps selection generic.

**Recommendation**

Choose **model 1 with a separate, coarse physical fracture resolution**. First test the bond solver on the existing graph: linear work may already remove the structural catastrophe. Then reduce independent bodies **and member collision shapes** where the owner accepts the visible difference. Fine decorative fragments must never create apparent reliable cover or obscure authoritative openings.

Blast’s public sparse core supports the complexity argument, but importing the whole SDK does not prove it: graph reduction is experimental, and its zero iteration setting becomes a size-dependent limit. Use an explicit positive cap and inspect convergence. [Implementation](https://raw.githubusercontent.com/NVIDIA-Omniverse/PhysX/main/blast/source/shared/stress_solver/stress.cpp)

For both goals, the provisional arithmetic is:

| Term | Full-speed ram | Whole-house collapse |
|---|---:|---:|
| Structural work, DERIVED | `43,480+Lr` visits | `40,446` visits |
| Physical representation, Resolution trial | At most `64` moving groups, `128` contact hulls | Same limit through complete breakup |
| Conditional physics estimate at historical `75 µs/group` | `4.8 ms` | `4.8 ms` |
| Proposed budgets: base + structure + creation + upkeep | `3+2+2+1.5=8.5 ms` | `8.5 ms` |
| Conditional total | **`13.3 ms`** | **`13.3 ms`** |
| Remaining against `15.625 ms` | **`2.325 ms`** | **`2.325 ms`** |

This is a **DERIVED feasibility condition, not a measured forecast**. The group coefficient assumes comparable contacts; the hull limit does not itself ensure that. The `64/128` counts are workload trial budgets, never per-house exceptions or permission to erase matter. If acceptable cover geometry needs more, or creation cannot fit, this arithmetic fails. No two-core speedup is assumed.

**What to measure first**

1. Prototype persistent bond solving on captured inputs, with no current cut searches underneath it. Measure maximum complete tick, force imbalance, support-loss response and memory on the actual hosted two-vCPU allocation.
2. Try the ram and complete collapse at existing graph resolution. Record load intake, sparse iterations, broken bonds, grouping, hull creation, candidate pairs, contact rows and solver time separately.
3. Challenge the model with a cantilever, arch, uneven supports, hanging timber, tree bending, a hard landing and a tank continuing through falling material. Compare with analytical limits or a converged reference—not today’s emergent output.
4. **Kill the fixed solver budget** if acceptable support response requires work exceeding the structural allowance, or material tuning is needed to conceal incomplete convergence.
5. **Kill the proposed physical resolution** if shared cover, passages, sight lines or tank interaction require more detail than fits the complete tick. Every tick over `15.625 ms`, including breakup and creation, fails the goal.