# Research brief: how should a burst's ring be shaped per round?

Overmatch is a video game. This research is about its damage model, which is game state only: a round with a burst core, once its fuze runs out, throws a **field** (a **burst**) whose **dose** reaches modules and crew stations, each an HP pool. Vocabulary (`.agents/GLOSSARY.md`): **dose**, **potency**, **field**, **burst**, **fragment cone**, **knocked out**, **HP pool emptied**, **reduced**. Crew are HP pools at stations; describe nothing else. Keep the whole note at the GAME-REPRESENTATION level (what the game computes and draws), never real-world weapon effects.

## The question (owner, 2026-10-27)

Should the ring a burst throws be **authored per round type**, **derived from the round's charge-to-driven-mass ratio** (and/or its cavity geometry), or **kept one constant shape** for every round? The owner wants an initial understanding of how different rounds would behave under each, so the GitHub task (#138) is up to date and queued for when it becomes relevant. Nothing is built now.

## What exists (verify every claim against the source; label MEASURED from code/data or DERIVED)

- `src/ballistics/burst.rs` `Burst::derived`: the sideways speed `v_s = speed_constant / sqrt(driven / core_mass + 0.5)`, `driven` the wall mass under the ring and the whole round less the core under the ball; the burst's energy from the driven mass.
- `src/ballistics/spall_cone.rs`: `FieldLaw::Ring { half_width }`; the ring's centre sits `atan2(1, λ)` off the round's axis, `λ = carried speed / v_s`, the carried speed being the round's speed at the burst point (`src/ballistics.rs` `burst_here` → `throw_burst`); the band is uniform over ±`half_width`.
- `assets/ballistics/spall_cone.ron`: `ring_half_width_deg: (10.0, Tuned(..))`, one value for every burst.
- `assets/materials/materials.ron` `Burst(..)` rows (the burst core); the round's burst block in the tank's spec sheet (`assets/tiger_1/tiger_1.tank.ron`) — find what each round authors (core mass, cavity diameter, base mass, delay, shape Ring/Ball, …).
- Prior notes in `.agents/scratch/damage-cam-2026-10-08/research/ap-aphe-2026-10-23/` (`games.md`, `law-shape.md`, `field-render-design.md`, `brief-4b-aphe-sim.md`, `brief-4f-burst-breakup.md`, `decision-2026-10-23.md`, `calls-queue-2026-10-26.md`) and `.agents/scratch/destruction-2026-10-26/wot-scout-round{1..7}.md` (how one arcade game represents bursting rounds; design input, structure not numbers).

## What to produce

One note, `.agents/scratch/damage-cam-2026-10-08/research/ap-aphe-2026-10-23/ring-shape-by-round-2026-10-27.md`, under ~150 lines:

1. **What the law already reads per round.** Which parts of the ring's shape already differ between rounds through existing data (the tilt through `v_s` and the carried speed; the strength through the energy), and which are one constant (the band's width). A short table: quantity, source, per-round or global.
2. **The three options**, each with: what data it needs (present today or new, and where it would live: `BurstInputs` / the round's burst block / `spall_cone.ron`); what it would make visibly different between round families at the game level — a thick-walled small-charge armour-piercing round with a burst core, a thin-walled large-charge blast round, and the later families the scope goal names (squash-head, shaped-charge, tandem, rockets); whether it passes the doctrine "tanks are data: would this line survive tank #2 unchanged?" and "balance lives in data, laws are fixed"; its cost to build and to tune.
   - (a) constant shape, today's;
   - (b) derived: a width (and, if it earns its place, a band profile) as a function of quantities the round already authors — the charge-to-driven-mass ratio, the cavity's length against its diameter, the wall's extent along the axis. Reason from geometry at the game level (e.g. a long cylindrical wall throwing a narrow band, a short or rounded body a wider one) and state each such step as DERIVED, not as fact;
   - (c) authored: a width per round in its burst block.
3. **How games represent it**, from the prior notes only: do any distinguish burst shapes by round type, and how (one paragraph).
4. **Recommendation and its trigger**: which option, and the concrete event that makes building it relevant (e.g. a second bursting round whose cavity differs enough that option (b) draws a visibly different ring — compute, from the shipped data and option (b)'s candidate law, how different the 88 mm round's ring would be from today's 10°, DERIVED).
5. **Open questions** the owner must answer before building, one line each.

## Rules (binding)

- **Research only.** No code, data, test or doc change. The only file you write is the note above. Do not commit; the orchestrator commits it.
- **No web search and no web fetch** on any damage, round or explosive topic. Sources are the repo and the prior notes listed. If a question needs an outside source, list it under open questions instead.
- Do not open `.agents/scratch/**/shape-law.md`, `tiger-ammo-history.md` or `burst-delay-history.md`.
- No cargo, no Blender, no binary launches; never open a window or take the screen.
- Do not spawn subagents of your own; if this needs a fan-out, report that and stop. Do not message other sessions.
- Every claim about the code or data carries `file:line`. Label numbers MEASURED or DERIVED.

## Report

The note's path, its recommendation in two sentences, and the open questions. No agent ids, no timings.
