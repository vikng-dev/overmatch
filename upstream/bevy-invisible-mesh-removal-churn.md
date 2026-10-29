# Draft: invisible mesh changes send redundant per-entity removal messages

**LOCAL DRAFT — not filed.** Source checked in Overmatch's vendored `bevy_pbr 0.19.1`,
`src/render/mesh.rs`; no engine change is applied by this lane.

## Reproduction and observation

A hidden offscreen Overmatch scene contains static tree hierarchies with hidden cut caps and
out-of-range primitives. Its application's `PhysicsInterpolationPlugin::interpolate_all()` also
interpolates static rigid-body roots. Those writes, including floating-point pose noise, change
`GlobalTransform` and then `PreviousGlobalTransform` throughout the trees.

MEASURED with temporary extraction/collection counters, daytime: owner working, two tanks:
frame 300 queued 35,955 removals, of which 34,375 had changed global transforms, 35,584 had changed
previous transforms, and none had changed visibility. Material-readiness retries were zero.
Examples: `cap_bottom_00.CutWood`, `cap_top_00.CutWood`, `trunk_00.Bark`, and unnamed tank mesh
entities. The full snapshot table and the game-side intervention are in
`../.agents/scratch/perf-2026-10-29/bookkeeping-diagnosis.md`.

## Source path

- `extract_meshes_for_gpu_building` selects meshes on changes including `GlobalTransform` and
  `PreviousGlobalTransform`, independently of whether the mesh is visible.
- `extract_mesh_for_gpu_building` calls `queue.remove(entity.into(), any_gpu_culling)` whenever
  `ViewVisibility::get()` is false, even if the render mesh-instance map already lacks the entity.
- `collect_meshes_for_gpu_building` sends every queued removal separately through
  `std::sync::mpsc`. Its consumer calls `remove_mesh_input_uniform`, which returns immediately
  when the map lacks the entity, then `RenderGpuCulledEntities::remove`, which unconditionally
  appends to its removal vector even if its map also lacked the entity.
- The channel symbol in a sampler is insufficient to infer material readiness: the removal
  and readiness-retry channels carry the same entity type.

## Candidate upstream optimization

Avoid queueing a removal when the entity has no live instance to remove. Alternatively, batch
worker removal messages, eliminating the per-entity channel wakeups while preserving current
lifecycle handling. These are proposals, not validated engine patches: test visibility transitions,
material readiness, component remove/reinsert, CPU/GPU culling switches, despawn, and instance-buffer
compaction before applying an engine change.

## Application workaround

Ease only the bodies that move (opt-in interpolation), and avoid identical local-transform
assignments.
This removes the spurious tree invalidations without changing their hierarchy or visible meshes.
Moving invisible tank meshes can still take this path; removing their mesh components and restoring
them later would add view lifecycle state solely to bypass engine bookkeeping. That workaround is
not applied.
