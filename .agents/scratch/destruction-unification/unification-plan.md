Subject: the video game’s environment destruction model; game state only.

**Verdict: ready to queue implementation under the decisions below. No owner answer blocks it.** The first task is a falsification experiment, not a promise that the design will survive testing. Empirical blast validation remains incomplete; that keeps #189 open, but does not block this unification.

The designs have drifted in both directions. Freed trunks already became ballistic solids in `7ee6831`, supplementing buildings’ separated ballistic/contact hulls in `59555297` (`src/trees/motion.rs:175`, `src/buildings/breakage.rs:2097`): **spine step 1 is already landed**. Pushing work also landed in `ff37724` and its debit correction `d3ffc0a`; `Origin::work_share` confirms it (`src/buildings/fracture.rs:775`). The physical-duration gap remains at `src/buildings/fracture.rs:947`. Survival still reads face discs, wear still subtracts a pool, and trees still bypass `strike` (`src/buildings/fracture.rs:668`, `:2940`; `src/trees.rs:851`). Those code facts do not override the accepted solid-survival ruling.

All citations below are at **HEAD `14d1ec819c260ccab83e324afe0c56ab0e5957b3`**. Numerical defaults, tolerances and estimates proposed below are **DERIVED engineering choices**, not measurements; reported observations are labelled **MEASURED**. Design identifiers and source line numbers are references, not quantities.

The accounting is carried once in each task’s **Absorbs** and **Deletes** fields. “Survival lanes” means the six lanes in their listed order. Every implementation session has a DERIVED working budget below ninety minutes, including its targeted checks; prerequisite work makes the final cutover mechanical.

### T1. Try to disprove solid survival before changing the game
Does: Reuse the offline fir experiment and shipped wall geometry to exercise actual convex cells, section sampling and existing load laws in tools-only tests. Compare cut seeds, refinement, stretch and aimed openings; print outcomes and render the first broken-fir comparison. Absorbs: spine step 0, survival’s hidden experiment, material-proxy P0.
Files: `tools/src/buildings/geom.rs:12`, `tools/src/buildings/fracture.rs:81`, `src/buildings/fracture.rs:2886`. Deletes: experimental losing arms before reporting; no runtime selector.
Proof: Players rely on cuts not secretly setting toughness: identical solid/removal/load inputs must give convergent section readings despite tessellation. Mutants read seam discs or chunk length as durability; compare analytical beam sections and the declared seed-spread decision.
Wire and data: none shipped. Builder: Claude, for section geometry and visual comparison.
Blocked by: none. Conflicts with: T13, T20.
Player sees: nothing yet. Eye gate: first fir’s fracture language and whether openings explain the break; downstream cutter/view work may change.

### T2. Remove material fields no law reads
Does: Delete unread toughness and reference strain-rate fields, retaining their useful source information beside facts derived from them. Repair the registry tests that pin reference outputs or reject an admitted zero resistance. Absorbs: P1; backlog load-11 and load-12.
Files: `src/substances.rs:157`, `:188`, `:1070`, `:1789`; `assets/materials/materials.ron:398`. Deletes: `toughness`, `compressive_dif_strain_rate`, parser requirements and exclusive test arms.
Proof: Asset authors rely on admission matching the consumed type. Missing consumed facts fail at admission; removing an unread field does not alter a law’s inputs; mutants retain the obsolete requirement or reject valid zero resistance.
Wire and data: material text and content digest change; no wire shape change. Builder: Codex.
Blocked by: T1. Conflicts with: T5, T9–T11.
Player sees: no intended change. Eye gate: none.

### T3. Retire the stored field-results oracle
Does: Keep the field-cast suite and its useful observations, but remove agreement with a repeatedly blessed historical output file as its success condition. Preserve input identity only where client derivation requires it, and report cross-platform comparisons as comparisons. Absorbs: backlog ballistics-28.
Files: `src/ballistics/field_casts.rs:779`, `src/ballistics/field_casts.pinned:1`. Deletes: stored output oracle, `PINNED_TOTAL`, blessing path and obsolete pin prose.
Proof: Developers rely on law changes exposing invariant failures rather than demanding new goldens. Retain conservation, geometry and required derivation witnesses; a mutant violating one must fail independently of an earlier run.
Wire and data: none. Builder: Codex.
Blocked by: none. Conflicts with: none of this queue’s other file owners.
Player sees: nothing. Eye gate: none; this is a testing-standard decision.

### T4. Let the library settle resting islands
Does: Exercise Avian’s existing sleep settings first, then correct its generic new-contact wake boundary where no awake non-static participant exists. Keep ordinary impact and support-removal waking; introduce no debris timer or parallel settling system. Absorbs: folded-in slowdown, library-waking candidate.
Files: `vendor/avian3d-0.7.0-child-pose/src/collision/narrow_phase/system_param.rs:240`, `src/debris.rs:185`, `scripts/perf/collapse.sh:1`. Deletes: any experimental force-sleep or alternate wake arm.
Proof: Physics users rely on sleepers staying asleep without a wake source and waking on real impact/support loss. Mutants wake sleeping/static pairs or suppress incoming movers; measure **automatic** settlement with the guard alone, without forced sleep, on native and independent piles.
Wire and data: none unless measured evidence justifies a required library-setting datum. Builder: Codex.
Blocked by: none. Conflicts with: T30’s measurements.
Player sees: resting debris should stop consuming repeated work while remaining movable. Eye gate: only if changing library thresholds would visibly truncate motion.

### T5. Give contacts physical pulse inputs
Does: Admit a generic compliant-contact proxy from construction geometry, effective inertia and required stiffness data. Choose a series-compliance pulse: each side contributes axial compliance from its occupied geometry; unidentified stiffness uses one explicit global `StandIn`, initially projected from the existing reference material, rather than a per-object adjustment. Absorbs: #256’s missing physical input.
Files: `src/contact_transfer.rs:74`, `:130`, `src/physical_laws.rs:11`, `src/buildings/fracture.rs:739`. Deletes: any proposed fixed contact-time constant or body-kind stiffness branch.
Proof: All contact consumers rely on finite, positive pulse inputs established once at the edge. Mutants omit one side’s compliance, duplicate effective mass per point, or admit nonrepresentable simulation values.
Wire and data: required contact-proxy data; no wire change. Builder: Codex.
Blocked by: T2. Conflicts with: T6, T9–T11.
Player sees: nothing until integration. Eye gate: none; the proxy’s applicability is explicit.

### T6. Count a blow independently of the physics step
Does: Aggregate each contact’s physical pulse without multiplying its budget across points or solver subdivisions. Use the DERIVED compliant-pulse duration and impulse-normalized peak for strength, retain actual transferred momentum for launch, and keep continuous presses separate; preserve the pushing-work term already landed. Absorbs: remaining #256 implementation.
Files: `src/contact_transfer.rs:261`, `src/buildings/fracture.rs:947`, `src/buildings/breakage.rs:741`; current tree consumer `src/trees.rs:980`. Deletes: solver-step force bounds and their historical rate-dependent assertions.
Proof: Players rely on the same physical blow having the same verdict under timestep subdivision. Mutants divide by the step, multiply contact points, omit pushing work or create momentum; analytical sustained-topple and pulse fixtures supply expectations.
Wire and data: none. Builder: Codex.
Blocked by: T5. Conflicts with: T11–T19, T24, T27.
Player sees: slow pushes and impacts follow the stated force/work limits. Eye gate: report changed ram outcomes; do not tune them back.

### T7. Close the protocol file’s existing ratchet debt
Does: Publish every moving aim ancestor required by an admitted rig, using stable construction indices, and present non-owner angles at the shared interpolation cursor. This closes the protocol-file obligations before destruction changes its registration; the owner’s separate local-input presentation question remains outside this task. Absorbs: backlog net-21 and net-22.
Files: `src/net/protocol.rs:411`, `:884`, `:923`, `:1130`. Deletes: the single-yaw-only remote contract and arrival-time remote application.
Proof: Observers rely on all moving ancestors and hulls sharing one presented time. Mutants omit an ancestor or apply arrival-time angles; use an inline multi-ancestor rig and production registration.
Wire and data: PROTOCOL_REV bump and wire fingerprints. Builder: Codex.
Blocked by: none. Conflicts with: T15, T24.
Player sees: remote aim assemblies follow their complete authoritative pose. Eye gate: a short turning-hull capture checks temporal coherence.

### T8. Make the shared debris carrier boundary sound
Does: Parse finite positions and valid rotations before presentation, require each live body’s carrier pose, and move the stress scenario’s authored construction into required labelled data. Keep this carrier suitable for the common destruction path. Absorbs: backlog net-03, net-06 and net-16.
Files: `src/net/debris_stress.rs:52`, `:125`, `:232`, `:468`. Deletes: embedded scenario facts and silent missing-pose publication.
Proof: Clients rely on malformed peer poses being visibly refused before interpolation; authority relies on every live carrier publishing. Mutants pass NaNs, accept invalid rotations or silently skip a missing guaranteed pose.
Wire and data: scenario RON; checked ingress may retain existing serialization. Builder: Codex.
Blocked by: none. Conflicts with: T24, T30.
Player sees: valid debris behaves as before; bad records are refused visibly. Eye gate: none.

### T9. Give debris spread one global owner
Does: Move the common debris half-angle out of individual material rows into required field-shape data. Existing transport types may carry the resolved value, but materials cannot author competing copies; keep the crew-dose scale outside this edit. Absorbs: P2.
Files: `src/substances.rs:279`, `:381`, `:597`, `src/ballistics/spall_cone.rs:28`. Deletes: per-row half-angle fields and their admission/test alternatives.
Proof: Field consumers rely on every emitter reading the same admitted shape datum. Mutants retain a material override or bypass global admission; compare the old and new resolved inputs once.
Wire and data: new global shape datum and material migration; no wire shape change. Builder: Codex.
Blocked by: T2, T3. Conflicts with: T10, T11.
Player sees: no intended spread change. Eye gate: none.

### T10. Reduce crushing and rear-face inputs
Does: Replace per-material crushing and rear-face parameters with the material design’s explicit global proxies. Run its held-input visibility matrix; retain the rear-face law only if its distinct result survives outside already removed matter, otherwise delete that law in this change. Absorbs: P3 and material call 5.
Files: `src/buildings/fracture.rs:128`, `:144`, `:3796`, `src/substances.rs:188`. Deletes: superseded per-row blast fields, plus the rear-face path if the experiment rejects it.
Proof: The retained laws must satisfy their independently derived pressure, impulse and momentum relationships. Compare before/after once in the report, including brick; no test requires brick’s previous outcome.
Wire and data: required global proxies and registry migration. Builder: Claude, because the visibility disposition requires visual judgement.
Blocked by: T1, T6, T9. Conflicts with: T11–T19, T27.
Player sees: material differences remain only where supported by the reduced model. Eye gate: exposed dishes/scabs and distinguishable material response.

### T11. Admit the final material row
Does: Introduce resistance, density, friction, bond strength, bond work and section strength, with a checked optional bond. Derive shear from the selected global ratios; use one shared wood row initially, with section strength read once, and document illustrative future rows without shipping unused materials. Absorbs: P4, P5 and spine step 4, as superseded by solid survival.
Files: `src/substances.rs:157`, `src/buildings/fracture.rs:80`, `assets/trees/wood.ron:47`. Deletes: independently authored shear pairs, along-grain face stamping and the proposed Hankinson/per-face-strength path.
Proof: Bakers and laws rely on cutting requiring a complete bond and every bonded solid having section strength. Mutants admit partial bonds, derive strength from stretch or omit section strength.
Wire and data: required registry schema; material/content digest changes. Builder: Codex.
Blocked by: T10. Conflicts with: T12–T19, T24.
Player sees: roofs may change response; species do not acquire separate toughness dials. Eye gate: compare the roof and fir using the common wood row.

### T12. Name the graph and its pieces distinctly
Does: Reserve **structure** for one material piece subdivided into chunks, matching the owner’s proposal; call the connected multi-material assembly a **construction**. Rename the current graph accordingly, use `ChunkState`, `Matter` and `Breakable`, and reserve wire consolidation for cutover. Absorbs: spine step 2.
Files: `src/buildings/fracture.rs:307`, `:612`, `src/buildings/breakage.rs:543`, `src/buildings/schema.rs:50`, `.agents/GLOSSARY.md:23`. Deletes: misleading internal wall/building-specific names, not live functionality.
Proof: A refactor compares before/after once; retain interface and production-composition tests, with no new emergent-output golden. A valid multi-material construction must not be refused as a mono-material structure.
Wire and data: rename asset types coherently; bump protocol only if serialized names actually change. Builder: Codex.
Blocked by: T11. Conflicts with: T13–T27.
Player sees: no intended change. Eye gate: none.

### T13. Bake real faces and cut addresses
Does: Extend the bake with face polygons, structure axes and named baked cuts, and re-bake the house atomically with its reader. Populate the common geometry contract for every current producer so subsequent law work needs no object-kind branch. Absorbs: spine step 3; backlog load-13.
Files: `src/buildings/schema.rs:221`, `tools/src/buildings/contact.rs:49`, `tools/src/buildings.rs:134`, `src/buildings/fracture.rs:284`. Deletes: area/centroid-only face representation and asset-specific stochastic assertions.
Proof: Law consumers rely on matching face copies, area/centroid agreement and cuts naming their crossed faces. Mutants reverse winding, omit a crossing face or require cracks when authored probability permits none.
Wire and data: bake-format bump and same-landing house re-bake; no LFS geometry requirement. Builder: Claude, for polygon orientation and cut geometry.
Blocked by: T12. Conflicts with: T14, T18, T20, T21.
Player sees: walls continue working with the new format. Eye gate: inspect the re-baked house’s interfaces.

### T14. Read a surviving section from points
Does: Implement section area, centroid, second moments and Navier stress on the original solid minus removals. Keep section sampling independent of the fracture cutter; derive resolution from convergence rather than a fixed sample count. Absorbs: survival lane 1.
Files: `src/buildings/fracture.rs:2886`, `:2940`, `:2986`. Deletes: disc-own-moment and disc-rim substitutions when the common reader replaces them.
Proof: Structure users rely on analytical rectangle/circle readings, translation invariance and removal-aware area. Mutants use face discs or restore removed points; do not assert that every removal monotonically lowers bending capacity after recentering.
Wire and data: none. Builder: Codex.
Blocked by: T13. Conflicts with: T16, T17, T27.
Player sees: no separate feature until integration. Eye gate: none.

### T15. Record removed solid once
Does: Make channels, dishes and departed chunks update one authoritative removal union. Repeating an opening adds no removal; the bounded drawing table becomes a projection of that state and cannot discard structural history. Absorbs: survival lane 2.
Files: `src/buildings/breakage.rs:321`, `:489`, `:1670`, `:1769`, `src/net/protocol.rs:278`. Deletes: additive overlapping wear as structural truth and the drawing cap’s influence on survival.
Proof: Players and replicas rely on idempotent removal and identical reconstruction. Mutants count overlapping holes twice or stop recording at the view cap; test duplicate, overlapping, oblique and departed-solid cases.
Wire and data: PROTOCOL_REV bump for the removal/reconstruction contract; retain causes, not derived verdicts. Builder: Codex.
Blocked by: T7, T13. Conflicts with: T17, T19, T24, T26.
Player sees: repeated fire into the same opening no longer weakens untouched material. Eye gate: confirm drawn openings explain the surviving ligament.

### T16. Read loads on the solid’s axis
Does: Apply the admitted section strength and authored axis to the common section reader, including the specified two-support moment. Preserve the declared limitation for indeterminate shared supports; do not implement stiffness distribution. Absorbs: survival lane 3.
Files: `src/buildings/schema.rs:136`, `src/buildings/fracture.rs:2854`, `:3011`, `:2190`. Deletes: seam orientation acting as whole-section strength.
Proof: Owners of supported parts rely on cantilever and simply supported beam equations under controlled loads. Mutants use cut normals, omit a support or count whole-section strength per face.
Wire and data: required axis and section-strength data, baked into current assets. Builder: Codex.
Blocked by: T11, T14. Conflicts with: T17, T18, T27.
Player sees: support and remaining section determine survival. Eye gate: unsupported roof spans and wall corners.

### T17. Switch survival and remove face cracking
Does: Integrate solid sections, removal state and service loading into the production law sequence. Use the accepted DERIVED default service pressure of **600 Pa**, explicitly a wind `StandIn`, then delete the face-cavity crack route and capture the wall response. Absorbs: survival lanes 4 and 5.
Files: `src/buildings/fracture.rs:1026`, `:1143`, `:1709`, `src/buildings/breakage.rs:1762`. Deletes: `crack`, its Lamé/disc-only helpers, old cut-based survival and superseded test arms; retain bond work for opening and rings.
Proof: Players rely on an unchanged opening not accruing damage, a broken state never resurrecting, and unsupported matter leaving. Mutants omit service load, retain face cracking or let view limits decide survival.
Wire and data: required service-load datum and content fingerprint; no additional wire payload. Builder: Codex.
Blocked by: T6, T15, T16. Conflicts with: T18, T19, T24, T27.
Player sees: holes and remaining material determine whether walls, roofs and current trunks hold. Eye gate: report shot counts, falls and wrong holds without preserving old numbers.

### T18. Make every anchor name its attachment
Does: Carry attachment kinds through baked anchors and resolve them through the shared list, including root plates. Validate converted simulation values at admission; retain the current declared zero-work root-plate approximation until the post-playtest field work. Absorbs: spine step 5; backlog destruction-04.
Files: `src/buildings/schema.rs:247`, `src/buildings/breakage.rs:199`, `src/buildings/attachments.rs:81`, `:119`. Deletes: anchors implicitly borrowing the chunk’s material seam.
Proof: Construction authors rely on named anchorage and representable strengths/work. Mutants ignore the declared kind or admit finite doubles that become zero/infinity in simulation.
Wire and data: bake-format bump and atomic re-bake; attachment data migration. Builder: Codex.
Blocked by: T13, T17. Conflicts with: T20, T21, T24.
Player sees: anchorage follows construction data. Eye gate: static shove can uproot or snap according to competing capacities.

### T19. Allocate line work by the disc’s actual contacts
Does: Allocate entrance work/momentum by weighted first-contact samples on each chunk; allocate travel work by their weighted occupied chords. Preserve total work, impulse and moment, using recorded contact ownership for collapsed chords; a missing required owner is a producer-contract failure, never a nearest-chunk guess. Absorbs: folded-in allocation gap.
Files: `src/buildings/breakage.rs:1080`, `:1126`, `:1165`, `src/ballistics.rs:2214`. Deletes: all entrance work going to a minimum-distance winner and `nearest_chunk` as a line fallback.
Proof: Players rely on edge hits loading what the disc met. Mutants choose the axis chunk, multiply work across overlaps or lose torque; use rim-only, shared-edge, zero-length and reordered-chunk fixtures.
Wire and data: internal delivered-contact contract; bump protocol if any serialized record changes. Builder: Codex.
Blocked by: T15, T17. Conflicts with: T22, T24, T27.
Player sees: edge hits distribute their effect consistently. Eye gate: inspect rim-only wall and trunk strikes.

### T20. Make the cutter a generic command
Does: Add convex-polyhedron clipping, polygon contact and data-selected `Whole`, `Courses` and `Cells` rules in the separate tools package. Resolve material keys from the registry; an inline concrete fixture demonstrates adding a material without Rust branching. Absorbs: spine step 6, P6; backlog load-06.
Files: `tools/Cargo.toml:5`, `tools/src/buildings/geom.rs:12`, `tools/src/buildings/house.rs:29`, `:45`, `tools/src/buildings/contact.rs:76`. Deletes: `Material::{Brick,Wood}` and `Cutter::{Masonry,Timber}` as closed asset-specific selectors.
Proof: Asset authors rely on convex cells tiling the source and interfaces following declared construction. Mutants hard-code material identities, lose clipped volume or select attachments from cutter type.
Wire and data: tools command/spec migration; re-bake affected house output with its reader. Builder: Claude, for three-dimensional clipping.
Blocked by: T13, T18. Conflicts with: T21, T25.
Player sees: existing content continues through generic bake rules. Eye gate: inspect clipped interfaces, especially roof contacts.

### T21. Certify and bake the fir
Does: Bake the fir from construction profile data, with look-only stretch and the common chunk table. Certify volume closure, matching faces, valid cuts, anchor reach, nondegenerate hulls and the DERIVED maximum chunk contribution of **30%** at assessed load-bearing sections. Absorbs: spine step 7, survival lane 6.
Files: `scripts/prop/plan.py:18`, `scripts/prop/build.py:396`, `scripts/prop/test_prop.py:225`, `src/buildings/schema.rs:343`. Deletes: slice-count-based fracture certification; closes backlog dev-18 by checking decoded geometry rather than a ring-count literal.
Proof: Runtime consumers rely on certified geometry independent of asset identity. Mutants leave gaps, exceed the section-resolution limit or hide missing anchors; repeated bakes must produce canonical identical bytes.
Wire and data: new construction bake/certificate; stage replacement view artifacts for atomic activation. Builder: Claude, for geometric certification.
Blocked by: T1, T20. Conflicts with: T23–T25.
Player sees: nothing changes before activation. Eye gate: first certified bake; stretch/pattern may change, strength may not be tuned to the cutter.

### T22. Prepare one runtime for arbitrary constructions
Does: Make the building runtime consume the common construction table, with complete mutable destruction state built synchronously at spawn and immutable geometry shared. Generalize load routing, identity and group ownership so tree activation requires no tree-specific law; keep the present field sampler unchanged. Absorbs: spawn-state decision and cutover preparation.
Files: `src/buildings.rs:220`, `src/buildings/breakage.rs:493`, `:552`, `:684`, `:2050`. Deletes: lazy `Live` construction and building-only assumptions in the common runtime.
Proof: Every spawned construction relies on complete asset-independent state, including a headless composition with no view assets. Mutants initialize on first hit, consult a GLB or reset removals when a group splits.
Wire and data: common asset references and content digest; no new field implementation. Builder: Codex.
Blocked by: T18–T21. Conflicts with: T23–T27.
Player sees: walls retain normal behaviour through the common runtime. Eye gate: none beyond a wall continuity capture.

### T23. Prepare the common wood view
Does: Generate bark outer faces, inner wood faces and branch-card ownership from the bake, using the same presented group membership as walls. Preserve intact instancing and move touched appearance/range choices into labelled presentation data; stage activation with cutover. Absorbs: spine step 10.
Files: `src/trees/view.rs:395`, `:441`, `:535`, `src/buildings/view.rs:21`, `scripts/prop/gen_tree.py:156`. Deletes: superseded experimental view arms; the active sliced view is removed at T24.
Proof: Players rely on each card base belonging to exactly one live chunk and every visible chunk having one owner at the cursor. Mutants duplicate cards or switch membership on packet arrival.
Wire and data: generated view assets, certificate and LFS changes, activated together at T24. Builder: Claude, for surface frames and visual judgement.
Blocked by: T21, T22. Conflicts with: T24–T26.
Player sees: the prepared preview shows bark, exposed wood and attached foliage. Eye gate: crystal-like breaks trigger displaced visual faces, then a grain-plane cutter if still necessary.

### T24. Move trees over atomically and remove their exceptions
Does: Activate the certified tree construction, common laws, common view and one cause ledger/carrier contract together. Derive swaps and falling membership from causes; build freed contact hulls directly as centroid-local convex shapes, making the belt’s existing shape contract hold. Absorbs: spine steps 8 and 9; folded-in tree crash.
Files: `src/trees.rs:125`, `:449`, `:582`, `:650`, `:851`, `:889`; `src/trees/motion.rs:18`, `:41`, `:105`; `src/net/destruction.rs:22`; `src/buildings/breakage.rs:350`, `:1030`.
Deletes: `segment_hp`, `hp_per_m2`, `fell_on_pool_emptied`, `load_trunk` and its missing `strike`, `TrunkLoads`, `structure_of`, equal-slice fracture, `TreeState::Down`, `TreePiece`, `TrunkState`, `TreeSegment`, `TreeParts`, old tree ledger, sliced broken view, `collider_density`, re-layering/reparent path, trunk exclusion, and serialized `WallEvent::Fall`/`Swap`.
Proof: Port `evidence/witness.patch` to production common detachment and belt gathering, retaining admission assertions; the compound-shape mutant must fail. Run the original `evidence/reproduce.py` with `nearest.ron`, preserving real UDP admission/input and proving actual contact, server survival and client completion.
Wire and data: PROTOCOL_REV bump, registration/type fingerprints, common causes and chunk-list carriers; atomically activate bake, view/LFS artifacts and certificates. Builder: Codex; geometry and views are already prepared.
Blocked by: T7, T8, T19, T22, T23. Conflicts with: T25–T30.
Player sees: trees fracture, fall, split and rest through the same system as walls. Eye gate: replay the original drive and inspect the first complete fall.

### T25. Cut planks through the same cells
Does: Make boards explicit mono-material structures and cut them with the common cell rule, preserving their declared attachments. Re-bake the roof so changed piece shape comes from construction and cutter data. Absorbs: spine step 11.
Files: `tools/src/buildings/house.rs:64`, `tools/src/buildings/fracture.rs:107`, `tools/buildings/house_1.house.ron:1`. Deletes: remaining depth-length timber cutting and its exclusive assertions.
Proof: Roof users rely on board boundaries, conserved geometry and declared attachment ownership. Mutants cut across unrelated boards or infer nailing from an element name.
Wire and data: house bake/content digest changes; no new wire payload. Builder: Claude, for board geometry and the roof capture.
Blocked by: T20, T24. Conflicts with: T26, T30.
Player sees: longer wood pieces with non-square fracture ends. Eye gate: roof breaks must read as wood; service response remains a separate observation.

### T26. Keep proxies until matter leaves
Does: Derive contact-proxy replacement from departed matter rather than wear or cracks, and use one hull for eligible same-material groups. Admit a group hull only when its excess volume lies within the independently derived geometric allowance; otherwise use the common per-chunk representation, without a runtime experiment flag. Absorbs: spine step 12.
Files: `src/buildings/breakage.rs:1844`, `:1900`, `:2097`, `src/buildings/view.rs:21`. Deletes: first-wear/first-crack collider swapping and unconditional per-chunk construction where a certified group proxy suffices.
Proof: Players rely on each live chunk belonging to exactly one ballistic solid and spent chunks belonging to none. Mutants admit a hull across a real gap or omit a freed solid; compare discovery, corridor and field readers.
Wire and data: derived topology; bump protocol if the carrier contract changes. Builder: Claude, for proxy geometry and toothed-end inspection.
Blocked by: T24, T25. Conflicts with: T27–T30.
Player sees: fewer unnecessary collider swaps; no visible solid is skipped. Eye gate: a round stopping on air rejects that group proxy.

### T27. Reuse construction graph work
Does: Cache immutable incidence/adjacency and reuse assessment scratch storage, evaluating owned subgraphs while preserving stable chunk addresses. Invalidate topology/removal-dependent results explicitly; retain moving-frame load recomputation. Absorbs: folded-in slowdown’s construction-work candidate.
Files: `src/buildings/fracture.rs:1066`, `:1275`, `:1617`, `:2100`, `src/buildings/breakage.rs:513`. Deletes: repeated immutable graph allocation and full-source-graph work that an owned group does not require.
Proof: Law consumers rely on identical verdicts after cracks, removals, splits, support changes and changed presses. Mutants reuse stale state; compare the refactor once in its report, leaving no historical-output test.
Wire and data: none. Builder: Codex.
Blocked by: T17, T24, T26. Conflicts with: T30.
Player sees: no intended behaviour change. Eye gate: none; measure rather than promise speedup.

### T28. Measure the forest change honestly
Does: Run the corrected off-screen A/B/B/A comparison and headless tick benchmark against the recorded pre-cutover revision. Verify drawn-frame gates and separate rendering, entity and collider effects; report memory and disturbed-forest cost too. Absorbs: #150’s corrected acceptance.
Files: `scripts/perf/run-forest-cost.sh:1`, `src/headless_test.rs:1`; baseline correction is [#150’s owner comment](https://github.com/vikng-dev/overmatch/issues/150#issuecomment-6501139324). Deletes: temporary comparison arms after reporting.
Proof: The invariant is equivalent scene/input construction and valid observation, not reproducing an old timing. Mutants omit drawn frames or compare different populations; all resulting times are MEASURED observations.
Wire and data: none. Builder: Codex.
Blocked by: T24, T26. Conflicts with: T29, T30 through the machine measurement lock.
Player sees: the forest should cost less while retaining collision and cover. Eye gate: none.

### T29. Prove ramming through the real server and clients
Does: Capture a tree ram and building ram through production server simulation, UDP input and the complete client presentation stack, with a second observing client. Verify contact delivery, law decisions, launch, continuing traversal, later contacts and shared-time presentation; include a reconnect snapshot check. Absorbs: folded-in real-Battle ramming proof; closes #232.
Files: `src/contact_transfer.rs:234`, `src/buildings/breakage.rs:684`, `src/net/walls.rs:103`, `src/net/debris_stress.rs:232`. Deletes: temporary diagnostic source changes; keep reusable scenario inputs.
Proof: Players rely on the complete producer/consumer chain, not a hand-built imitation. Mutants omit contact registration, carrier publication or common-clock handover; conservation tests remain analytical, while breach sizes and speeds are reported.
Wire and data: scenario inputs only. Builder: Codex; visual anomalies go to Claude for diagnosis.
Blocked by: T6, T24–T26. Conflicts with: T28, T30’s measurements.
Player sees: a vehicle can load either material through the same production interaction. Eye gate: momentum, clipping and agreement between both clients.

### T30. Price collapse and finish the implementation queue
Does: Use `scripts/perf/collapse.sh` to measure native collapse, automatic settling and recurring law work after T4 and T27, then price the disturbed forest, hull construction and broken meshes under the production composition. Record the final limits and deletion census in the design’s single narrative home; complete the final lens review. Absorbs: remaining slowdown measurement, final pricing, #237’s outstanding implementation pricing.
Files: `scripts/perf/collapse.sh:1`, `scripts/perf/collapse.py:102`, `src/buildings/breakage.rs:1234`, `.agents/docs/design/wall-destruction.md:270`. Deletes: measurement instrumentation and any abandoned optimization arms.
Proof: Operators rely on complete observation windows and the configured server tick budget. Measure native pieces separately from independent stress piles, count recurring assessments, and price a DERIVED twenty-fir burst scenario including hull builds; no historical timing becomes a law test.
Wire and data: none. Builder: Codex.
Blocked by: T4, T27–T29. Conflicts with: all concurrent performance runs.
Player sees: the completed unified system is ready for the next build and first playtest. Eye gate: final wall, roof and forest capture; unresolved budget failure blocks the build.

**Issue disposition.** #264 closes with T1’s accepted-direction record; #268 closes after T11 and its T1 visibility evidence; #256 closes after T6; #150 after T28; #232 after T29; #188, already implemented in `00274ea` + `59ba0a5`, closes administratively after T29 verifies its surviving contract. #106 stays open for its wider destination, including other destructible assets. #189 stays open for independently verified empirical breach/non-breach cases; plausible captures cannot discharge that source requirement. #119 stays open until the first playtest establishes a sufficiently mature contract, honouring [the owner’s deferral](https://github.com/vikng-dev/overmatch/issues/119#issuecomment-6441664957).

**Sub-issue accounting.** Already completed: #120 (`84960f1`, `9765a6e`), #140 (`8922b27`), #141 (`c504965`, `e2b6a0f`), #149 (`8de96ee`, `9a5504a`); #121’s provisional scripted choice was superseded by streamed dynamics (`67272b2`). #122’s review and #123’s corrected off-screen comparison remain completed evidence. #193’s rehome guard remains mounted. #210 (`09e20be2`, `93479c47`), #233 (`559cb505`), #235, #236 and #249 are landed foundations, not new implementation work; their relevant contracts are exercised above. Nested #237 closes after T30; #269 remains closed research with implementation explicitly deferred.

**Scheduling and landing state.** Use only the three leased bays. Start T1/T4/T7 together; T2/T3/T8 can occupy the next independent slots. After T13, tooling T20 can overlap section work T14–T16; after the bake interface stabilizes, T23 can overlap remaining law work. T28 and T29 can be prepared independently, but machine measurements run serially. Any tasks naming the same file conflict even when their numbered dependency does not repeat that fact; refresh line citations at dispatch.

Every landing includes atomic reader/data migration, per-file formatting, targeted tests and CI-feature clippy through `serial-cargo`, a clean lane commit and the relevant backlog-row closure. Independent reviewers apply every lens at boundaries after T13, T17, T24 and T30. Wall play remains available after every landing; before T24, the known tree-drive crash remains and `main` is unsuitable for that drive. T24 is the earliest emergency build point that survives it; T25–T30 finish the owner’s required scope, so the **planned next build is after T30**.

The original drive first survives after **T24**, with both the joined witness and original runner proving it. A faster order can postpone T4, T27 and pricing, but cannot safely skip the bake, common runtime, carrier and view prerequisites. The explicitly rejected belt-reader stopgap would reach a fix sooner, but has no task.

**DERIVED estimate:** thirty builder sessions averaging roughly sixty-five minutes give about thirty-three agent-hours; twenty read-only lens passes at roughly twenty minutes add about seven. The largely serial law/content path plus the single compile/measurement queue gives roughly **twenty-six to thirty-four elapsed working hours**, excluding owner eye-gate response time and failed kill gates.

**Risks and kill criteria.**

- **Solid survival/cut independence — T1 first.** It is the riskiest assumption and cheapest to test without migrating runtime or assets. Retain the DERIVED beam-error limit of ten percent and the declared maximum seed spread of one shot; failure blocks downstream survival/cutover work. Improve sampling or certified cut resolution first; do not tune strength or piece length to a count.
- **Baked cuts miss load sharing — T16/T17, then play.** Analytical determinate fixtures must pass. A visible wrong hold opens the accepted stiffness-solve door; later cutter/view work waits if the solid model cannot explain that hold.
- **Fir response — T1/T21/T24.** The old shot prediction is dropped; roughly five shots is the owner’s provisional feel default, not an assertion. Likewise, a particular fir uprooting before snapping is an observation; test generic competing anchor/section capacities instead.
- **Material reduction hides a visible distinction — T1/T10/T11.** Retain a parameter only when the prescribed experiment exposes its separate effect. If wood never reads acceptably, revisit the proxy or cutter; do not invent species exceptions.
- **Crystal-like wood or breaks missing openings — T23/T24.** Change visual displacement first, then the data-selected cutter. Re-bake and repeat certification; surviving-solid laws remain unchanged.
- **Sleeping or performance fails — T4/T27/T30.** Forced sleep is not success. Residual automatic wake churn or an over-budget priced scenario blocks the build and triggers further library-mechanism work, not a debris-specific freeze rule.
- **Proxy stops rounds on air — T26/T29.** Reject the offending proxy and retain exact chunk solids. Price the resulting cost; the old DERIVED forest-saving estimate and linear collider-cost extrapolation are dropped as acceptance numbers.

**Owner questions: none block queue readiness.** Code, analytical laws, accepted rulings and the stated defaults settle the pre-playtest choices; eye gates above are observations during work, not unanswered planning questions.

- **Metal stays rigid:** adopt the proposed scope limit. Fasteners may release rigid parts; crumpling requires later scope and laws, not an unanswered call here.
- **“Structure”:** use the owner’s mono-material meaning; rename today’s whole-graph `Structure` to construction in T12. Blindly renaming `BuildingAsset` to `StructureAsset` would preserve the ambiguity.
- **Holes:** they are authoritative for survival, not drawing only. Keep gross contact hulls and inertial mass until chunks depart as an explicit resolution approximation; captures must expose any visible contradiction. The drawing cap cannot limit structural removals.
- **Rounds on edges:** an engineering ownership/conservation problem, resolved by T19 and the existing sampled-disc geometry contract; no owner choice.
- **Rebuilding at spawn:** complete sim state comes from admitted data at spawn in T22. Geometry caches may be shared or rebuilt; a loaded view never constructs sim state.
- **Stored results:** ballistics-28 is a standards correction, handled by T3, not a balance decision.
- **Material calls 1–5:** use six fields with section strength replacing `along`; one shared wood row until another ships; stretch is look only and shots are observations; rigid metal is the scope limit; rear-face retention follows T10’s visibility experiment.
- **Field stop naming its chunk:** defer that second merge gap with the post-playtest field adapter. Preserve stable chunk addresses now; build no exposed-face field table, flight-carrying fragment model, plate-work extension or cavity/burst replacement before the first playtest. The provisional field design owns those later tasks and its unresolved section-work judgement.