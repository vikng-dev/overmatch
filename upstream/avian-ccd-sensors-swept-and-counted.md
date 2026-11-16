# Sensors are swept as solid colliders and counted toward CCD thickness

Subject: rigid-body continuous collision in a multiplayer tank video game; game state only.

**Target:** published avian3d 0.8.0, `solve_continuous` and `update_body_size_metrics`.
**Status:** LOCAL DRAFT, not filed; upstream head unchecked. Source verified against the cached
published crate without network access. The disabled-collider sweep exclusion is already in
upstream PR #1101; the proposed query extends that exclusion to sensors at the same site.

## Mechanism

The target-side tree traversal skips sensor proxies, but the moving-side collider query does
not exclude `Sensor`. A fast body's forward sensor can therefore produce its earliest time of
impact and clamp the whole body before a solid collider reaches the obstacle. Sensors register
intersections, so they must not impose solid collision response from either side.

The minimum `BodySizeMetrics::ccd_thickness` also includes sensors and disabled colliders. A
thin non-solid attachment consequently makes an otherwise thick body enter the fast-body path.
Filtering the sweep alone leaves this unnecessary work. Filtering the cached thickness alone
leaves moving sensors on bodies that genuinely require CCD.

## Fix

Exclude both `Sensor` and `ColliderDisabled` in the existing sweep query. Skip their thicknesses
when taking the body's minimum. Refresh the metric when either marker is added or removed,
including when neither shape, transform nor centre of mass changes. Keep sweep-radius computation
unchanged: that separate metric also serves sleeping and contact recycling.

The published metrics loop is retained here; no game-side batching, hook or diagnostic API is
required. Despawned, detached or entity-disabled colliders absent from the owner query have no
enabled attachment to refresh at that point.

## Witnesses

`src/tests/avian_swept_ccd.rs` mounts the game's narrow physics harness, reads the existing
`SolverBodyFlags::IS_FAST`, and checks two class invariants for sensors and disabled colliders:

- A thin solid attachment first makes its body fast; marking it non-solid stops that, and
  removing the marker restores it. The centre of mass is held fixed so it cannot supply the
  invalidation. The server relies on non-solid attachments leaving solid CCD eligibility alone.
- A genuinely fast body passes the sensor's geometric impact plane while its solid collider
  still stops at the wall. The server relies on sensor intersections imposing no solid response.

Fixture dimensions and travel set up those relations; no prior physics output is an oracle.
MEASURED: both guards passed with the fix. Reversing the thickness exclusion failed the first;
restoring it and reversing the moving-sensor filter failed the second. Both mutants were reverted.
Detailed commands and evidence are in [the lane report](../.agents/scratch/swept-ccd-sensors-and-price.md).

## Apply to published 0.8.0

Apply at the extracted crate root with `patch -p1`. The patch carries no fork markers and depends
on none of the game's other patches.

```diff
--- a/src/dynamics/ccd/mod.rs
+++ b/src/dynamics/ccd/mod.rs
@@ -593,7 +593,10 @@
 /// at the first impact, leaving the next frame's collision detection to resolve the contact.
 #[cfg(any(feature = "parry-f32", feature = "parry-f64"))]
 fn solve_continuous(
-    colliders: Query<(&Collider, &Position, &Rotation, Option<&CollisionLayers>)>,
+    colliders: Query<
+        (&Collider, &Position, &Rotation, Option<&CollisionLayers>),
+        (Without<ColliderDisabled>, Without<Sensor>),
+    >,
     ccd_query: Query<CcdBodyQuery>,
     mut bodies: ResMut<SolverBodies>,
     trees: Res<ColliderTrees>,
--- a/src/dynamics/rigid_body/body_size_metrics.rs
+++ b/src/dynamics/rigid_body/body_size_metrics.rs
@@ -10,7 +10,7 @@

 use crate::{
     collision::collider::{
-        AnyCollider, ColliderContext,
+        AnyCollider, ColliderContext, ColliderDisabled, Sensor,
         collider_hierarchy::{ColliderOf, RigidBodyColliders},
         collider_transform::ColliderTransform,
     },
@@ -35,7 +35,7 @@
     /// to determine how far the body can move in a single timestep before it might start to
     /// tunnel through geometry.
     ///
-    /// This is the minimum [`ccd_thickness`] of the colliders attached to the body,
+    /// This is the minimum [`ccd_thickness`] of the enabled, non-sensor colliders attached to the body,
     /// and typically corresponds to the minimum distance from the centroid
     /// of any given shape to its surface.
     ///
@@ -96,8 +96,25 @@
         Query<BodySizeMetricsData, Changed<ComputedCenterOfMass>>,
         Query<BodySizeMetricsData>,
     )>,
-    colliders: Query<(Entity, &C, &ColliderTransform)>,
-    changed_colliders: Query<&ColliderOf, Or<(Changed<C>, Changed<ColliderTransform>)>>,
+    colliders: Query<(
+        Entity,
+        &C,
+        &ColliderTransform,
+        Has<Sensor>,
+        Has<ColliderDisabled>,
+    )>,
+    changed_colliders: Query<
+        &ColliderOf,
+        Or<(
+            Changed<C>,
+            Changed<ColliderTransform>,
+            Added<Sensor>,
+            Added<ColliderDisabled>,
+        )>,
+    >,
+    collider_owners: Query<&ColliderOf>,
+    mut removed_sensors: RemovedComponents<Sensor>,
+    mut removed_disabled: RemovedComponents<ColliderDisabled>,
     context: StaticSystemParam<C::Context>,
 ) {
     let context = context.into_inner();
@@ -107,11 +124,15 @@
         let mut ccd_thickness: f32 = f32::INFINITY;
         let mut sweep_radius: f32 = 0.0;

-        for (entity, collider, collider_transform) in colliders.iter_many(rb_colliders).matched() {
+        for (entity, collider, collider_transform, sensor, disabled) in
+            colliders.iter_many(rb_colliders).matched()
+        {
             // Compute the CCD thickness
-            let ctx = ColliderContext::new(entity, &context);
-            let thickness = collider.ccd_thickness_with_context(ctx);
-            ccd_thickness = ccd_thickness.min(thickness);
+            if !sensor && !disabled {
+                let ctx = ColliderContext::new(entity, &context);
+                let thickness = collider.ccd_thickness_with_context(ctx);
+                ccd_thickness = ccd_thickness.min(thickness);
+            }

             // Compute the sweep radius
             let ctx = ColliderContext::new(entity, &context);
@@ -133,11 +154,15 @@

     // Bodies with a collider that changed. Note that bodies may be visited
     // multiple times, but the end result will be the same.
-    if changed_colliders.is_empty() {
-        return;
-    }
     let mut all_bodies = bodies.p1();
-    for collider_of in &changed_colliders {
+    // Removed markers restore a solid collider. Despawned/detached colliders have no owner
+    // here; their removal is handled by the collider hierarchy and mass-property update.
+    for collider_of in changed_colliders.iter().chain(
+        removed_sensors
+            .read()
+            .chain(removed_disabled.read())
+            .filter_map(|entity| collider_owners.get(entity).ok()),
+    ) {
         if let Ok((mut size_metrics, rb_colliders, com)) = all_bodies.get_mut(collider_of.body) {
             *size_metrics = compute(rb_colliders, com);
         }
```

MEASURED: the patch applied cleanly to a fresh extraction of the cached published crate in
`scratchpad/draft-check-ccd/sensors-e_xlweg3/avian3d-0.8.0` under the lane's requested scratch root.
The unmodified published manifest is used through a sibling `check-harness` path dependency with
`default-features = false`, `features = ["3d", "parry-f32"]`; its source is `pub use avian3d;`.
This avoids resolving unused, uncached published dev-dependencies, as in the pruning draft.
`cargo check --offline --lib` through `scripts/serial-cargo` passed for Avian and the harness.
The full command and archive digest are in the lane report.

## What fixing this unlocks for us

Retire these CCD exclusions and marker invalidations when a published release provides the same
non-solid contract. Other fork patches remain independent. This fix neither implements nor
justifies per-collider motion gating; the hull-first wall probe remains a separate observation.
