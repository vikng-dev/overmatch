# Track lane 3b review: pieces under the belt

Subject: the game's tank driving physics, game state only.

Reviewed `d8b09746` and `5d37cc54` (issue #238, lane 3) against the error-handling,
code-branching and base-truths lenses and for correctness.

## Findings

- **P1, fixed by the material-pair change (lane 4).** `forces::emit_contact_forces` gripped every
  piece patch at the field's coefficient (0.9) while `rubble::piece_impulses` clamped the piece's
  drag at the pair `√(0.9 · μ_piece)`. The two sides of one contact used different coefficients:
  the hull took more traction from a brick than the brick took back, and no piece's substance
  changed the hull's grip. Each contact now carries one pair (`BeltContact::mu`) that both sides
  read.
- **P3, support resultant.** Detection is exact for convex pieces: the six support calls only
  reject, then the convex boundary is clipped to the link's footprint prism before its extrema
  are read. It is no sampled stand-in. The resultant is not exact: each overlap supports its
  clipped rectangle at its deepest point, so a slanted piece supports a whole link at its
  highest point. The error is bounded by the piece's height change within one link's footprint.
  `pieces-under-belt.md` states this.
- **P3, reaction order.** Piece reactions are sequential impulses in tank-entity order, which is
  deterministic on the server. A later tank's support damping and slip read the piece's
  velocity after earlier tanks' impulses, so two tanks on one piece depend on entity order.
  Sequential-impulse solvers share this property. The rubble module doc names the impulse order
  but not this read.
- **P3, partial-width rise.** The 0.077 m driving rise over 0.1 m pieces is a fact of the
  support law, not a gap in the footprint. The law spreads each column's spring over its share
  of the shoe width, and the strip beside a piece keeps pressing the ground. A piece covering
  share `s` of every grounded shoe therefore lifts a level hull by `s·h`. The fixture's DERIVED
  share is 0.7280, so the share law predicts 0.0728 m against a MEASURED 0.077294 m. The
  4.5 mm excess is ASSUMED to come from the zero-travel sprocket and idler arcs meeting the
  pieces. The full-width pad's 0.099954 m follows the same law at `s = 1`. A real shoe is rigid
  across its width and would ride at the piece's full height. That lateral rigidity is a
  construction fact the law does not read; it is the owner's call, because it also changes how
  a curb under one shoe edge supports the hull.
- **P3, acceptance band.** `a_driving_tank_rides_onto_loose_pieces_through_its_belt_support`
  accepts 0.5–1.25 h, an ASSUMED band. The share law gives a tighter class invariant (`s·h`
  within the end-arc allowance), which the test now prints.

Stand-ins: `RubbleOracle`, `TerrainOracle::probe` and `surface_velocity`, `Probe`, the sandbox's
oracle fold and their tests are deleted. Only the rubble module's rejected-alternatives prose names
the probe-line fold.

## Lane 4: friction from the material pair

The track's material is tank data (`track.substance`), checked at load (`L2.SPEC`). Every track
contact's coefficient is `GripParams::pair`: the track's row with the ground type under it, or
with the piece's row, under the one pair rule. The bump stops, side plates and belt read that one
row. `track::sim::MU`, the bump stop's named substance, the sandbox's own coefficient and
`belt_friction` are deleted. `net::extrapolate` bounds hull acceleration by the selected tank's
pair on the base ground.

Decision: keep the owner-tuned grip of 0.9 on the shipped ground. Under the pair rule, the base
ground's 0.5 needs a track row of 0.9² / 0.5 = 1.62 (DERIVED), recorded as `Tuned` in
`materials.ron`. That is far above a steel sliding coefficient (ASSUMED 0.4–0.8 on stone). The missing factor is the shoe's
grousers biting into the ground (#146). The same row now sets the bump stops' and side plates'
pairs: with a brick (0.65), √(1.62 · 0.65) = 1.03, against 0.57 before.

MEASURED on the shipped ground, headless, before and after: top speed 10.684 m/s both (F8);
neutral pivot −0.1338 rad/s both (ratio 0.7100). The sliding-grip test MEASURED a ratio of
1.414214 between the shipped ground types of least and greatest friction (0.3 and 0.6), equal to
the DERIVED pair ratio √2.
