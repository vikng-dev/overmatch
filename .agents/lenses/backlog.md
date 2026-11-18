# Lens backlog

This is every open lens finding in the tree that no lane has closed; findings about the game's damage model concern game state only, with crew represented as HP pools at stations.
A lane that edits a listed file closes that file's rows in the same change, verifying each first; a row found not to be a defect is deleted with the reason in the commit.
P1 rows have their own lanes.
This file only shrinks, and a closed row is deleted, never struck through.

## load

| id | P | file::symbol | lens: class | finding, one line with the concrete case | fix, one line | owner |
|---|---|---|---|---|---|---|
| load-02 | P2 | `src/geometry_lod.rs::resolve_chains` | error-handling: Silent exit | A certificate chain for nonexistent `"Missing#0"` passes validation because resolution visits only existing meshes. | Require every declared source and rung to resolve; name the certificate, key and repair. | ratchet |
| load-24 | P3 | `src/settings/ui.rs::tests::the_selection_skips_disabled_rows` | testing: Coupled assertion | A permitted `defaults.vsync: Off` taste change fails the ON premise before testing selection, though row skipping is unchanged. | Set the disabled-cap fixture to explicit `VsyncMode::On` rather than pinning the authored default. | ratchet |
| load-30 | P3 | `src/tank/view.rs::bind_tank_view` | testing: Unwitnessed arm | The late-view path (a view arriving after the launch) has no test: `WorldInstanceReady`'s `InstanceId` has no public constructor, so a witness needs the real spawner over a loaded world asset | Witness it through the world-asset spawner | ratchet |

## net

| id | P | file::symbol | lens: class | finding, one line with the concrete case | fix, one line | owner |
|---|---|---|---|---|---|---|
| net-05 | P2 | `src/net/spawn_map.rs::spawn_map_overlay` | error-handling: Late check | A decodable 16-bit grayscale `map_ui.png` reaches play and supplies an incompatible integer texture to the UI binding. | Refuse incompatible formats before play, naming the asset and required conversion. | ratchet |
| net-09 | P2 | `src/net/spawn_map.rs::SPAWN_LIMIT_FRACTION` | base-truths: Bare number in code | Fixed map-relative clearance does not guarantee grounding for a larger hull or smaller map. | Derive the admissible region from terrain extent and constructed footprint, shared by client and authority. | ratchet |
| net-10 | P2 | `src/net/spawn_map.rs::ui_map_shows_the_world_the_click_math_points_at` | base-truths: Asset-specific gate | Valid symmetric maps fail the mirror-correlation requirement; flat maps make correlation undefined. | Prove coordinate orientation with an asymmetric inline fixture. | ratchet |
| net-11 | P3 | `src/net/matchmaking/worker.rs::Reply`, `TicketResponse::answer` | error-handling: Erased error | HTTP, decoding, resolution, thread-start and malformed-answer failures cross live workflow boundaries as strings. | Preserve typed variants until the UI or logging boundary. | ratchet |

## view

| id | P | file::symbol | lens: class | finding, one line with the concrete case | fix, one line | owner |
|---|---|---|---|---|---|---|


## dev

| id | P | file::symbol | lens: class | finding, one line with the concrete case | fix, one line | owner |
|---|---|---|---|---|---|---|
| dev-16 | P2 | `scripts/tank/trio.py::coherence` | error-handling: Outside input reaching a panic | Certificate `"chains": {"Hull#0": []}` gets a named shape failure, then an uncaught `AttributeError` at `chain.get`, hiding the refusal. | Parse the certificate into checked shapes at admission; skip checks whose shape is invalid. | ratchet |
| dev-17 | P2 | `scripts/tank/test_build.py::TigerCensus`; `ShippedTrio` | base-truths: Asset-specific gate | Named wheel primitives with exactly eight node references, and pinned wheel and ammunition names, go red on a valid construction or naming change. | Derive membership and counts from the document; assert census and dedup relationships. | ratchet |
| dev-19 | P3 | `scripts/hooks/pushed_assets.sh::assets_push_targets` | code-branching: Kept alternative | The retired pre-push selection and hydration route stays implemented and tested with no production caller; CI uses the range route. | Delete the route, its exclusive helpers and test arms; keep the shared predicates and CI-range evidence. | ratchet |
| dev-20 | P3 | `scripts/tank/trio.py::coherence` | brevity: Repeated why | The inline paragraph repeats the docstring's reason that hashes exclude certificate-body claims and verification checks constructive laws. | Delete the inline paragraph; net 4 lines (DERIVED). | ratchet |

## destruction

| id | P | file::symbol | lens: class | finding, one line with the concrete case | fix, one line | owner |
|---|---|---|---|---|---|---|

## ballistics

| id | P | file::symbol | lens: class | finding, one line with the concrete case | fix, one line | owner |
|---|---|---|---|---|---|---|
| ballistics-24 | P3 | `.agents/scratch/research/walk-panics-2026-11-03.md` Reach | base-truths: Claim beyond the evidence | "No flight that did not panic changed": runs go unpaired after a panic, and agreement omits a changed `SampleSeed.t`, which `sitting_reach` reads. | Pair flights by seed, compare `t`, or narrow the claim. | ratchet |

| ballistics-30 | P3 | `assets/ballistics/spall_cone.ron::cone_emitted_hp_per_joule` | base-truths: Retrospective calibration | The unit is set against a retired reference cone of 395 HP, a target with no remaining owner. | State the crew-station outcome it aims at as a balance decision, or replace it with the fragment-hit law it names. | ratchet |
