# Overmatch avian3d patch

This directory is the published `avian3d` 0.7.0 crate (extracted from the `.crate` whose sha256
matches `Cargo.lock`) with the source changes inventoried below.
(`Cargo.lock` from the published crate is omitted; it is not read
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
change-tick scans but no pose composition. A manually attached collider follows `ColliderOf` through
`RigidBodyColliders`, without requiring a Transform `Children` component on its body.

Re-enabling an entity (removing a disabling component of `DefaultQueryFilters`) changes membership
without a pose write. Bevy clears its removal messages every `App::update` (`World::clear_trackers`),
so a boundary in the fixed schedule that read them lost a re-enable made in the frames between
steps. `ReenabledPlugin`, added by `ColliderTransformPlugin`, writes the buffered message
`Reenabled` from an observer on each disabling component's removal; a `Message` lives until a fixed
step has run, and each boundary reads it through its own cursor, placing the re-enabled child, or
the re-enabled body's attached colliders. Disabling components registered by later plugins are
watched from `finish`; one registered after that is refused by every boundary
(`WatchedDisabling::assert_covers`). The game's collider history (`src/ballistics/motion.rs`) reads
the same message.

Guard: `src/tests/avian_child_pose_churn.rs`; the production walk boundary is also exercised by
`src/ballistics/motion.rs` tests. The current inspection is
`.agents/scratch/perf-2026-11-01/server-tick.md`; the earlier defect and local upstream draft are
`upstream/avian-child-collider-pose-churn.md`.

## Attachment refusal (`src/collision/collider/collider_hierarchy/mod.rs`)

`ColliderOf::on_insert` returned without listing the collider in its body's `RigidBodyColliders`
when the collider or the body had no `GlobalTransform`. Every child placement boundary reaches a
moved body's colliders through that list, so such a child stayed at its birth pose. Avian's
`PhysicsTransformPlugin` requires `Transform` through `Position`; Overmatch's networked composition
disables that plugin (`src/net/physics.rs`), so there a body spawned without `Transform` took this
path silently. The patch panics, naming both entities and the fix.

Guard: `src/tests/avian_child_pose_churn.rs`
(`a_collider_attached_to_a_body_without_a_global_transform_is_refused`).

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
`construction::breakage::refit_systems`) falls back to the unfiltered
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

## Island contact waking and rest history (`src/collision/narrow_phase/system_param.rs`, `src/dynamics/solver/islands/sleeping.rs`)

A sleeping island's new contact supplies a wake only through an awake non-static participant.
Dormant new contacts enter the sleeping pair set and leave no active manifold handles. Wakes
resolve a representative body's current island after all contact changes, so a later merge
cannot retire a requested wake's destination.

`WakeIslands` preserves the rest timers of already-awake merge participants. Newly woken bodies
restart their rest interval; explicit removal of `Sleeping` resets its initiating body's timer
in the removal hook. The library's thresholds, time to sleep and whole-island eligibility rule
are unchanged.

Every linked island body retains `BodyIslandNode` and `SleepTimer` until it is unlinked.
`WakeIslands` refuses a failed body query with an invariant panic instead of silently ending
the traversal with a partly woken island.

Guard: `src/tests/avian_island_sleep.rs`; production impact and support-loss guards remain in
`src/debris.rs`. Local upstream draft: `upstream/avian-island-rest-history.md`.

## Prepared collider mass (`src/collision/collider/parry/mod.rs`)

`Collider::with_mass_properties_cache` computes the current scaled shape's mass properties at a
specified density before insertion. `ComputeMassProperties::mass_properties` returns that value
for the same density; shape mutation, replacement and a changed scale invalidate it. The cache
is cloned with the collider and omitted from serialization. Sensors still install zero mass.

The construction prepares the cache on workers for each chunk collider that is no sensor; the
piece body, not its colliders, owns the occupied mass, inertia and centre. Guard:
`construction::breakage::tests::prepared_contact_mass_is_valid_at_insertion_and_invalidates_with_its_inputs`.
Local upstream draft: `upstream/avian-prepared-collider-mass.md`.

## Minimum speculative margin (`src/collision/narrow_phase/system_param.rs`)

`NarrowPhase::update` applies the configured `contact_tolerance` floor to the effective speculative
margin shared by geometry queries and point pruning. Quiet contacts inside that band survive.
Mechanism, evidence and limits: `upstream/avian-minimum-speculative-margin.md` (local draft).
Guards: `stationary_contacts_keep_the_configured_minimum_speculative_margin` in
`src/tests/avian_island_sleep.rs` and `settled_freed_matter_sleeps_within_avians_rest_window` in
`src/construction/breakage/tests.rs`.

## Unknown contact features (`src/collision/contact_types/mod.rs`)

Subject: the game's server physics and environment destruction, game state only.

`ContactManifold::match_contacts` accepted equal `PackedFeatureId::UNKNOWN` values as matching
identities, copying the first previous point's warm start regardless of anchor distance. The
feature-ID path now requires known IDs; unknown points use the existing anchor-distance path,
with both orientations under the same unknown-feature condition. No sleep rule changes.

Guard in the host crate: `src/tests/avian_warm_start.rs`, `unknown_contact_features_match_by_position`.
A fixture's spatially matching point carries 11 N s; the published path instead copied 5 N s
from a point one metre away (MEASURED by the guard before the fix).

Local upstream draft: `upstream/avian-unknown-contact-feature-identity.md` (DRAFT, not filed).

## Nearest warm-start contact (`src/collision/contact_types/mod.rs`, `src/collision/narrow_phase/system_param.rs`)

The matcher selects the closest admissible anchors across all prior manifolds of the pair,
rather than the first point inside the radius. MEASURED by the same guard: at 0.05 m separation,
inside the 0.1 m fixture radius, first-match also copied 5 N s instead of the coincident point's
11 N s. `src/collision/narrow_phase/system_param.rs` supplies all prior points in one iterator,
so a later manifold cannot overwrite an earlier closer match. No point flattening allocation is
added. DERIVED work is O(current points × prior points), the same worst-case search bound as the
published matcher, with a full candidate scan replacing early exit at the first match.

Pair-wide guard: `src/tests/avian_warm_start.rs::nearest_warm_start_is_selected_across_old_manifolds`.

Local upstream draft: `upstream/avian-nearest-warm-start-contact.md` (DRAFT, not filed).
