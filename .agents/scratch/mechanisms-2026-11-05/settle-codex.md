**Subject: the video game’s environment destruction model—houses, walls and trees settling and collapsing. Game state only. “Rounds” means solver iterations.**

Read-only research: no edits, builds, tests or game execution. **MEASURED** below means reported by the named lanes, not independently remeasured here. **DERIVED** means inferred from equations or inspected code. **CLAIMED** identifies published claims, owner requirements or proposed experimental targets.

**The central problem is asking a changing, possibly unstable structure to reach static equilibrium before it may respond.** The implementation also has a defective globalization step, but fixing that alone cannot make an actual mechanism equilibrate. Physical inertia offers the cleanest alternative: unsupported motion becomes motion, and a brief blow survives as momentum.

The target is **CLAIMED, owner requirement: every complete server tick within 15.625 ms on the hosted two-vCPU machine**, including collision detection, fracture, body creation and replication. No reviewed source establishes that budget for this workload.

**What the local evidence establishes**

| Evidence | Consequence |
|---|---|
| **MEASURED:** house graph has 1,122 chunks and 3,258 joints; intact authored-size boot records zero cracks and continued ticks. | Initial stability is established for that observation; changed-load performance is not. [Session report](https://github.com/vikng-dev/overmatch/blob/02c093c648cd1dd714013930bc24f247948286b3/.agents/scratch/bondsolve/report-s5.md) |
| **MEASURED:** cracking the four highest-use roof joints gives first equilibrium after 354 ticks, 11,356 products and 20 contact passes. | **DERIVED:** at the target tick duration, that is approximately 5.53 seconds before the next equilibrium-based fracture decision. [Settling diagnosis](https://github.com/vikng-dev/overmatch/blob/7644651a/.agents/scratch/bondsolve/settle.md) |
| **MEASURED:** lifted beam takes 30,458 products; its final elastic contact set needs 85. | Finding the contact set dominates; this is not simply an expensive elastic solve. [Settling diagnosis](https://github.com/vikng-dev/overmatch/blob/7644651a/.agents/scratch/bondsolve/settle.md) |
| **MEASURED:** the 300 kN press cracks 540 joints, then residual reaches approximately 7e110. | Accepted iterates diverge; eventual finiteness assertions are a real production failure path. [Settling diagnosis](https://github.com/vikng-dev/overmatch/blob/7644651a/.agents/scratch/bondsolve/settle.md) |
| **MEASURED:** point-Jacobi condition estimate is a Ritz lower bound, κ ≥ 2.8e4; node-block Jacobi reduces products by approximately 24% on the sampled tangent. | Better preconditioning matters, but the measured improvement does not cure the outer iteration. [Settling diagnosis](https://github.com/vikng-dev/overmatch/blob/7644651a/.agents/scratch/bondsolve/settle.md) |
| **MEASURED:** inexact Newton reduces first re-equilibrium to 41–72 ticks but loses mechanism detection. | **DERIVED:** approximately 0.64–1.13 seconds remains. Mechanism detection must be independent of finishing a particular CG sequence. [Settling diagnosis](https://github.com/vikng-dev/overmatch/blob/7644651a/.agents/scratch/bondsolve/settle.md) |

The earlier explosive cascade was not evidence that the houses physically collapsed: unfinished fields were being judged as equilibria. The convergence gate fixed that defect. The subsequent reports distinguish genuine converged overloads from unfinished redistribution. [Contact introduction](https://github.com/vikng-dev/overmatch/blob/02c093c648cd1dd714013930bc24f247948286b3/.agents/scratch/bondsolve/report-s3.md), [gate correction](https://github.com/vikng-dev/overmatch/blob/02c093c648cd1dd714013930bc24f247948286b3/.agents/scratch/bondsolve/report-s4.md), [gravity diagnosis](https://github.com/vikng-dev/overmatch/blob/02c093c648cd1dd714013930bc24f247948286b3/.agents/scratch/bondsolve/gravity-s4.md).

**What the game actually needs**

Each tick needs authoritative geometry and motion, which connections retain cohesion, which interfaces still contact, and any resulting topology changes. Displacements need not be a player-facing output, but *compatibility or an alternative constitutive rule is still needed to determine individual connection loads*.

These are different questions:

- **Can some admissible force distribution support this structure?** Rigid-block feasibility can answer this without elastic stiffness.
- **Which connection reaches its brittle limit first along this loading history?** Equilibrium alone generally cannot.
- **What moves after support becomes inadequate?** Dynamics answers this directly; a static solve needs a separate mechanism certificate and handoff.

For example, statically indeterminate parallel connections can share a load in many equilibrium-preserving ways. Elastic compatibility chooses a distribution from their stiffnesses. Minimum force norm chooses another rule; perfect plastic redistribution chooses another. Removing displacement unknowns does not remove that modeling choice.

A complementary-energy formulation can solve for forces while retaining material compliance. It changes the numerical variables, not the underlying stiffness contrast or contact difficulty.

**Cost accounting that applies to every candidate**

Let `N` be active simulation nodes, `E` structural connections, `S` contact quadrature sites, `L` incoming loads, `H` collision hulls, `P` candidate hull pairs and `C` generated contact constraints.

All following complexity statements are **DERIVED**, assuming bounded work per connection/site:

- Fixed sparse work is linear in `N+E+S+L`; convergence to a specified accuracy need not be.
- Current fixed-round work is `O(R(N+E+S)+L)`, including bounded contact-pass overhead. Counting only CG products misses projections, backtracking and tangent rebuilding.
- **DERIVED from code:** the current cap is 32 rounds, with two load columns. Nominal original-joint traversal is therefore `32 × 2 × 3,258 = 208,512` joint visits, before quadrature expansion and other work. [Solver](https://github.com/vikng-dev/overmatch/blob/02c093c648cd1dd714013930bc24f247948286b3/src/construction/fracture/bondsolve.rs)
- The cap is per owner. Whole-tick cost sums over every active owner.
- Collision discovery is additional. In crowded configurations, `P` can grow quadratically in `H`. Sleeping and fixed constraint sweeps do not remove that bound.
- No operation count here is a millisecond forecast. The earlier reports’ assumed coefficients do not price this solver on the current host. [Earlier Codex research](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/mechanisms-2026-11-05/linear-codex.md), [earlier Claude research](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/mechanisms-2026-11-05/linear-claude.md)

**Rigid-block equilibrium and limit analysis: useful, but a different fracture model**

A force-feasibility formulation is schematically:

`equilibrium: Bq + f = 0`, with interface forces `q` inside compression, friction and cohesive-capacity sets.

An LP can use polyhedral friction; an SOCP can retain circular friction cones; a QP can select among admissible equilibria. These formulations avoid resolving tiny elastic deflections.

The lower-bound theorem is powerful within its material and plasticity assumptions. It must not be extended casually to chronological brittle cracking or non-associated Coulomb sliding. Gilbert, Casapulla and Ahmed address the latter through successive optimization problems rather than a universal single-LP cure. [Livesley](https://doi.org/10.1002/nme.1620121207), [Gilbert–Casapulla–Ahmed](https://iris.unina.it/handle/11588/300870).

Whiting, Ochsendorf and Durand use force feasibility and a tension penalty to assess masonry. Their objective serves structural design; it does not recover the loading history of brittle mortar or nailed timber. Kao’s comparison also shows that LP and QP objectives select materially different force distributions as indeterminacy increases. [Whiting primary paper](https://people.csail.mit.edu/ewhiting/resources/pubs/WhitingOchsendorfDurand-09.pdf), [Kao on mathematical programming](https://block.arch.ethz.ch/brg/files/KAO_2021_JENCM_understanding-rigid-block-equilibrium-method-mathematical-programming_1634206446.pdf).

There is a further trap: force-feasible contact forces need not be kinematically accessible. Coupled rigid-block analysis adds compatibility and contact activation, producing nonlinear optimization without a global convergence guarantee. **CLAIMED bibliographic correction:** the relevant Kao coupled-analysis paper is published in 2022; the brief identifies 2017. [CRA primary paper](https://crl.ethz.ch/papers/CRApreprint.pdf).

| Required assessment | Rigid-block candidate |
|---|---|
| Player sees | Convincing thrust paths, rocking and arch stability if contact activation is modeled adequately. Pure feasibility can keep a brittle structure standing by finding an alternative load path it cannot physically reach. |
| Tick cost | **DERIVED:** matrix-free first-order steps can cost `O(N+E+S)` each. Certificate convergence is not generally linear or bounded by a fixed step count. Direct optimizer factorizations have fill-dependent cost. |
| Robustness | Convex formulations can supply certificates for their modeled constraints. Neither a timeout nor failure to converge is an instability certificate. Full frictional compatibility loses the simple convex guarantee. |
| Within-tick blow | Requires dynamics or the current declared `impulse / tick-duration` surrogate. Static feasibility alone supplies neither impact duration nor peak demand. |
| Code change | Deletes elastic displacement solving; adds force variables, admissibility sets, certificate machinery and a load-sharing policy. Adequate contact activation adds much of the complexity back. |

**Decision:** use this family as an offline asset checker and diagnostic reference, not the primary hard-deadline brittle-fracture solver.

**Where resting contacts belong**

| Placement | Benefit | Physical and computational price |
|---|---|---|
| Inside the structural solve | Resolves cracked-face compression and bearing within a still-connected construction. | Owns nonlinear contact, slip, mechanisms and settling. A static formulation can legitimately have no answer. |
| Between rigid cohesive groups in Avian | Existing collision dynamics owns falling, rocking, sliding and resting. Structural stress receives actual external reactions. | A closed cracked seam *inside the same rigid group* cannot exert an inter-body reaction. Deleting it from the structural model loses a load path unless the group gains internal motion or is subdivided. |
| Within a compliant dynamic structural representation | Bonds, bearing and cracked-face contact share the same evolving coordinates. | Requires physical mass, motion/history state and controlled collision resolution. It replaces the static settlement problem rather than relocating it. |

Blast verifies the separation of responsibilities: its stress extension operates on a support graph and receives collision impulses; physics is supplied externally. It also supports graph reduction. It does **not** establish that every brick should become a body or that partial stress iterates meet Overmatch’s equilibrium-before-fracture requirement. NVIDIA explicitly describes graph reduction as experimental. [Blast stress documentation](https://nvidia-omniverse.github.io/PhysX/blast/docs/api/extensions/ext_stress.html).

Avian’s pinned source confirms island-wide sleeping: waking one connected body wakes its island; removing contacts or joints can wake bodies. Separate sleeping bricks therefore improve idle cost but can all awaken during a ram. [Avian pinned sleeping implementation](https://raw.githubusercontent.com/avianphysics/avian/v0.7.0/src/dynamics/rigid_body/sleeping.rs).

**Decision:** physical contacts belong with physical motion. Moving them to Avian is sound only at a representation boundary that permits the relevant relative motion. “Bonds only plus one rigid house body” is incomplete.

**Settling as dynamics**

Physical dynamics and dynamic relaxation should be distinguished. Dynamic relaxation adds fictitious inertia and damping to search for a static answer. It can be an effective solver, but its iteration clock is not automatically gameplay time. [Dynamic-relaxation primary paper](https://doi.org/10.1016/j.compstruc.2011.10.010).

| Candidate | Player sees; blow intake | DERIVED cost and robustness | Change against current code |
|---|---|---|---|
| **Explicit physical stepping** | Immediate movement, vibration and settling. An impulse changes momentum once. | `O(s(N+E+S)+L)` for fixed substeps `s`, plus collision work. Linear undamped central differences require `h ≤ 2/ω_max`; real stiff materials can make `s` unaffordable. | Deletes static equilibrium iteration and its mechanism gate; adds velocities, inertia, damping/contact history and transient fracture accounting. |
| **Dynamic relaxation with fictitious mass** | If intermediate states are hidden, delayed static decisions; if displayed, motion follows an artificial clock. A blow needs explicit preservation outside the relaxation iterations. | Same fixed-step linear work. Suitable mass/damping improves stability, but convergence still depends on the spectrum and existence of equilibrium. | Keeps the static objective, replaces CG/contact iteration with pseudo-dynamics. Does not inherently fix lost pulses or physical settling. |
| **Implicit physical stepping** | Settling and mechanisms evolve continuously; short blows persist as momentum. | A bounded iterative implementation costs `O(sI(N+E+S)+L)`. **DERIVED:** positive physical mass adds `M/h²` to a linear elastic tangent, removing static mechanism singularity. Exact linear backward Euler is stable; arbitrary truncated nonlinear solves do not inherit a blanket guarantee. | Replaces static equilibrium with a momentum equation. Adds a nonlinear dynamic solve and physical history; removes dependence on CG breakdown to permit movement. |
| **Small-step XPBD/compliant constraints** | Similar physical motion with inexpensive local contact updates. Impulses enter momentum; unilateral multipliers prevent tensile contact. | `O(sI(N+E+S)+L)` for fixed local sweeps, plus collision work. Positive compliance/inverse-mass denominators avoid global null-direction division, but finite sweeps do not guarantee accurate connection forces or arbitrary nonlinear convergence. | Deletes runtime static contact CG, backtracking and static-mechanism classification; adds per-cell poses, momenta, persistent contact state and constraint multipliers. |

XPBD gives compliance a physical interpretation, using stiffness-derived compliance rather than an arbitrary correction fraction. Its authors nevertheless state that early termination creates artificial compliance and that XPBD does not converge faster than PBD. Its force estimates are useful, but a small fixed sweep count is not a certificate of accurate brittle-fracture forces. [Original XPBD paper](https://mmacklin.com/xpbd.pdf).

Small substeps can outperform spending the same constraint-update count on one large step. That is relevant here because changing contact states are the expensive part of the current approach; it remains an experimental result to verify on stiff, heterogeneous construction. [Small Steps primary paper](https://mmacklin.com/smallsteps.pdf).

Mass scaling is especially dangerous for a ram. **DERIVED:** uniformly multiplying mass by `a` increases the explicit stability timestep by `sqrt(a)` but changes the velocity increment from impulse `J` to `J/(am)`. It changes impact response, not just solver cost. Use fictitious mass only on a clearly separate equilibrium-search clock. [Mass-scaling analysis](https://arxiv.org/abs/2410.23816).

The accepted coarse fracture time permits committing qualifying breaks together at the tick boundary. It does not require waiting for static equilibrium, nor authorize slowing physical motion. Dynamic fracture must account for actual stored energy, work and dissipation once; the current difference between sustained and pulse static energies cannot simply be relabeled as transient fracture work. [Current structural law](https://github.com/vikng-dev/overmatch/blob/02c093c648cd1dd714013930bc24f247948286b3/.agents/docs/design/construction-destruction.md#structural-calculation).

**Inside the current formulation: what the named methods really guarantee**

At a sliding site, tangential capacity changes with normal compression. **DERIVED from the inspected law:** tangential force therefore has a derivative with respect to normal closure, while normal force has no corresponding tangential derivative. The full Jacobian is generally nonsymmetric. The current tangent omits that coupling to retain a symmetric operator.

Consequently, this is not an exact Newton method for the full friction law, and that law is not generally the gradient of a convex displacement energy. It also lacks accumulated sliding history. Changing the optimizer does not add that missing history. [Contact force and linearization](https://github.com/vikng-dev/overmatch/blob/02c093c648cd1dd714013930bc24f247948286b3/src/construction/fracture/bondsolve.rs#L619).

In the table, `V=N+E+S`; work bounds are **DERIVED** for fixed iteration budgets.

| Method | Monotonicity/convergence guarantee | Tick cost and applicability |
|---|---|---|
| **MPRGP** | Objective decrease and published convergence/termination results for the appropriate strictly convex, bound-constrained QP. Not a theorem for pressure-dependent Coulomb contact. | `O(IV)` only with a sparse/local Hessian action. Displacement contact inequalities are coupled constraints; dualizing gives a contact operator containing `K⁻¹`, so one “product” can require an elastic solve. Substantial reformulation, not a drop-in CG replacement. [Dostál–Schöberl](https://doi.org/10.1007/s10589-005-4557-7) |
| **Projected Gauss–Seidel** | Exact coordinate minimization decreases a fixed convex quadratic objective. True coupled Coulomb updates lack that general guarantee. Ordering and stiffness contrast can make convergence very slow. | Local compliant primal updates or rigid-body impulse sweeps can be linear per sweep. Static force-space updates are nonlocal through elastic compliance. Adds little machinery, but may exchange global-solve cost for long propagation tails. [Contact proximal methods](https://erleben.github.io/pubs/2017/erleben.17/erleben.17.pdf) |
| **Semismooth Newton** | Fast local convergence under regularity and nonsingular generalized Jacobians. Mechanisms violate those assumptions. Globalization needs a valid merit function and descent/failure handling. | Bounded matrix-free inner/outer work can be linear at fixed caps; direct factorization is not. The full sliding tangent generally needs a nonsymmetric Krylov solver rather than CG. Adds proper derivatives, globalization and independent mechanism handling. [Static Coulomb implementation](https://pmc.ncbi.nlm.nih.gov/articles/PMC10643369/) |
| **ADMM** | Convex convergence under its subproblem assumptions; individual residuals/objectives need not decrease monotonically. Arbitrarily truncated inner solves need additional analysis. | Projections are local; the global solve dominates. Full nonlinear Coulomb ADMM explicitly lacks a theoretical convergence guarantee. Penalty changes can require refactorization. Adds primal/dual state and global-solve machinery. [Convex theory](https://stanford.edu/~boyd/papers/admm_distr_stats.html), [contact formulation](https://arxiv.org/html/2405.17020v1) |
| **Update active set once per tick, with fixed-set CG** | A scheduling bound, not a convergence theorem. Sets can alternate indefinitely; changing loads can repeatedly restart propagation. | `O(IV)` plus contact classification. Small implementation change, but merely spreading today’s iteration over ticks preserves its principal failure modes. |

For all static candidates, the player sees delayed fracture decisions unless another model supplies motion. A within-tick blow remains a load-history problem: either preserve a separate assessment transaction until completion, accepting latency and a declared quasi-static approximation, or use dynamics. Repeatedly reapplying the impulse would double-count it.

The current code rebuilds the load columns from current sources and restarts when they change. **DERIVED:** an unfinished brief pulse can disappear before the equilibrium gate permits its fracture verdict. This is confirmed by code reading, not a measured gameplay capture. [Load intake and restart](https://github.com/vikng-dev/overmatch/blob/02c093c648cd1dd714013930bc24f247948286b3/src/construction/fracture/bondsolve.rs#L1624).

**A monotone repair is necessary, but insufficient**

Including the anchor among backtracking candidates and rejecting every worsening trial prevents accepted-state residual growth. It does not ensure a descent direction, finite trial arithmetic, convergence, or a valid mechanism verdict. A rejected trial must not silently become an equilibrium.

For an anchored, frictionless compliant problem, elastic energy plus unilateral compression penalties is convex. **DERIVED:** gradient steps using a verified spectral bound can decrease that energy with linear sparse work. This is a useful diagnostic reference. It does not confer the same guarantee on the current coupled friction law.

A proper static repair therefore needs globalization with a descent fallback, inexact inner solves, better preconditioning, independent mechanism certification and deliberate pulse lifetime. Published composite methods illustrate monotone globalization with explicit failure handling, rather than accepting the least bad trial. [Farrell–Maurini](https://onlinelibrary.wiley.com/doi/10.1002/nme.5300).

No algorithm name guarantees “never panic.” Bounded work, finite accepted state, convergence and physical accuracy are separate obligations. Numerical rejection must be an explicit unfinished/failure outcome; it cannot publish guessed cracks or freeze a mechanism indefinitely.

**How much is construction sensitivity?**

**DERIVED from the named asset audit:** each roof slope has thirteen long boards, with no rafters or equivalent intermediate framing. Automatic edge nailing supplies connections whose declared density describes a different fastening arrangement. **MEASURED:** worst unit-load use is approximately 99.3%, against a **CLAIMED asset value explicitly marked Unknown: 3,240 Pa** shear analogue. [Roof audit](https://github.com/vikng-dev/overmatch/blob/02c093c648cd1dd714013930bc24f247948286b3/.agents/scratch/bondsolve/roof-s3.md).

This plausibly explains compliant modes, sensitivity to a few failed connections and difficulty distinguishing “barely held” from “free to move.” Stiff bearing beside compliant nails also supplies genuine stiffness contrast.

It does not excuse accepting increasing residuals. The lifted-beam witness exposes the numerical defect independently of this roof. Correct framing may improve conditioning and behavior; it cannot be the solver’s robustness strategy.

The useful distinction is whether a slow mode is a legitimate elastic deformation, sliding contact or an actual mechanism. Physical inertia handles all as evolving motion; a static solver must classify them.

**Granularity: preserve fracture detail without solving every detail equally**

**DERIVED:** the graph contains 6,732 rigid-node coordinates per load column before constraints. Its size alone does not establish that fine structural resolution is unaffordable. The measured bottleneck is poor conditioning multiplied by contact iteration.

| Approach | What the player retains or loses | DERIVED cost and obligations |
|---|---|---|
| **Hierarchy used only as a preconditioner** | Retains the fine joints, material law and staircase crack paths. | Fixed V-cycles can be linear if coarse graphs and interpolation remain sparse. Setup/rebuild work must also be bounded. Include rigid-motion modes of weakly connected regions; never interpolate across an opened seam as if bonded. |
| **Panels or larger simulation cells** | Retains major breaches and falls; can suppress hinges and force cracks toward panel boundaries. | Reduces solve and body counts. Requires physically derived effective stiffness and a demonstrated perceptual resolution, rather than making panels infinitely rigid. |
| **Coarse solve with local refinement** | Can retain detailed cracks near impacts, openings and stress concentrations. | Refinement must also follow poor support and high error, not just impact proximity. Local refinement cannot recover a global mechanism excluded by the coarse space. |
| **Blast-style support graph** | Useful separation between fracture detail and stress resolution. | Reduces graph work, but reduction changes load sharing and needs fracture-aware grouping. Experimental random merging is not an acceptable construction law. |

Elastic multigrid benefits from rigid-body near-nullspace information. Fracturing introduces additional weakly connected regions whose independent motions matter; a generic geometric hierarchy alone can miss them. [PETSc guidance](https://www.mcs.anl.gov/petsc/petsc-3.15/docs/docs/manual/ksp.html), [fracture-aware near-nullspaces](https://onlinelibrary.wiley.com/doi/10.1002/nme.5300).

Prefer preconditioning before physically merging cells when the goal is retaining the fine answer. Prefer physical coarsening when body/contact cost demands it. These solve different problems. Detailed graphics attached to a coarser simulation mesh have shipped successfully, but that precedent supplies no price for this server. [Parker–O’Brien](https://doi.org/10.1145/1599470.1599492).

**Server authority and the complete budget**

All candidates can be server-only. Under the stated stream contract, clients need neither identical floating-point results nor replay of structural calculations. Stable identifiers and reproducible ordering remain useful for diagnosis.

Compute independent owners or contact islands in parallel, then commit shared effects in a declared order. Internal constraint work can use disjoint groups or coloring. Do not assume the hosted second core halves a single house’s solve.

Whole-house collapse must price collision candidates, contact rows, body/hull creation and authoritative updates alongside structural work. Sleeping is an idle optimization. An iteration cap alone cannot guarantee the complete tick budget.

**Recommended path and first experiment**

**Replace runtime post-crack static settlement with physical, compliant rigid-cell dynamics, prototyped using small-step XPBD with real mass and inertia.** Keep material stiffness, attachment compliance, strength and fracture energy as data. Let cracked faces contact through the same evolving coordinates. Use coarse physical cells where interaction resolution permits, retaining finer fracture representation where it matters.

This deletes the runtime static contact-equilibrium loop, its backtracking, CG-breakdown mechanism detector and ephemeral pulse columns. It adds physical state, contact/sliding history, transient work accounting and a deliberate fine-to-coarse representation. It is a substantial change, with a stronger physical explanation for settling.

The first experiment should test the numerical core before implementing a full fracture rewrite:

- Use the existing analytic lifted-board case, parallel connections with strongly different stiffnesses, a sliding contact whose pressure changes, and an impulsively loaded mass–spring system. Keep strengths disabled initially so numerical behavior is exposed.
- **CLAIMED Resolution trial:** begin with eight substeps and one local constraint sweep per substep. **DERIVED:** substep duration at the target tick is approximately 1.953 ms. Preserve the material parameters while varying numerical resolution.
- Measure support reactions against statics, connection-force sharing against stiffness ratios, momentum intake against the applied impulse, and energy against external work and deliberate dissipation.
- **CLAIMED screening kill criterion:** reject the proposed work allocation if hand-checkable settled reactions or elastic force sharing differ by more than 1%, or if resolution changes reveal that trustworthy fracture forces require unaffordable work. This screening tolerance does not authorize uncertain decisions near a strength threshold.
- Then exercise the named post-crack house and strong-press scenarios. They must settle or move without nonfinite state, lost impulse or indefinite static waiting. Compare against a converged reference and analytical invariants, never old crack counts.
- Price the resulting representation through ram and complete collapse on the hosted allocation. **CLAIMED hard kill criterion:** any complete tick exceeding 15.625 ms rejects that representation/work allocation; averages and percentiles cannot pass it.

**Runner-up:** retain elastic compatibility and repair the static contact method with full friction linearization, safeguarded inexact Newton, independent mechanism certification and fracture-aware preconditioning. Switch to it if compliant dynamics cannot produce trustworthy connection forces within the hosted budget, while the repaired static method demonstrates bounded contact work and acceptable response latency on the same analytical cases.