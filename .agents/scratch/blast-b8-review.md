# Blast lane 8 review

Subject: the game's environment destruction and damage model, game state only. Structure loads
and physical transport are in scope; crew HP pools are unchanged.

Reviewed `63c495cf` through `70cb34e8` against parent `42135567`: every changed hunk, the
error-handling, code-branching and base-truths lenses, issue #237's settled text, and the supplied
blast study. Standards and spec were read directly; the owner forbids delegation.

## Standards

- **P2, fixed:** `sandbox::wall::Charge::of` invents zero speed when a core or cased round is
  absent. The new products-momentum invariant then panics from `Charge::blast`, which the
  readout calls even before the operator uses the tool. A next tank without a burst block
  reaches it. The tool now refuses missing core/casing construction at initialization and
  names the asset field and repair. The cased/bare operator setting has complete construction
  in either position; no unsupported bare-only fallback remains.
- **P3:** `Blast::products_floor` repeats crossing measurements and a named asset's numerical
  outcome already narrated in the wall design. Keep model results in that design; the module
  needs only its input units, field invariant and assumptions.
- **P3:** `Blast::peak_pressure` says it is a function of scaled distance alone. Its products
  correction also depends on the charge design's products momentum relative to its blast
  energy. `products_floor` names this condition, but the entry-point comment should too.
- No new per-object rule, runtime alternative or silent failure was found in the production
  field, hull exit or contact changes.

## Spec and correctness

- **Decision: retain the impulse floor.** The study contradicts itself: it first recommends
  `max(fit, products)` for impulse, then asks for a peak-only clamp while citing an impulse
  under-read as the reason. The settled issue explicitly calls for correction of contact
  impulse. Flooring impulse by the authored products momentum is the honest reading of that
  decision. The model's reflection factor and retention of the reflected fit's pulse duration
  are both labelled ASSUMED, in code and the design. Neither is a verified contact-charge law.
- The cylindrical linear-speed Gurney profile gives scalar products momentum `(2/3) C v` by
  integrating over its mass; its outer speed is the same casing speed used by the existing
  burst law. The bare tool takes that law's zero-casing limit. No second chemical-energy
  allocation is introduced.
- Outside the products/fit crossing, `specific_impulse` and `peak_pressure` evaluate the
  pre-floor expressions in the same arithmetic order. The tests compare float bits over
  scaled charges, ranges and incidence angles. The crossing is specific to charge design,
  rather than a universal fixed scaled distance.
- `Block::exit` keeps exactly the outgoing-plane upper bounds of convex clipping. An interior
  point needs no entering-plane bound, so its exit is unchanged mathematically. Its caller's
  impulse-weighted face centroid lies on the convex face up to rounding. Ignoring parallel
  side-plane violations therefore permits an edge centroid without treating it as a missed
  hull; a point already outside an outgoing plane exits at zero.
- `WallState::contacts` keeps a cracked bonded seam only while both sides remain standing.
  Those static chunks retain their baked poses, so loss of tensile cohesion does not open a
  geometric gap. Opened rests and departed neighbours still expose the back. The test covers
  each case. Propagation through contacts to a farther free surface remains the documented
  approximation.
- No P1 or P2 spec finding remains. The unsupported sandbox construction is the one P2
  standards finding above.

## Verification

Formatting, locked workspace/all-target clippy with `dev_ui,bitprobe` and warnings denied,
and targeted sandbox-wall, fracture and doc-citation tests pass (MEASURED test results).
The new refusal test exercises initialization with no cased-round construction. No engine API,
collider or spawned body was added. Tests ran unattended and headless through the machine lock;
no game window, capture, pricing run or external write was made.
