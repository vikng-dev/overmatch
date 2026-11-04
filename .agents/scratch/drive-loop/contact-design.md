**Decision:** replace the asymmetric rubble reaction with a coupled compliant solve inside Avian’s physics step, and replace shear’s expiry timer with geometric contact lifetime. Retain the bounded spring-slider grip model. This design concerns the game’s tank-driving state only; no implementation, builds or tests were performed.

**Rubble exchange — the missing truth.** Both bodies must participate in the same contact response, including their permitted linear and angular motion, suspension compliance, and the contacts holding the recipient up.

The finding matches [rubble.rs](/Users/Yan/github/vikng-dev/overmatch/src/track/rubble.rs:977). `explicit_recipients` includes source inertia, but only selects the recipient’s reaction. [sim.rs](/Users/Yan/github/vikng-dev/overmatch/src/track/sim.rs:1019) has already applied the hull’s full force. The unstable route therefore breaks action–reaction; adding inertia to its classification did not couple the actual exchange.

No relative closing does **not** imply no normal force: a compressed suspension can push, and a resting piece can transmit weight into the ground. Conversely, an unsupported piece cannot supply an independent external support force. It can exchange momentum with the hull while its compression unloads.

| Option | Changes and player feel | Cost on the server; risk; files |
|---|---|---|
| **External implicit solve** | Solve the belt contacts together from both bodies’ inertia; apply opposite impulses. Landings lose the explicit launch instability, and loose pieces accelerate under the hull. | Relatively contained, but recipient–ground contacts still respond in a separate solve. Heavy stacks can exhibit splitting error and delayed load transmission. Touch `rubble.rs`, `forces.rs`, `sim.rs`, sandbox adapter and contact tests. |
| **Compliant rows inside Avian — recommended** | Solve suspension and tangential exchange against current solver-body motion alongside ordinary contacts. A supported pile transmits load; a collapsing pile moves with the tank pressing it. Grip on landings follows the load actually exchanged. | More integration work and repeated row visits; connected piles limit parallelism. Main risks are solver ordering, sleeping bodies and drivetrain coupling. Touch the external option’s files, a new contact-solver module, `transmission.rs` and its tests, plus contact-transfer publication. |
| **Symmetric explicit substeps** | Keep the force law, recomputing geometry and applying equal opposite forces at smaller physics intervals. Ride remains closest in formulation to today. | Work grows with the stiffest contact and lightest permitted recipient. Substepping only the belt leaves the recipient’s other contacts split. Touch the same force/adapter files and physics scheduling; worst-case cost is the main risk. |

These costs are **DERIVED structural comparisons**, not measured timings. The hosting target is **DERIVED from the stated contract: two CPU cores**.

**Recommended rubble law.** Geometry supplies compression and contact anchors; construction supplies suspension stiffness and damping. Material data supplies friction. The contact module returns solved impulses and updated state through one interface; callers cannot independently choose a hull force and recipient cap.

For orientation, the isolated, locally linear backward-Euler normal row is **DERIVED**:

\[
W=\mathcal J M^{-1}\mathcal J^\mathsf T,\qquad
J_n=\max\!\left(0,\frac{h[kx+(c+hk)u_*]}{1+h(c+hk)W}\right)
\]

Here `x` is compression, `u_*` is closing speed after external velocity integration, and `W` includes both endpoints’ rotational response. This is a derivation aid: the production row must evaluate the actual engagement/profile law and update geometry across substeps.

Apply opposite incremental impulses at the same world contact point. Iterate shared-body rows using updated motion. Fixed and prescribed-motion supports remain external constraints, with their reactions recorded explicitly.

Grip must use the normal load this solve actually carries, rather than the discarded explicit force. Include belt inertia and drivetrain work in tangential exchange; preserve the existing distinction between grip and rolling resistance. Remove `explicit_recipients`, its stability classification, and the recipient-only catch-up clamp.

Avian’s vendored [normal solver](/Users/Yan/github/vikng-dev/overmatch/vendor/avian3d-0.7.0-child-pose/src/dynamics/solver/contact/normal_part.rs:117) already demonstrates paired effective mass and accumulated impulses. Its relaxation path removes softness, so ordinary softened collision contacts cannot simply become suspension springs. Use dedicated compliant rows, with explicit ordering and convergence checks. [Catto’s solver discussion](https://box2d.org/posts/2024/02/solver2d/) supports accumulated impulse projection and explains this relaxation distinction.

**Rubble proof — tests must name these invariants and their users.**

- **Contact exchanges conserve momentum.** Unrestricted dynamic endpoints receive opposite linear impulses and matching angular impulses; tanks and loose supports rely on this. Include off-centre contacts and a hull that is also another belt’s recipient.
- **Compression can release energy; discretization cannot invent it.** Account for kinetic, suspension and shear energy, external work, drivetrain input and dissipation. Replace “the piece never passes belt velocity” as a universal obligation.
- **Support follows constraints.** An uncompressed, co-falling pair has no invented support; a compressed pair can unload transiently. Adding ground beneath the recipient permits sustained weight transmission.
- **Numerical classification cannot change the physical law.** Mass, stiffness and timestep sweeps converge without a full-reaction/clamped-reaction discontinuity.
- **A contact’s effects are delivered once.** Hull motion, recipient motion, belt reaction and contact-transfer readings derive from the same solved exchange. A no-op recipient application must fail the production-adapter witness.
- **Partitioning preserves the represented contact.** Equivalent footprint subdivisions converge to the same resultant and work without multiplying support or advancing shear repeatedly.

**Shear lifetime — the missing truth.** Recoverable shear belongs to an ongoing material contact. Contact existence comes from geometry; damping and force magnitude describe its response.

The finding matches [forces.rs](/Users/Yan/github/vikng-dev/overmatch/src/track/forces.rs:1364). Collection already records positive elastic penetration when damping clips the emitted support force away. Nevertheless, `forget_departed_elements` preserves shear after **all** such evidence disappears. Its departure test explicitly removes the ground and requires retention until expiry.

The comment that a parked belt cannot present stale shear to different ground is insufficient: the hull or supporting piece can move while belt phase stays fixed.

| Option | Changes and player feel | Cost on the server; risk; files |
|---|---|---|
| **Element-wide geometric lifetime** | Keep shear while any valid footprint contact remains; erase immediately when none remains. Damping-only dropouts preserve grip, actual hops release it. | Cheapest: reuse collection evidence. However, one surviving patch can preserve shear belonging to a departed support. Touch `forces.rs`, `sim.rs`, bitprobe state and tests. |
| **Persistent geometric witnesses — recommended** | Associate shear with material elements and continuing support patches. Follow local anchors on moving pieces; expire departed portions and start new contacts unstrained. Bumps retain bite while connected; landings cannot recover an airborne spring. | Additional witness storage and matching. Recheck previously active witnesses when sampling becomes ambiguous. Risks are false departures at seams and incorrect split/merge handling. Also touch `oracle.rs`, rubble footprint queries and sandbox tests. |
| **Stateless tangential constraints** | Eliminate shear history and solve stick/slip directly from current motion and strength. Removes stale-contact memory entirely, but changes presliding response and spring-back. | Broader belt/transmission solve and different player feel. Touch the recommended rubble solver, transmission and force tests. It is a viable separate constitutive choice, beyond the required lifetime correction. |

**Recommended lifetime model.** Use explicit `Separated` and `Touching` state, with continuing support witnesses and bounded recoverable shear. Construct storage synchronously at spawn.

A witness identifies the supporting body or continuous terrain surface, its covered material region, and local geometric anchors. Triangle or quadrature changes alone do not create a new contact. Simultaneous supports need distinct histories where they can depart independently.

Positive elastic overlap remains contact evidence when damping suppresses force. Confirmed clearance, lost footprint overlap, return-run departure or removal of the supporting surface releases the affected shear immediately. Nearby speculative contacts carry no old shear across a confirmed gap.

When ordinary samples disappear, recheck the previous witness against the actual local footprint geometry. Use spatial uncertainty derived from query precision, rather than elapsed ticks. Swept evidence must detect a resolved departure and recontact within an update.

Delete the dwell field and countdown. Nominal-weight enter/leave thresholds cease deciding lifetime; evolve bounded shear continuously under the chosen contact law. Preserve continuing histories through patch subdivision without copying their stored energy into newly contacting regions.

The distinction between geometric detection and force activation is established elsewhere too: [MuJoCo documents detected contacts that generate no force](https://mujoco.readthedocs.io/en/stable/computation/index.html#margin-and-gap). That supports separating the concepts, not retaining shear across a real gap.

**Lifetime proof — tests must name these invariants and their users.**

- **Damping cannot end geometric contact.** Continuing overlap with suppressed normal force retains the contact history; parking and rough-ground grip rely on it.
- **Departure releases shear.** Confirmed clearance erases history at that observation, and immediate recontact starts unstrained.
- **History belongs to the continuing support.** A replacement or removed moving piece cannot inherit another support’s strain.
- **Geometry bookkeeping cannot alter grip.** Continuous terrain seams, reordered patches and equivalent subdivision preserve the continuing history; partial departure releases only the departed portion.
- **A graze cannot store an unaccounted launch.** Load-ramp tests check bounded recoverable work and force through the coupled solve, rather than asserting a particular membership branch.
- **Temporal refinement resolves the same events.** Departure/recontact results converge as physics intervals and geometric sampling are refined.

**Parallel ownership.** Replace the global `piece_impulses` walk with connected-group evaluation.

Build vertices from dynamic body owners, merging source and recipient identities when they name the same body. Belt state belongs to its hull owner. Belt contacts connect their endpoints; shared sources therefore connect their recipients. Fixed supports are immutable terminals and do not join unrelated tanks.

Under the recommended solve, extend connectivity through participating ordinary contacts and joints: distinct belt graphs touching through moving rubble are coupled. Include every support row affecting an owner, including field and fixed-surface rows.

Each disconnected group owns private working velocities, shear state and impulse accumulators. Evaluate groups on the existing compute pool; retain declared row order within each group. Batch small groups without making independent evaluation serial by assumption.

Commit completed results in a declared group/owner/contact order. Keep shared contact-transfer mutation at that commit. Wake participating sleeping bodies before solver-state preparation; sleep must not silently turn a dynamic recipient into an immovable support.

The [drive-loop report](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/drive-loop/report.md) correctly identifies within-group velocity dependencies, but those dependencies do not justify serial evaluation of disconnected groups. Verify disconnected composition against isolated solves, including reversed worker completion. Hosted dispatch and convergence costs remain unmeasured.

**History count and stacking.** I counted distinct corrective commits, including relevant test-only corrections, at inspected HEAD `63312624`; initial features, mechanical adaptations, prose-only changes, refactors and performance work are excluded.

| Scope | Count and auditable lineage |
|---|---|
| **Rubble exchange and its immediate adapters/tests** | **DERIVED: 18 rounds = 14 behavioural corrections plus 4 test-only follow-ups.** Behaviour: `3404fc54`, `b691454f`, `7a0c0636`, `d029b3ef`, `81baee80`, `9a091f1e`, `fe9e047f`, `0968f3df`, `11ecb471`, `1754b7a8`, `7f4f4690`, `66bca7ce`, `d8296b68`, `9552cf70`. Tests: `c72e9b2a`, `c837aed7`, `64c474ed`, `b290d7d4`. |
| **Element lifetime/membership and its tests** | **DERIVED: 3 rounds = 2 behavioural corrections plus 1 test follow-up.** `8ce2eddd` introduced elastic membership, hysteresis and dwell; `1d738a38` changed low-load accumulation; `4989a311` covered an existing member fading through the hysteresis band. The dwell itself was introduced once, without subsequent retuning. |

Rubble clearly shows stacking: clamp, stationary-drag repair, hull exception, stability classification, damping correction, derivative corrections, coupled-inertia correction and light-recipient isolation. The unresolved asymmetry survives them all.

Lifetime shows a smaller compensation stack: elastic evidence solves the demonstrated damping dropout, while hysteresis, dwell and low-load policy accumulate around it. The timer’s justification remains unproved by geometry.

The [reaction review record](/Users/Yan/github/vikng-dev/overmatch/.agents/docs/design/wall-destruction-sweep-2.md:530) already acknowledges the asymmetry and omitted source-support springs. Its proposed held/free classification would remain a compensation unless those supports participate in the exchange. The [ground-contact proposal](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/designs-2026-11-03/ground-contact-merged.md) calls for geometric lifetime, but current code has not implemented it. The [solver-track proposal](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/designs-2026-11-03/solver-tracks-merged.md) supplies a useful direction; its model results do not establish production cost or ride quality.

This is a design for owner approval. Its acceptance evidence is the named laws, convergence, hosted cost and observed driving feel—not preservation of today’s rubble rise, landing response or spring-back distance.