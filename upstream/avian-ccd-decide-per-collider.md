# Decide swept CCD eligibility per collider

Subject: rigid-body collision in a multiplayer tank video game; game state only.

**Target:** published avian3d 0.8.0. **Status:** LOCAL DRAFT, not filed; upstream head unchecked.
Source checked against the cached published crate, without web access.

A compound body's thinnest child currently selects every supported child for a sweep. This makes
an unrelated attachment's thickness decide how a thick hull meets a wall. Keep the body gate as
an early-out, then apply the same threshold to each collider's own CCD thickness before mass
properties, swept bounds or tree traversal. Low-risk children receive ordinary discrete contact.

Reuse the existing conservative body-wide motion bound: translation plus angular motion about
the body's centre of mass. A child's own radius about its origin would underestimate an offset
child's rotation. For a single-collider body the existing body gate and this gate use the same
thickness, so eligibility is unchanged. No thresholds, defaults or contact laws change.

The sweep-shape initializer uses a local `LazyCell`: centroid/mass properties are computed only
when a candidate passes the self, non-solid, layer, touching-contact and target-query filters
and reaches TOI dispatch, once per moving collider. The thickness already required by eligibility
is reused. This three-line lazy-construction patch changes no collision result and is folded into
this draft; it does not need a separate upstream proposal.

The game's three small collision witnesses cover a thick child's discrete first step beside a
thin child, thin-child and single-collider wall protection, and a stationary-COM offset rotation.
They assert geometric relationships, not earlier measured positions.

Apply with `patch -p1` at the published crate root; no other fork patch is required:

```diff
--- a/src/dynamics/ccd/mod.rs
+++ b/src/dynamics/ccd/mod.rs
@@ -741,6 +741,13 @@
                 continue;
             }

+            // A thin child must not force thicker children to sweep. The body-wide bound
+            // includes offset rotation and preserves eligibility for a single collider.
+            let min_extent = shape1.ccd_thickness();
+            if max_motion <= ccd.threshold * min_extent.f32() {
+                continue;
+            }
+
             let layers1 = layers1.copied().unwrap_or_default();

             let motion1 =
@@ -760,15 +767,15 @@
             );
             let query_aabb = obvhs::aabb::Aabb::from(swept_aabb);

-            let fast_shape = CcdFastShape {
+            let fast_shape = core::cell::LazyCell::new(|| CcdFastShape {
                 shape: shape1,
                 swept_aabb: parry::bounding_volume::Aabb::new(
                     swept_aabb.min.real(),
                     swept_aabb.max.real(),
                 ),
                 local_centroid: shape1.mass_properties(1.0).local_com,
-                min_extent: shape1.ccd_thickness(),
-            };
+                min_extent,
+            });

             // Sweep the collider against the relevant trees.
             for tree in trees_to_query.into_iter().flatten() {
```

MEASURED: the patch applied cleanly to the cached published 0.8.0 source. A sibling harness
with `default-features = false`, `features = ["3d", "parry-f32"]` and `pub use avian3d;`
compiled offline using the published manifest unchanged:

```sh
scripts/serial-cargo check --offline --lib --manifest-path \
  /private/tmp/ccd4-drafts/per-collider/check-harness/Cargo.toml \
  --target-dir target/draft-check-ccd --features dev_ui,bitprobe
```

The cache archive SHA-256 is
`ca7479ccec5f1440675946bada8c805f2b6d514af78e1d2b0937ca53b1e2c543`.
Mutant results are recorded in the continuing lane report
[here](../.agents/scratch/swept-ccd-decisions-and-price.md).
