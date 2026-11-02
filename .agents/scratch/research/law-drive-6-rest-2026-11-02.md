# Driving review closure, lane 6 (#146)

Subject: the game's driving and damage model, game state only. Crew are HP pools at stations.
MEASURED start: 2026-11-02 22:30:58 +07; the lane stops at a green step within 60 minutes.

The two lane-5 reviews are claims checked against source before each change. No windows,
captures, pricing, issue comments, push, merge or release. Builds and tests use serial-cargo's
machine lock with unattended, headless, muted execution. Engine APIs are checked against the
locked Bevy 0.19.1 and avian3d 0.7.0 registry source before use.

## Dispositions

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|
| Claude P3 | zero-rate clutch test misses parked/rest arms | verified: removed-gate mutant opens parked clutch | driving: align reviewed drivetrain invariants (#146) |
| Claude P3 | ceiling gear/rate witness vacuous | verified: removed-rate-gate mutant brakes; live command below top gear now exercises C2 | driving: align reviewed drivetrain invariants (#146) |
| Claude P3 | beyond-capacity reversal doc overclaims service proof | verified source: test asserts retained direction and refreshed pause only; doc narrowed | driving: align reviewed drivetrain invariants (#146) |
| Claude P3 | redundant reversing and grip predicates in stop forces | verified source: working reversal has full service or parked clutch Out; grip admission belongs to lever | driving: align reviewed drivetrain invariants (#146) |
| Claude P3 | pricing and solve key catch floor on different signs | verified red: -45,950.83 N price versus +15,289.641 N live-floor price | driving: align reviewed drivetrain invariants (#146) |
| Claude P3 | debug HUD first row has nine literal spaces | verified source: first row alone indented; removed | driving: align reviewed drivetrain invariants (#146) |
| Claude P3 | stale scheduler, brake-latch and grip-source sentences | verified at every listed site against current source; corrected | driving: align reviewed drivetrain invariants (#146) |
| Claude + Codex P3 | lane-5 record says revision 77 | verified against revision history: the eight-field revision is 78; corrected record | driving: align reviewed drivetrain invariants (#146) |
| Claude + Codex P3 | free-torque doc misstates steering and governor gates; gear return comment omits rate | verified source; states engaged throttle, authority and window-or-steer admission | driving: align reviewed drivetrain invariants (#146) |
| Claude P3 | crank guard repeats over-rev expression | verified source; return reuses over | driving: align reviewed drivetrain invariants (#146) |
| Claude P3 | GripLaw measurement history points at draft | verified against ADR-0026's falsification and bounds; pointer corrected | driving: align reviewed drivetrain invariants (#146) |
| Codex P3 | drag_fraction promises bounded descent in every gear | verified by renewed F6 measurement; doc states cap-limited condition | driving: align reviewed drivetrain invariants (#146) |
| lane ratchet | glossary calls crank nonnegative | verified against signed state and rollback witness; definition corrected | driving: align reviewed drivetrain invariants (#146) |
| S3 owner call | F6 vehicle-speed acceptance remains unmet | not a new implementation defect: reproduced construction limit, documented with measurement | driving: assert construction-derived coast equilibrium (#146) |

| Claude P2 | fixed rest fields mix staffed and unstaffed fuel and pretend to be data | verified source: only zero and one clutch variant admitted | driving: remove fixed control-rest authoring (#146) |
| Claude P2 | S3 lower-gear speed bound is print-only | verified: half-drag mutant fails the new equilibrium assertion | driving: assert construction-derived coast equilibrium (#146) |
| Codex P2 | backward crank reports stopped to audio and HUD | verified red: public RPM 0 and no negative HUD reading | driving: report backward crank rotation (#146) |
| Claude P2 | backward crank buys rollback cuts after restaffing | verified red: restaffing bought a cut | driving: preserve rollback gear under backward drag (#146) |
| Claude P1 | zero-rate task takes staffed park | verified red: staffed coast never latched | driving: retain staffed park at zero task rate (#146) |

## Evidence and decisions

The accepted design separates staffing from task throughput: a staffed Driver retains the park
lever even when the game's engine or transmission HP pool is emptied. The first witness starts
unparked at 3 m/s uphill on a ten-degree supported grade, then checks both retained directions at
rest between the construction's dynamic and static brake capacities. These are inline Fixture
inputs; all pass conditions are class invariants.

MEASURED red: `staffed_zero_rate_stops_and_parks_without_engine_work` failed on the base at
"staffing, not task throughput, owns park". Removing only the task-rate gate repairs staffing
ownership; empty-station park persistence and zero-rate clutch rest retain their own gates.
MEASURED step-1 gates: fmt and CI clippy passed; targeted track, HUD, row and doc-citation tests
passed (276 tests). No wire or authoring-schema change.

MEASURED step-2 red: the retained-notch rollback witness failed at "restaffing cannot buy a
rollback cut". C2 now treats backward rotation as motoring, never combustion; candidate ceiling
admission uses rotation magnitude. A lower landing beyond the backward ceiling is refused even
when inspected directly. This changes no wire field.
MEASURED step-2 gates: fmt, CI clippy and 277 targeted tests passed. Base protocol revision is
80 (lane 5's eight-field change was revision 78); this lane has not changed wire types.

MEASURED step-3 red: the backward rotation witness returned `(0.0, false)` instead of
`(3100.0, true)`. Public `EngineRpm` keeps its u16 wire representation and publishes rotation
magnitude for the existing audio consumer; the owner HUD keeps the signed crank reading.
Nonfinite simulated crank state now fails explicitly before public conversion. The engine-loop
consumer already pauses only at zero and otherwise plays at the public rotation rate.
MEASURED step-3 gates: fmt, CI clippy, 286 targeted track/HUD/audio/citation tests and all five
shipped drive-load tests passed. The shipped load run also checks the combined park/rollback fixes.

MEASURED step-4 red: halving only production `engine_drag` made the shipped S3 witness fail:
DERIVED equilibrium 1.7630874 m/s versus MEASURED final range [17.964113, 18.292593] m/s.
The mutant is removed. The expectation inverts the authored mid-band anchor and Coulomb share
against nominal weight, grade, rolling resistance and gear ratio; it does not call the drag
implementation for its expected answer. The numerical comparison permits one percent for the
coupled contact discretization and finite settling window. Only constructions whose root lies
outside the one-rpm regularization and below both the crank guard and clutch cap claim this bound.
Taller gears whose guarded drag cannot balance this grade retain only the crank/regime evidence.
MEASURED restored-law S3: F4 final ranges [1.775128, 1.775237] m/s downhill and
[-1.774553, -1.774450] m/s after rollback, within the DERIVED 1.7630874 m/s root's one-percent
contact tolerance. F6 reproduces 13.315221 m/s after 20 simulated seconds downhill and
-8.660275 m/s after rollback, still accelerating, with 3100 rpm maximum crank magnitude.
MEASURED step-4 gates: fmt, CI clippy and 283 targeted tests (including all drive-load cases) pass.
The design now gives the F6 measurement explicitly and leaves another loss or changed over-rev
contract to the owner; this lane adds neither empty-station brakes nor a belt-speed clip.

Step 5 is a source-verified removal of fields with no admitted freedom: `fuel_share` and `steer`
accepted only zero, and the clutch enum had only `SpringEngaged`. They and their validator are
removed with the dead unstaffed combustion arithmetic and the zero-axis forwarding function.
The fixed rest is the absence of actuation: no fuel request, centered axes, spring-engaged clutch
after an already-paid cut. Staffed open-clutch rest now names its own zero fuel request, without
reusing an empty-station value. A future independent idle actuator must be actual construction,
not a nonzero unstaffed pedal or a per-vehicle override. No tank schema or wire type changes.
MEASURED step-5 gates: fmt, CI clippy and 283 targeted tests including all shipped drive-load cases
passed. No rest-field reference remains in runtime code or the driving asset.

## Final review step: evidence and decisions

MEASURED red witnesses: removing only the clutch's zero-rate predicate yielded Out at parked
release instead of In; removing only the ceiling brake's rate predicate applied brakes with
staffing true and rate zero. Both mutants are removed. The retained positive-command ceiling
fixture has grip and room to upshift wherever the ladder permits it, so C2 gear holding is live.
The existing below-top-gear C2 witness remains independent of the ceiling-brake evidence.

The unmodified candidate-price path failed the cut-through-zero witness: -45,950.83 N versus
+15,289.641 N from the live crank's admitted floor. Pricing now takes pre-tick crank speed; one
`idle_floor` predicate decides sign, staffing and catch capacity for both pricing and settlement.
A free crank cannot pass through zero under drag during its open cut. A negative vehicle landing
therefore cannot revoke a positive live crank's catch floor. Slip launch still uses its commanded
launch rpm. Instruments pass the same live crank; no second floor policy remains.

The redundant `!reversing` stop predicate cannot change the force: an unparked reversal has full
service (so it never takes the one-way force branch and always uses dynamic capacity), while a
parked reversal with Driver work has clutch Out. Zero-rate or absent controls already exclude the
one-way branch. Geometry support is checked once at lever admission. The existing
held-reversal, beyond-capacity, launch and stop witnesses cover the surviving regimes.

The beyond-capacity reversal test keeps its narrower claim; it does not pretend to measure full
service force. HUD indentation is removed directly. Current docs name park/reversal state and
Driver gear decisions, the free-torque authority and window-or-steer gate, engaged fueling, the
conditional descent equilibrium and ADR-0026's measurement history. The glossary defines signed
crank rotation. These corrections introduce no new design alternative or ADR.

The historical lane-5 record now says revision 78 in both places. Its ancestor commit message is
retained as history; this lane does not rewrite an ancestor on main. Current wire revision stays
80: no replicated type, field, serialization or codec changed. No tank asset or spec field changed,
so the Blender fixture does not require an update. No open lens-backlog row names an edited file;
the remaining driving row names death-flow input outside this lane.

The localized reviewer causes supplied the diagnosis; direct red witnesses tested those causes
rather than running a broad hypothesis search. All values in the new arithmetic setup are inline
Fixture inputs; pass conditions refer to generic laws and owner parameters. No test of a replaced
rest path or alternate control policy is kept.

## Final gates and handoff

MEASURED final gates: `cargo fmt --all --check` and CI clippy with
`--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings` passed. The focused
clutch, ceiling and floor witnesses plus `doc_citations ui_ascii net_boundary bake::` guards
passed **47 tests**. The whole library ran once with the same feature set: **2,196 passed,
26 ignored, zero failed**. It includes all track, HUD, drive-row, shipped drive-load, S4,
headless composition and wire-pin checks. Ignored measurements/tools were not run.
Python compilation verified the comment-only analyzer edit. `git diff --check` passed.

MEASURED green checkpoint: 2026-11-02T23:20:07+07:00; DERIVED lane elapsed
49 min 9 s from the measured start, within the 60-minute boundary.
The lane commits six green steps on its leased branch, with the requested co-author trailer;
no push, merge, release, capture, pricing or issue comment. No process started by this lane remains.

Players regain a staffed park lever with an emptied powertrain HP pool, retain the gear while
braking backward rotation after restaffing, and hear/see backward crank rotation. Low-gear
motoring equilibrium is asserted, not printed as an observation alone. All implementation review
findings are closed. The remaining #146 acceptance call is S3's F6 vehicle-speed limit: another
physical loss or changed over-rev contract requires an owner decision; this lane invents neither
brakes at an empty station nor a belt-speed clip.
