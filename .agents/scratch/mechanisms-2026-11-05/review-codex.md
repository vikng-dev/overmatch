Subject: the video game’s environment destruction model; game state only.

**My decision: change the structural model first, then separate gameplay rubble from visual fragments.** Those together plausibly buy an order of magnitude on the ram. Settled rubble becoming ground does not address the initial stall. Moving a collapsing group as one body already exists; savings require fewer physical groups **and fewer member colliders**.

The headline needs correcting: **MEASURED 32.013 ms was a window’s median**, on a hosted server reporting a one-vCPU quota. Individual ticks in the initial ram windows reached **85.831 ms**. The separate seconds-long collapse stalls were on the ten-core Mac. [Raw ram log](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/owner-play-2026-11-03/ram/server.log)

The pipeline is close to your description, with consequential differences:

- **Intake:** contact points already combine into one deposit per physics step. Sleeping contacts retain their presses without starting assessments. Other deposits remain ordered.
- **Structure:** identical grounded presses already reuse a held verdict. Changed loads, support and moving-group frames still cause substantial work; unchanged topology alone cannot justify skipping it.
- **Loose pieces:** connected chunks already share a rigid body, but retain contact and ballistic colliders per member. Moving groups can undergo further structural failure.
- **Rest:** whole contact islands sleep. A tank or a few rotating members can keep a pile awake.
- **Updates:** changed group poses target all clients; stationary poses stop publishing. Structural changes already travel as ledger entries. [Current implementation](/Users/Yan/github/vikng-dev/overmatch/src/construction/breakage.rs:1), [rest](/Users/Yan/github/vikng-dev/overmatch/src/debris.rs:1), [pose publication](/Users/Yan/github/vikng-dev/overmatch/src/net/debris_stress.rs:470)

The following ranking concerns expected ram savings. DERIVED estimates are conditional hypotheses; sample percentages are CPU attribution, not measured wall-time savings.

1. **Replace repeated structural trials with one compact, change-driven support solve.**  
   **Today/cost:** contacts drive removal, impact, section, weight, ring and support checks. MEASURED historical ram: 116,129 assessments produced no persistent changes; optimized samples attributed 73–75% to assessment, including weight. [Evidence](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/slowdown-hunt/report.md:20)  
   **Proposed:** keep local material removal, but assess strength on a coarser support graph independent of visible cuts. Store distributed acceleration compactly; update affected regions when supports, loads or spin change. Reuse a held result only within a demonstrated strength margin.  
   **Precedent:** The Finals’ incremental structural solver; Blast’s graph reduction and bounded iterations; Red Faction’s coarse structural cadence. None establishes our price. [Existing research](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/slowdown-hunt/hosting-research.md:1)  
   **Players:** retain breached cover, openings, leaning and collapse after support loss; surrender some fine crack placement, exact failure order and secondary crumbling. Fresh impacts still receive immediate evaluation.  
   **Doctrine:** a deliberate approximation of material and construction laws. Resolution must be generic and labelled; strength must not be tuned to hide coarse geometry. It replaces the current fracture formulation rather than preserving every verdict.  
   **Saving, DERIVED:** reusing nine assessments in ten and halving each remaining assessment gives **20× structural savings**; both inputs need measurement. The larger opportunity is eliminating multiplying work, described below.

2. **Simulate gameplay-sized rubble; make fine breakup presentation.**  
   **Today/cost:** every physical group brings motion, contacts, possible reassessment and transport. MEASURED owner row: 72 awake bodies, 3.028 ms physics versus roughly 0.33 ms quietly; creation and contact-intake costs were not isolated.  
   **Proposed:** use a coarse physical fracture hierarchy with proportionately fewer contact regions. Fine fragments follow timestamped authoritative breakup events and have no gameplay effect; retain physical pieces wherever movement, cover or traversal matters.  
   **Precedent:** World of Tanks separates detailed destruction presentation; Dagor’s published engine has noncolliding client debris; Chaos separates simulation and replication depths. Dagor’s exact War Thunder settings remain unproven.  
   **Players:** preserve the main collapse silhouette, large obstructions and movable cover. Individual small fragments cease stacking or being nudged; decorative detail must not appear to provide reliable cover or obstruct sight lines.  
   **Doctrine:** explicitly relaxes the current all-physical presentation contract. Gameplay remains server-owned at the shared cursor; cosmetic client motion becomes an allowed exception. Coarsening needs construction/material rules, not building-specific exemptions.  
   **Saving, DERIVED:** **tenfold fewer bodies and contact regions** gives roughly tenfold body-dependent work if density stays comparable—about **2.4 ms** from the historical ram’s gross 2.7 ms incremental physics. Additional law and transport savings overlap item 1.

3. **Make rest stop work reliably; retain finite-mass rubble.**  
   **Today/cost:** island sleep can be delayed by rotation and contact wakes; sleeping near-pairs still incur work. MEASURED diagnostic native pile: forced sustained sleep changed update time from 41.17 to 0.96 ms, including avoided structural work. [Probe](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/diag-pieces-rest/report.md:1)  
   **Proposed:** repair the generic wake/contact lifecycle, retain held loads, and remove genuinely inactive pairs from active processing. Wake on actual impact or support loss.  
   **Precedent:** ordinary rigid-body sleep; Box2D explicitly removes inactive work. No supplied shipped-game evidence establishes a special rubble-freezing rule.  
   **Players:** piles finish settling and remain quiet, then respond when disturbed. This preserves later pushing and crossing better than turning them into terrain.  
   **Doctrine:** lifecycle repairs preserve the model; deliberately truncating visible motion is a resolution choice. **Rubble-as-ground adds packing, sliding, plowing and ballistic occupancy laws, loses voids and overhangs, and has no demonstrated initial-ram saving. I would defer it.**  
   **Saving, DERIVED:** the diagnostic sleep ceiling was **42.9×** for that old pile state. Initial-impact saving is **zero before rest**; savings after items 1–2 are smaller, and a production-safe repair remains unproven.

4. **Represent uninterrupted flight as an authoritative trajectory.**  
   **Today/cost:** free-flying groups use the same per-step body machinery as contacting groups. The ram’s contact-free share is unmeasured.  
   **Proposed:** evaluate gravity-driven trajectories between authoritative encounters; retain server collision discovery and switch to interactive motion when necessary. Clients render accepted trajectories at the shared cursor.  
   **Precedent:** Dagor documents scripted-gravity client debris; the supplied research establishes no equivalent authoritative server architecture.  
   **Players:** isolated falls can look unchanged. Accurate encounters with moving tanks and rotating objects are the difficult part; prescribed falls that ignore interference would change play substantially.  
   **Doctrine:** base laws and one timeline can survive. Baked or unconditional kinematic falls instead sacrifice emergent interaction and can push tanks as though infinitely massive.  
   **Saving, DERIVED:** even deleting the historical row’s entire 3.028 ms physics saves only **1.10× total tick time**; actual free-flight savings are smaller and encounter queries consume part of them. This is not a first ram experiment.

5. **Transmit compact changes, retaining one presentation time.**  
   **Today/cost:** changed poses still contain position and quaternion float fields. MEASURED historical ram send windows were around 0.6 ms, **outside the fixed tick**; publication and serialization were not separately priced.  
   **Proposed:** quantize and delta-compress authoritative poses, batch updates, and send no fine cosmetic poses. Start with the existing cadence; change cadence only after measuring interpolation needs.  
   **Precedent:** The Finals reduced destruction traffic through quantization and delta compression; Teardown prioritizes motion state separately from reliable structural commands.  
   **Players:** compact precision may be invisible; sparse updates can round off collisions and fast rotations. Every gameplay object must still represent the same presentation time.  
   **Doctrine:** compression costs precision, not authority. Reduced cadence may require more shared buffering; a separate rubble clock would violate one timeline.  
   **Saving, DERIVED:** using the research’s illustrative 55-bit pose instead of 224 float-field bits gives **4.1× payload reduction**, before framing. CPU savings are unmeasured; it cannot remove the construction tick stall.

**The system can have fewer mechanisms.** I would organize it around three responsibilities: determine broken/supporting geometry; move gameplay-relevant matter; present authoritative state and decorative detail. Rest belongs to motion. Networking publishes those states. Within structure, local removal plus one support/strength formulation can replace competing section, ring and weight searches. The lever inventory counts many resolutions and implementation details, not independent necessities.

**The minimum ram combination is items 1 and 2.** Item 3 improves the tail but cannot rescue the first collision.

DERIVED illustrative arithmetic from the historical owner rows, rounded: quiet baseline **3.0 ms**, extra physics **2.7 ms**, other extra work **26.3 ms**. If structural/intake work falls twentyfold and body work tenfold:

`3.0 + 26.3/20 + 2.7/10 = 4.585 ms`

That is **18.3× less destruction overhead**. It assumes all the remaining non-physics increment benefits; unclassified work must be measured.

For the observed **85.831 ms** ram peak, a more conservative conditional calculation is:

`3.0 + (85.831 − 3.0)/10 = 11.283 ms`

That leaves **4.342 ms**. Any unscaled work inside that excess reduces the margin. Neither calculation assumes a speedup from the second vCPU, and neither establishes a hosted maximum.

**The whole-house stall requires a more fundamental reduction.** DERIVED: **43.362 seconds / 100 = 433.62 ms**; reaching 15.625 ms requires **2,775×** before reserving other work. Simply running the present laws less often cannot achieve the goal.

The weight-cost lane identifies a credible source of such growth: MEASURED one pass expanded 2,126 external loads across 962 chunks into **14,325,344 records**. A compact representation with one external record and seven inertia terms per chunk would hold **8,860**, a DERIVED **1,617× reduction in record count**, before simplifying graph searches. This is an operation-count opportunity, not a wall-time prediction; nonlinear source effects need explicit treatment. The lane’s final optimized runtime remains unmeasured. [Analysis](/Users/Yan/github/vikng-dev/overmatch/.claude/worktrees/bay-1/.agents/docs/design/construction-weight-cost.md:97)

**Measure first:** instrument the actual full-speed ram on the hosted two-vCPU allocation, recording each tick’s intake, framing, strength/support, creation, physics and publication costs. Record graph size, expanded loads, failure passes, bodies, colliders and contacts together.

Then compare the compact structural representation first, followed by coarse physical breakup. Include the initial breach, complete collapse, settling and a tank entering the falling structure. Inspect shared openings, stopping resistance, cover and traversal.

The decisive result is the **maximum complete tick**, including creation and cascades. A cheaper median or quieter final pile does not discharge the goal.