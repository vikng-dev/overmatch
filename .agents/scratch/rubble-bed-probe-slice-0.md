# Rubble-bed probe, slice 0: the game's destruction model

Game state only: pieces, joints, loads and fill. Branch `lane/rubble-bed-probe`, baseline `c0d70cbd`. This lane retains only this report. The experiment follows `rubble-bed-design-2026-11-18.md`; the two preceding crush notes are references, not test oracles.

**Gate: FAIL for every pair. Decision: build changed.** The column probe retains matter and unloads the belly, but does not restore traversal; its capped local repose also leaves over-steep grids at occupied boundaries. The stiff pair keeps sinking during the park. These apparatus limits prevent a verdict against every bed formulation.

## Method and limits of the apparatus

All observed table entries below are **MEASURED** ranges over independent process repetitions. Percentages, angles converted from radians, elapsed seconds from ticks, adjacent-tick accelerations, raster areas/volumes and line-count estimates are **DERIVED**. Repeated endpoints describe repeatability of one placement, not placement coverage. Configured angles and packing fractions remain **Unknown physical facts**; observing a configured value does not turn it into a measured material property.

The disposable arm returns `merge_fragments`' input without coarsening, intercepts the light fragments of `route`, and deposits each chunk once, in its current construction frame. It reads the chunk's admitted `layout.blocks[c].points` and occupied `Structure::chunks[c].volume`; the deposit raster weights projected world AABB/cell overlaps. This conserves volume but approximates the distribution inside a rotated convex footprint. The packing fraction expands occupied material volume into bulk fill. An integer depth unit is **DERIVED 0.0000625 m³**: **MEASURED configuration 0.25 m** square cells and **MEASURED configuration 1 mm** depths. A fractional-unit accumulator carries deposit quantization; reported physical bed volume excludes that accumulator.

The probe uses one sparse column field, with a deliberately enlarged finite domain (**DERIVED approximately 256 m per side**, **DERIVED 1023 × 1023 possible cells**), rather than the design's storage tile. It refuses out-of-domain writes. No boundary is a material sink. This avoids making this experiment a tile-border experiment; production border exchange and tile storage cost are untested. Each positive cell is an ordinary static cuboid on `Layer::Terrain`, with shipped brick contact friction, without `FieldSurface`, wire, mesh or a track-field term. Belts therefore use the existing solid-footprint path (`src/track/rubble.rs:99`); hulls use Avian. No dynamic body pose, velocity or force is written by the bed law.

Each fixed tick reads solver transfers and retained sleeping transfers from `ContactTransfers`, plus sampled belt loads from `TrackContacts`. Contacts are rasterized by column and grouped by loading body (each belt side separately); connected occupied contact cells form a patch. Its narrow bounding width and occupied-cell area enter the specified Vesic bearing expression. Overload lowers those cells together by **DERIVED 2 mm/tick**, the integer floor of `g dt²`, and spreads the removed units to the nearest free ring. This point-to-cell patch approximation is not an exact projected contact polygon, and its resolution sensitivity is untested.

Repose transfers integer depth over cardinal and diagonal neighbours until no permitted transfer remains, refusing failure to converge within **MEASURED configuration 256 sweeps**. A destination cannot rise above an overhead solid's conservative projected AABB underside. Repose can consequently retain a steeper boundary against an occupying body; free-surface slopes and occupied boundaries are reported separately. Bearing and repose run after the shipped tick's split commit; changed cells replace colliders on the same entities before the next tick. Counts distinguish cell changes from repeated cell visits.

The three parameter pairs are **MEASURED configurations**, chosen before results: loose **34° / 0.55**, nominal **38° / 0.60**, stiff **44° / 0.64**. None was tuned after a result. The shipped brick row is **MEASURED by asset read 1,845 kg/m³** (`assets/materials/materials.ron:630`), not the design's unchecked density. The resulting bearing factors are **DERIVED 204.4, 423.7 and 1301.0 kPa/m** respectively. The formula is implemented as specified, not independently validated as a geotechnical model.

The controls mount the shipped simulation plugin with the intervention inactive. The stuck replay uses the original `settle` helper, standing ram and its **MEASURED configuration 1.4 m** offset; it preserves the complete **DERIVED 25 s** driven prefix and measures the following **DERIVED 15 s**. The intervention removes existing retained weld grouping after `LawSet`, before `SplitSet`, skipping owners already pending a split. That is a declared migration experiment, distinct from starting without welds. A second standing case starts without welds at drive onset. Free cases collapse the same house without welds from the freeing event, then make repeated full-demand passes through the same evolving lane; only the tank is repositioned between passes. The untouched-lane comparison is a separate newly collapsed world with one pass, not an adjacent lane claimed to be untouched inside this narrow pile. The free placement uses the original control footprint and source placement arithmetic, not a post-settle position settled twice.

Parking drives toward the pile middle, applies the shipped service-hold command for **DERIVED 10 s**, and measures a further **DERIVED 10 s** with those brakes held. It does not teleport the hull onto fill or clamp its motion. The mound is an independent column cone beside the house, clear of its construction, with **MEASURED configuration 1.25 m** continuous apex before rasterization; its measured raster height is lower. This checks approach to the toe separately from traversing a collapsed-house layout.

A whole-carrier retirement uses the preceding report's literal state criterion: an already-existing carrier retires with every owned chunk routed to fill and no kept child. It is counted even though its matter remains in the bed. Newly freed small fragments that never became server bodies do not count. This is not a visual-pop measurement: headless runs establish state changes and geometry, not how decoration would look. The report does not relabel such a retirement as zero merely because volume is conserved.

The strict gate requires drive-on through the old footprint within the rescue window, zero such whole-carrier retirements, and conservation of bed-plus-body material. “Hangs” means final-second mean belly share above the requested threshold. “Gone” requires a completed footprint traversal within the requested rest-height tolerance, or a bed below the requested maximum-height threshold. “Blocked” uses the matching shipped control's measured crossing time, not the full drive watchdog; crossing later still counts YES. An incomplete traversal cannot establish the rest-height branch of “gone.”

## Results

**Strict rescue gate: FAIL for every pair. Decision: build changed.** None of the bed replays drives through; all retire whole carriers. Volume survives and the belly unloads, but the hull stops against column faces. Separately, the stiff pair keeps sinking during the park. This is not evidence that every granular-bed formulation fails: the column geometry snags, and this local capped-flow implementation also fails the design's unqualified repose invariant near occupying bodies. A passing production bearing law has not been established.

The completed matrix has **MEASURED 72 executions**: **MEASURED three independent processes** for each of **MEASURED four arms × six scenarios**. Every scenario/arm's entire telemetry stream is byte-identical across its repetitions. No result below comes from a development run. The current standing control reproduces a persistent beach; it does not reproduce the earlier report's exact trajectory or body census.

### 1. Stuck start, conservation and retirements

The shared prefix ends at **MEASURED 0.03359–0.03359 m/s**, with **DERIVED 63.173–63.173%** of weight on hull proxies, **MEASURED 60–60 carriers**, and **MEASURED 13.349474–13.349474 m³** loose material. The following table covers only the rescue window. Accounting starts after the scripted collapse for free/park runs and at intervention for the stuck replay; buried anchors and already script-dusted material are excluded. Conservation includes the unchanged standing material so a change of ownership cannot masquerade as loss. Physical bed volume is packed back to material volume; the fractional accumulator is excluded.

| Arm | Forward progress (m) | Final speed (m/s) | Final-second belly (%) | Material retained (%) | Whole-carrier retirements | Gate |
|---|---|---|---|---|---|---|
| shipped control | 0.464–0.464 | 0.04132–0.04132 | 62.736–62.736 | 100.000000–100.000000 | 0–0 | remains beached |
| 34° / 0.55 | 0.319–0.319 | 0.00021–0.00021 | 0.465–0.465 | 99.999921–99.999921 | 60–60 | FAIL |
| 38° / 0.60 | 0.012–0.012 | 0.00007–0.00007 | 0.000–0.000 | 99.999749–99.999749 | 60–60 | FAIL |
| 44° / 0.64 | 0.025–0.025 | 0.00057–0.00057 | 0.000–0.000 | 99.999767–99.999767 | 60–60 | FAIL |

All bed arms end with **MEASURED 0–0 loose bodies**. The retirement count includes conversion at the declared intervention; **MEASURED 5–5 whole-carrier retirements** per pair are prepared by the migration hook, while the shipped owner/landing paths retire the rest after coarsening is disabled. Thus volume conservation is not a zero-pop result under the preceding report's state definition. No material disappears from the accounting; static fill replaces it.

The physical-volume shortfall is quantization, bounded by one carried deposit unit. Including that fractional unit gives **DERIVED 100.000000–100.000000%** conservation within floating-point summation precision. It is not counted as collider volume.

Starting without welds from drive onset also fails to traverse, so failure is not confined to converting an old pile:

| Pair | Progress over the standing drive (m) | Final speed (m/s) | Whole-carrier retirements | Crossed |
|---|---|---|---|---|
| 34° / 0.55 | 2.374–2.374 | 0.00892–0.00892 | 22–22 | NO |
| 38° / 0.60 | 2.138–2.138 | 0.00008–0.00008 | 17–17 | NO |
| 44° / 0.64 | 2.131–2.131 | -0.00006–-0.00006 | 19–19 | NO |

### 2. Repeated passes, untouched lane and absurd ends

Each bed arm crosses on every free pass. The untouched-world one-pass comparison is identical in every reported measurement to its matching first pass. These are **MEASURED ranges**; first-pass rows therefore cover both the first pass and that untouched-lane experiment. All bed material remains after the pass; neither a blank centre path nor zero remaining bodies is used as a proxy for lost volume.

| Pair | Pass | Bed max (m) | Footprint (m²) | Bulk fill (m³) | Crossing / control (s) | Hangs | Gone | Blocked |
|---|---|---|---|---|---|---|---|---|
| 34° / 0.55 | 1 / untouched | 0.927–0.927 | 49.7500–49.7500 | 16.03850–16.03850 | 6.938–6.938 / 6.047–6.047 | NO | NO | YES |
| 34° / 0.55 | 2 | 0.927–0.927 | 49.8125–49.8125 | 16.03850–16.03850 | 7.672–7.672 / 11.891–11.891 | NO | NO | NO |
| 34° / 0.55 | 3 | 0.927–0.927 | 49.8125–49.8125 | 16.03850–16.03850 | 7.266–7.266 / 11.016–11.016 | NO | NO | NO |
| 38° / 0.60 | 1 / untouched | 0.980–0.980 | 41.1875–41.1875 | 14.70194–14.70194 | 8.109–8.109 / 6.047–6.047 | NO | NO | YES |
| 38° / 0.60 | 2 | 0.969–0.969 | 41.1875–41.1875 | 14.70194–14.70194 | 8.422–8.422 / 11.891–11.891 | NO | NO | NO |
| 38° / 0.60 | 3 | 0.952–0.952 | 41.1875–41.1875 | 14.70194–14.70194 | 8.031–8.031 / 11.016–11.016 | NO | NO | NO |
| 44° / 0.64 | 1 / untouched | 0.901–0.901 | 47.1875–47.1875 | 13.78306–13.78306 | 10.938–10.938 / 6.047–6.047 | NO | NO | YES |
| 44° / 0.64 | 2 | 0.920–0.920 | 47.3750–47.3750 | 13.78306–13.78306 | 10.016–10.016 / 11.891–11.891 | NO | NO | NO |
| 44° / 0.64 | 3 | 0.895–0.895 | 47.5000–47.5000 | 13.78306–13.78306 | 7.844–7.844 / 11.016–11.016 | NO | NO | NO |

The final-second belly share is **MEASURED 0–0%** throughout this table. “Blocked YES” on a first/untouched pass means slower than control, not failure to cross within the drive. Later passes satisfy that time comparison, but the lane is not mechanically unchanged: fill continues to move with no bodies left to deposit.

| Pair | Changed-cell commits, passes 1 / 2 / 3 | New whole-carrier retirements during pass 1 | Material retained over all free passes (%) |
|---|---|---|---|
| 34° / 0.55 | 1111–1111 / 1277–1277 / 1312–1312 | 2–2 | 99.999890–99.999970 |
| 38° / 0.60 | 1288–1288 / 1606–1606 / 1543–1543 | 2–2 | 99.999816–99.999925 |
| 44° / 0.64 | 2804–2804 / 2238–2238 / 1693–1693 | 3–3 | 99.999691–99.999950 |

Changed-cell commits count a cell again if it changes on another tick; they are not a count of unique places. Maximum height need not decrease monotonically: late deposits and heave can raise a free crest while lowering the loaded lane.

### 3. Motion, sink and launches

The following extrema combine the time samples and all repetitions in each window. Acceleration is **DERIVED adjacent-tick vertical velocity difference / dt**, never across a reposition. It is not an Avian substep peak. Pitch and roll are signed angles in degrees, **DERIVED** from body orientation. Free rows combine the repeated passes.

| Window | Arm | Vertical speed (m/s) | Vertical acceleration (m/s²) | Pitch (°) | Roll (°) |
|---|---|---|---|---|---|
| stuck | shipped control | -0.031 to 0.027 | -1.48 to 1.72 | 5.73 to 6.04 | -0.30 to 0.27 |
| stuck | 34° / 0.55 | -0.415 to 0.361 | -9.12 to 22.46 | 1.47 to 5.91 | -0.80 to 0.00 |
| stuck | 38° / 0.60 | -0.421 to 0.144 | -9.30 to 22.78 | 1.48 to 5.91 | -0.76 to 0.00 |
| stuck | 44° / 0.64 | -0.425 to 0.125 | -6.86 to 23.17 | 1.50 to 5.91 | -0.76 to 0.01 |
| free | shipped control | -0.804 to 0.999 | -9.55 to 36.08 | -4.13 to 5.72 | -3.83 to 2.41 |
| free | 34° / 0.55 | -1.849 to 1.109 | -8.63 to 106.88 | -11.91 to 17.17 | -1.73 to 4.25 |
| free | 38° / 0.60 | -1.875 to 0.782 | -8.01 to 105.98 | -12.54 to 19.56 | -2.03 to 2.20 |
| free | 44° / 0.64 | -1.466 to 0.853 | -9.19 to 92.84 | -11.95 to 23.93 | -7.19 to 1.16 |

The bed crossings are substantially rougher than control: the largest upward acceleration samples occur while arresting downward movement, not as unsupported upward launches. A tick-level diagnostic using upward speed above **MEASURED analysis configuration 0.05 m/s** and sampled-plus-solver vertical support below **MEASURED analysis configuration 5%** of weight finds **MEASURED 0–0 qualifying ticks** in every bed drive, park and mound run. Control's first free pass has **MEASURED 4–4 such ticks**, **DERIVED 0.0625–0.0625 s**. No arm has a sampled tick with positive vertical velocity and nonpositive total vertical support. These diagnostics do not exclude a shorter substep launch or establish visual comfort.

Sink after conversion is measured from the last prefix height to the lowest height over the full rescue window. The minimum timestamp is not a time to settled rest. The largest continuous descent uses consecutive negative-vertical-velocity ticks, with no magnitude cutoff:

| Pair | Maximum sink (m) | Minimum reached after (s) | Largest continuous descent (m) | Its duration (s) |
|---|---|---|---|---|
| 34° / 0.55 | 0.1398–0.1398 | 7.3438–7.3438 | 0.0761–0.0761 | 0.7031–0.7031 |
| 38° / 0.60 | 0.1485–0.1485 | 7.5938–7.5938 | 0.0774–0.0774 | 0.7031–0.7031 |
| 44° / 0.64 | 0.1429–0.1429 | 7.8281–7.8281 | 0.0782–0.0782 | 0.6875–0.6875 |

These **MEASURED excursions** include the carrier-to-bed intervention; their elapsed times are **DERIVED**. A completed settling duration was not established. For free driving, descending the mound and bearing sink are not independently identifiable from hull height alone; the park below supplies a loaded, braked sink measurement without a drive-off.

### 4. Ten-second park and creep

All runs arrive through driving and braking. Endpoint displacement, accumulated path length and downward motion are different readings. Sleeping is the shipped Avian island verdict; retained velocity on a sleeping body is not motion.

| Arm | Endpoint travel (cm) | Path travel (cm) | Downward creep (mm) | Sleeping ticks | Ends asleep | Cell changes/s |
|---|---|---|---|---|---|---|
| shipped control | 0.000–0.000 | 0.000–0.000 | 0.000–0.000 | 640–640 / 640 | YES | 0–0 |
| 34° / 0.55 | 1.514–1.514 | 2.134–2.134 | 13.133–13.133 | 381–381 / 640 | YES | 0–318 |
| 38° / 0.60 | 1.424–1.424 | 5.598–5.598 | 11.624–11.624 | 269–269 / 640 | YES | 0–289 |
| 44° / 0.64 | 7.408–7.408 | 7.757–7.757 | 73.073–73.073 | 20–20 / 640 | NO | 554–679 |

The stiff pair fails the **DERIVED comparison bar 3 cm** even by endpoint travel, and is still changing cells at the end. Loose and nominal pass that endpoint comparison and eventually sleep; nominal's accumulated path exceeds it. None justifies a blanket “parked means no give.” Per-second changed-cell-commit counts, each vector identical over repetitions:

- 34° / 0.55: **MEASURED** `318, 297, 303, 271, 45, 0, 0, 0, 0, 0`.

- 38° / 0.60: **MEASURED** `260, 289, 241, 245, 238, 219, 0, 0, 0, 0`.

- 44° / 0.64: **MEASURED** `645, 631, 627, 679, 627, 588, 623, 594, 567, 554`.

The stiff trace keeps triggering irreversible bearing transfers under the unchanged hold command; it is a downward ratchet over this window. The samples do not isolate solver-force noise from continued settling and the changing raster patch. No unbounded-time claim follows, and no force filter or deadband was tuned to make it sleep.

### 5. Cost in counts and contact continuity

Awake counts below are loose bodies, excluding sleeping and frozen bodies. The driven tank adds one awake body; its sleep is reported above. Body counts after collapse are read before the drive, not inferred from the old dust arm. Work counts cover the measured drive windows; collapse-time peaks are not captured by this stream.

| Window | Arm | Bodies before | Bodies after | Awake/tick | Mean awake | Columns below hull, peak | Changed cells/tick, peak | Repose cell visits/tick, peak |
|---|---|---|---|---|---|---|---|---|
| stuck | shipped control | 60–60 | 60–60 | 34–56 | 52.88854–52.88854 | 0 | 0 | 0 |
| stuck | 34° / 0.55 | 60–60 | 0–0 | 0–0 | 0.00000–0.00000 | 519 | 1683 | 733936 |
| stuck | 38° / 0.60 | 60–60 | 0–0 | 0–0 | 0.00000–0.00000 | 515 | 1645 | 622582 |
| stuck | 44° / 0.64 | 60–60 | 0–0 | 0–0 | 0.00000–0.00000 | 512 | 1619 | 478486 |
| free | shipped control | 15–15 | 39–39 | 0–39 | 5.88417–5.88417 | 0 | 0 | 0 |
| free | 34° / 0.55 | 2–2 | 0–0 | 0–1 | 0.00125–0.00125 | 373 | 63 | 93464 |
| free | 38° / 0.60 | 2–2 | 0–0 | 0–2 | 0.00354–0.00354 | 337 | 52 | 62278 |
| free | 44° / 0.64 | 2–2 | 0–0 | 0–2 | 0.00083–0.00083 | 356 | 120 | 76426 |

Column counts are conservative counts under the union of projected hull-proxy AABBs, not a claim that every column contacts the hull. Repose visits count repeated neighbour reads, not unique cells, and exclude overhead-cap raster construction and deposit/heave search. Thus a tiny awake population does not establish a cheap bed. The large conversion visit peak is particularly unlike one sweep over one storage tile. Hosted narrow-phase time, collider-replacement time and an optimized inactive-tile path remain unmeasured.

At every bed commit the raw stream records solver point count, total hull solver normal force and contact identities, then the following physics tick. The table aggregates those adjacent comparisons; it includes real load changes and motion, not only cache effects. Normal forces are sums of absolute normal components of the transferred force, not the solver's accumulated `normal_impulse` field.

| Window | Pair | Commit comparisons | Points before / next tick | Point-count change | Normal-force change (MN) | Retained bed pair IDs (%) |
|---|---|---|---|---|---|---|
| stuck | 34° / 0.55 | 766 | 2 to 120 / 0 to 102 | -120 to 9 | -5.299 to 5.025 | 99.348–99.348 |
| stuck | 38° / 0.60 | 311 | 2 to 120 / 0 to 50 | -120 to 9 | -1.505 to 1.541 | 99.409–99.409 |
| stuck | 44° / 0.64 | 341 | 2 to 120 / 0 to 37 | -120 to 8 | -1.531 to 1.545 | 99.465–99.465 |
| free | 34° / 0.55 | 209 | 3 to 45 / 3 to 45 | -32 to 35 | -7.180 to 7.168 | 97.466–97.466 |
| free | 38° / 0.60 | 254 | 3 to 44 / 3 to 44 | -10 to 15 | -4.407 to 2.465 | 98.195–98.195 |
| free | 44° / 0.64 | 446 | 1 to 73 / 1 to 55 | -72 to 30 | -8.382 to 8.382 | 97.770–97.770 |
| park | 34° / 0.55 | 257 | 102 to 125 / 102 to 125 | -6 to 6 | -0.572 to 0.565 | 99.990–99.990 |
| park | 38° / 0.60 | 359 | 135 to 153 / 135 to 153 | -3 to 4 | -0.588 to 0.551 | 99.924–99.924 |
| park | 44° / 0.64 | 639 | 182 to 305 / 182 to 305 | -4 to 6 | -0.382 to 0.372 | 99.995–99.995 |

Most pair identities survive, so this collider-update path does not erase every hull contact at every commit. Normal force nevertheless jumps by substantial fractions or multiples of fixture weight. Surviving pair IDs do not prove surviving manifold warm-start impulses; this experiment cannot causally separate shape rebuild, patch quantization and actual changing support. It does not repeat the settlement probe's rigid-body-type switch.

### 6. Repose mound, toe and the apparatus boundary

No column mound is crossed, including the static control mound. Each pair has some early bearing flow before the final jam. An earlier development snapshot described the final zero-yield state; the complete traces establish that initial toe flow did occur.

| Arm | Stall hull Z (m) | Final speed (m/s) | Horizontal hull reaction peak (MN) | Toe max lowering (mm) | Toe max heave (mm) | Loaded-cell yield steps | Crossed |
|---|---|---|---|---|---|---|---|
| shipped control | -17.121–-17.121 | -0.000006–-0.000006 | 10.658–10.658 | 0–0 | 0–0 | 0–0 | NO |
| 34° / 0.55 | -17.403–-17.403 | -0.000002–-0.000002 | 9.946–9.946 | 120–120 | 28–28 | 351–351 | NO |
| 38° / 0.60 | -17.119–-17.119 | -0.000001–-0.000001 | 10.658–10.658 | 46–46 | 35–35 | 36–36 | NO |
| 44° / 0.64 | -17.378–-17.378 | -0.000034–-0.000034 | 9.785–9.785 | 122–122 | 18–18 | 106–106 | NO |

The toe is the approach half of the saved raster. Lowering and heave are same-cell initial-to-final differences, not disappearance. The hull's nearly horizontal contact normals accompany the large horizontal reactions; the final tank origin remains before the mound's centre. This is direct evidence of column-face snagging. It is not proof that a smooth repose slope is unclimbable or that the specified bearing formula cannot flatten its toe. No smooth-heightfield A/B was run.

There is a second limitation beyond those vertical box faces. At the end of every free pass, with the tank clear, the height grid meets the repose bound. Around occupying bodies, the local capped sweep can stop with a steeper height grid:

| Pair | Maximum grid slope after a clear free pass (°) | After stuck replay (°) | During park, final grid (°) |
|---|---|---|---|
| 34° / 0.55 | 33.947–33.947 | 74.260–74.260 | 64.323–64.323 |
| 38° / 0.60 | 37.977–37.977 | 73.683–73.683 | 42.239–42.239 |
| 44° / 0.64 | 43.965–43.965 | 76.849–76.849 | 53.673–53.673 |

These are **DERIVED neighbouring-height gradients**, not the cuboids' vertical side faces. A blocked destination prevents a local downhill transfer; the probe does not find a globally repose-compatible outward route around that cap. Its conservative AABB underside also approximates the actual underside of a rotated body. Therefore **the unqualified “never steeper than repose” claim is not established and is violated by this implementation**. The park result is a failure of this implementation, not a clean isolation of the intended global granular law. A global redistribution or an obstacle-aware boundary treatment must be specified and checked before claiming all three laws hold together; silently treating the blocked case as “settled” is not a production outcome.

## Verdict and next decision

**FAIL on the requested strict gate for every pair. Hangs NO; gone NO; blocked YES on first and untouched free passes, NO on passes two and three, for every pair. Build changed.**

Keep the volume-bearing representation as an investigation, but move a continuous heightfield surface and a globally repose-compatible capped-flow treatment into the refuting slice before building wire, drawing and the full surface-reader migration. Use actual geometric patches to establish width/area rather than treating this contact-point raster as settled physics. Repeat the stuck and braked near-capacity cases there. Do not add a tuned deadband to hide the stiff pair's ongoing downward movement. Do not ship the already-resting weld conversion: production deposits must follow freeing/landing events, and the from-outset standing run must pass too.

The evidence supports static fill retaining matter with very few awake loose bodies. It rejects a claim that this cheap column/local-flow arm already provides drive-on, no retirements, an invariant repose surface and a firm parked bed across the bracket. It does not establish “the bed is not the answer,” and it supplies no evidence favouring smaller bodies or ground yield instead.

Not established: the corrected globally constrained bearing model; smooth-mound climbability and toe reduction; exact convex-footprint/underside integration; indefinite parked stability; substep motion or visual pops; a genuinely untouched adjacent strip of the same narrow physical pile; collapse-time cost peaks; hosted cost; tile-border conservation; mixed materials; production late-join/replica compatibility; or a causal measurement of warm-start reset. These are limits of the experiment, not production switches retained in code.

## Production surface inventory and size

These are current-source seams, not assertions copied from the design. Production needs a shared top-surface interface and a timestamped bed state; the immutable base grid can remain immutable.

| Reader | Current source | Required production change |
|---|---|---|
| Track oracle | `src/track/oracle.rs:515` | Union the bed's top with blocks and the canonical grid for depth, normals and buried reads. |
| Force-column plane sampling | `src/track/world_contact.rs:36`, `src/track/world_contact/terrain.rs:14` | Sample the same top facet; adding only an oracle term leaves this cached-plane path reading base ground. |
| Ground type and belt grip | `src/track/oracle.rs:603`, `src/track/world_contact.rs:365`, `src/track/stop_contacts.rs:229`, `src/ground_field.rs:269` | Select the rubble row when fill is the supporting surface; keep print softness separate from bearing yield. |
| Physical colliders | `src/world.rs:305`, `src/terrain_grid.rs:738` | Publish matching bed heightfield geometry, with empty cells handled deliberately; mark it `FieldSurface` once belts read it through the surface interface. |
| The game's rounds' march | `src/ballistics.rs:888`, `src/ballistics.rs:2123` | The production march reads `grid.cast_ray` directly; it must also cast against the bed. `contact_classes.rs` is a test module, not the production seam. Apply this to authority and presented march at their respective stream cursor. |
| Spawn height | `src/terrain_grid.rs:197`, `src/terrain_grid.rs:217`, `src/net/server.rs:1337` | Max-over-footprint placement must include fill for every spawn/respawn path. |
| Decoration landing | `src/debris/decor.rs:94`, `src/debris/decor.rs:220` | Settle presentation against the bed surface rather than base height only. Production deposits occur at landing, unlike this in-place shortcut. |
| Client mesh | `src/terrain_grid.rs:768`, `src/terrain_lod.rs:212`, `src/world.rs:441` | Build/update bed tiles from the replicated samples, without rebuilding the immutable terrain ladder. |
| Drawn wheels/belts and cache invalidation | `src/track/view.rs:151`, `src/track/view.rs:793`, `src/track/terrain.rs:39` | Include the bed in the view oracle and invalidate its relevant cache on bed revisions; global map revision is not an established cheap per-tick bed mechanism. |
| Replication and late join | `src/net/protocol.rs:901`, `src/net/construction.rs:39`, `src/net/construction.rs:294` | Add admitted cell/tile snapshots and diffs, world identity and authoritative tick; present them on the shared stream delay. `Shed` is presentation and late joiners receive none. Existing append-only ledger diffs demonstrate a mechanism, not a ready-made mutable-tile protocol. |
| Track prints | `src/track_prints.rs:511` | Ground-height and refit reads must take the bed surface/revision; zero rubble softness must not leave stamps projected onto the buried base grid. |
| Ground dust presentation | `src/vfx/muzzle.rs:685` | Place ground dust at the presented bed height and choose its ground response from the supporting material. |
| Runtime operator scenarios | `src/net/debris_stress.rs:430`, `src/tank/battle.rs:601`, `src/tank/battle.rs:652` | Runtime footprint placement, ground aim and grade diagnostics must query the same current surface when a bed exists. |
| Camera/aim terrain casts | `src/world.rs:616` | Existing spatial casts read the new Terrain-layer colliders automatically; preserve their layer and presented-time geometry. |
| Sleeping supports | `src/debris.rs:245` | Preserve wake-on-surface-change semantics for hulls and pieces; the existing static-collider change path supplies them in the probe. |

Map-authoring scatter and clearance (`src/scatter.rs:109`, `src/scatter/clearance.rs:101`) still read the immutable base terrain when the world is built. The base terrain shadow bake (`src/terrain_shadow.rs:121`) likewise stays a base-grid product; the bed mesh needs current geometry/shadow integration rather than a per-tick rebake of that map product.

**DERIVED rough changed/added line estimates**, including meaningful interface/lifecycle tests and without counting this disposable harness as production:

| Design slice | Rough scope |
|---|---|
| Bed surface, deposits at landing, repose, every reader, replication and mesh | 1,600–2,400 Rust/data/test lines. Surface/collider ownership, streaming cursor and late-join admission dominate; this is not a small crumble patch. |
| Bearing | 400–700 lines for load patches, conservative displacement, owner/border ordering and conservation/limit tests. Exact contact-area integration may enlarge it. |
| Delete the weld | 100–200 changed/added lines and 70–120 deleted lines, including state/ledger lifecycle tests; migrate no already-resting welded pile as a production shortcut. |
| Hosted remeasurement and freeze decision | 100–250 instrument/script lines, plus reports. Freeze deletion is conditional; neither its deletion nor a hosted speedup follows from these counts. |
| View-only seated chunks | 200–400 lines, independent of collision and fill conservation. |

This is **DERIVED approximately 2,400–4,000 added/changed lines before any conditional freeze deletion**, with the ranges sharing interface work rather than promising additive precision. Unknowns include material mixtures, tile borders, delayed presentation, changing-collider narrow phase and late join. The unresolved capped-flow revision can enlarge these estimates. No wire bytes or hosted milliseconds were measured.

## Artifacts and restoration

The ignored raw streams are `target/bed0-{arm}-{case}-r{1,2,3}.jsonl`, with matching `.log` files; cases are `stuck`, `free`, `park`, `mound`, `fresh`, and `fresh-stuck`. `target/bed0-derived.json` and `target/bed0-summary.json` hold the aggregates. The manifest SHA-256 is `b27e6020c22ec7f7f171b507c63f3a931f1ee39308154c0af771655efb8201af`. Each hash below applies to all **MEASURED three repetitions** of that configuration.

| Case | Arm | JSONL SHA-256 |
|---|---|---|
| stuck | control | `15c29b09f6e6552e33365a537af2e4b11f15eb8fc3dc92dccca103cfd469be94` |
| stuck | loose | `18f7d4200aeb0b67279169901786db3f462462b8bc2f662867458c2394ef1fd0` |
| stuck | nominal | `22ccfacf45013af80f93b42926553e3021595cd66fef7fa5a0bec07ec15b24ee` |
| stuck | stiff | `e0ec44f5d7e7d9eb53726e488bed577d9eb0593878a50d062fc8645293333c01` |
| free | control | `03849a3cfc9d06dffb5c42645116860c2b41d7fa504927d48894f85e7e3968c8` |
| free | loose | `040080c40d19e3ca58940090bcf68667ec829e386e3120f01b27d90db8e3c34b` |
| free | nominal | `23ce15b4a63a88b55c39c475814f94aca2acae622f2febdab399229315c7c0ec` |
| free | stiff | `9ce4ff8052ddcee6343ed8e7ab7cbb2ef5fc281d4eb5e746164f895b7544cc4d` |
| park | control | `3dd23da772195a23b3db12ed7b5c7d40839f998a4ea2a8b68033e63f5670dcad` |
| park | loose | `e03265b1b8881f1337d391cb253ce09e34d1613c1614f430735463b6780c36ce` |
| park | nominal | `d3094ea61b51894c717331b47885cf1f6e9124492761d127a353fb7eb9a5db2a` |
| park | stiff | `728569fa56bb5dc776f8c1361bdfab52b416c7692c29b41d49061e65202ed3ac` |
| mound | control | `64660f9de0129599d96951dcf53b6428023cb4de0ea62ac6ea1db7ddfe6bebb0` |
| mound | loose | `b59885a85fa5a7fc86a129cf629b29f919ce4b3cbbb17947b7bb6053ea3a33e5` |
| mound | nominal | `335a681c5b69f7ac76c15b5688d779ca9985bd2ee276584f6b7046515c435280` |
| mound | stiff | `19b52bc61e05ed0562f7c9faeee87a7acafea29e03ccf5d896b6c9923286b685` |
| fresh | control | `cea831240fa1846896434bd55c990273834b6254bc3c64c607c2374256fb89f1` |
| fresh | loose | `4860b01aacac69ceba97321525de806fccd9df622c0bd24e161531554969a603` |
| fresh | nominal | `42ddb42eddb8ce53fac8a83f56aa0754aac0aac1bfc159b8b74160e0d34f4688` |
| fresh | stiff | `d06b05e34086f639b908ec68d0f720e2699a1c04cada1121c4832c178e5a2588` |
| fresh-stuck | control | `6ed2e2bd9a8bdfc28e4cb99975123babc97379702525835ba2b8594608b566f5` |
| fresh-stuck | loose | `f1d830baa15204c18693a65db93fc425b62324986fef4c0ebf1cf310541be4f0` |
| fresh-stuck | nominal | `ff9aee444fba7da86c41d70da2d91154d3ebe8211699c04b8e96fb6a6a1991b7` |
| fresh-stuck | stiff | `b2661ed0cdb97cc370b5c85b8c840a841fe162e9f231e5dee60cb2f36832f45e` |

The final disposable instrument SHA-256 was `59bf932ab8da3677f42e7c14f89b47f04ced4e8cb5d500c80a68943bf8c93d30`; its Rust source and all source hooks are deleted. The matrix ran the compiled `edit`-profile library test `rubble_bed_zero_probe --ignored --nocapture --test-threads=1`, with `BED_ARM`, `BED_CASE`, `BED_REP`, `OVERMATCH_UNATTENDED=1` and `OVERMATCH_MUTE=1`. Independent processes ran two at a time; counts are not timing claims. All compilation used `scripts/serial-cargo` and `--features dev_ui,bitprobe`.

Development runs are excluded: an early redundant fixture-lock acquisition was terminated by its exact test PID after compilation had completed; placement reconstruction was replaced with the original helper/offset, and a too-early manual split was moved between the shipped law and split stages after the duplicate-deposit invariant caught it. No in-flight compilation was killed and no physical parameter was tuned after a result.

After restoring the production tree, `git diff --exit-code -- src` is clean. `cargo fmt --all --check` passed. CI clippy (`scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`) passed. Targeted `debris::crumble` tests passed (**MEASURED 11 tests**), and the shipped-plugin support-change/wake regression passed (**MEASURED one test**). Only this report is committed; no production code, test arm, data edit or switch remains. No push, GitHub comment, debugger, display/audio run, forbidden-document read or damage-topic web search was used.
