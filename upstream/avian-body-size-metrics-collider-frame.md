# Rotated off-origin colliders can underestimate a body's sweep radius

Subject: the multiplayer tank game's server physics; game state only.

**Target:** published avian3d 0.8.0, `update_body_size_metrics`.
**Status:** LOCAL DRAFT, not sent; published source checked from the local cache, without web access.

`ColliderTransform` maps a collider-local point into body space as rotation times point plus
translation. `ComputedCenterOfMass` is body-local. `max_distance_to_point_with_context` measures
against an AABB at local identity, so its input must be collider-local. Subtracting translation
alone mixes the two frames; inverse rotation is also required. The collider's shape already
carries its scale.

DERIVED: for a shape with local bounding centre `c`, its body-space centre is `t + R*c`.
The old query instead measures `|COM - t - c|`; the correct distance is
`|COM - t - R*c| = |R.inverse()*(COM-t) - c|`. Their difference can reach `2*|c|`.
The half-diagonal bound added by the shape's distance query is unchanged. CCD eligibility and
contact recycling consume this radius.

The game-side guard in `src/tests/avian_body_size_metrics.rs`,
`a_rotated_off_origin_collider_bounds_its_distance_from_the_body_com`, constructs a cube with an
off-origin bounding centre and tests both an unrotated and a half-turned child. The bound must
contain every transformed fixture corner. The half-turn case fails without inverse rotation;
the unrotated case retains its existing result.
The game's shipped turret collider has its local bounding centre at the origin, and its track
and construction chunk colliders have identity rotation relative to their bodies; those shapes
are unaffected.

Apply independently at the published crate root with `patch -p1`:

```diff
--- a/src/dynamics/rigid_body/body_size_metrics.rs
+++ b/src/dynamics/rigid_body/body_size_metrics.rs
@@ -115,7 +115,9 @@

             // Compute the sweep radius
             let ctx = ColliderContext::new(entity, &context);
-            let point = com.0 - collider_transform.translation;
+            // The distance query takes a collider-local point; the COM is body-local.
+            let point =
+                collider_transform.rotation.inverse() * (com.0 - collider_transform.translation);
             let distance_to_com = collider.max_distance_to_point_with_context(point.real(), ctx);
             sweep_radius = sweep_radius.max(distance_to_com);
         }
```

MEASURED: the independent patch applied to the cached published crate and its offline library
check passed through `scripts/serial-cargo`:

```sh
scripts/serial-cargo check --offline --lib --manifest-path \
  /tmp/swept-ccd-drafts/radius/check-harness/Cargo.toml \
  --target-dir target/draft-check-ccd --features dev_ui,bitprobe
```

The commands and red/green witness are recorded in
[the collapse diagnosis](../.agents/scratch/swept-ccd-collapse-diagnosis.md). The draft
uses the published manifest unchanged through an offline path-dependency harness with
`default-features = false`, `features = ["3d", "parry-f32"]`.

Retire this bug-fix patch when a published release transforms the COM into the collider frame
before measuring its sweep radius. It is independent of per-collider CCD selection.
