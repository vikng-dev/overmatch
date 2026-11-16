# Mass recomputation aggregates descendants of fully explicit bodies, and queues once per collider

Found in a game using avian3d. Local draft; not filed.

**Crate:** avian3d 0.8.0 (crates.io). **Status:** not sent. Source audit: Unchanged on `main` at 2c2d6934 (2026-11-04): MEASURED, `main`'s `src/` differs from 0.8.0 only in `dynamics/ccd/mod.rs` (#1101), and this diff applies to `main` with `patch --dry-run`. Upstream's own TODO at the observer reads "Avoid duplicating work with the above observer".

## Defect

A body with `Mass`, `AngularInertia`, `CenterOfMass` and all three `NoAuto*` markers takes every
output from its own components: `MassPropertyHelper::update_mass_properties` computes
`total_mass_properties` over the body and all its descendants, then replaces each result with the
explicit value (`src/dynamics/rigid_body/mass_properties/system_param.rs:60-129`). Three paths do
that discarded work:

- the `On<Insert<RigidBodyColliders>>` observer recomputes on the body's first collider;
- `queue_mass_recomputation_on_collider_mass_change` queues `RecomputeMassProperties` for the body
  whenever any of its colliders' mass, transform or mass components change;
- the recompute itself aggregates the descendants.

The queue also issues one insert command per changed collider: a body whose N colliders changed
gets N commands for one marker (the recompute runs once, since it queries
`With<RecomputeMassProperties>`). The explicit-body skip is result-identical: every output of such a
body is replaced (DERIVED from `update_mass_properties`).

## Minimal repro

1. Spawn a body with `Mass(1.0)`, `AngularInertia`, `CenterOfMass`, `NoAutoMass`,
   `NoAutoAngularInertia`, `NoAutoCenterOfMass` and N child colliders.
2. Change every child's `ColliderDensity` in one frame.
3. Expected: no aggregation over the children, and at most one queue command. Actual: N insert
   commands for the body and one full aggregation over N colliders (DERIVED); a change to two
   children's mass queued 2 recomputations of one body (MEASURED on 0.8.0, found in a game using
   avian3d).

## Fix

Against the pristine crate (`a/` and `b/` are the crate root):

```diff
--- a/src/dynamics/rigid_body/mass_properties/mod.rs
+++ b/src/dynamics/rigid_body/mass_properties/mod.rs
@@ -291,7 +291,10 @@
         // TODO: Avoid duplicating work with the above observer.
         app.add_observer(
             |trigger: On<Insert<RigidBodyColliders>>, mut mass_helper: MassPropertyHelper| {
-                mass_helper.update_mass_properties(trigger.entity);
+                // Fully explicit mass properties do not depend on collider membership.
+                if !mass_helper.explicit.contains(trigger.entity) {
+                    mass_helper.update_mass_properties(trigger.entity);
+                }
             },
         );
 
@@ -353,6 +356,17 @@
     Changed<AngularInertia>,
     Changed<CenterOfMass>,
 )>;
+
+/// A query filter for entities whose [`Mass`], [`AngularInertia`], and [`CenterOfMass`] are all
+/// explicit, with automatic computation disabled for each, so descendants cannot affect them.
+pub(super) type ExplicitMassProperties = (
+    With<Mass>,
+    With<AngularInertia>,
+    With<CenterOfMass>,
+    With<NoAutoMass>,
+    With<NoAutoAngularInertia>,
+    With<NoAutoCenterOfMass>,
+);
 
 /// Queues mass property recomputation for rigid bodies when their [`Mass`], [`AngularInertia`],
 /// or [`CenterOfMass`] components are changed.
@@ -379,8 +393,11 @@
 /// [`Mass`], [`AngularInertia`], or [`CenterOfMass`] components of their colliders are changed.
 fn queue_mass_recomputation_on_collider_mass_change(
     mut commands: Commands,
+    explicit: Query<(), ExplicitMassProperties>,
+    changed_properties: Query<(), MassPropertyChanged>,
+    mut queued: Local<bevy::ecs::entity::EntityHashSet>,
     mut query: Query<
-        &ColliderOf,
+        (Entity, &ColliderOf),
         Or<(
             Changed<ColliderMassProperties>,
             Changed<ColliderTransform>,
@@ -388,7 +405,17 @@
         )>,
     >,
 ) {
-    for &ColliderOf { body } in &mut query {
+    queued.clear();
+    for (collider, &ColliderOf { body }) in &mut query {
+        // A fully explicit body is unaffected by its colliders, except by its own explicit
+        // properties when it is its own collider.
+        if explicit.contains(body) && (collider != body || !changed_properties.contains(body)) {
+            continue;
+        }
+        // Queue each body once, however many of its colliders changed.
+        if !queued.insert(body) {
+            continue;
+        }
         if let Ok(mut entity_commands) = commands.get_entity(body) {
             entity_commands.insert(RecomputeMassProperties);
         }
--- a/src/dynamics/rigid_body/mass_properties/system_param.rs
+++ b/src/dynamics/rigid_body/mass_properties/system_param.rs
@@ -12,6 +12,7 @@
 /// [mass properties]: crate::dynamics::rigid_body::mass_properties
 #[derive(SystemParam)]
 pub struct MassPropertyHelper<'w, 's> {
+    pub(super) explicit: Query<'w, 's, (), super::ExplicitMassProperties>,
     query: Query<
         'w,
         's,
@@ -58,8 +59,14 @@
     /// This takes into account the mass properties of descendants, unless the given entity has the [`NoAutoMass`],
     /// [`NoAutoAngularInertia`], or [`NoAutoCenterOfMass`] marker components.
     pub fn update_mass_properties(&mut self, entity: Entity) {
-        // Compute the total mass properties of the entity and its descendants.
-        let mut mass_props = self.total_mass_properties(entity);
+        // Every output of a fully explicit entity is replaced by its explicit value below, so
+        // aggregating its descendants would be discarded work.
+        let mut mass_props = if self.explicit.contains(entity) {
+            MassProperties::ZERO
+        } else {
+            // Compute the total mass properties of the entity and its descendants.
+            self.total_mass_properties(entity)
+        };
 
         let Ok((
             mut computed_mass,
```

## Notes

A body that is its own collider still queues its own explicit edits (`collider == body` with a
changed `Mass`, `AngularInertia` or `CenterOfMass`). Partially explicit and automatic bodies
aggregate exactly as before.
