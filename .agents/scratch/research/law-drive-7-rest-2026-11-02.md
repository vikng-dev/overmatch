# Driver control seam, lane 7 (#146)

Subject: the game's driving model.
MEASURED start: 2026-11-02 17:23:15 UTC. Lane limit: 60 minutes (owner instruction).

## Dispositions

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Audit 2 | pricing carries live crank and C2 repeats its sign | verified in source: candidate pricing takes an already-admitted positive floor; backward combustion is zero | driving: remove drivetrain control residue (#146) |
| Audit 3 | optional grip changes the brake law | verified red: missing construction did not panic; three old witnesses disabled park by removing grip | driving: remove drivetrain control residue (#146) |
| Audit 8 | redundant drag fade | verified algebra: the discrete stop cap alone bounds drag continuously at zero for any positive crank inertia | driving: remove drivetrain control residue (#146) |
| Audit residue | shift check implied by clutch; binary service stored as float | verified source: shift opens the clutch before brakes; service has only off/on values | driving: remove drivetrain control residue (#146) |
| Claude P3 | forward crank during rollback buys a cut | verified red: retained launch gear 4 changed to 2 | driving: remove drivetrain control residue (#146) |
| Audit 1 / Claude P1 | scattered Driver ownership; zero-rate steer releases park | verified red: undeliverable steering released the lever | driving: let the Driver own resolved controls (#146) |
| Claude P2 | ceiling braking conflates staffing with throughput | verified red: a staffed zero-rate descent accelerated above the curve ceiling | driving: let the Driver own resolved controls (#146) |
| Claude P3 | `belt_tick` forwards default fields; identical engine early returns | verified source: the adapter now uses `DriveSignal::default`; torque receives Driver-admitted throughput | driving: let the Driver own resolved controls (#146) |
| Audit 6 | two thresholds per detent and reachability search | verified source: keyboard input is monotone between requests; the exit band only delays it | driving: use one threshold per steering detent (#146) |
| Audit 4 | reflected inertia priced on slip-held landing | verified source: candidate In pricing applies reflected mass even below its admitted floor | not fixed; fallback in driving: record the rejected Slip experiment (#146) |
| Audit 5 | Slip admission narrower than accepted design | verified source: at-rest check excludes moving entries below idle in launch gears | not fixed; fallback in driving: record the rejected Slip experiment (#146) |
| Audit 7 | over-rev guard has two jobs | verified source and audit measurement: crank guard also bounds available drag on descent | driving: declare the over-rev engine stand-in (#146) |
| Codex P2 | S3 equilibrium admits the rolling-resistance fade band | verified algebra: full rolling resistance is valid only above its owner's fade speed | driving: admit and exercise different drivetrain construction (#146) |
| Claude P3 | HUD negative zero | verified source: signed formatting rounds small backward rpm to negative zero | driving: close the remaining control review findings (#146) |
| Claude P3 | backward HUD witness bypasses `readout` | verified source: it constructs the display value directly; readout has no backward assertion | driving: close the remaining control review findings (#146) |
| Claude P3 | live idle-floor sign check has no witness | verified source: existing backward engine/gear cases do not distinguish this floor admission | driving: close the remaining control review findings (#146) |
| Claude P3 | rollback witness overclaim, no-op and unlabelled precision | verified source at all named sites | driving: close the remaining control review findings (#146) |
| Claude P3 | `drag_fraction` says actuation threshold | verified source: the base is Coulomb drag, with no actuation threshold | driving: close the remaining control review findings (#146) |
| Claude P3 | Signed shaft excludes crank sign | verified source: contradicts the adjacent signed crank definition | driving: close the remaining control review findings (#146) |
| Claude P3 | determinism fixture carries an unsupported measurement | verified record: no new measurement supports the current-law 4 m/s claim | driving: close the remaining control review findings (#146) |
| Owner condition | second inline drivetrain and class invariants | verified: three constructions share launch, reverse, slope, settling and staffed-descent invariants | driving: admit and exercise different drivetrain construction (#146) |
| Tank #2 | ten-degree unstaffed descent must accelerate | verified test-premise defect; not a game-law defect: stronger drag can arrest it; assertions retained with construction-derived demand | driving: admit and exercise different drivetrain construction (#146) |
| Whole-library gate | spec tests pin removed class caps and belt floor | verified red; obsolete cases deleted; owning Blender spec is admitted directly | driving: admit and exercise different drivetrain construction (#146) |

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



MEASURED step-2 gates: formatting, CI clippy and 288 targeted track/HUD/load/citation tests
passed. Both red regressions and the independent phase-hold witness pass. No engine API or
wire shape was added. No new physical predicate was needed; the audit stop rule is not triggered.

## Step 3

MEASURED red: the old law selected wide from straight at the tight threshold, and admitted
an authored wide threshold exactly on the first notch. Both new witnesses failed before the
change. The detent witness probes each threshold and its immediately preceding representable
float from every prior detent, without pinning keyboard travel time or a numerical taste.

Each detent now has one threshold. The off fields, bounded reachability search, two search-only
panic witnesses, hysteresis arms and redundant persistent-notch witness are removed together.
Direct admission refuses a threshold coincident with either the neutral-based or clamped-endpoint
notch grid. Its witness uses binary Fixture notches; it tests admission, not the deleted search.
The glossary now states the same detent model. No analog input or alternate detent path is kept.

## First-round checkpoint and numeric inventory (historical)

Steps 4–7 are not certified by this checkpoint. In particular, the broader Slip admission and
slip-held landing price have not been tried, so the existing narrow form and upshift refusal
remain. The over-rev engine outcome is still the owner's call; no new over-rev law is built.
The remaining review findings and additional inline drivetrain are still required for the lane's
full acceptance. Existing lab and seed witnesses are not described as the requested new fixture.

MEASURED source inventory of production numeric literals in `transmission.rs` (comments and
strings excluded): 0, 0.0, 0.05, 0.1, 0.5, 1, 1.0, 2, 2.0, 2.5, 3, 3.0, 3.6, 8, 24, 60.0,
64.0, 100.0, 255.0, 20_000.0, 50_000.0, 100_000.0. Their remaining non-Resolution uses are:

| use | numbers | status |
|---|---|---|
| over-rev guard | 100 rpm | Still mislabelled Resolution while also bounding descent drag; pending step 5's StandIn move |
| torque-curve admission | 20,000 rpm; 100,000 N m | Existing numeric domain caps, not vehicle facts; pending step 7 audit |
| static brake admission | upper factor 2.5 | Existing unlabelled class cap; pending step 7 audit |
| crank-inertia admission | 0.1–100 kg m² | Existing unlabelled domain; pending step 7 audit |
| clutch-capacity admission | 100–50,000 N m | Existing unlabelled domain; pending step 7 audit |
| belt-inertia admission | lower 1 kg | Existing numerical divisor floor without an explicit Resolution label; pending step 7 audit |
| shift-time admission | upper 3 s | Existing coarse bound below the representation limit; pending step 7 audit |
| exact algebra and indexing | 0, 1, 2, 3, 1/2 | Signs, shares, paired outputs, interpolation polynomial, tuple fields and detent values; no authored tuning |
| exact representation | 8; 255 | State projection arity and u8 countdown maximum |
| exact unit conversions | 3.6; tau/60 | km/h to m/s and rpm to rad/s |

The other literals are explicitly Resolution: the solver clock 64 Hz, zero tolerances 0.05,
24 bisections and the separately named 100 rpm fuel-governor ramp. The over-rev 100 rpm shares
that numeral but does a different job; it is listed separately above.

Legacy prose also contains source-specific examples (MEASURED lexical inventory, not renewed
physical measurements): 19/20 teeth; 373/478 and 300/224 kN m; 0.337 m/s at 3000 rpm; efficiency
0.9; drag 20–30 percent; clutch 1.3 times peak. These are comments, not runtime constants, and
were not remeasured or used as pass conditions by this lane.

No edited law contains a tank identity or per-tank branch. Full tank-#2 certification remains
open: the constructor caps above restrict otherwise possible construction; the pending S3 test
admission excludes neither the rolling fade band nor its invalid full-resistance expectation;
the additional drivetrain's class scenarios have not been run. These are named limits, not a
claim that every admitted drivetrain has been proved by the two existing fixtures.


## Decisions and checkpoint evidence

- Grip is required construction for every rig reaching the joint step; support-only calibration
  continues to use the contact law without invoking that step.
- Service is boolean. Its existing stop regime supplies zero gear command during rollback.
  This closes the forward-crank cut case without a new backward-motion predicate.
- C2 compares drag magnitude. Candidate pricing takes the available catch floor without live
  crank state; the live sign guard remains in crank settlement.
- The discrete free-crank stop alone regularizes drag at zero; raw Coulomb drag needs no second fade.
- The Driver owns staffing and work once. Staffing owns park and ceiling brakes; throughput owns
  fuel, gear/clutch work and worked steering. Empty-station construction rests persist.
- An explicit phase Brake intent retains its service hold without acquiring a staffed park lever,
  clutch-opening work or fuel. The persistent client notch is not cleared.
- A detent is selected by one threshold, from any history. Direct notch-grid admission replaces
  the search; no analog-input policy or alternative selector is kept.
- Tests of the deleted fade, rollback-cut recovery, reachability search and hysteresis go with
  those paths. The surviving tests assert the new class invariants.
- No wire type changed: PROTOCOL_REV remains 80. No tank-sheet/spec field changed, so there is
  no Blender fixture schema change. No engine API was added.
- Steps 4–7 stay pending rather than being described as completed or accepted. In particular,
  the Slip experiment has no outcome yet, the over-rev outcome remains the owner's call, and
  tank-#2 acceptance is not certified.

MEASURED final whole-library result: 2,212 passed, 26 ignored, zero failed. It includes the
track, HUD, drive-load, S1/S2/S4, rollback, bake and source guards. Formatting and CI clippy passed.
MEASURED source/assets delta against the dispatch base: **41 lines removed** in runtime files
and the input asset; **106 lines added** in test files, for **65 net lines added** excluding this
record and the unchanged-size glossary edit. The net-removed count for source/assets is **-65**;
this is not presented as a line-count reduction. Staffing reads fall 9 to 1 and work guards 5 to 1.

Players stop buying cuts during a service-braked rollback and stop releasing park for an
undeliverable steering request. Staffed ceiling braking continues with an emptied engine HP
pool. Radius selection responds at one threshold in either direction. These are asserted paths;
no visual capture, timing/pricing run or playtest was taken. The remaining #146 acceptance work
is steps 4–7 above and the owner's over-rev/empty-station descent outcome.

MEASURED separate whole-tree guard rerun: 44 passed, zero failed (`doc_citations ui_ascii
net_boundary bake::`). `git diff --check` passed. No open lens-backlog row names an edited file.
MEASURED closing checkpoint: 2026-11-02 18:20:02 UTC, 56 min 47 s after the measured start.
The next physical-law experiment, its S1/S2/S4 and rollback checks, and a possible tested revert
cannot fit the remaining 3 min 13 s. The lane therefore stops at green step 3 within the owner's
60-minute limit. No predicate-based stop was triggered. Steps 4–7 remain unattempted.

Three ordered commits carry the requested co-author trailer. No push, merge, release, issue
comment or external message was sent. All executions used the machine lock, unattended,
headless and muted; no lane-started process remains after the final checks.

## Resumed round: step 4 experiment

Subject: the game's driving model. MEASURED restart: 2026-11-02 18:24:15 UTC;
60-minute deadline 19:24:15 UTC. The same lane branch and bay continue.

The design admission was tried with a red moving-below-idle launch-gear witness (In instead
of Slip). The experiment replaced the at-rest restriction with the declared launch-gear domain,
priced fresh candidate launch Slip by that same domain, removed the upshift-into-lug refusal,
and omitted reflected inertia when the admitted idle guard holds the crank above the shaft.
The test of the replaced moving-lug admission and its pricing-only companion went with it.

MEASURED trial: 272 track tests passed, three failed. A capable quarter-pedal lab launch on
level ground selected F5 and finished at zero speed (final speed spread 0.052916586 m/s),
failing sustained travel. Resuming a retained notch also failed to gain perceptible speed.
The old catch-floor price expectation also changed, as expected from omitting reflected inertia.
S2 stayed green: the two grade fixtures launched without rollback or paid cuts, ending at
3.3605297 and 2.1343575 m/s. Reversal, rollback retention and backward ceiling witnesses passed.
The shipped headless S1/S4 trial was not run after these decisive class-invariant failures.

Decision: use the brief's explicit fallback. Restore the entire step-4 experiment, including
its replaced tests, and retain the narrow Slip entry and upshift-into-lug refusal. No compensating
predicate is introduced. The design statement alone does not preserve capable part-pedal travel;
removing the refusal still attracts an unsustainable landing. This is a failed experiment,
not a certified simplification. Its next attempt needs a design audit, not another local gate.

The audit's 3,152 to 1,964 lines and 17 to 8 state fields are a historical reduction already
reported in its verdict, not a projection for this lane's remaining steps. MEASURED dispatch
baseline: transmission.rs 1,966 lines and TransmissionState eight fields. Step 4 therefore
cannot honestly promise another 1,188-line or nine-field deletion: the audit already counted it.

MEASURED restored step-4 gates: formatting and CI clippy passed; 286 targeted track, HUD,
shipped drive-load and citation tests passed, including S1/S2/S4 and rollback. The green
checkpoint deliberately retains the original law. Commit: driving: record the rejected Slip experiment (#146).

## Step 5: over-rev provenance

The guard's 100 rpm width now belongs to assets/driving/engine.ron, labelled StandIn for the
missing engine over-rev outcome. It remains the same independent clutch guard and may set
available descent drag; this lane builds no replacement over-rev law. The owner decision is
outside this local fix, not an approval blocker. The field is required, finite, positive and
StandIn-labelled before construction. The arithmetic scale and witnesses read that one owner.
MEASURED gates: formatting, CI clippy and 286 targeted tests passed. No tank-sheet/spec or
wire shape changed. Commit: driving: declare the over-rev engine stand-in (#146).

Queue waits make the owner's priority clause relevant: step 7 precedes the remaining step-6
findings. Its fade-band expectation is included because Tank #2 must exercise that expectation.

## Step 7: different drivetrain, same invariants

Fixture: a three-forward/two-reverse-speed, 18,000 kg rig, ratios anchored at 5/12/25 km/h,
1,100 N m peak at 1,400 rpm, 2 kg m² crank and 3,000 kg per-side belt inertia. Its curve,
gear count, reductions, mass, power, shift window and capacities differ from the seed.
The fixture's data was not retuned after a failed assertion. All class scenarios also retain
the seed and the prior arithmetic drivetrain.

MEASURED first run: launch, held reversal, slope lever, part-pedal travel and joint-step
gear settling passed on the new construction. Its twenty-degree launch selected gear 2,
never rolled backward, paid zero cuts and ended at 2.3704474 m/s. Reversal paid exactly one
13-tick cut after the data-owned 32-tick pause. Top-gear settling reached gear 3 at
6.7000537 m/s with zero changes in the final simulated second. Seed and lab also settled.

Three red findings: constructor caps rejected otherwise finite construction; the coast
expectation failed the rolling fade; and the zero-rate descent fixture assumed every
upper-middle gear must accelerate on ten degrees. The last is not a game-law defect:
this rig's motoring drag already arrests that descent. Assertions are unchanged; the
brake witness's force setup now exceeds the construction's own drag by a share of its
service capacity, so a working brake is necessary on every construction. No vehicle datum
was tuned to make that witness pass. This is a correction to its class premise, not an
engine/brake-law patch or a relaxed pass condition.

The unexplained limits 20,000 rpm, 100,000 N m, static factor 2.5, crank 0.1–100 kg m²,
clutch 100–50,000 N m, belt inertia 1 kg and shift duration 3 s are removed. Positive
algebraic domains and finite derived ratios, divisors and capacities remain. Shift duration
must round to a representable u8 tick countdown; its saturating conversion is gone.
The actual derived-overflow witnesses remain. A full-library run exposed five obsolete
spec cap cases and the tiny-but-finite belt-floor case; those tests go with the replaced
rule. The same commit updates the Blender fixture to a small finite crank and reads its
literal spec from its owning script in a Rust admission witness. Python syntax also passes.

S3's independent expected root solves Coulomb/viscous drag plus the ground owner's rolling
fade, rather than assuming full rolling force. Its class is a positive unclamped root below
the crank/clutch limits and outside the discrete crank-stop cap. Each fixture must exercise
a root, and the suite must exercise a root inside the fade band. Missing a root cannot make
the witness vacuous. It uses the production drag/contact law only to check the independently
inverted balance, never to compute the expected speed.

MEASURED intermediate gates: formatting, CI clippy and 288 targeted checks passed. The first
whole-library run was 2,213 passed, one obsolete spec test failed, 26 ignored; the two corrected
spec tests subsequently passed in the focused run. No structural drivetrain change was needed.
The final unchanged-source verification batch covers this step and the independent review fixes;
its measured totals are in the closing evidence below. Commit: driving: admit and exercise different drivetrain construction (#146).

MEASURED final verification batch: 2,216 library tests passed, 26 ignored; 44 whole-tree guards and 7 explicit class witnesses passed. CI clippy, formatting and diff checks passed.

## Step 6: review closure and final evidence

MEASURED red: the backward HUD witness now goes through the real readout and fails on
negative-zero formatting. The formatter normalizes only the already-formatted zero string;
no new rpm threshold is authored. A real backward crank is also covered directly by readout.
The idle-floor witness covers the live negative-crank admission on every class drivetrain;
it passed on the current law and distinguishes the positive catch floor from the negative
independent guard. Public audio still consumes rotation magnitude.

The rollback witness now claims retained gear, not an unasserted braking outcome. It uses
Unmanned intent with the retained request rather than manually masking the request, drops
the no-op speed assignment, and labels stop/ceiling precision as Resolution. Drag's base is
Coulomb drag, and shaft/crank signs have their own references. The unsupported determinism
measurement is removed rather than replaced by a new claim. Historical steering numbers
and deleted-control narration are removed while preserving why a proportional band loses
capacity. The cubic smoothstep's exact coefficients are stated beside it.

Decisions: retain the failed step-4 fallback and its tests; no extra physical gate or state.
Keep the over-rev outcome as declared data, with no new law. Admit construction by domains,
finite derived arithmetic and representation, rather than by vehicle-class caps. Preserve
all brake assertions; derive a fixture demand that actually needs brakes. Reuse one verified
working tree for the two independent commit groups, with no source edit between checks and
commits. No wire change: PROTOCOL_REV remains 80. No engine API was added. The spec test edit
carries the Blender fixture update in the same step-7 commit. No applicable lens-backlog row
names an edited file; none was silently waived. All five lenses were read against the diff.

Final numeric inventory in transmission.rs, MEASURED from source with strings/comments
excluded: 0, 0.0, 0.05, 0.5, 1, 1.0, 2, 2.0, 3, 3.0, 3.6, 8, 24, 60.0, 64.0, 100.0.
Only Resolution values remain authored in code: 0.05 zero/rest tolerances, 24 bisections,
64 Hz solver clock and 100 rpm governor ramp. Every other number is exact: zero/identity,
paired outputs and half factors, cubic smoothstep coefficients, eight-field projection,
km/h conversion 3.6, rpm conversion tau/60, and u8::MAX (255) representation bounds.
Tau is an exact mathematical constant. The over-rev 100 rpm is now StandIn data, separately
from the Resolution governor ramp. No production line requires a vehicle-specific change for
the second drivetrain; only each inline Fixture's declared construction differs. The narrow
Slip/refusal laws retain data-derived boundaries and pass both constructions unchanged.

The audit's historical line reduction was independently checked in git: 3,152 to 1,964.
The lane's dispatch baseline was already 1,966 lines and eight state fields. Thus the missing
1,188-line/nine-field shrink is not an unrealized projection: that reduction preceded this
lane. Step 4's failed experiment cannot be presented as a new simplification. Its broader
Slip/landing design needs another audit. The over-rev engine outcome and empty-station
brake policy remain the owner's previously reserved scope, not blockers or new decisions.

Players stop buying cuts during a service-braked rollback, stop releasing park for steering
that cannot be delivered, and stop seeing a false minus on zero-rounded rpm. Staffed ceiling
braking works independently of engine throughput. Steering enters and exits each detent at
one threshold. These are test-proved paths, not a visual capture or a playtest.

MEASURED closing source: transmission.rs 1,950 lines (dispatch 1,966), 8 state fields (dispatch eight). PROTOCOL_REV remains 80.
MEASURED line deltas against dispatch: +181; implementation-file/assets bucket -72; dedicated-test-file bucket +253; all tracked files +520 (net lines removed -520). The file buckets include inline tests in their owning implementation file.
MEASURED closing evidence written 2026-11-02 19:24:20 UTC, 60.09 minutes after restart. No push, merge, release, issue comment, capture or pricing run. All started test processes completed under the single machine lock, unattended, headless and muted.
