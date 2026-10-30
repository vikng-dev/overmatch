# A zero-vertex mesh logs `Use-after-free` in the mesh allocator

**Status: fixed upstream, nothing to file.** bevyengine/bevy#24960 (merged 2026-07-14; closes
#24874, duplicate #25246) ships in **v0.20.0** and is absent from v0.19.1, our pin (both VERIFIED
by reading `crates/bevy_render/src/mesh/allocator.rs` at the two tags).

## Mechanism (bevy_render 0.19.1)

- `extract_render_asset` extracts every `Added`/`Modified` `Mesh` asset whose usage includes
  `RENDER_WORLD`, whether or not any entity draws it (`src/render_asset.rs:311-351`).
- `MeshAllocator::allocate_meshes` skips allocation for a mesh whose vertex buffer size is zero
  (`src/mesh/allocator.rs:375`), but its copy loop (`:423-426`) still calls
  `copy_mesh_vertex_data` and, when the mesh has an index vector (even an empty one),
  `copy_mesh_index_data` (`:453-460`).
- `SlabAllocator::copy_element_data` finds no allocation for the key and logs
  `Use-after-free: attempted to copy element data for an unallocated key`
  (`src/slab_allocator.rs:974-975`), then returns.

Nothing is uploaded and nothing is freed early: the log line is the whole effect. One line per
zero-vertex mesh per extraction, two when it carries an index vector.

## Upstream's fix

v0.20.0 skips the zero-size mesh in the copy loop too, and logs
`warn!("Mesh {:?} contains no vertices.")` in the staging loop instead (`allocator.rs:434` at the
tag). A zero-vertex mesh still logs once per extraction after the bump, as a warning.

## Our side

Not a workaround but the fix: the game never hands the renderer a zero-vertex mesh. The one
producer found, the shell trail ribbon, holds its `Mesh3d` only while it has stations
(`src/vfx/trail.rs`, `TrailRibbon`). Evidence and counts: `.agents/scratch/perf-2026-10-29/despawn-176/`.

## What fixing this unlocks for us

Nothing to delete. After the 0.20 bump, a zero-vertex mesh reads as a `WARN` naming its asset id
instead of a misleading `ERROR`.
