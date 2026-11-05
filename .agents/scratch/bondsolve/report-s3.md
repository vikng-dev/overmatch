# Bondsolve, session 3

Subject: the video game's environment destruction model, game state only.

## Contact step

The construction design's Structural calculation section owns the law. The contact step
replaces welded trial reactions and local lift/tip/slide rejection with projected pressure and
friction, redistribution, and an explicit contact-mechanism outcome. The retired bearing-lever
function and its isolated assertion are deleted. Closed cohesive cracks remain contacts;
entirely open cracked patches and escape mechanisms separate. `Separate` is an authoritative
record cause distinct from `Crack`; the wire revision and own-type fingerprint change together.

The analytical board, overhanging plank, both sides of the friction angle, endpoint reversal,
closed-crack compression/lift, finite pulse and beam witnesses pass. The unit-normal witness
guards an actual numerical defect: f32 source normals need renormalizing in f64 before the
normal/shear projection. Without it, the house's contact residual stopped decreasing at a
MEASURED relative residual about 1.91e-8; the corrected solve passes its 1e-8 gate.

Placed scales reuse an intact displacement guess and still solve their own loads before
admission. No crack, separation, verdict, fate or pre-settled configuration is copied. The
bulk scaling derivation supplies a warm guess, not a waiver of any load or convergence gate.

The broader contact run had 256 passes and two fixture failures. Those fixtures now explicitly
separate supports and close the contacting body's actual momentum change, including its final
velocity. The subsequent contact/guard run passed 68 tests, with two observational tools
ignored. Formatting and workspace/all-target CI clippy passed. This is incremental verification,
not a claim that the broader failing invocation passed.

## Fixing compliance and roof facts

The attachment data now supplies required normal/lateral slip slopes. DERIVED EN 1995-1-1
section 7.1 reference lateral stiffness is 781.421 N/mm per nail, or 8.335 MN/m³ of contact
area at the declared density. Axial withdrawal remains an explicit Unknown lateral analogue.
Bulk compression/friction acts in parallel; it is not the fixing's stiffness or counted twice
in its strength. Strengths, fracture energies and the house geometry/joins are unchanged.

[Roof construction, sources, grain axes and owner options](roof-s3.md) records the missing
framing and inappropriate automatic board-edge nailing. Directional timber response remains
open; its along/across facts are sourced there, not substituted into scalar strengths to pass.

The saturated contact tangent retains transverse slip stiffness and its affine force term.
The mechanism scan uses physical point motion and linear work in contact sites. Admission
uses the closest admitted intact displacement as a guess; every scale still solves separately.

## Final gravity observation

MEASURED by both ignored census instruments; observations are not assertion oracles:

- Unit house: 1,122 chunks, 3,258 joints; zero initial overloads, first-tick cracks/departures,
  or sparse rounds. Unit tree: 37 chunks, 132 joints; the same zero outcomes.
- All 635 tree placements, scales 0.50205–1.499289: no cracks or departures in the observed
  stages, no unfinished owner after 128 law ticks.
- All 74 house placements, scales 0.549203–1.463569: first-stage cracks range 0–15, with
  zero first-stage departures. Crack ranges for stages 1–7 are 0–14, 0–10, 0–11, 0–44,
  0–104, 0–784 and 0–1,092; those eight stages show zero departures. **35 houses remain
  unfinished after 128 law ticks.** Individual scaled failure classes need diagnosis.

**The spawn bar is not met.** A clean unit fixture does not establish that the placed houses
stand. Do not price or claim play readiness from this result. No strength was raised and no
cracks or settled topology were seeded.

## Gates and cutoff

Formatting and final workspace/all-target CI clippy passed. The complete compliance/contact/
attachment/stats run passed 36 tests (two ignored), before the final scale-guess and mechanism
scan changes; their restarted caller run reported no failure but was interrupted. The new
fixing-versus-bulk ownership witness passed within that run. **All 48 final source/bake guards
passed on the final source.** Both observational gravity instruments completed successfully.
Earlier interrupted invocations and partial caller runs are not full passes.

The dedicated server rebuild was stopped at the 90-minute cutoff before a boot began.
Therefore the requested ten-second boot has **zero collected stats lines and no completed
correctness observation**; zero lines is not a running-server result. No lane process remains.
No timing run, ram price, capture or visible binary was taken. Next session must diagnose the
scaled-house failure classes, finish caller verification, and perform the headless boot before
pricing. Source logs and censuses are under `target/bondsolve-s3-*` in this bay.
