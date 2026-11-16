# Child collider pose update marks every child collider changed every frame

Found in a game using lightyear_avian3d. Local draft; not filed.

**Crate:** lightyear_avian3d 0.31.0 (crates.io). **Status:** not sent. Source audit: Unchanged on lightyear `main` at 736e1695 (2026-11-05), where the crate now lives under `crates/integration/avian`; this diff applies there with `patch --dry-run` (MEASURED).

## Defect

`LightyearAvianPlugin::update_child_collider_position` (`src/plugin.rs`, in `RunFixedMainLoop`
`AfterFixedMainLoop`, Position mode) assigns every child collider's `Position` and `Rotation` through
`Mut` every frame, changed or not, so every child collider of every body, static ones included, is
`Changed<Position>` every frame. Avian's copy has the same unconditional writes. The separate
[Avian access proposal](avian-child-collider-pose-churn.md) exposes that system for application
replacement; this draft changes only the published Lightyear writer's change detection.

Avian's moved-collider AABB pass then recomputes every child collider's AABB on the next physics
step, since the writes are newer than its `LastPhysicsTick`.

## Minimal repro

A static body with N child colliders, nothing moving: after each frame, N children report
`Changed<Position>` (DERIVED). Found in a game using lightyear_avian3d: with this copy and Avian's
writing only changed poses, physics step p50 fell from 904 to 618 µs on a world of ~13.8k static map
entities (MEASURED on lightyear_avian3d 0.30.1 and avian3d 0.7.0, same writes).

## Fix

Against the pristine crate (`a/` and `b/` are the crate root):

```diff
--- a/src/plugin.rs
+++ b/src/plugin.rs
@@ -924,14 +924,19 @@
                 continue;
             };
 
-            position.0 = rb_pos.0 + (rb_rot * collider_transform.translation).real();
+            // Write only a pose that changed: this runs every frame over every child collider, and
+            // an unconditional write marks each one changed whether or not it moved, so every
+            // `Changed<Position>`/`Changed<Rotation>` consumer treats it as moved.
+            position.set_if_neq(Position(
+                rb_pos.0 + (rb_rot * collider_transform.translation).real(),
+            ));
             #[cfg(feature = "2d")]
             {
-                *rotation = *rb_rot * Rotation::from(collider_transform.rotation);
+                rotation.set_if_neq(*rb_rot * Rotation::from(collider_transform.rotation));
             }
             #[cfg(feature = "3d")]
             {
-                *rotation = (rb_rot.0 * collider_transform.rotation).normalize().into();
+                rotation.set_if_neq((rb_rot.0 * collider_transform.rotation).normalize().into());
             }
         }
     }
```

## Notes

`set_if_neq` compares with `PartialEq`. An application that needs bit-exact stored poses can
compare each coordinate's `to_bits` before writing (including signed zero and NaN payloads).
The pass still visits every child collider every frame; visiting only children whose inputs
changed is a separate change.


## Local disposition

The application removes this public system during plugin cleanup and installs its own changed-input
writer. It uses the published crate, so this proposal would retire zero local fork lines.
