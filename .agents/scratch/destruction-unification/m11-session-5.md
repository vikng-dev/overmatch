Subject: **the video game's environment destruction model and damage model; game state and presentation only.**

## Rebase

Rebased onto gated main, whose tip was **perf: prepare a real-match server load runner (#186)**.
The single textual conflict was the prop build's module documentation; the current one-source
construction description wins. Main's bake-error, sandbox/capture admission and load-runner changes
remain in the branch.

Main's load runner still counted serialized departure events, which the common cause ledger no
longer carries. **net: derive load-test departures from common causes (#106)** observes departed
chunk state and reconstructs its ledger witness through the shared support derivation. Its two
simultaneous fixture constructions must publish sufficient causes on one authority tick. The
runner's refusal now describes a derived departure.

The supplied `lanes/sweep.md` was absent. Review used `rules.md`, the six repository lenses and
their current backlog. No forbidden document or damage-topic web search was used.

## Runtime and contract deletion

- Deleted `src/trees.rs`, `src/trees/motion.rs` and `src/trees/view.rs`, including their segment
  HP pools, trunk loads, slice state, local fall schedules, collider reparent/refit code, GLB
  templates, cap meshes, replica partitioning assumptions and tests of that retired path.
- Deleted `Prototype::Tree`, the map's tree library and generator-file digest inputs, and the
  scatter branch that could still construct the retired runtime. Map roots, construction causes,
  released groups, view meshes and dressing have only the common construction path.
- Removed the construction blast adapter and moving-body exclusion for the retired trunk owner.
  Construction chunk walks own construction matter; other dynamic bodies receive their ordinary
  world-space impulses. Removed the unused separate root-plate accessor; common foundation
  admission and the root-plate law remain.
- Removed tree plugin wiring, the launch ordering edge and reset's separate loose-trunk lifetime.
  `collider_rehome` remains for production ownership changes outside the deleted runtime.
- Deleted the capture plan's segment-fall cue and its test/schema arms. The shot cue selects common
  constructions. Shot-trace crossings, scene counts, the scripted battle's observations and their
  tracked reader now name constructions. The two environment caster categories become one
  `Construction` category, with the single `construction-casters` lever used by the ablation runner.
- The game's damage model no longer emits the tree-only `PoolEmptied` event. Deleted its
  `DoseContext`, `Landed`, per-deposit context vectors, settlement deposit index, emptying-context
  lookup, playback frame conversion and retired consequence-context witness. HP conversion,
  settlement history, emptied-pool reporting and the pen cam's mark-based explanation remain.
  A cone still settles once: combining its nonnegative saturating integer contributions per
  address gives the same total, independently of order.
- The server payload now includes both construction geometry and dressing, and excludes
  generator-only tree RONs. Updated deployment and moved-collider patch documentation to name
  their current readers. No vendored engine implementation or tripwire changes.
- The glossary defines wood through material properties and construction, without segment HP or
  a separate ram threshold. Species names generation and appearance parameters.

The session-2 generator/source/certificate replacement remains the owner of `assets/trees/`:
`TreeSource`, species data and regenerated inspection artifacts are authoring inputs/outputs, not
runtime simulation contracts. Its removal of `wood.ron` already landed earlier in this lane.

MEASURED scoped source census: no retired runtime identifiers, segment-view metadata, separate
HP event/context types or old caster selectors remain in `src/`, `scripts/`, `tools/`, `assets/`,
`.github/`, `DEPLOY.md` or the affected vendored patch note. Prior reports, design history and the
M13 baseline retain their historical evidence. Session 6 still owns the final census and mutants.

## Discharged ratchet

- `net-21`: network startup refuses an aim chain with an unstreamed moving ancestor. Static
  intermediate frames remain legal; the streamed Gunner yaw/pitch can compose through each other.
  The production admission-plugin witness covers concealed servo and roadwheel ancestors above
  either axis, plus a legal static chain and unrelated moving branches.
- `view-22`: the shipped aim installer is exercised with interleaved owners at different physics
  poses, and across a committed update followed by an absent commitment. Each servo reads its own
  root; no commitment retains its target instead of reusing stale scratch. Detached assemblies use
  Bevy's `iter_ancestors`. `ServoAngles` names `gunner_yaw` and `gunner_pitch`, independently of the
  structural assembly. Timing policy does not change.
- `view-21`: the camera documents its hull-frame blend separately from the Gunner rest-frame
  intent bound; ADR-0013 describes the existing traverse-motor gate; the view-cast cap states its
  resolution role and leaves the sky distance to its data owner.
- `view-11`: required `assets/ui/camera.ron` and `assets/ui/style.ron` own camera framing/input
  tastes, pre-bind stand-ins and shared font/palette styling. Build-time admission rejects missing,
  unsourced, test-only and invalid fields with the file, structured field path and repair. It
  generates existing constant interfaces, preserving admitted f32 bits without handwritten copies.
- `view-05`: both bundled font handles join `PresentationLoads` before tank admission. Bevy 0.19's
  font loader wraps bytes without decoding them, so startup also verifies font format, required
  glyph coverage and readable outlines. Corrupt/truncated data refuses before play. Decision:
  name the already locked `ttf-parser` dependency directly; no dependency version changes.

These five rows are removed from the backlog after verification. The existing `view-23` row names
`sight.rs`, which this session does not edit; its travel-window repair stays open. The pre-bind FOV
constant interface is preserved under M11's explicit allowance to avoid unrelated caller churn.
`net-22`, `servo.rs`, `load-27`, `load-31`, grip dwell, `GRIP_SHEAR_FLOOR_M` and `piece_impulses`
remain outside this change.

## Verification

MEASURED results; these runs overlap:

- Rebase targets: **318 passed, 4 existing ignored**, covering capture/sandbox admission, the load
  runner, common construction/map/scatter and the documentation guard.
- Broad deletion coverage passed the construction, map/scatter, world, scripted-battle, full
  headless, real-UDP world-reset, view/ownership and whole-tree checks. Its two interim failures
  were the deliberate wire-definition re-pin and the new data-test mutation helper; both are fixed.
- Final affected game damage-model/presentation run: **707 passed, 15 existing ignored**, including
  ballistics, pen-cam explanations, fields, data/font/camera admission, protocol, ownership,
  load-runner, common spawn/material witnesses and the whole-tree guards.
- Exact-f32 emission and guard recheck: **61 passed, none ignored**. Final strengthened network
  admission witness and guard recheck: **49 passed, none ignored**. Both include `doc_citations`,
  `ui_ascii`, `net_boundary` and `bake::`.
- The existing field-cast comparison matches **2,009 records** and ordered digest
  **`0x15eb25370bef2b50`**, MEASURED, as in session 4. No output pin changes or new historical oracle.
- `PROTOCOL_REV` is deliberately **88**. The own-definition fingerprint is
  **`0x1ca5596988d05ea5`**, MEASURED by the tripwire. The ordered registration fingerprint remains
  **`0x8ce64195b777b742`**. Both manifest tripwires pass.
- The tracked server allowlist dry run contains **12 files**, including all **4 construction and
  dressing files**, and no generator-only tree file, MEASURED. Action YAML, its extracted Bash,
  the ablation runner's Zsh and both edited Python scripts pass syntax checks.
- Formatting, diff whitespace and CI-feature workspace/all-target clippy pass on the final code.
  The diff and associated tests were self-reviewed under all six lenses.

Every Cargo build/test and the allowlist execution used the machine lock. Game executions were
unattended, headless and muted. No frame/tick pricing, capture, external issue comment or push.

## Session 6

Still required: the joined production detachment/belt witness and compound-constructor mutant;
the original `reproduce.py` drive with `nearest.ron`, real admission/input/contact, authority
survival and client completion; an observing client and reconnect/reset proof; and the final
whole-tree deletion census and specified ledger/carrier/generation/cursor mutants. The reset
fixture here is transport coverage, not that production-drive proof. No new owner decision is
needed for this session's implementation.
