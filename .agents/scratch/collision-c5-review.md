# Collision c5 review

Subject: the game's environment destruction and physics, game state only; contacts never dose a tank.

Reviewed prerequisite commits `ea77e880`, `47d0eab3`, `f1a435ab` against AGENTS.md, all three repository lenses, #232's owner decision, #235, and the blast model recorded in `wall-destruction.md`. The standards and spec checks were performed by the implementer; the lane prohibits subagents.

## Standards

- **P2, `ContactTransfers::contact_mass`: zero inverse response is a valid locked-body contact.** Avian's `ContactNormalPart::generate` uses `recip_or_zero` for that response while retaining the dynamic body's constraint. An unconditional positive-inverse assertion stops the authority for such a body. Fixed by retaining the physical infinite source mass and excluding zero blows from hinge demand. Solver and mixed-load regressions cover both paths.
- **P3, `Origin::Body::mass`: stale field documentation.** The source mass is the contact's combined-blow centroid mass, not each point's mass. Corrected the field comment.
- No P1 finding. Other changed hunks have no additional findings under error-handling, code-branching or base-truths.

## Spec

- The shared source mass fixes the repeated expenditure of the receiver mass across a manifold. The cube regression checks the combined mass against its construction; the actual source response is still capped jointly across departures.
- Retain the ideal-gas far reflection, joined at the near fit's domain edge. The rejected recalled branch violates the normal-reflection lower bound. This is a generic gas law, not a fitted behavior exception.
- **P3 evidence limit:** all peak-pressure coefficients remain CLAIMED, including the incident fit feeding the far reflection. Join, scaling, monotonicity and reflection-bound tests establish internal consistency, not agreement with a primary empirical table. No independent coefficient validation is claimed; source validation remains an evidence gate for the blast model.
- No additional P1/P2 spec finding. The peak force and the pulse-factor helpers are deliberately staged for their named blast lanes; this review does not claim they are already consumed by a law.

## Deferred collision-lane findings

- Degenerate hull: refused before play by the layout load gate, naming the hull field and rebake fix.
- A queued contact on a chunk already freed by an earlier deposit: the carrier already launched. Retain the solver stop for that stale contact until the next physics step can contact the carrier. Dust alone refunds the whole blow. This is the named absence of a standing receiver, not a second launch or refund.
- Tiny exchange: carry products and source sums in double precision; a wholly rounded-away response has zero exchange and returns the unspent blow. The fixture sweeps ordinary, tiny and subnormal inputs.
- Design prose: state the field's zero-source-mass limit directly, without retelling the study's contradictory test wording.
