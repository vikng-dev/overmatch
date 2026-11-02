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
