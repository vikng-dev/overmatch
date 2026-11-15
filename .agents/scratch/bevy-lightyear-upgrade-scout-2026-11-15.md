# Bevy 0.20 + lightyear 0.31 upgrade scout (2026-11-15)

Read-only scout at `main` 252939a0. Scope: Bevy, lightyear and every other Bevy-coupled dependency.
The Avian 0.8 rebase of `vendor/avian3d-0.7.0-child-pose` belongs to a separate scout whose notes
(`.agents/scratch/avian-0.8-rebase-scout-2026-11-15.md`) did not exist when this was written; nothing
here prices that rebase.

Labels: MEASURED = read from a registry, a tag, a file or a grep on the date above. DERIVED = my
inference from measured facts; the inputs are named. Call-site counts are `grep -rE --include=*.rs`
over `src tools build.rs` with the pattern given; they count lines, comments included, so they are
upper bounds. `backend/` is TypeScript (MEASURED: `backend/package.json`), outside this upgrade.

Sources used throughout:

- crates.io API (`/api/v1/crates/<name>`, `/<name>/<ver>/dependencies`) and the published `.crate`
  sources of both versions, diffed locally.
- Bevy migration guide: `bevyengine/bevy-website` `content/learn/migration-guides/0.19-to-0.20.md`
  (75 items, MEASURED `grep -c '^### '`). Cited below as MG "<heading>".
- Bevy release notes: `bevyengine/bevy` tag `v0.20.0`, `_release-content/release-notes/*.md`.
- lightyear: `cBournhonesque/lightyear` compare `0.30.1...0.31.0` (18 commits, MEASURED via the
  GitHub compare API) and the published crate diffs.

## 1. Release state today

| Crate | We pin (Cargo.lock) | Latest released | Targets |
|---|---|---|---|
| bevy | 0.19.1 | **0.20.1** (2026-10-28); 0.20.0 was 2026-10-08, rc.1 2026-09-15 | - |
| lightyear | 0.30.1 | **0.31.0** (2026-10-21) | bevy ^0.20, avian3d ^0.8 (via `lightyear_avian3d`), bevy_replicon ^0.45.2 |
| avian3d | 0.7.0 (vendored fork) | **0.8.0** (2026-10-20) | bevy ^0.20, parry3d ^0.31, obvhs ^0.4, bevy_heavy ^0.6 |
| bevy_replicon | 0.44.3 | 0.45.5 (2026-10-31) | bevy ^0.20 (0.45.x); 0.44.x is bevy ^0.19 |
| bevy_egui | 0.42.0 | 0.43.1 (2026-10-10) | bevy ^0.20, egui ^0.36 (unchanged) |

All MEASURED from crates.io. lightyear-to-Bevy mapping, MEASURED from each release's
`bevy_app` requirement: 0.30.x is bevy 0.19, 0.31.0 is bevy 0.20. lightyear has no GitHub
"release" object for 0.31.0 (latest release object is 0.30.0) but the tag `0.31.0` and the crates
exist; commit 8a926d6f.

No release after 0.20 exists and none is in RC. Bevy's 0.21 milestone is open (30 open, 34 closed
issues, MEASURED GitHub milestones API); 0.20.2 is a patch milestone with 1 open item.

**A consistent set exists right now:** bevy 0.20.1 + lightyear 0.31.0 + avian3d 0.8.0 +
bevy_replicon 0.45.5 + bevy_egui 0.43.1. Nothing upstream is missing. What is missing is ours: the
five vendored patches (section 2) have no 0.20-era versions yet.

**The set moves as one.** avian3d 0.7 requires bevy ^0.19 and 0.8 requires ^0.20;
`lightyear_avian3d` 0.31 requires avian3d ^0.8. There is no intermediate lockfile (DERIVED from the
requirements above): Bevy, lightyear, Avian, replicon and egui change in the same commit.

## 2. Bevy-coupled dependencies

### Direct dependencies (`Cargo.toml`)

| Dependency | Now | For Bevy 0.20 | Action |
|---|---|---|---|
| `bevy` (features `debug`, `jpeg`, `basis-universal`) | 0.19.1 | 0.20.1; all three features still exist (MEASURED `bevy-0.20.1/Cargo.toml`) | bump |
| `bevy_winit` (named only to fix the feature set) | 0.19 | 0.20.1 | bump; re-check the "second copy of bevy_winit" reason still holds with bevy_egui 0.43 |
| `avian3d` (`serialize`) | 0.7.0 fork | 0.8.0 | other scout |
| `lightyear`, `lightyear_transport`, `lightyear_sync`, dev `lightyear_inputs_native` | 0.30.1 | 0.31.0 | bump |
| `bevy_replicon` | 0.44 | 0.45.5 | bump (lightyear 0.31 needs ^0.45.2) |
| `bevy_egui` (`dev_ui`) | 0.42 | 0.43.1 | bump; changelog shows only the Bevy update and one input fix |
| `wgpu` (named for `settings::probe`) | 29.0 | bevy_render 0.20.1 needs ^30; latest 30.0.2 | bump to 30 |
| `glam` (`serde`, `scalar-math`) | 0.32.1 | bevy_math 0.20.1 needs ^0.33.2; latest 0.33.12 | bump; ADR-0028 pin, tripwire and `deny.toml` bounds move with it |
| `winit` | 0.30 | bevy_winit 0.20.1 still ^0.30 | none |
| `ron` | 0.12 | bevy_asset 0.20.1 still ^0.12 | none |
| `image`, `gltf`, `ttf-parser`, `sha2`, `postcard`, `libm`, `flate2` (`bitprobe`), `ureq`, `libc`, `objc-sys` | - | not Bevy-coupled, or "same locked version as Bevy's" comments to re-verify after the lock moves (`postcard`, `libm`, `sha2`, `objc2-app-kit`) | re-verify comments |

Transitive, engine-coupled, all with 0.20 releases (MEASURED crates.io): bevy_heavy 0.6.0,
bevy_transform_interpolation 0.6.0, bevy_enhanced_input 0.27.2, leafwing-input-manager 0.22.0,
aeronet_io 0.22.0, parry3d 0.31, obvhs 0.4.0. The text stack moves parley 0.9 to 0.11.
`bevy-inspector-egui` is not a dependency.

New Bevy crates in 0.20: `bevy_shape`, `bevy_curve`, `bevy_extract` (MEASURED: diff of
`bevy_internal` dependency tables).

### Vendored / patched crates (`[patch.crates-io]`)

Patch sizes are MEASURED `diff -ruN | diffstat` of `src/` against the pristine published crate.

| Vendored crate | Patch size | State in the target release | Action |
|---|---|---|---|
| `bevy_reflect-0.19.1-scalar-math` | 1 file, +4 -14 | bevy_reflect 0.20.1 still registers `BVec3A`/`BVec4A` with `Serialize, Deserialize` (`src/impls/glam.rs:502-515`). glam 0.33.2 still gates their serde impls on `not(scalar-math)`, but glam **0.33.12** imports them unconditionally (`src/features/impl_serde.rs`, `mod bool`). | **Drop** if the lock resolves glam >= the release that lifted the gate (0.33.12 confirmed; first such release not identified). Otherwise re-carry on 0.20.1. |
| `bevy_pbr-0.19.1-scalar-math` | 9 files, +330 -21 | Four separate changes, see below. | Re-port as `bevy_pbr-0.20.1-...` |
| `lightyear_netcode-0.30.1-admission` | 4 files, +141 -19 | Upstream 0.31.0 changed 28 lines of this crate, all in `server_plugin.rs`; `Client::denied_reason` and `ForeignProtocolHandler` are not upstream (MEASURED grep on 0.31.0). | Re-apply on 0.31.0; mechanical. |
| `lightyear_avian3d-0.30.1-child-pose` | 2 files, +10 -38 | Upstream 0.31.0 `update_child_collider_position` still writes every child unconditionally (`src/plugin.rs:910-937`); `rollback.rs` now snapshots a `MovedProxies` value (upstream crate diff: 6 files, +980 -267). | Re-port on 0.31.0 **after** the Avian fork: it delegates to the fork's `ChildColliderPoses` and `moved_proxies()`/`set_moved`. |
| `avian3d-0.7.0-child-pose` | 31 files, +1905 -342 | - | other scout |

`bevy_pbr` patch, item by item:

1. **`MeshUniform` `#[repr(C, align(16))]`.** 0.20.1 `MeshUniform` is still a bare
   `#[derive(ShaderType, Clone)]` (`src/render/mesh.rs:526`), but `UninitBufferVec::new` now sizes
   items by `T::SHADER_SIZE` instead of `size_of::<T>()` (`bevy_render-0.20.1/src/render_resource/buffer_vec.rs`,
   `UninitBufferVec::new`). DERIVED: the 164-vs-176-byte mismatch should be gone at its cause.
   Not proven: `src/tests/gpu_layout.rs` and a windowed Metal boot decide. Other Rust-size-allocated
   uploads (`RawBufferVec` still uses `size_of::<T>()`, line 57) need the same inventory the test
   already runs.
2. **`directional_shadow_factor` backport.** Upstream in 0.20.1 (`pbr_types.wesl:123,151`,
   `pbr_functions.wesl:604`). **Drop.**
3. **Heightfield shadow** (`heightfield_shadow.rs`, view bindings 39/40, `shadows.wgsl` receiver
   law). Not upstream. Group-0 bindings in 0.20.1 end at 38 (`mesh_view_bindings.wesl:112`), so 39
   and 40 are still free. The five patched shader files are now `.wesl` with `import package::...`
   syntax, so the WGSL edits are rewritten, not rebased. **Re-port; the largest vendored item.**
4. **Mesh-instance removal batching/skip.** 0.20.1 still sends one `removed_tx.send(entity)` per
   entity over `mpsc` (`mesh.rs:2543,2602,2698`). **Re-carry.** `collect_meshes_for_gpu_building`
   was restructured enough that this is a re-implementation against new code, guarded by
   `src/tests/bevy_mesh_removal_lifecycle.rs`.

`src/terrain_shadow.rs:557-559` `include_str!`s two vendored shader files by path and lifts text
between markers; the paths and the `.wgsl` extension change with the re-port.

## 3. Bevy migration surface (0.19 to 0.20)

One guide lies between us and the target. Game size for scale: 386 `.rs` files, 326,993 lines
(MEASURED `find src tools build.rs`). Items with zero hits that cannot matter here (BSN, Feathers,
Sprite backend, Atmosphere, OIT, `bevy_settings`, text input widgets, `FilteredResources`,
`WorldQuery` impls, `define_label!`, `ReflectFromPtr`, `FromType`, `AssetId::invalid`,
`MainEntityHashMap`, `CompositingSpace`, `SortedCamera`, `ExtractedUiNode`, `ComponentInfo`,
`with_luminance`, `CalculatedClip`, `TextReader`, `FontSource` variants, `Font::from_bytes`,
`BorderRadius { .. }` literals, `Val::resolve`, `IntoObserverSystem`, manual `SystemParam` impls)
were each grepped and returned 0; they are omitted from the table.

| Item (MG heading) | What changed | Call sites here (pattern) | Kind |
|---|---|---|---|
| Moved Observer events `B` Bundle generic | `On<Add, T>` becomes `On<Add<T>>`, same for `Insert`/`Remove`/`Discard`/`Despawn` | **72** in 35 files: 42 `Add`, 18 `Remove`, 7 `Insert`, 5 `Discard` (`On<(Add\|Insert\|Remove\|Replace\|Discard\|Despawn), ?[^>]+>`); 0 custom-event uses of the second generic | mechanical |
| WESL shaders | naga_oil is gone; a shader with `#import`/`#ifdef`/`#{..}` must become `.wesl`; Bevy module paths follow file paths (`bevy_pbr::forward_io` to `bevy_pbr::render::forward_io`); `#{MATERIAL_BIND_GROUP}` becomes `constants::MATERIAL_BIND_GROUP` | **17 of 17** files in `assets/shaders/`, 2,494 lines; 16 have `#import` (37 import lines), 81 `#ifdef`-family lines, 33 `#{..}` substitutions; 2 game-to-game imports (`track_belt.wgsl`, `construction_holes.wgsl`). 182 Rust lines reference shader paths/defs (`\.wgsl\|ShaderRef\|shader_defs\|ShaderDefVal\|...`) | mechanical per file, but every pipeline must be seen rendering; **needs verification by eye** |
| Query iterators from "many" entities | `par_iter_many*` closures receive `Result<Item, QueryEntityError>` | **6** (`iter_many\|par_iter_many`): `track/world_contact/admission.rs:120`, `net/debris_stress.rs:542`, `construction/breakage.rs:1952`, `debris/crumble.rs:670`, `construction/breakage/sweep_intake.rs:877,1225` | **design, small**: each site must pick `expect` (a bug) or a named legitimate absence. The guide's `let Ok(..) else { return }` is the silent fifth outcome AGENTS.md forbids. All six are server sim paths. |
| Flat pointer events | `Pointer<Click>` becomes `PointerClick`; fields move to `event.pointer.*` | **3** observers (`Pointer<`): `settings/ui.rs:1256`, `net/matchmaking/lobby_ui.rs:529`, `leave_ui.rs:104` | mechanical |
| `Tonemapping::None` skips the pass on both versions | DERIVED from 0.19.1 and 0.20.1 `bevy_core_pipeline/src/tonemapping/node.rs:46`; HDR also bypasses material-shader tonemapping | **1**: `damage_cam/pen_cam/look.rs` (HDR masks, `DebandDither::Disabled`) | Keep `None` for the game's damage-model depth masks; `Linear` would add a pass |
| `DepthAttachment`/`ViewDepthTexture`/`ViewPrepassTextures::depth` | renamed to `ViewDepthStencilTexture`; `depth` is `Option<DepthStencilAttachment>` with separate depth-only/all-aspect views | **1 file**, 5 lines: `src/scene_depth.rs` (constructs `ViewPrepassTextures` at :174, reads `ViewDepthTexture` at :186) | **design, small**: the module hands the main pass depth to the prepass slot; the view/aspect choice must be re-derived |
| Use ECS for render world window data | `ExtractedWindows` resource becomes `Query<&ExtractedWindow>` / `Query<&SurfaceData>` | **1**: `settings/probe.rs:73` | mechanical with care (raw surface handles) |
| `WgpuWrapper` removed | `RenderQueue`/`RenderAdapter`/`RenderInstance` fields private; still `Deref` | 6 use sites in 3 files, all `Res<..>` deref | likely none |
| Extraction is generic over worlds | `#[derive(ExtractComponent)]` needs `#[extract_app(RenderApp)]`; `ExtractResource` likewise | **3** derives: `scene_depth.rs:76`, `render_scale.rs:147,181` | mechanical |
| Several built-in schedules order sets weakly | `RenderSystems` sets, `Core3d` pass sets and `UiSystems` keep order only between systems whose tracked access conflicts | **9** of our systems sit in those sets (`in_set\(RenderSystems::\|Core3dSystems::`): `render_cost.rs:345`, `scene_depth.rs:110,132`, `frame_cost.rs:208`, `render_scale.rs:205,220`, `track_prints/upload.rs:100`, `damage_cam/pen_cam/picture.rs:1248`, one test | **review each**. `track_prints/upload.rs` and `pen_cam/picture.rs` write through `Res<RenderQueue>` (interior mutability, untracked): DERIVED risk that an upload is no longer ordered before the pass that samples it. Fix is an explicit `.before/.after`. |
| `MeshTag(x)` is now `MeshTag::new(x)` | constructor and `.value` | **4** constructions (`MeshTag\(`): `tank_surface.rs:836,1604`, `track/belt_draw.rs:708`, `vfx/billboard.rs:323`; 1 `tag.0` | mechanical |
| `MeshAabb::compute_aabb` renamed | `get_aabb` | **3**: `damage_cam/pen_cam/stage.rs:866,3442,3621` | mechanical |
| `NextState::set_if_neq` renamed | `set_if_different`; old name deprecated (warning, and CI is `-D warnings`) | **2** (`src/flow.rs`) | mechanical |
| `EditableText` and `TextInput` separated | `ui_widgets::EditableTextInputPlugin` is now `TextInputPlugin` (MEASURED `bevy_ui_widgets-0.20.1/src/text_input.rs:763`) | **1**: `src/lib.rs:359` | mechanical; see plugin lists below |
| `RenderDebugOverlay` added to default plugins | a new default plugin, keybindings off | 43 `.disable::<` lines in 8 files enumerate Bevy plugins (`lib.rs`, `bitprobe.rs`, `headless_test.rs`, `net/physics.rs`, ...) | **review**: every hand-kept plugin list must be re-read against 0.20's `DefaultPlugins` (new: render debug overlay, sprite mesh plugins, more widgets) so the server floor stays GPU-less and minimal |
| macOS app activation follows `Window::focused` | bevy_winit sets `with_activate_ignoring_other_apps(initially_focused)` (`bevy_winit-0.20.1/src/lib.rs:111-118`) | hidden/offscreen path: `run_window.rs:160-168`, `platform.rs:16` | none required; enables a deletion (section 6) |
| `ScheduleBuildSettings` gains `shuffle_seed` | extra field under `debug` (we enable `debug`) | **4** literals, all with `..default()`/`..new()` | none |
| `TextFont::default()` is `FontSize::Rem(1.)` | default size follows `RemSize` | 37 `TextFont` literals; `font_size:` set explicitly in the ones sampled; 1 bare `TextFont::default()` in a test | check each literal sets `font_size` |
| `Val::Em`/`Val::Rem`, `Node` requires `EmSize` | new variants; exhaustive `match` on `Val` breaks | 298 `Val::` lines; exhaustive matches not counted | compile will tell |
| `bevy_shape` / `bevy_curve` split | import paths move unless via prelude | 5 and 1 lines | mechanical |
| Some large error variants boxed | `AssetLoadError` variants | 1 doc mention | none expected |
| `&str` to `Name` needs `'static` | `From<&str>` removed | 0 `Name::from(`; 127 `Name::new`/`Name` lines unaffected by the signature | compile will tell |
| Exclusive systems unified | `&mut World` is a `SystemParam`; `System::is_exclusive` gone | 136 `&mut World` lines; 0 uses of the removed APIs | none expected |
| `ShaderBuffer` stores typed data | `new` takes `Vec<T>`; `set_data` and `From<T>` removed | 64 lines / 11 files match `ShaderBuffer\|ShaderStorageBuffer`; constructor forms not split out | **check**: this type backs the VFX and belt instance buffers; each constructor and `set_data` call moves |
| `UnpreparedBindGroup` to `BindGroupBuilder` | only manual `AsBindGroup` impls | 22 `AsBindGroup` lines, derive use | none expected |
| Opt-in `ScreenSpaceTransmission` | off by default | 0 `specular_transmission\|diffuse_transmission` | none |
| UI rendering is now retained | only render-world UI code | 0 | none; watch UI materials (`optic_mask.wgsl` imports `bevy_ui::ui_vertex_output`) |
| `ui::Interaction`/`Button` deprecated | - | 0 (the UI already uses `Hovered`/`Pressed`, 26 lines) | none |
| `DeferredWorld::query` deprecated | - | 5 hooks take `DeferredWorld`; `.query(` on them not found | none expected |

Not in the migration guide, found by diffing crates:

- **`SystemParam` access plumbing changed** (`FilteredAccessSet` to `SystemAccess`), seen in
  lightyear's own port. We have 0 manual `SystemParam` impls (`unsafe impl.*SystemParam\|init_access`).
- **`Time<Virtual>` clamp order changed**: `max_delta` now clamps after `relative_speed` scaling
  (`bevy_time` `virt.rs` diff). Our only `Time<Virtual>::set_relative_speed` callers are the dev
  sandbox (`sandbox.rs:956`, `sandbox/panel.rs:169`); `net/interp_delay.rs:106,114` sets a lightyear
  timeline's speed, a different type. DERIVED: no effect on the product's fixed step.
- **`bevy_tasks`**: 9 changed lines, test-only thread floor. `bevy_app` `TaskPoolPlugin`: 2 lines.
  No documented task-pool or parallel-query change beyond the `par_iter_many` item. 87 lines in 28
  files touch task pools and should compile unchanged (DERIVED).
- **wgpu 29 to 30** (wgpu `CHANGELOG.md` v30.0.0). Our direct `wgpu::` use (tests and
  `settings::probe`): `get_mapped_range` now returns `Result` (**2** sites), compute pass
  `dispatch` renamed `dispatch_workgroups` (**2** lines match, form not checked), `VertexState.buffers`
  takes `Option<..>` (3 lines in 1 file, through Bevy's descriptor, so Bevy's type decides).
  "Integer shader I/O no longer defaults to `@interpolate(flat)`": our shaders have 0 integer
  inter-stage fields; the 2 integer `@location(15)` items are vertex inputs
  (`track_belt_mesh.wgsl:18`, `pen_cam_look_belt.wgsl:16`), expected unaffected.
- **glam 0.32.1 to 0.33.x changes arithmetic**: `Mat3` determinant/inverse aligned with `Mat4`
  (0.33.8), `FloatExt::lerp` form changed (0.33.11), `try_inverse` finiteness (0.33.12) (glam
  `CHANGELOG.md`). Sim outputs may move in the last bits on both targets. ADR-0028 already requires
  the cross-target bitprobe pair for any math dependency change.

Areas the brief asked about, with nothing found: fixed-timestep schedules (293 lines, 70 files; no
0.20 item touches `FixedUpdate`/`RunFixedMainLoop`), required components (16 `#[require(`), the
glTF/`WorldAssetRoot` path (60 lines, 16 files; `bevy_world_serialization` and `bevy_gltf` remain in
0.20.1 and the guide has no item for them), RON/reflection (`ron` stays 0.12; we do not use the
reflect serializer), headless modes (`ScheduleRunnerPlugin` diff is formatting only). "Nothing
found" means nothing in the guide and nothing in the diffs I read; the compiler is the real test.

## 4. Lightyear migration surface (0.30.1 to 0.31.0)

18 commits. Per-crate source diffs (MEASURED diffstat): `lightyear_inputs_native` and
`lightyear_serde` unchanged; `core` +15 -7; `sync` +25 -19; `interpolation` +49 -27; `transport`
+13 -7; `messages` +6 -4; `inputs` +12 -5; `connection` +58 -31; `replication` +151 -151 (observer
syntax plus moved tests); `link` +346 -300; `udp` +669 -321; `netcode` +28 -25.

| Item | What changed | Call sites here | Kind |
|---|---|---|---|
| Endpoints separated from the server role (#1741) | `LinkOf { server }` is `LinkOf { endpoint }`; `Server { conditioner }` is a unit marker requiring the new `Endpoint { conditioner }`; `ServerLinkPlugin` is `EndpointLinkPlugin`; `ServerUdpIo` is a unit struct requiring `UdpEndpoint`, which now owns `recv_buffer_pool_misses` | `LinkOf { server }`: **1** (`net/server_stats.rs:1311`). `ServerUdpIo::default()`: 15 lines / 8 files, still compiles (derives `Default`). `recv_buffer_pool_misses`, `ServerLinkPlugin`, `UdpLinkOfIO`: 0. Server-side conditioner on `Server`: 0 (ours is the client `Link` conditioner, `net/client.rs:133`) | mechanical |
| Input validation runs only on the server (#1774) | `InputSystems::ValidateInputs.run_if(is_server)` | we register validation on the server (`InputValidationAppExt`, 1 import) | none; behaviour we already assume |
| Interpolation dynamic access (#1794) | internal `SystemParam` access now panics with B0001 on a conflicting query instead of declaring it loosely | 85 lines / 14 files use `InterpolationTimeline`/`Config`/`Registry`/`Systems`; public API unchanged in the diff | **watch**: a system pairing lightyear's interpolation param with its own `ConfirmedHistory<C>` query (13 lines, 3 files) can now panic at startup. Fail-fast, found on first boot. |
| Shared render delay (ADR-0037) | `timeline.rs` diff is observer syntax and the access type only; `set_relative_speed`, `InterpolationConfig`, `SyncTargetTimeline`, `SyncAdjustment`, `TimelineSync` unchanged | `net/interp_delay.rs`, `net/frontier` | none found |
| Replication setup | `lightyear_replication` changes are observer syntax; replicon 0.45 underneath | 42 registration lines / 4 files; 21 `Replicate`-family lines | none found |
| bevy_replicon 0.44.3 to 0.45.5 (used directly, 56 lines / 12 files, including `shared::replication::registry::*` internals and `test_fns`) | changelog lists no API change, only "Update to Bevy 0.20" and collection-speed work; internals are not covered by a changelog | deep paths: `ReplicationRegistry`, `ctx::{SerializeCtx, WriteCtx}`, `receive_fns::MutWrite`, `storage::ReplicationStorage`, `FilterRegistry`, `ServerTick` | compile will tell; **wire**: `net/protocol.rs:82` fingerprints the replicon version, so the protocol fingerprint changes and client, server and the compatibility record move together |
| `PeerId` gains `PartialOrd, Ord` | additive | 2 imports | none |
| Prediction fixes (#1751, #1756, #1757, #1781) | prediction crate only | we enable no `prediction` feature; 3 `Rollback`-family lines | none |
| Avian integration (#1772 re-register invalid proxy keys; rollback snapshot rewrite) | `lightyear_avian3d` +980 -267 | our vendored patch | re-port, section 2 |
| netcode | `server_plugin.rs` only | our vendored patch | re-apply, section 2 |

Custom shaders a Bevy render change breaks: all 17 in `assets/shaders/` (WESL item above) plus the
five patched files inside vendored `bevy_pbr`. lightyear itself ships no shader we use.

## 5. Toolchain, CI, build notes

- **MSRV**: bevy 0.20.1 and lightyear 0.31.0 declare `rust-version` 1.97.1 (crates.io). We pin
  1.98.0 (`rust-toolchain.toml`). **No toolchain change.**
- **CI** (`.github/workflows/`): no workflow names a Bevy version. Effects are indirect:
  - `ci.yml`, `assets.yml`, `bitprobe.yml` key the rust cache on `hashFiles('vendor/**')`; renaming
    the vendored crates re-keys every family, so the first run is cache-cold (`ci.yml:74-76` records
    a MEASURED 28 min tests lane in that state).
  - `deny.toml:60-61` bans any glam other than 0.32.1 outside `obvhs`; both bounds and
    `src/tests/determinism_deps.rs` (`AUDITED_SPLIT_GLAM`) move to the new glam. obvhs goes 0.3.x to
    0.4.0 with Avian 0.8; whether it still carries a private glam is the Avian scout's to confirm.
  - `deny.toml:19` ignores RUSTSEC-2026-0344 because bevy_image 0.19 pins ruzstd 0.8;
    bevy_image 0.20.1 requires ruzstd ^0.9. The ignore can probably go (verify with `cargo deny`).
  - `bitprobe.yml` and the macOS/Linux pair must run on the upgrade (ADR-0028).
  - `field-casts.yml` triggers on `vendor/**` and builds on three OSes; expect it to run.
  - Linux system packages (`libasound2-dev libudev-dev`, ...) are unchanged in Bevy's 0.20 docs as
    far as I read; not separately verified.
- **Build time / binary size**: neither the guide nor the release notes I read state numbers.
  `chain_weak` is described as a render-world parallelism gain, without figures. Not measured here.

## 6. What the upgrade buys

Deletions and simplifications, each to be confirmed when the code compiles:

- `vendor/bevy_reflect-0.19.1-scalar-math` (whole crate) if glam resolves to a release with scalar
  `BVec3A`/`BVec4A` serde (0.33.12 has it).
- `bevy_pbr` patch items 1 (`MeshUniform` alignment, if the test and a Metal boot agree) and 2
  (`directional_shadow_factor`, upstream). The patch shrinks to the heightfield shadow and the
  removal batching. `upstream/bevy-uninitbuffervec-rust-size-vs-shader-stride.md` becomes obsolete
  for `UninitBufferVec`.
- `platform::revoke_macos_activation` (`src/platform.rs:16-33`) and its `Startup` registration
  (`run_window.rs:168`): 0.20 does not request activation when the primary window is created with
  `focused: false`. The hidden path would set `focused = false` beside `visible = false`. Whether
  winit still forces `ActivationPolicy::Regular` (dock icon) is not answered by the Bevy change;
  test on a hidden capture run before deleting.
- `deny.toml` RUSTSEC-2026-0344 ignore.

New instruments, optional:

- `ScheduleBuildSettings::shuffle_seed` (release note "Schedule randomization"): a seeded shuffle
  of unordered conflicting systems. Fits the four ambiguity tests that already set build settings
  (`tank_surface.rs:1489`, `view.rs:472`, `net/protocol.rs:2270`, `track/view.rs:960`).
- `chain_weak`/`before_weak`/`after_weak` for our own set chains, the parallel-work lens's subject.
- Font fallback lists (`FontSource::list`): the AGENTS.md "no glyph fallback" rule describes 0.19.
  A fallback face becomes possible; the coverage test still guards the bundled face.
- Panics inside systems, commands and observers are routed to the fallback error handler, which
  re-panics by default (release note "Catching Panics"). ADR-0011 holds only while nothing installs
  a continuing handler: `damage_cam/pen_cam/game.rs:621` sets `bevy::ecs::error::ignore` in one app
  today and would start swallowing panics there. **Review that site at the flip.**

Server tick cost, no numbers claimed:

- Bevy per-column change ticks (release note): a column with no changed component is skipped by
  change-detection filters. Relevant to our `Changed<..>` scans over thousands of static colliders,
  the same population the vendored Avian patch prunes by hand.
- bevy_replicon 0.45.0-0.45.3: skips unchanged entities and archetypes when collecting changes and
  caches component storage per archetype (replicon `CHANGELOG.md`).
- Whether either lets a vendored Avian prune be dropped is for the Avian scout and a measurement
  against `.agents/scratch/perf-2026-11-01/server-tick.md`.

## Conclusion

### Does a consistent dependency set exist today?

Yes: bevy 0.20.1, lightyear 0.31.0, avian3d 0.8.0, bevy_replicon 0.45.5, bevy_egui 0.43.1, wgpu 30,
glam 0.33.x, on our current Rust 1.98.0. The gap is our five vendored patches, none of which has a
0.20-era port.

### Staged plan

Effort is DERIVED from the call-site counts and patch sizes above, in focused lane-days, and
excludes the Avian fork rebase. Low confidence: the compiler will find items no guide lists.

| # | Slice | Lands green alone? | Effort (DERIVED) |
|---|---|---|---|
| 0 | **Baseline on 0.19.** Record the cross-target bitprobe pair, server-tick cost, and a capture of every custom-shader path (terrain, tank surface, construction, belts, VFX, pen-cam, optic mask, render-scale upscale) for the flip's one before/after comparison. | yes | 0.5-1 |
| 1 | **Avian fork on 0.8** (other scout). Blocks slice 2's `lightyear_avian3d` re-port and the flip. | no, it needs Bevy 0.20; it merges as part of slice 2 | not priced here |
| 2 | **The flip**, one lane, one merge. Ordered commits inside the lane: (a) manifest, lock, vendored re-ports: `bevy_pbr` 0.20.1 (heightfield shadow in WESL, removal batching), netcode 0.31.0, `lightyear_avian3d` 0.31.0, drop `bevy_reflect`; glam pin, `deny.toml`, tripwire. (b) Rust compile fixes: 72 observers, 6 `par_iter_many` sites, pointer events, `scene_depth`, `settings::probe`, plugin lists, wgpu 30, lightyear `LinkOf`. (c) 17 shaders to WESL. (d) Verification: targeted tests, bitprobe pair, hosted e2e, windowed Metal boot, captures against slice 0. | **only as a whole**: no intermediate lockfile exists (section 1) | (a) 2-4, (b) 2-4, (c) 2-5, (d) 1-3: **7-16** |
| 3 | Delete `revoke_macos_activation` if a hidden capture run stays unfocused. | yes | 0.5 |
| 4 | Drop the `MeshUniform` alignment patch and the RUSTSEC ignore if not already settled inside slice 2. | yes | 0.5 |
| 5 | Optional adoptions: `shuffle_seed` in the ambiguity tests, `chain_weak` review, font fallback. | yes, each | 0.5-2 each |
| 6 | Measure server tick after the flip; decide whether per-column ticks or replicon 0.45 retire any vendored prune. | yes | 1-2 |

### Top risks

1. **Shader port is all-or-nothing and only verifiable by eye.** 17 files and the vendored
   heightfield law change language in the same commit that changes the renderer. A shader that fails
   to compile is loud; one that compiles and shades differently is not.
2. **Sim bits move** with glam 0.33 and parry 0.31, on top of whatever Avian 0.8 changes. Emergent
   outputs will move; that is an observation, not a regression, but the cross-target pair must stay
   identical.
3. **Vendored `bevy_pbr` re-port** against restructured mesh-collection code and WESL shaders; the
   heightfield shadow reaches every lit mesh.
4. **Weak render-set ordering** with our `Res<RenderQueue>` uploads: a one-frame-late texture is
   easy to miss.
5. **Unlisted breaks**: replicon internals we import by deep path, plugin lists, and anything the
   guide omits (two such changes were found by diffing: `SystemAccess`, `Time<Virtual>` clamp order).
6. **Wire compatibility**: the protocol fingerprint changes; client, hosted server image and the
   matchmaking compatibility record ship together.
7. **Cold CI** on the flip's push and its fix-forwards (vendor re-key).

### Together with the Avian rebase, or separately?

Together at the merge, because the dependency graph leaves no choice: Bevy 0.20 without Avian 0.8
does not resolve, and neither does the reverse. Separately as work: the Avian fork rebase is its own
lane and goes first, since the `lightyear_avian3d` re-port and every sim test depend on it. Order:
slice 0, Avian fork rebase to a compiling crate, then the flip lane on top of it, then follow-ups.

### Questions only the owner can answer

1. Is there a reason to move now (a needed 0.20 feature, an upstream fix, staying within one release
   of upstream), or does this wait for a quieter point? Bevy 0.21 has no RC; nothing forces the date.
2. Settled: the game's damage-model depth masks keep `Tonemapping::None`, which skips the pass on both versions.
3. May the flip land as one large merge that is green only at its end, with the six lenses run over
   the whole diff, or do you want the lane reviewed commit by commit before the merge?
4. Hosted-server and client release: is a forced simultaneous update (protocol fingerprint change)
   acceptable at the next alpha, and which alpha?
5. Keep carrying the heightfield shadow as a `bevy_pbr` fork, or use the upgrade to move it to
   per-material sampling through the now-upstream `directional_shadow_factor` hook? The fork is the
   costliest vendored item on every future Bevy bump.
6. Do the drafted upstream reports (`upstream/`) get sent before the re-port, so some patches might
   be dropped by 0.21 instead of carried again?
7. Font fallback: keep "no fallback, coverage-tested" as policy, or ship a fallback face?
