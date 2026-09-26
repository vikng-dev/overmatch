# Overmatch Bevy PBR patch

This directory is the published `bevy_pbr` 0.19.1 crate with one source change, tagged
`// OVERMATCH PATCH:` so the diff against pristine 0.19.1 is self-describing. (`Cargo.lock` and
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
size are identical to the default SIMD glam layout. `tests/gpu_layout.rs` pins those offsets and
checks every publicly reachable Rust-repr GPU upload from the audited Bevy paths against its Encase
minimum size.

The generic bug and proposed upstream fix are recorded in
`upstream/bevy-uninitbuffervec-rust-size-vs-shader-stride.md`. Not fixed in 0.19.1 (`MeshUniform`
there is still a bare `#[derive(ShaderType, Clone)]`).

## Retired: shadow views inherit the light's `RenderLayers`

The 0.19.0 vendor also carried a backport of bevyengine/bevy #24797. 0.19.1 ships it (upstream's
shape: `RenderLayers` on the extracted light entity, copied onto every shadow view, and a
non-optional `&RenderLayers` on `queue_shadows`' view query), so it is no longer patched here. The
record is `upstream/bevy-shadow-view-ignores-light-render-layers.md`.

Re-evaluate and preferably remove this vendored crate when upgrading Bevy.
