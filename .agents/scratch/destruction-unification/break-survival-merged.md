# What decides whether a structure stands: merged design

Subject: the game's environment destruction and damage model, game state only. Merged from two independent drafts beside this file, `break-survival-claude.md` (model: `break-survival-model/`) and `break-survival-codex.md`. Nothing is built. Numbers are MODEL unless marked: 88 mm AP, aim ±.1 m at 1 m, median (range), five seeds.

## The sentence

**Survival is read on the solid, never on the cut: a hit opens part of a slice through the thing, and it stands while what is left holds its load; the cut decides only which faces open and what falls.**

## What both drafts reached independently

1. Connectivity between chunks answers whether a path exists, not whether it carries the load. A structural read is required.
2. The remaining material decides survival, and holes count. This overturns "holes stay drawn".
3. The cutter holds no durability dial: no tuning of shots through piece length, and stretch no longer derives from strengths.
4. An invisible seed must not change a result: unexplained spread is noise, since a player can't learn a tessellation. At fixed aim the current model gives 24 shots on one seed and over 40 on two.
5. Pattern and directional strength double-count only when both aim at one bulk behaviour. Pattern gives paths, strength gives resistance. Whole-section strength is one fact, read once.
6. Shipped tools (Houdini, Blast, Chaos) separate cut from strength, and none claims cut independence: their strengths are authored per asset. Crack-band theory rescales work with cell size but cannot supply missing crack paths. Here energy isn't the limit anyway: a line loses 482 kJ against a section's 754 J of fracture work (DERIVED).

Code faults both see: `wear` reduces a pool, not its section; Navier substitutes discs for face shapes (one plank section reads 1.89, .95 or .47 of truth at 1, 4 or 16 discs); `weigh` leaves some shared supports unassessed.

## Where they differ, and my pick

| Question | Claude: section read | Codex: stress graph | Pick |
|---|---|---|---|
| **The law** | About 200 points over a plane on the element's axis; today's Navier and blow laws read what remains | Loads distributed across baked faces, with stiffness; mode-specific traction | **Section read** |
| **Cost** | .3–.6 ms per hit at a fifth hit, at an assumed 10 ns (DERIVED) | Up to 6 unknowns per chunk, iterated after failures; unmeasured | **Section read** |
| **Reach** | One axis per element; arches and forks unanswered; a failed wall may hang on its corners | General load sharing | **The graph is the door** if those show wrong in play, which is the earlier ruling on the stiffness solve |
| **Row** | Row of six; shear stays derived | Keep shear and mode-specific work | **Six,** by the material pass's test of what shows |

## Results

| Check | Current direction | Section read |
|---|---|---|
| Seed medians, stretch 2 | 5.5–16 | 3–4 |
| Finer cut (volume ÷1, 2, 4, 8) | 14, 17, 31, 40 | 4 at ÷1, ÷4 |
| Bending capacity, by stretch | 15–61 kN·m | 232 kN·m, any cut (DERIVED) |
| Shove at diameter .21, .42, .83 m | snaps | snaps, uproots, holds (DERIVED) |
| Wall by chunk size ×2, ×1, ×½, ×¼ | 14, 26, 19.5, 16 | 15; 9.5 with the service load |

Other reads: APCR 52 (39–60); machine gun over 300. It breaks within .1 m of the hits, leaving about .17 of the section. Twice as thick means ×8 bending capacity and ×4 root plate. Repeating a hole opens nothing, and walking shots across brings it down: aim gives 2–8 on a wide spread. One 88 mm HE fells nothing: at .1 m it crushes 1.7 L, and at .2 m, the closest blow the model ran, its blow reaches .05 of the section and .28 of the root plate. Not run: a contact burst, a bigger charge, a burst on a holed trunk. A plank is 8 (4–17), with 1 at no section strength, and outlasting a trunk reads oddly.

Misses:
- Stretch 1 at this cell volume gives seed medians of 1–4, because a cell is 48 % of the section; hence a bake certificate, no chunk over 30 % of a section.
- A later opposite hole can improve a read by up to 41 % by re-centring what is left, though never above intact and never lifting a fallen thing.

Not seen by the model: the fall, resting on teeth, oblique lines, fragment rays, real hulls, multi-support walls, the look.

## What changes

- **The row:** resistance, density, friction, bond strength, bond work, **section strength**. Faces read bond strength and work, for rings only. Every bonded row has a section strength, so there is no grain branch: brick's equals its bond, a trunk's is 36 MPa. This overturns the proposal to drop `along` and retires the angle law and per-face stamping.
- **Stretch** is look only.
- **New:** one global `service_pressure_pa`, a `StandIn` naming wind; without it a fir stands on 4 %, close to a felling hinge's recommended 10 %. This is where toughness lives: 300, 600, 1,200 Pa give 9.5, 5, 4 shots. Element `axis` in data; a two-support moment; the chunk certificate.
- **Goes:** the face crack law, tree HP, the shot target.

## The test

Hidden, beside fracture lane 0: real bakes, five seeds, stretch 2 and 4, plus the east wall. Print section beside graph counts; stills at one aim. Dies if seeds differ by over one shot (a player can't see a cut, a stated decision), or visible breaks miss the holes.

## Lanes, each under an hour

`Section` with Navier on points; lines and removals open points; section strength, axis, two-support moment; service load and refusal; delete face crack, wall capture; chunk certificate.

## Owner calls, with my picks

1. Read survival on the solid: **yes.**
2. Service load: **600 Pa, about 5 shots,** your eye at the first bake.
3. Repeating a hole adds nothing: **accept.**
4. A burst does less to a trunk than a solid shot: **accept** until the first HE capture.
5. Species: **one row until a second ships.**

## The whole HE round (`break-survival-model/e18_he.py`)

The 88 mm HE read above is the air blast alone. The shipped round at contact, instant fuze, also throws a 4.39 kg nose piece on at about 800 m/s and 2,328 kJ of casing in a ring 44–64° off the flight axis. MODEL, aim ±.1 m, median (range):

| Trunk at 1 m | Whole round, shots | AP, shots | Casing ring alone, shots | One centre hit, cross-section left |
|---|---|---|---|---|
| .20 m | 1 (1–1) | 1 (1–2) | 13 (7–30) | .04 |
| .40 m | 4 (2–10) | 4 (2–10) | none fell in 30 | .50 |

The nose piece does the cutting: 483 kJ in the .40 m trunk, against AP's 488. Its mass rests on the round file's assumed cavity and base. 30 % of the casing reaches the .40 m trunk, leaving .92 of the worst cross-section. The push is not in the shot counts: nose, casing and blast at .2 m total 2,034 N·s·m about the base, .83 of tipping the root plate and .16 of breaking the intact section (DERIVED); AP alone is .26 of the plate. Replaces owner call 4's "a burst does less to a trunk than a solid shot". Not run: the real shell through the shipped code, an oblique line, a burst on an already holed trunk.

## What explosions lack (`break-survival-model/e19_he.py`; owner: "the plug is realistic for APHE, but not a thin HE case")

The ring law drives only the steel beside an assumed filler cylinder and keeps the rest solid; HE inherits it from APHE. MODEL, with an ILLUSTRATIVE cavity of 53 × 255 mm (no source) leaving a .4 kg fuze. Cross-section left after one centre hit / median shots, aim ±.1 m:

| Trunk at 1 m | Shipped, plug | No plug | No plug, flight energy kept |
|---|---|---|---|
| .20 m | .04 / 1 | .63 / 3 | .43 / 2 |
| .40 m | .50 / 4 | .85 / none in 25 | .75 / none in 25 |

Casing 2,467 kJ, 4,928 kJ with flight energy; 57 % reaches the .40 m trunk. "None in 25" overstates: the model's later rays land on the original surface, and the crater isn't in that read.

Gaps: (1) no case break-up; (2) flight energy dropped at the burst (`RayDeposit::casing`); (3) a fragment deposits at the face, never along a line through what it hit, so a large energy opens little (the fragment-hit law `spall_cone.ron` already names as missing). Sourced data needed: cavity drawing; fuze function time (100 µs is 8 cm at 800 m/s; shipped crater .74 L at the .178 m setback, 2.55 L at contact).

Checked and not a gap so far: a bare charge at contact through the shipped crater law, against the RECALLED, UNVERIFIED demolition rule P = D²/40 lb. .9 kg (rule cuts .23 m): 33 cm wide, 16 cm deep. 3.1 kg (rule cuts .42 m): 48 cm wide, 25 cm deep, against the .26 m notch that fells it. Buried charges not tested.

Correction: the root plate's blow capacity (2,441 N·s·m) counts only 28 J of lifting the tree over its hinge, with no root or soil work, so the .83 above and every blow-on-plate ratio overstate tipping. If the plate held 179 kN·m through an ASSUMED 5°: 15.6 kJ, capacity 57,000 N·s·m, a whole round about .04 (DERIVED). The static shove read is unaffected. The plate needs a work term.

## Owner's ruling, 2026-11-03

"it sounds good to me. mark it as accepted, and we adjust as we go later, but (12) is full unification." Survival is read on the solid: accepted. Aligning runs now, ahead of the next game build, and it is the whole of `generic-fracture-merged.md`, not trees alone. Call 1 is settled; calls 2, 3 and 5 take the picks above, to be judged by eye at the first bake.
