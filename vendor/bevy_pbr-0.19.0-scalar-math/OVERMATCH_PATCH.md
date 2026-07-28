# Overmatch Bevy PBR patches

This directory is the published `bevy_pbr` 0.19.0 crate with two source changes. Every edit site is
tagged `// OVERMATCH PATCH:` so the diff against pristine 0.19.0 is self-describing.

## 1. `MeshUniform` alignment

`MeshUniform` has `#[repr(C, align(16))]`.

With glam 0.32.1 `scalar-math`, Rust `Vec4` keeps its DERIVED 16-byte payload but drops its DERIVED
16-byte Rust alignment. Bevy's `UninitBufferVec<MeshUniform>` allocates by
`size_of::<MeshUniform>()`, while the matching bind layouts use Encase's WGSL-correct
`MeshUniform::min_size()`. Without the patch, the Rust structure ends at a DERIVED 164 bytes and the
shader structure ends at a DERIVED 176 bytes; wgpu therefore rejects the MEASURED first 164-byte
buffer against the 176-byte minimum binding size.

The explicit C layout preserves the existing field order, and DERIVED 16-byte structure alignment
restores only the missing DERIVED 12 bytes of tail padding. The field offsets and DERIVED 176-byte
size are identical to the default SIMD glam layout. `tests/gpu_layout.rs` pins those offsets and
checks every publicly reachable Rust-repr GPU upload from the audited Bevy paths against its Encase
minimum size.

The generic bug and proposed upstream fix are recorded in
`upstream/bevy-uninitbuffervec-rust-size-vs-shader-stride.md`.

## 2. Shadow views inherit the light's `RenderLayers`

`prepare_lights` spawns each shadow view with `spawn_empty()` and never gives it a `RenderLayers`,
but `queue_shadows` (`src/render/light.rs`) tests every candidate caster against that view's
`RenderLayers` — absent, so `RenderLayers::default()`, so layer 0. Any mesh on a non-zero layer
therefore casts no shadow at all, even from a light whose own mask covers that layer, contradicting
the main-world filter in `bevy_light` which accepted the caster against the *light's* mask.

The patch copies the LIGHT's mask (never the camera's — a caster invisible to the camera must still
cast) onto all three kinds of shadow view:

- `ExtractedPointLight` gains a `render_layers` field, populated in `extract_lights` from the
  point/spot light entity's `Option<&RenderLayers>` (absent → `RenderLayers::default()`), with
  `Changed<RenderLayers>` added to both extract filters so the *cached* point/spot shadow views are
  rebuilt when the mask changes.
- `create_point_shadow_maps` and `create_spot_shadow_map` insert `light.render_layers.clone()` on
  the view-light entity.
- The directional cascade loop in `prepare_lights` inserts
  `ExtractedDirectionalLight::render_layers.clone()` on each cascade's view-light entity.

Upstream fixed this in bevyengine/bevy **#24797** (fixes #24792), merged 2026-07-08, milestone
**0.19.1** — which has not been released, so the patch stays until it is. Upstream's shape differs
(it attaches `RenderLayers` as a component on the extracted light entity and drops
`ExtractedDirectionalLight::render_layers`); ours is smaller and changes no public API, which is
what a vendored backport wants. Full decode, minimal repro and the comparison:
`.agents/docs/upstream/bevy-shadow-view-ignores-light-render-layers.md`.
`tests/bevy_shadow_view_render_layers.rs` is the tripwire that fails if a vendor refresh drops it.

Re-evaluate and preferably remove this vendored crate when upgrading Bevy.
