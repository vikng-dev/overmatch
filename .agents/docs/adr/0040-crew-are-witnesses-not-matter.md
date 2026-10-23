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
  box is wounded by what passed through it:
  - **the line:** the shell's disc-mean chord through the box (the walk's own disc sampling, cut
    at the rest point of a shell stopped inside) × the **round's** wound rate, as a `Line`
    deposit;
  - **the field:** each spall direction entering the box doses it at its strength there × its
    share, as a `Field` deposit, and continues to the first hard body.

The wound rate is round data (`rounds.ron`, reference-mm per metre of chord, TUNED). Dose units,
HP pools, the once-per-box rounding and the `ShotDose` tally are the hard world's. A witness
substance sits only on a crew-role volume and a crew-role volume wears only a witness substance;
the bake refuses a tank file that breaks either, naming the node.

The laws are in `.agents/docs/design/armor-penetration-and-damage.md` §5, §6 and §13.2. The code
is in `src/substances.rs` (the kinds), `src/ballistics/wound.rs` (the witness pass) and
`src/ballistics/resolve.rs` (`field_direction`).

## Why

- A penetrator damages a hard body by the energy that body absorbs, and soft tissue by its wound
  channel. `chord × resistance` measures the first and says nothing about the second: flesh at
  1 % of RHA needed about 30 cm of chord to kill a 30 HP crewman (DERIVED).
- A body does not stop fragments. Crew shadowing crew was an artefact of `passage: FirstSolid`
  stopping at every HP box.
- A property that only one substance uses is not a property of every substance. A class with its
  own law is the honest shape.

## Consequences

- The hard world does not see crew. The shell's path, every armour event, every field
  direction's reach and every module's dose read the same with the crew removed (a class
  invariant test, `src/ballistics/wound/tests.rs`).
- Aiming at a crew station is rewarded. A clean main-gun pass kills: 10 cm of chord at 30
  reference-mm/m is 30 HP (DERIVED). A graze of the box's edge wounds.
- One perforation wounds everyone in the cone, and nobody hides behind a comrade. Modules still
  shield.
- Caliber reaches a crewman only through the round's wound rate.
- The dials are data: the wound rate per round, crew HP, and the cone's strength and direction
  count.
- A crewman exit throws no cone, and a fragment's reach surface in the pen cam ends only on hard
  bodies.
- Crew consequences are unchanged (stations, backfill, the knockout rule of playtest fork F2). They remain the place for
  crew-only ideas such as wounded-not-dead.

**Amended 2026-10-23.** The field converts its dose to HP at its own calibration; every crew
station has one shared 30 HP pool (TUNED, provisional) that no tank file authors, and every other
HP pool is its sturdiness class times the box's mean face (#2 and #3 in
`.agents/scratch/damage-cam-2026-10-08/research/ap-aphe-2026-10-23/decision-2026-10-23.md`; design
doc §5, §6). One perforation now knocks out every crew station fully inside the cone's knockout
radius, about 2 m from the hole for a weak ordinary perforation at the intended humanoid's 0.2 m²
face and 3.2 m at the placeholder crew box's (DERIVED); a station the cone's dim edge grazes keeps
HP, reduced. Nobody still hides behind a comrade, modules still shield, and the clean main-gun
pass above still empties the crew pool by its line alone.

## Rejected

- **A wound factor on every material.** Steel and engine blocks would have to carry a value that
  only flesh uses. That is overgeneralisation: a dummy field on every hard substance, and a
  plausible wrong value whenever one is mis-set.
- **A binary transit kill by role.** "The line through a crew box kills" turns a graze into a kill
  and has no channel for a wound. It also adds a role branch to the damage law, where a dose
  through HP already has one.
- **Hole volume.** The tissue volume the shell's disc sweeps, rather than its chord, would make a
  crewman's dose scale with caliber squared. It double-counts what the per-round wound rate
  already carries, and it adds a geometric quantity no other law reads.
