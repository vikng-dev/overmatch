# Waking an island resets the sleep timers of bodies that were already awake

Found in a game using avian3d. Local draft; not filed.

**Crate:** avian3d 0.8.0 (crates.io). **Status:** not sent. Source audit: Unchanged on `main` at 2c2d6934 (2026-11-04): MEASURED, `main`'s `src/` differs from 0.8.0 only in `dynamics/ccd/mod.rs` (#1101), and this diff applies to `main` with `patch --dry-run`.

## Defect

`merge_islands` keeps a merged island marked sleeping when the smaller island was sleeping
(`src/dynamics/solver/islands/mod.rs:1025-1030`), so an island can be marked sleeping while some of
its bodies are awake and accumulating rest time. Waking it walks every linked body and sets
`sleep_timer.0 = 0.0` (`WakeIslands`, `src/dynamics/solver/islands/sleeping.rs`), so an awake body
that touches a sleeping island loses its rest history and needs a full `TimeToSleep` again; bodies
that keep touching sleepers may never sleep.

## Minimal repro

1. Body A awake and at rest, its `SleepTimer` at 0.4 s with `TimeToSleep` 0.5 s; body B asleep.
2. `PhysicsIslands::merge_islands` on their islands, then apply `WakeIslands` to the result.
3. Expected: A keeps 0.4 s, B restarts at 0. Actual: both 0. The same sequence took an awake body's
   timer from 0.0625 s to 0 on 0.7.0 (MEASURED, found in a game using avian3d); DERIVED for 0.8.0,
   whose walk is unchanged.

## Fix

Against the pristine crate (`a/` and `b/` are the crate root):

```diff
--- a/src/dynamics/solver/islands/mod.rs
+++ b/src/dynamics/solver/islands/mod.rs
@@ -1378,6 +1378,8 @@
     on_remove = BodyIslandNode::on_remove,
     clone_behavior = Custom(clone_placeholder_body_island_node),
 )]
+// Enable observers can create a node before its first solver index.
+#[require(crate::dynamics::rigid_body::sleeping::SleepTimer)]
 pub struct BodyIslandNode(IslandNode<Entity>);

 // Avoid incorrect prev/next links when cloning a rigid body
--- a/src/dynamics/solver/islands/sleeping.rs
+++ b/src/dynamics/solver/islands/sleeping.rs
@@ -121,6 +121,12 @@
         return;
     }

+    // This body has lost Sleeping before the queued wake, which resets only sleeping members.
+    // BodyIslandNode requires its timer even before a solver index.
+    world
+        .get_mut::<SleepTimer>(ctx.entity)
+        .expect("every BodyIslandNode retains its required SleepTimer for waking")
+        .0 = 0.0;
     world.commands().queue_silenced(WakeBody(ctx.entity));
 }

@@ -457,6 +463,7 @@
             &'static BodyIslandNode,
             &'static mut SleepTimer,
             Option<&'static RigidBodyColliders>,
+            Has<Sleeping>,
         )>,
         SResMut<PhysicsIslands>,
         SResMut<ContactGraph>,
@@ -514,11 +521,9 @@
                     let mut body = island.head_body;

                     while let Some(entity) = body {
-                        let Ok((body_island, mut sleep_timer, colliders)) = bodies.get_mut(entity)
-                        else {
-                            body = None;
-                            continue;
-                        };
+                        let (body_island, mut sleep_timer, colliders, was_sleeping) = bodies
+                            .get_mut(entity)
+                            .expect("every sleeping island link resolves to a live BodyIslandNode with SleepTimer; otherwise waking would leave later bodies asleep");

                         // Transfer the contact pairs to the awake set, and add touching contacts to the constraint graph.
                         if let Some(colliders) = colliders {
@@ -539,7 +544,10 @@

                         bodies_to_wake.push(entity);
                         body = body_island.next;
-                        sleep_timer.0 = 0.0;
+                        // Awake members of a merged island retain their rest history.
+                        if was_sleeping {
+                            sleep_timer.0 = 0.0;
+                        }
                     }
                 }
             }
```

## Notes

The walk resets only bodies that were sleeping (`Has<Sleeping>`). A body whose `Sleeping` is
removed directly has lost the marker by the time its queued `WakeBody` runs, so the removal hook
resets that body's timer itself. The solver still resets a timer when the body exceeds a threshold.

`BodyIslandNode` requires `SleepTimer` in the engine, including nodes created by enable
observers before their first solver index. The requirement and the wake tripwire are one fix.
Removing a required timer afterwards violates the node invariant and panics at the wake boundary.
One lifecycle behavior differs from pristine: a body enabled while asleep since spawn now wakes;
the enable observer can fetch its newly required timer and queue the wake. The local lifecycle
test explicitly asserts this behavior.

## Local verification

MEASURED: the final patch applies to a fresh cached published 0.8.0 crate and its library compiles
offline with all default features through `scripts/serial-cargo check`, using the path-dependency
harness described in [the draft index](README.md#avian-draft-compile-checks). The sleep command's
query remains its original three-element tuple; only the wake command adds `Has<Sleeping>`.
