# Lens b7 fix disposition

Subject: the video game's environment destruction and damage model; game state only.
All seven assigned findings were verified in the current source. None was rejected.

| Finding | Disposition |
|---|---|
| 1 — graybox substance | Fixed in `scatter: let belts read graybox solid surfaces (#106)`. Grayboxes use the common solid-collider belt path, carrying authored friction and the shared rigid-surface rolling law. Their bounds remain spawn obstacles. The belt witness checks actual contacts over two different underlying soils. |
| 2 — peer input | Fixed in `net: refuse malformed construction streams visibly (#106)`. Duplicate ledgers, malformed batches and malformed pieces produce typed refusals through the match-close/lobby path. Admission commits a complete ledger batch or nothing; checked piece geometry binds its map owner, chunks, scale and origin before presentation. Unknown ordinals refuse once the map is selected; future generations still wait for selection. |
| 3 — parallel owners | Fixed in `construction: prepare contacts and colliders by owner (#106)`. Contact conversion and collider/hull preparation use the compute pool per owner. Contact results collect by original source index; structural commands retain query order and each owner's element-then-chunk sequence. The existing flush and AABB refit preserve same-update collision visibility. |
| 4 — record shortening | Fixed with the peer boundary. Generation changes alone reset derived collision and presentation state. Same-generation shortening asserts, including future entries the presentation clock has not released. |
| 5 — historical HP oracle | Fixed in `ballistics: keep game damage-model totals as observations (#106)`. Remove the fixed-total test; retain the generic energy-times-owned-unit tests and the existing emission measurement bed. The field unit and its balance justification are unchanged. |
| 6 — brevity | Remove the unused scatter material-store borrow, redundant surface type comment and REV 86 diary paragraph; condense the field-dose test rationale and share the two grip tests' identical setup. The remaining cleanup is in `quality: state construction and grip invariants once (#106)`. |
| 7 — bake entry point | Fixed in `construction: gate the bake write entry point (#106)`. `bake_to` owns bake/admit/write for both the CLI and the filesystem fixture. The fixture supplies an absent output root, asserts the game's diagnostic and proves no directory or artifact is created. The direct admission API test remains. |

## Additional checks and decisions

- The same peer boundary refuses a shrinking or rewritten ledger prefix, empty or unordered piece groups, missing generation tags, and changes to an admitted carrier's identity. Retirement remains a valid update.
- The checked piece origin uses a positive mass sum and f64 accumulation, removing the old epsilon substitute for missing mass. Its chunks come from the admitted map data.
- DERIVED: graybox contacts now use exact collider footprints rather than the analytic field's rounded boxes. This deliberately selects the existing generic solid-surface law; it adds no graybox-specific physical rule.
- DERIVED: the grip fixture extraction preserves both previous call sequences: one zero-velocity tick followed by twenty identical creep ticks. Both original invariants remain asserted.
- Verified and closed backlog row `ballistics-34`: the view has no hold-visibility gate. Rows `ballistics-31` through `ballistics-33`, `net-21`, `net-22` and `view-21` concern untouched items and remain open.
- The excluded tree replacement, piece-reaction clamp, grip dwell, shear floor and field-unit balance decision remain with their owners.

## Evidence

- MEASURED: the construction, surface, driving, field and whole-tree guard rerun passed 313 tests, with four measurement tests ignored. The final focused receive/surface/guard run passed 67 tests.
- MEASURED: every headless check passed in the initial combined run; its only failure was the new scatter fixture registering Avian requirements after spawning collider archetypes. Registering physics before spawning and advancing explicit fixed steps corrected that fixture, and the belt witness passed on rerun.
- MEASURED: both contact and collider preparation produced equal results with one and two worker threads. The existing construction collision/refit witness passed.
- MEASURED: the real match lifecycle removed the link, stayed disconnected beyond the reconnect window, returned to the lobby and rendered the construction-refusal cause in its `Text`.
- MEASURED: all 47 construction-tool tests passed. Temporarily bypassing `bake_to`'s admission call made the filesystem witness fail with `a refused bake created its output root or artifacts`; its cleanup removed that deliberately created temporary output. Production admission was restored byte-for-byte and all 47 tool tests passed again.
- MEASURED, the game's damage model: the observation bed reports 396.513336 HP for the reference cone and 13,920.837891 HP for the shipped reference ring. Neither total is a pass condition.
- Formatting and workspace/all-target CI clippy passed. MEASURED: the final whole-tree `doc_citations`, `ui_ascii`, `net_boundary` and `bake::` guard run passed all 48 tests.

Bevy 0.19 and Avian 0.7 APIs were checked against pinned local sources. Replicon's receive code also explicitly handles locally despawned replica entities. No pricing, captures, visible clients or pushes were run.
