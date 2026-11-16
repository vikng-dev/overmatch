# Expose child collider pose placement for application scheduling

**Crate:** avian3d 0.8.0. **Status:** not sent. Local draft; no external action taken.

`ColliderTransformPlugin` registers `update_child_collider_position` in
`PhysicsStepSystems::First`, but the function is crate-private and its module is private. An
application that owns child placement cannot remove that system through Bevy 0.20.1's
`App::remove_systems_in_set` while retaining the plugin's ancestry and transform propagation.

Make the system public and re-export it beside `ColliderTransformPlugin`. Its implementation,
registration and default behavior stay unchanged. This is the access seam proposed by PR #896;
source audit recorded that PR open, without a local filing.

## Reproduction

Install `PhysicsPlugins`, then try to remove the child placement system by its typed system set.
With 0.8.0 this cannot compile from another crate. With the export it can; setup should require
one removed system and install the replacement only after removal. Bevy's removal initializes the
schedule, so replacing it before all plugins have built can initialize an incomplete composition.

## Proposed patch

Against pristine 0.8.0:

```diff
--- a/src/collision/collider/collider_transform/mod.rs
+++ b/src/collision/collider/collider_transform/mod.rs
@@ -2,7 +2,7 @@
 
 mod plugin;
 
-pub use plugin::ColliderTransformPlugin;
+pub use plugin::{ColliderTransformPlugin, update_child_collider_position};
 
 use crate::prelude::*;
 use bevy::prelude::*;
--- a/src/collision/collider/collider_transform/plugin.rs
+++ b/src/collision/collider/collider_transform/plugin.rs
@@ -65,7 +65,8 @@
 }
 
+/// Updates child collider poses from their bodies.
 #[allow(clippy::type_complexity)]
-pub(crate) fn update_child_collider_position(
+pub fn update_child_collider_position(
     mut collider_query: Query<
         (
             &ColliderTransform,
```

The unconditional pose writes are a separate change-detection defect. The corresponding
[Lightyear draft](lightyear-avian-child-collider-pose-churn.md) documents `set_if_neq`; exposing
this Avian system does not change that behavior. A changed-only writer also remains free to
visit only children whose inputs changed.
