# Destruction review follow-up (#106)

Subject: the game's environment destruction and damage model; game state only.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude F6; Codex P1 | Rim-only crossings choose one midpoint-nearest chunk | Verified red: at axis x=2.0001 the front took about 7 J, the rear none (MEASURED fixture) | buildings: allocate disc exchanges by sampled chunk chords (#106) |
| Claude F3 | Missing hole records pass a loop over an empty list | Verified: no independent expected chunk set | buildings: allocate disc exchanges by sampled chunk chords (#106) |
| Claude F7–8; Codex thickness/cap | The contact gap changes ballistic thickness; cap and guards contradict certified geometry | Verified red against the spawned collider; whole-hull face casts pass after the fix | buildings: separate ballistic hulls from contact separation (#106) |
| Claude F4 | Cone-only events are discarded | Verified red: zero cones recorded; cone, witness and once-only next-line checks pass after the fix | dev: retain cone-only events for the next wall deposit (#106) |
| Claude F2/F5; Codex launch | Sandbox launch and inspector tests bypass production paths | Verified against their callees; replacements reach the observer, deposit system and body spawn | dev: test wall deposits through launch and body spawning (#106) |
| Claude F14/F16; Codex numeric | Sandbox number jobs and duplicate shrink law | Verified; initial appearance fields moved to data, fixture values labelled, shrink equation shared. Remaining palette and puff magnitudes are corrected in `fix-freed-trunks.md` | dev: source inspection appearance and label scenario fixtures (#106); buildings: separate ballistic hulls from contact separation (#106) |
| Claude F9–11 | Release sampling, centre mutant claim and occupied-hull prose | Verified; state at most eight samples, drop the unsupported mutant claim, name whole occupied hulls | buildings: separate ballistic hulls from contact separation (#106) |
| Claude F12/F13 | Reversed hole-cap decision, stale API/verdict prose and decal fade in code | Verified; decision reversal recorded, emitted-work and terrain-verdict prose corrected, fade sourced as Taste | dev: align destruction prose with the current invariants (#106); dev: source inspection appearance and label scenario fixtures (#106) |
| Claude F15 | Previous lane's commit message names the wrong protocol revision | Verified historical prose; reviewed code used 77, current bay uses 78 (version identifiers); no current code defect | Corrected in this record |
| Ratchet | ballistics-26–28: code-authored gravity, air density and cavity inertia | Verified; dropped on landing, because `physical_laws` already owns these facts on main | — |
| Claude F1 | destruction-02 | Reserved by the brief for the other lane; src/trees.rs untouched | — |

## Truths, deletions and player-visible results

- Exposed disc-sample chords, clipped at rest, supply construction weights. Transit work and
  momentum divide by their summed lengths in each chunk; entrance-only exchange stays on the
  earliest contacted chunk. Midpoint-nearest allocation is deleted. Edge crossings now wear
  every contacted chunk instead of choosing a winner when the axis clears an edge.
- Whole occupied geometry supplies ballistic thickness, mass and inertia. A separate sensor adds
  no solver contact or mass; the inset hull supplies contacts with explicit occupied properties.
  A rubble crossing now reads the whole material chord. The shared inset operation deletes the
  sandbox's second shrink equation and impossible denominator guards.
- Cones update the inspector's witness immediately, even without a line. WallLog originally retained their
  strength globally until the next line; `fix-freed-trunks.md` records the emitter/shell attribution correction. Cone-only ticks no longer disappear from the
  inspector. The admitted nonempty ray set is asserted instead of hidden by a denominator fallback.
- The sandbox tests observe the actual pool, channel and readout, initial launch velocities, and
  computed occupied mass/inertia after initialization. The old direct-collider and load-helper
  substitutes are gone. Initial spin is read before integration; torque-free rotation can change
  angular velocity afterwards. Mesh assets are registered for Avian's collider-cache reader.
- Adjustable wall appearance and decal depth fade are required Taste data. Layout, counts, tool
  inputs and capture times stay labelled fixtures under the brief's dev-sandbox exception;
  diagnostic ray stride is Resolution.

## Decisions

- Public VolumeCrossing endpoints stay on the shared axis, preserving rested_in's rest-point
  invariant. Sample contacts are authority-local delivery data. No wire or client-derived law
  changes, so no protocol bump; the current revision remains 78.
- Preserve the existing nearest-face contact gap and small-hull contraction floor. The floor is
  Resolution for solid hull construction; it can reduce the nominal face gap. No material or
  physical-law parameter is retuned. Visual seams supply their independent Taste gap.
- The pending-cone decision is superseded by the emitter/shell attribution contract in
  `fix-freed-trunks.md`; rebuilding the wall still clears the log.
- Narrow the inspector test's name to work through pools, holes and readout; the companion launch
  test owns momentum evidence. The hole guard requires both expected chunk identities and both
  records. Sum its sampled f32 work in f64 at the same error bound to avoid accumulator drift.
- The trunk reader's test fixture supplies the new contact field; its law is unchanged.
- Record that the prior lane brief reversed hole-cap crumbling because view capacity cannot
  empty a structural pool. Retain ricochet rows ballistics-23–25 with their assigned rebuild and
  leave destruction-02 with its assigned lane. No issue or external repository was modified.
- No captures, pricing runs, visible windows, pushes or damage-topic web searches.

## Verification

MEASURED: the rim, spawned ballistic-face and cone-only regressions failed before their fixes.
The new production-playback check retains off-axis contacts beside the shared axis; the exposed
contact check clips them at rest without changing public crossing endpoints.

Passed: cargo fmt --all --check; CI clippy with dev_ui,bitprobe; selected wall/presentation/model
admission checks; the corrected launch test; targeted buildings::breakage, debris::,
track_sandbox::rubble, ballistics::, pen_cam and field_casts; and the whole-tree doc_citations,
ui_ascii, net_boundary and bake:: guards. The final aggregate targeted run reported 633 passed,
no failures (MEASURED). Headless checks passed with the hull change.

The detached whole-library attempt remained queued behind the machine lock at the lane's
sixty-minute stop and was canceled before execution. Rerun the whole-library gate; it is the
only remaining verification task. All implementation steps are committed and the bay is clean.
