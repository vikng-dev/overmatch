# Draft: removing `ColliderOf` orphans the collider's standalone proxy

**LOCAL DRAFT — not filed.** Correctness and performance finding against `avian3d 0.7.0` with
Bevy 0.19.1, read in the published source.

## Source and consequence

`src/collider_tree/update.rs`, the observer on `Remove<ColliderOf>` ("Case 3": move to the
standalone tree if `C` still exists) queries `&ColliderTreeProxyKey`, removes the proxy from its
tree, adds a new proxy to `standalone_tree` and inserts the new key into `MovedProxies`, but never
writes the new key to the collider's `ColliderTreeProxyKey`. The component keeps naming the old
tree and id.

- On despawn, when this observer runs before `remove_from_tree_on::<Remove, C, …>` ("Case 2";
  the order follows component registration), Case 2 removes the old id from the old tree, which no
  longer holds it, and the standalone proxy stays, naming a dead entity. The broad phase pairs
  standalone proxies with every tree, so it pairs with everything its frozen bounds overlap,
  forever; the narrow phase walks those pairs every step.
- On a live removal (the body's `RigidBody` removed), a later removal of the collider frees the
  old id in the old tree, which by then may hold another collider's proxy.

## Evidence

Overmatch's dedicated server despawns and respawns its map (static bodies with child colliders)
between rounds. After one rebuild the contact graph held 46 087 active pairs on a map of static
bodies alone, against 0 before it, and physics p50 rose 348 to 594 us per step (MEASURED,
2026-11-01, M4). With the change below, the same run holds 0 pairs after the rebuild and physics
p50 is 232 us (MEASURED).

## Proposed upstream change

Query `&mut ColliderTreeProxyKey` in Case 3 and assign `*proxy_key = new_proxy_key` after adding
the standalone proxy. Overmatch carries exactly that (`vendor/avian3d-0.7.0-child-pose`,
`OVERMATCH_PATCH.md`), guarded by `src/tests/avian_collider_of_removal.rs`.

## Our workaround and its removal condition

The vendored patch above. Remove it with the vendored crate once upstream writes the key back.

## What fixing this unlocks for us

Nothing beyond the workaround: the patch is the whole fix.
