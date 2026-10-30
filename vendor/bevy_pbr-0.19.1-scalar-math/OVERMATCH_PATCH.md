# Overmatch Bevy PBR patch

This directory is the published `bevy_pbr` 0.19.1 crate with the source changes below, each tagged
`OVERMATCH PATCH` so the diff against pristine 0.19.1 is self-describing. (`Cargo.lock` and
`.cargo_vcs_info.json` from the published crate are omitted; neither is read for a path dependency.)

## `MeshUniform` alignment (`src/render/mesh.rs`)

`MeshUniform` gains `#[repr(C, align(16))]`.

With glam 0.32.1 `scalar-math`, Rust `Vec4` keeps its DERIVED 16-byte payload but drops its DERIVED
16-byte Rust alignment. Bevy's `UninitBufferVec<MeshUniform>` allocates by
`size_of::<MeshUniform>()`, while the matching bind layouts use Encase's WGSL-correct
`MeshUniform::min_size()`. Without the patch, the Rust structure ends at a DERIVED 164 bytes and the
shader structure ends at a DERIVED 176 bytes; wgpu therefore rejects the MEASURED first 164-byte
buffer against the 176-byte minimum binding size.

The explicit C layout preserves the existing field order, and DERIVED 16-byte structure alignment
restores only the missing DERIVED 12 bytes of tail padding. The field offsets and DERIVED 176-byte
size are identical to the default SIMD glam layout. `src/tests/gpu_layout.rs` pins those offsets
and checks every publicly reachable Rust-repr GPU upload from the audited Bevy paths against its
Encase minimum size.

The generic bug and proposed upstream fix are recorded in
`upstream/bevy-uninitbuffervec-rust-size-vs-shader-stride.md`. Not fixed in 0.19.1 (`MeshUniform`
there is still a bare `#[derive(ShaderType, Clone)]`).

## `directional_shadow_factor` (backport of bevyengine/bevy#24936)

`PbrInput` gains `directional_shadow_factor` (default 1.0, set in `pbr_input_new`), which
`apply_pbr_lighting` multiplies into each directional light's shadow inside the
shadows-enabled-receiver block, exactly as upstream merged it after 0.19.1. A material folds a custom
shadow term into the sun through it instead of evaluating the directional light a second time. The
terrain material's track-print sun term uses it (`assets/shaders/terrain_blend.wgsl`). Being inside
that block, the factor lapses with the light's shadow maps: a light with `shadow_maps_enabled` off,
or a mesh with `NotShadowReceiver`, ignores it.

## The heightfield shadow (`src/heightfield_shadow.rs`)

A world-anchored occlusion field toward one directional light, which every lit fragment samples:

- `HeightfieldShadow` (main-world resource): an `Rg32Float` image of per-texel occluders (height,
  distance along the light), its world mapping, the light direction it was baked for, and the
  light's angular radius, which sets the penumbra from each occluder's distance. Extracted while
  present, removed when not.
- Two bindings in EVERY mesh view layout, `@group(0) @binding(39)` (the texture, non-filterable)
  and `@binding(40)` (`GpuHeightfieldShadow`, a uniform), filled by
  `prepare_mesh_view_bind_groups` with the field's image once on the GPU and the 2D fallback
  image before that. The uniform's `ray.w` says whether the field is bound.
- `shadows.wgsl`: `fetch_directional_shadow` multiplies its cascade result by
  `fetch_heightfield_shadow` for the light whose direction matches the field's, after the cascade
  envelope check, so the field ends where the light's cascades end and is skipped wherever the
  light casts no shadow or the mesh receives none.
- `GpuHeightfieldShadow::from_field` is public so the application can run the shader's receiver
  law against its own fields on a headless device (`src/terrain_shadow.rs` tests).

Upstream has the hook (`directional_shadow_factor`) but no shared field: there, every material
that wants the heightfield's shadow samples its own. Here the field is in the view bindings because
every receiver (terrain, vehicles, trees, buildings) must see a ridge's shadow, and most of them
are glTF `StandardMaterial`s.

## Mesh instance removals (`src/render/mesh.rs`, issue #173)

GPU mesh building only. Each change is tagged with its own name, `OVERMATCH PATCH (removal
batching)` or `OVERMATCH PATCH (removal skip)`, and either can be carried or dropped alone. Upstream
`main` still has the stock behaviour of both; the draft is
`upstream/bevy-invisible-mesh-removal-churn.md`.

- **Removal batching** (`collect_meshes_for_gpu_building`). Stock sends every queued removal through
  an `mpsc` channel, one send per entity, from the worker task that prepares that queue's changed
  meshes. Here the workers leave `removed` in their queues, and the consumer drains every queue
  after the prepared instances are applied: the same point in the frame where stock applied the
  channel's removals. The order among removals was never defined, since the channel interleaved
  the workers.

Guards: `src/tests/bevy_mesh_removal_lifecycle.rs`, render-world lifecycle tests on a real adapter
(`#[ignore]`d, since CI has none; the module doc has the command). Counter: `SPIKE_MESH_REMOVALS=1`
(`src/render_cost.rs`), which logs the removals queued per frame and how many had an instance.

## Retired: shadow views inherit the light's `RenderLayers`

The 0.19.0 vendor also carried a backport of bevyengine/bevy #24797. 0.19.1 ships it (upstream's
shape: `RenderLayers` on the extracted light entity, copied onto every shadow view, and a
non-optional `&RenderLayers` on `queue_shadows`' view query), so it is no longer patched here. The
record is `upstream/bevy-shadow-view-ignores-light-render-layers.md`.

Re-evaluate and preferably remove this vendored crate when upgrading Bevy.
