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
