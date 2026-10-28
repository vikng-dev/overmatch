# Per-segment pools: shots to break the shipped fir (issue 149)

This note is about the game's damage model for trees. A tree is HP pools and a fall, and
everything here is game state.

- Code: this lane on main `9307a20`, with issue 149's law in place. Each trunk segment is its own
  volume with its own pool: `hp_per_m2` × π r², for r the mean of the segment's end radii at the
  instance's scale (its radius at mid-height). The rate is 1500 HP per square metre
  (`assets/trees/wood.ron`, TUNED). The dose law is unchanged: the energy the round loses in a
  segment at 3000 J per HP point (`assets/ballistics/energy_dose.ron`). The segment whose pool
  empties is the cut.
- The experiment is the throwaway `segment-pools-measure.rs` beside this note, an ignored headless
  test that never merges. It is the 2026-10-28 harness
  (`../damage-model-2026-10-28/energy-dose-measure.rs`), adapted to read the struck segment's tally
  and to count shots until the tree's state leaves `Standing`. To re-run it, copy it to
  `src/ballistics/tree_shot_measure.rs`, declare `#[cfg(test)] mod tree_shot_measure;` in
  `src/ballistics.rs`, and run
  `scripts/serial-cargo test --lib --features dev_ui,bitprobe tree_shot_measure -- --ignored --nocapture`.
  Raw output: `segment-pools-measured.txt`.
- Every number in a table is MEASURED (macOS arm64, `edit` profile) unless labelled DERIVED. A
  pool's size is DERIVED: the law applied to the asset's radii. The harness printed every one.

## Method

- **The world** is the 2026-10-28 one: a headless app with no window, avian, the ballistics
  `sim_plugin` on the game's 15.625 ms fixed tick, the `trees::fell_on_pool_emptied` observer and
  a flat floor.
- **The trees** are one real fir_1 (norway spruce, 15 segments, 27.02 m, dbh 0.40 m), spawned
  through `trees::spawn` from the shipped `fir_1.tree.ron`, `species.ron`, `wood.ron` and
  `materials.ron` at three instance scales:
  - **mature**: scale 1, dbh 0.40 m;
  - **young**: scale 0.5, dbh 0.20 m, its base segment 0.2025 m across at mid-height;
  - **sapling**: scale 0.25, dbh 0.10 m, its base segment 0.101 m across.

  The young tree and the sapling are the fir scaled down, the same way the map's scatter scales
  an instance.
- **The rounds** are fired with `FireShell::of_round` at their guns' muzzle speeds:
  - `88mm_AP` (the solid Pzgr. 39) and `88mm_APCR`, from `kwk36_l56`;
  - `7.9mm_AP`, from `mg34`;
  - two fixture 75 mm APCBC bodies, standing in for a 75 mm AP round, since none ships. Both are
    6.8 kg and 75 mm with the APCBC type's nose (the solid 88's record with its body swapped), at
    790 and 925 m/s. They are fixture data, not a sourced round.
- **The shot** flies level along −Z from 50 m, aimed up by ½ g t² to meet the trunk at height `h`,
  and offset sideways by 0 or 0.5 of the local radius. The heights:
  - on the mature fir: 1 m (segment 0), 10 m (segment 5) and 20 m (segment 11);
  - on the young tree and the sapling: the middle of the base segment (0.45 m and 0.225 m).
- **Repetition**: the same shot on a fresh tree until the tree is down, or until 5 shots in a row
  remove nothing (cap 2000).

## The struck pools (DERIVED, 1500 HP per square metre)

| tree | segment | across at mid-height | pool HP |
|---|---|---|---|
| mature | 0 (0–1.8 m) | 0.405 m | 193 |
| mature | 5 (9.0–10.8 m) | 0.289 m | 98 |
| mature | 11 (19.8–21.6 m) | 0.130 m | 20 |
| young | 0 (0–0.9 m) | 0.2025 m | 48 |
| sapling | 0 (0–0.45 m) | 0.101 m | 12 |

The whole fir's pools, base to tip: 193, 172, 152, 133, 115, 98, 82, 67, 53, 41, 30, 20, 12, 5
and 2 HP.

## Results

Each cell reads *dose of shot 1, HP · shots to break*. The dose is the energy the round lost in
the struck segment over 3000 J; a pool takes no more than it holds. Every tree broke at the
segment it was shot in.

| round | mature 1 m, centred | 1 m, 0.5 r | 10 m, centred | 10 m, 0.5 r | 20 m, centred | 20 m, 0.5 r | young base, centred | young, 0.5 r | sapling base, centred | sapling, 0.5 r |
|---|---|---|---|---|---|---|---|---|---|---|
| 88mm_AP | 156.0 · **2** | 116.8 · 2 | 109.8 · 1 | 81.4 · 2 | 49.8 · 1 | 36.0 · 1 | 76.0 · **1** | 50.2 · 1 | 29.9 · 1 | 22.3 · 1 |
| 88mm_APCR | 29.3 · 7 | 25.3 · 8 | 20.8 · 5 | 15.7 · 7 | 10.1 · 2 | 7.5 · 3 | 14.6 · 4 | 11.0 · 5 | 7.1 · 2 | 5.2 · 3 |
| 7.9mm_AP | 1.04 · 187 | 1.04 · 187 | 0.91 · 109 | 0.80 · 122 | 0.49 · 41 | 0.43 · 47 | 0.67 · 72 | 0.59 · 81 | 0.36 · **34** | 0.31 · 39 |
| fixture 75 mm, 790 m/s | 115.1 · 2 | 86.5 · 3 | 81.3 · 2 | 60.6 · 2 | 38.3 · 1 | 24.7 · 1 | 56.5 · 1 | 40.6 · 2 | 24.2 · 1 | 17.9 · 1 |
| fixture 75 mm, 925 m/s | 126.9 · 2 | 95.4 · 3 | 89.6 · 2 | 66.8 · 2 | 42.2 · 1 | 27.2 · 1 | 62.3 · 1 | 44.8 · 2 | 26.7 · 1 | 19.8 · 1 |

Shots to empty the mature fir under its one 240 HP pool, before per-segment pools, for the same
cells (`../damage-model-2026-10-28/`):

| round | 1 m, c | 1 m, 0.5 r | 10 m, c | 10 m, 0.5 r | 20 m, c | 20 m, 0.5 r |
|---|---|---|---|---|---|---|
| 88mm_AP | 2 | 2 | 3 | 3 | 5 | 7 |
| 88mm_APCR | 9 | 10 | 12 | 15 | 24 | 30 |
| 7.9mm_AP (pools kept fractions) | 232 | 232 | 268 | 295 | 497 | 553 |

The half-radius cells there predate the wood's small entry bend (`594f2f8`, `9307a20`), which
shortened the 88's chord at 1 m from 0.320 m to 0.289 m. So compare the centred cells only.

## Against the aims

- **Calibration: the 88's solid round breaks a mature fir's base at the second centred shot.**
  It deals 156 HP against the base segment's 193 HP. Half a radius off, it deals 117 HP: still 2
  shots. The fir survives one centred 75 mm round (115–127 HP) and breaks at the second.
- **Thin wood breaks in fewer shots.** The 88 takes 2 centred shots at the fir's base, 1 at
  10 m and 1 at 20 m. Under the one pool it took 2, 3 and 5. The machine gun takes 187 at the
  base, 109 at 10 m and 41 at 20 m, where it took 232, 268 and 497. Every round now breaks a
  segment in fewer shots the thinner it is.
- **The 88 breaks a mature fir's thin top in 1 shot** (20 m: 49.8 HP against 20). **It breaks a
  0.2 m young tree in 1** (76.0 HP against 48). Both match the issue's DERIVED expectation.
- **The machine gun cuts a 10 cm sapling in 34 centred hits, not about 15.** The issue's
  expectation was DERIVED. Each hit deals 0.36 HP (1 075 J, perforating) against the base
  segment's 12 HP. One rate serves both aims, so they pull against each other: 15 hits would
  need a 5 HP sapling pool, about 640 HP per square metre. At that rate the fir's base would hold
  82 HP, so the 88 would break it in one shot (DERIVED).

## Readings

- **The rate is the only knob, and it moves every tree together.** For one round, shots go
  roughly with the diameter of the struck segment: its pool goes with d², the chord with d. So
  the sapling-to-base ratio is set by geometry and the round, 34 to 187 for the machine gun and
  1 to 2 for the 88. Only the rate sets where the whole ladder sits.
- **The tip is nearly free.** The top two segments hold 5 and 2 HP. A single machine-gun hit
  removes 0.06–0.18 HP there (`../damage-model-2026-10-28/fractional-pools-measured.txt`, 25 m),
  so it still takes tens of hits (DERIVED). Any main-gun round breaks them at once.
- **The sub-calibre round stays weak on wood.** The APCR's core loses a fifth of the solid round's
  energy, so it takes 7 shots at the base and 2 at the top.
- **A machine-gun round that stops in the trunk deals all it had.** At the fir's base, centred
  and half a radius off, it comes to rest 0.34 m in and deals its whole 3 111 J. That is why both
  cells read 187.
