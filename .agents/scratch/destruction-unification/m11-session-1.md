Subject: **the video game's environment destruction model, game state and presentation only.**

M11 session 1 activates the prepared construction data on the live map. This is an unmerged
step on the five-session lane; the legacy system's removal belongs to the remaining sessions.

## Activation

- `assets/maps/kalinovo/prototypes.ron` selects `Construction` for every fir placement. Map admission
  now loads the prepared construction geometry, green-wood material, declared attachments and
  dressing, and checks every authored scale through `ConstructionLibrary::admit_placement`.
- The existing common scatter path builds complete `Breakage` synchronously, one intact collider
  per element, spawn obstacles, and the shared construction mesh at each instance's scale. No
  additional simulation constructor, detachment route or view activation switch was needed.
- `ConstructionLibrary::load` retains the required dressing text beside the geometry text for
  `map::content_digest`. The digest witness independently reads both files of every admitted kind;
  omitting either file fails compatibility. No legacy tree files enter the live map's digest.
- Scatter tests exercise common construction placement, full pools without view assets, identity,
  graybox surface-evidence markers, shared meshes/material and instance scale. The production
  headless composition proves the live map has complete common state and no legacy tree roots.
- The old headless segmented-body witness is replaced by the live activation census and
  `admitted_map_kinds_detach_with_common_centroid_local_colliders`. The latter mounts the production
  common plugin for each placed construction kind, isolates an above-ground chunk as fixture
  support loss, and verifies stable identity, a dynamic owner, direct convex contact geometry,
  `ColliderOf` ownership and moving bounds. The existing column lifecycle witness still proves
  retained wear/holes, later splits, physics motion and replica reconstruction.
- The battle lane-clearance test treats trunks as construction footprints. Its runtime tree-only
  counters remain for the planned diagnostic deletion; no driving law or grip parameter changed.

## Evidence

MEASURED, the shipped headless map census:

| Map / kind | Instances | Intact construction colliders | Colliders per instance |
|---|---:|---:|---:|
| Kalinovo / fir_1 | 635 | 8,255 | 13 |
| Kalinovo / house_1 | 74 | 4,070 | 55 |
| Kalinovo total | 709 | 12,325 | — |

These are observations of the spawned world, not pinned counts in tests. Terrain and vehicle
colliders are outside this construction census. The no-view and common-detachment witnesses pass
for both admitted kinds. The trunk remains its authored stacked bands; collider consolidation and
pricing remain M12 and the orchestrator's work.

MEASURED: the first verification run passed **308 tests**, with **7 existing ignored tests**, covering
`map::`, `scatter::`, `construction::`, all of `headless_test`, `doc_citations`, `ui_ascii`,
`net_boundary` and `bake::`. Workspace/all-target CI-feature clippy and formatting passed.
MEASURED: the final recheck passed **98 tests**, with none ignored, covering the affected map and
scatter tests, `tank::battle::`, authored spawn-point clearance, `net::world_reset::` (including its
existing real-UDP lifecycle fixture), and the required `doc_citations`, `ui_ascii`, `net_boundary`
and `bake::` guards. Final CI-feature workspace/all-target clippy and formatting passed. These runs
overlap; their counts are not a count of distinct tests.

All compilation and test execution used `scripts/serial-cargo` with the required features and
machine lock. Test processes were unattended, headless and muted. No frame/tick pricing,
captures, UDP drive or owner eye proof was taken in this session.

## M13 baseline

[`m11-pre-cutover-manifest.json`](m11-pre-cutover-manifest.json) records the exact pre-cutover parent
in `parent_sha`, as explicitly requested, and its subject, **construction: wood view on the common
view, dressed from data (#106)**. It also records the path, byte length and SHA-256 of every tracked
asset's working bytes before activation: **169 files, MEASURED**, including hydrated LFS content.
The planning baseline remains owned by `unification-plan-2.md`; it is not the runtime cutover parent.

## Handoff and plan corrections

Session 1 is activation only. Session 2 must begin with the orchestrator-authorized rebase after
`lens-b7-fix` lands, build on its admitted ledger descriptor type, consolidate identities/causes,
carriers and reconstruction, presentation-time membership, reconnect/reset and registration, and
bump `PROTOCOL_REV` deliberately. This session does not edit those concurrent functions.

The legacy `Prototype::Tree`, map library fields, scatter branch and independent `mixed_fixture_manifest`
remain because the separate ledger/reset tests still exercise those contracts. The common scatter
tests use `construction_fixture_manifest`; consolidate that fixture when the streams consolidate.
The old tree modules, scheduling, ballistics hooks, diagnostic consumers, assets and glossary
remain for sessions 2–4, as dispatched. There is no new runtime choice or compatibility adapter.

The plan's one-collider intact trunk expectation does not describe the admitted bake: the measured
count above confirms the dispatch's stacked-band correction. M8 also required dressing files but
had not included their bytes in the map digest; activation repairs that compatibility gap.

Session 5 still owns the joined production detachment/belt witness and compound-constructor mutant,
the original `reproduce.py` / `nearest.ron` UDP drive, observer/reconnect proof and deletion census.
This session's detachment test does not claim to supply that joined proof. No owner decision is
needed here. `net-22`, `servo.rs`, `load-27`, `load-31`, grip dwell, `GRIP_SHEAR_FLOOR_M` and
`piece_impulses` are untouched. No lens-backlog row names the edited files; no row is closed by
this activation. The diff was self-reviewed against all six lenses. Remaining explicitly budgeted
ratchet work stays in its dispatched session.
