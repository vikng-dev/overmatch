# Track contact: how the belt meets the world

Subject: the video game's tank driving physics (the track belt, the suspension and their contact
with terrain, structures and rubble); game state only. The decision is
[[0041-track-contact-channels]]. This file owns the narrative; modules hold their own invariants.
Numbers are MEASURED on 2026-11-08 unless labelled DERIVED or dated otherwise.

## Three channels, one law

| The belt meets | Channel | In the solver? | Owner |
|---|---|---|---|
| ground, standing construction, map blocks, another hull | sampled support | no | `track::forces` |
| a wall or a step past the belt's travel | outer-face stop and side plates | yes, as solver contacts moving at belt speed | `track::bump_stop`, `track::stop_contacts` |
| loose pieces | one soft run per side on `Layer::TrackBelt` | yes, as compliant contacts | `track::soft_run` |

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
through one reader of the contact transfers, the runs' with it.

**The soft run.** Per side, one convex collider: the rest belt's outer outline across the shoe's
measured faces, on `Layer::TrackBelt`, which meets only `Layer::Debris`. It is read as links of the
belt's pitch, each carrying `k' × link` and `c' × link` at the deepest overlap any contact gives it,
shared among the contacts on it by depth. A contact's shares are its stiffness and damping; its
deepest points that bear split them evenly, and a rest length places its force at the step's start
(`ContactCompliance::rest`), so force, stiffness and damping each add up to the links' own however a
piece splits. A piece's contact is read on the one face of the run its normal picks. On a rim face
its points are projected onto the face's outline edge, and its overlap runs along it, linear between
them; a contact on a lateral cap carries one link's law along its normal, outside the links' budget
(the cap law, below). Each contact is compliant: Avian's stock contact solves its relaxing pass
rigidly along the normal, so a body cannot rest on it at a load-proportional deflection (DERIVED
from `ContactNormalPart::solve_impulse`). The vendored patch adds an absolute per-point law
(`upstream/avian-compliant-contact.md`), with a signed rest length, under which a point whose spring
is free at the narrow phase bears nothing all step, and `soft_run` is its one writer: it keeps every
point, so a contact's law is a function of the contact as the narrow phase left it. Nothing freezes
a loaded piece.

**Rest.** A tank at rest sleeps with its pile through Avian's island sleep, whatever carries it.
The sampled support goes through the non-waking force call, so no force resets the rest timer,
and a sleeping tank runs no belt pass (`track::sim::NotResting`): its contacts keep the last
pass's points, and a fixed construction under it keeps that pass's reaction in its load reading
(`contact_transfer`, IMPLICIT CONTACTS). Avian's rest test reads motion only, and a body that
starts from rest under an acceleration below `SleepThreshold`'s 0.15 m/s over `TimeToSleep`'s
0.5 s, 0.3 m/s² (DERIVED), would sleep before reaching the threshold. Each pass judges what motion
cannot show (`track::sim::apply_track_forces`), and one that finds the tank not at rest zeroes
its timer (`track::sim::settle_rest`): drive demand, a belt its brake does not hold, a brake meeting a
stop beyond its capacity, every gripping patch at or past its budget, or a sampled support whose
body is not static. A tank creeping past its grip limit keeps creeping:
`contact_law_shipped_brakes_capture_below_moving_limits_and_move_above_static_limits` judges the
slope limits under production rest, mud at 17.0° against its 15.5° limit among them. A tank
never rests on a support that can move by itself, another tank's hull among them, an admitted
limit: Avian links bodies into an island only through solver contacts and joints
(`dynamics/solver/islands`), the sampled support is neither, so that support's motion would wake
nothing it carries, and a joint per tank and support, which would carry the link, was rejected
on cost. The drivetrain is no part of the rest: a sleeping tank's hull and contacts stand still, but
its transmission steps each tick on its last pass's belt loads with the belts at rest
(`track::sim::wake_driven_tanks`). A step that holds both belts and slides no brake is staged and
commits if the tank still rests once the tick's wakes have applied (`track::sim::settle_rest`), so a
repaired engine spools up under the resting hull; any other step is discarded and wakes the tank,
whose full pass runs that tick from the same state, as does a tank whose island another body's wake
reaches: a drivetrain advances once per tick. Drive demand (a pedal, a steer, or a belt
still coasting) wakes the tank before any trial: under a pedal the lever holds a belt only against
rollback, a sign test on contact jitter that frozen loads cannot answer, and an awake twin's belts
crept from the first tick of full throttle (MEASURED once), so its full pass moves it within its
tick. An emptied engine may wake the tank: its clutch springs in and the crank can slide the
parked brakes (MEASURED on the repair fixture: the clutch went in and both belts slid). A support that goes, is disabled, turns sensor or changes wakes it as
it wakes a sleeping piece (`debris`), through the colliders its last pass sampled
(`track::sim::SampledSupports`). A round's impulse and the tank's own shot wake it through the
waking impulse; a moving body or a falling piece wakes it through the island wake.

**One law.** `k'` and `c'` per metre of belt are the spring channel's calibration
(`forces::SupportLaw`, from `ForceParams`). A shoe is a rigid plate: a piece under part of its
width still loads the running gear through the whole link, so a link takes `k' × link` whatever
its coverage. Coverage moves where the force acts, so the roll moment follows the contact's
position.

Known differences between the channels, none tuned:

- **The datum.** The sampled support's zero-force datum is the droop envelope, so the tank's weight
  holds it at its rest pose on ground. The run's is the rest outer face, so a piece's force adds to
  the ground's under the same belt, both springs acting: a hull on a loose slab `h` high rises `h/2`
  where the same slab fixed lifts it `h`, at twice the link's stiffness, and a tank carried on
  pieces alone rides one free travel lower against them than on ground (DERIVED: the same `k'` from
  a datum one free travel higher). The drawn belt follows the drawn pieces (`track::view`). The
  datum at the droop envelope, past the ground's own intrusion at each link, measured worse
  (MEASURED 2026-11-14): the stop's lift share fell to 0.21–0.25, but the straight drive's heave
  peak rose from 14.9 to 24.1 m/s², the hull's rise from 0.15–0.18 to 0.26–0.39 m, creep crossings
  fell from 5/12 to 2/12, and a tank asleep on pieces no longer woke on drive demand. Its rise is
  about one free travel (0.173 m), the datum working; its law, a secant stiffness, explains at most
  8 % less damping (DERIVED, below); its heave and creep changes were never isolated from the moved
  geometry and the higher ride; and the lost wake was a rest judgement, the tank sleeping while its
  pitch settled. What would settle it: the run kept at the rest face with a collision margin of the
  travel's reach, each link's rest length a dead band of the ground's own intrusion there, so a
  piece carries only its excess past the ground (the whole link where no ground reaches) and the
  hull rises `h` on the loose slab as on the fixed one. It moves the ride height on heaps, so it
  goes to the owner as a proposal first; it would round the reach beside the shoe's edge, where the
  margin is a sphere's.
- **Partial coverage.** Over ground the sampled support is a column quadrature, so a curb under one
  edge takes that column's share. A loose piece under part of the width carries the rigid plate
  whole: its link takes its whole law. Width enters only the sampled channel's ground.
- **No engagement ramp** in the compliant contact; the spring channel ramps over
  `suspension.engage`.
- **Friction on pieces** is Avian's Coulomb at the pair's μ, not a belt element's strain, and no
  rolling resistance acts on a piece.
- **One face per piece.** A convex pair gets one contact with one normal (DERIVED from parry 0.27:
  `contact_manifold_pfm_pfm` clips the faces `ConvexPolyhedron::local_support_feature` picks, the
  ones whose normals best match the contact's, then adds its GJK witness point). Avian exposes each
  point midway between the two shapes' witnesses (`contact_manifolds`, avian 0.7), so an exposed
  point need not lie on the run's face: a run witness at (z, y) = (3, 3), outward normal (0.6, 0.8)
  and 0.1 m of overlap expose (2.97, 2.96), inside the outline. The reader selects one face from the
  normal and projects every exposed point onto it, so a piece bearing across a corner of the outline
  loads that face alone; the belt past the corner takes nothing from it that step. At an exact tie
  between two edges the run may take the one parry did not, and the feet then clamp onto their
  shared corner.
- **The cap law** is a `StandIn`. A contact on a lateral cap carries one link's `k'` and `c'` along
  its normal, split over its points in overlap however many parry gives it, outside the links'
  budget, so a piece against the shoe's side takes no support from the rim's links. It continues the
  rim's law across a flip, where parry's two depths are equal: a piece within one link, bearing at
  one point, carries the same force on either face, and one reaching `n` links on the rim `1/n` of
  it on the cap (DERIVED). The truth beside a shoe is steel, and the stop gives it above the
  compression wrap, sharing the run's lateral faces and meeting every layer (`bump_stop`): the stop
  is the lateral limit. Two cases meet a cap: a pivot sweeping the shoe's side into pieces, and a
  piece under the shoe's edge whose least overlap is sideways, an artefact of one normal per convex
  pair whose truth is the radial law, which no cap law gives. A rigid cap would eject by their
  lateral overlap the pieces the soft run leaves inside the solid; the nearest measurement is the
  belt segments' side plates below, 28.5 reversals (related, not the same, since a piece there sat
  in two colliders at once). An inert cap would hand those pieces to the rigid stop. Parry clips a
  face to its first four vertices (the TODO in `ConvexPolyhedron::local_support_feature`, parry
  0.27), so a cap contact, its face an N-gon, is mostly the lone GJK witness point (DERIVED).
- **The law within a step.** A contact's shares and the points that bear are read once, at the
  narrow phase, so its stiffness is exact for a common compression of its links, not for one piece
  retreating alone. The points split the force by `overlap + rest`, so the centre of pressure and
  the rotational stiffness are those of at most four parry points, and Avian's midpoint point sits
  half the overlap inside the run, so a foot near a corner can land one link over.
- **The secant it replaced** was a damping error, not a feel defect: within a step a secant `k_sec`
  in place of the tangent `K` acts as a damper of `(k_sec − K)·dt/2`, about `0.084·(k_sec/K − 1)` of
  the link's damping for the shipped law (DERIVED: `c'/k' = 2ζ/ω = 0.093 s` unless the travel is
  chain-clamped, at 64 Hz), +4 % for a piece `d` and `3d` deep across one link.
- **Restitution** above zero would take a rigid impulse on a compliant point
  (`ContactConstraint::apply_restitution`); pieces and runs set none, and Avian's default is zero.

## The boundary, one rule

The channel is the layer a collider was built on. `Layer::Debris` (a construction's fallen pieces
and debris stress fragments, at any mass) meets the soft run and the stop, and no side plate; the
sampled support reads no collider a run meets (`rubble::RubbleQuery::near`), so the ground under a
piece keeps its own support and the drivetrain reads the piece through the run's solver friction.
Every other solid stays in the sampled support. The dynamic bodies left there are the hulls and a
launched turret, which stays on `Layer::Vehicle`. The explicit spring is stable for a body while
`dt² k / m + 2 dt c / m` stays under 4. DERIVED: about 0.2 for the shipped belt law on a 50 t body
at 64 Hz, so the bound holds down to about 2.5 t.

## Resolution

48 × 3 is a `Resolution` number (`forces::CONTACT_RESOLUTION`). Belt stage on the local Mac for 30
tanks: 0.79 ms at 97 × 3, 0.51 at 48, 0.38 at 24, 0.32 at 16; the tick stops falling visibly below
48. Turning at 48 is within 0.2 % of 97, and drifts 1 % at 24 and 2 % at 16; 16 fails the slope law
on gravel. The three lateral points are Simpson's nodes (both shoe edges and the centre), exact for
lateral pressure through a cubic profile.

## What was rejected, and why

- **The belt fully in the solver.** 46 segments and about 44 constraints per tank at rest. It passed
  the card's laws but for the slope hold, by under 1.3× the precision on speed. A hull is one body,
  so Avian's colouring puts half of those constraints in the overflow colour, solved on one thread
  (22 of 44), and the physics step priced 9.0 ms against the sampled channel's 1.0 ms.
- **A custom reaction law for pieces.** A clamped explicit impulse that held a loaded piece as
  ground. The pile's reaction ratio read 0.58 / 0.56 / 0.012 against the required 1.0, and in the
  loose heap it moved 39 / 49 pieces against the belt segments' 17 / 18. Local ram, first 4 s after
  impact: belt stage 2.8 to 4.4 ms against 1.1, and up to 64 ticks more than two late against
  none. It was also the larger code: its removal is −1548 +184 lines.
- **Belt segments, with side plates meeting pieces.** 0.3 m sprung slabs over the wheels' travel,
  their outer faces on the droop envelope, the sampled support shadowing the ground beside a piece
  across the whole shoe. On the house heap a pivot shook pieces back and forth under the belt, 28.5
  reversals median against the soft run's 0, and the tank slept after stopping in 41 of 48 starts
  against 47 of 48 (MEASURED 2026-11-14, 12 to 36 starts per arm).
- **Exact-triangle terrain clipping.** It added 5.3 ms per tick for 30 tanks (4.2 to 9.4 ms p50,
  local); fixture loads moved 0.04 % on flat ground once replaced by facet samples. Convex overlays
  stay for solids, where the facets are few and the outline matters.
- **Stiffness following the covered width.** On the belt segments it halved the loose-heap climb
  (rise max 0.185 / 0.223 m to 0.104 / 0.113 m) and has no basis in a rigid shoe. The width enters
  the sampled channel only through its quadrature over ground.
