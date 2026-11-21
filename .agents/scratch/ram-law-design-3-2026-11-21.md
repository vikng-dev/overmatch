# Ram law, design round 3

Subject: the game's building model (game state only): chunks joined by joints with strengths, loose
pieces, and contact loads read back from the physics solver, when a heavy vehicle drives into a
wall. Nothing was built or run. Code citations are `main` unless `lane/ram-a` is named. MEASURED
numbers are from `lane/ram-a:.agents/scratch/ram-a-experiment.md` or, where a run is named, read
this round from that run's `observations.json` / `run.log` under
`.claude/worktrees/bay-1/target/ram-a/`. DERIVED numbers show their arithmetic. The vehicle is the
fixture (57 t), never the spec.

## 1. Premise check

**The law, once.** Jointed matter can take from a body only what its joints can carry:
- a **force**, up to the cut's capacity, for as long as something keeps applying it (weights and
  drives, known before the solve);
- a **motion**, up to each joint's strength over its impedance (ρ·c·v ≤ strength, the plane-wave
  relation): a joint asked to pass more parts before its neighbour moves.

So a body's momentum is judged **once, before the solve, with everything that moves with it**: the
matter its motion cannot be passed through leaves with it in one plastic exchange, or the stop is
real. Nothing the solver does afterwards is a load: after the solve only persisting forces are read.

**Is it one statement or three truths?** One law, and it needs two sites, not three plus `main`'s
four readings. Two because force and momentum are two quantities (the premise check's finding,
which stands) and because one is read before the solve and one after. The three arms are three
sites only because the judgement itself was never replaced:

| Arm | What it really is | Why it had to be an add-on |
|---|---|---|
| P, `lane/ram-a:src/construction/sweep.rs:376-389` | the motion half, one joint deep | it sits inside the Hertz gate and the static cut it should replace |
| A, `lane/ram-a:src/construction/breakage.rs:927-945`, `sweep_intake.rs:363-388` | the force half, correct as stated | none: this is the law's second site |
| B, `lane/ram-a:sweep_intake.rs:254-273`, `:1245-1252` | "with everything that moves with it" | `train_mass` exists only to feed a train into the Hertz mass |

**P is wrong as built, and the records show it.** P reads the sliding speed of the *struck chunk's
own material seam*, not of the joints around the patch. For a timber chunk that is
8.9 MPa / √(448 kg/m³ × 47.3 MPa) = 61 m/s (DERIVED from the Wood row,
`assets/materials/materials.ron:379-451`), so P never engages on timber and the static cut's
whole source side still leaves. The joints that actually hold a roof are `nailed`, 3.24 kPa
(`assets/materials/attachments.ron:55`, `:65`): they pass motion below
3,240 / 145,600 = 0.022 m/s (DERIVED). MEASURED consequences:
- `B-hall-1`, tick 1661 (0.31 s after contact): one sweep frees 116 chunks, leaving as one piece of
  115 chunks, 6,756 kg, at 0.62 m/s; the hull has no verdict and no solver impulse that tick. At
  58.7 kg a chunk (DERIVED) it is roof timber: 87 % of the roof's 7.8 t (DERIVED from the spec's
  member sizes at 448 kg/m³).
- `B-long-1`, tick 1653 (0.20 s): the hull's verdict (10.6 MN against 1.03 MN) frees 183 chunks in
  one tick: the roof, plates and gable tops.
- `diag/Adiag/hall-1/run.log`, tick 351: a 56 kg piece at 1.38 m/s, demand 27,262 N against a cut
  of 27,153 N, frees 53 chunks. A cut that costs 27 kN in newtons costs a 56 kg board nothing to
  move: design round 1's fault 2, still live.

**Smallest form.**

*Site 1, momentum, before the solve: `Geometry::resolve` (`src/construction/sweep.rs:344-393`).*
Reads: each swept chunk's own witness (already kept per target, `sweep.rs:659`), the joints
(`JointStrength::limits`, `bondsolve.rs:459`; `bearing`, `fracture.rs:269`), the receiving chunk's
density, shear modulus and contact compliance, the striker as an exchange `Body` (its train
included). No new data field, no code constant.
1. Flood from each swept chunk across every live joint that passes its motion: normal traction
   √(ρE′)·v_n within [−tensile, bearing], shear traction √(ρG)·v_t within cohesion plus friction on
   the compression present (exact constants of the law's form).
2. If the flood reaches the ground or uncovered matter, the structure answers as a whole: today's
   static cut with the drive alone as demand (a crawl still fails a panel; a buried core is a real
   stop).
3. Otherwise the flooded set is the plug. It leaves if the plastic exchange's loss
   (`Exchange::dissipated`, `sweep.rs:824`) pays its perimeter's crack work (`crack_work`,
   `crumble.rs:78`); else it is held: a light body really does stop.

*Site 2, force, after the solve: one `persisting` reading (arm A, restated).* A body presses
standing matter, and pushes another body, with no more than the forces acting on it along that
contact: weight, drives, and what its pushers pass on under the same bound. The ground and any body
of no finite mass act with none: a reaction is the stop itself (MEASURED: A's first build, without
this, put 2.0-6.3 MN into the part law and dropped the hall to 12 standing). `read_contacts`
(`breakage.rs:880`, the `/ dt` press at `:1066`) and `pushes` (`sweep_intake.rs:313-339`) both read
it; the solver supplies only where the contact is.

*What moves with a body* (arm B's gather, kept): bodies that pressed it along its motion last step
and still move along it. They enter site 1 as the striker `Body` (combined mass at its momentum's
velocity) and share the exchange by mass.

**Each site on `main` becomes:**
| Site | Becomes |
|---|---|
| Hertz demand, `sweep.rs:363-364`; `PatchLoad::threshold`, `:389`, `:653`; `SweepSource::effective_mass`, `sweep_intake.rs:213-236`; `contact_law::peak_speed` | deleted. `peak_force` and `Contactor::RIGID` stay for landings (`crumble.rs:527`, `:573`) |
| Static cut `plug`, `sweep.rs:400-456` | kept for step 2 of site 1 only; deletion is plan step 5 |
| Held gate, `sweep_intake.rs:65-91` | unchanged code; "held" now means a judged real stop |
| `read_contacts` whole transfer / dt | the persisting force over the solver's points |
| `pushes` StandIn | the same persisting bound per (receiver, pusher) pair; one fold also yields the carriers |
| `crumble::landings`, `crumble.rs:402-424` | unchanged: it already splits closing momentum from the held rest |
| Arms P, A's code, B's `train_mass` | not merged; rebuilt as the two sites above |

**The open faults, by derivation.**
- *Pushed piece (same disease).* A 563 kg piece pushed at 4.58 m/s was judged alone, held, and
  stopped the hull through it (−116, −173, −99 kN·s, MEASURED round 1). Judged as its train
  (57.56 t) it frees the panels in its path: about 1.3 t, so 4.58 × 1.3 / 58.9 = 0.10 m/s lost
  (DERIVED). B already measures this: hall worst tick 2.45 → 0.68 m/s.
- *The hull's held verdict on side-wall ends (same disease).* `B-ram-1`, tick 1715: demand =
  capacity = 2.34 MN, then −274 kN·s from standing matter = 57,000 × 4.81 m/s (MEASURED). A Hertz
  peak of 2.34 MN for 57 t is a closing speed of about 0.45 m/s (DERIVED: (2.34 / 73.5)^(5/3) of
  m·v² at 8 m/s): the verdict judged one witness's normal and the solver removed ten times that
  motion. Under site 1 every swept chunk is seeded with its own witness, and a wall met end-on
  passes the motion along its length (bearing passes below 8.9 m/s) but not into its bed joints
  (0.065 m/s): the wall's strip leaves as a plug. Upper bound, the whole 6 m wall above grade,
  6.9 t: 4.5 × 6.9 / 63.9 = 0.49 m/s in the tick (DERIVED), against 4.79 MEASURED. Not verified:
  which contact points stopped the hull; step 2's report replays this tick.
- *The 21 t one-tick stop (same disease, a third place).* `B-passes-1`, tick 2282: −778.6 kN·s
  = 57,000 × 13.66 m/s, all of it from a body retired within the tick (`retired_ns`, MEASURED): the
  solver stopped the hull on the piece held rigid against the ground, and the landing law broke the
  piece afterwards. It is the same order fault: judged after the solve. The sweep admits standing
  owners only (`Owners … Without<Piece>`, `sweep_intake.rs:238-248`). Site 1's fast branch needs
  no ground, so it applies to a lying piece unchanged; a slow or light meeting is the solver's
  ordinary contact (the piece moves as a body). A swept strip of 20 chunks at 23.6 kg (21.3 t /
  902): 10.63 × 0.47 / 57.5 = 0.09 m/s a tick (DERIVED). Plan step 4.
- *The board on the deck: no fault.* A loose board resting on a flat deck under friction is what
  the laws say. A rule to shed it would be a per-object rule. Owner question 2.
- *The rafter ties: construction data.* 26 joints at up to 1.04 × capacity under gravity
  (MEASURED, `Adiag` tick 1). `nailed` spreads two skew nails per 0.6 m of plate over the contact
  area (10.7 nails/m²) and its shear is an Unknown StandIn; its own comment names a densely nailed
  joint as "a kind of its own once play shows it needs one". A lapped member joint needs its own
  attachment row (nail count over the lap's area, lateral capacity). No missing law: a joint stays
  a bond over its area. Unknown until a lateral nail capacity is read from a source.

## 2. Is "half must stand" a sound bar?

No. It is one number for two geometries, counted in chunks (a 59 kg roof chunk equals a 430 kg
pier), and the lab counts a roof resting in its shell as fallen. Expectations, DERIVED from the
specs (`tools/constructions/house_1.house.ron:11-36`; `lane/ram-a:tools/constructions/ruins_hall.house.ron`):

**House (4 × 6 m, both 6 m walls crossed, hull 3.76 m, eaves 2.475 m).**
- In the path: 3.76 to 4.16 m of each 6 m wall, full height, and nothing bridges it: 63-69 % of
  both long walls. The two 4 m walls are 1.12 m clear of the tracks and should stand.
- Roof: boards only, laid across the path, no rafters; the eaves are below a 3 m turret (vehicle
  height not checked). The roof is unbraced and mostly in the way: anything from "upper boards stay
  on the gables" to "all of it in the shell" is believable. Gable tops with no roof: either.
- Range: below-eaves masonry at the geometric ceiling is 433 of 727 chunks (MEASURED under B: piers
  395/573, sills 28/98, lintels 10/56). With roof, plates and gable tops all gone: 433 / 962 =
  45 %. With gable tops and off-path boards kept: about 60 %. Absurd: under about 40 % (a wall
  beside the path falls: today's 6 %) or over about 62 % (matter in the path stands).
- **B's 45 % is inside, at the floor.** Its walls are right. Its roof and gable tops left in one
  tick through the static cut (tick 1653): the right end state here for the wrong reason.

**Hall (12 × 16 m, both 16 m walls crossed mid-room, cross walls at ±3 m carrying the ridge).**
- In the path: 4.2 to 6.3 m of each 16 m wall, 8 to 13 m of about 76 m of wall line: 11-17 %.
- Roof: rafter pairs at 1 m on the long walls' plates, ridge on four gables. Each breach takes the
  feet of about five pairs in the middle room: that bay sags or drops. The end rooms' rafters
  (10 of 16 m) stand on unbroken walls.
- Range: masonry 290-320 of 361, roof members 85-154 of 154: about 70-90 % of 515. Absurd: under
  about 55 % (an end-room roof or a wall outside the path falls) or over about 92 %.
- **B's 34 % is outside, on the absurd side.** Piers stand 144 of 280 where the path explains a
  loss of at most 17 %. What does it: tick 1661, the roof freed as one 6.76 t rigid piece by the
  static cut (wrong law), on `nailed` joints 50-100 times under a member joint (data). In A's diag
  run the same roof then levers the gables through the ridge (gable-ridge tip, ridge-ridge at
  2,939 × capacity, ticks 347-353). That the off-path piers follow from the loose roof is an
  inference from the timeline, not verified: B has no diag run.

**Replace the bar with:** matter in the swept outline is gone; matter outside it falls only if its
support left. The lab reports off-path loss by part and by mass, and counts a roof at rest on
standing walls as standing. Percent standing stays an observation.

## 3. The build for `main`

One lane (`lane/ram-law`, from `main`), one commit per step, measured after each on a sibling lane
that merges the lab (as `lane/lab-on-belt-main` did; the lab does not land). Line estimates are
DERIVED from the lane's measured arms or named analogues; the last three estimates of this kind ran
2.2 times over, so read each "added" as up to 2.2 times. Wire: no replicated type changes in any
step; the sweep is server-only (`sweep_intake.rs:62-70`); `PROTOCOL_REV` untouched. Plug sizes and
wire bytes will move: observations for the report.

**Step 0. The thaw order (a `main` bug, independent).** `src/debris/freeze.rs:947` sorts thawing
pieces by entity; ids vary between runs of one scene. Take `8a7ab552`'s change and its test
`thaw_order_follows_no_entity_order`. MEASURED +51 −9. Deletes nothing.

**Step 1. Force channel.** `src/construction/breakage.rs` (`read_contacts`, `prepare_contacts`),
`src/construction/breakage/sweep_intake.rs` (`pushes`, `sources`, `SweepAccount::persisting`).
MEASURED on the lane: +79 −12. Deletes the whole-transfer `/ dt` press and the unbounded push.
- Parallel: the per-body bound is one serial fold over the step's points before the per-owner
  conversion tasks (data dependency: a body's press sums over every construction it touches);
  `pushes` keeps its batches and folds pairs in key order. Loads commit as now, in source order.
- Tests: `a_stop_presses_with_no_more_than_the_force_behind_it` (a body the solver stops on
  standing matter queues no press beyond its persisting force along the contact; the part law
  relies on a press being sustained; mutant: `impulse / dt`). `a_reaction_is_no_push` (a piece
  between a static body and a wall is pushed with nothing; mutant: A's first build, which
  dropped the hall).
- `no_bodys_kind_enters_a_contacts_loads` (`breakage/tests.rs:2095`): keep its first invariant
  (a piece's loads equal a plain body's: no body-kind gate); delete the assertion at `:2161-2167`
  that loads sum to the whole momentum change: that is the deleted law, and the first new test
  carries its replacement.

**Step 2. The judgement.** `src/construction/sweep.rs` (`resolve`, a flood beside `cut`),
`sweep_intake.rs` (`resolve_patches` passes the striker `Body`), `src/contact_law.rs`,
`src/net/ram.rs:1271` (a print). Added about 95 (flood and joint test 50: analogue `Geometry::cut`
20 plus `plug`'s joint loop 30; energy gate 15: crumble's budget; impedances 10; signature 20).
Removed about 70 (Hertz in `resolve`, `threshold`, `effective_mass`, `peak_speed`).
- Parallel: inside `resolve_patches`' existing per-(source, owner) tasks, reading structure, state
  and preload immutably with its own scratch. Flood order is chunk index over authored incident
  lists; no hash order. Plugs commit in (body, owner) order as now (`sweep_intake.rs:1279`).
- Tests: `a_joint_passes_motion_only_below_its_strength_over_impedance` (a fast block frees its
  swept patch and the bearing column behind it, never the cut's far side; bounds read from the
  material row; mutant: the static cut at speed). `a_weak_fixing_carries_no_neighbour_along` (two
  stiff members on a weak attachment: striking one leaves the other; mutant: P's struck-chunk
  seam, the roof fault). `a_strike_that_cannot_pay_its_cracks_is_held` (mutant: no energy gate).
  `crawl_drive_and_stronger_material_follow_the_cut_threshold` (`sweep.rs:2073`) keeps its crawl
  half and loses its Hertz bracket.
- Report: both fitted fixtures, the narrow house, `B-ram-1` tick 1715 replayed.

**Step 3. The train.** `sweep_intake.rs`: carriers from the same pair fold as `pushes`,
`source_body`, `apply_exchange`. MEASURED B is +131 −13; less `train_mass` (20) and the second
fold (15): about +95.
- Parallel: the gather is read-only per mover; each carrier's share is applied in the same ordered
  exchange loop, after its source. Two pieces on one carrier each exchange against the carrier's
  pre-tick velocity; each exchange conserves momentum by construction.
- Tests: `a_pushed_piece_strikes_with_its_train` (a light piece a heavy body pushes frees matter it
  could not alone, and piece, carrier and plug momenta sum to zero change; mutant: judged alone).
- `net::ram::tracked_ram_respects_sweep_contacts_and_accounts_for_momentum` (`src/net/ram.rs:882`):
  rewrite its pairing to sum every account entry of the hull. Its invariant (the sweep's account
  equals the hull's momentum change) is still the law; only "one entry per body" was incidental.

**Step 4. Lying pieces as targets.** `sweep_intake.rs` (`Owners`, the cast in the target's moving
frame, the split through `PendingSplit` as crumble's), `SweepContacts`. No close analogue: the
earlier strike path was +564 for a different design; estimate +150, read as up to +330. Build only
after step 3's `passes` run still shows the stop. Test: `a_lying_piece_gives_way_as_a_wall_does`
(the same strike frees the same patch standing or loose: no body-kind gate on the target).

**Step 5, candidate. The static cut out of the sweep.** After steps 1-2 the cut only ever answers
"does the drive exceed capacity", which the structure check now receives as the persisting press.
One lab crawl run decides: if a wall under a slow push yields through the structure check within a
believable time, `plug` and `sweep/mincut.rs` (572 lines, MEASURED `wc`) and their tests go, and
each site holds one quantity.

**Not on `main` yet:** the hall and a lapped-timber attachment row land together, when the hall is
adopted as a fixture.

## 4. Show the owner (two questions)

1. **Hall, from above** (`…/bay-1/target/ram-a/round-2/clips/hall/pass-1-close.mp4`, right panel,
   3 to 5.5 s): the whole roof sags into the shell after the tank has crossed one room. *Should the
   roof over the two end rooms stay up?* Recommendation: yes. Their rafters stand on unbroken
   walls; it falls today because a flying piece frees the whole roof at once and its fixings are a
   stand-in. A yes also commits the roof-joint data work.
2. **House, side** (`…/round-2/clips/long/pass-1-side.mp4`, right panel, 11.75 s): a board lies on
   the rear deck as the tank drives away. *Fine, or a fault?* Recommendation: fine; it is a loose
   board on a flat deck, and removing it would need a rule for boards.

## 5. Rejected alternatives

- **Land the three arms as built:** three sites around an unreplaced judgement; P reads the wrong joint (61 m/s on timber).
- **Hertz with the train's mass (B's `train_mass`):** still turns momentum into a peak force, on a StandIn curvature.
- **A solver impulse clamp at the cut's capacity:** a fork divergence, and it leaves the hull overlapping matter nothing removed.
- **Solve, judge, solve again:** cost, and a solver step still sets the load.
- **Raise `nailed` until the roof holds:** tuned past its truth; the fix is the judgement plus a sourced row.
- **"Half must stand" as a gate:** one number for two geometries; the house cannot much exceed 60 %.
- **A cap on plug size, or "a roof never leaves whole":** rules for one kind of object.
- **Carriers by swept-bounds overlap (round 1's carried cast):** new pair queries; last step's contacts give one link already. Reopen if chains two deep show a stop.
- **Delete the static cut now:** unmeasured for a crawl; step 5 decides.
- **Break a large lying piece into fill on contact:** the owner rejected pieces turning to fine fill on landing.

## Not verified

Which body's sweep freed the hall roof at tick 1661 (only that it was not the hull's, and its
size); that the off-path piers fall because of the loose roof; which contact points stopped the hull
at `B-ram-1` tick 1715; the vehicle's turret height; that the Wood seam reads the bulk row's shear
cohesion; step 4's size; hosted cost of the flood.
