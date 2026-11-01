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

**One reaction law for every belt recipient.** The current stability verdict is
`track::rubble::explicit_recipients`: the trace of each connected spring/damper component,
including its source and recipient inverse responses. Each patch contributes
`(dt² k + 2 dt c) (1/m_source + 1/m_receiver)` (DERIVED, effective rigid masses at the patch).
The RESOLUTION limit is 4, from the symplectic Euler amplification matrix. A component below
it takes the whole opposite reaction; other recipients keep the sequential clamp. Fixed
sources prescribe their surface velocity and couple no recipients together. No body-kind
membership selects a reaction. The equal-collider, equal-inertia driving fixture remains green;
its baseline parked reaction was -9414.24 N s (MEASURED).

**A moving structure under a belt.** The impulse a moving structure (a loose chunk group) took from
a belt is staged as an implicit contact on it, named with its source, through the reading fixed
construction's reaction joins (`RubbleBodies::record_moving_contacts`,
`ContactTransfers::stage_on_moving`); breakage reads it as the group's `Origin::Contact`. Its blow
is the pair's plastic exchange: the source's and the structure's inverse effective masses for the
closing patches' distribution summed, as the solver's constraint mass is for a solver contact.
MEASURED (fixture: a 14.4 kg source closing at 1 m/s on a free slab of the same mass): blow
6.05 N·s against the reduced-mass bound 7.2 N·s, the structure's reading equal to the 15.34 N·s
the slab took.

**Open: physical contact duration (#256).** The pushing-work term is implemented below.
The contact force still uses the numerical solver interval: `Load::mean_force` divides the blow
by `Origin::{Body, Contact}::step`. The remaining law is `tau = pi sqrt(m_pair / k_pair)`
(DERIVED: the pair spring's half period), with the pair's contact stiffness in material data,
labelled `Unknown` until a force-indentation source settles it. No duration data or constant is
introduced by this continuation. The wall design's "Body contact duration" note remains until
that adapter and its genuine physics-clock invariance test land.

### Review verification: hull work continuation (#256)

Subject: the game's driving and building destruction models; game state only. Both review files
were present. Findings are checked against code and executable class fixtures.

| Reviewer | Finding | Verified / not a defect | Fixed in (subject) |
|---|---|---|---|
| Codex P1; Claude P1 | Reaction stability ignores damping | verified: fixture 500 kg, 2 MN/m, 100 kN s/m, 1 cm press, closing 1 m/s at 64 Hz leaves at -2.7500002 m/s (MEASURED red test) | track: include damping in recipient stability (#256) |
| Codex P1; Claude P3 | Later patches record velocities changed by earlier impulses | verified: two 1000 kg bodies, 800 N s reaction at their COM normal, closing 1 m/s; one patch reads 499.99997 N s, two read 399.99997 N s (MEASURED red test) | track: retain tick-start closing speeds for every reaction (#256) |
| Codex P2; Claude P2 | Reported elastic stiffness omits engagement derivative | verified: uniform 1 cm press in a 2 cm ramp reports 1,999,999.9 N/m against a finite-difference force derivative of 3,999,707.3 N/m (MEASURED red test) | track: include the engagement slope in patch stiffness (#256) |
| Claude P2 | The explicit trace omits the source's inverse response | verified: DERIVED fixture, two 1000 kg bodies and a 10 MN/m spring at 64 Hz, recipient-only rate 2.4414 versus pair rate 4.8828; the omitted-source reaction leaves -2.125 m/s, corrected test leaves 0 m/s (MEASURED) | track: bound coupled belt reactions with both bodies inertia (#256) |

| Claude P3 | A missing receiver-map entry can silently stand for a moving recipient | verified: staging a dynamic recipient through the fixed entry point did not fail (MEASURED red contract test); the reader now checks the actual rigid-body state before publishing | contact: enforce the implicit receiver inertia contract (#256) |
| Claude P3 | Module documentation describes only fixed source-mass budgeting | verified: moving contacts use their actual received impulse and the pair mass | contact: enforce the implicit receiver inertia contract (#256) |

The reaction law and its current bound are stated above; regression fixtures exercise the
damper, the ramp derivative, tick-start closing readings and the coupled source response.

**Continuation verification gates (before the hull-law remainder).** Formatting and CI clippy
passed. Contact-reader/driving/citation selection: 230 passed. Destruction, headless scenarios,
whole-tree guards and wire pins: 205 passed, 4 existing ignored measurement tests. The focused
review evidence selection passed all 5 tests. PROTOCOL_REV remains 63: no serialized type changed,
and both wire pin tests pass. No captures, pricing runs, pushes or subagents were started here.

**Shifted readings (MEASURED software-state fixtures).** The moving slab's blow changes from the
baseline 6.05 N s to 6.9103637 N s (about +0.86 N s, +14.2%); its received impulse remains
15.339572 N s and its reduced-mass bound remains 7.2000003 N s. The change is the corrected
pre-reaction closing reading, not an extra reaction. The parked equal receivers retain their
9414.24 N s normal momentum. The ramp reports 3999997.8 N/m against the finite-difference
3999707.3 N/m; the one- and two-patch blows both read 499.99997 N s.

### Pushing work in the energy limit (#256)

Subject: the game's building destruction model; game state only. Fixed construction holds a
source until its group opens. For source effective mass `m`, receiver modal mass `M`, incoming
blow `J`, and plastic momentum share `s = M/(m+M)`, the freed receiver's kinetic term is
`J² M/[2(m+M)²]`; the pushing source contributes `J² m/[2(m+M)²]`. Their sum is
`J²/[2(m+M)]` (DERIVED). Thus an energy check against `sqrt(2 M W)` reads `J sqrt(s)`, not
`J s`. `Origin::work_share` supplies this factor to the section, bearing-rest and punch-ring
energy checks. Hinge checks use the hinge's modal mass; sliding/ring checks use their
translation mass. Fields and actual contacts already integrated by the solver retain their
whole impulse. Launch and give-back still use `Origin::share`, the momentum share; the work
factor never sizes a launch.

**Reproduction and class evidence.** The generic four-seam plug fixture has a 50 t source
closing at 0.2 m/s and a 28.8 kg receiver (FIXTURE). DERIVED from those inputs and the fixture
material: receiver term 0.575337 J, pair work 999.4243 J, required work 7.1 J. The old energy
reading demands 5.7566843 N s against 20.22276 N s and fails the new slow-ram test. The corrected
reading demands approximately 239.93 N s and opens the ring. This fixture deliberately passes
the force gate under the current numerical bound; it isolates the missing work, and does not
discharge the physical-duration remainder above. The hinge mass sweep and both-limit fixtures
now pin pair work while preserving the independent strength requirement.

Fixed in: **fracture: count pushing work in contact energy (#256)**.

**Work-term gates.** The selected destruction, driving, contact-reader, headless, whole-tree
and wire-pin run passed 433 tests, with 4 existing ignored measurement tests; CI clippy and
formatting passed. Physical contact duration, material stiffness authoring and load-gate wiring,
the real physics-clock invariance test, and retirement of the wall design's duration note remain.
PROTOCOL_REV stays 63 because no wire type changed. The old shipped-house ram measurements in
the wall design are explicitly labelled as the receiver-only-energy baseline; no capture or
new shipped-house measurement is claimed.

**Green stop at the lane limit.** Continuation began at 03:55:55 +07 and reached its final green
checks at 04:55:33 +07 (MEASURED wall-clock, 59 minutes 38 seconds). The final comment cleanup
also passed 125 fracture/citation/whole-tree/wire tests and CI clippy. All review findings and
the pushing-work term are committed separately. The duration remainder above is deliberately
unimplemented; the approximation note is retained because the code still uses the solver step.

## Part 7: sweep-destr-5 verification and the section law (#249)

Subject: the game's building destruction and damage model, game state only.
The two sweep-destr-5 reviews were checked against their inputs and the current laws.
The earlier green implementation steps landed on main; this appendix extends their record.

| Reviewer | Finding | Verified / not a defect | Fixed in (subject) |
|---|---|---|---|
| Codex | P2 later structural cuts use the former parent's inertia | verified | buildings: reframe each group before the next seam failure (#249) |
| Codex | P2 opposed axial loads seed only their whole connected union | verified | buildings: assess opposed loads in separate punch patches (#249) |
| Codex | P2 numeric birth ordering reverses ancestry at tick wrap | verified | buildings: order split presentation across tick wrap (#249) |
| Claude | P2 rounded free-fall field supplies a spurious bearing direction | verified | buildings: cancel free-fall forces before reading their direction (#249); buildings: cancel paired free-fall wrenches before reading their moment (#249) |
| Claude | P3 a detached group's cracked ground anchor remains a bearing contact | verified | buildings: discard detached anchors as bearing contacts (#249) |
| Claude | P3 a moving group loses its frame when dust consumes all external sources | verified | buildings: frame every unsupported group after source removal (#249) |
| Claude | P3 the outer frame loop repeats until no failure, despite its stated stopping condition | verified | buildings: reframe each group before the next seam failure (#249) |
| Claude | P3 belt loads on dynamic recipients still do not reach the structure laws | verified | track: stage the belt's impulse on a moving structure as its contact (#256), landed on main; fenced code unchanged here |
| Record, new | an axial pull chooses a bending axis from roundoff | verified | buildings: resolve bending axes above input roundoff (#249) |
| Record, new | bearing rise is construction Y rather than the group's field | verified | buildings: read bearing rise against the group field (#249) |

Every owned counterexample was run red: the split presentation across tick wrap; a binary-exact
pair under equal opposed contacts; both signs of the averaged-gravity residual; a rest surviving
after dust consumes the only external source; a detached anchor; an X-directed field with Y- and
X-normal rests; an axial pull at three fixture scales; and a cheap end seam detaching before a
gently loaded far seam. The latter covers both impulse and sustained-force assessment. The
extra-pass finding follows directly from the old outer loop's failure-empty condition; its fix
shares the current-partition framing change. The belt finding was verified against the former
dynamic-recipient skip; main now carries its assigned lane's fix, cited above.

**Resolution of a bending axis.** DERIVED from input magnitudes and arithmetic counts:
`gamma(k) = k*u/(1-k*u)`, `u = f32 EPSILON/2`, bounds centroid accumulation/division, normal
summation/normalization including its conditioning, and point/cross/projection and load summation.
Absolute magnitudes are summed before cancellation. A moment within that bound has no resolved
axis; no finite certified bound also means no resolved axis. A resolved eccentricity still bends.
MEASURED on the isolated old section fixture: 112.83792 N s m demand against 0.20999762 N s m
capacity from coordinate rounding alone. `Cause::Push` is angular impulse, whose unit includes the
lever's metre; this is a separate isolated reading from part 5's framed-row measurement.

**Current group frames.** `GroupLoads` accepts external sources and spin; it derives inertia from
the current partition. Both strength loops refresh it after a membership change, before another
section, ring or rest is assessed. Their stopping condition is no remaining failure. The cross-module gate exposed a tied-load
reference chosen by chunk index: reversing the chunk table changed the post-split outcome.
Construction-centroid order now breaks that tie, and the original wall renumbering assertion
remains unchanged. Gravity and
frame forces use matching rounded products before division. Gravity and applied wrenches are
accumulated separately so opposite point terms follow matching arithmetic on long groups too.
The added free-falling row regression checks exact zero force and moment for 100 fixture chunks;
MEASURED red test on the old sequential cancellation: zero force and a Z moment of
-2.910383e-11 N m.

**Test time.** Two shielding tests repeatedly drove the full house's fracture search, although
their invariant needs only a crushable surface and a blocker. A one-cube fixture goes through the
real asset parser and resolver. Exact ray impulse, body-layer selection, bare wear, shaded
non-wear and the inside-source control remain asserted. Complete shading legitimately leaves no
live fracture state. The immutable shipped-house fixture resolves once per process; loader
refusal tests still parse their own inputs. The real Avian strong/gentle landing pair remains.
MEASURED serial baseline from the preceding bay: `buildings::` 245.26 s, 136 passed and one ignored;
`a_ray_that_stops_on_a_moving_body_pushes_it_by_its_share_and_deposits_nothing` carried
200.838 s, and `the_ground_stops_a_blast_and_a_body_holding_the_burst_shades_nothing` carried
40.343 s. The final-tree result is
5.02 s (145 passed, 2 existing ignored), one test thread, under the machine lock; compilation and queue waits are
excluded by the test harness timer. The implementation is
`buildings: use minimal shielding fixtures and share immutable layouts (#249)`.

Every implementation step passed formatting, CI clippy, its targeted tests and doc citation
checks. Both resumed law steps passed `buildings::`, `trees::`, `debris::`, `sandbox::`,
`headless_test` and `doc_citations`: respectively 229 and 230 passed, 7 existing ignored.
The final record passed formatting, CI clippy and the whole-tree `doc_citations`, `ui_ascii`,
`net_boundary` and `bake::` guards, with headless checks again on this checkpoint tree.
No captures, pricing runs, pushes or subagents. Fenced paths and `Load::mean_force` are unchanged.
All findings from the sweep-destr-5 reviews above are verified and fixed. The sweep-destr-6
review double is the next continuation; this checkpoint makes no disposition of its new findings.

## Part 8: sweep-destr-6 double verification (#249)

Subject: the game's building destruction and damage model, game state only. Both review files
were present at the start and were read again before this table. C1–C3 and L1–L8 follow the
Codex and Claude reports' order. Every listed finding is verified and fixed.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Codex C1, P2 | Later cuts retain the former parent's inertia | verified: the saved split regression fails on the old code; the current impulse and sustained-force cases detach the hard end while keeping the gently loaded far seam | buildings: reframe each group before the next seam failure (#249) |
| Codex C2, P2 | Co-directional unequal loads merge away local axial separation | verified: the new pair regression remains whole on the old code; local candidates separate the unequal pair while equal-load controls stay joined | buildings: assess each loaded chunk before merging punch patches (#249) |
| Codex C3, P3 | Verification dispositions and measured test-time comparison are missing | verified: the base record ended at Part 6; Part 7 and this continuation supply the missing tables, subjects and measurements | docs: record the sweep-destr-5 verification and section-law gates (#249); docs: record the sweep-destr-6 double verification (#249) |
| Claude L1, P2 | Sequential free-fall cancellation leaves a rigid moment | verified: Part 7's long-row red test has zero force but a nonzero moment; paired accumulation gives exact zero for both | buildings: cancel paired free-fall wrenches before reading their moment (#249) |
| Claude L2, P2 | Structural and sustained-force loops read a split-off in its former frame | verified: C1's counterexample; both assessment loops now derive inertia from the current partition before another failure | buildings: reframe each group before the next seam failure (#249) |
| Claude L3, P3 | The outer frame pass repeats after a crack that frees nothing | verified by its failure-empty stopping condition; the redundant outer pass is removed, with reframing inside the assessment loops | buildings: reframe each group before the next seam failure (#249) |
| Claude L4, P3 | Absolute coordinates enlarge the bending-axis resolution band | verified: translating the construction origin erases an otherwise resolved axis on the old code; local coordinates retain exactly the same hinge demand | buildings: resolve section bending in local coordinates (#249) |
| Claude L5, P3 | An uncertifiable arithmetic bound silently becomes no axis | verified: the boundary regression does not panic on the old helper; the finite-bound invariant now fails in every build | buildings: refuse uncertifiable section roundoff bounds (#249) |
| Claude L6, P3 | The pushed-chunk helper keeps an unused predicate alternative | verified: its sole caller always passes true; the predicate and wrapper are removed, preserving membership and first-load order | buildings: remove the unused push-filter alternative (#249) |
| Claude L7, P3 | Complete shading lacks an open-ray witness | verified: the new partial-shadow control rejects the blanket-suppression mutant after proving some fixture rays are intercepted and others remain open | buildings: pin partial blast shading with a local blocker (#249) |
| Claude L8, P3 | The record leaves verified findings and test timing unreported | verified: C3's missing record; the checkpoint and final-tree measurements are distinguished below | docs: record the sweep-destr-5 verification and section-law gates (#249); docs: record the sweep-destr-6 double verification (#249) |

The unequal pair uses ASSUMED fixture contacts of -1000 and -1 N s through equal-mass chunks'
centres, with binary-exact geometry. Its relative impulses are -499.5 and +499.5 N s (DERIVED).
Both the impulse and sustained-force paths now read local punch patches; equal contacts retain
uniform translation. Coherent patches and their union remain candidates for cooperating loads.
The existing chunk-renumbering assertion passes unchanged.

The axis counterexample uses ASSUMED fixture data: 4096 repeated axial loads totalling 1000 N s,
a 1/1024 m eccentricity, and an 8 m origin translation. MEASURED before the fix: the translated
case returns no axis. After the fix both origins resolve and their demands are exactly equal.
Centroids, levers, the hinge and the demand now share a section-local origin. The count-dependent
summation allowance multiplies lever magnitudes; point subtraction has its own fixed-depth
allowance. This supersedes Part 7's absolute-position bound. An uncertifiable bound is now a
bug, never a reason for a section to hold: the direct boundary test reaches k*u = 1 at 2^24
rounded operations (DERIVED from f32 unit roundoff), without allocating that many loads.

Codex's checked-sound wrench claim covered the shorter fixtures. The longer row discriminates
the Claude finding, as Part 7's measured red result shows. The partial-shading test also checks
its geometry explicitly: the smaller blocker intercepts some fixture rays and leaves others
open. A temporary mutant discarded every chunk deposit whenever any ray stopped on a moving
body. It failed at the open-ray wear assertion; restoring the production function byte-for-byte
recovered the green result. No mutant or extra production branch remains.

MEASURED final-tree `buildings::`, one test thread under the shared machine lock:
31.69 s (148 passed, 2 existing ignored). The saved baseline is 245.26 s (136 passed, one ignored); Part 7's
checkpoint is 5.02 s (145 passed, two ignored), before the additional local punch candidates.
The final tree performs that additional candidate work and includes the new regressions.
All durations are the test harness's suite time, excluding compilation and queue waits.

Every implementation commit passed formatting, CI clippy and its targeted tests with
`dev_ui,bitprobe`. The law changes also ran `buildings::`, `trees::`, `debris::`, `sandbox::`,
`headless_test` and `doc_citations`; the final such selection passed 233 tests, with seven existing
ignored measurements. The final record passed formatting, CI clippy, the whole-tree
`doc_citations`, `ui_ascii`, `net_boundary` and `bake::` guards, and headless checks.
MEASURED final guard/headless selection: 61 passed, four existing ignored measurements.
All scoped work is complete. The track, contact-transfer and ballistics fences are unchanged.
No push, capture, pricing run or subagent was used.
