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
