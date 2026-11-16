# Contact matching takes the first admissible previous point, and later manifolds overwrite it

Found in a game using avian3d. Local draft; not filed.

**Crate:** avian3d 0.8.0 (crates.io). **Status:** not sent. Source audit: Unchanged on `main` at 2c2d6934 (2026-11-04): MEASURED, `main`'s `src/` differs from 0.8.0 only in `dynamics/ccd/mod.rs` (#1101), and this diff applies to `main` with `patch --dry-run`.

## Defect

`ContactManifold::match_contacts` copies the warm start of the first admissible previous point and
breaks, though a later point may be much closer. The narrow phase
(`src/collision/narrow_phase/system_param.rs:897-901`) then calls it once per previous manifold of
the pair, so a match from a later previous manifold overwrites one from an earlier manifold without
any distance comparison: the result depends on manifold order.

This draft also contains the unknown-feature identity fix
([its own draft](avian-unknown-contact-feature-identity.md)), since it rewrites the same loop.

## Minimal repro

```rust
#[test]
fn nearest_admissible_previous_point_wins() {
    let near = Vector::X * 0.08;
    let mut previous_near = ContactPoint::new(near, near, RVector::ZERO, 0.0);
    previous_near.warm_start_normal_impulse = 5.0;
    let mut previous_here = ContactPoint::new(Vector::ZERO, Vector::ZERO, RVector::ZERO, 0.0);
    previous_here.warm_start_normal_impulse = 11.0;

    let point = ContactPoint::new(Vector::ZERO, Vector::ZERO, RVector::ZERO, 0.0);
    let mut manifold = ContactManifold::new([point], Vector::Y);
    manifold.match_contacts(&[previous_near, previous_here], 0.1);

    // First-admissible copies 5.0, from the point 0.08 away.
    assert_eq!(manifold.points[0].warm_start_normal_impulse, 11.0);
}
```

DERIVED for 0.8.0, not compiled here; the same fixture copied 5 instead of 11 on 0.7.0 after the
identity fix (MEASURED, found in a game using avian3d). Across manifolds: give a pair two previous
manifolds, the closer point in the first; the second call overwrites it.

## Fix

Against the pristine crate (`a/` and `b/` are the crate root):

```diff
--- a/src/collision/contact_types/mod.rs
+++ b/src/collision/contact_types/mod.rs
@@ -428,51 +428,61 @@
 
     /// Copies impulses from previous contacts to matching contacts in `self`.
     ///
-    /// Contacts are first matched based on their [feature IDs](PackedFeatureId), and if they are unknown,
-    /// matching is done based on contact positions using the given `distance_threshold`
-    /// for determining if points are too far away from each other to be considered matching.
+    /// Each contact takes the impulses of the closest admissible previous contact, or none. A
+    /// contact whose [feature IDs](PackedFeatureId) are both known admits a previous contact with
+    /// the same IDs, in either order. A contact with an unknown feature ID admits a previous
+    /// contact whose anchors both lie within `distance_threshold` of its own, in either
+    /// orientation. Closest is the smaller sum of the two anchors' squared distances; a tie goes
+    /// to the earlier previous contact.
     #[inline]
     pub fn match_contacts(&mut self, previous_contacts: &[ContactPoint], distance_threshold: f32) {
+        self.match_contacts_in(previous_contacts.iter(), distance_threshold);
+    }
+
+    /// [`match_contacts`](Self::match_contacts) over previous contacts drawn from any number of
+    /// manifolds, so the closest match across all of them wins.
+    #[inline]
+    pub fn match_contacts_in<'a>(
+        &mut self,
+        previous_contacts: impl Iterator<Item = &'a ContactPoint> + Clone,
+        distance_threshold: f32,
+    ) {
         // The squared maximum distance for two contact points to be considered matching.
         let distance_threshold_squared = distance_threshold.powi(2);
 
         for contact in self.points.iter_mut() {
-            for previous_contact in previous_contacts.iter() {
-                // If the feature IDs match, copy the contact impulses over for warm starting.
-                if (contact.feature_id1 == previous_contact.feature_id1
-                    && contact.feature_id2 == previous_contact.feature_id2) ||
-                    // we have to check both directions because the entities are sorted in order
-                    // of aabb.min.x, which could have changed even the two objects in contact are the same
-                    (contact.feature_id2 == previous_contact.feature_id1
-                    && contact.feature_id1 == previous_contact.feature_id2)
-                {
-                    contact.warm_start_normal_impulse = previous_contact.warm_start_normal_impulse;
-                    contact.warm_start_tangent_impulse =
-                        previous_contact.warm_start_tangent_impulse;
-                    break;
-                }
+            // Two `UNKNOWN` IDs compare equal without naming the same feature.
+            let unknown_features = contact.feature_id1 == PackedFeatureId::UNKNOWN
+                || contact.feature_id2 == PackedFeatureId::UNKNOWN;
 
-                let unknown_features = contact.feature_id1 == PackedFeatureId::UNKNOWN
-                    || contact.feature_id2 == PackedFeatureId::UNKNOWN;
-
-                // If the feature IDs are unknown and the contact positions match closely enough,
-                // copy the contact impulses over for warm starting.
-                if unknown_features
-                    && (contact.anchor1.distance_squared(previous_contact.anchor1)
-                        < distance_threshold_squared
-                        && contact.anchor2.distance_squared(previous_contact.anchor2)
-                            < distance_threshold_squared)
-                    || (contact.anchor1.distance_squared(previous_contact.anchor2)
-                        < distance_threshold_squared
-                        && contact.anchor2.distance_squared(previous_contact.anchor1)
-                            < distance_threshold_squared)
-                {
-                    contact.warm_start_normal_impulse = previous_contact.warm_start_normal_impulse;
-                    contact.warm_start_tangent_impulse =
-                        previous_contact.warm_start_tangent_impulse;
-                    break;
+            let mut closest: Option<(f32, &ContactPoint)> = None;
+            for previous_contact in previous_contacts.clone() {
+                // Both directions, because the pair's order can change between steps.
+                let same_features = !unknown_features
+                    && ((contact.feature_id1 == previous_contact.feature_id1
+                        && contact.feature_id2 == previous_contact.feature_id2)
+                        || (contact.feature_id2 == previous_contact.feature_id1
+                            && contact.feature_id1 == previous_contact.feature_id2));
+                for (anchor1, anchor2) in [
+                    (previous_contact.anchor1, previous_contact.anchor2),
+                    (previous_contact.anchor2, previous_contact.anchor1),
+                ] {
+                    let near1 = contact.anchor1.distance_squared(anchor1);
+                    let near2 = contact.anchor2.distance_squared(anchor2);
+                    let admissible = same_features
+                        || (unknown_features
+                            && near1 < distance_threshold_squared
+                            && near2 < distance_threshold_squared);
+                    if admissible && closest.is_none_or(|(distance, _)| near1 + near2 < distance) {
+                        closest = Some((near1 + near2, previous_contact));
+                    }
                 }
             }
+
+            if let Some((_, previous_contact)) = closest {
+                contact.warm_start_normal_impulse = previous_contact.warm_start_normal_impulse;
+                contact.warm_start_tangent_impulse = previous_contact.warm_start_tangent_impulse;
+            }
         }
     }
 
--- a/src/collision/narrow_phase/system_param.rs
+++ b/src/collision/narrow_phase/system_param.rs
@@ -895,10 +895,13 @@
                     // TODO: Cache this?
                     let distance_threshold = 0.1 * self.length_unit.0;
 
+                    // Match against all of the pair's previous points at once, so a later
+                    // previous manifold cannot overwrite a closer match from an earlier one.
                     for manifold in contacts.manifolds.iter_mut() {
-                        for previous_manifold in old_manifolds.iter() {
-                            manifold.match_contacts(&previous_manifold.points, distance_threshold);
-                        }
+                        manifold.match_contacts_in(
+                            old_manifolds.iter().flat_map(|previous| previous.points.iter()),
+                            distance_threshold,
+                        );
                     }
                 }
```

## Notes

Closest is the smaller sum of both anchors' squared distances over the admissible orientation; a tie
keeps the earlier previous point. `match_contacts` keeps its signature and delegates to
`match_contacts_in`, which takes the previous points of every old manifold as one iterator, so the
caller allocates nothing.

## Local verification

The combined identity and nearest-match fix passes the pinned 0.8.0 headless guards for positional
admissibility, cold starts without a predecessor, and nearest points within and across manifolds
(MEASURED). The assertions are unchanged; direct unit calls use the engine matcher.
