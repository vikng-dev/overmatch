# The fewest numbers a breakable material needs: merged design

Subject: the game's environment destruction and damage model, game state only. Merged from two independent drafts beside this file, `material-proxy-claude.md` (with a throwaway model, `material-proxy-model/`) and `material-proxy-codex.md`. Nothing is built. Labels: MEASURED, DERIVED, MODEL (never game code). Fits `generic-fracture-merged.md`.

## The sentence

**A material is its resistance, density, friction, bond strength and bond work, plus an along-grain strength where it has a grain; how it breaks is a rule in the asset's data, and everything else is one global value.**

## The owner's question: is it just pattern and bond strength?

Close. For breaking, yes, with one more number: **bond work,** how brittle or tough the bond is. Both drafts found this independently. Without it plank and glass read alike: an 88 mm HE burst takes a 1 m panel at 15.5 and 18.3 m, against 5.7 and 23 m with it (MODEL). Beyond breaking, a row also needs weight, resistance to a round, and friction, which hulls and tracks share.

## What both drafts agree on

1. Bond strength and bond work stay separate.
2. Delete `toughness` and `compressive_dif_strain_rate`: nothing but the parser reads them (grep, `src/substances.rs` only).
3. The debris half angle goes global: 26.2° in all 12 hard rows (MEASURED). Armour reads it too, so it moves to the spall cone's data.
4. `hp_per_m2` goes, by ruling. No moisture variable. Green and seasoned wood are two rows holding numbers.
5. Metal stays rigid: it comes off whole at its bolts.
6. Balance lives in game units, never in strength or density.

## Where they differ, and my pick

| Question | Claude draft | Codex draft | Pick |
|---|---|---|---|
| **Row size** | 5, plus `along` | Up to 12, plus 2 along | **5 plus `along`** |
| **Shear** | Derived from strength and work by one ratio each | Kept: "reject that cut" | **Derived,** with the test below as the check |
| **Crushing and rear-face numbers** | One global each; `Law 2` deleted if unseen | Kept, folded into pairs | **Global,** deleted if unseen |
| **Debris share** | Not raised | New `Y`, since `resistance` also sets emission through the exit ratio | **No `Y` yet:** a derivation until two materials need different answers. Named as a known second job for `resistance` |
| **Species** | No row unless a cut shows them | Keep spruce, pine, birch: 36/46/63 MPa | **No row unless shown:** one shot moved, or 25 % in reach or piece speed |
| **Along-grain work** | One length: `work × along / strength`, 5,544 J/m² | Keep 22,340 as an analogue, ASSUMED either way | **One length:** the ASSUMED value gives 40 shots or more |

**Why the smaller row despite Codex's objection.** Codex measured sensitivity at a fixed energy, where each number does move an output. Claude's model follows it to the picture: what shows first. The worst case is shear on plank. HE plug reach goes .70 → 1.82 m, but a panel's 5.7 m reach acts first (MODEL). Real plank shear is about 15× what the ratio derives (8.9 against .57 MPa, DERIVED), so this cut gives up truth for no picture. If the test finds a picture, shear returns as a fact.

## The row

`bond: None` means the rule is `Whole`, attachments decide, and the asset compiler refuses a cutting rule without a bond, so that state can't be authored.

| Material | Resist | kg/m³ | Friction | Strength | Work J/m² | Along | Rule |
|---|---|---|---|---|---|---|---|
| Brick | 36.5 | 1845 | .65 | .69 MPa | 10 | | shipped faces |
| Plank | 40 | 448 | .5 | 2.4 MPa | 370 | 74 MPa | planks |
| Trunk | 40 | 850 | .5 | 2.4 MPa | 370 | 36 MPa | grain |
| Concrete | 38 | 2400 | .65 | 2.9 MPa | 139 | | Voronoi |
| Cut stone | 60 | 2700 | .6 | .69 MPa | 10 | | blocks |
| Glass | 100 | 2500 | .4 | 41 MPa | 8 | | shards |
| Sandbag | 34 | 1600 | .8 | None | | | whole, resting |
| Sheet steel | 800 | 7900 | .5 | None | | | whole, bolted |

The last five are MODEL rows and prove the shape, nothing more. Plank's 74 MPa is recalled and unverified: the grain lane reads it. No two read alike in the model: an armour-piercing composite round (APCR) pops brick and not concrete; machine-gun fire stops in .23 m of stone and passes concrete; glass pops under machine-gun fire and plank doesn't.

The seven globals are all `StandIn` and take brick's ratios, because brick's row has the sourced numbers (TN 3A, MC2010): 2.357, .18, 2,159 m/s, 3.1, .239, 2.84, 26.2°. That is a choice of source. Brick not moving is a side effect of it and earns nothing in this design.

## What moves

Results, not requirements. All MODEL unless marked, none measured in the game.

- **House walls:** nothing moves, since the globals are brick's. About 10 88 mm AP shots or 4 88 mm HE at window height to sever the east wall (rough; no count exists at the current law).
- **Fir:** today 2 88 mm AP shots at 1 m, APCR 7, machine gun 187 (MEASURED), because of the hit-point number. Under the laws it depends on the cut: stretched Voronoi at the fracture design's 4.04 gives 39, at 2 gives 12, at 1 gives 3; planes every .7 m give 4.5, every .35 m give 3.
- **Why tougher:** a round's along-grain crack reaches .12 m (DERIVED), so long pieces stay whole. A solid shot drills through a trunk and does not sever it.
- **APCR and machine guns no longer bring a fir down.**
- **Correction:** the fracture design's "2–3 shots" rested on an ASSUMED 760 kJ. MEASURED speeds give 467 kJ (DERIVED).

If the owner wants a number of shots, that is a balance decision and it lives in the cut's piece length, never in strength.

## The test, and what kills it

P0, a hidden bay test beside fracture lane 0: print shots to sever the east wall; run the fir grid over cut and piece length; take half-and-double stills of shear, crushing, rear-face and half angle at a held energy (Codex's check). The owner judges by eye.

An input whose two pictures match retires. The row is wrong if:
- a plank flips between popping and not, or 25 % of reach shows unmasked;
- a crushed dish or rear-face scab shows outside a popped chunk;
- plank and glass, or concrete and stone, look alike in a capture;
- no cut gives a wood read and a shot count the owner accepts.

## Lanes, each under an hour

- P1 delete the unread numbers. P2 half angle global. P3 crushing and rear-face globals. P4 shear derived, `Bond` type. All between fracture lanes 1 and 2. P3 and P4 claim no change for brick, so each compares before and after once in its report and leaves no test.
- P5, in fracture lane 4: `along`, Plank and Trunk rows. Species numbers go in lane 8 with `hp_per_m2`.
- P6, in fracture lane 6: a concrete `Fixture` row proves that a material adds data alone.

## Owner calls, with my picks

1. The row of five plus `along`: **yes.**
2. Two wood rows, species only if a cut shows them: **yes.**
3. Stretch stops deriving from strengths, and a shot count is optional: **no target until the first bake;** your eye picks between planes at .7 m and Voronoi at 2.
4. Metal stays rigid: **yes.**
5. Delete the rear-face law if the test shows it unseen: **yes, after the test.**

## Owner's challenge: does along-grain strength double-count the elongated pattern?

Largely yes, for rounds. `material-proxy-model/double_count.py`, MODEL, median shots of 88 mm AP to part a fir, cap 40, line hits with no weight or bending check:

| Stretch | 1 | 2 | 4.04 |
|---|---|---|---|
| Along 36 MPa | 3 | 12 | 39 |
| No grain, 2.4 MPa everywhere | 2 | 12 | 33 |

Stretch carries the count and the strength adds little: both express one cause. Not covered: a trunk's section under its own weight, a tank's shove or a burst, where the number still acts (bending capacity 15×, DERIVED). A cut with no cross-grain faces, such as whole planks, never reads it. Proposal: no `along` by default; the first bake compares a fir with and without it, under weight, a shove and a burst.

## Superseded

The section on along-grain strength is answered by `break-survival-merged.md`: section strength, read once.
