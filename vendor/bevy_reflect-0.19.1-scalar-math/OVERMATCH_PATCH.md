# Overmatch Bevy Reflect patch

This directory is the published `bevy_reflect` 0.19.1 crate with one source change, tagged
`// OVERMATCH PATCH:`. (`Cargo.lock`, `.cargo_vcs_info.json` and `examples/reflect_docs.rs` from the
published crate are omitted; none is built for a path dependency.)

## `BVec3A` / `BVec4A` reflect without serde (`src/impls/glam.rs`)

The workspace enables `glam/scalar-math` (ADR-0028). glam's scalar implementation provides no serde
impls for the aligned boolean masks `BVec3A` and `BVec4A`, so upstream's
`impl_reflect_opaque!(::glam::BVec3A(Clone, Debug, Default, Deserialize, Serialize))` does not
compile. The patch registers both with `(Clone, Debug, Default)` only — the rest of their opaque
reflection support is unchanged.

`tests/determinism_deps.rs` is the tripwire that fails if a dependency bump splits `glam` and
silently drops the `scalar-math` pin. Re-evaluate this vendor whenever Bevy or glam moves.
