# One destruction system: merged design

Subject: the game's environment destruction and damage model, game state only. Merged from two independent drafts, `generic-fracture-codex.md` and `generic-fracture-claude.md` beside this file, and checked against the destruction direction audit, `../codex/audit-destruction-direction.md`. Nothing here is built. Labels: MEASURED (a count or shipped file), MODEL (a throwaway Python model, `proto_fir.py`, not game code), DERIVED, sourced.

## The sentence

**Every breakable thing is one baked table of chunks and the faces between them, cut offline by a rule its data picks; one set of laws, one rule for what a round sees, one ledger and one view read that table, so a new material or asset adds data, never a system.**

Both drafts reached this independently, and so did the audit. Runtime fracture and voxels were both rejected: each costs far more, and neither buys anything the owner asked for.

## What both drafts agree on

- **One table.** A tree stops being a chain of 15 equal slices; it bakes the way a house does.
- **Trees lose their hit-point number.** A round reduces wood through the same laws as brick. Shots to fell a fir come from the laws (predicted 2–3 against today's 2, MODEL).
- **Grain is a material fact.** A face's strength comes from the angle between the pull and the grain (Hankinson, Wood Handbook, sourced from an excerpt; the first grain lane reads the PDF). Brick is the same law with a different axis.
- **Interfaces stay data.** Mortared, nailed, resting and root plate are named kinds in `attachments.ron`, never decided by a fracture rule.
- **One collider until disturbed.** An intact fir is one proxy. About 1.1 ms of idle server tick comes back (DERIVED, linearity assumed; 635 firs MEASURED). That closes the forest cost task.
- **Rigid pieces only.** Nothing bends or flows: a shed's panels tear off at fasteners, a truck is a rigid wreck. New behaviour of that kind means a new law later, and the drafts say so openly.
- **The bake lives in tools,** with a certificate that refuses bad output naming the file, the field and the fix.

## Where they differ, and my pick

| Question | Draft A (Claude) | Draft B (Codex) | Pick |
|---|---|---|---|
| **How a structure decides to break** | Baked cuts: the bake lists cross-sections; today's bending law reads each one. 1.2–1.5 % median error against the beam law below 9 m (MODEL) | A stiffness solve over every bond, run on an event | **Baked cuts**, owner call below |
| **How wood is cut** | Stretched Voronoi; the stretch comes from the wood's own strengths (k = 4.0 spruce, 5.2 pine, 7.1 birch) | Grain-directed split planes with staggered cross-cuts; warns Voronoi can read as elongated stone | **Stretched Voronoi first,** owner's eye decides; the rule is data, so the swap is cheap |
| **Wire** | One ledger of causes; pieces derive on each client (about 225 B for a felled fir, DERIVED) | Atomic transactions with explicit membership, plus a checkpoint | **Ledger of causes:** already how walls work, smaller |
| **What a spent chunk leaves** | Leaves the sim; residue is view only | Residue above a size keeps geometry and mass | **View only:** simpler; the other is a later law |
| **What a round sees** | One rule: a ballistic solid on every live chunk, standing, freed or settled | Not stated at this level | **Draft A's rule**, added at the owner's request |
| **Path** | 13 lanes; about 2,600 non-test and 2,900 test lines go (MEASURED totals) | 15 lanes; a solver prototype must earn adoption first | **Draft A's path**, with the gaps below |

## Gaps the merge adds

The audit named two things neither draft fully closes, so they join the path:

1. **A line's work goes to chunks by what the disc met.** Today all entrance work goes to one chunk picked by a rounding-sensitive minimum, and a collapsed chord falls back to the nearest chunk. Group proxies keep this partition step, so it needs one stated allocation rule.
2. **A field stop names its chunk.** Today every nearby building is searched and a tolerance-backed recast guesses the owner.

And one limit stays stated: holes remain drawn. A chunk's pool falls while its geometry, mass and section stay whole. The audit calls this a compensation (medium confidence); both drafts keep it as a declared approximation, because a chunk is the resolution a player reads. This is an open choice, flagged, not hidden.

## Owner calls

1. **How a structure decides to break.** Baked cuts keep today's bending law, the house's numbers and a small path. Their cost: a load shared across several supports still is not distributed, which the audit lists as a compensation (a part can stay standing that should not). The stiffness solve closes that, at about 7,000 unknowns for the house (DERIVED), with every number moving. **Recommend baked cuts,** and open the solve only if a wrong hold shows in play.
2. **Trees lose their hit-point number.** Both drafts recommend yes. Cost: balance moves to wood's row, with no direct dial.
3. **Roofs change.** Planks stop snapping across the grain and fall as long pieces with slanted ends. Recommend accepting. Cost: visibly different breaks.
4. **Metal stays rigid.** Recommend accepting. Cost: a truck is a rigid wreck, and a crumple can only be drawn.
5. **Your eye on the first broken fir.** Stretched Voronoi may read as crystal. If it does: displaced faces first, then the grain-plane rule.

## What would prove it wrong

From both drafts:
- the Rust section read exceeds 10 % against the beam law below 18 m;
- shots to fell land far from 2–3;
- a blast on 20 firs exceeds one 15.6 ms tick once hull builds are priced;
- the forest saving comes in under 90 % of 1.1 ms;
- a fir stops uprooting before it snaps;
- a round visibly stops on air at a group's toothed end, which sends groups back to one solid per chunk, at about 1.1 ms for 100 felled firs (DERIVED).

## The path

0. Prototype in tools only: cut a fir, feed the table to today's laws in a test, print section errors and shots to fell.
1. The round rule on today's code (freed pieces are met).
2. Renames: `WallState` to `ChunkState`, `Masonry` to `Matter`, and the rest.
3. Face polygons and baked house sections, fixtures re-measured.
4. Grain row; roof before and after.
5. Anchor kinds. **Merge adds:** line allocation and field stop addresses.
6. Hull proxies, `contact` on polygons, rules in data.
7. Fir bake and certificate.
8. Trees cut over at once; A/B/B/A closes the forest cost.
9. Carriers unified; tree wire deleted.
10. Bark, inner faces, branch cards; capture for your eye.
11. Planks as cells.
12. Swap on leave, one-hull groups, pricing.

## Owner rulings, 2026-11-03

1. Baked cuts accepted. Open the stiffness solve only if a wrong hold shows in play.
2. "no editor yet." The bake is a command; a node graph could be added above the table later.
3. "align trees, make sure to clean up the exceptions previously made for them." Trees lose the hit-point number. The path must delete, not wrap: `segment_hp` and `hp_per_m2` (second pool), `fell_on_pool_emptied`, `load_trunk`'s missing strike, `TrunkLoads`, `structure_of` (a table built in code), the 15 equal slices, `TreeState::Down` (second wire) with `Fall` and `Swap`, `moving_near`'s trunk exclusion, one wood strength at every angle. Each is checked against the code by the lane that removes it.
4. Open: green wood against seasoned wood as two rows; roofs change; metal stays rigid; eye on the first broken fir.
5. "I don't think it's correct to model moisture, we need a proxy." No moisture variable: a row holds a bond strength, nothing more. Owner asks whether a material needs only its fracture pattern and internal bond strength; proxy pass on the row proposed (15 physical numbers today, MEASURED by field name).
6. "queue the pass. find the balance between nature and proxy, where we don't lose visible granularity but don't waste time modeling niche details." Runs before the first grain lane; two drafts from `material-proxy-brief.md`.
7. Roofs change: accepted, read from the owner's rule against pinning behaviour to what it was before. Open: metal, eye.
