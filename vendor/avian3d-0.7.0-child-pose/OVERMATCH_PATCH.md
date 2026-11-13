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
in the removal hook. Waking does not change configured rest thresholds, time to sleep or whole-island eligibility.

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

## Body-pair contact reduction (`src/collision/contact_types/reduction.rs`)

Two bodies touching through many collider pairs (a broken piece's chunk hulls along a seam) carry
one manifold per hull pair. `NarrowPhase::reduce_body_pair_contacts` runs after the pair update and
before status bits are read: it groups the touching pairs of each two bodies and clusters their
contacts by support normal; a cluster admits normals within about 20 degrees of its seed, so its
members can differ by about 40; across unequal normals the hull argument is an approximation. Normal
forces along one direction hold any load whose line falls inside the projected convex hull of their
points, so each normal keeps the vertices of the hull of the points that bear, less those within
`HULL_TOLERANCE` of the segment between their kept neighbours: every bearing point lies within that
tolerance of the kept outline. It keeps the hull of the points that carried load last step the same
way, so a loaded point leaves only when other loaded points enclose it. Without that second hull a
point up to `BEARING_GAP` open became a vertex, made a loaded corner interior and removed it, and
the body dropped onto the newcomer (DERIVED: 2 mm of free fall ends at 0.2 m/s, above the 0.15 m/s
rest speed; MEASURED on the local ram recipe, 35 to 200 loaded vertices a second left that way while
its pile settled). A point bears when its
separation, less what its closing speed covers in the physics step, is within `BEARING_GAP`, or when
it carried load last step; a point dropped as not bearing can overlap by the end of the step only by
what the step's change in relative velocity adds beyond that margin, and bears on the next step.
Points inside the hull are what is removed; there is no point ceiling. MEASURED: also keeping each
normal's deepest point, as an overlap corrector, kept the one-house collapse awake three to eight
times longer in each of six runs, its piles sliding and breaking for seconds after the event.

A group is left whole for the step when any point closes faster than the slower body's
`SleepThreshold` (a landing, every press of it kept), when it spans more than `MAX_NORMALS`, or when
a point it would remove overlaps deeper than the deepest kept point of its normal by more than the
overlap slop: the kept corners would not correct that overlap. The slop is
`NarrowPhaseConfig::contact_tolerance`, the separation inside which the narrow phase reports a
contact, borrowed as a length; the solver itself tolerates no overlap, so a point reinstated past the
slop is pushed out from a cold start (MEASURED: 0 to 6 groups in a ram run of 7,000 to 35,000). A landing pair's
blows can still move, since another pair sharing either body may be reduced and the solve couples
them. A removed point's share of the solved impulses goes to the points that remain.

Only pairs between two dynamic `ReducibleContacts` bodies qualify; a static side and a
contact-modification hook each exclude the pair at eligibility (MEASURED on twelve collapsing
houses: reducing piece-terrain contacts doubled awake piece-ticks, the piles failing to settle;
piece-piece alone did not). Cluster seeds and every tie follow the pairs' colliders, then each
pair's manifolds by their least features, then the points' features, never depth or active-pair
order; where features are unknown or repeat, ties fall to the narrow phase's generated order,
deterministic for the same geometry but not tied to the contact.

Each kept point stays in its own manifold and collider pair, so its per-collider attribution is
unchanged. Warm starting is not continuous: matching runs against only the previous reduced set, so
a removed point loses its impulse and one reselected later starts cold unless the matcher finds it
another predecessor. A loaded hull vertex leaves only at half of `HULL_TOLERANCE`, which makes
reselection rarer without ruling it out. `CollisionDiagnostics` carries the groups offered, those
left whole for each reason, and two warm-start selection counters: points holding a matched warm
start that reduction removed, and points kept with none.

A pair left with no manifold stays touching (events, `CollidingEntities`, island link), with no
constraint handle: hence the two patched status-change sites in `narrow_phase/system_param.rs` (the
started-touching debug assertion and the stopped-touching island unlink, which now also fires for a
pair with an island and no handle; island splitting in `dynamics/solver/islands/mod.rs` likewise
walks contacts with an island link rather than constraint handles). The stationary-miss cache is
unaffected: it only stores pairs with no manifold from the geometry query, and reduction runs only
on pairs with manifolds after that query.

Guards: `src/tests/avian_contact_reduction.rs`.

## Rest energy (`src/dynamics/solver/islands/sleeping.rs`, `src/dynamics/solver/solver_body/mod.rs`, `src/dynamics/rigid_body/sleeping.rs`)

Subject: the game's rigid-body rest, server physics and scenery, game state only.

A body rests when either the engine's original separate linear/angular velocity test passes or
`v² + ω·I_world·ω/m < (SleepThreshold::linear * PhysicsLengthUnit)²`: the original test misreads a
slender body's roll, the energy test does not, and the engine's own test stays because it is more
permissive for large bodies. Both comparisons remain strict and preserve threshold signs.
The original angular setting, default, reflection and serialization remain available.
`TimeToSleep` and whole-island eligibility are unchanged.

The energy expression is twice kinetic energy per mass; its half cancels against the rest-speed
energy. Computed inertia is rotated to world space. Physical inertia is used because locks mask
force increments while prescribed velocities still move in `integrate_positions`. Zero/infinite
mass and singular/infinite inertia follow the engine's zero-reciprocal convention.

Body-owned timers update on the compute pool; island wake bits and the split candidate commit
afterward. Equal split timers choose the lower island id.

Guards: `src/tests/avian_island_sleep.rs` (spin axis, independent local-frame energy calculation,
degenerate properties, original-test rest, transverse see-saw, supported periodic roll).
Counts and scope: `.agents/scratch/rest-energy.md`.

## Moved-list removal by index (`src/collider_tree/tree.rs`, `src/collider_tree/update.rs`, `src/collider_tree/optimization.rs`)

Avian keeps two lists of the proxies moved since the last physics step: each tree's
`moved_proxies`, which tree optimization reinserts, and the `MovedProxies` resource, whose order
the broad phase makes its pairs in. Stock 0.7.0 took a proxy out of either by a scan from the
front: `ColliderTree::remove_proxy` scanned the tree's whole list, also to learn that the proxy
was not in it, and `MovedProxies::remove` scanned its vector for a key its set held. Avian's own
observers call both: `add_to_tree_on` on every `ColliderOf` insert (a collider moved to another
body leaves the tree and joins it again) and `remove_from_tree_on` on every `ColliderDisabled`.
Both lists grow through a tick with every proxy added, so a tick that rehomes N colliders compares
on the order of N² entries, counted over every construction breaking in that tick together.

MEASURED (scheduled-collapse recipes, the tick after the break, construction phase): one house,
1,924 colliders rehomed and 962 disabled and re-enabled, compared 4.26 M tree-list entries and
1.70 M `MovedProxies` entries; six houses, 11,544 and 5,772, compared 109.7 M and 40.0 M
(149.6 M together, 25 times one house's for six times the colliders). The patch compares none:
one slot read per tree removal (2,886 and 17,316) and one map lookup per `MovedProxies` removal
(976 and 7,424).

Why the game cannot avoid the scans: they run inside Avian's observers, between the game's
`ColliderOf` insert and its return, with no setting that skips or batches them. Moving a collider
to another body requires that insert, and dropping its stale contact pairs requires the disable
(`src/collider_rehome.rs`). The lists cannot be emptied first: the next step's broad phase and
optimization read them.

The patch gives each list an index. `ColliderTree::moved_proxies` is private beside `moved_slots`
(by proxy index, the proxy's position in the list); every write goes through `mark_moved`,
`remove_proxy`, `take_moved`, `clear_moved` and `set_moved`, and readers take
`moved_proxies()`. `MovedProxies` keeps each key's position in a map where it kept a set. A
removal `swap_remove`s the recorded position and re-records the entry that took its place.

That leaves the lists stock Avian leaves, entry for entry, because a list never holds a proxy
twice: the position the scan found is the only one. For `MovedProxies` its own `insert` refuses a
held key. For a tree's list: `add_proxy` lists a new id, and an id is reused only after
`remove_proxy` unlisted it; `update_tree` lists a proxy only when `MovedProxies::insert` accepted
its key, and a listed proxy's key is held, since every site that removes a key removes the proxy
from its tree first and the two are cleared, and restored by lightyear, together. Stock Avian
relied on this silently (a second entry would have outlived its proxy's removal); `mark_moved`
panics on it.

Guard: `src/tests/avian_moved_lists.rs` (both lists equal the scanning form after every step of
a tick of 2,000 rehomes and 1,000 toggles, and of 20,000 generated adds, removals, marks and
clears with reused ids; a second mark is refused).

## Sleeping bodies' velocity increments (`src/dynamics/integrator/mod.rs`, `src/dynamics/rigid_body/forces/plugin.rs`)

Subject: the game's rigid-body rest, server physics, game state only.

The world-frame velocity increments (gravity, `ConstantForce`, `ConstantLinearAcceleration`,
`ConstantAngularAcceleration`) are built only for bodies with a `SolverBody`, the set
`clear_velocity_increments` clears. A sleeping body has no solver body but keeps its
`VelocityIntegrationData`. As published, every step added gravity to it and scaled it by the
substep without clearing it, so it held `g·h/(1 − h)` and its first step awake gained an extra
`N·g·h²/(1 − h)` (DERIVED; `h` the substep length in s, `N` the substeps). MEASURED on the shipped
tank's wake tick: 22.8 N·s of downward momentum its contact account did not hold, 0.26 % of the
step's weight impulse. A force applied to a sleeping body through the waking `Forces` call now
waits in the increment for the body's first step awake, where stock Avian scaled it away.
The patched force adapters keep upstream's serial loops: the patch changes only their filter.

Guard: `src/tests/avian_island_sleep.rs` (`a_woken_body_takes_only_its_first_steps_gravity`).
Local upstream draft: `upstream/avian-sleeping-velocity-increments.md`.
