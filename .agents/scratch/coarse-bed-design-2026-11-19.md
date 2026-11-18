# Coarse bed: design

Subject: the game's destruction model (pieces, joints, loads and fill), game state only. Nothing was built or run. Code citations are at `142a1519`, read with `git show`, paths under `/Users/Yan/github/vikng-dev/overmatch/`. "Recalled" marks a number that is in no file I read.

## Decisions

- **Node spacing is 0.5 m, not the queue's 1 m.** A 0.7 m track lane cannot be held at 1 m (section 3).
- **State is two conserved solids per node** (coarse, fines). Depth, firmness and relief amplitude are all derived from them.
- **The published surface stays the prototype's 0.25 m, 33×33 grid per 8 m tile.** Only how its samples are computed changes, so every reader is untouched.
- **Relief is integer-only and evaluated only at the 0.25 m lattice nodes**, at publish. It is bit-identical on both ends for free.
- **Build as a sibling module `src/rubble_coarse/`**, copied from the prototype and publishing into the same `BedSurface`. The lab selects the arm.
- **Honest headline:** the wire gain is about 5 to 15 times on a drive and 2.5 to 5 times on a collapse; the server gain is small. The larger prize is that roadwheel-scale relief becomes real and consistent, which the full bed lacks today.

## 1. Coarse state per node

| Item | Value | Job |
|---|---|---|
| Node spacing | 0.5 m, 16×16 per 8 m tile, index fits `u8` | Resolution: at least one node row always lies inside a 0.7 m lane; three or more rows stay untouched between the tracks |
| `coarse: u16` | solid height still at grain size, 2 mm units | Resolution, see below |
| `fines: u16` | solid height crushed, 2 mm units | Resolution |
| Unit volume | 0.0005 m³ solid (DERIVED, 0.25 m² × 2 mm); one baked chunk of 0.014625 m³ is 29 units | exact form constant |
| Packing coarse / fines | 0.50 / 0.65 | Unknown (the options note's proposals; a bulk-density source settles them) |
| Grain | 0.245 m | StandIn: one grain for all fill; DERIVED from the house asset per the options note; the truth is per deposit |
| Relief share | 0.5 of the grain, peak to trough | Taste, ride-roughness family; scales the grain, does not replace it |
| Fine lattice | 0.25 m (`STEP`, `src/rubble_bed.rs:20`) | Resolution: force stations sit 0.264 m apart (DERIVED, 97 links × 0.1305 m pitch / 48 stations, `src/track/forces.rs:375-382`), so nothing finer is felt |

- **Why 0.5 m for sight:** a sight line is blocked by height over metres. 2 m cells would do, so the tracks set the size.
- **Why 2 mm solid:** it is 3.1 to 4 mm of bulk. One step under one node moves at most 1/12 of a belt's support by 4 mm. I expect that to be far below a jolt, but the ride response is Unknown; the comparison in section 8 measures it.
- **Sub-quantum give** is carried per body, as today (`src/rubble_bed/bearing.rs:85`, `:416-421`).
- **Derived per node, never stored:** bulk depth, coarse share, friction angle (38° coarse to 44° fines, Unknown) and relief amplitude.

## 2. The relief function

Each coarse node (I, J) sits at fine node (2I, 2J) and owns the 2×2 block of fine nodes (2I+a, 2J+c), a and c in {0, 1}.

1. **Depth.** `b(I,J)` is an integer function of the two solids, with two constants fixed at load from the packings.
2. **Smooth part, in quarter-millimetres.** A fine node on a coarse node gets `4b`; an edge midpoint gets `2(b1+b2)`; a cell centre gets `b1+b2+b3+b4`. No division, so no rounding.
3. **Relief.** `h = mix32(I, J)`, a fixed integer mixer over wrapping `u32`. `h` picks one of about eight four-entry patterns that each sum to zero, such as (+3, −1, −1, −1) or (+1, +1, −1, −1), and a rotation.
4. **Amplitude.** `a = min(grain × share × coarse/(coarse+fines) / 4, S_min / deepest negative entry)`, where `S_min` is the smallest smooth value in the block.
5. **Sample.** `top = base + (S + a × pattern) × 0.00025`, the same single float operation as today (`src/rubble_bed.rs:373`).

Against the six requirements:

- **(a) Volume:** the weights 1, ½ and ¼ over a node's 3×3 fine stencil sum to 4, so the smooth part integrates to exactly `b × 0.25 m²`. Each block's relief sums to zero. The depth cap in step 4 keeps every height at or above the base without clamping, which would break the sum.
- **(b) Continuity:** the surface is the facet interpolation of single-valued lattice samples. Tile borders read the neighbour's nodes, as publish already does (`src/rubble_bed.rs:364-373`). The existing "rebuild if the east or north neighbour changed" rule (`:356`) still covers the stencil.
- **(c) Blocks where coarse, smooth where crushed:** a +3 entry is one pyramid 0.5 m across its base, the owner's "50 cm stone". At full coarse share it stands 92 mm proud with 31 mm dips (DERIVED, 4a = 0.1225 m). At zero share it is flat.
- **(d) Local, no swimming:** the hash reads only integer lattice coordinates. State only scales amplitude, so lumps shrink in place. Crushing one node changes its 3×3 fine stencil and at most four blocks' amplitudes.
- **(e) Cost:** the track columns sample the tile's `HeightGrid` exactly as now (`src/track/world_contact.rs:26-38`, `world_contact/terrain.rs:20-49`). Relief adds 1,089 hashes per rebuilt tile.
- **(f) Identity:** bit-identical, since everything before the final multiply is integer.

**What the client derives:** the fine samples, and from them its collider (camera, aim and presented round casts), the belt and wheel view (`src/track/view.rs:837-846`), the mesh and chunk seating. It does not simulate the tank.

**What breaks if it differs:**
- A few millimetres: nothing visible.
- A different pattern: the hull heaves over bumps the viewer cannot see, wheels oscillate over the wrong stones, and a presented round clips a lump the server never had. So the pattern must match; the last bit need not, and matching it costs nothing.

**Heavy feel (DERIVED):** relief RMS is 54 mm (a√3). Over 16 roadwheels at independent phases the hull heaves about 13 mm RMS. The wheels work and the hull barely moves.

## 3. How the tank presses it

**Survive unchanged in form:**
- Deposit by projected overlap with a carried fraction (`src/rubble_bed/deposit.rs:20-78`). Every shed chunk arrives as coarse.
- Facet repose (`src/rubble_bed/flow.rs:20-74`), on the smooth part only. Relief is not state, so repose cannot relax it. This is the options note's grain floor (option C), by construction.
- Give, heave to the nearest free ring, and underside caps (`bearing.rs:194-264`, `:415`).

**Changed:**
- **Belt patch size** must come from the loaded contact samples' own second moments. From node masks (`bearing.rs:138-166`) a 0.7 m belt reads as 0.5 or 1.0 m wide, which moves capacity by about −50% to +100% (DERIVED, capacity goes roughly as width squared).
- **Caps** take the minimum of rays at a node's four owned fine nodes, so a 0.25 m wall is not missed.
- **The slope term** (`bearing.rs:390-401`) reads the smooth part only. A lip now needs 18 cm over 0.5 m to yield, where it needed 9 cm over 0.25 m.

**New (the graded law, at node resolution):**
- **Crushing.** Converting coarse to fines lowers the node, and the load's descent work pays for new surface.
- That gives a threshold pressure `p* = W / (1/φc − 1/φf)` = 13.8 kJ/m³ / 0.4615 = 29.9 kPa (DERIVED; recalled 50 J/m² fracture energy, proposed packings).
- The fixture's belts press at 111 kPa (DERIVED in the options note). A 300 kg panel over 1 m² presses at 3 kPa and crushes nothing.
- The rate is bounded by the give law's descent.
- **Firming.** Capacity reads the patch's coarse share, so a crushed lane holds 2.9 times more (DERIVED in the options note, Nγ 78.0 to 224.6).

**Lost:**
- Rut walls ramp over 0.5 m, not 0.25 m.
- A 0.7 m track publishes a lane about 1.0 ± 0.25 m wide (DERIVED: a partly covered edge node at 40% still sees 44 kPa and crushes whole).
- All lumps on a node shrink together; no single stone has its own history.
- Solid patches (belly, pieces) are quantised to 0.5 m.

**Lane narrower than a cell:**
- At 0.5 m a lane is held, one or two node rows wide, and the 2.36 m between the tracks keeps its crown (track centres ±1.528 m, `assets/tiger_1/tiger_1.tank.ron:28`; 0.7 m width recalled).
- At 1 m it is not: a lane between rows crushes two rows partly into a 2 m shallow trough. No extra field fixes that more cheaply than halving the cell.

## 4. Wire cost (all DERIVED; the full bed's figures are MEASURED)

- **Entry:** `(u8 node, u16 coarse, u16 fines)` as postcard varints (recalled encoding), 3 to 5 bytes, 4 typical.
- **Full bed, for scale:** 44,232 B/s at 64 Hz and about 3.5 B per entry is about 197 node changes per tick.

| Case | Derivation | Coarse bed | Full bed MEASURED |
|---|---|---|---|
| First pass, 5 m/s (speed Unknown) | 9.6 lane nodes per metre × 16 to 40 steps, plus about 50 berm entries: 200 to 430 entries/m × 4 B × 5 m/s, plus about 1 kB/s of diff headers | 3 to 9 kB/s | 44,232 B/s peak |
| Repeat pass on a crushed lane | no coarse left to convert; overload give on slopes only | about 1 kB/s | not separated |
| Collapse | 994 to 1,122 chunks × about 2.5 nodes each, doubled for repose: 5,500 entries × 4.5 B = 25 kB over 1.5 to 3 s (duration Unknown) | 8 to 16 kB/s | 38,144 B/s |
| Late join, one house | 4 tiles × (512 B of zeros + about 100 B) | about 2.4 kB | about 5.1 kB (DERIVED, 4 × 1,024 + 970 filled nodes) |

- **Cross-check for the drive:** 49 nodes under the footprint, stepping on 25 to 77% of ticks, gives 3 to 12 kB/s.
- **Hard bound:** one tile's whole state is 1.28 kB.
- **Lever not taken:** committing every 4 ticks caps a drive near 3.5 kB/s.
- Replicon framing, acknowledgements and client count are excluded, as in the measurement.

## 5. Server cost (DERIVED relative to the full bed; nobody has measured the split)

| Work per tick | Full bed | Coarse bed |
|---|---|---|
| Underside rays, one per filled node (`bearing.rs:310-319`) | about 970 for a 60 m² bed | the same 970 (four per node) |
| Repose facets, commit copy and diff compare (`src/rubble_bed.rs:288-294`) | 1 | ¼ |
| Publish: 33×33 samples, new heightfield collider, sleeping scan, for each changed tile and its neighbours (`:351-420`) | 1 | 1, since some node steps on nearly every tick |

- **Estimate:** 0.6 to 1.0 of the full bed.
- **What dominates (hypothesis):** the collider rebuild and the rays. The coarse bed helps neither.
- The only MEASURED figure is the whole tick, 1.28 to 1.49 ms in local debug (fair test).
- Caching rays over static solids would help both arms equally.

## 6. What a second player sees

- **Geometry: nothing differs.** Mounds, lanes and every lump come from replicated state through the same integer function.
- **Late joiners** get the same relief, which today's shed chunks cannot give them.
- **Timing:** diffs apply on arrival, ahead of the delayed hull, exactly as in the full bed.
- **Client-only:** dust and particles.
- **Drawn chunks** differ only if they stay shed-driven. A follow-up should draw them from the same hash, one at each proud node, so the visible stone is the collision lump.
- **None of this is meaningful by the owner's test:** sight lines, bumps and lanes are identical.

## 7. Build outline

**Route:** a sibling module, not a generic lattice. `STEP`, `SIDE` and `COUNT` are constants threaded through four files; making them parameters would be a switch in shipped code.

1. **Share the reader seam.** `Surface`, `BedSurface`, `Topped` and `ground()` stay where they are (`src/rubble_bed.rs:118-270`). Extract the collider-and-wake tail of `publish` (`:381-421`) into one function both arms call.
2. **Copy** `bearing.rs`, `deposit.rs` and `flow.rs` into `src/rubble_coarse/` and edit them.
3. **Write** the state, relief, commit and publish, and `src/net/rubble_coarse.rs`.
4. **Lab selection.** Shipped compositions keep mounting `rubble_bed::plugin` (`src/lib.rs:718`). The lab adds the coarse plugin itself and calls `configure_sets` on each arm's `Publish` set (`src/rubble_bed.rs:272-273`, `:461-468`) with a lab-owned run condition. It resets the world on a switch. No shipped file branches on the arm.

| Part | Lines (DERIVED from the analogue's size) |
|---|---:|
| Lattice, tile, working set with two solids (analogue `rubble_bed.rs:19-116`) | 110 |
| Relief: mixer, patterns, amplitude, sample | 70 |
| Commit and publish (analogue `:276-423`, less the shared tail) | 110 |
| Shared tail extraction in `rubble_bed.rs` | 55 |
| Deposit and flow edits | 40 |
| Bearing: sample-based patch, crush, firming, block caps | 175 |
| Wire: diff, admission, registration, revision bump (analogue `src/net/rubble_bed.rs:12-103`) | 100 |
| View keyed by surface, not `BedTile` (`src/rubble_bed/view.rs:8`) | 10 |
| Lab selection | 30 |
| Tests | 140 |

- **Total:** about 840 authored (range 700 to 1,000), plus about 530 copied unchanged.
- **Tests:** solids conserved; relief zero-sum and never below base; server and client samples equal; a crush changes only its stencil; diff round-trip.
- **If the graded lane lands on the explore branch first,** copy from it; the two-solid edits (about 150 lines) are then already made.

## 8. The one comparison

- **Setup:** same collapse, same scripted line at one fixed creep speed, three passes, in three arms: today's game, the graded full bed with its grain floor, and the coarse bed. The bare full bed is smooth at lattice scale, so it would lose on wheel motion for the wrong reason.
- **By eye:** a fixed side camera at wheel height, clips side by side and unlabelled. The owner names which is which, and says whether the wheels oscillate as they do today.
- **By number, per arm and pass:**
  - RMS roadwheel travel.
  - Peak upward hull acceleration per tick (the fair test's jolt measure: 14.86 to 30.15 m/s² for the bed against 36.08 for control).
  - Speed lost over the crossing.
  - Lane cross-section at 0.25 m after pass one.
- **Reading it:** if the owner cannot pick the arms and wheel travel is within the spread between passes, the coarse bed is indistinguishable. The lane width, 1.0 m against 0.7 m, is where I expect it to show first.

## 9. Not verified

- Belt width (0.7 m) and contact length are recalled, not read from data.
- Ride frequency and damping were not read, so the "4 mm step is not a jolt" claim is unmeasured.
- The crush threshold rests on a recalled fracture energy and proposed packings. The Brick row's energy was not read.
- The crush law is new and unmeasured, like the give law.
- Postcard's varint sizes and replicon's per-diff framing are recalled.
- The drive speed and collapse duration behind the wire figures are assumed.
- The server split is a hypothesis; no per-system timing exists.
- The options note's "bed mean 0.03 to 0.25 ms" was not re-checked.
- The lab harness and the graded lane are being built in the bays and were not read. The `configure_sets` selection assumes the lab composes its own app.
- Whether client and server are guaranteed the same `physical_laws.ron` and base grid bits was not traced. The relief identity depends on both.
- Whether `Dust` entries carry volume that should arrive as fines was not traced.
- How settled large pieces go static is queue item 5 and is not designed here. The bed only needs their undersides.

### Critical Files for Implementation
- /Users/Yan/github/vikng-dev/overmatch/src/rubble_bed.rs
- /Users/Yan/github/vikng-dev/overmatch/src/rubble_bed/bearing.rs
- /Users/Yan/github/vikng-dev/overmatch/src/net/rubble_bed.rs
- /Users/Yan/github/vikng-dev/overmatch/src/track/world_contact/terrain.rs
- /Users/Yan/github/vikng-dev/overmatch/src/net/protocol.rs
