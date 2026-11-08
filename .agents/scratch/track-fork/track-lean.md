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

DERIVED bounds: at least three stations enclose a loop. Its perimeter divided by station count bounds the unresolved arc interval. The fixture's MEASURED belt length is 12.651316 m, giving intervals 0.1304 / 0.2636 / 0.5271 / 0.7907 m at 97 / 48 / 24 / 16 stations. Fewer stations replace more of the curved outline with chords and coarsen the advected shear field; a sharp feature inside an interval can change contact onset and load placement.

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
