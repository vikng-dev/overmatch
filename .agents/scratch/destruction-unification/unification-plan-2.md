Subject: **the game’s environment destruction model; game state only.**

Row 12 moves trees into the existing building destruction system, using today’s joint cracking and wear.
Walls keep their courses; trunks and roof planks use the same cutter’s grain-stretched cells, selected and sized in data.
The separate tree simulation, HP pools, ledger, carriers, and collider exceptions disappear together.
The original tree drive becomes the cutover’s crash proof.
**A fir’s toughness will still depend on how finely it is cut.** Choose the cut for appearance; report shot counts for judgement. Never adjust cut size to obtain strength here.
Batch the owner’s eye gates into **two sittings, DERIVED**: prepared fir/roof previews, then the complete cutover drive and falls.
The next row-12 build follows the cutover, deletion audit, and checks.
Hosted capacity acceptance belongs to row 13, on the owner’s **2-vCPU allocation, DERIVED planning requirement**.

## Basis and parallel tracks

This plan uses HEAD `536564fa547ed34f2783ea8d148a82a5846401bb`. Citations below are refreshed to that source. No edits, builds, tests, or web searches were performed. Issue dispositions describe proposed closure conditions, not verified live GitHub status.

Task numbers, issue numbers, versions, revisions, dates, and source line numbers are identifiers. Proposed counts, budgets, and acceptance choices are **DERIVED**; inspected source values and historical observations are **MEASURED transcriptions**, not new runtime measurements.

The current code wins over conflicting prose:

- Wear still subtracts a chunk-volume pool; holes do not provide solid-survival truth: [fracture.rs:668](/Users/Yan/github/vikng-dev/overmatch/src/buildings/fracture.rs:668), [breakage.rs:1769](/Users/Yan/github/vikng-dev/overmatch/src/buildings/breakage.rs:1769).
- Runtime joints contain area, centroid, normal, and bond—not polygons: [schema.rs:221](/Users/Yan/github/vikng-dev/overmatch/src/buildings/schema.rs:221).
- The no-live-joint `weigh` return is already landed: [fracture.rs:2110](/Users/Yan/github/vikng-dev/overmatch/src/buildings/fracture.rs:2110). T27 must preserve and extend the saved work, not implement that fix again.
- Tree detachment still constructs translated compounds: [motion.rs:113](/Users/Yan/github/vikng-dev/overmatch/src/trees/motion.rs:113). The belt still requires boxes or convex polyhedra: [rubble.rs:245](/Users/Yan/github/vikng-dev/overmatch/src/track/rubble.rs:245).

Use **three bays, DERIVED from the fixed resource limit**. Assign these tracks to available leases; do not assume a particular bay is free.

| Window | Existing leased lane | Cutter/content track | Runtime/data track |
|---|---|---|---|
| Start | **`settle-islands` — old T4; already running. Do not redispatch it.** | M3 geometry and convex-source schema | M1 unread fields, then M2 anchors |
| After source/schema preparation | Next ready independent task | M4 common cutter | M5 shared runtime |
| After cutter/runtime interfaces exist | M6 fir bake | M7 roof recut | M9 graph reuse, after M5 |
| As prepared content becomes available | M8 common wood view | Remaining content checks and fixes | M9 completion; then M10 immediately before cutover |
| Final row-12 window | M11 cutover branch owns the affected runtime files | Read-only review/capture assessment | Read-only review/capture assessment |

M6 and M7 run side by side. M9 does not wait for either content bake. M8 can prepare its view machinery from the admitted schema while M6 finishes; its complete preview needs M6’s output.

Compiles and machine measurements remain serialized. Read-only lens reviewers need no bay. Every implementation landing follows [AGENTS.md:29](/Users/Yan/github/vikng-dev/overmatch/AGENTS.md:29), including targeted checks, CI-feature clippy, clean lane commits, and orchestrator-controlled merging and publication.

## Tasks

Each task closes its listed ratchet obligations in its implementation lane. Lower-priority backlog rows receive no separate cleanup lanes, per [lenses/README.md:21](/Users/Yan/github/vikng-dev/overmatch/.agents/lenses/README.md:21).

### M1 — Delete unread material inputs  
**Old T2. Builder: Codex.**

**Work and files.** Remove `toughness` and `compressive_dif_strain_rate` from consumed/parser types and shipped material text. Preserve useful source information beside facts derived from them. Repair the registry tests at [substances.rs:157](/Users/Yan/github/vikng-dev/overmatch/src/substances.rs:157), [substances.rs:188](/Users/Yan/github/vikng-dev/overmatch/src/substances.rs:188), [substances.rs:1070](/Users/Yan/github/vikng-dev/overmatch/src/substances.rs:1070), and [substances.rs:1789](/Users/Yan/github/vikng-dev/overmatch/src/substances.rs:1789); migrate [materials.ron:396](/Users/Yan/github/vikng-dev/overmatch/assets/materials/materials.ron:396).

**Deletes.** Unread fields, their requirements, obsolete rejection arms, and tests pinning unrelated reference outputs.

**Proof.** Material authors rely on admission matching what laws consume. A mutant retaining either obsolete requirement must reject an otherwise valid inline row; a mutant rejecting admitted zero resistance must fail the domain witness. Compare consumed material values before/after once in the report.

**Wire/data.** Material text and content digest change; wire shape does not.

**Dependencies.** No real blocker. **T1→T2 was merge order only and disappears.** Land before final content-digest capture as merge order, not a law dependency.

**Conflicts.** Any material-registry edit; M2’s attachment admission may need a routine rebase.

**Ratchet.** `load-11`, `load-12`: [backlog.md:15](/Users/Yan/github/vikng-dev/overmatch/.agents/lenses/backlog.md:15).

**Player/eye.** No intended change; no owner eye gate.

### M2 — Make anchorage explicit in construction data  
**Old T18; its T13/T17 prerequisites are removed. Builder: Codex.**

**Work and files.** Extend [schema.rs:247](/Users/Yan/github/vikng-dev/overmatch/src/buildings/schema.rs:247) so anchors name their attachment. Resolve them through [attachments.rs:81](/Users/Yan/github/vikng-dev/overmatch/src/buildings/attachments.rs:81), then build today’s law inputs at [breakage.rs:199](/Users/Yan/github/vikng-dev/overmatch/src/buildings/breakage.rs:199). Update the writer at [tools/buildings.rs:170](/Users/Yan/github/vikng-dev/overmatch/tools/src/buildings.rs:170), construction specification, and house bake atomically.

Move the existing root-plate conversion out of the tree constructor at [trees.rs:575](/Users/Yan/github/vikng-dev/overmatch/src/trees.rs:575) into the shared attachment resolution boundary. A footprint divided among several base chunks receives its attachment budget once; it must not acquire the complete hold independently on every face.

**Deletes.** Implicit “anchor borrows chunk seam” construction and the need for tree-owned attachment resolution. The old tree caller remains only until M11 deletes it.

**Proof.** Construction authors and load laws rely on declared attachment identity and representable simulation values. Mutants ignore the named kind, duplicate its total hold across base faces, or admit a value that becomes zero/infinity on conversion. Analytical attachment fixtures establish the expected aggregate capacity.

**Wire/data.** Required anchor data, bake-format change, atomic reader/writer/house migration. No new network payload. Retain the existing no-separation-work root-plate approximation; plate work remains parked.

**Dependencies.** No real blocker. **Merge order: M2 before M3**, because both change the schema, writer, and house bake. M3 can develop its geometry independently.

**Conflicts.** M3, M4, M5, M11 through schema/layout/writer files.

**Ratchet.** `destruction-04` at [backlog.md:85](/Users/Yan/github/vikng-dev/overmatch/.agents/lenses/backlog.md:85). Because the writer is edited, also close `load-13` at [backlog.md:17](/Users/Yan/github/vikng-dev/overmatch/.agents/lenses/backlog.md:17): stochastic construction tests use inline authored probabilities and never require shipped cracks.

**Player/eye.** Anchorage comes from data. Uproot-versus-snap outcomes are observations in the batched wood preview and final drive, not preserved outcomes.

### M3 — Supply convex source geometry without changing the damage method  
**Old T20 geometry preparation plus the necessary shape portion of T22. Builder: Claude, for 3D geometry.**

**Work and files.** Add convex-polyhedron clipping and mass/face calculations beside today’s polygon/prism implementation: [geom.rs:12](/Users/Yan/github/vikng-dev/overmatch/tools/src/buildings/geom.rs:12), [geom.rs:93](/Users/Yan/github/vikng-dev/overmatch/tools/src/buildings/geom.rs:93), [contact.rs:76](/Users/Yan/github/vikng-dev/overmatch/tools/src/buildings/contact.rs:76).

Generalize the admitted intact source shape beyond the prism-only [schema.rs:136](/Users/Yan/github/vikng-dev/overmatch/src/buildings/schema.rs:136). Use a common convex-hull representation for runtime source pieces; the house generator may continue using prisms internally. Adapt [buildings.rs:199](/Users/Yan/github/vikng-dev/overmatch/src/buildings.rs:199), the writer at [tools/buildings.rs:79](/Users/Yan/github/vikng-dev/overmatch/tools/src/buildings.rs:79), and the view’s prism assumptions at [view.rs:136](/Users/Yan/github/vikng-dev/overmatch/src/buildings/view.rs:136) and [view.rs:303](/Users/Yan/github/vikng-dev/overmatch/src/buildings/view.rs:303). Re-bake the house with its reader.

**T13 decision:** park its runtime face polygons, named cuts, and survival axes. The cutter needs polygons internally to clip and calculate contacts. It emits the area/centroid/normal joints today’s laws already consume. Views can recover faces from certified hulls, as [view.rs:153](/Users/Yan/github/vikng-dev/overmatch/src/buildings/view.rs:153) already does.

**Deletes.** Runtime prism-only source assumptions and view code that assumes every exposed crack reaches an extrusion cap. No joint-cracking or wear law is removed.

**Proof.** Bakers, collision constructors, and views rely on closed convex cells and correct shared interfaces. Mutants reverse winding, lose clipped volume, create overlap/slivers, or compute mismatched contact area/centroid. Use independently calculated convex fixtures and scale/translation relationships; canonical bakes reproduce identical bytes.

**Wire/data.** Source-shape format migration and atomic house re-bake; content digest changes. No face-polygon wire payload.

**Dependencies.** Geometry work starts immediately. M2 precedes landing only for shared-file merge order. M4 genuinely needs the clipping implementation.

**Conflicts.** M2/M4 writer and schema; M5 runtime source readers; M8 view.

**Ratchet.** `load-13` must already be closed by M2; verify it remains closed. No other current backlog row names these edited files.

**Player/eye.** Walls remain usable. Add hull/interface stills to the first batched sitting; broken wood appearance is judged after M6–M8.

### M4 — Expose the common cutter and data-selected patterns  
**Old T20 command/integration. Builder: Claude, for 3D cutting.**

**Work and files.** Extend the existing tools command at [bake-buildings.rs:7](/Users/Yan/github/vikng-dev/overmatch/tools/src/bin/bake-buildings.rs:7) into the common bake entry point. Replace the closed selectors at [house.rs:29](/Users/Yan/github/vikng-dev/overmatch/tools/src/buildings/house.rs:29) and [house.rs:45](/Users/Yan/github/vikng-dev/overmatch/tools/src/buildings/house.rs:45), and dispatch at [fracture.rs:81](/Users/Yan/github/vikng-dev/overmatch/tools/src/buildings/fracture.rs:81).

Construction data selects `Whole`, `Courses`, or grain-stretched `Cells`, supplies piece scale and grain frame, and names material registry keys and attachments. Keep courses as a geometric pattern, not a branch on brick identity. Accept explicit convex pieces so adding another object does not require another generator in Rust.

**Deletes.** `Material::{Brick,Wood}`, material-to-pattern inference, and attachment selection inferred from cutter kind. M7 removes the last legacy timber caller and helper; no experimental runtime selector is introduced.

**Proof.** Content authors rely on geometry and registry data determining the result. An inline additional-material fixture passes without changing Rust. Mutants hard-code a material name, ignore grain orientation, lose partition volume, or infer fastening from the pattern.

**Wire/data.** Required cutter specification and command inputs; no network shape change. Pattern/stretch/size carry appearance or resolution provenance, never a strength claim.

**Dependencies.** Real blocker: M3. Anchor-output integration uses M2. Independent of M5’s runtime generalization.

**Conflicts.** M3 tooling, M7 house generator/specification. Freeze the cutter interface before M6 and M7 branch.

**Ratchet.** `load-06`: [backlog.md:13](/Users/Yan/github/vikng-dev/overmatch/.agents/lenses/backlog.md:13). `load-13` remains closed from M2.

**Player/eye.** None immediately. The first batched sitting judges the resulting fir and roof cuts.

### M5 — Make the existing runtime accept arbitrary constructions  
**Old T22. Builder: Codex.**

**Work and files.** Generalize admission, identity, and ownership around [buildings.rs:57](/Users/Yan/github/vikng-dev/overmatch/src/buildings.rs:57), [buildings.rs:220](/Users/Yan/github/vikng-dev/overmatch/src/buildings.rs:220), [breakage.rs:126](/Users/Yan/github/vikng-dev/overmatch/src/buildings/breakage.rs:126), and [breakage.rs:543](/Users/Yan/github/vikng-dev/overmatch/src/buildings/breakage.rs:543).

Build complete mutable destruction state synchronously at spawn. Replace `live: None` and first-touch initialization at [breakage.rs:552](/Users/Yan/github/vikng-dev/overmatch/src/buildings/breakage.rs:552) and [breakage.rs:573](/Users/Yan/github/vikng-dev/overmatch/src/buildings/breakage.rs:573). Share immutable geometry; preserve stable chunk addresses and state through [breakage.rs:513](/Users/Yan/github/vikng-dev/overmatch/src/buildings/breakage.rs:513).

**Deletes.** Lazy simulation construction and assumptions that admitted common constructions must be houses. Keep today’s `strike → structural → weigh → fall` behavior.

**Proof.** Headless authority, split groups, and replicas rely on complete spawn state. A production-plugin fixture without view assets must support the complete lifecycle. Mutants defer initialization, read a GLB, reset wear on splitting, or change stable addresses.

**Wire/data.** Common construction lookup and digest coverage; tree activation waits for M11. No new field model.

**Dependencies.** Real blockers: admitted shape contract M3 and shared anchor resolution M2. No dependency on the fir bake, roof recut, survival work, or edge allocation.

**Conflicts.** M3 source readers; M9 and M11 `breakage.rs`. M8 consumes this task’s presented-group contract.

**Ratchet.** No additional current backlog rows name the core files. Keep this task’s scope out of unrelated map/view modules until activation.

**Player/eye.** Wall continuity only; compare once in the report. No separate owner sitting.

### M6 — Bake and certify the fir as construction data  
**Old T21, with survival-specific certification removed. Builder: Claude, for 3D geometry.**

**Work and files.** Replace the fracture-slice generation contract at [plan.py:18](/Users/Yan/github/vikng-dev/overmatch/scripts/prop/plan.py:18) and segment/view coupling at [build.py:391](/Users/Yan/github/vikng-dev/overmatch/scripts/prop/build.py:391). Feed the trunk’s convex envelope and authored grain-stretched cell specification through M4.

Certify closure, non-overlap, matching interfaces, positive representable mass/inertia, valid indices, anchor reach, canonical output, and view-source identity.

**Name the intact contract here:** **one collider until disturbed, DERIVED contract**. An intact trunk is a convex source element with an internal chunk table; it does not spawn colliders for all cells. Today’s generic wear/crack disturbance may trigger replacement. Departure-only replacement remains M12.

**Material decision (orchestrator's default, replacing the planner's single-row choice).** A trunk is green timber, a plank is dry timber: different materials, so each gets its own row in the material registry, read by the same laws. Move each species' measured green-wood facts out of the tree data ([trees/wood.ron:49](/Users/Yan/github/vikng-dev/overmatch/assets/trees/wood.ron:49): rupture 36 MPa spruce and 46 MPa pine, fresh density 850 kg/m³) into material rows, with their labels intact; the dry `Wood` row at [materials.ron:396](/Users/Yan/github/vikng-dev/overmatch/assets/materials/materials.ron:396) stays the planks'. This is the review's reading ("one row" means one per species, not one for all wood) and keeps today's fir mass and strength inputs, so the cutover doesn't silently halve a fir's mass. No tree-specific law or override survives: the tree data only names its material. Add no compensation factor.

**Deletes.** Slice-count fracture certification and its ring-count oracle at [test_prop.py:225](/Users/Yan/github/vikng-dev/overmatch/scripts/prop/test_prop.py:225). No survival-section contribution limit, seed-shot gate, service pressure, or section-strength field is added.

**Proof.** Runtime users rely on admitted geometry, not the asset’s name or a fixed vertex count. Mutants introduce gaps, missing anchors, degenerate hulls, duplicate addresses, or a stale certificate. Decode geometry to derive mutation sizes.

**Wire/data.** New common construction bake and certificate; replacement view artifacts remain staged until M11.

**Dependencies.** Real blockers: M3/M4 and M2’s anchor format. Runtime play preview also needs M5. M7 is independent.

**Conflicts.** M8 prop/view tooling and M11 activation.

**Ratchet.** `dev-18`: [backlog.md:76](/Users/Yan/github/vikng-dev/overmatch/.agents/lenses/backlog.md:76).

**Player/eye.** First broken-fir preview. Choose grain stretch and cell dimensions for appearance; print shot counts without an acceptance target.

### M7 — Recut roof planks through the same cells  
**Old T25. Builder: Claude, for board geometry and visual judgement.**

**Work and files.** Make board boundaries explicit construction pieces in [house.rs:64](/Users/Yan/github/vikng-dev/overmatch/tools/src/buildings/house.rs:64), preserve their declared joins, and replace [fracture.rs:107](/Users/Yan/github/vikng-dev/overmatch/tools/src/buildings/fracture.rs:107). Migrate [house_1.house.ron:77](/Users/Yan/github/vikng-dev/overmatch/tools/buildings/house_1.house.ron:77) and re-bake the house.

**Deletes.** Depth-length timber fracture, its exclusive helpers/assertions, and the last legacy timber caller. Board layout is construction; breaking each board is the common cell cutter.

**Proof.** Roof users rely on conserved geometry, declared fastening, and cuts staying within each authored board. Mutants cut across unrelated boards, duplicate contacts, or derive nailing from an element name.

**Wire/data.** House specification, bake, and content digest change; no additional payload.

**Dependencies.** Real blocker: M4. **No dependency on tree cutover.** Runs alongside M6.

**Conflicts.** M4 house/tooling files and generated house output from earlier migrations.

**Ratchet.** `load-06` and `load-13` must remain closed from M4/M2 respectively.

**Player/eye.** Longer wood pieces and irregular ends; changed roof response is reported. Judge the roof beside the fir in the first sitting.

### M8 — Prepare the wood view on common group ownership  
**Old T23. Builder: Claude, for surface geometry and visual judgement.**

**Work and files.** Replace sliced appearance binding around [trees/view.rs:442](/Users/Yan/github/vikng-dev/overmatch/src/trees/view.rs:442) and [trees/view.rs:533](/Users/Yan/github/vikng-dev/overmatch/src/trees/view.rs:533), using common membership and presentation time at [buildings/view.rs:21](/Users/Yan/github/vikng-dev/overmatch/src/buildings/view.rs:21). Update generated ownership in [gen_tree.py:156](/Users/Yan/github/vikng-dev/overmatch/scripts/prop/gen_tree.py:156).

Bark follows original exterior surfaces, exposed faces read as wood, and each branch-card base belongs to its containing chunk. Preserve intact mesh sharing/instancing. Move touched range and appearance choices into required labelled presentation data.

**Deletes.** Prepared replacement’s slice assumptions and experimental visual alternatives. M11 removes the still-active old view.

**Proof.** Players rely on unique visible ownership at the shared cursor. Mutants duplicate cards, leave them attached to departed matter, or switch ownership on packet arrival. Exercise the actual common view composition.

**Wire/data.** Generated view assets, ownership metadata, certificates, and LFS artifacts; activate together in M11.

**Dependencies.** Real interface blocker: M5. Complete asset preview needs M6; preparation can overlap its bake.

**Conflicts.** M6 prop tooling, M11 tree/view deletion.

**Ratchet.** `dev-18` must remain closed if prop tests change. No other current row names these files.

**Player/eye.** Bark, exposed wood, foliage attachment, and fracture language. Crystal-like appearance first gets visual-face treatment; if necessary revise the data-selected grain cutting pattern and re-certify. Never alter strength or choose cell size to recover a shot count.

### M9 — Reuse graph work in the shared runtime  
**Old T27. Builder: Codex.**

**Work and files.** Cache immutable incidence/adjacency and reuse scratch storage around [fracture.rs:1066](/Users/Yan/github/vikng-dev/overmatch/src/buildings/fracture.rs:1066), [fracture.rs:1275](/Users/Yan/github/vikng-dev/overmatch/src/buildings/fracture.rs:1275), [fracture.rs:1617](/Users/Yan/github/vikng-dev/overmatch/src/buildings/fracture.rs:1617), and [fracture.rs:2100](/Users/Yan/github/vikng-dev/overmatch/src/buildings/fracture.rs:2100). Assess owned groups rather than repeatedly rebuilding the source construction through [breakage.rs:513](/Users/Yan/github/vikng-dev/overmatch/src/buildings/breakage.rs:513).

Preserve the landed no-live-joint return. A group without live joints performs no section/ring assessment; this must not skip wear, holes, or launch bookkeeping.

**Deletes.** Repeated immutable allocation and irrelevant source-graph traversal.

**Proof.** All construction consumers rely on unchanged law verdicts under cracks, wear, splits, support loss, changed presses, force decreases, and spin. Mutants reuse stale topology/load state or omit owned members. Run a temporary paired comparison once and remove the comparison implementation; retain invariant witnesses, not historical outputs.

**Wire/data.** None.

**Dependencies.** Real blocker: M5, so the shared ownership representation is optimized once. No dependency on M6/M7/M8, T17, or group hulls. **Merge order: M9 before M11** to avoid competing `breakage.rs` rewrites.

**Conflicts.** M5/M11 runtime files. Performance observations take the machine lock.

**Ratchet.** No current backlog row names these files.

**Player/eye.** No intended behavioral change; no separate eye gate. Local Mac before/after observations establish implementation effect only. Exact float press comparison at [breakage.rs:1811](/Users/Yan/github/vikng-dev/overmatch/src/buildings/breakage.rs:1811) is not replaced by an unjustified tolerance.

### M10 — Repair the shared debris-carrier boundary immediately before activation  
**Old T8. Builder: Codex.**

**Work and files.** Check peer poses before interpolation at [debris_stress.rs:125](/Users/Yan/github/vikng-dev/overmatch/src/net/debris_stress.rs:125), require the guaranteed publication target at [debris_stress.rs:466](/Users/Yan/github/vikng-dev/overmatch/src/net/debris_stress.rs:466), and move the operator stress construction at [debris_stress.rs:52](/Users/Yan/github/vikng-dev/overmatch/src/net/debris_stress.rs:52) into required provenance-bearing scenario data.

**Deletes.** Invalid-pose admission, silent missing-pose publication, and embedded stress geometry/physical constants.

**Proof.** Replica presentation relies on finite positions and valid rotations before interpolation. Authority relies on every live carrier publishing. Mutants pass malformed poses, silently lose the carrier, or bypass required scenario admission.

**Wire/data.** Preserve valid pose serialization if checked deserialization permits it. Add scenario data. A revision-only change in `protocol.rs` receives the owner’s bookkeeping exception.

**Dependencies.** No intrinsic implementation blocker. **Scheduling/merge order:** land immediately before M11; it is required because M11 removes the tree carrier path at [debris_stress.rs:288](/Users/Yan/github/vikng-dev/overmatch/src/net/debris_stress.rs:288).

**Conflicts.** M11 carrier edits; subsequent stress measurements.

**Ratchet.** `net-03`, `net-06`, `net-16`: [backlog.md:33](/Users/Yan/github/vikng-dev/overmatch/.agents/lenses/backlog.md:33), [backlog.md:36](/Users/Yan/github/vikng-dev/overmatch/.agents/lenses/backlog.md:36), [backlog.md:43](/Users/Yan/github/vikng-dev/overmatch/.agents/lenses/backlog.md:43).

**Player/eye.** Valid motion continues; malformed input is visibly refused. No separate eye gate.

### M11 — Activate trees atomically, delete their system, and prove the original drive  
**Old T24 + tree half of T29; final deletion/lens audit retained here. Builder: Codex. Claude diagnoses geometry or appearance findings.**

This is **five prepared sessions on one branch, merged once, DERIVED sizing**. Session boundaries are clean lane commits, not partially activated releases.

**Prepared sessions**

- **Activation:** map admission, digest, scatter, full spawn state, common detachment, and common view activation.
- **Ledger/carrier consolidation:** shared identities, causes, reconstruction, presentation-time membership, reconnect/reset, and protocol registration.
- **Required ballistics ratchet:** remove tree scheduling/test hooks and discharge the obligations their file carries.
- **Remaining ratchet and deletion audit:** repair affected shared presentation obligations, remove old assets/contracts, and update the stale glossary.
- **Production proof and fixes:** joined collider witness, original UDP drive, observer/reconnect capture, final lenses, and the deletion census.

**Files.** Primary deletion sites are [trees.rs:125](/Users/Yan/github/vikng-dev/overmatch/src/trees.rs:125), [trees.rs:449](/Users/Yan/github/vikng-dev/overmatch/src/trees.rs:449), [trees.rs:582](/Users/Yan/github/vikng-dev/overmatch/src/trees.rs:582), [trees.rs:851](/Users/Yan/github/vikng-dev/overmatch/src/trees.rs:851), [trees.rs:889](/Users/Yan/github/vikng-dev/overmatch/src/trees.rs:889), [trees/motion.rs:105](/Users/Yan/github/vikng-dev/overmatch/src/trees/motion.rs:105), and [net/destruction.rs:22](/Users/Yan/github/vikng-dev/overmatch/src/net/destruction.rs:22).

Consolidate [breakage.rs:350](/Users/Yan/github/vikng-dev/overmatch/src/buildings/breakage.rs:350), [breakage.rs:595](/Users/Yan/github/vikng-dev/overmatch/src/buildings/breakage.rs:595), [breakage.rs:1030](/Users/Yan/github/vikng-dev/overmatch/src/buildings/breakage.rs:1030), [net/walls.rs:103](/Users/Yan/github/vikng-dev/overmatch/src/net/walls.rs:103), [protocol.rs:1142](/Users/Yan/github/vikng-dev/overmatch/src/net/protocol.rs:1142), [map.rs:426](/Users/Yan/github/vikng-dev/overmatch/src/map.rs:426), and [scatter.rs:344](/Users/Yan/github/vikng-dev/overmatch/src/scatter.rs:344).

Remove surviving consumers in [ballistics.rs:1957](/Users/Yan/github/vikng-dev/overmatch/src/ballistics.rs:1957), [ballistics/motion.rs:962](/Users/Yan/github/vikng-dev/overmatch/src/ballistics/motion.rs:962), [debris/launch.rs:255](/Users/Yan/github/vikng-dev/overmatch/src/debris/launch.rs:255), [world.rs:49](/Users/Yan/github/vikng-dev/overmatch/src/world.rs:49), [shot_trace.rs:175](/Users/Yan/github/vikng-dev/overmatch/src/shot_trace.rs:175), [tank/battle.rs:861](/Users/Yan/github/vikng-dev/overmatch/src/tank/battle.rs:861), dev-capture selectors, production-plugin wiring in `lib.rs`, and world-reset/headless/ballistics tests. Replace tree-specific diagnostics with construction-addressed observations.

**Deletes.**

- `segment_hp`, `hp_per_m2`, `fell_on_pool_emptied`, separate segment HP components.
- `load_trunk`, `TrunkLoads`, `structure_of`, tree-specific density/seam overrides, equal-slice fracture construction.
- `TreeState::Down`, `TrunkState`, `TreeSegment`, `TreeParts`, `TreePiece`, `LooseTrunk`, tree-only schedules and collider refit/reparent code.
- The tree ledger and carrier, sliced broken view, tree type fingerprints, obsolete slice certificates and test arms.
- Trunk exclusion in `moving_near`.
- Serialized derived `Fall`/`Swap` events: common reconstruction derives departure/swaps from authoritative causes, preserving today’s disturbance semantics.
- Unused tree simulation files/data after the usage census. Species/generation and foliage remain data, without a tree simulation plugin.

Do not leave compatibility aliases or dead adapters behind. Keep `collider_rehome` if other production callers still require it; its separate guard remains valid.

**Proof.**

- Port the saved [witness.patch](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/diag-tree-crash/evidence/witness.patch) to common production detachment and production belt gathering. Restoring the compound constructor must fail.
- Freed contact hulls are direct centroid-local convex shapes, using the existing common path at [breakage.rs:1900](/Users/Yan/github/vikng-dev/overmatch/src/buildings/breakage.rs:1900) and [breakage.rs:2097](/Users/Yan/github/vikng-dev/overmatch/src/buildings/breakage.rs:2097).
- Run [reproduce.py:15](/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/diag-tree-crash/evidence/reproduce.py:15) with `nearest.ron`. Require real admission/input, actual contact, server survival, and client completion—not merely absence of a panic.
- Add an observing client and reconnect/reset proof. Mutants omit publication, duplicate chunk ownership, retain an old world generation, or hand over at arrival instead of the shared cursor.
- Test conservation analytically. Capture outcomes such as fall direction, shot count, breach size, and speed remain observations.

**Wire/data.** Atomic common ledger/carrier registration, world/instance/chunk identity, revision and both fingerprint updates; activate all construction/view/certificate/LFS changes together. Record the exact pre-cutover parent SHA and content manifest for M13, alongside the planning baseline SHA above.

**Dependencies.** Real blockers: M2–M6, M8, M10. M7 is required for row-12 completion but is not a tree runtime prerequisite. M9→M11 is merge order. `settle-islands` is an existing parallel lane, not a newly invented dependency on survival work.

**Conflicts.** All listed tree/network/runtime consumers; M9 and M10 must finish their conflicting edits first. No concurrent machine capture.

**Ratchet—explicitly budgeted, not waived silently.**

- `ballistics-31` through `ballistics-34`: [backlog.md:97](/Users/Yan/github/vikng-dev/overmatch/.agents/lenses/backlog.md:97), [backlog.md:103](/Users/Yan/github/vikng-dev/overmatch/.agents/lenses/backlog.md:103). Move the crew-line game-unit scale and cosmetic cue age to required labelled data; remove unreachable authority/cosmetic arms and false visibility prose. Parking T19 does **not** avoid these: tree hooks still require editing `ballistics.rs`.
- `net-21`: refuse network rigs whose moving ancestors the existing stream cannot represent. This is admission work, not the dropped remote-rig feature. Registration changes exceed the revision-only exception.
- `net-22`: **owner-directed deferral to #187**. Do not edit `servo.rs`; `load-27`/`load-31` remain outside this lane.
- `view-22`, including its `ServoAngles.turret` obligation: restore the ownership/commitment witnesses, use the existing ancestor traversal API, and clarify the yaw name without changing its timing policy.
- `view-21` is triggered by removing tree lifetime handling from `world.rs`. Close the complete row’s false prose. Its camera edit also brings `view-11`; its font-style edit brings `view-05`. Their files are [camera.rs:171](/Users/Yan/github/vikng-dev/overmatch/src/camera.rs:171), [ui_font.rs:27](/Users/Yan/github/vikng-dev/overmatch/src/ui_font.rs:27), [ui_font.rs:114](/Users/Yan/github/vikng-dev/overmatch/src/ui_font.rs:114), and [ADR-0013:23](/Users/Yan/github/vikng-dev/overmatch/.agents/docs/adr/0013-composable-rig-control.md:23). Required data may generate existing constant interfaces through [build.rs:20](/Users/Yan/github/vikng-dev/overmatch/build.rs:20) to avoid unrelated caller churn; no handwritten duplicate values remain.
- M10’s carrier rows remain closed. Update the obsolete wood definition at [GLOSSARY.md:30](/Users/Yan/github/vikng-dev/overmatch/.agents/GLOSSARY.md:30).

These are attached obligations within the cutover lane; none receives a separate lower-priority lane.

**Player/eye.** Trees now receive the same cracking, wear, contact, fall, continued-splitting, and presentation path as walls. Final sitting: original drive, subsequent traversal, fir fall, exposed wood/foliage, roof fall, and agreement between clients. This is the next-build gate.

### Row-13 tasks retained for later execution

These tasks are startable reservations, not blockers inserted ahead of row 12.

| Task | Work, files, deletion | Proof; wire/data; builder | Dependencies, conflicts, ratchet | Player and owner gate |
|---|---|---|---|---|
| **M12 — Group collision hull lever, old T26** | Separate disturbance/view replacement from contact replacement where appropriate; try certified same-material group proxies at [breakage.rs:1844](/Users/Yan/github/vikng-dev/overmatch/src/buildings/breakage.rs:1844), [breakage.rs:1900](/Users/Yan/github/vikng-dev/overmatch/src/buildings/breakage.rs:1900), [breakage.rs:2050](/Users/Yan/github/vikng-dev/overmatch/src/buildings/breakage.rs:2050). Delete losing experimental arms. | Players rely on complete occupied coverage without gap bridging. Mutants omit chunks or enclose real voids. Derive a geometric allowance before admitting approximate hulls; retain exact ballistic solids. Data: certificates/representation policy; wire changes only if carrier shape changes. **Claude.** | Real blocker: M11 common ownership. No solid-survival dependency. Conflicts with runtime/view edits and measurements. No current core-file backlog row; protocol changes would carry its remaining obligations. | Potentially cheaper contact geometry. Reject a candidate that stops rounds on air or visibly misrepresents rolling/traversal; price the exact alternative. |
| **M13 — Restore and run forest comparison, old T28** | **Recreate the runner first:** `scripts/perf/run-forest-cost.sh` is absent at HEAD; the old task cannot invoke it. Use [headless_test.rs:3116](/Users/Yan/github/vikng-dev/overmatch/src/headless_test.rs:3116) as an existing lifecycle-test location, replacing its obsolete tree assumptions through M11. Prepare runner/validity witnesses, then measure in a separate session. Delete temporary comparison arms afterward. | Equivalent initial scene/input and complete observation windows are the invariants. Mutants omit drawn frames or change the initial population. Record rendering, entities, colliders, memory, disturbance and achieved ticks separately. No wire/data change. **Codex.** | Real blocker: M11 and its recorded baseline. M12 is optional; repeat after it if retained. Machine-lock conflict with M14/M15. No current row names the new runner; inherit any rows of measurement files actually changed. | Forest visuals/collision remain intact. No new appearance decision; this supplies #150’s cost evidence. |
| **M14 — Hosted building ram proof, old T29 building half** | Adapt the real input replay to a building and use [contact_transfer.rs:234](/Users/Yan/github/vikng-dev/overmatch/src/contact_transfer.rs:234), [breakage.rs:684](/Users/Yan/github/vikng-dev/overmatch/src/buildings/breakage.rs:684), and [net/walls.rs:103](/Users/Yan/github/vikng-dev/overmatch/src/net/walls.rs:103). Preserve reusable scenario data; delete temporary instrumentation. | Production registration→contact→laws→launch→later traversal→carrier→observer/reconnect must complete. Mutants belong in integration tests; captures report outcomes. Scenario data only. **Codex; Claude for visual diagnosis.** | Real blocker: common runtime/cutover. No T6 or T19 dependency. Repeat affected proof if M12 changes collision representation. Measurement-lock conflicts. Existing file rows remain closed; no new ratchet-only lane. | Owner judges momentum, clipping, and shared-time presentation alongside row-13 scale results. |
| **M15 — Price the completed system on the hosted allocation, old T30** | Adapt [collapse.sh:2](/Users/Yan/github/vikng-dev/overmatch/scripts/perf/collapse.sh:2) and [collapse.py:102](/Users/Yan/github/vikng-dev/overmatch/scripts/perf/collapse.py:102); today that runner is tankless and starts a local debug binary. Use the hosted workflow at [session.sh:17](/Users/Yan/github/vikng-dev/overmatch/scripts/edgegap/session.sh:17). Delete temporary counters and rejected optimization arms. | Operators rely on verified allocation, complete windows, and actual production workload. Include native collapse, independent diagnostic piles, retained rubble, vehicles remaining in contact, and concurrent destruction. Record assessments, allocations, hull/mesh spikes, active body-time, physics, send time, memory, throttling and achieved rate. No historical timing becomes a law test. **Codex.** | Real prerequisites for the final result: completed `settle-islands`, M9, M11, M13/M14, and any retained M12 candidate. Measurement runs serialize; scenario/instrument preparation can overlap. If [server_host.rs:72](/Users/Yan/github/vikng-dev/overmatch/src/net/server_host.rs:72) changes, close `net-08`. | Hosted **2 vCPU, DERIVED requirement**, verified from quota/cpuset/startup reporting—not merely a compute-thread setting. The current tick budget is **15.625 ms, DERIVED from [server.rs:46](/Users/Yan/github/vikng-dev/overmatch/src/net/server.rs:46)**. Owner sees the measured concurrency/cost curve and visible concessions separately. |

## What moved where

| Destination | Old tasks | Decision and remaining obligation |
|---|---|---|
| **“Damage method: a simpler model, after we see it”** | **T1, T14–T17** | Park the experiment, solid-section reader, authoritative removal union, axis/load changes, service loading, and survival switch. Keep today’s cracking, volume wear, and drawing-hole cap. No cut-independence claim survives in row 12. |
| Same parked row | **T10, T11** | Park crushing/rear-face simplification and the reduced material schema. Keep today’s material inputs and independent shear facts. The review’s shear stills, rear-face visibility experiment, and material-distinction tests belong here when these changes resume. |
| Same parked row | **T5, T6** | Keep today’s contact counting, solver-step peak-force approximation, and landed pushing-work debit. Future work must report secondary pieces per collapse, not only vehicle ram outcomes. The review’s momentum/work-only alternative remains a design candidate, not row-12 code. |
| Same parked row | **T19** | Keep current entrance/chord allocation for the move. The correct future producer contract is [DeliveredLine:1157](/Users/Yan/github/vikng-dev/overmatch/src/ballistics.rs:1157), not the old unrelated citation. Ownership/conservation work remains explicitly deferred. |
| Same parked row | **T13** | Runtime face polygons, named cuts, and survival axes are unnecessary for today’s laws. Tool-local polygons and generic convex source shapes are supplied by M3/M4. |
| Naming work, folded into the later damage-method change | **T12** | **Park the broad rename.** Existing names are imperfect, but renaming their many consumers now buys no cutover behavior and expands ratchet conflicts. In this document, “construction” means the assembly; code’s `Structure` still names that graph. No fictitious existing glossary entry is cited. |
| **Row 13** | **T26, T28, T29 building half, T30 pricing** | M12–M15. Group hulls are a measured lever, not an assumed prerequisite. Tree ram proof remains M11. Final row-12 deletion/lens review stays M11. |
| **Dropped** | **T9** | The owner’s per-material spall-width ruling stands. No global-width migration. |
| **Dropped** | **T3** | No kept task needs to edit `field_casts.rs` or its pinned file. `ballistics-28` remains ratchet debt, not a separate lane. |
| **Dropped as a task** | **T7** | Revision-only edits receive the bookkeeping exception. M11’s actual registration changes still discharge applicable rows; `net-22` waits on #187. |
| **Already running** | **T4** | Name **`settle-islands`** and integrate its reviewed result. Do not plan another settling lane or ship the report’s guard-only experiment as proven. |
| **Kept independently** | **T2, T27** | M1 starts without T1. M9 follows the shared runtime, not survival or group hulls. |

When survival work resumes, apply the review’s missing cost gate: section reads must not repeatedly scan removal history. Establish bounded per-plane retained state and convergence/cost evidence before putting that reader on recurring load paths.

The field-adapter design remains provisional and post-playtest: no exposed-face field table, carried-flight fragment replacement, cavity replacement, or plate-work implementation enters row 12.

The review also identified small-arms-through-brick and delay-fuze behavior. Preserve these as separate follow-up defects in the game’s damage model, with the local design observations explicitly distinguished from production reproduction. They do not justify changing the move’s damage method.

The review’s missing-input objection has changed: the merged designs and crash witness are tracked at this HEAD. No kept task depends on a temporary Python experiment. Keep the current tracked design home; do not create a competing narrative by copying it elsewhere.

## Estimate

**Row 12: DERIVED estimate of 31–41 agent-hours and 22–30 elapsed working hours**, excluding owner response time and remaining work already underway in `settle-islands`.

Assumptions:

- Implementation and targeted checks: **22–28 agent-hours, DERIVED**.
- Independent lens review at the schema boundary, prepared-content boundary, and final cutover: **3–5 agent-hours, DERIVED**.
- Fix allowance: **6–8 agent-hours, DERIVED**, covering a fix round after each boundary and an additional geometry/cutover correction.
- M11 occupies **five prepared sessions, DERIVED**, on the same branch. Geometry/schema and cutter tasks also span clean sessions where needed.
- Warm bays, existing dependencies, and the serialized compile queue are available. No estimate assumes parallel compiles.
- Owner eyes are batched into **two sittings, DERIVED**; a rejected appearance adds re-bake/re-capture time rather than material tuning.

**Row 13: an additional 16–24 agent-hours, DERIVED**, including runner reconstruction, hosted measurements, analysis, review, and fix allowance. Hosting wait and an unsuccessful capacity gate are excluded; a failed gate needs a new decision based on its evidence.

## Issue disposition

| Issue | Closure condition |
|---|---|
| **#150** | **After M13**, with M6/M11’s “one collider until disturbed” contract and corrected forest comparison. The structural change alone does not close its measurement acceptance. |
| **#232** | Tree proof completes in M11; **close after M14** supplies the building half of the production server/client proof. |
| **#188** | Implementation is already recorded as landed. Close administratively after M14 verifies its surviving building interaction contract. |
| **#237** | **After M15** supplies outstanding implementation pricing. |
| **#264** | Keep open; its solid-survival direction/experiment is parked. Record the narrowed scope without claiming that experiment passed. |
| **#268** | Keep open with T10/T11 and their visibility evidence. |
| **#256**, and the related contact-duration demand in **#245** | Keep open; T5/T6 are parked. Ramming through today’s method does not satisfy timestep-independent counting. |
| **#250** | Row-13 hosted capacity gate. Use the recorded requirement for a full Battle with destruction; the review’s **30-tank workload is DERIVED from that recorded requirement**, not a new measurement. Do not substitute a tankless collapse run. |
| **#187** | Remains the owner of the angle-presentation decision and `net-22`; row 12 makes no timing-policy decision. |
| **#106** | Remains open for its wider destruction destination and additional assets. |
| **#189** | Remains open for independently supported empirical validation; plausible wood/wall captures do not discharge it. |
| **#119** | Remains deferred until play establishes a mature enough contract. |
| Previously completed foundations | Preserve their completed status and relevant regression coverage. This plan does not reopen #120, #140, #141, #149, #122, #123, #193, #210, #233, #235, #236, #249, or closed research #269. |

No issues are filed, closed, or commented on by this read-only plan.

## Risks and kill criteria

- **Cut-dependent toughness is accepted scope, not solved.** Today’s joints and wear make the cut affect fir survival. Record cut settings, aim, shot counts, mass, and outcomes. Reject attempts to recover a count by changing cell size or strength. If appearance is acceptable but toughness is unacceptable, that evidence starts the parked damage-method row.
- **Material-input changes must be explicit.** Common `Wood` replaces tree-only seam/density overrides. Reject hidden fresh-mass paths, per-tree strength overrides, or source labels claiming an assumed transfer was measured.
- **Geometry must be sound before activation.** Failed closure, interface, convexity, index, mass/inertia, anchorage, or certificate checks block M11. Fix geometry/data; do not weaken class-invariant admission.
- **The crash proof must join the real producer and reader.** A hand-built convex fixture alone is insufficient. Compound mutation must fail the joined witness; the original drive must reach contact and complete.
- **Presentation must remain coherent.** Duplicate/missing chunks, foliage attached to departed matter, arrival-time membership changes, or broken reconnect/reset reconstruction block the cutover.
- **Graph reuse must preserve the current method.** Stale topology, ignored force decreases, skipped moving-frame loads, or altered law verdicts reject M9. No tolerance or reduced assessment policy is introduced as an optimization.
- **Automatic settlement is not forced freezing.** The slowdown report explicitly does not prove its guard-only candidate in production. Preserve impact and support-loss waking; do not add a debris-only sleep rule.
- **Capacity remains unproven until row 13.** Local Mac comparisons establish before/after effects only. Hosted results require the verified allocation, complete windows, vehicles and native construction work. A failed scale gate triggers measured lever choices; it does not retroactively turn row 12 into a survival rewrite.
- **Group hulls can lose.** If a candidate bridges air or visibly harms contact behavior, retain exact chunk solids, delete the candidate, and report their cost.
- **No hidden prerequisites.** Parked service pressure, section strength, removal history, cut addresses, face polygons, contact duration, and new edge allocation are not silently required by a kept task. The current-method deferrals are binding owner scope decisions.