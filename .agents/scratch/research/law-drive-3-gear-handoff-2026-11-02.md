# law-drive-3-gear bounded handoff (#146)

Subject: the game's driving model, game state only. Where the Driver station is involved,
crew are HP pools at stations.

The MEASURED feature-work interval was 08:24:41–09:24:05 UTC, 2026-11-02: 59 min 24 s.
The impending 60-minute limit stopped the ungreen gear-law step. Required gate corrections,
verification and recording the green handoff finished afterward. No push, merge, release, issue comment, capture, frame/tick pricing
or visible window occurred. The bay remains leased; it is not released by this lane.

## Committed scope

- **S1:** `driving: read piece support and separate landing force terms (#146)`.
- **S2:** `driving: record same-instant calibration and gear-law handoff (#146)`.

Only the support/input and instrument work is landed on this lane. The gear scheduler, wire
state, required asset shape, ignored soft-launch test and current margin asset remain unchanged.
S1 includes the ladder-shape diagnostic recognized by the existing specification guard;
S2 records the evidence and dispositions. A full recovery gate caught a mistake in the first
support attempt: actual normals had also changed the physical overlap law, and a coarse cell's
pressure centroid need not hit its convex surface. The final S1 confines the actual-face query
to Driver facts, restores the physical overlap orientation and names an empty geometric sample
as absence. The full-app piece-driving regression now passes. S1 was amended with these gate
corrections so that it is independently landable.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude D1 | P2 piece normals manufacture grade from hull pitch | verified red: the pitched-pad/level-piece contact pass measured 22353.906 N of false grade; the actual struck convex face makes it pass; its query is confined to the Driver observation | S1 |
| Claude D1 | P2 shot bound uses M and never proves the incline | already fixed by `driving: bound fuel and control launch slip (#146)`; verified source and paired execution, 0.093750 N maximum L change with an independent incline assertion | prior clutch subject |
| Claude D1 | P2 course asserts only parked L | verified source; every rolling sample now asserts the uninterrupted plane's load during cuts and acceleration | S1; acceleration mutant was not executed |
| Claude D1 | P2 turn check accepts float noise, omits finite patch span, lacks detent coverage | partial: finite-span/normal-pressure test failed red and now passes; both ladders' detent mappings are tested. The shipped `extra > 0` assertion remains weak | S1 for unit/detent coverage; shipped quantitative assertion is only in the draft |
| Claude D1 | P2 reverse ladder exceeds shared steering rows | verified red: the authoring seam admitted a reverse ladder longer than its steering table; admission now refuses it before play | S1 |
| Claude D1 | P2 solved delivery appears in transmission input | already fixed by `driving: bound fuel and control launch slip (#146)`; source confirms delivered/priced outputs belong to the clutch/engine payload | prior clutch subject |
| Claude D1 | P2 first-coupled gap is not hysteresis M | verified with the 524-sample decomposition below. A new deadband is derived, but not installed without the green price/gear law | measurement in S1/S2; gear law open |
| Claude D1 | P2 absolute global Newton margin scales incorrectly | already fixed by the clutch subject: current data is a weight share and the second-weight fixture passes | prior clutch subject |
| Claude D1 | P3 price law has no independent cap/launch test | verified source; a cap/launch/governor test passed in the draft, but is not landed | open |
| Claude D1 | P3 duplicated reverse flip and unnamed support-only weight | verified source; shared observation helper and reverse-sign test passed in the draft | open |
| Claude D1 | P3 force probe prices shaped rather than propulsive command | prior clutch subject already uses propulsive command, avoiding the opposing-throttle claim. Separating requested from shaped pedal for the new gear law is draft only | prior fix partial; requested-pedal gear seam open |
| Claude D1 | P3 turn moment uses weight instead of normal load | verified red on a 30-degree finite patch; normal pressure now owns both grip and turn moment | S1 |
| Claude D1 | P3 clipped 30-degree pair/20-degree forecast is presented as landing agreement | verified source; architecture now identifies grip clipping and forecast drift as limitations rather than independent price validation | S2 |
| Claude D1 | P3 stale probe-only report claim | verified source: support facts populate it in every build; dead suppression and false comment removed | S1 |
| Claude D1 | P3 margin has no production policy reader | verified source; the production reader exists only in the unlanded gear draft | open until gear lane completes |
| Claude D1 | P3 forwarding SupportFact constructor | verified source; public named fields replace the positional forwarding constructor | S1 |
| Claude D1 | P3 per-tick cost | explicitly deferred to the orchestrator's batch; not measured | orchestrator |
| clutch handoff / backlog | D0, bake and sim/HUD ratchet rows | unchanged files, listed below; no row was silently closed | open |

Source-verified findings are distinguished from executed red tests. No claim is made that the
acceleration, dropped-radius, dropped-half-span or swapped-detent mutants were all executed.
The pitched-piece, reverse-admission and normal-pressure regressions were executed red before
fixing them. The detent table and finite-span tests now make those unit mutants fail by contract.

## Same-instant M instrument

MEASURED 524 settled coupled samples, retaining D2's one-rpm settlement criterion. Every sample
uses that tick's shaft price, actual pre-tick crank free torque and integrated crank acceleration.
Clutch/grip caps remain explicit. Forecast speed drift, crank-speed difference and crank inertia
are reported separately from the numerical residual. No desired gear enters the calibration.

| force term, N | minimum | median | maximum |
|---|---:|---:|---:|
| forecast minus delivery | -90016.468750 | 4696.843750 | 381493.687500 |
| forecast speed drift | -86185.625000 | -4385.539062 | 365662.625000 |
| shaft price minus actual-crank price | -90663.187500 | 0.000000 | 3.019531 |
| crank acceleration | -64705.390625 | 9978.351563 | 376685.937500 |
| numerical residual | -1.546875 | -0.007812 | 1.062500 |

The old maximum landing mean, MEASURED 261415.078125 N, comprises speed drift 64340.410156 N,
crank-speed difference -5189.072266 N, crank acceleration 202263.812500 N and numerical residual
-0.077271 N. Separately averaged f32 terms have rounding differences.

DERIVED: residual span 2.609375 N exceeds maximum absolute residual 1.546875 N. Adding the
MEASURED observer bound 0.406250 N gives 3.015625 N. Divide by 559170 N nominal weight and
round strictly upward at one part per million: **proposed M = 0.000006 W**, or 3.355020 N on this
rig. The current asset deliberately remains 0.469133 W: the old scheduler/instrument is still
the production path, and the new law was not green at the boundary. This calibration is evidence
for the next step, not a covert production retune.

[All samples and landing means](drive-gear-measurements-2026-11-02.json) retain the distribution.
Architecture section 5a owns the calibration narrative and limitations.

## Unlanded gear experiment and its limits

The draft is outside the repository at `/private/tmp/law-drive-3-gear-draft.patch`. Its new
actuation file is `/private/tmp/law-drive-3-gear-actuation.ron`; the proposed driver margin file
is also saved as `/private/tmp/law-drive-3-gear-driver.ron`. The patch was taken before the support gate corrections. Reconciling its context with the
amended S1 may be necessary; preserve the green observation/physical-contact separation.
These files are local continuation aids, not committed alternatives.

The draft replaces the EMA, confirmation counters, dwell, held target, bands, addressing arms,
landing decay and protective rescue with C1-C3. It prices a changed gear's cut, holds the current
gear without a cut, includes coupled crank inertia, uses requested pedal, retains the support
moment for each candidate radius, moves remaining actuation stand-ins to required labelled data,
and bumps the changed serialized state to protocol revision 70. The hill-hold latch remains the
transitional brake owner. Its net dedicated transmission-test reduction was MEASURED **2010
lines**, 4048 to 2038; none of that scheduler/test deletion is committed. **Committed step-2 net
old-law test lines removed: zero.**

MEASURED draft evidence, not acceptance:

- Fixture gear walks `[4, 5]` and `[4, 5, 6, 7, 8]` reached their top ladders.
- Grade launches gained distance in 30 simulated seconds: 10 degrees, F4, 61.272700 m;
  20 degrees, F4, 45.313130 m; 30 degrees, F3, 30.309292 m. These witness capability, not
  per-vehicle pass conditions.
- **S1 is red against the design expectation.** Sand visited `[4, 1, 4, 5, 6]`; it launches but
  undoes preselection during initial crank spool. F7's landed spare from the F6 equilibrium was
  MEASURED about -5650 N, consistently refusing that upshift with the proposed M. The likely
  remaining distinction is that a launch-slip crank spooling below the first curve point is
  not an engaged crank lugging under load; that C1 change was not implemented after the boundary.
- **S4 is not certified.** The first clean 40-second-entry experiment gave concrete/dirt/gravel
  F8 from both entries and mud F6 from both. Sand was F6 from rest and F7 rolling; the latter
  still had -441.906250 N spare at 40 seconds. Extending the observation first crossed the finite
  slab edge. A later relocated/reused fixture then produced near-stationary F4 rolling entries
  which its weak equilibrium condition accepted. Those apparent results are invalid proof.
  Continue with a complete pose/rig reset or fresh app per entry, enough planar travel, positive
  support, and a force-equilibrium assertion that rejects a high-spare stationary launch.
- The draft paired uphill shot kept F4 and its complete shift timeline unchanged; MEASURED
  maximum L difference 0.078125 N. The committed path's observer-only pair is 0.093750 N;
  no committed new-law no-shift claim is made.
- The last draft whole-lib run had 2110 passed, 1 failed, 26 ignored. The failing S1 and the
  invalid S4 witness prevent committing it. Earlier runs exposed stale hashes, deleted-field
  witnesses, initial-spool assumptions and parked preselection being paired with an old landing;
  their fixes are preserved only in the draft.

## Decisions

1. Stop feature work at the measured 60-minute boundary and retain only the green support and
   measurement step. Preserve the draft outside the bay rather than commit an unproved law.
2. Read a convex piece's actual struck face using the version-verified Avian 0.7 `Collider::cast_ray`;
   a coarse overlap cell with no struck face at its pressure centroid has no geometric Driver
   sample. The coarse physical overlap law retains its existing orientation; it is a different
   contract from the actual support plane used for L. No hull-up fallback enters L.
3. Use normal load for the uniform-pressure turn moment; preserve finite patch span and both
   ladder/detent mappings in direct tests. Refuse missing reverse steering rows at admission.
4. Separate same-tick physical terms before deriving M. Do not reduce M to force a desired gear;
   do not install the proposed margin until the price/gear path is green.
5. Retain the current scheduler, protocol revision and asset shape on the committed lane. The
   draft's physical-price, requested-pedal, C3 and data-classification decisions are provisional
   continuation work, not a second live implementation.
6. Leave runtime cost to the orchestrator and the brake/ceiling policies to their dispatched lanes.
   Do not push, merge, release or comment on issues.

## Remaining rows and gates

Untouched D0/backlog: driving-01, driving-04, driving-05, driving-07, driving-08, driving-09,
driving-10 and view-08; bake load-03, load-07, load-08, load-09; ballistics-20 in sim/HUD tests.
Driving-03 retains lane-4/lane-5 ownership. The committed files name none of these backlog rows;
no backlog deletion is included. The new gear law, production M, S1, S4, requested pedal,
reverse-adapter factoring and quantitative shipped turn assertion remain next-step work.

MEASURED final gates on the committed source: `cargo fmt --all --check`; CI clippy
`--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`; whole library
**2145 passed, 27 ignored, 0 failed**; separate `doc_citations ui_ascii net_boundary bake::`
guards **44 passed, 0 failed**. The whole suite includes headless, pen_cam, field_casts and the
piece-driving regression. All binaries were serialized, unattended, headless and muted.
No process started by this lane remains running. No push, merge or bay release was performed.
