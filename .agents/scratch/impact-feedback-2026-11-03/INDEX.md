# Impact stills: today against the design read

The game's impact presentation, the view only. No law code landed: the design arm is the model's numbers typed by hand into throwaway copies of `assets/vfx/presentation.ron` (`ron/`), one per case. Every picture is an observation for the owner's eye. Both arms sit on the fix "vfx: powder never draws the dust sheet's clod cell".

Each composite: top row today, bottom row design, columns the times after contact. Each panel holds the whole scene with a yellow box over its zoomed inset. The +0.02 s column appears only where a flash exists. A parked Tiger gives the scale, 18 m beside a ground strike.

## Look in this order

`composites/<case>.<framing>.jpg`. Framings: `g500`, `t500`, `g1000`.

1. `he88_dirt` then `he88_dirt_thin`: 88 mm HE burst on dirt, column opacity 1 then 0.6. The largest difference.
2. `ap88_gravel`: 88 mm AP on gravel. The plainest column.
3. `ap88_mud` then `ap88_mud_h217`: 88 mm AP on mud, column 7.7 m then 2.17 m.
4. `ap88_steel` then `skid_steel`: 88 mm AP on steel, then a skid.
5. `r79_steel`, `r79_gravel`, `r79_mud`, `r79_mud_loft3`: rifle on steel, gravel, mud, mud with thrown powder ×3.

Read `g500` for all of them first, then `t500`, then `g1000`. Not rendered, for lack of a turn on the machine lock: `skid_steel.g1000`, `he88_dirt.g1000`, `he88_dirt_thin.g1000`.

## My read, per case, at 500 m through the sight

| Case | What a player can now tell | What they stop seeing, or where it is worse |
|---|---|---|
| 88 HE on dirt | A burst, at once: a dark mass wider than the tank stands up where today shows a speck | Gone by +2.5 s, with the smoke and the haze too faint to hold the spot. Opacity 0.6 reads as a smudge, worse than 1 |
| 88 AP on gravel | Where the round landed, for about 2 s: a dark cone taller than the tank, where today shows a speck | Nothing is lost against today. Nothing marks the spot after +2.5 s |
| 88 AP on mud, 7.7 m | A landing on mud at all: today shows nothing on the ground | Only the base reads: the top stands against the dark sky, dark on dark |
| 88 AP on mud, 2.17 m | Barely anything: a smudge at the tank's foot | Worse than 7.7 m: it does not answer "where did it land" |
| 88 AP on steel | Very little: a brighter flash at +0.02 s, one or two sparks at +0.15 s and +0.5 s | The spall puff does not read in either arm |
| 88 AP skid | The flash stretches along the hull at +0.02 s, against today's round one: a glance reads as a glance for one frame | Nothing after that frame; no fan is visible at this range |
| Rifle on steel | No difference I can see | |
| Rifle on gravel | One speck at +0.02 s where today has none | No column in either arm |
| Rifle on mud, and ×3 | Nothing, in any arm, even zoomed ×6 | |

At 1,000 m through the sight, only the design's 88 mm ground reads leave a mark, and a small one; today leaves nothing. The ground at the gravel view's 1,000 m bearing is blown out, which washes the column further, and a house near the mud view's 1,000 m firing spot fills part of the frame. Third person at 500 m tells the same story at about half the size. Steel reads there are too small to judge, and the white dot crowds them.

## The listed risks

- **Column as a wall:** not at these sizes. Opacity 1 on dirt is a heavy dark blot, not a wall; the parked tank stays visible beside it. A column between the gun and a tank was not framed, so hiding a tank by the column itself is untested.
- **Rifle on mud invisible:** yes, in both arms, and with thrown powder ×3.
- **Haze hiding a tank:** no. The haze is faint and floats above the ground (see below), so this picture cannot show the worst of it.
- **Soil kinds alike at 500 m:** mostly yes. The kinds differ by how much they throw; by colour, only slightly (mud rustier, gravel olive).
- **Sparks no better than today:** yes. At 500 m they are specks in both arms.

## What makes both arms hard to see

These hold in both arms and are not the design's doing.

- **Nothing draws a sky.** The analytic sky only lights the scene: the game's own views have no `Skybox` or `ClearColor` (MEASURED by grep of `src`), so behind everything is a clear colour of about (45, 46, 50) (MEASURED), near the shaded dust. A column against it disappears. I moved the strikes to ground with terrain behind them for this reason; the map's mud lies low, so its backdrop reaches only 2.7 m above the strike (DERIVED from the height map).
- **The dust is dark and olive against the ground it came from.** The palette shades by the sprite's signal, and the powder cells' signal stays below about 0.5 (MEASURED), so the shade floor dominates. The design leaves colours out, so I left `soil_shade` alone.
- **Powder cells differ in size.** Cell 0 covers about a third of the area of cells 1 and 2 (MEASURED, `tools/signal.py`), and which one a sprite draws is a roll. Today's plume can land on the small one.
- **The dirt site's ground is blown out** at the bearing that gives a backdrop, so its panels are very bright. Same sun in both arms.

## Where a picture differs from the design, and why

Column and powder:

- Heights fall short of the stated ones: about 4.7 m of visible mass on the gravel column at +1 s (MEASURED from pixels, by eye), not the DERIVED 5.8 m, because the sprite is an eroding shape inside a quad. I enlarged the quad to compensate: aspect ×(1/0.45), haze, spall and small-powder sizes ÷0.6, sharpness raised (14 column and small powder, 8 haze and spall). These are my fit to the sheet, not model numbers.
- The column is three stacked quads, not two, to make a large cell likely. Opacity is set for two overlapping, so where three overlap it is above the stated value; the erosion makes it uneven.
- Erosion is `powder_erosion_end` 0.4 in the design arm, to sidestep a separate defect: today's 0.25 sits below the powder cells' signal, so a sprite still shows cover at despawn and pops. Not fixed in the commit, different cause.
- The ring is alpha 0, not removed. `strike.lifted` stays, so typed opacities are divided by the site's lift share (mud 0.391, gravel 0.836, dirt 0.906, DERIVED).
- The 2.17 m arm keeps the 7.7 m arm's opacity 0.532: the drafts state none for it.
- The haze floats: the recipe centres its quad half a quad above the ground, so it hangs higher than a haze lying on the ground would.

Pieces and sparks:

- Clods are the streak sprite drawn as a thick dash, not a clump.
- Hot pieces have no drag: speed is set for a reach of 5.7 m in 0.48 s, not the 31 m/s launch. Terrain sparks have no gravity. Their cooling colour is an approximation in the existing `hot_*` gradient, for 88 mm only; rifle keeps its gradient. Streak width 1.2 and glow 14 are my tastes.
- The rifle read has no pieces, by the existing compact path.

Marks and smoke:

- The scar stands in for a permanent crater: 60 s, outside the prints store. It does not read at 500 m in either arm at this graze.
- **The burst's smoke cannot be shown fairly.** It is a second raised strike on Rubber, near-black at opacity 0.7 (my taste), not filler colour. Under the shared erosion it is gone by +2.5 s where the design holds it for 4.5 s.

Setup:

- **The skid is hand-placed**, not a real flight: a cue with a `Ricochet` direction at the draft's assumed 20° graze. Steel strikes are hand-placed on a parked hull side with the `Struck` outcome, so neither arm has a flame lick.
- **The sight is a held camera** at gun height (2.3 m) with the view's 25° field (`tiger_1.tank.ron`; 4.87 px/m at 500 m, 2.44 at 1,000 m, DERIVED), not the real optic: its writer follows an aim nobody is steering. No reticle or mask.
- **A white dot sits at the exact screen centre** of every frame, in both arms, before the strike too. It is not part of the read. I did not trace its source. Each framing looks 3 m above the strike so it clears the read.
- No site is pure: mud is 0.87 mud, 0.13 dirt; gravel is 0.82 gravel, 0.18 dirt; dirt is 0.51 dirt, 0.47 gravel, 0.02 sand. The map has no pure dirt (MEASURED: painted dirt peaks at 0.62). Soil and sparks are rolled from a fixed seed, and the same seed gives each arm a different sequence.
- +0.15 s is captured at +0.16 s (0.02 s step); the label says +0.15. Frame time was not measured: the lane rules forbid it.

## Sites

Bearing from strike to firing position, degrees from +Z toward +X.

| Ground | x, z | 500 m bearing | 1,000 m bearing | Ground behind the strike (DERIVED) |
|---|---|---|---|---|
| Mud | 665, -675 | 315 | 345 | 2.7 m, 2.0 m |
| Gravel, steel | -15, 375 | 270 | 225 | 11.1 m, 11.7 m |
| Dirt | 55, 390 | 230 | 230 | 9.6 m, 10.8 m |

## Running it again

`ron/` holds the design copies, `plans/` the capture plans (absolute paths into the session's scratchpad), `tools/` the throwaway scripts, and `tools/stills-capture-patch.diff` the capture levers (never landed: look file, parked tank, plate strike, field of view). Apply the patch to a bay, build, then run `gen.py`, `drive.sh` and `composite.py`.

Raw stills are not committed: about 200 MB (MEASURED) against the 60 MB limit. They stay at `/private/tmp/claude-502/-Users-Yan-github-vikng-dev-overmatch/13e20afd-7b0e-4ad2-9b6e-b78f39af0d6c/scratchpad/impact-stills/raw/` for as long as that scratchpad lives.
