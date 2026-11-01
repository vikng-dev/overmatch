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

## Part 5: blind reviews of part 4

Subject: the game's building destruction (fracture, joints, chunk groups and their structure law),
game state only. Two blind reviews of part 4 (one Codex, one Claude) were each decided against the
code on main before any fix. The lane started at 22:28:23 +07; no captures, pricing runs, pushes or
subagents.

| Reviewer | Finding | Verdict | Fixed in (subject) |
|---|---|---|---|
| Codex | P1 `WallPiece` wire change without a repinned type hash | verified (part-4 snapshot) | net: pin the merged wire types at PROTOCOL_REV 60 (since re-pinned by "net: pin the merged wire types at PROTOCOL_REV 62") |
| Codex | P2 contact roots skip assessment uncounted | verified | buildings: root a moving group's sections at one reference chunk (#249) |
| Codex | P2 inertial blows seed the whole group as its only ring | verified | buildings: grow punch rings from external loads only (#249) |
| Codex | P2 an arrived child shows beside its still-shown parent | verified | buildings: draw each split chunk by one piece and index the handover (#249) |
| Codex | P2 moving groups meet phantom unbreakable elements | verified | buildings: keep unbreakable elements with the placed building (#249) |
| Codex | P2 collinear overlapping profile edges pass | verified | buildings: refuse a profile edge that touches or retraces another (#249) |
| Claude | P2 a loaded loose group's energy terms lose its weight | verified | buildings: read a moving group's own weight in its energy laws (#249) |
| Claude | P2 belt loads on a moving chunk group never reach its laws | verified | open (below) |
| Claude | P3 per-frame handover scan over every arrived piece | verified (DERIVED cost) | buildings: draw each split chunk by one piece and index the handover (#249) |
| Claude | P3 field blows on chunks turned to dust move a moving group | verified | buildings: move a moving group only by field blows on chunks that stand (#249) |
| Claude | P3 a group freed in the same run is never framed | verified | buildings: assess a group a blow frees in its own frame in the same run (#249) |
| Claude | P3 no centripetal term in the group frame | verified | buildings: load a spinning group's seams by its centripetal pull (#249) |
| Claude | P3 a child drawn twice with its parent | verified (Codex's P2) | buildings: draw each split chunk by one piece and index the handover (#249) |
| Claude | P3 per-piece state clones and per-source frame loads | not a defect: a DERIVED cost with no wrong state; the pricing lane measures it | — |
| Claude | P3 `retire_wall_piece` skips a missing carrier silently | verified | net: expect a chunk group's carrier when its split retires it (#249) |
| Claude | P3 `Origin::Contact::normal` read by no law | verified | buildings: drop the contact normal no law reads (#249) |
| Claude | P3 the free-fall test is vacuous | verified | buildings: make the free-fall test read the frame's gravity cancel (#249) |
| Claude | P3 no test of a level face's element axes on the mesh path | verified | buildings: test a level face's element axes on the mesh path (#249) |
| Claude | P3 house-specific assertions in the roof probe | verified | buildings: keep only class invariants in the roof recheck (#249) |
| Claude | P3 the gentle drop passes by the multi-root limit | verified | buildings: root a moving group's sections at one reference chunk (#249) |
| Claude | P3 stale rest-law docs | verified | buildings: read a moving group's own weight in its energy laws (#249) |
| Claude | P3 stale breakage module doc; `RetirePiece` holds `PendingSplit`'s doc | verified | buildings: state moving groups and splits in the breakage docs (#249) |
| Claude | P3 "protocol revision 59" in the records | verified | sweep-2: docs: name the merged protocol revision in the destruction record; `wall-destruction.md`: docs: record the sweep-destr-4 review verification (#249) |

Each verified law finding has a law fixture that fails on the old code (ASSUMED geometry and
seams, labelled in each test): an exact two-chunk pull the old ring seeding held; a hard end strike
on a slab touched along its length the old multi-root graph left whole; a rest on a lying group
any blow opened; an anchored row whose freed part never split; a spinning slab whose weak seam
held at any spin. The real Avian drop pair still discriminates with every section of the flat
set-down now assessed.

**Verified here, fixed in part 6:** a belt's loads on a moving chunk group. The belt sized and
applied a dynamic recipient's clamped impulse (`track::rubble::piece_impulses`) but staged a
structure reaction (`record_static_contacts`) only for fixed construction, so a tank driving over
a fallen slab moved it and its laws never saw the load.

**New, not fixed:** the section law takes a bending axis from any nonzero moment, so an axial pull
whose line meets the section centre bends it by f32 rounding alone. MEASURED on the row fixture
(chunk spacing 0.2 m, a 1000 N·s axial contact): a rounding-only axis gave a section demand of
56.4 N·s against a capacity of 0.21; on binary-exact coordinates the same pull bends nothing. The
punch ring now reads the pull either way, but the section's reading depends on rounding. Also,
`bears_on_contact` still reads "rising" as construction-frame `y` on a moving group, where the
group's field gives the direction.

## Part 6: what a tank delivers to what it pushes (#256)

Subject: the game's driving and building destruction models, the loads a tank's hull and belt put
on walls and fallen pieces; game state only. Each finding was decided against the code before any
fix. The lane started at 01:22:27 +07; no captures, pricing runs, pushes or subagents.

| Reviewer | Finding | Verdict | Fixed in (subject) |
|---|---|---|---|
| Codex (sweep-drive-7) | P2 `belt_held` is a per-body rule: two identical co-moving receivers take the full normal reaction or none by what they are | verified: `apply_track_forces` passed the driving tanks of its `With<Tank>` query as `belt_held`, and `piece_impulses` chose the whole reaction by membership alone | track: size every belt recipient by its mass and the belt's spring (#256) |
| Claude (server-perf-2) | P3 `piece_impulses`' "the same lexicographic geometry key" comment describes the edit, not the code | verified | track: size every belt recipient by its mass and the belt's spring (#256) |
| Claude (sweep-destr-5) | a belt's loads on a moving chunk group never reach the group's laws (part 5) | verified: `record_static_contacts` skipped every dynamic recipient, and nothing else staged one | track: stage the belt's impulse on a moving structure as its contact (#256) |

**One reaction law for every belt recipient.** The belt's support is an explicit penalty spring
read at the tick-start pose. A dynamic body takes the whole opposite reaction where the explicit
step integrates the belt's springs against it stably, by its own construction: over all its patches
from every source this tick, `dt² Σ kᵢ / mᵢ < 4`, `kᵢ` each patch's spring rate (DERIVED from the
support law: the per-metre spring over the patch's contacting length), `mᵢ` the body's effective
mass along the patch's normal. RESOLUTION: 4 is symplectic Euler's undamped limit (DERIVED), read
against the trace bound, which is at least the stiffest rigid mode's `k dt² / m`. Any lighter body is a piece and keeps the
clamped sequential impulse, so Avian's implicit solver holds it. Nothing names the body's kind: a
hull under another tank's belt and a body of the same collider, mass and inertia without running
gear take the same momentum, −9414.24 N·s under the parked fixture stack (MEASURED; fixture 30 t
boxes, `dt² Σk/m` 0.146).

**A moving structure under a belt.** The impulse a moving structure (a loose chunk group) took from
a belt is staged as an implicit contact on it, named with its source, through the reading fixed
construction's reaction joins (`RubbleBodies::record_moving_contacts`,
`ContactTransfers::stage_on_moving`); breakage reads it as the group's `Origin::Contact`. Its blow
is the pair's plastic exchange: the source's and the structure's inverse effective masses for the
closing patches' distribution summed, as the solver's constraint mass is for a solver contact.
MEASURED (fixture: a 14.4 kg source closing at 1 m/s on a free slab of the same mass): blow
6.05 N·s against the reduced-mass bound 7.2 N·s, the structure's reading equal to the 15.34 N·s
the slab took.

**Open: the hull's work and the contact's duration (#256).** Not changed here. `Load::mean_force`
still divides a body contact's blow by the solver step, and a section's energy limit still reads
only the freed group's own energy, so a slow ram's verdict still depends on the physics rate and
on the non-monotone gap in "Bodies in contact". The law to land: a body contact's force comes from
the pushing body's mass and closing speed and the pair's contact duration, `τ = π √(m / k)` (DERIVED:
a linear contact spring's half period) with `k`
the pair's contact stiffness in the material data (labelled `Unknown` until a source settles it),
and the pushing body's work enters the energy limit beside the freed group's own. It needs
`breakage::contact_load` to state the duration in `Origin::{Body, Contact}` instead of
`ContactTransfers::dt`, and the section, ring and rest laws to read the pair's work; both were
reserved for lane sweep-destr-6 this round. The wall design's "Body contact duration" note stays
until that law lands.

### Review verification: hull work continuation (#256)

Subject: the game's driving and building destruction models; game state only. Both review files
were present. Findings are checked against code and executable class fixtures.

| Reviewer | Finding | Verified / not a defect | Fixed in (subject) |
|---|---|---|---|
| Codex P1; Claude P1 | Reaction stability ignores damping | verified: fixture 500 kg, 2 MN/m, 100 kN s/m, 1 cm press, closing 1 m/s at 64 Hz leaves at -2.7500002 m/s (MEASURED red test) | track: include damping in recipient stability (#256) |

The spring/damper verdict includes `dt² k/m + 2 dt c/m` over all recipient patches; its
RESOLUTION limit is 4 (DERIVED from the symplectic Euler amplification matrix).
