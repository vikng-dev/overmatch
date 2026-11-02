# Two panics in the march: causes, reach, fixes (#260)

Subject: the game's damage model, how a round moves across armour; game state only. A panic ends the process, the server's per-round path included (ADR-0011), so one round can end a whole match.

## In plain words

**A machine-gun round could end the process by skidding onto a plate.** A 7.9 mm round flies as one line. When its tick ended a few millimetres short of a plate it was going to skid on, the skid put that line exactly on the plate and pointed it along the plate. A line lying in a face's plane has no honest inside or outside, and the march's check that closed shells alternate entry and exit failed. It now stops 1 mm short of the plate. A round that already stands within that 1 mm has no room to turn and ends on the plate.

**A spent 88 mm round could end the process by rubbing along a plate.** A disc touching with one rim sample turns only a thirteenth of the way toward the plate's plane per contact, so it converges on the plane over many contacts. The arithmetic stopped that at 0.02 degrees: below 3.45e-4 rad the `acos` of the two headings' dot product reads zero, so the heading stayed pointed into the plate for good and the rim kept touching it every few micrometres, past the step's limit of 4096 decisions. The angle now comes from the cross product, the heading keeps converging, and the same shots end in about 100 contacts. The limit is untouched.

**One more panic is still live.** The live range met `UnexpectedExit` on an 88 mm APCR disc too, which neither fix touches. See "Still open".

**How often a player meets them.** Rarely, but not never: on the unfixed code, from a real muzzle over real ground, about 1 machine-gun flight in 84 000 and about 1 APCR flight in 8 400 ended the process (one event each, so the rates are poorly known), and the crossing limit was never met in 33 601 cannon flights. The last release, `v0.7.0-alpha.9`, does not carry either fixed cause.

## Membership

**State before the panic, MEASURED by trace.** `mg34 7.9mm_AP` born at (13.480293, 3.381734, -6.838281), post-step velocity (-300.5629, 434.80978, 539.0987). The first contact struck a face with normal (1, 0, 0). The start stood under the resting height (4.95 mm of height, DERIVED: rest 3.95 + clearance 1), so `to_resting_height` was not positive and `skid_turn` took `contact`, which stands the axis on the face. The slide heading was (0.0, 0.6277986, 0.77837586), exactly in the face's plane.

**What the panic assumed.** That certified closed shells alternate along a ray. Two things broke it:

1. **A ray exactly in a face's plane.** Probe hits for shell 0: entry at 0.2102, exit at 0.3777 on Edge(9, 28), exit at 0.3846 on Edge(9, 31). The in-plane triangle claims nothing, and each rim neighbour claims its edge alone, so one real transition counts per rim edge. The assumption is untrue of such a ray. Any ray not exactly in a face's plane keeps it.
2. **A ray a hair outside a world bound.** `mg34 7.9mm_AP` born at (13.627799, 4.562245, -5.3784): axis (-8.43e-8, 0.85127, -0.52473), start 2.4e-7 m past the union bound's `hi.x`. The world AABB is rounded and does not strictly hold its faces, and `leave` read that start as already past the box, so the probe began inside the plate: exit with no entry. `mg34 7.9mm_AP` born at (10.371233, 4.795458, -6.657592): start 1.17e-7 m below `lo.x`, so the corridor ended short, `IncompleteCorridor`.

Of the classes the brief asks about, this is a grazing ray on a face, not a shared edge, a welded seam or two coincident faces.

**Fix.** `skid_turn`: a round flown as its axis alone turns `REST_CLEARANCE` short of the face (`min` of the first touch and the clearance height), so its slide runs 1 mm over the plate. Discs are untouched: their axis is not their lowest sample. `probe_reach`: the probe leaves the bounds grown by `PROBE_MARGIN`, which the doc already claimed and which the candidate filter already used.

**Left as it stands, P3.** A ray exactly in a face's plane still panics. No skid reaches it now, and a live hull pose makes exact coplanarity improbable (DERIVED, not measured beyond the run below). The principled answer is symbolic perturbation in `cross_triangle`, which replaces the fans the collector names contacts by; not narrow, so not done.

## Crossing limit

**State before the panic, MEASURED by trace of 4098 decisions.** `kwk36_l56 88mm_AP` born at (13.448209, 5.9313054, -3.65905), post-step velocity (-114.379295, -136.64839, -74.76734). The first skid (193.25 to 136.6 m/s) leaves the heading nearly along the top face. Coverage is 1/13, one rim sample. The heading's part into the face goes -0.00112, -0.00103, -0.00096, each 12/13 of the last, until it freezes at -0.00035158466. The contact distance then falls to rounding: median 2.47e-5 m, smallest 5.98e-6 m, sum 2.087 m over the whole step.

**What the panic assumed.** That a finite step fits its limit. It was true of the law and false of the arithmetic: the law closes a share of the angle per contact, and the `acos` read no angle to close (DERIVED: acos(1 - 2^-24) = 3.45e-4 rad). Raising the limit would only have stalled it (about 350 000 exchanges a tick, DERIVED).

**Fix.** `turned_toward` reads the angle as `atan2(|d x t|, d . t)` and drops the 1e-5 floor. The heading now closes to a unit direction's rounding, about 110 contacts from 1.1e-3 rad (DERIVED: ln(1.1e-3 / 1e-7) / ln(13 / 12)). MEASURED: the same shot flies 97 exchanges and no panic. All 24 aims recorded as panics in `contact-classes-2026-11-03.md` now fly (`the_shots_that_once_ended_the_process_fly`).

**This did not need the rebuild's contact identity.** The chain ends by the law's own convergence. The rebuild still has work here: those 97 exchanges are one contact in any plain sense, each with its own mark and shock.

## Reach on a real map

`panic_reach_on_shipped_ground` flies every fired round from a muzzle 2.2 m over the shipped ground, 60 to 600 m out with a clear line, lofted and led, at the shipped hull standing on that ground at a drawn spot, yaw and slope, still or moving at up to 12 m/s and 0.5 rad/s. Flights in a volley share a tick. Seeded by `REACH_SLICE`. The hull is a posed kinematic body, not driven through its tracks, with no damage consequence. A "touched" flight left an armour mark.

<!-- reach tables -->

**Can a player meet it?** The machine-gun rate is about one process end per 84 000 flights; a match with some thousands of rounds meets it about once in tens of matches (DERIVED, assuming a thousand or two touching rounds a match). Crossing limit: not in this count, since it needs a spent round on a nearly coplanar pair of faces.

## The last release

Read at `v0.7.0-alpha.9` with `git show`, not run.

- **Ricochet there is specular.** `walk::begin` blends toward the reflection, and `decide` resumes at `position + normal * radius + out * MARCH_EPS`. No heading lies along a face, and no line round's axis stays on one. The membership cause came with the subjects "ballistics: replace angle gates with pressure contact (#147)" and "ballistics: give skid contacts a clock or a terminal rest (#260)".
- **The ungrown `leave` is there,** without the skid that reaches it.
- **The `acos` in `bend_toward` is there,** but a skid there lifts off the face by the disc's radius and advances at least 1 mm per exchange, so the chain above cannot form (DERIVED).

So the release carries neither fixed cause, and `main` does. Whether it carries the still-open APCR panic is unknown until that cause is known.

## Still open

- **P1, `UnexpectedExit` on an APCR disc, live.** `kwk36_l56 88mm_APCR`, still hull, slice 0: `UnexpectedExit { sample: 4, key: ShellKey { volume: 213v0, primitive: 260v0, shell: 0 }, t: 0.002513847579125947, triangles: [48] }`. Same `t` before and after these fixes. It did not reproduce from any one shot of its volley alone, since an earlier shot in the volley can move the hull; the bed now prints the whole volley (`PANIC`). Not traced: the lane ran out of its hour. Rerun `REACH_SLICE=0` and replay that line.
- **P2, a contact is many exchanges.** About 100 marks and shocks for one rim rubbing a plate. The rebuild's contact identity.
- **P3, a line round within 1 mm of a face ends on it.** New reach of an existing outcome: about 1e-4 / cos(incidence) of line-round skids (DERIVED: 1 mm over 11.8 m of tick travel). An alternative is to lift it the rest of the 1 mm with no clock; it adds a teleport, so not taken.
- **P3, `contact` is a parry cast in world f32, the walker's hit is exact.** The difference sets a chain's step length.
- **P3, a mixed vertex fan reads as tangent.** Wrong at a saddle vertex (DERIVED, not measured).
