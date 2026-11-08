# Track contact: how the belt meets the world

Subject: the video game's tank driving physics (the track belt, the suspension and their contact
with terrain, structures and rubble); game state only. The decision is
[[0041-track-contact-channels]]. This file owns the narrative; modules hold their own invariants.
Numbers are MEASURED on 2026-11-08 unless labelled DERIVED.

## Three channels, one law

| The belt meets | Channel | In the solver? | Owner |
|---|---|---|---|
| ground, standing construction, map blocks, another hull | sampled support | no | `track::forces` |
| a wall or a step past the belt's travel | outer-face stop and side plates | yes, as solver contacts moving at belt speed | `track::bump_stop`, `track::stop_contacts` |
| loose pieces | belt segments on `Layer::TrackBelt` | yes, as compliant contacts | `track::belt_segment` |

**The sampled support.** Each side has 48 stations round the closed loop and three points across
the shoe (both edges and the centre). Terrain gives one height-grid facet plane per column, read
for the column's three longitudinal depths (`world_contact::terrain`). A solid gives its clipped
convex faces (`world_contact::convex`): the footprint is partitioned so each covered cell belongs
to its deepest surface. The spring and damper act along the surface normal; each belt element's
strain carries the grip. A solid takes the whole opposite reaction: a fixed construction into its
load reading, a dynamic body as an impulse at the contact.

**The stop.** Past the wheels' travel a full-width prism per side on `Layer::Vehicle` is the belt's
hard limit, and thin slices at the shoe faces meet bodies laterally. Each surface moves at belt
speed in the solver (`stop_contacts`), so friction against a wall lifts the hull at μ times the
forward push wherever the belt wraps an end wheel. Solver friction returns to the transmission
through one reader of the contact transfers, the segments' with it.

**The segments.** The rest loop is cut into 0.3 m slabs (`SEGMENT_M`, Resolution) over suspension
travel, the outer face on the droop envelope. They meet only `Layer::Debris`. Each pair is a
compliant contact: Avian's stock contact solves its relaxing pass rigidly along the normal, so a
body cannot rest on it at a load-proportional deflection (DERIVED from
`ContactNormalPart::solve_impulse`). The vendored patch adds an absolute per-point law set by the
contact hook (`upstream/avian-compliant-contact.md`). A settled pile sleeps with the tank through
Avian's island sleep; nothing freezes a loaded piece.

**One law.** `k'` and `c'` per metre of belt are the spring channel's calibration
(`SegmentLaw`, `ForceParams`), and the envelope comes from the same travel knots, so the segment
and the sampled support hold a tank at the same deflection on a surface wider than the shoe. A
shoe is a rigid plate: a piece under part of its width still loads the running gear through the
whole link, so a segment takes `k' × length` whatever its coverage, shared over every point it
touches. Coverage moves where the force acts, so the roll moment follows the contact's position.

Known differences between the channels, none tuned:

- **Partial coverage.** The sampled support is a column quadrature over ground, so a curb under
  one edge takes that column's share; a segment takes its whole law. Over 0.1 m pieces covering
  0.728 of the shoe the rise is 0.119 m on segments against a covered-share reading of 0.073 m
  (DERIVED). Width enters only the sampled channel.
- **No engagement ramp** in the compliant contact; the spring channel ramps over
  `suspension.engage`.
- **Friction on pieces** is Avian's Coulomb at the pair's μ, not a belt element's strain, and no
  rolling resistance acts on a piece.

## The boundary, one rule

The channel is the layer a collider was built on. `Layer::Debris` (a construction's fallen pieces
and debris stress fragments, at any mass, `RubbleBodies::loose`) goes to the segments; the sampled
support keeps it in the footprint partition, so a covered cell takes no ground support and the
Driver perceives it, and applies no spring force. Every other solid stays in the sampled support.
The only dynamic body left there is a hull. DERIVED: for the shipped belt law on a 50 t hull at
64 Hz, `dt² k / m + 2 dt c / m` is about 0.2 against an explicit limit of 4.

## Resolution

48 × 3 is a `Resolution` number (`forces::CONTACT_RESOLUTION`). Belt stage on the local Mac for 30
tanks: 0.79 ms at 97 × 3, 0.51 at 48, 0.38 at 24, 0.32 at 16; the tick stops falling visibly below
48. Turning at 48 is within 0.2 % of 97, and drifts 1 % at 24 and 2 % at 16; 16 fails the slope law
on gravel. Three lateral points are the knee: the gap between points stays below the 0.5 m piece
size floor, so no piece sits unseen under a shoe (DERIVED).

## What was rejected, and why

- **The belt fully in the solver.** 46 segments and about 44 constraints per tank at rest. It passed
  the card's laws but for the slope hold, by under 1.3× the precision on speed. A hull is one body,
  so Avian's colouring puts half of those constraints in the overflow colour, solved on one thread
  (22 of 44), and the physics step priced 9.0 ms against the sampled channel's 1.0 ms.
- **A custom reaction law for pieces.** A clamped explicit impulse that held a loaded piece as
  ground. The pile's reaction ratio read 0.58 / 0.56 / 0.012 against the required 1.0, and in the
  loose heap it moved 39 / 49 pieces against the segments' 17 / 18. Local ram, first 4 s after
  impact: belt stage 2.8 to 4.4 ms against 1.1, and up to 64 ticks more than two late against
  none. It was also the larger code: its removal is −1548 +184 lines.
- **Exact-triangle terrain clipping.** It added 5.3 ms per tick for 30 tanks (4.2 to 9.4 ms p50,
  local); fixture loads moved 0.04 % on flat ground once replaced by facet samples. Convex overlays
  stay for solids, where the facets are few and the outline matters.
- **A segment's stiffness following the covered width.** It halved the loose-heap climb (rise max
  0.185 / 0.223 m to 0.104 / 0.113 m) and has no basis in a rigid shoe. The width enters the
  sampled channel only through its quadrature over ground.
