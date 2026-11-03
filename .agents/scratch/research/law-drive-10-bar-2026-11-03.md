# Driving owner's-bar remainder (#146)

Subject: the game's driving model. References to the game's damage model below mean engine
and crew HP pools and their gameplay state only.

MEASURED start: 2026-11-02 23:41:50 UTC. The owner's sixty-minute boundary is 00:41:50 UTC.
No capture, timing measurement, visible window, issue comment, push or merge is authorized here.

## Crank ceiling

Commit: `driving: declare the crank ceiling independently of curve sampling (#146)`.

Claude's finding is verified by an executed red: an end-clamped Fixture tail extends from
3000 to 3500 rpm with identical sampled delivered torque, yet the old ceiling becomes 3500 rpm.
`engine.crank_limit_rpm` is now required, with provenance on every sheet. Landing refusal,
ceiling service brakes, the over-rev guard and the force-roundoff scale read that fact.
The regression reaches all four, including an actual ceiling-brake force and over-limit landing.

Admission requires idle < governed <= rated, curve data bracketing governed speed, and a finite
crank limit at or beyond the completed fuel cut. The curve may end before or after that limit;
its endpoint has only its torque-interpolation job. The named limit refusal tells the author to
raise the limit or lower governed speed; the tank loader adds the source filename. The Blender,
bake, spec and compact fixtures change with the schema.

Decision: the arithmetic lab's limit is 1900 rpm, beyond its 1800 rpm governor's completed cut;
its zero-torque endpoint no longer substitutes for a shut fuel rack. These are inline Fixture
values. Shipped feel is preserved by construction: the shipped limit remains exactly 3000 rpm,
the old endpoint, and no shipped torque, inertia, gearing, steering or actuation value changes.
No wire type changes; `PROTOCOL_REV` is deliberately unchanged.

MEASURED first gate: formatting and CI clippy passed; the targeted module/guard run passed 434
checks, with four ignored, and exposed one test comparison at a steep curve segment. That test
sampled 1740 rpm, converted to rad/s for the law, then compared the result with the unconverted
rpm rather than the rpm represented by the input. Its 0.0024 N m discrepancy is f32 roundoff,
not excess fueling. The comparison now reads the actual input operating point; the production
law and tolerance are unchanged. The focused rerun and asset guards are recorded below.

MEASURED correction gate: formatting and CI clippy passed. The focused fuel-rack/ceiling,
doc-citation and asset guards passed 188 tests, with four ignored. All other 434 targeted checks
had passed before the assertion correction. No backlog row names an edited file in this step.

## Inertia admission

Commit: `driving: bound crank admission by torque slope (#146)`.

MEASURED red: adding the inline Fixture point (826 rpm, 1311 N m), collinear with its neighbours,
refused the unchanged construction with an inertia error. The minimum-gap bound is removed.
Admission splits the governed torque law at raw-curve knots and both fuel-cut boundaries.
Each span is quadratic, so its largest absolute derivative is at an endpoint. The Resolution
bound is J >= dt * max|dT/domega|: a falling branch cannot reverse a crank-speed perturbation
in one free-crank Euler tick. Adding a collinear knot cannot change that stability reason.
A separate genuinely steep branch must still be refused with the named curve/inertia fields
and an instruction to correct construction data or increase solver resolution.

Decision: no fixture inertia changes. The runtime torque arithmetic is unchanged. The virtual
free-crank energy admission now bounds one tick's acceleration by raw peak torque * dt / J,
rather than assuming a whole fuel-cut width, which the retired gap bound had guaranteed.
The crank-limit gate compares the actual rpm difference with the fuel-cut width so rounding
at very large authored rpm cannot admit fuel still flowing at the limit.

MEASURED inertia gate: formatting, CI clippy and 437 targeted module/source-guard tests passed,
with four ignored. The collinear-knot regression is green, and the steep branch is refused.

## Launch target and shared final gates

Commit: `driving: target the torque peak available after fuel cut (#146)`.

MEASURED reds: the post-cut raw-peak regression returned no capable launch on the seed-class
fixture, then on the compact-first fixture. Compact's tail was 900 N m at 2600 rpm with its
2200 rpm governor, exactly the reported construction. The fixed target searches the same
fuel-governed polynomial spans used for inertia admission, including each concave span's
interior vertex. Ties retain the lowest rpm at or above idle. A separate analytic Fixture
checks a peak inside the fuel ramp, so clamping the raw peak to governed speed is insufficient.
Both class constructions now select within their own launch domains and replace an initial
cruise gear. The raw peak N m remains the motoring-drag anchor; only the deliverable peak's rpm
owns launch and steering rev targets. Shipped peak rpm is unchanged.

MEASURED focused baseline: all three launch/interior-peak/pivot regressions passed. The final
formatting, CI clippy, whole-tree guards and whole-library run are shared across independent
items 3, 4 and 5, then each item is committed separately once that final tree is green. Queue
waits count toward the hour; sharing the final gates avoids repeating the same headless suite.

MEASURED shared final gate: formatting and CI clippy passed, all 44 whole-tree guards passed,
and the whole library passed 2,266 tests with 29 ignored. The whole-library run includes the
required track, headless, command and crew-UI tests. No wire type changed; `PROTOCOL_REV` stays
unchanged. No backlog row names any edited file. The final source was read against error
handling, branching, base truths, testing and brevity; the explicitly outstanding item-6 rows
below prevent a claim that the whole drivetrain clears the bar.

## Sheet-derived test entry points

Commit: `driving: derive test entry points from each construction (#146)`.

Verified Claude's two couplings by reading their exact preconditions: the rundown witness
required 2400 rpm regardless of governed speed, and course entry indexed F6 regardless of the
ladder length. The rundown now reaches a Fixture point 95% of the way from authored idle to
governed rpm. Course entry uses the construction's upper-middle gear and its mid-band crank
speed, deriving belt speed through that gear's ratio and sprocket radius. The file sweep also
bounded the declared launch limit by the actual ladder in the coast witness; a declared launch
limit may legitimately exceed a short ladder. The hull fixture reads gravity from its owner.

The second-spec integration doc explicitly states that it replaces the match's single blueprint.
It makes no coexistence claim. Decisions: preserve the narrow arithmetic/contact fixtures and
read their owned inputs; do not build a second-match-construction mechanism. The corrected
rundown, course, coast and second-spec tests all passed in the shared whole-library gate above.

## Pivot witness and executed mutant

Commit: `driving: exercise steering work in the unfueled pivot witness (#146)`.

Verified Codex's finding: every old stationary seed set task throughput to zero, so Driver
cleared steering and set park. The replacement retains the stopped zero-work corner, but each
stored-energy seed has staffed, available controls and starts at authored governed/limit rpm.
Above the rev target no combustion is requested. The witness stops before renewed fueling,
asserts actual induced/catch torque is zero, and requires counter-rotating belts, released park
and an open main clutch. Total crank-plus-belt energy may only decrease in that interval.

MEASURED production mutation: the actual `step` caller passed only mean coupling forces to
`engine_work`, while still applying its steering forces. The repaired witness failed: energy
rose from 70,675.53 to 73,689.69 J without combustion. The saved production source was restored
and compared byte-for-byte; the shared whole-library gate then passed. No mutant remains.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude | Crank ceiling inferred from curve endpoint | Verified executed red; all four readers covered | driving: declare the crank ceiling independently of curve sampling (#146) |
| Claude | Inertia floor depends on digitization | Verified executed collinear red; steep refusal stays | driving: bound crank admission by torque slope (#146) |
| Codex | Launch target beyond fuel cut | Verified executed reds on both classes and interior-peak green | driving: target the torque peak available after fuel cut (#146) |
| Claude | Seed-specific rundown/course entry | Verified literal/index preconditions; corrected tests pass | driving: derive test entry points from each construction (#146) |
| Codex | Pivot witness never pivots | Verified; actual steering-work mutant red, restored green | driving: exercise steering work in the unfueled pivot witness (#146) |
| Claude + Codex | Small rows, item 6 | Verified by source; unfinished at the hour boundary | — |

## Item 6: verified, still outstanding

These rows were not dismissed as non-defects. They remain for the next green step.

| finding | live evidence / needed change |
|---|---|
| `Controls` staffing doc | `engine_torques`, `idle_catch_capacity` and the adapter also resolve served work; correct the claim of exclusive Driver ownership. |
| `steer_radii_m` doc | Candidate pricing and `load::turn_radius` consume the radii; they are not presentation-only. |
| Optional grip after admission | `step` still reconstructs `DriveConstruction` with an `expect` each tick; braking repeats it. Retain checked driving construction after the edge. |
| `ParsedTankCommand::hold` doc | Names nonexistent `DriveTarget::Brake`; the variant belongs to `DriveIntent`. |
| `DriveFeel` admission/error | The first threshold gate permits `tight_on == 1`, which reachable-notch admission refuses; the parse error lists only three of five fields. |
| Edit-history comments | The named comments remain in `drive.rs`, `transmission.rs`, `spec.rs`, the shipped sheet and transmission tests. Preserve current rationale while removing history labels. |
| Conditional part-pedal/domain evidence | Capability guards still have no per-class non-vacuity requirement. |
| Weak launch, grade, top-gear and paid-shift shapes/prose | The reported selector-controlled absence, implied else assertion, universal prose and double negative remain. |
| `price_driver_force` default-build residue | Static caller check confirms non-test use only by the bitprobe trace; its compile guard is still absent. |
| `TrackGripElements` doc | Still describes removed initialization replication and rollback/checkpoint paths. |

## Decisions, feel and disposition

- Keep one required, provenanced crank limit independent of torque sampling. Preserve the
  shipped 3000 rpm value; the inline lab moves to completed fuel cut at 1900 rpm.
- Use the monotone explicit-tick torque-slope bound, including fuel cut; do not alter physical
  fixture inertia to satisfy digitization. Derive the free-energy bound from peak torque/J.
- Target the deliverable torque maximum, including ramp vertices; retain raw peak N m for the
  existing drag anchor. No shipped torque evaluation or target rpm changes.
- Derive rundown/course entry from each sheet. The 95% band point and upper-middle gear are
  Fixture choices, not pass conditions on a named tank. The second-spec fixture replaces the
  match singleton. Coexistence is explicitly outside this lane.
- Exercise the existing unfueled interval with staffed controls; add no new Driver or Slip
  predicate to make the pivot witness possible.
- Share final gates for independent items 3–5 and commit each separately; start no item-6
  overhaul with insufficient time left for its gates. No stop-rule design change was attempted.
- Leave rollback arrest, sub-idle/upshift refusal, over-rev outcome and empty-station descent
  decisions untouched. No wire change, so deliberately no `PROTOCOL_REV` bump.

Shipped feel is unchanged by construction: its ceiling, deliverable peak rpm, runtime torque
arithmetic, inertia, gears, steering, brakes and actuator data are unchanged. The complete
library and shipped driving witnesses pass; there was no visual playtest or capture. A future
construction with a raw torque peak beyond fuel cut no longer loses a capable launch. Curve
redigitization no longer forces an inertia change or moves the crank ceiling.

Judgment: **the full owner's bar does not clear yet**. Items 1–5 are complete; checked grip,
non-vacuous test evidence and the stale interface/history prose in item 6 still stand in the way.
No new owner gameplay decision is needed to complete those already-authorized rows.

MEASURED queue evidence: 15 cargo calls waited a total of 1,573 seconds, with a longest wait of
444 seconds, from the queue log's queued/started timestamps. These waits count toward the hour.
Final gates: formatting, CI clippy, 44 whole-tree guards and 2,266 library tests passed; 29 library
tests are ignored measurement/opt-in cases. Asset guards also passed in the first step. No push,
merge, issue comment, capture, pricing run or visible window occurred. All lane-run processes
terminated. The bay stays on its lane branch for the orchestrator; it is not released here.
