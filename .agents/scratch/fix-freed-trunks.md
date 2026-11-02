# Freed trunks and destruction follow-up (#106)

Subject: the game's environment destruction and damage model; game state only.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude; Codex | P1: flight and aim miss freed trunks | Verified red at the freed-state discovery query; standing, freed and settled discovery, corridor, field and aim checks now pass | trees: make freed trunks ballistic solids (#106) |
| Claude | P3: sensor mass guard and Debris prose | Verified: the old mass tolerance admitted the extra unit-density solid; tightened the spawned-body mass guard and corrected the layer documentation | trees: make freed trunks ballistic solids (#106) |

Decisions:
- Retain each original segment's volume and HP entity as its Armor sensor. Separate contact siblings carry density and no volume. Standing segments keep their existing combined contact/solid form. Re-splits remove departed contact siblings; reconnect restores the original standing entities.
- Replicas need the same sensors for aim at the streamed pose. Their fuzes never resolve crossings (`ballistics::burst::Fuze`); this is an aim contract, not client prediction.
- `ProjectileMarchWorld::armour` asserts Armor membership on ballistic collider ancestry in every build, complementing discovery's assertion in the reverse direction.
- No wire or client-derived simulation law changed, so no protocol revision bump. The unaccepted generic fracture design and its deferred allocator, hole and cloning findings remain untouched.
- Players stop firing and aiming through freed wood. Falling and settled trunks are ballistic cover at their live poses.
- DERIVED census: one additional contact collider per freed segment on authority and replicas; standing forest collider count is unchanged. No frame/tick pricing or captures were taken, per lane rules; the orchestrator owns pricing. `run-walk.py` has not been run in this lane.

Verification: the real-query regression failed before the fix at `state 1: flight must meet the wood` (MEASURED). The targeted first run passed 102 tests; the full item-one gate passed 651 tests, with 14 ignored measurement/pricing beds (MEASURED). Format check and CI clippy passed. The gate includes trees, debris, network destruction, buildings, ballistics, aim, recording, headless checks and documentation citations. Whole-library and whole-tree final guards remain to run before the final commit.
