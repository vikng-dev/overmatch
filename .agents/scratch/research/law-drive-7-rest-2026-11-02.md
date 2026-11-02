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

## Step 2

MEASURED red: both `zero_rate_steering_retains_park_on_a_supported_slope` and
`zero_rate_staffed_descent_keeps_the_crank_ceiling_with_brakes` failed before the seam change.
They enter the real joint `step`, using two independently authored drivetrains. The first holds
steering then releases it with an emptied engine HP pool on ten degrees; the second starts at
the data-derived ceiling with no key, comparing staffed and empty-station outcomes.

`DriveIntent` now crosses the shaping adapter intact. One `driver` produces `Controls` and the
mechanical tick consumes those controls. Unmanned intent gives closed fuel, centered steering,
retained gear and park, and spring engagement after a paid cut. Explicit Brake intent retains
the existing phase/repair hold contract. The Driver admits task throughput once; engine torque,
catch, gear, clutch and brake laws no longer each infer station presence.

The single existing work predicate also resolves steering: no task work means no worked steering
lever. Park sees that resolved lever, so an undeliverable neutral turn cannot release it. Brakes
instead require staffing, independently of engine throughput. A paid cut still expires on its
mechanical countdown. No new physical premise or compensating predicate is introduced.
MEASURED source counts: staffing reads in the transmission fall from 9 to 1; zero/positive task
work guards fall from 5 to 1. The held-lever and clutch-opening distinctions are retained.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Audit 1 / Claude P1 | scattered Driver ownership; zero-rate steer releases park | verified red: undeliverable steering released the lever | driving: let the Driver own resolved controls (#146) |
| Claude P2 | ceiling braking conflates staffing with throughput | verified red: a staffed zero-rate descent accelerated above the curve ceiling | driving: let the Driver own resolved controls (#146) |
| Claude P3 | `belt_tick` forwards default fields; identical engine early returns | verified source: the adapter now uses `DriveSignal::default`; torque receives Driver-admitted throughput | driving: let the Driver own resolved controls (#146) |
| Audit 6 | two thresholds per detent and reachability search | verified source: keyboard input is monotone between requests; the exit band only delays it | pending step 3 |
| Audit 4 | reflected inertia priced on slip-held landing | verified source: candidate In pricing applies reflected mass even below its admitted floor | pending step 4 experiment |
| Audit 5 | Slip admission narrower than accepted design | verified source: at-rest check excludes moving entries below idle in launch gears | pending step 4 experiment |
| Audit 7 | over-rev guard has two jobs | verified source and audit measurement: crank guard also bounds available drag on descent | pending step 5 |
| Codex P2 | S3 equilibrium admits the rolling-resistance fade band | verified algebra: full rolling resistance is valid only above its owner's fade speed | pending step 6 |
| Claude P3 | HUD negative zero | verified source: signed formatting rounds small backward rpm to negative zero | pending step 6 |
| Claude P3 | backward HUD witness bypasses `readout` | verified source: it constructs the display value directly; readout has no backward assertion | pending step 6 |
| Claude P3 | live idle-floor sign check has no witness | verified source: existing backward engine/gear cases do not distinguish this floor admission | pending step 6 |
| Claude P3 | rollback witness overclaim, no-op and unlabelled precision | verified source at all named sites | pending step 6 |
| Claude P3 | `drag_fraction` says actuation threshold | verified source: the base is Coulomb drag, with no actuation threshold | pending step 6 |
| Claude P3 | Signed shaft excludes crank sign | verified source: contradicts the adjacent signed crank definition | pending step 6 |
| Claude P3 | determinism fixture carries an unsupported measurement | verified record: no new measurement supports the current-law 4 m/s claim | pending step 6 |
| Owner condition | second inline drivetrain and class invariants | existing lab and seed cover separate construction; the requested additional fixture is not yet added | pending step 7 |

MEASURED step-2 gates: formatting, CI clippy and 288 targeted track/HUD/load/citation tests
passed. Both red regressions and the independent phase-hold witness pass. No engine API or
wire shape was added. No new physical predicate was needed; the audit stop rule is not triggered.
