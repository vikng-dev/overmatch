# Design sketch: weapon groups, fire selection & ammunition states

**Status: PROVISIONAL — idea, not a decision.** Recorded 2026-07-01 while shaping the concept;
**deliberately deferred** behind the single-player vertical slice. Nothing here is built. The current
Tiger (one cannon + one MG-coax, no ammo types) needs none of it — this is design-ahead for
multi-weapon and multi-turret tanks, and for ammunition selection (the nearest real piece). Expect
parts to be wrong until we build against a second, genuinely-weird tank. When decided, the
load-bearing parts graduate to an ADR (refining 0012/0013) and the terms to `GLOSSARY.md`.

Vocabulary from `.agents/skills/codebase-design` (seam, depth) and `.agents/skills/domain-modeling`
(challenge, stress-test with scenarios). Provisional terms are parked at the bottom, **not** yet
promoted to the glossary.

## The spine

Everything here hangs off one idea:

> **Every weapon, at every instant, has a ballistic solution = f(weapon, its active round). A weapon
> group shares one elevation servo, so it can physically *lay* exactly one member's solution at a
> time — the rest ride.**

Selection is navigating that structure; the reticle renders it; ammunition states feed each weapon's
solution.

## Why this exists

Today the superelevation solver hard-reads `Rig.muzzle` (the single `Primary` weapon), so "which
weapon the sight solves for" is frozen at bind and the coax silently rides the gun's θ. That's correct
for the Tiger but structurally cannot express a second cannon on the same mount (different
ballistics), a second turret, or ammunition with different ballistics. This doc is the model those
need.

## Physical vs semantic primitives

Composition = a small set of physical nodes (the model owns) bound to a small set of semantic roles
(the RON declares). The aim is that any tank — including the oddities below — is these primitives
*arranged*, not a special case.

**Physical (model nodes; axis-as-node per ADR-0012):**

- **Pivot** — a 1-DOF servo node, yaw or pitch; its orientation *is* its axis.
- **Muzzle / Barrel** — bore origin; the recoiling barrel.
- **Hull** — the body (and, for turretless tanks, the traverse itself — see below).

**Semantic (RON, keyed to nodes):**

- **Traverse** — the azimuth mechanism serving a mount: *yaw servo | hull | fixed*.
- **Group** — the weapons on one **elevation servo**; the unit of laying + fire selection. Derived
  from "which muzzles descend from this pitch pivot."
- **Weapon** — a bore + a **kind** + a magazine of loadable **rounds**.
- **Kind** — primary cannon / secondary cannon / MG / spotter / (ATGM later). Drives defaults + UI
  exclusions.
- **Station** — the crew place operating a traverse + group(s) (existing glossary term). The player's
  **active group** is the station they occupy.
- **Spotter link** — an MG ballistics-matched to a gun for ranging-by-tracer.

Two structural facts the oddities force:

- A **group is anchored on the elevation servo, not the turret.** One traverse can host several groups
  (Nb.Fz main turret: `{75 + 37}` on one pitch, `{MG34}` on another).
- **Traverse is not always a servo.** Turretless tanks (VT 1-2, Strv-103) traverse by steering the
  hull. A group's azimuth resolves to a yaw servo, or the hull, or nothing (fixed).

## The selection stack

Three nested levels; within a group, **two orthogonal axes**:

1. **Active group** (= station) — which mount the player is laying/ranging. Cycle to switch.
   Single-mount tanks: one, no cycle.
2. Within the active group:
   - **Laying** — which member's ballistic solution drives the shared elevation. Default =
     highest-kind cannon. **Trivial when members share ballistics** (twin identical guns), **real when
     they differ** (75 vs 37).
   - **Fire selection** — which barrels actually discharge on the trigger: one / a cycled subset /
     salvo. **Rich when a group has many equal barrels** (Ontos), trivial for one weapon.

   These are independent: Ontos has trivial laying + rich fire selection; Nb.Fz is the reverse. Do
   not conflate them.
3. Per weapon — the **round trio** (below).

MGs (and spotters) drop out of laying and the round trio entirely: single belt, never laid, always
along-for-the-ride. That is what keeps the UI uncluttered.

## Ammunition states

Under **Round** (a *type* of ammunition; carries the ballistics that key its range table — distinct
from **Shell** = a round in flight), each weapon tracks three states:

- **Selected** — the round the player designated to load next; a standing order, no physical cost.
- **Loaded** — the round seated in the breech; what fires. Swapping costs a reload. May be none.
- **Active** — the round the **fire-control solution + reticle reflect**. Equals loaded when seated;
  during a load it is the *incoming*; through an unload it stays the *outgoing*, until the incoming
  begins loading.

`active` is a **projection** of the reload pipeline, not independent state — derive it from
`(loaded, selected, phase)`, do not store it separately, or it desyncs.

**Phase machine** (the reload pipeline):

- **Loading** — seating the incoming round (the existing `shooting::Reload` timer). `active` =
  incoming.
- **Unloading** — clearing the breech to swap. `active` = outgoing until cleared.

**Reconcile policy** (selected vs loaded):

- Breech empty or mid-load → load `selected` immediately; selecting a different round mid-load
  **interrupts and redirects** (progress lost — a partial-load penalty).
- Round seated, `selected` differs → **protect the seated round** (still fireable); a **confirm /
  second press** commits the swap (unload → load). The game does not dump a loaded round on a whim.

Scenario that justifies three states: *AP loaded, select HE, fire before HE seats* → AP fires
(loaded ≠ selected), and the reticle showed AP throughout (active = loaded). Two states can't express
that.

Consequence for code: the range table becomes per **(weapon, round)**, and the reticle reads the
**active round's** table. Build a table per loadable round at bind (a handful per weapon).

## The reticle

For the active group, the reticle draws a ranging indicator **per member**, each from its own
(weapon, active round) table: the **laid** member on the sight line (centre), others offset by their
ballistic difference at the dialed range — a real multi-scale sight. Switching the laid weapon swaps
which mark is centred. (Open: whether MG/spotter members draw a mark or are omitted to de-clutter.)

## Default fire-control mode

All weapons fire on their triggers; each group's elevation solves for its **laid weapon's active
round**; non-laid members ride. Naming it "default" leaves the door open for siblings (single-weapon
mode, independent-turret laying, salvo modes) — noted, not built.

## Oddity tanks — the primitive-forcing scenarios

| Tank | What it forces |
| --- | --- |
| **VT 1-2** — twin 120s, one pitch, yaw-by-hull, fire either/both | Traverse = hull (not a servo); **fire selection ⟂ laying** (identical guns → trivial lay, real left/right/both) |
| **Nb.Fz** — 3 turrets; main turret 75 + 37 on one pitch + MG34 on another | **Group anchored on elevation, not turret** (one traverse, two groups); 75 vs 37 → **real laying** (dual-scale sight) |
| **T-35** — 37s in own turrets; main turret 75 + ball-mount MG | Many small independent groups, each with its **own station/crew**; ball mount = a group-of-one with its own traverse + elevation |
| **Ontos** — 6 rifles one axis, each a bolted ranging MG, fire 1/cycle/all | **Rich fire selection** + trivial laying; the **spotter** — an MG matched to the gun so tracer = range-by-observation (an alt to the dial) |

"Any tank from primitives" is a **north star, not a milestone** (per `rig-ron-sot-and-composability.md`):
good primitive boundaries only appear under the tension of a real second tank. Don't abstract around
the Tiger — add one deliberately-weird tank and let it reveal the seams.

## Open decisions (parked)

- **Kind taxonomy** — extend `trigger: Primary/Secondary` into a `kind` carrying MG/spotter-ness +
  laid priority, or keep them as separate fields?
- **Reticle for MG/spotter members** — draw their marks (informational) or omit them (de-clutter)?
- **Range: per-group vs shared** — cycling groups: each holds its own dialed range, or one designated
  range broadcast to all (the "commander says 800, each solves" idea)?
- **Ammo switch cost** — partial-load penalty on interrupt; unload time; confirm / second-press vs a
  dedicated unload input.
- **Spotter ranging** — range-by-tracer as a real alternative to the dial, or cosmetic only?
- **Fire-selection inputs** — mapping single / cycle / salvo to keys without cluttering the common
  (single-weapon) case.

## Migration seam

One hardcode carries the whole thing: today `aim::drive_aim_servos`, `camera::gunner_camera`,
`sight::drive_gunner_aim` / `toggle_sight`, and `sight::update_ranging_reticle` all read `rig.muzzle`
for θ. That becomes **"the active group's laid weapon's active-round table."** `Rig.muzzle` stays
"what LMB fires"; the *laid solution* becomes its own lookup, and `firecontrol::RangeTable` moves from
per-weapon to per-(weapon, round). The whole feature lands behind that single seam — behaviour is
identical until a tank has a second selectable weapon or a second round.

## Provisional glossary terms (NOT yet promoted)

Parked here; promote to `GLOSSARY.md` on build, as an "Armament" cluster.

- **Round** / **Shell** — a loadable ammunition *type* / a round in flight.
- **Selected / Loaded / Active round** — the reload-pipeline states; `active` is the fire-control
  projection.
- **Traverse** — azimuth mechanism (yaw servo | hull | fixed); may host several groups.
- **Group** — weapons on one elevation servo; the laying + fire-selection unit.
- **Kind** — weapon role (primary / secondary cannon, MG, spotter…).
- **Laying** vs **Fire selection** — the two orthogonal per-group axes.
- **Spotter** — an MG ballistics-matched to a gun for ranging-by-tracer.
