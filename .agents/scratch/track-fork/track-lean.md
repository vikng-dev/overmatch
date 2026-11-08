# Track lean: force-based boat

Subject: the game's tank driving physics, game state only. The support law, per-element friction circle and transmission remain the boat's laws. No timing runs or captures.

## Terrain cut

One height-grid facet plane is sampled at each column midpoint, then reused for its three longitudinal depths and surface evaluations. Solids and pieces retain their exact convex footprint overlay. The view retains its original ground oracle.

DERIVED scope: depth is exact within a facet while the profile probes remain exposed. Creases, finite-map coverage and entry into the inward reach/burial window are sampled. This gives up exact terrain pressure coverage and exact crease resultants. Rounded analytic blocks retain their refinement.

MEASURED: the retired path performed 582 terrain overlays per tank per tick (temporary counter, removed with that path). At 97 stations and three lateral points the replacement performs 582 facet-plane samples and zero terrain triangulations/overlays. The production counter is `belt_terrain_samples`; solid triangulations remain cached once per collider per tick.

MEASURED fixtures: empty 64 m square, 1 m cells; flat, slope h=0.1z, and a 0.15 m crease crossing the spawn. The shipped plugin/tank is fixture data. Settle for 192 ticks, then drive for 128 ticks; both at the production fixed rate. Work counters cover two tanks and are divided by two; the following contacts/loads/poses belong to the controlled tank. All outputs are observations, not gates.

| Terrain/phase | Contacts before/after | Load N before/after | Hull XYZ m before | Hull XYZ m after |
|---|---:|---:|---|---|
| flat/settle | 889/198 | 553735.562/553530.875 | 10.43515, -0.058797598, 4.8683033 | 10.437687, -0.058770597, 4.8583374 |
| flat/drive | 765/198 | 554325.938/554524.875 | 10.228305, -0.058469355, 2.406871 | 10.223877, -0.058517396, 2.4021754 |
| incline/settle | 829/204 | 553046.188/553187.938 | 10.290286, 0.41865897, 4.7784314 | 10.39483, 0.41086328, 4.7024455 |
| incline/drive | 671/204 | 555988.562/555857.000 | 9.977908, 0.09765065, 1.5653402 | 9.994781, 0.0914827, 1.5034467 |
| crease/settle | 957/205 | 555272.250/554841.875 | 10.289513, -0.049774706, 4.979911 | 10.288524, -0.04928136, 4.979029 |
| crease/drive | 876/200 | 555326.000/555521.500 | 10.258291, -0.04848826, 2.48849 | 10.253947, -0.047824085, 2.4861095 |

| Terrain/phase | World force XYZ N before | World force XYZ N after |
|---|---|---|
| flat/settle | -3110.2283, 553735.56, -14183.872 | -3274.7947, 553530.9, -14683.698 |
| flat/drive | -3827.0542, 554325.94, -60159.594 | -4115.9067, 554524.9, -61119.96 |
| incline/settle | -4842.631, 554437.25, -13666.423 | -6393.303, 554337.1, -16096.965 |
| incline/drive | -1483.3771, 556726.6, -20348.352 | -1642.5957, 556612.1, -20171.326 |
| crease/settle | -1518.9539, 555648.75, -5212.8667 | -1664.0763, 555460.06, -5561.7354 |
| crease/drive | 736.0013, 556459.4, -59839.41 | 740.22754, 556697.8, -60047.707 |

## Validation changes

MEASURED: the corner fixture's peak overlap changed from 0.000961304 m to 0.12753296 m; late compression stayed below 0.001 m. The base passes its original arrival bound; the sampled path exposes its omitted contact-discovery interval. The bound now includes one fixed tick of hull-point travel. DERIVED by Cauchy-Schwarz in the mass metric: point speed <= sqrt(2E / minimum effective mass). Avian 0.7 refreshes manifolds once per physics tick outside the substep loop. The static and no-growth checks remain. No engine/contact parameter was tuned.

The grid/convex fixture retains surface-point, normal and material contracts, and prints quadrature-dependent force, torque and load differences. Exact equality of whole-rig resultants is no longer a premise of the sampled adapter.

Green terrain step: formatting, CI clippy, 337 track/boundary/doc checks and the observation bed. The initial wider run passed 589 checks, including construction and the other headless checks; its two failures were the boundary and equivalence checks repaired above.

## Plain waste cut

A cached componentwise bound on all grid facet gradients proves a direction back-facing before any terrain sample. This retains steep-terrain end-wheel contact and analytic ceilings; solids always keep the complete outline. The bounds are DERIVED once from immutable grid samples. Terrain triangulation is gone; solid triangulation was already cached once per collider per tick, and clipping is once per link before columns.

The diagnostic digest and its 1,746 logarithms per tank/tick (DERIVED at 97 x 3) are absent without tests/bitprobe. Shipped server builds in `.github/actions/build-server/action.yml` and `.github/workflows/release.yml` enable neither optional feature. The entire contact-collection bracket is `BeltContacts` / `belt_contacts_us`. The now-unused collider-less convex-face case was removed.

MEASURED terrain samples after rejection, per-tank end-tick averages over the two fixture tanks: 288–292.5, versus 582 before rejection. Fixture forces and poses agree with the prior sampled path. Formatting, CI clippy, track/doc/server-stats checks and the observation bed passed.

## Session 1 boundary

Steps 1–2 were green at the first session's 90-minute boundary. The resolution prototype was saved outside the active Rust tree for continuation. Production remained at 97 stations and three lateral points.

## Session 2: force resolution

Force stations are independent of drawn links. `CONTACT_RESOLUTION` owns their count and lateral quadrature; the shipped setting remains **97 × 3 Simpson nodes**. Grip fields are sized synchronously at spawn for stations × nodes, and their diagnostic reader uses that stride. The sandbox's live probe and grip views draw the selected force resolution. The `TrackGripElements::for_links` constructor name remains intact, preserving the network attachment guard.

`TrackGear::from_construction` runs the existing support calibration at the selected resolution. The friction, support and transmission laws are unchanged. Only test fixtures rebuild their force field before their first fixed tick; production simulation state remains spawn-built.

DERIVED bounds: at least three stations enclose a loop. Its perimeter divided by station count bounds the unresolved arc interval. The fixture’s nominal material-belt length is DERIVED as 12.651316 m from link count × measured pitch, giving nominal interval scales 0.1304 / 0.2636 / 0.5271 / 0.7907 m at 97 / 48 / 24 / 16 stations; the exact bound uses the force loop’s closed polyline perimeter. Fewer stations replace more of the curved outline with chords and coarsen the advected shear field; a sharp feature inside an interval can change contact onset and load placement.

The lateral rules keep the complete measured shoe bounds. Three Simpson nodes use edge/centre/edge weights 1/6, 2/3, 1/6; two Gauss nodes lie at centre ± width/(2√3), with equal weights. DERIVED: both integrate linear-pressure force and roll moment exactly. MEASURED fixture width 0.7247143 m gives DERIVED node gaps 0.36236 m and 0.41841 m, respectively, below the data-owned nominal 0.5 m piece size floor. A volume floor is not a minimum piece width. Solid/piece admission here clips the full-width footprint independently of nodes, so their detection does not rely on this gap. Two nodes give up direct terrain samples at both shoe edges and the centre.

### Work matrix

MEASURED with the shipped plugin in a generic empty flat grid, over the first 128 ticks of a straight full-throttle launch. The world has two tanks; the counter is divided by two. Ranges are end-tick per-tank averages, not timings. Contacts and final pose belong to the controlled tank.

| Stations/side | 3 Simpson: terrain queries/tank/tick | 2 Gauss: terrain queries/tank/tick | Contacts at final tick, 3 / 2 |
|---:|---:|---:|---:|
| 97 | 285–288 | 190–192 | 204 / 136 |
| 48 | 138–141 | 92–94 | 105 / 72 |
| 24 | 69–72 | 46–48 | 57 / 36 |
| 16 | 45–48 | 30–32 | 42 / 28 |

The original exact terrain path made 582 overlays/tank/tick; the sampled path before return-run rejection made 582 facet queries. Only 16 × 2 reaches the requested 10–40-query range in this launch fixture. Work on solids and pieces remains the exact convex path.

MEASURED one-time comparison: at 97 × 3 the flat/incline/crease observation bed reproduces every session-1 sampled contact count, force, load and printed hull pose above exactly. No output-equality test was added. Across resolutions the launch end positions vary; these are observations for the card, never gates.

Validation: formatting and CI clippy passed; 663 targeted track, sandbox, headless, construction, scatter, state-hash and whole-tree checks passed. The adjusted sandbox calibration and doc guards passed, as did the eight-setting work matrix and three-terrain observation bed. No timing runs, windows or captures.

## Session 2: scorecard sweep

Merged `lane/track-score` through **“track: add one-lane walls and lateral reach to the driving scorecard (#198)”**. This reader uses `ContactTransfers`, labels collider roles and includes the under-track layer and one-lane walls. The card itself is unchanged by this arm; only `src/headless_test/track_scorecard/arm.rs` selects force resolution before the first fixed tick. `scripts/track-resolution-scorecard` runs all eight settings, one locked, unattended, headless test process per setting. Its selector exists only in the test seam.

**All eight cards passed their slope and energy-decay law checks.** Reaction symmetry remains an observation because this arm retains the boat’s clamped recipient law. Every other result below is an observation. Each setting links to its complete Markdown card; the adjacent JSON file retains unrounded values. The boat column is the score lane’s MEASURED, rounded baseline in [track-score.md](track-score.md), not a new baseline run by this lane.

### Work beside each card

MEASURED: the query column comes from the launch matrix above. The card’s open-ground work covers one tank over simulated seconds 5–10; its piece-layer work covers the parked four-second interval. These are different workloads. The fixed card reader does not display `belt_terrain_samples`, so that production counter is reported separately rather than relabelled as another card counter. Terrain queries are grid-facet plane lookups, not Parry casts. The 10–40 range is reached by 16 × 2 on this flat fixture; it is not a bound for all terrain orientations.

| Setting / complete card | Launch queries/tank/tick | Open belt contacts | Open force applications | Layer belt contacts | Layer slab checks | Layer solver points |
|---|---|---|---|---|---|---|
| Boat baseline | 582 overlays | 786 | 1572 | 693 | 1358 | 111 |
| [97x3](track-lean-cards/97x3.md) | 285–288 | 203.25 | 406.50 | 693.35 | 1358 | 111.46 |
| [97x2](track-lean-cards/97x2.md) | 190–192 | 135.56 | 271.12 | 506.82 | 1358 | 111.27 |
| [48x3](track-lean-cards/48x3.md) | 138–141 | 103.61 | 207.23 | 435.95 | 672 | 111.08 |
| [48x2](track-lean-cards/48x2.md) | 92–94 | 69.15 | 138.30 | 312.13 | 672 | 111.35 |
| [24x3](track-lean-cards/24x3.md) | 69–72 | 54.64 | 109.28 | 320.94 | 336 | 111.53 |
| [24x2](track-lean-cards/24x2.md) | 46–48 | 36.42 | 72.85 | 221.66 | 336 | 111.39 |
| [16x3](track-lean-cards/16x3.md) | 45–48 | 38.27 | 76.54 | 303.89 | 224 | 111.25 |
| [16x2](track-lean-cards/16x2.md) | 30–32 | 25.51 | 51.03 | 218.18 | 224 | 111.24 |

DERIVED from those MEASURED means: 16 × 2 makes 30.8 times fewer open-ground belt contacts than the boat baseline (786 / 25.513), and 8.0 times fewer than lean 97 × 3 (203.25 / 25.513). This is a work reduction, not a timing or whole-server speedup. The exact piece path still makes 218 contacts and 224 slab checks per tick on the under-track layer at 16 × 2.

### Slope and turning (card lines 1–2)

MEASURED. Settle includes initial suspension motion as well as shear recovery. Turn ratios are wide-short / tight-short / wide-long / tight-long. Pivot yaw is its magnitude. Every below-limit hold and above-limit motion check passed on all five grounds at every setting.

| Setting | Settle range, mm | Achieved / geared turn radius | Pivot yaw, rad/s |
|---|---|---|---|
| Boat baseline | 78–123 | 1.28 / 1.27 / 1.35 / 1.33 | 0.129 |
| [97x3](track-lean-cards/97x3.md) | 77.7–122.9 | 1.285 / 1.282 / 1.350 / 1.336 | 0.1286 |
| [97x2](track-lean-cards/97x2.md) | 77.7–122.9 | 1.285 / 1.285 / 1.350 / 1.335 | 0.1288 |
| [48x3](track-lean-cards/48x3.md) | 75.5–118.7 | 1.283 / 1.280 / 1.346 / 1.332 | 0.1290 |
| [48x2](track-lean-cards/48x2.md) | 75.4–118.4 | 1.284 / 1.284 / 1.346 / 1.332 | 0.1292 |
| [24x3](track-lean-cards/24x3.md) | 74.4–116.1 | 1.275 / 1.270 / 1.332 / 1.317 | 0.1305 |
| [24x2](track-lean-cards/24x2.md) | 74.4–116.1 | 1.275 / 1.275 / 1.331 / 1.317 | 0.1308 |
| [16x3](track-lean-cards/16x3.md) | 72.0–115.7 | 1.258 / 1.253 / 1.310 / 1.296 | 0.1332 |
| [16x2](track-lean-cards/16x2.md) | 72.0–115.7 | 1.258 / 1.257 / 1.309 / 1.296 | 0.1333 |

### Acceleration (card line 3)

MEASURED speed at 40 simulated seconds, m/s. Ground order is fixed below. Every setting finishes in F8 / F8 / F8 / F5 / F6 respectively. Complete launch traces and times to 5 m/s are in each card.

| Setting | Concrete | Dirt | Gravel | Mud | Sand |
|---|---|---|---|---|---|
| Boat baseline | 10.63 | 10.34 | 10.58 | 3.23 | 4.83 |
| [97x3](track-lean-cards/97x3.md) | 10.6346 | 10.3413 | 10.5847 | 3.2333 | 4.8284 |
| [97x2](track-lean-cards/97x2.md) | 10.6346 | 10.3414 | 10.5848 | 3.2335 | 4.8285 |
| [48x3](track-lean-cards/48x3.md) | 10.6349 | 10.3419 | 10.5851 | 3.2348 | 4.8292 |
| [48x2](track-lean-cards/48x2.md) | 10.6349 | 10.3422 | 10.5851 | 3.2350 | 4.8294 |
| [24x3](track-lean-cards/24x3.md) | 10.6355 | 10.3435 | 10.5857 | 3.2376 | 4.8307 |
| [24x2](track-lean-cards/24x2.md) | 10.6354 | 10.3436 | 10.5857 | 3.2378 | 4.8311 |
| [16x3](track-lean-cards/16x3.md) | 10.6362 | 10.3456 | 10.5861 | 3.2401 | 4.8323 |
| [16x2](track-lean-cards/16x2.md) | 10.6363 | 10.3457 | 10.5862 | 3.2403 | 4.8323 |

### Obstacles and lateral support (card lines 4–6)

MEASURED. At every setting, the 0.2 and 0.4 m steps are crossed and the 0.6–1.2 m steps are not; the first three trenches are crossed and the 1.25-span trench is not. These outcomes are not gates. The table preserves changes inside those shared outcomes. The baseline summary does not report the 0.4 m step time.

| Setting | 0.4 m step time, s | 0.75-span trench time, s | 1.25-span trench max sink, m | Curb roll, degrees | Half-ledge roll, degrees |
|---|---|---|---|---|---|
| Boat baseline | unreported | 6.13 | 0.943 | 0.30 | −3.8 |
| [97x3](track-lean-cards/97x3.md) | 5.250 | 6.125 | 0.943 | 0.298 | -3.809 |
| [97x2](track-lean-cards/97x2.md) | 5.250 | 6.125 | 0.946 | 0.307 | -3.791 |
| [48x3](track-lean-cards/48x3.md) | 5.250 | 6.109 | 0.907 | 0.297 | -3.806 |
| [48x2](track-lean-cards/48x2.md) | 5.250 | 6.109 | 0.904 | 0.307 | -3.764 |
| [24x3](track-lean-cards/24x3.md) | 5.344 | 6.078 | 0.901 | 0.298 | -3.759 |
| [24x2](track-lean-cards/24x2.md) | 5.344 | 6.078 | 0.951 | 0.307 | -3.732 |
| [16x3](track-lean-cards/16x3.md) | 5.703 | 6.047 | 0.936 | 0.300 | -3.630 |
| [16x2](track-lean-cards/16x2.md) | 5.812 | 6.047 | 0.928 | 0.309 | -3.605 |

MEASURED wall pushes (line 4b): upward-positive belt vertical force, mean over the last two seconds, in kN. Every one-lane case registers some belt contact during the push, including 16 × 2. No setting climbs a wall. The full-width wall meets the hull proxy first and has zero belt-channel force at every setting. Line 0’s construction and lateral-reach readings remain in every complete card; no collider or authored geometry changes in this lane.

| Setting | One-lane first touch, both lanes | Left belt mean, kN | Right belt mean, kN | Maximum rise, mm |
|---|---|---|---|---|
| Boat baseline | belt + bump stop | 0.93 | 0.95 | 3.7 |
| [97x3](track-lean-cards/97x3.md) | belt, bump stop | 0.803 | 1.533 | 3.73 |
| [97x2](track-lean-cards/97x2.md) | belt, bump stop | 1.370 | 1.751 | 3.73 |
| [48x3](track-lean-cards/48x3.md) | belt, bump stop | 0.477 | 0.861 | 3.59 |
| [48x2](track-lean-cards/48x2.md) | belt, bump stop | 0.574 | 0.939 | 3.59 |
| [24x3](track-lean-cards/24x3.md) | bump stop | 0.232 | 0.345 | 3.70 |
| [24x2](track-lean-cards/24x2.md) | bump stop | 0.164 | 0.329 | 3.70 |
| [16x3](track-lean-cards/16x3.md) | bump stop | 0.053 | 0.127 | 5.38 |
| [16x2](track-lean-cards/16x2.md) | bump stop | 0.000 | 0.000 | 5.38 |

### Ride and the under-track layer (card lines 7–8)

MEASURED. All kick-decay checks pass; last peak means the final half-second’s peak hull kinetic energy. The layer carries about 100% of hull weight through the pieces, with about 92% through the belt channel. Its reaction ratio stays about 0.08: the known clamped-reaction asymmetry survives every resolution, with no belt-loaded piece asleep. Coarsening has not supplied the missing reciprocal load.

| Setting | Washboard pitch RMS, degrees | Washboard vertical speed RMS, m/s | Last kick peak, J | Layer reaction ratio | Layer height jitter RMS, mm |
|---|---|---|---|---|---|
| Boat baseline | 0.16 | 0.023 | 11 | 0.081 | 0.10 |
| [97x3](track-lean-cards/97x3.md) | 0.1631 | 0.02281 | 10.81 | 0.0806 | 0.121 |
| [97x2](track-lean-cards/97x2.md) | 0.1627 | 0.02282 | 10.81 | 0.0805 | 0.125 |
| [48x3](track-lean-cards/48x3.md) | 0.1679 | 0.02289 | 11.13 | 0.0809 | 0.119 |
| [48x2](track-lean-cards/48x2.md) | 0.1675 | 0.02287 | 11.14 | 0.0809 | 0.124 |
| [24x3](track-lean-cards/24x3.md) | 0.1649 | 0.02283 | 12.67 | 0.0815 | 0.136 |
| [24x2](track-lean-cards/24x2.md) | 0.1649 | 0.02283 | 12.65 | 0.0814 | 0.136 |
| [16x3](track-lean-cards/16x3.md) | 0.1734 | 0.02313 | 17.24 | 0.0829 | 0.125 |
| [16x2](track-lean-cards/16x2.md) | 0.1734 | 0.02313 | 17.24 | 0.0829 | 0.128 |

MEASURED pile approaches: none reaches the pile centre at 0.6 throttle. The full-throttle centre result changes with resolution. Advance is the final hull centre past the pile’s near edge, after braking and parking; it is not the maximum advance or the centre-crossing time. The two pile cases react through their rigid solver colliders with ratio approximately 1, as the complete cards show.

| Setting | 0.6 throttle final advance, m | Full-throttle final advance, m | Full-throttle centre time, s |
|---|---|---|---|
| Boat baseline | 0.16 | 1.84 | not reached |
| [97x3](track-lean-cards/97x3.md) | 0.111 | 1.959 | 6.234 |
| [97x2](track-lean-cards/97x2.md) | 0.641 | 1.655 | not reached |
| [48x3](track-lean-cards/48x3.md) | 0.144 | 1.964 | 6.328 |
| [48x2](track-lean-cards/48x2.md) | 0.216 | 1.952 | 6.203 |
| [24x3](track-lean-cards/24x3.md) | 0.127 | 1.954 | 6.297 |
| [24x2](track-lean-cards/24x2.md) | 0.137 | 1.960 | 6.344 |
| [16x3](track-lean-cards/16x3.md) | 0.112 | 1.963 | 6.219 |
| [16x2](track-lean-cards/16x2.md) | 0.034 | 1.966 | 6.203 |

### Knee and decisions

My reading of the observations: **two Gauss nodes are the practical lateral knee on the stable-ground lines.** Across the four station counts, adding the third node changes achieved/geared turning ratios by at most 0.35%, the five grounds’ 40 s speeds by less than 0.00037 m/s, curb roll by about 0.009 degrees, half-ledge roll by at most 0.042 degrees, and washboard pitch RMS by less than 0.00043 degrees. These are DERIVED differences between the measured cards, not tolerances or new tests. Both rules retain full-width piece detection.

**48 × 2 is my conservative combined knee for owner review.** The road responses are already close to the denser field; reducing stations further materially moves the front-wrap contact observation: both 97 and 48 first touch one-lane walls with belt and bump stop together, while 24 and 16 first touch with the bump stop alone. At 16 × 2 the belt still touches during the push, but its last-two-second mean vertical contribution is zero in both lanes. The 0.4 m step takes 5.81 s instead of 5.25 s at 97 × 3; turn radii tighten by up to about 3.1%, and half-ledge roll moves from −3.809 to −3.605 degrees. The query-target setting therefore has a geometric cost to examine in play.

**There is no established plateau across the whole card.** The pile approach remains resolution-sensitive, including a centre-crossing change between 97 × 3 and 97 × 2. The wall-force readings also continue to move. The lateral knee above is a judgement about stable-ground observables, not a claim that every row stopped moving. No resolution below 16 stations or above three lateral nodes was tested.

The shipped value stays **97 × 3**, as dispatched. The report nominates 48 × 2, which uses 92–94 flat-launch terrain queries/tank/tick, for review; it does not claim that this reaches 10–40. Only 16 × 2 reaches that range in the measured launch fixture. No stiffness, friction, rate, support law, transmission or collider was tuned to improve a card result.

Validation: all eight complete cards passed; final formatting, CI clippy and 49 targeted whole-tree/sandbox checks passed after the scorecard merge. The earlier 663-test track/headless/construction/scatter/state-hash/guard run passed, followed by the adjusted calibration/doc checks and both ignored observation beds. The card reader is byte-for-byte the merged score lane version; the arm seam is its only edited card file. No timing runs, captures, issue comments or pushes.

Scope note: the existing `load-29` backlog row names `resolve_rig_nodes` in `src/tank/spawn.rs`; that admission code is unchanged. Only spawn-time grip dimensions in that file were changed, under the brief’s resolution/query-only scope. The multi-file admission-type work remains recorded in the backlog.

## Session 3: outer-face hard limit

The stop encloses the shoe's **outer face** at each wheel's full compression. Roadwheel centres rise by their authored bump travel; hull-fixed end wheels stay put. The measured pin-to-outer offset expands every pin-line circle, so plate thickness is no longer an invented compression stroke. The enclosure tests derive both roadwheel-limited and end-wheel-limited bounds from construction, allowing the route's chord sagitta.

Decision: a segment with zero construction travel has **no boat spring band**. Its outer-face collider owns contact from first touch, including the return run. Such segments are rejected before terrain or convex queries. Spring-law fixtures now declare positive travel and elevate their pin lines by the same distance, preserving their stated envelope penetration; the obsolete spring response on the return run was removed. A deep-overlap zero-travel fixture refuses any oracle call.

Decision: retain `SidePlate` for lateral contacts across the suspension band. Its contact hook removes radial contacts, retaining only lateral faces and carrying their belt speed too.

Validation of this step: formatting and CI clippy passed. The broad geometry run passed 787 checks, including headless, construction, pen-camera, field-cast and whole-tree guards; its 33 failures were zero-travel spring fixtures. After correcting those fixtures, all 68 targeted force, rubble and documentation checks passed. No tuning, captures, timing runs or pushes.


## Session 3: moving outer faces and pricing

Subject: the game's tank driving physics, game state only. **The shipped resolution remains 97 × 3.**

The outer-face stop now carries the belt's surface velocity on every surviving contact. Radial faces read the contact plane's tangent; lateral faces read the closest edge of their closed construction outline. `WorldContacts` composes the construction sweep first, then belt motion. Field contacts read the field's grip; solid pairs retain the track substance's ordinary material combination. The cached shapes and motion outlines are built before spawn; every collider is attached with the body.

`ContactTransfers::solver_groups` supplies actual delivered normal and friction momentum. The reader indexes it by owner in solver order; each tank projects only its own tangential impulses onto its belt directions in the parallel driving pass. The transmission receives their reaction on the next tick, with no second hull force. An absent mechanical transfer is `None`, including zero-transfer speculative rows. A capacity-satisfied brake reports its existing hold verdict; a loaded rigid face held by it has exactly zero surface speed. This exposes the existing brake constraint without changing its capacity, force equation, fuel, gearing or inertia.

The sandbox shares the same field, publishes its surface speeds on its spawn-built body and feeds the same solver reading to its joint transmission. Its reset clears those speeds. No alternate motor or friction arm remains.

**Decisions and costs.** End-wheel and return faces have no spring band. Their contact placement and friction are Avian manifolds rather than advected spring elements; the sprung band's support, shear and transmission laws remain. `SidePlate` stays only for lateral contact through the suspension band: removing its radial contacts gives up its former rigid rest-height support on pieces. Its lateral shape remains a filled convex slice, with surface motion taken from the nearest outline edge. The solver reaction is coupled one tick later; it is not a new belt degree of freedom inside Avian's solve.

### Terrain work after the zero-travel cut

MEASURED with the same first-128-tick, two-tank flat-launch fixture as session 2; counts are divided by two. These are grid-facet lookups, not Parry casts or timings. The original arm performed 582 terrain overlays/tank/tick. Every setting still performs zero terrain overlays. Full raw readings: [work-counts.txt](track-lean-session3-cards/work-counts.txt).

| Stations/side | 3 Simpson: session 2 → session 3 | 2 Gauss: session 2 → session 3 | Final spring contacts, 3 / 2 |
|---:|---:|---:|---:|
| 97 | 285–288 → 234–237 | 190–192 → 156–158 | 201 / 132 |
| 48 | 138–141 → 117–120 | 92–94 → 78–80 | 102 / 68 |
| 24 | 69–72 → 60–66 | 46–48 → 40–44 | 54 / 36 |
| 16 | 45–48 → 42–48 | 30–32 → 28–32 | 36 / 26 |

Only 16 × 2 remains wholly inside the requested 10–40-query range on this fixture. No new knee or shipped setting is chosen from these counts. The longitudinal and lateral bounds and losses stated in session 2 still apply to the sprung field; the rigid outer face now resolves end-wheel contact independently of station count.

### The two cards beside session 2

MEASURED complete cards: [97 × 3](track-lean-session3-cards/97x3.md), [48 × 2](track-lean-session3-cards/48x2.md), with adjacent JSON retaining unrounded values. **Both cards pass every claimed law check.** The scorecard reader is unchanged; `arm.rs` is its only edited file. Below, each arrow is session 2 → session 3. Step sizes, wall dimensions, throttles and horizons are the card's generic fixture inputs, not requirements on a tank.

Line 4, steps: the 0.6 m step is newly crossed at both settings. First touch there is the outer-face stop plus hull proxy; at 0.4 m it is the stop. No climb height is a gate.

| Step | 97 × 3: crossing time | 48 × 2: crossing time |
|---|---:|---:|
| step 0.2 m | 4.594 s → 4.594 s | 4.594 s → 4.594 s |
| step 0.4 m | 5.250 s → 5.688 s | 5.250 s → 5.578 s |
| step 0.6 m | not crossed → 6.250 s | not crossed → 7.344 s |
| step 0.8 m | not crossed → not crossed | not crossed → not crossed |
| step 1 m | not crossed → not crossed | not crossed → not crossed |
| step 1.2 m | not crossed → not crossed | not crossed → not crossed |

Line 4b, walls: force means cover the last two seconds; vertical is upward-positive. Total vertical includes solver normal, solver friction and the external spring channel. The latter is zero on every new wall push. All cases stay behind the face of the 4 m wall: the one-lane cases raise the hull but do not climb over it. A full-width wall still meets the hull proxy first and reproduces its session-2 readings.

| Setting / wall | First touch, before → after | Normal kN | Friction vertical kN | Total vertical kN | Maximum rise mm |
|---|---|---:|---:|---:|---:|
| 97x3, full width | hull proxy Hull_Collider → hull proxy Hull_Collider | 297.17 → 297.17 | -75.36 → -75.36 | -75.24 → -75.24 | 3.73 → 3.73 |
| 97x3, left lane | belt, bump stop → bump stop | 265.03 → 197.20 | -80.30 → +54.51 | -79.49 → +54.51 | 3.73 → 40.56 |
| 97x3, right lane | belt, bump stop → bump stop | 279.65 → 175.57 | -84.58 → +54.68 | -83.05 → +54.68 | 3.73 → 40.29 |
| 48x2, full width | hull proxy Hull_Collider → hull proxy Hull_Collider | 286.90 → 286.90 | -78.59 → -78.59 | -78.59 → -78.59 | 3.59 → 3.59 |
| 48x2, left lane | belt, bump stop → bump stop | 266.15 → 209.33 | -80.32 → +48.33 | -79.74 → +48.33 | 3.59 → 38.26 |
| 48x2, right lane | belt, bump stop → bump stop | 275.22 → 212.11 | -83.06 → +54.70 | -82.13 → +54.70 | 3.59 → 38.61 |

MEASURED tick traces, separate from the card summary: the first nonzero wall force is tick 116 at both settings. The upward friction peak, 1.50–1.57 MN, occupies exactly that one tick (one tick above 500 kN in each trace). There is brief onset chatter, including negative friction, followed by an upward-positive late push. The late interval has no negative friction ticks and no sustained alternating-sign cycle. DERIVED chatter duration below is last negative tick minus first contact tick, divided by the fixture's fixed tick rate. The peak overlap is the contact graph's largest stop/wall penetration, not a visual measurement.

| Trace | Peak stop overlap mm | Onset sign reversals | Last negative after contact, s | Late friction range kN | Late friction standard deviation kN |
|---|---:|---:|---:|---:|---:|
| [97x3, left](track-lean-session3-cards/97x3-Left-wall.csv) | 1.138 | 4 | 0.344 | 52.99–57.39 | 1.212 |
| [97x3, right](track-lean-session3-cards/97x3-Right-wall.csv) | 1.138 | 4 | 0.344 | 53.97–56.39 | 0.567 |
| [48x2, left](track-lean-session3-cards/48x2-Left-wall.csv) | 8.309 | 13 | 0.562 | 47.20–50.38 | 0.843 |
| [48x2, right](track-lean-session3-cards/48x2-Right-wall.csv) | 8.309 | 7 | 0.375 | 52.47–58.42 | 1.667 |

Line 8, rubble: both settings now reach the pile centre even at 0.6 throttle, but the retained clamped recipient law supplies less reciprocal load as the spring channel carries more of the pile. Removing radial side-plate support exposes that limitation on the parked layer too: its belt share rises from about 92% to 100%, and its reaction ratio falls from about 0.081 to about 0.012. No loaded piece is asleep. This lane does not repair that law or claim reaction symmetry.

| Setting / case | Centre time s | Final advance m | Reaction ratio | Maximum sink mm | Vertical jitter RMS mm |
|---|---:|---:|---:|---:|---:|
| 97x3, pile, 0.6 throttle | — → 8.234 | 0.111 → 1.952 | 1.000 → 0.582 | 0.000 → 121.594 | 1.679 → 3.566 |
| 97x3, pile, full throttle | 6.234 → 5.797 | 1.959 → 1.962 | 1.000 → 0.562 | 0.000 → 125.193 | 0.654 → 4.151 |
| 97x3, parked layer | — → — | — → — | 0.081 → 0.012 | 0.439 → 29.755 | 0.121 → 2.724 |
| 48x2, pile, 0.6 throttle | — → 8.047 | 0.216 → 1.956 | 1.000 → 0.555 | 0.000 → 156.454 | 2.000 → 7.600 |
| 48x2, pile, full throttle | 6.203 → 5.844 | 1.952 → 1.959 | 1.000 → 0.417 | 0.000 → 151.005 | 1.023 → 0.212 |
| 48x2, parked layer | — → — | — → — | 0.081 → 0.012 | 0.462 → 29.191 | 0.124 → 2.713 |

MEASURED work beside those cards (different workloads from the launch matrix):

| Setting | Open-ground spring contacts | Layer spring contacts, before → after | Layer slab checks, before → after | Layer solver points, before → after |
|---|---:|---:|---:|---:|
| 97x3 | 203.25 | 693.35 → 693.76 | 1358 → 546 | 111.46 → 0.00 |
| 48x2 | 69.15 | 312.13 → 303.98 | 672 → 280 | 111.35 → 0.00 |

### Cold-start capture is not static hold

MEASURED during the gate diagnosis: the existing cold concrete fixture at 27.867° and x = 100 m briefly asks about 144.46 kN per side of the brakes. The data supplies 144 kN static and 96 kN moving capacity per side. After that static breach, the moving capacity cannot arrest the slope's DERIVED demand of about 131 kN per side. The new contact path travels about 561.78 m over the original 60 s horizon; the committed outer-limit step captured after about 0.118 m. Disabling the moving hook did not remove the new path's cold capture failure. The small perturbation that tips this cold trajectory into breakaway was not isolated. This sensitivity remains an observation for play review, not a parameter adjustment or a claim of unchanged behavior.

The old full-app gate treated a static limit as a guaranteed cold capture limit. It now asserts capture below the **moving** grip/brake limits and continuing motion above the lower **static** limit. Its numerical precision is unchanged. The transmission's independent stationary-hold/breakaway test remains, now also checking the reported hold verdict inside and outside capacity. The unchanged card's origin-based static cases pass at both requested resolutions. No stiffness, grip, brake force, rate, gear or inertia was tuned to obtain these verdicts.

### Pricing lever and validation

`OVERMATCH_TRACK_RESOLUTION=<stations>x<points>` is read once before dedicated-server app construction. Unset is the owner's Resolution choice **97x3**. A typed refusal rejects malformed text, non-Unicode input, fewer than three stations or a lateral count other than two or three before play. Calibration and spawn-time grip dimensions read the same parsed selection. Standalone clients keep the shipped default. `scripts/track-resolution-scorecard 97x3 48x2` uses the same selector; the retired test-only environment variable is removed.

The lever's users are the orchestrator's 30-tank pricing script and the tracked scorecard script. It remains for those pricing runs and is to be deleted at landing, as dispatched. This lane measured work only: no server timing runs, captures, windows, issue comments, pushes or merges from the solver arm.

Validation: formatting and CI clippy passed. The final targeted run passed **656 checks**, including track, sandbox, headless, construction and whole-tree guards; the eight-setting work matrix passed. Both complete cards and both pairs of wall traces passed (four ignored observation tests). Geometry's pen-camera and field-cast gates passed in the outer-limit step. Temporary diagnosis instrumentation and the losing hook-disabled comparison are removed. The shipped resolution remains **97 × 3**; the owner still decides the knee and whether this arm's rubble/capture costs are acceptable.
