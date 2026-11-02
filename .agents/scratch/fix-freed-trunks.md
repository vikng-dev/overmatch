# Freed trunks and destruction follow-up (#106)

Subject: the game's environment destruction and damage model; game state only.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude; Codex | P1: discovery and aim miss freed trunks | Verified red at the real freed-state flight query; standing, freed and settled discovery, corridor, field and aim now pass | trees: make freed trunks ballistic solids (#106) |
| Claude; Codex; tree reviewer | P1/P2: far-coordinate collapse skips surviving contacts | Verified red: the neighbouring contacted chunk received no work; the regression covers both contact and point-only outcomes | buildings: read contacts before collapsed-axis point exchanges (#106) |
| Claude | P2: inspector assigns foreign cones to the wall and carries potency across shells | Verified red: the old reader counted two cones instead of one; emitter and interleaved-shell witness passes | dev: attribute wall cones and verify destruction readers (#106) |
| Codex | P2: rest-clipping witness never asserts the truncated plate | Verified weak witness: unclipped input ends at 1.75 m instead of the resolved 1.25 m (MEASURED); the strengthened test passes with actual rest clipping, whose production law was already correct | dev: attribute wall cones and verify destruction readers (#106) |
| Claude; Codex | P3: sensor's no-mass claim lacks a rejecting witness | Verified old tolerance admitted unit-density duplicate mass; strict occupied-mass guard and removal/recompute control pass on the actual spawned body | trees: make freed trunks ballistic solids (#106); dev: attribute wall cones and verify destruction readers (#106) |
| Claude; Codex | P3: construction cone denominator contradicts inspector admission | Verified: the construction reader's max(1) hid an empty recorded direction set; both readers now share the nonempty-ray assertion, exercised through ShellMarched | dev: attribute wall cones and verify destruction readers (#106) |
| Claude; Codex | P3: appearance values, puff scale, prose, readout bound and tool refusals | Verified source sites; required Taste palette/magnitudes own appearance, puff scale has one owner, animate_puffs links an item, range provenance states its derived bound, scalar/palette refusal tests pass; earlier record's overclaim corrected | dev: attribute wall cones and verify destruction readers (#106) |
| Codex | P3: volume_contacts endpoint clamps | Not a live defect: walk::encounters creates contacts inside each encounter, including merged contacts; the redundant start/end clamps are removed and actual rest clipping stays | dev: attribute wall cones and verify destruction readers (#106) |
| Brief | Block::inradius max(1) | Not a live defect: its sole caller, inset_point, asserts at least four corners before reading the radius; redundant fallback removed and the certified-solid precondition stated | buildings: share physical laws and close fracture backlog (#106) |
| Brief | One gravity | Verified duplicate owner; changing the environment fixture to 10.0 left structure at 9.81 (MEASURED red); buildings, trees and flight now read the same owner, with shipped gravity restored unchanged | buildings: share physical laws and close fracture backlog (#106) |
| Lens backlog | destruction-06: fixed shipped-material expectation | Verified literal crushing expectation despite registry-derived construction; test now iterates admitted rows and derives expectations from resistance/reference facts, including a nonnegative zero outcome | buildings: share physical laws and close fracture backlog (#106) |
| Lens backlog | destruction-09: empirical parameters compiled as constants | Verified source; existing physical-laws data now owns the unchanged field fits, equivalence factor and TNT game-unit convention. Ungated from=0 admitted malformed data (MEASURED red); domain/finite-result admission tests cover the gate | buildings: share physical laws and close fracture backlog (#106) |

## Player result

Players stop firing and aiming through freed wood. Falling and settled trunk segments provide ballistic cover at their live poses, just as standing segments do. Far-coordinate sampled contacts retain their work and momentum. The wall inspector stops showing another body's exit cone and reporting another shell's cone strength as its own. Appearance magnitudes and physical parameters retain their existing values.

## Decisions

- Keep each original trunk segment's volume and HP entity as its Armor-only sensor after release. Separate inset contact siblings carry density and no BallisticVolume. Re-splits remove departed contacts; standing reconstruction restores the original solid entities.
- Give replicas the same sensors because aim reads their streamed poses. Replica fuzes never walk crossings (ballistics::burst::Fuze); this adds no client prediction.
- Assert Armor membership for every ballistic collider ancestry in armour(), in every build, complementing discovery's reverse assertion.
- Read surviving contacts before the point outcome when an axis collapses. Only a line with no mapped sampled contact wears the chunk at its point. Preserve all delivered work and momentum; leave final ownership allocation to the unaccepted design.
- Retain authority-local emitter identities on raw cone records, filter the inspector by those volumes, and retain pending potency per live shell until that shell's wall line consumes it. Serializable cone records and the wire are unchanged.
- Keep production rest clipping. Remove only the redundant encounter clamps. Share the nonempty direction-set assertion between construction and inspector readers. Keep explicit sensor-removal mass evidence rather than claiming an untested no-mass property.
- Require appearance tastes in the existing wall-tool file, validate scalar and color domains at admission, and reuse one puff-start value. Restore the range note as a derived bound, not a measured camera result.
- Move the existing physical-laws Rust module to shared crate scope and reuse assets/ballistics/physical_laws.ron. Add no new data file or second home. Move the empirical fit parameters and TNT convention into that existing sourced file; preserve all domain, knot and coefficient literals and f32 evaluation operations. Keep the equivalence factor labelled StandIn because its claimed source was not verified in this lane; retain the fit tables' existing source attribution without a damage-topic web search.
- Validate fit domains, coefficients and finite positive results before play. Interval Horner bounds catch interior overflow that endpoint checks miss; subdivision is Resolution for admission cost/overestimation, not a new field law.
- Replace the material test's shipped-value pin with the registry relationship. Remove Block::inradius's dead fallback; do not add a duplicate caller guard. Close both backlog rows naming fracture.rs; no other open backlog row names an edited source file.
- Do not bump PROTOCOL_REV: no wire or client-derived simulation law changed. The shared gravity and empirical parameter values are unchanged. The allocator behind take_lines' first, unequal chord fixtures, rim-only holes, per-share contact cloning and the separate tree P3 patch stay with the unaccepted generic-fracture design.
- Delete the handoff patch in the last commit and remove its temporary handoff commit from delivered lane history; retain this record. The two P1 commits stand.

## Evidence and gates

MEASURED red evidence: the freed-state flight query missed wood; the far collapsed-axis contact assigned all work to the point chunk; the inspector counted two cones rather than one; unclipped contacts extended to 1.75 m instead of rest=1.25 m; a changed environment gravity left the structural field at 9.81 rather than 10.0; and the ungated fit parser admitted from=0. The temporary gravity change is restored. Source comparison confirms all four fit tables preserve their domain, knot and coefficient literal lists exactly.

MEASURED first-round gates: 102 initial targeted tests; 651 expanded item-one tests with 14 ignored measurement beds; 215 rounding-step tests with three ignored beds. Format check and CI clippy passed before both P1 commits.

MEASURED continuation draft gates: format check, CI clippy and 712 targeted/whole-tree tests passed, with 14 ignored measurement beds. This includes trees, debris, network destruction, buildings, ballistics, aim, recording, headless compositions, wall tooling and doc_citations/ui_ascii/net_boundary/bake guards. The sensor-removal mass control, malformed-cone construction observer, live emitter record and tool admission witnesses passed.

MEASURED final gates: format check and CI clippy passed; 869 targeted/whole-tree tests passed with 11 ignored measurement beds; the detached, unfiltered whole-library run passed 2,223 tests with 26 ignored and zero failures. The final run includes all headless, recording and whole-tree guards.

MEASURED continuation clock: started 19:05:25 UTC; the whole-library gate completed by 19:42:16 UTC, inside the forty-minute limit. No runtime gate remains canceled or queued.

## Scope and unrun work

No implementation leftovers are intended. The generic-fracture design still requires the owner's decision on its deferred findings listed above. No source patch is a handoff artifact.

run-walk.py was not run; forest frame/tick cost is unpriced. DERIVED collider census: one additional collider per freed segment on authority and replicas; standing forest collider count is unchanged. No captures, pricing runs, windows, pushes, issue comments or damage-topic web searches were made. All test runs use the machine lock and OVERMATCH_UNATTENDED=1, OVERMATCH_WINDOW=headless, OVERMATCH_MUTE=1.
