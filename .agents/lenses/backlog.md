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
| load-31 | P3 | `src/tank/servo.rs::advance_servo` | base-truths: Stated law unwitnessed | Hand-arm friction-only braking and arrival have no witness though §7c claims them, and `slew == 0` erases stored momentum in one tick: a Gunner HP pool emptied at .55 rad/s stops dead while quality .001 coasts | State a brake fact or let friction stop it; witness the hand arm | ratchet |

## net

| id | P | file::symbol | lens: class | finding, one line with the concrete case | fix, one line | owner |
|---|---|---|---|---|---|---|
| net-04 | P2 | `src/net/fire_presentation.rs::recover_unannounced_rounds` | error-handling: Outside input reaching a default | An out-of-range disclosed round index logs refusal but presents index zero instead. | Validate indices against handshake-matched construction at ingress and return a typed refusal. | ratchet |
| net-05 | P2 | `src/net/spawn_map.rs::spawn_map_overlay` | error-handling: Late check | A decodable 16-bit grayscale `map_ui.png` reaches play and supplies an incompatible integer texture to the UI binding. | Refuse incompatible formats before play, naming the asset and required conversion. | ratchet |
| net-09 | P2 | `src/net/spawn_map.rs::SPAWN_LIMIT_FRACTION` | base-truths: Bare number in code | Fixed map-relative clearance does not guarantee grounding for a larger hull or smaller map. | Derive the admissible region from terrain extent and constructed footprint, shared by client and authority. | ratchet |
| net-10 | P2 | `src/net/spawn_map.rs::ui_map_shows_the_world_the_click_math_points_at` | base-truths: Asset-specific gate | Valid symmetric maps fail the mirror-correlation requirement; flat maps make correlation undefined. | Prove coordinate orientation with an asymmetric inline fixture. | ratchet |
| net-11 | P3 | `src/net/matchmaking/worker.rs::Reply`, `TicketResponse::answer` | error-handling: Erased error | HTTP, decoding, resolution, thread-start and malformed-answer failures cross live workflow boundaries as strings. | Preserve typed variants until the UI or logging boundary. | ratchet |
| net-12 | P3 | `src/net/tests/net_fire_release.rs::Fix` | code-branching: Test arm of a deleted path | `HeldLast` retains and executes a retired detector alongside production `ForTick`. | Delete the retired implementation and selector; retain regression inputs against production `attest`. | ratchet |
| net-15 | P3 | `src/net/resync.rs::hold_unknown_history` | base-truths: Compensation | Unknown retained history is treated as removal, then repaired by restoring components and maintaining markers. | Represent unknown history separately in interpolation's presence decision; delete removal-and-restoration repairs. | ratchet |
| net-17 | P3 | `src/net/hit_feel.rs::CUE_RETAIN` | base-truths: Bare number in code | Confirmation-marker and flash decay consume embedded retention and cutoff tastes. | Move them into required presentation data naming their effect families. | ratchet |
| net-22 | P2 | `src/net/protocol.rs::apply_servo_angles`, `src/tank/servo.rs::drive_servos` | error-handling: One timeline | Non-owner angles apply at arrival while hulls render at the shared delay (ADR-0037), so a remote's turret leads its hull by the delay. Older than this chain; one reviewer rated it P1. About .6° at 5.85°/s and an ASSUMED 100 ms (DERIVED); a look call. Remote twin of issue 187 | Decide with issue 187: one presentation clock for angles | ratchet |

## view

| id | P | file::symbol | lens: class | finding, one line with the concrete case | fix, one line | owner |
|---|---|---|---|---|---|---|
| view-04 | P2 | `src/render_scale.rs::prepare_render_scale` | error-handling: Dropped error | Terminal shader compilation failure clears the override and leaves native resolution indefinitely; the later failure handler never matches. | Distinguish pending compilation from terminal errors and fail explicitly for a broken shipped shader. | ratchet |
| view-12 | P3 | `src/sfx/mod.rs::SPATIAL_SCALE` | base-truths: Bare number in code | Falloff, ear separation, mix levels, engine slew and playback bands are embedded presentation tastes. | Move them into required audio data with separate job labels. | ratchet |
| view-13 | P3 | `src/terrain_shadow.rs::SUN_ANGULAR_RADIUS_RAD` and penumbra floor | base-truths: Bare number in code | Solar radius and minimum penumbra lack distinct required classifications. | Put the solar fact in sourced environment data; classify the floor separately and retain code ownership only for justified resolution. | ratchet |


## dev

| id | P | file::symbol | lens: class | finding, one line with the concrete case | fix, one line | owner |
|---|---|---|---|---|---|---|
| dev-13 | P2 | `scripts/cost/analyze.py::parse` | error-handling: Dropped error | A malformed interior row is counted but never reported, so missing expensive ticks skew cost percentiles and the CLI succeeds. | Refuse malformed interior rows with file and line; name any permitted torn tail. | ratchet |
| dev-14 | P2 | `scripts/cost/analyze.py::main` | error-handling: Invented default | A trace declaring `tick_hz: 128` gets duration at 128 Hz but per-round cost at a hardcoded 64 Hz, a factor of two (DERIVED). | Require the trace's tick rate once and carry it into every conversion. | ratchet |
| dev-15 | P2 | `scripts/perf/pencam_cost.py::parse_marks_has` | error-handling: Silent exit | An x-ray opening parses as `glass-on` but the paired-off guard searches `glass`, so an off-run with inherited `OVERMATCH_XRAY` contaminates the baseline and passes. | Recognize every opening mark of the surface and refuse a contaminated off-run. | ratchet |
| dev-16 | P2 | `scripts/tank/trio.py::coherence` | error-handling: Outside input reaching a panic | Certificate `"chains": {"Hull#0": []}` gets a named shape failure, then an uncaught `AttributeError` at `chain.get`, hiding the refusal. | Parse the certificate into checked shapes at admission; skip checks whose shape is invalid. | ratchet |
| dev-17 | P2 | `scripts/tank/test_build.py::TigerCensus`; `ShippedTrio` | base-truths: Asset-specific gate | Named wheel primitives with exactly eight node references, and pinned wheel and ammunition names, go red on a valid construction or naming change. | Derive membership and counts from the document; assert census and dedup relationships. | ratchet |
| dev-19 | P3 | `scripts/hooks/pushed_assets.sh::assets_push_targets` | code-branching: Kept alternative | The retired pre-push selection and hydration route stays implemented and tested with no production caller; CI uses the range route. | Delete the route, its exclusive helpers and test arms; keep the shared predicates and CI-range evidence. | ratchet |
| dev-20 | P3 | `scripts/tank/trio.py::coherence` | brevity: Repeated why | The inline paragraph repeats the docstring's reason that hashes exclude certificate-body claims and verification checks constructive laws. | Delete the inline paragraph; net 4 lines (DERIVED). | ratchet |

## destruction

| id | P | file::symbol | lens: class | finding, one line with the concrete case | fix, one line | owner |
|---|---|---|---|---|---|---|
| destruction-05 | P2 | `src/terrain_blend.rs::BLEND_LAYERS`, `BASE_GROUND`; `src/terrain_grid.rs` base-pack selection | base-truths: Named asset in logic | Replacing a texture pack's scale or ground material requires Rust edits; otherwise rendering and contact properties retain the previous pack's facts. | Declare pack identity, dimensions and ground membership in shared validated surface data. | ratchet |
| destruction-08 | P3 | `src/terrain_blend.rs::AO_BIAS` and blend appearance constants | base-truths: Bare number in code | AO bias, blend width and macro-tint settings are visual choices compiled into Rust. | Move them into required terrain `Taste` data with admission checks. | ratchet |
| destruction-09 | P2 | `src/terrain_grid.rs::SPAWN_FOOTPRINT_HALF_M`, `SPAWN_CLEARANCE_M` | base-truths: Per-asset constant | A valid larger hull can extend beyond the sampled square's fixed half-side or below the fixed spawn-root clearance and intersect terrain at spawn. | Derive footprint and minimum root height from baked construction; put any intended drop clearance in labelled spawn data. | ratchet |

## ballistics

| id | P | file::symbol | lens: class | finding, one line with the concrete case | fix, one line | owner |
|---|---|---|---|---|---|---|
| ballistics-24 | P3 | `.agents/scratch/research/walk-panics-2026-11-03.md` Reach | base-truths: Claim beyond the evidence | "No flight that did not panic changed": runs go unpaired after a panic, and agreement omits a changed `SampleSeed.t`, which `sitting_reach` reads. | Pair flights by seed, compare `t`, or narrow the claim. | ratchet |

| ballistics-30 | P3 | `assets/ballistics/spall_cone.ron::cone_emitted_hp_per_joule` | base-truths: Retrospective calibration | The unit is set against a retired reference cone of 395 HP, a target with no remaining owner. | State the crew-station outcome it aims at as a balance decision, or replace it with the fragment-hit law it names. | ratchet |

## driving

| id | P | file::symbol | lens: class | finding, one line with the concrete case | fix, one line | owner |
|---|---|---|---|---|---|---|
| driving-12 | P2 | `src/track/route.rs::sag_span_at` endpoint exemption | error-handling: Silent exit | Radial `on_circle` also exempts a lower-pole contact: DERIVED real-arithmetic witness `from=(0,0.1)`, `to=(-2,0.1)`, excess `0.02`, circles `[((-2,0),0.1), ((0,0.22),0.12), ((0,0),0.1)]` lifts the next sample to the upper hemisphere and cuts about 117.7 mm against the 1 mm contract; this is not a Rust-run measurement. | Represent endpoint support ownership explicitly, retain clipping for lower-pole contacts, and add the counterexample as a route regression test. | ratchet |
