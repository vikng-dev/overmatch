# Manifold pruning lets separated points displace bearing points

Subject: rigid-body contact selection in a video game's server physics; game state only.

**Target:** published avian3d 0.8.0, `ContactManifold::prune_points` in
`src/collision/contact_types/mod.rs`. **Status:** LOCAL DRAFT, not filed; upstream head unchecked.
Source verified against the cached published crate, without network access.

## Mechanism

The first two selections multiply squared projected distance by squared penetration, floored at
1 mm squared. Squaring a negative penetration rewards separation: a point at -12 mm has
144 times the depth weight of a bearing point at +0.5 mm (DERIVED from the heuristic).
Separated points are legitimate candidates admitted by tolerance or closing speed, but retaining
them at the expense of bearing points removes current support from the solver manifold.

Clamping depth to zero before squaring removes that reward but does not guarantee bearing
priority: the floor ties shallow bearing and separated points, and area selection ignores depth.

## Rule and shape

Penetrating (bearing) points first; speculative (separated) points fill free slots by the same
selection. Zero penetration bears. Clamp depth before squaring.

Move upstream's three loops into `select_four`, restricting every loop to eligible indices and
returning selection order `[point1, point2, point3, point4]`, with `usize::MAX` for empty sides.
Keep upstream's comments, first-wins ties and assertions. Homogeneous manifolds select all points
and construct in upstream polygon order `[point1, point3, point2, point4]`, omitting empty sides.
All-bearing output is unchanged. All-separated output has the touching manifold's anchors and
order; it may keep fewer than four, just as upstream does.

Mixed manifolds visit bearing then separated tiers. A tier that fits is kept whole. An overflowing
tier contributes selection-order picks, then any remaining tier indices in input order, until
capacity is reached. Thus every mixed manifold with more than four points keeps exactly four,
even when a tier's area selection has empty sides. There is no projected-anchor deduplication.
The helper runs only with at least two eligible points: an overflowing tier has more members
than the positive number of free slots. No lone-point fallback is needed.

Selecting only bearing geometry and filling separated slots by input order loses speculative
coverage. Input order is not polygon order: Parry emits face 1's inside vertices, then face 2's,
then edge intersections (`parry3d-0.31.1/src/shape/polygonal_feature3d.rs`,
`contacts_face_face`). Both tiers therefore use the same selection.

The normal solver turns separated points into speculative stops; the game's contact reducer
reads their `normal_speed` after pruning. These justify retaining speculation in spare slots.
Separation monotonicity has its own reason: separation must not be rewarded.

## Relations and witnesses

The game's `src/tests/avian_contact_pruning.rs` uses generated square/edge-midpoint
and pentagonal manifolds, rotated and translated, with all sign assignments and touching,
sub-floor and above-floor depths (Fixture). Its independent published-upstream oracle can select
a subset in selection order. The relations pin contracts, not historical physics outputs:

- R1: all-bearing output equals upstream, including order.
- R2: all-separated anchors and order equal the same manifold with zero penetration.
- R3: mixed output keeps `min(bearing_count, 4)` bearing points.
- R4: mixed input with more than four points keeps exactly four.
- R5: with more than four bearing points in a mixed manifold, no separated point survives and
  every oracle pick on the bearing subset survives, even when the oracle leaves empty sides.
- R6: with fewer than four bearing points and overflowing speculation, the separated output is
  the first `free` selection-order oracle picks on the zero-depth separated subset.
- R7: making a separated point more separated never promotes it from dropped to kept.

`upstream_can_keep_fewer_than_four_bearing_points` also retains the compatibility witness:
collinear all-bearing anchors have no signed area extremes, and a depth-weighted segment on a
convex pentagon's edge can have points on only one side (DERIVED from the selection).
The workspace excludes `vendor/`, so these public-API guards live in the game's test module.

## Apply to published 0.8.0

Apply the following patch at the extracted crate root with `patch -p1`. It changes only pruning;
none of the game's other fork patches is needed.

```diff
--- a/src/collision/contact_types/mod.rs
+++ b/src/collision/contact_types/mod.rs
@@ -478,6 +478,7 @@

     /// Prunes the contact points in the manifold to a maximum of 4 points.
     /// This is done to improve performance and stability.
+    /// Penetrating (bearing) points first; speculative (separated) points fill free slots by the same selection.
     #[inline]
     #[cfg(feature = "3d")]
     pub fn prune_points(&mut self) {
@@ -493,81 +494,135 @@
         // Neither of these should become zero, so we clamp the minimum distance.
         const MIN_DISTANCE_SQUARED: f32 = 1e-6;

+        // OVERMATCH PATCH: upstream's selection over the indices `eligible` admits (at least two),
+        // in selection order [point1, point2, point3, point4]; usize::MAX marks an empty side.
+        fn select_four(
+            projected: &[Vector],
+            penetrations_squared: &[f32],
+            normal: Vector,
+            eligible: impl Fn(usize) -> bool,
+        ) -> [usize; 4] {
+            // Find the point that is farthest from the center of mass, as its torque has the largest influence,
+            // while also taking into account the penetration depth heuristic.
+            let mut point1_index = 0;
+            let mut value = f32::MIN;
+            for (i, point) in projected.iter().enumerate() {
+                if !eligible(i) {
+                    continue;
+                }
+                let v = point.length_squared().max(MIN_DISTANCE_SQUARED) * penetrations_squared[i];
+                if v > value {
+                    value = v;
+                    point1_index = i;
+                }
+            }
+            let point1 = projected[point1_index];
+
+            // Find the point farthest from the first point, forming a line segment.
+            // Also consider the penetration depth heuristic.
+            let mut point2_index = usize::MAX;
+            let mut max_distance = f32::MIN;
+            for (i, point) in projected.iter().enumerate() {
+                if !eligible(i) {
+                    continue;
+                }
+                if i == point1_index {
+                    continue;
+                }
+                let v = point.distance_squared(point1).max(MIN_DISTANCE_SQUARED)
+                    * penetrations_squared[i];
+                if v > max_distance {
+                    max_distance = v;
+                    point2_index = i;
+                }
+            }
+            debug_assert!(point2_index != usize::MAX, "No second point found");
+            let point2 = projected[point2_index];
+
+            // Find the farthest points on both sides of the line segment in order to maximize the area.
+            let mut point3_index = usize::MAX;
+            let mut point4_index = usize::MAX;
+            let mut min_value = 0.0;
+            let mut max_value = 0.0;
+            let perp = (point2 - point1).cross(normal);
+            for (i, point) in projected.iter().enumerate() {
+                if !eligible(i) {
+                    continue;
+                }
+                if i == point1_index || i == point2_index {
+                    continue;
+                }
+                let v = perp.dot(point - point1);
+                if v < min_value {
+                    min_value = v;
+                    point3_index = i;
+                } else if v > max_value {
+                    max_value = v;
+                    point4_index = i;
+                }
+            }
+
+            [point1_index, point2_index, point3_index, point4_index]
+        }
+
         // Project the contact points onto the contact normal and compute the squared penetration depths.
         let (projected, penetrations_squared): (Vec<Vector>, Vec<f32>) = self
             .points
             .iter()
             .map(|point| {
+                let depth = point.penetration.max(0.0);
                 (
                     point.anchor1.reject_from_normalized(self.normal),
-                    (point.penetration * point.penetration).max(MIN_DISTANCE_SQUARED),
+                    (depth * depth).max(MIN_DISTANCE_SQUARED),
                 )
             })
             .unzip();

-        // Find the point that is farthest from the center of mass, as its torque has the largest influence,
-        // while also taking into account the penetration depth heuristic.
-        let mut point1_index = 0;
-        let mut value = f32::MIN;
-        for (i, point) in projected.iter().enumerate() {
-            let v = point.length_squared().max(MIN_DISTANCE_SQUARED) * penetrations_squared[i];
-            if v > value {
-                value = v;
-                point1_index = i;
-            }
-        }
-        let point1 = projected[point1_index];
+        // TODO: The points in the manifold should be in an `ArrayVec`, and the input points should be separate.
+        let points = core::mem::take(&mut self.points);
+        let bearing = |i: usize| points[i].penetration >= 0.0;
+        let bearing_count = (0..points.len()).filter(|&i| bearing(i)).count();

-        // Find the point farthest from the first point, forming a line segment.
-        // Also consider the penetration depth heuristic.
-        let mut point2_index = usize::MAX;
-        let mut max_distance = f32::MIN;
-        for (i, point) in projected.iter().enumerate() {
-            if i == point1_index {
-                continue;
+        // OVERMATCH PATCH: homogeneous manifolds keep upstream's selection and construction order.
+        if bearing_count == 0 || bearing_count == points.len() {
+            let [point1_index, point2_index, point3_index, point4_index] =
+                select_four(&projected, &penetrations_squared, self.normal, |_| true);
+            // Construct the manifold, ensuring the order is correct to form a convex polygon.
+            self.points.push(points[point1_index]);
+            if point3_index != usize::MAX {
+                self.points.push(points[point3_index]);
             }
-            let v =
-                point.distance_squared(point1).max(MIN_DISTANCE_SQUARED) * penetrations_squared[i];
-            if v > max_distance {
-                max_distance = v;
-                point2_index = i;
+            self.points.push(points[point2_index]);
+            if point4_index != usize::MAX {
+                debug_assert_ne!(point3_index, point4_index);
+                self.points.push(points[point4_index]);
             }
+            return;
         }
-        debug_assert!(point2_index != usize::MAX, "No second point found");
-        let point2 = projected[point2_index];

-        // Find the farthest points on both sides of the line segment in order to maximize the area.
-        let mut point3_index = usize::MAX;
-        let mut point4_index = usize::MAX;
-        let mut min_value = 0.0;
-        let mut max_value = 0.0;
-        let perp = (point2 - point1).cross(self.normal);
-        for (i, point) in projected.iter().enumerate() {
-            if i == point1_index || i == point2_index {
-                continue;
+        // OVERMATCH PATCH: mixed manifolds keep bearing points before separated ones. A tier that
+        // fits the free slots is kept whole; a larger tier gives upstream's picks from that tier,
+        // then its other points in input order.
+        for tier in [true, false] {
+            let free = 4 - self.points.len();
+            if free == 0 {
+                break;
             }
-            let v = perp.dot(point - point1);
-            if v < min_value {
-                min_value = v;
-                point3_index = i;
-            } else if v > max_value {
-                max_value = v;
-                point4_index = i;
+            let in_tier = |i: usize| bearing(i) == tier;
+            let picks = if (0..points.len()).filter(|&i| in_tier(i)).count() > free {
+                select_four(&projected, &penetrations_squared, self.normal, in_tier)
+            } else {
+                [usize::MAX; 4]
+            };
+            for i in picks
+                .into_iter()
+                .filter(|&i| i != usize::MAX)
+                .chain((0..points.len()).filter(|&i| in_tier(i) && !picks.contains(&i)))
+                .take(free)
+            {
+                self.points.push(points[i]);
             }
         }
-
-        // Construct the manifold, ensuring the order is correct to form a convex polygon.
-        // TODO: The points in the manifold should be in an `ArrayVec`, and the input points should be separate.
-        let points = core::mem::take(&mut self.points);
-        self.points.push(points[point1_index]);
-        if point3_index != usize::MAX {
-            self.points.push(points[point3_index]);
-        }
-        self.points.push(points[point2_index]);
-        if point4_index != usize::MAX {
-            debug_assert_ne!(point3_index, point4_index);
-            self.points.push(points[point4_index]);
-        }
     }

     /// Returns the contact point with the largest penetration depth.
```

MEASURED: this pruning-only patch applies cleanly to the cached published crate and passes
`cargo check` offline with `3d,parry-f32`, through a path-dependency harness that omits uncached
published dev-dependencies. Compile proof, the harness and mutant results are recorded in
[the lane report](../.agents/scratch/contact-pruning-tiers.md).
Patch classification and measured source counts:
[OVERMATCH_PATCH.md](../vendor/avian3d-0.8.0-overmatch/OVERMATCH_PATCH.md).

Retire this local pruning patch when an upstream release satisfies these relations. Other fork
patches remain independent. No tolerance adjustment or experiment switch is shipped.
