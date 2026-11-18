# Ruins, not a pool: simulation-side options

Subject: the game's destruction model (pieces, joints, loads and fill), game state only. I read the code and the three reports on `lane/rubble-bed-drive` at `142a1519` via `git show`; nothing was run or edited. Paths are relative to `/Users/Yan/github/vikng-dev/overmatch`. "Recalled" marks a number from memory that is in no repo file I read.

## 1. Why it is a pool: what the code and numbers say

**P1. All of the house is fill before the tank arrives.**
- Smoke: 235,231 units = 14.7019 m³ bulk (MEASURED, prototype report). At packing 0.60 (`assets/ballistics/physical_laws.ron:12`) that is 8.8212 m³ solid (DERIVED), exactly the house's occupied material (fair test line 147).
- The count was the same before and after the crossing (MEASURED), so every piece had already converted.

**P2. The ruin state existed and one pass erased it.**
- Fair test after collapse: 7 bodies, pile height 2.949 m, bed peak 0.35–0.39 m (MEASURED).
- After pass 1: 0 bodies, height 0.70–0.78 m (MEASURED). Control with the old weld: 39 bodies, 1.668 m.
- Mechanism: a hull or belt is `Contactor::RIGID` (`src/debris/crumble.rs:494-496`), so the piece may spend the whole `½ J v` on cracks (`:422-427`).
- Fragments under the 0.5 m floor are shed (`:234-237`, `:677-679`) and become fill (`src/rubble_bed/deposit.rs:88-178`).
- The grain is the baked chunk: 0.014625 m³, size 0.245 m (DERIVED from `assets/constructions/house_1/house_1.construction.ron`). A body needs about 9 bonded chunks (DERIVED, 0.125 / 0.0146).
- A 300 kg panel met at 5 m/s dissipates up to 3.75 kJ (DERIVED, ½·m·v²). At a recalled mortar fracture energy of 10–100 J/m² over a 0.056 m² joint, that pays for 600 or more joints (DERIVED, recalled input). The physics is plausible; the missing part is what the fill then is.

**P3. Timber disappears into masonry fill.**
- A whole roof board is about 0.2 × 0.05 × 6.6 m = 0.066 m³, size 0.40 m (DERIVED from the `east/board_0` hull). It is under the floor while intact.
- The floor's own text says "a long splinter counts as small" (`physical_laws.ron:9`).
- Bulk weight is one running mean (`src/rubble_bed/bearing.rs:345`), so a board becomes anonymous brick-coloured depth.

**P4. Fill volume caps the height near what was measured.**
- 14.70 m³ over the 4 × 6 m plan is 0.61 m mean; over the measured 56.5–71.5 m² footprints it is 0.21–0.26 m (DERIVED).
- A repose ridge along the roughly 20 m wall line: h = √(V·tan38°/20) = 0.76 m (DERIVED). Measured peaks are 0.78–0.96 m, so the bed already sits at its repose limit.
- One ideal cone of all of it would be 2.05 m (DERIVED). Walls stood 2.475 m, ridge 4.075 m.
- Angle is a weak lever: 45° instead of 38° raises that ridge 13% (DERIVED, √(1/0.781)).

**P5. Bearing planes anything a track touches to about 20°.**
- Capacity is stateless (`bearing.rs:167-188`): 0.5·γ·B·Nγ·shape·(1−tanβ)²·area.
- γ = 1845 × 0.60 × 9.81 = 10.86 kN/m³; Nγ(38°) = 78.0 (DERIVED from the code's form).
- For a 0.7 × 3.6 m belt patch (recalled fixture size), q is about 273 kPa (DERIVED). Fixture weight 57,000 kg (`assets/tiger_1/tiger_1.tank.ron:5`) over 5.04 m² is 111 kPa (DERIVED).
- So flat fill never gives, and the patch yields whenever (1−tanβ)² < 0.41, that is β > 20° (DERIVED).
- β is the steepest drop from any patch node to any non-patch neighbour, bare ground included (`:390-401`). A 9 cm lip over one 0.25 m node already yields (DERIVED), and a 0.25 m step has zero capacity (`:173`).
- Nothing firms up, so yielding ends only when the geometry is flat. Overload sinks at up to g·dt² = 2.4 mm per tick, 0.15 m/s (DERIVED, 64 Hz).

**P6. Displaced fill spreads like a displaced fluid.**
- Equal integer shares go to the nearest free ring in every direction (`bearing.rs:226-246`). There is no berm ahead of the mover.

**P7. Nothing in the pile can be touched.**
- Drawn chunks have no colliders (report, "Limits").
- Bed tiles are `Layer::Terrain` (`src/rubble_bed.rs:404`), so a piece landing on loose fill crumbles as on firm ground (`crumble.rs:527`).

## 2. Verdict on the five hypotheses

| # | Verdict | Evidence |
|---|---|---|
| 1 | Confirmed in magnitude, with one correction | P4. The limit is where the matter falls (a ring on the wall lines) and the lack of anything to bank against, not volume alone. |
| 2 | Confirmed, and stronger than stated | P1 and P2. In the smoke the tank converted nothing; it was all fill already. |
| 3 | Not established either way | Every reported collapse is authored: lines on every buried, anchored and bottom-band chunk (`src/headless_test/track_scorecard/rubble_climb.rs:438-470`), felling (`src/construction/breakage/felling.rs:1-4`), or "the existing workload". No number describes a ram. |
| 4 | Confirmed | One `u16` per node (`rubble_bed.rs:37`), one global bulk weight (`bearing.rs:345`), stateless capacity. The plan deferred compaction and mixed materials (plan lines 68-69). |
| 5 | Refuted as stated | `src/rubble_bed/flow.rs:20-74` has no time term. Its budget is 32,768 facet visits per tick (`flow.rs:4`) against about 970 nodes for a 60 m² bed (DERIVED), so repose completes within a tick. The fluid read comes from P5, P6 and a single angle with no hysteresis. |

On hypothesis 3, the play path has no whole-house trigger that I found. The hull's swept volume frees a patch (`src/construction/sweep.rs:1-5`), the part graph decides what stands (`src/construction/fracture/parts.rs:1-21`), and connectivity releases the rest (`src/construction/fracture.rs:1121`). So a ram's outcome is emergent. For this fixture the geometry predicts near-total loss anyway: a 4 × 6 m single cell with 0.25 m walls and 6.6 m boards spanning gable to gable, met by a hull about 3.7 m wide (recalled).

## 3. Options

Each item gives mechanism, what the player sees, cost, size and risk. Line counts are DERIVED from the prototype's module sizes: bearing 475, deposit 180, flow 88, wire 224.

**A. Two conserved classes per node (graded rubble).**
- **Mechanism:** store solid height of coarse and of fines, two `u16`, in place of depth. Depth is s_c/φ_c + s_f/φ_f. Deposits arrive as coarse at the chunk grain (0.245 m, a construction fact). Loose-coarse and compacted-fines packing and friction angle are material properties (Unknown; proposals 0.50 and 0.65).
- **Comminution law:** the work an overload does in sinking, (V−Q)·dz, pays for new surface at the seam's fracture energy, which is already in data. For cubes going from x₁ to x₂: W = G·6·(1/x₂ − 1/x₁) per m³ of solid.
- **Derivation:** 0.245 m to 0.02 m at a recalled 50 J/m² is 13.8 kJ/m³. A lane of 2 × 0.7 m over 0.6 m of coarse holds 0.42 m³ of solid per metre, so 5.8 kJ/m. The fixture's sinkage work is about 559 kN × 0.14 m / 3.6 m = 21.7 kJ/m. One pass can crush its own lane and costs roughly a 4% grade (all DERIVED, recalled G).
- **Player:** a blocky field; the first pass crunches down two lanes of fines; a light body crushes nothing.
- **Server:** about 160 node updates per tick under two belts (DERIVED, 5 m²/0.0625 × 2), against a measured bed mean of 0.03–0.25 ms.
- **Wire:** a tile snapshot goes from 2,048 to 4,096 bytes (DERIVED). A driven lane adds about 7 m²/s = 112 nodes/s × ~3 bytes ≈ 340 B/s (DERIVED), against a measured diff peak of 44,232 B/s.
- **Size:** about 350 lines (state and accessors 80, deposit 30, law 120, wire 60, tests 60).
- **Risk:** three Unknown inputs; depth is no longer the conserved integer, so every conservation test moves to solids.

**B. Compaction that changes capacity.**
- **Mechanism:** capacity reads the node's own classes. By the code's own form, Nγ(44°) = 224.6 against 78.0 at 38°, 2.9 times (DERIVED). With density, a firmed lane holds lips up to about 33° instead of 20° (DERIVED, (1−t)² < 0.12).
- **Sink:** d·(1 − 0.50/0.65) = 0.23·d, so 0.14 m for 0.6 m of coarse (DERIVED, proposed packings).
- **Player:** crunch, sink, then a firm road; the lane walls stay because 0.14 m over 0.25 m is 29°, under repose.
- **Cost:** nothing beyond A. About 60 lines inside `Shape::capacity` and its caller.
- **Risk:** a parked hull settled 1.5–6.2 cm in the fair test (DERIVED there); compaction should end that, unverified.

**C. Grain-scale relaxation floor.**
- **Mechanism:** a granular continuum has no slope below one grain. `STEP` is 0.25 m and the grain is 0.245 m, so a facet drop under (coarse share × grain) is exempt from repose at `flow.rs:29`. Fines keep the smooth law.
- **Player:** heaps hold 0.1–0.25 m blocky relief in the collider itself, so the ride and the view agree; crushed lanes are visibly smooth.
- **Cost:** none on server or wire. About 25 lines.
- **Risk:** relief depends on deposit order; the ride gets rougher; with P5 unchanged a track still planes it, so it needs B.

**D. Directional displacement.**
- **Mechanism:** displaced fill moves along the load's motion and away from its resultant, not in equal ring shares. This is the passive-pressure direction (Rankine Kp = tan²(45° + φ/2) = 4.2 at 38°, DERIVED).
- **Player:** berms beside lanes and a shoved heap ahead of a hull.
- **Cost:** about 60 lines in `Bed::spread`. No wire change.
- **Risk:** needs a per-patch velocity; berms may beach a hull again, which is the believable end of that range.

**E. Large elements persist.** Three truths are missing, none of them a rule for one object:
1. *Fill is a soft contactor.* Give bed tiles their own compliance from the bed's bearing stiffness. A piece on fill then shares a blow's energy by compliance, as moving groups already do (`crumble.rs:422-426`), and most of it goes into fill give. About 80 lines.
2. *Span decides breaking.* The landing law already balances presses and judges every joint (`crumble.rs:157-230`). With true bed reactions, a slab bridging a void cracks at midspan and a slab bedded flat does not. This follows from item 1.
3. *Size is what the player perceives.* A 6.6 m board is perceivable, but the cube root of volume says 0.40 m. This is the owner's stated taste, so it is a question for them: longest extent, or a per-substance grain so only matter that actually comminutes becomes fill.
- **Player:** the tank shoves or climbs a bedded panel, snaps a bridging one, and beams lie across the heap.
- **Server:** at most V/size³ bodies: 8.82/0.125 = 70 at the current floor, 17 at 0.8 m (DERIVED). Control's 39 bodies cost 2.07–2.31 ms against 1.28–1.49 ms for the bed (MEASURED, local debug).
- **Size:** about 200 lines with tests.
- **Risk:** long thin bodies are contact-heavy; beaching on a 0.5–0.6 m lump returns (fair test line 193) unless B lets the lump press into loose fill.

**F. Partial collapse.**
- It needs construction facts, not a law change, as far as the code shows: buildings several hull widths across, cross walls, and roof members spanning the short way.
- Fill already banks against anything standing. A solid over a node caps it (`bearing.rs:35-75`) and repose cannot export into a capped node (`flow.rs:38-44`).
- A 1 m stub retains a wedge of H²/(2·tanφ) = 0.64 m³ per metre (DERIVED).
- **Size:** bake-spec data plus one ram instrument reading standing chunks. `src/net/ram.rs:1077-1086` already counts absent chunks.
- **Risk:** larger constructions cost sweep and part-solve work; unmeasured.

**G. Rejoining.**
- Freeze already turns a slept island static (`src/debris/freeze.rs:9-18`), but any mover's reach thaws it whole (`:34-44`), after which the hull blow crumbles it.
- **Mechanism:** a settled piece becomes a placed construction held by rests at its contact patches. Rests are the push-only joints the part law already solves (`parts.rs:10-16`). The hull then meets it through the sweep's minimum cut against joint capacity, not a rigid-anvil blow.
- **Cost:** zero solver work at rest; the wire reuses the final pose `RetirePiece` already carries (`src/construction/breakage.rs:66-69`).
- **Size:** about 450 lines.
- **Risk:** the largest unknown here. I did not trace whether the sweep would shove such a piece or cut it into sub-floor chunks, and building a `Breakage` mid-match must satisfy ADR-0014.

**H. Rounds and bursts on the bed.**
- Blast rays already stop on the ground and carry a reflected impulse (`breakage.rs:18-29`).
- **Mechanism:** a node's fill column takes v = I/(ρ_bulk·depth), flies ballistically and deposits at its landing through the existing pending arrivals (`deposit.rs:82-86`). Integer volume is conserved.
- **Cost:** a 2 m burst touches about 200 nodes (DERIVED, π·r²/0.0625), about 600 bytes of diffs (DERIVED).
- **Size:** about 250 lines.
- **Risk:** the throw is invisible unless the view draws it; a burst under a parked hull needs the wake path.

**I. One source for drawn chunks.** The coarse class per node is what the view should draw as blocks, so sim and view cannot disagree and a crushed lane shows without a second channel. This is the interface to the view agent; it adds no sim cost beyond A.

## 4. Ranked top three

**1. Graded bed, firming and grain relief (A + B + C + I).** Recommended.
- It answers "sludge", "turns to dust" and "plastic" with one state change, and it gives the view agent its channel.
- Outline: change `BedTile` and `Bed` to two solids; update deposit; add the comminution law in `bearing::run`; make capacity per node; add the relaxation floor; bump the wire revision; move tests to solids.
- Size: about 435 lines (DERIVED, 350 + 60 + 25).

**2. The first plus persistent elements (E).**
- Soft bed contactor and compliance share, then put the size question to the owner. This restores the fair test's seven-panel, 2.95 m state as something a tank shoves and climbs.
- Size: about 635 lines (DERIVED).

**3. The second plus a shell and craters (F + H + D).**
- A larger multi-cell construction in data, the ram instrument, burst craters and directional berms.
- Size: about 950 lines plus bake data (DERIVED). G stays out until its sweep question is traced.

## 5. Cheapest experiment for the next drive

Build A's two solids on the server with only the one-way crush under an overloaded patch, add C's 25 lines, and tint the plain bed mesh by coarse share. About 230 lines (DERIVED), no new art.

In one drive the owner sees whether a blocky heap with two crushed, firm, smooth lanes reads as ruins, and feels whether crunch-then-firm reads as heavy. If it still reads as a pool, the answer is elements (E, F), not grading.

A free check before that: stop after a collapse and look before driving. The fair test says large panels should be lying there (7 bodies, 2.95 m). The smoke says none are. That one look settles which is true in play.

## 6. Decisions

- I recommend combination 1, with the section 5 experiment first.
- I treat the owner's size-over-mass floor as their call and raise P3 as a question, not a change.
- I did not design G beyond a sketch, because the sweep's response to a rested piece is untraced.

**Not verified:**
- No ram was run, so hypothesis 3 is open.
- Belt width, contact length and hull width are recalled, not read from data.
- The brick seam's fracture energy was not read.
- Fair test line 147 does not say whether its 1.5797 m³ is bulk or solid.
- Why the smoke had no bodies before the crossing while the fair test had seven is unexplained.

### Critical Files for Implementation
- /Users/Yan/github/vikng-dev/overmatch/src/rubble_bed.rs
- /Users/Yan/github/vikng-dev/overmatch/src/rubble_bed/bearing.rs
- /Users/Yan/github/vikng-dev/overmatch/src/rubble_bed/flow.rs
- /Users/Yan/github/vikng-dev/overmatch/src/rubble_bed/deposit.rs
- /Users/Yan/github/vikng-dev/overmatch/src/debris/crumble.rs
