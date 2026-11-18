# Crush probe, slice 0b: the game's destruction model

Game state only: chunks, joints, contacts and loads. Branch `lane/crush-probe`, baseline `67e33cd7`; preceding report `1e46a8e7` contains experiment 0a. This commit retains only this report.

**Escape gate: PASS. Production decision: FAIL for arm W as tested.** In every repetition the already-beached control stays slow and belly-supported, while applying W releases the same loaded carriers and restores drive. But every collapsed-house pile loses all its loose simulation volume on its first traversal. This reaches the design's explicit absurd end. Do not ship the proposed rest-opening/sub-floor shedding combination unchanged. No law parameter, size floor, ground response or vehicle data was tuned.

## Method, denominators and scope

The final matrix contains **MEASURED three independent process executions per arm per configured start**, **MEASURED 30 executions**. Each start/arm's complete JSON stream is byte-identical across its repetitions. The ranges below describe execution repeatability, not randomized placement coverage. All table observations are **MEASURED**, including aggregates of measured samples; ratios, degree conversions, elapsed seconds from ticks, geometry-derived sizes and scalar-operation estimates are **DERIVED from those measurements**. A repeated endpoint is intentional. Profile vectors show the common value of each bin's degenerate repetition range.

The headless `Bed` mounts the shipped simulation plugin and authors the same standing ram, collapsed-house placement seeds and yard as experiment 0a. The fixture retains the original golden-ratio seed nudges. The controls' original drive endpoints reproduce 0a. The historical light-pedal heap stalls still do not reproduce: both controls traverse. The standing control does beach, although its trajectory differs from the historical issue trace.

- Both arms finish the shipped setup before intervention. **DERIVED 64 ticks/s** is the shipped tick rate. The standing run preserves the complete original **DERIVED 25 s** prefix, then continues for **DERIVED 15 s**. At intervention its demand is still full, speed is **MEASURED 0.02931–0.02931 m/s**, and its hull proxies carry **DERIVED 60.933–60.933%** of fixture weight. W therefore rescues an existing beach rather than merely avoiding a different approach.
- W starts at drive onset for the heap, free and yard starts. The heap drives last **DERIVED 40 s** at the existing light pedal. Free and yard drives use full demand and **DERIVED 25 s** per pass. Each free execution makes **MEASURED three passes** over the same evolving pile; between passes only the tank is repositioned through `Bed::place`. The pile, transmission and world retain their state. These are repeat traversals, not a model of turning around.
- At W activation existing retained edges become cracked, closed rests once. New size-floor rests can form only at a previously uncracked bond's first failure. A later opening cannot coarsen that edge again. The disposable implementation kept an edge-history list for instrumentation; a production implementation should delete it.
- W commits an unscaled press-only strength solve before the normal landing system, after the existing owner laws and physics writeback. It assesses every awake, unfrozen construction carrier not already pending retirement, including empty-contact owners. It preserves individual contact points and their moments. Presses use the shipped blow/press decomposition; they carry no blow peak, energy cap or plastic fragment-mass attenuation. The separate shipped blow pass keeps its energy cap. Both paths use the existing part/rest solver, ownership split and `route`/`Dust`/`Shed` path. The existing **DERIVED 16-pass** bound remains; no screen, filtering or budget is introduced.
- Existing owner laws remain active. In the rescue they open the belly carriers and prepare their retirement before the added press pass skips those pending splits. The reported result is for the combined W arm, not proof that its new press pass alone causes the escape. A rest judged open stays open in both paths. No tank pose, velocity or force is written by the intervention.

Fixture weight is **MEASURED 559,170 N**. Belly share below is positive net upward hull-proxy reaction divided by that weight, clipped at zero per tick; track support is the belt's sampled support plus soft-run and bump-stop solver reactions. Side plates are excluded. This whole-weight share differs from 0a's conditional share on hypothetical failing loose owners. Final height is the hull datum's world Y over level ground, not belly clearance; its rest datum is **MEASURED −0.013260 m**. Rest belly clearance remains **DERIVED 0.476604 m**, and the unchanged occupied-volume size floor is **DERIVED 0.500 m** from shipped data.

Removed volume counts unique matter leaving simulation, excluding buried matter and the surviving standing remnant. Collapsed-house starts contain **MEASURED 8.82118–8.82118 m³** of loose pile volume plus **MEASURED 1.19282–1.19282 m³** of standing non-buried material; the latter survives W. Removal shares use the loose pile as denominator. Counts distinguish chunks made dust, `Shed` groups, and entire existing server carriers retired with all their chunks dusted. Floating-point volume sums differ in their last accumulation places; displayed precision does not imply exact arithmetic conservation.

## 1. Hull support and restored drive

For `stuck`, means cover the intervention-to-end window. For the other starts they cover the drive, or all free passes. Every final-second belly mean in W is **MEASURED 0–0%**.

| Start | Arm | Mean belly share (%) | Final hull Y (m) | Final pitch (degrees, DERIVED) | Final forward speed (m/s) | Forward progress (m; stuck after intervention, others last pass) |
|---|---|---:|---:|---:|---:|---:|
| stuck | control | 64.03283–64.03283 | 0.163037–0.163037 | 3.052–3.052 | 0.02954–0.02954 | 1.823–1.823 |
| stuck | W | 0.06816–0.06816 | -0.013281–-0.013281 | 0.503–0.503 | 9.50738–9.50738 | 81.770–81.770 |
| heap-s2 | control | 0.00405–0.00405 | -0.013394–-0.013394 | 0.346–0.346 | 2.43315–2.43315 | 45.240–45.240 |
| heap-s2 | W | 0.00074–0.00074 | -0.013253–-0.013253 | 0.325–0.325 | 2.98762–2.98762 | 81.074–81.074 |
| heap-s6 | control | 0.48343–0.48343 | -0.009605–-0.009605 | 0.111–0.111 | 1.43564–1.43564 | 24.777–24.777 |
| heap-s6 | W | 0.00000–0.00000 | -0.013248–-0.013248 | 0.324–0.324 | 2.99584–2.99584 | 82.563–82.563 |
| free | control | 0.00903–0.00903 | -0.013251–-0.013251 | 0.317–0.317 | 10.63279–10.63279 | 184.492–184.492 |
| free | W | 0.00066–0.00066 | -0.013292–-0.013292 | 0.416–0.416 | 4.38363–4.38363 | 65.476–65.476 |
| yard | control | 0.09098–0.09098 | -0.013250–-0.013250 | 0.317–0.317 | 10.63278–10.63278 | 191.808–191.808 |
| yard | W | 0.09098–0.09098 | -0.013250–-0.013250 | 0.317–0.317 | 10.63278–10.63278 | 191.808–191.808 |

The standing control's final-second belly share is **MEASURED 69.371–69.371%**; W's is **MEASURED 0–0%**, and its final-second track share is **DERIVED 99.998–99.998%**. W advances **MEASURED 81.770–81.770 m** after the stuck moment against control's **MEASURED 1.823–1.823 m** of creep. **MEASURED three of three** W rescues pass the requested drive-on gate; all controls finish slow with demand applied.

The standing support history makes the change in load path visible. These are **DERIVED percentages** of measured fixture weight, averaged over the named simulation intervals; each cell is the repetition range.

| Drive interval (s, DERIVED) | Control belly / tracks (%) | W belly / tracks (%) |
|---|---|---|
| 0–6.25 | 5.559–5.559 / 95.840–95.840 | 5.559–5.559 / 95.840–95.840 |
| 6.25–25 | 67.045–67.045 / 32.985–32.985 | 67.045–67.045 / 32.985–32.985 |
| 25–25.25 | 67.116–67.116 / 32.934–32.934 | 4.090–4.090 / 52.698–52.698 |
| 25.25–26 | 64.755–64.755 / 35.364–35.364 | 0–0 / 112.933–112.933 |
| 26–40 | 63.939–63.939 / 36.045–36.045 | 0–0 / 100.087–100.087 |

Track reaction briefly exceeds weight while arresting the fall. The other starts have transient support encounters and zero final-second belly share in both arms; their complete time series are in the raw artifacts.

A first glancing belly contact is not the start of a beach. In the standing case it occurs at **MEASURED tick 109**, **DERIVED 1.703125 s** under the series' zero-based sample labels, while tracks already carry more than **DERIVED 90%**. The literal first-contact-to-first-threshold interval is therefore **DERIVED 0–0 s** in both arms. The meaningful recovery after the beach is:

| Standing recovery reading | Control | W |
|---|---:|---:|
| First track support above 90% after intervention (s) | 6.171875–6.171875, transient | 0.187500–0.187500 |
| First full 1 s track-support mean above 90%, confirmed after intervention (s) | Not reached in the remaining 15 s | 0.984375–0.984375 |
| First belly contact to that post-beach instantaneous crossing (s) | 29.468750–29.468750, transient | 23.484375–23.484375 |
| First belly contact to that full-window confirmation (s) | Not reached | 24.281250–24.281250 |

The full-second mean is an analysis window, not a delay or tuning applied to the law. The crossing heaps already have track support above the threshold at their first positive belly sample: their literal intervals are **DERIVED 0–0 s**. W `heap-s6` has no positive belly sample at all. Free control's literal interval is **DERIVED 0.156250–0.156250 s**, W's **DERIVED 0–0 s**; yard is **DERIVED 0–0 s** in either arm. These early crossings do not establish release from a beach. Free endpoints above are after the final repositioned pass, so the retained drivetrain makes them unsuitable for a fresh-start acceleration comparison.

## 2. What happened to the carriers and pile

`stuck` begins this accounting at intervention, with a shared **MEASURED 61–61 bodies** and **MEASURED 13.34947–13.34947 m³** loose volume. Other starts begin at drive onset. Counts are cumulative over the stated window; free counts include its passes, with all W removal on the first.

| Start | Arm | Bodies before → after (each a repetition range) | Chunks dusted | Shed groups | Volume removed (m³) | Initial loose pile removed (%) |
|---|---|---|---:|---:|---:|---:|
| stuck | control | 61–61 → 61–61 | 0–0 | 0–0 | 0.00000–0.00000 | 0.000–0.000 |
| stuck | W | 61–61 → 13–13 | 741–741 | 646–646 | 10.36697–10.36697 | 77.658–77.658 |
| heap-s2 | control | 15–15 → 32–32 | 0–0 | 0–0 | 0.00000–0.00000 | 0.000–0.000 |
| heap-s2 | W | 15–15 → 0–0 | 603–603 | 348–348 | 8.82118–8.82118 | 100.000–100.000 |
| heap-s6 | control | 15–15 → 33–33 | 0–0 | 0–0 | 0.00000–0.00000 | 0.000–0.000 |
| heap-s6 | W | 15–15 → 0–0 | 603–603 | 350–350 | 8.82118–8.82118 | 100.000–100.000 |
| free | control | 15–15 → 39–39 | 0–0 | 0–0 | 0.00000–0.00000 | 0.000–0.000 |
| free | W | 15–15 → 0–0 | 603–603 | 370–370 | 8.82118–8.82118 | 100.000–100.000 |
| yard | control | 108–108 → 108–108 | 0–0 | 0–0 | 0.00000–0.00000 | 0.000–0.000 |
| yard | W | 108–108 → 108–108 | 0–0 | 0–0 | 0.00000–0.00000 | 0.000–0.000 |

At the standing intervention the actual loaded carriers have **DERIVED sizes 0.55747–0.60822 m** and **MEASURED heights 0.52926–0.72672 m**. All their source chunks leave as dust/Shed in the intervention tick; they are absent from the post-commit body list, and next tick their belly support is zero. Thus the original rigid-carrier obstruction is removed in simulation, not inferred from a cloned solve. The remaining bodies are not a surviving belly bridge.

Height profiles use **MEASURED eight bins**, each collecting the highest transformed individual chunk-AABB point overlapping its ground rectangle; these are conservative chunk envelopes, not a sampled surface or a pile volume. Empty bins read ground height. For the standing case the rectangle is fixed at the intervention hull position, **DERIVED 7 m longitudinally by 4 m across**, so driving away cannot make the after-profile empty by changing the measurement location. Collapsed profiles cover the original settled pile rectangle, including material across its full width. The bin direction is increasing world Z. Each vector element is the same in all repetitions.

| Region | Before (m; each bin's range has equal endpoints) | Control after (m) | W after (m) |
|---|---|---|---|
| stuck | [2.512, 2.512, 2.512, 2.512, 0.511, 0.706, 0.616, 0.250] | [2.352, 0.702, 0.702, 0.583, 0.474, 0.474, 0.810, 0.810] | [0.000, 0.000, 0.000, 0.000, 0.000, 0.050, 0.050, 0.250] |
| heap-s2 | [3.009, 3.009, 3.009, 3.009, 3.009, 3.009, 3.009, 3.009] | [0.301, 0.300, 0.300, 0.300, 0.300, 0.250, 0.250, 0.250] | [0.000, 0.000, 0.000, 0.000, 0.000, 0.000, 0.000, 0.000] |
| heap-s6 | [3.009, 3.009, 3.009, 3.009, 3.009, 3.009, 3.009, 3.009] | [1.667, 1.667, 1.667, 1.667, 1.667, 1.667, 1.667, 1.667] | [0.000, 0.000, 0.000, 0.000, 0.000, 0.000, 0.000, 0.000] |
| free | [3.009, 3.009, 3.009, 3.009, 3.009, 3.009, 3.009, 3.009] | [1.670, 1.670, 1.668, 1.668, 1.667, 1.667, 0.449, 0.449] | [0.000, 0.000, 0.000, 0.000, 0.000, 0.000, 0.000, 0.000] |
| yard | [0.447, 0.584, 0.660, 0.710, 0.710, 0.686, 0.644, 0.541] | [0.516, 0.516, 0.549, 0.549, 0.544, 0.554, 0.554, 0.505] | [0.516, 0.516, 0.549, 0.549, 0.544, 0.554, 0.554, 0.505] |

The tall envelope bins include material beside the belly within that rectangle; they are not the measured supporting carriers' heights. For the standing start there is no loose pile before the ram, so its original `Site` width is undefined; only the fixed intervention rectangle is used here. Yard volume is **DERIVED from measured body mass divided by its shipped fixture density**, because those single-chunk bodies have no construction ledger or joints. Its two arms are byte-identical.

## 3. Free-running repeated passes

All observations below repeat identically in each execution. The original pile contains **MEASURED 15–15 bodies**, **MEASURED 603–603 chunks** and **MEASURED 8.82118–8.82118 m³** loose volume. Both arms begin with the same **MEASURED 3.009–3.009 m** maximum height envelope.

| Arm | Pass | Removed this pass (m³) | Initial pile removed this pass (%) | Whole server carriers dusted this pass | Shed groups / chunks this pass | Bodies left | Pile maximum height after pass (m) |
|---|---|---:|---:|---:|---|---:|---:|
| control | 1 | 0.00000–0.00000 | 0.000–0.000 | 0–0 | 0–0 / 0–0 | 39–39 | 1.668–1.668 |
| control | 2 | 0.00000–0.00000 | 0.000–0.000 | 0–0 | 0–0 / 0–0 | 39–39 | 1.668–1.668 |
| control | 3 | 0.00000–0.00000 | 0.000–0.000 | 0–0 | 0–0 / 0–0 | 39–39 | 1.670–1.670 |
| W | 1 | 8.82118–8.82118 | 100.000–100.000 | 26–26 | 370–370 / 603–603 | 0–0 | 0.000–0.000 |
| W | 2 | 0.00000–0.00000 | 0.000–0.000 | 0–0 | 0–0 / 0–0 | 0–0 | 0.000–0.000 |
| W | 3 | 0.00000–0.00000 | 0.000–0.000 | 0–0 | 0–0 / 0–0 | 0–0 | 0.000–0.000 |

**The “essentially gone after one pass” absurd end occurs: all loose pile matter and all loose bodies are gone in W.** The standing remnant is not a surviving rubble bed. The controls lose no volume but do change: they split into more carriers and their height lowers as they move and settle. W's later unchanged passes mean the pile was already deleted, not that a lower bed has stopped yielding. This is a failure of the proposed production behavior despite the escape gate passing.

## 4. Motion and popping

Velocity is measured from the hull body. Acceleration below is **DERIVED as adjacent-tick Δvertical_velocity / dt**, not Avian substep or acceleration inside a contact impulse. Windows include a whole-carrier pop tick and the following **DERIVED 1 s**, including pops away from the tank; they conservatively cover motion coincident with giving way without claiming every pop caused it. Each extrema value is identical across repetitions.

| W start | Vertical velocity extrema (m/s) | Vertical acceleration extrema (m/s², DERIVED) | Largest downward hull excursion in these windows (m) |
|---|---|---|---:|
| stuck | -1.1856–-1.1856 to 0.3744–0.3744 | -8.9496–-8.9496 to 6.5322–6.5322 | 0.29314–0.29314 |
| heap-s2 | -0.0161–-0.0161 to 0.0406–0.0406 | -0.7945–-0.7945 to 1.4685–1.4685 | 0.00373–0.00373 |
| heap-s6 | -0.0120–-0.0120 to 0.0380–0.0380 | -0.8720–-0.8720 to 1.3635–1.3635 | 0.00249–0.00249 |
| free | -0.0117–-0.0117 to 0.0230–0.0230 | -1.0253–-1.0253 to 2.2235–2.2235 | 0.00234–0.00234 |
| yard | N/A: no whole-carrier pop | N/A | N/A |

For the actual standing belly carriers, the shared drop is **MEASURED 0.29314–0.29314 m**, below even the shortest loaded carrier's **MEASURED 0.52926–0.52926 m** height. Its upward rebound peaks at **MEASURED 0.37437–0.37437 m/s**, not an upward launch at removal. No measured hull excursion exceeds the height of a known belly-supporting carrier that gave way. Comparing the same drop to every simultaneous distant pop would falsely attribute it to unrelated carriers as short as **MEASURED 0.23969 m**. The heap and free excursions are smaller than **MEASURED 0.004 m**. No launch or excessive layer drop is demonstrated by the recorded motion. Visual quality and substep peaks remain unestablished.

A whole-carrier pop requires all chunks of an already-existing server body to be routed as dust in one tick, with no kept child. Newly released fragments that were always sub-floor are not counted as pops. The size column is occupied-volume cube root, not vertical height. Controls and yard have **MEASURED 0–0** such pops.

| W start | Whole-carrier pops over run | Carrier size range (m, DERIVED) | Separate ticks with pops | Maximum material removed in one tick (m³) |
|---|---:|---|---:|---:|
| stuck | 48–48 | 0.50133–0.73469, identical each repetition | 2–2 | 10.19391–10.19391 |
| heap-s2 | 31–31 | 0.50030–0.79921, identical each repetition | 19–19 | 1.83539–1.83539 |
| heap-s6 | 29–29 | 0.50144–0.81854, identical each repetition | 18–18 | 1.83539–1.83539 |
| free | 26–26 | 0.50217–0.80925, identical each repetition | 17–17 | 1.83539–1.83539 |
| yard | 0–0 | N/A | 0–0 | 0.00000–0.00000 |

The standing intervention removes **MEASURED 47–47 whole carriers in its first tick**, then **MEASURED 1–1** in the next. Its first-tick volume loss is **DERIVED 76.362–76.362%** of the loose pile. No complete multi-body pile disappears in one tick: the free pile's largest one-tick loss is **DERIVED 20.807–20.807%**, and it disappears across the encounter. Whole welded carriers nevertheless do pop, in large numbers. A graceful hull drop does not excuse that material-loss result.

## 5. Cost in counts

Awake census is after structural commit on each tick, excluding sleeping and frozen pieces. It therefore excludes carriers already removed that tick: entering the standing intervention there are **MEASURED 50–50 awake bodies**, although its first post-commit census is much smaller. The count table spans each complete drive including the shared standing prefix, or all free passes. Scalar operations are **DERIVED algorithmic counts** of part/kernel factors and solves: added press solves, existing landing solves, and `evaluate_owner`'s existing joint-law assessments. They exclude Avian contact solving, sweep casting, intake, spawning and decoration; they are neither instructions nor hosted timings.

| Start | Arm | Awake bodies / tick, min–max | Mean awake bodies (repetition range) | Press assessments / tick, min–max | Joint-solver scalar ops / tick, max (repetition range) | Mean scalar ops / tick (repetition range) |
|---|---|---:|---:|---:|---:|---:|
| stuck | control | 0–58 | 48.7383–48.7383 | 0–0 | 6,664,388–6,664,388 | 36501.03–36501.03 |
| stuck | W | 0–58 | 29.8160–29.8160 | 0–22 | 6,664,388–6,664,388 | 37642.48–37642.48 |
| heap-s2 | control | 0–32 | 17.5234–17.5234 | 0–0 | 1,858,447–1,858,447 | 5256.29–5256.29 |
| heap-s2 | W | 0–15 | 0.2715–0.2715 | 0–12 | 5,570,507–5,570,507 | 20672.12–20672.12 |
| heap-s6 | control | 0–33 | 21.8367–21.8367 | 0–0 | 1,727,977–1,727,977 | 4934.14–4934.14 |
| heap-s6 | W | 0–15 | 0.2051–0.2051 | 0–14 | 5,760,786–5,760,786 | 16428.50–16428.50 |
| free | control | 0–39 | 4.7917–4.7917 | 0–0 | 1,456,849–1,456,849 | 2732.95–2732.95 |
| free | W | 0–12 | 0.0848–0.0848 | 0–12 | 6,857,727–6,857,727 | 9048.85–9048.85 |
| yard | control | 0–108 | 15.1719–15.1719 | 0–0 | 0–0 | 0.00–0.00 |
| yard | W | 0–108 | 15.1719–15.1719 | 0–0 | 0–0 | 0.00–0.00 |

After the standing intervention alone, control has **MEASURED 49–53 awake bodies/tick**, averaging **MEASURED 50.56875–50.56875**, while W has **MEASURED 0–3**, averaging **MEASURED 0.109375–0.109375**. Their mean joint-solver work is respectively **DERIVED 1,917.06–1,917.06** and **DERIVED 4,960.93–4,960.93 scalar operations/tick**; W peaks at **DERIVED 1,458,851–1,458,851** there. W's added press pass peaks at **DERIVED 461,547–461,547 operations/tick** in that window. Lower later body counts come from removing the pile, not evidence that a retained rest bed is cheap. On the collapsed piles W increases joint-solver work even while almost eliminating the later awake population. No hosted solve budget is established.

## 6. Verdict stability

Source joint identities survive ownership changes. The probe checks stored closed/open states across ticks and child owners, records rest openings in both the added press and existing law paths, and detects a closed state returning after an observed opening. It also checks that a previously cracked joint is never eligible to form a new weld. The initial baseline-to-W conversion is the declared intervention, not counted as re-welding a W-opened rest.

| W start | Distinct rest openings recorded | Opens then re-welds | Open→closed on adjacent ticks or across later ownership |
|---|---:|---:|---:|
| stuck | 598–598 | 0–0 | 0–0 |
| heap-s2 | 296–296 | 0–0 | 0–0 |
| heap-s6 | 303–303 | 0–0 | 0–0 |
| free | 313–313 | 0–0 | 0–0 |
| yard | 0–0 | 0–0 | 0–0 |

Every reported reversal count is **MEASURED 0–0**. A held rest eventually opening is its legitimate one-way transition, not an oscillating verdict. The experiment does not re-close an open edge to obtain a hypothetical next-tick “hold” answer: doing that would test a different state machine. It therefore establishes committed-state monotonicity, not solver-noise stability of a hypothetical reversible near-threshold contact or a delayed solve budget.

## Production shape and decision

The deciding truth is now observed: restoring the missing rest semantics can release the belly carriers. This fixture does not require a lower size floor or ground yield to restore drive. Neither alternative was tested, and the escape result cannot establish whether a sound single chunk on hard ground could still beach a tank. The present failure is excessive disappearance after opening, not an unbroken block remaining under the belly. Keep the next investigation at that seam; do not lower numbers merely to obtain a nicer outcome.

A production version of the tested arm would change these functions and interfaces:

- `debris::crumble::{landings, touches, crumble, crumble_landings}`: delete the blow-only gate; retain press and blow paths separately, use full carried press with strength-only commitment, preserve contact wrenches, and keep the blow's energy cap and mass scaling confined to blows. The new press work must share the existing ownership-parallel preparation and ordered commit boundary; the disposable exclusive system is not the production scheduling design.
- `merge_fragments`: weld only at a bond's first failure, writing cracked/closed state. Delete the `retained` argument and its old-edge unconditional union path. A rest opening routes its freed sub-floor fragments through existing `route`; it does not create another weld.
- `construction::breakage::{Live, evaluate_owner}` and `breakage::sweep_intake::cut_fragments`: delete `rubble_joints`, its construction/clone/child filtering, and pass the first-bond-failure information and mutable state through coarsening. Keep subsequent rest openings monotone. Update event preparation after coarsening so it reports the final joint state.
- `route`, `server_body`, `Shed`, `PendingSplit::of` and `split_groups` already provide the tested size decision, dust/decor and ownership/body retirement. They need no replacement law. `Assessment::landing`/the part graph already judge cracked, closed joints as push/friction rests; no new lift/tip/slide law was invented.

**DERIVED rough implementation scope, not a completed production diff:** about **120–180 changed/added Rust lines** across those seams, **50–80 deleted Rust lines** for retained-edge storage/union and gate plumbing, plus about **50–100 lines** of focused state-transition/ledger tests and the moving-group exception's documentation. These estimates exclude a screen, budget, new material representation, bulk bearing and a solution to the measured disappearance failure. The temporary probe and counters are substantially larger and are all removed.

**No new replicated event variant or wire-schema change is inherently needed.** Existing `Dust` entries and `Shed` messages carry the measured removal. Applied to a still-closed joint, existing `Crack` removes cohesion without opening it; existing `Separate` opens it (`Shape::apply`). They can carry the rest lifecycle from construction spawn. Dust/Shed alone are not sufficient for joints that remain in simulation: emit `Crack` when a first-failure weld closes and `Separate` when it later opens. Current callers prepare separation causes before coarsening, so production must correct that event emission rather than append an undo/re-weld event after `Separate`. The disposable arm updates server live state and removal records but does not implement or validate that joint-ledger correction or a network replica. The artificial mid-run conversion of baseline welds is not represented to a replica: `Crack` cannot undo an already-recorded `Separate`. The no-new-variant production shape assumes the corrected lifecycle starts from construction spawn, not a live migration of an old ledger. No new event type was added to make this probe work.

**Decision:** retain the PASS on the requested rescue gate, reject W's measured pile behavior, and keep this lane report-only. The data does not support landing the current slice-2 dust policy as a credible lower rubble bed. A smaller-body representation might retain matter at higher solver cost, but its required size, cost and feel remain unknown; ground yield is also untested. Neither is promoted to the missing truth by this experiment.

## What remains unestablished

- Exact reproduction of the historical #374 trajectory or historical heap stalls; these are the current shipped-plugin scenarios and one confirmed persistent control beach.
- Which rest-opening causes dominate the pile-wide failure, or how much the existing spin/owner-law path contributes compared with the added press pass. The combined arm is tested; a press-only/rest-only factorial comparison is not.
- A plausible persistent rubble bed, smaller server bodies, mound/decor collision, ground yield, bulk bearing, or a tuned alternative. None was substituted after seeing W fail its material-preservation check.
- Visual popping/decoration quality, Avian substep velocity/acceleration peaks, or an exact occupied-surface height raster. The report supplies chunk envelopes and tick-level motion only.
- Hosted performance, parallel production scheduling, a screening rule, a solve budget, or safe load filtering near a live rest's threshold.
- Replica/ledger compatibility in an end-to-end session. The existing event vocabulary is sufficient by source inspection; the necessary emission changes were not built or shipped.

## Artifacts, restoration and checks

The final streams remain ignored in this bay at `target/crush0b-{arm}-{case}-s{seed}-r{rep}.jsonl`, with corresponding `.log` files. Each arm/start's hash below applies to all repetitions. Temporary development runs are not used as independent evidence for these ranges. Final execution used the same compiled `edit`-profile library test, `crush_zero_b_probe --ignored --nocapture --test-threads=1`, in separate processes with `CRUSH_ARM`, `CRUSH_CASE`, `CRUSH_SEED` and `CRUSH_REP`. Every compilation used `scripts/serial-cargo` with `--features dev_ui,bitprobe`. Headless environment: `OVERMATCH_UNATTENDED=1 OVERMATCH_MUTE=1`.

| Start | Arm | Final JSONL SHA-256 |
|---|---|---|
| stuck | control | `e884368dcc9f46313e16a919b2d4a626a1a73fa8d5bae63ec322e869daaf56ae` |
| stuck | W | `b5e63fcddfe6f10be6a5a587838f975757f59ab0d3557761e16555664ab1efc0` |
| heap-s2 | control | `c8a9eab76828110747052bd8a6eb49246dbdcea85a5427a8296d9684c7fdc90a` |
| heap-s2 | W | `fc95edf5413a9155f60a92418c095a52db609d94632c076ef67c97b146bce388` |
| heap-s6 | control | `ca21a41fb0ef40fb2add8ed3c9aafbec0a9d5e96f3b5db4486b9b53d641a6bc4` |
| heap-s6 | W | `341d1eae02f87cc533fecbf50f2c1f6823164f88bee8a4a948b5ba730012ce22` |
| free | control | `b70ec3537843a09bbe89c420b53e0b9041f0be1af7d920f12ce2e0dbaf1c5198` |
| free | W | `200623f038d237cbfca24444b2f0756ebbbcd5cb902497b483d3af5984fa3f9c` |
| yard | control | `a88be46c4a4d37143dc6a810b5b5f5ab1894c88533f2fc47a015b8fc32289da7` |
| yard | W | `a88be46c4a4d37143dc6a810b5b5f5ab1894c88533f2fc47a015b8fc32289da7` |

All probe Rust edits were restored before validation. `git diff --exit-code -- src` is clean. `cargo fmt --all --check` passed. `scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings` passed. `scripts/serial-cargo test --lib --features dev_ui,bitprobe debris::crumble` passed (**MEASURED 11 tests**). No production code, feature switch, probe test or data edit is retained; no debugger, display/audio run, forbidden document read, damage-topic web search, GitHub comment or push was used.
