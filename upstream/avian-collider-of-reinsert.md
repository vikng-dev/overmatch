# Hierarchy observers re-insert a `ColliderOf` that already names the right body

Found in a game using avian3d. Local draft; not filed.

**Crate:** avian3d 0.8.0 (crates.io). **Status:** not sent. Source audit: Unchanged on `main` at 2c2d6934 (2026-11-04): MEASURED, `main`'s `src/` differs from 0.8.0 only in `dynamics/ccd/mod.rs` (#1101), and this diff applies to `main` with `patch --dry-run`.

## Defect

`ColliderHierarchyPlugin` inserts `ColliderOf { body }` from two observers
(`src/collision/collider/collider_hierarchy/plugin.rs`): one on `Add<(RigidBody, ColliderMarker)>`
and `on_collider_body_changed` on `Insert<(ChildOf, RigidBody, AncestorMarker<ColliderMarker>)>`.
Neither checks whether the collider already has a `ColliderOf` naming that body. `ColliderOf` is an
immutable relationship component, so each redundant insert runs its `on_discard` and `on_insert`
hooks (removing the collider from the body's `RigidBodyColliders` and adding it back) and fires every
`Insert<ColliderOf>` observer, for a value that did not change.

A collider spawned as a child of its body with its `ColliderOf` already in the bundle is attached
4 times instead of once (MEASURED on 0.8.0, found in a game using avian3d).

## Minimal repro

```rust
let body = world.spawn(RigidBody::Dynamic).id();
world.spawn((Collider::sphere(0.5), ColliderOf { body }, ChildOf(body)));
// Count `Insert<ColliderOf>` with an observer: expected 1, observed 4 (MEASURED on 0.8.0).
```

## Fix

Against the pristine crate (`a/` and `b/` are the crate root):

```diff
--- a/src/collision/collider/collider_hierarchy/plugin.rs
+++ b/src/collision/collider/collider_hierarchy/plugin.rs
@@ -17,6 +17,7 @@
              mut commands: Commands,
              query: Query<(), With<ColliderMarker>>,
              rb_query: Query<Entity, With<RigidBody>>,
+             attachments: Query<&ColliderOf>,
              parent_query: Query<&ChildOf>| {
                 let entity = trigger.entity;
 
@@ -34,6 +35,10 @@
                     },
                     Some,
                 ) {
+                    // A collider spawned with its `ColliderOf` already in place needs no re-insert.
+                    if attachments.get(entity).is_ok_and(|of| of.body == body) {
+                        return;
+                    }
                     commands.entity(entity).insert(ColliderOf { body });
                 }
             },
@@ -54,7 +59,12 @@
         );
 
         // Update `ColliderOf` components for colliders when their ancestors change.
-        app.add_observer(on_collider_body_changed);
+        app.add_observer(
+            on_collider_body_changed::<(ChildOf, AncestorMarker<ColliderMarker>), false>,
+        );
+        // A rigid body inserted into a hierarchy always re-attaches its colliders, which repairs
+        // colliders attached before the body existed.
+        app.add_observer(on_collider_body_changed::<RigidBody, true>);
 
         // Remove `ColliderOf` from colliders when their rigid bodies are removed.
         app.add_observer(on_body_removed);
@@ -63,8 +73,8 @@
 
 /// Updates [`ColliderOf`] components for colliders when their ancestors change
 /// or when a rigid body is added to the hierarchy.
-fn on_collider_body_changed(
-    trigger: On<Insert<(ChildOf, RigidBody, AncestorMarker<ColliderMarker>)>>,
+fn on_collider_body_changed<B: Bundle, const BODY_INSERTED: bool>(
+    trigger: On<Insert<B>>,
     mut commands: Commands,
     query: Query<
         Has<ColliderMarker>,
@@ -74,6 +84,7 @@
     parent_query: Query<&ChildOf>,
     body_query: Query<Entity, With<RigidBody>>,
     collider_query: Query<(), With<ColliderMarker>>,
+    attachments: Query<&ColliderOf>,
 ) {
     let entity = trigger.entity;
 
@@ -94,8 +105,13 @@
         return;
     };
 
+    // A hierarchy change re-attaches only colliders that do not already name this body.
+    let needs_attachment = |collider: Entity| {
+        BODY_INSERTED || !attachments.get(collider).is_ok_and(|of| of.body == body)
+    };
+
     // Insert `ColliderOf` for the new child entity.
-    if is_collider {
+    if is_collider && needs_attachment(entity) {
         commands.entity(entity).insert(ColliderOf { body });
     }
 
@@ -109,7 +125,7 @@
         }
 
         // Update the `ColliderOf` component.
-        if collider_query.contains(child) {
+        if collider_query.contains(child) && needs_attachment(child) {
             commands.entity(child).insert(ColliderOf { body });
         }
     }
```

## Notes

`RigidBody` insertion keeps the unconditional re-insert: it repairs colliders that were attached
before their body existed, and a body can be re-inserted on an entity whose colliders name an older
body. Only hierarchy changes (`ChildOf`, `AncestorMarker`) and the add observer skip an attachment
that already names the nearest body. The observer is split into two instances of one generic
function.
