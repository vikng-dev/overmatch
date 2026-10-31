# Collision c6 prerequisite review

Subject: the game's environment destruction and physics, game state only; contacts never dose a tank.

Reviewed `8e793eee`, `36c695b6`, and `0e8a1a36` at `532c6822` against AGENTS.md, the error-handling, code-branching and base-truths lenses, and the owner decisions in #232, #235 and #237. Both standards and spec were reviewed by the implementer; this lane permits no subagents.

## Standards

- **P2, `fracture::Origin::share`, numerical failure:** a locked source has infinite effective mass. A tiny inverse hinge mass can round to zero, making their product undefined and silently hiding the demand. Fixed in `1fd9050f`: evaluate the infinite-source-mass limit explicitly. The regression sweeps zero, subnormal and ordinary receiver responses. The locked contact is a legitimate state with zero source response, now handled by its named limit instead of producing a numerical failure.
- **P3, `fracture::unseated`, unstated input:** its comment said gravity and presses although the rest pass also reads blows. Fixed with the rest-law change.
- **P3, `collision-c5-review.md`, stale disposition:** the earlier review retained the static stop for a stale departed contact. Corrected here to the live, momentum-conserving whole refund.
- No retained switch, per-object rule, asset-specific pass condition or additional silent-failure finding in the remaining changed hunks. The relabelling test compares the law's data view, not an asset-specific outcome.

## Spec and correctness

- **P1, `fracture::section_failure`, inconsistent hinge direction:** the axis was chosen from raw angular impulses but demand used pulse-weighted impulses. Opposing raw loads could cancel exactly, or choose the opposite hinge, even when the finite pulse is held by strength and contributes nothing. Fixed in `2f65b6a2`: choose the axis from effective blows. The regression compares an impulsive load alone with the same load plus an opposing strength-held pulse, at cancellation and reversal.
- **P2, `fracture::unseat`, twist reported as tipping:** with no vertical resultant, any torque counted as a tip, including pure twist about vertical. The new peak-strength reader reaches the same defect. Fixed in `30b3dc8b`: only horizontal overturning moments can displace the pressure centre. The regression distinguishes twist, overturning and sideways sliding. Torsional friction remains an explicitly unassessed mode, not a fictitious tipping failure.
- **Stale contact:** the whole refund is correct. The source's solver stop removed the queued blow; the carrier from an earlier deposit already received its own launch. Refunding the queued blow restores the source, adding no second impulse to that carrier. A later dynamic contact transfers momentum through the solver. The branch and its regression name the legitimate absence of a standing receiver.
- **One pulse law:** `Load::felt` branches on stated peak-force data, not a source kind. The finite pulse meets strength through the rigid-plastic demand factor; an impulsive load is its vanishing-duration limit. Body-blow strength gating remains expressly assigned to collision lane 7. This review does not claim that contacts already meet both limits.
- **P3, phase approximation:** the design explicitly labels in-phase addition of an impulsive line and a finite pulse ASSUMED. The effective angular impulses now add consistently, but no arrival times or deformation history exist in `Load`. This is a model boundary, not a proven phase relation. Resolving delayed or cancelling pulses requires load history; no undocumented fallback or new switch is introduced here.
- **P3, geometry boundary:** a group lying across a section plane has no Navier reading; `section_pulse` explicitly keeps the impulsive energy limit there, as the design records. Its finite-pulse strength is unassessed, like the weight check's corresponding geometry. This remains a named model limitation pending a static solve.

## Measurement audit

MEASURED by rerunning blast lane 2's saved `measure3.rs` in `sandbox::wall::tests` after the fixes: the same matrix has 78 cases and exactly one different verdict, bare 5 kg at height 1 m and standoff 1 m, 230 versus 229 fallen chunks, with two versus one Push failures. All other case summaries agree. These are fixture inputs, not empirical acceptance bounds.

The fixture is the sandbox's free-standing east-wall cut, not the complete house. The charges are ASSUMED energy inputs 1.47 MJ and 5 x 4.184 MJ, heights 1, 1.45 and 2 m, standoffs 0.5, 1, 2, 4, 6, 8, 10, 11, 12, 13, 14, 16 and 20 m. The comparison drops each peak for the impulsive counterpart while keeping the same loads and runs strike, structural, weigh and fall. The temporary comparison arm was removed after the run. Log: `/tmp/collision-c6-wall-matrix.log`; the original harness and log remain in the orchestrator's `scratchpad/blast-b2/`.

This reproduces the prior count claim, not the proposed far-toppling distance or agreement with material experiments. Primary validation of the recalled pressure-fit coefficients remains the prior review's evidence gap. No captures or timing measurements were taken.

Destruction review: fixed one P1 and two P2 findings; four P3 dispositions above (documentation, stale disposition, phase, geometry). No additional findings.
