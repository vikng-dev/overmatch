# How spall and bursts meet structures: one direction

Subject: the game's environment destruction and damage model, game state only. Merged from two independent drafts beside this file, `structures-fields-claude.md` (model: `structures-fields-model/`) and `structures-fields-codex.md`. Nothing is built, and nothing gets built before the first playtest. Sources are as the drafts read them; I did not re-read them.

## The sentence

**Read each closed-form field directly on a structure's exposed faces instead of walking rays; let a burst's pieces fly on with the round's motion and cut lines into the solid; decide survival on what's left of the section, from one record of everything that happened to it. Tanks keep the sampler.**

## Your questions

**Is the math already in place of the rays?** Half. The field laws are already closed forms. Where they meet geometry is still rays: 64 for a cone, 256 for a ring, 4,096 for a blast sphere. That's the part this replaces, for structures only.

**Is the plug right for APHE but not a thin HE case?** Your read holds, and it needs no HE rule. One law, one construction fact: the cavity. A narrow cavity leaves a heavy nose (APHE); a wide one leaves little. Today's HE keeps 4.39 kg solid only because of an assumed 65 mm cavity; an also assumed 53 mm leaves .4 kg (DERIVED). German 88 mm HE static tests recovered 1,488 and 1,543 fragments, with incomplete recovery (MEASURED, cited by a draft). No source gives the true cavity or the largest piece.

**What's missing for explosions?** Three things:
1. **Pieces stop at the first face.** A piece should cut a line with a depth, like a round, only smaller.
2. **Flight is dropped.** The part of a burst's flight energy that goes missing is 1.24 MJ on today's HE (MODEL), not the 2.98 MJ I quoted earlier, because the nose keeps its share. With no plug, the pieces should carry all of it. The surprise is that this motion is the blow: at contact on a .40 m trunk, air blast gives 120 of 3,639 N·s (MODEL).
3. **Bursts inside a solid and root plates.** A burst inside a solid has no cavity energy, and a root plate has strength but no work.

**Does an 88 mm HE fell a tree?** Nobody knows, and the sources hold no single-round record. It turns on one number, how much work a stem absorbs under a millisecond pulse:

| Work assumed (spruce, .45 m at chest height; a draft's citation) | One HE at contact fells up to |
|---|---|
| 50 kJ/m², small samples | .84 m (MODEL): every shipped fir |
| Same, with an ASSUMED .07 rad at the plate | .54 m (MODEL) |
| Whole-stem bending, 184 kJ | .35 m (MODEL) |

Shipped firs are .30–.65 m. Solid AP at .20, .40, .61 m takes 1, 2, 6 shots; HE at 50 kJ/m² takes 1, 1, 1, and at the stem figure 1, 2, none in 20 (all MODEL). Pick by eye at the first bake; don't pretend a source settles it.

## Where the two drafts agree, independently

- Rays are wrong for structures. Past 4.37 m (DERIVED) a ray outsizes a chunk face, so a house burst loses impulse: .55 of faces missed at 5.5 m (MODEL). That's worse than the topple reversal we measured.
- Reading the field on exposed faces is the pick: a face can't fall between rays, and no seed decides a result.
- Exact visibility and raster stay refused on cost (16.8 ms server, DERIVED; 14.4–70.8 ms M4, MEASURED).
- Issue 204 narrows to tank boxes, where small targets need real visibility. The sampler stays there. One field law, two geometries.
- The section read stays, with changed inputs. Its pick that a burst does less than a solid shot is overturned.
- One fragment law for crew stations and structures, each reading it its own way.
- Root plate gets work, not just strength. No moisture, no per-object rule, no HE branch.

## Where they differ, and my pick

| Question | Claude | Codex | Pick |
|---|---|---|---|
| Fragments | One mean piece mass, read as an expected value | Fragment classes with a mass and speed spread | **One mass, expected value.** Doubling it moves section left only .88, .79, .77 at 5, 20, 80 g (MODEL). The tests give about 5.6 g mean (DERIVED, 8.4 kg steel over 1,500 pieces), but big pieces do the cutting, so 5–20 g stays a stated range |
| Burst inside a solid | Enclosed share goes on the existing cavity law | Needs a separate, validated response table | **Cavity law, marked unsourced.** No data exists to validate a table. Simplest model until play or a source says otherwise |
| Contact | Pieces plus a local blast dish | Same gap, named | **Pieces plus dish,** gap stated |
| Cost bar, server | Quarter tick, 3.9 ms, per house burst, at .4 bursts/s | .5 ms per burst, at 1.9 bursts/s | **Quarter tick and cheaper than today's rays,** priced beside them. Both rates are assumed, and today's tick is already over budget (DERIVED) |
| Service load in the section read | Keep | Refuse any pressure picked to hit a shot count | **Keep, derived from the structure's weight and height.** Tuning it to a shot count is refused |

## What it does to the code

**Deletes:** `BLAST_RAYS`, `intercepted`, the grazing floor, the stop tolerance, seeds on structure fields. **Adds:** a table of exposed faces built from data at spawn, simple shading by segments at fixed patches on each face, piece mass and cavity as round data, a removal record. New rounds and materials are then data only. Estimated 1.8 ms server per house burst (DERIVED, on assumed per-read costs).

**Data row, and what a player sees if it moves:** piece mass (pockmarks and depth); section work (shots to fell); plate rotation (ramming only, a shell reads .025 of it, MODEL); cavity (plug or none). Everything else already exists.

## It's wrong if

- Stems absorb far more than the small-sample figure under a millisecond pulse.
- Pieces mostly pass through or shatter, which lowers the blow.
- On/off shading on a patch reads worse in play than rays did.
- The face read misses its cost bar on a real bake.

## Also found

- **Small-arms line against brick:** 7.9 mm passes a .25 m wall on the first round (MODEL); the US manual says 45 rounds through 9 in double brick, and "most rounds only pass through a previously eroded hole". Likely a separate defect in how the bullet's strength is read. Its own task.
- **Delay fuze:** clears one trunk and bursts 73 m on at 731 m/s (DERIVED). The German manual's use against timber and houses needs a round that stops inside. Open.

## Not tested, not sourced

Real bakes, falls, oblique lines, real shading, HEAT, the look, real tick cost. No source for: cavity drawings, largest piece, case break-up, fuze function time, a burst partly inside a solid, fragments in brick, a single HE on a known tree. One draft's search on case break-up was refused and it did not retry.

## The test before any build, about an hour

Hidden, real east wall and fir bake, today's path beside the face read. Bars, from laws or stated decisions: delivered impulse equals the face integral within 1 %; no topple reversal past the wall's diagonal; turning anything invisible changes no bit; a piece cloud integrates to its energy over crush; momentum equals mass times carried speed; a contact burst punches and never topples; quarter tick. Printed, no bar: shots, stills.

## The lanes, if it wins and the test passes

About eleven, each under an hour: pieces carry flight; piece mass and cavity data; face table; blast on faces, rays deleted; shading; clouds on faces; expected opening on the section; blow on section work; plate work; bursts inside a solid; pricing. Crew law after.

## Your one call

**Is this the direction for destruction after the first playtest?** My pick: **yes,** with the cheap test first. Section work stays open until you see the first bake.

## Owner's ruling, 2026-11-03

"okay, provisional. we'll see it and decide more later." The direction stands provisionally. Nothing is built before the first playtest; section work and anything else waits for what the first bake and play show.
