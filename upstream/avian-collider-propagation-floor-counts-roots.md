# Collider propagation's parallel floor counts roots rather than hierarchy work

**Crate:** avian3d 0.8.0. **Status:** not sent. Local draft; no external action taken.

## Defect

`propagate_collider_transforms` dispatches a query of hierarchy roots through
`ParallelQueryForEach` with `MIN_PAR_ITER_ENTITIES` (4,096). Each query item recursively walks a
whole hierarchy, so root count is not the number of transforms updated. A small set of roots can
hold thousands of collider descendants and still take the serial path. The helper's floor was
chosen for cheap per-entity work, as its own documentation states.

## Evidence

Historical MEASURED comparison on a scheduled construction-collapse workload, Bevy 0.20.1 /
Avian 0.8.0, debug server, compute pool 10, one run per path: `physics_prepare` (the phase
containing propagation) took 436.8 µs wall / 443.7 µs CPU per tick serial and 176.6 µs wall /
621.5 µs CPU dispatched. This measures the containing phase, not an isolated propagation timer.
The corresponding 0.7 stack measured 184.8 µs wall / 637.9 µs CPU.

A separate historical MEASURED comparison on Bevy 0.19 / Avian 0.7, house-collapse pile, phase A,
median of 10 runs, compute pools 2 versus 1: both containing phase segments were level or faster
with 2. The hosted two-core allocation uses 2 compute workers. The current-release comparison
above used 10 workers; there is no new two-worker timing claim here. The local fork record owns
the original commands and provenance.

## Proposed patch

Restore direct dispatch of disjoint root hierarchies. Upstream's forest invariant already proves
the recursive mutable accesses from different roots cannot overlap. This changes dispatch only;
no result depends on which worker walks a root. A workload-aware floor could be considered
separately, but root count alone does not estimate this walk's cost.

Against pristine 0.8.0:

```diff
--- a/src/collision/collider/collider_transform/plugin.rs
+++ b/src/collision/collider/collider_transform/plugin.rs
@@ -2,7 +2,6 @@
     ancestor_marker::{AncestorMarker, AncestorMarkerPlugin},
     physics_transform::PhysicsTransformSystems,
     prelude::*,
-    utils::{MIN_PAR_ITER_ENTITIES, ParallelQueryForEach},
 };
 use bevy::{
     ecs::{intern::Interned, schedule::ScheduleLabel},
@@ -119,8 +118,8 @@
     >,
     parent_query: Query<(Entity, Ref<Transform>, Has<RigidBody>, Ref<ChildOf>), ShouldPropagate>,
 ) {
-    root_query.par_for_each_mut(
-        MIN_PAR_ITER_ENTITIES,
+    // A root owns a whole hierarchy; a per-entity floor counts roots, not the work walked.
+    root_query.par_iter_mut().for_each(
         |(entity, transform, children)| {
             for (child, child_transform, is_child_rb, child_of) in parent_query.iter_many(children).matched() {
                 assert_eq!(
```

## Local verification

The game retains its child-pose invariants and runs its physics guards with this dispatch. No
dispatch-only assertion is needed, and no new timing measurement is used for this proposal.
