# Walls break in game: two-peer captures (#188)

Subject: the game's environment destruction, game state only.

## How

`capture.sh` (under the machine lock): a local headless server fires the wall cue
(`OVERMATCH_CAPTURE_WALL`, `src/dev_capture/wall.rs`) at server second 90, and three hidden, muted
capture clients connect to it. Peers **a** and **b** hold the outside pose the server logs for the
wall (7 m out from the pier's outer face); **c** holds the inside pose (a metre past the house's
middle, looking at the same pier). Frames at client seconds 25 (before) and 120 and 135 (after).
Dev profile, `--features dev_ui,bitprobe`, the lane's final source. The wall is chosen by class: the
house nearest the map's centre (`house_0036`), its largest breakable pier (`west/pier_0`).

Cases: `88mm_AP`; `88mm_HE` in fuze mode 0 (instant); `x5`, a bare 5 kg charge on the pier's outer
face 0.3 m above its foot (energy 5 × 4.184 MJ, the TNT-equivalent ASSUMED in `materials.ron`).

## The peers agree

Pixels differing by more than 24/255 between peer a and peer b at the same client second (MEASURED,
1600 × 900 frames):

| case | 25 s | 120 s | 135 s |
|---|---|---|---|
| AP | 0 | 0 | 0 |
| HE | 0 | 0 | 1 |
| 5 kg | 0 | 5 | 0 |

At 120 s the 5 kg case still had a piece settling (21 px change from 120 s to 135 s on peer a); by
135 s the two peers are pixel-equal. Before against after on each peer: AP 216 px (the cracks), HE
11,700 px, 5 kg 144,670 px.

Stills: `<case>-out-before.jpg`, `<case>-out-after-peer-a.jpg`, `<case>-out-after-peer-b.jpg`,
`<case>-in-before.jpg`, `<case>-in-after.jpg` (after = 135 s).

## What the laws did (MEASURED, the server's `buildings` debug lines)

| case | energy into house_0036 | chunks | dust | cracks | structural sections (demand / capacity, N·s·m) | fell |
|---|---|---|---|---|---|---|
| AP | 431,624 J (2 lines, 55 spall stops) | 55 | 0 | 13 | none | 0 |
| HE | 122,089,952 J (1 line, 182 fragment stops, 2049 blast rays) | 56 | 1 | 386 | none | 39 chunks in 10 groups, each at 0 m/s |
| 5 kg | 9,510,022 J (1862 of 4096 blast rays) | 103 | 2 | 346 | 8.99 / 0.651, 2743.6 / 1233.9, 1607.4 / 381.3 | 843 chunks in 5 groups |

The 5 kg groups: 691 chunks (7,967 kg) at 0.173 m/s, 95 (1,090 kg) at 0.713 m/s, 51 (588 kg) at
rest, and 4 (51 kg) and 2 (19 kg) thrown at 19.8 and 5.1 m/s.

## Against #188's acceptance

- **The same break on both clients:** yes, to the pixel once settled (table above).
- **One AP shot leaves a hole and topples nothing:** it topples nothing (no section reaches its
  capacity: a house pier is held by its corners and lintels, where the sandbox's lone wall failed at
  406 / 314) but **leaves no hole**. The shot deposits 431.6 kJ over 55 chunks, and a 0.0146 m³
  chunk's pool is 1.98 MJ (DERIVED: 135.5 MJ/m³ × 0.014625 m³), so no pool empties; the cracks are
  the visible result. This is the design review's finding 4 (sub-chunk holes), #189; nothing tuned.
- **A large charge at a wall's base brings a section down:** the 5 kg charge cracks three sections,
  one a plane that runs round the whole house at grade, so the upper house (all four walls and both
  gables, 691 chunks) is freed as one body; with 0.173 m/s it slides by eye some 0.2–0.3 m on its
  foundation and stands. The roof is wood, which states no seam and never breaks, so it stays where
  it was and the moved gables show through it. Nothing falls over. The design review's findings 3
  and 6 (sections and collapse), #189; nothing tuned.
- **A round meets a fallen piece as brick** and **a chunk crushed or loose never blocks as the
  intact element:** tests in `src/buildings/breakage/tests.rs`.

## Found by the captures, fixed in the lane

- **The structural check's search** tried every section with a breadth-first walk per push: one HE
  burst beside a house held the server's laws for 1.15 s by the log's timestamps (MEASURED under
  parallel builds; indicative, not a frame-time measurement). It now searches only the sections that
  cut a push's shortest path to the ground, the same answers (test
  `the_pruned_structural_check_is_the_full_search`); the same burst's lines span 42 ms.
- **A push on a chunk already loose** made every section in the house hold it, with a demand over
  capacity that does not fall with distance: it now bends no section, as the check's own doc said.
- **The foundation below grade** fell as a body inside the terrain when the anchor plane cracked,
  and the solver threw the house about 3 m by eye. The soil now holds every chunk wholly below
  grade (`Chunk::buried`, test `a_buried_chunk_is_held_by_the_soil`).

## Open, for #189

- A burst's blast rays have no attenuation: one HE burst swapped elements of seven other houses
  with a single 359 J ray each (5 kg: 5,107 J each), spawning their chunk colliders.
- The HE's 122 MJ is almost all the fragment field's strength (182 stops); the blast carries
  1.47 MJ (DERIVED in `src/guns.rs`'s test: core energy less the casing's). Review finding 1.
- The share of a round's lost momentum the crushed brick carries away stays open: the struck chunks
  keep all of it.
