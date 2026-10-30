# One draw per belt side from a frame table, issue #172

Each side's belt is one mesh entity. Its `BeltFrames` table (`src/track/link_view.rs`) holds one
span (two drawn pin joints) per MATERIAL link, so record `m` stays on link `m` for the belt's life.
The CPU fit is unchanged; `BeltFrames::fit` replaces the per-entity placer and marks the table only
when a record changes. The GPU draws the table (`src/track/belt_draw.rs`): the rung's shoe repeated
once per link with a per-vertex link tag, the shoe's own `StandardMaterial` extended by `BeltShoes`,
whose vertex stage (`assets/shaders/track_belt_mesh.wgsl` through `track_belt.wgsl`) places each
shoe rigidly from its span. The sim is untouched (ADR-0014): no sim, aim or collider file changed.

DERIVED: 194 shoe entities per tank become 2 belt entities; no per-link transform is written. GPU
triangles are unchanged.

## Adapter choice

Both adapters read the same table. The custom-instancing adapter was prototyped as far as the
bevy 0.19 source, which settled it:

- bevy's instancing example (`examples/shader_advanced/custom_shader_instancing.rs`, v0.19.0) queues
  its own `Transparent3d` item with its own pipeline and disables frustum culling. The shoe needs
  the opaque, depth-prepass and shadow phases with the bindless `StandardMaterial` bind group, and
  the x-ray's look and mask materials: all of that would be rebuilt by hand.
- The material route rides every existing phase. An extension's prepass vertex shader also makes
  bevy bind the material in its depth-only prepass and shadow paths
  (`vendor/bevy_pbr-0.19.1-scalar-math/src/material.rs`, `prepass_reads_material`), which the
  shadow pass needs to place shoes before the ribbon takes over. The only extra plumbing is one
  vertex attribute appended in `specialize`.

Cost of the chosen adapter: GPU memory for the repeated meshes, one set per link count shared by
every belt. MEASURED log line: `5 rung meshes of 97 links per side built; shoe reach 0.374 m`.

## Consumers

| Reader | How it is served |
|---|---|
| Rung selection (`select_belt_rungs`) | Nearest link origin from the table through the belt's `GlobalTransform`, the same arithmetic propagation did; writes the belt's one `Mesh3d`, looked up by count and rung. |
| Prints (`track_prints::stamp_shoes`) | Each link's pose = belt `GlobalTransform` × `BeltFrames::pose(m)`, the composition propagation gave a per-link child (bit-identical inputs); memory keyed `ShoeKey(belt, m)`. No GPU readback. |
| Shadow ribbon handoff | Unchanged ribbon; `PROXIED_CASTER` goes on the belt entity when the ribbon lands. Before it lands (or under `OVERMATCH_TRACK_SHADOW_PROXY=off`) the belt casts through its prepass vertex stage. |
| X-ray look and mask copies | A belt is dressed in `BeltLookMaterial` (its tier's `LookMaterial` + `BeltLook`, `assets/shaders/pen_cam_look_belt.wgsl`), its mask copy likewise, with `NoFrustumCulling`; restore puts the belt material back. |
| Culling | `NoAutoAabb` plus an `Aabb` from the table: every anchor ± the shoe's canonical reach. |
| Showcase | `ShoeBelt::pin` unchanged. |
| Sandbox | `track_sandbox::link_view` spawns one belt per side, fits with `Stragglers::Park`, resizes the table and swaps the draw on a live link-count change, and keeps its explicit visibility switch on the belt. |
| Hot reload | `rebind_on_reinstance` despawns each belt; its buffer and material handles go with it. |
| Separate pen-cam stage | Untouched: it never read the live pool. |

## Gates

MEASURED: `cargo fmt --all --check` pass; `scripts/serial-cargo clippy --locked --all-targets
--features dev_ui,bitprobe -- -D warnings` pass; `scripts/serial-cargo test --lib --features
dev_ui,bitprobe -- damage_cam::pen_cam track:: track_prints render_policy`: 321 passed, 1 ignored.

New or rewritten tests: `a_pitch_of_travel_walks_every_link_one_station` (stable identity and phase,
both directions, whole loop), `a_table_keeps_one_record_per_link_and_reports_only_real_changes`
(count, stragglers, no change on refit), `the_shader_placement_is_the_cpu_placement`,
`settled_standing_tank_writes_no_belt_wheel_or_ribbon_data` (a moving belt writes both tables and no
transform but the running gear's), `every_copy_of_the_repeated_shoe_is_the_shoe_tagged_with_its_link`,
`a_resized_belt_takes_its_new_counts_mesh_at_its_rung`, and the selector tests on tables.

No frame-time measurement or A/B/B/A run was taken (the orchestrator prices this).

## Pixel parity

Hidden captures, same assets, `capture.sh` (the #155 options: fixed `1/128 s` step, `1280x720`, no
UI or gizmos). Scenes, ASSUMED capture configuration: washboard course standing (12.5 s) and moving
(14 s) with the camera at offsets `(8, 1.6, 2)` (near, rung 0/1), `(35, 5, 10)` (mid) and
`(85, 8, 20)` (far, rung 4); the near shot with the x-ray on (`OVERMATCH_XRAY=11-20`); the printed
map (`kalinovo`, prints demo) standing and moving.

THE BASELINE IS NOT REPEATABLE. Two runs of the baseline binary differ by tens of thousands of pixels
(column "baseline control"); two runs of the candidate are identical. The cause of the baseline's
variance is not isolated. ASSUMED: draw order among 194 overlapping shoe entities, or rung/spawn
timing, varies with asynchronous loading; one mesh per side draws in fixed index order. Each
candidate still is therefore compared with the closer of the two baseline runs.

MEASURED whole-frame counts, `pixels differing at all / over 8/255 / max channel delta`
(`matrix.json`, `matrix.py`):

| Still | Baseline control | Candidate control | Candidate vs baseline |
|---|---|---|---|
| near 12.5 s | 54493 / 5320 / 79 | 0 / 0 | 306 / 4 / 15 |
| near 14 s | 84837 / 8415 / 106 | 0 / 0 | 217 / 5 / 11 |
| mid 12.5 s | 6745 / 1865 / 112 | 0 / 0 | 3 / 0 / 1 |
| mid 14 s | 19139 / 2148 / 130 | 0 / 0 | 8 / 1 / 10 |
| far 12.5 s | 2364 / 283 / 90 | 0 / 0 | 2830 / 412 / 124 |
| far 14 s | 4331 / 271 / 117 | 0 / 0 | 8322 / 575 / 154 |
| x-ray 12.5 s | 20725 / 1763 / 78 | 0 / 0 | 636 / 12 / 31 |
| x-ray 14 s | 25609 / 1901 / 89 | 0 / 0 | 549 / 8 / 40 |
| printed 12.5 s | 68934 / 6770 / 112 | 0 / 0 | 423 / 5 / 23 |
| printed 16 s | 53914 / 4840 / 93 | 0 / 0 | 276 / 0 / 8 |

Single-run pairs, MEASURED: the LOD showcase (`OVERMATCH_LOD_SHOWCASE=1`, 10 s) 1718 / 8 / 45; the
near standing shot with the shoes casting (`OVERMATCH_TRACK_SHADOW_PROXY=off`) 340 / 3 / 15, against
37309 / 3300 between that shot and the ribbon-cast one, so the belt's prepass shadow matches the
shoes'.

Causes:

- Near, mid, x-ray, printed, showcase: scattered single pixels on shoe edges and facets. The shader
  places `swing(correction · (p − origin)) + anchor` and then the belt's world transform, where
  bevy multiplied one CPU-composed matrix; the two agree to float rounding (DERIVED; the test holds
  them within 1e-5 m) and a rounding step flips edge pixels. Prints: the CPU inputs are
  bit-identical by construction, and the printed stills sit at the same level.
- Far: the belt and wheel rows differ less than the baseline differs from itself; the excess is the
  turret and gun, rows 330–342 of the frame (`stills/far-00014.0000-difference-8x.png`): the gun is
  laid on a different bearing. No aim, sim or collider code changed. UNVERIFIED hypothesis: at 87 m
  the screen-centre aim ray passes through the own tank, so the aim point lies a few metres from the
  turret and its bearing is ill-conditioned; 194 fewer entities per tank change entity allocation
  and any entity-ordered iteration downstream. Owner review.

`stills/` holds unscaled crops of the full frames: each pair's baseline (the closer run), candidate
and an 8x absolute difference. Full frames regenerate with:

```sh
export OVERMATCH_CARGO_LOCK_WAIT_S=10800
scripts/serial-cargo build --locked --bin overmatch --features dev_ui,bitprobe
.agents/scratch/perf-2026-10-29/track-belt-172/capture.sh target/debug/overmatch target/track-172/after
python3 -I .agents/scratch/perf-2026-10-29/track-belt-172/matrix.py target/track-172 > matrix.json
python3 -I .agents/scratch/perf-2026-10-29/track-belt-172/crops.py target/track-172 stills
```

(`before/`, `before2/` from the baseline `57052e2`, `after/`, `after2/` from the candidate.)

Not run: the track sandbox binary (it has no hidden-window lever) and a GLB hot reload; both paths
compile and share the game's table, fit and draw code.
