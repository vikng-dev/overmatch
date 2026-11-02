# Gear-law continuation (#146)

Subject: the game's driving model, game state only. Where the Driver station is involved,
crew are HP pools at stations.

## Acceptance record

The accepted design is `drive-control-design-2026-11-02.md` section 2.
The prior handoff's draft reproduced S1 red: MEASURED `[4, 1, 4, 5, 6]`; 64 other targeted
transmission tests passed. A direct positive-spare Slip witness and the pre-decision spool
instrument distinguish the C1 lug check from a real deficit and from the brake latch.
MEASURED at the false F4-to-F1 choice: Slip, 509.41153 rpm, +160323.6 N current spare,
0.43394318 m/s shaft speed, hill hold false. The direct selector witness also fails red.
C1 now admits a sub-curve lug only in In; a genuine deficit remains eligible in every regime.
The price likewise no longer turns an engaged sub-idle lug into a launch slip.

MEASURED final targeted proof: **150 passed, 0 failed**. The fixture walks are `[4, 5]` and
`[4, 5, 6, 7, 8]`; both reach the top in order. The independent loaded-rig launch test runs the
mechanical step on both construction fixtures, beyond checking the selector's price.

| proof | design expectation | MEASURED outcome |
|---|---|---|
| S1 | highest capable start, then climb; refuse an unsustainable next gear | `[4, 5, 6]`; F7 landed spare -5650.0586 N, M 3.9159636 N |
| grade launch | a capable supported tank gains uphill distance | 30 s: 10 degrees F4 +61.2207 m; 20 degrees F4 +45.331383 m; 30 degrees F3 +30.337069 m |
| paired rearward shot uphill | no shift caused by the shot | F4 retained; complete gear/window timelines equal; maximum paired L change 0.078125 N |
| S4 | final gear sustainable, next landed spare below M, both entries at equilibrium | not certified in this primary step; the original 40 s observational fixture remains |

The first apparent 30-degree failure was not a launch from rest: MEASURED -2.5422776 m/s
shaft speed on its first command tick, then an engaged lug held at 500 rpm. It therefore correctly
failed to restart Slip under the landed moving-lug rule. The class proof now resets motion,
strain, command shaping and drivetrain state on the supported pose, asserts rest, and then
launches. No moving-lug or empty-station behavior was restored.

| same engine at nominal mass | MEASURED mechanical residual min..max, N | DERIVED M, N |
|---|---|---|
| five-gear fixture, 20 t | -0.123535156..0.10986328 | 0.6282519 |
| eight-gear fixture, 20 t | -0.79296875..1.2363281 | 3.5529938 |
| five-gear fixture, 60 t | -0.078125..0.09643555 | 1.0206518 |
| eight-gear fixture, 60 t | -0.96875..1.3710938 | 3.9453936 |

Each test checks the residual span as well as the absolute extremes against the crank term,
not merely multiplication of a share. The observer has its own independent-weight error witness.
[Raw measurements](drive-gear-3b-measurements-2026-11-02.json) distinguish failed diagnostic runs
from the final targeted run.

## Deletion and state

The law deletes the band, reserve-confirmation and protective arms of the old decision path,
its target selection and landing decay; demand EMA and seeding marker; both confirmation
counters; reversal dwell and last shift direction; held grade target; crank corroboration and
protective rescue. The postshift rpm margin, duplicate gravity constant, asset rpm bands and
addressing selector leave with their tests. The now-unread per-side rolling-reaction input also
leaves both adapters: mechanical total reactions and the Driver's geometric load remain separate.
State falls from 17 to 10 fields (DERIVED exhaustive projection); protocol revision 71 and the
wire-definition pin deliberately name the changed contract.

MEASURED dedicated transmission tests: **4295 -> 2413 lines, net 1882 removed**. The surviving
clutch, fuel, brake, steering, energy, empty-station and moving-lug tests retain their invariants.
The price has independent launch/cap witnesses, including symmetric grip under engine braking;
the adapter test proves requested pedal is read before the actuator ramp. Shipped turn evidence
compares the full finite-span uniform-pressure moment against both authored detent radii.

## Full-library support fixture

The first whole-library run was MEASURED **2130 passed, 1 failed, 26 ignored**. Its lone failure
was `a_driving_tank_rides_onto_loose_pieces_through_its_belt_support`: 0.35 requested pedal did
not reach the pieces under the new launch selection. This gate claims the game's support-law
composition. Its arrival drive now requests full power, retaining every hull-rise, piece-height
and support-contact assertion. That arrival exposed a second fixture dependency: a released
higher gear coasts beyond the finite pieces during the settle interval (MEASURED final rise
-0.001671 m). The gate now applies service braking until forward motion is below the owned
parking resolution and explicitly asserts that the settled footprint stays inside the pieces.
MEASURED passing rise is **0.076172 m**, against **0.072800 m DERIVED** from coverage; peak piece
centre is **0.050000 m**, peak speed **0.153281 m/s**. The shipped gear-walk replacement records
`[4, 5, 6, 7, 8]`; both corrected fixture tests and the doc-citation guard pass (5 tests total).
The partial-pedal obstacle approach remains unproved and belongs in playtest evidence.

## S4 continuation fixture

MEASURED 40-second observations in the first whole-library run (m/s); these use the retained
observational fixture's partial reset and do **not** certify sustainable equilibrium:

| ground | rest | rolling entry |
|---|---|---|
| concrete | F8, 10.65021 | F8, 10.65021 |
| dirt | F8, 10.56997 | F8, 10.56997 |
| gravel | F8, 10.61011 | F8, 10.61011 |
| mud | F6, 4.84918 | F6, 4.84918 |
| sand | F6, 4.87471 | F7, 6.30139 |

The observations agree with the design's expected dirt convergence and sand split in gear number;
neither a certified convergence nor a certified vehicle-truth split is claimed. S4 is the sole
uncompleted acceptance step; its fixture repair is left under the brief's explicit time-limit
exception while the primary law's required final checks complete.

Use a fresh app for each ground/entry, not the reused app's partial teleport. Start far enough
from the forward slab edge for the full observation horizon; assert positive geometric support
and remain within the uninterrupted plane. Reset the complete pose, velocity, drivetrain,
shaped command and belt strain. A stationary launch cannot certify equilibrium: require positive
travel speed and small current spare, not merely `current >= -M` plus a blocked next gear. Record
both entries for every ground before calling convergence or a vehicle-truth split.

## Reviewer dispositions

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude + Codex clutch review P2 | multiplication-only M test proves no second-rig bound | verified source; independent-weight mechanical and observer instruments replace it | `driving: select gears by landed spare force (#146)` |
| Claude D1 P3 | price lacks independent cap/launch evidence | verified source; regime/cap witness added | `driving: select gears by landed spare force (#146)` |
| Claude D1 P3 | reverse flip duplicated; support-only weight unnamed | verified source; both adapters call the shared ladder-frame observation, with support-only absence named | `driving: select gears by landed spare force (#146)` |
| Claude D1 P3 | gear decisions read shaped pedal | verified source; requested pedal has its own input | `driving: select gears by landed spare force (#146)` |
| Claude D1 P2 | shipped turn assertion accepts float noise | verified source; finite-span uniform-pressure expectation replaces positivity | `driving: select gears by landed spare force (#146)` |
| Claude D1 P3 | M has no production policy reader | verified source; candidate comparisons consume the admitted M | `driving: select gears by landed spare force (#146)` |
| ratchet dev-08 | stiffness ceiling is vehicle-specific | verified source; equilibrium and derivation checks remain, dimensional ceiling removed | `driving: select gears by landed spare force (#146)` |
| ratchet ballistics-20 | driving tests pin HUD text | verified source; task-rate assertions remain, formatting belongs to HUD tests | `driving: select gears by landed spare force (#146)` |
| lane testing lens | the old shipped climb pins F6 and names retired dwell | verified source; its replacement asserts an exact gear-by-gear walk through the declared ladder top | `driving: select gears by landed spare force (#146)` |

The driving-03 row spans an unedited command transition and the later brake/ceiling laws;
its sim-side persistence contract is stated, but the row remains open for its named owners.
Untouched rows: driving-01, driving-04, driving-05, driving-07, driving-08, driving-09,
driving-10, view-08; load-03, load-07, load-08, load-09 in `src/bake.rs`.

## Decisions

- Preserve the landed empty-station, catch-authority and moving-lug laws.
- Admit construction limits for launch gears and shift steps; delete the scheduler bands and
  addressing alternatives with their old tests. Keep the physical contact law unchanged.
- Reject six-ppm-weight M after its unchanged-engine 20-tonne witness fails red: MEASURED
  -0.792969 to 1.236328 N residual versus 1.177200 N M. Use the sum of observer resolution
  `0.000001 W` and crank roundoff resolution `0.0000001 J*omega_over/dt*max(G/r)`.
  The 524-sample span and observer residual set those upward-rounded coefficients; no desired
  gear enters the calibration. Fixed-engine 20/60-tonne witnesses check the bound itself.
- Price holding without a cut and changed gears with their cut; include reflected crank inertia.
- A real Slip uses launch rpm; an engaged sub-idle crank retains its lug price. Cap both signs
  by clutch and grip. C1's lug permission requires In, while a real deficit remains eligible.
- Keep requested pedal separate from actuator travel, and retain the geometric turn moment so
  every candidate can use this tick's detent and its own radius. Both adapters share ladder sign.
- Move surviving actuation stand-ins into required labelled data and admit them before play;
  one immutable cache owns the Driver resolution data. The probe uses the mechanical mass helper.
- Reset the grade-launch proof to asserted rest; give the piece-support fixture full requested
  power while retaining its physical acceptance bounds. The old partial-pedal arrival is unproved.
- Keep the transitional hill-hold latch until the brake lane. Without a protective upshift,
  the crank ceiling is bounded by the existing over-rev clutch slip guard; the Driver holds
  overrun gear. The ceiling lane still owns service braking there.
- Use protocol revision 71: revision 70 already belongs to the base's game-state wire contract.
- Take no captures or frame/tick pricing; no push, merge, release or issue comment.

## Work boundary

MEASURED start: 10:05:17 UTC. The law/data implementation and first gate corrections stopped at
11:00:28 UTC (55 min 11 s). The 60-minute boundary was 11:05:17 UTC. Subsequent source work was
confined to the mandatory piece-support gate correction exposed by validation; no S4 feature
or fixture implementation started. Queued verification and the green commit finished afterward.
S4 is preserved as a concrete continuation plan above, not an unproved production test.

## Final gates

MEASURED final checks: `cargo fmt --all --check`; CI clippy with `--locked --workspace --all-targets
--features dev_ui,bitprobe -- -D warnings`; primary targeted proof **150 passed**; corrected
support/gear-walk/doc-citation proof **5 passed**; whole-tree `doc_citations ui_ascii net_boundary
bake::` guards **44 passed**; final whole library **2131 passed, 26 ignored, 0 failed**.
The failed first whole run justified the rerun after correcting its finite support fixture.
The full suite includes the headless, pen-cam and ordinary field-cast gates; ignored measurements
remain ignored. No physical contact code changed. Every run was serialized, unattended, headless
and muted. No capture or frame/tick pricing was taken. No lane process remains running; no push,
merge, release or issue comment occurred.

MEASURED final support-fixture source correction: 11:08:50 UTC. Final gate completion was verified
at 11:14:44 UTC. The required validation therefore extended beyond the 60-minute work boundary.
