# Driving sweep 2: review and disposition

Subject: the game's driving, tracks and running gear, game state only. Part 1's record is
[`sweep-drive-1-review.md`](sweep-drive-1-review.md); this lane reviewed its thirteen commits
(`81ebb5b3`..`5ea18dd0`) and took its open rows. Base `5ea18dd0`, bay-2, MEASURED start
2026-11-01 11:30:43 +0700. No captures, pricing runs, deployments, API calls or pushes.

## Review of sweep-drive-1

MEASURED, headless, on `5ea18dd0`: full throttle F8, 10.684 m/s over the last second (spread
0.0023 m/s); neutral pivot −0.1338 rad/s, 0.7100 of its 0.1884 rad/s kinematic ceiling. Both equal
the last recorded values before the lane, so the declared stations, sides and parent-frame posing
leave the shipped tank driving as before on flat ground.

| Finding | Severity | Disposition |
|---|---|---|
| Cmd+Q route mounted only if `WinitPlugin` was already added: a root adding the quit plugin before `DefaultPlugins` silently lost the route `81ebb5b3` made a startup refusal | P2 | Fixed: `TerminateRoute` decides in `Plugin::finish`. |
| `check_quit_latency`'s `None` arm unreachable once the route mounts only beside `WinitPlugin` | P3 | Fixed with the P2: `Res<WinitSettings>`. |
| The three `quit` refusals | none | Agree: the delegate class is the linked winit's contract and the update mode is the root's own composition, both code, so a panic (bug outcome) is the right class. Neither is an operator lever. |
| Sheet prose said nothing geometric is authored and the tooth count is the only sprocket datum | P3 | Fixed (comments only, line count kept). |
| Two `#[cfg(test)]` circle builders twinned `pose_circles`: tests built circles by a path the product no longer takes | P3 | Fixed: the tests build a rest list and call `pose_circles`; the twins are deleted. `RigGeom::wheel_pin_radius` stays a test-only reading for one wrap test's probe bound. |
| `axle_position` composes the full parent affine (scale included) while the bake's `root_position` composes `pos += rot·t` without scale: under a uniformly scaled parent group the axles and the armour/collider placement disagree | P3 | Open, load area: one composition should serve both. |
| `braking_deceleration`'s grade probe uses the inertia box length, a mass-distribution fact, as the contact footprint | P3 | Open (scripted battle only). |

Scripted stopping (`07b8664e`), battle ownership (`8ddeba3a`), aim volumes (`86545414`) and the
reserve scaling (`e29dabb8`) were read for correctness; no P1 or P2 found.

## Driving inventory, part 2

| Finding | Disposition |
|---|---|
| G5 | **Partly fixed, P1 stays open.** Dynamic support is generic: a solid collider (no `Sensor`) whose layers meet the hull's contact class, on a dynamic body the caller's piece query can write (self and other tanks are outside it). Static support still reads `BuildingElement`/`BuildingChunk`. |
| R19–R25 | Fixed: every `bind_link_template` / `mirrored_mesh` fault refuses; only assets still loading wait. A vehicle view instance without the template is refused on `WorldInstanceReady`. |
| `Stragglers::Park` | Disagree with the inventory: `track_sandbox::link_view` parks stragglers on a live-tuned link count, a product caller, not a test-only arm. |
| `ShoeBelt::pin` | No finding now: its one writer is the capture plan's `LodShowcase` scene (`lod_showcase`), a named user. |
| G1 | **Open, for the owner.** Findings below; the constant is unchanged. |

### G5: what remains

Admitting static bodies generically (standing and fallen trunks, scatter) is blocked by a missing
reaction path, not by representation: a static body takes none of the belt's reaction, and the
structure laws (`contact_transfer`, the walls' and trunks') read load only from solver transfers.
An admitted standing trunk would carry the belt, which climbs it by traction, and never be loaded,
so no tank could fell a tree it meets with its track. Walls already have this gap: the belt bears
on a wall element and the wall never feels it. The fix is to feed the belt's reaction on a static
structure into the same transfer reading, then drop the building identity rule and add the field
representation rule (heightfield and authored blocks are already field surfaces and must not be
counted twice) and cylinder support in `Surface::triangles` (scatter trunks). Other tanks still
need a tick-start snapshot and staged reaction writes in the server order.

### G1: grip shear compliance

`GRIP_SHEAR_MODULUS_M` = 75 mm is applied to every track on every ground. CLAIMED sources: Wong's
Janosi–Hanamoto shear deformation modulus K is a soil property, about 6 mm (clay at maximum
compaction), 10 mm (firm sandy terrain) to 25 mm (loose sand), per Altair's summary of Wong,
*Theory of Ground Vehicles* (2008); `static-friction-design.md` quotes 75 mm as Wong's value for
rubber pads on firm ground. The shipped track is `TrackSteel`, so the 75 mm is a rubber-pad value
on a steel track, and the truth it stands for belongs to the track/ground pair (ground-type data),
not to one constant.

MEASURED at 64 Hz, headless, the shipped tank on flat ground, value changed temporarily and
reverted (`headless_test`, `track::forces`, `track::transmission`, `track::bump_stop`,
`track::rubble`, 103 tests):

| K (mm) | Top speed (m/s), spread | Pivot (rad/s), ratio | Belts L / R (m/s) | Tests |
|---|---|---|---|---|
| 75 (shipped) | 10.684, 0.0023 | −0.1338, 0.7100 | 0.275 / −0.300 | pass |
| 25 | 10.693, 0.0046 | −0.1333, 0.7074 | 0.226 / −0.350 | pass |
| 10 | 10.694, 0.0074 | −0.1331, 0.7065 | 0.224 / −0.352 | pass |
| 6 | 3.302 in F5, 0.0972 | −0.1331, 0.7064 | 0.225 / −0.351 | top speed and gear climb fail |
| 5 | 2.107 in F4, 0.1206 | −0.1331, 0.7063 | 0.226 / −0.350 | top speed and gear climb fail |

So the law keeps flat-ground top speed and pivot at the sourced 10–25 mm, with the last-second
speed spread growing 2–3× and the pivot's belt speeds turning asymmetric; below about 10 mm the
straight-line drive collapses at this tick rate. Only the one shipped construction was measured;
synthetic transmission fixtures passed at each value but measure no stability margin. Not changed:
the move to a pair property, the value for the base ground and the feel of the softer or stiffer
park are the owner's.
