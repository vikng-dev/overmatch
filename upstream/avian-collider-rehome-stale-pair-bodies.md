# avian3d 0.7: a collider moved onto another rigid body keeps its contact pairs naming the old body

**Target:** github.com/avianphysics/avian · avian3d 0.7.0; `main` checked against a local clone, no fetch (pair bodies set once at the broad phase, `ColliderOf` immutable: unchanged) · **Severity for us:**
HIGH before the workaround: the dedicated server panicked inside Avian's island manager about a
minute into a 29-bot match with wall debris (overmatch #193) · **Status:** LOCAL DRAFT, not filed.

## Suggested title

Contact pairs keep their bodies when a collider's `ColliderOf` changes, which corrupts the island
lists

## Summary

The broad phase records a pair's two bodies when it makes the pair, and nothing rewrites them. When
a collider moves onto another rigid body (its subtree is detached and given its own `RigidBody`),
`ColliderOf` changes but every pair the collider already holds goes on naming the old body. The
solver applies those pairs' impulses to the old body, and the island manager links them into the
old body's island, where the walks that later unlink a body's pairs never look. The island is split
or removed under the pair, and a later unlink panics: "Previous contact has no island", "Island
IslandId(n) does not exist", or, with debug assertions, `island.contact_count > 0` and
`prev_contact_island.next == Some(contact_id)`.

## Mechanism (avian3d 0.7.0)

- `collision/broad_phase/bvh_broad_phase.rs:178,190`: a new pair takes `body1`/`body2` from the
  tree proxies. No system writes them again.
- `collision/collider/collider_hierarchy/plugin.rs:66-116` (`on_collider_body_changed`): a
  `RigidBody` inserted mid-hierarchy re-inserts `ColliderOf` on the colliders below it. Nothing
  touches their pairs. The narrow phase reads each collider's new `ColliderOf` for the contact
  computation (`collision/narrow_phase/system_param.rs:523-528`) but keeps the pair's stored bodies.
- `dynamics/solver/plugin.rs:404` (`prepare_contact_constraints`): the constraint's bodies are the
  pair's stored ones.
- `dynamics/solver/islands/mod.rs:509-526` (`add_contact`): a pair that starts touching is linked
  into the island of its stored bodies, the old body's.
- The walks that find a body's pairs use the colliders it holds now (`RigidBodyColliders`):
  `remove_body_on` (`collision/narrow_phase/mod.rs:465-500`, on a body's despawn or type switch)
  and `split_island` (`dynamics/solver/islands/mod.rs:1120`). Neither finds the moved collider's
  pair from the old body. `split_island` destroys the base island first (`:1070`), so the pair is
  left holding the destroyed island's id. `BodyIslandNode::on_remove` (island removal at `:1374-1376`) removes the
  old body's island when its last body goes, with the pair still counted.
- The island id is then reused (`dynamics/solver/islands/mod.rs:463-465`), and the next unlink of the pair reads another island's lists.
- One path hides the defect. When the subtree's root is made a body in place, `on_insert_rigid_body`
  (`dynamics/solver/solver_body/plugin.rs`) queues the `SolverBody` that brings `BodyIslandNode`,
  and `on_collider_body_changed` queues the new `ColliderOf`s. If the colliders join first,
  `on_body_remove_rigid_body_disabled` (`collision/narrow_phase/mod.rs`, on `Add<BodyIslandNode>`)
  drops their pairs as a side effect and the broad phase remakes them correctly. If the island
  node comes first, the new body holds no colliders yet and the stale pairs survive. Both
  observers answer `Insert<RigidBody>`, so which runs first is observer order, not a contract.

## Evidence

MEASURED on overmatch's dedicated server (dev build, 29 bots and no client, the debris stress
lever at 50 pieces every 2 s, temporary probes in the vendored crate, not committed):

- A cooked-off tank's turret becomes its own dynamic body. Its colliders' pairs with a static body
  kept naming the hull: four such pairs at the first knockout wave, about 49 s in.
- About 3 s later three of them started touching and were linked into their hulls' islands. One
  island split then left a pair pointing at the destroyed island.
- The server panicked the same second. Every run from overmatch `dc010e6e` on panicked at
  53-58 s (7 runs); every run before it reached 150 s (alpha.4, `8389fbe2`, alpha.5, `bc11ed15`,
  `240cbcf5`, 39-48 knockouts each). `dc010e6e` changes no physics: it drops client-only Bevy
  plugins from the server. That it flips the observer order above is ASSUMED, not measured: Bevy
  0.19.1 keeps a component's observers in an `EntityHashMap` keyed by observer entity
  (`bevy_ecs/src/observer/centralized_storage.rs:148`), and fewer plugins renumber those entities.
- With the workaround below the same runs show no stale pair and no panic.

## Minimal reproduction

Pinned in this repository as an inverted test,
`src/collider_rehome.rs::tests::avian_still_names_a_rehomed_colliders_former_body`. It passes
while the defect stands. The collider moves onto a body that already exists, which keeps the
observer-order side effect above out of the path. In outline (default plugins, islands on, one
64 Hz step per update):

```rust
let ground = world.spawn((RigidBody::Static, Position(Vec3::new(0.0, -0.5, 0.0)), Collider::cuboid(40.0, 1.0, 40.0))).id();
let hull = world.spawn((RigidBody::Dynamic, Transform::from_xyz(0.0, 0.5, 0.0), Position(Vec3::new(0.0, 0.5, 0.0)))).id();
world.spawn((ChildOf(hull), Transform::IDENTITY, Collider::cuboid(2.0, 1.0, 2.0)));
let node = world.spawn((ChildOf(hull), Transform::from_xyz(3.0, 0.0, 0.0))).id();
let boxed = world.spawn((ChildOf(node), Transform::IDENTITY, Collider::cuboid(1.0, 1.0, 1.0))).id();
for _ in 0..8 { app.update(); }                  // boxed rests on the ground; the pair names hull
let turret = world.spawn((Position(/* node's world pose */), Rotation(..), RigidBody::Dynamic)).id();
world.entity_mut(boxed).insert(ChildOf(turret)); // ColliderOf { body: turret }
app.update(); app.update();                     // the pair still names hull
world.entity_mut(hull).despawn(); app.update(); // debug: islands/mod.rs:1372 contact_count == 0
```

In a release build the hull's island is removed with the pair still counted, and the turret's
despawn then unlinks the pair from an island that no longer exists.

## Suggested fix

When a collider's `ColliderOf` changes to another body, remove its pairs the way
`remove_collider_on` does (unlink from the constraint graph and the island, send `CollisionEnd` for pairs with contact events on)
and mark its proxy moved, so the broad phase makes them again under the new body. Alternatively,
detect `ColliderOf.body != pair.body` in `update_contacts` and route the pair through the same
removal. Rewriting the stored bodies in place would also need the pair's constraint handles and
island link moved, which the removal path already does correctly.

## Our workaround and its removal condition

`src/collider_rehome.rs`: an `On<Insert, ColliderOf>` observer. When any of the collider's pairs
names another body for it, it inserts and removes `ColliderDisabled` in one command batch, so
Avian's own removal runs and the re-enabled proxy is marked moved. A collider whose pairs already
name its body is left alone. `a_rehomed_colliders_pairs_name_its_new_body` pins it.

Remove the observer when `avian_still_names_a_rehomed_colliders_former_body` fails: that failure
means Avian rewrites the pair itself.

## What fixing this unlocks for us

Deleting one observer and its tests. Nothing else waits on it.
