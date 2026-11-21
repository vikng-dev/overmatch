# Cutter, local zone, gravity: design check

Subject: the game's building model (game state only): cells joined by joints, loose pieces, and what
a moving body does to standing cells in its path. Nothing was built or run. Code citations are
`lane/brick-fracture` at `8cc1519e` (the base) unless a lane is named. MEASURED numbers name their
record; DERIVED ones show their arithmetic. The vehicle (57 t) and `house_1` are fixtures, never the
spec.

**Verdict: BUILD IT WITH CHANGES.** Rule 3 (gravity only) is supported by a run. Rule 1 needs a
different bound than crushing strength, and a train. Rule 2 as sketched derives to the whole facade;
with its missing term it derives to less than one cell at ram speed, so the first build has no ring.

## The law (one page)

1. **Cutter (before the solve).** A standing cell can stop a body only by holding it over the cell's
   own depth. For each cell a body's swept volume meets this tick (its witness: normal `n`, closing
   speed `v`), with `d` the cell's extent along `n`:

   `D = P + ½ m v² / d`  against  `H`

   - `m`: the effective mass at the witness of the body and its train.
   - `P`: the forces that persist behind them along `n`: weights and drives. Never a solver reaction.
   - `H`: the least force that moves the swept cells along `n`: today's directional minimum cut
     (`Geometry::plug`), used as a price only.
   - `D > H`: the swept cell is freed, and only it. `D ≤ H`: it keeps its solver contact; that is a
     real stop.
2. **What a freed cell does.** It leaves in the body's velocity field after one plastic exchange
   (momentum conserved, today's `exchange`). That exchange is the whole drag: the body loses the
   share of its momentum the freed mass takes, a few cells a tick.
3. **The train.** Bodies that pressed a body along its motion in the last step, and still move along
   it, strike with it, to any depth. So a loose piece between a hull and a wall transmits; the wall
   yields if the train's `D` beats its `H`, else the stop is real.
4. **Gravity only.** The support check reads the structure's own weight and field impulses (rounds,
   bursts). It reads no contact from any body, awake or asleep.
5. **Connectivity** frees what lost its last path to the ground, at rest, as today.
6. **Local zone: one region law for impulsive loads.** A load that crushes volume `V_c` at a point
   on a face fractures the standing matter within `r_f = r_c · k^⅓` of it, through intact seams of
   one material; `r_c = (3 V_c / 2π)^⅓`, `k = σ_c,d / (2 σ_t,d)`. It replaces `take_zone`. A burst
   feeds it today. A vehicle's strike on a thin wall crushes too little to reach a second cell
   (DERIVED below), so the cutter does not call it in this build.

No authored number is added. `½`, `2`, `3/2π` are constants of the laws' forms. `d`, `m`, `P`, `H`
come from geometry, mass data and the material rows.

**What the player gets:** a jagged opening about the hull's width (whole cells, so 0 to one cell
proud each side), what the opening left unsupported falling under gravity, the walls beside it
standing, the tank slowed smoothly and never bounced. A slow or coasting body against something
its energy and drive cannot move is stopped, as it should be.

## 1. Premise

**Channels on the base through which a body loads or removes standing cells:**

| # | Channel | Where | Under the candidate |
|---|---|---|---|
| 1 | Sweep judgement: Hertz peak plus drive against the minimum cut; a release frees the cut's whole source side | `sweep.rs:344-395` (`peak_force` `:363`, source side `:379`), `plug` `:400` | demand replaced, frees swept cells only |
| 2 | Every dynamic body is a sweep source, and this tick's freed pieces again for 8 rounds | `sweep_intake.rs:613` `sources`, `:391` `DEPARTURE_ROUNDS`, `:793` | kept |
| 3 | Connectivity after a cut | `sweep_intake.rs:1386` `cut_fragments`, `fracture.rs:1126` | kept |
| 4 | Solver contact on a held or overlapped cell, read as force: whole transfer over the step | `breakage.rs:881` `read_contacts`, `:1067` `press: -point.impulse / transfers.dt()` | deleted |
| 5 | Sleeping bodies' presses, carried into every run of the laws | `breakage.rs:857` `RestingLoads`, `:896`, `:1948` | deleted |
| 6 | The belt's applied contact, through the same intake | test `an_implicit_belt_press_reaches_the_wall_laws_reading`, `breakage/tests.rs:2000` | deleted with 4 |
| 7 | `pushes`: last step's contact transfer on a loose body, over the step, added to its drive | `sweep_intake.rs:313-341`, read at `:628`, `:645`, `:652` | deleted; the train replaces it |
| 8 | The part solve's sustained channel, where 4 and 5 land | `parts.rs:1251`, `:1616-1620` | reads gravity only for standing owners |
| 9 | The hook that decides which standing cells the solver may touch | `sweep_intake.rs:44-121` `SweepContacts` | unchanged |

Landings (`crumble.rs:402`) load moving groups only, never standing cells (`breakage.rs:35-36`).

**Did any arm run with every contact-to-structure channel off?** One run came close, and it is the
best evidence there is. `lane/ram-law:.agents/scratch/ram-plug-refuter.md:88-107` (arm P, narrow
house, one run, diagnostic switches): with all awake presses and `pushes` off,
- the hull's openings took 78 cells and the house stood at 1,044 of 1,122 for 0.84 s (MEASURED);
- the hull rebounded at −1.81 m/s, went 0.64 m back, never crossed in 16 s, 22 belt reversals;
- channel 5 was still on: "a 115 kN press from 12 resting contacts" then took the house to 292.

So gravity only keeps the building and stops the vehicle on a wedged piece. Arm A (bounded readings,
`ram-a-experiment.md:141-156`) repeats it on the fitted house: −1.06 m/s, 1.31 s under 0.5 m/s. Arm
B's train then removed the stop (B': min +7.11 m/s, worst tick 1.00 m/s, 45 % in place). Every
large loss B and D still had is a channel this candidate deletes:
- B: Hertz through the static cut (`ram-law-premise-check-4:99-104`).
- D house: 154 cells by the cut's source side, 449 by the support check under a bounded press
  (tick 1692: 127 kN failed 101 piers), and the 3.15 m/s tick from roof boards the energy gate held
  (`ram-a-experiment.md:733-745`).

**Prediction from the records.** House: D's sweep took 212 cells and connectivity 16; without the
cut and the press the rest is what gravity alone drops. In place 40 to 60 %. Motion like B'.

**The strongest reason it fails like the eight before.** Each round found a new place where the
solver stops the hull on matter the law held. Here that place is a loose piece the hull did not
press last step: one stopped against the far wall before the hull arrives, or a heap two pieces
deep. With the support check deaf, nothing but the train relieves it. B' did not hit it on the
fitted house (freed pieces travel with the hull); nobody has shown it cannot.

**Where the premise is wrong.**
- "Drag bounded by the wall's crushing strength over the contact patch" bounds nothing: 36.5 MPa ×
  0.056 m² = 2.1 MN a cell against 39 kN of hold (DERIVED, section 2). The drag is the momentum the
  freed mass takes.
- The ring from that capped force derives to the whole facade (section 3).

## 2. Cutter

**Found and freed.** The base already does the finding: `Geometry::swept` casts each source's hulls
over its tick's travel and keeps each target's earliest inward witness (`sweep.rs:267`, `:659`);
freed cells never reach the solver (`SweepContacts`). Worth rebuilding from the arms:
- judging each witness at its own normal and speed (C and D; the earliest-witness judgement let the
  solver stop the hull at 4.53 m/s, `ram-a-experiment.md:559-562`);
- the train (B, rebuilt in C: `lane/ram-law:sweep_intake.rs:346` `pairs`, `:1200` `source_body`,
  `:1253` `apply_exchange`).

Not worth rebuilding: the flood (C), the closing-loss energy gate (D: it reads the struck cell as a
free body, so a long timber cell struck off-centre never pays), bounded presses (A), `train_mass`
(B), freeing the cut's source side (every arm but D's momentum branch).

**The gate.** `D = P + ½ m v² / d > H`. It is the mean force that would stop the body within the
cell's depth. Brittle joints do less work than `H·d`, so the gate over-holds: it protects standing
walls from light debris and is irrelevant for a heavy body by two orders.
- `H` is the minimum cut's capacity with the patch as source (`plug`, unchanged). It prices bearing
  correctly: a slab resting on a wall is held by the column under it; a wall end pushed end-on is
  priced at its row sliding on its bed joints. The cut's source side is never freed.
- Fast path: when `D` exceeds the sum of the patch's own boundary joints (an upper bound on any
  cut), no flow is solved. A heavy body at speed always takes it.

Fixture arithmetic, DERIVED. Brick cell: 14.1 L (`lenses-2.md`), 26 kg at 1845 kg/m³, face 0.056 m²,
side 0.237 m, four side joints of 0.059 m² at 0.165 MPa cohesion: `H` = 39 kN, `d` = 0.25 m.

| Striker | `D` | Verdict against 39 kN |
|---|---|---|
| Hull, 57 t at 10.63 m/s | 3.22 MJ / 0.25 m = 12.9 MN | freed, 330 times over |
| Hull at 0.1 m/s under drive (ASSUMED grip 0.9 × 559 kN) | 1 kN + 503 kN | freed: a crawl still gets through |
| One 26 kg piece at 10.63 m/s | 1.47 kJ / 0.25 m = 5.9 kN | held; frees only above 27 m/s |
| 471 kg piece at 2.0 m/s alone (D's tick 1692) | 3.8 kN | held |
| The same piece with the hull as its train | 115 kJ / 0.25 m = 460 kN plus drive | freed; D measured that cut at ≥ 119 kN |
| 1.27 t piece at 10 m/s (B's largest) | 254 kN | freed, cell by cell |

**The freed cell.** A loose server-owned body. Cells freed together from one owner stay connected
(`cut_fragments`, the 0.5 m merge rule), so pieces break large. Velocity: the striker's rigid field
after the plastic exchange (`exchange`, `sweep.rs:752`); matter that only lost its support starts at
rest (`fragment_motion`). The exchange welds sliding too; with single swept cells that is tens of
kilograms and stays as is.

**What the hull feels.** MEASURED under D: 8 to 23 cells a tick along the path. 23 × 26 kg = 0.6 t:
10.63 × 0.6 / 57.6 = 0.11 m/s in the worst tick (DERIVED). One single-leaf wall, 4.9 t in a 10.8 m²
outline: 10.63 × 4.9 / 61.9 = 0.84 m/s over about 13 ticks (DERIVED).

**A real stop** is `D ≤ H`: a cell borne by the ground or buried matter along `n`, a member whose
seams outweigh the body's energy and drive, a light or slow body. The solver's contact is the stop.

**The end-on clip, same law.** A track meets a wall's end. The cut prices the row sliding: 25 cells
× 2 bed joints × 9.7 kN = 487 kN (DERIVED). At 10.63 m/s `D` is several meganewtons; under drive alone
503 kN passes narrowly. The cell is freed, travels with the hull, and the hull's own sweep reaches the
next cell within two ticks (advance 0.166 m a tick, cell 0.237 m). Wall mass eaten: ASSUMED 1.2 m of
hull front on a 0.25 m wall, 550 kg per metre, 0.016 m/s a tick. MEASURED under D, which already
frees swept cells only: worst tick 0.50 m/s, min +6.69 m/s, no roll-back
(`ram-a-experiment.md:724`). A coasting body below about 2.7 m/s is stopped: that is the law, not
a case.

## 3. Local zone

**The sketch, derived.** Contact pressure capped at crushing strength `p = σ_c` on the patch, spread
until hoop tension falls to tensile strength. Hull-front patch ASSUMED 3.76 × 1.0 m (the front's
height was not read), half-width `a` = 1.88 m, equal-area radius 1.09 m.

| Form | Brick (σ_c 36.5, σ_t 0.69; dynamic 86.0, 2.14 MPa) | Timber, `Wood` row (5.6, 2.4; dynamic 33.4, 2.4 MPa) |
|---|---|---|
| Plate, hole pressed in plane: `σ_θ = p (a/r)²`, `r_f = a √(σ_c/σ_t)` | 1.88 × 7.27 = 13.7 m | 1.88 × 1.53 = 2.9 m |
| Cavity in a solid: `σ_θ = p/2 (r_c/r)³`, `r_f = r_c (σ_c,d / 2σ_t,d)^⅓` | 1.09 × 2.72 = 3.0 m | 1.09 × 1.91 = 2.1 m |

On a 6 m wall both brick results are the whole facade: absurd. **The missing term:** the patch is
released at its perimeter long before the contact reaches crushing. Perimeter hold over patch area:
0.165 MPa × (2 × 1.0 + 3.76) m × 0.25 m / 3.76 m² = 0.063 MPa (DERIVED), 580 times under σ_c and
under σ_t. Nothing past the perimeter is asked to carry more than a joint's strength.

**With the term: the zone follows what is actually crushed.** Upper bound: all of a strike's plastic
loss crushes matter at σ_c,d (ASSUMED rigid-plastic: work = strength × volume).

| | Brick cell | Timber cell (plate cut 0.15 × 0.15 × 0.75 m, 7.6 kg) |
|---|---|---|
| Loss, ½ m_cell v² at 10.63 m/s | 1.47 kJ | 0.43 kJ |
| Crushed `V_c` | 17 mL | 13 mL |
| `r_c = (3V_c/2π)^⅓` | 2.0 cm | 1.8 cm |
| `r_f` | 5.5 cm | 3.5 cm |
| Zone volume, share of the cell | 0.34 L, 2.5 % | 0.09 L, 0.5 % |

Both are below one cell. So the opening is the swept cells, whole: hull width plus 0 to 0.24 m each
side per height, 3.76 to 4.2 m (DERIVED from the cut's 0.21 m pitch), plus what gravity takes.
MEASURED under B': 1.72 to 4.16 m. That is not an exact hull outline, and it is far from the facade.

**If the first clip reads as underpowered, the lever is a law, not a number.** How far a wall
learns of a push before its joint lets go: `R = c_s · δ_c / v`, with `δ_c = 2 G_II / τ_c` = 2 × 28.4
/ 0.165e6 = 0.34 mm and `c_s = √(G/ρ)` = 1,365 m/s (DERIVED): 4.4 cm at 10.63 m/s, 0.17 m at
2.7 m/s, 0.47 m at 1 m/s, 4.7 m at 0.1 m/s. A fast punch is clean; a slow push takes the panel,
which is what masonry does. Named, not built: no fixture asks for it yet.

**One law with `take_zone`, reshaped.** `take_zone` (`fracture.rs:1232-1257`) is this law coded as a
volume budget inside one element (`part_of`), with no radius and a hand-copied ratio
(`materials.ron:448`, `:515`, `:556`, `:600`, `:683`). Its replacement, `fractured`:
- shared input: the crushed volume and its point. A burst's is Law 1's; a body's strike would be
  its loss over σ_c,d, when one is ever large enough;
- `k = σ_c,d / (2 σ_t,d)` computed beside the law from `Compaction::strength` and `Spall::tensile`,
  floored at one (a zone holds at least what was crushed). The `fractured_zone` field and its five
  rows go;
- `r_c` the half-ball of `V_c` (the load is on a face), `r_f = r_c k^⅓`;
- reached: from the struck cell through live, uncracked bonded joints between cells of one material,
  a cell admitted while its equal-volume sphere reaches within `r_f` of the point;
- spent nearest first up to `k V_c`, as now, so a finer cut loses the same volume where the ball
  lies inside the matter. In a thin wall the ball holds less than the budget and the zone stops.
- Check (DERIVED, `lenses-2.md`): 200 kg at 1 m, `V_c` 95 L, `r_f` 0.97 m; the 0.25 m wall holds
  0.74 m³ inside it against a 1.92 m³ budget.

`crack` (`fracture.rs:1025`, `:1085`) reads the same hoop field on each seam's plane for a round's
line. It stays a sibling reading: its cavity pressure is the walk's crush energy, a pulse's is
σ_c,d. Merging the two pressures is not in this candidate. The green timber rows divide by an
along-grain strength, so their `k` floors at one as today; their across-grain tensile strength is
missing data, to be filed.

## 4. Gravity-only support check

**Reads today:** gravity, field impulses, every awake contact's transfer over the step, every
sleeping contact's retained press, the belt's applied contact. **Reads under the candidate:**
gravity and field impulses.

**Deleted:** `read_contacts`, `prepare_contacts`, `contact_load`, `ContactSource`, `ContactInput`,
the intake witness, `RestingLoads`, `RawDeposit::contacts` and its readers (`breakage.rs:1856`,
`:1917`, `:1948`), `Origin::Contact`, and `ContactTransfers`' resting retention if nothing else
reads it (19 mentions, readers found only in tests).

**Loose pieces resting on standing structure: their weight does not count.** Reason: the only
reading of it is the sleeping press, which is what the solver last held, not a weight (MEASURED:
115 kN from 12 resting contacts felled a house that had stood, `ram-plug-refuter.md:107`). Cost: a
standing beam or slab never fails under piled debris or a parked vehicle. No shipped construction
has a storey that would show it. The sound form, when one does: a sleeping body's `m·g` on the cell
under it.

**A piece pinned between a hull and a standing wall.** A rigid piece transmits. The wall cell yields
when the train's `D` beats its `H`; if the wall is borne by ground or buried matter the stop is
real. The piece is never crushed for being in the way.

**The one-tick stop against an already felled house (15.2 m/s, MEASURED) is out of scope.** The
sweep admits standing owners only (`Without<Piece>`, `sweep_intake.rs:238-248`); a felled house is
one 21.3 t loose body the hull meets in the solver. What would touch it: lying pieces as cutter
targets (design 3's step 4, estimated +150 to +330 lines).

## 5. The unzipping under plain gravity

Hall, D, tick 1652: 13 roof members swept, then 111 cells in one tick by the support check
(MEASURED). Two causes, both real, neither a ram law:
- **Law form.** `bonds` judges every interface on one settled field and cracks every overstressed
  one in the same tick (`parts.rs:1587-1614`), and an interface's joints crack together once one
  reaches its strength (`:1616`). Nothing sheds load between the first failure and the last.
- **Data.** The roof's fixings are `nailed`, 3.24 kPa, a StandIn 50 to 100 times under a member
  joint (design 3); a hanging board levers it 100 to 1,400 times over (`ram-premise-check:98-101`).

It is not correct: the end rooms' rafters stand on unbroken walls. **Nothing of it belongs in this
candidate.** The hall is not on `main`. The sound law is the most overstressed interface first,
solved again next tick; it changes how every burst collapse unfolds and what a tick costs, so it is
its own design. The candidate's report attributes every tick that drops 8 or more standing cells.

## 6. One path

Lines are DERIVED from the cited ranges; read each "added" as up to 2.2 times (the record of these
estimates).

| | Removed | Added |
|---|---|---|
| Contact intake (section 4), `breakage.rs` | about 300 | 0 |
| Hertz in `resolve`, `PatchLoad::{demand, threshold}`, `contact_law::peak_speed` | about 45 | 0 |
| The judgement: per witness, `D`, fast path, swept cells only | about 15 | about 50 |
| `pushes` and its account entries; the train (C's step 3 MEASURED +76 −48, less its bounded push) | about 50 | about 80 |
| `take_zone`, `fractured_zone` (field, check, five rows) | about 45 | about 45 |
| Production | about 455 | about 175 |
| Tests: contact loads, Hertz brackets, source-side releases out; section 9's in | about 700 | about 300 |

Kept: `sweep/mincut.rs` (the price of a hold), `peak_force` (landings, `crumble.rs:527`), the hook.
No lever, flag or data field is added.

## 7. Server and wire

- Everything runs on the server, in `SweepSet`, `FixedPostUpdate`, before `PhysicsSystems::Prepare`.
- Patches resolve per (source, owner) in parallel, reading structure, state and gravity preload
  immutably (`resolve_patches`, `sweep_intake.rs:1046`).
- The train: one read-only fold of the last step's contact groups, batches joined in key order as
  `pushes` is now; the closure walks pairs in entity-key order. Serial because a train's membership
  sums over bodies (named data dependency).
- Exchanges solve per body in batches and apply in body order; cuts commit in (body, owner) order
  (`commit`, `sweep_intake.rs:1225`). The part solve stays per owner.
- No replicated type changes: the record still carries `Event::Separate` and the piece types.
  `PROTOCOL_REV` stays 107. Wire bytes will move (fewer separations): an observation.

## 8. Pass line, fixed before building

Instrument: `scripts/perf/collapse.py --ram`, `house_0001`, three runs a pass, plus one diagnostic
run a pass attributing every tick that drops 8 or more standing cells. Face-on: the heading that
meets a 6 m wall square (1.12 m clear each side). End-on: the heading that meets the 4 m gable, so
both tracks run into the long walls' ends. Contact at the recipe's 95 % of governed speed.

| Reading | Face-on: believable | Face-on: absurd | End-on: believable | End-on: absurd |
|---|---|---|---|---|
| `min_forward_m_s` | +5 to contact − 0.5 | ≤ +2, or no loss | ≥ +3 | ≤ 0 |
| `largest_forward_change_m_s` | under 1 | ≥ 2.3 | under 1.5 | ≥ 3 |
| Seconds under 0.5 m/s; `stall_s` | 0; 0 | any | 0; 0 | any |
| `crossing_s` | 0.9 to 1.6 | none, or over 3 | 1.2 to 2.5 | none |
| `in_place_share` | 40 to 60 % | under 35 %, over 65 % | not read: all of it is in the path | |
| `opening_m2` | 6 to 12 | under 5.6 (the hull's own outline), 14.9 (the face) | not read | |
| `displacement_m` | under about 3 | matter thrown clear of the footprint | observation | |
| Largest piece | up to about 2 t | 5 t or more: a wall-sized body | the same | |

Ranges DERIVED: in place from design 3's geometry (433 of 727 masonry cells off the path) on the
brick's 11.0 m³ and the timber's 2.0 m³ above grade; the opening from 3.76 to 4.3 m by 2.475 m.

**Kill line: back to design, no fix round.** Any of:
- face-on `in_place_share` under 35 %;
- `min_forward_m_s` ≤ 0, or more than 0.25 s under 0.5 m/s, on either pass;
- a one-tick change of 2.3 m/s or more face-on, 3 m/s or more end-on;
- runs of one binary on different sides of any threshold.

A tick in which the support check alone drops 20 or more cells off the path does not kill the
cutter; it opens section 5's design.

**A larger building is not needed for this verdict.** The house decides hole against collapse and
the vehicle's motion. It cannot show a roof staying on unbroken walls (its roof is unbraced boards
in the path); the landing report says that claim is unread.

## 9. Build order (one builder, one bay, from `main` once the fracture lane has landed)

Before the first commit: drain the ratchet backlog rows naming `sweep.rs`, `sweep_intake.rs` and
`breakage.rs`.

1. **Gravity only.** Section 4's deletions. Test `a_standing_structure_hears_no_contact`: on the
   shipped sim plugin, a body resting on a wall and one pressed against it leave the part solve's
   loads equal to gravity's. Relied on by rule 4. Mutant: any contact press restored.
2. **The judgement.** `resolve` per witness with `D`, the fast path, swept cells only; Hertz out.
   - `a_cell_that_cannot_stop_a_body_within_its_depth_is_freed`: bounds read from the material row,
     the same verdict at two timesteps. Mutant: a demand that reads the step.
   - `a_release_frees_only_swept_cells`. Mutant: the cut's source side.
   - `a_weight_at_rest_is_borne_in_bearing`: a slab on a wall top frees nothing. Mutant: the hold
     priced from the cell's shear joints alone.
   - The crawl half of `crawl_drive_and_stronger_material_follow_the_cut_threshold` stays.
3. **The train.** Carriers from the last step's press pairs, closed over depth; one exchange shared
   by mass; `pushes` out.
   - `a_pressed_body_strikes_with_its_train`: momenta of piece, carrier and freed cells sum to no
     change. Mutant: judged alone.
   - `a_reaction_is_no_carrier`: the ground and static bodies carry nothing. Mutant: arm A's first
     build.
   - `net::ram`'s momentum test sums every account entry of the hull's train.
4. **Measure.** Section 8, no code. Report the tables, the attribution, the count of `Unresolved`
   vetoes, tick p95, and what a tree does on the forest map, as observations.
5. **The zone as a region.** Section 3's `fractured`; independent of 1 to 4. The cut-independence
   test keeps its claim where the ball lies in the matter;
   `a_zone_stops_at_its_radius_and_crosses_elements_of_one_material`. Mutants: the element filter;
   no radius.

**The owner's first clip:** face-on, follow camera. Watch the wall beside the hole in the second
after contact: it should stand, with a ragged edge about the tank's width. Then the tank at the far
wall: no hitch.

## Rejected

- **Crushing strength as the drag bound:** 53 times the hold of a cell (2.06 MN against 39 kN,
  DERIVED); binds nothing.
- **The ring as capped force spread to tensile strength:** the whole facade; the patch's release is
  the missing term.
- **Hold from the cell's own joints, no cut:** a slab at rest sinks through a wall, or (with bearing
  counted) an end-on clip under about 4.9 m/s is a rigid stop (DERIVED: 2.1 MN of bearing, 0.5 MN
  of drive, 31.7 t at the track edge for an ASSUMED 6.3 × 3.7 m hull).
- **An energy gate on crack work:** joules; any debris above 1 m/s cuts.
- **The cut on the drive for slow strikes, freeing its source side (D):** 154 cells on the house.
- **A bounded press into the support check (A, D):** 127 kN failed 101 piers.
- **Sleeping weights kept:** the retained press is not a weight.
- **A tuned ring width:** a number hiding the reach law.

## Not verified

The hull front's height; the headings that give the two passes on `house_0001`; whether an
`Unresolved` target's veto (`sweep.rs:357-359`) ever stops the hull; the cost of a cut per witness
on the slow path; that `ContactTransfers`' resting retention has no other reader; how a tree behaves
(the support check no longer hears a slow push, so a trunk shears where struck or stops the tank);
which share of a strike's loss really crushes matter; hosted cost of any of it.
