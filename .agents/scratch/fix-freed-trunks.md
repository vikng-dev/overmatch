# Freed trunks and destruction follow-up (#106)

Subject: the game's environment destruction and damage model; game state only.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude; Codex | P1: flight and aim miss freed trunks | Verified red at the freed-state discovery query; standing, freed and settled discovery, corridor, field and aim checks now pass | trees: make freed trunks ballistic solids (#106) |
| Claude; Codex; tree reviewer | P1/P2: far-coordinate collapse skips surviving contacts | Verified red: the neighbouring contact received no work; collapsed axes now read sampled contacts before the point outcome, preserving all work and momentum | buildings: read contacts before collapsed-axis point exchanges (#106) |
| Claude | P3: sensor mass guard and Debris prose | Verified: the old mass tolerance admitted the extra unit-density solid; tightened the spawned-body mass guard and corrected the layer documentation | trees: make freed trunks ballistic solids (#106) |
| Claude; Codex | P2: inspector assigns foreign cones to the wall and carries strength across shells | Verified red: foreign cone count was 2 rather than 1; emitter/per-shell fix passes the observer witness | dev: attribute wall cones and verify destruction readers (#106) |
| Codex | P2: rest-clipping test never asserts the truncated plate | Verified red with unclipped contacts: 1.75 m rather than the resolved 1.25 m; stronger test passes against the real rest value | dev: attribute wall cones and verify destruction readers (#106) |
| Claude; Codex | P3: appearance ownership, tool refusals, empty cone denominator, mass control and redundant contact clamps | Verified by source; data migration, refusal tests, shared ray-share assertion and sensor-removal mass control pass; endpoint clamps verified redundant against walk::encounters | dev: attribute wall cones and verify destruction readers (#106) |
| Brief / ratchet | One gravity; Block::inradius guard; fracture backlog destruction-06 and destruction-09 | Verified duplicate gravity owner and listed sites; untouched at the timebox | Unfixed |

Decisions:

- Retain each original segment's volume and HP entity as its Armor sensor. Separate contact siblings carry density and no volume. Standing segments keep their existing combined contact/solid form. Re-splits remove departed contact siblings; reconnect restores the original standing entities.
- Replicas need the same sensors for aim at the streamed pose. Their fuzes never resolve crossings (`ballistics::burst::Fuze`); this is an aim contract, not client prediction.
- `ProjectileMarchWorld::armour` asserts Armor membership on ballistic collider ancestry in every build, complementing discovery's assertion in the reverse direction.
- No wire or client-derived simulation law changed, so no protocol revision bump. The unaccepted generic fracture design and its deferred allocator, hole and cloning findings remain untouched.
- Players stop firing and aiming through freed wood. Falling and settled trunks are ballistic cover at their live poses.
- DERIVED census: one additional contact collider per freed segment on authority and replicas; standing forest collider count is unchanged. No frame/tick pricing or captures were taken, per lane rules; the orchestrator owns pricing. `run-walk.py` has not been run in this lane.

Verification: the real-query regression failed before the fix at `state 1: flight must meet the wood` (MEASURED). The targeted first run passed 102 tests; the full item-one gate passed 651 tests, with 14 ignored measurement/pricing beds (MEASURED). Format check and CI clippy passed. The gate includes trees, debris, network destruction, buildings, ballistics, aim, recording, headless checks and documentation citations. The continuation passed the whole-tree guards with the expanded targeted gate; the whole-library run remains for the final step.

Rounding step: format check, CI clippy and 215 targeted tests passed (MEASURED), including the far-coordinate regression, trees, debris, network destruction, buildings and doc citations. The earliest-contact entrance allocator is unchanged.

## Continuation

The saved draft is applied. Format check, CI clippy and the expanded targeted/whole-tree gate passed: 712 tests, 14 ignored measurement beds (MEASURED). This includes the actual sensor-removal mass control, nonempty-cone construction reader, live emitter record, wall attribution/admission tests, rest-clipping witness, headless compositions and documentation/UI/network/bake guards. Earlier red evidence remains as stated above; the rest-clipping law itself was already correct.

Remaining implementation: shared gravity, Block::inradius's redundant denominator, and fracture backlog destruction-06/destruction-09. The final whole-library gate follows those edits. The handoff artifact will be deleted in the last commit; it is not a landing artifact.

No pushes, issue comments, windows, captures, pricing runs or damage-topic web searches. Standing forest collider census is unchanged; freed segments add one collider each (DERIVED). run-walk.py was not run. The generic-fracture design still requires the owner's decision.
