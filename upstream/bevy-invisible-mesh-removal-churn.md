# Invisible moving meshes re-queue a mesh-instance removal every frame

**LOCAL DRAFT, PR-ready, not filed.** Overmatch carries both changes in its vendored `bevy_pbr`
0.19.1 (`vendor/bevy_pbr-0.19.1-scalar-math/OVERMATCH_PATCH.md`, "Mesh instance removals"; issue
#173). Bevy `main` still has the stock behaviour of both: VERIFIED in
`crates/bevy_pbr/src/render/mesh.rs` at `c0b545c` (2026-10-20, the newest commit touching the file
on 2026-10-30). A search of Bevy's issues and PRs found none on it; open PR #25956 edits
`extract_meshes_for_gpu_building` near these lines, so a rebase may be needed. **Frame-time delta:
not yet measured** (the orchestrator measures it; add it under Measurements before sending).

## PR title

`Don't re-queue mesh-instance removals for invisible meshes that have no instance`

## PR body

### Objective

`extract_meshes_for_gpu_building` selects meshes whose `GlobalTransform` or
`PreviousGlobalTransform` changed, whether they are visible or not, and `extract_mesh_for_gpu_building`
queues a removal for every invisible one. A hidden mesh that keeps moving (an LOD level that is out
of range, a part hidden inside its parent, anything outside every frustum under a moving parent)
therefore queues a removal every frame, long after its instance was removed on the frame it went
invisible. `collect_meshes_for_gpu_building` then sends each removal through an `mpsc` channel, one
send per entity, and its consumer looks the entity up, finds nothing, and appends it to
`RenderGpuCulledEntities::removed`, which every view then walks.

In a scene of 30 vehicles (each a hierarchy of 808 entities whose global transforms change every
frame, MEASURED), 8,040 removals were queued per frame, of which 0.02 per frame had an instance to
remove (MEASURED; Measurements below).

### Solution

Two independent commits; either stands alone.

1. **Apply removals without the channel.** The worker tasks leave each queue's `removed` list in
   place, and the consumer drains every queue after the prepared instances are applied: the same
   point in the frame where the channel's removals were applied. Removals were never ordered among
   themselves, since the channel interleaved the workers. No other behaviour changes.
2. **Queue a removal only for an entity that has an instance.** In
   `extract_mesh_for_gpu_building`, an invisible mesh queues its removal only if
   `render_mesh_instances` contains it. This is sound because:
   - every slot of the current input buffer and every `RenderGpuCulledEntities::entities` entry
     belongs to an instance in that map, so a removal for an entity without one changes nothing
     except re-listing it in `RenderGpuCulledEntities::removed`, which each view acts on only for
     entities it already lists;
   - extraction reads the map as the previous frame's `collect_meshes_for_gpu_building` left it, and
     this frame's collection adds an instance only for an entity queued as changed this frame. An
     invisible entity is not, because `extract_meshes_for_gpu_building` visits each entity at most
     once per frame (the re-extraction bitfield skips entities the changed query saw).

   A `Mesh3d` that is removed, or an entity that is despawned, still queues its removal
   unconditionally from the `RemovedComponents<Mesh3d>` loop.

   One behaviour is not reproduced: a view absent from `collect_gpu_culled_meshes` in the frame its
   entity was removed keeps listing the entity in `entities_gpu_culling`. The redundant removals
   cleaned that up only if the entity happened to move while invisible; neither version guarantees
   it.

### Testing

Render-world lifecycle tests on a real adapter (Overmatch's
`src/tests/bevy_mesh_removal_lifecycle.rs`; Bevy's CI has no GPU either, so they would need a home
that does). Each drives `PbrPlugin` through one family of edges and checks after every frame that
the mesh instances are exactly the visible meshes with a `Mesh3d`, that input slots are owned one
to one and hold the entity's current translation, and that the GPU-culled sets lie inside the
instances:

- visibility transitions: `Visibility::Hidden`, a hidden parent, leaving the frustum, one-frame
  hides, each while moving;
- material readiness: a reserved material handle hidden and moved before its asset arrives, and one
  that arrives while visible;
- `Mesh3d` removed and reinserted while visible, within one frame, while hidden and moving;
- `NoCpuCulling` toggled while visible and while hidden, and the frame's culling mode switched by
  replacing the camera;
- despawns while visible, hidden, hidden and moving, GPU-culled, and with the parent.

All five pass on stock 0.19.1, with commit 1, and with both (MEASURED). A sixth test pins commit 2's
contract (one removal, for the instance, on the frame a mesh goes invisible; none while it moves
hidden; none for a mesh whose material never became ready). It fails on stock and with commit 1
alone, on the transition frame and on every moving frame, and passes with commit 2 (MEASURED).

Found while writing them, outside this change: inserting or removing `NoIndirectDrawing` on a live
camera fails wgpu validation in 0.19.1 (`preprocess_gpu_indexed_frustum_culling_bind_group` used
with the `mesh preprocessing (direct)` pipeline; MEASURED on Metal).

### Measurements

Overmatch, 30 vehicles, hidden offscreen window, M4 / Metal, dev build; counts after extraction and
before collection, each window about 5 s, startup windows excluded (MEASURED counts, DERIVED rates):

| | Frames | Removals queued | With an instance | Channel sends |
|---|---|---|---|---|
| stock | 1,701 | 13,676,214 (8,040 / frame) | 33 | 8,040 / frame (one per removal) |
| both commits | 1,234 | 43 (0.03 / frame) | 43 | none |

Frame time: pending. An earlier sampling profile of the same scene put
`Sender<MainEntity>::send` from this collector among the top busy stacks (CLAIMED in
`.agents/scratch/perf-2026-10-29/bookkeeping-diagnosis.md`, from an intrusive sampler whose frame
numbers are not used).

Pictures: three scenes captured before and after at a fixed step are byte-identical (MEASURED;
`.agents/scratch/perf-2026-10-29/removal-173/`).

## Diff against `v0.19.1`

The vendored copy carries the same lines, marked `OVERMATCH PATCH (removal batching)` and
`OVERMATCH PATCH (removal skip)`. Rebase onto `main` before sending; the hunks below apply to
`main`'s identical code at other line numbers.

Commit 1, `crates/bevy_pbr/src/render/mesh.rs`, `collect_meshes_for_gpu_building`:

```diff
     let (prepared_tx, prepared_rx) = mpsc::channel();
     let (reextract_tx, reextract_rx) = mpsc::channel();
-    let (removed_tx, removed_rx) = mpsc::channel();
 ...
                     RenderMeshInstanceGpuQueue::CpuCulling {
-                        ref mut changed,
-                        ref mut removed,
+                        ref mut changed, ..
                     } => {
                         let prepared_tx = prepared_tx.clone();
                         let reextract_tx = reextract_tx.clone();
-                        let removed_tx = removed_tx.clone();
 ...
                                 });
-
-                            for entity in removed.drain(..) {
-                                removed_tx.send(entity).unwrap();
-                            }
                         });
 ...
                     RenderMeshInstanceGpuQueue::GpuCulling {
                         ref mut changed_cpu_culling,
                         ref mut changed_gpu_culling,
-                        ref mut removed,
+                        ..
                     } => {
                         let prepared_tx = prepared_tx.clone();
                         let reextract_tx = reextract_tx.clone();
-                        let removed_tx = removed_tx.clone();
 ...
                                     .set(current_uniform_index, mesh_culling_builder);
                             }
-
-                            for entity in removed.drain(..) {
-                                removed_tx.send(entity).unwrap();
-                            }
                         });
 ...
             drop(prepared_tx);
             drop(reextract_tx);
-            drop(removed_tx);
         });
 ...
-    while let Ok(batch) = removed_rx.recv() {
-        let entity = batch;
-        remove_mesh_input_uniform(entity, &mut *render_mesh_instances, current_input_buffer);
-        render_gpu_culled_entities.remove(entity);
+    // Apply each queue's removals after every prepared instance. Their order among themselves is
+    // undefined, as it was when the workers interleaved them through a channel.
+    for queue in render_mesh_instance_queues.iter_mut() {
+        let (RenderMeshInstanceGpuQueue::CpuCulling { removed, .. }
+        | RenderMeshInstanceGpuQueue::GpuCulling { removed, .. }) = queue
+        else {
+            continue;
+        };
+        for entity in removed.drain(..) {
+            remove_mesh_input_uniform(entity, &mut *render_mesh_instances, current_input_buffer);
+            render_gpu_culled_entities.remove(entity);
+        }
     }
```

Commit 2, `extract_mesh_for_gpu_building`:

```diff
     // If the entity is invisible, remove it.
     if !view_visibility.get() {
-        queue.remove(entity.into(), any_gpu_culling);
+        // Only an entity with an instance has anything to remove: every input slot and every
+        // `RenderGpuCulledEntities::entities` entry belongs to an instance in this map. The map is
+        // as the previous frame's `collect_meshes_for_gpu_building` left it, and this frame's adds
+        // an instance only for an entity queued as changed, which this one is not.
+        if render_mesh_instances.contains_key(&MainEntity::from(entity)) {
+            queue.remove(entity.into(), any_gpu_culling);
+        }
         return;
     }
```

## Application side (Overmatch)

Issue #168 already stopped the static forest from reaching this path (static bodies no longer take
interpolation writes; `bookkeeping-diagnosis.md`). What remained were the vehicles' own hidden
meshes, which move with the vehicle and cannot be kept still. Removing their `Mesh3d` while hidden
would avoid the path, at the cost of view lifecycle state kept only to bypass engine bookkeeping;
that was not done.

## What fixing this unlocks for us

Dropping both vendored patches at the next Bevy bump, if upstream takes them. Nothing else depends
on them.
