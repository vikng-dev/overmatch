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

Local upstream draft: `upstream/avian-unknown-contact-feature-identity.md` (DRAFT, not filed).

## Nearest warm-start contact (`src/collision/contact_types/mod.rs`, `src/collision/narrow_phase/system_param.rs`)

The matcher selects the closest admissible anchors across all prior manifolds of the pair.
`src/collision/narrow_phase/system_param.rs` supplies all prior points in one iterator, so a later
manifold cannot overwrite an earlier closer match. Flattening allocates no point buffer.
DERIVED work is O(current points × prior points), the published matcher's worst-case bound,
with a full candidate scan replacing early exit at the first match.

Pair-wide guard: `src/tests/avian_warm_start.rs::nearest_warm_start_is_selected_across_old_manifolds`.

Local upstream draft: `upstream/avian-nearest-warm-start-contact.md` (DRAFT, not filed).

## Physics work counters (`src/dynamics/solver/`, `src/collision/`, `src/utils.rs`)

Contact passes add serial and parallel constraint visits to `SolverDiagnostics`, summed over
substeps and restitution. Overflow is serial; visits include zero-restitution no-ops.
`par_for_each` returns its existing dispatch decision, including the single-worker serial case;
its thresholds, iteration and chunking are unchanged. No counter feeds a physics decision.

Narrow phase counts active non-touching pairs whose owners are static, sleeping or have exactly
zero linear and angular velocity, then counts their admitted geometry queries. Standalone
colliders are static; a missing body-query result is unknown and excluded. Reusable thread-local
cells hold the tallies, reduced after the pair jobs finish. Body reads precede bounds admission
so rejected pairs are counted too; solver state and geometry inputs are unchanged.

The existing per-step diagnostics reset clears these counters. `src/net/ram.rs` exports them
alongside stage timers and a collector-only contact/color/dynamic-degree census through
`src/cost.rs`. The tracked reader is `scripts/perf/collapse.py`, which saves the nested `avian`
account in `tick-attribution.json` without adding its timers again to tick parts. The census
partitions immutable contact and constraint reads across compute-worker batches, reuses their
local incidence maps, and combines the integer counts after those batches complete.

Guards: `src/net/ram/avian_tests.rs`.

## Complete collider batches and explicit mass ownership

Subject: the game's server physics and construction collapse, game state only.

`src/collision/collider/collider_hierarchy/plugin.rs` keeps an existing `ColliderOf` on hierarchy
changes when it already names the nearest body. A complete spawn bundle can therefore supply the attachment
alongside its shape and enter the body's collider tree directly. Hierarchy-driven attachment
and rehoming still work when ownership differs. `AncestorMarker` is re-exported from `lib.rs`
so a prepared group can mark its collider ancestry at spawn, avoiding a deferred marker write
and subtree walk for each member of a batch.

The game's group constructor inserts the body, including an empty `RigidBodyColliders`, before
adding any collider. This order is required for direct batch attachment. A collider that precedes
its body's `RigidBody` is initially unlisted and standalone; `Insert<RigidBody>` retains upstream's
unconditional attachment re-insert to repair its membership and tree, even when `ColliderOf`
already names that body. The group path has no colliders to repair at body insertion.
Avian's relationship hook then appends each collider directly. Without that collection, a batch
would queue competing single-collider lists. Bevy 0.19.1's `spawn_batch` and `insert_batch` keep
per-entity hooks and observers; the game batches the occupied sensor and contact archetypes in
one structural command. Bevy still queues each `ChildOf` collection update, and the BVH still
needs one leaf insertion per collider. No hook, shape initialization or leaf is bypassed.

`src/dynamics/rigid_body/mass_properties/` recognizes fully explicit ownership only when `Mass`,
`AngularInertia`, `CenterOfMass` and all three corresponding `NoAuto*` markers exist. Membership
changes do not recompute those outputs, and their update never aggregates descendants whose
contributions would all be replaced. Collider changes queue one request per other owner.
Self-attached colliders still queue their owner's explicit property edits; partially explicit
and automatic bodies retain descendant aggregation. Initial body computation and the normal
prepare-phase update remain in place.

Guards: `src/tests/avian_spawn_batch.rs` and
`construction::breakage::tests::departure_and_split_preserve_each_chunks_collider_contract`.

## Pairless-layer broad-phase prune (`src/collision/broad_phase/bvh_broad_phase.rs`)

`collect_collision_pairs` skips a moved proxy's tree traversals when its collision-layer
memberships or filters are empty: either empty term makes `CollisionLayers::interacts_with`
false for every partner. Each proxy stays in exactly one collider tree, preserving spatial queries.
Eligibility reads the current proxy layers each step, including sensor and standalone proxies.
Tree access is read-only; missing live proxies or their bounds are invariant failures.

`CollisionDiagnostics::broad_phase_candidates` counts non-self BVH leaf visits before duplicate,
layer and body filtering, reduced from worker-local counters. The existing tick trace exposes it
under `avian`, for the ram and scheduled-collapse instruments.

Guard: `src/tests/avian_broad_phase_layers.rs`. Counts and reproduction:
`upstream/avian-pairless-broad-phase.md` (local report).

## Stationary non-touching pair cache

`ContactPair` retains a transient boxed query/pruning snapshot only after a built-in collider
pair produces no manifolds before contact hooks. Unchanged stationary misses skip geometry;
touching pairs and hook-rejected contacts always query. Custom backends remain uncached because
their `Context` may supply additional inputs. Cache hits allocate nothing.

The snapshot covers collider world poses, shape change ticks, owners, query/pruning margins,
relative linear and angular velocities, collider offsets, rotated COMs
and step length. Prediction distance is the sum of the two recorded margins. Sleep state only
controls eligibility: a wake alone cannot change the pure geometry result. Separate anchor
inputs preserve the existing pruning expression's arithmetic.
Bounds, layers and constraint eligibility are processed before reuse. New and deserialized pairs
have no snapshot; pair removal drops it. Broad-phase discovery and wake scheduling are unchanged.
Changing `PhysicsLengthUnit` refreshes the cached global margins alongside configuration changes.

Guards: `src/net/ram/avian_tests.rs`, including wake-and-move, independent pruning inputs,
either collider, child rehoming, new neighbors, same-step motion and touching geometry refresh. The sweep's hook-only rejection remains a
per-step decision, including the unresolved-target path fixed in `27562bd8`.
Count evidence, same-input geometry comparison and reproduction: `upstream/avian-still-pairs.md`.
