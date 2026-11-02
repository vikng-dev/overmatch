# Destruction review follow-up (#106)

Subject: the game's environment destruction and damage model; game state only.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude F6; Codex P1 | Rim-only crossings choose one midpoint-nearest chunk | Verified red: the front took about 7 J at axis x=2.0001, the rear none (MEASURED fixture) | buildings: allocate disc exchanges by sampled chunk chords (#106) |
| Claude F3 | Missing hole records pass a loop over an empty list | Verified: the guard had no independent expected chunk set | buildings: allocate disc exchanges by sampled chunk chords (#106) |
| Claude F7–8; Codex thickness/cap | Contact separation changes ballistic thickness; cap and guards contradict certified geometry | Verified red against the spawned collider; whole-hull casts now pass | buildings: separate ballistic hulls from contact separation (#106) |
| Claude F4 | Cone-only events are discarded by read_hits | Verified: the early return precedes the cone cursor; pending | — |
| Claude F2/F5; Codex launch | Sandbox launch and inspector tests bypass production paths | Verified against their callees; pending | — |
| Claude F14/F16; Codex numeric | Sandbox number jobs and duplicate shrink law | Verified; pending | — |
| Claude F9–13 | Stale sample, mass-hull, decision and API prose; view fade in code | Verified; pending | — |
| Claude F15 | Previous lane's commit message names the wrong protocol revision | Verified historical prose; reviewed code used 77, current bay uses 78 (version identifiers) | Record corrected here; no current code defect |
| Claude F1 | destruction-02 | Verified, reserved by the brief for the other lane; src/trees.rs untouched | — |

## Truths and decisions

- A delivered crossing retains its exposed disc-sample chords, clipped at rest. Construction
  work and momentum divide by their summed lengths in each chunk; entrance-only work and
  momentum remain on the earliest contacted chunk. This deletes midpoint-nearest allocation.
- Public VolumeCrossing endpoints remain on the shared axis, preserving rested_in's rest-point
  invariant. Contacts are authority-local event data, outside the wire and the client-derived
  presentation laws; no protocol bump.
- The hole test requires both expected chunk identities and both channel records. Its conservation
  assertion sums the sampled f32 loads in f64 at the same error bound, so accumulator rounding
  does not masquerade as lost work.
- The trunk reader's test fixture supplies the new contact field; its law is unchanged.
- Ricochet backlog rows ballistics-23–25 remain with their explicitly assigned rebuild. Rows
  ballistics-26–28 name pre-existing model facts in the delivery record's parent file; they remain
  open pending the numeric ratchet work. No row is closed without verification and its fix.

## Verification

MEASURED: the rim regression failed before the fix; after it, the head-on and rim fixtures both
retain the expected front/rear shares and conserve work and momentum. Production playback retains
covered off-axis contacts, while exposed-contact tests retain the shared axis readout.

Passed: cargo fmt --all --check; CI clippy with dev_ui,bitprobe; targeted buildings::breakage,
sandbox::wall, ballistics::, trees::motion, pen_cam, field_casts and doc_citations. The aggregate
suite reported 604 passed and no failures (MEASURED). Whole-lib and final guards remain pending.

The whole occupied hull now supplies a filter-free ballistic sensor; the independent inset hull
supplies contacts and explicit occupied mass. Avian 0.7 sensors add neither solver contacts nor
mass. The shared inset_point operation deletes the sandbox's duplicate shrink equation; its
small-hull floor is labelled Resolution and its nominal gap is no longer promised as a minimum.
The occupied-hull design prose and the release-sampling/centre test claims now state their actual
invariants. The ballistic face regression went red before this fix (MEASURED).

CI clippy and formatting passed. The breakage, debris and headless tests passed individually in
the combined run; two new sandbox-path fixtures failed because their mesh-asset message was not
registered. Those uncommitted test replacements remain item 4, not evidence for this fix.
