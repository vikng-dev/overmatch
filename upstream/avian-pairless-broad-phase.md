# Broad phase traverses the trees for proxies whose collision layers can match nothing

Found in a game using avian3d. Local draft; not filed.

**Crate:** avian3d 0.8.0 (crates.io). **Status:** not sent. Source audit: Unchanged on `main` at 2c2d6934 (2026-11-04): MEASURED, `main`'s `src/` differs from 0.8.0 only in `dynamics/ccd/mod.rs` (#1101), and this diff applies to `main` with `patch --dry-run`.

## Defect

`collect_collision_pairs` (`src/collision/broad_phase/bvh_broad_phase.rs`) traverses every tree for
each moved proxy, then rejects each candidate with `CollisionLayers::interacts_with`. That test is
`(self.memberships & other.filters) != NONE && (other.memberships & self.filters) != NONE`
(`src/collision/collider/layers.rs:424-427`), so a proxy with empty memberships or empty filters
can form no pair: its traversals find candidates and reject all of them. Such proxies are common
for colliders that exist only for spatial queries.

The system also takes `ColliderTrees` as `ResMut` and never writes it, which serializes it against
every reader of the trees.

## Minimal repro

A collider with `CollisionLayers::new(LayerMask::ALL, LayerMask::NONE)` moving through K overlapping
colliders visits K tree leaves per step and produces no pair. In a constructed case the visits went
from 2 to 0 with the fix, with pairs unchanged (MEASURED, found in a game using avian3d).

## Fix

Against the pristine crate (`a/` and `b/` are the crate root):

```diff
--- a/src/collision/broad_phase/bvh_broad_phase.rs
+++ b/src/collision/broad_phase/bvh_broad_phase.rs
@@ -49,7 +49,7 @@
 }
 
 fn collect_collision_pairs<H: CollisionHooks>(
-    trees: ResMut<ColliderTrees>,
+    trees: Res<ColliderTrees>,
     moved_proxies: Res<MovedProxies>,
     hooks: StaticSystemParam<H>,
     par_commands: ParallelCommands,
@@ -83,6 +83,15 @@
                     let tree = trees.tree_for_type(proxy_type1);
                     let proxy1 = tree.get_proxy(proxy_key1.id()).unwrap();
 
+                    // With no memberships or no filters, `CollisionLayers::interacts_with` is
+                    // false for every partner, so this proxy's traversals cannot produce a pair.
+                    // It stays in its tree for spatial queries.
+                    if proxy1.layers.memberships == LayerMask::NONE
+                        || proxy1.layers.filters == LayerMask::NONE
+                    {
+                        continue;
+                    }
+
                     let Some(proxy_aabb1) = tree.get_proxy_aabb(proxy_id1) else {
                         continue;
                     };
```

## Notes

The proxy stays in its tree, so spatial queries still find it. The early `continue` happens before
the traversal, so it changes no pair: both endpoints' traversals of a pair already apply the same
layer test.
