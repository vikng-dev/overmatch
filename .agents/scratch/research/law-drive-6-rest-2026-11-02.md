# Driving review closure, lane 6 (#146)

Subject: the game's driving and damage model, game state only. Crew are HP pools at stations.
MEASURED start: 2026-11-02 22:30:58 +07; the lane stops at a green step within 60 minutes.

The two lane-5 reviews are claims checked against source before each change. No windows,
captures, pricing, issue comments, push, merge or release. Builds and tests use serial-cargo's
machine lock with unattended, headless, muted execution. Engine APIs are checked against the
locked Bevy 0.19.1 and avian3d 0.7.0 registry source before use.

## Dispositions

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
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
