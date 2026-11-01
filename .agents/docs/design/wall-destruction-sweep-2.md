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
- The three original replica P3s were committed in part 3 ("net: hold the replica's wall view until its clock syncs"): waiting presentation cursor, checked segment conversion and typed tree-word errors.
- The two additional review P2s were committed in part 3 ("buildings: refuse a profile boundary traced twice" and "trees: refuse a part binding that draws no mesh"). The first guard still admitted a dense star inside the plane tolerance; part 4 verifies and fixes that distinct-vertex counterexample.

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

- "buildings: refuse a profile boundary traced twice" (both reviewers' P2). Repeated vertices are refused. The half-plane test still admitted a distinct-vertex dense
  star whose crossings fit inside its distance tolerance; part 4 adds the independent
  topological crossing refusal.
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
The material rows already carry Young's modulus in `bulk.blast.youngs_modulus`, parsed by
`substances` and used by `Spall::of`. A stiffness solve instead lacks joint thickness and the
attachment stiffness of an `OpeningOnly` fixing: neither `Joint` nor `Seam` states them. They
would need sourced construction facts; another material modulus would duplicate existing data.
A split must spawn new bodies from the group's chunk data at the split tick and
replicate the split as a record event (the wire changes, PROTOCOL_REV bumps); and the contact
impulses a loose body receives are read in `src/contact_transfer.rs`, outside this lane's files.
That is a lane of its own.

D11 (the terrain packs' identities, repeat sizes and ground types in code) stays open: moving
them into pack data changes the terrain mesh's UV scale source, the blend shader's uniforms and
the print and dust readers of `terrain_blend`, which the driving lane's prints share.


## Part 4: blind reviews and moving groups

| Reviewer | Finding | Verdict | Fixed in (subject) |
|---|---|---|---|
| Codex | P2 dense distinct-vertex winding inside plane tolerance | verified | buildings: refuse crossing profiles inside the plane tolerance (#249) |
| Codex | P3 retired forest runner still named | verified | buildings: test generic face frames and replica presentation (#249) |
| Claude | P2 wrong missing stiffness truth | verified | docs: state the actual stiffness data gap (#249) |
| Claude | P3 forest references and claimed tracked start-lever user | verified | buildings: test generic face frames and replica presentation (#249) |
| Claude | P3 retired wood visual still listed as shipped | verified | buildings: test generic face frames and replica presentation (#249) |
| Claude | P3 inventory says committed work remains uncommitted | verified | docs: state the actual stiffness data gap (#249) |
| Claude | P3 waiting-clock accessor test misses actual presentation | verified | buildings: test generic face frames and replica presentation (#249) |
| Claude | P3 surface-frame assertion assumes a horizontal run | verified | buildings: test generic face frames and replica presentation (#249) |

There are eight findings, including the two overlapping forest reports; none was rejected.
The Codex dense-star regression was run red: the load gate accepted the ASSUMED unit-radius,
501-vertex step-two boundary. An independent topological crossing test now refuses it.
Claude's stiffness claim was checked directly against the parsed material data and `Spall::of`:
Young's modulus already exists; joint thickness and attachment stiffness are the missing data.
The old presentation test cannot distinguish the review's `map_or(true, ...)` mutant. The new
view test feeds an entry to `present_record` while Waiting, asserts it is held, then releases
it at a synchronized cursor. The surface test now covers ASSUMED tilted/vertical runs and
azimuths while asserting a unit, in-plane construction tangent. The start lever retains its
existing operator, the owner's hand-driving of terrain-print spots; its deleted script is no
longer presented as a current user.

### The eave state

The shipped-house fixture removes the wall beneath one plate and then both gables, exactly as
part 3 did. **MEASURED:** the opposite plane and its plate leave as a group of 80 chunks; the
other 74 roof chunks remain. Their only intact external boundary is **MEASURED eight nailed
eave patches**, totalling **DERIVED 0.075 m2** from the baked joint areas. No gable rest or
ridge joint has a live other side. **MEASURED seventeen already-open fixing patches** still
have closed compression contact with the grounded plate. Those are legitimate contacts;
the authored bond kinds and areas do not need a data change.

A controlled counterfactual removes only those opened patches' compression credit, preserving
all chunks and the eight intact nails. **MEASURED:** the same section then fails at
**2,532,788.5 Pa demand versus 3,240 Pa tensile strength**. Eave nailing alone does not hold it.
The visible survivor is the existing mixed bonded/contact limit, entirely at the eave, not a
hidden gable rest. The stiffness solve stays held. The full-plane recheck previously reported
zero unassessed chunks because a successful enclosing wall check overwrote the local deferral;
the regression was run red and that count is corrected by retaining local deferrals.

### Model and validation

The model's narrative lives in `wall-destruction.md`, "One assessment path for fixed and moving
chunk groups". The implementation uses shared construction data and the same law pipeline;
actual physics contacts become transient resting joints, their already-transferred impulse is
not exchanged twice. A split retains the parent's actual rigid velocity field and replicates
birth/retirement ticks on the same presentation cursor (protocol revision 60, shared with the module repair snapshot).

The baseline two-structure contact regression was run red: the reader panicked because it
assumed every structural body was static. The existing pure slab fixtures already discriminated
strong/gentle input, free fall and single chunks; the missing runtime path is tested with real
Avian drops. New frame-wrench and rest-direction fixtures guard the accelerating-frame terms.

**Validation:** formatting and CI clippy passed. The targeted run passed **MEASURED 209 tests**,
with four existing measurement beds ignored, including all standing-wall fixtures, the real
strong/gentle drop pair, exact collider momentum inheritance, two-sided contacts, delayed split
presentation, the wire pin, world reset, headless collider checks, and the whole-tree
`doc_citations`, `ui_ascii`, `net_boundary` and `bake` guards.

The first real drop stayed whole. Its first nonzero reactions were on the end chunk, while
zero-reaction speculative points were also being used as reference supports. Removing those
zero load references fixed the unchanged drop fixture; the dedicated zero-reaction regression
pins that distinction. Neither seam characterization nor drop geometry was changed to make it
pass. The split motion test verifies mass, linear/angular momentum and rigid kinetic energy.

Implementation commit: "buildings: assess and split every chunk group in its own frame (#249)".
The eave's visible mixed-support error remains the held stiffness idea; no modulus, thickness,
material strength, attachment area or construction bond was tuned for it. Pricing is separate.

**Capture handoff:** the production binary built and two hidden, silent, locked house-breach
attempts completed. Neither supplied the requested collapse with landing splits. The final
normal-driving scenario remained queued behind another lane's machine lock at the measured
90-minute cutoff (start 18:56:06 +07; cutoff 20:26:06) and was cancelled. Item 3 remains open;
`caps-destr4/index.md` in the lane scratchpad labels the retained attempt images and the unrun
plan honestly. No pricing or visible-window run was performed.
