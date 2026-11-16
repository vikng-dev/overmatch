# Hidden moving meshes queue a mesh-instance removal every frame

Found in a game using bevy_pbr. Local draft; not filed.

**Crate:** bevy_pbr 0.20.1 (crates.io). **Status:** not sent. Source audit: The touched lines are unchanged on bevy `main` at ef5312a2 (2026-11-15), and this diff applies there with `patch --dry-run` (MEASURED).

## Defect

`extract_meshes_for_gpu_building` selects meshes whose transforms changed, visible or not, and
`extract_mesh_for_gpu_building` queues a removal for every invisible one
(`src/render/mesh.rs:2166-2170`). A hidden mesh that keeps moving (an out-of-range LOD, a part
hidden under a moving parent, anything outside every frustum) queues a removal every frame, long
after its instance was removed. `collect_meshes_for_gpu_building` sends each through a channel, finds
no instance, and lists the entity in `RenderGpuCulledEntities::removed`, which every view walks.

## Minimal repro

Spawn 10,000 meshes with `Visibility::Hidden` under a parent whose `Transform` changes every frame:
10,000 removals queued and sent per frame, none with an instance (DERIVED). A scene with ~8,000
hidden moving meshes queued 8,040 removals per frame, 0.02 per frame with an instance to remove
(MEASURED on bevy 0.19.1, same code, found in a game using bevy).

## Fix

Against the pristine crate (`a/` and `b/` are the crate root):

```diff
--- a/src/render/mesh.rs
+++ b/src/render/mesh.rs
@@ -2163,9 +2163,13 @@
     queue: &mut RenderMeshInstanceGpuQueue,
     any_gpu_culling: bool,
 ) {
-    // If the entity is invisible, remove it.
+    // If the entity is invisible, remove its instance. One without an instance has nothing to
+    // remove: every input slot and every GPU-culled entry belongs to an instance in
+    // `render_mesh_instances`, and this frame adds instances only for changed, visible meshes.
     if !view_visibility.get() {
-        queue.remove(entity.into(), any_gpu_culling);
+        if render_mesh_instances.contains_key(&MainEntity::from(entity)) {
+            queue.remove(entity.into(), any_gpu_culling);
+        }
         return;
     }
```

## Notes

Only an entity with an instance has anything to remove: every input slot and every
`RenderGpuCulledEntities::entities` entry belongs to an instance in `RenderMeshInstancesGpu`, and
this frame adds instances only for changed, visible meshes. Independent of
[the removal-channel draft](bevy-mesh-removal-channel.md).
