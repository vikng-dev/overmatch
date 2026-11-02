# Driver control seam, lane 7 (#146)

Subject: the game's driving and damage model, game state only. Crew are HP pools at stations.
MEASURED start: 2026-11-02 17:23:15 UTC. Lane limit: 60 minutes (owner instruction).

## Dispositions

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Audit 2 | pricing carries live crank and C2 repeats its sign | verified in source: candidate pricing takes an already-admitted positive floor; backward combustion is zero | driving: remove drivetrain control residue (#146) |
| Audit 3 | optional grip changes the brake law | verified red: missing construction did not panic; three old witnesses disabled park by removing grip | driving: remove drivetrain control residue (#146) |
| Audit 8 | redundant drag fade | verified algebra: the discrete stop cap alone bounds drag continuously at zero for any positive crank inertia | driving: remove drivetrain control residue (#146) |
| Audit residue | shift check implied by clutch; binary service stored as float | verified source: shift opens the clutch before brakes; service has only off/on values | driving: remove drivetrain control residue (#146) |
| Claude P3 | forward crank during rollback buys a cut | verified red: retained launch gear 4 changed to 2 | driving: remove drivetrain control residue (#146) |

## Step 1

The grip-free steering witness now states its released-park, paid-cut baseline. Brake passivity
compares the stop-force law against free belt integration; coast checks zero brake contribution.
Neither requires a different physical law for a calibration rig. Missing grip construction is
an invariant failure even when a tick does not price a candidate gear.

C2 compares fuel plus catch to drag magnitude, so backward motoring needs no crank-sign gate.
Candidate pricing no longer carries live crank state: a cut cannot reverse a free crank and
pricing uses its admitted catch floor. The live `crank_guards` sign check remains. The existing
service decision also supplies zero gear command while stopping rollback, rather than treating
forward crank inertia as permission to buy a cut. This replaces the old brake/opposed-command
condition with the already-computed brake regime.

The drag fade is deleted; `stop_drag` bounds the discrete free-crank loss. Service is boolean,
and a clutch that is not Out already excludes a paid shift in the stop-force stage.

No wire shape changes; protocol revision remains unchanged. No tank-spec field changes.
No lens-backlog row names an edited file.

The first targeted run found two tests of the replaced paths: raw drag vanishing under the
extra fade, and lower-gear recovery while service cannot arrest rollback. The fade assertion
and that recovery test are deleted. A free-crank witness now checks actual integration from
both sides of zero; the retained-gear service witness replaces the rollback-cut expectation.
MEASURED first targeted run: 283 passed, those two obsolete expectations failed; all shipped
load cases passed. This is evidence of a behavior change, not a behavior-preserving cleanup.

MEASURED step-1 gates: formatting and CI clippy passed; 285 targeted track/HUD/load/citation
checks passed. Both initial red witnesses now pass, as does free-crank stopping from either side
of zero. The source diff was read under all five lenses; no applicable backlog row is open.
