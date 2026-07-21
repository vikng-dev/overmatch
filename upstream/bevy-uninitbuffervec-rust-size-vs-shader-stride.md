# `UninitBufferVec` allocates Rust size instead of shader array stride

## Summary

`UninitBufferVec<T>` reserves GPU bytes with Rust `size_of::<T>()`, but the bind-group layouts for
the same `T` use Encase's shader layout. Those sizes can legitimately differ. When glam's
`scalar-math` feature lowers the Rust alignment of `Vec4`, Bevy 0.19's `MeshUniform` loses its final
DERIVED 12 bytes of Rust tail padding: the allocation becomes DERIVED 164 bytes while
`MeshUniform::min_size()` remains the WGSL-correct DERIVED 176 bytes. wgpu then rejects the buffer
as smaller than the binding's minimum size.

This is a Bevy allocation/stride bug, not an Encase layout bug and not a fundamental incompatibility
between Bevy and scalar math.

## Environment

- Bevy 0.19.0, Encase 0.12.0, glam 0.32.1 with `scalar-math`
- Observed on a macOS aarch64 Metal client after enabling scalar math workspace-wide
- The existing headless compositions set `WgpuSettings.backends = None`, so they cannot exercise
  buffer allocation or bind-group validation

## Evidence

The Bevy 0.19 [`MeshUniform`](https://github.com/bevyengine/bevy/blob/v0.19.0/crates/bevy_pbr/src/render/mesh.rs#L514-L556)
payload ends at DERIVED byte 164. Its `Vec4` arrays contain a DERIVED total of eight values whose
payloads remain DERIVED 16 bytes under scalar math, so every internal field keeps the same offset as
the SIMD build; only Rust's maximum structure alignment falls and the final padding disappears.

Encase derives shader metadata independently of Rust alignment. Its `Vec4` shader alignment is
DERIVED 16 bytes, and its structure derive rounds the final size to that alignment. All byte counts
in the resulting layout are DERIVED:

```text
[Vec4; 3]              48
[Vec4; 3]              48
[Vec4; 2]              32
f32 + u32                8
UVec2                     8
five u32                 20
                         ---
payload end              164  DERIVED
shader struct size       176  DERIVED (round up to 16-byte alignment)
```

The macOS Metal failure independently MEASURED the same mismatch in wgpu validation: the first
mesh output buffer was 164 bytes while the binding required a 176-byte minimum.

Bevy 0.19's
[`UninitBufferVec::new`](https://github.com/bevyengine/bevy/blob/v0.19.0/crates/bevy_render/src/render_resource/buffer_vec.rs#L703-L719)
stores `item_size: size_of::<T>()`, and
[`reserve`](https://github.com/bevyengine/bevy/blob/v0.19.0/crates/bevy_render/src/render_resource/buffer_vec.rs#L788-L804)
allocates `item_size * capacity`. GPU preprocessing puts its generic batch output `BD` in that
buffer; PBR instantiates `BD` as `MeshUniform`. In contrast, the matching storage-buffer bind layout
uses Encase's `T::min_size()`.

A wider allocation can make the single-element minimum-size validation pass while retaining the
wrong per-element stride, so lowering `min_binding_size` or merely rounding the total buffer size is
not a sound fix.

## Scope audit

The direct scalar-sensitive `ShaderType` inventory and the raw/partially initialized buffer
instantiations in `bevy_render`, `bevy_pbr`, `bevy_sprite_render`, and this project were audited.
`MeshUniform` is the only currently exercised structure affected by this mechanism.

Other Rust-repr uploads reachable from the project (`MeshInputUniform`, `MeshCullingData`,
`GpuClusteredLight`, and `RenderClusteredDecal`) have matching Rust and shader sizes. `Mesh2dUniform`
also has a representational size difference under scalar math, but Bevy 0.19 writes it through an
Encase-backed `GpuArrayBuffer`; its Rust representation is not copied as the shader representation.

## Why Bevy main is still affected

As checked on 2026-07-21, Bevy main still:

- initializes [`UninitBufferVec.item_size` from `size_of::<T>()`](https://github.com/bevyengine/bevy/blob/main/crates/bevy_render/src/render_resource/buffer_vec.rs);
- uses [`UninitBufferVec<BD>` for generic GPU batch output](https://github.com/bevyengine/bevy/blob/main/crates/bevy_render/src/batching/gpu_preprocessing.rs); and
- supplies [`MeshUniform` as the PBR batch output](https://github.com/bevyengine/bevy/blob/main/crates/bevy_pbr/src/render/mesh.rs).

Main's `MeshUniform` has gained another `u32`, but that changes the scalar mismatch to only DERIVED
168 Rust bytes versus DERIVED 176 shader bytes. Current scalar glam still does not give `Vec4` a
16-byte Rust alignment. The mechanism therefore remains present; upgrading alone is not a fix.

## Suggested fix

`UninitBufferVec` should allocate each element using Encase's array stride, conceptually:

```rust
item_size: <[T; 1] as ShaderSize>::SHADER_SIZE.get() as usize,
```

The existing `GpuArrayBufferable` bound already includes `ShaderType + ShaderSize`. The array type's
shader size is important: `T::min_size()` alone is not a generic element stride. For example, a
shader `vec3<f32>` has a DERIVED 12-byte value size but a DERIVED 16-byte array stride. Encase's
`<[T; 1]>::SHADER_SIZE` applies the element's shader alignment and therefore expresses that stride.

An upstream test should cover both a structure whose Rust tail padding differs from its shader size
and a `vec3`-shaped element whose array stride exceeds its value size.

## Local workaround

Overmatch vendors `bevy_pbr` 0.19.0 and adds `#[repr(C, align(16))]` to `MeshUniform`. This restores
DERIVED size 176 under scalar math without changing the SIMD layout or any internal field offset.
`tests/gpu_layout.rs` statically checks the patched type and every publicly reachable Rust-repr GPU
upload against `ShaderType::min_size()`; the test needs no GPU.

This workaround is deliberately narrower than the suggested upstream fix and must be re-evaluated
on every Bevy upgrade.

---

## Status (local record)

- 2026-07-21: **DRAFTED, NOT FILED.** File upstream only on explicit direction; record the issue or
  PR URL here when filed.
- Prior issue/PR searches found no report specifically covering `UninitBufferVec` Rust size versus
  Encase shader array stride under scalar math.
