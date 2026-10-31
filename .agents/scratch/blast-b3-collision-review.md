# Collision lane 7 prerequisite review

Subject: the game's environment destruction and physics, game state only. Contacts here never dose a tank.

Reviewed `89daae10`, `463d62f4` and `eeb3140a` against AGENTS.md, the base-truths, error-handling and code-branching lenses, and the accepted contact and blast models in the wall design and #237. The implementer performed both standards and spec reviews; this lane permits no other agents.

## Standards and correctness

One P2 implementation defect was established and fixed: the ring's bounded plane projection rejected feasible translations on nonorthogonal boundaries. No additional P1 was established. The non-monotone ram is a consequence of the accepted reading, not evidence of a mistyped coefficient or a broken plastic-share calculation. It remains an owner decision, as the brief permits. The numerical force approximation below is a material-model limitation, not a claim of converged contact physics.

- **P2, `ring` translation: a feasible direction was rejected.** Three successive rounds of normal projection leave a residue on a triangular prism's side normals. The candidate fails the final half-space check even though translation along the extrusion remains feasible and the load does positive work on it. This is reached by nonorthogonal chunk boundaries and oblique contact loads, with no change to the generic law's contract. Fixed by `translation`: enumerate the cone's possible active planes and edges, preserving the maximum-work admissible direction. The regression uses a triangular plug, both extrusion senses and all boundary orders, and checks its shear strength/work. No alternate projection path remains.
- **P3, `Origin::Body::step` / `Load::mean_force`: contact time is unstated.** The stored time is `ContactTransfers::dt`, the numerical physics step. Avian's public solver accumulates constraint impulses over substeps; it does not resolve material contact compliance or duration. `blow / step` is a lower bound on the peak if the transfer occurred within the step, not a physical peak. The field path has a separately stated peak and does not use that time. The code comment and design now state this distinction and its rate sensitivity.
- **P3, `RING_GROWTH`: an overstrong search claim.** The old comment said capacity must increase with each grown ring and therefore the cap never decides the breach. That argument needs uniform strengths and suitable geometry; neither is an invariant of the generic structure. Nonuniform seams or a concave group can have a weaker boundary beyond the search. The comment now names the finite candidate search. The cap remains a resolution approximation, not a material truth.
- **P3, `body_demand_uses_the_held_hinge_mass_and_fields_are_the_zero_mass_limit`: imprecise cross-reference.** The force-limit test is now named by symbol. Its vanishing-step fixture isolates an impulsive limit; it is not a physical contact-time measurement.

The remaining changed hunks introduce no retained alternative, per-vehicle rule, asset-specific pass condition, invented shear default or silent error path. `Shear::OpeningOnly` states the attachment schema's current knowledge; the missing shear law of a nailed fixing remains explicit, rather than borrowing solid timber's shear data.

## The ram reversal: exact terms and decision

The prior lane's MEASURED fixture has a body mass of 46.8 t and an Avian rest speed near 0.15 m/s. At closing speeds 0.12–0.14 m/s its contact is wholly a press, `m v / dt`; the strength-only check breaks four chunks. At 0.16–0.37 m/s it is a blow. The strength gate can yield but the energy gate does not: the freed group's energy after the plastic exchange is `J² M / (2 (m_b + M)²)`. At 0.38 m/s that gate is paid and the prior lane reports nineteen chunks freed. Those are the prior lane's measurements, not a new simulation in this review.

This follows directly from `ContactTransfers::split` filtering closing momentum at the body's rest speed, `weigh` reading presses by strength alone, and `ring_failure` / `section_failure` reading body blows with `Origin::share` in the demand. It also explains why changing only the numerical force gate cannot close the energy gap. There is no missing factor in the implementation of the accepted freed-group-energy reading.

**Decision:** retain that accepted contract in this material-data lane. The owner must choose whether continuous contact can pay separation work from the moving pair's kinetic energy instead. That would use `J² / (2 (m_b + M))`, multiplying today's available work by `(m_b + M) / M` (DERIVED), and changes sections, rings and rests, not merely this fixture. A sleep-threshold tweak would hide the discontinuity rather than supply that base truth. Launch and give-back still need their existing momentum share; an energy-law change must not replace those shares indiscriminately.

## Physics-rate evidence

`a_body_contact_step_bounds_its_peak_from_below` keeps one fixture plug's impulse and both inertias fixed, and samples numerical steps at 64, 128 and 256 Hz (ASSUMED fixture rates). MEASURED: the strength is 41,250 N, the impulse 429.6875 N s; the force bounds are 27,500, 55,000 and 110,000 N. The plug holds, then fails, then fails. Its energy suffices throughout; only the step-based strength bound changes. Thus the result **does change with physics rate** even when the underlying blow is unchanged. The class invariant asserted is that tightening that bound cannot turn a failure back into a hold, not that the rates agree.

A steady force is different: its transfer is `F dt`, so division returns the same `F`. The existing `sustained_overturning_is_independent_of_tick_rate` covers that contract. Resolving rate-independent body peaks requires contact compliance/duration from the materials and construction; inventing a constant duration or silently deleting the strength limit would implement a different law.

## The three changed fixtures

1. **Hinge seams without shear:** valid isolation. `a_push_fails_the_section_at_its_fracture_impulse` pins the angular impulse capacity of a hinge. With cohesive shear supplied, a different translation mechanism may be cheaper, so requiring the hinge verdict on that fixture was no longer valid. Separate punch-ring tests keep the cohesive path exercised.
2. **Hole-test push reduced:** valid fixture repair. `a_hole_changes_no_collider_mass_or_structural_result` needs the same chunk standing for a second game-model input in both copies. Its assertion compares the two copies and their replicas, not a chosen breach size. The smaller ASSUMED push stays below the new ring threshold; the test does not claim the old larger input should hold.
3. **Vanishing timestep:** valid limit fixture, limited evidence. The body/field comparison tests vanishing source mass with strength already exceeded. A vanishing source mass alone cannot remove a finite-duration force gate. Its tiny step establishes the additional impulsive limit, not a realistic contact duration. The separate rate sweep now makes the limitation visible.

Destruction review: one P2 geometry fix; three P3 documentation dispositions. Owner decisions remain the accepted ram energy reading and a physical body-contact duration model. The sourced far reflected-pressure branch matches the audited coefficients; no change to it was warranted.

## Validation

MEASURED: all 39 fracture tests, 5 sandbox-wall tests and 3 doc-citation guards pass, within the combined 139 targeted building/material/spec/wall/doc tests. Formatting and locked workspace/all-target clippy with `dev_ui,bitprobe` and warnings denied pass. Tests ran through the shared build queue with unattended/headless settings; no game window, capture or pricing run was used.
