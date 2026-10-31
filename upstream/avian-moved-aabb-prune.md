# Draft: the moved-collider AABB pass fetches every collider every step

**LOCAL DRAFT — not filed.** Performance finding against `avian3d 0.7.0` with Bevy 0.19.1, read in
the published source. Upstream already names it: the TODO at `src/collider_tree/update.rs:888`
("par-iter over all colliders, check if they have actually changed since the `LastPhysicsTick`").

## Source and consequence

`update_moved_collider_aabbs` (`src/collider_tree/update.rs`, `ColliderTreeSystems::UpdateAabbs`,
every physics step) runs a parallel iteration over every enabled collider, fetching its entity,
pose, shape, `ColliderAabb`, `EnlargedAabb`, margin and proxy key, and only then skips the collider
if none of its `Position`, `Rotation` and shape ticks is newer than `LastPhysicsTick`. A world of
standing static colliders pays the fetch for every one of them every step.

## Evidence

Overmatch's dedicated server, 30 tanks on its product map (~13.8k map entities, MEASURED): the
pass and its parallel batches cost 2391 µs per tick traced (DERIVED, 1753 + 638 MEASURED
2026-10-31, M4, `bevy/trace_chrome`) before the child-pose fix of
[avian-child-collider-pose-churn.md](avian-child-collider-pose-churn.md), when every static child
collider was also recomputed. With that fix they cost 236 µs per tick traced (DERIVED, 99 + 137
MEASURED the same day, 30 bots and no client): the fetch of colliders that did not move. The change
below is priced separately.

## Proposed upstream change

Add the change filter to the query: `Or<(Changed<Position>, Changed<Rotation>, Changed<C>)>`, so a
collider whose ticks did not move since the system's previous run costs three tick reads and is
never fetched; keep the `LastPhysicsTick` check in the body. This is exact while
`LastPhysicsTick` is newer than the system's previous run, since every tick newer than it is then
newer than that run too. `update_last_physics_tick` runs after the system in every step, so inside
the physics schedule that always holds. For an instance the application schedules elsewhere, fall
back to the unfiltered query when it does not (`SystemChangeTick::last_run` against
`LastPhysicsTick`).

The prune changes nothing the pass writes, so only a count can test it: a counter of examined
colliders in `ColliderTreeDiagnostics` (`moved_aabb_candidates` in our patch), with a test that a
step in which nothing moved examines no collider, and one that a static collider moved, rescaled,
added or removed is found where a spatial query looks for it after one step.

## Workaround

Overmatch vendors the crate with this patch beside the child-pose one:
`vendor/avian3d-0.7.0-child-pose` (`OVERMATCH_PATCH.md`), guarded by
`src/tests/avian_moved_aabb_prune.rs`. Removal condition: upstream filters the pass on change ticks.

## What fixing this unlocks for us

Drop the vendored crate, together with the child-pose record's fix. Nothing else.
