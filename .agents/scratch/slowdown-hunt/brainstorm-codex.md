Subject: the video game’s environment destruction and damage model; game state only.

**I would test the owner’s aggregate-settlement idea first, and dynamic bulk rubble second.** Both could remove the server’s obligation to simulate the internal arrangement of a pile while retaining meaningful tank interaction. Their savings remain unknown. The supplied evidence establishes neither feasibility nor impossibility on two cores.

This was read-only: no files changed, no builds, no web search. The GitHub connector retrieved [issue #186](https://github.com/vikng-dev/overmatch/issues/186) after `gh` failed; it has no comments. All experiments below are proposals.

**What the evidence allows**

At thirty tanks, **MEASURED** tick p50 is 14.4–16.3 ms, p95 is 17–22 ms, CPU use is about 102–103% of one core, and physics occupies about 38% of the tick. The **DERIVED** budget is 15.625 ms, leaving between 1.225 ms of p50 headroom and a 0.675 ms overrun. That baseline already needs work before adding city destruction.

There is unused CPU capacity, but it cannot automatically shorten dependent work. As an illustrative **DERIVED optimistic case**, leave physics serial and perfectly divide everything else between two cores:

`tick = 0.38T + 0.62T/2 = 0.69T`

That gives roughly 9.9–11.2 ms, leaving 4.4–5.7 ms. This assumes complete independence, no dispatch overhead and no competing work; it is not a destruction allowance.

The source also changes how to interpret the historical reports:

- Connected loose chunks **already move as one rigid body**, with member contact hulls and ballistic geometry. “Group pieces” alone repeats existing behavior. The further opportunity is to replace internal rigid-body motion or contact computation. See [group preparation](/Users/Yan/github/vikng-dev/overmatch/src/construction/breakage.rs:2449).
- Moving groups retain fracture state and can split again. Construction evaluation already runs across independent owners, followed by ordered effects. Some historical traversal costs have also been addressed through [owned membership and shared topology](/Users/Yan/github/vikng-dev/overmatch/src/construction/fracture.rs:680).
- Clients reconstruct structural changes from the ledger; moving groups receive authoritative poses. Sleeping poses already stop publishing. Client reconstruction is therefore an existing seam, but it currently does not replace server motion computation. See [ledger](/Users/Yan/github/vikng-dev/overmatch/src/net/construction.rs:1) and [pose publication](/Users/Yan/github/vikng-dev/overmatch/src/net/debris_stress.rs:504).

The old city arithmetic mixes hosts, builds and assumed piece counts. I would retain its workload—standing city, concurrent collapses, lasting rubble—as an experimental scenario, without carrying its coefficients into a two-core verdict.

The ranking below weighs expected savings against implementation and gameplay risk. Savings overlap and must not be added.

| Rank | Change in server responsibility | Expected saving | Main risk |
|---|---|---|---|
| 1 | Solve aggregate collapse movements; clients arrange pieces | High | Aggregate contact must feel credible |
| 2 | Treat rubble as a dynamic bulk material | Very high for dense, accumulated rubble | Traversal and cover change |
| 3 | Represent free flight as trajectories between authoritative events | Limited to moderate | Contact discovery may consume the saving |
| 4 | Solve the collapse outcome before its intermediate motion | High | Unexpected interference invalidates the outcome |
| 5 | Authorize geometry transitions; make the entire fall presentation | Very high | Major change to the physical game |
| 6 | Replace simultaneous debris contacts with ordered exchanges | Potentially high | Load transmission becomes order-sensitive |
| 7 | Clients compute candidates; server verifies certificates | Potentially very high | Verification may be as expensive as solving |

**1. Aggregate settlement with client reconstruction — the owner’s seed, extended**

**Mechanism.** The server represents a collapsing group through collective movements: translation, rotation, opening along failed seams, bending or compaction. Material and construction data determine the permitted movements and their resistance. Clients arrange the visible pieces around those authoritative coordinates.

The server stops integrating each internal piece and resolving its internal contacts. It still owns the group’s gameplay surface, exchanges with tanks and other constructions, and any fracture that changes that surface. This goes beyond a larger rigid slab: the aggregate can deform and settle.

**Cost and saving.** Unknown. The relevant comparison is the current cost of internal bodies, contacts, fracture assessments and carriers against the aggregate solve plus its external contacts. A small coordinate set does not help if the implementation retains every member collider or reconstructs every internal contact.

**What it costs.** Individual pieces can no longer independently jam or redistribute load unless the aggregate model represents that behavior. Keeping a detailed render mesh while simplifying only the dynamics can produce apparent cover that the authority does not recognize. The visible gameplay surface must follow the authoritative aggregate.

One timeline can survive: clients evaluate received coordinates at the shared cursor. Fine interior arrangement can vary where it has no gameplay consequence; shared openings and silhouettes need reproducible reconstruction. Base truths survive if the collective laws derive from material, mass and construction rather than named collapse animations.

**Cheapest test.** One native collapse into an uneven pile, followed by a tank crossing and pushing it. Build only enough aggregate motion to represent that case. Compare total construction/contact work and inspect tank support, momentum exchange, openings and the shared silhouette. If it needs almost every original contact to work, the hypothesis fails early.

**Precedent.** No exact shipped implementation is established by the supplied research. Chaos clusters are a related engine capability, but rigid clustering alone is already catalogued.

**2. Rubble becomes a dynamic bulk material**

**Mechanism.** Broken construction enters a material field holding mass, momentum, packing and material composition. The server computes flow, compaction and the pile’s exposed surface. Clients generate stones, planks and small movements consistent with that state.

The server stops owning persistent individual rubble bodies, mutual piece contacts and their sleeping islands. Unlike static merged rubble, this material remains movable: tanks can displace it, and removing support can make it flow.

**Cost and saving.** Unknown; potentially the strongest reduction in accumulated rubble cost. Work follows active material regions and their interfaces rather than the number of visible fragments. However, dense pressure or friction solves could replace one expensive coupled problem with another. Measure both the initial conversion and sustained material work.

**What it costs.** Bulk material naturally describes mounds better than interlocking beams, arches or suspended slabs. A single height surface loses overhangs and passages; a volumetric representation costs more. Packing must also supply an honest ballistic occupancy model, otherwise the server creates protection in visible gaps.

One timeline and server authority can remain intact. Cross-client decorative variation adds no authority cheating surface. Base truths require bulk friction, packing and strength laws; arbitrary “rubble slows tanks” rules would conceal the missing model.

**Cheapest test.** Begin with a pile beside a tank, avoiding fracture changes. Compare pushing, crossing, resting on it and removing its support. Include a sight line through a gap. Measure whether total work depends primarily on material interfaces rather than original fragment count.

**Precedent.** None established. Battlefield’s deliberately shaped persistent piles are a visual analogue, not evidence of a dynamic bulk-material solver.

**3. Free flight is a trajectory, with computation at encounters**

**Mechanism.** On departure or an authoritative impulse, the server creates a trajectory and its next possible encounter. Between encounters it evaluates motion as needed rather than running ordinary debris integration every physics step. Clients evaluate the accepted trajectory at the shared cursor.

The server stops routine integration and repeated pose publication during uninterrupted flight. It retains encounter discovery, authoritative contact resolution and invalidation when another actor changes the future.

**Cost and saving.** Unknown, probably smaller than the first two ideas because dense contact and settling remain. A cheap first measurement is the fraction of destruction CPU work spent on genuinely contact-free bodies. That fraction bounds the opportunity before implementation. Swept queries, rotating geometry and trajectory invalidation subtract from it.

**What it costs.** A departure is a complete cause only until an unknown interaction occurs. New tank motion, construction changes and subsequent impulses must invalidate affected predictions on the server. Arbitrary rotating shapes also make encounter calculation difficult; analytic translation does not solve rotational collision.

Clients must never advance past an authoritative trajectory’s validity. This preserves one timeline without client contact prediction. Base laws can remain unchanged, with numerical representation and event discovery carrying the implementation risk.

**Cheapest test.** Instrument a recorded collapse to count uninterrupted flight intervals and their CPU share. Then prototype trajectory handling for an isolated falling group and introduce a tank into its path. Stop if bookkeeping consumes most of the avoided integration work.

**Precedent.** No matching authoritative debris architecture is established. Dagor’s documented scripted client motion is a partial analogy, not proof of this server mechanism.

**4. Solve the destination first, then represent the journey**

**Mechanism.** When support fails, the server computes an admissible resting arrangement from the current geometry, gravity, material and incoming momentum. It authorizes a transition toward that arrangement, including time-dependent obstruction and a finite momentum/work account. Clients generate the detailed fall.

The server stops discovering the final pile through prolonged piece-by-piece collision. This differs from baked playback: the destination is computed from the current world.

**Cost and saving.** Unknown. Compare one arrangement solve plus interference handling against the entire original collapse and settling tail. The favorable case is a long, mostly uninterrupted collapse; repeated interference could make it more expensive than ordinary dynamics.

**What it costs.** A physically admissible endpoint does not establish a physically credible route. A conservative swept envelope can block empty space; a permissive one can miss contact. A tank entering during the fall must alter the transition and receive finite, accounted effects. Prescribed motion cannot become an infinite-mass pusher.

One timeline survives if the transition is server-owned and timestamped. Base truths pay a substantial cost: the system chooses among outcomes without resolving all intermediate causes.

**Cheapest test.** Solve one collapse beside a movable obstacle. Introduce the obstacle midway through the transition. Assess endpoint computation, obstruction accuracy and replanning cost before adding visual detail.

**Precedent.** None established. Cached destruction is related technology, but does not establish runtime outcome solving with interference.

**5. The server owns openings and ruins; the falling process is presentation**

**Mechanism.** The authority publishes geometry transitions: a support fails, an opening becomes traversable, cover disappears, and a ruin occupies its final region. Clients produce the entire visible collapse between those timestamped facts.

The server stops simulating falling rubble, its contacts and its secondary physical effects. Structural failure computation can remain authoritative.

**Cost and saving.** Unknown in milliseconds, but this removes the broadest set of obligations. Residual cost is failure evaluation, geometry replacement, retained ruin queries and event transport. It cannot remove the existing tank workload or all construction work.

**What it costs.** Falling matter no longer freely pushes tanks or initiates further physical collapse. Animation and gameplay occupancy can disagree during the transition. Giving moving debris gameplay effects would require explicitly authoring another authoritative mechanism.

One shared timeline remains possible, but the physical experience changes considerably. Base truths become weaker because intermediate behavior is presentation rather than an outcome of motion laws. This is a legitimate product choice if the savings justify it.

**Cheapest test.** Replay an existing collapse using only timestamped gameplay geometry and client animation. Drive another tank into it during the transition. The decisive evidence is whether players accept what blocks, moves and becomes traversable.

**Precedent.** Siege’s unreplicated small debris and World of Tanks’ separate destruction presentation are partial precedents. Neither establishes this whole-collapse contract. The later [precedent inventory](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/slowdown-hunt/fidelity-levers.md:157) supersedes the older report’s missing Siege replication information.

**6. Contacts become ordered transactions between owners**

**Mechanism.** Each falling owner advances against immutable occupancy and proposes exchanges at encounters. A declared commit order applies each exchange once to both participants, accounting for momentum and work. The server does not repeatedly solve a simultaneous many-body rubble island.

This changes the contact model itself; merely parallelizing the existing coupled solve is already required by ownership and already catalogued.

**Cost and saving.** Unknown. Savings require fewer revisits to interacting contacts. Candidate discovery remains, and dense piles may require repeated transactions until the original coupled solve effectively reappears. Benchmark convergence and total work together.

**What it costs.** Simultaneous load sharing becomes order-dependent. A pile may transmit tank weight late, accumulate penetration or dissipate too much energy despite every individual exchange conserving momentum. The [contact design](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/drive-loop/contact-design.md:17) identifies the important counterexample: support includes both bodies’ motion and the constraints holding the recipient up.

One timeline survives because the authority commits every result. Deterministic worker execution is unnecessary if only accepted effects are replicated. The base-truths cost is replacing simultaneous coupling with a sequential approximation.

**Cheapest test.** A tank supported by a stacked pile. Reverse transaction order and vary partitioning while checking support, momentum and energy accounting. If acceptable behavior requires extensive iteration, price that iteration before pursuing the architecture.

**Precedent.** None established in the supplied shipped-game research.

**7. Clients provide computed results with verifiable certificates**

**Mechanism.** The server sends immutable construction state and authoritative inputs to a client worker. The client proposes fracture or settlement results together with evidence. The server verifies that evidence and commits the accepted result at an authoritative tick.

The server stops searching for the result where verification is substantially cheaper than solving.

**Cost and saving.** Unknown. The necessary inequality is:

`verification + transport + deadline handling < local solve`

Conservation alone is cheap to check but insufficient. Completeness of collision detection, nonpenetration, material behavior and support may be expensive to certify. Client availability and response latency also matter.

**What it costs.** This introduces an authority attack surface: plausible but favorable results, omitted encounters, oversized proofs and withheld responses. Comparing multiple clients does not establish correctness. A missed deadline needs a deliberate server outcome whose cost is included.

One timeline can survive if results become visible only after server acceptance. Client work would be delegated computation rather than local prediction. A rigorously checkable model may require replacing the current physical formulation, which is itself a base-truths and engineering cost.

**Cheapest test.** Keep transport out initially. Produce a certificate for a small structural result and benchmark verification against solving. Feed it candidates that conserve momentum while omitting an obstruction or support. If the verifier cannot cheaply reject them, stop before building networking.

**Precedent.** Teardown establishes client simulation with authoritative correction, not inexpensive certification. No exact shipped precedent is established here.

**The two tests I would start**

First, **aggregate settlement**. It is closest to the owner’s idea and preserves a recognizable physical construction. Its decisive question is whether a small collective state can supply believable external contact without retaining the internal contact population.

Second, **dynamic bulk rubble**. It attacks the lasting city-wide cost that a good initial collapse can conceal. Its decisive question is whether tanks can push and traverse persistent rubble through a material interface whose cost stays independent of decorative fragment count.

Both experiments should initially cover one collapse or pile with real tank interaction. Scaling a visually convincing but mechanically isolated demonstration would answer too little.

Final pricing belongs on the actual two-vCPU allocation, with the completed common construction system, thirty active tanks, receiving clients, concurrent collapses and accumulated rubble. Measure total CPU work, tick tails, achieved tick rate, construction work, contact work, sends and client reconstruction. The current collapse runner is tankless; [M15 already identifies that limitation](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/destruction-unification/unification-plan-2.md:320).

**My decision is to pursue representations that eliminate internal debris state before buying cores.** If their external-contact and occupancy requirements force the server to reconstruct nearly all individual contacts, that would be strong evidence against the proposed savings. A two-core impossibility finding would then need the measured minimum cost of the destruction behavior the owner chooses to retain, beside the measured thirty-tank floor.