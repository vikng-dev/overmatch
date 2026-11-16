# The moved-collider AABB pass fetches every collider every step

Found in a game using avian3d. Local draft; not filed.

**Crate:** avian3d 0.8.0 (crates.io). **Status:** not sent. Source audit: Unchanged on `main` at 2c2d6934 (2026-11-04): MEASURED, `main`'s `src/` differs from 0.8.0 only in `dynamics/ccd/mod.rs` (#1101), and this diff applies to `main` with `patch --dry-run`. Reported as issue #1102 (open), and named in the pass's own TODO.

## Defect

`update_moved_collider_aabbs` (`src/collider_tree/update.rs`) runs a parallel iteration over every
enabled collider, fetching its pose, shape, AABBs, margin and proxy key, and only then skips it when
none of its `Position`, `Rotation` and shape ticks is newer than `LastPhysicsTick`. A world of
standing colliders pays the fetch for all of them every step.

## Minimal repro

Spawn 10,000 static colliders and step with nothing moving: the pass visits 10,000 colliders per
step and updates none (DERIVED).

## Fix

Against the pristine crate (`a/` and `b/` are the crate root):

```diff
--- a/src/collider_tree/update.rs
+++ b/src/collider_tree/update.rs
@@ -18,7 +18,7 @@
     ecs::{
         change_detection::Tick,
         entity_disabling::Disabled,
-        query::QueryFilter,
+        query::{QueryFilter, QueryItem},
         system::{StaticSystemParam, SystemChangeTick},
     },
     platform::collections::HashMap,
@@ -857,25 +857,33 @@
 
     diagnostics.update += start.elapsed();
 }
+
+/// The data [`update_moved_collider_aabbs`] reads per collider.
+type MovedCollider<C> = (
+    Entity,
+    Ref<'static, Position>,
+    Ref<'static, Rotation>,
+    &'static mut ColliderAabb,
+    &'static mut EnlargedAabb,
+    &'static mut ColliderAabbMargin,
+    Ref<'static, C>,
+    Option<&'static CollisionMargin>,
+    &'static ColliderTreeProxyKey,
+);
 
 /// Updates the AABBs of colliders that have been manually moved after the previous physics step.
 pub fn update_moved_collider_aabbs<C: AnyCollider>(
     mut colliders: ParamSet<(
+        Query<MovedCollider<C>, Without<ColliderDisabled>>,
+        Query<&EnlargedAabb, Without<ColliderDisabled>>,
+        // The same colliders, pruned by their change ticks.
         Query<
+            MovedCollider<C>,
             (
-                Entity,
-                Ref<Position>,
-                Ref<Rotation>,
-                &mut ColliderAabb,
-                &mut EnlargedAabb,
-                &mut ColliderAabbMargin,
-                Ref<C>,
-                Option<&CollisionMargin>,
-                &ColliderTreeProxyKey,
+                Without<ColliderDisabled>,
+                Or<(Changed<Position>, Changed<Rotation>, Changed<C>)>,
             ),
-            Without<ColliderDisabled>,
         >,
-        Query<&EnlargedAabb, Without<ColliderDisabled>>,
     )>,
     narrow_phase_config: Res<NarrowPhaseConfig>,
     length_unit: Res<PhysicsLengthUnit>,
@@ -910,11 +918,8 @@
 
     // TODO: This doesn't do velocity-based enlargement like the dynamic/kinematic AABB update.
     //       We should overall rework CCD to not rely on velocity-based AABB enlargement for all bodies.
-    // TODO: par-iter over all colliders, check if they have actually changed since the `LastPhysicsTick`
-    let mut collider_query = colliders.p0();
-    collider_query.par_for_each_mut(
-        MIN_PAR_ITER_ENTITIES,
-        |(
+    let update_moved = |item: QueryItem<'_, '_, MovedCollider<C>>| {
+        let (
             entity,
             pos,
             rot,
@@ -924,7 +929,8 @@
             collider,
             collision_margin,
             proxy_key,
-        )| {
+        ) = item;
+        {
             // Skip if the collider's AABB can't have changed since the last physics tick.
             let collider_changed = collider.last_changed().is_newer_than(last_tick.0, this_run);
             if !pos.last_changed().is_newer_than(last_tick.0, this_run)
@@ -980,8 +986,19 @@
 
                 thread_local_bit_vec.set(proxy_key.id().index());
             }
-        },
-    );
+        }
+    };
+    // When `LastPhysicsTick` is newer than this system's previous run, every collider the tick
+    // check above accepts also passes the change filter, so the filtered query visits only
+    // colliders that moved or changed shape. `par_iter_mut` directly: counting a change-filtered
+    // query first would visit every collider again. Otherwise, visit every collider.
+    if last_tick.0.is_newer_than(system_tick.last_run(), this_run) {
+        colliders.p2().par_iter_mut().for_each(update_moved);
+    } else {
+        colliders
+            .p0()
+            .par_for_each_mut(MIN_PAR_ITER_ENTITIES, update_moved);
+    }
 
     // Reinsert moved proxies in each tree.
     let aabb_query = colliders.p1();
```

## Notes

The filtered query (`Or<(Changed<Position>, Changed<Rotation>, Changed<C>)>`) is exact while
`LastPhysicsTick` is newer than the system's previous run: every tick newer than `LastPhysicsTick`
is then also newer than that run, so every collider the body's check accepts passes the filter.
Inside `PhysicsSchedule` that always holds, since `update_last_physics_tick` runs after the pass
each step; otherwise the pass falls back to the unfiltered query. It iterates with `par_iter_mut`
directly, since counting a change-filtered query first would visit every collider again.

An unconditional child-pose writer makes even standing children pass this filter every step.
The [Avian access proposal](avian-child-collider-pose-churn.md) lets an application replace that
writer; the [Lightyear proposal](lightyear-avian-child-collider-pose-churn.md) avoids marking
unchanged poses through `set_if_neq`.
