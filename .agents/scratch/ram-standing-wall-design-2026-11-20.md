# Ram into a standing wall: motion passes a joint only within its strength

Subject: the game's building model (game state only): a brick house when a heavy vehicle drives into
a wall. Nothing was built or run. Citations are `main`. MEASURED numbers are the wall-yield report's
(`.agents/scratch/rubble-explore/wall-yield/report.md`, local M4) unless a source is given; DERIVED
ones show their arithmetic. The tank is the fixture (57,000 kg), never the spec. The tick is 1/64 s.

## 1. The rebound

The experiment's account, checked against source:

| Claim | Verdict |
|---|---|
| The standing cut frees a 600-chunk panel at tick 42 | Holds. `Geometry::resolve` (`src/construction/sweep.rs:350`) releases the whole source side of the minimum cut; `cut_fragments` (`sweep_intake.rs:1386`) parts only its perimeter, so it leaves as one connected body. |
| The 0.5 m size rule keeps the panel one body | Wrong emphasis. `merge_fragments` (`src/debris/crumble.rs:245`) only joins a fragment under 0.5 m to a neighbour. The strike's 489 parted joints carved 34 crumbs of one to three chunks off a 564-chunk fragment that was whole anyway (MEASURED). Without the rule the hull still meets 564 chunks. |
| The solve moves 473 kN·s hull to panel, 525 kN·s panel to house | Consistent. A plastic stop needs 57,000 × 6.57 = 374 kN·s (DERIVED); the other 99 kN·s is the +1.7 m/s reversal. Restitution is 0; Avian pushes overlap out at up to 4 m/s (`vendor/avian3d-0.8.0-overmatch/src/dynamics/solver/plugin.rs:595`). Which overlap fed it is not verified. |
| The house is judged "a tick late" | Half right. The panel's cut read its own free motion (0.15 m/s, 259 kN) and held; a held patch is a cast member, so `SweepContacts` (`sweep_intake.rs:93`) keeps its solver contact rigid and unlimited for the tick. The house is judged the same tick, after the solve, by `read_contacts` (`breakage.rs:880`), and next tick by `pushes` (`sweep_intake.rs:313`). |

So the hull stopped on a 16 t rigid body (600 × 27 kg, DERIVED) wedged against matter the solver holds
immovable. A held verdict says "this body's own motion will not break it"; the solver reads "nothing will".

## 2. One ram brings the house down

**The Hertz peak is the wrong load.** DERIVED from `contact_law.rs` and the Brick row (E 8.6 GPa,
ν 0.25, chunk sphere radius 0.152 m): 57 t at 8 m/s gives approach 62 mm and P = 73.5 MN, matching
the 71.9 MN MEASURED at tick 41. That is the whole hull stopped elastically on one brick lump:
contact area π·R·δ = 0.030 m², mean pressure 2.5 GPa, 68 times the row's 36.5 MPa crushing
strength. It is outside its own elastic range, and one scalar for a patch of any size.

**But its size is not why the panel goes.** The cut is binary: once demand exceeds the minimum
cut, `mincut.rs` returns that cut's source side whatever the excess. The panel goes because the
cut is static: it finds the cheapest joints in newtons, and moving 600 chunks costs it nothing.
0.697 MN is about 68 joints at the shear cohesion (0.165 MPa × 0.0625 m² = 10.3 kN, DERIVED); the
hull's own outline is about 46. A detail decides between them. One candidate: a patch reads one
direction, its earliest witness's normal (`sweep.rs:366`); on the rim of the first hole that
normal lies in the wall's plane, where joints bear at 2.28 MN each and carry the motion to the
panel's edges. Tick 42's witness normal is not in the report, so this is not verified.

**Then the solver's impulse is read as a static force.** `read_contacts` divides the step's
transfer by the step: 525 kN·s × 64 = 33.6 MN (DERIVED) on a house whose cuts hold under 1 MN.
The structure check cracks wherever that force runs: 402 to 160 chunks, ticks 43 to 47. The
impulse exists only because the solver stopped 57 t on immovable matter.

Not the cause: the joint data (Measured rows); the size rule; missing fracture dissipation (the plastic
exchange already loses about ½ × 83.5 kN·s × 8 m/s = 334 kJ against 68 × 1.8 J of crack work, DERIVED).

**Real masonry.** Single-wythe brick is weak out of plane, so a slow push does fail a whole
panel. A fast strike punches an opening about the vehicle's outline; the rest stands, and what
the opening left unsupported sags or falls.

## 3. The missing truth

**Matter has inertia, and a joint passes on only as much motion as its strength allows.** To
bring a struck chunk's neighbour to speed v, the joint between them must carry the stress ρ·c·v
(the plane-wave relation, exact; c the wave speed in the joint's mode). Past the joint's
strength, it parts before the neighbour moves. DERIVED from the Brick row (ρ 1845 kg/m³,
G 3.44 GPa, E/(1−ν²) 9.17 GPa):

| Mode at the joint | Impedance ρc | Strength | Motion passes below |
|---|---|---|---|
| Sliding | √(ρG) = 2.52 MPa·s/m | 0.165 MPa + friction | 0.065 m/s |
| Opening | √(ρE′) = 4.11 MPa·s/m | 0.69 MPa | 0.17 m/s |
| Closing (bearing) | 4.11 MPa·s/m | 36.5 MPa | 8.9 m/s |

Anything faster than a slow walk shears out the matter in its path, plus the matter behind it in
compression; slower, the structure answers as a whole and statics applies. The static cut has no
speed in it (fault 2). The solver has no strength in it (fault 1). One omission.

## 4. The design

**Law.** In the sweep, a released plug is the swept patch grown across every live joint that
passes the patch's motion. Joint mode comes from the witness direction and the joint normal (as
`translation_resistance` splits it), strength from `JointStrength::limits` and `bearing`, stress
from ρ·c·v at the witness's closing speed.
- If the plug reaches the ground or an uncovered chunk through passing joints, the body cannot move it: the verdict is today's static cut with the drive alone as demand.
- Otherwise it releases when the plastic exchange's loss pays the crack work of its perimeter (`Exchange::dissipated` against `crack_work`, both existing). This keeps light matter honest: a 27 kg chunk needs about 1.0 m/s to pay 4 × 1.8 J (DERIVED, reduced mass 13.5 kg); today's Hertz threshold for that case is about 0.7 m/s (DERIVED).
- Pieces stay large and climbable: a plug leaves connected, its inner joints whole; the size rule is unchanged.
- **Loose matter in the path passes motion whole.** A piece inside a faster source's swept bounds is cast with that source's motion and joins its one plastic exchange, carrying its own momentum in. Standing matter behind a pushed piece is judged in the same tick by the body really pushing. This replaces the `pushes` StandIn.

**Data.** Existing fields only: `density`, `shear_modulus`, `contact` compliance, the seam's
`tensile`, shear cohesion, friction and fracture energies, `bearing`. No new field, no new code
constant; ρ·c·v is a law's form.

**Tick placement.** Unchanged: `SweepSet`, `FixedPostUpdate`, before `PhysicsSystems::Prepare`,
server only. The flood runs inside the per-patch resolve, which already evaluates owners in
parallel (`resolve_patches`). The carrier lookup reads the sorted source list immutably, one piece
per task. Each carrier solves its one exchange in `exchange_all`'s batches. Effects (velocities,
ledger, account, spawns) commit as now, in (body, owner) order. No new serial stage or shared
writer.

**Deletes.**
- The Hertz demand in the sweep: `peak_force` and `peak_speed` in `resolve`, `PatchLoad::threshold`, `SweepSource::effective_mass`, `contact_law::peak_speed`. `peak_force` stays for landings (`crumble.rs`), which this design does not touch.
- `pushes`, its field, its account entries and its two tests.
- The experiment's strike path (`strike_pieces`, `strike_motion`, `commit_pieces`, held pairs) is not adopted. Only its carried-momentum term in `exchange` is rebuilt.
- Candidate, checked after the refuter: the static cut then only ever runs with the drive as demand, a load the post-solve structure check already receives. If nothing needs both, `mincut.rs` (572 lines with tests, MEASURED by `wc`) and `plug` go.

## 5. What it should produce

Expectations with absurd ends, not targets. Fixture arithmetic is labelled.

| Outcome | Believable | Absurd |
|---|---|---|
| Opening | About the vehicle's outline; matter above may stand, sag or fall | Narrower than the vehicle; the whole wall |
| Speed through one single-wythe wall | A modest loss, never a reversal. Fixture, DERIVED: 10.8 m² outline (MEASURED opening) is about 180 chunks, 4.9 t; 4.9 / 61.9 = 8 %, 0.6 m/s at 8 m/s | No loss; a stop; a reversal |
| Lengthwise peak | Tens of m/s² for a tick. Fixture, DERIVED: 0.6 m/s in one tick is 38 m/s², against 544 MEASURED | Hundreds |
| House after one wall | The other walls and most of the roof stand | Under half standing |
| Thick wall with a buried core | Holds; the vehicle stops or climbs | Passed without cost |
| Slow push, under about 0.1 m/s | Drive against the cut, as today | A crawl punching a clean outline |

## 6. The smallest experiment that would refute it

On a branch from `lane/lab-on-belt-main` (the lab is not on `main`): one arm of about ten lines in
`Geometry::resolve`. When the witness speed exceeds the sliding row's speed, keep `sweep.chunks`
as the swept patch instead of the cut's source side. No flood, no energy gate, no carried pieces:
the fixture wall is one chunk thick. Run `scripts/debris-lab run ram <out>` twice per side and
read `observations.json`. Refuted if, with the arm on:
- the house still ends under about half standing (the spread is not the panel cut); or
- minimum forward speed is still below zero while no piece over about 200 chunks exists (the rebound is not a wall-sized body, and carried pieces must carry the fix alone); or
- the lengthwise peak is still in the hundreds of m/s².

Also report the largest piece, the largest solver transfer onto standing matter, crossing time and tick p95. The lane deletes the arm when it reports.

## 7. Estimate (DERIVED from analogous code)

| Part | Added | Removed | Analogue |
|---|---|---|---|
| Flood and joint test in `sweep.rs` | 45 | 15 | `Geometry::cut` (20) plus the `plug` joint loop (30) |
| Impedances on the material; energy gate | 25 | 0 | `Compliance::of`; crumble's fracture budget |
| Hertz, `effective_mass`, `threshold`, `peak_speed` | 0 | 65 | counted on `main` |
| Carried pieces: gather and cast | 90 | 0 | `prepare_source` (40) and the experiment's gather |
| Carried momentum in `exchange`, write-back | 60 | 0 | the experiment's `sweep.rs` change |
| `pushes`, its field and account | 0 | 50 | counted on `main` |
| Production | 220 | 130 | net +90 |
| Tests | 100 | 85 | flood, carried and momentum tests in; Hertz and `pushes` tests out |

The last estimate of this kind ran 2.2 times over (260 estimated, 564 built, MEASURED), so read
net +90 as +90 to +250. If the static cut goes, the total is net negative by about 400.

## 8. What it does not solve

- **A hull meeting a large loose piece at rest on the ground** (the lab's passes scenario, −673 kN·s MEASURED). The same law applies to a piece as a target; the experiment priced that machinery at +564 lines and it is left out. Plugs get smaller here; fallen roofs do not.
- **A rigid stop on a real obstacle** (thick wall, permanent core, ground) and the overlap push-out after it: still Avian's contact.
- **Thick walls above the bearing row's speed** (8.9 m/s, DERIVED): struck chunks free with matter behind them. Bounded by momentum; unmeasured.
- **Landings.** The Hertz peak stays in `crumble.rs`; the same question, not asked here.
- **Chains of pieces deeper than one.**
- **What stands above an opening.** Connectivity and the structure check decide, as today.
- **The post-solve read of a held contact** still divides an impulse by the step. After this design it sees slow pushes, resting loads and light matter, which a static check is valid for.
- **Hosted cost.** The flood is cheaper than a cut over the whole structure, but unmeasured.

## 9. Rejected alternatives

- **Inertia as arcs in the minimum cut** (each chunk costs ρ·c·v·A to move): inertial stress does not add along a compression column, so a thick wall would hold a fast vehicle and pass a slow one.
- **A solver impulse clamp at the cut's capacity**: a fork divergence, and it leaves the hull overlapping matter nothing removed.
- **The experiment's strike, with the held push handed to the piece's cut**: keeps the wall-sized piece and the static cut that makes it, on top of +564 lines.
- **Dropping the size rule, or crumbling the panel harder**: pieces must break large, and the 564-chunk fragment was whole regardless.
- **Raising joint strengths in data**: Measured rows; a tuned value hiding a missing law.
- **A cap on plug size or width**: a rule for one kind of object.
- **A smaller or crushing-capped Hertz demand**: the verdict is binary, so the same panel leaves.
- **Clamping the press `read_contacts` reads**: treats the reading, not the stop that made it.
