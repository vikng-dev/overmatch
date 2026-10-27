# The ring's shape by round: constant, derived or authored (research, 2026-10-27)

Subject: the game's damage model. A burst throws a field (the ring or the ball, playtest fork F7)
whose dose reaches modules and crew stations, each an HP pool; this note is about what the game
computes and draws. Brief: `brief-ring-shape-research.md`. Labels: MEASURED (read from code, data
or a record), DERIVED (arithmetic here, from shipped data unless ILLUSTRATIVE), INFERENCE (a
game-level reading, not a fact). Sources: the repo and the listed notes only. Nothing is built.

## 1. What the law already reads per round

The ring is `FieldLaw::Ring { half_width }` (`src/ballistics/spall_cone.rs:75-77`): uniform over
polar angles `θ_c ∓ h` (`:152-158`) about `θ_c = atan2(1, λ)` (`:186`, `:197-199`), `λ` = carried
speed / `v_s` (`src/ballistics.rs:3752`), its near edge held at the axis.

| quantity | source | per round or global |
|---|---|---|
| `v_s = K (M/C + ½)^−½`, `M` the wall | `src/ballistics/burst.rs:148-160` | per round, DERIVED at load |
| tilt `θ_c = atan2(1, v_c / v_s)` | `spall_cone.rs:186`, `ballistics.rs:3752` | per shot (`v_c`) and per round (`v_s`) |
| strength `S = ½ M v_s² / 1000 · r_E` | `burst.rs:166`, `spall_cone.rs:418-422` | per round; `r_E` global (`assets/ballistics/spall_cone.ron:75`) |
| ring or ball; nose piece's mass | `assets/rounds/88mm_APHE_ring.round.ron:20`; `burst.rs:75`, `:151-156` | per round |
| half-width `h` = 10° | `spall_cone.ron:83-84`, parsed `spall_cone.rs:381-392`, given to every ring `:440-445` | **global**, TUNED "one datum for every ring" |
| uniform profile across the band | `spall_cone.rs:152-158`, `:297-302` | law constant |
| ring directions `N` = 256 | `spall_cone.ron:90` | global |

The shipped 88 mm ring (`88mm_APHE_ring.round.ron:12-34`; body steel 7800 kg/m³,
`assets/materials/materials.ron:129`; core 1650 kg/m³, `K` 2680 m/s, `materials.ron:233-235`):
cavity length `L` 36.33 mm, wall 1.440 kg, wall thickness `t = (d − d_c)/2` 26.15 mm, `M/C` 24.0,
`v_s` 541.5 m/s, 211.1 kJ (DERIVED; agrees with design doc :594-597). Its `d_c` is TUNED, borrowed
from its predecessor (`88mm_APHE_ring.round.ron:33-34`); every geometric number for it inherits that.

Two DERIVED facts bound option (b):
- For the ring, `M/C = (ρ_steel / ρ_c)·((d/d_c)² − 1)`: `L` cancels, wall and core both scaling
  with it (`burst.rs:72-74`); the 88: 4.727 × 5.076 = 24.0. The charge-to-driven-mass ratio *is*
  the diameter ratio, and it already sets the tilt through `v_s`; a width read from it would read
  the tilt's input twice.
- What the round carries and the shape never reads is the cavity's aspect, `L/d_c = 4C/(π ρ_c d_c³)`
  (1.02 for the 88), or the wall's section `t/L` (0.72). `L` enters today only through `M`, as
  strength: a heavier core at fixed `d_c` lengthens the cavity and raises `S`; tilt and width stay.

## 2. The three options

Families at the game level, tilt at s1's carried speed 244.6 m/s (design doc :626). The 88 is
shipped data; the next two are ILLUSTRATIVE ratios (no such round authored; the 88's core row
assumed):
- **Burst-core armour-piercing, the 88:** `L/d_c` 1.02, `t/L` 0.72, `M/C` 24.0, `θ_c` 65.7°.
- **Thin-walled large-charge blast round** (`d_c/d` 0.8, `L/d_c` 2): `t/L` 0.0625, `M/C` 2.66,
  `v_s` 1508 m/s, `θ_c` 80.8°. **A rocket's long thin body** (`d_c/d` 0.9, `L/d_c` 6): `t/L`
  0.009, `M/C` 1.11, `θ_c` 83.4° (DERIVED).
- **Squash-head, shaped-charge, tandem:** the owner's direction gives them other sources, not a
  ring: "a sphere on the inner face with no line", "a line with high penetration and a small cloud"
  (`../../product/in-battle/owner-direction-2026-10-10.md:83-85`); a tandem is two stages in turn
  (INFERENCE). A ring width touches them only where a body's own wall becomes a band.

### (a) Constant shape, today's
- **Data:** none new (`spall_cone.ron:83`). **Visible:** every ring a 20° band; at rest it covers
  sin 10° = 17.4 % of the sphere at 5.76× an even sphere's brightness (design doc :624) for every
  round, so in-band knockout radius goes only as √S. Families differ by tilt and strength only.
- **Doctrine:** passes tank #2 (`AGENTS.md`, "Tanks are data"); one global TUNED datum.
- **Cost:** none to build; tuning one number moves every ring at once.

### (b) Derived from the round's geometry
- **Data:** none new. `Parts::of` derives `L` (`burst.rs:72`); `d`, `d_c` are in `RoundBody` and
  `BurstInputs` (`burst.rs:41-42`). `Burst::derived` (`:120-168`) would store `h` on `Burst`, and
  `throw_burst` (`ballistics.rs:3752`) pass it to `BurstFields::field` (`spall_cone.rs:440-445`).
  No wire schema change: a wire cone already carries its descriptor, the ring's half-width in it
  (`src/damage_cam/wire.rs:754`, `:40-42`). Anchored, one law constant `k` replaces the global.
- **The law, each step INFERENCE:**
  1. Today's tilt reads the wall's pieces as leaving perpendicular to the axis at `v_s`, plus the
     carried speed (`burst.rs:109-111`). A straight wall alone gives a zero-width band, so `h` must
     come from where the wall stops being straight: its ends.
  2. **R1**, "spread from the ends": the ends' share grows as the wall gets short against its
     thickness, `h = atan(k · t/L)`. Long and thin throws a near-flat disc, short and thick fans.
     `law-shape.md:40-41` ("a cylindrical body's static spray is mostly a side belt") reads so.
  3. **R2**, "along the line from the core's centre": the band is the wall as seen from the
     cavity's centre, `h = atan(k · L/d_c)`. A long wall throws *wider*, toward the sphere.
  R1 and R2 disagree in direction for long bodies; nothing in the repo settles them.
- **Visible** (DERIVED; `k` = 1, or anchored so the 88 keeps 10°: R1 `k` 0.245, R2 `k` 0.173):

| round | R1 | R1 anchored | R2 | R2 anchored |
|---|---|---|---|---|
| burst-core AP, the 88 | 35.7° | 10° | 45.5° | 10° |
| thin-walled blast (ILLUSTRATIVE) | 3.6° | 0.9° | 63.4° | 19.1° |
| rocket body (ILLUSTRATIVE) | 0.5° | 0.1° | 80.5° | 46.1° |

  Under R1 a long thin body becomes a near-planar disc, at rest 16× (3.6°) to 65× (0.9°) an even
  sphere's brightness against the 88's 5.76×: at equal strength an in-band knockout radius 1.7–3.4×
  the 10° ring's (`R ∝ √b`, design doc :615-617), and no dose off its plane. Under R2 it tends to
  the even sphere at rest. Squash-head, shaped-charge, tandem: unchanged, not rings.
- **Doctrine:** the closest fit to laws that emerge from facts and to "players learn shape by round
  type" (`decision-2026-10-23.md:110-114`); passes tank #2; no per-round tuning. But the law is
  unsourced, and un-anchored it widens the one shipped ring 3.6–4.6× (§4).
- **Cost:** small build (one function, one field, a test per invariant); the reading needs a
  source; tuning at most one global `k`.

### (c) Authored per round
- **Data:** a width per ring round inside the block's ring variant (`option: Ring(half_width_deg)`,
  `src/rounds.rs:166-174`, `burst.rs:16-23`), so a ball round cannot carry one;
  `ring_half_width_deg` leaves `spall_cone.ron`. Plumbing and wire as (b).
- **Visible:** whatever is authored; nothing ties a width to the body, so rounds of near-equal
  geometry can throw unlike bands and a player cannot read a band from a round.
- **Doctrine:** passes tank #2 and is the most local balance lever; but with no source each width is
  a TUNED guess, as the 88's 10° is (`spall_cone.ron:83-84`). The owner kept a fixed per-round
  opening as the fallback only (`decision-2026-10-23.md:30`).
- **Cost:** small build (as (b), without the law); one guess per new ring round, for good.

## 3. How games represent it

No surveyed game ships a burst shape that differs by round type. War Thunder's burst is "a
simplified pattern without sectors" plus a smaller sphere, its per-round data a delay, an arming
threshold and the core's mass, no shape (`games.md:17`); its 2024 proposal of sectors (head, base,
a case cone), which would have tied the shape to the round's parts, was tested and shelved after a
48.6/51.4 vote (`games.md:52`, `:76-79`). Gunner HEAT PC! rounds carry a per-round
`ForcedSpallAngle` and burst piece count `DetonateSpallCount` (`games.md:20`; names read from mods,
the angle's role in a burst not stated). Armored Warfare's blast round is a spherical radius
(`games.md:27`). World of Tanks names an AP-HE type (`wot-scout-round4.md:40-42`) but resolves it
server-side, out of reach (`:11-16`); its per-shell effect data carries a splash radius and
strength (`wot-scout-round7.md:35-39`, `:46-48`). Shipped practice puts the per-round difference in
a sphere's count, potency or radius; the one shape-per-round proposal found was shelved.

## 4. Recommendation and its trigger

**Recommendation.** Keep (a), one global width: there is one ring round, F7 may remove the ring,
and the two readings that would derive a width disagree in direction, so neither (b) nor (c) has
anything to stand on yet. When the trigger fires, build (b) anchored so the 88 keeps its width
(one global law constant, the per-round difference from the cavity's aspect, never from `M/C`) if
an outside source settles R1 against R2, and otherwise (c) as a width in the round's `Ring` option.

**The trigger, both in order.**
1. **F7 settles with the ring kept.** If the ball wins, the ring round leaves the list
   (`.agents/scratch/playtest-forks/README.md:292-295`) and the question closes: a ball has no width.
2. **A second ring round is authored whose band anchored (b) moves visibly**, i.e. by at least one
   direction spacing, 5.0° on the shipped ring at `N` 256 (MEASURED, `field-render-design.md:20`).
   Anchored R1 leaves 5°–15° when `t/L` leaves 0.36–1.09 (the 88: 0.72); anchored R2 when `L/d_c`
   leaves 0.50–1.55 (the 88: 1.02) (DERIVED). The ILLUSTRATIVE blast round clears both, in opposite
   directions (0.9° under R1, 19.1° under R2).

**The 88 under un-anchored (b)**, DERIVED: 35.7° (R1) or 45.5° (R2) against 10°. At rest its
brightness falls from 5.76× to 1.71× / 1.40×, its in-band crew knockout radius from 4.75 m to 2.59
/ 2.34 m (the ball's at rest: 2.00 m) and a hard module's from 3.36 m to 1.83 / 1.66 m. At s1's
244.6 m/s the band 55.7°–75.7° becomes 29.9°–101.4° / 20.2°–111.2°, crew radius 4.98 m → 2.71 /
2.46 m, and at `N` 256 the spacing coarsens to about 9–10° (spacing as the square root of the
band's solid angle, from the MEASURED 5.0°). That is a re-tune of F7's default, not a refactor.

## 5. Open questions for the owner

1. F7: does the ring stay? If not, the question closes.
2. R1 or R2 (or another reading): which does an outside source on burst-core bodies support?
3. If (b): anchor at the 88's 10° (`k` TUNED), or let the 88 take its derived width?
4. Does the 88's borrowed cavity diameter (`88mm_APHE_ring.round.ron:33-34`) need a source first?
5. Under R1, does a near-planar disc read well, or does it need a floor (a threshold in the law)?
6. A uniform band or a profile (bright centre, end skirts): does a profile earn a visible difference?
7. Should the ring's `N` follow its band's solid angle as bands widen (cost: `spall_cone.ron:90-93`)?
8. Do squash-head, shaped-charge, tandem get own field sources (`owner-direction-2026-10-10.md:83-85`)?
9. Is a rocket's body a ring at all, or another source?
