Subject: **the video game's environment destruction model and damage model; game state and presentation only.**

## Review carry-overs

The commit **construction: close the cutover review carry-overs (#106)** repairs the findings
that remained after the earlier sessions:

- Prop publication creates all destination directories after construction and dressing admission,
  before replacing artifacts. Filesystem failures reach the command boundary as visible OS errors;
  the message names the cause and tells the author to fix the filesystem and rebuild. A failure
  during publication no longer claims that every artifact is unchanged.
- An untagged peer ledger produces `ConstructionRefusal::UntaggedLedger`, reaches the ordinary
  visible match-close path, and preserves the admitted record. The refusal witness includes an
  already admitted, correctly tagged ledger so malformed input cannot hide beside it. The malformed
  batch, shrinking-ledger and rewritten-prefix cases share one admitted-ledger fixture.
- `ColliderChange::apply` owns command dispatch for both production reconciliation and its
  worker-independence witness. Every geometry assertion remains.
- `Faces::element` emits the hull and owned cards together. Whole and broken views call that one
  operation; the forwarding `chunk_of` helper is gone.
- The exterior/cut witness identifies cards by their zero ownership tags and compares each solid
  face against its geometry-derived tint. Equal card/skin tints or white skin are permitted.
- The house dressing header points to its schema without narrating its empty fields.

Verified already closed: dressing is computed and checked before publication and during lint;
the prop tests now guard that order explicitly. Session 3 already removed the protocol's revision
history paragraph. No protocol edit is needed for either carry-over.

MEASURED: **33 prop tests passed**, including first publication to a missing construction directory,
invalid dressing refusing lint/build before writes, and visible filesystem refusal. **281 targeted
game-library tests passed, with 7 existing tests ignored**: construction, the full headless suite,
and the required `doc_citations`, `ui_ascii`, `net_boundary` and `bake::` guards. Formatting and
CI-feature workspace/all-target clippy passed.

## Required scalar data

`assets/ballistics/crew_line.ron` owns the crew-line HP scale as a required `Unit`;
`assets/vfx/fire_cues.ron` owns cosmetic cue age as a required muzzle-cue `Taste`.
The shared provenance reader admits both during `build.rs`, including refusal of missing fields,
bare values, blank provenance and test-only labels in every build. Diagnostics name the file,
field and repair. A zero crew-line scale is refused; zero cosmetic age is a valid authored taste.

Decision: generate the existing constant interfaces from those admitted scalars. This keeps
presentation/audio callers on one interface without copying authored values into Rust. This build-time scalar admission pattern can also
serve the camera/UI tastes explicitly budgeted for session 5. The shared
admission module is also mounted by library tests. The added build dependencies are the same
already locked serde/RON dependencies used by runtime asset admission.

The dose accumulator's resolution is independently fixed in HP: **100,000,000 dose units per HP**
(MEASURED transcription of its `Resolution` choice). DERIVED: each contribution's rounding error
is bounded by **0.000000005 HP**; an admitted
crew-line scale changes dose magnitude without changing that resolution. Settlement fixtures use
explicit HP/dose units, so permitted crew-line scale edits do not change their rounding, pooling
or emptying-context obligations.

MEASURED transcription of the required authored choices: **10 HP per crew-line coefficient unit**
and **16 fixed ticks of cosmetic cue age**. Their shipped compiled values are unchanged.

## Ballistics ratchet

- Removed the tree fall/derive sets from the march phase chain, march-probe ordering and the
  production-composition schedule witness. No source under `src/ballistics/`, or in
  `src/ballistics.rs`, references `crate::trees`, `TreeFallSet` or `TreeDerived`.
- Removed the tree-world/spawn helpers, segment pool and shot-count helpers, segment-radius
  constants and the segment-HP/fall tests. Their now-unused test-only `fixture_kind_of` helper
  is removed from `src/trees.rs`; no runtime function there changes. Those tests pinned the deleted system's pool scaling,
  height cuts and fall-direction rule. Common construction tests instead own wear, support loss,
  stable chunk identity, energy/momentum accounting and view/replica reconstruction.
- Rebuilt the convex hand-off witness with an inline low-resistance frustum fixture, independently
  of a tree runtime or a species' shape. It retains every disc-offset/incidence and stand-off case
  and requires a positive chord from every intersecting disc.
- Ported the flight/corridor/field/aim query witness to common construction matter. The production
  common plugin releases an isolated fixture chunk after support loss; the same occupied-volume
  queries read it intact, released at another pose, and sideways asleep. This is a query witness;
  session 6 still owns the joined production belt/UDP proof.
- `MarchTick` has one role: authority. Replica return precedes its construction. Removed its
  deposit flag, redundant authority branches, and the unreachable client ground-stop trace arm,
  including its unused recorder resource and lifetime.
  Client lifecycle recording stays in public-path presentation. Read-only resolver contexts and
  their instrument callers remain available; no alternate runtime march is retained.
- Collector documentation describes common construction convex geometry, not segmented trunks.
  The fixture frustum's exact-plane assertions remain generic collector obligations.
- Editing the collector also brought `ballistics-29`: its unproved miss-share bound is removed.
  Successful T-junction walks still require one box interval; the unpaired-ray share is reported
  as an observation. The fixture must retain successful occupied-box rays.

Backlog rows `ballistics-29`, `ballistics-31`, `ballistics-32` and `ballistics-33` are closed by
these changes. `ballistics-34` was already closed by **quality: state construction and grip
invariants once (#106)**; the current shell view has no hold-visibility gate, and this session
adds none. The phase-chain change carries no physical-law or grip adjustment.

## Verification

MEASURED: **720 targeted game-library tests passed, with 21 existing tests ignored**. They cover
ballistics (including the pen cam, public-path presentation and field casts), common construction
laws/view, peer construction admission, all headless witnesses, muzzle presentation, consumed-fire
release and the required whole-tree guards. The new common release/query and convex hand-off
witnesses pass.

The one before/after field-cast comparison matches **2,009 records** and their ordered digest
**`0x15eb25370bef2b50`**, MEASURED. No cast output pin changes. This is evidence for unchanged
outcomes of this refactor, not an additional acceptance test.

MEASURED: the final helper-deletion/guard recheck passed **88 tests**, with none ignored. It covers
all retained local tree tests, scalar admission, convex hand-off, the common occupied-matter query
witness, the T-junction observation and the required whole-tree guards. Formatting and CI-feature
workspace/all-target clippy passed on the final source. The runs overlap; their counts are not a
unique total.

MEASURED: the intentionally open T-junction fixture produced **23 unpaired rays out of 66,049**.
That is an observation, not a pass threshold. Successful walks still require one occupied-box
interval. No frame/tick costs were measured.

The diff and associated tests were self-reviewed against all six lenses. The only change to the
remaining local runtime's file removes its unreferenced test fixture. The runtime itself remains
for session 5.

## Handoff and plan corrections

Session 5 owns deletion of the remaining tree runtime and consumers, obsolete assets/contracts,
the glossary's wood definition, `net-21`, `view-21`, `view-22` and their associated rows. Session 6
owns the joined collider/belt witness, the original `reproduce.py` / `nearest.ron` UDP drive,
observer/reconnect proof and the final whole-tree deletion census. The query and schedule proofs
here do not claim those production-drive results.

The dispatch's carry-over list includes two findings already closed before this session, and its
ballistics range includes the already closed `ballistics-34`. The collector and resolver contained
additional old tree references beyond the primary scheduling/test sites; removing those references
also brought the explicitly reported `ballistics-29` obligation.

No owner decision is needed. `net-22`, `servo.rs`, `load-27`, `load-31`, grip dwell,
`GRIP_SHEAR_FLOOR_M` and `piece_impulses` are untouched. No capture, pricing, UDP drive, external
issue comment or push was made. All game/asset executions were unattended, headless and muted,
under the machine lock. No subagents were used.
