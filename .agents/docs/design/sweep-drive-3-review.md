# Driving sweep 3: review and disposition

Subject: the game's driving, tracks and environment destruction, game state only. For servo
capability and weapon-clock rows, the subject is the game's damage model: crew are HP pools at
stations. This record extends [part 2](sweep-drive-2-review.md), which extends
[part 1](sweep-drive-1-review.md); their design narrative and measurements are not repeated here.

Base: main at "buildings: refuse a peer wall entry the map cannot hold where it arrives", branch
`lane/sweep-drive-3`, bay-1. MEASURED start: 2026-11-01 12:32:09 +0700.
The lane used no agents, captures, pricing, deployments, external APIs or pushes. Builds and tests
used the shared queue, `dev_ui,bitprobe`, unattended/headless settings and the bay's warm target.

## Review of sweep-drive-2

The six commits were read against standards, their originating brief and all three lenses.
The review was performed directly: this dispatch prohibited subagents.

| Finding | Disposition |
|---|---|
| P2: `RubbleBodies::gather` interpreted any failed dynamic force query as an excluded tank, concealing a malformed admitted body | Fixed, "name support exclusions and require admitted body state": exclusions are explicit entity lists; admitted dynamic force state is required. A regression removes computed mass and requires a bug outcome. |
| P2: generic dynamic supports could lock axes, while the snapshot advanced unrestricted mass/inertia | Fixed, "retain locked axes in dynamic support reactions": snapshot and Avian impulse response use the same effective inertia. A locked direction at rest cannot acquire a reaction through infinity times zero. |
| P2: `WorldInstanceReady` was later than the available file gate, and vehicle detection by axle names let an export missing those names evade template refusal | Fixed, "refuse incomplete shoe templates at the view file gate": the selected view's template is checked at startup. "certify belt meshes and storage before publication" adds pin-domain and decoded-mesh certification before template publication. Loading waits remain distinct from malformed loaded data. |
| Cmd+Q decision in `Plugin::finish` | Agree. Local Bevy 0.19 source shows `App::finish` runs after plugin builds; `WinitPlugin::build` installs its settings. No native window was launched to exercise Cmd+Q. |
| `Stragglers::Park` disagreement | Agree with part 2. `track_sandbox::link_view::place_links` is a live caller for the count-tuning instrument. The inventory's test-only claim is stale. |
| G1 temporary measurement edits | Confirmed: `GRIP_SHEAR_MODULUS_M` is still `0.075` in both the base and this lane. No G1 tuning or feel choice was made. |
| P3: parent scale versus baked axle position | Fixed, "share baked axle positions and refuse scaled parents": use the baked rigid position and refuse scaled ancestors before publishing gear. |
| P3: scripted braking used inertia extents as a contact footprint | Fixed, "take scripted braking grade across declared contacts": sample grade over the lane projection of declared contact cylinders, including asymmetric stations and lateral spans. |
| P3: test-builder comment attached to the wrong fixture; stale claim that no shoe exports tangents | Fixed beside the related construction/template changes. |

No additional P1 was found in the part-2 diff. Its retained G1 and G5 findings were still open
at the base; their disposition is below.

## G5 and new findings

- **G5 static half fixed**, "deliver static belt reactions to structural load readings". Implicit belt reactions join `ContactTransfers` for the
  physics step that integrates the force. The existing wall and trunk readers consume that
  reading. Static support uses solid/layer-compatible geometry; `FieldSurface` excludes the
  heightfield and authored blocks already represented by the field. Cylinder support has its
  approximation bound beside `Surface::triangles`. Kinematic surfaces retain their motion.
  Class tests cover reaction conservation against source momentum, the actual wall-law queue,
  cylinder support, field exclusion, footprint subdivision and kinematic slip.
- **Other tanks remain explicitly excluded.** The game adapter's mutable hull query and mutable
  `Without<Tank>` recipient query enforce its current write ownership. Admitting another hull
  requires a tick-start snapshot of all recipients and staged writes before physics, while
  retaining the established driving/recoil/physics order. Deleting a filter alone is incorrect.
  This optional extension was deferred in favour of the dispatched admission/error groups.
- **New P2 fixed**, "certify full station positions before rig publication": `RigGeom::validate` now certifies every station's full position.
  A finite side-plane route did not establish a finite lateral coordinate for posing/projection.
- **New P2 fixed**, "certify belt meshes and storage before publication": template axes no longer select fallback directions; decoded
  shoes certify finite attributes/bounds, complete/indexable triangles and draw-index capacity.
  Allocator pages, retained materials and CPU backing buffers are required where their owning
  draw writes or releases them. Final review also requires the certified cache count when a live
  belt selects a rung, and finite selection distance for every admitted shoe. A malformed loaded asset cannot become a permanently empty belt.
- **New P2 fixed**, "preserve print identities and complete coverage counts": world-print coverage used byte counters even where an otherwise
  supported raster assigns more near texels to one world texel. Wider CPU counters preserve full
  coverage; GPU formats remain unchanged. Off-map lookup uses wide subtraction before conversion.
  CPU count storage per active world tile is wider; pricing remains the orchestrator's work.

## Error-handling inventory

| IDs | Status |
|---|---|
| R01, R03–R07 | Fixed, "require admitted servo limits and registered state": complete motor-domain admission; selected source/slot and source/view lifetime required; missing volume relationships are bugs. |
| R02 | Already fixed: `requires` is mandatory authored data. |
| R08–R10 | Open at part 3: clock exhaustion froze the gate; closed in [part 4](#part-4-blind-reviews-of-part-3-and-the-open-rows). |
| R11–R12, R15, R17–R18 | Already fixed by the preceding sweeps; their stronger admission/size checks remain. |
| R13–R14, R16 | Fixed, "enforce contact and calibration invariants in every build": finite positive calibration area; checked phase domain; shared wide material addressing; invalid contact loops cannot return a successful empty report. The validity flags and adapter skip branches are deleted. |
| R19–R25 | Part 2's unconditional faults retained, with earlier structural/file checks and decoded certification in this lane. |
| R26–R28 | Fixed, "certify belt meshes and storage before publication": certify before publication; no permanent refusal cache; file/live count bounds and checked draw allocation; retained write targets required. |
| R29–R30, R38–R42 | Already fixed by preceding work, retained. |
| R31–R36 | Open at part 3: tank-surface membership/material/scale and print-asset gates; closed in part 4. |
| R37 | Fixed, "preserve print identities and complete coverage counts": every stamp requires its registered shoe. Zero press, empty footprints and off-map texels are named absences. |

## Verification

MEASURED green batches, after their relevant fixes:

- Static support/contact/destruction/driving/geometry/headless: **542 passed, 7 ignored**.
- Parent frame: **206 passed, 6 ignored**; contact footprint/battle: **213 passed, 6 ignored**.
- Force/calibration: **400 passed, 7 ignored**; full station admission: **208 passed, 6 ignored**.
- Servo/spec/bake/geometry/headless: **263 passed, 6 ignored**.
- Belt/spec/bake/sandbox/geometry/headless: **464 passed, 7 ignored**.
- Print store and citations: **27 passed, 1 ignored**.

Every implementation commit passed formatting, locked workspace/all-target CI clippy and its
targeted tests. Geometry batches included `pen_cam`, `field_casts` and `headless_test`.
The broad template batch initially exposed a malformed synthetic glTF fixture; after supplying
its required buffer view, template/citation tests passed. A contact test initially assumed full
body mass at an off-centre point; it now uses the class momentum bound, with a separate contact
subdivision test and the wall-reader integration check. No failing check remains accepted.

MEASURED after G5, headless: full throttle F8 **10.684 m/s**, last-second spread **0.0023 m/s**;
neutral pivot **−0.1338 rad/s**, **0.7100** of its DERIVED kinematic ceiling. The final whole-tree
checks and post-change measurements are recorded below after their run.

MEASURED final whole-tree/print/quit guard batch: **73 passed, 1 ignored**. The final
live-belt read, geometry/headless and whole-tree batch then passed **279 tests, 7 ignored**,
with formatting and locked workspace/all-target CI clippy clean. No full-suite run was made.

MEASURED most recent flat-ground run, after the physics/storage changes: F8 **10.684 m/s**,
spread **0.0023 m/s**; neutral pivot **−0.1338 rad/s**, ratio **0.7100**. The final retained-belt
read guards change no admitted-state geometry or force arithmetic.

MEASURED completion clock check: 2026-11-01 13:59:16 +0700. DERIVED elapsed:
87 minutes 7 seconds from the measured start. The orchestrator retains
G1 feel review, other-tank support, the open inventory rows, pricing/captures, merge and push.

## Part 4: blind reviews of part 3, and the open rows

Base: main at "ballistics: remove unread cone-source producer state", lane sweep-drive-4, bay-2.
MEASURED start: 2026-11-01 14:26:34 +0700. No agents, captures, pricing, deployments, external APIs
or pushes; builds and tests through the shared queue with `dev_ui,bitprobe`, unattended and
headless.

### The two blind reviews

Codex and Claude read part 3's twelve commits without seeing each other. Every finding was decided
against the code. "Mutant" below means the fix reverted in the working tree and the new test run
against it.

| Reviewer | Finding | Verdict | Fixed in |
|---|---|---|---|
| Codex | P2: `belt_draw::repeatable` admits attributes of unequal length, and repetition then misaligns every later link | Verified: four positions under three normals, UVs and tangents were accepted; the mutant fails the new class test | "refuse shoe attributes of unequal length before repetition" |
| Claude | P3: the implicit blow budget is keyed on exact normal bits, so links of one run each spend the source's mass | Verified: four patches along normals one ulp apart read a blow of 3999.9995 N·s against the source's 1000 N·s closing momentum on the mutant (MEASURED) | "share one implicit blow budget per collider and source body" |
| Claude | P3 (arguably P2): the adapter's staging is untested, and the trunk fixture is no game object | Verified, both halves: the mutant dropping the adapter's `record_static_contacts` call fails the new headless test, and every earlier test reached that method only by calling it itself (read); no game collider is a cylinder (tree segments are two-ring convex hulls), so the cylinder arm of `Surface::triangles` served only its test | "test the adapter's static belt reaction on a parked tank"; "support trunks as the hulls trees build" |
| Claude | P3: the startup template gate reads and parses the whole view glb | Verified: `gltf::Gltf::from_slice` copies the binary chunk into an owned buffer; the view is 67,301,428 bytes, its JSON chunk 226,528 (MEASURED) | "read the view file gate from its JSON chunk alone" |
| Claude | P3: this record cited eleven lane hashes | Verified: none is on main | this part's commit: every citation is a subject |
| Claude | P3: stale rubble scope text | Verified: `Surface` said a static surface takes nothing | "state the static structure reaction in the rubble docs" |

Nothing this lane read contradicts either reviewer's "checked and sound" list; those items were not re-verified one by one. Tally: Codex
one finding, verified; Claude five, all verified.

### Open rows

| IDs | Status |
|---|---|
| R08–R10 | Fixed, "make weapon clock exhaustion a bug outcome (R08-R10)". Decision: a bug, not a session outcome. 2^32 ticks at 64 Hz is 776 days (DERIVED), past any session; every deadline shift and the local clock's advance expect inside the range (`tank::SESSION_TICK_RANGE`). The frozen sentinel state is deleted with its tests; the replicated fields are unchanged. |
| R31–R33 | Fixed, "refuse unrowed, unloaded and non-uniform tank surface parts (R31-R33)". The shipped view has no scaled node (MEASURED: 87 nodes), so the scale refusal cannot fire on shipped data. |
| R34–R36 | Fixed, "certify the print shoe and mask at their first read (R34-R36)". The shoe's positions are an invariant of template certification; an underside covering no print cell and a mask the CPU cannot read are refused naming the file and the fix. The shipped mask is 8-bit RGB (`map` test), which Bevy's PNG loader decodes to a readable format. |

### Other tanks as belt support: deferred

Not attempted in this lane's time. The obstacle, read from the adapter as it stands:

- `RubbleBodies::gather` reads body state through `Query<PieceData, Without<Tank>>`, while the
  tank query holds every tank's `Forces` mutably. Admitting a tank needs one tick-start snapshot of
  all recipients from a single body query (the tanks' own `Forces` reached through it, or a
  `ParamSet`), taken before any tank's pass.
- The snapshot is shared by every tank's pass, so each pass must skip its own hull's surfaces
  (today the shared exclusion list does it for all tanks at once).
- A reaction on a tank must be staged and written after every pass. Each pass reads its own hull's
  velocity live through `Forces`; an impulse written mid-loop would let entity order decide what
  the next tank's belt sees. Writes land in `apply_track_forces` before physics, in the
  established driving/recoil/physics order.
- Each tank's own colliders are always in its own `near` list, so admitting tanks wholesale would
  read every hull's shape on open ground each tick and break the cost gate in the rubble module
  doc. Admit a tank body only where another tank's footprint query reaches it.
- Tank collision proxies are convex hulls (`tank::spawn`), a shape the support check reads.

### Verification

MEASURED green batches after their fixes: belt, contact, rubble and the new headless test **31
passed**; shooting, tank, crew UI, tank surface, prints, link view and belt **112 passed, 1
ignored**; link view, rubble, prints and contact **66 passed, 1 ignored**; the whole-tree guards
(`doc_citations ui_ascii net_boundary bake::`) with the whole `headless_test` module **61 passed, 2
ignored**. The mutant run failed all three new tests. Every commit passed the formatting check and
locked workspace/all-target CI clippy. No full-suite run, and no geometry, collider or running-gear
change, so `pen_cam` and `field_casts` were not run.

MEASURED: the parked tank's slab read the belt's reaction in 64 of 64 ticks, a mean **0.9999** of the
weight. G1 is untouched (`GRIP_SHEAR_MODULUS_M` unchanged). The orchestrator retains G1, other-tank
support, pricing of the JSON-chunk gate, captures, merge and push.

## Part 5: blind reviews of part 4, one deadline domain, and hull support

Subject: the game's driving and damage model, game state only. Crew are HP pools at stations;
weapon changes concern fire eligibility and timing only. Base: main at "net: rebuild the battlefield
at the next fill-up", lane `sweep-drive-5`, bay-1. MEASURED start: 2026-11-01 16:03:17 +0700.

### Every blind-review finding

| Reviewer | Finding | Verdict and witness | Fixed in |
|---|---|---|---|
| Codex C1 | P2: mixed normals choose the first patch's source mass | Verified: orthogonal patches at one off-centre point have different effective masses; the previous group chose its first normal. The regression reverses staging and requires identical mass and reaction bits. | "driving: keep distinct implicit contact normals in distinct budgets" |
| Codex C2 | P2: equal column lengths admit shear | Verified: the review's composed stretches and rotation passed the old length-only check; the new refusal test failed on that check. | "driving: certify worn rest frames before view admission" |
| Codex C3 | P2: an admitted reload reaches the clock-overflow panic | Verified: `67_108_860.0` seconds passed the old spec gate; the refusal regression failed, and arming across the counter end panicked. | "driving: share wrapping battle and weapon deadlines" |
| Claude L1 | P2: a resting top-face press becomes another direction's blow | Verified: the original split read a **100 N·s** blow on the resting face (MEASURED). It now reads zero, while rounded copies of one normal still share their source budget. | "driving: keep distinct implicit contact normals in distinct budgets" |
| Claude L2 | P3: an unarmed paused gate still freezes silently | Verified: deserializing `ready_tick: null, paused_at_tick: 5` succeeded on the original type; the rejection test failed. Ready, Loading and Paused are now disjoint phases. | "driving: share wrapping battle and weapon deadlines" |
| Claude L3 | P3: the session-range expect does not bound an authored delay | Verified, the same admitted-duration witness as C3. Reload and swap refusals name the weapon field and repair; the containing asset gate supplies the tank file. | "driving: share wrapping battle and weapon deadlines" |
| Claude L4 | P3: a peer's last-counter deadline panics the owner's pause | Verified: a serialized gate at `u32::MAX` decoded, then panicked on pause in the original code. The same round trip and pause now succeed. | "driving: share wrapping battle and weapon deadlines" |
| Claude L5 | P3: the surface-frame refusal is late and omits the glb path | Verified by the two entry points: the document was available at startup, but the check lived in the dress. Startup now checks composed worn frames and names the glb, node path and export fix; a document fixture covers the refusal and articulated boundary. | "driving: certify worn rest frames before view admission" |
| Claude L6 | P3: the trunk fixture never reaches the trunk reader | Verified: part 4's hull fixture has neither `TreeSegment` nor `TreeBody`; its transfer assertion cannot prove consumption. The new spawned-tree test requires an implicit press to open a structural joint and change the tree state. | "driving: test the trunk reader on implicit belt loads" |

All findings are verified; none is dismissed. Part 4's statement that the invalid sentinel state
was deleted, and its claim to have tested both the adapter and trunk consumer, were too strong.
The new enum and consumer test close those gaps.

### Decisions and invariants

- Implicit contacts share a budget within a direction, not across distinct faces. The numerical
  co-normal envelope is ASSUMED at 32 float epsilons; canonical staging fixes the representative
  and accumulation order, and the mass uses the load-weighted direction. The one-ulp subdivision
  guard remains green. This changes no material or grip tuning.
- `SimTick` is the common battle/weapon deadline domain and implements no numeric ordering.
  Deadline comparison is its signed wrapped difference; the local weapon counter wraps. The
  DERIVED maximum authored span is `TickSpan::MAX`, strictly below half the counter. Reload,
  swap, cyclic and mode load gates enforce it. Frozen work is a checked `TickSpan`, parsed during
  serde decoding; malformed JSON and actual postcard wire fixtures are refused at that edge.
  Resuming starts the frozen interval from the resume tick, even after clock wraps. Revision 58
  and the wire manifest deliberately record the changed gate encoding. Network clock publication
  continues to read `LocalTimeline`; its transport-clock implementation is outside this change.
- Worn rest frames must have finite, nonzero, equal-length orthogonal axes within the shader's
  tolerance. The startup JSON document checks the same articulated boundary the dress uses;
  the dress also guards programmatically constructed views. Uniform rigid frames remain legal.
- Every tank pass reads one tick-start support snapshot. All body force state comes through one
  body query; the driving query holds no second `Forces` borrow. A hull's shapes enter only when
  another source's footprint query reaches them, and each pass excludes its own body by identity.
  The response ledger sizes the existing clamp without changing the snapshot; writes land after
  all passes, within `DrivingForces`, before recoil and physics. No recipient-kind law was added.
- DERIVED cost property: a lone tank's own colliders are removed before shape gathering, so its
  open-ground pass reads no hull shape. The sensor reference and solid hull produce identical
  driving and reaction bits. Pricing is the orchestrator's work; no timing measurement was taken.

### Verification

MEASURED green batches: contact and citations **10 passed**; surface, link-view and citations
**35 passed**; clock, shooting, spec, state hashes, all `net::` and citations **354 passed, 1 ignored**.
The clock batch includes real loopback UDP and the battle boundary tests. Formatting and locked
workspace/all-target CI clippy passed before each implementation commit.

MEASURED: the combined driving, rubble, tree, `pen_cam`, `field_casts`, headless and citation
batch passed **252 tests, 5 ignored**. The two targeted mutants both failed: immediate reaction
writes broke entity-order identity, and dropping implicit contacts from `fell_on_loads` left the
trunk joint intact. Restoring the implementation passed both tests, the upper-belt/lower-hull
load and reaction test, and the lone-hull field-only identity test. No additional P1/P2 finding
was found in the touched files.

MEASURED: final formatting and locked workspace/all-target CI clippy passed. The required
whole-tree guard batch (`doc_citations ui_ascii net_boundary bake::`) remained queued on the
machine lock at the wall-clock limit, and was stopped with this lane's own processes. Its result
remains unverified. Before merge, the orchestrator must run:

```sh
OVERMATCH_CARGO_LOCK_WAIT_S=10800 OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless \
  scripts/serial-cargo test --lib --features dev_ui,bitprobe -- doc_citations ui_ascii net_boundary bake::
```

The later optional numeric load read was not run. The completed support test establishes
positive carried load and a downward recipient impulse; its exact values were not collected.

MEASURED completion clock check: 2026-11-01 17:32:45 +0700. DERIVED elapsed: 89 minutes 28 seconds.

G1 (`GRIP_SHEAR_MODULUS_M`) is unchanged. No agents, captures, pricing, deployments, external API
calls or pushes. The orchestrator retains G1 feel review, pricing, merge and push.
