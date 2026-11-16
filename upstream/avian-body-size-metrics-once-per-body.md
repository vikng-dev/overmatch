# Body size metrics recompute a body once per changed collider

Found in a game using avian3d. Local draft; not filed.

**Crate:** avian3d 0.8.0 (crates.io). **Status:** not sent. Source audit: Unchanged on `main` at 2c2d6934 (2026-11-04): MEASURED, `main`'s `src/` differs from 0.8.0 only in `dynamics/ccd/mod.rs` (#1101), and this diff applies to `main` with `patch --dry-run`.

## Defect

`update_body_size_metrics` (`src/dynamics/rigid_body/body_size_metrics.rs`) recomputes a body's
`BodySizeMetrics` over all of the body's colliders, once per changed collider: its own comment says
"bodies may be visited multiple times, but the end result will be the same". A body whose N
colliders change in one step (spawn, rescale) costs N recomputations of N colliders each.

## Minimal repro

Spawn a body with 200 child colliders in one frame: 200 recomputations × 200 colliders = 40,000
collider visits, each scanning the collider's shape, where 200 suffice (DERIVED).

## Fix

Against the pristine crate (`a/` and `b/` are the crate root):

```diff
--- a/src/dynamics/rigid_body/body_size_metrics.rs
+++ b/src/dynamics/rigid_body/body_size_metrics.rs
@@ -6,7 +6,10 @@
 
 use core::marker::PhantomData;
 
-use bevy::{ecs::system::StaticSystemParam, prelude::*};
+use bevy::{
+    ecs::{entity::EntityHashSet, system::StaticSystemParam},
+    prelude::*,
+};
 
 use crate::{
     collision::collider::{
@@ -14,7 +17,7 @@
         collider_hierarchy::{ColliderOf, RigidBodyColliders},
         collider_transform::ColliderTransform,
     },
-    dynamics::rigid_body::mass_properties::components::ComputedCenterOfMass,
+    dynamics::rigid_body::{RigidBody, mass_properties::components::ComputedCenterOfMass},
     math::ToRealPrecision,
     schedule::{PhysicsSchedule, PhysicsStepSystems},
 };
@@ -90,15 +93,23 @@
 );
 
 fn update_body_size_metrics<C: AnyCollider>(
+    mut bodies: Query<BodySizeMetricsData>,
     // Body size metrics only need updating when the center of mass
     // or one of the colliders changes.
-    mut bodies: ParamSet<(
-        Query<BodySizeMetricsData, Changed<ComputedCenterOfMass>>,
-        Query<BodySizeMetricsData>,
-    )>,
+    // A body with no collider has no `RigidBodyColliders` (Avian removes it with
+    // the last one) and no size to measure.
+    moved_centers: Query<
+        Entity,
+        (
+            With<RigidBody>,
+            With<RigidBodyColliders>,
+            Changed<ComputedCenterOfMass>,
+        ),
+    >,
     colliders: Query<(Entity, &C, &ColliderTransform)>,
     changed_colliders: Query<&ColliderOf, Or<(Changed<C>, Changed<ColliderTransform>)>>,
     context: StaticSystemParam<C::Context>,
+    mut stale: Local<EntityHashSet>,
 ) {
     let context = context.into_inner();
 
@@ -126,20 +137,18 @@
         }
     };
 
-    // Bodies whose center of mass changed.
-    for (mut size_metrics, rb_colliders, com) in &mut bodies.p0() {
-        *size_metrics = compute(rb_colliders, com);
-    }
-
-    // Bodies with a collider that changed. Note that bodies may be visited
-    // multiple times, but the end result will be the same.
-    if changed_colliders.is_empty() {
+    // A stale body is recomputed once, however many of its colliders changed.
+    // Each stale body owns its metrics and reads only its own colliders, so they run on the
+    // compute pool; a step with no stale body dispatches nothing.
+    stale.clear();
+    stale.extend(&moved_centers);
+    stale.extend(changed_colliders.iter().map(|collider_of| collider_of.body));
+    if stale.is_empty() {
         return;
     }
-    let mut all_bodies = bodies.p1();
-    for collider_of in &changed_colliders {
-        if let Ok((mut size_metrics, rb_colliders, com)) = all_bodies.get_mut(collider_of.body) {
+    bodies.par_iter_many_unique_mut(&*stale).for_each(|body| {
+        if let Ok((mut size_metrics, rb_colliders, com)) = body {
             *size_metrics = compute(rb_colliders, com);
         }
-    }
+    });
 }
```

## Notes

Each stale body (center of mass changed, or any collider changed) is collected once into a reused
`EntityHashSet` and recomputed once. Each body owns its mutable metrics and reads only its own
colliders, so `par_iter_many_unique_mut` computes independent bodies in parallel. The set makes a
second visit unrepresentable; an empty stale set dispatches nothing.
