# Burst on a brick wall: why the opening is small and what to change

Subject: the game's damage model for constructions (chunks, joints, loads, fill), game state only. The scenario fires the round `88mm_HE` from 8 m with its default `instant` impact fuze, aimed at the middle of `east/pier_3` (`lane/rubble-graded`: `src/rubble_lab/actions.rs` lines 240-265, `assets/dev/rubble_lab.ron` lines 13-15). Main's `src/` and `assets/` have not changed since the measured baseline `efe66d31` (the git log between them is empty).

**Short answer:** the 0.133 m² opening is exactly what the current laws predict. Two things cause it, and neither is a material number:
- The casing fragments are thrown from the fuze tip, which sits on the wall face. All their energy and push lands on one chunk, which turns to dust, and its push is dropped with it.
- The push from a burst is only judged where whole wall elements meet, spread over a whole tick. The joints around the chunks that were hit are never judged under it.

## 1. The path on main
1. **Where the burst sits.** The impact fuze fires on contact, so the tip is on the face (MEASURED tip x = 2.000). `burst_here` (`src/ballistics.rs:3853`) records the field origin at `Burst::filler_center` (`burst.rs:129`). That point is 0.178 m outside the face (MEASURED 2.178; the datum is `filler_setback`). The burst is not inside the wall and not past it. Afterwards the nose piece (4.41 kg, DERIVED) carries on as the round (`ballistics.rs:3890`) and crosses both walls (MEASURED 227 kJ and 224 kJ left in them).
2. **One burst, two field origins.**
   - The burst sphere (airblast) starts at the filler centre (`breakage.rs:1177-1190`).
   - The casing ring starts at the tip (`ballistics.rs:3995`, `origin: flight.position`). The ring is a band at atan(v_s/v) ± 10°, i.e. 44°-64° ahead of the flight axis (`spall_cone.rs:32-33`).
   - A field ray stops where it enters the first volume, and a ray that starts inside a volume stops at its own origin (`resolve.rs:1265-1267`). So every casing ray stops at the tip, on the struck chunk.
3. **The airblast law.** It uses Swisdak's Kingery-Bulmash reflected and incident fits read at W/1.8, the ConWep incidence blend, and a detonation-products floor of p/(2πR²) (`fracture.rs:1473-1536`). It is traced with 4096 rays to the nearest standing chunk, and each chunk keeps the share of a ray's cone its silhouette intercepts (`fracture.rs:1791-1848`). The ground and static surfaces only block rays (`breakage.rs:2258-2259`). The header's "stops on the ground" therefore carries no reflected push; that is verified.
4. **What becomes a load.**
   - Airblast faces become `Load::blast` with zero energy (`fracture.rs:944-1008`). Two laws read them: Law 1 crushes the face (millimetres here), and Law 2 throws a rear slab (scab). The scab's volume leaves the chunk, but its momentum leaves the game, because `breakage.rs:2059` discards it.
   - Casing rays become `Load::field`. They crush energy / 135 MJ/m³ of brick and push with p = 2E/v_s along the ray (`breakage.rs:820-845`).
   - The nose piece becomes `Load::line`.
5. **Which joints part.**
   - (a) `crack` (`fracture.rs:1080-1115`) uses a cavity-stress field (Lamé) from loads that carry energy, which means only the casing and the nose line. Airblast loads never cause these cracks.
   - (b) The part law treats each wall element as one rigid node. A push enters as J/step, capped by its peak force (`parts.rs:1235-1253`). A joint between elements cracks only if that field beats its strength and the elastic work it stores is at least ΣG·A (`parts.rs:1621-1694`).
   - Joints inside an element never feel a push. The chunk-scale solver runs only for the gravity preload, for landings, and in `holes` after a removal, and `holes` reads sustained loads only (`parts.rs:1508-1567`).
6. **Removal happens before the verdict.** `strike` wears chunk volume first. A chunk turned to dust has its loads filtered out before the part law runs (`breakage.rs:2409-2421`), so its push leaves the simulation.
7. **Speed of released chunks.** A released group leaves with its pushes, less the fracture work of the joints it broke (`debit_work`, `fracture.rs:1251-1303`; `breakage.rs:2500-2543`). Spin comes from the angular impulse.
8. **Body, drawn piece, or dust.** Small fragments are first merged across joints parted in this event, then routed by size (`crumble.rs:232-312, 748-780`; `breakage.rs:1693, 1728`).
   - A group with cube-root volume of 0.5 m or more stays a server body (`server_rubble_min_size_m`, a Taste).
   - Anything smaller is recorded as dust plus a view-only shed: a drawn piece flying with its launch motion.
   - One chunk is 0.0146 m³ (cube root 0.24 m), so every single chunk the burst frees is a view-only shed. Main has no fill; the bed/fill model is on the lab lane only.

## 2. Numbers, and what the laws as written predict
**Round (DERIVED, `burst.rs:141-245`):**

| Quantity | Value |
|---|---|
| Cavity length | 0.171 m |
| Casing mass M | 3.68 kg |
| Casing sideways speed v_s | 1124 m/s |
| Casing energy | 2.33 MJ |
| Core chemical energy | 3.80 MJ |
| Airblast energy | 1.47 MJ, W = 0.351 kg TNT, fits read at 0.195 kg |
| Fit scale | 0.580 m/kg^⅓ |
| Detonation-products momentum | 680 N·s |
| Charge radius | 0.051 m |
| Airblast reach | 23.2 m |

**Brick row and chunks:**
- From data: density 1845 kg/m³, joint tensile strength 0.69 MPa, shear cohesion 0.165 MPa, fracture energies G_I 10 and G_II 28.4 J/m², stiffness E 8.6 GPa.
- DERIVED: crush energy 135 MJ/m³ (0.0365 × the steel reference's 3.71 GPa cavity strength); dynamic compressive strength 86 MPa.
- Chunks run through the full 0.25 m thickness with 0.26 × 0.225 m faces (0.0585 m²) and weigh 15-37 kg (typically 27 kg). The wall carries 461 kg per m² of face.

**Airblast on the face (DERIVED):**
- At the aim point: Z = 0.31, specific impulse 3.4 kPa·s, peak pressure 109 MPa.
- It falls to 0.70 kPa·s at 0.2 m and 0.12 kPa·s at 0.5 m out.
- Total push: 190 N·s within 0.2 m, 350 N·s within 0.5 m.
- Law 1 crushes about 4 mm deep, inside r ≈ 0.08 m.

**Casing (DERIVED):** 2.33 MJ and 2436 N·s pushing into the wall (4140 N·s scalar × cos 54°), all at the tip. That is seven times the airblast's push within 0.5 m.

**Today, predicted (DERIVED) against measured:**
- The casing alone crushes 0.0172 m³, which is 1.17 times the struck chunk's volume, so that chunk becomes dust (MEASURED: 1 chunk disintegrated). Its 2436 N·s, the nose piece's 289 N·s and about 120 N·s of airblast leave with the dust.
- The Lamé field of 2.33 MJ at one point beats 0.69 MPa out to 0.74 m. It cracks only joints whose plane contains the direction to the burst, so the cracks run outward from it and do not ring it (MEASURED: 15 cracks reaching 0.69 m; the picture shows outward lines).
- The neighbouring chunks take 22-40 N·s of airblast each. That reaches the 1.3 t pier as 1.4-2.6 kN of force, and nothing inside the pier parts.
- `holes` then frees the chunk beside the window opening (0.0743 m²).
- Opening: 0.0585 + 0.0743 = 0.133 m² DERIVED, against 0.13275 m² MEASURED.

**What limits the opening, in rank order:**
1. **The ring is thrown from the tip.** One chunk takes all of the casing's energy and push, turns to dust, and the push is dropped with it.
2. **The push is read as a tick-mean force.** At J/dt, freeing one chunk in shear needs about 620 N·s on that chunk (0.24 m² × 0.165 MPa ÷ 64). That is more than the whole airblast. The stored elastic work also under-reads the energy of a push by (ω·dt)² ≈ 3e4 to 8e4 (DERIVED, with ω = √(K/m) ≈ 1.1e4 to 1.8e4 rad/s for a 27 kg chunk). The tick is a solver constant doing a law's job.
3. **A single-point Lamé source only cracks outward,** so nothing frees a plug.

**What does not limit it:**
- There is no energy budget and no per-event cap (`HOLES_PER_CHUNK` only affects drawing).
- Ray count is ample: 100-600 rays per chunk face within 0.3 m.
- The distance falloff is the fits' own.
- The round bursts on contact; it does not pass through first.
- No shielding: the trace sees the intact wall.

**Stale claim:** the design doc's Law 3 "Built (lane 2)" (`construction-destruction.md:260-261`) names `plastic_demand`, `strength_ratio` and `section_pulse`. None of them exists on main; they were removed in `5dcf2868`.

## 3. Authored numbers on the path
| Number | Label | Finding |
|---|---|---|
| Tick 1/64 s | Resolution | The push verdict reads J/dt, so a solver constant does a law's job (the design's Limits section at line 940 admits this). |
| Brick `resistance` 36.5 | StandIn (penetration) | Also sets the 135 MJ/m³ crush energy, which sizes fragment and line removal and the Lamé clamp. Two jobs. |
| Brick `fracture_energy` 10, `tensile_dif` 3.1 | Measured("CLAIMED, not read") | The label claims a measurement the text says was not read; should be Unknown. |
| Brick `shear_modulus` 3.44e9 | Unknown, with StandIn text | Label and text disagree. |
| Brick `compressive_dif` 2.357 | StandIn | A fit at an assumed 100/s, applied at every rate. |
| `surface_burst_factor` 1.8 | StandIn | A claimed fact labelled as a stand-in. |
| `GRAZING_COS` 0.1 | "ASSUMED, Resolution" | Changes the law's patch at grazing angles: a law floor labelled as resolution. |
| `base_mass` 0.3, `cavity_diameter` 0.065 | Unknown | Together they derive a 4.41 kg nose (47 % of the round) that the ring law sends through two walls. |
| Amatol density, speed constant, energy | Unknown | These set v_s and the casing/airblast split. |
| `filler_setback` 0.178 | Derived (assumed geometry) | Today it is only the airblast's origin. |
| `ring_half_width` 10° | Unknown | Fine. |
| Casing push p = 2E/v_s | Law, documented | The casing's real forward momentum is M·v = 3014 N·s, against 2436 applied (DERIVED). |
| `server_rubble_min_size_m` 0.5 | Taste | Through the merge rule, it also decides a physical grouping. |
| `BLAST_RAYS`, `LOAD_TOLERANCE`, `LOOSE_GAP_M` | Resolution | Fine. |
| `burst_flash_scale` 0.02 | Taste on ∛energy | Fine. |

## 4. Believable range from first principles (decided as a range, not pinned)
- **Push into the wall.** Fragments leaving at 44°-64° from 0.18 m out land on a ring of radius 0.17-0.37 m. Together with the airblast that is about 2.7-3.3 kN·s within r ≈ 0.4 m (DERIVED).
- **Wall speed.** A masonry disc of radius R carrying that push moves at 2750/(461·π·R²): 9.7 m/s at R = 0.45 m, 3.4 m/s at 0.75 m, 1.9 m/s at 1.0 m.
- **When a chunk parts.** A chunk parts from its neighbour when ½·m_red·Δv² exceeds the joint's elastic energy at its strength plus G·A. For these chunks that is 0.94 J in tension and 1.65 J in shear, i.e. Δv of 0.26-0.5 m/s (DERIVED).
- **The load is a sharp blow, not a slow push.** The pulse lasts τ = 2i/P ≈ 63 µs. That is shorter than a quarter of the chunk's vibration period on its joints (about 0.6 ms).
- **Cross-check (recalled):** the field-manual breaching rule P = R³KC (feet, K ≈ 0.35 for masonry, C ≈ 3.5 untamped) puts about 0.3 kg of TNT at a breach radius equal to a 0.25 m wall. The airblast alone sits at that threshold, before any fragments.

**Believable outcome:**
- An opening of 0.2-1.0 m² (about 0.5-1.1 m across), i.e. 4-17 chunks of this house's size.
- Pieces from the centre thrown into the room at 5-20 m/s; pieces from the rim at 1-3 m/s or dropping.
- Cracks out to 0.5-1.0 m from the aim point; the wall stands.

**Absurd ends:**
- One or two chunks gone with nothing thrown. Today's result sits at this end.
- The whole pier gone (2.9 m², from grade to the wall plate) or the whole wall face (10.8 m²).
- A 27 kg chunk leaving faster than about 30 m/s, which would put more than a third of the burst's push on one chunk.

## 5. Design (generic laws and existing data; nothing per tank, per round or per object)
**Change 1: one origin for both burst fields (the missing fact is where the casing is).**
- `throw_burst` throws the ring from `burst.filler_center(...)`, the point the airblast already uses, and the inside-a-tank (`holding_box`) check reads the same point.
- On the real chunk grid (DERIVED): the ring lands at r 0.17-0.37 m. Six chunks take 320-410 kJ and 330-430 N·s each, which crushes 13-19 % of each and turns none to dust.
- Risk: the hit-point side moves too. Contact bursts on armour spread over a ring instead of a point, and the recorded pen-cam ring scenes (s13, s16) change. Those are observations, not obligations. The armour design's §5a owner reviews containment for a burst just past a plate.

**Change 2: judge pushes at chunk scale, as an impulsive load (the missing law term; it restores the design's own Law 3).**
- For each chunk taking a push: v = J/m (scab momentum is already subtracted).
- Each intact bonded joint of that chunk cracks when ½·m_red·|Δv|² ≥ F_c²/(2K) + G·A in its mode:
  - normal: against tensile strength, normal stiffness K_n and G_I;
  - shear: against cohesion plus friction, shear stiffness K_s and G_II.
  - A side with no push, or the ground, counts as at rest.
- The strength check and the energy check collapse into this one inequality, since F = Δv·√(K·m_red).
- Inputs: chunk masses, joint areas, stiffnesses from `bondsolve::joint_edge`, `JointStrength` and the seam fracture energies. There is no new number and no tick.
- It runs in `run_laws` after `strike` and before `advance`. Its cracks reach the existing paths: unbonded joints, slide or mechanism, release, and `debit_work` charging the cracked joints' G·A.
- `bonded_verdict` reads its pulse the same way, using whole-part velocities J/m_part. The J/step pulse column is deleted, so there is one path. Contact handling is unchanged.
- Cheap early exit (DERIVED): a push below √(2·m·G·A) ≈ 5 N·s on a brick chunk cannot crack anything.
- Side effect: a kinetic round's line will now throw the chunks it crosses, because a line's push is the momentum the round lost.
- Rejected:
  - Running the chunk solver with an equivalent force J·ω: about 1 M operations per part, and still a quasi-static reading.
  - A potency multiplier: a taste applied to a law.
  - Weaker brick: the data is sourced.
  - A shorter tick: a solver constant doing a law's job.

**Change 3: the merge rule reads separation.** `merge_fragments` joins two fragments across a joint parted this event only when their launch velocities differ by less than that joint's Δv_c from the Change 2 law. Without this, chunks thrown apart at different speeds fly as one rigid slab. The launch velocities have to be computed before the merge.

**Expected result with changes 1 + 2 + 3, on this aim (DERIVED):**
- Opening 0.45-0.9 m² (7-15 chunks), the wall stands.
- The struck chunk becomes dust through the scab, or is thrown at about 6 m/s.
- Six chunks are thrown into the room at 10-18 m/s.
- Two to six more leave at 1-2.5 m/s and drop inward.
- Cracks remain standing 0.45-0.7 m from the aim point, both outward and around the opening.
- Change 1 alone: 0.1-0.5 m². Chunks leave only where the Lamé fields (0.39 m reach per 350 kJ) happen to cut all of a chunk's joints.

**What becomes of released chunks.** Each freed chunk is a view-only shed, released in the burst's own tick. That is a stand-in the player cannot tell apart inside a room. A chunk turned to dust leaves as a view-only plume carrying its push (see the view list below).

**Server cost.**
- The verdict is a closed form per joint: about 20 loaded chunks × 6 joints (DERIVED). That is negligible next to one chunk-solver factorisation of about 1 M operations (the design gives 985,830 for the house's largest element).
- Server bodies added: 0 (with Change 3, no merged group reaches 0.125 m³).
- Network per burst: about 10 shed messages, dust entries and around 50 crack entries.
- Unmeasured candidates for the "spikes at initial destruction":
  - the blast trace: 4096 rays × each construction within 23.2 m, plus 4096 engine ray casts;
  - the part-graph refactorisation (351k operations for the house);
  - repeated `holes` passes within one tick;
  - element-to-chunk collider swaps on first touch.
- How to bound it by what the player notices least:
  - Never delay the release at the point struck.
  - If the burst tick is over budget, let `holes` run one pass per tick. Secondary loosening arriving 16-50 ms later is invisible.
  - A separate lever: a law-derived reach, where a ray stops once its push cannot move a 27 kg chunk past the 0.15 m/s rest speed. That is about 3.0 m against 23.2 m, roughly 60× less area (DERIVED).

**View, a separate list.**
- Exists today:
  - burst flash: 0.02 × ∛E, about 2.3 m across for 0.04 s (`vfx/impact.rs:369-403`);
  - per-material impact dust puff and powder colour;
  - drawn crack lines and crater dishes;
  - view-only sheds (cap 256);
  - the gun's report at the muzzle.
- Missing (each scaled from a physics quantity with one taste scalar):
  1. A sound at the burst point; `assets/sfx` holds only cannon reports, the click and the engine. Loudness from airblast energy.
  2. A dust plume sized from the removed mass (dust + crushed + scab), ∝ ∛mass like the flash. Today a dusted chunk simply vanishes.
  3. Spray from the back face: the scab's mass and speed as view-only pieces.
  4. A thrown chunk breaking into brick-size pieces according to how much of it was crushed.
  5. Masonry break and landing sounds, from fracture work and landing energy.

**Build outline (DERIVED: the listed edits, sized against comparable code in the same files):**
1. Ring origin and containment point: about 5 lines of code, 10 of doc, 30 of test (about 45).
2. Impulsive verdict: about 70 lines (`bonded_verdict` is 74), 10 of wiring, 20 replacing the pulse column, 25 of doc, 110 of tests. The tests cover the 0.98/1.02 gates, tick independence at 1/64 versus 1/128, and an inner chunk cracking only its own joints (about 235).
3. Merge reads separation: 25 lines in `crumble.rs`, 15 of reordering in `breakage.rs`, 40 of test (about 80).
4. Design doc: replace the stale Law 3 text, add the ring origin, update Limits (about 30).
5. Lab-lane metrics: the burst tick's maximum time and the cost-stage counters (about 20).
- Total about 410 lines. The view items come to about 300 more.

**Cheapest before-and-after on this wall.** Run from a leased bay on `lane/rubble-graded` (the lab is not on main), with the same seed 24301:
```sh
scripts/rubble-lab today-build main target/rubble-lab/today
scripts/rubble-lab run burst target/rubble-lab/b-today --clips --today target/rubble-lab/today
scripts/rubble-lab run burst target/rubble-lab/b-ring --clips   # arm A: change 1, one line
scripts/rubble-lab run burst target/rubble-lab/b-full --clips   # arm B: changes 1+2+3
scripts/rubble-lab compare target/rubble-lab/b-today target/rubble-lab/b-full target/rubble-lab/b-cmp
```

| `direct_burst` / principles | Today (MEASURED) | Arm A (DERIVED) | Arm B (DERIVED) |
|---|---|---|---|
| `wall_face_removed_m2` | 0.133 | 0.1-0.5 | 0.45-0.9 |
| Brick chunks lost | 2 | 2-8 | 7-15 |
| `burst_chunks_flew` | 3 | 1-6 | 6-12 |
| `disintegrated_without_shed` | 1 | 0-1 | 0-1 |
| Farthest crack from aim, m | 0.69 | 0.5-0.7 | 0.5-0.9 |

Also compare `cracked_standing_joints`, `local_p99_tick_ms` (5.84 ms today, MEASURED locally, not hosted), the new burst-tick maximum, the close and side clips, and the thrown pieces.

## Decisions
- I chose changes 1 + 2 + 3, plus the view list, and judge them against the believable range.
- Arm A runs first because it is one line.
- The `holes` deferral and the derived reach wait until the burst tick's cost has been measured.
- No material, fit, ray-count or tick change.

## What I could not verify
- That the casing rays really stop at the tip in the running game. I inferred it from `resolve.rs` and it matches both the crack reach and the one chunk turned to dust, but there is no ray dump.
- Which joint each of the 15 cracks is, and why 6 timber pieces were released.
- Whether the scab takes the whole struck chunk.
- Whether the ring origin moves armour containment or the recorded hit-point fixtures.
- The milliseconds per operation on the 2-core host.
- The recalled breaching rule.

### Critical Files for Implementation
- /Users/Yan/github/vikng-dev/overmatch/src/ballistics.rs
- /Users/Yan/github/vikng-dev/overmatch/src/construction/breakage.rs
- /Users/Yan/github/vikng-dev/overmatch/src/construction/fracture/parts.rs
- /Users/Yan/github/vikng-dev/overmatch/src/debris/crumble.rs
- /Users/Yan/github/vikng-dev/overmatch/.agents/docs/design/construction-destruction.md
