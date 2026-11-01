# Overmatch avian3d patch

This directory is the published `avian3d` 0.7.0 crate (extracted from the `.crate` whose sha256
matches `Cargo.lock`) with the source changes below, tagged `OVERMATCH PATCH` so the diff against
pristine 0.7.0 is self-describing. (`Cargo.lock` from the published crate is omitted; it is not read
for a path dependency.)

## Child-collider pose writes (`src/collision/collider/collider_transform/plugin.rs`)

`update_child_collider_position` (`PhysicsStepSystems::First`, every physics step) assigned every
child collider's `Position` and `Rotation` through `Mut`, whether or not the value changed. Every
child collider of every rigid body, static bodies included, was therefore marked changed every
step, and `update_moved_collider_aabbs` (`src/collider_tree/update.rs`), which skips a collider
whose `Position`, `Rotation` and shape are unchanged since the last physics tick, recomputed every
one of their AABBs every step. On the product map that is every building element and tree segment,
none of which moves.

The patch computes the same pose with the same expressions and assigns `Position` and `Rotation`
only where the new value differs in bits (`f32::to_bits` per component, so `-0.0` against `0.0`
and NaN payloads count as different). The stored values are bit-identical to the unpatched
system's; only the change ticks of unchanged values differ.

Who reads those change ticks: in this crate, `update_moved_collider_aabbs` (above, a pure function
of pose and shape: skipping it for an unchanged pose leaves the same AABB), `wake_on_changed`
(rigid bodies only; the query excludes `RigidBody`) and `position_to_transform` (rigid bodies and
`ApplyPosToTransform` only). `lightyear_avian3d` delegates its corresponding pass to this parameter
(`vendor/lightyear_avian3d-0.30.1-child-pose`), so both boundaries keep the same write semantics.

`ChildColliderPoses` is the exported system parameter for the same placement law at all of the
application's boundaries: Avian before narrow phase, the game's walk after physics, and
lightyear's frame replication. It reads changed local poses, attachments and own poses, plus the
collider lists of bodies whose pose, kind or collider membership changed. Unchanged inputs cost
change-tick scans but no pose composition. Removing a default disabling component refreshes
membership even without a pose write. A manually attached collider follows `ColliderOf` through
`RigidBodyColliders`, without requiring a Transform `Children` component on its body.

Guard: `src/tests/avian_child_pose_churn.rs`; the production walk boundary is also exercised by
`src/ballistics/motion.rs` tests. The current inspection is
`.agents/scratch/perf-2026-11-01/server-tick.md`; the earlier defect and local upstream draft are
`upstream/avian-child-collider-pose-churn.md`.

## Moved-collider bounds prune (`src/collider_tree/update.rs`, `src/collider_tree/diagnostics.rs`)

`update_moved_collider_aabbs` (`ColliderTreeSystems::UpdateAabbs`, every physics step) fetched every
collider's pose, shape, bounds and proxy key in a parallel iteration, then skipped those whose
`Position`, `Rotation` and shape ticks were not newer than `LastPhysicsTick`. On the product map
that is thousands of standing static colliders fetched every step for nothing.

The patch adds a third query to the system's `ParamSet`: the same data, filtered
`Or<(Changed<Position>, Changed<Rotation>, Changed<C>)>`, so a collider whose ticks did not move
since the system's previous run costs the filter's three tick reads and is never fetched. The
per-collider body, its `LastPhysicsTick` check included, is unchanged and runs for every collider
the filter passes. That is exact while `LastPhysicsTick` is newer than the system's previous run:
every tick newer than `LastPhysicsTick` is then newer than that run too, so the filter passes every
collider the check accepts. `update_last_physics_tick` runs after this system in every step, so
inside the physics schedule it always holds; any other instance (Overmatch runs the system in
`trees::derive_systems` and `buildings::breakage::refit_systems`) falls back to the unfiltered
query whenever it does not.

`ColliderTreeDiagnostics` gains `moved_aabb_candidates`, the colliders the pass examined this step
(reset every step with the rest of the diagnostics), counted per batch and added once per batch. It
is the one observable of the prune: everything else the pass writes is the same with or without it.

Guard: `src/tests/avian_moved_aabb_prune.rs` (standing static colliders are not examined; a static
collider moved, rescaled, added or removed reaches the collider trees). Record:
`upstream/avian-moved-aabb-prune.md`.

## Standalone key write-back (`src/collider_tree/update.rs`)

The observer on `Remove<ColliderOf>` (Case 3 in that file's comments) moves a collider that keeps
its `C` from its body's tree to the standalone tree, but read the collider's `ColliderTreeProxyKey`
immutably and never wrote the new key back. The key kept naming the old tree and id, so the
collider's later `Remove<C>` (Case 2) removed that id from the old tree, which no longer held it,
and the standalone proxy outlived its collider. An orphan proxy's bounds stay where the collider
was, and the broad phase pairs standalone proxies with everything, so every orphan pairs with
every collider it overlaps, for the rest of the process. On a despawn, the two observers run in
component registration order, and on Overmatch's server the despawned colliders leaked: its map
rebuild between Battles left 46 087 contact pairs on a map of static bodies, which holds none
without a standalone proxy (`.agents/scratch/perf-2026-11-01/server-tick.md`). With the stale key,
a live collider's later removal also frees whatever proxy holds that id by then.

The patch queries the key `&mut` and assigns the standalone key after adding the proxy. Nothing
reads the key's change tick.

Guard: `src/tests/avian_collider_of_removal.rs` (a collider that left its body takes its proxy
when despawned; a static compound rebuilt in place holds no contact pair). Record:
`upstream/avian-collider-of-removal-orphan-proxy.md`.
