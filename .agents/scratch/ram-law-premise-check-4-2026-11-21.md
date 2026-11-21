# Ram law, premise check 4

Subject: the game's building model (game state only): chunks joined by joints, loose pieces, and
the pre-solve sweep that judges what a moving body's path frees, when a heavy vehicle drives
through a wall. Nothing was built or run this round. Code citations are `lane/ram-law` at
`bfe59fc2` unless `main` is named. MEASURED numbers are from
`lane/ram-law:.agents/scratch/ram-a-experiment.md` or read this round from the diagnostic logs
`.claude/worktrees/bay-1/target/ram-law/runs/{Cdiag5-long,Cdiag5-hall,Bdiag-hall}/run.log`.
Ticks are the logs' `DIAG lab tick=` labels; the builder's notes name the same events one tick
lower (1641/1647 for 1642/1648). DERIVED numbers show their arithmetic. The vehicle (57 t) is the
fixture, never the spec.

## 0. The answer

The builder's diagnosis is wrong about the cause. The exchange's weld is identical on `main`, B
and C; what C changed is **how much leaves**: design 3's flood grows a strike's plug through every
joint that can pass the motion, and at ram speeds that is the whole stack above a struck chunk and
every timber member joined to it. The friction-bounded exchange would change the direction that
matter leaves in, not the amount, and by derivation does not save either building. Do not build it.

Recommendation: **one round, a deletion.** On `lane/ram-law`, cut the flood down to its seed: a
strike frees the chunk it sweeps, if its closing loss pays that chunk's cracks, and nothing else.
Everything outside the path is left to the laws that already own it: connectivity and the
structure solve under gravity and persisting forces. This is the owner's rule stated as the law,
and it is the part of arm B that was right, under C's cleaner judgement. Kill line and fallback in
section 3.

## 1. Is the builder's diagnosis right?

**What is true in it.** `exchange` (`src/construction/sweep.rs:956-1039`) solves one six-coordinate
rigid field for the striker and everything it released: all components, no friction limit. So
released matter does leave at the striker's velocity, sliding included.

**Why it is not the cause.**
- The exchange is the same code in every arm: `main:src/construction/sweep.rs:752`, and
  `git diff ca7299b9 bfe59fc2` (B to C) changes no line of it. B stood 45 % / 52 % in place with it.
- A controlled pair in the records. Hall, tick 1648, the hull's front rising under the roof line:
  same witness chunk (108), same normal (−0.04, 1.00, 0.00), closing 4.86 m/s under B and 4.47 under
  C; the hull's velocity matches to 0.3 m/s through tick 1647 in both logs. **B frees 1 chunk
  (`plug=1 plugchunks=[108]`). C frees 99** (2 pier, 5 wall plate, 17 rafter, 39 tie, 36 roof;
  5,418 kg), and that body, in the same tick's second round, frees 101 more (40 gable, 42 roof,
  14 rafter, 3 pier, 2 plate). Standing goes 495 → 265 in that tick under C, 513 → 509 under B. All
  MEASURED. One thing differs between the two: the plug rule.
- The plug rule is the flood (`sweep.rs:480-582`). It adds a neighbour whenever the plane-wave
  traction lies inside the joint's limits (`:540-560`). Design 3's own numbers say what that means:
  bearing passes below 8.9 m/s and a timber seam below 61 m/s (design 3, lines 36 and 101-102). So
  a strike from below carries the whole stack above it, and any strike on a timber member carries
  every member seated on or continuous with it. On the hall that is all 39 ties and 64 % of the
  roof's 154 members from one upward strike on one chunk.
- The gate does not bound it. The closing loss against crack work (`:385-386`) read 21,109 J
  against 796 J for the 99 chunks, 13,919 against 695 for the 101, and on the house 1,200 against
  6.2, 210 against 2.4, 2,973 against 22 (MEASURED): 26 to 190 times over. Mortar, nailed and
  resting perimeters cost joules; a ram carries kilojoules. At ram speeds the flood alone sets the
  plug.

**House, tick 1642 (0.03 s after contact; standing 1086 → 745 in one tick, MEASURED).**
1. Round 1, the hull: 12 witnesses. Eight flood one chunk each. Four, on an up-and-forward normal
   (0.66, 0.75) at 7.0 m/s, flood 30-34 chunks each: the pier column above and the wall plate
   along its whole length, including the parts outside the hull's width. Plug 65 (46 pier, 15 wall
   plate, 4 sill).
2. Round 2, in the same tick (`DEPARTURE_ROUNDS`, `sweep_intake.rs:433-480`): that plug, now a
   1,260 kg body at 10.14 m/s, strikes the gable beside it and frees 44 (27 gable, 15 wall plate,
   2 pier). Round 3: a 929 kg body at 5.61 m/s frees 39 (25 gable, 10 pier, 4 lintel).
3. **The other 193: connectivity, not a force solve.** `cut_fragments`
   (`sweep_intake.rs:1507-1552`) parts a plug's perimeter joints and then frees every standing
   chunk left with no joint path to a ground anchor or buried chunk (`crumble::fragments`,
   `fracture::loose_groups`, `src/construction/fracture.rs:1121`); they leave at rest
   (`fragment_motion`, `:1471-1472`). The three cuts report plugs of 65, 44, 39 and fragments of
   105, 197, 39: 341 gone, of which 40 + 153 + 0 = 193 are matter whose last support the plugs
   took (all 98 roof boards, the remaining gable groups, plates, lintel groups). The builder's
   "static solve" is a misnomer. The law is the owner's rule exactly (falls only if its support
   left) and it is right; its input, the plugs, was wrong. Not verified: which joints were those
   groups' last path.
4. Also MEASURED and not the sweep: later one-tick drops with few or no sweep fragments (house
   tick 1668, 76 gone with 10 swept; tick 1682, 46 with none; hall tick 1665, 26 with 6). Those are
   the structure solve under gravity and bounded press. Not diagnosed; some will remain under any
   plug rule.

**C's worst house tick is the flood too (inference).** At tick 1642 eleven hull witnesses at
4.2 m/s (carriers of 15-23 t; normals not logged) flood into the ground, are classed `Grounded`
(`sweep.rs:554-555`) and are judged by the static cut against the drive alone, so they hold; next
tick the hull reads (−0.07, +1.29, −8.65) m/s from (0, 0, −10.30): the 1.62 m/s worst tick, the
hull riding up on held sills. Inferred from the two adjacent log entries, not probed.

**The truth the exchange should express.** A contact gives a normal impulse that brings the pair
to one normal velocity at the contact, and a tangential impulse of at most friction times it,
against the slip. Matter a body slides under or past receives only that friction, and sliding
frees nothing (the gate already says so, `sweep.rs:435-466`). The weld is exact only for matter
that ends inside the body's swept volume and cannot slip out of it. So the weld is a real fault,
but a small one once plugs are single swept chunks: it then gives tens of kilograms a few m/s of
tangential speed they should not have. It is a later, contained change, never a rescue.

**Did B work because it was right?** It was right where it was "the swept patch leaves and
nothing else", and wrong everywhere else; nothing cancelled.
- Right by accident: P's test compares the witness speed with the struck chunk's own seam sliding
  speed (0.065 m/s for brick, DERIVED in design 1). Every real strike on brick exceeds it, so on
  masonry P was unconditionally "free the swept patch": the owner's rule. That is why B's walls sit
  at the geometric ceiling (piers 395 of 573) and why the hull at hall tick 1648 freed one chunk.
- Wrong, and each large B loss traces to it (MEASURED, round 3's "three unknowns"): the hall roof
  at tick 1661 (a 432 kg piece, Hertz demand 359 kN against a 77 kN cut, the static cut's source
  side: 115 chunks); off-path piers at 1668 and 1686 (thrown pieces, Hertz against the cut); gable
  and lintel chunks at 1695 and 1700 (`train_mass` demands of 371 kN and 1.1 MN at 0.001 m/s); the
  house's roof, plates and gable tops at 1653 (10.6 MN against 1.03 MN, 183 chunks). All are Hertz
  demand through the static cut, which C's tree has already deleted.

## 2. The decision

**(a) One more round on C with the friction-bounded exchange. Rejected by derivation.**
- What changes: `exchange` becomes a per-contact impulse (normal plastic, tangential capped), so
  it needs each plug's witness; today it is one solve per body over all its plugs
  (`exchange_all`, `sweep_intake.rs:1158-1188`). Readers: `apply_exchange` (`:1253`),
  `launch_after` (`:1331`), `commit`'s `ReleasedMotion` (`:1367-1377`), `fragment_motion`
  (`:1460`), the account's `dissipated` (`:1289`), two tests in `sweep.rs` (`:3047`, `:3085`) and
  `net::ram`'s momentum test. About +120 to +200 lines (DERIVED from those sites; no analogue).
- Hall roof-lift, DERIVED: the flood reads only the closing speed and normal (`sweep.rs:505-506`),
  so the same 99 chunks leave. They rise at 4.47 × 9,582 / (9,582 + 5,418) = 2.86 m/s (0.42 m),
  with at most friction × 2.86 sideways: 1.7 m/s at 0.6, against 5.3-5.8 MEASURED. The second
  strike's loss scales with speed squared: 13.9 kJ × (1.7 / 5.32)² = 1.4 kJ against 695 J: it still
  pays at 0.6 and fails to at 0.4. The second event would hang on a friction coefficient; the
  first already has 99 of 154 roof members loose, under design 3's floor of 85 in place.
- House far side, DERIVED: the 65-chunk plug still leaves, at 7.0 m/s along (0.66, 0.75) plus
  friction: about 4.6 + 0.6 × 7.0 × 0.75 = 7.8 m/s forward against 10.14 welded. The gable strike's
  loss is about ½ × 142 kg × 7.8² = 4.3 kJ against 212 J. Unchanged outcome.
- The owner would see the same heaps, with the roof section tossed up instead of carried forward.

**(b) Land B's behaviour in a clean form.** As built there is none: three sites around a Hertz
judgement, P reading the wrong seam, `train_mass`. But B's behaviour has a clean form, and it is
option (d): B's right half without its wrong half.

**(c) Keep `main`'s machinery, change only what brings buildings down.** Two things do on `main`:
the Hertz demand through the static cut (the hall flat in 0.1 s: 50.3 MN against 0.70 MN) and the
whole-transfer press (9.77 MN into the part law). Removing them is arms P and A, which then stall
on a held loose piece (−1.06 m/s, 0.32 m back at 8.2 m/s contact), which needs B's train. So (c)
walks back into B's three sites. No shorter cut exists on `main`'s machinery.

**(d) Recommended: C with the flood cut to its seed.**
- The rule: the sweep judges what is in a body's path, one swept chunk per witness. The chunk
  leaves if the strike's closing loss pays its own perimeter's crack work; else it holds and, as
  now, the static cut answers against the drive alone (the crawl). Motion passes on the way it
  does in the world at this resolution: through matter that is moving. The freed chunk is a body
  at the striker's speed that sweeps what is in its own path in the same tick's later rounds, each
  strike sharing momentum and paying its own cracks, and a piece the vehicle presses strikes as
  its train. Matter beside or above the path falls when connectivity or the structure solve says
  its support left.
- What changes: `flood` loses its extension branch and its `Grounded` returns (`sweep.rs:552-561`)
  and keeps only the classification of the seed chunk's incident joints for pricing
  (`:519-551`, `:564-580`); the shared-speed term (`:505`) and the carrier argument go;
  `resolve` (`:377-417`) sends an unpaid witness, not a grounded one, to the cut. Tests:
  `a_joint_passes_motion_only_below_its_strength_over_impedance` goes;
  `a_weak_fixing_carries_no_neighbour_along` becomes "a strike frees only what it sweeps";
  the held and crawl tests stay. About −90 +25 production (DERIVED from those line ranges).
  `Exchange` and all its readers are untouched. No wire change.
- The owner would see (expectation, a range, not a pin): a hole the vehicle's width through each
  wall; walls and gables beside the path standing; the roof over the breach dropping as its
  supports go; on the hall the end rooms' roof staying on unbroken walls. House 45-60 % in place,
  hall 55-90 %.
- Debt it leaves, named: the static cut still frees its whole source side for a slow strike
  whose drive beats it (`sweep/mincut.rs`, 572 lines; design 3's step 5); the weld; site 2's
  chain is not built; lying pieces are not targets (the one-tick stop against a felled house,
  15.2 m/s, is still there and is the next thing between this and "without sticking").
- Not a rule for one kind of object: it states where the sweep's authority ends. What it gives up
  is an instantaneous rigid plug ahead of the strike; the chain of strikes carries the same
  momentum at chunk resolution, up to eight rounds deep a tick.

**Why design 3's estimate was wrong: the reasoning, not the lab or the fixtures.**
- Nothing was run before it (design 3, line 5), and its central mechanism was new.
- It derived the flood only against the three vehicle faults (pushed piece, held verdict, the
  21 t stop). It never asked what a flood seeded under a lintel or on a plate reaches, though its
  own two numbers (8.9 and 61 m/s) answer it.
- It took the energy gate as the bound on plug size. Perimeter crack work is joules.
- It blamed the roofs on the static cut and counted deleting the cut at speed as the cure; the
  flood frees roofs whole by another route.
- Its expected results (section 2) were geometry, what ought to stand, never a derivation of what
  the law would do.
- The fixtures and lab did their job: both exposed it within 2 and 8 ticks of contact, the house
  identically in 4 of 4 runs.
- The builder's diagnosis repeated the habit: it looked at the velocity of what left, not at how
  much left, and proposed an addition.

**Rounds, and why this estimate deserves more trust.** One experiment round (a deletion on the
existing branch). If it passes: one clean build on `main` (design 3's steps 0, 1, 3 and a smaller
step 2) and the usual one or two lens rounds. Reasons, each checkable:
1. It is not a new mechanism. "The swept patch only" ran on masonry for three arms on both
   fixtures; under A in round 1 the hall's roof stood over the first breach.
2. The decisive event is a controlled pair in the records (1 chunk against 99 at the same tick).
3. Every large B loss is a Hertz-through-cut event, and that path is already deleted in C's tree.
4. The unknowns are named: swept-only on timber at speed has never run (long timber chunks leave
   whole); the narrow house without the flood may land between C's 2.09 and B's 4.79 m/s worst
   tick (B's cause there was the single-witness judgement, which C fixed and this keeps); the
   structure solve's own later drops remain. My odds that it passes the line below: about 60 %.

## 3. Kill criterion (thresholds fixed now, before the run)

Arm D = `bfe59fc2` plus the one commit. Same staging and fixtures as round 3 (from rest, contact
10.63 m/s), 4 runs on each fitted fixture, 2 on the narrow house `ram` and `passes`. "In place" is
the lab's `in_place_above_grade`.

**Continue (clean build on `main`) only if all of:**
- house in place ≥ 40 % and hall in place ≥ 55 % (design 3's absurd-side bounds, DERIVED from the
  specs; B' read 45 % and 52 %, C 18 % and 13-18 %);
- on both fitted fixtures: no roll-back, 0 s under 0.5 m/s, worst tick ≤ 2.3 m/s (today's game
  reads 2.29; C 1.62 and 0.96);
- on the narrow house `ram`: no roll-back and worst tick under B's 4.79 m/s.

**Anything else is stop.** No near-miss round, whatever single event explains the miss. Fallback,
named now: no eighth law round. Build C as measured (`bfe59fc2`'s law) cleanly on `main` as the ram
law for early access. It is the best vehicle motion of any arm on all three fixtures and no worse
than today's game on the buildings (6 / 18 % against 6 / 22 %; 4-5 / 13-18 % against 0 / 5 %), with
Hertz, the whole-transfer press and three arms deleted. The goal line "keeps what is still
supported" then goes to the owner as unmet, with the clips, as a product call. B as built is not
the fallback: it is the three stacked sites.

Report with the verdict, as observations: in place by part; `passes` pass 1's stop; any release
judged `Force` that frees more than 20 chunks, with its tick.

## 4. What must be true of the measuring

- **Thresholds precede the run** (section 3), and the arms compare on one staging: D against the
  saved `B'` and `C` binaries (`target/ram-law/bin/{Bp,C,Tp}`).
- **A split run cannot decide.** The lab shares one World with its view, so entity ids vary and C
  split on the hall at tick 1656. That did not touch C's verdict: the hall was at 265 standing by
  tick 1648 and 215 by 1650, before the split, and both outcomes read 4-5 %. Rule for D: the
  verdict counts only if every same-seed run lands on the same side of every threshold; report the
  first tick the runs part. If runs straddle a threshold, the round reads as stop.
- **Every large drop is attributed.** One diagnostic run per fitted fixture with round 3's probe
  (`target/ram-law/diag-C5.patch`: per-sweep plug, fragments, judgement; per-tick parts gone).
  Each tick losing 8 or more chunks is assigned to the sweep's plug, to connectivity, or to the
  structure solve, by tick. This is what made the check above possible; without it a pass or a
  fail is a number without a cause.
- **Read in place, not standing,** and by part. Tick cost from this lab is not hosted cost; do
  not read it.
- **The real-server instrument confirms the house before the `main` build lands.** It can place
  the house only. The hall's reading rests on the lab alone, and the landing report says so.

## Not verified

Which joints were the last path to ground for the house's 153 connectivity-freed chunks; the cause
of the structure-solve drops in section 1 item 4; that the hull's ride-up at house tick 1643 is
the held grounded sills; how long timber chunks behave under the swept-chunk rule; the vehicle's
height against the eaves; hosted cost of any of it.
