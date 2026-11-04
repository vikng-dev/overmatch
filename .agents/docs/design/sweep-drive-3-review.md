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

## Part 6: blind reviews of part 5

Subject: the game's driving and damage model, game state only. Crew are HP pools at stations;
weapon changes concern fire eligibility and timing only. Base: main at "net: pin the merged wire
types at PROTOCOL_REV 60", lane `sweep-drive-6`, bay-2. MEASURED start: 2026-11-01 20:34:11 +0700.
No agents, captures, pricing, deployments, external APIs or pushes.

### Every blind-review finding

| Reviewer | Finding | Verified / not a defect | Fixed in |
|---|---|---|---|
| Codex | P2: the published weapon clock saturates, so a deadline armed at the counter's end never matures | Verified. Cause: lightyear_core 0.30.1's `Tick` saturates (its `AddAssign` is `saturating_add`), so `increment_local_tick` holds `LocalTimeline` at `u32::MAX` and `publish_shot_clock` copied that value every tick. A 2-tick deadline armed there wraps to tick 1 and the clock never reaches it. The new test requires the published sequence `MAX, 0, 1, 2` with the deadline maturing at 1; the old publication gives `MAX` four times. | "net: keep the published tick counting past the timeline's saturating end (#238)" |
| Claude | P2: a tank resting on another tank's hull gives that hull no reaction | Verified. With the upper hull at rest, the old clamp's closing speed is 0, so `stop` is 0, the push is 0, `catch_up` is 0 and the drag is 0. The new test requires the lower hull's momentum to be the exact opposite of every force the upper belt applied through it. | "driving: give belt-held hulls the whole reaction, sized after every pass (#238)" |
| Claude | P2: `same_normal`'s ASSUMED 32-epsilon envelope splits the budget of every curved run | Verified. Normals 1° apart (1.7e-2 rad) fall outside the envelope (3.8e-6 rad, DERIVED), so the review's four patches from one 1000 kg source closing at 1 m/s each spent the whole mass, giving 4000 N·s. The new curved-run test bounds the blow at the source's 1000 N·s, plus the DERIVED 0.1% that a 3° spread adds. | "contact: size one implicit blow budget against the source's whole distribution (#238)" |
| Claude | P3: the respawn opening is a raw `u32` deadline with its own hand-written wrapped difference | Verified by reading: `tick.wrapping_sub(now) as i32` and `clock.0.wrapping_add(wait.ticks)`. Opening and arming now use `SimTick`, and `RespawnWait` holds a `TickSpan`. The replicated `Respawn::Opens::tick` field stays a raw `u32`: `src/ballistics/recording_tests.rs`, outside this lane, constructs it. | "driving: type every weapon and battle span and refuse paused rounds (#238)" |
| Claude | P3: no type carries the span limit; `duration_ticks` re-derives each shot's span and expects what the spec admitted | Verified by reading: `gate.arm(now, u32::MAX)` compiled. `SimTick::after` and `arm` now take a `TickSpan`; battle spans are `TickSpan`s; `FireCycle` holds spans parsed once by `TickSpan::from_secs` at the spec gate; `duration_ticks` and the spec's separate `duration` check are deleted. The fire system asserts the fixed timestep the spans were parsed at. The clock resources stay raw counters because `src/damage_cam/delivery.rs` reads them. | same |
| Claude | P3: Paused with `belt_remaining > 0` is representable and freezes silently | Verified: `{"timing":{"Paused":{"work":3,"since":0}},"belt_remaining":5}` has every field of the original type and decodes there; `tick_weapon_gate`'s belt-has-rounds branch reads only `deadline_reached`, which is false for Paused. Paused work is load work (a `Single` holds no belt; an `Automatic` pauses only a dry belt's swap), so decoding refuses it (`WeaponGateError::PausedWithRounds`) and `pause` panics on a belt with rounds as a bug outcome. REV 61. | same |
| Claude | P3: the peer-gate tests parse the old schema and duplicate each other | Verified by reading: the JSON lacks `timing`, so it is refused before any phase logic runs. Both are replaced by one test of current-schema invalid phases (work past the horizon; paused with rounds), in JSON and postcard, plus a valid paused decode. | same |
| Claude | P3: entity order sizes a recipient that two tanks touch | Verified by reading: the ledger advanced each clamp in tank-entity order. Recipients are now sized after every pass, over every tank's contacts at once, in one canonical order of the contacts' geometry. The new two-tank shared-plank test requires identical bits with the spawn order exchanged. | "driving: give belt-held hulls the whole reaction, sized after every pass (#238)" |
| Claude | P3: two mechanisms for self-support exclusion | Verified by reading: the sandbox passed `&[hull_entity]` to `gather`; the game passed `&[]` and called `exclude_source`. `gather` no longer takes an exclusion list, and both adapters call `exclude_source`. Each adapter's `near` filter remains: in the game it is the cost gate, and in the sandbox it is required because the piece query cannot borrow the hull. | same |
| Claude | P3: `pieces.clone()` deep-copied every gathered surface each tick; stale `RubbleBodies` doc | Verified by reading. With sizing after all passes, recipients are sized on the snapshot itself, and the copy is deleted. The doc now names the shared tick-start snapshot. | same |

All ten findings are verified; none is dismissed. Neither reviewer's "checked and sound" list was
re-verified item by item.

MEASURED mutant run: each of the five new tests was run against its fix reverted in the working tree
(the publication copying the timeline; the belt-held branch removed; the canonical sort removed; the
blow budgeted per patch, as the envelope did for distinct normals; the paused-rounds refusal removed),
and all five failed. The publication read `MAX` on all four ticks; the curved run read **3999.9995
N·s** against 1000 N·s; the resting stack's lower hull read zero momentum against a **9414.239 N·s**
reaction; the shared plank's bits differed with the spawn order exchanged.

### Decisions and laws

- **One wrapping tick.** The game's tick is the count of fixed ticks run. Below lightyear's
  saturating end the published tick equals the timeline's tick. After that, each fixed tick the
  timeline is held at the end adds one more. Lightyear's own transport tick still saturates, and
  its input and replication ordering past that end (776 days at 64 Hz, DERIVED) belong to lightyear.
- **What holds the recipient decides its side** (superseded: one law by mass, inertia and the belt's
  springs, `construction-destruction-sweep-2.md` part 6). A body whose own support is a belt law this
  tick (any driving tank's hull) takes the whole opposite reaction, `−(normal × load + traction) ×
  dt`, as fixed construction does, with no closing-speed condition. Every other dynamic body is a
  piece and keeps the clamped sequential impulse. This is a fact about construction, not about a
  vehicle kind: it holds for any tank. Stability, DERIVED from the stack fixture's MEASURED
  9414 N·s per 1/64 s at its nominal 1 cm press: `k ≈ 6e7 N/m`, so `k·dt²/m ≈ 0.5` for a 30 t
  hull, and coupling two equal hulls doubles it to about 1, inside the explicit limit of 4.
- **One implicit budget per (collider, source).** The blow is `m_w v_w`, capped at what the
  closing patches moved. `v_w` is their moved-weighted closing speed. `m_w = (Σw)² / (wᵀ K w)` is
  the source's effective mass for that weighted distribution of unit normal impulses, with `K` its
  inverse mass matrix between the points, rotation and locked axes included. Co-located co-normal
  copies reduce to the old centroid mass. Orthogonal faces, both closing, receive their summed
  components. Resting patches take no share. No envelope or threshold remains, so the law holds
  for any running gear.

### Verification

MEASURED with the fixes: the curved run's blow is **1000.38074 N·s** against the source's 1000
N·s, which is the DERIVED `m_w` for normals spread 3°. The resting stack's lower hull momentum is
**[0, −9414.24, −27.157345] N·s** against a **[0, 9414.239, 27.157337] N·s** reaction. The closing
stack's lower hull now takes **−0.331169 m/s** from a **637,483.938 N** upper belt load (the whole
reaction; part 5 clamped it).

MEASURED green batches: shooting, sim_tick, spec, crew_ui, `vfx::muzzle`, state hash and all `net::`
**384 passed, 1 ignored** (the wire-type pin then re-pinned at REV 61). The combined batch passed
**1033 tests, 10 ignored**: contact transfer, track, trees, constructions, `headless_test`, `pen_cam`,
`field_casts`, all `net::`, the weapon modules, and the whole-tree guards (`doc_citations ui_ascii
net_boundary bake::`). The new tests, contact transfer, rubble and citations passed **35** after
the mutants were restored. On the final tree, the whole-tree guards with `track::sim` passed **50**.
Every commit passed `cargo fmt --all --check` and locked
workspace/all-target CI clippy. No full-suite run was made.

Not done in this lane, for the orchestrator: `Respawn::Opens::tick` stays a raw replicated `u32`
(`src/ballistics/recording_tests.rs` constructs it), and `ShotClock`/`WeaponClock` stay raw
counters (`src/damage_cam/delivery.rs` reads them); typing either means editing those files. G1
(`GRIP_SHEAR_MODULUS_M`) is unchanged.

MEASURED completion clock check: 2026-11-01 21:14:30 +0700. DERIVED elapsed: 40 minutes 19 seconds.

## Part 7: blind reviews of part 6 and the remaining tick counters

Subject: the game's driving and damage model, game state only; weapon changes concern fire
identity and eligibility. Lane `sweep-drive-7`, bay-1, based on main at "docs: record the view-6
review verification (#215)". MEASURED start: 2026-11-01 16:02:56 UTC. This extends part 6;
its provisional clock extension and descriptions of reaction sizing are rechecked here.

### Every blind-review finding

| Reviewer | Finding | Verified / not a defect | Fixed in (subject) |
|---|---|---|---|
| Codex | C2: sandbox sizes piece reactions separately per track | Verified: `apply_belt_support_field` called the shared sequential clamp once per side, allowing left-side impulses to change the right side's input before the canonical sort. Both side reports now enter one production `piece_impulses` call. | "driving: size both sandbox tracks in the production reaction pass (#238)" |
| Codex | C1: wrapped published tick disagrees with transport and crashes attributed fire | Verified: the second saturated timeline tick publishes 0, while `queue_fire` asserted it was MAX. The private extension is deleted; network publication refuses transport exhaustion and fire transport reads the published clock used at spawn. | "net: use one admitted timeline for fire and deadline clocks (#238)" |
| Claude | L1: the same disagreement also invalidates client fire catch-up and replica path ages | Verified, same counter-end witness: fire tick 0 against a saturated local or interpolation cursor. The session now refuses before that state exists; native numeric catch-up remains the network law. | same |
| Claude | L2: a late join starts a different process-local counter at saturation | Verified: `held_at_end` started at zero in each process regardless of authority elapsed time. There is no per-process extension now; an exhausted network session is refused rather than independently continued. | same |
| Codex | C3: parsed `FireCycle` is discarded and durations are parsed again at spawn | Verified by the call graph: `checked`, `weapon_gate` and `Weapon::of_spec` each called `fire_cycle`. Deserialization now consumes `WeaponSpecFile` into a `WeaponSpec` holding the joined cycle; spawn only copies it. | "weapons: retain the fire cycle parsed at the data edge (#238)" |
| Claude | L3: the support-law comment claims every recipient takes a clamp | Verified: `piece_impulses` gives belt-integrated recipients the whole reaction. The comment now states both recipient laws and the tick-start velocity invariant. | "driving: state the actual support coupling invariant (#238)" |
| Claude | L4: reaction ordering and source-mass docs contradict their implementations | Verified: the reaction pass sorts all sources geometrically; `contact_mass` uses a distribution of normals, reducing to a centroid only when co-normal. The docs state these invariants. | "contact: document canonical reactions and distributed source mass (#238)" |
| Claude | L5: field contacts sort and allocate per-contact vector keys | Not a defect on this base: already fixed by "driving: skip reaction sorting for contacts without dynamic recipients (#250)". The filter precedes key construction, the key is an array, and `field_and_fixed_support_never_enter_the_dynamic_reaction_sort` checks zero keys. | already fixed on main |
| Claude | L6: belt integration membership should depend on current support, and resting loose pieces take no normal reaction | Not a defect in the implemented law. Membership identifies the force-stepped construction, including an airborne or overturned source; removing it on contact loss would discard that source's incoming reaction. The separate loose-piece clamp intentionally gives a resting piece no normal impulse, pinned by the existing resting-piece test. It is an asymmetric approximation, not momentum conservation; the module doc now says that explicitly. | documentation clarified in "contact: document canonical reactions and distributed source mass (#238)" |
| Claude | L7: duration ceiling and horizon are implemented three times | Verified by reading the three constructors. `TickSpan::from_duration` now owns quantization, including a typed zero-timestep refusal; weapon seconds, Battle phases and respawn waits call it. | "timing: share duration quantization across every span gate (#238)" |
| Claude | L8: the composition timestep check happens only on fire | Verified: a nondefault fixed step with no armed weapon reached play unchecked. The weapon plugin now checks its fixed step at startup, before any gate or shot can run. | "weapons: refuse a mismatched timestep before play (#238)" |

### Clock decision and remaining-counter scope

Part 6's private extension was not a valid continuation of the network session: input,
replication, fire release and interpolation are numerically ordered by Lightyear. Network play
therefore reads that single finite timeline and refuses its exhausted endpoint with a typed error,
a visible restart instruction and process status 2. No per-process epoch is retained. Offline
play continues to wrap. Both publication resources carry `SimTick`, as does `Respawn::Opens::tick`;
raw values are extracted only at existing wire, recording and presentation boundaries. REV 63
and the wire type fingerprint deliberately record the respawn definition change; its transparent
encoding remains the same.

The `src/ballistics.rs`, `src/ballistics/recording_tests.rs`, damage-camera and presentation edits
are only clock constructors or raw-value boundary conversions. No ballistic law is changed. G1
(`GRIP_SHEAR_MODULUS_M`) is unchanged. No captures, pricing, agents, issue comments or pushes.

### Additional clock finding and regression evidence

Verified additional finding: `FixedLast` advanced the local weapon fallback even in a network
composition, leaving it one tick ahead of both `LocalTimeline` and `ShotClock` between fixed
steps. `PublishedWeaponClock` now names the composition that owns publication; the local fallback
runs only without that owner. Fixed in "net: use one admitted timeline for fire and deadline
clocks (#238)". Offline advancement and its pause/wrap behavior remain covered.

MEASURED mutant evidence: the per-track sandbox pass gave one contact **22.881598 N·s** where
the production pass gave **19.58482 N·s**, reversing those magnitudes at the other contact. The
strengthened comparison sorts results by contact position, so this is a sizing failure rather
than merely output order. Without the ownership guard the weapon clock read **43** against the
published clock's **42**. Moving the timestep check into the fixed loop allowed a nondefault-rate
composition through an explicitly present empty startup schedule. Removing the exhaustion gate
admitted `SimTick(4294967295)` instead of returning `NetworkTickExhausted`. Each mutant failed its
regression; all mutant source is restored before the final gates.

### Final verification

MEASURED final broad batch: **774 passed, 8 ignored**. This includes all `net::`,
weapon/spec/tank timing, recording and delivery, sandbox belt reactions, track/contact tests,
`pen_cam`, `field_casts`, `headless_test`, and the whole-tree guards. The Battle advance API
also carries `SimTick` directly; it does not unwrap and reconstruct the published clock.

Each implementation commit passed `cargo fmt --all --check`, locked workspace/all-target
CI clippy, its targeted tests and `doc_citations`. MEASURED final whole-tree guard batch
(`doc_citations ui_ascii net_boundary bake::`): **44 passed, 0 ignored**.
The four counter/sizing/startup regressions pass with all mutants removed. No full-suite run
was made. The only build diagnostic was the existing linker unwind-section size warning.

MEASURED completion clock check: 2026-11-01 17:06:16 UTC. DERIVED elapsed: 63 minutes
20 seconds. G1 is unchanged; its feel call, pricing, captures, merge and push remain
with the orchestrator.


## Part 8: verification of the sweep-drive-7 doubles

Subject: the game's driving model and damage-model timing, game state only.
The law-inertia lane checked both review reports against the implementation.

| Reviewer | Finding | Verified / not a defect | Fixed in (subject) |
|---|---|---|---|
| Claude | P3: `collect_column_contacts` claims both recipient laws read tick-start velocity | Verified: loose-piece pushes update the snapshot before the next clamp; belt-integrated recipients read no velocity. | "driving: resolve the sweep-drive-7 review leftovers (#259)" |
| Claude | P3: `WeaponSpec` has a second serde parse path with no caller | Verified: tank mounts deserialize through the named `WeaponSpecFile` map. Direct serde admission is deleted; the conversion remains the named map's implementation. | same |
| Claude | P3: `TickSpanError::ZeroTimestep` represents a code-constant fault as outside input | Verified: both production callers supply code-owned timesteps. Zero now panics as a bug, with a regression naming that invariant. | same |
| Codex | P3: `FireMode::Automatic` links to the moved tracer field | Verified: the field belongs to `WeaponSpecFile`; the rustdoc link now addresses that field. | same |
| Codex | P2: `belt_held` is a per-body rule | Routed to law-hull-work; its code is outside this lane. | routed |
| Claude | P3: `queue_fire` clock cross-check compares one clock to itself | Routed to sweep-net-8; its code is outside this lane. | routed |

MEASURED verification for the review leftovers: formatting and locked workspace/all-target CI
clippy passed; the targeted `spec sim_tick track::forces doc_citations` batch passed **58 tests**.


## Part 9: verification of the law-inertia doubles

Subject: the game's driving model, game state only.
The law-ground-1 lane checked both review reports against the implementation.

| Reviewer | Finding | Verified / not a defect | Fixed in (subject) |
|---|---|---|---|
| Claude | P2: the replay fixture's slope seed rests the hull on the flat slab, rear stations inside it | Verified: the seed's cast foot lands on the slab at z ≈ −37.55, short of where the 20° face leaves it (z ≈ −37.71). MEASURED at slope tick 0: 330 loaded contacts up to 0.653 m off the face's plane, the slab carrying more load than the face, the hull pitching from 19.0° to 11.4° within 16 ticks. The new on-face assertion fails on that seed. | "driving: seat the replay's slope start on the 20 degree face (#259)" |
| Claude | P3: the idler is reflected at its contact radius, not the pin-line radius it turns at | Verified: the belt wraps the idler, which turns with the pin line, the circle the route wraps and the view spins it on. Shifts, DERIVED: 4,763.42 → 4,749.86 kg/side (idler term 103.81 → 90.25 kg); brake 2.88605 → 2.88723 m/s²; flat top-gear acceleration 0.66782 → 0.66809 m/s². The review's 25.56 mm pin offset gives 89.80 kg; the rig's measured offset gives the 90.25 kg above. | "driving: reflect the wrapped idler at its pin-line radius (#259)" |
| Claude | P3: the transmission's Tiger fixture still uses the deleted 16,000 kg and claims to mirror the sheet | Verified. At the derived 4,750 kg, 67 of 68 transmission tests hold. The hill-hold release test does not: the forward hold walks F3 → F2 → F1 and releases in F1 (MEASURED), because the release gate reads the one-tick clutch-lock force, which scales with belt inertia, while target selection reads modeled capability. That test keeps a labelled 16,000 kg fixture; the gate disagreement is routed to the owner. | "driving: run the transmission's Tiger fixture at the derived belt inertia (#259)" |
| Claude | P3: two docs still say "belt + drivetrain" inertia | Verified at `RigSpec::belt_inertia` and `predict_shift_landing_m`. | "driving: name the belt inertia the sandbox and landing predictor read (#259)" |
| Claude | P3: nothing tests the unequal-sides admission in `RigGeom::build` | Verified: with the admission call disabled no test failed before; the new rig test fails under that mutant and passes restored. | "driving: test that running-gear admission names the sheet it refuses (#259)" |
| Codex | No findings | Nothing to verify. | none |

The same hold walk shows in the booted sim on the course's 20° face (MEASURED, slope trace): the
hold re-targets down the ladder to F1 and releases in F1. The owner decides whether the release gate should
read the gear's modeled capability, as target selection does.
