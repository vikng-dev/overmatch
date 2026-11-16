# Non-solid colliders in sweeps and CCD thickness

Subject: rigid-body collision in a multiplayer tank video game; game state only.

**Target:** published avian3d 0.8.0. **Status:** LOCAL DRAFT, not filed; upstream head unchecked.
Source checked against the cached published crate without web access. The disabled sweep
exclusion is already in upstream PR #1101; the query extends it to sensors at the same site.

## Mechanism and shipping scope

Target tree traversal rejects sensor proxies, but the moving-side query does not reject Sensor.
A forward sensor can therefore clamp its body's motion before the solid collider meets a wall.
Our fork carries only the moving-side query exclusion, with a game-side collision test covering
both sensors and disabled colliders. MEASURED: that test passes; removing the sensor exclusion
fails it. The fork does not change body thickness or marker invalidation.

The cached body CCD thickness also includes sensors and disabled colliders, so a thin non-solid
attachment can cause unnecessary fast-body work. The broader proposal below excludes those
thicknesses. Additions invalidate via change detection; removals queue the owning body through
`On<Remove<Sensor>>` and `On<Remove<ColliderDisabled>>` observers, following the observer pattern
in published `backend.rs` and `collider_tree/update.rs`. Each backend retains its own set until
the next physics step. `RemovedComponents` messages are unsuitable: they expire after two frame
updates and a fixed physics step need not run in that window. The queue stores bodies, so a
later detached collider cannot erase its former owner's pending refresh.

The thickness proposal is **compile-checked only**, not shipped or runtime-tested in this lane.
The earlier transition test was removed with the fork's thickness change; its previous outcome
does not verify this observer proposal. No shipped game path inserts or removes Sensor.

Sweep radius is unchanged. Its current readers are the CCD speed test and contact recycling;
it is not consumed by sleeping. The published metrics loop is otherwise retained, independent
of the game's batching and instrumentation patches.

## Apply to published 0.8.0

Apply with `patch -p1` at the extracted crate root; no other fork patch is required.

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
@@ -6,11 +6,14 @@

 use core::marker::PhantomData;

-use bevy::{ecs::system::StaticSystemParam, prelude::*};
+use bevy::{
+    ecs::{entity::EntityHashSet, system::StaticSystemParam},
+    prelude::*,
+};

 use crate::{
     collision::collider::{
-        AnyCollider, ColliderContext,
+        AnyCollider, ColliderContext, ColliderDisabled, Sensor,
         collider_hierarchy::{ColliderOf, RigidBodyColliders},
         collider_transform::ColliderTransform,
     },
@@ -35,7 +38,7 @@
     /// to determine how far the body can move in a single timestep before it might start to
     /// tunnel through geometry.
     ///
-    /// This is the minimum [`ccd_thickness`] of the colliders attached to the body,
+    /// This is the minimum [`ccd_thickness`] of the enabled, non-sensor colliders attached to the body,
     /// and typically corresponds to the minimum distance from the centroid
     /// of any given shape to its surface.
     ///
@@ -72,6 +75,13 @@

 impl<C: AnyCollider> Plugin for BodySizeMetricsPlugin<C> {
     fn build(&self, app: &mut App) {
+        app.insert_resource(MarkerRemovalRefresh::<C> {
+            bodies: EntityHashSet::default(),
+            marker: PhantomData,
+        });
+        app.add_observer(queue_marker_removal::<C, Sensor>);
+        app.add_observer(queue_marker_removal::<C, ColliderDisabled>);
+
         // Update body size metrics before they are consumed by the solver and continuous collision
         // detection. Allowing ambiguities lets multiple collision backends coexist.
         app.add_systems(
@@ -80,6 +90,25 @@
                 .before(PhysicsStepSystems::Solver)
                 .ambiguous_with_all(),
         );
+    }
+}
+
+// Removal observers retain invalidations until the next physics step, even when no
+// fixed step runs during the frame in which a marker is removed. Each backend drains its own set.
+#[derive(Resource)]
+struct MarkerRemovalRefresh<C: AnyCollider> {
+    bodies: EntityHashSet,
+    marker: PhantomData<C>,
+}
+
+fn queue_marker_removal<C: AnyCollider, M: Component>(
+    event: On<Remove<M>>,
+    colliders: Query<&ColliderOf>,
+    mut refresh: ResMut<MarkerRemovalRefresh<C>>,
+) {
+    // Standalone and already-detached colliders have no owning body's metric to invalidate.
+    if let Ok(owner) = colliders.get(event.entity) {
+        refresh.bodies.insert(owner.body);
     }
 }

@@ -96,8 +125,23 @@
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
+    mut marker_refresh: ResMut<MarkerRemovalRefresh<C>>,
     context: StaticSystemParam<C::Context>,
 ) {
     let context = context.into_inner();
@@ -107,11 +151,15 @@
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
@@ -133,12 +181,16 @@

     // Bodies with a collider that changed. Note that bodies may be visited
     // multiple times, but the end result will be the same.
-    if changed_colliders.is_empty() {
+    if changed_colliders.is_empty() && marker_refresh.bodies.is_empty() {
         return;
     }
     let mut all_bodies = bodies.p1();
-    for collider_of in &changed_colliders {
-        if let Ok((mut size_metrics, rb_colliders, com)) = all_bodies.get_mut(collider_of.body) {
+    for body in changed_colliders
+        .iter()
+        .map(|owner| owner.body)
+        .chain(marker_refresh.bodies.drain())
+    {
+        if let Ok((mut size_metrics, rb_colliders, com)) = all_bodies.get_mut(body) {
             *size_metrics = compute(rb_colliders, com);
         }
     }
```

MEASURED: this patch applies cleanly to the cached published crate and the complete proposal
passes the offline compile below. The published manifest is unchanged; a sibling harness uses
`default-features = false`, `features = ["3d", "parry-f32"]` and `pub use avian3d;`, avoiding
unused, uncached dev-dependencies.

```sh
scripts/serial-cargo check --offline --lib --manifest-path \
  /private/tmp/ccd4-drafts/sensors/check-harness/Cargo.toml \
  --target-dir target/draft-check-ccd --features dev_ui,bitprobe
```

Archive SHA-256: `ca7479ccec5f1440675946bada8c805f2b6d514af78e1d2b0937ca53b1e2c543`.

## Retirement

Retire our moving-side exclusion when a published release excludes non-solid moving colliders.
The thickness proposal has no shipping code to retire. Per-collider eligibility is independent:
[its draft](avian-ccd-decide-per-collider.md) retains the body early-out and uses each child's own
thickness for sweep selection. Verification and repeated pricing are in
[the continuing lane report](../.agents/scratch/swept-ccd-decisions-and-price.md).
