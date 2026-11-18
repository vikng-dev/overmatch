# Crushing under the tank's weight: design (game state only; nothing built or run)

Paths below are under `R = /Users/Yan/github/vikng-dev/overmatch`. Every finding is from reading code and issues, so anything about outcomes is DERIVED until slice 0 runs.

## Short answer

**The missing truth:** weight is a force that is always there, and a loose piece is made of the same joints and material as when it stood. Today a loose piece's contact forces are read every step and thrown away unless one of them is a blow. So the tank's slow press breaks nothing in a pile.

**The law (completing #232's accepted model):** every force on a chunk group is judged against what carries it, whether the group stands or lies and whether the force is a blow or a press. A press must beat strength; a blow must also pay energy.

**Answer to the owner's question (disintegration or cluster breakage?):** both, as successive states of the same matter under one law.
1. **Bonded:** a cluster splits along joints the force overloads. This is the main fix.
2. **Resting:** pieces below the server's body size ride together as a heap held by friction only.
3. **Gone:** a bit squeezed out of that heap leaves the solver as drawn decoration. Crushing a single piece through its bulk is the law's third term and almost never fires for brick under this tank.

**What stops it:** the tank's weight is fixed. Each failure lowers the hull onto more pieces and onto its tracks, so the pressure falls. An evenly loaded bed at track pressure is below every masonry strength in the data.

**One honest limit:** a compact, sound block loaded squarely holds by the authored data, and real masonry would too. What gives there is the ground, which the game has data for but uses only for track prints. That is a second truth, outside these slices, gated on slice 0.

**Decisions:** build cluster breakage first, then heap disintegration, then bulk bearing with a corrected datum. Crushed matter leaves as the existing decoration. Ground yield waits for the probe's verdict.

## Facts

### 1. What happens today

**A freed "grouped chunk"** is one rigid body (`Piece`, `R/src/construction/breakage.rs:672`). It keeps its construction state: pools, internal bonds, opened joints (`R/.agents/docs/design/construction-destruction.md:1054-1056`).

**Loose piece under belly or track:**
- Its contacts are read every step. Each point's press is collected at `R/src/debris/crumble.rs:427`.
- A group with no blow past its rest speed is then dropped at `R/src/debris/crumble.rs:459`.
- Cracks are also capped by the kinetic energy the blow dissipated (`crumble.rs:206`), which is zero for a press.
- The design doc states the exception itself and flags it "the owner's to confirm": a moving group's own contacts do not enter the laws (`construction-destruction.md:1064-1065`, `1152-1154`).
- Issue #374 names the same gap as an Unknown.

**The size-floor weld:**
- A fragment under `server_rubble_min_size_m` (0.5 m, Taste, `R/assets/ballistics/physical_laws.ron:9`) is merged into a neighbour across a parted joint (`crumble.rs:245-314`).
- These retained joints restore no strength, but nothing ever judges them again (`breakage.rs:553`).
- A floor-size body therefore cannot break further: `crumble.rs:711-713` returns "still one body".
- The floor (0.5 m) sits just above the fixture's belly clearance of 0.477 m (MEASURED, #374).

**Standing remnant:** built, in two places.
- Before physics, the hull sweep casts the hull against standing chunks only (`R/src/construction/sweep.rs:22`; `R/src/construction/breakage/sweep_intake.rs:55,247`). A resting hull loads the cut with its drive, gravity included (`construction-destruction.md:463-474`).
- The cut's compression capacity is bearing strength times joint area (`sweep.rs:436-451`). A plug that fails is released as loose bodies, not crushed.
- What the cut holds presses through the solver. Every transfer becomes a press-only load (`breakage.rs:1116-1131`, `crushed: 0.0`), belt reactions included (`R/src/contact_transfer.rs:39-42,264`).
- The structure check judges tension, shear, lift, tip and slide (`R/src/construction/fracture.rs:1229-1235`). It has no compression cause.

**The 2026-11-11 finding** ("the force answer never settled while the tank pressed"):
- The blow half has no code path left. Contact loads carry `blow: ZERO` (`breakage.rs:1123`) and the sweep judges the hull's impact before physics.
- The cost half plausibly remains. A press that moves more than `LOAD_TOLERANCE` (1e-5 of the owner's weight, `R/src/construction/fracture/parts.rs:40,1358`) re-solves the owner every tick.
- I could not run it, so this is unconfirmed on main.

### 2. The crush law today

- Each chunk has a volume pool, worn by `Load::crushed` (`fracture.rs:828-838, 1201-1216`).
- Feeders: a round's line (`fracture.rs:905`), fragments (`:926`), blast Law 1 (`:161, :978`), and scab removal (`:996`).
- No contact feeds it. `crushed` is 0.0 at `breakage.rs:1122`, `crumble.rs:95` and `attachments.rs:697`.
- `Material::bearing` (`fracture.rs:269`) is read only by the sweep's cut.

### 3. A spent pool

- The chunk becomes `Fate::Dust` (`fracture.rs:833-836`). It does not shrink or split; a worn chunk keeps its collider (the pool is read only at `fracture.rs:832-833`).
- It is replicated: `Event::Wear` and `Event::Dust` are ledger entries (`breakage.rs:398-405`).
- Sub-floor fragments become dust entries plus one `Shed` message, drawn client-side with no body (`crumble.rs:748-778`; doc `:1172-1194`).

### 4. Does the authored data say it should fail?

Tank fixture: 57,000 kg (`R/assets/tiger_1/tiger_1.tank.ron:5`), so W = 559 kN (DERIVED).

Brick strengths in `R/assets/materials/materials.ron`:
- Compressive strength 36.5 MPa, Measured, labelled "unit strength, not the masonry prism's" (`:647-651`).
- Assemblage strength 12.265 MPa, DERIVED inside the Young's modulus label (`:667-671`).
- Joint tension 0.69 MPa (`:634`) and cohesion 0.165 MPa (`:681`).

| Case (all DERIVED) | Pressure | vs 36.5 MPa unit | vs 12.3 MPa assemblage |
|---|---|---|---|
| One real brick's bed face, 0.022 m², full W | 25.4 MPa | holds | fails |
| Smallest game chunk, 0.12 m, 0.72 W | 28 MPa | holds | fails |
| 0.225 m chunk under a belt, 44 kN (MEASURED load, #374) | 0.87 MPa | holds | holds |
| Wall stub 0.25 × 0.25 m, 0.72 W | 6.4 MPa | holds | holds |
| House pier section 0.25 × 0.55 m, 0.72 W | 2.9 MPa | holds | holds |

- **Crushing through the bulk: no.** Columns and lumps hold by 2 to 12 times.
- **Powdering: no.** Brick's crush energy is 135 MJ/m³ (doc `:1300`). A 0.2 m descent under 0.72 W pays for 0.6 litre (DERIVED).
- **Joints: yes.** 0.72 W sheared across a 0.25 × 0.25 m joint is 6.4 MPa, 39 times cohesion. A 0.25 m slab spanning 1 m under 200 kN bends to 4.8 MPa against 0.69 MPa.
- **Compact block, squarely loaded: holds.** A 0.5 m block cracks only if the load sits more than about 12 cm off centre.
- **The ground: yes, on soil.** 0.72 W on 0.25 m² is 1.6 MPa, 15 times the tank's nominal 107 kPa (DERIVED, `R/.agents/docs/design/track-model/architecture.md:518`). The print softness in `R/assets/terrain/ground_types.ron` gives 0.34 m of sinkage on sandy loam, 0.09 m on firm dirt and none on paving.

**One number, two jobs:** `compressive_strength` serves blast compaction of the unit and bearing of the assemblage (`fracture.rs:43-45, 223`). The doc's own table asks for the assemblage value "sourced independently" (`:88`).

### 5. Issues

- **#333:** the observation, and the checkpoint this design answers.
- **#232:** the accepted model; "any body's contact, the belt's reaction and weight" is unbuilt for loose pieces.
- **#374:** hull bellied on a 0.69 m bonded fragment carrying 0.72 W; 7–8 pieces under the belly (MEASURED).
- **#379:** one hosted ram gave 52 pieces, 35–49 awake for 8 s, then all frozen (MEASURED). About 0.12 ms per awake piece per tick (DERIVED from its 5.7 ms at 48).
- **#312:** parked; reopens with #333.
- **#351, #314:** unaffected in kind.
- **#382:** piece boundaries decide where clusters split, so its answer changes this law's look, not its form.

## Design

### The law

One system, the existing `crumble_landings`, with its gate removed.

1. **Every step, every awake loose group's contacts are judged.** Presses are summed into one patch per chunk per other collider, as blows already are, so the narrow phase's point count changes nothing.
2. **Press pass:** `Assessment::landing` runs on the presses alone. Failures commit on strength only, because the pressing body pays the fracture work. That work is about 2.5 J per 0.25 m² joint against kilojoules of hull descent (DERIVED), and standing structures already do this (`fracture.rs:1245`).
3. **Blow pass:** as today, capped by the landing's energy.
4. **A weld is a rest.** A joint the size floor welded is recorded cracked and closed, which the laws already read as push and friction only (`fracture.rs:713`). A weld is made once, when its bond breaks. One that later lifts, tips or slides stays open, and a sub-floor fragment it frees leaves through the existing `route` as dust plus `Shed`.
5. **Bulk bearing (third term):** a chunk whose through-load exceeds bearing strength times its largest section (`fracture::silhouette`) spends its pool and becomes dust. Smaller corner crushing is centimetres and not simulated.

**Data read:** joint strengths and friction (existing), one new bearing datum (assemblage, DERIVED value already in the row), and chunk geometry. No new taste.

**Effects:**
- Standing remnant: unchanged in kind. The bearing datum corrects its cut capacity.
- Loose cluster: splits where the force runs.
- Welded heap: carries load in compression and loses what is squeezed out.

### Which outcome carries the fix

Cluster breakage carries most of it (DERIVED, not established): bonds are 20 to 200 times weaker than bulk, and pieces in a pile bridge and bear off-centre. Disintegration finishes the job for tall heaps. Bulk crush is completeness.

### What stops it

- **Pressure against strength.** Nominal track pressure of 107 kPa is below cohesion (165 kPa), tension (690 kPa) and bearing (12–36 MPa). A bed loaded evenly breaks nothing.
- **Load sharing.** Each failure moves weight to the tracks and to more pieces.
- **Nothing left to break.** A single chunk has no joint, and a compression chain opens no weld.
- **Monotone states.** Bonded, then resting, then gone, never back. The settle probe's 200-per-second flip between fixed and loose cannot occur.

**Believable range:**
- Absurd end: a tank hangs on a few pieces with its tracks spinning (today).
- Absurd end: a pile is gone after one pass and the tank drives through at ground level.
- Between: the hull sinks into the pile within about a second as slabs snap and tall heaps lose their tops, then rides a lower, wider bed. A compact block on hard paving may still hang it.

### Tick order

- The law reads the last physics step's transfers (`crumble.rs:620-624`).
- It commits inside `LandingSet`; children are born in `SplitSet` before publish (`breakage.rs:3618-3634`).
- No pose or velocity of the tank is written. Children keep the parent's rigid motion, so the hull comes down under gravity through the solver.
- The longest free drop is one chunk layer, 0.12–0.42 m (doc `:1178`). The landing after it is a blow, judged with energy.
- Decoration takes its mass and momentum out of the simulation, an existing accepted gap (doc `:258`).

### Server cost

- **Intake is free:** presses are already gathered and discarded.
- **The solve is the cost.** A non-cracking landing of a 66-chunk group is 1.0 M scalar operations (DERIVED count, doc `:1204-1205`).
- **Bound 1:** a cheap strength screen per piece skips the solve when total force and lever cannot reach the weakest joint.
- **Bound 2:** a per-tick budget of press solves, most overloaded first. A press is steady, so a late verdict is the same verdict. This is a `Resolution` number and it is Unknown.
- **Parallelism:** one task per piece, commits in entity order, exactly as `crumble.rs:669-742` does now.
- **At 30 tanks:** only tanks touching awake pieces pay.

### Body count, both ways

- **Splitting adds.** A cluster of volume V makes at most V / 0.125 m³ bodies. A house pier of 0.40–1.10 m³ makes 3–8 (DERIVED). Each awake body costs about 0.12 ms per tick on the hosted server (DERIVED).
- **Disintegration removes.** A floor-size heap that loses a bit drops below the floor and leaves whole.
- **Net** under a tank is unknown until measured: more bodies for the first moments, fewer after.

**Effect on other issues:**
- #379 is not fixed. Its census must be re-read.
- #312 may improve because the tall loaded stack goes. Re-measure with its three poses.
- Neither failed direction is repeated: no static settling, no belt rows.

### What the player sees

| Option | Cost | Read |
|---|---|---|
| A. Existing decoration: bits fly, rest, fade | none on the server | pile lowers; bits vanish after their lifetime |
| B. Smaller server bodies | halving the floor is 8 times the bodies | best motion, worst cost |
| C. A mound: shed volume raises a rubble layer in the ground | runtime terrain against the one-surface invariant (`R/src/terrain_grid.rs:12`), plus replication | truest; large |

I recommend A now. C belongs with ground yield later.

### Deletes

- The blow-only gate (`crumble.rs:459`).
- `rubble_joints` and the `retained` argument.
- The doc's moving-group exception.
- #374's Unknown.

## Cheapest refuting experiment (slice 0, report only)

- **Setup:** a disposable probe in `R/src/headless_test/track_scorecard/rubble_climb.rs`, on the #374 stuck case and the heap seeds.
- **Method:** each tick, for each awake piece, feed its presses to `crumble()` on a cloned state with unlimited energy. Commit nothing.
- **Measured:** the share of belly-borne weight on pieces whose joints fail; the height of what would remain against clearance; operations per tick.
- **Verdict (binary):** do the pieces that carry the hull fail under the press they carry?
- **Absurd ends:** none fail (the law is not the missing truth here, and ground yield goes first); every piece under the tracks fails on the free-running control case (the stop does not hold).

## Slices

1. **Press on loose groups.** Gate removed, patch-summed presses, budget and screen, doc section rewritten.
2. **Welds are rests.** `rubble_joints` deleted; opened welds shed.
3. **Bulk bearing.** New assemblage datum, read by this law and the sweep's cut.
4. **Re-measure #374, #312 and #379.** Decide ground yield from what still beaches.

**Unknown numbers:**
- The press-solve budget: settled by hosted cost per solve.
- Whether the houses' masonry is the row's Type N assemblage: a construction fact.
- The share of square-compression beaching: slice 0.
- Pile volume lost per pass: an observation after slice 2.

## Risks and what I could not establish

- A compact block squarely loaded still holds. That is honest, and possibly still absurd on soft ground.
- Solver force noise across a flat manifold may read as bending and over-break.
- Splits wake islands and add spawn cost during a drive-over (#314).
- A floor-size heap vanishes whole on losing one bit, which may read as popping.
- Not established: outcomes of any kind, hosted cost, the shipped rest speed that divides blow from press, and whether the 2026-11-11 cost still holds on main.

### Critical Files for Implementation
- /Users/Yan/github/vikng-dev/overmatch/src/debris/crumble.rs
- /Users/Yan/github/vikng-dev/overmatch/src/construction/fracture/parts.rs
- /Users/Yan/github/vikng-dev/overmatch/src/construction/fracture.rs
- /Users/Yan/github/vikng-dev/overmatch/src/construction/breakage.rs
- /Users/Yan/github/vikng-dev/overmatch/.agents/docs/design/construction-destruction.md
