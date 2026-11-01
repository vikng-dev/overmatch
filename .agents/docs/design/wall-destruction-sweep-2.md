# Environment destruction sweep, part 2

Subject: the game's environment destruction and damage model, game state only.

The lane started at 12:47:08 +07 and reached its measured 90-minute cutoff at 14:17:08.
Base: "buildings: refuse a peer wall entry the map cannot hold where it arrives"; reviewed part 1:
from "buildings: refuse non-finite and malformed building geometry at load" to that base. No subagents, captures,
pricing runs, pushes, deployments or external-service calls were used.

## Review of part 1

Standards and correctness: no P1 finding in the reviewed diff; three P2s fixed separately:

- R1: local turns admitted self-intersecting profiles. Every vertex must now lie in every
  edge's interior half-plane. Malformed-geometry refusals include the repair ("buildings: refuse crossing profile boundaries at load").
- R2: tree bindings were validated only on falling. Missing, duplicate and outside bindings
  are now refused at template construction with glb, extras field and repair ("trees: refuse invalid part bindings when the template loads").
- R3: invalid scales were refused in scatter without their source filename. The manifest
  parser now names the file, instance, scale axis and repair ("map: refuse invalid instance scales at the manifest edge").

The peer wall entry is outside process input at `net::walls::receive_ledger`, the first
map-aware receive gate. `world::plugin` spawns the map synchronously in Startup before the
client's asset-gated connection and Update receives. Admission checks happen there once;
downstream shape checks assert the internal record invariant. The structural/weight loop
assertions correctly require monotone crack progress. Removing the segmented-tree merge
fallback is correct: required shipped primitives must merge or refuse.

D1 disagreement: retain the owner-approved hole-cap crumbling path. No alternative was added.

## Physical findings

**Item 1: onset fixed, continued loose-body fracture remains open** ("buildings: open only locally overloaded joints under weight"). The old
weight check used the worst joint's tensile ratio to open an entire section, including
compressed neighbours. Only individually overloaded joints now open; disconnected groups
are not static compression props. Material strengths were not tuned.

**MEASURED**, identical headless shipped-hull/house push: before, 1,003 chunks in 48 groups,
largest 664; after, 316 chunks in 59 groups, largest 24. The fixture uses the shipped hull
and house, normal driving/contact systems, ASSUMED initial speed 1.2 m/s and placement.
The roof gate now checks local fixing failure instead of requiring every coplanar fixing
to open. Its residual 74 of 150 roof chunks stand; the pass reports zero touched chunks
unassessed. This does not certify the residual load sharing. A full mixed-contact/fixing
static solve remains a model limit. Loose bodies still have no continued seam assessment:
uniform free fall alone supplies no relative gravity demand (DERIVED), but contact and
angular motion can load seams and require a further generic law.

**Item 2: fixed** ("sandbox: use the game's one blast and deposit path"). The game, sandbox and geometry tests now use one ray driver;
the sandbox also uses `run_laws` and shared exact-collider launches. The cased source is
explicit dev data. Filler and casing facts come together from that compiled round, replacing
an unrelated core lookup. Zero kinetic cavity energy does not mean zero pressure removal.

**MEASURED**, aimed solid-face cases through the shared path:

| Fixture | Standoff | Crushed volume | Rear scabs | Impulse |
|---|---:|---:|---:|---:|
| Cased HE, 0.907 kg | 0.10 m | 0.014013 m3 | 4 | 963.1 N s |
| Cased HE, 0.907 kg | 0.30 m | 0.003444 m3 | 1 | 939.0 N s |
| Cased HE, 0.907 kg | 2.00 m | 0 | 0 | 688.8 N s |
| Bare, 0.900 kg | 0.10 m | 0.010725 m3 | 4 | 2042.6 N s |

All four report zero kinetic cavity energy and zero cracks. Initial off-solid measurements
used a doorway point; the final fixture uses the sandbox's actual aimed solid face.

## Inventory dispositions

- D9-D10 fixed ("buildings: author surface appearance and grain in data"): declared planks visual, palette lanes and repeats; required
  construction grain frames; explicit graybox face lanes/top tangent; no Wood-key or
  orientation fallback. Building asset format 5 was rebaked canonically. MEASURED topology
  remains 31 elements, 1,163 chunks, 3,185 joints and 81 anchors. ARM supplies metallic response.
- D12 and the retired Fir alternative fixed ("scatter: remove the retired graybox fir path"): delete variant, constants,
  collider/view arms, obsolete arguments and the ignored comparison bed. Shipped trees remain.
- D11 left as instructed: terrain-pack schema/loader work is not a data-only fix.
- The three original P3s remain uncommitted: waiting presentation cursor before synchronization,
  checked replica segment conversion and typed tree-word errors. Their implementation passed
  CI clippy, but its targeted test build did not finish within the clock window.
- Two additional review P2s remain uncommitted: repeated traversal of a convex profile boundary,
  and a bound tree template with no standing meshes. Guards and regressions were prepared.

## Verification and handoff

All seven implementation commits are green. Formatting and CI clippy passed before each
batch. The largest completed pass: 10 canonical bake tests plus 395 relevant game tests,
including pen_cam, field_casts, headless_test, doc_citations, ui_ascii, net_boundary and bake.
Ignored measurement beds were not run. The final follow-up passed CI clippy but its test
compile was stopped at the clock cutoff, so none of that unverified source was committed.

The pending follow-up patch was applied, tested and committed by part 3 (below): its repeated-
vertex refusal and replica P3s as written, its empty-standing-view check replaced by a stronger
per-binding one. The owner still needs a follow-up for mixed load sharing and continued fracture
of loose groups; no arbitrary group-size cap or per-object exception was introduced.
No wire representation changed; PROTOCOL_REV was deliberately unchanged.

## Part 3: review verification and follow-up

Subject: the game's environment destruction, game state only. Two blind reviews of part 2 (one
Codex, one Claude) were each decided against the code before any fix; the per-finding verdicts
went to the orchestrator. Commits, by subject:

- "buildings: refuse a profile boundary traced twice" (both reviewers' P2). With distinct
  vertices, every vertex inside every edge's half-plane admits only a simple convex boundary: a
  second winding must step over a strict hull corner, which leaves that corner outside the edge.
- "trees: refuse a part binding that draws no mesh" (both). The patch's empty-merged-view check
  passed a template missing one trunk's mesh alone; every trunk and cap binding must now be some
  mesh's nearest binding.
- "net: hold the replica's wall view until its clock syncs" (the three replica P3s, from the patch).
- "buildings: make a graybox top tangent one of the cube's edges" (Codex P2): `BoxAxis`, X or Z.
- "sandbox: cut the game's holes and share its block adapter" (Codex P2, Claude P3): the sandbox
  now reaches HOLES_PER_CHUNK, and runs `fracture::sample_blocks`, the adapter its tests exercise.
- "buildings: pin the roof gate and the loose-prop law" (Claude P2, two P3s): the roof gate asserts
  that the plane fixed to the plate that lost its wall leaves whole and that no touched chunk stands
  unassessed; a fixture arm whose only prop is itself loose now falls (it stands with `grounded`
  all true); the two push reporters are measurement beds, run on demand.
- "buildings: run a level face along its element" (Claude P3): an Upright element's level face
  takes its tangent from the element's run `u` (or depth `w` where `u` crosses the face at over
  45°), not world X; the spliced view-test doc is repaired.
- "buildings: gate every shipped building material's palette lane" (Claude P3, part): a test over
  the shipped assets. Left: visuals.ron is still parsed twice (the tank surface's library and the
  building palette's), and `tank_surface::Visual` still carries the building lane; folding the
  palette into the client's one visual library is a view refactor with no behaviour change.
- "assets: drop the orphan wood visual and the retired forest comparison" (Claude P2, two P3s):
  the forest cost script compared trees against the deleted cones and failed every trees row once
  the scatter line lost its fir count; it and its battery parts are deleted. Stale prose that named
  deleted constants or the cones is rewritten.
- "buildings: key blast hits by their stop instead of a chunk field" (Claude P3): a body stop's
  placeholder `chunk: 0` is gone; `Load::blast` takes each hit keyed by the chunk it stopped on.

Not fixed, P3: tree bindings are still refused when a client reads the glb, not by a
shipped-asset gate: reading a glb outside the asset server is a separate tool.

### Open: continued fracture and mixed load sharing

**MEASURED** on the shipped house after the roof gate's removals: the plane nailed at its eave to
the standing wall's plate, its ridge free, stays (74 of 150 roof chunks; one released group of 80, the east plane and its plate), and the weight
check reports no touched chunk unassessed. Whether eave fixings alone hold a cantilevered plane is
the open law, not a tuning question.

The law both open items need is the same one: a group's joints share its loads by stiffness, not
one cut set at a time. For a static group held through several sections, rests or cracked seams
(a span, a propped arm, the eave-held plane), equilibrium alone leaves the joint forces
indeterminate; a joint graph solved by the stiffness method (each joint a spring of its contact
area over its seam thickness, scaled by the material's elastic modulus) fixes them, and each
joint's traction then meets its own strength as now. For a loose group, the same solve runs in
the group's own accelerating frame (d'Alembert): the body force per unit mass is `g − a − α × r −
ω × (ω × r)` and its contacts' impulses are loads at their points, so uniform free fall loads
nothing (DERIVED) while a landing or a spin loads seams, and an overloaded seam splits the body.
Obstacles: the substances carry no elastic modulus yet (a material truth to add, with
provenance); a split must spawn new bodies from the group's chunk data at the split tick and
replicate the split as a record event (the wire changes, PROTOCOL_REV bumps); and the contact
impulses a loose body receives are read in `src/contact_transfer.rs`, outside this lane's files.
That is a lane of its own.

D11 (the terrain packs' identities, repeat sizes and ground types in code) stays open: moving
them into pack data changes the terrain mesh's UV scale source, the blend shader's uniforms and
the print and dust readers of `terrain_blend`, which the driving lane's prints share.
