# Three panics in the march: causes, reach, fixes (#260)

Subject: the game's damage model, how a round moves across armour; game state only. A panic ends the process, the server's per-round path included (ADR-0011), so one round can end a whole match.

## In plain words

**A machine-gun round could end the process by skidding onto a plate.** A 7.9 mm round flies as one line. When its tick ended a few millimetres short of a plate it was going to skid on, the skid put that line exactly on the plate and pointed it along the plate. A line lying in a face's plane has no honest inside or outside, and the march's check that closed shells alternate entry and exit failed. It now stops 1 mm short of the plate. A round that already stands within that 1 mm has no room to turn and ends on the plate.

**A spent 88 mm round could end the process by rubbing along a plate.** A disc touching with one rim sample turns only a thirteenth of the way toward the plate's plane per contact, so it converges on the plane over many contacts. The arithmetic stopped that at 0.02 degrees: below 3.45e-4 rad the `acos` of the two headings' dot product reads zero, so the heading stayed pointed into the plate for good and the rim kept touching it every few micrometres, past the step's limit of 4096 decisions. The angle now comes from the cross product, the heading keeps converging, and the same shots end in about 100 contacts. The limit is untouched.

**A round running almost along a long plate could end the process as it left that plate.** After a perforation the march restarts 1 mm past the exit. It looked up what each sample was still inside on the rays the crossing had walked, then cast the next corridor from a freshly formed start, which is a slightly different line. On a ray 0.005 degrees off a plate's face, that rounding moved the plate's exit by 1.3 mm, more than the restart's 1 mm. The lookup said "already out", the new line said "still in", and the corridor met an exit with nothing open. Every corridor now reads its samples on its own line, the read a first touch and a skid already used, and the round pays the last 0.3 mm of plate and flies on.

**How often a player meets them.** Rarely, but not never: on the code before the first two fixes, from a real muzzle over real ground, about 1 machine-gun flight in 84 000 ended the process, and the crossing limit was never met in 33 601 cannon flights. The third cause ended about 1 disc flight in 2 400 on that range at this lane's HEAD (2 events, on a harness that flew half its volleys at a hull that had left), and none of the same 4 800 after the fix. On the corrected harness it ended none of 4 800, either side of the fix, so its real rate is poorly known. The last release, `v0.7.0-alpha.9`, carries the third cause by reading and not the first two.

## Membership

**State before the panic, MEASURED by trace.** `mg34 7.9mm_AP` born at (13.480293, 3.381734, -6.838281), post-step velocity (-300.5629, 434.80978, 539.0987). The first contact struck a face with normal (1, 0, 0). The start stood under the resting height (4.95 mm of height, DERIVED: rest 3.95 + clearance 1), so `to_resting_height` was not positive and `skid_turn` took `contact`, which stands the axis on the face. The slide heading was (0.0, 0.6277986, 0.77837586), exactly in the face's plane.

**What the panic assumed.** That certified closed shells alternate along a ray. Two things broke it:

1. **A ray exactly in a face's plane.** Probe hits for shell 0: entry at 0.2102, exit at 0.3777 on Edge(9, 28), exit at 0.3846 on Edge(9, 31). The in-plane triangle claims nothing, and each rim neighbour claims its edge alone, so one real transition counts per rim edge. The assumption is untrue of such a ray.
2. **A ray a hair outside a world bound.** `mg34 7.9mm_AP` born at (13.627799, 4.562245, -5.3784): axis (-8.43e-8, 0.85127, -0.52473), start 2.4e-7 m past the union bound's `hi.x`. The world AABB is rounded and does not strictly hold its faces, and `leave` read that start as already past the box, so the probe began inside the plate: exit with no entry. `mg34 7.9mm_AP` born at (10.371233, 4.795458, -6.657592): start 1.17e-7 m below `lo.x`, so the corridor ended short, `IncompleteCorridor`.

Of the classes the brief asks about, this is a grazing ray on a face, not a shared edge, a welded seam or two coincident faces.

**Fix.** `skid_turn`: a round flown as its axis alone turns `REST_CLEARANCE` short of the face (`min` of the first touch and the clearance height), so its slide runs 1 mm over the plate. Discs are untouched: their axis is not their lowest sample. `probe_reach`: the probe leaves the bounds grown by `PROBE_MARGIN`, which the doc already claimed and which the candidate filter already used.

**Left as it stands, P3.** A ray exactly in a face's plane still panics. No skid reaches it now, and a live hull pose makes exact coplanarity improbable (DERIVED, not measured beyond the run below). The principled answer is symbolic perturbation in `cross_triangle`, which replaces the fans the collector names contacts by; not narrow, so not done.

## A membership read on another line

**State before the panic, MEASURED by trace.** `kwk36_l56 88mm_AP`, `REACH_SLICE=0`. The record's earlier APCR event (sample 4, shell 0, an exit 2.5 mm past a start) has the same shape but was not traced: the commits under #263 moved the flights, its aim does not panic at this HEAD, and this one does (`ALONG_A_LONG_FACE`). Sample 4 of the disc entered Face(0) 4.0367 m behind its start and leaves Face(38) 0.0294 m ahead of it, at about 9e-5 rad to that face (DERIVED from the exit normal and the axis). So it runs 4.07 m inside one plate. Then:

| Line | Formed from | Face(38) exit |
|---|---|---|
| Certifying probe | axis A | +0.02942646 m |
| Transit | axis B, one ulp from A (`walk::begin`'s `Dir3::new` of a unit vector, READ) | +0.02943522 m, on the crossing's own perforation |
| Next corridor | axis A, origin re-formed 1 mm past that exit | 0.0307 m from the old start, +0.00029561 m from its own |

The seed came from the transit's line, read one nudge past the perforation: outside. The next corridor's line still had 0.3 mm of plate ahead. An exit with nothing open: `UnexpectedExit { sample: 4, t: 0.00029560641662396006, triangles: [38] }`. The crossing moved 1.3 mm between lines; rounding across the line is about 1.2e-7 m (DERIVED: 1.3 mm x 9e-5).

**Geometry class.** A ray nearly, not exactly, in a long plate's face plane. Near neighbour of the first cause; no vertex, edge or fan involved. The exit is `Face(38)`, so the mixed saddle fan is not this cause (the codex lead); the seed read on another line is (the claude lead, now MEASURED).

**State crossing rounds.** The range hull's pose and velocity, and nothing else seen. First touch read a velocity of (-0.350, -0.044, -0.596) m/s on a stance standing with zero velocity: the range's hull is kinematic and each hit's impulse moves it, so the earlier shots move it for the later ones. "Still hull" in the tables below means standing still when the volley is fired, not while it lands. A shared-state defect in the march: none found. This is why one shot alone did not reproduce it; the replay needs both volleys the harness flew together.

**Is the invariant true?** This is the third cause under "certified closed shells alternate from established membership", and the verdict is that the arithmetic never guaranteed the claim. Alternation holds along one line in one arithmetic, from a membership read on that line, bar two degenerate rays: one exactly in a face's plane (the first cause) and one through a saddle vertex with a mixed fan. "From established membership" was false wherever that membership was read on another line, and three sites did: `decide`'s miss, `bite`'s perforation and `clear_path`. `SampleSeed`'s own doc already said "certified on the ray that consumes it"; those three broke it. The topology bands (about 1e-6 m) cover a re-measure only above about 0.1 rad (DERIVED: 1.2e-7 / 1e-6), so no band could.

**Fix.** `next_starts`: those three sites read through `certified_samples`, on the line the next corridor casts. `DiscWalk::resume_at` is gone: one read path. Cost: one more containment probe per crossing and per examined span, not timed (DERIVED from the code).

**What it does not retire.** `IncompleteCorridor` stays the corridor's extension signal, panicking only at the horizon on a broken certificate. The in-plane panic stays: a ray exactly in a plane has no parity on its own line, so one line does not help it. Still P3 below.

## Crossing limit

**State before the panic, MEASURED by trace of 4098 decisions.** `kwk36_l56 88mm_AP` born at (13.448209, 5.9313054, -3.65905), post-step velocity (-114.379295, -136.64839, -74.76734). The first skid (193.25 to 136.6 m/s) leaves the heading nearly along the top face. Coverage is 1/13, one rim sample. The heading's part into the face goes -0.00112, -0.00103, -0.00096, each 12/13 of the last, until it freezes at -0.00035158466. The contact distance then falls to rounding: median 2.47e-5 m, smallest 5.98e-6 m, sum 2.087 m over the whole step.

**What the panic assumed.** That a finite step fits its limit. It was true of the law and false of the arithmetic: the law closes a share of the angle per contact, and the `acos` read no angle to close (DERIVED: acos(1 - 2^-24) = 3.45e-4 rad). Raising the limit would only have stalled it (about 350 000 exchanges a tick, DERIVED).

**Fix.** `turned_toward` reads the angle as `atan2(|d x t|, d . t)` and drops the 1e-5 floor. The heading now closes to a unit direction's rounding, about 110 contacts from 1.1e-3 rad (DERIVED: ln(1.1e-3 / 1e-7) / ln(13 / 12)). MEASURED: the same shot flies 97 exchanges and no panic. All 24 aims recorded as panics in `contact-classes-2026-11-03.md` now fly (`the_shots_that_once_ended_the_process_fly`).

**This did not need the rebuild's contact identity.** The chain ends by the law's own convergence. The rebuild still has work here: those 97 exchanges are one contact in any plain sense, each with its own mark and shock.

## Reach on a real map

`panic_reach_on_shipped_ground` flies every fired round from a muzzle 2.2 m over the shipped ground, 60 to 600 m out with a clear line, lofted and led, at the shipped hull standing on that ground at a drawn spot, yaw and slope, still or moving at up to 12 m/s and 0.5 rad/s. Flights in a volley share a tick. Seeded by `REACH_SLICE`. The hull is a posed kinematic body, not driven through its tracks, with no damage consequence. A "touched" flight left an armour mark.

**Unfixed code, MEASURED, 7 slices, 117 826 flights:**

| Round | Flights | Touched | Process ends | Per 1000 flights |
|---|---|---|---|---|
| 7.9mm_AP | 84 225 | 34 702 | 1, `UnexpectedExit`, moving hull | 0.012 |
| 88mm_AP | 8 400 | 3 491 | 0 | 0 |
| 88mm_APCR | 8 401 | 3 365 | 1, `UnexpectedExit`, still hull | 0.119 |
| 88mm_APHE_ring | 8 400 | 3 438 | 0 | 0 |
| 88mm_HE | 8 400 | 7 503 | 0 | 0 |

Still and moving hulls got half the flights each. `DrainExhausted`: 0 in 33 601 cannon flights. One event per row, so these rates are poorly known.

**With both fixes, MEASURED, the first 3 of those seeds, 50 497 flights:**

| Round | Flights | Touched | Process ends | Per 1000 flights |
|---|---|---|---|---|
| 7.9mm_AP | 36 096 | 14 913 | 0 | 0 |
| 88mm_AP | 3 600 | 1 526 | 0 | 0 |
| 88mm_APCR | 3 601 | 1 416 | 1, the same `UnexpectedExit`, same `t` | 0.278 |
| 88mm_APHE_ring | 3 600 | 1 495 | 0 | 0 |
| 88mm_HE | 3 600 | 3 189 | 0 | 0 |

The machine-gun event of slice 1 is gone on its own seed. Zero in 36 096 only bounds the rate under 0.083 per 1000 (DERIVED, rule of three), which does not separate it from 0.012: the evidence the class is gone is the trace and the recorded aims, not this count.

**Can a player meet it?** The machine-gun rate is about one process end per 84 000 flights; a match with some thousands of rounds meets it about once in tens of matches (DERIVED, assuming a thousand or two touching rounds a match). Crossing limit: not in this count; it needs a spent disc with one rim sample on a face after a skid.

**At this lane's HEAD, same seeds, MEASURED.** The tables above predate the #263 commits. Slice 0, with the old lookup and the new read side by side on every next start: they disagreed on 2 of 4 947 reads, and those two are the two panics, so no flight that did not panic changed. After the fix the same two flights fly on, with 4 of 4 755 reads differing, 2 each.

| Round | Lookup: flights, touched, process ends | One line: flights, touched, process ends |
|---|---|---|
| 7.9mm_AP | 12 032, 4 980, 0 | 12 032, 5 044, 0 |
| 88mm_AP | 1 201, 490, 1 | 1 200, 486, 0 |
| 88mm_APCR | 1 200, 477, 0 | 1 200, 475, 0 |
| 88mm_APHE_ring | 1 202, 514, 1 | 1 200, 487, 0 |
| 88mm_HE | 1 200, 1 113, 0 | 1 200, 1 043, 0 |

A panic rebuilds the range, so the two columns are not paired flight by flight past the first event. Both events are `UnexpectedExit` on sample 4 with a disagreement at that read: the one traced above, and `88mm_APHE_ring` under way, `t: 0.03350488155194269`, triangle 40, not traced further, which the same flight on the new read flies through. Zero in 4 800 disc flights bounds the rate under 0.62 per 1000 (DERIVED, rule of three), which does not separate it from the lookup's 0.42: the evidence the class is gone is the trace, the replayed volley and the removed path, not this count.

**The harness flew half the volleys at a hull that had left.** A trigger queues the spawn, so `follow`'s first flying count read none and it returned `Ok(0)` for that volley; its shots spawned at the next `stand`, aimed at the old pose. The next volley flew 24 shots, 12 of them not aimed there. So about half the flights in every table above, these included, were not aimed at the hull, the "touched" columns undercount and the rates per aimed flight are wrong by up to 2. `follow` now flushes. MEASURED, slice 0 flushed, both reads: the same table to the last digit, 0 disagreements in 10 002 reads, no process end, so the fix changes no flight there: 7.9mm_AP 12 032 flights, 10 046 touched; 88mm_AP 1 200, 1 012; 88mm_APCR 1 200, 988; 88mm_APHE_ring 1 200, 1 018; 88mm_HE 1 200, 1 177.

**Chain flights, the review's open question, MEASURED.** Does a heading that closes to about 1e-7 rad let a chain flight meet this panic? 3 000 flights jittered about the five recorded rim aims (1 mm of birth, about 1e-3 rad of heading; a temporary measure, not kept) made 2 600 chains of 30 contacts or more, the longest 101, with no process end on either read. Zero bounds it under 1.2 per 1000 chains (DERIVED), and the path the review names, a restart on a re-formed line, no longer exists.

## Mutants

MEASURED, one build with all three mutants, all five tests red:

| Mutant | Test | Verdict |
|---|---|---|
| A line round turns at first touch | `a_line_round_never_turns_onto_the_face_it_skids_on` | "0.0015 m: the axis turned 0 m off the face" |
| The probe leaves ungrown bounds | `a_probe_leaves_the_grown_bounds_from_a_start_past_a_slab` | "behind 0 m, ahead 1.1722866 m" |
| The blend reads its angle with `acos` | `a_blend_closes_its_share_of_any_angle` | "0.009999996 rad closes half to 0.005002539 rad" |
| The same | `a_chain_of_rim_contacts_converges_on_the_face` | "the heading stands 0.0003734138 rad off the face" |
| All three | `the_shots_that_once_ended_the_process_fly` | red at `kwk36_l56 88mm_AP` born at (13.448209, 5.9313054, -3.65905) |

| Next starts looked up on the rays the crossing walked | `a_volley_along_a_long_face_flies_out` | `UnexpectedExit { sample: 4, t: 0.00029560641662396006, triangles: [38] }`, the recorded bits; green on the fix |

MEASURED on the bed's binaries, which is what splits the row before last: with both line-round fixes and the `acos` left, only the recorded 88 mm aim panicked; with all three left, the recorded line-round aims did too. A line-round mutant alone was not built against that test.

## The last release

Read at `v0.7.0-alpha.9` with `git show`, not run.

- **Ricochet there is specular.** `walk::begin` blends toward the reflection, and `decide` resumes at `position + normal * radius + out * MARCH_EPS`. No heading lies along a face, and no line round's axis stays on one. The membership cause came with the subjects "ballistics: replace angle gates with pressure contact (#147)" and "ballistics: give skid contacts a clock or a terminal rest (#260)".
- **The ungrown `leave` is there,** without the skid that reaches it.
- **The `acos` in `bend_toward` is there,** but a skid there lifts off the face by the disc's radius and advances at least 1 mm per exchange, so the chain above cannot form (DERIVED).

- **The third cause is there.** `decide`, `bite` and `clear_path` all read `resume_at` at the tag, the next corridor re-forms its origin in the same f32 sum, `collect_collider`'s local origin is the same, and the panic string is the same. Not run there, and that build's aim that meets it is unknown.

So the release carries the third cause and not the first two; `main` carried all three until this fix.

## Still open

- **P2, a contact is many exchanges.** About 100 marks and shocks for one rim rubbing a plate, about 39 ms of authority time in one 15.6 ms tick where it happens, plus a message per bounce (DERIVED: 97 exchanges x 400 us per impact at k = 13). The rebuild's contact identity.
- **P2, a line round within 1 mm of a face ends on it, at full speed.** `skid_turn`'s third arm is keyed on `radius == 0`, so on `DISC_MIN_CALIBER`, and a 20 mm round at that height slides. All its energy goes to shock and its momentum to impulse where the law says slide, on about 8.5e-5 / cos(incidence) of line-round skids (DERIVED: 1 mm over 11.8 m of tick travel). Moving the arm onto discs changes where near-normal `Held` discs end, so it is not narrow, and lifting the round adds a teleport. The audit names zero-turn rest in the rebuild's scope.
- **P2, `REST_CLEARANCE` has a second job.** It decides slide against stop as well as its stand-off: three outcomes by tick phase (4.96 mm slides at 4.95, 4.94 slides at 1, 0.99 stops). The job goes with the third arm above. Its doc is still false for a line round on the first arm: see the table below.
- **P3, `contact` is a parry cast in world f32, the walker's hit is exact.** The difference sets a chain's step length.
- **P3, a ray exactly in a face's plane or through a saddle vertex panics.** No skid reaches the first now, and an exact hit on either needs exact arithmetic on a live pose (DERIVED, not seen in any count here). The principled answer is symbolic perturbation in `cross_triangle`, which replaces the fans the collector names contacts by; not narrow.

## Reviews of the first two fixes

Every row of both reviews, verified against the code.

| Row | Disposition |
|---|---|
| claude P1, `resume_at` seeds on one line, corridor on another | CONFIRMED by trace, fixed above |
| codex P1, unresolved panic at `integrate_field` | Same, fixed |
| codex P2 and claude P3, `UnexpectedExit`'s doc and panic string | Fixed: both state the one-line invariant, and the doc both exceptions |
| claude P2, chain flights near 1e-7 rad | Measured above, none; the path it names no longer exists |
| claude P2, `skid_turn` keyed on `radius == 0` | Verified, left, named under "Still open": the rebuild's |
| claude P2, `REST_CLEARANCE` second job | Verified, left, named under "Still open": the rebuild's |
| claude P3, the record understates non-panicking change | True, stated here: the `atan2` moved every low-coverage disc skid's heading, not only those under 3.45e-4 rad. A 1e-2 rad blend closes half to 5.0e-3, was 5.0025e-3 (MEASURED by the `acos` mutant above); about 6e-5 rad of read error at 1e-3 rad (DERIVED). A flight that rubbed a plate and left no longer keeps a frozen heading |
| claude P3, chain cost unstated | True, stated under "Still open" |
| claude P3, "still hull" mislabel | True, MEASURED above |
| claude P3, subject of "dev: measure march panics on the shipped ground (#260)" | True, history; left |
| claude P3, `atan2` from platform libm | True (`trees.rs` uses `libm::atan2f`). **Not done at the hour** |
| claude P3, `REST_CLEARANCE`'s doc false for a line round on the first arm | True. **Not done at the hour** |
| claude P3, `the_shots_that_once_ended_the_process_fly` never asks that an aim touches | True. **Not done at the hour** |
| claude P3, `a_chain_of_rim_contacts_converges_on_the_face` never calls `begin` or `decide` | True: it kills only what `a_blend_closes_its_share_of_any_angle` kills, and the recorded 88 mm aims fly the chain end to end. To delete. **Not done at the hour** |
| claude P3, "a world AABB contains its collider" | True, and `probe_reach` relies on rounding under `PROBE_MARGIN`, to about 8 km of world coordinate (DERIVED). **Not done at the hour** |
| claude P3, unlabelled numbers | True for `ENTRANCE_SPAN`, `FIELD_REACH`, `MAX_STEP_CROSSINGS`, `REACH_*`. `DISC_MIN_CALIBER` is the rebuild's to decide. **Not done at the hour** |
