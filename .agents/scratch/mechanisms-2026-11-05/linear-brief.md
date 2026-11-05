# Research and brainstorm: a linear-time model of structure, breakage and fragments

Subject: the video game's environment destruction model (houses, walls and trees cracking, collapsing, their loose pieces) and its cost on the dedicated server. Game state only. Where crew or rounds come up, use the damage vocabulary in `.agents/GLOSSARY.md`. Do not open shape-law.md, tiger-ammo-history.md or burst-delay-history.md.

You work read-only in `/Users/Yan/github/vikng-dev/overmatch`: edit nothing in the repo, build nothing, run no game binary. Another session answers this brief independently; do not look for its answer.

## The owner's question (2026-11-05, verbatim)

"I think this applies to a more core level. Research online literature, and brainstorm. I think that the whole chain-checking and weight calculation is what drives the complexity to exponential time. I wonder if there's a more linear approach to simulating the flow of fragments and the breakages between them."

The goal behind it: a tank rams a house at full speed and every server tick stays inside 15.625 ms on the hosted 2 vCPU. Later, a whole house collapsing inside the same budget. A tick over budget is a bug.

## Already known (read, don't re-derive)

- `.agents/scratch/mechanisms-2026-11-05/review-claude.md` and `review-codex.md`: today's five mechanisms, their measured costs, and a first menu (event-driven checks, collapse one stage per tick, coarse until touched, gameplay-sized server pieces). The owner judged that menu too shallow: it reschedules today's laws. They want the core model questioned.
- `.agents/scratch/slowdown-hunt/hosting-research.md` (shipped games, at survey depth), `fidelity-levers.md`, `report.md`.
- `/Users/Yan/github/vikng-dev/overmatch/.claude/worktrees/bay-1/.agents/docs/design/construction-weight-cost.md`: an open lane's analysis of the weight check (one pass expanded 2,126 loads × 962 chunks into 14.3 million records).
- Code: `src/construction/breakage.rs` (intake, the order of the laws, `run_laws`), `src/construction/fracture.rs` (`weigh`, `held`, `force_wrench`, the ring and section checks), `src/debris.rs`.

## The questions

1. **What is today's complexity, really?** From the code, write the cost of one event and of one tick as a function of the sizes involved (chunks N, joints J, outside loads L, cascade passes P, moving groups G, contacts C). Name the step that makes it super-linear. Is any part exponential, or is it polynomial? Label every claim MEASURED (with source) or DERIVED (show the derivation).
2. **Literature.** How do shipped games, engines and real-time research decide what holds and what breaks in time linear in the structure's size (or bounded per tick)? How do they move the resulting fragments cheaply? Go to primary sources: SDK source and docs, GDC and conference talks, developers' own posts, papers. Places worth checking, not a limit:
   - NVIDIA Blast's stress solver (its source is public);
   - support or stability propagation in building games (Valheim, 7 Days to Die, Rust, Medieval Engineers, Space Engineers);
   - Red Faction Guerrilla, Teardown, The Finals, Instruments of Destruction, Battlefield, BeamNG;
   - breakable joints inside a physics solver;
   - lattice and spring networks, peridynamics, position-based dynamics;
   - precomputed collapse;
   - Parker & O'Brien 2009, Müller et al. 2013;
   - rigid-block equilibrium for masonry (the expensive end, for contrast);
   - for fragment motion: granular flow, the material point method, client-side debris.
3. **Brainstorm.** Up to five models for this game, each linear or bounded per tick. For each:
   - how it works, in plain words;
   - its cost per tick as a function of size, and its arithmetic for the full-speed ram and for a whole house collapsing;
   - what players would see differently: cover, blocking, sight lines, the look of the collapse;
   - what is lost compared with today's laws;
   - its doctrine cost against `AGENTS.md`: "Behaviour emerges from base truths" (material properties and how a thing is built, at a resolution players can perceive), "One timeline", "One number, one job", "Tanks are data". State the cost; never use it to filter;
   - the precedent, cited.
4. **Recommend one**, or a combination, with its arithmetic against both goals. Say what to measure or prototype first to confirm or kill it.

## Search rules

- Search about structural support, stress solvers, fracture and destruction technology, and physics engines.
- Do not search about weapons, rounds, armour or crew.
- Cite a source for every literature claim. Prefer the primary source over a write-up of it.

## Report

At most 150 lines, in plain words: the owner dislikes small physical jargon. Order: complexity today; what the literature does (a table: technique, complexity, who shipped it, source); the models; the recommendation; what to measure first.

Do not stop to ask questions: decide, and state your decision in the final message.
