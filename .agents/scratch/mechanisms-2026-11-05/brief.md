# Core-mechanism review: a believable destruction model, ten to a hundred times cheaper

Subject: the video game's environment destruction model (constructions cracking, collapsing, their loose pieces, and how they reach players) and its cost on the dedicated server. Game state only. Use the damage vocabulary in `.agents/GLOSSARY.md` where it applies. No web search. Do not open shape-law.md, tiger-ammo-history.md or burst-delay-history.md.

You work read-only in `/Users/Yan/github/vikng-dev/overmatch`: write nothing and build nothing. Your final message is your report. Another session answers the same brief independently; do not look for its answer.

## The owner's goal and worry (2026-11-05)

**Goal:** a tank rams a house at full speed, and every server tick stays inside its 15.625 ms budget on the hosted 2 vCPU. Mass destruction comes later.
- Today the owner's own ram had a worst tick of 32 ms on a 10-core Mac (MEASURED, `.agents/scratch/owner-play-2026-11-03/ram/stats.txt`).
- A whole house coming down gives single ticks of 17 to 43 seconds (MEASURED, `.agents/docs/design/server-frame-stall.md`; analysis in progress in `.agents/docs/design/construction-weight-cost.md` on branch `lane/weigh-cost`, bay-1).
- The owner's ruling: a tick over budget is a bug, never a load the game tolerates.

**Worry, in the owner's words:** "perhaps we should look at the core levers first, not small things like shapes and such, but core ideas about the mechanism and how it simulates fragments — perhaps there are ways to get a believable behaviour via a simplified simulation that's orders cheaper. Perhaps the fact itself that we have 65 levers is worrying to me, since there are so many components to the simulation."

The owner doubts that two earlier brainstorm ideas reach this goal: settled rubble becoming ground, and moving a collapsing group as one body that players' machines break up. Treat both on merit. They come back only if they win.

## Read (what's known; don't re-derive it)

- `AGENTS.md`: "One timeline" (ADR-0037: the server simulates everything, clients predict nothing, determinism matters only where a client derives state), "Behaviour emerges from base truths" (at the resolution of variables a player can perceive or act on), "Server work is parallel by ownership", "One number, one job".
- `.agents/scratch/slowdown-hunt/`:
  - `report.md`;
  - `hosting-research.md` (how shipped games host destruction; its "How stress is made cheap" says none of the read sources runs the stress solve per contact);
  - `fidelity-levers.md` (the 65 tweak levers; don't repeat them);
  - `brainstorm-codex.md` and `brainstorm-claude.md`.
- `.agents/scratch/collapse-bench/report.md`.
- The module docs of `src/construction/breakage.rs` (deposits, contacts, the order of the laws), `src/construction/fracture.rs`, `src/debris.rs` and `src/debris/launch.rs`. Read further into the code as you need to.

## The question

The pipeline reduces to a handful of core mechanisms. Our first cut is below; correct it if the code says otherwise.
1. **Intake:** shells, blasts, and every contact on every physics step become deposits.
2. **Structure check:** the laws re-evaluate cracking, crushing, weight and support after each deposit.
3. **Loose pieces:** each is a full rigid body colliding with every other piece, the ground and tanks.
4. **Rest:** sleep.
5. **Updates:** each piece's pose goes to every client.

For each mechanism:
- What does a player actually perceive from it, or act on (cover, blocking, line of sight, the look of a collapse)?
- What does it cost in the ram? Use measured numbers where they exist; label anything else DERIVED and show the derivation.
- What is the simplest model that keeps what players perceive, at ten to a hundred times less server cost? Some options to weigh:
  - a coarser schedule, or incremental evaluation;
  - fewer, larger server bodies, with fine fragments shown only by clients;
  - kinematic or scripted motion where physics adds nothing a player acts on;
  - evaluation only on change.
- Which shipped game did it that way, per `hosting-research.md`?
- What would look or play differently?
- What does it cost against the doctrine? State the cost; never use it to filter.

Then answer:
- **Mechanism count:** can the whole system be fewer mechanisms, not just cheaper ones?
- **Minimum combination:** which two or three mechanism changes, together, plausibly put the full-speed ram inside budget on 2 vCPU? Show the arithmetic.

## Report (your final message)

A menu of at most six mechanism changes, ranked by expected saving on the ram. Each gets five to eight lines:
- the mechanism;
- today's model and cost;
- the proposed model;
- the precedent;
- what players would see differently;
- the doctrine cost;
- the expected saving, DERIVED.

Then your minimum combination for the ram goal, with its arithmetic. Then what you would measure first to confirm or kill it. Plain words where you can: the owner dislikes small physical jargon. Under 120 lines.

Do not stop to ask questions: decide, and state your decision in the final message.
