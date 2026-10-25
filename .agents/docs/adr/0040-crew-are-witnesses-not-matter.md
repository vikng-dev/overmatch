# 0040 — Crew are witnesses, not matter

Status: ACCEPTED 2026-10-17 by the owner. `PROTOCOL_REV` stays 33 while unreleased.

## Decision

A substance is one of two kinds, each under its own law.

- **Hard** — armour and modules. It has a resistance factor and is a layer of the §13 walk: it
  resists the shell, is charged the shell's spend, throws spall at a body exit and shadows the
  spall field.
- **Witness** — crew. It has no resistance at all: the kind carries no factor field, so a zero or
  a dummy value cannot be written down. It is no layer: it never changes the shell's state, never
  resists, never shadows and never throws. Once the hard world has resolved a shot, each witness
  box is dosed by what passed through it:
  - **the line:** the shell's disc-mean chord through the box (the walk's own disc sampling, cut
    at the rest point of a shell stopped inside) × the **round's** crew line rate, as a `Line`
    deposit;
  - **the field:** each spall direction entering the box doses it at its strength there × its
    share, as a `Field` deposit, and continues to the first hard body.

The crew line rate is round data (`rounds.ron` `wound`, reference-mm per metre of chord, TUNED). Dose units,
HP pools, the once-per-box rounding and the `ShotDose` tally are the hard world's. A witness
substance sits only on a crew-role volume and a crew-role volume wears only a witness substance;
the bake refuses a tank file that breaks either, naming the node.

The laws are in `.agents/docs/design/armor-penetration-and-damage.md` §5, §6 and §13.2. The code
is in `src/substances.rs` (the kinds), `src/ballistics/wound.rs` (the witness pass) and
`src/ballistics/resolve.rs` (`field_direction`).

## Why

- A penetrator's potency against a hard body is the energy that body absorbs; against a crew
  pool it is the chord through the station. `chord × resistance` measures the first and says
  nothing about the second: a crew substance at 1 % of RHA needed about 30 cm of chord to empty
  a 30 HP pool (DERIVED).
- A body does not stop fragments. Crew shadowing crew was an artefact of a field direction
  stopping at the first HP box it entered, crew boxes included.
- A property that only one substance uses is not a property of every substance. A class with its
  own law is the honest shape.

## Consequences

- The hard world does not see crew. The shell's path, every armour event, every field
  direction's reach and every module's dose read the same with the crew removed (a class
  invariant test, `src/ballistics/wound/tests.rs`).
- Aiming at a crew station is rewarded. A clean main-gun pass empties the pool by its line
  alone: 10 cm of chord at 30 reference-mm/m is 30 HP (DERIVED). A graze of the box's edge
  leaves it reduced.
- One perforation doses every crew station in the cone, and nobody hides behind a comrade.
  Modules still shield. The field converts at its own calibration (`spall_cone.ron`), set so a
  station fully inside the cone's knockout radius — about 2 m from the hole for a weak ordinary
  perforation at the intended humanoid's 0.2 m² face, 3.2 m at the placeholder crew box's
  (DERIVED) — is knocked out; a station the cone's dim edge grazes keeps HP, reduced.
- Caliber reaches a crew pool only through the round's crew line rate.
- The dials are data: the crew line rate per round, the shared crew pool (30 HP, TUNED,
  provisional, `box_classes.ron`; no tank file authors it), the field's HP per reference-mm, and
  the cone's strength and direction count. Every other HP pool is its sturdiness class times the
  box's mean face (#2 and #3 in
  `.agents/scratch/damage-cam-2026-10-08/research/ap-aphe-2026-10-23/decision-2026-10-23.md`;
  design doc §5, §6).
- A crew exit throws no cone, and a fragment's reach surface in the pen cam ends only on hard
  bodies.
- Crew consequences are unchanged (stations, backfill, the knockout rule of playtest fork F2).
  They remain the place for crew-only ideas such as a reduced state with an effect.

**Amended 2026-10-23.** The field's own calibration, the shared crew pool and the class × face
pools above replaced the first version's reading, under which the field left a station reduced
and only the line knocked it out.

## Rejected

- **A crew-dose factor on every material.** Steel and engine blocks would have to carry a value
  that only the crew substance uses. That is overgeneralisation: a dummy field on every hard
  substance, and a plausible wrong value whenever one is mis-set.
- **A binary transit knockout by role.** "The line through a crew box knocks it out" turns a
  graze into a knockout and has no channel for a reduced state. It also adds a role branch to the
  damage law, where a dose through HP already has one.
- **Hole volume.** The box volume the shell's disc sweeps, rather than its chord, would make a
  crew pool's dose scale with caliber squared. It double-counts what the per-round crew line rate
  already carries, and it adds a geometric quantity no other law reads.
