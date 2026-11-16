# Mesh-instance removals cross an mpsc channel one entity at a time

Found in a game using bevy_pbr. Local draft; not filed.

**Crate:** bevy_pbr 0.20.1 (crates.io). **Status:** not sent. Source audit: The touched lines are unchanged on bevy `main` at ef5312a2 (2026-11-15), and this diff applies there with `patch --dry-run` (MEASURED).

## Defect

`collect_meshes_for_gpu_building` (`src/render/mesh.rs`) has each worker task drain its queue's
`removed` list into an `mpsc` channel, one `send` per entity, and the consumer applies them after
the prepared instances. The removals need no worker: they are already sorted per queue, and their
effects commute (the channel interleaved the workers, so their order was never defined).

## Minimal repro

Any frame with many removals (a large group of meshes hidden at once, or the per-frame churn in
[the invisible-mesh draft](bevy-invisible-mesh-removal-churn.md)) costs one channel send and
receive per removed entity (DERIVED).

## Fix

Against the pristine crate (`a/` and `b/` are the crate root):

```diff
--- a/src/render/mesh.rs
+++ b/src/render/mesh.rs
@@ -2540,7 +2540,6 @@
     // Channels used by parallel workers to send data to the single consumer.
     let (prepared_tx, prepared_rx) = mpsc::channel();
     let (reextract_tx, reextract_rx) = mpsc::channel();
-    let (removed_tx, removed_rx) = mpsc::channel();
 
     {
         // Reference data shared between tasks
@@ -2566,13 +2565,12 @@
                         // This can only happen if the queue is empty.
                     }
 
+                    // Removals stay in the queue for the consumer below.
                     RenderMeshInstanceGpuQueue::CpuCulling {
-                        ref mut changed,
-                        ref mut removed,
+                        ref mut changed, ..
                     } => {
                         let prepared_tx = prepared_tx.clone();
                         let reextract_tx = reextract_tx.clone();
-                        let removed_tx = removed_tx.clone();
                         scope.spawn(async move {
                             #[cfg(feature = "trace")]
                             let _span = info_span!("prepared_mesh_producer").entered();
@@ -2597,21 +2595,16 @@
                                         }
                                     }
                                 });
-
-                            for entity in removed.drain(..) {
-                                removed_tx.send(entity).unwrap();
-                            }
                         });
                     }
 
                     RenderMeshInstanceGpuQueue::GpuCulling {
                         ref mut changed_cpu_culling,
                         ref mut changed_gpu_culling,
-                        ref mut removed,
+                        ..
                     } => {
                         let prepared_tx = prepared_tx.clone();
                         let reextract_tx = reextract_tx.clone();
-                        let removed_tx = removed_tx.clone();
                         scope.spawn(async move {
                             #[cfg(feature = "trace")]
                             let _span = info_span!("prepared_mesh_producer").entered();
@@ -2692,10 +2685,6 @@
                                     .set(current_uniform_index, prepared.mesh_input_uniform);
                                 mesh_culling_data_buffer
                                     .set(current_uniform_index, mesh_culling_builder);
-                            }
-
-                            for entity in removed.drain(..) {
-                                removed_tx.send(entity).unwrap();
                             }
                         });
                     }
@@ -2707,7 +2696,6 @@
             // will finish. Without this, the scope would deadlock on the blocked consumer.
             drop(prepared_tx);
             drop(reextract_tx);
-            drop(removed_tx);
         });
     }
 
@@ -2739,10 +2727,18 @@
         };
         render_gpu_culled_entities.update(entity, render_layers, no_cpu_culling);
     }
-    while let Ok(batch) = removed_rx.recv() {
-        let entity = batch;
-        remove_mesh_input_uniform(entity, &mut *render_mesh_instances, current_input_buffer);
-        render_gpu_culled_entities.remove(entity);
+    // Apply each worker queue's removals here, after every prepared instance as before, with no
+    // channel message per entity. Removal effects commute: the channel never ordered them.
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
     while let Ok(batch) = reextract_rx.recv() {
         let entity = batch;
```

## Notes

The consumer drains every queue's `removed` list after all prepared instances, the same point in the
frame where the channel's removals were applied.
