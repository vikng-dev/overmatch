# Contact matching treats two unknown feature IDs as the same feature

Found in a game using avian3d. Local draft; not filed.

**Crate:** avian3d 0.8.0 (crates.io). **Status:** not sent. Source audit: Unchanged on `main` at 2c2d6934 (2026-11-04): MEASURED, `main`'s `src/` differs from 0.8.0 only in `dynamics/ccd/mod.rs` (#1101), and this diff applies to `main` with `patch --dry-run`.

## Defect

`ContactManifold::match_contacts` (`src/collision/contact_types/mod.rs:435-476`) has two defects:

1. Its feature test accepts `contact.feature_id1 == previous.feature_id1 && …` when all of them are
   `PackedFeatureId::UNKNOWN` (`0`), which is how `ContactPoint::new` initializes them. A point with
   unknown features therefore copies the warm start of the first previous point that also has
   unknown features, at any distance, and the positional branch documented for unknown features
   never runs.
2. The positional condition parses as `(unknown && direct) || swapped`, so the swapped-orientation
   distance test applies to known-feature points too, matching points whose known IDs differ.

## Minimal repro

In the module's scope:

```rust
#[test]
fn unknown_feature_ids_match_by_position() {
    let far = Vector::X * 10.0;
    let mut previous_far = ContactPoint::new(far, far, RVector::ZERO, 0.0);
    previous_far.warm_start_normal_impulse = 5.0;
    let mut previous_here = ContactPoint::new(Vector::ZERO, Vector::ZERO, RVector::ZERO, 0.0);
    previous_here.warm_start_normal_impulse = 11.0;

    let point = ContactPoint::new(Vector::ZERO, Vector::ZERO, RVector::ZERO, 0.0);
    let mut manifold = ContactManifold::new([point], Vector::Y);
    manifold.match_contacts(&[previous_far, previous_here], 0.1);

    // 0.8.0 copies 5.0 from the point 10 units away.
    assert_eq!(manifold.points[0].warm_start_normal_impulse, 11.0);
}
```

The same case copied 5 instead of 11 on 0.7.0, whose matcher is the same (MEASURED, found in a game
using avian3d); this test is DERIVED for 0.8.0, not compiled here.

## Fix

Against the pristine crate (`a/` and `b/` are the crate root):

```diff
--- a/src/collision/contact_types/mod.rs
+++ b/src/collision/contact_types/mod.rs
@@ -437,14 +437,20 @@
         let distance_threshold_squared = distance_threshold.powi(2);
 
         for contact in self.points.iter_mut() {
+            // A point with an unknown feature ID has no identity: two `UNKNOWN` IDs compare equal
+            // without naming the same feature, so such a point is matched by position only.
+            let unknown_features = contact.feature_id1 == PackedFeatureId::UNKNOWN
+                || contact.feature_id2 == PackedFeatureId::UNKNOWN;
+
             for previous_contact in previous_contacts.iter() {
                 // If the feature IDs match, copy the contact impulses over for warm starting.
-                if (contact.feature_id1 == previous_contact.feature_id1
+                if !unknown_features
+                    && ((contact.feature_id1 == previous_contact.feature_id1
                     && contact.feature_id2 == previous_contact.feature_id2) ||
                     // we have to check both directions because the entities are sorted in order
                     // of aabb.min.x, which could have changed even the two objects in contact are the same
                     (contact.feature_id2 == previous_contact.feature_id1
-                    && contact.feature_id1 == previous_contact.feature_id2)
+                    && contact.feature_id1 == previous_contact.feature_id2))
                 {
                     contact.warm_start_normal_impulse = previous_contact.warm_start_normal_impulse;
                     contact.warm_start_tangent_impulse =
@@ -452,20 +458,17 @@
                     break;
                 }
 
-                let unknown_features = contact.feature_id1 == PackedFeatureId::UNKNOWN
-                    || contact.feature_id2 == PackedFeatureId::UNKNOWN;
-
                 // If the feature IDs are unknown and the contact positions match closely enough,
-                // copy the contact impulses over for warm starting.
+                // in either orientation, copy the contact impulses over for warm starting.
                 if unknown_features
-                    && (contact.anchor1.distance_squared(previous_contact.anchor1)
+                    && ((contact.anchor1.distance_squared(previous_contact.anchor1)
                         < distance_threshold_squared
                         && contact.anchor2.distance_squared(previous_contact.anchor2)
                             < distance_threshold_squared)
-                    || (contact.anchor1.distance_squared(previous_contact.anchor2)
-                        < distance_threshold_squared
-                        && contact.anchor2.distance_squared(previous_contact.anchor1)
-                            < distance_threshold_squared)
+                        || (contact.anchor1.distance_squared(previous_contact.anchor2)
+                            < distance_threshold_squared
+                            && contact.anchor2.distance_squared(previous_contact.anchor1)
+                                < distance_threshold_squared))
                 {
                     contact.warm_start_normal_impulse = previous_contact.warm_start_normal_impulse;
                     contact.warm_start_tangent_impulse =
```

## Notes

A point with an unknown ID matches by position only, in either orientation; a point with known IDs
matches by identity only. [The nearest-match draft](avian-nearest-warm-start-contact.md)
contains this fix; this is the minimal version.

## Local verification

The combined identity and nearest-match fix passes the pinned 0.8.0 headless guards for positional
admissibility, cold starts without a predecessor, and nearest points within and across manifolds
(MEASURED). The assertions are unchanged; direct unit calls use the engine matcher.
