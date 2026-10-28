# Energy dose: rounds against the shipped fir (issue 113)

This note is about the game's damage model for trees. A tree is one HP pool, and everything here is
game state.

- Code: lane `energy-dose` on main `d2ce280`, with issue 113's law in place: a hard pool's line dose
  is the kinetic energy the round loses crossing it, ½ m (v_in² − v_out²), all of ½ m v_in² when it
  comes to rest there; one rate, 3000 J per HP point (`assets/ballistics/energy_dose.ron`); every
  tree one wood (`Wood`, factor 40), its pool 600 HP per metre of breast-height diameter
  (`assets/trees/wood.ron`).
- The experiment is the throwaway `energy-dose-measure.rs` beside this note, an ignored headless
  test. It never merges. To re-run it, copy it to `src/ballistics/tree_shot_measure.rs`, declare
  `#[cfg(test)] mod tree_shot_measure;` in `src/ballistics.rs`, and run
  `scripts/serial-cargo test --lib --features dev_ui,bitprobe tree_shot_measure -- --ignored --nocapture`.
- Re-run on the combined state rebased onto main `c4fce0c`, including the per-depth energy
  allocation fix, person-sized crew boxes, and ring-only bursts: every table cell below is
  unchanged (MEASURED). Raw output: `energy-dose-merged-measured.txt`.
- Every number in a table is MEASURED (macOS arm64, `edit` profile). Prose numbers are MEASURED
  unless labelled DERIVED.

## Method

The method of `.agents/scratch/destruction-2026-10-26/shot-dose-measurement.md`, cut down:

- **The world.** A headless app with no window: `MinimalPlugins`, `TransformPlugin`,
  `AssetPlugin`, avian `PhysicsPlugins`, the ballistics `sim_plugin` on the game's 15.625 ms fixed
  tick, the `trees::fell_on_pool_emptied` observer, and a flat 800 m floor.
- **The tree.** One real fir_1 (norway spruce, 15 frustum segments, height 27.02 m, dbh 0.4000 m)
  spawned through `trees::spawn` at scale 1, resolved from `fir_1.tree.ron`, `species.ron`,
  `wood.ron` and `materials.ron`: a 240 HP pool of `Wood`.
- **The rounds.** `FireShell::of_round` on rounds fired from their guns at the guns' muzzle speeds:
  `88mm_AP` (the solid Pzgr. 39), `88mm_APHE_ring` and `88mm_APCR` from `kwk36_l56`, and
  `7.9mm_AP` from `mg34`. Two fixture 75 mm APCBC bodies stand in for the "75 mm AP round" of the
  calibration aim, since no 75 mm round ships: 6.8 kg, 75 mm, the APCBC type's nose (the solid
  88's record with its body swapped), at 790 and 925 m/s. Both are fixture data, not a sourced
  round.
- **The shot.** Horizontal along −Z from 50 m, aimed up by ½ g t² to meet the trunk at height `h`,
  offset sideways by a fraction of the local radius r(h).
- **Repetition.** The same shot on the same tree until its pool is emptied, or 5 shots in a row
  remove nothing (cap 2000). A row reads the first shot: its outcome in the trunk, the disc-mean
  chord, the readout cost (chord × factor), the energy the round lost in the trunk (the dose), the
  speeds in and out, and the HP it removed; then the shots to empty.

## Results

Centred and half-radius shots at three heights. Each cell reads *HP of shot 1 · shots to empty*;
`never` is a cell where the shot removes 0 HP, so the pool is never emptied.

| round | h = 1 m, centred | 1 m, 0.5 r | 10 m, centred | 10 m, 0.5 r | 20 m, centred | 20 m, 0.5 r |
|---|---|---|---|---|---|---|
| 88mm_AP | 156 · 2 | 129 · 2 | 110 · 3 | 90 · 3 | 50 · 5 | 37 · 7 |
| 88mm_APHE_ring | 156 · 2 | 129 · 2 | 110 · 3 | 90 · 3 | 50 · 5 | 37 · 7 |
| 88mm_APCR | 29 · 9 | 26 · 10 | 21 · 12 | 17 · 15 | 10 · 24 | 8 · 30 |
| 7.9mm_AP | 1 · 240 | 1 · 240 | 1 · 240 | 1 · 240 | 0 · never | 0 · never |
| fixture 75 mm APCBC, 790 m/s | 115 · 3 | 95 · 3 | 81 · 3 | 67 · 4 | 38 · 7 | 28 · 9 |
| fixture 75 mm APCBC, 925 m/s | 126 · 2 | 105 · 3 | 89 · 3 | 74 · 4 | 42 · 6 | 31 · 8 |

The first shot of the centred base cell, per round:

| round | outcome | chord m | cost ref-mm | energy lost J | v in m/s | v out m/s | HP |
|---|---|---|---|---|---|---|---|
| 88mm_AP | through | 0.3882 | 15.53 | 466 710 | 763.8 | 701.3 | 156 |
| 88mm_APCR | through | 0.3974 | 15.90 | 87 894 | 915.2 | 864.0 | 29 |
| 7.9mm_AP | rest | 0.3184 | 12.74 | 2 872 | 697.8 | 0 | 1 |
| fixture 75 mm, 790 m/s | through | 0.3905 | 15.62 | 344 047 | 779.5 | 711.7 | 115 |
| fixture 75 mm, 925 m/s | through | 0.3905 | 15.62 | 379 058 | 913.5 | 850.3 | 126 |

## Against the aims

- **The 88 out-doses the machine gun per shot on the shipped fir.** Centred at the base, the
  88mm_AP deals 156 HP (466 710 J lost) and the machine-gun round 1 HP (it comes to rest inside
  and deals all it had, 2 872 J). **Shots to empty the mature fir: 2 for the 88, 240 for the
  machine gun.** Under the reference-mm law it replaced the 88 dealt 155 HP and took 8 shots on
  the 1200 HP spruce, while a stopped machine-gun round dealt its whole capability.
- **A mature fir survives one 75 mm AP round through the trunk.** The fixture 75 mm bodies deal
  115 HP (790 m/s) and 126 HP (925 m/s) centred at the base against the 240 HP pool; the second
  shot empties it at 925 m/s, the third at 790 m/s.

## Readings

- **The rate holds the main gun where it was.** 156 HP for the 88 at the base against 155 under the
  old law: the rate was set from the reference round at its muzzle speed (DERIVED ≈ 3090 J per
  0.1 mm of reference steel, rounded to 3000), and the round meets the trunk at 763.8 m/s.
- **The machine gun's per-shot rounding.** A box's dose from one shot is rounded once to whole
  points (design doc §6). The machine-gun round's 2 872 J at the base is 0.96 HP and rounds to 1;
  at 20 m it perforates the thinner trunk and loses 1 409 J, 0.47 HP, which rounds to 0. So at
  20 m the belt never fells the fir, at any rate of fire.
- **The sub-calibre round deals little to wood.** The 88mm_APCR meets the trunk as its core, a
  sixth of the solid round's hole area (DERIVED from the round file), and loses 87 894 J where
  the solid round loses 466 710 J: 29 HP and 9 shots at the base.
- **Higher up the trunk takes more shots.** The pool is the whole tree's, set by its breast-height
  diameter, while the chord shrinks with the taper: 2 shots at 1 m, 3 at 10 m, 5 at 20 m for the
  88. The plain pool is the decided law; the per-segment remaining-diameter law is held in reserve
  (epic #103).
- **The burst round is the solid round on a trunk.** 88mm_APHE_ring matches 88mm_AP in every
  cell: the trunk's largest cost, 15.53 reference-mm at the base, is short of the 35 its fuze
  arms on (`88mm_APHE_ring.round.ron`), so it never bursts.
