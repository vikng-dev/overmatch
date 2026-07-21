# Overmatch Bevy PBR patch

This directory is the published `bevy_pbr` 0.19.0 crate with one source change:
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
`upstream/bevy-uninitbuffervec-rust-size-vs-shader-stride.md`. Re-evaluate and preferably remove
this vendored crate when upgrading Bevy.
